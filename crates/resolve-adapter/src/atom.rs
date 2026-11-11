//! Verification of the bounded Avid DNxHR LB and two mono OP-Atom sibling profile.
use crate::{
    media::{media_digest, probe_file, tc_frames},
    prepare::{rate, utf8_path},
    Result, StreamSelector, VerifiedBinding,
};
use avid_reader::mxf::{read_identity, QualifiedClock, DNXHR_LB_CODING};
use graph::{source::SourceMedia, Rational};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomBindingSpec {
    pub(crate) item_id: String,
    // Ordered by graph lane: picture, A1, A2. Physical indexes are per file.
    pub(crate) siblings: [SiblingSpec; 3],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SiblingSpec {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) media_id: String,
    pub(crate) index: usize,
    pub(crate) channel: Option<usize>,
}

pub(crate) struct AtomInput {
    pub(crate) path: PathBuf,
    pub(crate) stream: StreamSelector,
    pub(crate) expected_sha256: Option<String>,
}

pub(crate) struct AtomMedia {
    pub(crate) files: [AtomFile; 3],
    pub(crate) master_package: [u8; 32],
    pub(crate) reel: String,
}

pub(crate) struct AtomFile {
    pub(crate) path: PathBuf,
    pub(crate) package: [u8; 32],
    pub(crate) modified_micros: u64,
    clock: QualifiedClock,
    digest: [u8; 32],
}

impl AtomMedia {
    pub(crate) fn recheck(&self) -> Result<()> {
        for file in &self.files {
            if media_digest(&file.path)? != file.digest {
                return Err("verified OP-Atom sibling bytes changed".into());
            }
        }
        Ok(())
    }
}

impl AtomFile {
    pub(crate) fn agrees_with_clock(&self, media: &SourceMedia) -> bool {
        let Some(clock) = &media.clock else {
            return false;
        };
        let witness = &self.clock;
        let origin = &clock.provenance;
        clock.effective_count == witness.effective_count
            && clock.rate == rate()
            && clock.display_base == 24
            && !clock.drop
            && media.start == clock.exact_start()
            && origin.origin_count == witness.origin_count
            && origin.source_offset == witness.source_offset
            && Some(origin.source_rate)
                == Rational::new(witness.source_rate.0.into(), witness.source_rate.1.into())
            && origin.source_package == witness.source_package
            && origin.timecode_track == witness.timecode_track
            && origin.route == witness.route
    }
}

fn umid(value: &Value) -> Result<[u8; 32]> {
    let text = value
        .as_str()
        .and_then(|s| s.strip_prefix("0x"))
        .ok_or("missing OP-Atom package UMID")?;
    hex::decode(text)?
        .try_into()
        .map_err(|_| "invalid OP-Atom package UMID".into())
}

pub fn verify_atom_binding(spec: AtomBindingSpec, tool: &Path) -> Result<VerifiedBinding> {
    verify_atom(
        spec.item_id,
        spec.siblings.map(|sibling| AtomInput {
            path: sibling.path,
            stream: StreamSelector {
                media_id: sibling.media_id,
                index: sibling.index,
                channel: sibling.channel,
            },
            expected_sha256: Some(sibling.sha256),
        }),
        tool,
    )
}

