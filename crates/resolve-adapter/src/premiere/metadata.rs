//! Metadata stays on its saved owner. Logging labels do not select files or clocks.
use crate::Result;
use graph::{source::SourceGraph, Loss, Sidecar, Value};
use prproj::{
    model::{ItemId, Project},
    xml::{NodeId, Tree},
};
use std::path::Path;

pub(crate) const LOSS: &str = "premiere.clip_metadata";

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
