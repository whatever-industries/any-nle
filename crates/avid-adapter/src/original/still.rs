//! Physical admission for the observed full-raster, untagged imported-still
//! profile. This does not reconstruct missing historical file identity.
use super::{missing_locator, resolve_with_claim, RootMapping};
use avid_reader::source::SourceInput;
use std::{
    fmt,
    io::{BufReader, Cursor, Read},
    path::Path,
};

/// Admit EXIF only when its directories describe dimensions and identity
/// orientation. The linked original remains unchanged.
pub(super) fn linked_png_with_dimension_metadata(path: &Path, raster: (i64, i64)) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let decoder = png::Decoder::new_with_limits(
        BufReader::new(file),
        png::Limits { bytes: 1024 * 1024 },
    );
    let Ok(mut reader) = decoder.read_info() else { return false };
    if reader.finish().is_err() {
        return false;
    }
    let info = reader.info();
    (i64::from(info.width), i64::from(info.height)) == raster
        && info.animation_control.is_none()
        && info.icc_profile.is_none()
        && info.uncompressed_latin1_text.is_empty()
        && info.compressed_latin1_text.is_empty()
        && info.utf8_text.is_empty()
        && info.exif_metadata.as_deref().is_some_and(|bytes| {
            verify_exif_dimensions(bytes, [info.width, info.height]).is_some()
        })
}

