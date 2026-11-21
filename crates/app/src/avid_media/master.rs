//! Internal AVB master for the indexed 1080p24 DNxHR SQ and dual PCM24 pilot.
//! The index remains owned by Media Composer. No database file is consumed here.
mod avb;

use super::{timecode, MediaFile, Result, MEDIA_DIR};
use crate::{backend_bundle::BackendTools, output, rewrap};
use avid_reader::io::MobId;
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const RATE: i64 = 24;
const SAMPLE_RATE: i64 = 48_000;
const SAMPLES_PER_FRAME: i64 = SAMPLE_RATE / RATE;
const TICKS: i64 = super::TICKS;

#[derive(Deserialize)]
struct Manifest {
    schema: String,
    edit_rate: String,
    timecode: String,
    files: Vec<MediaFile>,
}

struct BoundFile {
    package: MobId,
    upstream: MobId,
    lane: i16,
    path: PathBuf,
    stamp: rewrap::source::Stamp,
    offset: i64,
    kind: Essence,
}

enum Essence {
    Video { coding: [u8; 16] },
    Sound { samples: i64 },
}

impl BoundFile {
    fn track_kind(&self) -> i64 {
        match self.kind {
            Essence::Video { .. } => 1,
            Essence::Sound { .. } => 2,
        }
    }
}

/// Write a fresh bin from a stage manifest and files on a standalone Avid media volume.
pub fn write_master_bin(
    manifest_path: &Path,
    folder: &Path,
    destination: &Path,
    tools: &BackendTools,
) -> Result<()> {
    output::preflight(destination, None)?;
    if !destination
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("avb"))
    {
        return Err("managed master destination needs an .avb extension".into());
    }
    let name = destination
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty() && name.len() <= 255)
        .ok_or("managed master needs a UTF-8 bin name of 1 to 255 bytes")?;
    let folder = fs::canonicalize(folder)?;
    let volume = media_volume(&folder)?;
    let bytes = read_manifest(manifest_path)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema != "anynle.avid-media-stage/1" || manifest.edit_rate != "24" {
        return Err("expected an Avid media stage/1 manifest at 24 fps".into());
    }
    let clock = timecode(&manifest.timecode, 24, false)?;
    let start = u32::try_from(clock.0)?;
    let frames = manifest
        .files
        .first()
        .and_then(|file| file.frames)
        .filter(|frames| *frames > 0 && *frames <= i64::from(i32::MAX) / SAMPLES_PER_FRAME)
        .ok_or("managed master needs a positive picture extent within the PCM descriptor limit")?;
    let [video, a1, a2] = manifest.files.as_slice() else {
        return Err("managed master requires picture and exactly two mono files".into());
    };
    if video.source_channel.is_some()
        || video.samples.is_some()
        || a1.source_channel.is_none()
        || a2.source_channel.is_none()
        || (a1.source_stream, a1.source_channel) == (a2.source_stream, a2.source_channel)
    {
        return Err(
            "managed master requires picture and two distinct source audio selectors".into(),
        );
    }
    let files = [
        inspect(
            &folder,
            video,
            "Picture.mxf",
            1,
            frames,
            clock,
            &manifest.timecode,
            tools,
        )?,
        inspect(
            &folder,
            a1,
            "Audio-01.mxf",
            1,
            frames,
            clock,
            &manifest.timecode,
            tools,
        )?,
        inspect(
            &folder,
            a2,
            "Audio-02.mxf",
            2,
            frames,
            clock,
            &manifest.timecode,
            tools,
        )?,
    ];
    let mut ids = HashSet::new();
    for file in &files {
        if !ids.insert(file.package) || !ids.insert(file.upstream) {
            return Err(format!(
                "{}: duplicate managed package identity",
                file.path.display()
            )
            .into());
        }
    }
    let frames = u32::try_from(frames)?;
    let doc = avb::build(name, volume, start, frames, &files)?;
    if read_manifest(manifest_path)? != bytes {
        return Err("stage manifest changed during managed master preparation".into());
    }
    for file in &files {
        if rewrap::source::stamp(&file.path)? != file.stamp {
            return Err(format!(
                "{} changed during managed master preparation",
                file.path.display()
            )
            .into());
        }
    }
    doc.write_new(destination)?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Vec<u8>> {
    const MAX_BYTES: u64 = 65_536;
    if !fs::metadata(path)?.is_file() {
        return Err("media stage manifest must be a regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES as usize {
        return Err("media stage manifest exceeds 64 KiB".into());
    }
    Ok(bytes)
}

fn media_volume(folder: &Path) -> Result<&str> {
    let valid_number = folder
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|s| {
            s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u32>().is_ok_and(|n| n > 0)
        });
    let volume = folder
        .parent()
        .filter(|p| p.ends_with("MXF"))
        .and_then(Path::parent)
        .filter(|p| p.ends_with(MEDIA_DIR))
        .and_then(Path::parent)
        .filter(|_| valid_number)
        .ok_or("select a numbered Avid MediaFiles/MXF folder on the mounted media drive")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if volume.parent().is_none()
            || fs::metadata(volume)?.dev()
                == fs::metadata(volume.parent().ok_or("missing volume parent")?)?.dev()
        {
            return Err("Avid MediaFiles must be at the mounted media drive root".into());
        }
    }
    #[cfg(not(unix))]
    return Err("this managed master pilot requires a named mounted media volume".into());
    #[cfg(unix)]
    volume
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| s.is_ascii() && !s.chars().any(char::is_control))
        .ok_or_else(|| "the managed master pilot needs a named ASCII media volume".into())
}

