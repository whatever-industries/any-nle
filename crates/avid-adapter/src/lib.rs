//! Direct Avid source adapter. No destination model or writer participates in extraction.
//! Source facts and graph construction each have a 64 MiB accounted budget; these are
//! distinct from the encoded graph ceiling and are not peak-RSS guarantees.
mod effects;
pub mod index;
pub mod original;
pub mod proxy;
mod revision;
mod sequence;
mod multicam;
pub mod managed_multistream;
mod subclips;
use avid_reader::{
    extract::{self, AvidMarker, Essence, KIND_PICTURE, KIND_SOUND},
    io::MobId,
    source::{self as av, Observed, Rate},
};
use graph::{source::*, *};
use std::{
    collections::{HashMap, HashSet},
    mem::size_of,
    path::{Path, PathBuf},
};

/// A source timing route that would otherwise omit a saved timeline edit.
pub const UNQUALIFIED_ROUTE_TIMING: &str = "source_route_timing";
pub const PAN_AUTOMATION_LOSS: &str = "pan_automation";

fn source_master_copy<'a>(
    mob: MobId, bin: usize, copies: &HashMap<MobId, Vec<(usize, usize)>>, items: &'a [SourceItem],
) -> Option<&'a SourceItem> {
    let all = copies.get(&mob)?;
    let mut local = all.iter().filter(|(owner, _)| *owner == bin);
    let index = match (local.next(), local.next()) {
        (Some((_, index)), None) => *index,
        (None, None) if all.len() == 1 => all[0].1,
        _ => return None,
    };
    items.get(index)
}

pub struct ReadResult {
    pub graph: SourceGraph,
    /// Saved bin ownership for consumers that assemble destination folders.
    pub sequence_bins: HashMap<String, usize>,
    pub group_bins: HashMap<String, usize>,
    /// Exact original bytes pinned before/after stream probing and checked again at read completion.
    pub original_inputs: Vec<av::SourceInput>,
    pub proxy_bindings: Vec<proxy::Binding>,
    /// A saved original-file route was not resolved as an original or native file.
    pub has_unresolved_original_links: bool,
    pub diagnostics: Vec<String>,
    pub audio_group_issues: Vec<String>,
    pub scan_issues: Vec<String>,
    /// Scan diagnostics for ambiguous native packages needed by the selected bin tracks.
    pub ambiguous_managed_packages: Vec<String>,
    pub missing_native_picture: bool,
    pub missing_managed_audio: Vec<String>,
    pub files_scanned: usize,
}

/// The stage at which a controlled source read stopped. Keep the diagnostic
/// text separate so a caller can select a recovery action without parsing it.
pub enum SourceReadIssue {
    Bin(String),
    OriginalLocator(String),
    MissingOriginal(String),
    OriginalMedia(String),
    MediaScan(String),
    SourceFacts(String),
}

impl SourceReadIssue {
    pub fn into_message(self) -> String {
        match self {
            Self::Bin(message)
            | Self::OriginalLocator(message)
            | Self::MissingOriginal(message)
            | Self::OriginalMedia(message)
            | Self::MediaScan(message)
            | Self::SourceFacts(message) => message,
        }
    }
}

impl From<String> for SourceReadIssue {
    fn from(message: String) -> Self {
        Self::OriginalMedia(message)
    }
}

impl From<&str> for SourceReadIssue {
    fn from(message: &str) -> Self {
        Self::OriginalMedia(message.into())
    }
}
pub fn read(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
) -> Result<ReadResult, String> {
    read_with_ffprobe(inputs, media_roots, project, bin_names, std::path::Path::new("ffprobe"))
}

pub fn read_with_ffprobe(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    ffprobe: &std::path::Path,
) -> Result<ReadResult, String> {
    read_managed(inputs, media_roots, project, bin_names, ffprobe, false)
}

pub fn read_managed(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    ffprobe: &Path,
    link_managed_op1a: bool,
) -> Result<ReadResult, String> {
    let facts =
        av::extract_source_paths(inputs, av::SourceLimits::default()).map_err(|e| e.to_string())?;
    resolve_media_and_build(
        &facts,
        media_roots,
        project,
        bin_names,
        original::OriginalIndex::default(),
        &original::OriginalOptions { ffprobe: ffprobe.into(), link_managed_op1a, ..Default::default() },
        &mut || Ok(()),
    )
    .map_err(SourceReadIssue::into_message)
}

