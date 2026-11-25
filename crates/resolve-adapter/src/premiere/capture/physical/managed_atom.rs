//! Verify the physical members of an Avid-managed DNxHD/mono PCM24 set.
use super::{file_state, inspect_audio, n, ratio, s, span, umid, Physical, Stream};
use crate::{media, Result};
use avid_reader::mxf;
use graph::{
    source::{SourceClock, SourceClockProvenance},
    Rational, Value,
};
use std::{
    io::{BufRead, Read},
    path::Path,
};

pub const RECEIPT: &str = "premiere.managed_op_atom";
const OP_ATOM: &str = "060e2b34.04010102.0d010201.10030000";
const DNXHD_1253: [u8; 16] = [6, 14, 43, 52, 4, 1, 1, 10, 4, 1, 2, 2, 0x71, 0x13, 0, 0];
const DNXHD_FRAME_BYTES: i64 = 188_416;

#[derive(Debug)]
pub struct ManagedAtom {
    pub material: String,
    pub name: String,
    pub siblings: [String; 3],
    pub index: usize,
    pub clock: SourceClock,
    pub count: i64,
    pub transfer: Option<[u8; 16]>,
    pub primaries: Option<[u8; 16]>,
    pub matrix: Option<[u8; 16]>,
}

impl ManagedAtom {
    pub fn receipt(&self) -> Value {
        Value::Obj(vec![
            ("material_package".into(), Value::Str(self.material.clone())),
            ("name".into(), Value::Str(self.name.clone())),
            (
                "file_packages".into(),
                Value::Arr(self.siblings.iter().cloned().map(Value::Str).collect()),
            ),
        ])
    }
}

pub(super) fn is_op_atom(probe: &serde_json::Value) -> bool {
    s(&probe["format"], "format_name") == "mxf"
        && s(&probe["format"]["tags"], "operational_pattern_ul") == OP_ATOM
}

pub fn verify_avid_managed_atom(path: &Path, ffprobe: &Path) -> Result<ManagedAtom> {
    let state = file_state(path)?;
    let probe = media::probe_file(ffprobe, path, "mxf")?;
    let facts = inspect(path, ffprobe, &probe, state)?;
    facts
        .managed
        .ok_or_else(|| "missing managed OP-Atom identity".into())
}

