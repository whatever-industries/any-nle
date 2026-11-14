//! Physical admission for the observed full-raster, untagged imported-still
//! profile. This does not reconstruct missing historical file identity.
use super::{resolve, RootMapping};
use avid_reader::source::SourceInput;
use std::{
    io::{Cursor, Read},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngRole {
    /// Ignores saved alpha, including on the retained RGBA comparison still.
    Opaque,
    /// The saved import uses an actual alpha channel.
    Matte,
}

const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_DECODED: usize = 1920 * 1080 * 4;

/// Resolve only a saved locator or explicit root map. Bind the revision to the
/// very bytes that were decoded so the caller can recheck it before publication.
pub fn verify_imported_png(
    locators: &[String],
    mappings: &[RootMapping],
    role: PngRole,
) -> Result<SourceInput, String> {
    let path = resolve(locators, mappings)?;
    let file = std::fs::File::open(&path).map_err(|e| format!("open imported still: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("stat imported still: {e}"))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE as u64 {
        return Err("imported PNG is not a regular file within the size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read imported still: {e}"))?;
    if bytes.len() as u64 != metadata.len() {
        return Err("imported PNG size changed while reading".into());
    }
    verify_png(&path, &bytes, role)?;
    let revision = crate::revision::sha256_reader(bytes.as_slice())
        .map_err(|e| format!("hash imported still: {e}"))?;
    Ok(SourceInput {
        path,
        revision: hex::encode(revision),
    })
}

fn verify_png(path: &Path, bytes: &[u8], role: PngRole) -> Result<(), String> {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("png"))
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("imported still is not a PNG".into());
    }
    // Only IHDR/IDAT/IEND are observed. In particular, reject APNG, tRNS,
    // sampling/aspect metadata and color or orientation instructions.
    let mut remainder = &bytes[8..];
    let mut count = 0;
    loop {
        let Some(header) = remainder.get(..8) else {
            return Err("truncated PNG chunk".into());
        };
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let total = size.checked_add(12).ok_or("PNG chunk size overflow")?;
        remainder = remainder.get(total..).ok_or("truncated PNG chunk")?;
        match (&header[4..], count, size) {
            (b"IHDR", 0, 13) => (),
            (b"IDAT", 1..=1024, _) => (),
            (b"IEND", 2..=1025, 0) if remainder.is_empty() => break,
            _ => return Err("unsupported PNG chunk layout or metadata".into()),
        }
        count += 1;
    }
    let decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: 2 * MAX_DECODED,
        },
    );
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
            PngRole::Matte => info.color_type == png::ColorType::Rgba,
        }
    {
        return Err("unsupported PNG raster, bit depth, or alpha layout for this import".into());
    }
    let size = reader
        .output_buffer_size()
        .filter(|n| *n <= MAX_DECODED)
        .ok_or("PNG decoded size exceeds the limit")?;
    reader
        .next_frame(&mut vec![0; size])
        .map_err(|e| format!("PNG decode failed: {e}"))?;
    // A header-only probe would accept corrupt pixels and an invalid end chunk.
    reader
        .finish()
        .map_err(|e| format!("PNG did not finish decoding: {e}"))
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
}