/// Original paths are resolved only from saved locators and explicit prefix mappings.
pub fn read_with_originals(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
) -> Result<ReadResult, String> {
    read_with_originals_observed(
        inputs,
        media_roots,
        project,
        bin_names,
        options,
        &mut |_, _| Ok(()),
    )
}
/// Source progress is reported before each native bin open, without retaining decoded nodes.
pub fn read_with_originals_observed(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
    before: &mut dyn FnMut(usize, &std::path::Path) -> Result<(), String>,
) -> Result<ReadResult, String> {
    read_with_originals_controlled(
        inputs,
        media_roots,
        project,
        bin_names,
        options,
        before,
        &mut || Ok(()),
    )
}
/// The optional control is checked during original source hashing and between selected tracks.
pub fn read_with_originals_controlled(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
    before: &mut dyn FnMut(usize, &std::path::Path) -> Result<(), String>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<ReadResult, String> {
    read_with_originals_for_review(inputs, media_roots, project, bin_names, options, before, check)
        .map_err(SourceReadIssue::into_message)
}

pub fn read_with_originals_for_review(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
    before: &mut dyn FnMut(usize, &std::path::Path) -> Result<(), String>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<ReadResult, SourceReadIssue> {
    read_with_media_policy(inputs, media_roots, project, bin_names, options, false, before, check)
}

pub fn read_with_media_policy(
    inputs: &[av::SourceInput], media_roots: &[PathBuf], project: &str, bin_names: &[String],
    options: &original::OriginalOptions, allow_offline: bool,
    before: &mut dyn FnMut(usize, &Path) -> Result<(), String>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<ReadResult, SourceReadIssue> {
    let facts = av::extract_source_paths_observed(inputs, av::SourceLimits::default(), before)
        .map_err(|e| SourceReadIssue::Bin(e.to_string()))?;
    let originals = if allow_offline {
        original::OriginalIndex::scan_allow_offline(&facts, options, check)?
    } else {
        original::OriginalIndex::scan_for_review(&facts, options, check)?
    };
    resolve_media_and_build(
        &facts,
        media_roots,
        project,
        bin_names,
        originals,
        options,
        check,
    )
}

fn resolve_media_and_build(
    facts: &av::SourceProject,
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    mut originals: original::OriginalIndex,
    options: &original::OriginalOptions,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<ReadResult, SourceReadIssue> {
    // Tracks qualified against linked originals do not need native MXF headers.
    let linked_tracks = &originals.tracks;
    let wanted = facts
        .masters
        .iter()
        .flat_map(|m| {
            m.master.tracks.iter().enumerate().filter_map(|(i, t)| {
                (linked_tracks.is_empty() || !linked_tracks.contains_key(&(m.id.clone(), i)))
                    .then(|| t.file_mob.umid_hex())
            })
        })
        .collect();
    let index = index::MediaIndex::scan_controlled(media_roots, &wanted, check)
        .map_err(SourceReadIssue::MediaScan)?;
    if options.attach_avid_proxies {
        originals.select_avid_proxies(facts, options, &index, check)?;
    }
    if options.link_managed_op1a {
        managed_multistream::bind(facts, &index, &mut originals, &options.ffprobe, check)?;
    }
    let motion_mxf =
        verify_motion_mxf(facts, &index, &options.ffprobe, check).map_err(SourceReadIssue::MediaScan)?;
    build_selected(facts, &index, project, bin_names, &originals, &motion_mxf)
        .map_err(SourceReadIssue::SourceFacts)
}

/// Bounded entry point also permits byte-free synthetic source-fact tests.
pub fn build(
    facts: &av::SourceProject,
    index: &index::MediaIndex,
    project: &str,
    bin_names: &[String],
) -> Result<ReadResult, String> {
    build_selected(
        facts,
        index,
        project,
        bin_names,
        &original::OriginalIndex::default(),
        &MotionMxf::default(),
    )
}

#[derive(Default)]
struct MotionMxf<'a> {
    picture: HashMap<&'a std::path::Path, Option<(Rational, Rational)>>,
    pcm16: HashMap<&'a std::path::Path, Rational>,
}

fn verify_motion_mxf<'a>(
    facts: &av::SourceProject,
    index: &'a index::MediaIndex,
    ffprobe: &std::path::Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MotionMxf<'a>, String> {
    let masters: HashSet<_> = facts.sequences.iter().flat_map(|seq| &seq.events)
        .filter(|e| e.saved_timewarp.is_some()
            || e.event.picture.as_ref().is_some_and(|p| p.corner_pin.is_some())
            || e.picture_extension.is_some_and(av::PictureExtension::requires_square_pixels)
            || e.picture_chain.iter().any(|stage| match stage {
                av::PictureStage::Extension(effect) => effect.requires_square_pixels(),
                av::PictureStage::Static(effect) => effect.corner_pin.is_some(),
            }))
        .map(|e| e.event.master).collect();
    let mut probed = HashSet::new();
    let mut verified = MotionMxf::default();
    for master in facts.masters.iter().filter(|m| masters.contains(&m.master.mob_id)) {
        for track in master.master.tracks.iter().filter(|t| t.kind == KIND_PICTURE) {
            let Some(path) = index.files.get(&track.file_mob.umid_hex()).map(|p| p.as_path()) else { continue; };
            if probed.insert(path) {
                check()?;
                if let Some(clock) = original::square_1080p_mxf(path, ffprobe) {
                    verified.picture.insert(path, clock);
                }
            }
        }
    }
    let managed_audio: HashSet<_> = facts.sequences.iter().flat_map(|seq| &seq.events)
        .filter(|e| e.event.kind == KIND_SOUND && e.motion_adapter_phase.is_some())
        .map(|e| e.event.master).collect();
    for master in facts.masters.iter().filter(|m| managed_audio.contains(&m.master.mob_id)) {
        let [track] = master.master.tracks.as_slice() else { continue };
        if master.rate != Rate::new(24, 1) || track.kind != KIND_SOUND || track.index != 1
            || !matches!(&track.essence, Some(Essence::Sound {
                sample_rate: 48_000.0, bits: 16, channels: 1,
            }))
            || master.offsets.as_slice() != [Observed::Known(0)]
        {
            continue;
        }
        let package = track.file_mob.umid_hex();
        let Some(path) = index.files.get(&package).map(|p| p.as_path()) else { continue };
        if !probed.insert(path) { continue; }
        let id = index.identity(path)?;
        if id.file_packages != [package] || id.file_essence_tracks != 1
            || id.file_tracks != [(48_000, 1, track.length)]
            || !id.qualified_file_clock().is_some_and(|clock|
                clock.rate == (24, 1) && !clock.drop && clock.source_offset == 0)
        {
            continue;
        }
        check()?;
        if let Ok(probe) = original::probe_offline_atom(path, ffprobe) {
            let ul = probe.get("format").and_then(|f| f.get("tags"))
                .and_then(|tags| original::text(tags, "operational_pattern_ul"));
            let Some(Value::Arr(streams)) = probe.get("streams") else { continue };
            let [pcm] = streams.as_slice() else { continue };
            if ul.as_deref() == Some("060e2b34.04010102.0d010201.10010000")
                && original::text(pcm, "codec_type").as_deref() == Some("audio")
                && original::text(pcm, "codec_name").as_deref() == Some("pcm_s16le")
                && original::text(pcm, "time_base").as_deref() == Some("1/48000")
                && original::number(pcm, "channels") == Some(1)
                && original::number(pcm, "start_pts") == Some(0)
                && original::number(pcm, "duration_ts") == Some(track.length)
                && original::complete_mono_pcm16_packets(path, ffprobe, track.length)
            {
                let extent = Rational::new(i128::from(track.length), 48_000)
                    .ok_or("managed Motion Adapter duration overflow")?;
                verified.pcm16.insert(path, extent);
            }
        }
    }
    Ok(verified)
}

fn build_selected(
    facts: &av::SourceProject,
    index: &index::MediaIndex,
    project: &str,
    bin_names: &[String],
    originals: &original::OriginalIndex,
    motion_mxf: &MotionMxf,
) -> Result<ReadResult, String> {
    validate_rates(facts)?;
    if facts.accounted_bytes > av::MAX_SOURCE_FACT_BYTES {
        return Err("source facts exceed 64 MiB".into());
    }
    let mut budget = Budget::default();
    budget.charge(size_of::<SourceGraph>() + project.len())?;
    // Report strings are owned too; reserve escaped names before formatting.
    for skipped in &facts.skipped {
        budget.charge(
            skipped
                .name
                .len()
                .checked_mul(10)
                .and_then(|n| n.checked_add(skipped.reason.len() + 64))
                .ok_or("diagnostic budget overflow")?,
        )?;
    }
    for input in &originals.original_inputs {
        budget.charge(
            size_of::<av::SourceInput>() + input.path.as_os_str().len() + input.revision.len(),
        )?;
    }
    for note in &originals.notes {
        budget.charge(size_of::<String>() + note.len())?;
    }
    let mut out = ReadResult {
        sequence_bins: HashMap::new(),
        group_bins: HashMap::new(),
        original_inputs: originals.original_inputs.clone(),
        proxy_bindings: Vec::new(),
        has_unresolved_original_links: facts.masters.iter().any(|source| {
            source.master.tracks.iter().zip(&source.originals).enumerate().any(|(i, (track, original))| {
                original.is_some()
                    && !index.files.contains_key(&track.file_mob.umid_hex())
                    && !originals.tracks.contains_key(&(source.id.clone(), i))
            })
        }),
        graph: SourceGraph {
            multicams: Vec::new(),
            multicam_uses: Vec::new(),
            project: Project {
                name: project.into(),
            },
            media: vec![],
            items: vec![],
            bins: vec![],
            sequences: vec![],
            losses: vec![],
            sidecars: vec![],
        },
        diagnostics: facts
            .skipped
            .iter()
            .map(|s| format!("{:?}: skipped ({})", s.name, s.reason))
            .chain(originals.notes.iter().cloned())
            .collect(),
        audio_group_issues: vec![],
        scan_issues: vec![],
        ambiguous_managed_packages: vec![],
        missing_native_picture: false,
        missing_managed_audio: vec![],
        files_scanned: index.files_scanned,
    };
    for (i, name) in bin_names.iter().enumerate() {
        budget.charge(size_of::<Bin>() + name.len() + 32)?;
        out.graph.bins.push(Bin {
            id: format!("bin:{i}"),
            name: name.clone(),
            parent: None,
            items: vec![],
        });
    }
    subclips::append(&facts.subclips, &mut out.graph, &mut budget)?;
    for still in &facts.imported_stills {
        budget.charge(still.id.len() + 2048)?;
        for path in &still.import.locators {
            budget.charge(path.len() + 64)?;
        }
        let int_or_null = |value: Option<i64>| value.map(Value::Int).unwrap_or(Value::Null);
        let flag = |value: Option<bool>| value.map(Value::Bool).unwrap_or(Value::Null);
        let data = obj(vec![
            ("id", s(&still.id)),
            ("mob", s(still.master.urn())),
            ("track", Value::Int(still.track)),
            ("file_mob", s(still.file_mob.urn())),
            ("file_track", Value::Int(still.file_track)),
            ("rate", still.rate.map(|rate| s(rational(rate))).unwrap_or(Value::Null)),
            ("master_length", int_or_null(still.master_length)),
            ("file_in", int_or_null(still.file_in)),
            ("source_length", int_or_null(still.source_length)),
            ("import_aspect", int_or_null(still.import_aspect)),
            ("color_level", int_or_null(still.color_level)),
            ("ignore_alpha", flag(still.import.ignore_alpha)),
            ("invert_alpha", flag(still.import.invert_alpha)),
            ("saved_locators", Value::Arr(still.import.locators.iter().map(s).collect())),
        ]);
        append_bin_receipt(&mut out, &mut budget, still.bin, "imported_stills", data)?;
    }
    for (name, bin) in &facts.duplicates {
        out.diagnostics.push(format!(
            "{name:?} is also in bin {:?}; a copy is imported there with that bin's attributes",
            bin_names
                .get(*bin)
                .ok_or("duplicate references unknown bin")?
        ));
    }
    if !index.ambiguous.is_empty() {
        let ambiguous: HashSet<_> = index.ambiguous.iter().map(String::as_str).collect();
        let mut required = HashSet::new();
        for source in &facts.masters {
            for (track, saved) in source.master.tracks.iter().enumerate() {
                let package = saved.file_mob.umid_hex();
                if ambiguous.contains(package.as_str())
                    && !originals.tracks.contains_key(&(source.id.clone(), track))
                {
                    required.insert(package);
                }
            }
        }
        for uid in &index.ambiguous {
            let issue = format!("ambiguous MXF file package {uid}");
            if required.contains(uid) {
                out.ambiguous_managed_packages.push(issue.clone());
            }
            out.scan_issues.push(issue);
        }
    }
    for (p, e) in &index.unreadable {
        out.scan_issues.push(format!("{}: {e}", p.display()));
    }
    let mut copies: HashMap<avid_reader::io::MobId, Vec<(usize, usize)>> = HashMap::new();
    let mut source_masters = Vec::new();
    for (n, source) in facts.masters.iter().enumerate() {
        let id = format!("clip:clip-{}", n + 1);
        let checkpoint = (
            out.graph.media.len(),
            out.graph.sidecars.len(),
            out.graph.losses.len(),
            budget.used,
        );
        let missing_audio_before = out.missing_managed_audio.len();
        match master(source, index, &id, &mut out, &mut budget, originals, motion_mxf) {
            Ok(item) => {
                let bin = out
                    .graph
                    .bins
                    .get_mut(source.master.bin)
                    .ok_or("master references unknown bin")?;
                budget.charge(size_of::<String>() + id.len())?;
                bin.items.push(id);
                copies
                    .entry(source.master.mob_id)
                    .or_default()
                    .push((source.master.bin, out.graph.items.len()));
                out.graph.items.push(item);
                budget.charge(2 * size_of::<&av::SourceMaster>())?;
                source_masters.push(source);
            }
            Err(e) => {
                let e = match e {
                    MasterError::Unsupported(e) => e,
                    MasterError::MissingPicture(e) => {
                        out.missing_native_picture = true;
                        e
                    }
                    MasterError::Budget(e) => return Err(e),
                };
                out.graph.media.truncate(checkpoint.0);
                out.graph.sidecars.truncate(checkpoint.1);
                out.graph.losses.truncate(checkpoint.2);
                budget.used = checkpoint.3;
                for message in &out.missing_managed_audio[missing_audio_before..] {
                    budget.charge(message.capacity() + size_of::<String>())?;
                }
                retain_unresolved_master(source, &e, &mut out, &mut budget)?;
                out.diagnostics
                    .push(format!("{:?}: skipped ({e})", source.master.name));
            }
        }
    }
    let multicams = multicam::append(&facts.groups, &copies, &mut out, &mut budget)?;
    for group in &facts.groups {
        budget.charge(size_of::<(String, usize)>() + group.id.len() + 32)?;
        if out.group_bins.insert(group.id.clone(), group.bin).is_some() {
            return Err("duplicate saved Avid Group identity".into());
        }
    }
    let media_by_id: HashMap<_, _> = out.graph.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let item_routes: Vec<_> = out
        .graph
        .items
        .iter()
        .enumerate()
        .map(|(index, i)| {
            let mut audio = HashSet::new();
            let mut raster = None;
            let mut motion_picture = None;
            let mut square_pixels = false;
            let mut timewarp_picture = None;
            let mut motion_audio = HashMap::new();
            let mut managed_motion_audio = None;
            let mut same_rate_aac = HashMap::new();
            for im in &i.media {
                let m = media_by_id[im.media.as_str()];
                if m.kind == StreamKind::Audio {
                    if let Some(n) = m.stream {
                        audio.insert(n);
                        if m.online == Some(true) && m.original.is_none() && n == 1
                            && i.media.len() == 1 && i.stream_groups.is_empty()
                            && im.source_offset == Some(Rational::ZERO)
                            && m.bits == Some(16) && m.sample_rate == Some(48_000)
                            && m.locations.first().and_then(|p| motion_mxf.pcm16.get(std::path::Path::new(p)))
                                == Some(&m.duration)
                        {
                            managed_motion_audio = Some(m.duration);
                        }
                    }
                } else {
                    raster = m.frame_size;
                    let verified = m.locations.first()
                        .and_then(|p| motion_mxf.picture.get(std::path::Path::new(p)));
                    square_pixels = m.original.is_some() || verified.is_some();
                    if m.original.is_none() && raster == Some((1920, 1080))
                        && im.source_offset == Some(Rational::ZERO)
                    {
                        timewarp_picture = verified.copied().flatten()
                            .filter(|(rate, _)| Some(*rate) == m.rate)
                            .map(|(_, duration)| duration.min(m.duration));
                    }
                    if m.original.is_some() && im.source_offset == Some(Rational::ZERO)
                        && matches!(m.rate.map(|r| (r.num(),r.den())), Some((24,1) | (24_000,1001)))
                    {
                        motion_picture = Some(m.duration);
                    }
                }
            }
            if let Some(master) = source_masters.get(index) {
                for (ordinal, track) in master.master.tracks.iter().enumerate() {
                    if let Some(Essence::Picture { display_aspect: Some((num, den)), .. }) = &track.essence {
                        square_pixels &= raster.is_some_and(|(width, height)|
                            i128::from(*num) * i128::from(height) == i128::from(*den) * i128::from(width));
                    }
                    if let Some(selected) = originals.tracks.get(&(master.id.clone(), ordinal)) {
                        if selected.same_rate_aac_origin {
                            same_rate_aac.insert(track.index, selected.duration);
                        }
                        if let Some(origin) = selected.motion_audio_origin {
                            if matches!(master.offsets.get(ordinal), Some(Observed::Known(0))) {
                                motion_audio.insert(track.index, (origin, selected.duration));
                            }
                        }
                    }
                }
            }
            sequence::ItemRoutes {
                audio, raster, square_pixels, motion_picture, motion_audio, managed_motion_audio, same_rate_aac,
                timewarp_picture: timewarp_picture.filter(|_| square_pixels),
            }
        })
        .collect();
    for seq in &facts.sequences {
        let before = out.graph.sequences.len();
        sequence::append(seq, &copies, &item_routes, &multicams, &mut out, &mut budget)?;
        if out.graph.sequences.len() > before {
            budget.charge(size_of::<(String, usize)>() + seq.id.len() + 32)?;
            if out.sequence_bins.insert(seq.id.clone(), seq.info.bin).is_some() {
                return Err("duplicate saved Avid sequence identity".into());
            }
        }
    }
    out.graph.validate().map_err(|e| e.to_string())?;
    out.graph.encoded_len().map_err(|e| e.to_string())?;
    Ok(out)
}

#[derive(Default)]
struct Budget {
    used: usize,
}
impl Budget {
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.used = self
            .used
            .checked_add(n)
            .filter(|n| *n <= av::MAX_SOURCE_FACT_BYTES)
            .ok_or("source graph construction exceeds 64 MiB accounted budget")?;
        Ok(())
    }
    fn value(&mut self, v: &Value) -> Result<(), String> {
        self.charge(size_of::<Value>())?;
        self.value_payload(v)
    }
    fn value_payload(&mut self, v: &Value) -> Result<(), String> {
        match v {
            Value::Str(s) => self.charge(s.capacity()),
            Value::Arr(a) => {
                self.charge(a.capacity() * size_of::<Value>())?;
                for x in a {
                    self.value_payload(x)?;
                }
                Ok(())
            }
            Value::Obj(a) => {
                self.charge(a.capacity() * size_of::<(String, Value)>())?;
                for (k, v) in a {
                    self.charge(k.capacity())?;
                    self.value_payload(v)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
fn validate_rates(facts: &av::SourceProject) -> Result<(), String> {
    let rate = |r: Rate| {
        if Rate::new(r.num, r.den) == Some(r) {
            Ok(())
        } else {
            Err("malformed source rate witness".to_string())
        }
    };
    let route = |v: &[av::RouteWitness]| {
        for w in v {
            if let Some(r) = w.rate {
                rate(r)?;
            }
        }
        Ok::<_, String>(())
    };
    for m in &facts.masters {
        if let Some(r) = m.rate {
            rate(r)?;
        }
        for w in &m.marker_witnesses {
            if let Some(r) = w.local_rate {
                rate(r)?;
            }
        }
    }
    for subclip in &facts.subclips {
        for r in subclip.rate.into_iter().chain(subclip.tracks.iter().filter_map(|t| t.rate)) {
            rate(r)?;
        }
    }
    for g in &facts.groups {
        if let Some(r)=g.rate{rate(r)?;}
        for l in &g.lanes {
            if let Some(r)=l.rate{rate(r)?;}
            for a in &l.alternatives {
                if let Some(r)=a.endpoint.rate{rate(r)?;}
                for p in &a.pieces{if let av::SourceGroupPiece::Media{rate:r,..}=p{rate(*r)?;}}
            }
        }
    }
    for s in &facts.sequences {
        for u in &s.group_uses{
            if let Some(r)=u.rate{rate(r)?;}
            for e in &u.alternatives{if let Some(r)=e.rate{rate(r)?;}}
        }
        if let Some(r) = s.rate {
            rate(r)?;
        }
        for e in &s.events {
            if let Some(q) = &e.range {
                rate(q.record_rate)?;
                rate(q.source_rate)?;
                route(&q.route)?;
            }
        }
        for t in &s.transitions {
            if let Some(r) = t.rate {
                rate(r)?;
            }
        }
        for m in &s.markers {
            if let Some(r) = m.local_rate {
                rate(r)?;
            }
        }
        for i in &s.unresolved {
            if let Some(r) = i.record_rate {
                rate(r)?;
            }
            route(&i.route)?;
        }
    }
    Ok(())
}
fn reserve_marker(m: &AvidMarker, budget: &mut Budget) -> Result<(), String> {
    // The typed marker and raw receipt each own these texts; pre-charge both copies.
    for text in [&m.name, &m.comment, &m.user, &m.color, &m.id] {
        budget.charge(
            text.len()
                .checked_mul(2)
                .ok_or("marker text budget overflow")?,
        )?;
    }
    if let Some(t) = &m.track_attr {
        budget.charge(t.len())?;
    }
    budget.charge(2048)
}
fn s(v: impl ToString) -> Value {
    Value::Str(v.to_string())
}
fn raw_float(v: f64) -> Value {
    if v.is_finite() {
        Value::Float(v)
    } else {
        obj(vec![("f64_bits", s(format!("{:016X}", v.to_bits())))])
    }
}
fn obj(v: Vec<(&str, Value)>) -> Value {
    Value::Obj(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn opt_s(v: Option<&str>) -> Value {
    v.map(s).unwrap_or(Value::Null)
}
// Preserve type and bytes, without resolving an opaque bin-local reference or
// interpreting an unknown control as a default. Preflight before allocating hex.
fn output_mix(m: &extract::MasterOutputMix, budget: &mut Budget) -> Result<Value, String> {
    use extract::MasterOutputMix as M;
    let payload = match m {
        M::Text(v) => v.len(),
        M::Bytes(v) => v.len().checked_mul(2).ok_or("mix payload overflow")?,
        _ => 0,
    };
    let mut reservation = Budget { used: budget.used };
    reservation.charge(payload.checked_add(512).ok_or("mix payload overflow")?)?;
    let (kind, raw) = match m {
        M::Absent => ("absent", Value::Null),
        M::Integer(v) => ("integer", Value::Int(*v)),
        M::Text(v) => ("text", s(v)),
        M::Bytes(v) => {
            use std::fmt::Write;
            let mut hex = String::with_capacity(payload);
            for byte in v {
                write!(&mut hex, "{byte:02x}").unwrap();
            }
            ("bytes_hex", Value::Str(hex))
        }
        M::Reference(v) => ("bin_object_ref", Value::Int(i64::from(*v))),
        M::Null => ("null", Value::Null),
    };
    Ok(obj(vec![("type", s(kind)), ("raw", raw)]))
}
fn rational(r: Rate) -> Rational {
    Rational::new(r.num as i128, r.den as i128).expect("positive reduced i64 source rate")
}
fn source_input_transforms(
    source: &av::SourceMaster,
    budget: &Budget,
) -> Result<Option<Value>, String> {
    if source.input_transforms.is_empty() {
        return Ok(None);
    }
    let mut reservation = Budget { used: budget.used };
    for transform in &source.input_transforms {
        let text = match &transform.value {
            Observed::Known(text) | Observed::Unresolved(text) => text,
        };
        reservation.charge(text.len() + 512)?;
    }
    Ok(Some(Value::Arr(source.input_transforms.iter().map(|transform| {
        let (key, text) = match &transform.value {
            Observed::Known(text) => ("text", text),
            Observed::Unresolved(reason) => ("reason", reason),
        };
        obj(vec![
            ("track", Value::Int(transform.track)),
            ("owner_mob", s(transform.file_mob.urn())),
            (key, s(text)),
        ])
    }).collect())))
}
fn time(n: i64, r: Rate) -> Result<Rational, String> {
    Rational::new(n as i128 * r.den as i128, r.num as i128)
        .ok_or("source time overflows rational".into())
}
fn kind(k: i64) -> Option<StreamKind> {
    match k {
        KIND_PICTURE => Some(StreamKind::Video),
        KIND_SOUND => Some(StreamKind::Audio),
        _ => None,
    }
}
fn observed(v: &Observed<i64>) -> Value {
    match v {
        Observed::Known(n) => obj(vec![("value", Value::Int(*n))]),
        Observed::Unresolved(r) => obj(vec![("reason", s(r))]),
    }
}
fn loss(
    out: &mut ReadResult,
    budget: &mut Budget,
    id: &str,
    aspect: &str,
    reason: impl Into<String>,
) -> Result<(), String> {
    let reason = reason.into();
    budget.charge(size_of::<Loss>() + id.len() + aspect.len() + reason.len())?;
    out.graph.losses.push(Loss {
        object: id.into(),
        aspect: aspect.into(),
        reason,
    });
    Ok(())
}
fn sidecar(
    out: &mut ReadResult,
    budget: &mut Budget,
    adapter: &str,
    id: &str,
    data: Value,
) -> Result<(), String> {
    budget.charge(size_of::<Sidecar>() + adapter.len() + id.len())?;
    budget.value(&data)?;
    out.graph.sidecars.push(Sidecar {
        adapter: adapter.into(),
        object: id.into(),
        data,
    });
    Ok(())
}
fn append_bin_receipt(
    out: &mut ReadResult,
    budget: &mut Budget,
    bin: usize,
    key: &str,
    data: Value,
) -> Result<(), String> {
    let id = out
        .graph
        .bins
        .get(bin)
        .ok_or("native fact references unknown bin")?
        .id
        .clone();
    if let Some(existing) = out
        .graph
        .sidecars
        .iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == id)
    {
        budget.value(&data)?;
        budget.charge(size_of::<Value>())?;
        let Value::Obj(fields) = &mut existing.data else {
            return Err("invalid bin source sidecar".into());
        };
        if let Some((_, v)) = fields.iter_mut().find(|(k, _)| k == key) {
            let Value::Arr(a) = v else {
                return Err("invalid bin receipt list".into());
            };
            a.push(data);
        } else {
            budget.charge(key.len() + size_of::<(String, Value)>())?;
            fields.push((key.into(), Value::Arr(vec![data])));
        }
        Ok(())
    } else {
        sidecar(
            out,
            budget,
            "avid.source",
            &id,
            obj(vec![(key, Value::Arr(vec![data]))]),
        )
    }
}
fn retain_unresolved_master(
    source: &av::SourceMaster,
    reason: &str,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<(), String> {
    let m = &source.master;
    // Guard all potentially large cloned payloads before building an opaque receipt.
    let mut reservation = Budget { used: budget.used };
    reservation.charge(
        m.name.len()
            + source.id.len()
            + m.tracks.len() * 2048
            + m.other_groups.iter().map(|s| s.len()).sum::<usize>(),
    )?;
    for (k, v) in &m.columns {
        reservation.charge(k.len() + v.len() + size_of::<(String, Value)>())?;
    }
    for (_, _, mk) in &m.markers {
        reserve_marker(mk, &mut reservation)?;
    }
    let markers = m
        .markers
        .iter()
        .enumerate()
        .map(|(i, (_, _, mk))| {
            let mut native = master_marker_native(source, i, mk);
            if let Value::Obj(fields) = &mut native {
                fields.extend([
                    ("name".into(), s(&mk.name)),
                    ("comment".into(), s(&mk.comment)),
                    ("user".into(), s(&mk.user)),
                    ("color".into(), s(&mk.color)),
                    ("graph_index".into(), Value::Null),
                ]);
            }
            native
        })
        .collect();
    let mix = output_mix(&m.output_mix, budget)?;
    let mut raw = vec![
        ("occurrence", s(&source.id)),
        ("name", s(&m.name)),
        ("mob_id", s(m.mob_id.urn())),
        ("reason", s(reason)),
        ("native_rate", raw_float(m.edit_rate)),
        (
            "qualified_rate",
            source.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
        ),
        ("length", Value::Int(m.length)),
        (
            "color_rgb16",
            m.color
                .map(|c| Value::Arr(c.into_iter().map(Value::Int).collect()))
                .unwrap_or(Value::Null),
        ),
        (
            "start",
            m.start
                .as_ref()
                .map(|t| {
                    obj(vec![
                        ("count", Value::Int(t.frames)),
                        ("rounded_base", Value::Int(t.fps)),
                        ("drop", Value::Bool(t.drop)),
                    ])
                })
                .unwrap_or(Value::Null),
        ),
        (
            "aux_timecodes",
            Value::Arr(
                m.aux_timecodes
                    .iter()
                    .map(|a| {
                        obj(vec![
                            ("track_index", Value::Int(a.track_index)),
                            ("position", Value::Int(a.position)),
                            ("length", Value::Int(a.length)),
                            ("count", Value::Int(a.start)),
                            ("rounded_base", Value::Int(a.fps)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "columns",
            Value::Arr(
                m.columns
                    .iter()
                    .map(|(k, v)| Value::Arr(vec![s(k), s(v)]))
                    .collect(),
            ),
        ),
        ("source_file", opt_s(m.source_file.as_deref())),
        ("source_path", opt_s(m.source_path.as_deref())),
        (
            "tracks",
            Value::Arr(
                m.tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        obj(vec![
                            ("kind", Value::Int(t.kind)),
                            ("index", Value::Int(t.index)),
                            ("file_package", s(t.file_mob.umid_hex())),
                            ("native_rate", raw_float(t.edit_rate)),
                            ("length", Value::Int(t.length)),
                            ("clip_length", Value::Int(t.clip_length)),
                            (
                                "offset",
                                source.offsets.get(i).map(observed).unwrap_or(Value::Null),
                            ),
                            ("gain_db", t.gain_db.map(raw_float).unwrap_or(Value::Null)),
                            ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
                            ("descriptor", descriptor(&t.essence)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "stereo_pairs",
            Value::Arr(
                m.stereo_pairs
                    .iter()
                    .map(|(a, b)| Value::Arr(vec![Value::Int(*a), Value::Int(*b)]))
                    .collect(),
            ),
        ),
        (
            "raw_groups",
            Value::Arr(m.other_groups.iter().map(s).collect()),
        ),
        ("marker_native", Value::Arr(markers)),
        ("master_output_mix", mix),
    ];
    if let Some(transforms) = source_input_transforms(source, budget)? {
        raw.push(("input_color_transforms", transforms));
    }
    if let Some(managed) = &source.managed_multistream {
        raw.push(("managed_multistream", managed_multistream::saved_facts(managed, budget)?));
    }
    append_bin_receipt(out, budget, m.bin, "unresolved_masters", obj(raw))?;
    loss(
        out,
        budget,
        &format!("bin:{}", m.bin),
        "source_master",
        format!(
            "{}: {reason}; native master facts retained on bin",
            source.id
        ),
    )
}
fn clock(v: Option<(u64, u16, bool)>) -> Value {
    v.map(|(n, b, d)| {
        obj(vec![
            ("count", s(n)),
            ("rounded_base", Value::Int(b as i64)),
            ("drop", Value::Bool(d)),
        ])
    })
    .unwrap_or(Value::Null)
}
fn tc(n: i64, fps: i64) -> String {
    if fps <= 0 {
        return String::new();
    }
    let f = (n as i128).rem_euclid(86400i128 * fps as i128);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        f / (3600 * fps as i128),
        f / (60 * fps as i128) % 60,
        f / fps as i128 % 60,
        f % fps as i128
    )
}
fn master_start_timecode(start: &extract::Timecode) -> Option<String> {
    if !start.drop {
        return Some(tc(start.frames, start.fps));
    }
    let dropped = match start.fps {
        30 => 2,
        60 => 4,
        _ => return None,
    };
    let per_minute = start.fps * 60 - dropped;
    let per_ten_minutes = per_minute * 10 + dropped;
    let count = start.frames.rem_euclid(per_ten_minutes * 6 * 24);
    let omitted = dropped * (9 * (count / per_ten_minutes)
        + ((count % per_ten_minutes - dropped).max(0) / per_minute));
    let mut label = tc(count + omitted, start.fps);
    label.replace_range(8..9, ";");
    Some(label)
}
fn marker_native(m: &AvidMarker) -> Value {
    obj(vec![
        ("id", s(&m.id)),
        ("frame", Value::Int(m.frame)),
        ("length", m.length.map(Value::Int).unwrap_or(Value::Null)),
        ("created", m.created.map(Value::Int).unwrap_or(Value::Null)),
        (
            "rgb16",
            Value::Arr(m.rgb16.into_iter().map(Value::Int).collect()),
        ),
        ("track_attr", opt_s(m.track_attr.as_deref())),
    ])
}
fn master_marker_native(source: &av::SourceMaster, index: usize, m: &AvidMarker) -> Value {
    let mut v = marker_native(m);
    if let Value::Obj(fields) = &mut v {
        if let Some(w) = source.marker_witnesses.get(index) {
            fields.extend([
                ("owner".into(), s(&w.owner)),
                ("kind".into(), Value::Int(w.kind)),
                ("track".into(), Value::Int(w.track)),
                ("local_offset".into(), observed(&w.local_offset)),
                (
                    "local_rate".into(),
                    w.local_rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                ),
                (
                    "master_frame".into(),
                    w.master_frame.map(Value::Int).unwrap_or(Value::Null),
                ),
            ]);
        }
    }
    v
}
fn marker(
    m: &AvidMarker,
    start: Option<Rational>,
    local: Option<Rational>,
    rate: Option<Rate>,
    owner: String,
    owner_kind: MarkerOwner,
    track: Option<TrackRef>,
    mut binding: SourceBinding,
) -> Result<SourceMarker, String> {
    if start.is_none() && local.is_none() && binding.reason.is_none() {
        binding.reason = Some("marker local and record clocks are unresolved".into());
    }
    Ok(SourceMarker {
        name: m.name.clone(),
        comment: m.comment.clone(),
        user: m.user.clone(),
        color: (!m.color.is_empty()).then(|| Label {
            palette: "avid".into(),
            name: m.color.clone(),
        }),
        start,
        duration: match (m.length, rate) {
            (Some(n), Some(r)) => time(n, r)?,
            _ => Rational::ZERO,
        },
        track,
        owner: owner_kind,
        owner_occurrence: owner,
        owner_local: local,
        owner_rate: rate.map(rational),
        binding,
        source: None,
        inherited: false,
    })
}
fn descriptor(e: &Option<Essence>) -> Value {
    match e {
        Some(Essence::Picture {
            compression,
            resolution_id,
            width,
            height,
            display_aspect,
        }) => obj(vec![
            ("compression", s(compression)),
            ("resolution_id", Value::Int(*resolution_id)),
            ("width", Value::Int(*width)),
            ("height", Value::Int(*height)),
            (
                "display_aspect",
                display_aspect
                    .map(|(n, d)| Value::Arr(vec![Value::Int(n), Value::Int(d)]))
                    .unwrap_or(Value::Null),
            ),
        ]),
        Some(Essence::Sound {
            sample_rate,
            bits,
            channels,
        }) => obj(vec![
            ("sample_rate", Value::Float(*sample_rate)),
            ("bits", Value::Int(*bits)),
            ("channels", Value::Int(*channels)),
        ]),
        _ => Value::Null,
    }
}
enum MasterError {
    Unsupported(String),
    MissingPicture(String),
    Budget(String),
}
impl From<&str> for MasterError {
    fn from(e: &str) -> Self {
        Self::Unsupported(e.into())
    }
}
impl From<String> for MasterError {
    fn from(e: String) -> Self {
        Self::Unsupported(e)
    }
}
fn offline_managed(
    source: &av::SourceMaster, index: usize, item: &str, audio_index: usize,
    video: &mut bool, labels: &mut HashSet<i64>, out: &mut ReadResult, budget: &mut Budget,
) -> Result<SourceMedia, MasterError> {
    let track = &source.master.tracks[index];
    let rate = Rate::from_avid(track.edit_rate).ok_or("offline descriptor rate is unresolved".to_string())?;
    let clock = source.file_clocks.get(index).and_then(Option::as_ref)
        .filter(|c| (c.rate.num, c.rate.den, c.base, c.drop) == (24_000, 1001, 24, false))
        .ok_or_else(|| format!("offline managed track {} requires a saved 23.976 non-drop file clock", track.index))?;
    if source.offsets.get(index) != Some(&Observed::Known(0))
        || source.rendered_freeze || source.rendered_gain.is_some()
        || track.length <= 0 || clock.file_track != track.index
    {
        return Err("offline managed media has an unqualified offset, render or track selector".to_string().into());
    }
    let (id, kind, codec, size, sample_rate, bits, stream) = match &track.essence {
        Some(Essence::Picture { compression, width: 1920, height: 1080,
            display_aspect: Some((16, 9)), resolution_id: 0 })
            if track.kind == KIND_PICTURE && compression == "RI00" && rate == clock.rate && !*video =>
        {
            *video = true;
            (format!("{item}:video"), StreamKind::Video, Some("AVdn".into()), Some((1920,1080)), None, None, None)
        }
        Some(Essence::Sound { sample_rate: 48000.0, bits: 24, channels: 1 })
            if track.kind == KIND_SOUND && rate == Rate::new(48_000,1).unwrap() && matches!(track.index, 1 | 2) =>
        {
            labels.insert(track.index);
            (format!("{item}:audio:{audio_index}"), StreamKind::Audio, None, None, Some(48000), Some(24), Some(track.index))
        }
        _ => return Err("offline managed descriptor is outside the 1080p23.976 DNx/PCM24 profile".to_string().into()),
    };
    let package = track.file_mob.umid_hex();
    let suffix = if kind == StreamKind::Video { "V1".to_owned() } else { format!("A{}", track.index) };
    let path = format!("/AnyNLE-Offline/{package}/{suffix}-{package}.mxf");
    let effective = clock.count.checked_add(clock.offset).ok_or("offline timecode overflow".to_string())?;
    let local_offset = time(clock.offset, clock.rate)?.to_units(rate.num)
        .filter(|n| n % rate.den == 0).map(|n| n / rate.den)
        .ok_or("offline physical timecode offset is off the source clock".to_string())?;
    let source_clock = SourceClock {
        effective_count: effective as u64, rate: rational(clock.rate), display_base: 24, drop: false,
        provenance: SourceClockProvenance {
            origin_count: clock.count as u64, source_offset: local_offset, source_rate: rational(rate),
            source_package: clock.package.umid_hex(), timecode_track: "TC1".into(),
            route: "avid_saved_file_to_physical_timecode".into(),
        },
    };
    sidecar(out, budget, "avid.source", &id, obj(vec![
        ("legacy_clock", self::clock(Some((clock.count as u64, 24, false)))),
        ("file_package", s(package)), ("file_track", Value::Int(clock.file_track)),
        ("descriptor_length", Value::Int(track.length)), ("descriptor", descriptor(&track.essence)),
        ("native_track", Value::Int(track.index)),
    ])).map_err(MasterError::Budget)?;
    budget.charge(size_of::<SourceMedia>() + size_of::<SourceItemMedia>() + id.len()*2 + path.len() + 512)
        .map_err(MasterError::Budget)?;
    Ok(SourceMedia {
        id, kind, codec, frame_size: size, sample_rate, bits, stream,
        locations: vec![path], rate: (kind == StreamKind::Video).then(|| rational(rate)),
        duration: time(track.length, rate)?, start: source_clock.exact_start(), clock: Some(source_clock),
        online: Some(false), original: None,
    })
}

fn master(
    source: &av::SourceMaster,
    index: &index::MediaIndex,
    id: &str,
    out: &mut ReadResult,
    budget: &mut Budget,
    originals: &original::OriginalIndex,
    motion_mxf: &MotionMxf,
) -> Result<SourceItem, MasterError> {
    let m = &source.master;
    budget
        .charge(size_of::<SourceItem>() + id.len() + m.name.len() + source.id.len())
        .map_err(MasterError::Budget)?;
    for (k, v) in &m.columns {
        budget
            .charge(k.len() + v.len() + size_of::<(String, Value)>())
            .map_err(MasterError::Budget)?;
    }
    for text in m
        .source_file
        .iter()
        .chain(m.source_path.iter())
        .chain(m.other_groups.iter())
    {
        budget.charge(text.len()).map_err(MasterError::Budget)?;
    }
    for (_, _, mk) in &m.markers {
        reserve_marker(mk, budget).map_err(MasterError::Budget)?;
    }
    let mut item = SourceItem {
        id: id.into(),
        name: m.name.clone(),
        media: vec![],
        umid: Some(m.mob_id.umid_hex()),
        tape: m
            .columns
            .iter()
            .find(|(k, _)| k == "Tape")
            .map(|(_, v)| v.clone()),
        rate: source.rate.map(rational),
        color_rgb16: m.color,
        stream_groups: vec![],
        markers: vec![],
    };
    let mut video = false;
    let mut labels = HashSet::new();
    if let Some(proxy) = originals.proxies.get(&source.id) {
        sidecar(out, budget, proxy::ADAPTER, id, proxy.fact()).map_err(MasterError::Budget)?;
        out.proxy_bindings.push(proxy.clone());
    }
    let mut raw_tracks = vec![];
    let mut audio_ordinal = 0;
    for (tindex, t) in m.tracks.iter().enumerate() {
        let audio_index = audio_ordinal;
        if t.kind == KIND_SOUND {
            audio_ordinal += 1;
        }
        if let Some(verified) = originals.managed_op1a.get(&source.id) {
            let media = managed_multistream::add_media(source, t, id, audio_index, verified, index, out, budget)?;
            video |= t.kind == KIND_PICTURE;
            if t.kind == KIND_SOUND {
                labels.insert(t.index);
            }
            item.media.push(SourceItemMedia {
                media: media.id.clone(), gain_db: t.gain_db.unwrap_or(0.0), source_offset: Some(Rational::ZERO),
            });
            out.graph.media.push(media);
            raw_tracks.push(obj(vec![
                ("kind", Value::Int(t.kind)),
                ("label", Value::Int(t.index)),
                ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
                ("offset", observed(&Observed::Known(0))),
            ]));
            continue;
        }
        if let Some(selected) = originals.tracks.get(&(source.id.clone(), tindex)) {
            let stream_id = if selected.kind == StreamKind::Video {
                if video {
                    return Err("more than one original picture track".into());
                }
                video = true;
                format!("{id}:video")
            } else {
                labels.insert(t.index);
                format!("{id}:audio:{audio_index}")
            };
            let offset = match (source.offsets.get(tindex), source.rate) {
                (Some(Observed::Known(n)), Some(r)) => Some(time(*n, r)?),
                _ => None,
            };
            let original = &selected.stream;
            budget
                .charge(
                    size_of::<SourceMedia>()
                        + size_of::<SourceItemMedia>()
                        + size_of::<OriginalStream>()
                        + stream_id.len() * 2
                        + selected.path.as_os_str().len()
                        + original
                            .saved_locators
                            .iter()
                            .map(|s| s.capacity() + size_of::<String>())
                            .sum::<usize>()
                        + 512,
                )
                .map_err(MasterError::Budget)?;
            let mut native_original = vec![
                ("file_package", s(t.file_mob.umid_hex())),
            ];
            let source_clock = if let Some(clock) = &selected.column_timecode {
                native_original.extend([
                    ("binding", s("avid_source_columns")),
                    ("descriptor_length", Value::Int(t.length)),
                    ("descriptor", descriptor(&t.essence)),
                    ("native_kind", Value::Int(t.kind)),
                    ("native_track", Value::Int(t.index)),
                    ("file_track", Value::Int(clock.file_track)),
                ]);
                Some(SourceClock {
                    effective_count: clock.count as u64, rate: rational(clock.rate),
                    display_base: 24, drop: false,
                    provenance: SourceClockProvenance {
                        origin_count: clock.count as u64, source_offset: 0, source_rate: rational(clock.rate),
                        source_package: clock.package.umid_hex(), timecode_track: "TC1".into(),
                        route: graph::source::AVID_SOURCE_COLUMNS_TIMECODE.into(),
                    },
                })
            } else if originals.proxies.contains_key(&source.id) && selected.kind == StreamKind::Audio {
                native_original.extend([
                    ("binding", s("avid_source_columns")),
                    ("descriptor", descriptor(&t.essence)),
                    ("native_kind", Value::Int(t.kind)),
                    ("native_track", Value::Int(t.index)),
                ]);
                None
            } else {
                let raw = source.originals[tindex].as_ref().ok_or("original descriptor witness missing")?;
                native_original.extend([
                    ("descriptor_class", s(&raw.descriptor_class)),
                    ("descriptor_rate", Value::Float(raw.descriptor_rate)),
                    ("descriptor_length", Value::Int(raw.descriptor_length)),
                    ("stored_quantization_bits", raw.stored_quantization_bits.map(Value::Int).unwrap_or(Value::Null)),
                    ("descriptor", descriptor(&t.essence)),
                    ("native_kind", Value::Int(t.kind)),
                    ("native_track", Value::Int(t.index)),
                    ("file_track", raw.file_track.map(Value::Int).unwrap_or(Value::Null)),
                ]);
                None
            };
            let original_clock = if selected.online && selected.kind == StreamKind::Video
                && selected.codec == "avc1" && selected.rate == Some(Rational::integer(24))
                && selected.stream.container == "mov" && m.tracks.len() == 1
                && offset == Some(Rational::ZERO)
            {
                source.file_clocks.get(tindex).and_then(Option::as_ref)
                    .filter(|clock| (clock.rate.num, clock.rate.den, clock.base, clock.drop) == (24, 1, 24, false))
                    .and_then(|clock| {
                        let effective = u64::try_from(clock.count.checked_add(clock.offset)?).ok()?;
                        (effective < 24 * 60 * 60 * 24).then(|| SourceClock {
                            effective_count: effective,
                            rate: Rational::integer(24),
                            display_base: 24,
                            drop: false,
                            provenance: SourceClockProvenance {
                                origin_count: clock.count as u64,
                                source_offset: clock.offset,
                                source_rate: Rational::integer(24),
                                source_package: clock.package.umid_hex(),
                                timecode_track: "TC1".into(),
                                route: "avid_saved_original_to_physical_timecode".into(),
                            },
                        })
                    })
            } else {
                None
            };
            let source_clock = source_clock.or(original_clock);
            if let Some(timecode) = &selected.embedded_mov_timecode {
                let mut witness = vec![("observation", observed(timecode))];
                if let Observed::Known(count) = timecode {
                    witness.extend([
                        ("label", s(tc(*count, 24))),
                        ("rate", s("24")),
                        ("stream", Value::Int(2)),
                    ]);
                }
                native_original.push(("embedded_mov_timecode", obj(witness)));
            }
            if let Some(origin) = selected.motion_audio_origin {
                native_original.push(("motion_adapter_file_track_start", Value::Int(origin)));
                native_original.push(("verified_aac_priming_samples", Value::Int(1024)));
            }
            if selected.same_rate_aac_origin {
                native_original.push(("same_rate_aac_file_track_start", Value::Int(1)));
                native_original.push(("verified_aac_priming_samples", Value::Int(1024)));
            }
            sidecar(
                out,
                budget,
                "avid.source",
                &stream_id,
                obj(native_original),
            )
            .map_err(MasterError::Budget)?;
            out.graph.media.push(SourceMedia {
                id: stream_id.clone(),
                kind: selected.kind,
                locations: vec![selected
                    .path
                    .to_str()
                    .ok_or("original path is not UTF-8")?
                    .into()],
                codec: Some(selected.codec.clone()),
                frame_size: selected.raster,
                rate: selected.rate,
                sample_rate: selected.sample_rate,
                bits: selected.bits,
                duration: selected.duration,
                start: source_clock.as_ref().and_then(SourceClock::exact_start),
                clock: source_clock,
                stream: (selected.kind == StreamKind::Audio).then_some(t.index),
                online: Some(selected.online),
                original: Some(original.clone()),
            });
            item.media.push(SourceItemMedia {
                media: stream_id,
                gain_db: t.gain_db.unwrap_or(0.0),
                source_offset: offset,
            });
            raw_tracks.push(obj(vec![
                ("kind", Value::Int(t.kind)),
                ("label", Value::Int(t.index)),
                ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
                (
                    "offset",
                    source
                        .offsets
                        .get(tindex)
                        .map(observed)
                        .unwrap_or(Value::Null),
                ),
            ]));
            continue;
        }
        let rate = Rate::from_avid(t.edit_rate)
            .ok_or_else(|| format!("unsupported edit rate {}", t.edit_rate))?;
        let Some(path) = index.files.get(&t.file_mob.umid_hex()) else {
            if originals.allow_offline {
                let media = offline_managed(source, tindex, id, audio_index, &mut video, &mut labels,
                    out, budget)?;
                item.media.push(SourceItemMedia {
                    media: media.id.clone(), gain_db: t.gain_db.unwrap_or(0.0),
                    source_offset: Some(Rational::ZERO),
                });
                out.graph.media.push(media);
                raw_tracks.push(obj(vec![
                    ("kind", Value::Int(t.kind)), ("label", Value::Int(t.index)),
                    ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
                    ("offset", observed(&Observed::Known(0))),
                ]));
                continue;
            }
            if t.kind == KIND_SOUND {
                budget.charge(m.name.len()*4 + 160).map_err(MasterError::Budget)?;
                out.missing_managed_audio.push(format!("{:?} A{}: missing file package {}",
                    m.name, t.index, t.file_mob.umid_hex()));
                out.diagnostics.push(format!(
                    "{:?} audio A{}: media for file mob {} not found; converted without it",
                    m.name,
                    t.index,
                    t.file_mob.umid_hex()
                ));
                continue;
            }
            return Err(MasterError::MissingPicture(format!(
                "media for file mob {} not found", t.file_mob.umid_hex()
            )));
        };
        let identity = index.identity(path)?;
        if let Some(origin) = source.rendered_gain {
            if m.edit_rate != 24.0 || !matches!(m.length, 24 | 48) {
                return Err("AudioSuite Gain requires a qualified 24 fps render span".to_string().into());
            }
            let playable_samples = m.length * (48_000 / 24);
            if identity.material_packages != [m.mob_id.umid_hex()]
                || identity.file_packages != [t.file_mob.umid_hex()]
                || !identity.source_packages.contains(&origin.umid_hex())
                || identity.file_tracks != [(48_000, 1, playable_samples)]
                || !identity.timecodes.is_empty()
            {
                return Err("AudioSuite Gain MXF disagrees with the saved render identity or sample clock".to_string().into());
            }
        }
        let (stream_id, stream_kind, codec, size, sample_rate, bits, stream) =
            match (t.kind, &t.essence) {
                (
                    KIND_PICTURE,
                    Some(Essence::Picture {
                        compression,
                        width,
                        height,
                        ..
                    }),
                ) => {
                    if video {
                        return Err("more than one picture track".to_string().into());
                    }
                    video = true;
                    let codec = match compression.as_str() {
                        "RI00" if source.rendered_freeze
                            && identity.picture_essence_coding == Some(avid_reader::mxf::PRORES_422_HQ_CODING)
                            && identity.material_packages == [m.mob_id.umid_hex()]
                            && identity.file_packages == [t.file_mob.umid_hex()]
                            && identity.file_tracks == [(24, 1, t.length)]
                            && identity.timecodes.is_empty() => "apch",
                        "AVHD" => "AVdn",
                        "RI00"
                            if identity.picture_essence_coding
                                == Some(avid_reader::mxf::PRORES_422_LT_CODING) =>
                        {
                            "apcs"
                        }
                        "RI00"
                            if matches!(
                                identity.picture_essence_coding,
                                Some(avid_reader::mxf::DNXHR_LB_CODING | avid_reader::mxf::DNXHR_LB_CODING_0A)
                            ) =>
                        {
                            "AVdn"
                        }
                        _ => {
                            return Err(
                                format!("unsupported picture compression {compression:?}").into()
                            )
                        }
                    };
                    if !(16..=16384).contains(width) || !(16..=16384).contains(height) {
                        return Err("unsupported picture raster".to_string().into());
                    }
                    (
                        format!("{id}:video"),
                        StreamKind::Video,
                        Some(codec.into()),
                        Some((*width, *height)),
                        None,
                        None,
                        None,
                    )
                }
                (
                    KIND_SOUND,
                    Some(Essence::Sound {
                        sample_rate,
                        bits,
                        channels,
                    }),
                ) => {
                    if *channels != 1
                        || !matches!(*sample_rate, 44100.0 | 48000.0 | 96000.0)
                        || !matches!(*bits, 16 | 24)
                        || !(1..=64).contains(&t.index)
                    {
                        return Err("unsupported audio descriptor".to_string().into());
                    }
                    labels.insert(t.index);
                    (
                        format!("{id}:audio:{audio_index}"),
                        StreamKind::Audio,
                        None,
                        None,
                        Some(*sample_rate as i64),
                        Some(*bits),
                        Some(t.index),
                    )
                }
                _ => return Err(format!("track {} has an unsupported descriptor", t.index).into()),
            };
        if let Some(origin) = source.rendered_gain {
            sidecar(out, budget, "avid.audiosuite_gain", &stream_id,
                obj(vec![("origin_package", s(origin.umid_hex()))])).map_err(MasterError::Budget)?;
        }
        let duration = match identity.file_tracks.as_slice() {
            [(n, d, len)] if *n > 0 && *d > 0 => {
                Rational::new(*len as i128 * *d as i128, *n as i128)
                    .ok_or("MXF duration overflow")?
            }
            _ => time(t.length, rate)?,
        };
        // The optional clock lives inline in the media record; precharge its owned
        // provenance payload before the reader creates the qualified owned witness.
        if let Some(q) = identity.qualified_file_clocks.first() {
            budget
                .charge(
                    q.source_package.capacity()
                        + q.timecode_track.capacity()
                        + q.route.capacity()
                        + size_of::<SourceClock>(),
                )
                .map_err(MasterError::Budget)?;
        }
        let source_clock = identity
            .qualified_file_clock()
            .map(|q| {
                Ok::<_, String>(SourceClock {
                    effective_count: q.effective_count,
                    rate: Rational::new(q.rate.0 as i128, q.rate.1 as i128)
                        .ok_or("invalid qualified clock rate")?,
                    display_base: q.rounded_base,
                    drop: q.drop,
                    provenance: SourceClockProvenance {
                        origin_count: q.origin_count,
                        source_offset: q.source_offset,
                        source_rate: Rational::new(
                            q.source_rate.0 as i128,
                            q.source_rate.1 as i128,
                        )
                        .ok_or("invalid qualified source rate")?,
                        source_package: q.source_package,
                        timecode_track: q.timecode_track,
                        route: q.route,
                    },
                })
            })
            .transpose()?;
        let start = source_clock
            .as_ref()
            .map(|c| c.exact_start().ok_or("qualified MXF clock overflow"))
            .transpose()?;
        let offset = match (source.offsets.get(tindex), source.rate) {
            (Some(Observed::Known(n)), Some(r)) => Some(time(*n, r)?),
            _ => None,
        };
        if offset.is_none() {
            loss(
                out,
                budget,
                id,
                "source_offset",
                format!("track {} item-to-file offset is unresolved", t.index),
            )
            .map_err(MasterError::Budget)?;
        }
        let legacy = identity.start_timecode();
        let legacy_start = match legacy {
            Some((n, b, false)) if b > 0 => {
                Rational::new(n as i128, b as i128).unwrap_or(Rational::ZERO)
            }
            _ => Rational::ZERO,
        };
        sidecar(
            out,
            budget,
            "avid.source",
            &stream_id,
            obj(vec![
                ("legacy_clock", clock(legacy)),
                ("legacy_start", s(legacy_start)),
                (
                    "legacy_start_provenance",
                    s("nominal non-drop compatibility aid; not target representability"),
                ),
                ("legacy_file_clock", clock(identity.file_start_timecode())),
                ("file_package", s(t.file_mob.umid_hex())),
                ("descriptor_rate", s(rational(rate))),
                ("descriptor_length", Value::Int(t.length)),
                (
                    "master_offset",
                    source
                        .offsets
                        .get(tindex)
                        .map(observed)
                        .unwrap_or(Value::Null),
                ),
                ("native_kind", Value::Int(t.kind)),
                ("native_track", Value::Int(t.index)),
                ("descriptor", descriptor(&t.essence)),
                (
                    "picture_coding",
                    identity
                        .picture_essence_coding
                        .map(|v| s(v.iter().map(|b| format!("{b:02X}")).collect::<String>()))
                        .unwrap_or(Value::Null),
                ),
            ]),
        )
        .map_err(MasterError::Budget)?;
        let media = SourceMedia {
            id: stream_id.clone(),
            kind: stream_kind,
            locations: vec![path.to_str().ok_or("media path is not UTF-8")?.into()],
            codec,
            frame_size: size,
            rate: (stream_kind == StreamKind::Video).then(|| rational(rate)),
            sample_rate,
            bits,
            duration,
            start,
            clock: source_clock,
            stream,
            online: Some(true),
            original: None,
        };
        budget
            .charge(
                size_of::<SourceMedia>()
                    + size_of::<SourceItemMedia>()
                    + stream_id.len() * 2
                    + path.as_os_str().len()
                    + 128,
            )
            .map_err(MasterError::Budget)?;
        item.media.push(SourceItemMedia {
            media: stream_id,
            gain_db: t.gain_db.unwrap_or(0.0),
            source_offset: offset,
        });
        out.graph.media.push(media);
        raw_tracks.push(obj(vec![
            ("kind", Value::Int(t.kind)),
            ("label", Value::Int(t.index)),
            ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
            (
                "offset",
                source
                    .offsets
                    .get(tindex)
                    .map(observed)
                    .unwrap_or(Value::Null),
            ),
        ]));
    }
    if item.media.is_empty() {
        return Err(
            "no media found for any clip track; clip and its edits left out"
                .to_string()
                .into(),
        );
    }
    for (a, b) in &m.stereo_pairs {
        if labels.contains(a) && labels.contains(b) {
            budget
                .charge(64 + 2 * size_of::<i64>())
                .map_err(MasterError::Budget)?;
            item.stream_groups.push(vec![*a, *b]);
        } else if (*a, *b) == (1, 2) && labels.contains(a)
            && !originals.tracks.contains_key(&(source.id.clone(), 0))
            && matches!(m.tracks.as_slice(), [track] if index.files.get(&track.file_mob.umid_hex())
                .is_some_and(|path| motion_mxf.pcm16.contains_key(path.as_path())))
        {
            loss(out, budget, id, "audio_group",
                "Saved stereo declaration has only its authored A1 track. The verified mono PCM file remains mono.")
                .map_err(MasterError::Budget)?;
        } else {
            out.audio_group_issues.push(format!(
                "{:?}: stereo A{a}/A{b} missing a channel; available media remains mono",
                m.name
            ));
        }
    }
    for g in &m.other_groups {
        match avid_reader::extract::parse_multichannel_group(g) {
            Some((_, channels)) if channels.iter().all(|n| labels.contains(n)) => {
                budget
                    .charge(64 + channels.capacity() * size_of::<i64>())
                    .map_err(MasterError::Budget)?;
                item.stream_groups.push(channels);
            }
            _ => out.audio_group_issues.push(format!(
                "{:?}: unsupported or incomplete native audio group {g}",
                m.name
            )),
        }
    }
    let mut marker_receipts = vec![];
    for (i, (_, _, mk)) in m.markers.iter().enumerate() {
        let witness = source.marker_witnesses.get(i);
        let rate = witness.and_then(|w| w.local_rate);
        let mut native = master_marker_native(source, i, mk);
        if mk.length.is_some_and(|n| n != 0) && rate.is_none() {
            if let Value::Obj(fields) = &mut native {
                fields.push(("graph_index".into(), Value::Null));
            }
            marker_receipts.push(native);
            loss(out,budget,id,"marker_timing","native spanned marker duration has no qualified owner rate; marker retained only as native facts").map_err(MasterError::Budget)?;
            continue;
        }
        if let Value::Obj(fields) = &mut native {
            fields.push(("graph_index".into(), Value::Int(item.markers.len() as i64)));
        }
        marker_receipts.push(native);
        let local = match (witness.map(|w| &w.local_offset), rate) {
            (Some(Observed::Known(n)), Some(r)) => Some(time(*n, r)?),
            _ => None,
        };
        let start = match (witness.and_then(|w| w.master_frame), source.rate) {
            (Some(n), Some(r)) => Some(time(n, r)?),
            _ => None,
        };
        let track = witness.filter(|w| w.track > 0).and_then(|w| {
            kind(w.kind).map(|kind| TrackRef { kind, index: w.track })
        });
        item.markers.push(marker(
            mk,
            start,
            local,
            rate,
            witness
                .map(|w| w.owner.clone())
                .unwrap_or_else(|| source.id.clone()),
            MarkerOwner::Sequence,
            track,
            SourceBinding {
                placements: vec![],
                reason: None,
            },
        )?);
    }
    let avid = obj(vec![
        ("mob_id", s(m.mob_id.urn())),
        (
            "columns",
            Value::Obj(m.columns.iter().map(|(k, v)| (k.clone(), s(v))).collect()),
        ),
        (
            "start_timecode",
            m.start
                .as_ref()
                .and_then(master_start_timecode)
                .map(s)
                .unwrap_or(Value::Null),
        ),
        (
            "aux_timecodes",
            Value::Arr(
                m.aux_timecodes
                    .iter()
                    .map(|a| {
                        obj(vec![
                            ("track_index", Value::Int(a.track_index)),
                            ("timecode", s(tc(a.start, a.fps))),
                            ("frames", Value::Int(a.start)),
                            ("fps", Value::Int(a.fps)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("source_file", opt_s(m.source_file.as_deref())),
        ("source_path", opt_s(m.source_path.as_deref())),
    ]);
    sidecar(out, budget, "avid", id, avid).map_err(MasterError::Budget)?;
    if let Some(import) = &source.imported_audio {
        sidecar(out, budget, "avid.audio_import", id, obj(vec![
            ("saved_locators", Value::Arr(vec![s(&import.locator)])),
            ("physical_package", s(import.physical_package.umid_hex())),
            ("source_edit_rate", s("24")),
            ("source_timecode_frame", Value::Int(import.start_frame)),
            ("frames", Value::Int(import.frames)),
            ("managed_sample_rate", Value::Int(48_000)),
            ("managed_bit_depth", Value::Int(16)),
            ("lanes", Value::Arr(import.lanes.iter().map(|lane| obj(vec![
                ("master_track", Value::Int(lane.master_track)),
                ("file_package", s(lane.file_package.umid_hex())),
                ("file_track", Value::Int(lane.file_track)),
                ("physical_track", Value::Int(lane.physical_track)),
            ])).collect())),
        ])).map_err(MasterError::Budget)?;
    }
    let mix = output_mix(&m.output_mix, budget).map_err(MasterError::Budget)?;
    let mut native = vec![
        ("occurrence", s(&source.id)),
        ("native_rate", source.rate.map(|r| s(rational(r))).unwrap_or(Value::Null)),
        ("drop", Value::Bool(m.start.as_ref().is_some_and(|t| t.drop))),
        ("raw_groups", Value::Arr(m.other_groups.iter().map(s).collect())),
        ("tracks", Value::Arr(raw_tracks)),
        ("marker_native", Value::Arr(marker_receipts)),
        ("master_output_mix", mix),
    ];
    if let Some(transforms) = source_input_transforms(source, budget).map_err(MasterError::Budget)? {
        native.push(("input_color_transforms", transforms));
    }
    if let Some(managed) = source.managed_multistream.as_ref().filter(|_| originals.managed_op1a.contains_key(&source.id)) {
        native.push(("managed_multistream", managed_multistream::saved_facts(managed, budget)
            .map_err(MasterError::Budget)?));
    }
    sidecar(
        out,
        budget,
        "avid.source",
        id,
        obj(native),
    )
    .map_err(MasterError::Budget)?;
    budget
        .charge(
            size_of::<SourceItem>()
                + item.name.capacity()
                + id.len()
                + item.markers.len() * size_of::<SourceMarker>(),
        )
        .map_err(MasterError::Budget)?;
    Ok(item)
}

#[cfg(test)]
mod tests;
