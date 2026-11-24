//! Read-only media plan construction and review; creates no output files.
use super::backend::{probe, version, OP_ATOM};
use super::mono::GainPaths;
use super::receipts::{obj, text};
use super::source::{free_space, id_value, pcm_essence_bytes, stamp};
use super::validation::timecode;
use super::{Input, Job, LtRangePictures, Plan, R};
use graph::json::Value;
use graph::source::SourceGraph;
use prproj::convert::model::{ticks_per_unit, Fraction, Model, TICKS_PER_SECOND as TPS};
use prproj::convert::prepared::PreparedProjectInput;
use prproj::convert::rewrap::supported_rate;
use std::{cell::LazyCell, collections::{BTreeSet, HashMap, HashSet}};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) enum PrepareIssue {
    Destination(String),
    Tools(String),
    Source(String),
    SourceFile { path: PathBuf, message: String },
}
impl PrepareIssue {
    pub(crate) fn into_message(self) -> String {
        match self {
            Self::Destination(message)
            | Self::Tools(message)
            | Self::Source(message)
            | Self::SourceFile { message, .. } => message,
        }
    }

    pub(super) fn source_file(path: &str) -> impl Fn(String) -> Self + '_ {
        move |message| Self::SourceFile { path: PathBuf::from(path), message }
    }
}
impl From<String> for PrepareIssue {
    fn from(message: String) -> Self {
        Self::Source(message)
    }
}
impl From<&str> for PrepareIssue {
    fn from(message: &str) -> Self {
        Self::Source(message.into())
    }
}

/// Shared read-only preparation for the native DNx, ProRes and saved Gain route.
pub(crate) struct NativeMediaPreparation<'input, 'graph> {
    pub model: &'input Model,
    pub source: &'graph SourceGraph,
    pub writer_input: Option<&'input mut PreparedProjectInput>,
    pub gain: &'input GainPaths<'graph>,
    pub managed_pictures: LtRangePictures,
}

impl NativeMediaPreparation<'_, '_> {
    pub(crate) fn preflight(&self) -> R<()> {
        if self.gain.is_empty() {
            prproj::convert::native_av::preflight(self.model).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn prepare(
        self,
        directory: &Path,
        media_roots: &[String],
        project: &Path,
        graph: Option<&Path>,
        ffmpeg: &Path,
        ffprobe: &Path,
    ) -> Result<Plan, PrepareIssue> {
        let plan = Plan::prepare_media_review(
            self.model, Some(self.source), directory, media_roots, project, graph, ffmpeg, ffprobe, true, false, self.gain,
            self.managed_pictures,
        )?;
        if let Some(input) = self.writer_input {
            plan.attach_mono(input)?;
        }
        Ok(plan)
    }
}

fn validate_destination(
    directory: &Path,
    media_roots: &[String],
    project: &Path,
    graph: Option<&Path>,
) -> R<(PathBuf, PathBuf)> {
    if fs::symlink_metadata(directory).is_ok() {
        return Err("rewrap destination already exists; choose a NEW directory".into());
    }
    let parent = directory
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|e| format!("rewrap parent: {e}"))?;
    if !parent.is_dir() {
        return Err("rewrap parent is not a directory".into());
    }
    let name = directory
        .file_name()
        .ok_or("rewrap directory needs a name")?;
    if name == "." || name == ".." {
        return Err("invalid rewrap directory".into());
    }
    let destination = parent.join(name);
    let destination_text = destination
        .to_str()
        .ok_or("rewrap destination must be UTF-8")?;
    if destination_text
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}'))
    {
        return Err("rewrap destination contains characters unsafe in a project path".into());
    }
    if destination.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("Avid MediaFiles")
    }) {
        return Err("rewrap destination cannot be inside Avid MediaFiles".into());
    }
    for root in media_roots {
        let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
        if destination.starts_with(root) {
            return Err("rewrap destination must be outside scanned source-media roots".into());
        }
    }
    for path in std::iter::once(project).chain(graph) {
        let pp = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let p = fs::canonicalize(pp)
            .map_err(|e| e.to_string())?
            .join(path.file_name().ok_or("output lacks name")?);
        if p == destination || p.starts_with(&destination) {
            return Err("project/graph output cannot overlap the rewrap directory".into());
        }
    }
    Ok((parent, destination))
}

fn referenced_clip_ids(model: &Model) -> HashSet<&str> {
    model.bins.iter().flat_map(|bin| bin.clips.iter().map(String::as_str))
        .chain(model.sequences.iter().flat_map(|seq| seq.events.iter().map(|event| event.clip.as_str())))
        .collect()
}

