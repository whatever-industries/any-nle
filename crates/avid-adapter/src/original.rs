//! Exact saved-locator/root-map lookup and bounded read-only physical-stream probes.
use crate::{OriginalLocatorIssue, OriginalLocatorKind, SelectedMediaIndex, SourceReadIssue};
use avid_reader::source as av;
use graph::{source::OriginalStream, Rational, StreamKind, Value};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
    process::{ChildStdout, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
mod still;
mod source_media;
mod additional_media;
mod same_rate_movie;
pub(super) mod legacy_mxf;
const MAX_ORIGINAL_INDEX_BYTES: usize = 64 << 20;
pub const SOURCE_MEDIA_UNAVAILABLE: &str = "preferred source media unavailable";
pub use still::{verify_imported_png, PngRole};
#[derive(Clone, Debug)]
pub struct RootMapping {
    pub saved_root: PathBuf,
    pub current_root: PathBuf,
}
#[derive(Clone, Debug)]
pub struct OriginalOptions {
    pub link_managed_op1a: bool,
    /// Direct Premiere links use a verified dry input when Gain needs prepared media.
    pub link_dry_gain: bool,
    pub root_mappings: Vec<RootMapping>,
    pub ffprobe: PathBuf,
    pub prefer_source_media: bool,
    pub attach_avid_proxies: bool,
    pub proxy_rewrap_dir: Option<PathBuf>,
    pub ffmpeg: PathBuf,
    pub allow_media_gaps: bool,
    /// Keep the qualified pair of managed mono siblings when their picture is absent.
    pub preserve_managed_audio_without_picture: bool,
    /// Verify ordinary linked streams for a destination with a native importer.
    pub verify_linked_imports: bool,
    /// Collect saved-path and MXF package details for Review blockers.
    pub collect_blocked_locators: bool,
}
impl Default for OriginalOptions {
    fn default() -> Self {
        Self {
            link_managed_op1a: false,
            link_dry_gain: false,
            root_mappings: vec![],
            ffprobe: PathBuf::from("ffprobe"),
            prefer_source_media: false,
            attach_avid_proxies: false,
            proxy_rewrap_dir: None,
            ffmpeg: "ffmpeg".into(),
            allow_media_gaps: false,
            preserve_managed_audio_without_picture: false,
            verify_linked_imports: false,
            collect_blocked_locators: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Selected {
    pub verified_for_native_import: bool,
    pub path: PathBuf,
    pub online: bool,
    pub stream: OriginalStream,
    pub kind: StreamKind,
    pub codec: String,
    pub raster: Option<(i64, i64)>,
    pub rate: Option<Rational>,
    pub sample_rate: Option<i64>,
    pub bits: Option<i64>,
    pub duration: Rational,
    /// Source-frame origin for the qualified automatic adapter only.
    pub motion_audio_origin: Option<i64>,
    /// The verified same-rate Avid AAC file-track start is one frame.
    pub same_rate_aac_origin: bool,
    pub mp3_delay_samples: Option<i64>,
    /// Physical source clock corroborated by the saved managed clock and the MOV.
    pub column_timecode: Option<av::SavedFileClock>,
    /// A file-declared 24 fps MOV timecode, separate from Avid logging and stream PTS.
    pub(crate) embedded_mov_timecode: Option<av::Observed<i64>>,
}
#[derive(Default)]
pub struct OriginalIndex {
    pub tracks: HashMap<(String, usize), Selected>,
    pub(crate) allow_offline: bool,
    pub(super) allow_media_gaps: bool,
    pub(super) preserve_managed_audio_without_picture: bool,
    pub(super) mixed_rate_managed_audio: HashSet<avid_reader::io::MobId>,
    pub original_inputs: Vec<av::SourceInput>,
    /// Explicit destination defaults, never observations of missing source tags.
    pub notes: Vec<String>,
    pub(crate) proxies: HashMap<String, crate::proxy::Binding>,
    pub(crate) managed_op1a: HashMap<String, crate::managed_multistream::VerifiedManagedDirectOut>,
    pub(crate) omitted_unused: HashMap<String, String>,
    pub(super) omitted_media: HashMap<String, String>,
    pub(super) legacy_mxf: HashMap<String, String>,
    pub(super) references: Option<crate::unused::References>,
}

impl OriginalIndex {
    pub(crate) fn select_avid_proxies(
        &mut self, facts: &av::SourceProject, options: &OriginalOptions, index: &crate::index::MediaIndex,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), SourceReadIssue> {
        let mut claims = HashMap::<LocatorClaimKey, LocatorClaim>::new();
        for track in self.tracks.values() {
            let Some((path, claim)) = resolve_with_claim(&track.stream.saved_locators, &options.root_mappings)? else { continue; };
            if path == track.path {
                match claims.entry(locator_claim_key(&path)?) {
                    Entry::Occupied(mut entry) => entry.get_mut().intersect(&claim),
                    Entry::Vacant(entry) => {
                        entry.insert(claim);
                    }
                }
            }
        }
        let mut pinned = self.original_inputs.iter().map(|i| (i.path.clone(),i.revision.clone())).collect();
        let mut used = 0;
        source_media::select(facts, options, self, &mut claims, &mut pinned, &mut used, check, Some(index))
    }
}

fn selection_cost(master: &av::SourceMaster, raw: &av::OriginalTrack, path: &Path) -> usize {
    std::mem::size_of::<Selected>() + path.as_os_str().len()
        + raw.locators.iter().map(String::len).sum::<usize>() + master.id.len() + 256
}

const OFFLINE_ORIGINAL_PROFILE: &str = "saved descriptor is outside the offline WAV or same-rate MP4 profile";
const NO_CONTAINER_ROUTE: &str = "original suffix has no qualified physical container route";

fn saved_original_layout(master: &av::SourceMaster) -> String {
    use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};

    master.master.tracks.iter().zip(&master.originals).take(16)
        .filter_map(|(track, raw)| {
            let raw = raw.as_ref()?;
            let label = match track.kind {
                KIND_PICTURE => format!("V{}", track.index),
                KIND_SOUND => format!("A{}", track.index),
                _ => return None,
            };
            let mut detail = format!("{label} {}", raw.descriptor_class);
            if let Some((width, height)) = raw.display_raster {
                detail.push_str(&format!(" {width}×{height}"));
            }
            if let Some(bits) = raw.stored_quantization_bits {
                detail.push_str(&format!(" {bits}-bit"));
            }
            if let Some(channels) = raw.channels {
                detail.push_str(&format!(" {channels} ch"));
            }
            if track.kind == KIND_PICTURE {
                detail.push_str(&format!(" at {} fps", raw.descriptor_rate));
            } else {
                detail.push_str(&format!(" at {} Hz", raw.sample_rate.unwrap_or(raw.descriptor_rate)));
            }
            if let Some(file_track) = raw.file_track {
                detail.push_str(&format!(" (saved file track {file_track})"));
            }
            Some(detail)
        }).collect::<Vec<_>>().join(", ")
}

/// The first omission profile requires one coherent saved file per master.
/// Leave different locators and incomplete saved routes to normal validation.
fn shared_saved_locator(master: &av::SourceMaster) -> Option<&str> {
    let first = master.originals.first()?.as_ref()?.locators.first()?;
    let expected = saved_parts(first).ok()?;
    master.originals.iter().all(|raw| raw.as_ref().is_some_and(|raw|
        !raw.locators.is_empty()
        && raw.locators.iter().all(|path| saved_parts(path).ok().as_ref() == Some(&expected))
    )).then_some(first)
}

fn gap_saved_locator(master: &av::SourceMaster) -> Option<&str> {
    use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};
    let path = shared_saved_locator(master)?;
    let mut lanes = HashSet::new();
    let mut channels = HashSet::new();
    if master.originals.len() != master.master.tracks.len()
        || !master.id.contains("/item/") || master.rendered_freeze || master.rendered_gain.is_some()
        || master.rate.is_none() || master.master.length <= 0
        || !master.master.tracks.iter().zip(&master.originals).all(|(track, raw)| {
            let raw = raw.as_ref().expect("shared locator requires original descriptors");
            track.index > 0 && lanes.insert((track.kind, track.index))
                && track.length > 0 && raw.descriptor_length > 0
                && raw.descriptor_rate.is_finite() && raw.descriptor_rate > 0.0
                && match track.kind {
                    KIND_PICTURE => track.index == 1 && raw.file_track == Some(1)
                        && raw.display_raster.is_some_and(|(width, height)| width > 0 && height > 0),
                    KIND_SOUND => raw.channels.zip(raw.file_track).is_some_and(|(count, channel)| {
                        (1..=count).contains(&channel) && channels.insert(channel)
                    }),
                    _ => false,
                }
        })
    {
        return None;
    }
    Some(path)
}

// A well-formed file which agrees with the saved full span can be reported
// as an unsupported profile. Other probe or identity failures remain errors.
fn unsupported_pcm_movie(master: &av::SourceMaster, streams: &[Value]) -> bool {
    use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};
    let [video, audio] = streams else { return false };
    let raster = number(video, "width").zip(number(video, "height"));
    let rate = ratio(video, "avg_frame_rate");
    let channels = number(audio, "channels");
    let bits = match text(audio, "codec_name").as_deref() {
        Some("pcm_s16le" | "pcm_s16be") => 16,
        Some("pcm_s24le" | "pcm_s24be") => 24,
        _ => return false,
    };
    text(video, "codec_name").as_deref() == Some("h264")
        && raster.is_some_and(|(width, height)| width >= 3840 && height >= 2160)
        && video.get("side_data_list").is_none_or(|v| matches!(v, Value::Arr(a) if a.is_empty()))
        && text(video, "field_order").as_deref() == Some("progressive")
        && text(video, "sample_aspect_ratio").as_deref() == Some("1:1")
        && rate.is_some() && ratio(video, "r_frame_rate") == rate
        && number(video, "index") == Some(0) && number(audio, "index") == Some(1)
        && number(video, "start_pts") == Some(0) && number(audio, "start_pts") == Some(0)
        && ratio(audio, "time_base") == Rational::new(1, 48_000)
        && number(audio, "sample_rate") == Some(48_000)
        && channels == Some(2)
        && master.master.tracks.len() == 3
        && master.master.tracks.iter().zip(&master.originals).all(|(track, raw)| {
            let Some(raw) = raw else { return false };
            match track.kind {
                KIND_PICTURE => raw.descriptor_class == "CDCI" && raw.display_raster == raster
                    && av::Rate::from_avid(raw.descriptor_rate).map(crate::rational) == rate
                    && number(video, "nb_frames") == Some(raw.descriptor_length),
                KIND_SOUND => raw.descriptor_class == "PCMA" && raw.stored_quantization_bits == Some(bits)
                    && raw.channels == channels && raw.sample_rate == Some(48_000.0)
                    && number(audio, "duration_ts") == Some(raw.descriptor_length),
                _ => false,
            }
        })
}

fn offline_original(master: &av::SourceMaster, index: usize) -> Result<Option<Selected>, String> {
    let track = &master.master.tracks[index];
    let raw = master.originals[index].as_ref().unwrap();
    let mut locators = raw.locators.iter().map(|path| saved_parts(path));
    let saved = locators.next().transpose()?.ok_or("saved original path is absent")?;
    for other in locators {
        if other? != saved {
            return Err("offline original has conflicting saved locators".into());
        }
    }
    let path = PathBuf::from(&raw.locators[0]);
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
    let movie = extension == "mp4";
    let has_picture = master.master.tracks.iter().any(|t| t.kind == avid_reader::extract::KIND_PICTURE);
    let (kind, codec, representation, bits, rate, raster) = match (&track.essence, raw.descriptor_class.as_str()) {
        (Some(avid_reader::extract::Essence::Picture { width: 1920, height: 1080 | 1088, .. }), "CDCI")
            if track.kind == avid_reader::extract::KIND_PICTURE && extension == "mp4"
                && raw.display_raster == Some((1920, 1080)) && raw.descriptor_rate == 24.0
                && track.edit_rate == 24.0 && raw.descriptor_length == track.length
                && raw.file_track == Some(1) =>
            (StreamKind::Video, "avc1", None, None, Some(Rational::integer(24)), Some((1920, 1080))),
        (Some(avid_reader::extract::Essence::Sound { bits, .. }), "PCMA")
            if track.kind == avid_reader::extract::KIND_SOUND && extension == "wav"
                && matches!(bits, 16 | 24) && raw.descriptor_rate == 48000.0 && !has_picture =>
            (StreamKind::Audio, if *bits == 16 {"pcm_s16le"} else {"pcm_s24le"},
                Some(if *bits == 16 {"pcm16"} else {"pcm24"}), Some(*bits), None, None),
        (_, "MPGA") if track.kind == avid_reader::extract::KIND_SOUND && extension == "mp4"
            && has_picture && raw.descriptor_rate == 46.875 && raw.channels == Some(2)
            && raw.motion_audio_track_start != Some(1) =>
            (StreamKind::Audio, "aac", Some("aac"), None, None, None),
        _ => return Ok(None),
    };
    let (time_base, channels, channel_index, sample_rate) = if kind == StreamKind::Video {
        (Rational::new(1, 24).unwrap(), None, None, None)
    } else {
        let count = raw.channels.filter(|c| matches!(c, 1 | 2)).ok_or("offline channel count unresolved")?;
        let channel = raw.file_track.filter(|n| (1..=count).contains(n)).ok_or("offline channel selector unresolved")? - 1;
        if raw.sample_rate != Some(48_000.0) || bits.is_some() && raw.stored_quantization_bits != bits {
            return Err("offline audio descriptor has an unsupported sample clock or representation".into());
        }
        (Rational::new(1, 48_000).unwrap(), Some(count), Some(channel), Some(48_000))
    };
    if raw.descriptor_length <= 0 || master.offsets.get(index) != Some(&av::Observed::Known(0)) {
        return Err("offline original requires a known zero item offset and a positive extent".into());
    }
    Ok(Some(Selected {
        verified_for_native_import: false,
        path,
        online: false,
        stream: OriginalStream {
            saved_locators: raw.locators.clone(),
            container: if movie {"mov"} else {"wav"}.into(),
            stream_index: if movie && kind == StreamKind::Audio {1} else {0},
            channel_index,
            channels,
            time_base,
            start_pts: 0,
            duration_ts: raw.descriptor_length,
            sample_representation: representation.map(str::to_owned),
        },
        kind,
        codec: codec.into(),
        raster,
        rate,
        sample_rate,
        bits,
        duration: Rational::new(raw.descriptor_length.into(), time_base.den().into())
            .ok_or("offline original extent exceeds the time model")?,
        motion_audio_origin: None,
        same_rate_aac_origin: false,
        mp3_delay_samples: None,
        column_timecode: None,
        embedded_mov_timecode: None,
    }))
}
pub(crate) fn text(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}
pub(crate) fn number(v: &Value, k: &str) -> Option<i64> {
    match v.get(k)? {
        Value::Str(s) => s.parse().ok(),
        Value::Int(n) => Some(*n),
        _ => None,
    }
}
fn ratio(v: &Value, k: &str) -> Option<Rational> {
    text(v, k).and_then(|s| {
        let (n, d) = s.split_once('/')?;
        Rational::new(n.parse().ok()?, d.parse().ok()?)
    })
}
const ORIGINAL_PROBE_ENTRIES:&str="stream=index,codec_type,codec_name,codec_tag_string,profile,level,pix_fmt,width,height,avg_frame_rate,r_frame_rate,sample_aspect_ratio,field_order,color_range,color_space,color_transfer,color_primaries,sample_rate,channels,channel_layout,bits_per_sample,bits_per_raw_sample,time_base,start_pts,duration_ts,nb_frames:stream_side_data=side_data_type:stream_tags=rotate,timecode,reel_name,file_package_umid:format=format_name:format_tags=major_brand,timecode,operational_pattern_ul,material_package_umid:packet=stream_index,pts,duration,pos:packet_side_data=side_data_type,skip_samples,discard_padding";
const UNTAGGED_COLOR_NOTE:&str="Original picture has incomplete or unspecified color/range tags. The writer uses its BT.709 destination profile as a default; this is not observed source color or range.";

/// A suffix chooses a supported route only after the physical demuxer family
/// corroborates it. In particular, a zero PTS does not turn PCM in another
/// container into a WAV source.
fn qualify_container(path: &Path, probe: &Value) -> Result<&'static str, String> {
    let format = probe.get("format").and_then(|f| f.get("format_name"));
    let Some(Value::Str(format)) = format else {
        return Err("original physical container format is absent or unresolved".into());
    };
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let movie_family = !format.is_empty()
        && format
            .split(',')
            .all(|name| matches!(name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"));
    match extension.as_str(){
        "wav" if format=="wav"=>Ok("wav"),
        "mov"|"mp4" if movie_family=>Ok("mov"),
        "wav"=>Err("original physical container mismatch: .wav requires the WAV demuxer family".into()),
        "mov"|"mp4"=>Err("original physical container mismatch: .mov/.mp4 requires the FFmpeg MOV demuxer family".into()),
        _=>Err(NO_CONTAINER_ROUTE.into()),
    }
}

/// Shared geometry, color and orientation checks for the admitted movie profiles.
/// A missing color declaration is a destination default, not a source BT.709 fact.
fn qualify_movie_picture(video: &Value, allow_adapter_rate: bool) -> Result<bool, String> {
    qualify_movie_geometry(video, (1920, 1080), allow_adapter_rate)
}

fn qualify_movie_geometry(
    video: &Value, raster: (i64, i64), allow_adapter_rate: bool,
) -> Result<bool, String> {
    let rate = ratio(video, "avg_frame_rate");
    let qualified_rate = rate == Some(Rational::integer(24))
        || (allow_adapter_rate && rate == Rational::new(24_000, 1001));
    if !qualified_rate {
        let adapter = if raster == (1920, 1080) { ", or 23.976 fps with a qualified automatic adapter" } else { "" };
        return Err(format!(
            "original picture geometry/rate is unqualified: requires square-pixel progressive {}x{} at 24 fps{adapter}",
            raster.0, raster.1
        ));
    }
    qualify_movie_geometry_at_rate(video, raster, rate.unwrap())
}

fn qualify_movie_geometry_at_rate(video: &Value, raster: (i64, i64), rate: Rational) -> Result<bool, String> {
    if number(video, "width") != Some(raster.0)
        || number(video, "height") != Some(raster.1)
        || ratio(video, "avg_frame_rate") != Some(rate)
        || ratio(video, "r_frame_rate") != Some(rate)
        || text(video, "field_order").as_deref() != Some("progressive")
        || text(video, "sample_aspect_ratio").as_deref() != Some("1:1")
    {
        return Err(format!(
            "original picture geometry/rate is unqualified: requires square-pixel progressive {}x{} at {rate} fps",
            raster.0, raster.1
        ));
    }
    let mut defaulted = false;
    for field in ["color_space", "color_transfer", "color_primaries"] {
        match video.get(field) {
            Some(Value::Str(value)) if value=="bt709"=>{},
            None=>defaulted=true,
            Some(Value::Str(value)) if matches!(value.as_str(),"unknown"|"unspecified")=>defaulted=true,
            _=>return Err(format!("original picture {field} is unqualified: requires BT.709 or an untagged/unspecified value; HDR and other color profiles are not admitted")),
        }
    }
    match video.get("color_range") {
        Some(Value::Str(value)) if value=="tv"=>{},
        None=>defaulted=true,
        Some(Value::Str(value)) if matches!(value.as_str(),"unknown"|"unspecified")=>defaulted=true,
        _=>return Err("original picture color range is unqualified: full-range and other tagged ranges are not admitted".into()),
    }
    match video.get("side_data_list") {
        None=>{},Some(Value::Arr(rows)) if rows.is_empty()=>{},
        _=>return Err("original picture side data is unqualified: HDR, rotation and other stream side data require separate interpretation".into()),
    }
    match video.get("tags") {
        None => {}
        Some(Value::Obj(tags)) => {
            let mut rotation = tags.iter().filter(|(name, _)| name == "rotate");
            if let Some((_, value)) = rotation.next() {
                let zero = match value {
                    Value::Str(s) => s.parse::<f64>().is_ok_and(|n| n.is_finite() && n == 0.0),
                    Value::Int(n) => *n == 0,
                    Value::Float(n) => n.is_finite() && *n == 0.0,
                    _ => false,
                };
                if !zero || rotation.next().is_some() {
                    return Err("original picture rotation is unqualified: nonzero or unresolved tags.rotate cannot use the identity-orientation writer profile".into());
                }
            }
        }
        _ => return Err("original picture rotation metadata is unresolved".into()),
    }
    Ok(defaulted)
}

fn qualify_movie(video: &Value, audio: Option<&Value>, allow_adapter_rate: bool) -> Result<bool, String> {
    qualify_avc_profile(video)?;
    let defaulted = qualify_movie_picture(video, allow_adapter_rate && audio.is_some())?;
    if let Some(audio) = audio {
        qualify_stereo_aac_lc(audio)?;
    }
    Ok(defaulted)
}

fn qualify_stereo_aac_lc(audio: &Value) -> Result<(), String> {
    if text(audio, "codec_name").as_deref() != Some("aac")
        || text(audio, "codec_tag_string").as_deref() != Some("mp4a")
        || text(audio, "profile").as_deref() != Some("LC")
        || number(audio, "sample_rate") != Some(48_000)
        || number(audio, "channels") != Some(2)
        || text(audio, "channel_layout").as_deref() != Some("stereo")
    {
        return Err("original movie audio profile is unqualified: requires AAC LC/mp4a 48000 Hz stereo".into());
    }
    Ok(())
}

fn qualify_avc_profile(video: &Value) -> Result<(), String> {
    qualify_avc_high(video, 40)
}

fn qualify_avc_high(video: &Value, level: i64) -> Result<(), String> {
    if text(video, "codec_name").as_deref() != Some("h264")
        || text(video, "codec_tag_string").as_deref() != Some("avc1")
        || text(video, "profile").as_deref() != Some("High")
        || number(video, "level") != Some(level)
        || text(video, "pix_fmt").as_deref() != Some("yuv420p")
    {
        return Err(format!("original picture profile is unqualified: requires H.264 High level {}.{} avc1/yuv420p; Hi10 and wider profiles are not admitted", level / 10, level % 10));
    }
    Ok(())
}

fn is_avc_pcm_movie(streams: &[Value]) -> bool {
    matches!(streams, [video, audio] if text(video, "codec_name").as_deref() == Some("h264")
        && matches!(text(audio, "codec_name").as_deref(),
            Some("pcm_s16le" | "pcm_s24le" | "pcm_s24be"))
        && number(audio, "channels") == Some(2))
}

fn qualify_avc_pcm(
    streams: &[Value], path: &Path, master: &av::SourceMaster,
) -> Result<bool, String> {
    let [video, audio] = streams else {
        return Err("AVC PCM requires one picture and one stereo PCM stream".into());
    };
    let mp4 = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"));
    let mov = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mov"));
    if !(mp4 || mov) || shared_saved_locator(master).is_none()
        || master.master.tracks.len() != 3 || master.originals.len() != 3 || master.offsets.len() != 3
        || !master.offsets.iter().all(|offset| *offset == av::Observed::Known(0))
    {
        return Err("AVC PCM requires a shared MOV/MP4 locator and three tracks with saved zero offsets".into());
    }
    let rate = master.rate.filter(|rate| matches!((rate.num, rate.den),
        (24, 1) | (24_000, 1001) | (25, 1) | (30_000, 1001)))
        .ok_or("AVC PCM master edit rate is unqualified")?;
    let rate_exact = crate::rational(rate);
    let raster = number(video, "width").zip(number(video, "height"))
        .filter(|(w, h)| (16..=16_384).contains(w) && (16..=16_384).contains(h))
        .ok_or("AVC PCM raster is missing or out of range")?;
    let level = number(video, "level").filter(|level| matches!(level, 40 | 51))
        .ok_or("AVC PCM requires H.264 High level 4.0 or 5.1")?;
    qualify_avc_high(video, level)?;
    let defaulted = qualify_movie_geometry_at_rate(video, raster, rate_exact)?;
    let bits = match (mp4, text(audio, "codec_name").as_deref(), text(audio, "codec_tag_string").as_deref()) {
        (true, Some("pcm_s16le"), Some("ipcm")) | (false, Some("pcm_s16le"), Some("sowt")) => 16,
        (true, Some("pcm_s24le"), Some("ipcm")) | (false, Some("pcm_s24le" | "pcm_s24be"), Some("in24")) => 24,
        _ => return Err("AVC PCM audio representation does not match its container".into()),
    };
    let frames = number(video, "nb_frames").filter(|n| *n > 0)
        .ok_or("AVC PCM picture count is unresolved")?;
    let samples = same_rate_movie::frame_audio_extent(frames, rate, false)
        .ok_or("AVC PCM audio extent overflows")?;
    if number(video, "index") != Some(0) || number(audio, "index") != Some(1)
        || text(audio, "channel_layout").as_deref() != Some("stereo")
        || number(audio, "channels") != Some(2) || number(audio, "sample_rate") != Some(48_000)
        || number(audio, "bits_per_sample") != Some(bits)
        || number(audio, "duration_ts") != Some(samples)
    {
        return Err("AVC PCM requires picture stream 0 and full-span stereo PCM16/24 at 48 kHz on stream 1".into());
    }
    check_saved_avc_pcm(master, frames, raster, rate_exact, bits, samples)?;
    Ok(defaulted)
}

fn check_saved_avc_pcm(
    master: &av::SourceMaster, frames: i64, raster: (i64, i64), rate: Rational,
    bits: i64, samples: i64,
) -> Result<(), String> {
    use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};
    let mut lanes = HashSet::new();
    let mut channels = HashSet::new();
    for (track, raw) in master.master.tracks.iter().zip(&master.originals) {
        let raw = raw.as_ref().ok_or("AVC PCM saved descriptor is missing")?;
        let matched = match track.kind {
            KIND_PICTURE => track.index == 1 && lanes.insert((KIND_PICTURE, 1))
                && raw.descriptor_class == "CDCI" && raw.display_raster == Some(raster)
                && av::Rate::from_avid(raw.descriptor_rate).map(crate::rational) == Some(rate)
                && raw.descriptor_length == frames
                && raw.file_track == Some(1),
            KIND_SOUND => matches!(track.index, 1 | 2) && lanes.insert((KIND_SOUND, track.index))
                && raw.descriptor_class == "PCMA" && raw.stored_quantization_bits == Some(bits)
                && raw.descriptor_rate == 48_000.0 && raw.sample_rate == Some(48_000.0)
                && raw.channels == Some(2) && raw.descriptor_length == samples
                && raw.file_track.is_some_and(|ch| matches!(ch, 1 | 2) && channels.insert(ch)),
            _ => false,
        };
        if !matched {
            return Err("AVC PCM saved extent or Avid lane/physical channel assignment disagrees".into());
        }
    }
    if !lanes.contains(&(KIND_PICTURE, 1)) || channels.len() != 2 {
        return Err("AVC PCM requires saved V1 and both independent audio lanes".into());
    }
    Ok(())
}

fn qualify_prores_hq(path: &Path, probe: &Value, streams: &[Value]) -> Result<bool, String> {
    qualify_prores_hq_timecode(path, probe, streams, Rational::integer(24), "00:00:00:00")
}

fn qualify_prores_hq_timecode(
    path: &Path, probe: &Value, streams: &[Value], rate: Rational, expected: &str,
) -> Result<bool, String> {
    let [video, timecode] = streams else {
        return Err("linked ProRes HQ requires one picture stream and one timecode track".into());
    };
    verify_prores_hq_timecode_track(path, probe, video, timecode, 1, rate, expected)?;
    qualify_movie_geometry_at_rate(video, (1920, 1080), rate)
}

fn embedded_avc_mov_timecode(
    probe: &Value, streams: &[Value], video: &Value,
) -> Option<av::Observed<i64>> {
    let format_tags = probe.get("format").and_then(|f| f.get("tags"));
    if format_tags.and_then(|tags| text(tags, "major_brand")).as_deref() != Some("qt  ") {
        return None;
    }
    let tag = |stream: &Value| stream.get("tags").and_then(|tags| text(tags, "timecode"));
    let video_label = tag(video);
    let format_label = format_tags.and_then(|tags| text(tags, "timecode"));
    let mut tracks = streams.iter().filter(|s| text(s, "codec_tag_string").as_deref() == Some("tmcd"));
    let track = tracks.next();
    if track.is_none() && video_label.is_none() && format_label.is_none() {
        return None;
    }
    let count = (|| {
        let track = track?;
        let label = video_label.as_deref()?;
        let base = ratio(video, "time_base").filter(|base| base.num() > 0)?;
        let duration = number(video, "duration_ts").filter(|&n| n > 0)?;
        if tracks.next().is_some()
            || tag(track).as_deref() != Some(label)
            || format_label.as_deref().is_some_and(|other| other != label)
            || number(track, "index") != Some(2)
            || text(track, "codec_type").as_deref() != Some("data")
            || number(track, "nb_frames") != Some(1)
            || number(track, "start_pts") != Some(0)
            || ratio(track, "avg_frame_rate") != Some(Rational::integer(24))
            || ratio(track, "time_base") != Some(base)
            || number(track, "duration_ts") != Some(duration)
        {
            return None;
        }
        let parts = label.as_bytes();
        if parts.len() != 11 || [2, 5, 8].into_iter().any(|i| parts[i] != b':') {
            return None;
        }
        let mut fields = [0; 4];
        for (field, pair) in fields.iter_mut().zip(parts.split(|&c| c == b':')) {
            if pair.len() != 2 || !pair.iter().all(u8::is_ascii_digit) {
                return None;
            }
            *field = i64::from((pair[0] - b'0') * 10 + pair[1] - b'0');
        }
        let [h, m, s, f] = fields;
        (h < 24 && m < 60 && s < 60 && f < 24).then_some((h * 3600 + m * 60 + s) * 24 + f)
    })();
    Some(match count {
        Some(count) => av::Observed::Known(count),
        None => av::Observed::Unresolved("embedded MOV timecode lacks one matching 24 fps non-drop track and picture declaration".into()),
    })
}

fn verify_prores_hq_timecode_track(
    path: &Path, probe: &Value, video: &Value, timecode: &Value, index: i64, rate: Rational, expected: &str,
) -> Result<(), String> {
    verify_prores_timecode_track(path, probe, video, timecode, index, rate, expected, ("apch", "HQ"))
}

fn verify_prores_timecode_track(
    path: &Path, probe: &Value, video: &Value, timecode: &Value, index: i64,
    rate: Rational, expected: &str, (tag, profile): (&str, &str),
) -> Result<(), String> {
    let brand = probe.get("format").and_then(|f| f.get("tags"))
        .and_then(|tags| text(tags, "major_brand"));
    if !path.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("mov"))
        || brand.as_deref() != Some("qt  ")
        || number(video, "index") != Some(0)
        || text(video, "codec_type").as_deref() != Some("video")
        || text(video, "codec_name").as_deref() != Some("prores")
        || text(video, "codec_tag_string").as_deref() != Some(tag)
        || text(video, "profile").as_deref() != Some(profile)
        || text(video, "pix_fmt").as_deref() != Some("yuv422p10le")
    {
        return Err(format!("linked ProRes picture requires QuickTime ProRes 422 {profile}/{tag}, 10-bit 4:2:2"));
    }
    verify_movie_timecode_track(video, timecode, index, rate, expected)
        .map_err(|_| format!("linked ProRes HQ requires one matching {rate} fps non-drop timecode track at {expected}"))
}

