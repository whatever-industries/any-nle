//! Linked single-file stills. The archive retains one physical frame and an
//! explicit hold. Only the Premiere draft gets an extended editable extent.
use super::{
    Loss, Rational, Result, SourceGraph, SourceSequence, StreamKind, Value,
};
use crate::{
    input_scaling::{self, Sizing},
    linked::SidecarLookup,
    media::{probe_json, stamp, FileStamp},
    prepare::frames_on,
};
use graph::source::OriginalStream;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const SRGB_IEC61966_2_1: &str = "2b3aa1645779a9e634744faf9b01e9102b0c9b88fd6deced7934df86b949af7e";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Png,
    Jpeg,
}

impl Format {
    pub fn from_codec(codec: Option<&str>) -> Option<Self> {
        match codec? {
            "png" => Some(Self::Png),
            "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    fn matches_path(self, path: &Path) -> bool {
        path.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case(self.name())
                || self == Self::Jpeg && ext.eq_ignore_ascii_case("jpg")
        })
    }
}

pub(super) struct Original {
    path: PathBuf,
    stamp: FileStamp,
    pub format: Format,
    pub raster: (i64, i64),
}

impl Original {
    pub fn open(locator: &Path, tool: &Path, format: Format) -> Result<Self> {
        if !format.matches_path(locator) {
            return Err(format!("linked still needs a literal {} path", format.name()).into());
        }
        let path = fs::canonicalize(locator)?;
        let file = fs::File::open(&path)?;
        let stamp = stamp(file.metadata()?)?;
        if stamp.bytes > MAX_BYTES as u64 {
            return Err("linked still exceeds the file size limit".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != stamp.bytes {
            return Err("linked still changed while reading".into());
        }
        let raster = match format {
            Format::Png => decode_png(&bytes)?,
            Format::Jpeg => verify_jpeg(&bytes, tool, &path)?,
        };
        let original = Self {
            path,
            stamp,
            format,
            raster,
        };
        original.recheck(locator)?;
        Ok(original)
    }

    pub fn recheck(&self, locator: &Path) -> Result<()> {
        if fs::canonicalize(locator)? != self.path
            || stamp(fs::metadata(&self.path)?)? != self.stamp
        {
            return Err(format!("verified Resolve still changed: {}", locator.display()).into());
        }
        Ok(())
    }
}

fn bounded_raster(width: i64, height: i64) -> bool {
    (16..=8192).contains(&width) && (16..=8192).contains(&height) && width * height <= 16_777_216
}

fn decode_png(bytes: &[u8]) -> Result<(i64, i64)> {
    let mut chunks = bytes
        .strip_prefix(b"\x89PNG\r\n\x1a\n")
        .ok_or("invalid PNG signature")?;
    let mut metadata = HashSet::new();
    loop {
        let header = chunks.get(..8).ok_or("incomplete PNG chunk header")?;
        let size = u32::from_be_bytes(header[..4].try_into()?) as usize;
        chunks = chunks
            .get(size.checked_add(12).ok_or("PNG chunk size overflow")?..)
            .ok_or("incomplete PNG chunk")?;
        match &header[4..] {
            b"IHDR" | b"IDAT" => {}
            kind @ (b"pHYs" | b"iCCP" | b"sRGB") if metadata.insert(kind) => {}
            b"IEND" if size == 0 && chunks.is_empty() => break,
            _ => return Err("unqualified PNG animation, color, or ancillary chunk".into()),
        }
    }
    let decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: 2 * MAX_BYTES,
        },
    );
    let mut reader = decoder.read_info()?;
    let info = reader.info();
    let (width, height) = (i64::from(info.width), i64::from(info.height));
    if !bounded_raster(width, height)
        || info.interlaced
        || info.bit_depth != png::BitDepth::Eight
        || !matches!(info.color_type, png::ColorType::Rgb | png::ColorType::Rgba)
        || metadata.contains(&b"pHYs"[..])
            && !info
                .pixel_dims
                .is_some_and(|dims| dims.xppu != 0 && dims.xppu == dims.yppu)
        || metadata.contains(&b"sRGB"[..])
            && (info.srgb.is_none() || metadata.contains(&b"iCCP"[..]))
        || metadata.contains(&b"iCCP"[..])
            && !info
                .icc_profile
                .as_ref()
                .is_some_and(|icc| hex::encode(Sha256::digest(icc)) == SRGB_IEC61966_2_1)
    {
        return Err("linked PNG needs bounded square-pixel RGB with standard sRGB color".into());
    }
    let rgba = info.color_type == png::ColorType::Rgba;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= MAX_BYTES)
        .ok_or("PNG decoded extent exceeds the limit")?;
    let mut pixels = vec![0; size];
    reader.next_frame(&mut pixels)?;
    reader.finish()?;
    if rgba && pixels.chunks_exact(4).any(|pixel| pixel[3] != 255) {
        return Err("linked Resolve PNG with transparency requires separate qualification".into());
    }
    Ok((width, height))
}