pub(crate) fn verify_atom(
    item_id: String,
    inputs: [AtomInput; 3],
    tool: &Path,
) -> Result<VerifiedBinding> {
    let mut files = Vec::new();
    let mut selectors = Vec::new();
    let mut master = [0; 32];
    let mut origin = 0;
    let mut frames = 0;
    let mut reel = String::new();
    let mut packages = [[0; 32]; 3];
    for (lane, sibling) in inputs.into_iter().enumerate() {
        let picture = lane == 0;
        if sibling.stream.media_id.is_empty()
            || sibling.stream.index != lane
            || sibling.stream.channel != (!picture).then_some(0)
        {
            return Err("unsupported physical OP-Atom sibling selector".into());
        }
        let path = fs::canonicalize(&sibling.path)?;
        utf8_path(&path)?;
        if path.extension().and_then(|s| s.to_str()) != Some("mxf") {
            return Err("expected a verified OP-Atom MXF file".into());
        }
        let digest = media_digest(&path)?;
        if let Some(expected) = sibling.expected_sha256 {
            let expected: [u8; 32] = hex::decode(expected)?
                .try_into()
                .map_err(|_| "bad verified sibling digest")?;
            if digest != expected {
                return Err("OP-Atom sibling does not match its verified digest".into());
            }
        }
        let probe = probe_file(tool, &path, "mxf")?;
        let tags = &probe["format"]["tags"];
        let pattern = tags["operational_pattern_ul"]
            .as_str()
            .unwrap_or_default()
            .replace('.', "");
        if !pattern.starts_with("060e2b34040101") || pattern.get(16..28) != Some("0d0102011003") {
            return Err("expected the qualified Avid OP-Atom operational pattern".into());
        }
        let material = umid(&tags["material_package_umid"])?;
        let streams = probe["streams"]
            .as_array()
            .ok_or("missing OP-Atom stream inspection")?;
        let [_, _, _] = streams.as_slice() else {
            return Err("expected the three OP-Atom sibling references".into());
        };
        let stream = &streams[lane];
        if stream["index"] != lane
            || stream["codec_type"] != if picture { "video" } else { "audio" }
        {
            return Err("OP-Atom sibling does not contain the selected physical essence".into());
        }
        let clock = tc_frames(
            stream["tags"]["timecode"]
                .as_str()
                .ok_or("missing OP-Atom timecode")?,
        )?;
        let package = umid(&stream["tags"]["file_package_umid"])?;
        if picture {
            master = material;
            origin = clock;
            reel = stream["tags"]["reel_name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if reel.len() > 255 || reel.chars().any(char::is_control) {
                return Err("unsupported OP-Atom reel name".into());
            }
            frames = stream["duration_ts"]
                .as_i64()
                .filter(|n| (1..=i64::from(i32::MAX)).contains(n))
                .ok_or("missing bounded OP-Atom video duration")?;
            if stream["codec_name"] != "dnxhd"
                || stream["profile"] != "DNXHR LB"
                || stream["width"] != 1920
                || stream["height"] != 1080
                || stream["coded_width"] != 1920
                || stream["coded_height"] != 1088
                || stream["sample_aspect_ratio"] != "1:1"
                || stream["field_order"] != "progressive"
                || stream["pix_fmt"] != "yuv422p"
                || stream["r_frame_rate"] != "24000/1001"
                || stream["time_base"] != "1001/24000"
                || stream["color_range"] != "tv"
                || stream["color_space"] != "bt709"
                || stream["color_transfer"] != "bt709"
                || stream["color_primaries"] != "bt709"
            {
                return Err("unsupported OP-Atom DNxHR LB picture profile".into());
            }
        } else if stream["codec_name"] != "pcm_s24le"
            || stream["sample_rate"] != "48000"
            || stream["time_base"] != "1/48000"
            || stream["channels"] != 1
            || stream["bits_per_sample"] != 24
            || stream["duration_ts"] != frames * 2002
        {
            return Err("unsupported OP-Atom mono PCM24 profile or sample extent".into());
        }
        if material != master || clock != origin || stream["start_pts"] != 0 {
            return Err("OP-Atom sibling master identity or clock mismatch".into());
        }
        for (i, reference) in streams.iter().enumerate() {
            let reference_package = umid(&reference["tags"]["file_package_umid"])?;
            if reference["index"] != i
                || reference_package == [0; 32]
                || (!picture && reference_package != packages[i])
                || (i != lane
                    && (reference["codec_type"] != "data"
                        || reference["tags"]["data_type"]
                            != if i == 0 { "video" } else { "audio" }))
            {
                return Err("OP-Atom sibling references or physical selectors disagree".into());
            }
            if picture {
                packages[i] = reference_package;
            }
        }
        if packages[0] == packages[1] || packages[0] == packages[2] || packages[1] == packages[2] {
            return Err("OP-Atom sibling file packages must be distinct".into());
        }
        let identity = read_identity(&path)?;
        let native_clock = identity
            .qualified_file_clock()
            .ok_or("unresolved OP-Atom file clock")?;
        let expected_extent = if picture {
            (24000, 1001, frames)
        } else {
            (48000, 1, frames * 2002)
        };
        if identity.material_packages != [hex::encode_upper(master)]
            || identity.file_packages != [hex::encode_upper(package)]
            || identity.file_tracks != [expected_extent]
            || native_clock.effective_count != origin as u64
            || native_clock.rate != (24000, 1001)
            || native_clock.rounded_base != 24
            || native_clock.drop
            || (picture && identity.picture_essence_coding != Some(DNXHR_LB_CODING))
        {
            return Err(
                "physical OP-Atom package, clock or essence extent contradicts the binding".into(),
            );
        }
        verify_packets(tool, &path, picture, expected_extent.2)?;
        let modified_micros = fs::metadata(&path)?
            .modified()?
            .duration_since(UNIX_EPOCH)?
            .as_micros()
            .try_into()?;
        files.push(AtomFile {
            path,
            package,
            modified_micros,
            clock: native_clock,
            digest,
        });
        selectors.push(sibling.stream);
    }
    let files: [AtomFile; 3] = files.try_into().map_err(|_| "missing OP-Atom siblings")?;
    let streams: [StreamSelector; 3] = selectors
        .try_into()
        .map_err(|_| "missing OP-Atom selectors")?;
    for i in 0..3 {
        for j in i + 1..3 {
            if files[i].path == files[j].path || streams[i].media_id == streams[j].media_id {
                return Err("duplicate OP-Atom sibling binding".into());
            }
        }
    }
    let binding = VerifiedBinding {
        item_id,
        path: files[0].path.clone(),
        streams,
        frames,
        timecode_frames: origin,
        source_package: files[0].package,
        modified_micros: files[0].modified_micros,
        tagged_rec709: true,
        digest: files[0].digest,
        atom: Some(AtomMedia {
            files,
            master_package: master,
            reel,
        }),
    };
    binding.recheck()?;
    Ok(binding)
}