/// Exact model locators for a read-only follow-up after a file-specific refusal.
pub(crate) fn referenced_input_paths(model: &Model) -> impl Iterator<Item = &Path> {
    let used = referenced_clip_ids(model);
    let mut seen = HashSet::new();
    model.clips.values()
        .filter(move |clip| used.contains(clip.id.as_str()))
        .flat_map(|clip| clip.video.iter().map(|video| video.path.as_str())
            .chain(clip.audio.iter().map(|audio| audio.path.as_str())))
        .filter(move |path| seen.insert(*path))
        .map(Path::new)
}

fn splits_mono_wavs(prores: bool, audio_lanes: usize) -> bool {
    !prores && audio_lanes > 32
}

/// Preview picture rewraps before a media folder is chosen. Deduplicate the
/// whole bundle first, since distinct bundles can each emit a WAV from one input.
pub(crate) fn preview_picture_recipes(
    bundles: impl Iterator<Item = (Vec<PathBuf>, bool)>,
) -> Vec<Vec<PathBuf>> {
    let mut recipes = Vec::new();
    for (paths, prores) in bundles.collect::<BTreeSet<_>>() {
        if splits_mono_wavs(prores, paths.len().saturating_sub(1)) {
            recipes.extend(paths.into_iter().map(|path| vec![path]));
        } else {
            recipes.push(paths);
        }
    }
    recipes
}

/// Saved source recipes for a full rewrap's missing-folder Review.
pub(crate) fn preview_native_recipes(model: &Model, gain: &GainPaths<'_>) -> Vec<Vec<PathBuf>> {
    let used = referenced_clip_ids(model);
    let clips = model.clips.values().filter(|clip| used.contains(clip.id.as_str()));
    let mut recipes = preview_picture_recipes(clips.clone().filter_map(|clip| {
        let picture = clip.video.as_ref()?;
        let mut audio: Vec<_> = clip.audio.iter().collect();
        audio.sort_by_key(|stream| stream.avid_track);
        let paths = std::iter::once(PathBuf::from(&picture.path))
            .chain(audio.iter().map(|stream| PathBuf::from(&stream.path)))
            .collect();
        let prores = avid_reader::mxf::ProResProfile::from_fourcc(&picture.codec).is_some();
        Some((paths, prores))
    }));
    if !gain.is_empty() {
        let mono: BTreeSet<_> = clips
            .filter_map(|clip| super::mono::audio_clip(clip).ok())
            .map(|audio| PathBuf::from(&audio.path))
            .collect();
        recipes.extend(mono.into_iter().map(|path| vec![path]));
    }
    recipes
}

impl Plan {
    fn input_paths(&self, indices: &[usize]) -> Vec<PathBuf> {
        indices
            .iter()
            .map(|&index| self.inputs[index].path.clone())
            .collect()
    }

    pub(crate) fn estimate_sources(&self) -> Vec<Vec<PathBuf>> {
        let mono_recipe = |pcm: &super::mono::MonoPcm| vec![pcm.input_path().to_owned()];
        let mut recipes = Vec::new();
        for job in &self.jobs {
            recipes.push(self.input_paths(job.packaged_inputs()));
            recipes.extend(job.wide_audio.iter().map(mono_recipe));
        }
        recipes.extend(self.mono.iter().map(mono_recipe));
        recipes
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn op1a_targets(&self) -> impl Iterator<Item = (Vec<PathBuf>, PathBuf)> + '_ {
        self.jobs.iter().enumerate().map(|(index, job)| {
            (
                self.input_paths(&job.inputs),
                self.destination.join(format!("media-{:04}.mxf", index + 1)),
            )
        })
    }

    pub fn estimated_bytes(&self)->u64 {self.estimate}
    pub fn available_bytes(&self)->u64 {self.free}

