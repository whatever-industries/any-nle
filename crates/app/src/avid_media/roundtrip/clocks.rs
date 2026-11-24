use super::*;

/// Follow the same plain master -> file -> import relationship used by capture.
/// A declaration of an import MobID in JSON cannot by itself select a track.
fn verify_clock_owner(donor: &Donor<'_>, master: u32, import: MobId, offset: i64) -> Result<()> {
    let mut package_ids = HashSet::new();
    for track in tracks(donor.node(master)?)? {
        let component = reference(track, "component")?;
        if !matches!(integer(donor.node(component)?, "media_kind_id")?, 1 | 2) {
            continue;
        }
        let leaf = plain_clip(donor, component)?;
        let Some(Value::MobId(package_id)) = leaf.get("mob_id") else {
            return Err("master track has no saved file MobID".into());
        };
        let package = donor.mob(&package_id.urn())?;
        let upstream = only(tracks(donor.node(package)?)?, "file-to-import track")?;
        let upstream = plain_clip(donor, reference(upstream, "component")?)?;
        if upstream.get("mob_id") != Some(&Value::MobId(import))
            || integer(upstream, "start_time")? != offset
        {
            return Err("donor master-to-import clock route or offset disagrees".into());
        }
        package_ids.insert(*package_id);
    }
    if package_ids.is_empty() {
        return Err("donor has no verified file-to-import tracks".into());
    }
    // Masters sharing a file package or its import MobID would also see the change.
    for candidates in donor.mobs.values() {
        for id in candidates {
            if *id == master {
                continue;
            }
            let node = donor.node(*id)?;
            if integer(node, "mob_type_id")? != 2 {
                continue;
            }
            for track in tracks(node)? {
                let component = reference(track, "component")?;
                let Ok(leaf) = plain_clip(donor, component) else {
                    continue;
                };
                let Some(Value::MobId(package)) = leaf.get("mob_id") else {
                    continue;
                };
                if package_ids.contains(package)
                    || master_package_uses_import(donor, *package, import)?
                {
                    return Err("auxiliary clock import is shared by another master".into());
                }
            }
        }
    }
    Ok(())
}

fn master_package_uses_import(donor: &Donor<'_>, package: MobId, import: MobId) -> Result<bool> {
    if package == import {
        return Ok(true);
    }
    for id in donor.mobs.get(&package).into_iter().flatten() {
        for track in tracks(donor.node(*id)?)? {
            let component = reference(track, "component")?;
            if let Ok(leaf) = plain_clip(donor, component) {
                if leaf.get("mob_id") == Some(&Value::MobId(import)) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub(super) fn prepare(
    graph: &SourceGraph,
    donor: &Donor<'_>,
    owner: &str,
    saved: &Json,
    changes: &mut Changes,
) -> Result<()> {
    let item = only(
        graph.items.iter().filter(|item| item.id == owner),
        "graph clock owner",
    )?;
    let master = donor.mob(
        item.umid
            .as_deref()
            .ok_or("clock owner has no full Avid MobID")?,
    )?;
    let Json::Arr(segments) = saved else {
        return Err("invalid saved clock list".into());
    };
    if segments.is_empty() || segments.len() > 32 {
        return Err("expected one to 32 saved clock segments".into());
    }
    let first = &segments[0];
    let import: MobId = text(first, "mob")?.parse()?;
    let offset = number(first, "source_offset")?;
    verify_clock_owner(donor, master, import, offset)?;
    let native = donor.mob(&import.urn())?;
    for row in segments {
        let kind = number(row, "kind")?;
        let label = number(row, "track")?;
        let position = number(row, "position")?;
        let length = number(row, "length")?;
        if text(row, "mob")?.parse::<MobId>()? != import
            || number(row, "source_offset")? != offset
            || position < 0
            || length <= 0
            || offset < position
            || position.checked_add(length).is_none_or(|end| offset >= end)
        {
            return Err("unresolved auxiliary clock origin or span".into());
        }
        let (class, fields) = match (text(row, "class")?, kind, label) {
            ("TCCP", 3, 2..) => (
                *b"TCCP",
                &[("start", "count"), ("fps", "base"), ("flags", "flags")][..],
            ),
            ("ECCP", 4, 1..) => (
                *b"ECCP",
                &[
                    ("start_ec", "count"),
                    ("film_kind", "film_kind"),
                    ("code_format", "code_format"),
                    ("base_perf", "base_perf"),
                ][..],
            ),
            _ => return Err("expected an auxiliary timecode or film edgecode track".into()),
        };
        let track = donor.track(native, kind, label)?;
        donor.unshared(track)?;
        let id = donor.segment(track, position, length, class)?;
        if rate(donor.node(id)?)?.to_string() != text(row, "rate")? {
            return Err("donor auxiliary clock rate disagrees".into());
        }
        for (field, key) in fields {
            changes.set(donor, id, field, Value::Int(number(row, key)?))?;
        }
        if class == *b"ECCP" {
            let header = list(row, "header")?
                .iter()
                .map(|value| match value {
                    Json::Int(value) => {
                        u8::try_from(*value).map_err(|_| "invalid film header byte")
                    }
                    _ => Err("invalid film header byte"),
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if header.len() != 8 {
                return Err("film edgecode header must contain eight bytes".into());
            }
            changes.set(donor, id, "header", Value::Bytes(header))?;
        }
    }
    changes.items.insert(owner.to_owned());
    Ok(())
}