fn verify_movie_timecode_track(
    video: &Value, timecode: &Value, index: i64, rate: Rational, expected: &str,
) -> Result<(), String> {
    let matching_timecode = |stream: &Value| {
        stream.get("tags").and_then(|tags| text(tags, "timecode")).as_deref() == Some(expected)
    };
    let picture_base = ratio(video, "time_base").filter(|base| base.num() > 0);
    let picture_duration = number(video, "duration_ts").filter(|&duration| duration > 0);
    if number(timecode, "index") != Some(index)
        || text(timecode, "codec_type").as_deref() != Some("data")
        || text(timecode, "codec_tag_string").as_deref() != Some("tmcd")
        || number(timecode, "nb_frames") != Some(1)
        || ratio(timecode, "avg_frame_rate") != Some(rate)
        || ratio(video, "avg_frame_rate") != Some(rate)
        || picture_base.is_none()
        || ratio(timecode, "time_base") != picture_base
        || picture_duration.is_none()
        || number(timecode, "duration_ts") != picture_duration
        || number(timecode, "start_pts") != Some(0)
        || !matching_timecode(video)
        || !matching_timecode(timecode)
    {
        return Err(format!("linked movie requires one matching {rate} fps timecode track at {expected}"));
    }
    Ok(())
}

/// Saved path syntax is parsed independently of the current OS. Only an explicit
/// prefix mapping may turn a foreign path into a local path; no basename search.
fn saved_parts(raw: &str) -> Result<(String, Vec<String>), String> {
    if raw.len() > 32768 || raw.chars().any(char::is_control) {
        return Err("original locator is too long or contains control characters".into());
    }
    let windows = raw.as_bytes().get(1) == Some(&b':')
        && raw.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && raw
            .as_bytes()
            .get(2)
            .is_some_and(|b| matches!(b, b'/' | b'\\'));
    let unc = raw.starts_with("\\\\");
    let normalized = if windows || unc {
        raw.replace('\\', "/")
    } else {
        raw.to_owned()
    };
    let (root, body) = if windows {
        (normalized[..2].to_ascii_uppercase(), &normalized[3..])
    } else if unc {
        let mut parts = normalized[2..].split('/');
        let server = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or("invalid UNC server")?;
        let share = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or("invalid UNC share")?;
        if [server, share].iter().any(|p| matches!(*p, "." | "..")) {
            return Err("invalid UNC root".into());
        }
        let used = 2 + server.len() + 1 + share.len();
        (
            format!(
                "//{}/{}",
                server.to_ascii_lowercase(),
                share.to_ascii_lowercase()
            ),
            normalized.get(used + 1..).unwrap_or(""),
        )
    } else if normalized.starts_with('/') {
        ("/".into(), &normalized[1..])
    } else {
        return Err("original locator is not a normalized absolute path".into());
    };
    let mut parts = Vec::new();
    for part in body.split('/') {
        if part == ".." {
            return Err("original locator contains a parent escape".into());
        }
        if !part.is_empty() && part != "." {
            parts.push(part.to_owned());
        }
    }
    Ok((root, parts))
}
#[derive(Default)]
struct LocatorClaim {
    saved_paths: HashSet<String>,
    mapped: bool,
}