    pub fn directory(&self) -> &Path {
        &self.destination
    }
    pub(crate) fn mode(&self, default: &'static str) -> &'static str {
        if self.mono.is_empty() {
            default
        } else if self.jobs.is_empty() {
            "native prepared mono PCM24 WAV"
        } else {
            "native prepared media (DNx OP1a / ProRes MOV / mono PCM24 WAV)"
        }
    }
    fn attach_mono(&self, input: &mut PreparedProjectInput) -> R<()> {
        use prproj::convert::prepared::PreparedClip;
        let outputs: HashMap<_, _> = self.mono.iter().enumerate()
            .map(|(i, audio)| (audio.source.as_str(), (i, self.mono_path(i))))
            .collect();
        if outputs.is_empty() { return Ok(()); }
        let mut bound = HashSet::new();
        for c in &mut input.clips {
            let PreparedClip::Legacy(clip) = c else { continue };
            if clip.video.is_some() { continue; }
            if let Some((index, path)) = clip.audio.first().and_then(|a| outputs.get(a.path.as_str())) {
                *c = PreparedClip::Original(super::mono::writer_clip(clip, path)?);
                bound.insert(*index);
            }
        }
        if bound.len() != outputs.len() {
            return Err("prepared mono WAV has no matching writer input".into());
        }
        Ok(())
    }
    pub(super) fn mono_path(&self, index: usize) -> PathBuf {
        self.destination.join(format!("audio-{:04}.wav", index + 1))
    }

    pub(super) fn wide_audio_path(&self, job: usize, lane: usize) -> PathBuf {
        self.destination.join(format!("media-{:04}-audio-{:02}.wav", job + 1, lane + 1))
    }

    pub(super) fn job_bindings(
        &self, index: usize, target: &str,
    ) -> impl Iterator<Item = prproj::convert::rewrap::MediaRelink> + '_ {
        let job = &self.jobs[index];
        let target = target.to_owned();
        job.inputs.iter().enumerate().flat_map(move |(ordinal, input)| {
            let wide_audio = !job.wide_audio.is_empty() && ordinal > 0;
            let target_path = if wide_audio {
                self.wide_audio_path(index, ordinal - 1).to_string_lossy().into_owned()
            } else { target.clone() };
            self.inputs[*input].aliases.iter().map(move |path| prproj::convert::rewrap::MediaRelink {
                source_path: path.clone(),
                target_path: target_path.clone(),
                audio_stream: ordinal.checked_sub(1).map(|lane| if wide_audio { 0 } else { lane as u32 }),
                separate_mono: job.separate_mono,
                pixel_aspect: if ordinal == 0 { job.pixel_aspect } else { None },
            })
        })
    }
    /// Future paths/channel selectors are fixed by the read-only plan, so project
    /// construction can be validated before any media is created.
    pub fn bindings(&self) -> Vec<prproj::convert::rewrap::MediaRelink> {
        let mut mappings = Vec::new();
        for (j, job) in self.jobs.iter().enumerate() {
            let target = self
                .destination
                .join(format!("media-{:04}.{}", j + 1, if job.mov {"mov"}else{"mxf"}))
                .to_string_lossy()
                .into_owned();
            mappings.extend(self.job_bindings(j, &target));
        }
        mappings
    }

    pub(crate) fn dnxhr_444_rgb_sources(&self) -> std::collections::HashSet<String> {
        self.jobs.iter().filter_map(|job| {
            let input = &self.inputs[job.inputs[0]];
            let picture = &input.probe.streams[0];
            // Premiere's native OP1a import interprets this descriptor as full-range RGB.
            (!job.mov
                && picture.profile == "DNXHR 444"
                && picture.pix == "yuv444p10le"
                && picture.color.iter().map(Option::as_deref).eq([
                    Some("1:1"), None, Some("bt709"), None, None, Some("progressive"),
                ]))
                .then_some(input.aliases.iter().cloned())
        }).flatten().collect()
    }

    pub fn prepare(
        model: &Model,
        directory: &Path,
        media_roots: &[String],
        project: &Path,
        graph: Option<&Path>,
        ffmpeg: &Path,
        ffprobe: &Path,
    ) -> R<Self> {
        Self::prepare_media(model,directory,media_roots,project,graph,ffmpeg,ffprobe,false)
    }
    /// Explicit mixed-codec preparation: DNx to OP1a, qualified ProRes to MOV.
    pub fn prepare_media(
        model: &Model,
        directory: &Path,
        media_roots: &[String],
        project: &Path,
        graph: Option<&Path>,
        ffmpeg: &Path,
        ffprobe: &Path,
        allow_mov: bool,
    ) -> R<Self> {
        Self::prepare_media_review(model, None, directory, media_roots, project, graph, ffmpeg, ffprobe, allow_mov, false, &HashMap::new(), HashMap::new())
            .map_err(PrepareIssue::into_message)
    }

    pub(crate) fn prepare_selected(
        model: &Model, directory: &Path, media_roots: &[String], project: &Path,
        ffmpeg: &Path, ffprobe: &Path,
    ) -> Result<Self, PrepareIssue> {
        Self::prepare_media_review(model, None, directory, media_roots, project, None, ffmpeg, ffprobe,
            true, true, &HashMap::new(), HashMap::new())
    }

