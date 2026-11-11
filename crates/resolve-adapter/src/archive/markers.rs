//! Same-build, sequence-owned range markers for the straight-cut writer.
use super::*;
use crate::prepare::PreparedMarker;

pub(super) fn write(project: &mut String, owner: &str, markers: &[PreparedMarker]) -> Result<()> {
    if markers.is_empty() {
        return Ok(());
    }
    let old = block(project, "LocableBlobSet")?;
    if old.contains(&format!("<BlobOwner>{owner}</BlobOwner>")) {
        return Err("duplicate sequence marker owner".into());
    }
    let mut entries = Vec::new();
    for marker in markers {
        let mut position = Vec::new();
        if marker.frame != 0 {
            pb_int(1, marker.frame * 2, &mut position);
        }
        let mut fields = Vec::new();
        pb_int(1, marker.color_bit, &mut fields);
        pb_bytes(3, marker.note.as_bytes(), &mut fields);
        pb_bytes(3, marker.duration.to_string().as_bytes(), &mut fields);
        pb_bytes(3, marker.name.as_bytes(), &mut fields);
        let mut body = Vec::new();
        pb_bytes(1, &fields, &mut body);
        let mut value = 2u32.to_be_bytes().to_vec();
        value.extend_from_slice(&u32::try_from(body.len())?.to_be_bytes());
        value.extend(body);
        let mut entry = Vec::new();
        pb_bytes(1, &position, &mut entry);
        pb_bytes(2, &value, &mut entry);
        pb_bytes(2, &entry, &mut entries);
    }
    let mut body = Vec::new();
    pb_bytes(2, &entries, &mut body);
    let payload = zstd::bulk::compress(&body, 3)?;
    let mut envelope = 10001u32.to_be_bytes().to_vec();
    envelope.extend_from_slice(&u32::try_from(payload.len() + 1)?.to_be_bytes());
    envelope.push(0x81);
    envelope.extend(payload);
    let fields = hex::encode(qt(&[("BlobData", Bytes(&envelope))]));
    let marker = format!(
        "<Element><Sm2SequenceLockableBlob DbId=\"{}\"><FieldsBlob>{fields}</FieldsBlob>         <BlobOwner>{owner}</BlobOwner><DbSavedTime>0</DbSavedTime></Sm2SequenceLockableBlob></Element>",
        uid()
    );
    let mut updated = old.clone();
    let end = updated
        .rfind("</LocableBlobSet>")
        .ok_or("missing Resolve marker set end")?;
    updated.insert_str(end, &marker);
    replace_once(project, &old, &updated)
}
