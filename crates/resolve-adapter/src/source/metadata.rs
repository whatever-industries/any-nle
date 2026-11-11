//! Saved Media Pool identifiers and cached reel observations for future conform work.
use super::{encoding, Blobs, Node, Result, Sidecar, SourceGraph, Value};
use sha2::{Digest, Sha256};

pub(super) fn capture(
    clip: &Node,
    cache: &Node,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
) -> Result<()> {
    let mut fields = Vec::new();
    if let Some(value) = clip
        .maybe("UniqueMediaPoolItemId")?
        .filter(|value| !value.is_empty())
    {
        fields.push(("UniqueMediaPoolItemId".into(), Value::Str(value.into())));
    }
    if let Some(encoded) = cache
        .maybe("VideoMetadata")?
        .filter(|value| !value.trim().is_empty())
    {
        let data = encoding::hex_bytes(encoded)?;
        blobs.charge(data.len())?;
        match encoding::qt(&data)
            .and_then(|map| Ok(map.text("ActualReelNumber")?.map(str::to_owned)))
        {
            Ok(Some(reel)) if !reel.is_empty() => {
                fields.push((
                    "BtVideoInfo.VideoMetadata.ActualReelNumber".into(),
                    Value::Str(reel),
                ));
            }
            Ok(_) => {}
            Err(_) => fields.push(("VideoMetadata.unresolved_sha256".into(), digest(&data))),
        }
    }
    if let Some(encoded) = cache
        .maybe("MediaMetadata")?
        .filter(|value| !value.trim().is_empty())
    {
        fields.push((
            "MediaMetadata.unresolved_text_sha256".into(),
            digest(encoded.as_bytes()),
        ));
    }
    if !fields.is_empty() {
        graph.sidecars.push(Sidecar {
            adapter: "resolve.media_metadata".into(),
            object: clip.id()?.into(),
            data: Value::Obj(fields),
        });
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> Value {
    Value::Str(hex::encode(Sha256::digest(bytes)))
}
