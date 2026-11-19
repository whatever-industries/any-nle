//! Same-build sequence, Media Pool and timeline edit marker encoding.
use super::*;
use crate::clip_label::ClipLabel;
use crate::prepare::PreparedMarker;

pub(super) fn write(project: &mut String, owner: &str, markers: &[PreparedMarker]) -> Result<()> {
    if markers.is_empty() {
        return Ok(());
    }
    append_markers(project, [(owner, markers)], "Sm2SequenceLockableBlob")
}

pub(super) fn write_pool<'a>(
    project: &mut String,
    rows: impl IntoIterator<Item = (&'a str, &'a [PreparedMarker])>,
) -> Result<()> {
    append_markers(project, rows, "Sm2MpItemLockableBlob")
}

fn append_markers<'a>(
    project: &mut String,
    rows: impl IntoIterator<Item = (&'a str, &'a [PreparedMarker])>,
    class: &str,
) -> Result<()> {
    use std::fmt::Write as _;
    let old = block(project, "LocableBlobSet")?;
    let mut appended = String::new();
    for (owner, markers) in rows {
        if old.contains(&format!("<BlobOwner>{owner}</BlobOwner>")) {
            return Err("duplicate Resolve marker owner".into());
        }
        let fields = marker_fields(markers, None)?;
        write!(appended,
            "<Element><{class} DbId=\"{}\"><FieldsBlob>{fields}</FieldsBlob><BlobOwner>{owner}</BlobOwner><DbSavedTime>0</DbSavedTime></{class}></Element>",
            uid()
        )?;
        if project.len().saturating_add(appended.len()) as u64 > MAX_XML {
            return Err("Resolve markers exceed the project XML budget".into());
        }
    }
    if appended.is_empty() {
        return Ok(());
    }
    let mut updated = old.clone();
    let end = updated
        .rfind("</LocableBlobSet>")
        .ok_or("missing Resolve marker set end")?;
    updated.insert_str(end, &appended);
    replace_once(project, &old, &updated)
}

fn marker_fields(markers: &[PreparedMarker], label: Option<ClipLabel>) -> Result<String> {
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
        if !marker.custom_data.is_empty() {
            pb_bytes(6, marker.custom_data.as_bytes(), &mut fields);
        }
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
    if let Some(label) = label {
        pb_int(5, u64::from(label.code()), &mut body);
    }
    let payload = zstd::bulk::compress(&body, 3)?;
    let mut envelope = 10001u32.to_be_bytes().to_vec();
    envelope.extend_from_slice(&u32::try_from(payload.len() + 1)?.to_be_bytes());
    envelope.push(0x81);
    envelope.extend(payload);
    Ok(hex::encode(qt(&[("BlobData", Bytes(&envelope))])))
}

pub(super) struct EditMetadata<'a> {
    pub(super) owner: String,
    pub(super) label: Option<ClipLabel>,
    pub(super) markers: &'a [PreparedMarker],
}

pub(super) fn write_edits(project: &mut String, edits: &[EditMetadata<'_>]) -> Result<()> {
    if edits.is_empty() {
        return Ok(());
    }
    use std::fmt::Write as _;
    let old = block(project, "LocableBlobSet")?;
    let mut entries = String::new();
    for edit in edits {
        let fields = if edit.markers.is_empty() {
            let label = edit.label.ok_or("missing Resolve edit label")?;
            hex::encode(qt(&[("BlobData", Bytes(&label.blob()))]))
        } else {
            marker_fields(edit.markers, edit.label)?
        };
        write!(entries, concat!(
            "<Element><Sm2TiItemLockableBlob DbId=\"{}\"><FieldsBlob>{}</FieldsBlob>",
            "<BlobOwner>{}</BlobOwner><DbSavedTime>0</DbSavedTime></Sm2TiItemLockableBlob></Element>"
        ), uid(), fields, edit.owner)?;
        if project.len().saturating_add(entries.len()) as u64 > MAX_XML {
            return Err("Resolve clip labels exceed the project XML budget".into());
        }
    }
    let mut updated = old.clone();
    let end = updated
        .rfind("</LocableBlobSet>")
        .ok_or("missing Resolve lockable blob set end")?;
    updated.insert_str(end, &entries);
    replace_once(project, &old, &updated)
}