fn verify_exif_dimensions(bytes: &[u8], [width, height]: [u32; 2]) -> Option<()> {
    let (short, long): (fn([u8; 2]) -> u16, fn([u8; 4]) -> u32) = match bytes.get(..4)? {
        b"II\x2a\0" => (u16::from_le_bytes, u32::from_le_bytes),
        b"MM\0\x2a" => (u16::from_be_bytes, u32::from_be_bytes),
        _ => return None,
    };
    if bytes.len() > 4096 {
        return None;
    }
    let mut offset = usize::try_from(long(bytes.get(4..8)?.try_into().ok()?)).ok()?;
    for is_primary in [true, false] {
        if offset < 8 {
            return None;
        }
        let count = usize::from(short(bytes.get(offset..)?.get(..2)?.try_into().ok()?));
        if !(1..=4).contains(&count) {
            return None;
        }
        let directory = bytes.get(offset..)?.get(2..2 + count * 12 + 4)?;
        // A secondary dimension directory can end with a pointer to EOF.
        // An in-file continuation would introduce another unverified directory.
        let next_ifd = long(directory[count * 12..].try_into().ok()?);
        if next_ifd != 0 && (is_primary || usize::try_from(next_ifd).ok()? != bytes.len()) {
            return None;
        }
        let mut child = None;
        let mut tags = std::collections::HashSet::new();
        for entry in directory[..count * 12].chunks_exact(12) {
            let tag = short(entry[..2].try_into().ok()?);
            let kind = short(entry[2..4].try_into().ok()?);
            if !tags.insert(tag) || long(entry[4..8].try_into().ok()?) != 1 {
                return None;
            }
            let value = match kind {
                3 => u32::from(short(entry[8..10].try_into().ok()?)),
                4 | 13 => long(entry[8..12].try_into().ok()?),
                _ => return None,
            };
            match (is_primary, tag, kind) {
                (true, 0x0100, 3 | 4) | (false, 0xa002, 3 | 4) if value == width => (),
                (true, 0x0101, 3 | 4) | (false, 0xa003, 3 | 4) if value == height => (),
                (true, 0x0112, 3) if value == 1 => (),
                (true, 0x8769, 4 | 13) if usize::try_from(value).ok()? >= offset + 2 + directory.len() => {
                    child = usize::try_from(value).ok();
                }
                _ => return None,
            }
        }
        let Some(next) = child else { return Some(()) };
        offset = next;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngRole {
    /// Ignores saved alpha, including on the retained RGBA comparison still.
    Opaque,
    /// The saved import uses an actual alpha channel.
    Matte,
    /// The direct overlay preserves only fully transparent or opaque pixels.
    BinaryMatte,
}

const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_DECODED: usize = 1920 * 1080 * 4;

pub struct VerifiedPng {
    pub input: SourceInput,
    /// A complete decoded image can still be unsuitable for a direct overlay.
    pub overlay_alpha_issue: Option<&'static str>,
    /// Verified shapes for the transparent and opaque planes, respectively.
    pub alpha_shapes: [Option<BinaryAlphaShape>; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinaryAlphaShape {
    /// Half-open pixel bounds in left, top, right, bottom order.
    pub outer: [u32; 4],
    pub hole: Option<[u32; 4]>,
}

#[derive(Debug)]
pub enum PngVerificationError {
    MissingOriginal,
    Invalid(String),
}

impl From<String> for PngVerificationError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

impl fmt::Display for PngVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOriginal => f.write_str(&missing_locator()),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

/// Resolve only a saved locator or explicit root map. Bind the revision to the
/// very bytes that were decoded so the caller can recheck it before publication.
pub fn verify_imported_png(
    locators: &[String],
    mappings: &[RootMapping],
    role: PngRole,
) -> Result<VerifiedPng, PngVerificationError> {
    let (path, _) = resolve_with_claim(locators, mappings)?
        .ok_or(PngVerificationError::MissingOriginal)?;
    let file = std::fs::File::open(&path).map_err(|e| format!("open imported still: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("stat imported still: {e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE as u64 {
        return Err(PngVerificationError::Invalid(
            "imported PNG is not a regular file within the size limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read imported still: {e}"))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(PngVerificationError::Invalid(
            "imported PNG size changed while reading".into(),
        ));
    }
    let (overlay_alpha_issue, alpha_shapes) = verify_png(&path, &bytes, role)?;
    let revision = crate::revision::sha256_reader(bytes.as_slice())
        .map_err(|e| format!("hash imported still: {e}"))?;
    Ok(VerifiedPng {
        input: SourceInput { path, revision: hex::encode(revision) },
        overlay_alpha_issue,
        alpha_shapes,
    })
}

type AlphaProfile = (Option<&'static str>, [Option<BinaryAlphaShape>; 2]);

fn verify_png(path: &Path, bytes: &[u8], role: PngRole) -> Result<AlphaProfile, String> {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("png"))
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("imported still is not a PNG".into());
    }
    // The earlier profiles admit only image data. The binary overlay also
    // admits the observed sRGB metadata and dimension witnesses.
    let mut remainder = &bytes[8..];
    let mut count = 0;
    let mut image_data = false;
    let mut metadata = std::collections::HashSet::new();
    loop {
        let Some(header) = remainder.get(..8) else {
            return Err("truncated PNG chunk".into());
        };
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let total = size.checked_add(12).ok_or("PNG chunk size overflow")?;
        let data = remainder.get(8..8 + size).ok_or("truncated PNG chunk")?;
        remainder = remainder.get(total..).ok_or("truncated PNG chunk")?;
        match (&header[4..], count, size) {
            (b"IHDR", 0, 13) => (),
            (b"IDAT", 1..=1024, _) => image_data = true,
            (b"IEND", 2..=1025, 0) if image_data && remainder.is_empty() => break,
            (kind, 1..=5, _) if role == PngRole::BinaryMatte && !image_data
                && binary_matte_metadata(kind, data) && metadata.insert(kind) => (),
            _ => return Err("unsupported PNG chunk layout or metadata".into()),
        }
        count += 1;
    }
    let mut options = png::DecodeOptions::default();
    options.set_skip_ancillary_crc_failures(false);
    let mut decoder = png::Decoder::new_with_options(Cursor::new(bytes), options);
    decoder.set_limits(png::Limits { bytes: 2 * MAX_DECODED });
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("invalid PNG header: {e}"))?;
    let info = reader.info();
    if (info.width, info.height) != (1920, 1080)
        || info.bit_depth != png::BitDepth::Eight
        || info.interlaced
        || !match role {
            PngRole::Opaque => {
                matches!(info.color_type, png::ColorType::Rgb | png::ColorType::Rgba)
            }
            PngRole::Matte | PngRole::BinaryMatte => info.color_type == png::ColorType::Rgba,
        }
    {
        return Err("unsupported PNG raster, bit depth, or alpha layout for this import".into());
    }
    let size = reader
        .output_buffer_size()
        .filter(|n| *n <= MAX_DECODED)
        .ok_or("PNG decoded size exceeds the limit")?;
    let mut pixels = vec![0; size];
    let frame = reader.next_frame(&mut pixels).map_err(|e| format!("PNG decode failed: {e}"))?;
    // Optional alpha fallback requires a complete, intact image.
    reader.finish().map_err(|e| format!("PNG did not finish decoding: {e}"))?;
    if role == PngRole::BinaryMatte {
        return Ok(binary_alpha(&pixels[..frame.buffer_size()]));
    }
    Ok((None, [None; 2]))
}

fn binary_alpha(pixels: &[u8]) -> AlphaProfile {
    let mut bounds = [[1920, 1080, 0, 0]; 2];
    let mut counts = [0u32; 2];
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        let plane = match pixel[3] {
            0 => 0,
            255 => 1,
            _ => return (Some("The PNG has partially transparent pixels"), [None; 2]),
        };
        let (x, y) = (index as u32 % 1920, index as u32 / 1920);
        include_pixel(&mut bounds[plane], x, y);
        counts[plane] += 1;
    }
    if counts.contains(&0) {
        return (Some("The PNG has uniform alpha"), [None; 2]);
    }
    (None, [0, 1].map(|i| {
        let outer = bounds[i];
        let missing = rectangle_area(outer) - counts[i];
        if missing == 0 {
            return Some(BinaryAlphaShape { outer, hole: None });
        }
        let [left, top, right, bottom] = outer;
        let selected = if i == 0 { 0 } else { 255 };
        let mut hole = [right, bottom, left, top];
        for y in top..bottom {
            let start = ((y * 1920 + left) * 4) as usize;
            let end = ((y * 1920 + right) * 4) as usize;
            for (column, pixel) in pixels[start..end].chunks_exact(4).enumerate() {
                if pixel[3] != selected {
                    include_pixel(&mut hole, left + column as u32, y);
                }
            }
        }
        // The complementary samples must fill one rectangle strictly inside
        // the outer bounds. Touching the edge would be a notch or a split.
        (hole[0] > left && hole[1] > top && hole[2] < right && hole[3] < bottom
            && rectangle_area(hole) == missing)
            .then_some(BinaryAlphaShape { outer, hole: Some(hole) })
    }))
}

fn include_pixel(bounds: &mut [u32; 4], x: u32, y: u32) {
    let [left, top, right, bottom] = bounds;
    *left = (*left).min(x);
    *top = (*top).min(y);
    *right = (*right).max(x + 1);
    *bottom = (*bottom).max(y + 1);
}

fn rectangle_area([left, top, right, bottom]: [u32; 4]) -> u32 {
    (right - left) * (bottom - top)
}

pub fn verify_imported_jpeg(
    locators: &[String], mappings: &[RootMapping], ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Option<SourceInput>, String> {
    let Some((path, _)) = resolve_with_claim(locators, mappings)? else { return Ok(None) };
    if super::additional_media::image_kind(&path) != Some("jpeg")
        || !std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE as u64)
    {
        return Err("imported JPEG is not a regular JPEG within the size limit".into());
    }
    let revision = || format!("could not verify imported JPEG {}", path.display());
    let before = crate::revision::file_revision_checked(&path, check).map_err(|e| format!("{}: {e}", revision()))?;
    if super::additional_media::probe_still(&path, ffprobe)? != Some((1920, 1080)) {
        return Err("imported JPEG requires a single 1920×1080 frame".into());
    }
    let after = crate::revision::file_revision_checked(&path, check).map_err(|e| format!("{}: {e}", revision()))?;
    if before != after {
        return Err("imported JPEG changed during verification".into());
    }
    Ok(Some(SourceInput { path, revision: before }))
}

/// Verify the actual single coded frame behind an imported still's logical hold.
pub fn verify_imported_jpeg_mxf(
    path: &Path,
    tool: &Path,
    package: &str,
    count: u64,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<SourceInput, String> {
    use avid_reader::mxf::{read_identity, PictureProfile};

    let info = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !info.is_file() || info.len() > MAX_FILE as u64 {
        return Err("expected a bounded regular MXF".into());
    }
    let revision = crate::revision::file_revision_checked(path, check).map_err(|e| e.to_string())?;
    let identity = read_identity(path).map_err(|e| e.to_string())?;
    let clock = identity.qualified_file_clock().ok_or("the MXF has no qualified file clock")?;
    if identity.file_packages.as_slice() != [package]
        || identity.file_essence_tracks != 1
        || identity.picture_essence_coding.and_then(PictureProfile::from_label) != Some(PictureProfile::Dnx(1237))
        || (clock.rate, clock.rounded_base, clock.drop, clock.effective_count)
            != ((24_000, 1001), 24, false, count)
    {
        return Err("MXF package, picture coding or clock differs from the saved still".into());
    }
    check()?;
    let probe = super::probe_metadata(path, tool)?;
    let Some(graph::Value::Arr(streams)) = probe.get("streams") else {
        return Err("MXF probe has no stream list".into());
    };
    let [picture] = streams.as_slice() else {
        return Err("expected only one MXF picture stream".into());
    };
    let tags = probe.get("format").and_then(|format| format.get("tags"));
    let profile = [
        ("codec_name", "dnxhd"), ("profile", "DNXHD"), ("sample_aspect_ratio", "1:1"),
        ("field_order", "progressive"), ("pix_fmt", "yuv422p"), ("color_range", "tv"),
        ("color_space", "bt709"), ("color_transfer", "bt709"), ("color_primaries", "bt709"),
        ("r_frame_rate", "24000/1001"), ("time_base", "1001/24000"),
    ];
    if tags.and_then(|v| super::text(v, "operational_pattern_ul")).as_deref()
            != Some("060e2b34.04010102.0d010201.10000000")
        || !profile.iter().all(|(key, value)| super::text(picture, key).as_deref() == Some(*value))
        || [("width", 1920), ("height", 1080), ("index", 0), ("start_pts", 0), ("duration_ts", 1)]
            .iter().any(|(key, value)| super::number(picture, key) != Some(*value))
        || picture.get("tags").and_then(|v| super::text(v, "file_package_umid"))
            .as_deref().and_then(|v| v.strip_prefix("0x")) != Some(package)
    {
        return Err("expected one Rec.709 DNxHD picture in the saved OP-Atom package".into());
    }
    check()?;
    if !super::complete_picture_packets(path, tool, 1) {
        return Err("MXF does not carry one complete coded picture".into());
    }
    if crate::revision::file_revision_checked(path, check).map_err(|e| e.to_string())? != revision {
        return Err("MXF changed during still verification".into());
    }
    Ok(SourceInput { path: path.to_owned(), revision })
}

fn binary_matte_metadata(kind: &[u8], data: &[u8]) -> bool {
    match kind {
        b"eXIf" => verify_exif_dimensions(data, [1920, 1080]).is_some(),
        b"sRGB" => matches!(data, [0..=3]),
        b"gAMA" => data == 45_455u32.to_be_bytes(),
        b"cHRM" => data == [31_270u32, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000]
            .map(u32::to_be_bytes).as_flattened(),
        // The retained import accepts this unspecified-density record at
        // the saved full canvas. Also allow an explicitly square density.
        b"pHYs" => data == [0, 0, 0, 0, 0, 0, 0, 1, 0]
            || (data.len() == 9 && data[8] <= 1 && data[..4] == data[4..8]
                && data[..4] != [0; 4]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(color: png::ColorType, width: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, 1080);
        encoder.set_color(color);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0; width as usize * 1080 * color.samples()])
            .unwrap();
        writer.finish().unwrap();
        bytes
    }

    #[test]
    fn linked_png_exif_accepts_dimensions_and_refuses_orientation_or_other_tags() {
        let mut exif = b"II\x2a\0\x08\0\0\0\x03\0".to_vec();
        for (tag, kind, value) in [(0x0100u16, 4u16, 32u32), (0x0101, 4, 16), (0x0112, 3, 1)] {
            exif.extend(tag.to_le_bytes());
            exif.extend(kind.to_le_bytes());
            exif.extend(1u32.to_le_bytes());
            exif.extend(value.to_le_bytes());
        }
        exif.extend([0; 4]);
        assert!(verify_exif_dimensions(&exif, [32, 16]).is_some());
        assert!(verify_exif_dimensions(&exif, [16, 32]).is_none());
        assert!(verify_exif_dimensions(&exif[..exif.len() - 1], [32, 16]).is_none());

        exif[42] = 6;
        assert!(verify_exif_dimensions(&exif, [32, 16]).is_none());
        exif[42] = 1;
        exif[34..36].copy_from_slice(&0x014au16.to_le_bytes());
        assert!(verify_exif_dimensions(&exif, [32, 16]).is_none());
    }

    #[test]
    fn linked_png_exif_dimension_subdirectory_cannot_point_to_another_image() {
        let exif = |next| {
            let mut metadata = b"II\x2a\0\x08\0\0\0\x01\0".to_vec();
            metadata.extend(0x8769u16.to_le_bytes());
            metadata.extend(13u16.to_le_bytes());
            metadata.extend(1u32.to_le_bytes());
            metadata.extend(26u32.to_le_bytes());
            metadata.extend([0; 4]);
            metadata.extend(1u16.to_le_bytes());
            metadata.extend(0xa002u16.to_le_bytes());
            metadata.extend(3u16.to_le_bytes());
            metadata.extend(1u32.to_le_bytes());
            metadata.extend(32u32.to_le_bytes());
            metadata.extend(u32::to_le_bytes(next));
            metadata
        };
        assert!(verify_exif_dimensions(&exif(44), [32, 16]).is_some());
        assert!(verify_exif_dimensions(&exif(26), [32, 16]).is_none());
        assert!(verify_exif_dimensions(&exif(45), [32, 16]).is_none());
    }

    #[test]
    fn physical_still_requires_expected_pixels_and_alpha_storage() {
        let path = Path::new("synthetic.png");
        let rgb = encode(png::ColorType::Rgb, 1920);
        let rgba = encode(png::ColorType::Rgba, 1920);
        verify_png(path, &rgb, PngRole::Opaque).unwrap();
        verify_png(path, &rgba, PngRole::Opaque).unwrap();
        verify_png(path, &rgba, PngRole::Matte).unwrap();
        assert!(verify_png(path, &rgb, PngRole::Matte)
            .unwrap_err()
            .contains("alpha layout"));
        assert!(verify_png(path, &encode(png::ColorType::Rgba, 960), PngRole::Matte).is_err());
        assert!(verify_png(path, &rgba[..rgba.len() - 3], PngRole::Matte).is_err());
        let mut bad_crc = rgba;
        let offset = bad_crc.len() - 1;
        bad_crc[offset] ^= 1;
        assert!(verify_png(path, &bad_crc, PngRole::Matte).is_err());
    }

    #[test]
    fn matte_verification_distinguishes_absence_from_an_invalid_original() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("synthetic.png");
        let locators = [path.to_str().unwrap().to_owned()];
        let verify = || verify_imported_png(&locators, &[], PngRole::BinaryMatte);
        assert!(matches!(verify(), Err(PngVerificationError::MissingOriginal)));

        std::fs::create_dir(&path).unwrap();
        assert!(matches!(verify(), Err(PngVerificationError::Invalid(_))));
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, b"invalid PNG").unwrap();
        assert!(matches!(verify(), Err(PngVerificationError::Invalid(_))));
    }

    #[test]
    fn direct_matte_requires_binary_alpha_and_verified_metadata() {
        let render = |middle_alpha| {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 1920, 1080);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            // The PNG sRGB chunk uses 1 for relative colorimetric intent.
            writer.write_chunk(png::chunk::sRGB, &[1]).unwrap();
            let mut pixels = vec![0; MAX_DECODED];
            pixels[3] = 255;
            pixels[7] = middle_alpha;
            writer.write_image_data(&pixels).unwrap();
            writer.finish().unwrap();
            bytes
        };
        let path = Path::new("synthetic.png");
        let mut binary = render(0);
        assert_eq!(verify_png(path, &binary, PngRole::BinaryMatte).unwrap(),
            (None, [None, Some(BinaryAlphaShape { outer: [0, 0, 1, 1], hole: None })]));
        let soft = render(128);
        assert_eq!(verify_png(path, &soft, PngRole::BinaryMatte).unwrap(),
            (Some("The PNG has partially transparent pixels"), [None; 2]));
        assert!(verify_png(path, &soft, PngRole::Matte).is_err(), "sRGB remains outside the legacy profile");
        assert_eq!(verify_png(path, &encode(png::ColorType::Rgba, 1920), PngRole::BinaryMatte).unwrap(),
            (Some("The PNG has uniform alpha"), [None; 2]));

        let mut damaged_soft = soft;
        let last = damaged_soft.len() - 1;
        damaged_soft[last] ^= 1;
        assert!(verify_png(path, &damaged_soft, PngRole::BinaryMatte).is_err());

        let srgb = binary.windows(4).position(|kind| kind == b"sRGB").unwrap();
        binary[srgb + 5] ^= 1;
        assert!(verify_png(path, &binary, PngRole::BinaryMatte).is_err());
    }

    #[test]
    fn split_matte_qualifies_one_enclosed_rectangular_hole() {
        let mut pixels = vec![0; MAX_DECODED];
        let row = 1920 * 4;
        for alpha in pixels[900 * row + 3..].iter_mut().step_by(4) {
            *alpha = 255;
        }
        let upper = BinaryAlphaShape { outer: [0, 0, 1920, 900], hole: None };
        let lower = BinaryAlphaShape { outer: [0, 900, 1920, 1080], hole: None };
        assert_eq!(binary_alpha(&pixels),
            (None, [Some(upper), Some(lower)]));

        let set_alpha = |pixels: &mut [u8], (x, y): (usize, usize), value| {
            pixels[y * row + x * 4 + 3] = value;
        };
        for y in 930..1045 {
            for x in 1440..1810 {
                set_alpha(&mut pixels, (x, y), 0);
            }
        }
        let cutout = BinaryAlphaShape { hole: Some([1440, 930, 1810, 1045]), ..lower };
        assert_eq!(binary_alpha(&pixels), (None, [None, Some(cutout)]));

        for alpha in pixels.iter_mut().skip(3).step_by(4) {
            *alpha = 255 - *alpha;
        }
        assert_eq!(binary_alpha(&pixels), (None, [Some(cutout), None]));

        set_alpha(&mut pixels, (1440, 930), 0); // A filled corner makes the hole nonrectangular.
        assert_eq!(binary_alpha(&pixels), (None, [None, None]));
        set_alpha(&mut pixels, (1440, 930), 255);
        set_alpha(&mut pixels, (20, 940), 255); // A second hole.
        assert_eq!(binary_alpha(&pixels), (None, [None, None]));
        set_alpha(&mut pixels, (20, 940), 0);
        set_alpha(&mut pixels, (20, 900), 255); // An opening on the outside edge.
        assert_eq!(binary_alpha(&pixels), (None, [None, None]));
    }
}