fn jpeg_header(bytes: &[u8]) -> Result<((i64, i64), bool)> {
    let mut data = bytes
        .strip_prefix(b"\xff\xd8")
        .ok_or("invalid JPEG start marker")?;
    let mut raster = None;
    let mut jfif = false;
    let mut exif = false;
    loop {
        let header = data.get(..4).ok_or("incomplete JPEG segment")?;
        let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
        let segment = data
            .get(4..length + 2)
            .ok_or("invalid JPEG segment length")?;
        data = data.get(length + 2..).ok_or("truncated JPEG segment")?;
        if header[0] != 0xff {
            return Err("invalid JPEG marker".into());
        }
        match header[1] {
            0xe0 if !jfif && segment.starts_with(b"JFIF\x00") => jfif = true,
            0xe1 if !exif && segment.starts_with(b"Exif\x00\x00") => exif = true,
            0xc0 if raster.is_none()
                && segment.len() == 15
                && segment[0] == 8
                && segment[5] == 3 =>
            {
                let height = i64::from(u16::from_be_bytes([segment[1], segment[2]]));
                let width = i64::from(u16::from_be_bytes([segment[3], segment[4]]));
                if !bounded_raster(width, height) {
                    return Err("JPEG raster exceeds the linked still limit".into());
                }
                raster = Some((width, height));
            }
            0xc4 | 0xdb | 0xdd | 0xfe => {}
            0xda if jfif && segment.len() == 10 && segment[0] == 3 => {
                let scan = data
                    .strip_suffix(b"\xff\xd9")
                    .ok_or("JPEG scan has no end marker")?;
                if scan.is_empty()
                    || scan.last() == Some(&0xff)
                    || scan.windows(2).any(|pair| {
                        pair[0] == 0xff && !matches!(pair[1], 0x00 | 0xff | 0xd0..=0xd7)
                    })
                {
                    return Err("expected one JPEG scan and no trailing images".into());
                }
                return Ok((
                    raster.ok_or("JPEG scan has no baseline picture header")?,
                    exif,
                ));
            }
            _ => {
                return Err(
                    "linked JPEG needs baseline JFIF with no alternate profile or image metadata"
                        .into(),
                )
            }
        }
    }
}

fn verify_jpeg(bytes: &[u8], tool: &Path, path: &Path) -> Result<(i64, i64)> {
    let (raster, has_exif) = jpeg_header(bytes)?;
    let decoded = probe_json(tool, path, &[
        "-max_alloc", "100663296", "-max_pixels", "16777216", "-err_detect", "explode",
        "-f", "mjpeg", "-show_frames", "-show_entries",
        "frame=stream_index,width,height,pix_fmt,color_range,color_space,sample_aspect_ratio:frame_tags=Orientation,orientation:frame_side_data=side_data_type",
    ])?;
    let Some([frame]) = decoded
        .get("frames")
        .and_then(|frames| frames.as_array())
        .map(Vec::as_slice)
    else {
        return Err("linked JPEG did not decode to one image".into());
    };
    let side_data = frame
        .get("side_data_list")
        .and_then(|rows| rows.as_array())
        .map_or(&[][..], Vec::as_slice);
    if frame["width"].as_i64().zip(frame["height"].as_i64()) != Some(raster)
        || frame["stream_index"].as_i64() != Some(0)
        || !matches!(
            frame["pix_fmt"].as_str(),
            Some("yuvj420p" | "yuvj422p" | "yuvj444p")
        )
        || frame["color_range"].as_str() != Some("pc")
        || frame["color_space"].as_str() != Some("bt470bg")
        || frame["sample_aspect_ratio"].as_str() != Some("1:1")
        || ["Orientation", "orientation"].into_iter().any(|key| {
            frame["tags"]
                .get(key)
                .is_some_and(|tag| tag.as_str() != Some("1"))
        })
        || side_data.len() != usize::from(has_exif)
        || side_data
            .iter()
            .any(|row| row["side_data_type"].as_str() != Some("EXIF metadata"))
    {
        return Err(
            "linked JPEG needs a square-pixel full-range image in its saved orientation".into(),
        );
    }
    Ok(raster)
}

