//! Read-only media plan construction and review; creates no output files.
use super::backend::{probe, version, OP_ATOM};
use super::receipts::{obj, text};
use super::source::{free_space, id_value, pcm_essence_bytes, stamp};
use super::validation::timecode;
use super::{Input, Job, Plan, R};
use graph::json::Value;
use prproj::convert::model::{ticks_per_unit, Model, TICKS_PER_SECOND as TPS};
use prproj::convert::rewrap::supported_rate;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

impl Plan {
    pub fn estimated_bytes(&self)->u64 {self.estimate}
    pub fn available_bytes(&self)->u64 {self.free}

    pub fn directory(&self) -> &Path {
        &self.destination
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
            for (n, i) in job.inputs.iter().enumerate() {
                for path in &self.inputs[*i].aliases {
                    mappings.push(prproj::convert::rewrap::MediaRelink {
                        source_path: path.clone(),
                        target_path: target.clone(),
                        audio_stream: if n == 0 { None } else { Some((n - 1) as u32) },
                    });
                }
            }
        }
        mappings
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
    /// Explicit mixed-codec preparation: DNx to OP1a, qualified ProRes LT to MOV.
    pub fn prepare_media(model:&Model,directory:&Path,media_roots:&[String],project:&Path,graph:Option<&Path>,ffmpeg:&Path,ffprobe:&Path,allow_mov:bool)->R<Self> {
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
        if model
            .sequences
            .iter()
            .any(|s| !supported_rate(s.rate) || s.width > 16384 || s.height > 16384)
        {
            return Err("rewrap preparation: output timeline rate/raster is not supported".into());
        }
        // Rewrapping must not create media for a timeline the existing writer will refuse.
        if model.sequences.iter().any(|s| {
            s.events.iter().any(|e| {
                e.audio_tracks.iter().any(|t| *t > 4)
                    || e.stereo.iter().any(|p| p.audio_track > 4)
                    || e.video_track.is_some_and(|t| t > 3)
            })
        }) {
            return Err("rewrap preparation: timeline layout exceeds the existing writer's 4 audio / 3 video tracks; source bins may carry up to 32 discrete mono files".into());
        }
        prproj::convert::model::validate_model(model).map_err(|e| e.to_string())?;
        let used: HashSet<&str> = model
            .bins
            .iter()
            .flat_map(|b| b.clips.iter().map(String::as_str))
            .chain(
                model
                    .sequences
                    .iter()
                    .flat_map(|s| s.events.iter().map(|e| e.clip.as_str())),
            )
            .collect();
        if allow_mov && model.clips.values().any(|c|used.contains(c.id.as_str()) && c.video.as_ref().is_some_and(|v|v.codec=="apcs") && c.audio.len()>8) {
            return Err("ProRes MOV currently supports at most eight qualified PCM lanes".into());
        }
        let versions = vec![version(ffmpeg, "ffmpeg")?, version(ffprobe, "ffprobe")?];
        let mut plan = Self {
            destination,
            inputs: vec![],
            jobs: vec![],
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
            versions,
            estimate: 0,
            free: 0,
        };
        let mut paths = HashMap::<PathBuf, usize>::new();
        let mut recipes = HashMap::<Vec<usize>, usize>::new();
        let mut owners = HashMap::<usize, Vec<usize>>::new();
        if model.clips.is_empty() {
            return Err("no clips selected for rewrapping".into());
        }
        for clip in model
            .clips
            .values()
            .filter(|c| used.contains(c.id.as_str()))
        {
            let video = clip.video.as_ref().ok_or_else(|| {
                format!(
                    "clip {}: audio-only/offline sources are not yet qualified",
                    clip.id
                )
            })?;
            if (video.codec != "AVdn" && !(allow_mov && video.codec == "apcs")) || !supported_rate(video.rate) || clip.audio.len() > 32 {
                return Err(format!("clip {}: rewrap supports qualified MXF rates, DNxHD/DNxHR 8-bit 4:2:2 picture and at most 32 mono PCM inputs",clip.id));
            }
            let source_id = avid_reader::mxf::read_identity(Path::new(&video.path))
                .map_err(|e| e.to_string())?;
            let (frames, base, drop) = source_id
                .file_start_timecode()
                .ok_or("missing/ambiguous source timecode")?;
            if base as i64 != (video.rate.num + video.rate.den / 2) / video.rate.den
                || i128::from(frames) * i128::from(ticks_per_unit(video.rate))
                    != i128::from(video.start)
            {
                return Err("native source timecode disagrees with exact model start".into());
            }
            let tc = timecode(video.start, video.rate, drop)?;
            let mut recipe = Vec::new();
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
                let path = fs::canonicalize(original).map_err(|e| format!("{original}: {e}"))?;
                if path.to_str().is_none()
                    || original
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}'))
                {
                    return Err("rewrap input path cannot be represented safely".into());
                }
                let index = if let Some(&i) = paths.get(&path) {
                    if !plan.inputs[i].aliases.contains(&original.to_string()) {
                        plan.inputs[i].aliases.push(original.into());
                    }
                    i
                } else {
                    let before = stamp(&path)?;
                    let p = probe(ffprobe, &path)?;
                    if p.op != OP_ATOM || p.streams.len() != 1 {
                        return Err(format!(
                            "{}: unqualified OP-Atom operational-pattern variant or stream layout",
                            path.display()
                        ));
                    }
                    let identity = id_value(&path)?;
                    if before != stamp(&path)? {
                        return Err("input changed during probe".into());
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
                let stream = &input.probe.streams[0];
                if stream.ticks != video.duration {
                    return Err(format!(
                        "clip {}: unequal source spans cannot be silently padded or trimmed",
                        clip.id
                    ));
                }
                let native_clock =
                    avid_reader::mxf::read_identity(&input.path).map_err(|e| e.to_string())?;
                if native_clock.file_start_timecode() != Some((frames, base, drop)) {
                    return Err(
                        "source native timecode components do not share the picture clock".into(),
                    );
                }
                if input.probe.tc.as_deref() != Some(&tc) {
                    return Err(format!(
                        "clip {}: source timecode does not match the exact model clock",
                        clip.id
                    ));
                }
                if at == 0 {
                    if video.codec == "apcs" {
                        super::mov::validate_source(stream, &input.probe.raw, video)?;
                    } else if stream.kind != "video"
                        || stream.codec != "dnxhd"
                        || stream.pix != "yuv422p"
                        || !["DNXHD", "DNXHR LB", "DNXHR SQ", "DNXHR HQ"]
                            .contains(&stream.profile.as_str())
                        || stream.rate != format!("{}/{}", video.rate.num, video.rate.den)
                        || stream.width != video.width
                        || stream.height != video.height
                    {
                        return Err(
                            "DNx stream descriptor/frame count disagrees with supported model"
                                .into(),
                        );
                    }
                } else {
                    let a = audio[at - 1];
                    if a.sample_rate != 48000 || !matches!(a.bits, 16 | 24) {
                        return Err("rewrap supports signed 16/24-bit little-endian mono PCM at 48 kHz only; no resampling or encoding".into());
                    }
                    let bytes = pcm_essence_bytes(&input.path)?;
                    let expected = (video.duration as i128) * 48000 * (a.bits as i128) / 8;
                    if bytes as i128 * TPS as i128 != expected {
                        return Err("PCM physical sample span disagrees with the model; padded/truncated inputs are not qualified".into());
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
                        return Err(
                            "PCM descriptor/span/start disagrees with supported mono48k model"
                                .into(),
                        );
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
            let picture = &plan.inputs[recipe[0]].probe.streams[0];
            if picture.ticks as i128 != picture.packets as i128 * ticks_per_unit(video.rate) as i128
            {
                if video.codec=="apcs" { return Err("ProRes packet/span mismatch: no DNx terminal-packet exception applies".into()); }
                let i = recipe[0];
                let input = &plan.inputs[i];
                let terminal = super::terminal::Terminal::qualify(&input.path, &input.probe, ffprobe, video.rate)
                    .map_err(|e| format!("DNx packet count does not match the declared picture span; terminal/extra packets are not qualified for automatic removal: {e}"))?;
                if input.stamp != stamp(&input.path)? {
                    return Err("input changed during terminal packet validation".into());
                }
                plan.inputs[i].terminal = Some(terminal);
            }
            for i in &recipe {
                if let Some(prior) = owners.insert(*i, recipe.clone()) {
                    if prior != recipe {
                        return Err("a physical source occurs in conflicting master recipes; ambiguous global relink refused".into());
                    }
                }
            }
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
                                obj(vec![
                                    ("avid_track", Value::Int(a.avid_track.unwrap())),
                                    ("source_path", text(&a.path)),
                                    ("output_audio_ordinal", Value::Int(ordinal as i64)),
                                    ("output_stream_index", Value::Int(if video.codec=="apcs" {1}else{ordinal as i64+1})),
                                    ("output_channel_index", Value::Int(if video.codec=="apcs" {ordinal as i64}else{0})),
                                    ("bits", Value::Int(a.bits)),
                                    ("sample_rate", Value::Int(a.sample_rate)),
                                    ("master_gain_db", Value::Float(a.gain_db)),
                                ])
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
                if video.codec=="apcs" {
                    for i in recipe.iter().skip(1) { plan.estimate=plan.estimate.checked_add(plan.inputs[*i].stamp.len).ok_or("size overflow")?; }
                }
                plan.jobs.push(Job {
                    inputs: recipe,
                    clips: vec![clip_facts],
                    tc,
                    rate: video.rate,
                    mov: video.codec=="apcs",
                });
            }
        }
        if plan.jobs.is_empty() {
            return Err("no referenced clips selected for rewrapping".into());
        }
        if plan.inputs.iter().any(|i| i.terminal.is_some()) {
            super::terminal::require_filter(ffmpeg)?;
        }
        plan.free = free_space(&parent)?;
        if plan.free < plan.estimate {
            return Err(format!(
                "insufficient disk space: estimated {} bytes, available {}",
                plan.estimate, plan.free
            ));
        }
        Ok(plan)
    }
    pub fn review_text(&self) -> String {
        let mut text = format!("Creates new media.\n\nNew media folder:\n{}\n\n{} new media file(s)\nEstimated additional space: {} ({} bytes)\nAvailable on selected volume: {} ({} bytes)\n\nEstimate includes input file sizes plus an 8 MiB allowance per output, and an additional audio scratch allowance for MOV. Actual size may differ; free space is checked again while writing.\n\nOriginal media stays in place; no encoding.\nPremiere audio-routing qualification remains pending.",
            self.destination.display(), self.jobs.len(),
            crate::media_prompt::size_label(self.estimate), self.estimate,
            crate::media_prompt::size_label(self.free), self.free);
        let count = self.inputs.iter().filter(|i| i.terminal.is_some()).count();
        if count > 0 {
            text.push_str(&format!("\n\n{count} source(s): exclude one verified zero-only terminal DNx packet; retain all coded pictures and audio. Excluded byte offsets and hashes recorded in media receipts."));
        }
        text
    }
    pub fn describe(&self) {
        println!("{}", self.review_text());
    }
}
