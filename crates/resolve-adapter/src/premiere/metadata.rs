//! Metadata stays on its saved owner. Logging labels do not select files or clocks.
use crate::{clip_label::PremiereLabel, Result};
use graph::{source::SourceGraph, Loss, Sidecar, StreamKind, Value};
use prproj::{
    build::time::UNSET_TICKS,
    model::{ItemId, Project},
    xml::{NodeId, Tree},
};
use std::path::Path;

pub(crate) const LOSS: &str = "premiere.clip_metadata";
pub(crate) const LABEL_LOSS: &str = "premiere.clip_label";
pub(crate) const TIMELINE_MARK_LOSS: &str = "premiere.timeline_mark";
pub(crate) const SYNC_LOCK_LOSS: &str = "premiere.track_sync_lock";

#[derive(Default)]
pub(super) struct TrackStateCapture {
    pub(super) mapped: Vec<Value>,
    pub(super) sync_unlocked: Vec<Value>,
    pub(super) losses: Vec<Loss>,
}

pub(super) fn capture_track_state(
    tree: &Tree,
    element: NodeId,
    kind: StreamKind,
    index: i64,
    owner: &str,
    captured: &mut TrackStateCapture,
) -> Result<()> {
    let lane = format!("{kind:?} {index}");
    let tracks = tree.findall(element, "ClipTrack/Track")?;
    let [track] = tracks.as_slice() else {
        return Err(format!("{owner}: {lane} has no unique Premiere track owner").into());
    };
    let flag = |tag| -> Result<Option<bool>> {
        let fields = tree.findall(*track, tag)?;
        match fields.as_slice() {
            [] => Ok(None),
            [field] if tree.is_childless(*field) && tree.text(*field) == Some("true") => Ok(Some(true)),
            [field] if tree.is_childless(*field) && tree.text(*field) == Some("false") => Ok(Some(false)),
            _ => Err(format!("{owner}: invalid Premiere {lane} {tag}").into()),
        }
    };
    let locked = flag("IsLocked")?.unwrap_or(false);
    let muted = flag("IsMuted")?.unwrap_or(false);
    if kind == StreamKind::Audio && muted {
        return Err(format!("{owner}: muted Premiere {lane} requires a verified Resolve mix mapping").into());
    }
    let disabled = kind == StreamKind::Video && muted;
    let sync_locked = flag("IsSyncLocked")?.unwrap_or(true);
    let bags = tree.findall(*track, "Node/Properties")?;
    let name = match bags.as_slice() {
        [] => "",
        [bag] => leaf(tree, *bag, "MZ.TrackName")?.unwrap_or(""),
        _ => return Err(format!("{owner}: {lane} has an ambiguous Premiere track property bag").into()),
    };
    if !crate::track_state::valid_name(name) {
        return Err(format!("{owner}: invalid Premiere {lane} name").into());
    }
    if !name.is_empty() || locked || disabled {
        captured.mapped.push(Value::Obj(vec![
            ("kind".into(), Value::Str(kind.as_str().into())),
            ("track".into(), Value::Int(index)),
            ("name".into(), Value::Str(name.into())),
            ("locked".into(), Value::Bool(locked)),
            ("disabled".into(), Value::Bool(disabled)),
        ]));
    }
    if !sync_locked {
        captured.sync_unlocked.push(Value::Obj(vec![
            ("kind".into(), Value::Str(kind.as_str().into())),
            ("track".into(), Value::Int(index)),
            ("sync_locked".into(), Value::Bool(false)),
        ]));
        captured.losses.push(Loss {
            object: owner.into(),
            aspect: SYNC_LOCK_LOSS.into(),
            reason: format!("Premiere {lane} sync lock is off; Resolve sync-lock mapping is unqualified"),
        });
    }
    Ok(())
}