    fn prepare_media_review(
        model: &Model,
        source: Option<&SourceGraph>,
        directory: &Path,
        media_roots: &[String],
        project: &Path,
        graph: Option<&Path>,
        ffmpeg: &Path,
        ffprobe: &Path,
        allow_mov: bool,
        prefer_separate_mono: bool,
        gain_paths: &GainPaths<'_>,
        mut managed_pictures: LtRangePictures,
    ) -> Result<Self, PrepareIssue> {
        let (parent, destination) = validate_destination(directory, media_roots, project, graph)
            .map_err(PrepareIssue::Destination)?;
        if model
            .sequences
            .iter()
            .any(|s| !supported_rate(s.rate) || s.width > 16384 || s.height > 16384)
        {
            return Err("rewrap preparation: output timeline rate/raster is not supported".into());
        }
        // Destination track capacity is validated by the staged project writer before execution.
        prproj::convert::model::validate_model(model).map_err(|e| e.to_string())?;
        let used = referenced_clip_ids(model);
        if allow_mov && !prefer_separate_mono && model.clips.values().any(|clip| {
            used.contains(clip.id.as_str())
                && clip.audio.len() > 8
                && clip.video.as_ref().is_some_and(|video| {
                    avid_reader::mxf::ProResProfile::from_fourcc(&video.codec).is_some()
                })
        }) {
            return Err("ProRes MOV currently supports at most eight qualified PCM lanes".into());
        }
        let versions = vec![
            version(ffmpeg, "ffmpeg").map_err(PrepareIssue::Tools)?,
            version(ffprobe, "ffprobe").map_err(PrepareIssue::Tools)?,
        ];
        let mut plan = Self {
            destination,
            inputs: vec![],
            jobs: vec![],
            mono: vec![],
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
            versions,
            estimate: 0,
            free: 0,
        };
        let mut paths = HashMap::<PathBuf, usize>::new();
        let mut recipes = HashMap::<Vec<usize>, usize>::new();
        let mut owners = HashMap::<usize, Vec<usize>>::new();
        let mut mono_sources = HashMap::<&str, usize>::new();
        let saved_descriptors = LazyCell::new(|| super::mov::saved_picture_descriptors(source));
        if model.clips.is_empty() {
            return Err("no clips selected for rewrapping".into());
        }
        for clip in model
            .clips
            .values()
            .filter(|c| used.contains(c.id.as_str()))
        {
            if allow_mov && !gain_paths.is_empty() && clip.video.is_none() {
                let path = clip.audio.first().ok_or("offline audio master in Gain conversion")?.path.as_str();
                let pcm = super::mono::MonoPcm::plan(clip, gain_paths.get(path).copied(), ffprobe)?;
                if let Some(&i) = mono_sources.get(path) {
                    if plan.mono[i] != pcm {
                        return Err("conflicting mono PCM source recipes".into());
                    }
                } else {
                    mono_sources.insert(path, plan.mono.len());
                    plan.estimate = plan.estimate.checked_add(pcm.size()).ok_or("size overflow")?;
                    plan.mono.push(pcm);
                }
                continue;
            }
            let video = clip.video.as_ref().ok_or_else(|| {
                format!(
                    "clip {}: audio-only/offline sources are not yet qualified",
                    clip.id
                )
            })?;
            let mov = avid_reader::mxf::ProResProfile::from_fourcc(&video.codec).is_some();
            let separate_mono = clip.audio.len() > 32 || (mov && prefer_separate_mono);
            let wide_dnx = splits_mono_wavs(mov, clip.audio.len());
            if wide_dnx && (clip.audio.len() > 64
                || !clip.stereo_pairs.is_empty() || !clip.multichannel_groups.is_empty()
                || clip.audio.iter().any(|audio| !matches!(audio.bits, 16 | 24)))
            {
                return Err(format!(
                    "{}: wide DNx rewrap requires 33–64 independent PCM16 or PCM24 mono lanes",
                    clip.name,
                ).into());
            }
            if (video.codec != "AVdn" && !(allow_mov && mov)) || !supported_rate(video.rate)
            {
                return Err(format!("clip {}: rewrap requires a qualified DNx or ProRes profile and rate",clip.id).into());
            }
            let picture_issue = PrepareIssue::source_file(&video.path);
            let source_id = avid_reader::mxf::read_identity(Path::new(&video.path))
                .map_err(|e| picture_issue(e.to_string()))?;
            let native_clock = source_id.file_start_timecode();
            let freeze = video.codec == "apch" && native_clock.is_none();
            let tc = if freeze {
                if !clip.audio.is_empty() || video.rate != Fraction::new(24, 1)
                    || video.start != 0 || video.duration != 5 * TPS
                    || source_id.picture_essence_coding != Some(avid_reader::mxf::PRORES_422_HQ_CODING)
                    || source_id.file_tracks != [(24, 1, 120)]
                    || !source_id.timecodes.is_empty() || !source_id.qualified_file_clocks.is_empty()
                    || source_id.file_packages.len() != 1 || source_id.material_packages.len() != 1
                    || clip.material_umid.as_ref() != source_id.material_packages.first()
                {
                    return Err("unqualified generated Freeze Frame package, extent or timecode".into());
                }
                None
            } else {
                let (frames, base, drop) = native_clock
                    .ok_or_else(|| picture_issue("missing/ambiguous source timecode".into()))?;
                if base as i64 != (video.rate.num + video.rate.den / 2) / video.rate.den
                    || i128::from(frames) * i128::from(ticks_per_unit(video.rate))
                        != i128::from(video.start)
                {
                    return Err(picture_issue(
                        "native source timecode disagrees with exact model start".into(),
                    ));
                }
                Some(timecode(video.start, video.rate, drop).map_err(&picture_issue)?)
            };
            let expected_op = if freeze { "060e2b34040101020d01020110000000" } else { OP_ATOM };
            let avid_prores_picture = !freeze && mov
                && source_id.picture_essence_coding.and_then(avid_reader::mxf::PictureProfile::from_label)
                    .is_some_and(|profile| matches!(profile, avid_reader::mxf::PictureProfile::ProRes(p) if p.fourcc() == video.codec))
                && source_id.file_tracks.contains(&(video.rate.num, video.rate.den, video.duration / ticks_per_unit(video.rate)))
                && source_id.file_packages.len() == 1;
            let avid_lt_picture = video.codec == "apcs" && clip.audio.is_empty()
                && source_id.picture_essence_coding == Some(avid_reader::mxf::PRORES_422_LT_CODING)
                && source_id.file_tracks == [(24, 1, video.duration / ticks_per_unit(video.rate))]
                && source_id.file_packages.len() == 1;
            let mut recipe = Vec::new();
            let mut pixel_aspect = None;
            let mut audio: Vec<_> = clip.audio.iter().collect();
            if audio.iter().any(|a| a.bits != audio[0].bits) {
                return Err("mixed PCM bit depths in one OP1a file are not supported; no media created or samples converted".into());
            }
            audio.sort_by_key(|a| a.avid_track);
            if audio.iter().any(|a| a.avid_track.is_none())
                || audio.windows(2).any(|w| w[0].avid_track == w[1].avid_track)
            {
                return Err("rewrap requires explicit unique Avid audio tracks".into());
            }
            for (at, original) in std::iter::once(video.path.as_str())
                .chain(audio.iter().map(|a| a.path.as_str()))
                .enumerate()
            {
                // Keep the graph's locator for attribution, including when the probe
                // has to use its canonical path to deduplicate physical files.
                let input_issue = PrepareIssue::source_file(original);
                let accepts_op = |op: &str| op == expected_op
                    || (at == 0 && (avid_lt_picture || avid_prores_picture) && op == super::backend::AVID_PRORES_LT_ATOM);
                let path = fs::canonicalize(original)
                    .map_err(|e| input_issue(format!("{original}: {e}")))?;
                if path.to_str().is_none()
                    || original
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}'))
                {
                    return Err(input_issue("rewrap input path cannot be represented safely".into()));
                }
                let index = if let Some(&i) = paths.get(&path) {
                    if !plan.inputs[i].aliases.contains(&original.to_string()) {
                        plan.inputs[i].aliases.push(original.into());
                    }
                    i
                } else {
                    let before = stamp(&path).map_err(&input_issue)?;
                    let p = probe(ffprobe, &path).map_err(&input_issue)?;
                    if !accepts_op(&p.op) || p.streams.len() != 1 {
                        return Err(input_issue(format!(
                            "{}: unqualified OP-Atom operational-pattern variant or stream layout",
                            path.display()
                        )));
                    }
                    let identity = id_value(&path).map_err(&input_issue)?;
                    if before != stamp(&path).map_err(&input_issue)? {
                        return Err(input_issue("input changed during probe".into()));
                    }
                    let i = plan.inputs.len();
                    plan.inputs.push(Input {
                        path: path.clone(),
                        aliases: vec![original.into()],
                        stamp: before,
                        probe: p,
                        identity,
                        terminal: None,
                    });
                    paths.insert(path, i);
                    i
                };
                let input = &plan.inputs[index];
                if !accepts_op(&input.probe.op) || (freeze && !input.probe.references.is_empty()) {
                    return Err(input_issue(
                        "conflicting or unqualified source operational pattern".into(),
                    ));
                }
                if input.probe.op == super::backend::AVID_PRORES_LT_ATOM {
                    super::backend::verify_stream_package(&input.probe, &source_id.file_packages)
                        .map_err(&input_issue)?;
                }
                let stream = &input.probe.streams[0];
                if stream.ticks != video.duration {
                    return Err(input_issue(format!(
                        "clip {}: unequal source spans cannot be silently padded or trimmed",
                        clip.id
                    )));
                }
                let input_clock = avid_reader::mxf::read_identity(&input.path)
                    .map_err(|e| input_issue(e.to_string()))?;
                if input_clock.file_start_timecode() != native_clock {
                    return Err(input_issue(
                        "source native timecode components do not share the picture clock".into(),
                    ));
                }
                if input.probe.tc != tc {
                    return Err(input_issue(format!(
                        "clip {}: source timecode does not match the exact model clock",
                        clip.id
                    )));
                }
                if at == 0 {
                    if mov {
                        pixel_aspect = super::mov::validate_source(stream, &input.probe.raw, video)
                            .map_err(&input_issue)?;
                        if let Some(aspect) = pixel_aspect {
                            let saved = saved_descriptors.get(video.path.as_str())
                                .map(Vec::as_slice).unwrap_or_default();
                            super::mov::verify_pixel_aspect(video, aspect, &input_clock, saved)
                                .map_err(&input_issue)?;
                        }
                    } else if stream.kind != "video"
                        || stream.codec != "dnxhd"
                        || !["yuv422p", "yuv422p10le", "yuv422p12le", "yuv444p10le", "yuv444p12le",
                             "gbrp10le", "gbrp12le"].contains(&stream.pix.as_str())
                        || !["DNXHD", "DNXHR LB", "DNXHR SQ", "DNXHR HQ", "DNXHR HQX", "DNXHR 444"]
                            .contains(&stream.profile.as_str())
                        || stream.rate != format!("{}/{}", video.rate.num, video.rate.den)
                        || stream.width != video.width
                        || stream.height != video.height
                    {
                        return Err(input_issue(
                            "DNx stream descriptor/frame count disagrees with supported model".into(),
                        ));
                    }
                    if let Some(profile) = source_id.picture_essence_coding.and_then(avid_reader::mxf::PictureProfile::from_label) {
                        if stream.profile != profile.probe_name() {
                            return Err(input_issue(format!(
                                "{} coding label disagrees with the coded picture profile",
                                profile.probe_name()
                            )));
                        }
                        if let avid_reader::mxf::PictureProfile::Dnx(cid) = profile {
                            verify_dnx_cid(&input.path, cid).map_err(&input_issue)?;
                        }
                    }
                } else {
                    let a = audio[at - 1];
                    if a.sample_rate != 48000 || !matches!(a.bits, 16 | 24) {
                        return Err(input_issue("rewrap supports signed 16/24-bit little-endian mono PCM at 48 kHz only; no resampling or encoding".into()));
                    }
                    let bytes = pcm_essence_bytes(&input.path).map_err(&input_issue)?;
                    let expected = (video.duration as i128) * 48000 * (a.bits as i128) / 8;
                    if bytes as i128 * TPS as i128 != expected {
                        return Err(input_issue("PCM physical sample span disagrees with the model; padded/truncated inputs are not qualified".into()));
                    }
                    if stream.kind != "audio"
                        || stream.channels != 1
                        || stream.samples != 48000
                        || stream.samples != a.sample_rate
                        || stream.bits != a.bits
                        || !matches!(a.bits, 16 | 24)
                        || stream.codec != format!("pcm_s{}le", a.bits)
                        || a.duration != video.duration
                        || a.start != video.start
                    {
                        return Err(input_issue(
                            "PCM descriptor/span/start disagrees with supported mono48k model".into(),
                        ));
                    }
                }
                if recipe.contains(&index) {
                    return Err("one OP-Atom file cannot stand in for two source tracks".into());
                }
                recipe.push(index);
            }
            // DATA placeholders must resolve to the other physical source files in
            // this exact recipe, with the same media kind. No orphan references.
            for i in &recipe {
                for (kind, id) in &plan.inputs[*i].probe.references {
                    let matched = recipe.iter().filter(|other| *other != i).any(|other| {
                        let input = &plan.inputs[*other];
                        input.probe.streams[0].kind == *kind && input.identity.get("file_packages")
                            .is_some_and(|v| matches!(v, Value::Arr(ids) if ids == &[Value::Str(id.clone())]))
                    });
                    if !matched {
                        return Err("OP-Atom reference does not resolve to a sibling source in this master recipe".into());
                    }
                }
            }
            let i = recipe[0];
            let input = &plan.inputs[i];
            let picture = &input.probe.streams[0];
            let native_lt = avid_lt_picture && input.probe.op == super::backend::AVID_PRORES_LT_ATOM;
            // A broken index may make the demuxer report the expected count by
            // shifting or dropping packets. Native LT still needs index proof.
            if freeze || native_lt
                || picture.ticks as i128 != picture.packets as i128 * ticks_per_unit(video.rate) as i128
            {
                let terminal = if freeze {
                    super::terminal::Terminal::qualify_freeze(&input.path, &input.probe, ffprobe)
                } else if native_lt {
                    // Only reuse a proof from the same preflight and backend.
                    // Execution still independently verifies the packet boundary.
                    match managed_pictures.remove(&input.path).filter(|p| p.ffprobe == ffprobe) {
                        Some(verified) => {
                            if verified.stamp != input.stamp
                                || source_id.file_packages != [verified.package.as_str()]
                                || input.probe.stream_indices != [verified.stream]
                                || picture.ticks != verified.terminal.frames * ticks_per_unit(Fraction::new(24, 1))
                                || picture.packets != verified.terminal.frames + i64::from(verified.terminal.bytes > 0)
                            {
                                return Err(picture_issue(
                                    "input changed since Color Adapter picture verification".into(),
                                ));
                            }
                            Ok(verified.terminal)
                        }
                        None => super::terminal::Terminal::qualify_lt(&input.path, &input.probe, ffprobe),
                    }
                } else if mov {
                    return Err(picture_issue(
                        "ProRes packet/span mismatch: no DNx terminal-packet exception applies".into(),
                    ));
                } else {
                    super::terminal::Terminal::qualify(&input.path, &input.probe, ffprobe, video.rate)
                        .map_err(|e| format!("DNx packet count does not match the declared picture span; terminal/extra packets are not qualified for automatic removal: {e}"))
                }.map_err(&picture_issue)?;
                if input.stamp != stamp(&input.path).map_err(&picture_issue)? {
                    return Err(picture_issue("input changed during terminal packet validation".into()));
                }
                if terminal.bytes > 0 {
                    plan.inputs[i].terminal = Some(terminal);
                }
            }
            for i in &recipe {
                if let Some(prior) = owners.insert(*i, recipe.clone()) {
                    if prior != recipe {
                        return Err("a physical source occurs in conflicting master recipes; ambiguous global relink refused".into());
                    }
                }
            }
            let job_index = recipes.get(&recipe).copied().unwrap_or(plan.jobs.len());
            let clip_facts = obj(vec![
                ("id", text(&clip.id)),
                ("name", text(&clip.name)),
                (
                    "audio_channel_map",
                    Value::Arr(
                        audio
                            .iter()
                            .enumerate()
                            .map(|(ordinal, a)| {
                                let mut channel = vec![
                                    ("avid_track", Value::Int(a.avid_track.unwrap())),
                                    ("source_path", text(&a.path)),
                                    ("output_audio_ordinal", Value::Int(if wide_dnx { 0 } else { ordinal as i64 })),
                                    ("output_stream_index", Value::Int(if wide_dnx {0} else if mov && !separate_mono {1}else{ordinal as i64+1})),
                                    ("output_channel_index", Value::Int(if mov && !separate_mono {ordinal as i64}else{0})),
                                    ("bits", Value::Int(a.bits)),
                                    ("sample_rate", Value::Int(a.sample_rate)),
                                    ("master_gain_db", Value::Float(a.gain_db)),
                                ];
                                if wide_dnx {
                                    channel.push(("output_path", text(plan.wide_audio_path(job_index, ordinal).to_string_lossy())));
                                }
                                obj(channel)
                            })
                            .collect(),
                    ),
                ),
                (
                    "stereo_pairs",
                    Value::Arr(
                        clip.stereo_pairs
                            .iter()
                            .map(|&(left, right)| {
                                Value::Arr(vec![Value::Int(left), Value::Int(right)])
                            })
                            .collect(),
                    ),
                ),
                ("multichannel_groups",Value::Arr(clip.multichannel_groups.iter().map(|g|obj(vec![("format",text(&g.format)),("channels",Value::Arr(g.channels.iter().map(|n|Value::Int(*n)).collect()))])).collect())),
                ("premiere_channel_routing", Value::Arr(clip.multichannel_groups.iter().map(|g| {
                    let selected = g.premiere_channels();
                    let roles: &[&str] = if g.format=="5.1" { &["L","R","C","LFE","Ls","Rs"] } else { &["L","C","R","Lss","Rss","Lsr","Rsr","LFE"] };
                    obj(vec![
                        ("source_group_format", text(&g.format)),
                        ("destination_channel_type", text(if g.format=="5.1" {"5.1"}else{"Adaptive"})),
                        ("destination_source_labels", Value::Arr(selected.iter().map(|n|Value::Int(*n)).collect())),
                        ("destination_physical_ordinals", Value::Arr(selected.iter().map(|n|Value::Int(audio.iter().position(|a|a.avid_track==Some(*n)).expect("validated complete group") as i64)).collect())),
                        ("destination_channel_roles", Value::Arr(roles.iter().map(|s|text(*s)).collect())),
                        ("premiere_speaker_layout", if g.format=="5.1" {text("5.1")}else{Value::Null}),
                    ])
                }).collect())),
                (
                    "material_umid",
                    clip.material_umid.clone().map(text).unwrap_or(Value::Null),
                ),
                (
                    "tape",
                    clip.tape_name.clone().map(text).unwrap_or(Value::Null),
                ),
                (
                    "avid_metadata",
                    clip.avid
                        .as_ref()
                        .map(|a| {
                            obj(vec![
                                ("mob_id", text(&a.mob_id)),
                                (
                                    "columns",
                                    Value::Arr(
                                        a.columns
                                            .iter()
                                            .map(|(k, v)| Value::Arr(vec![text(k), text(v)]))
                                            .collect(),
                                    ),
                                ),
                                (
                                    "source_file",
                                    a.source_file.clone().map(text).unwrap_or(Value::Null),
                                ),
                                (
                                    "source_path",
                                    a.source_path.clone().map(text).unwrap_or(Value::Null),
                                ),
                            ])
                        })
                        .unwrap_or(Value::Null),
                ),
            ]);
            if let Some(&j) = recipes.get(&recipe) {
                if plan.jobs[j].tc != tc {
                    return Err("shared source has conflicting timecode".into());
                }
                if plan.jobs[j].pixel_aspect != pixel_aspect {
                    return Err("shared ProRes source has conflicting pixel aspects".into());
                }
                plan.jobs[j].clips.push(clip_facts)
            } else {
                let j = plan.jobs.len();
                recipes.insert(recipe.clone(), j);
                plan.estimate = plan.estimate.checked_add(8 << 20).ok_or("size overflow")?;
                for i in &recipe {
                    plan.estimate = plan
                        .estimate
                        .checked_add(plan.inputs[*i].stamp.len)
                        .ok_or("size overflow")?
                }
                if mov {
                    for i in recipe.iter().skip(1) { plan.estimate=plan.estimate.checked_add(plan.inputs[*i].stamp.len).ok_or("size overflow")?; }
                }
                let wide_audio = if wide_dnx {
                    recipe[1..].iter()
                        .enumerate()
                        .map(|(lane, &index)| {
                            super::mono::MonoPcm::plan_wide_sibling(
                                &plan.inputs[index], video.start, &audio[lane].path,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?
                } else { Vec::new() };
                plan.jobs.push(Job {
                    inputs: recipe,
                    clips: vec![clip_facts],
                    tc,
                    rate: video.rate,
                    mov,
                    pixel_aspect,
                    separate_mono,
                    wide_audio,
                });
            }
        }
        if plan.jobs.is_empty() && plan.mono.is_empty() {
            return Err("no referenced clips selected for rewrapping".into());
        }
        if gain_paths.keys().any(|p| !mono_sources.contains_key(p)) {
            return Err("a saved AudioSuite Gain render has no verified WAV output".into());
        }
        if plan.inputs.iter().any(|i| i.terminal.is_some()) {
            super::terminal::require_filter(ffmpeg).map_err(PrepareIssue::Tools)?;
        }
        plan.free = free_space(&parent).map_err(PrepareIssue::Destination)?;
        if plan.free < plan.estimate {
            return Err(PrepareIssue::Destination(format!(
                "insufficient disk space: estimated {} bytes, available {}",
                plan.estimate, plan.free
            )));
        }
        Ok(plan)
    }
    pub fn review_text(&self) -> String {
        let wide_audio_count: usize = self.jobs.iter().map(|job| job.wide_audio.len()).sum();
        let estimate = if self.mono.is_empty() {
            "Estimate includes input file sizes plus an 8 MiB allowance per output, and an additional audio scratch allowance for MOV. Actual size may differ; free space is checked again while writing."
        } else {
            "Estimate includes the playable PCM bytes and WAV headers, plus allowances for any picture preparation. Free space is checked again while writing."
        };
        let mut text = format!("Creates new media.\n\nNew media folder:\n{}\n\n{} new media file(s)\nEstimated additional space: {} ({} bytes)\nAvailable on selected volume: {} ({} bytes)\n\n{estimate}\n\nOriginal media stays in place; no encoding.\nPremiere audio-routing qualification remains pending.",
            self.destination.display(), self.jobs.len() + self.mono.len() + wide_audio_count,
            crate::media_prompt::size_label(self.estimate), self.estimate,
            crate::media_prompt::size_label(self.free), self.free);
        let count = self.inputs.iter().filter(|i| i.terminal.is_some()).count();
        if count > 0 {
            text.push_str(&format!("\n\n{count} source(s): exclude one verified zero-only terminal packet; retain all coded pictures and audio. Excluded byte offsets and hashes recorded in media receipts."));
        }
        if !self.mono.is_empty() {
            text.push_str("\n\nMono PCM24 is copied into WAV. AudioSuite Gain uses the saved render within its playable span.");
        }
        if wide_audio_count > 0 {
            text.push_str(&format!("\n\n{wide_audio_count} wide-master mono lanes will link to separate PCM WAVs. Their picture is rewrapped to picture-only OP1a."));
        }
        text
    }
    pub fn describe(&self) {
        println!("{}", self.review_text());
    }
}

fn verify_dnx_cid(path: &Path, expected: u16) -> R<()> {
    use std::io::{Read, Seek, SeekFrom};
    let spans = super::source::essence_spans(path)?;
    let picture = spans.iter().find(|span| span.kind == 0x15)
        .filter(|span| span.bytes >= 44).ok_or("missing coded VC-3 picture")?;
    let mut source = fs::File::open(path).map_err(|e| e.to_string())?;
    source.seek(SeekFrom::Start(picture.offset + 40)).map_err(|e| e.to_string())?;
    let mut coded = [0; 4];
    source.read_exact(&mut coded).map_err(|e| e.to_string())?;
    if u32::from_be_bytes(coded) != u32::from(expected) {
        return Err("VC-3 picture compression ID disagrees with the MXF descriptor".into());
    }
    Ok(())
}