impl LocatorClaim {
    fn intersect(&mut self, other: &Self) {
        self.saved_paths.retain(|saved| other.saved_paths.contains(saved));
        self.mapped |= other.mapped;
    }
}

#[cfg(unix)]
type LocatorClaimKey = (u64, u64);
#[cfg(not(unix))]
type LocatorClaimKey = PathBuf;

fn locator_claim_key(path: &Path) -> Result<LocatorClaimKey, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("original locator is not a file: {}", path.display()));
        }
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Ok(path.to_owned())
    }
}

pub fn resolve(locators: &[String], mappings: &[RootMapping]) -> Result<PathBuf, String> {
    resolve_with_claim(locators, mappings)?.ok_or_else(missing_locator).map(|(path, _)| path)
}

fn missing_locator() -> String {
    "original locator resolution has 0 exact candidates; an explicit unambiguous root mapping is required".into()
}

#[derive(Default)]
struct LocatorDetails {
    rows: Vec<(usize, String)>,
    original_tracks: Vec<(usize, usize)>,
    bytes: usize,
    truncated: bool,
}

impl LocatorDetails {
    fn push(&mut self, bin: usize, detail: String) -> bool {
        self.bytes += detail.capacity() + std::mem::size_of::<(usize, String)>();
        if self.bytes > crate::index::REVIEW_DETAIL_BUDGET {
            self.truncated = true;
            return false;
        }
        self.rows.push((bin, detail));
        true
    }
}

fn blocked_locator_details(
    remaining: &[av::SourceMaster],
    first_track: usize,
    mappings: &[CheckedRootMapping<'_>],
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<LocatorDetails, SourceReadIssue> {
    let mut details = LocatorDetails::default();
    for (index, master) in remaining.iter().enumerate() {
        check()?;
        // Managed and mixed routes have different addressing rules. Only
        // examine masters that would enter the normal original-path loop.
        if master.pcm_prefix.is_some()
            || master.originals.len() != master.master.tracks.len()
            || master.originals.iter().any(Option::is_none)
            || legacy_mxf::saved_sibling_directory(master).is_some()
        {
            continue;
        }
        let mut seen = HashSet::new();
        let start = if index == 0 { first_track } else { 0 };
        for (track, raw) in master.originals.iter().enumerate().skip(start) {
            check()?;
            let raw = raw.as_ref().expect("original descriptor route checked");
            if !seen.insert(raw.locators.as_slice()) || (index == 0 && track == first_track) {
                continue;
            }
            let reason = match resolve_with_checked_mappings(&raw.locators, mappings) {
                Ok(Some(_)) => continue,
                Ok(None) => missing_locator(),
                Err(error) => error,
            };
            let saved = raw.locators.first().map_or("no saved path", String::as_str);
            let detail = format!("{:?} · {saved}: {reason} · {}", master.master.name, master.id);
            if !details.push(master.master.bin, detail) {
                return Ok(details);
            }
            details.original_tracks.push((index, track));
        }
    }
    Ok(details)
}

fn resolve_with_claim(
    locators: &[String],
    mappings: &[RootMapping],
) -> Result<Option<(PathBuf, LocatorClaim)>, String> {
    resolve_with_checked_mappings(locators, &check_root_mappings(mappings)?)
}

type CheckedRootMapping<'a> = ((String, Vec<String>), &'a Path);

fn check_root_mappings(mappings: &[RootMapping]) -> Result<Vec<CheckedRootMapping<'_>>, String> {
    mappings
        .iter()
        .map(|m| {
            if !m.current_root.is_absolute()
                || m.current_root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(
                    "current original root mappings must be absolute without parent escapes".into(),
                );
            }
            let saved = m
                .saved_root
                .to_str()
                .ok_or("saved root mapping is not UTF-8")?;
            Ok((saved_parts(saved)?, m.current_root.as_path()))
        })
        .collect()
}