fn verify_packets(tool: &Path, path: &Path, picture: bool, extent: i64) -> Result<()> {
    let mut child = Command::new(tool)
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mxf",
            "-select_streams",
            if picture { "v:0" } else { "a:0" },
            "-show_entries",
            "packet=pts,duration,size,pos",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| -> Result<()> {
        let mut output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or("missing OP-Atom packet inspection")?,
        );
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut line = String::new();
        let mut cursor = 0;
        let mut previous_end = 0;
        let mut tail = false;
        loop {
            line.clear();
            if (&mut output).take(96).read_line(&mut line)? == 0 {
                break;
            }
            let mut values = line.trim_end().split(',').map(str::parse::<u64>);
            let [Some(Ok(pts)), Some(Ok(duration)), Some(Ok(size)), Some(Ok(offset))] =
                std::array::from_fn(|_| values.next())
            else {
                return Err("invalid OP-Atom packet inspection".into());
            };
            if !line.ends_with('\n')
                || values.next().is_some()
                || pts != cursor
                || duration == 0
                || offset < previous_end
            {
                return Err("noncontiguous or malformed OP-Atom packet grid".into());
            }
            previous_end = offset
                .checked_add(size)
                .filter(|n| *n <= length)
                .ok_or("OP-Atom packet exceeds file")?;
            if picture && cursor == extent as u64 {
                if tail || duration != 1 || !matches!(size, 16384 | 24576) {
                    return Err("unqualified extra DNxHR OP-Atom packet".into());
                }
                file.seek(SeekFrom::Start(offset))?;
                let mut padding = [0; 24576];
                let padding = &mut padding[..usize::try_from(size)?];
                file.read_exact(padding)?;
                if padding.iter().any(|&byte| byte != 0) {
                    return Err("nonzero data in extra DNxHR OP-Atom packet".into());
                }
                tail = true;
                continue;
            }
            if (picture && (duration != 1 || size != 188416))
                || (!picture && duration.checked_mul(3) != Some(size))
            {
                return Err("OP-Atom packet disagrees with qualified essence size".into());
            }
            cursor = cursor
                .checked_add(duration)
                .filter(|n| *n <= extent as u64)
                .ok_or("OP-Atom packets exceed the qualified playable extent")?;
        }
        if cursor != extent as u64 {
            return Err("OP-Atom packets do not cover the playable extent".into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    if !status.success() {
        return Err("OP-Atom packet inspection failed".into());
    }
    Ok(())
}
