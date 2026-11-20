//! Classify the observed drive-like locator spelling on a saved MXF sibling bundle.
use avid_reader::{
    extract::{Essence, MediaTrack, KIND_PICTURE, KIND_SOUND},
    source::{Observed, Rate, SourceMaster},
};
use graph::Value;
use std::{collections::HashSet, path::Path};

use super::{number, text};
use crate::index::MediaIndex;

fn split_locator(path: &str) -> Option<(&str, &str)> {
    let [drive, b'/', b'/', ..] = path.as_bytes() else {
        return None;
    };
    if !drive.is_ascii_alphabetic()
        || path.len() > 32768
        || path
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':'))
    {
        return None;
    }
    let body = &path[3..];
    if body.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return None;
    }
    let (parent, name) = path.rsplit_once('/')?;
    (body.contains('/')
        && name
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("mxf")))
    .then_some((parent, name))
}

/// This spelling establishes package membership and a saved directory. Physical
/// paths must come from independently verified file packages in selected roots.
pub(crate) fn saved_sibling_directory(source: &SourceMaster) -> Option<&str> {
    let tracks = &source.master.tracks;
    let rate = Rate::new(24_000, 1001)?;
    let frames = source.master.length;
    let samples = frames.checked_mul(2002)?;
    if tracks.len() != 9
        || source.originals.len() != tracks.len()
        || source.offsets.len() != tracks.len()
        || !source
            .offsets
            .iter()
            .all(|offset| *offset == Observed::Known(0))
        || source.rate != Some(rate)
        || frames <= 0
        || source.managed_multistream.is_some()
    {
        return None;
    }

    let mut directory = None;
    let mut names = HashSet::with_capacity(tracks.len());
    let mut packages = HashSet::with_capacity(tracks.len());
    for (ordinal, (track, original)) in tracks.iter().zip(&source.originals).enumerate() {
        let raw = original.as_ref()?;
        let [path] = raw.locators.as_slice() else {
            return None;
        };
        let (parent, name) = split_locator(path)?;
        if directory.is_some_and(|saved| saved != parent)
            || !names.insert(name.to_ascii_lowercase())
            || !packages.insert(track.file_mob)
            || raw.file_track != Some(track.index)
            || raw.descriptor_length != track.length
        {
            return None;
        }
        let matching = if ordinal == 0 {
            track.kind == KIND_PICTURE
                && track.index == 1
                && track.length == frames
                && Rate::from_avid(track.edit_rate) == Some(rate)
                && Rate::from_avid(raw.descriptor_rate) == Some(rate)
                && raw.descriptor_class == "CDCI"
                && raw.display_raster == Some((1920, 1080))
                && matches!(
                    track.essence,
                    Some(Essence::Picture {
                        width: 1920,
                        height: 1080,
                        ..
                    })
                )
        } else {
            track.kind == KIND_SOUND
                && track.index == ordinal as i64
                && track.length == samples
                && track.edit_rate == 48_000.0
                && raw.descriptor_rate == 48_000.0
                && raw.descriptor_class == "PCMA"
                && raw.sample_rate == Some(48_000.0)
                && raw.channels == Some(1)
                && raw.stored_quantization_bits == Some(24)
                && matches!(
                    track.essence,
                    Some(Essence::Sound {
                        sample_rate: 48_000.0,
                        bits: 24,
                        channels: 1,
                    })
                )
        };
        if !matching {
            return None;
        }
        directory = Some(parent);
    }
    directory
}

fn package_label(track: &MediaTrack, package: &str) -> String {
    let lane = if track.kind == KIND_PICTURE { 'V' } else { 'A' };
    format!("{lane}{} ({package})", track.index)
}

fn indexed_package<'a>(
    index: &'a MediaIndex,
    package: &str,
    label: &str,
) -> Result<Option<&'a Path>, String> {
    if index.ambiguous.binary_search_by(|uid| uid.as_str().cmp(package)).is_ok() {
        return Err(format!("Ambiguous legacy MXF file package for {label}"));
    }
    Ok(index.files.get(package).map(|path| path.as_path()))
}

#[derive(Debug)]
pub(crate) struct PackageFinding {
    pub track: usize,
    pub message: String,
}