fn still_media(graph: &SourceGraph) -> HashMap<&str, &graph::source::SourceMedia> {
    graph
        .media
        .iter()
        .filter(|source| {
            source.kind == StreamKind::Video
                && Format::from_codec(source.codec.as_deref()).is_some()
        })
        .map(|source| (source.id.as_str(), source))
        .collect()
}

fn still_items<'a>(
    graph: &'a SourceGraph,
    media: &HashMap<&str, &graph::source::SourceMedia>,
) -> HashSet<&'a str> {
    graph.items.iter()
        .filter(|item| matches!(item.media.as_slice(), [link] if media.contains_key(link.media.as_str())))
        .map(|item| item.id.as_str())
        .collect()
}

pub(super) fn only_still_picture(graph: &SourceGraph, sequence: &SourceSequence) -> bool {
    let media = still_media(graph);
    if media.is_empty() || sequence.frame_size.is_none() {
        return false;
    }
    let items = still_items(graph, &media);
    let mut picture = sequence
        .placements
        .iter()
        .filter(|edit| edit.video_track.is_some())
        .peekable();
    picture.peek().is_some() && picture.all(|edit| items.contains(edit.item.as_str()))
}

pub(super) fn grade_omissions(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    claimed: &HashSet<&str>,
) -> Vec<Loss> {
    use crate::grade::{ADAPTER, CDL_ADAPTER, OPAQUE_ADAPTER, RANGE_ADAPTER};

    let media = still_media(graph);
    if media.is_empty() {
        return Vec::new();
    }
    let items = still_items(graph, &media);
    let graded: HashSet<_> = graph
        .sidecars
        .iter()
        .filter(|row| {
            matches!(
                row.adapter.as_str(),
                ADAPTER | CDL_ADAPTER | OPAQUE_ADAPTER | RANGE_ADAPTER
            )
        })
        .map(|row| row.object.as_str())
        .collect();
    sequence.placements.iter()
        .filter(|edit| edit.video_track.is_some() && items.contains(edit.item.as_str())
            && graded.contains(edit.id.as_str()) && !claimed.contains(edit.id.as_str()))
        .map(|edit| Loss {
            object: edit.id.clone(),
            aspect: "grade".into(),
            reason: format!(
                "Resolve clip grade omitted from the linked still on V{} at record time {} s. The original and hold are preserved.",
                edit.video_track.unwrap(), edit.record_start,
            ),
        })
        .collect()
}

