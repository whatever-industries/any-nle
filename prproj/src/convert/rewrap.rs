//! Explicit physical-media binding after OP1a preparation. Model/1 stays unchanged.
//! OP1a mono views select a flattened source channel, not an importer stream.
//! Preserve the complete physical layout alongside the explicit channel selector.
use super::writer::{Result, WriteError};
use crate::model::Project;
use crate::xml::mutations::insert_leaf_before;
use std::collections::{HashMap, HashSet};

/// FFmpeg 8.1 MXF content-package rates; source receipt in the implementation handoff.
pub const RATES: &[(i64, i64)] = &[
    (24, 1),
    (24000, 1001),
    (25, 1),
    (30, 1),
    (30000, 1001),
    (48, 1),
    (48000, 1001),
    (50, 1),
    (60, 1),
    (60000, 1001),
    (72, 1),
    (72000, 1001),
    (75, 1),
    (90, 1),
    (90000, 1001),
    (96, 1),
    (96000, 1001),
    (100, 1),
    (120, 1),
    (120000, 1001),
];
pub fn supported_rate(rate: super::model::Fraction) -> bool {
    RATES.contains(&(rate.num, rate.den))
}
/// Adobe display enums, not native OP1a acceptance. Extended rates use Frames (109).
pub fn display_format(rate: super::model::Fraction, drop: bool) -> Option<u32> {
    if !supported_rate(rate)
        || (drop
            && !matches!(
                (rate.num, rate.den),
                (30000, 1001) | (60000, 1001) | (90000, 1001) | (120000, 1001)
            ))
    {
        return None;
    }
    Some(match (rate.num, rate.den) {
        (24, 1) => 100,
        (24000, 1001) => 110,
        (25, 1) => 101,
        (30, 1) => 104,
        (30000, 1001) => {
            if drop {
                102
            } else {
                103
            }
        }
        (48, 1) => 113,
        (50, 1) => 105,
        (60, 1) => 108,
        (60000, 1001) => {
            if drop {
                106
            } else {
                107
            }
        }
        _ => 109,
    })
}
pub fn video_format(clip: &super::model::Clip, rate: super::model::Fraction) -> Result<String> {
    let code = display_format(rate, matches!(clip.time_display, Some(102 | 106)))
        .ok_or_else(|| invalid("unsupported rewrap video rate/timecode".into()))?;
    Ok(code.to_string())
}
/// Explicit opt-in format names; ordinary writer presets/refusals remain unchanged.
pub fn sequence_preset(rate: super::model::Fraction, width: i64, height: i64) -> Option<String> {
    let code = display_format(rate, false)?;
    if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
        return None;
    }
    Some(format!(
        "anynle-op1a/{}/{}/{}/{}",
        super::model::ticks_per_unit(rate),
        width,
        height,
        code
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaRelink {
    pub source_path: String,
    pub target_path: String,
    /// Zero-based output audio ordinal, not the absolute container stream index.
    /// For the bounded uniform-depth mono OP1a route this is the flattened source channel.
    pub audio_stream: Option<u32>,
    /// Each audio ordinal is a distinct mono stream in the output file.
    pub separate_mono: bool,
    /// Verified effective pixel aspect of a rewrapped MOV picture.
    pub pixel_aspect: Option<(u32, u32)>,
}

impl MediaRelink {
    pub(super) fn validate_pixel_aspect(&self) -> Result<()> {
        if let Some((n, d)) = self.pixel_aspect {
            if n == 0 || d == 0 || n == d || self.audio_stream.is_some()
                || !std::path::Path::new(&self.target_path).extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mov"))
            {
                return Err(invalid("rewrap pixel aspect requires a non-square MOV picture binding".into()));
            }
        }
        Ok(())
    }

    pub(super) fn is_separate_mono_wav(&self) -> bool {
        self.audio_stream == Some(0) && self.separate_mono
            && std::path::Path::new(&self.target_path).extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
    }
}
fn invalid(message: String) -> WriteError {
    WriteError::Other(crate::Error::Value(message))
}

/// Premiere's tested OP1a importer omits tracks whose PCM depth differs from the
/// first track. Refuse such an assembly instead of losing channels or converting PCM.
pub fn validate_audio_formats(model: &super::model::Model, mappings: &[MediaRelink]) -> Result<()> {
    let bound: HashMap<_, _> = mappings
        .iter()
        .filter(|m| m.audio_stream.is_some())
        .map(|m| (m.source_path.as_str(), m.target_path.as_str()))
        .collect();
    let mut formats = HashMap::new();
    for audio in model.clips.values().flat_map(|c| &c.audio) {
        if let Some(target) = bound.get(audio.path.as_str()) {
            if audio.sample_rate != 48000 || !matches!(audio.bits, 16 | 24) {
                return Err(invalid("rewrap audio requires mono 48 kHz PCM16/24".into()));
            }
            let format = (audio.sample_rate, audio.bits);
            if formats
                .insert(*target, format)
                .is_some_and(|old| old != format)
            {
                return Err(invalid(
                    "mixed PCM bit depths in one OP1a file are not supported; no sample conversion"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

pub fn apply(project: &mut Project, mappings: &[MediaRelink]) -> Result<()> {
    apply_views(project, mappings, true, &HashSet::new())
}

/// Native masters use full-source templates and channel vectors. Narrowing the
/// Media would make Premiere collapse a stereo bin template to mono on read-back.
pub fn apply_native(project: &mut Project, mappings: &[MediaRelink]) -> Result<()> {
    apply_views(project, mappings, false, &HashSet::new())
}

const DNXHR_444_RGB: &str = r#"{"baseColorProfile":{"colorProfileName":"1,1,1,Full,RGB,32f,Display-Referred"},"baseProfileType":1,"colorSpaceMetadata":{"dataRangeInfoValid":1}}"#;

pub(super) fn apply_views(
    project: &mut Project,
    mappings: &[MediaRelink],
    narrow: bool,
    dnxhr_444_rgb_sources: &HashSet<String>,
) -> Result<()> {
    let mut by_path = HashMap::new();
    let mut channels: HashMap<&str, HashSet<u32>> = HashMap::new();
    let mut layouts = HashMap::new();
    for m in mappings {
        m.validate_pixel_aspect()?;
        if layouts.insert(m.target_path.as_str(), m.separate_mono).is_some_and(|old| old != m.separate_mono) {
            return Err(invalid("rewrap bindings disagree about the target audio layout".into()));
        }
        if let Some(index) = m.audio_stream {
            if index >= 64 || (index >= 32 && !m.separate_mono) {
                return Err(invalid("rewrap audio ordinal exceeds its binding layout".into()));
            }
            channels.entry(&m.target_path).or_default().insert(index);
        }
        if m.target_path
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}'))
        {
            return Err(invalid("rewrap target path cannot be saved in XML".into()));
        }
        if !super::model::is_absolute_path(&m.target_path) || m.source_path == m.target_path {
            return Err(invalid(
                "rewrap needs a distinct absolute target path".into(),
            ));
        }
        if let Some(old) = by_path.insert(m.source_path.as_str(), m) {
            if old != m {
                return Err(invalid("conflicting rewrap mappings".into()));
            }
        }
    }
    for indices in channels.values() {
        if !(0..indices.len() as u32).all(|i| indices.contains(&i)) {
            return Err(invalid(
                "rewrap audio bindings must describe every output channel".into(),
            ));
        }
    }
    if dnxhr_444_rgb_sources.iter().any(|path| {
        !by_path.get(path.as_str()).is_some_and(|mapping| {
            mapping.audio_stream.is_none()
                && std::path::Path::new(&mapping.target_path).extension()
                    .and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("mxf"))
        })
    }) {
        return Err(invalid("DNxHR 444 source color requires a bound MXF picture".into()));
    }
    let dir = project
        .path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_string_lossy()
        .to_string();
    let nodes: Vec<_> = project.doc.tree.children(project.doc.root).to_vec();
    let mut found = HashSet::new();
    let mut states = HashMap::new();
    let mut state_remap = HashMap::new();
    for node in nodes {
        if !dnxhr_444_rgb_sources.is_empty() && project.doc.tree.tag(node) == "Media"
            && project.doc.tree.child_by_tag(node, "FilePath")
                .and_then(|path| project.doc.tree.text(path))
                .is_some_and(|path| dnxhr_444_rgb_sources.contains(path))
        {
            let e = project.doc.tree.child_by_tag(node, "VideoStream")
                .ok_or_else(|| invalid("DNxHR 444 binding lacks a video stream".into()))?;
            let stream = project.doc.resolve(e)?;
            let tree = &mut project.doc.tree;
            if tree.findtext(stream, "CodecType")?.and_then(|value| value.parse().ok())
                != Some(u32::from_be_bytes(*b"AVdn"))
            {
                return Err(invalid("DNxHR 444 source color requires a DNx picture stream".into()));
            }
            let color = tree.child_by_tag(stream, "OriginalColorSpace")
                .ok_or_else(|| invalid("DNxHR 444 binding lacks a source color field".into()))?;
            tree.set_text(color, Some(DNXHR_444_RGB));
        }
        let audio_stream = project
            .doc
            .tree
            .child_by_tag(node, "AudioStream")
            .map(|reference| project.doc.resolve(reference))
            .transpose()?;
        if let Some(&(n, d)) = project.doc.tree.child_by_tag(node, "FilePath")
            .filter(|_| project.doc.tree.tag(node) == "Media")
            .and_then(|path| project.doc.tree.text(path))
            .and_then(|path| by_path.get(path))
            .and_then(|mapping| mapping.pixel_aspect.as_ref())
        {
            let reference = project.doc.tree.child_by_tag(node, "VideoStream")
                .ok_or_else(|| invalid("rewrap pixel aspect binding lacks a video stream".into()))?;
            let stream = project.doc.resolve(reference)?;
            let interpretation = crate::model::footage::FootageInterpretation { stream, source: None };
            if interpretation.stored_pixel_aspect_ratio(&project.doc.tree).as_deref()
                != Some(&format!("{n},{d}"))
            {
                return Err(invalid("rewrap picture interpretation disagrees with the binding".into()));
            }
        }
        let tree = &mut project.doc.tree;
        if tree.tag(node) != "Media" {
            continue;
        }
        let Some(path_node) = tree.child_by_tag(node, "FilePath") else {
            continue;
        };
        let path = tree.text(path_node).unwrap_or("").to_string();
        let Some(m) = by_path.get(path.as_str()) else {
            continue;
        };
        let is_audio = tree.child_by_tag(node, "AudioStream").is_some();
        if is_audio != m.audio_stream.is_some() {
            return Err(invalid(
                "rewrap media kind disagrees with project source".into(),
            ));
        }
        for tag in ["FilePath", "ActualMediaFilePath", "RelativePath"] {
            let child = tree
                .child_by_tag(node, tag)
                .ok_or_else(|| invalid(format!("rewrap Media lacks {tag}")))?;
            let value = if tag == "RelativePath" {
                crate::build::media::media_relpath(&m.target_path, &dir)
            } else {
                m.target_path.clone()
            };
            tree.set_text(child, Some(value));
        }
        let key = tree
            .child_by_tag(node, "FileKey")
            .ok_or_else(|| invalid("rewrap Media lacks FileKey".into()))?;
        let state = tree
            .child_by_tag(node, "ContentAndMetadataState")
            .ok_or_else(|| invalid("rewrap Media lacks content state".into()))?;
        let modification = tree
            .child_by_tag(node, "ModificationState")
            .ok_or_else(|| invalid("rewrap Media lacks modification state".into()))?;
        let current_state = tree.text(state).unwrap_or("").to_string();
        let shared = states.entry(m.target_path.clone()).or_insert_with(|| {
            (
                tree.text(key).unwrap_or("").to_string(),
                current_state.clone(),
                tree[modification].attrs.clone(),
                tree[modification].text.clone(),
            )
        });
        tree.set_text(key, Some(shared.0.clone()));
        tree.set_text(state, Some(shared.1.clone()));
        tree[modification].attrs = shared.2.clone();
        tree[modification].text = if m.audio_stream.is_some_and(|i| i > 0) {
            None
        } else {
            shared.3.clone()
        };
        state_remap.insert(current_state, shared.1.clone());
        if let Some(index) = m.audio_stream {
            if let Some(child) = tree.child_by_tag(node, "StreamNumber") {
                tree.remove(node, child);
            }
            let mono_mov = !narrow && m.target_path.to_ascii_lowercase().ends_with(".mov") && channels[m.target_path.as_str()].len()>2;
            let mono_stream = m.separate_mono || mono_mov;
            if mono_stream && index>0 {insert_leaf_before(tree,node,"ContentAndMetadataState","StreamNumber",&index.to_string())?;}
            if narrow && !m.separate_mono {
                insert_leaf_before(
                    tree,
                    node,
                    "ContentAndMetadataState",
                    "SingleAudioChannel",
                    &index.to_string(),
                )?;
            }
            let stream =
                audio_stream.ok_or_else(|| invalid("rewrap audio view lacks stream".into()))?;
            let layout = tree
                .child_by_tag(stream, "AudioChannelLayout")
                .ok_or_else(|| invalid("rewrap AudioStream lacks layout".into()))?;
            tree.set_text(
                layout,
                Some(format!(
                    "[{}]",
                    vec!["{\"channellabel\":0}"; if mono_stream {1}else{channels[m.target_path.as_str()].len()}].join(",")
                )),
            );
            if narrow && !m.separate_mono {
                insert_leaf_before(
                    tree,
                    stream,
                    "FrameRate",
                    "SingleChannel",
                    &index.to_string(),
                )?;
            }
        }
        found.insert(path);
    }
    if found.len() != by_path.len() {
        return Err(invalid(
            "rewrap mapping names a source absent from the written project".into(),
        ));
    }
    let markers: Vec<_> = project
        .doc
        .tree
        .iter_tag(project.doc.root, "LastContentState")
        .collect();
    for node in markers {
        if let Some(new) = state_remap.get(project.doc.tree.text(node).unwrap_or("")) {
            project.doc.tree.set_text(node, Some(new.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_mono_wav_bindings_keep_distinct_media_states() {
        use super::super::opatom::{ensure_audio_media, ensure_video_media, MediaTable};
        let mut model = super::super::model::parse_model_str(include_str!(
            "../../../examples/opatom_clip.model.json"
        )).unwrap();
        let clip = model.clips.get_mut("clip-1").unwrap();
        let mono = clip.audio[0].clone();
        clip.audio = (1..=33).map(|lane| {
            let mut audio = mono.clone();
            audio.avid_track = Some(lane);
            audio.path = format!("/Volumes/Media/A{lane}.mxf");
            audio
        }).collect();
        let clip = &model.clips["clip-1"];
        let picture = clip.video.as_ref().unwrap();
        let mut bindings = vec![MediaRelink {
            source_path: picture.path.clone(), target_path: "/Volumes/Output/Picture.mxf".into(),
            audio_stream: None, separate_mono: true,
            pixel_aspect: None,
        }];
        bindings.extend(clip.audio.iter().enumerate().map(|(lane, audio)| MediaRelink {
            source_path: audio.path.clone(), target_path: format!("/Volumes/Output/A{}.wav", lane + 1),
            audio_stream: Some(0), separate_mono: true,
            pixel_aspect: None,
        }));
        super::super::native_av::validate_bindings(&model, &bindings).unwrap();
        let mut duplicate = bindings.clone();
        duplicate[33].target_path = duplicate[32].target_path.clone();
        assert!(super::super::native_av::validate_bindings(&model, &duplicate).is_err());

        let mut ids = crate::SeededUuids::new("wide-wav-binding");
        let mut project = Project::new(&mut ids).unwrap();
        let mut table = MediaTable::new();
        ensure_video_media(&mut project, &mut table, picture, clip, &mut ids).unwrap();
        for audio in &clip.audio {
            ensure_audio_media(&mut project, &mut table, audio, clip, &mut ids).unwrap();
        }
        apply_native(&mut project, &bindings).unwrap();
        let tree = &project.doc.tree;
        let mut hashes = HashSet::new();
        let mut paths = HashSet::new();
        for media in tree.iter_tag(project.doc.root, "Media") {
            if tree.child_by_tag(media, "AudioStream").is_none() { continue; }
            let state = tree.child_by_tag(media, "ModificationState").unwrap();
            assert!(tree.text(state).is_some_and(|body| !body.is_empty()));
            assert!(hashes.insert(tree.get(state, "BinaryHash").unwrap()));
            assert!(paths.insert(tree.findtext(media, "FilePath").unwrap().unwrap()));
            assert!(tree.child_by_tag(media, "StreamNumber").is_none());
        }
        assert_eq!(paths.len(), 33);

        let clip = model.clips.get_mut("clip-1").unwrap();
        for audio in &mut clip.audio {
            audio.bits = 24;
        }
        super::super::native_av::validate_bindings(&model, &bindings).unwrap();
    }

    #[test]
    fn dnxhr_444_color_is_scoped_to_the_planned_picture() {
        use super::super::opatom::{ensure_video_media, MediaTable};
        let model = super::super::model::parse_model_str(include_str!(
            "../../../examples/opatom_clip.model.json"
        )).unwrap();
        let clip = &model.clips["clip-1"];
        let picture = clip.video.as_ref().unwrap();
        let mut control = picture.clone();
        control.path = "/Volumes/Media/Control.mxf".into();
        let mut ids = crate::SeededUuids::new("dnxhr-444-color");
        let mut project = Project::new(&mut ids).unwrap();
        let mut table = MediaTable::new();
        ensure_video_media(&mut project, &mut table, picture, clip, &mut ids).unwrap();
        ensure_video_media(&mut project, &mut table, &control, clip, &mut ids).unwrap();
        let target = "/Volumes/Media/Rewrapped.mxf";
        let bindings = [MediaRelink {
            source_path: picture.path.clone(), target_path: target.into(),
            audio_stream: None, separate_mono: false,
            pixel_aspect: None,
        }];
        let unrelated = HashSet::from([control.path.clone()]);
        assert!(apply_views(&mut project, &bindings, false, &unrelated).is_err());
        apply_views(&mut project, &bindings, false, &HashSet::from([picture.path.clone()])).unwrap();
        let doc = &project.doc;
        let tree = &doc.tree;
        let color = |path: &str| {
            let media = tree.iter_tag(doc.root, "Media")
                .find(|&node| tree.findtext(node, "FilePath").unwrap().as_deref() == Some(path))
                .unwrap();
            let stream = doc.resolve(tree.child_by_tag(media, "VideoStream").unwrap()).unwrap();
            tree.findtext(stream, "OriginalColorSpace").unwrap().unwrap()
        };
        assert_eq!(color(target), DNXHR_444_RGB);
        assert_eq!(color(&control.path), crate::build::media::opatom::DNX_COLOR_SPACE);
    }

    #[test]
    fn incompatible_formats_and_invalid_channel_inventories_do_not_write() {
        let mut model = super::super::model::parse_model_str(include_str!(
            "../../../examples/opatom_clip.model.json"
        ))
        .unwrap();
        let tmp = std::env::temp_dir().join(format!("rewrap-refusal-{}", std::process::id()));
        std::fs::create_dir(&tmp).unwrap();
        let output = tmp.join("never.prproj");
        let target = tmp.join("never.mxf").display().to_string();
        let clip = &model.clips["clip-1"];
        let mut maps = vec![MediaRelink {
            source_path: clip.video.as_ref().unwrap().path.clone(),
            target_path: target.clone(),
            audio_stream: None,
            separate_mono: false,
            pixel_aspect: None,
        }];
        for (i, a) in clip.audio.iter().enumerate() {
            maps.push(MediaRelink {
                source_path: a.path.clone(),
                target_path: target.clone(),
                audio_stream: Some(i as u32),
                separate_mono: false,
                pixel_aspect: None,
            });
        }
        model.clips.get_mut("clip-1").unwrap().audio[1].bits = 24;
        assert!(super::super::writer::write_project_with_media(
            &model,
            &output,
            &mut crate::SeededUuids::new("refuse"),
            &maps
        )
        .unwrap_err()
        .to_string()
        .contains("mixed PCM"));
        model.clips.get_mut("clip-1").unwrap().audio[1].bits = 16;
        for index in [32, u32::MAX] {
            let mut bad = maps.clone();
            bad[2].audio_stream = Some(index);
            assert!(super::super::writer::write_project_with_media(
                &model,
                &output,
                &mut crate::SeededUuids::new("refuse"),
                &bad
            )
            .is_err());
        }
        maps[1].audio_stream = Some(1);
        assert!(super::super::writer::write_project_with_media(
            &model,
            &output,
            &mut crate::SeededUuids::new("refuse"),
            &maps
        )
        .unwrap_err()
        .to_string()
        .contains("every output channel"));
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        std::fs::remove_dir(tmp).unwrap();
    }
    #[test]
    fn bindings_match_python_oracle_and_preserve_default_output() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let tmp = std::env::temp_dir().join(format!(
            "op1a-writer-parity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&tmp).unwrap();
        let fixture = root.join("examples/opatom_clip.model.json");
        let model = super::super::model::load_model(&fixture).unwrap();
        let out = tmp.join("out.prproj");
        let target = tmp.join("media.mxf").to_string_lossy().to_string();
        let c = &model.clips["clip-1"];
        let mut maps = vec![MediaRelink {
            source_path: c.video.as_ref().unwrap().path.clone(),
            target_path: target.clone(),
            audio_stream: None,
            separate_mono: false,
            pixel_aspect: None,
        }];
        for (i, a) in c.audio.iter().enumerate() {
            maps.push(MediaRelink {
                source_path: a.path.clone(),
                target_path: target.clone(),
                audio_stream: Some(i as u32),
                separate_mono: false,
                pixel_aspect: None,
            });
        }
        let mut ids = crate::SeededUuids::new("op1a-parity");
        super::super::writer::write_project_with_media(&model, &out, &mut ids, &maps).unwrap();
        let rust = std::fs::read(&out).unwrap();
        std::fs::remove_file(&out).unwrap();
        let code = r#"import json,sys
from pathlib import Path
from avid2prproj.model import parse_model
from avid2prproj.writer import write_project
from avid2prproj.determinism import seeded_ids
v=json.loads(Path(sys.argv[1]).read_text()); c=v['clips'][0]; target=sys.argv[3]
m=[dict(source_path=c['video']['path'],target_path=target,audio_stream=None)]
m += [dict(source_path=a['path'],target_path=target,audio_stream=i) for i,a in enumerate(c['audio'])]
with seeded_ids('op1a-parity'): write_project(parse_model(v),Path(sys.argv[2]),media_relinks=m)
"#;
        let p = std::process::Command::new("python3")
            .arg("-c")
            .arg(code)
            .arg(&fixture)
            .arg(&out)
            .arg(&target)
            .env(
                "PYTHONPATH",
                format!(
                    "{}:{}",
                    root.join("src").display(),
                    root.join("vendor").display()
                ),
            )
            .env("PYTHONHASHSEED", "0")
            .output()
            .unwrap();
        assert!(p.status.success(), "{}", String::from_utf8_lossy(&p.stderr));
        assert_eq!(rust, std::fs::read(&out).unwrap());
        let (xml, _) = crate::xml::decompress_prproj(&rust).unwrap();
        let xml = String::from_utf8(xml).unwrap();
        assert!(xml.contains("<SingleAudioChannel>1</SingleAudioChannel>"));
        assert!(xml.contains("<SingleChannel>1</SingleChannel>"));
        assert!(!xml.contains("<StreamNumber>"));
        assert!(!xml.contains("A01.EXAMPLE.mxf"));
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