pub(super) fn capture_timeline_marks(
    project: &Project,
    sequence: NodeId,
    owner: &str,
    frame_ticks: i64,
    graph: &mut SourceGraph,
) -> Result<()> {
    let tree = &project.doc.tree;
    let properties = tree.findall(sequence, "Node/Properties")?;
    let properties = match properties.as_slice() {
        [] => return Ok(()),
        [properties] => *properties,
        _ => return Err(format!("{owner}: ambiguous Premiere sequence property bag").into()),
    };
    let mut saved = Vec::new();
    for (name, tag) in [("in", "MZ.InPoint"), ("out", "MZ.OutPoint")] {
        let Some(raw) = leaf(tree, properties, tag)? else { continue };
        let ticks: i64 = raw.parse().map_err(|_| format!("{owner}: invalid Premiere {tag} ticks"))?;
        if ticks == UNSET_TICKS {
            continue;
        }
        if ticks < 0 {
            return Err(format!("{owner}: unexpected negative Premiere {tag} ticks").into());
        }
        let mut fields = vec![("ticks".into(), Value::Int(ticks))];
        if ticks % frame_ticks == 0 {
            fields.push(("saved_frame".into(), Value::Int(ticks / frame_ticks)));
        }
        saved.push((name.into(), Value::Obj(fields)));
        graph.losses.push(Loss {
            object: owner.into(),
            aspect: TIMELINE_MARK_LOSS.into(),
            reason: format!("Premiere sequence {tag} at {ticks} ticks retained in graph; Resolve range mapping is pending"),
        });
    }
    if !saved.is_empty() {
        graph.sidecars.push(Sidecar {
            adapter: "premiere.timeline_marks".into(),
            object: owner.into(),
            data: Value::Obj(saved),
        });
    }
    Ok(())
}

pub(super) fn capture_edit_label(
    project: &Project,
    clip: NodeId,
    item: ItemId,
    owner: &str,
    graph: &mut SourceGraph,
) -> Result<()> {
    let tree = &project.doc.tree;
    let (name, packed) = edit_label_fields(tree, clip)?;
    if name.is_none() && packed.is_none() {
        return Ok(());
    }
    let mut matches_master = false;
    if let Some(master) = project.item(item).master {
        let references = tree.findall(master, "Clips/Clip")?;
        if references.len() > 32 {
            return Err("too many Premiere master clip references for label comparison".into());
        }
        let mut matching_kind = 0;
        let mut different = false;
        for reference in references {
            let template = project.doc.resolve(reference)?;
            if tree.tag(template) == tree.tag(clip) {
                matching_kind += 1;
                different |= edit_label_fields(tree, template)? != (name, packed);
            }
        }
        matches_master = matching_kind > 0 && !different;
    }

    let label = name.zip(packed).and_then(|(name, packed)| PremiereLabel::parse(name, packed));
    let mut fields = vec![("default_matches_master".into(), Value::Bool(matches_master))];
    for (field, value) in [("label_name", name), ("packed_color", packed)] {
        if let Some(value) = value {
            if value.len() > 128 {
                return Err("Premiere edit label field exceeds limit".into());
            }
            fields.push((field.into(), Value::Str(value.into())));
        }
    }
    graph.sidecars.push(Sidecar {
        adapter: crate::clip_label::PREMIERE_ADAPTER.into(),
        object: owner.into(),
        data: Value::Obj(fields),
    });

    let reason = match label {
        None => Some("unrecognized Premiere label fields omitted from Resolve".into()),
        Some(_) if matches_master => None,
        Some(label) => {
            let (_, source, destination) = label.resolve();
            if label.packed != prproj::convert::colors::LABEL_PACKED[label.index] {
                Some(format!("Premiere {source} uses a different saved swatch; Resolve uses {destination}"))
            } else if source != destination {
                Some(format!("Premiere {source} mapped to Resolve {destination}"))
            } else {
                None
            }
        }
    };
    if let Some(reason) = reason {
        graph.losses.push(Loss { object: owner.into(), aspect: LABEL_LOSS.into(), reason });
    }
    Ok(())
}

fn edit_label_fields(tree: &Tree, clip: NodeId) -> Result<(Option<&str>, Option<&str>)> {
    match tree.findall(clip, "Clip/Node/Properties")?.as_slice() {
        [] => Ok((None, None)),
        [properties] => Ok((
            leaf(tree, *properties, "asl.clip.label.name")?,
            leaf(tree, *properties, "asl.clip.label.color")?,
        )),
        _ => Err("ambiguous Premiere clip label properties".into()),
    }
}

fn leaf<'a>(tree: &'a Tree, parent: NodeId, tag: &str) -> Result<Option<&'a str>> {
    let mut fields = tree
        .children(parent)
        .iter()
        .copied()
        .filter(|id| tree.tag(*id) == tag);
    let Some(field) = fields.next() else {
        return Ok(None);
    };
    if fields.next().is_some() || !tree.children(field).is_empty() {
        return Err(format!("ambiguous Premiere metadata field {tag}").into());
    }
    Ok(tree.text(field).filter(|s| !s.is_empty()))
}