/// Run after selecting the timeline and scoping its owners. Group angles use
/// their own range rules, so this only admits directly placed stills.
pub(super) fn prepare(graph: &mut SourceGraph) -> Result<Vec<String>> {
    let sequence = &graph.sequences[0];
    let lookup = SidecarLookup::Linear(graph);
    let still_media = still_media(graph);
    if still_media.is_empty() {
        return Ok(Vec::new());
    }
    let items: HashMap<_, _> = graph
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let mut holds = HashMap::new();
    let mut fitted = Vec::new();
    let frame = Rational::new(1, 24).unwrap();
    for edit in &sequence.placements {
        let item = items[edit.item.as_str()];
        let Some(source) = item
            .media
            .iter()
            .find_map(|link| still_media.get(link.media.as_str()))
        else {
            continue;
        };
        let fail = || {
            format!(
                "{}: unresolved Resolve still hold, geometry or source binding",
                edit.id
            )
        };
        if sequence.rate != Rational::integer(24)
            || !matches!(item.media.as_slice(), [link] if link.source_offset == Some(Rational::ZERO) && link.gain_db == 0.0)
            || !item.stream_groups.is_empty()
            || source.kind != StreamKind::Video || source.rate != Some(sequence.rate)
            || source.duration != frame || source.start.is_some() || source.clock.is_some()
            || source.stream != Some(1) || source.original.is_some()
            || !lookup.get(super::ADAPTER, &item.id).is_some_and(|saved| {
                saved.get("kind") == Some(&Value::Str("standalone_still".into()))
                    && saved.get("cache_frames") == Some(&Value::Int(1))
            })
            || lookup.get("resolve.media_geometry", &source.id).is_some_and(|geometry| {
                !matches!(geometry.get("pixel_aspect_ratio"), Some(Value::Str(value)) if value == "1")
            })
            || edit.video_track.is_none() || !edit.audio_tracks.is_empty() || !edit.grouped_audio.is_empty()
            || edit.source_in != Rational::ZERO || edit.speed.is_some() || edit.transform.is_some()
            || !edit.effects.is_empty()
            || !edit.frame_map.as_ref().map_or(edit.duration == frame, |map| {
                frames_on(edit.duration, sequence.rate).ok()
                    .and_then(|count| usize::try_from(count).ok()) == Some(map.len())
                    && map.iter().all(|index| *index == 0)
            })
        {
            return Err(fail().into());
        }
        let mismatch =
            source.frame_size.ok_or_else(fail)? != sequence.frame_size.ok_or_else(fail)?;
        let sizing = input_scaling::prepare(lookup, &sequence.id, &edit.id, mismatch)
            .map_err(|error| format!("{}: {error}", edit.id))?;
        if sizing != mismatch.then_some(Sizing::Fit) {
            return Err(fail().into());
        }
        if mismatch {
            fitted.push(edit.id.clone());
        }
        holds
            .entry(source.id.clone())
            .and_modify(|duration: &mut Rational| {
                *duration = (*duration).max(edit.duration);
            })
            .or_insert(edit.duration);
    }
    if holds.len() != still_media.len() {
        return Err("Resolve still originals need directly placed still holds".into());
    }
    for source in graph
        .media
        .iter_mut()
        .filter(|source| holds.contains_key(&source.id))
    {
        let [_locator] = source.locations.as_slice() else {
            return Err(format!("{}: linked still needs one original locator", source.id).into());
        };
        source.duration = holds[&source.id];
        source.online = Some(true);
        source.original = Some(OriginalStream {
            saved_locators: source.locations.clone(),
            container: source.codec.clone().ok_or("missing linked still codec")?,
            stream_index: 0,
            channel_index: None,
            channels: None,
            time_base: frame,
            start_pts: 0,
            duration_ts: 1,
            sample_representation: None,
        });
    }
    for edit in &mut graph.sequences[0].placements {
        if items[edit.item.as_str()]
            .media
            .iter()
            .any(|link| holds.contains_key(&link.media))
        {
            edit.frame_map = None;
        }
    }
    Ok(fitted)
}

pub(super) fn apply_fit(
    prepared: &mut premiere_adapter::PreparedPremiereInput,
    fitted: Vec<String>,
) -> Result<()> {
    for owner in fitted {
        let &edit = prepared
            .ownership
            .get(&owner)
            .ok_or_else(|| format!("{owner}: Resolve still fitting lost its prepared edit"))?;
        prepared.input.fit_picture.insert(edit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(alpha: u8, aspect: (u32, u32)) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 32, 16);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_pixel_dims(Some(png::PixelDimensions {
            xppu: aspect.0,
            yppu: aspect.1,
            unit: png::Unit::Unspecified,
        }));
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[1, 2, 3, alpha].repeat(32 * 16))
            .unwrap();
        writer.finish().unwrap();
        bytes
    }

    #[test]
    fn png_proof_requires_complete_square_pixel_opaque_image() {
        let good = rgba(255, (1, 1));
        assert_eq!(decode_png(&good).unwrap(), (32, 16));
        assert!(decode_png(&rgba(254, (1, 1)))
            .unwrap_err()
            .to_string()
            .contains("transparency"));
        assert!(decode_png(&rgba(255, (1, 2)))
            .unwrap_err()
            .to_string()
            .contains("square-pixel"));
        assert!(decode_png(&good[..good.len() - 2]).is_err());
        let mut trailing = good;
        trailing.push(0);
        assert!(decode_png(&trailing).is_err());
    }

    #[test]
    fn jpeg_header_requires_one_baseline_scan_at_the_saved_raster() {
        // Header witness only. Physical binding separately requires decoding.
        let image = [
            0xff, 0xd8, 0xff, 0xe0, 0, 7, b'J', b'F', b'I', b'F', 0, 0xff, 0xc0, 0, 17, 8, 0, 16,
            0, 32, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1, 0xff, 0xda, 0, 12, 3, 1, 0, 2, 0x11, 3,
            0x11, 0, 63, 0, 0xab, 0xff, 0, 0xc7, 0xff, 0xd9,
        ];
        assert_eq!(jpeg_header(&image).unwrap(), ((32, 16), false));
        assert!(jpeg_header(&image[..image.len() - 1]).is_err());
        assert!(jpeg_header(&[image, image].concat()).is_err());
        let mut oversized = image;
        oversized[18] = 0xff;
        assert!(jpeg_header(&oversized).is_err());
    }
}