fn resolve_with_checked_mappings(
    locators: &[String],
    mappings: &[CheckedRootMapping<'_>],
) -> Result<Option<(PathBuf, LocatorClaim)>, String> {
    let mut paths = HashMap::<PathBuf, LocatorClaim>::new();
    for raw in locators {
        let (root, parts) = saved_parts(raw)?;
        let path = Path::new(raw);
        let mut candidates = Vec::new();
        if path.is_absolute() {
            candidates.push((path.to_owned(), false));
        }
        for ((saved_root, saved), current) in mappings {
            if root == *saved_root && parts.starts_with(saved) {
                let mut mapped = current.to_path_buf();
                for part in &parts[saved.len()..] {
                    let mut components = Path::new(part).components();
                    if !matches!(
                        (components.next(), components.next()),
                        (Some(Component::Normal(name)), None) if name == part.as_str()
                    ) {
                        return Err("saved locator component cannot be mapped literally on this platform".into());
                    }
                    mapped.push(part);
                }
                candidates.push((mapped, true));
            }
        }
        let saved_path = format!("{}/{}", root.trim_end_matches('/'), parts.join("/"));
        for (p, mapped) in candidates {
            match std::fs::metadata(&p) {
                Ok(meta) if meta.is_file() => {
                    let path = std::fs::canonicalize(&p).map_err(|e| e.to_string())?;
                    let claim = paths.entry(path).or_default();
                    claim.saved_paths.insert(saved_path.clone());
                    claim.mapped |= mapped;
                }
                Ok(_) => return Err(format!("original locator is not a file: {}", p.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::symlink_metadata(&p) {
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                        _ => return Err(format!("original locator could not be followed: {}", p.display())),
                    }
                }
                Err(e) => return Err(format!("{}: {e}", p.display())),
            }
        }
    }
    if paths.len() > 1 {
        let mut candidates: Vec<_> = paths.keys().collect();
        candidates.sort_unstable();
        let shown = candidates
            .iter()
            .take(4)
            .map(|path| format!("{path:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let more = if candidates.len() > 4 { ", …" } else { "" };
        return Err(format!(
            "original locator resolution has {} exact candidates: {shown}{more}. Check the saved path and root mappings so only the intended file resolves",
            candidates.len()
        ));
    }
    Ok(paths.into_iter().next())
}

pub(crate) fn bounded_probe(command: Command, timeout: std::time::Duration) -> Result<Vec<u8>, String> {
    run_probe(command, ProbeDeadline::Total(timeout), &mut || Ok(()), |stdout, _| {
        let mut bytes = Vec::new();
        stdout.take((1 << 20) + 1).read_to_end(&mut bytes)
            .map_err(|e| format!("stdout read failed: {e}"))?;
        if bytes.len() > 1 << 20 {
            return Err("stdout exceeds 1 MiB".into());
        }
        Ok(bytes)
    })
}

enum ProbeDeadline {
    Total(Duration),
    Idle(Duration),
}

const PICTURE_PACKET_IDLE_LIMIT: ProbeDeadline = ProbeDeadline::Idle(Duration::from_secs(30));

fn run_probe<T: Send + 'static>(
    mut command: Command,
    deadline: ProbeDeadline,
    check: &mut dyn FnMut() -> Result<(), String>,
    read: impl FnOnce(ChildStdout, &AtomicU64) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    use std::{sync::{mpsc, Arc}, time::Instant};
    let (timeout, track_progress) = match deadline {
        ProbeDeadline::Total(timeout) => (timeout, false),
        ProbeDeadline::Idle(timeout) => (timeout, true),
    };
    check()?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("stdout pipe unavailable".into());
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let progress = Arc::new(AtomicU64::new(0));
    let worker_progress = Arc::clone(&progress);
    std::thread::spawn(move || {
        let _ = tx.send(read(stdout, &worker_progress));
    });
    let mut last_activity = Instant::now();
    let mut verified = 0;
    let mut output = None;
    let mut status = None;
    let failure = loop {
        if let Err(reason) = check() {
            break reason;
        }
        if output.is_none() {
            match rx.try_recv() {
                Ok(Ok(value)) => output = Some(value),
                Ok(Err(reason)) => break reason,
                Err(mpsc::TryRecvError::Disconnected) => break "stdout reader disconnected".into(),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(e) => break format!("process status failed: {e}"),
            }
        }
        if let Some(status) = status {
            if let Some(value) = output.take() {
                if status.success() {
                    return Ok(value);
                }
                break format!("process refused the file ({status})");
            }
        }
        if track_progress {
            let current = progress.load(Ordering::Relaxed);
            if current != verified {
                verified = current;
                last_activity = Instant::now();
            }
        }
        if last_activity.elapsed() >= timeout {
            let detail = if track_progress { " without a verified picture packet" } else { "" };
            break format!("timed out after {} ms{detail}", timeout.as_millis());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    // Kill and reap only this owned child. A bounded receive drains the pipe without
    // an unbounded job-thread read/join, even if a broken descendant keeps a pipe open.
    let _ = child.kill();
    let _ = child.wait();
    if output.is_none() {
        let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
    }
    Err(failure)
}
/// Verify picture geometry and return the probed rate and duration for supported Timewarp clocks.
pub(super) fn square_1080p_mxf(path: &Path, tool: &Path) -> Option<Option<(Rational, Rational)>> {
    let mut command = Command::new(tool);
    command.args([
        "-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mxf", "-select_streams", "v",
        "-show_entries",
        "stream=width,height,sample_aspect_ratio,field_order,r_frame_rate,time_base,duration_ts",
        "-of", "json",
    ]).arg(path);
    bounded_probe(command, std::time::Duration::from_secs(10)).ok()
        .and_then(|raw| graph::json::parse(std::str::from_utf8(&raw).ok()?).ok())
        .and_then(|value| {
            let Some(Value::Arr(streams)) = value.get("streams") else { return None; };
            let [video] = streams.as_slice() else { return None; };
            if number(video, "width") != Some(1920) || number(video, "height") != Some(1080)
                || text(video, "sample_aspect_ratio").as_deref() != Some("1:1")
                || text(video, "field_order").as_deref() != Some("progressive")
            { return None; }
            let clock = match (text(video, "r_frame_rate").as_deref(), text(video, "time_base").as_deref()) {
                (Some("24/1"), Some("1/24")) => Some((24, 1)),
                (Some("24000/1001"), Some("1001/24000")) => Some((24_000, 1001)),
                _ => None,
            };
            Some(clock.and_then(|(num, den)| {
                let count = i128::from(number(video, "duration_ts").filter(|&n| n > 0)?);
                Some((Rational::new(num, den)?, Rational::new(count * den, num)?))
            }))
        })
}

fn read_picture_packet_row<'a>(
    packets: &mut impl BufRead, bytes: &'a mut Vec<u8>,
) -> Result<Option<&'a str>, String> {
    const MAX_PACKET_ROW: u64 = 128;
    bytes.clear();
    let length = packets.take(MAX_PACKET_ROW + 1)
        .read_until(b'\n', bytes)
        .map_err(|err| format!("stdout read failed: {err}"))?;
    if length == 0 {
        return Ok(None);
    }
    if length as u64 > MAX_PACKET_ROW {
        return Err(format!("row exceeds {MAX_PACKET_ROW} bytes"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "packet report is not UTF-8")?;
    Ok(Some(text.lines().next().unwrap_or("")))
}

fn verify_prores_packets(
    mut packets: impl BufRead, count: i64, ticks_per_frame: i128, mut mark_verified: impl FnMut(),
) -> Result<(), String> {
    let mut bytes = Vec::new();
    let mut seen = 0;
    while let Some(row) = read_picture_packet_row(&mut packets, &mut bytes)
        .map_err(|why| format!("linked ProRes HQ packet {seen}: {why}"))?
    {
        let expected = i128::from(seen) * ticks_per_frame;
        let mut fields = row.split(',').map(str::parse::<i128>);
        let valid = matches!(
            (fields.next(), fields.next(), fields.next(), fields.next()),
            (Some(Ok(pts)), Some(Ok(dts)), Some(Ok(duration)), None)
                if anynle_media_verification::ordered_picture_packet(
                    [pts, dts, duration], expected, ticks_per_frame,
                )
        );
        if seen >= count || !valid {
            return Err(format!(
                "linked ProRes HQ packet {seen} does not match the exact picture clock"
            ));
        }
        seen += 1;
        mark_verified();
    }
    if seen != count {
        return Err(
            "linked ProRes HQ picture packet count disagrees with the declared extent".into(),
        );
    }
    Ok(())
}

fn prove_prores_cadence(
    path: &Path, tool: &Path, video: &Value, rate: Rational,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let count = number(video, "nb_frames")
        .filter(|&count| count > 0)
        .ok_or("linked ProRes HQ requires a positive declared picture count")?;
    let base = ratio(video, "time_base")
        .filter(|base| base.num() > 0)
        .ok_or("linked ProRes HQ picture time base is missing")?;
    let numerator = i128::from(base.den()) * i128::from(rate.den());
    let denominator = i128::from(base.num()) * i128::from(rate.num());
    if denominator <= 0 {
        return Err("linked ProRes HQ picture rate is invalid".into());
    }
    let ticks_per_frame = numerator / denominator;
    if ticks_per_frame == 0
        || numerator % denominator != 0
        || number(video, "duration_ts").filter(|&duration| duration > 0).map(i128::from)
            .is_none_or(|duration| i128::from(count).checked_mul(ticks_per_frame) != Some(duration))
    {
        return Err(format!("linked ProRes HQ picture extent does not fit its exact {rate} fps clock"));
    }
    let mut command = Command::new(tool);
    command
        .args([
            "-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mov",
            "-select_streams", "v:0", "-show_packets", "-show_entries",
            "packet=pts,dts,duration:packet_side_data=", "-of", "csv=p=0",
        ])
        .arg(path);
    // Read through EOF so an extra or uneven packet after the metadata probe's
    // initial sixteen packets cannot enter the fixed-rate destination route.
    run_probe(command, PICTURE_PACKET_IDLE_LIMIT, check, move |stdout, progress| {
        verify_prores_packets(BufReader::new(stdout), count, ticks_per_frame, || {
            progress.fetch_add(1, Ordering::Relaxed);
        })
    }).map_err(|why| format!("linked ProRes HQ complete picture packet probe failed: {why}"))
}

struct OriginalProbe {
    metadata: Value,
    /// Present only after the whole physical picture packet clock was checked.
    prores_color_defaulted: Option<bool>,
    mp3: Option<additional_media::Mp3Extent>,
    still_decoded: Option<(i64, i64)>,
}

fn probe_with_linked_imports(
    path: &Path, tool: &Path, verify_linked_imports: bool, check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<OriginalProbe, String> {
    let metadata = probe_metadata(path, tool)?;
    if let Some(rate) = same_rate_movie::avc_picture_only_rate(path, &metadata) {
        same_rate_movie::prove_avc_picture_cadence(path, tool, &metadata, rate, check)?;
    }
    let prores_color_defaulted = match metadata.get("streams") {
        Some(Value::Arr(streams)) if streams.first().is_some_and(|s|
            text(s, "codec_name").as_deref() == Some("prores")
                && (same_rate_movie::is_25_or_2997(ratio(s, "avg_frame_rate"))
                    || same_rate_movie::custom_prores_raster(s))) =>
        {
            prove_prores_cadence(path, tool, &streams[0], ratio(&streams[0], "avg_frame_rate").unwrap(), check)?;
            None
        }
        Some(Value::Arr(streams))
            if streams.iter().any(|stream| text(stream, "codec_name").as_deref() == Some("prores")) =>
        {
            qualify_container(path, &metadata)?;
            let source_profile = match streams.as_slice() {
                [video, timecode] if text(video, "codec_tag_string").as_deref() == Some("apcs") => {
                    let rate = Rational::integer(24);
                    verify_prores_timecode_track(
                        path, &metadata, video, timecode, 1, rate, "00:00:00:00", ("apcs", "LT"),
                    ).and_then(|()| qualify_movie_geometry_at_rate(video, (1920, 1080), rate))
                }
                _ => qualify_prores_hq(path, &metadata, streams),
            };
            match source_profile {
                Ok(defaulted) => {
                    prove_prores_cadence(path, tool, &streams[0], Rational::integer(24), check)?;
                    Some(defaulted)
                }
                Err(_) if verify_linked_imports => None,
                Err(error) => return Err(error),
            }
        }
        _ => None,
    };
    let mp3 = if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp3")) {
        Some(additional_media::probe_mp3(path, tool, &metadata)?)
    } else {
        None
    };
    let still_decoded = additional_media::probe_still(path, tool)?;
    Ok(OriginalProbe { metadata, prores_color_defaulted, mp3, still_decoded })
}

fn probe(
    path: &Path, tool: &Path, check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<OriginalProbe, String> {
    probe_with_linked_imports(path, tool, false, check)
}

fn probe_metadata(path: &Path, tool: &Path) -> Result<Value, String> {
    let mut command = Command::new(tool);
    additional_media::single_jpeg_input(&mut command, path);
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-read_intervals",
            "%+#16",
            "-show_packets",
            "-show_entries",
            ORIGINAL_PROBE_ENTRIES,
            "-of",
            "json",
        ])
        .arg(path);
    let context = |why: String| {
        format!(
            "original stream probe {} using {}: {why}",
            path.display(),
            tool.display()
        )
    };
    let bytes = bounded_probe(command, std::time::Duration::from_secs(30)).map_err(context)?;
    graph::json::parse(
        std::str::from_utf8(&bytes).map_err(|_| context("stdout is not UTF-8".into()))?,
    )
    .map_err(context)
}

/// Independently inspect an original before reconciling a Premiere media record.
/// Uses the same bounded probe and ProRes cadence verification as direct linking.
#[doc(hidden)]
pub fn inspect_premiere_donor_original(
    path: &Path, tool: &Path, timecode: Option<&str>,
) -> Result<Value, String> {
    let metadata = probe_metadata(path, tool)?;
    match (qualify_container(path, &metadata)?, metadata.get("streams"), timecode) {
        ("mov", Some(Value::Arr(streams)), Some(label)) => {
            let rate = Rational::integer(24);
            qualify_prores_hq_timecode(path, &metadata, streams, rate, label)?;
            prove_prores_cadence(path, tool, &streams[0], rate, &mut || Ok(()))?;
        }
        ("wav", Some(Value::Arr(streams)), None) if streams.len() == 1 => {}
        _ => return Err("expected one ProRes picture with timecode or one mono WAV".into()),
    }
    Ok(metadata)
}

/// Compare a relink candidate with the same bounded source profiles used by
/// original linking. Times here are file-relative; clip logging TC is separate.
pub fn verify_offline_original(
    path: &Path, tool: &Path, container: &str, expected: &[(&OriginalStream, StreamKind, Rational, Option<i64>)],
) -> Result<(), String> {
    let inspected = probe(path, tool, &mut || Ok(()))?;
    let value = &inspected.metadata;
    if qualify_container(path, value)? != container {
        return Err("candidate container does not match the saved original".into());
    }
    let Some(Value::Arr(streams)) = value.get("streams") else {
        return Err("candidate stream list is absent".into());
    };
    let video = |s: &Value| text(s, "codec_type").as_deref() == Some("video");
    let audio = |s: &Value| text(s, "codec_type").as_deref() == Some("audio");
    match (container, streams.as_slice()) {
        ("wav", [a]) if audio(a) => {}
        ("mov", [v]) if video(v) => { qualify_movie(v, None, false)?; }
        ("mov", [v, a]) if video(v) && audio(a) => { qualify_movie(v, Some(a), false)?; }
        _ => return Err("candidate stream topology is outside the saved original profile".into()),
    }
    for (saved, kind, duration, bits) in expected {
        let s = streams.iter().find(|s| number(s, "index") == Some(saved.stream_index))
            .ok_or("candidate is missing the saved physical stream")?;
        let tb = ratio(s, "time_base").ok_or("candidate stream has no exact time base")?;
        let count = number(s, "duration_ts").ok_or("candidate stream has no exact duration")?;
        let start = number(s, "start_pts").or((container == "wav").then_some(0));
        if start != Some(0) || Rational::new(i128::from(count) * i128::from(tb.num()), i128::from(tb.den())) != Some(*duration) {
            return Err("candidate file-relative origin or extent differs from the saved original".into());
        }
        match kind {
            StreamKind::Video if video(s) && number(s, "nb_frames") == Some(saved.duration_ts) => {}
            StreamKind::Audio if audio(s) => {
                let codec = match saved.sample_representation.as_deref() {
                    Some("pcm16") => "pcm_s16le",
                    Some("pcm24") => "pcm_s24le",
                    Some("aac") => "aac",
                    _ => return Err("saved audio representation is unresolved".into()),
                };
                if text(s, "codec_name").as_deref() != Some(codec)
                    || tb != saved.time_base || number(s, "sample_rate") != Some(48_000)
                    || number(s, "channels") != saved.channels
                    || bits.is_some_and(|bits| number(s, "bits_per_sample") != Some(bits))
                    || !matches!((saved.channel_index, saved.channels), (Some(c), Some(n)) if (0..n).contains(&c))
                {
                    return Err("candidate audio representation or physical channel assignment differs".into());
                }
            }
            _ => return Err("candidate stream kind or picture frame count differs".into()),
        }
    }
    Ok(())
}

pub fn probe_offline_atom(path: &Path, tool: &Path) -> Result<Value, String> {
    let mut command = Command::new(tool);
    command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mxf", "-show_entries",
        "stream=codec_type,codec_name,profile,width,height,sample_aspect_ratio,field_order,channels,sample_rate,bits_per_sample,start_pts,time_base,duration_ts:format_tags=operational_pattern_ul",
        "-of", "json"]).arg(path);
    let bytes = bounded_probe(command, std::time::Duration::from_secs(30))?;
    graph::json::parse(std::str::from_utf8(&bytes).map_err(|_| "MXF probe output is not UTF-8")?)
}

/// Prove that the declared 48 kHz mono PCM span is present in the file.
pub(super) fn complete_mono_pcm_packets(path: &Path, tool: &Path, samples: i64, bytes_per_sample: i64) -> bool {
    mono_pcm_packets(path, tool, "mxf", samples, bytes_per_sample, false)
}

pub(super) fn complete_pcm24_with_zero_tail(path: &Path, tool: &Path, samples: i64) -> bool {
    mono_pcm_packets(path, tool, "mxf", samples, 3, true)
}

/// Check the complete payload of a one-stream donor file using the existing packet checks.
#[doc(hidden)]
pub fn verify_premiere_donor_payload(
    path: &Path, tool: &Path, samples_or_frames: i64, pcm_bits: Option<i64>,
) -> Result<(), String> {
    let suffix = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default();
    let verified = match pcm_bits {
        Some(bits @ (16 | 24)) if suffix.eq_ignore_ascii_case("wav") =>
            mono_pcm_packets(path, tool, "wav", samples_or_frames, bits / 8, false),
        Some(24) if suffix.eq_ignore_ascii_case("mxf") =>
            complete_mono_pcm_packets(path, tool, samples_or_frames, 3),
        None if suffix.eq_ignore_ascii_case("mxf") =>
            complete_picture_packets(path, tool, samples_or_frames),
        _ => false,
    };
    verified.then_some(()).ok_or_else(|| "original packet coverage differs from the saved playable extent".into())
}

fn mono_pcm_packets(path: &Path, tool: &Path, demuxer: &str, samples: i64, bytes_per_sample: i64, check_tail: bool) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut command = Command::new(tool);
    let entries = if check_tail { "packet=pts,duration,size,pos" } else { "packet=pts,duration,size" };
    command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-f", demuxer,
        "-select_streams", "a:0", "-show_packets", "-show_entries", entries, "-of", "csv=p=0"]).arg(path);
    let Ok(bytes) = bounded_probe(command, std::time::Duration::from_secs(30)) else { return false };
    let Ok(rows) = std::str::from_utf8(&bytes) else { return false };
    let mut end = 0_i64;
    let mut packet_end = None;
    let mut verified_tail = false;
    for row in rows.lines() {
        let mut fields = row.split(',');
        let (Some(pts), Some(duration), Some(bytes)) = (
            fields.next().and_then(|s| s.parse::<i64>().ok()),
            fields.next().and_then(|s| s.parse::<i64>().ok()),
            fields.next().and_then(|s| s.parse::<i64>().ok()),
        ) else { return false };
        if verified_tail
            || pts != end
            || !anynle_media_verification::pcm_payload_matches(duration, bytes_per_sample, bytes)
        {
            return false;
        }
        if check_tail {
            let Some(pos) = fields.next().and_then(|s| s.parse::<u64>().ok()) else { return false };
            if packet_end.is_some_and(|previous| previous != pos) {
                return false;
            }
            packet_end = pos.checked_add(bytes as u64);
            if packet_end.is_none() {
                return false;
            }
            if end == samples {
                let Ok(mut file) = std::fs::File::open(path) else { return false };
                let mut tail = [0; 6];
                if duration != 2 || fields.next().is_some()
                    || file.seek(SeekFrom::Start(pos)).is_err()
                    || file.read_exact(&mut tail).is_err() || tail != [0; 6]
                {
                    return false;
                }
                verified_tail = true;
                continue;
            }
        }
        if fields.next().is_some() { return false; }
        let Some(next) = anynle_media_verification::pcm_packet_end(end, duration, samples) else { return false };
        end = next;
    }
    end > 0 && end == samples && (!check_tail || verified_tail)
}