pub(super) fn check_source_origin(tree: &Tree, media: NodeId) -> Result<()> {
    let alternate = match leaf(tree, media, "UseAlternateStart")? {
        None | Some("false") => None,
        Some("true") => Some(
            leaf(tree, media, "AlternateStart")?.ok_or("missing Premiere alternate media start")?,
        ),
        Some(_) => return Err("invalid Premiere alternate media start flag".into()),
    };
    for value in [leaf(tree, media, "Start")?, alternate]
        .into_iter()
        .flatten()
    {
        if value.parse::<i64>()? != 0 {
            return Err(
                "Premiere media has a nonzero saved source origin; conform mapping pending".into(),
            );
        }
    }
    Ok(())
}

pub(super) fn check_locators(tree: &Tree, media: NodeId, saved: &Path) -> Result<()> {
    let stored = leaf(tree, media, "FilePath")?.ok_or("Premiere saved FilePath is missing")?;
    if Path::new(stored) != saved {
        return Err("Premiere item locator disagrees with its saved Media/FilePath".into());
    }
    if leaf(tree, media, "ActualMediaFilePath")?.is_some_and(|actual| Path::new(actual) != saved) {
        return Err("Premiere FilePath and ActualMediaFilePath disagree; resolve the original in Premiere before turnover".into());
    }
    Ok(())
}

