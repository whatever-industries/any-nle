//! Bounded Text+ source observation. Fusion expressions are never evaluated.
use super::{encoding, observations, text, Grid, Node, Result, SourceGraph, SourceSequence, Value};
use flate2::bufread::ZlibDecoder;
use graph::source::{SourceText, TextKind};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::io::Read;

const MAX_COMPOSITION: usize = 256 << 10;
pub(super) const ADAPTER: &str = "resolve.text_plus";
pub(super) const APPEARANCE_LOSS: &str = "Resolve Text+ composition requires a destination mapping";

fn inflate(encoded: &[u8], declared: usize) -> Option<Vec<u8>> {
    if declared > MAX_COMPOSITION {
        return None;
    }
    let mut output = Vec::new();
    ZlibDecoder::new(encoded)
        .take((declared + 1) as u64)
        .read_to_end(&mut output)
        .ok()?;
    (output.len() == declared).then_some(output)
}

fn inputs(payload: &[u8], blobs: &mut encoding::Blobs) -> Result<Option<Vec<(String, Value)>>> {
    let Some((header, data)) = payload.split_first_chunk::<4>() else {
        return Ok(None);
    };
    let Some(outer) = inflate(data, u32::from_be_bytes(*header) as usize) else {
        return Ok(None);
    };
    blobs.charge(outer.len())?;
    const MARKER: &[u8] = b"Compressed = true, }\0";
    let mut markers = outer
        .windows(MARKER.len())
        .enumerate()
        .filter(|(_, bytes)| *bytes == MARKER);
    let Some((offset, _)) = markers.next() else {
        return Ok(None);
    };
    if markers.next().is_some() {
        return Ok(None);
    }
    let Some((header, data)) = outer[offset + MARKER.len()..].split_first_chunk::<4>() else {
        return Ok(None);
    };
    let Some(inner) = inflate(data, u32::from_le_bytes(*header) as usize) else {
        return Ok(None);
    };
    blobs.charge(inner.len())?;
    let Ok(inner) = std::str::from_utf8(&inner) else {
        return Ok(None);
    };
    if inner.matches("= TextPlus {").count() != 1 {
        return Ok(None);
    }
    let mut result = Vec::new();
    for (saved, key) in [("StyledText", "text"), ("Font", "font"), ("Style", "style")] {
        let re = Regex::new(&format!(
            r#"\b{saved}\s*=\s*Input\s*\{{\s*Value\s*=\s*"((?:[^"\\]|\\.)*)""#
        ))?;
        let mut matches = re.captures_iter(inner);
        if let Some(value) = matches.next().filter(|_| matches.next().is_none()) {
            if let Some(value) = fusion_string(&value[1]) {
                if value.len() <= 32_768 {
                    result.push((key.into(), Value::Str(value)));
                }
            }
        }
    }
    for (saved, key) in [
        ("Size", "size"),
        ("VerticalJustificationNew", "vertical_justification"),
        ("HorizontalJustificationNew", "horizontal_justification"),
        ("Red1", "red"),
        ("Green1", "green"),
        ("Blue1", "blue"),
    ] {
        let re = Regex::new(&format!(
            r"\b{saved}\s*=\s*Input\s*\{{\s*Value\s*=\s*([^,}}]+)"
        ))?;
        let mut matches = re.captures_iter(inner);
        if let Some(value) = matches.next().filter(|_| matches.next().is_none()) {
            if let Some(number) = value[1]
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
            {
                result.push((key.into(), Value::Float(number)));
            }
        }
    }
    Ok(Some(result))
}

fn fusion_string(input: &str) -> Option<String> {
    let mut chars = input.chars();
    let mut text = String::new();
    while let Some(c) = chars.next() {
        text.push(match c {
            '\\' => match chars.next()? {
                'n' => '\n',
                'r' => '\n',
                't' => '\t',
                c @ ('\\' | '"') => c,
                _ => return None,
            },
            c if !c.is_control() => c,
            _ => return None,
        });
    }
    Some(text)
}

pub(super) fn capture(
    graph: &mut SourceGraph,
    sequence: &mut SourceSequence,
    node: &Node,
    grid: Grid,
    track: i64,
    blobs: &mut encoding::Blobs,
) -> Result<()> {
    let table = node.one("CompositionTable")?.entry()?;
    let saved = table.get("CompositionBA")?;
    let bytes = encoding::hex_bytes(saved)?;
    blobs.charge(bytes.len())?;
    let mut fields =
        if bytes.len() <= MAX_COMPOSITION && table.maybe("ActiveCompositionIdx")? == Some("0") {
            inputs(&bytes, blobs)?.unwrap_or_default()
        } else {
            vec![]
        };
    let content = fields.iter().find_map(|(key, value)| match value {
        Value::Str(value) if key == "text" => Some(value.clone()),
        _ => None,
    });
    fields.push((
        "composition_sha256".into(),
        text(&hex::encode(Sha256::digest(&bytes))),
    ));
    let edit = grid.edit(node, false)?;
    sequence.texts.push(SourceText {
        id: node.id()?.into(),
        kind: TextKind::Graphic,
        track,
        reason: content
            .is_none()
            .then(|| "Resolve Text+ content is unavailable".into()),
        content,
        record_start: Some(super::seconds(edit.start, grid.rate)?),
        duration: Some(super::seconds(edit.length, grid.rate)?),
    });
    observations::append(
        graph,
        ADAPTER,
        sequence.id.as_str(),
        Value::Obj({
            fields.push(("id".into(), text(node.id()?)));
            fields
        }),
    );
    super::loss(
        graph,
        node.id()?,
        "text_appearance",
        APPEARANCE_LOSS,
    );
    Ok(())
}