pub(super) fn complete_picture_packets(path: &Path, tool: &Path, frames: i64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut command = Command::new(tool);
    command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mxf",
        "-select_streams", "v:0", "-show_packets",
        "-show_entries", "packet=pts,dts,duration,size,pos", "-of", "csv=p=0"]).arg(path);
    let Ok(bytes) = bounded_probe(command, std::time::Duration::from_secs(30)) else { return false };
    let Ok(rows) = std::str::from_utf8(&bytes) else { return false };
    let mut next = 0;
    let mut tail = false;
    let mut last_end = None;
    for row in rows.lines() {
        let mut values = row.split(',').map(|v| v.parse::<i64>().ok());
        if tail || values.next() != Some(Some(next)) || values.next() != Some(Some(next))
            || values.next() != Some(Some(1))
        {
            return false;
        }
        let Some(size) = values.next().flatten().filter(|&size| size > 0) else { return false };
        let Some(pos) = values.next().flatten().filter(|&pos| pos >= 0) else { return false };
        if values.next().is_some() || last_end.is_some_and(|end| pos < end) { return false; }
        if next == frames {
            // A direct link can expose a short zero-only terminal packet.
            // It is outside the admitted edit span, which ends on a coded frame.
            if size > 65_536 || Some(pos) != last_end { return false; }
            let Ok(mut file) = std::fs::File::open(path) else { return false };
            let mut padding = vec![0; size as usize];
            if file.seek(SeekFrom::Start(pos as u64)).is_err()
                || file.read_exact(&mut padding).is_err() || padding.iter().any(|&b| b != 0)
            { return false; }
            tail = true;
        } else {
            last_end = pos.checked_add(size);
            if last_end.is_none() { return false; }
            next += 1;
        }
    }
    next > 0 && next == frames
}

/// Verify the physical picture interpretation required by the Avid source LUT profile.
pub fn verify_source_lut_picture(
    path: &Path, tool: &Path, package: &str, frames: i64,
) -> Result<(), String> {
    let mut command = Command::new(tool);
    command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mxf",
        "-select_streams", "v", "-show_entries",
        "stream=index,codec_name,profile,width,height,sample_aspect_ratio,field_order,pix_fmt,color_range,color_space,color_transfer,color_primaries,r_frame_rate,start_pts,time_base,duration_ts:stream_tags=file_package_umid",
        "-of", "json"]).arg(path);
    let bytes = bounded_probe(command, std::time::Duration::from_secs(30))?;
    let probe = graph::json::parse(std::str::from_utf8(&bytes)
        .map_err(|_| "MXF picture probe output is not UTF-8")?)?;
    let Some(Value::Arr(streams)) = probe.get("streams") else {
        return Err("source LUT picture probe has no stream list".into());
    };
    let [picture] = streams.as_slice() else {
        return Err("source LUT requires one verified picture stream".into());
    };
    let saved_package = package.strip_prefix("urn:smpte:umid:").unwrap_or_default().replace('.', "");
    let actual_package = picture.get("tags").and_then(|tags| text(tags, "file_package_umid"));
    let qualified = [
        ("codec_name", "dnxhd"), ("profile", "DNXHD"), ("sample_aspect_ratio", "1:1"),
        ("field_order", "progressive"), ("pix_fmt", "yuv422p"), ("color_range", "tv"),
        ("color_space", "bt709"), ("color_transfer", "bt709"), ("color_primaries", "bt709"),
        ("r_frame_rate", "24/1"), ("time_base", "1/24"),
    ].iter().all(|(key, expected)| text(picture, key).as_deref() == Some(*expected));
    if qualified && number(picture, "width") == Some(1920)
        && number(picture, "height") == Some(1080) && number(picture, "index") == Some(0)
        && number(picture, "start_pts") == Some(0) && number(picture, "duration_ts") == Some(frames)
        && saved_package.len() == 64 && actual_package.as_deref()
            .and_then(|tag| tag.strip_prefix("0x"))
            .is_some_and(|tag| tag.eq_ignore_ascii_case(&saved_package))
    {
        Ok(())
    } else {
        Err("source picture package, extent or Rec.709 DNxHD interpretation does not match the qualified LUT profile".into())
    }
}

pub fn verify_range_scale_picture(path: &Path, tool: &Path, frames: i64) -> Result<(), String> {
    let metadata = probe_metadata(path, tool)?;
    let Some(Value::Arr(streams)) = metadata.get("streams") else {
        return Err("source range scaling requires a verified ProRes HQ MOV".into());
    };
    qualify_prores_hq(path, &metadata, streams)?;
    let picture = &streams[0];
    if number(picture, "nb_frames") == Some(frames)
        && text(picture, "color_range").as_deref() == Some("tv")
        && text(picture, "color_space").as_deref() == Some("bt709")
    {
        Ok(())
    } else {
        Err("source range scaling requires matching picture extent and explicit Rec.709 video levels".into())
    }
}

/// Verify the color interpretation of a picture-only AVC MOV after exact media binding.
pub fn verify_avc_range_scale_picture(
    path: &Path, tool: &Path, rate: Rational, frames: i64,
) -> Result<(), String> {
    let metadata = probe_metadata(path, tool)?;
    let Some(Value::Arr(streams)) = metadata.get("streams") else {
        return Err("source range scaling requires a verified picture-only AVC MOV".into());
    };
    let [picture] = streams.as_slice() else {
        return Err("source range scaling requires a verified picture-only AVC MOV".into());
    };
    qualify_avc_high(picture, 40)?;
    qualify_movie_geometry_at_rate(picture, (1920, 1080), rate)?;
    if qualify_container(path, &metadata)? == "mov"
        && number(picture, "index") == Some(0)
        && number(picture, "start_pts") == Some(0)
        && number(picture, "nb_frames") == Some(frames)
        && text(picture, "color_range").as_deref() == Some("tv")
        && text(picture, "color_space").as_deref() == Some("bt709")
    {
        Ok(())
    } else {
        Err("source range scaling requires matching picture extent and explicit Rec.709 video levels".into())
    }
}

/// Additional color proof for an already bound picture-only ProRes HQ original.
pub fn verify_extended_sdr_picture(path: &Path, tool: &Path, frames: i64) -> Result<(), String> {
    let metadata = probe_metadata(path, tool)?;
    let Some(Value::Arr(streams)) = metadata.get("streams") else {
        return Err("extended SDR grade requires a verified ProRes HQ MOV".into());
    };
    let defaulted = qualify_prores_hq(path, &metadata, streams)?;
    if defaulted || number(&streams[0], "nb_frames") != Some(frames) {
        return Err("extended SDR grade needs explicit Rec.709 primaries, transfer, matrix, video levels and matching extent".into());
    }
    Ok(())
}

fn has_aac_priming_packet(probe: &Value, stream: i64) -> bool {
    let Some(Value::Arr(packets)) = probe.get("packets") else { return false; };
    let mut audio = packets.iter().filter(|p| number(p, "stream_index") == Some(stream));
    let (Some(first), Some(second)) = (audio.next(), audio.next()) else { return false; };
    number(first, "pts") == Some(-1024) && number(first, "duration") == Some(1024)
        && number(second, "pts") == Some(0)
        && matches!(first.get("side_data_list"), Some(Value::Arr(side)) if side.as_slice().first().is_some_and(|s| {
            side.len() == 1 && text(s, "side_data_type").as_deref() == Some("Skip Samples")
                && number(s, "skip_samples") == Some(1024) && number(s, "discard_padding") == Some(0)
        }))
}

fn same_rate_aac_master(
    master: &av::SourceMaster, samples: i64, picture: &Value, rate: av::Rate,
) -> bool {
    let [a, b, c] = master.master.tracks.as_slice() else { return false; };
    let Some(encoded_length) = samples.checked_add(1024) else { return false; };
    let Some(frames) = number(picture, "nb_frames").filter(|&n| n > 0) else { return false; };
    let legacy = rate == av::Rate::new(24, 1).unwrap();
    let mut lanes = [(a.kind, a.index), (b.kind, b.index), (c.kind, c.index)];
    lanes.sort_unstable();
    lanes == [(avid_reader::extract::KIND_PICTURE, 1),
        (avid_reader::extract::KIND_SOUND, 1), (avid_reader::extract::KIND_SOUND, 2)]
        && master.rate == Some(rate)
        && ratio(picture, "avg_frame_rate") == Some(crate::rational(rate))
        && same_rate_movie::frame_audio_extent(frames, rate, false) == Some(samples)
        && master.master.tracks.iter().all(|t| match t.kind {
            avid_reader::extract::KIND_PICTURE =>
                (if legacy { t.edit_rate == 24.0 } else { av::Rate::from_avid(t.edit_rate) == Some(rate) })
                    && t.length == frames,
            _ => t.edit_rate == 46.875 && t.length == encoded_length,
        })
        && (master.master.stereo_pairs.as_slice() == [(1, 2)]
            || !legacy && master.master.stereo_pairs.is_empty())
        && master.master.other_groups.is_empty()
        && master.offsets.len() == 3
        && master.offsets.iter().all(|o| matches!(o, av::Observed::Known(0)))
        && master.master.markers.is_empty()
        && (legacy || shared_saved_locator(master).is_some()
            && master.originals.iter().zip(&master.master.tracks).any(|(original, track)|
                track.kind == avid_reader::extract::KIND_PICTURE && original.as_ref().is_some_and(|raw|
                    raw.descriptor_class == "CDCI" && raw.display_raster ==
                        number(picture, "width").zip(number(picture, "height"))
                        && av::Rate::from_avid(raw.descriptor_rate) == Some(rate)
                        && raw.descriptor_length == frames && raw.file_track == Some(1))))
        && master.originals.iter().zip(&master.master.tracks)
            .filter(|(_, t)| t.kind == avid_reader::extract::KIND_SOUND)
            .all(|(original, t)| original.as_ref().is_some_and(|raw|
                raw.descriptor_class == "MPGA" && raw.descriptor_rate == 46.875
                    && raw.descriptor_length == encoded_length
                    && raw.sample_rate == Some(48000.0) && raw.channels == Some(2)
                    && raw.motion_audio_track_start == Some(1) && raw.file_track == Some(t.index)))
}

fn qualify_uhd_avc_aac(probe: &Value, streams: &[Value], master: &av::SourceMaster) -> Result<bool, String> {
    let [video, audio] = streams else {
        return Err("UHD AVC AAC MP4 requires one picture stream and one audio stream".into());
    };
    qualify_avc_high(video, 51)?;
    let defaulted = qualify_movie_geometry(video, (3840, 2160), true)?;
    qualify_stereo_aac_lc(audio)?;
    if number(video, "index") != Some(0) || number(audio, "index") != Some(1)
        || !has_aac_priming_packet(probe, 1)
        || !number(audio, "duration_ts").is_some_and(|samples|
            same_rate_aac_master(master, samples, video, av::Rate::new(24_000, 1001).unwrap()))
    {
        return Err("UHD AVC AAC MP4 requires matching 23.976 descriptors, two saved channels and verified AAC priming".into());
    }
    Ok(defaulted)
}
impl OriginalIndex {
    fn omit_unused(&mut self, master: &av::SourceMaster, reason: String, used: &mut usize) -> Result<(), String> {
        *used = used.checked_add(master.id.len() + reason.len() + 128)
            .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
            .ok_or("original omissions exceed 64 MiB")?;
        self.omitted_unused.insert(master.id.clone(), reason);
        Ok(())
    }

    fn omit_media(&mut self, master: &av::SourceMaster, reason: String, used: &mut usize) -> Result<(), String> {
        *used = used.checked_add(master.id.len() + reason.len() + 128)
            .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
            .ok_or("original omissions exceed 64 MiB")?;
        self.omitted_media.insert(master.id.clone(), reason);
        Ok(())
    }