pub(super) fn capture(
    project: &Project,
    item: ItemId,
    media: NodeId,
    item_id: &str,
    sha256: &str,
    graph: &mut SourceGraph,
) -> Result<Option<String>> {
    let tree = &project.doc.tree;
    // The turnover already limits bound items to 256. Limit each item's
    // retained metadata before cloning shared source values.
    let mut remaining: usize = 64 << 10;
    let mut copy_text = |value: &str| -> Result<Value> {
        if value.len() > 32 << 10 {
            return Err("Premiere metadata field exceeds graph text limit".into());
        }
        remaining = remaining
            .checked_sub(value.len())
            .ok_or("Premiere item metadata exceeds limit")?;
        Ok(Value::Str(value.into()))
    };
    let mut identity = vec![("verified_sha256".into(), Value::Str(sha256.into()))];
    let mut media_tape = false;
    for tag in [
        "FilePath",
        "ActualMediaFilePath",
        "FileKey",
        "Start",
        "AlternateStart",
        "UseAlternateStart",
        "TapeName",
        "AlternateTapeName",
    ] {
        if let Some(value) = leaf(tree, media, tag)? {
            media_tape |= matches!(tag, "TapeName" | "AlternateTapeName");
            identity.push((tag.into(), copy_text(value)?));
        }
    }
    let paths = tree.findall(media, "RelativePath")?;
    if paths.len() > 32 {
        return Err("Premiere media has too many saved relative locators".into());
    }
    let relative_paths = paths
        .into_iter()
        .filter_map(|id| tree.text(id).filter(|value| !value.is_empty()))
        .map(&mut copy_text)
        .collect::<Result<Vec<_>>>()?;
    if !relative_paths.is_empty() {
        identity.push(("RelativePath".into(), Value::Arr(relative_paths)));
    }
    graph.sidecars.push(Sidecar {
        adapter: "premiere.media_identity".into(),
        object: item_id.into(),
        data: Value::Obj(identity),
    });

    let data = project.item(item);
    let mut logging = Vec::new();
    let mut omitted = Vec::new();
    let mut tape = None;
    if let Some(master) = data.master {
        let references = tree.findall(master, "LoggingInfo")?;
        match references.as_slice() {
            [] => {}
            [reference] => {
                let owner = project.doc.resolve(*reference)?;
                if tree.tag(owner) != "ClipLoggingInfo" {
                    return Err("unexpected Premiere clip logging owner".into());
                }
                for (tag, label) in [
                    ("TapeName", "Tape Name"),
                    ("ShotTake", "Shot"),
                    ("Scene", "Scene"),
                    ("Description", "Description"),
                    ("LogNote", "Log Note"),
                ] {
                    if let Some(value) = leaf(tree, owner, tag)? {
                        logging.push((tag.into(), copy_text(value)?));
                        omitted.push(label);
                        if tag == "TapeName" {
                            tape = Some(value.into());
                        }
                    }
                }
            }
            _ => return Err("ambiguous Premiere clip logging reference".into()),
        }
    }
    if let Some(properties) = tree.find1(data.element, "ProjectItem/Node/Properties") {
        for (tag, label) in [
            ("Column.PropertyText.Comment", "Comment"),
            ("Column.PropertyText.Client", "Client"),
        ] {
            if let Some(value) = leaf(tree, properties, tag)? {
                logging.push((tag.into(), copy_text(value)?));
                omitted.push(label);
            }
        }
    }
    if !logging.is_empty() {
        graph.sidecars.push(Sidecar {
            adapter: "premiere.logging".into(),
            object: item_id.into(),
            data: Value::Obj(logging),
        });
    }
    if media_tape {
        omitted.push("media tape metadata");
    }
    if !omitted.is_empty() {
        graph.losses.push(Loss {
            object: item_id.into(),
            aspect: LOSS.into(),
            reason: format!(
                "Resolve turnover omits {}; the source values remain in graph/2",
                omitted.join(", ")
            ),
        });
    }
    Ok(tape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_state_keeps_picture_output_and_sync_lock_separate() {
        let mut tree = Tree::new();
        let element = tree.new_element("VideoClipTrack");
        let clip = tree.sub_element(element, "ClipTrack", &[]);
        let track = tree.sub_element(clip, "Track", &[]);
        let node = tree.sub_element(track, "Node", &[]);
        let properties = tree.sub_element(node, "Properties", &[]);
        for (parent, tag, value) in [
            (properties, "MZ.TrackName", "Picture & Titles"),
            (track, "IsLocked", "true"),
            (track, "IsMuted", "true"),
            (track, "IsSyncLocked", "false"),
        ] {
            let field = tree.sub_element(parent, tag, &[]);
            tree.set_text(field, Some(value));
        }
        let mut capture = TrackStateCapture::default();
        capture_track_state(&tree, element, StreamKind::Video, 1, "test", &mut capture).unwrap();
        let [mapped] = capture.mapped.as_slice() else { panic!("missing track state") };
        assert_eq!(mapped.get("name"), Some(&Value::Str("Picture & Titles".into())));
        assert_eq!(mapped.get("locked"), Some(&Value::Bool(true)));
        assert_eq!(mapped.get("disabled"), Some(&Value::Bool(true)));
        assert_eq!(mapped.get("sync_locked"), None);
        let [sync] = capture.sync_unlocked.as_slice() else { panic!("missing sync lock") };
        assert_eq!(sync.get("sync_locked"), Some(&Value::Bool(false)));
        assert_eq!(capture.losses[0].aspect, SYNC_LOCK_LOSS);
        tree.sub_element(track, "IsMuted", &[]);
        assert!(capture_track_state(&tree, element, StreamKind::Video, 1, "test", &mut capture).is_err());
    }

    fn media(fields: &[(&str, &str)]) -> (Tree, NodeId) {
        let mut tree = Tree::new();
        let media = tree.new_element("Media");
        for &(tag, value) in fields {
            let field = tree.sub_element(media, tag, &[]);
            tree.set_text(field, Some(value));
        }
        (tree, media)
    }

    #[test]
    fn saved_clock_and_override_are_checked_separately() {
        for (fields, valid) in [
            (&[][..], true),
            (
                &[
                    ("Start", "0"),
                    ("AlternateStart", "120"),
                    ("UseAlternateStart", "false"),
                ][..],
                true,
            ),
            (&[("Start", "120")][..], false),
            (
                &[
                    ("Start", "0"),
                    ("AlternateStart", "120"),
                    ("UseAlternateStart", "true"),
                ][..],
                false,
            ),
            (&[("UseAlternateStart", "true")][..], false),
            (&[("Start", "0"), ("Start", "120")][..], false),
        ] {
            let (tree, owner) = media(fields);
            assert_eq!(check_source_origin(&tree, owner).is_ok(), valid);
        }
    }

    #[test]
    fn differing_locator_with_same_basename_is_ambiguous() {
        let saved = Path::new("/original/A001.mov");
        let (tree, owner) = media(&[
            ("FilePath", "/original/A001.mov"),
            ("ActualMediaFilePath", "/original/A001.mov"),
        ]);
        assert!(check_locators(&tree, owner, saved).is_ok());
        let (tree, owner) = media(&[
            ("FilePath", "/original/A001.mov"),
            ("ActualMediaFilePath", "/different/A001.mov"),
        ]);
        assert!(check_locators(&tree, owner, saved)
            .unwrap_err()
            .to_string()
            .contains("disagree"));
    }
}
