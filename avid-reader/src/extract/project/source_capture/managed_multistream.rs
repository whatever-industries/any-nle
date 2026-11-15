//! Saved AVB witnesses for managed OP1a files from AVB-0023.
use super::*;

const MAX_ESSENCE_TRACKS: usize = 9;
const CONTAINER_HANDLER: [u8; 16] = [
    0x93, 0x06, 0x87, 0xb5, 0xdb, 0xc8, 0xe3, 0x41, 0xaa, 0xeb, 0x7c, 0x3d, 0xcb, 0x37, 0x54, 0x0c,
];

pub(super) fn capture(
    ctx: &Ctx<'_>,
    mob: &Node,
    master: &MasterClip,
    ambiguous: &HashSet<MobId>,
    budget: &SharedBudget,
) -> Option<ManagedMultistream> {
    let package = master.tracks.first()?.file_mob;
    if ambiguous.contains(&package)
        || ambiguous.contains(&master.mob_id)
        || !(2..=MAX_ESSENCE_TRACKS).contains(&master.tracks.len())
        || master.tracks.iter().any(|track| track.file_mob != package)
    {
        return None;
    }
    let file = ctx.mobs.get(&package)?;
    let descriptor = ctx.deref(file.get("descriptor"))?;
    if descriptor.class != *b"MULD"
        || !managed_locator(ctx, descriptor, package)
        || !matches!(
            unique_attribute(borrowed_attrs(ctx, descriptor), "_CONTAINER_HANDLER_GUID"),
            Some(Value::Bytes(bytes)) if bytes.as_slice() == CONTAINER_HANDLER
        )
    {
        return None;
    }
    let Value::RefList(children) = descriptor.get("descriptors")? else {
        return None;
    };
    if children.len() != master.tracks.len() {
        return None;
    }
    let mut descriptors = Vec::new();
    if !reserve(budget, &mut descriptors, children.len()) {
        return None;
    }
    for (index, reference) in children.iter().enumerate() {
        if reference.as_index().is_none() || children[..index].contains(reference) {
            return None;
        }
        let child = ctx.deref(Some(reference))?;
        if !matches!(&child.class, b"CDCI" | b"PCMA") || !managed_locator(ctx, child, package) {
            return None;
        }
        descriptors.push(ManagedMultistreamDescriptor {
            class: child.class,
            length: int(child, "length"),
            edit_rate: float(child, "edit_rate").and_then(Rate::from_avid),
            sample_rate: float(child, "sample_rate").and_then(Rate::from_avid),
            bits: int(child, "quantization_bits").or_else(|| int(child, "component_width")),
            channels: int(child, "channels"),
            width: int(child, "stored_width"),
            height: int(child, "stored_height"),
            frame_layout: int(child, "frame_layout"),
            display_aspect: match child.get("aspect_ratio") {
                Some(Value::List(parts)) => match parts.as_slice() {
                    [Value::Int(n), Value::Int(d)] if *n > 0 && *d > 0 => Some((*n, *d)),
                    _ => None,
                },
                _ => None,
            },
            resolution_id: int(child, "resolution_id"),
            compression: match child.get("compress_method") {
                Some(Value::Bytes(bytes)) => bytes.as_slice().try_into().ok(),
                _ => None,
            },
        });
    }
    let master_tracks = ctx.tracks(mob);
    let file_tracks = ctx.tracks(file);
    let mut tracks = Vec::new();
    if !reserve(budget, &mut tracks, master.tracks.len()) {
        return None;
    }
    for track in &master.tracks {
        let (_, source_clip) = track_source(ctx, &master_tracks, track.kind, track.index)?;
        if mob_id(source_clip) != Some(package) {
            return None;
        }
        let file_label = int(source_clip, "track_id").filter(|&label| label > 0)?;
        let (component, file_source) = track_source(ctx, &file_tracks, track.kind, file_label)?;
        let physical_track_number = physical_track_number(borrowed_attrs(ctx, component));
        tracks.push(ManagedMultistreamTrack {
            kind: track.kind,
            master_label: track.index,
            file_label,
            physical_track_number,
            master_offset: int(source_clip, "start_time"),
            file_source_package: mob_id(file_source),
            file_source_track: int(file_source, "track_id"),
            file_source_offset: int(file_source, "start_time"),
            file_source_length: int(file_source, "length"),
            file_source_rate: float(file_source, "edit_rate").and_then(Rate::from_avid),
        });
    }
    Some(ManagedMultistream {
        file_package: package,
        tracks,
        descriptors,
    })
}

fn physical_track_number(attributes: &[(String, Value)]) -> Option<u32> {
    unique_attribute(attributes, "_SAVED_AAF_PHYSICAL_TRACK_NUMBER")
        .and_then(Value::as_int)
        .and_then(|number| u32::try_from(number).ok())
        .filter(|&number| number != 0)
}

fn managed_locator(ctx: &Ctx<'_>, descriptor: &Node, package: MobId) -> bool {
    matches!(descriptor.get("physical_media"), None | Some(Value::Null))
        && ctx.deref(descriptor.get("locator")).is_some_and(|locator| {
            locator.class == *b"MSML"
                && int(locator, "domain_type") == Some(5)
                && mob_id(locator) == Some(package)
        })
}

fn track_source<'a>(
    ctx: &Ctx<'a>,
    tracks: &[&'a Node],
    kind: i64,
    label: i64,
) -> Option<(&'a Node, &'a Node)> {
    let mut matching = tracks.iter().filter_map(|track| {
        let component = ctx.deref(track.get("component"))?;
        (int(track, "index") == Some(label) && int(component, "media_kind_id") == Some(kind))
            .then_some((*track, component))
    });
    let (track, component) = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    Some((component, ctx.only_source_clip(track).ok()?))
}

#[cfg(test)]
mod tests {
    use super::physical_track_number;
    use crate::objects::Value;

    #[test]
    fn saved_physical_track_number_requires_one_nonzero_u32() {
        let key = "_SAVED_AAF_PHYSICAL_TRACK_NUMBER";
        let track = (key.to_owned(), Value::Int(0x1602_0100));
        assert_eq!(
            physical_track_number(std::slice::from_ref(&track)),
            Some(0x1602_0100)
        );
        assert_eq!(physical_track_number(&[track.clone(), track]), None);
        assert_eq!(physical_track_number(&[]), None);
        for invalid in [
            Value::Int(0),
            Value::Int(-1),
            Value::Int(1_i64 << 32),
            Value::Str("369230080".into()),
        ] {
            assert_eq!(physical_track_number(&[(key.into(), invalid)]), None);
        }
    }
}