    pub fn scan(facts: &av::SourceProject, options: &OriginalOptions) -> Result<Self, String> {
        Self::scan_for_review(facts, options, &mut || Ok(())).map_err(SourceReadIssue::into_message)
    }
    #[cfg(test)]
    pub(crate) fn scan_with(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        mut inspect: impl FnMut(&Path) -> Result<Value, String>,
    ) -> Result<Self, String> {
        Self::scan_with_control(
            facts, false, None,
            options,
            |path, _| {
                inspect(path).map(|metadata| OriginalProbe {
                    metadata,
                    prores_color_defaulted: None,
                    mp3: None,
                    still_decoded: None,
                })
            },
            &mut || Ok(()),
        )
        .map_err(SourceReadIssue::into_message)
    }
    pub fn scan_controlled(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::scan_for_review(facts, options, check).map_err(SourceReadIssue::into_message)
    }
    pub(crate) fn scan_for_review(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, SourceReadIssue> {
        let mut originals = Self::scan_for_review_with_policy(facts, options, false, None, check)?;
        let index = Default::default();
        let selected_media = SelectedMediaIndex::new(facts, &originals, &index, check)?;
        originals.resolve_legacy_mxf(facts, &selected_media, options, check)?;
        Ok(originals)
    }

    pub(super) fn resolve_legacy_mxf(
        &mut self, facts: &av::SourceProject, selected_media: &SelectedMediaIndex<'_>,
        options: &OriginalOptions, check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), SourceReadIssue> {
        if self.legacy_mxf.is_empty() { return Ok(()); }
        let index = selected_media.index;
        let permits_missing_media_fallbacks = selected_media.permits_missing_media_fallbacks();
        let collect_blocked = options.collect_blocked_locators
            && !self.allow_offline
            && !options.allow_media_gaps
            && self.references.is_none();
        let mut used = 0;
        for (ordinal, master) in facts.masters.iter().enumerate() {
            let Some(directory) = self.legacy_mxf.get(&master.id) else { continue };
            let issue = |message| OriginalLocatorIssue::new(
                OriginalLocatorKind::LegacyMxfPackage,
                format!("{:?} · {directory}: {message}", master.master.name),
                master.master.bin,
            );
            let (reason, primary_track, mut tracks) = match legacy_mxf::missing_siblings(master, index, check,
                &mut |path| probe_offline_atom(path, &options.ffprobe)) {
                Ok(missing) if missing.is_empty() => continue,
                Ok(missing) => {
                    let names = missing.iter().map(|finding| finding.message.as_str()).collect::<Vec<_>>();
                    let reason = format!("Legacy MXF locators require the saved file packages in a selected media root. Missing: {}.", names.join(", "));
                    if permits_missing_media_fallbacks {
                        if self.references.as_ref().is_some_and(|refs| refs.can_omit(master)) {
                            self.omit_unused(master, reason, &mut used)?;
                            continue;
                        }
                        if options.allow_media_gaps {
                            self.omit_media(master, reason, &mut used)?;
                            continue;
                        }
                    }
                    let tracks = missing.into_iter().map(|finding| (ordinal, finding.track)).collect::<Vec<_>>();
                    (reason, None, tracks)
                }
                Err(legacy_mxf::SiblingIssue::Package { track, message }) => {
                    (message, Some(track), vec![(ordinal, track)])
                }
                Err(legacy_mxf::SiblingIssue::Check(reason)) => {
                    return Err(SourceReadIssue::OriginalLocator(issue(reason)));
                }
            };
            let mut issue = issue(reason);
            if collect_blocked {
                let mut details = LocatorDetails::default();
                let conflicts = index.package_conflicts(check)?;
                let mut reported_conflicts = HashSet::new();
                // A missing-package summary already lists every absent lane in
                // its clip. A refusal on one lane can leave other lanes unlisted.
                let start = if primary_track.is_some() { ordinal } else { ordinal + 1 };
                'affected: for (offset, later) in facts.masters[start..].iter().enumerate() {
                    check()?;
                    let Some(directory) = self.legacy_mxf.get(&later.id) else { continue };
                    let exclude = if offset == 0 { primary_track } else { None };
                    let findings = legacy_mxf::package_index_findings(later, index, exclude);
                    if !findings.is_empty() {
                        let messages = findings.iter().map(|finding| finding.message.as_str()).collect::<Vec<_>>();
                        let detail = format!("{:?} · {directory}: {} · {}", later.master.name,
                            messages.join("; "), later.id);
                        if !details.push(later.master.bin, detail) {
                            break;
                        }
                        tracks.extend(findings.into_iter().map(|finding| (start + offset, finding.track)));
                    }
                    for track in &later.master.tracks {
                        let package = track.file_mob.umid_hex();
                        let Some((&package, conflict)) = conflicts.get_key_value(package.as_str()) else {
                            continue;
                        };
                        if !reported_conflicts.insert(package) {
                            continue;
                        }
                        let Some(detail) = conflict.detail(package) else { continue };
                        if !details.push(later.master.bin, detail) {
                            break 'affected;
                        }
                    }
                }
                issue.additional = details.rows;
                issue.details_truncated = details.truncated;
            }
            if options.collect_blocked_locators {
                issue.edits = super::locator_uses::collect_tracks(facts, tracks, check)?;
            }
            return Err(SourceReadIssue::OriginalLocator(issue));
        }
        Ok(())
    }
    pub(super) fn scan_for_review_with_policy(
        facts: &av::SourceProject, options: &OriginalOptions, allow_offline: bool,
        references: Option<&crate::unused::References>,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, SourceReadIssue> {
        Self::scan_with_control(facts, allow_offline, references, options,
            |path, check| probe_with_linked_imports(path, &options.ffprobe, options.verify_linked_imports, check), check)
    }
    fn scan_with_control(
        facts: &av::SourceProject,
        allow_offline: bool,
        references: Option<&crate::unused::References>,
        options: &OriginalOptions,
        mut inspect: impl FnMut(&Path, &mut dyn FnMut() -> Result<(), String>) -> Result<OriginalProbe, String>,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, SourceReadIssue> {
        let mut out = Self {
            allow_offline, allow_media_gaps: options.allow_media_gaps,
            preserve_managed_audio_without_picture: options.preserve_managed_audio_without_picture,
            ..Self::default()
        };
        // Keep one additional file for interleaved selections without retaining
        // probe metadata for the entire bin.
        const PROBE_CACHE_FILES: usize = 2;
        let mut probes = VecDeque::<(PathBuf, OriginalProbe)>::with_capacity(PROBE_CACHE_FILES);
        let mut used = 0usize;
        let mut pinned = HashMap::<PathBuf, String>::new();
        let mut locator_claims = HashMap::<LocatorClaimKey, LocatorClaim>::new();
        let collect_blocked = options.collect_blocked_locators
            && !allow_offline
            && !options.allow_media_gaps
            && references.is_none();
        let adapter_masters: std::collections::HashSet<_> = facts.sequences.iter()
            .flat_map(|s| &s.events)
            .filter(|e| e.motion_adapter_phase.is_some())
            .map(|e| e.event.master)
            .collect();
        'master: for (master_index, m) in facts.masters.iter().enumerate() {
            check()?;
            if m.pcm_prefix.is_some() || m.originals.iter().all(Option::is_none) {
                continue;
            }
            if m.originals.len() != m.master.tracks.len() || m.originals.iter().any(Option::is_none)
            {
                return Err("mixed or unresolved original descriptor routes in one master".into());
            }
            if let Some(parent) = legacy_mxf::saved_sibling_directory(m) {
                used = used.checked_add(m.id.len() + parent.len() + 128)
                    .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
                    .ok_or("legacy MXF locator index exceeds 64 MiB")?;
                out.legacy_mxf.insert(m.id.clone(), parent.into());
                continue 'master;
            }
            let unused_path = references.filter(|refs| refs.can_omit(m))
                .and_then(|_| shared_saved_locator(m));
            let gap_path = options.allow_media_gaps.then(|| gap_saved_locator(m)).flatten();
            for (i, raw) in m.originals.iter().enumerate() {
                check()?;
                let raw = raw.as_ref().unwrap();
                let t = &m.master.tracks[i];
                let locator_issue = |kind, why| {
                    let saved = raw.locators.first().map_or("no saved path", String::as_str);
                    OriginalLocatorIssue::new(kind, format!("{:?} · {saved}: {why}", m.master.name), m.master.bin)
                };
                let locator_error = |why| {
                    SourceReadIssue::OriginalLocator(locator_issue(OriginalLocatorKind::SavedPath, why))
                };
                let blocked = |kind, why, check: &mut dyn FnMut() -> Result<(), String>| {
                    let mut issue = locator_issue(kind, why);
                    if !options.collect_blocked_locators {
                        return Ok(SourceReadIssue::OriginalLocator(issue));
                    }
                    // A malformed mapping is a job setting, not an edit-specific failure.
                    let Ok(mappings) = check_root_mappings(&options.root_mappings) else {
                        return Ok(SourceReadIssue::OriginalLocator(issue));
                    };
                    let details = if collect_blocked {
                        blocked_locator_details(&facts.masters[master_index..], i, &mappings, check)?
                    } else {
                        LocatorDetails::default()
                    };
                    let tracks = std::iter::once((master_index, i)).chain(
                        details.original_tracks.into_iter().map(|(offset, track)| {
                            (master_index + offset, track)
                        }),
                    );
                    issue.edits = super::locator_uses::collect_saved_paths(facts, tracks, check)?;
                    issue.additional = details.rows;
                    issue.details_truncated = details.truncated;
                    Ok::<_, SourceReadIssue>(SourceReadIssue::OriginalLocator(issue))
                };
                let found = match resolve_with_claim(&raw.locators, &options.root_mappings) {
                    Ok(found) => found,
                    Err(why) => return Err(blocked(OriginalLocatorKind::SavedPath, why, check)?),
                };
                let Some((path, claim)) = found else {
                    if let Some(path) = unused_path.filter(|_| i == 0) {
                        let offline_profile_unavailable = allow_offline && {
                            let mut unsupported = false;
                            (0..m.originals.len()).all(|track| match offline_original(m, track) {
                                Ok(Some(_)) => true,
                                Ok(None) => {
                                    unsupported = true;
                                    true
                                }
                                Err(_) => false,
                            }) && unsupported
                        };
                        if !allow_offline || offline_profile_unavailable {
                            let reason = if offline_profile_unavailable {
                                format!("Original unavailable at {path}. {OFFLINE_ORIGINAL_PROFILE}.")
                            } else {
                                format!("Original unavailable at {path}.")
                            };
                            out.omit_unused(m, reason, &mut used)?;
                            continue 'master;
                        }
                    }
                    if let Some(path) = gap_path.filter(|_| i == 0) {
                        // Keep qualified offline items. For a master outside that
                        // profile, omit all siblings together before staging any.
                        let unsupported = if allow_offline {
                            let mut unsupported = false;
                            for track in 0..m.originals.len() {
                                unsupported |= offline_original(m, track).map_err(&locator_error)?.is_none();
                            }
                            unsupported
                        } else {
                            true
                        };
                        if unsupported {
                            out.omit_media(m, format!("Original unavailable at {path}."), &mut used)?;
                            continue 'master;
                        }
                    }
                    if !allow_offline {
                        return Err(blocked(OriginalLocatorKind::MissingOriginal, missing_locator(), check)?);
                    }
                    let selected = offline_original(m, i)
                        .and_then(|selected| selected.ok_or_else(|| format!(
                            "{OFFLINE_ORIGINAL_PROFILE}. Saved layout: {}", saved_original_layout(m)
                        )))
                        .map_err(|e| locator_error(format!(
                        "{e} ({} track {}, {} at {} Hz, file track {:?})",
                        raw.descriptor_class, t.index, raw.descriptor_length, raw.descriptor_rate, raw.file_track
                    )))?;
                    used = used.checked_add(selection_cost(m, raw, &selected.path))
                        .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
                        .ok_or("original index exceeds 64 MiB")?;
                    out.tracks.insert((m.id.clone(), i), selected);
                    continue;
                };
                if !options.root_mappings.is_empty() || options.prefer_source_media {
                    let key = locator_claim_key(&path).map_err(locator_error)?;
                    match locator_claims.entry(key) {
                        Entry::Occupied(mut entry) => {
                            let previous = entry.get_mut();
                            // Require a common witness across all selections of this path.
                            // An intermediate row with several aliases cannot join two sources.
                            previous.intersect(&claim);
                            if previous.mapped && previous.saved_paths.is_empty() {
                                return Err(locator_error(format!(
                                    "root mappings resolve distinct saved originals to {}; check the mappings",
                                    path.display()
                                )));
                            }
                        }
                        Entry::Vacant(entry) => {
                            used = used
                                .checked_add(
                                    std::mem::size_of::<(LocatorClaimKey, LocatorClaim)>()
                                        + path.as_os_str().len()
                                        + claim.saved_paths.capacity() * std::mem::size_of::<String>()
                                        + claim.saved_paths.iter().map(String::capacity).sum::<usize>(),
                                )
                                .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
                                .ok_or("original locator claims exceed 64 MiB")?;
                            entry.insert(claim);
                        }
                    }
                }
                let describe = |why: String| format!("{:?} · {}: {why}", m.master.name, path.display());
                let cached = probes.iter().position(|(p, _)| p == &path);
                let fresh_probe = cached.is_none();
                if let Some(index) = cached {
                    let probe = probes.remove(index).unwrap();
                    probes.push_front(probe);
                } else {
                    if probes.len() == PROBE_CACHE_FILES {
                        probes.pop_back();
                    }
                    // Pin bytes on both sides of the probe. Neither mtime nor a post-probe
                    // hash alone establishes which bytes supplied the stream facts.
                    let before = crate::revision::file_revision_checked(&path, check)
                        .map_err(|e| format!("original revision read failed: {e}"))?;
                    if pinned.get(&path).is_some_and(|old| old != &before) {
                        return Err("original content changed between source selections".into());
                    }
                    let observed = inspect(&path, check).map_err(describe)?;
                    let after = crate::revision::file_revision_checked(&path, check)
                        .map_err(|e| format!("original revision recheck failed: {e}"))?;
                    if before != after {
                        return Err("original content changed during stream probe".into());
                    }
                    if !pinned.contains_key(&path) {
                        used = used
                            .checked_add(
                                2 * (std::mem::size_of::<av::SourceInput>()
                                    + path.as_os_str().len()
                                    + 64),
                            )
                            .ok_or("original revision budget overflow")?;
                        if used > MAX_ORIGINAL_INDEX_BYTES {
                            return Err("original revisions exceed 64 MiB".into());
                        }
                        pinned.insert(path.clone(), before.clone());
                        out.original_inputs.push(av::SourceInput {
                            path: path.clone(),
                            revision: before,
                        });
                    }
                    probes.push_front((path.clone(), observed));
                }
                let physical = &probes.front().unwrap().1;
                let v = &physical.metadata;
                if !adapter_masters.contains(&m.master.mob_id) {
                    let direct = same_rate_movie::select(&path, v, m, i);
                    let direct = if options.verify_linked_imports && direct.is_err()
                        && same_rate_movie::may_need_import(&path, v)
                    {
                        match same_rate_movie::select_verified_import(
                            &path, v, m, i, &options.ffprobe, check,
                        ) {
                            Ok(value) => Ok(Some(value)),
                            Err(reason) => match direct {
                                Err(previous) => Err(previous),
                                _ => Err(reason),
                            },
                        }
                    } else {
                        direct
                    };
                    let selected = match direct {
                        Err(reason) if i == 0 && (gap_path.is_some()
                            || unused_path.is_some() && gap_saved_locator(m).is_some())
                            && matches!(v.get("streams"), Some(Value::Arr(streams)) if unsupported_pcm_movie(m, streams)) =>
                        {
                            let reason = describe(reason);
                            if unused_path.is_some() {
                                out.omit_unused(m, reason, &mut used)?;
                            } else {
                                out.omit_media(m, reason, &mut used)?;
                            }
                            continue 'master;
                        }
                        result => result.map_err(&describe)?,
                    };
                    if let Some((selected, defaulted)) = selected {
                        used = used.checked_add(selection_cost(m, raw, &path))
                            .and_then(|size| size.checked_add(if i == 0 && defaulted {
                                std::mem::size_of::<String>() + UNTAGGED_COLOR_NOTE.len()
                            } else { 0 }))
                            .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
                            .ok_or("original index exceeds 64 MiB")?;
                        if i == 0 && defaulted { out.notes.push(UNTAGGED_COLOR_NOTE.into()); }
                        out.tracks.insert((m.id.clone(), i), selected);
                        continue;
                    }
                }
                if let Some(selected) = additional_media::select(m, i, &path, physical).map_err(&describe)? {
                    used = used.checked_add(selection_cost(m, raw, &path))
                        .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
                        .ok_or("original index exceeds 64 MiB")?;
                    out.tracks.insert((m.id.clone(), i), selected);
                    continue;
                }
                let physical_container = match qualify_container(&path, v) {
                    Err(e) if e == NO_CONTAINER_ROUTE && i == 0 && unused_path.is_some() => {
                        out.omit_unused(m, describe(e), &mut used)?;
                        continue 'master;
                    }
                    Err(e) if e == NO_CONTAINER_ROUTE && i == 0 && gap_path.is_some() => {
                        out.omit_media(m, describe(e), &mut used)?;
                        continue 'master;
                    }
                    otherwise => otherwise.map_err(&describe)?,
                };
                let Value::Arr(streams) = v.get("streams").ok_or("probe lacks streams")? else {
                    return Err("probe streams malformed".into());
                };
                if streams.len() > 3 {
                    if i == 0 && (unused_path.is_some() || gap_path.is_some()) && streams.iter().all(|stream|
                        number(stream, "index").is_some_and(|n| n >= 0)
                            && text(stream, "codec_type").is_some())
                    {
                        let reason = describe("original has unqualified additional streams".into());
                        if unused_path.is_some() {
                            out.omit_unused(m, reason, &mut used)?;
                        } else {
                            out.omit_media(m, reason, &mut used)?;
                        }
                        continue 'master;
                    }
                    return Err("original has unqualified additional streams".into());
                }
                let videos: Vec<_> = streams
                    .iter()
                    .filter(|s| text(s, "codec_type").as_deref() == Some("video"))
                    .collect();
                let audios: Vec<_> = streams
                    .iter()
                    .filter(|s| text(s, "codec_type").as_deref() == Some("audio"))
                    .collect();
                if videos.len() > 1
                    || audios.len() > 1
                    || streams.iter().any(|s| {
                        !matches!(text(s, "codec_type").as_deref(), Some("video" | "audio"))
                            && text(s, "codec_tag_string").as_deref() != Some("tmcd")
                    })
                {
                    return Err("original stream selection is ambiguous/unqualified".into());
                }
                let mut picture_codec = "avc1";
                let avc_pcm = physical_container == "mov" && is_avc_pcm_movie(streams)
                    && !adapter_masters.contains(&m.master.mob_id);
                let uhd_avc_mp4 = physical_container == "mov"
                    && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
                    && videos.first().is_some_and(|picture|
                        number(picture, "width") == Some(3840) && number(picture, "height") == Some(2160)
                            && text(picture, "codec_name").as_deref() == Some("h264"))
                    && !adapter_masters.contains(&m.master.mob_id);
                let uhd_avc_aac = uhd_avc_mp4 && audios.first().is_some_and(|audio|
                    text(audio, "codec_tag_string").as_deref() == Some("mp4a"));
                if physical_container == "mov" {
                    let video = videos
                        .first()
                        .ok_or("original movie picture stream absent")?;
                    let picture_profile = if text(video, "codec_name").as_deref() == Some("prores") {
                        picture_codec = if text(video, "codec_tag_string").as_deref() == Some("apcs") {
                            "apcs"
                        } else {
                            "apch"
                        };
                        if m.master.tracks.len() != 1
                            || raw.descriptor_class != "CDCI"
                            || raw.descriptor_length != t.length
                            || raw.descriptor_rate != 24.0
                            || raw.file_track != Some(1)
                            || !matches!(&t.essence, Some(avid_reader::extract::Essence::Picture {
                                width: 1920, height: 1080, ..
                            }))
                            || m.master.start.as_ref().is_some_and(|tc| tc.frames != 0 || tc.fps != 24 || tc.drop)
                        {
                            Err("linked ProRes requires a matching saved 1080p24 picture-only descriptor and zero timecode origin".into())
                        } else {
                            physical.prores_color_defaulted
                                .ok_or_else(|| "linked ProRes has no complete verified picture packet clock".into())
                        }
                    } else if avc_pcm {
                        qualify_avc_pcm(streams, &path, m)
                    } else if uhd_avc_aac {
                        qualify_uhd_avc_aac(v, streams, m)
                    } else {
                        qualify_movie(video, audios.first().copied(), adapter_masters.contains(&m.master.mob_id))
                    };
                    if picture_profile.is_err() && options.verify_linked_imports
                        && !adapter_masters.contains(&m.master.mob_id)
                        && same_rate_movie::may_need_import(&path, v)
                    {
                        let imported = same_rate_movie::select_verified_import(
                            &path, v, m, i, &options.ffprobe, check,
                        );
                        if let Ok((selected, defaulted)) = imported {
                            used = used.checked_add(selection_cost(m, raw, &path))
                                .and_then(|size| size.checked_add(if i == 0 && defaulted {
                                    std::mem::size_of::<String>() + UNTAGGED_COLOR_NOTE.len()
                                } else { 0 }))
                                .filter(|n| *n <= MAX_ORIGINAL_INDEX_BYTES)
                                .ok_or("original index exceeds 64 MiB")?;
                            if i == 0 && defaulted { out.notes.push(UNTAGGED_COLOR_NOTE.into()); }
                            out.tracks.insert((m.id.clone(), i), selected);
                            continue;
                        }
                        if let Err(reason) = imported {
                            if !(i == 0 && (gap_path.is_some() || unused_path.is_some()) && unsupported_pcm_movie(m, streams)) {
                                return Err(describe(reason).into());
                            }
                        }
                    }
                    let defaulted = match picture_profile {
                        Ok(defaulted) => defaulted,
                        Err(reason) if i == 0 && (gap_path.is_some()
                            || unused_path.is_some() && gap_saved_locator(m).is_some())
                            && unsupported_pcm_movie(m, streams) =>
                        {
                            let reason = describe(reason);
                            if unused_path.is_some() {
                                out.omit_unused(m, reason, &mut used)?;
                            } else {
                                out.omit_media(m, reason, &mut used)?;
                            }
                            continue 'master;
                        }
                        Err(reason) => return Err(describe(reason).into()),
                    };
                    if defaulted && fresh_probe {
                        used = used
                            .checked_add(std::mem::size_of::<String>() + UNTAGGED_COLOR_NOTE.len())
                            .ok_or("original note budget overflow")?;
                        if used > MAX_ORIGINAL_INDEX_BYTES {
                            return Err("original notes exceed 64 MiB".into());
                        }
                        out.notes.push(UNTAGGED_COLOR_NOTE.into());
                    }
                }
                let (stream, kind) = if t.kind == avid_reader::extract::KIND_PICTURE {
                    (
                        *videos.first().ok_or("original picture stream absent")?,
                        StreamKind::Video,
                    )
                } else {
                    (
                        *audios.first().ok_or("original audio stream absent")?,
                        StreamKind::Audio,
                    )
                };
                let index = number(stream, "index").ok_or("original stream index absent")?;
                let tb = ratio(stream, "time_base").ok_or("original stream clock absent")?;
                let len = number(stream, "duration_ts")
                    .filter(|n| *n > 0)
                    .ok_or("original stream extent absent")?;
                let start = match number(stream, "start_pts") {
                    Some(n) => n,
                    None if physical_container == "wav" => 0,
                    None => return Err("original stream origin absent".into()),
                };
                if start != 0 {
                    return Err(
                        "nonzero original stream origins require separate qualification".into(),
                    );
                }
                let codec = text(stream, "codec_name").ok_or("original codec absent")?;
                let (container, representation, bits) =
                    match (physical_container, codec.as_str(), kind) {
                        ("mov", "h264", StreamKind::Video) => ("mov", None, None),
                        ("mov", "prores", StreamKind::Video) => ("mov", None, None),
                        ("mov", "aac", StreamKind::Audio) if videos.len() == 1 => {
                            ("mov", Some("aac"), None)
                        }
                        ("mov", "pcm_s16le", StreamKind::Audio) if avc_pcm => {
                            ("mov", Some("pcm16"), Some(16))
                        }
                        ("mov", "pcm_s24le" | "pcm_s24be", StreamKind::Audio) if avc_pcm => {
                            ("mov", Some("pcm24"), Some(24))
                        }
                        ("wav", "pcm_s16le", StreamKind::Audio) if videos.is_empty() => {
                            ("wav", Some("pcm16"), Some(16))
                        }
                        ("wav", "pcm_s24le", StreamKind::Audio) if videos.is_empty() => {
                            ("wav", Some("pcm24"), Some(24))
                        }
                        _ => return Err("original container/codec route is not qualified".into()),
                    };
                used = used
                    .checked_add(selection_cost(m, raw, &path))
                    .ok_or("original index budget overflow")?;
                if used > MAX_ORIGINAL_INDEX_BYTES {
                    return Err("original index exceeds 64 MiB".into());
                }
                let mut selected = Selected {
                    verified_for_native_import: false,
                    path: path.clone(),
                    online: true,
                    stream: OriginalStream {
                        saved_locators: raw.locators.clone(),
                        container: container.into(),
                        stream_index: index,
                        channel_index: None,
                        channels: None,
                        time_base: tb,
                        start_pts: start,
                        duration_ts: len,
                        sample_representation: representation.map(str::to_owned),
                    },
                    kind,
                    codec: if kind == StreamKind::Video {
                        picture_codec.into()
                    } else {
                        codec
                    },
                    raster: None,
                    rate: None,
                    sample_rate: None,
                    bits,
                    duration: Rational::new(len as i128 * tb.num() as i128, tb.den() as i128)
                        .ok_or("original duration overflow")?,
                    motion_audio_origin: None,
                    same_rate_aac_origin: false,
                    mp3_delay_samples: None,
                    column_timecode: None,
                    embedded_mov_timecode: None,
                };
                if kind == StreamKind::Video {
                    let rate =
                        ratio(stream, "avg_frame_rate").ok_or("original frame rate absent")?;
                    if text(stream, "codec_tag_string").as_deref() != Some(picture_codec)
                        || Some(rate) != ratio(stream, "r_frame_rate")
                        || index != 0
                        || text(stream, "field_order").as_deref() != Some("progressive")
                        || text(stream, "sample_aspect_ratio").as_deref() != Some("1:1")
                        || number(stream, "nb_frames") != Some(t.length)
                        || av::Rate::from_avid(t.edit_rate).map(|r| (r.num, r.den))
                            != Some((rate.num(), rate.den()))
                    {
                        return Err(
                            "original picture rate/extent/scan qualification mismatch".into()
                        );
                    }
                    if selected.duration
                        != Rational::new(t.length as i128 * rate.den() as i128, rate.num() as i128)
                            .ok_or("original frame duration overflow")?
                    {
                        return Err(
                            "original picture duration disagrees with frame extent/rate".into()
                        );
                    }
                    selected.rate = Some(rate);
                    if picture_codec == "apcs" {
                        selected.embedded_mov_timecode = Some(av::Observed::Known(0));
                    }
                    if container == "mov" && picture_codec == "avc1"
                        && rate == Rational::integer(24) && audios.len() == 1
                    {
                        selected.embedded_mov_timecode = embedded_avc_mov_timecode(v, streams, stream);
                    }
                    selected.raster = Some((
                        number(stream, "width").ok_or("original width absent")?,
                        number(stream, "height").ok_or("original height absent")?,
                    ));
                } else {
                    let sr = number(stream, "sample_rate").ok_or("original sample clock absent")?;
                    let channels = number(stream, "channels").ok_or("original channels absent")?;
                    let has_priming = representation == Some("aac")
                        && raw.motion_audio_track_start == Some(1)
                        && raw.descriptor_rate == 46.875 && sr == 48000 && channels == 2
                        && len.checked_add(1024) == Some(raw.descriptor_length)
                        && has_aac_priming_packet(v, index);
                    let qualified_priming = has_priming && adapter_masters.contains(&m.master.mob_id);
                    let same_rate_priming = has_priming
                        && !adapter_masters.contains(&m.master.mob_id)
                        && (uhd_avc_aac || path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mov"))
                            && videos.first().is_some_and(|picture|
                                same_rate_aac_master(m, len, picture, av::Rate::new(24, 1).unwrap())));
                    if !matches!(channels, 1 | 2)
                        || raw.channels != Some(channels)
                        || raw.sample_rate != Some(sr as f64)
                        || tb != Rational::new(1, sr as i128).ok_or("invalid sample rate")?
                        || (raw.descriptor_length != len && !qualified_priming && !same_rate_priming)
                        || index != if videos.is_empty() { 0 } else { 1 }
                    {
                        return Err(
                            "original audio descriptor/physical sample-clock mismatch".into()
                        );
                    }
                    if representation == Some("aac") && raw.descriptor_class != "MPGA"
                        || representation != Some("aac")
                            && (raw.descriptor_class != "PCMA"
                                || raw.stored_quantization_bits != bits)
                    {
                        return Err("original audio descriptor representation mismatch".into());
                    }
                    let ch = raw
                        .file_track
                        .ok_or("original source channel route missing")?
                        - 1;
                    if !(0..channels).contains(&ch) {
                        return Err("original source channel route outside physical stream".into());
                    }
                    selected.sample_rate = Some(sr);
                    selected.stream.channels = Some(channels);
                    selected.stream.channel_index = Some(ch);
                    if qualified_priming {
                        selected.motion_audio_origin = raw.motion_audio_track_start;
                    }
                    selected.same_rate_aac_origin = same_rate_priming;
                }
                out.tracks.insert((m.id.clone(), i), selected);
            }
        }
        if options.prefer_source_media && !options.attach_avid_proxies {
            source_media::select(
                facts, options, &mut out, &mut locator_claims, &mut pinned, &mut used,
                check, None,
            )?;
        }
        // Recheck every selected file, including cache hits. Publication rechecks these pins.
        for input in &out.original_inputs {
            if crate::revision::file_revision_checked(&input.path, check)
                .map_err(|e| format!("original final revision read failed: {e}"))?
                != input.revision
            {
                return Err("original content changed before source read completed".into());
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_avc_clock_requires_agreeing_timecode_witnesses() {
        let fixture = r#"{
            "format":{"tags":{"major_brand":"qt  "}},
            "streams":[
                {"time_base":"1/12288","duration_ts":24576,"tags":{"timecode":"10:02:03:04"}},
                {"index":1,"codec_type":"audio"},
                {"index":2,"codec_type":"data","codec_tag_string":"tmcd","nb_frames":"1",
                 "avg_frame_rate":"12288/512","time_base":"1/12288","start_pts":0,
                 "duration_ts":24576,"tags":{"timecode":"10:02:03:04"}}
            ]
        }"#;
        let observe = |json: &str| {
            let probe = graph::json::parse(json).unwrap();
            let Value::Arr(streams) = probe.get("streams").unwrap() else { panic!("expected streams") };
            embedded_avc_mov_timecode(&probe, streams, &streams[0])
        };
        assert!(matches!(observe(fixture), Some(av::Observed::Known(866_956))));
        for invalid in [
            fixture.replacen("10:02:03:04", "10:02:03:05", 1),
            fixture.replace("10:02:03:04", "10:02:03;04"),
            fixture.replace("10:02:03:04", "10:02:03:24"),
            fixture.replace("\"start_pts\":0", "\"start_pts\":1"),
        ] {
            assert!(matches!(observe(&invalid), Some(av::Observed::Unresolved(_))));
        }
    }

    #[test]
    fn original_adapter_priming_needs_observed_skip_packet_and_playable_zero() {
        let raw = r#"{"packets":[{"stream_index":0,"pts":0,"duration":512},
            {"stream_index":1,"pts":-1024,"duration":1024,"side_data_list":[
              {"side_data_type":"Skip Samples","skip_samples":1024,"discard_padding":0}]},
            {"stream_index":1,"pts":0,"duration":1024}]}"#;
        let parse = |s: &str| graph::json::parse(s).unwrap();
        assert!(has_aac_priming_packet(&parse(raw), 1));
        assert!(!has_aac_priming_packet(&parse(raw), 0));
        assert!(!has_aac_priming_packet(&parse(&raw.replace("\"skip_samples\":1024", "\"skip_samples\":0")), 1));
        assert!(!has_aac_priming_packet(&parse(&raw.replace("\"stream_index\":1,\"pts\":0", "\"stream_index\":1,\"pts\":1024")), 1));
    }
    fn picture_profile() -> Value {
        graph::json::parse(r#"{"codec_name":"h264","codec_tag_string":"avc1","profile":"High","level":40,"pix_fmt":"yuv420p","width":1920,"height":1080,"avg_frame_rate":"24/1","r_frame_rate":"24/1","field_order":"progressive","sample_aspect_ratio":"1:1"}"#).unwrap()
    }
    fn audio_profile() -> Value {
        graph::json::parse(r#"{"codec_name":"aac","codec_tag_string":"mp4a","profile":"LC","sample_rate":"48000","channels":2,"channel_layout":"stereo"}"#).unwrap()
    }
    fn set(v: &mut Value, key: &str, value: Value) {
        let Value::Obj(rows) = v else { panic!() };
        if let Some((_, old)) = rows.iter_mut().find(|(k, _)| k == key) {
            *old = value;
        } else {
            rows.push((key.into(), value));
        }
    }
    #[test]
    fn physical_container_family_requires_probe_evidence_matching_the_route() {
        fn probe(name: Value) -> Value {
            Value::Obj(vec![(
                "format".into(),
                Value::Obj(vec![("format_name".into(), name)]),
            )])
        }
        for (suffix, name, expected) in [
            ("wav", "wav", "wav"),
            ("WAV", "wav", "wav"),
            ("mov", "mov", "mov"),
            ("mp4", "mov,mp4,m4a,3gp,3g2,mj2", "mov"),
            ("MOV", "mov,mp4,m4a,3gp,3g2,mj2", "mov"),
        ] {
            assert_eq!(
                qualify_container(
                    Path::new(&format!("/Synthetic/source.{suffix}")),
                    &probe(Value::Str(name.into()))
                )
                .unwrap(),
                expected
            );
        }
        for suffix in ["wav", "mov", "mp4"] {
            let path = PathBuf::from(format!("/Synthetic/source.{suffix}"));
            for name in [
                "matroska,webm",
                "unknown",
                "",
                "mov,matroska",
                "mov,,mp4",
                "wav,mov",
            ] {
                assert!(
                    qualify_container(&path, &probe(Value::Str(name.into()))).is_err(),
                    "accepted {suffix}/{name}"
                );
            }
            for value in [
                Value::Null,
                probe(Value::Null),
                Value::Obj(vec![("format".into(), Value::Obj(vec![]))]),
                Value::Obj(vec![]),
            ] {
                assert!(qualify_container(&path, &value).is_err());
            }
        }
        assert!(qualify_container(
            Path::new("/Synthetic/source.wav"),
            &probe(Value::Str("mov,mp4,m4a,3gp,3g2,mj2".into()))
        )
        .is_err());
        for suffix in ["mov", "mp4"] {
            assert!(qualify_container(
                Path::new(&format!("/Synthetic/source.{suffix}")),
                &probe(Value::Str("wav".into()))
            )
            .is_err());
        }
    }
    #[test]
    fn initial_movie_profile_distinguishes_explicit_color_from_destination_defaults() {
        let base = picture_profile();
        let audio = audio_profile();
        assert_eq!(qualify_movie(&base, Some(&audio), false).unwrap(), true);
        assert_eq!(qualify_movie(&base, None, false).unwrap(), true);
        let mut tagged = base.clone();
        for field in ["color_space", "color_transfer", "color_primaries"] {
            set(&mut tagged, field, Value::Str("bt709".into()));
        }
        set(&mut tagged, "color_range", Value::Str("tv".into()));
        assert_eq!(qualify_movie(&tagged, Some(&audio), false).unwrap(), false);
        for unspecified in ["unknown", "unspecified"] {
            let mut v = tagged.clone();
            set(&mut v, "color_transfer", Value::Str(unspecified.into()));
            assert_eq!(qualify_movie(&v, Some(&audio), false).unwrap(), true);
        }
        let mut v = tagged;
        set(&mut v, "side_data_list", Value::Arr(vec![]));
        set(
            &mut v,
            "tags",
            Value::Obj(vec![("rotate".into(), Value::Str("0".into()))]),
        );
        assert_eq!(qualify_movie(&v, Some(&audio), false).unwrap(), false);
        // The actual command requests every field used by the admission/rotation checks.
        for key in [
            "profile",
            "level",
            "pix_fmt",
            "color_range",
            "color_space",
            "color_transfer",
            "color_primaries",
            "channel_layout",
            "stream_side_data=side_data_type",
            "stream_tags=rotate",
        ] {
            assert!(ORIGINAL_PROBE_ENTRIES.contains(key));
        }
    }
    #[test]
    fn initial_movie_profile_refuses_unqualified_picture_audio_hdr_and_rotation() {
        let base = picture_profile();
        let audio = audio_profile();
        for (field, value) in [
            ("codec_name", Value::Str("hevc".into())),
            ("codec_tag_string", Value::Str("avc3".into())),
            ("profile", Value::Str("High 10".into())),
            ("profile", Value::Str("Main".into())),
            ("profile", Value::Null),
            ("level", Value::Int(41)),
            ("level", Value::Int(-99)),
            ("level", Value::Null),
            ("pix_fmt", Value::Str("yuv420p10le".into())),
            ("pix_fmt", Value::Str("yuv422p".into())),
            ("pix_fmt", Value::Null),
            ("width", Value::Int(3840)),
            ("height", Value::Int(1088)),
            ("avg_frame_rate", Value::Str("24000/1001".into())),
            ("r_frame_rate", Value::Str("25/1".into())),
            ("field_order", Value::Str("tt".into())),
            ("sample_aspect_ratio", Value::Str("4:3".into())),
            ("color_space", Value::Str("bt2020nc".into())),
            ("color_primaries", Value::Str("bt2020".into())),
            ("color_transfer", Value::Str("smpte2084".into())),
            ("color_transfer", Value::Str("arib-std-b67".into())),
            ("color_range", Value::Str("pc".into())),
            ("color_transfer", Value::Null),
            (
                "side_data_list",
                graph::json::parse(r#"[{"side_data_type":"Mastering display metadata"}]"#).unwrap(),
            ),
            (
                "side_data_list",
                graph::json::parse(r#"[{"side_data_type":"Display Matrix"}]"#).unwrap(),
            ),
            ("tags", graph::json::parse(r#"{"rotate":"90"}"#).unwrap()),
            (
                "tags",
                graph::json::parse(r#"{"rotate":"unresolved"}"#).unwrap(),
            ),
            ("tags", graph::json::parse(r#"{"rotate":null}"#).unwrap()),
        ] {
            let mut v = base.clone();
            set(&mut v, field, value);
            assert!(
                qualify_movie(&v, Some(&audio), false).is_err(),
                "accepted {field}: {v:?}"
            );
        }
        for (field, value) in [
            ("codec_name", Value::Str("mp3".into())),
            ("codec_tag_string", Value::Str("unknown".into())),
            ("codec_tag_string", Value::Null),
            ("profile", Value::Str("HE-AAC".into())),
            ("profile", Value::Str("HE-AACv2".into())),
            ("profile", Value::Null),
            ("sample_rate", Value::Str("44100".into())),
            ("sample_rate", Value::Str("96000".into())),
            ("channels", Value::Int(1)),
            ("channels", Value::Int(6)),
            ("channel_layout", Value::Str("unknown".into())),
        ] {
            let mut v = audio.clone();
            set(&mut v, field, value);
            assert!(
                qualify_movie(&base, Some(&v), false).is_err(),
                "accepted audio {field}: {v:?}"
            );
        }
    }
    #[test]
    fn linked_prores_hq_refuses_conflicting_timecode_and_additional_streams() {
        let mov = Path::new("/Synthetic/source.mov");
        let probe = graph::json::parse(r#"{"format":{"tags":{"major_brand":"qt  "}}}"#).unwrap();
        let streams = graph::json::parse(r#"[
            {"index":0,"codec_type":"video","codec_name":"prores","codec_tag_string":"apch","profile":"HQ",
             "pix_fmt":"yuv422p10le","width":1920,"height":1080,"avg_frame_rate":"24/1","r_frame_rate":"24/1",
             "field_order":"progressive","sample_aspect_ratio":"1:1","time_base":"1/12288","duration_ts":122880,
             "tags":{"timecode":"00:00:00:00"}},
            {"index":1,"codec_type":"data","codec_tag_string":"tmcd","nb_frames":"1","avg_frame_rate":"24/1",
             "time_base":"1/12288","duration_ts":122880,"start_pts":0,"tags":{"timecode":"00:00:00:00"}}
        ]"#).unwrap();
        let Value::Arr(streams) = streams else { panic!() };
        assert_eq!(qualify_prores_hq(mov, &probe, &streams).unwrap(), true);
        assert!(qualify_prores_hq(Path::new("/Synthetic/source.mp4"), &probe, &streams).is_err());
        assert!(qualify_prores_hq(mov, &probe, &streams[..1]).is_err());
        for (index, key, value) in [
            (0, "profile", Value::Str("LT".into())),
            (0, "avg_frame_rate", Value::Str("24000/1001".into())),
            (0, "color_transfer", Value::Str("smpte2084".into())),
            (1, "codec_type", Value::Str("audio".into())),
            (1, "start_pts", Value::Int(1)),
            (1, "duration_ts", Value::Int(122879)),
            (1, "tags", graph::json::parse(r#"{"timecode":"00:00:01:00"}"#).unwrap()),
        ] {
            let mut changed = streams.clone();
            set(&mut changed[index], key, value);
            assert!(qualify_prores_hq(mov, &probe, &changed).is_err(), "{key}");
        }
        let mut with_audio = streams.clone();
        with_audio.push(audio_profile());
        assert!(qualify_prores_hq(mov, &probe, &with_audio).is_err());
    }
    #[test]
    fn linked_prores_hq_checks_every_picture_packet() {
        use std::fmt::Write;
        let count = 50_000;
        let ticks_per_frame = 10_000;
        let late_frame = 49_990;
        let mut rows = String::new();
        let mut late = 0;
        let mut last = 0;
        for frame in 0..count {
            let pts = i128::from(frame) * ticks_per_frame;
            if frame == late_frame { late = rows.len(); }
            if frame == count - 1 { last = rows.len(); }
            writeln!(rows, "{pts},{pts},{ticks_per_frame}").unwrap();
        }
        assert!(rows.len() > 1 << 20);
        let verify = |bytes: &[u8]| verify_prores_packets(bytes, count, ticks_per_frame, || {});
        verify(rows.as_bytes()).unwrap();
        assert!(verify(&rows.as_bytes()[..last]).unwrap_err().contains("packet count"));
        assert!(verify(&rows.as_bytes()[..rows.len() - 1]).is_ok());
        let mut extra = rows.clone();
        extra.push_str("500000000,500000000,10000\n");
        assert!(verify(extra.as_bytes()).unwrap_err().contains("packet 50000"));
        // Two late packets keep the exact overall duration and average rate.
        let mut uneven = rows;
        let later = uneven[late..].splitn(3, '\n').take(2).map(|r| r.len() + 1).sum::<usize>();
        uneven.replace_range(late..late + later, "499900000,499900000,10001\n499910001,499910001,9999\n");
        assert!(verify(uneven.as_bytes()).unwrap_err().contains("packet 49990"));
        let oversized = [b'1'; 129];
        assert!(verify(&oversized).unwrap_err().contains("row exceeds 128 bytes"));
    }
    #[cfg(unix)]
    #[test]
    fn probe_child_timeout_and_stdout_budget_are_bounded_and_reaped() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf partial; exec sleep 5"]);
        let error = bounded_probe(command, Duration::from_millis(40)).unwrap_err();
        assert!(error.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec yes synthetic"]);
        let start = Instant::now();
        let error = bounded_probe(command, Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("exceeds 1 MiB"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '{\"streams\":[]}'"]);
        assert_eq!(
            bounded_probe(command, Duration::from_secs(1)).unwrap(),
            br#"{"streams":[]}"#
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf refused; exit 3"]);
        assert!(bounded_probe(command, Duration::from_secs(1))
            .unwrap_err()
            .contains("refused"));
    }
    #[cfg(unix)]
    #[test]
    fn packet_probe_tracks_verified_progress_and_cancels_stalls() {
        use std::time::Instant;
        let read = |out, progress: &AtomicU64| {
            verify_prores_packets(BufReader::new(out), 5, 1, || {
                progress.fetch_add(1, Ordering::Relaxed);
            })
        };
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "for i in 0 1 2 3 4; do printf '%s,%s,1\n' \"$i\" \"$i\"; sleep 0.25; done"]);
        let start = Instant::now();
        run_probe(command, ProbeDeadline::Idle(Duration::from_millis(800)),
            &mut || Ok(()), read).unwrap();
        assert!(start.elapsed() > Duration::from_millis(800));

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '0,0,1\n'; exec sleep 5"]);
        let error = run_probe(command, ProbeDeadline::Idle(Duration::from_millis(80)),
            &mut || Ok(()), read).unwrap_err();
        assert!(error.contains("without a verified picture packet"));

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '0,0,1\n'; exec sleep 5"]);
        let start = Instant::now();
        let error = run_probe(command, ProbeDeadline::Idle(Duration::from_secs(30)), &mut || {
            if start.elapsed() >= Duration::from_millis(80) { Err("cancelled".into()) } else { Ok(()) }
        }, read).unwrap_err();
        assert_eq!(error, "cancelled");
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn foreign_saved_paths_require_explicit_component_mappings() {
        let root = std::env::temp_dir().join(format!("anynle-foreign-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let file = root.join("nested/control.wav");
        std::fs::write(&file, b"synthetic").unwrap();
        for (saved, prefix) in [
            (r"Q:\Recordings\nested\control.wav", r"Q:\Recordings"),
            ("/Volumes/Recorded/nested/control.wav", "/Volumes/Recorded"),
            (
                r"\\synthetic-host\Recordings\nested\control.wav",
                r"\\synthetic-host\Recordings",
            ),
        ] {
            let mappings = [RootMapping {
                saved_root: prefix.into(),
                current_root: root.clone(),
            }];
            assert_eq!(
                resolve(&[saved.into()], &mappings).unwrap(),
                std::fs::canonicalize(&file).unwrap()
            );
            let bad = saved.replace("nested", "../nested");
            assert!(resolve(&[bad], &mappings).is_err());
        }
        assert_ne!(
            saved_parts("/Recorded").unwrap().1,
            saved_parts("/RecordedOther").unwrap().1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exact_locator_mapping_refuses_basename_ambiguity_and_parent_escape() {
        let root =
            std::env::temp_dir().join(format!("anynle-original-path-{}", std::process::id()));
        let a = root.join("saved");
        let b = root.join("mapped");
        std::fs::create_dir_all(a.join("nested")).unwrap();
        std::fs::create_dir_all(b.join("nested")).unwrap();
        let mapped = b.join("nested/source.mp4");
        std::fs::write(&mapped, b"synthetic").unwrap();
        let saved = a.join("nested/source.mp4");
        let mappings = vec![RootMapping {
            saved_root: a.clone(),
            current_root: b.clone(),
        }];
        assert!(resolve(&[saved.to_string_lossy().into_owned()], &[]).is_err());
        assert_eq!(
            resolve(&[saved.to_string_lossy().into_owned()], &mappings).unwrap(),
            std::fs::canonicalize(&mapped).unwrap()
        );
        std::fs::write(&saved, b"other synthetic").unwrap();
        let error = resolve(&[saved.to_string_lossy().into_owned()], &mappings).unwrap_err();
        assert!(error.contains("2 exact candidates"));
        for path in [&mapped, &saved] {
            assert!(error.contains(&format!("{:?}", path.canonicalize().unwrap())));
        }
        assert!(resolve(
            &[a.join("../mapped/nested/source.mp4")
                .to_string_lossy()
                .into_owned()],
            &mappings
        )
        .is_err());
        assert!(resolve(&["source.mp4".into()], &mappings).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