/// Summarize what is known from the header index after preparation has stopped.
pub(super) fn package_index_findings(
    source: &SourceMaster,
    index: &MediaIndex,
    primary_track: Option<usize>,
) -> Vec<PackageFinding> {
    let mut findings = Vec::new();
    for (ordinal, track) in source.master.tracks.iter().enumerate() {
        if Some(ordinal) == primary_track {
            continue;
        }
        let package = track.file_mob.umid_hex();
        let label = package_label(track, &package);
        let message = match indexed_package(index, &package, &label) {
            Ok(Some(_)) => continue,
            Ok(None) => format!("{label} absent from the selected MXF package index"),
            Err(reason) => reason,
        };
        findings.push(PackageFinding { track: ordinal, message });
    }
    findings
}

#[derive(Debug)]
pub(crate) enum SiblingIssue {
    Check(String),
    Package { track: usize, message: String },
}

pub(crate) fn missing_siblings(
    source: &SourceMaster,
    index: &MediaIndex,
    check: &mut dyn FnMut() -> Result<(), String>,
    probe: &mut dyn FnMut(&Path) -> Result<Value, String>,
) -> Result<Vec<PackageFinding>, SiblingIssue> {
    let mut missing = Vec::new();
    let mut paths = HashSet::new();
    for (ordinal, track) in source.master.tracks.iter().enumerate() {
        check().map_err(SiblingIssue::Check)?;
        let at_track = |message| SiblingIssue::Package { track: ordinal, message };
        let package = track.file_mob.umid_hex();
        let video = track.kind == KIND_PICTURE;
        let label = package_label(track, &package);
        let Some(path) = indexed_package(index, &package, &label).map_err(at_track)? else {
            missing.push(PackageFinding { track: ordinal, message: label });
            continue;
        };
        let identity = index.identity(path).map_err(at_track)?;
        let (num, den) = if video { (24_000, 1001) } else { (48_000, 1) };
        let essence_track = (num, den, track.length);
        let timecode_track = (24_000, 1001, source.master.length);
        let matching_tracks = match identity.file_tracks.as_slice() {
            [only] => *only == essence_track,
            [first, second] => {
                (*first, *second) == (essence_track, timecode_track)
                    || (*first, *second) == (timecode_track, essence_track)
            }
            _ => false,
        };
        if !paths.insert(path)
            || identity.file_packages != [package]
            || identity.file_essence_tracks != 1
            || !matching_tracks
        {
            return Err(at_track(format!(
                "Legacy MXF file package, track, or extent disagrees for {label}"
            )));
        }
        let physical = probe(path)
            .map_err(|e| at_track(format!("Legacy MXF probe failed for {label}: {e}")))?;
        let Some(Value::Arr(streams)) = physical.get("streams") else {
            return Err(at_track(format!("Legacy MXF has no readable streams for {label}")));
        };
        let mut essence = streams
            .iter()
            .filter(|s| text(s, "codec_type").as_deref() != Some("data"));
        if !essence
            .next()
            .is_some_and(|s| matches_stream(s, video, track.length))
            || essence.next().is_some()
        {
            return Err(at_track(format!("Legacy MXF physical stream disagrees for {label}")));
        }
    }
    Ok(missing)
}

fn matches_stream(stream: &Value, video: bool, count: i64) -> bool {
    if number(stream, "start_pts") != Some(0) || number(stream, "duration_ts") != Some(count) {
        return false;
    }
    if video {
        text(stream, "codec_type").as_deref() == Some("video")
            && text(stream, "codec_name").as_deref() == Some("dnxhd")
            && text(stream, "time_base").as_deref() == Some("1001/24000")
            && text(stream, "field_order").as_deref() == Some("progressive")
            && text(stream, "sample_aspect_ratio").as_deref() == Some("1:1")
            && number(stream, "width") == Some(1920)
            && number(stream, "height") == Some(1080)
    } else {
        text(stream, "codec_type").as_deref() == Some("audio")
            && text(stream, "codec_name").as_deref() == Some("pcm_s24le")
            && text(stream, "time_base").as_deref() == Some("1/48000")
            && number(stream, "sample_rate") == Some(48_000)
            && number(stream, "channels") == Some(1)
    }
}