#[allow(clippy::too_many_arguments)]
fn inspect(
    folder: &Path,
    file: &MediaFile,
    name: &str,
    lane: i16,
    frames: i64,
    clock: (u64, u16, bool),
    label: &str,
    tools: &BackendTools,
) -> Result<BoundFile> {
    let invalid = |message: &str| format!("{name}: {message}");
    if file.name != name {
        return Err(invalid("manifest entries are out of order or the filename changed").into());
    }
    let path = folder.join(name);
    if !fs::symlink_metadata(&path)?.is_file() {
        return Err(invalid("expected a regular OP-Atom file").into());
    }
    let stamp = rewrap::source::stamp(&path)?;
    let atom =
        rewrap::inspect_staged_atom(&tools.ffprobe, &path).map_err(|error| invalid(&error))?;
    if atom.file != file.file_package_umid || atom.material != file.material_package_umid {
        return Err(invalid("MXF package IDs disagree with the stage manifest").into());
    }
    let package = MobId::from_urn(&atom.file)?;
    let material = MobId::from_urn(&atom.material)?;
    let mut expected_file = material;
    expected_file.material.data4[7] = 1;
    if material.material.data4[7] != 0 || material.instance[0] == 0xa1 || package != expected_file {
        return Err(invalid("unqualified FFmpeg material and file package relationship").into());
    }
    // The native index promotes each zero-origin file to a physical source mob
    // by using the material package's identity with this source-instance value.
    let mut upstream = material;
    upstream.instance[0] = 0xa1;
    if atom.timecode.as_deref() != Some(label) || atom.header_timecode != Some(clock) {
        return Err(invalid("MXF timecode disagrees with the stage manifest").into());
    }
    let spans = rewrap::essence_spans(&path).map_err(|error| invalid(&error))?;
    let [span] = spans.as_slice() else {
        return Err(invalid("expected a single contiguous OP-Atom essence KLV").into());
    };
    let offset = i64::try_from(span.offset)?;
    let kind = if file.source_channel.is_none() {
        let coding = qualify_picture(&atom, file, frames)
            .ok_or_else(|| invalid("expected qualified 1080p24 DNxHR SQ picture"))?;
        if span.kind != 0x15 || span.bytes != u64::try_from(frames)? * 606_208 {
            return Err(invalid("DNxHR SQ payload size disagrees with its frame count").into());
        }
        Essence::Video { coding }
    } else {
        let samples = frames * SAMPLES_PER_FRAME;
        if file.samples != Some(samples)
            || file.frames.is_some()
            || file.encoding != "pcm_s24le"
            || atom.codec != "pcm_s24le"
            || (atom.channels, atom.sample_rate, atom.bits) != (1, SAMPLE_RATE, 24)
            || atom.ticks != samples * (TICKS / SAMPLE_RATE)
            || span.kind != 0x16
            || span.bytes != u64::try_from(samples)? * 3
        {
            return Err(invalid("expected a full-length mono PCM24 file at 48 kHz").into());
        }
        Essence::Sound { samples }
    };
    Ok(BoundFile {
        package,
        upstream,
        lane,
        path,
        stamp,
        offset,
        kind,
    })
}

fn qualify_picture(atom: &rewrap::AtomEssence, file: &MediaFile, frames: i64) -> Option<[u8; 16]> {
    let header = |field, want: &[u8]| {
        atom.picture_descriptor
            .iter()
            .any(|(key, value)| *key == field && value == want)
    };
    let descriptor_matches = [
        ("stored_width", 1920u32),
        ("stored_height", 1088),
        ("display_width", 1920),
        ("display_height", 1080),
        ("component_depth", 8),
        ("horizontal_subsampling", 2),
        ("vertical_subsampling", 1),
        ("black_reference", 16),
        ("white_reference", 235),
        ("color_range", 225),
    ]
    .iter()
    .all(|&(name, value)| header(name, &value.to_be_bytes()));
    (file.encoding == "DNXHR SQ"
        && atom.profile == "DNXHR SQ"
        && atom.header_picture_profile == Some(avid_reader::mxf::PictureProfile::Dnx(1273))
        && atom.codec == "dnxhd"
        && atom.raster == (1920, 1080)
        && atom.pix == "yuv422p"
        && atom.packets == frames
        && atom.ticks == frames * (TICKS / RATE)
        && descriptor_matches
        && header("frame_layout", &[0])
        && header("display_aspect", &[0, 0, 0, 16, 0, 0, 0, 9]))
    .then_some(atom.picture_essence_coding)
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use avid_reader::{graph::direct_refs, primitives_extra::new_unique_mob_id};

    #[test]
    fn managed_master_places_referenced_objects_before_the_bin_root() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let file = |lane, kind| BoundFile {
            package: new_unique_mob_id().unwrap(),
            upstream: new_unique_mob_id().unwrap(),
            lane,
            stamp: rewrap::source::stamp(&path).unwrap(),
            path: path.clone(),
            offset: 5657,
            kind,
        };
        let files = [
            file(
                1,
                Essence::Video {
                    coding: [6, 14, 43, 52, 4, 1, 1, 10, 4, 1, 2, 2, 113, 39, 0, 0],
                },
            ),
            file(1, Essence::Sound { samples: 24_000 }),
            file(2, Essence::Sound { samples: 24_000 }),
        ];
        let bin = avb::build("Managed master", "Test volume", 120_606, 12, &files).unwrap();
        assert_eq!(bin.root_id() as usize, bin.object_count());
        for id in 1..=bin.root_id() {
            let object = bin.node(id).unwrap();
            assert!(direct_refs(object).iter().all(|target| *target < id));
        }
    }
}