pub(super) fn inspect(
    path: &Path,
    ffprobe: &Path,
    probe: &serde_json::Value,
    state: media::FileStamp,
) -> Result<Physical> {
    if !is_op_atom(probe) {
        return Err("expected Avid-managed MXF OP-Atom".into());
    }
    let rate = Rational::new(24_000, 1001).unwrap();
    let header = mxf::read_identity(path)?;
    let [material] = header.material_packages.as_slice() else {
        return Err("managed OP-Atom needs a unique material package".into());
    };
    let [file] = header.file_packages.as_slice() else {
        return Err("managed OP-Atom needs a unique file package".into());
    };
    let (mut video, mut audio) = (None, Vec::new());
    let streams = probe["streams"]
        .as_array()
        .filter(|streams| streams.len() == 3)
        .ok_or("managed OP-Atom needs a picture slot and two mono slots")?;
    let mut siblings = Vec::with_capacity(3);
    let mut physical = None;
    for (index, stream) in streams.iter().enumerate() {
        let kind = if index == 0 { "video" } else { "audio" };
        let id = s(&stream["tags"], "file_package_umid");
        if !umid(id) || n(stream, "index") != Some(index as i64) {
            return Err("managed OP-Atom has an ambiguous package selector".into());
        }
        let id = id[2..].to_ascii_uppercase();
        if id == *file {
            if physical.replace((index, stream)).is_some() || s(stream, "codec_type") != kind {
                return Err("managed OP-Atom has a conflicting essence slot".into());
            }
        } else if s(stream, "codec_type") != "data"
            || s(&stream["tags"], "data_type") != kind
            || stream.get("codec_name").is_some()
            || stream.get("nb_read_packets").is_some()
        {
            return Err("managed OP-Atom sibling slot contains unexpected essence".into());
        }
        if siblings.contains(&id) {
            return Err("managed OP-Atom has duplicate sibling packages".into());
        }
        siblings.push(id);
    }
    let (index, row) =
        physical.ok_or("managed OP-Atom file package is missing from the stream table")?;
    let witness = header
        .qualified_file_clock()
        .filter(|clock| {
            clock.rate == (24_000, 1001)
                && clock.rounded_base == 24
                && !clock.drop
                && clock.source_offset == 0
                && clock.effective_count < 24 * 86_400
        })
        .ok_or("managed OP-Atom needs a zero-offset physical timecode route at 23.976 fps")?;
    let clock = SourceClock {
        effective_count: witness.effective_count,
        rate,
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: witness.origin_count,
            source_offset: 0,
            source_rate: Rational::new(
                witness.source_rate.0 as i128,
                witness.source_rate.1 as i128,
            )
            .ok_or("invalid managed OP-Atom source clock rate")?,
            source_package: witness.source_package,
            timecode_track: witness.timecode_track,
            route: witness.route,
        },
    };
    let name = s(&probe["format"]["tags"], "material_package_name");
    if name.trim().is_empty() || name.len() > 255 {
        return Err("managed OP-Atom material package name is missing or too long".into());
    }
    let timecode = s(&row["tags"], "timecode");
    let (count, drop) = media::parse_timecode(timecode, 24)?;
    if drop
        || u64::try_from(count)? != clock.effective_count
        || s(&row["tags"], "reel_umid")
            .get(2..)
            .is_none_or(|id| !id.eq_ignore_ascii_case(&clock.provenance.source_package))
        || s(&probe["format"]["tags"], "material_package_umid")
            .get(2..)
            .is_none_or(|id| !id.eq_ignore_ascii_case(material))
    {
        return Err("managed OP-Atom stream identity or timecode contradicts its header".into());
    }
    let (time_base, count, duration) = span(row, false)?;
    let expected_track = if index == 0 {
        (24_000, 1001, count)
    } else {
        (48_000, 1, count)
    };
    if header.file_tracks.as_slice() != [expected_track] {
        return Err("managed OP-Atom stream extent contradicts its file package".into());
    }
    let label = |key| {
        header
            .picture_descriptor
            .iter()
            .find(|(name, _)| *name == key)
            .and_then(|(_, bytes)| bytes.as_slice().try_into().ok())
    };
    if index == 0 {
        if header.picture_essence_coding != Some(DNXHD_1253)
            || s(row, "codec_name") != "dnxhd"
            || s(row, "profile") != "DNXHD"
            || s(row, "pix_fmt") != "yuv422p"
            || s(row, "field_order") != "progressive"
            || !matches!(s(row, "sample_aspect_ratio"), "1:1" | "N/A" | "")
            || s(row, "color_range") != "tv"
            || n(row, "width").zip(n(row, "height")) != Some((1920, 1080))
            || s(row, "avg_frame_rate") != "0/0" && ratio(row, "avg_frame_rate")? != rate
            || ratio(row, "r_frame_rate")? != rate
            || time_base != Rational::new(1001, 24_000).unwrap()
        {
            return Err("managed OP-Atom needs progressive square-pixel HD DNxHD CID 1253".into());
        }
        picture_packets(ffprobe, path, count)?;
        video = Some(Stream {
            index: 0,
            codec: "dnxhd".into(),
            time_base,
            count,
            duration,
            size: Some((1920, 1080)),
            rate: Some(rate),
            channels: None,
            bits: None,
        });
    } else {
        let stream = inspect_audio(row, "mxf", path, ffprobe)?;
        if stream.bits != Some(24) || stream.channels != Some(1) || count % 2002 != 0 {
            return Err("managed OP-Atom needs mono 48 kHz PCM24 with a whole-frame extent".into());
        }
        audio.push(stream);
    }
    if file_state(path)? != state {
        return Err("managed OP-Atom changed while probing".into());
    }
    Ok(Physical {
        container: "mxf",
        video,
        audio,
        timecode: Some(timecode.into()),
        package: None,
        file_package: None,
        state,
        managed: Some(ManagedAtom {
            material: material.clone(),
            name: name.into(),
            siblings: siblings
                .try_into()
                .map_err(|_| "invalid OP-Atom sibling count")?,
            index,
            clock,
            count,
            transfer: label("transfer_ul"),
            primaries: label("primaries_ul"),
            matrix: label("matrix_ul"),
        }),
    })
}

fn picture_packets(ffprobe: &Path, path: &Path, count: i64) -> Result<()> {
    // Avid can append up to one alignment block pair after the last frame.
    // The bin writer verifies its actual bytes inside the essence KLV.
    media::inspect_movie_packets(
        ffprobe,
        path,
        "mxf",
        "v:0",
        "packet=pts,dts,duration,size",
        "csv=p=0",
        |output| {
            let mut row = String::new();
            for index in 0..count {
                row.clear();
                (&mut *output).take(128).read_line(&mut row)?;
                if row.trim_end() != format!("{index},{index},1,{DNXHD_FRAME_BYTES}") {
                    return Err("managed OP-Atom picture packet is off its saved frame grid".into());
                }
            }
            row.clear();
            (&mut *output).take(128).read_line(&mut row)?;
            if !row.is_empty()
                && (row.trim_end() != format!("{count},{count},1,8192")
                    || !output.fill_buf()?.is_empty())
            {
                return Err("managed OP-Atom picture has unqualified trailing packets".into());
            }
            Ok(())
        },
    )
}
