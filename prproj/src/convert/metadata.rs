//! Avid bin columns -> Premiere clip metadata (port of `avid2prproj/metadata.py`).
//!
//! Class 1 columns (Comment) live in the project item's property bag; class 0 logging columns
//! live on the master clip's `ClipLoggingInfo` as `ShotTake`, `TapeName`, `Description`,
//! `Scene`, `LogNote` (the order Premiere wrote). Everything else goes into one Log Note line.

use crate::convert::model::{py_str_repr, AvidInfo};
use crate::error::{value, Result};
use crate::model::{ItemId, Project};
use crate::xml::{NodeId, Tree};

/// `LOGGING`: (Avid column, ClipLoggingInfo element).
pub const LOGGING: [(&str, &str); 3] = [("Camroll", "TapeName"), ("Scene", "Scene"), ("Take", "ShotTake")];
/// `BAG`: (Avid column, property-bag key).
pub const BAG: [(&str, &str); 1] = [("Comments", "Column.PropertyText.Comment")];
pub const LOGGING_ORDER: [&str; 5] = ["ShotTake", "TapeName", "Description", "Scene", "LogNote"];
/// Avid-internal or already-represented columns that are not copied as text.
pub const SKIP: [&str; 2] = ["UNC Path", "Video"];

/// `(logging-info elements, property-bag leaves)` for one clip, in a stable order.
pub fn premiere_fields(avid: &AvidInfo) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut logging: Vec<(String, String)> = Vec::new();
    let mut bag: Vec<(String, String)> = Vec::new();
    let mut used: Vec<&str> = SKIP.to_vec();
    for (avid_name, tag) in LOGGING {
        if let Some(v) = avid.column(avid_name).filter(|v| !v.is_empty()) {
            logging.push((tag.to_string(), v.to_string()));
            used.push(avid_name);
        }
    }
    for (avid_name, key) in BAG {
        if let Some(v) = avid.column(avid_name).filter(|v| !v.is_empty()) {
            bag.push((key.to_string(), v.to_string()));
            used.push(avid_name);
        }
    }
    if let Some(f) = avid.source_file.as_deref().filter(|f| !f.is_empty()) {
        logging.push(("Description".to_string(), f.to_string()));
    }
    let mut notes: Vec<String> =
        avid.columns.iter().filter(|(k, v)| !used.contains(&k.as_str()) && !v.is_empty()).map(|(k, v)| format!("{k}: {v}")).collect();
    if let Some(p) = avid.source_path.as_deref().filter(|p| !p.is_empty()) {
        notes.push(format!("Source Path: {p}"));
    }
    if let Some(t) = avid.start_timecode.as_deref().filter(|t| !t.is_empty()) {
        notes.push(format!("Avid Start: {t}"));
    }
    for aux in &avid.aux_timecodes {
        // Aux TC number = track index - 2 is a hypothesis (one fixture, TC3/TC4).
        notes.push(format!("Aux TC{}: {}", aux.track_index - 2, aux.timecode));
    }
    if !notes.is_empty() {
        // One line: Premiere showed an empty Log Note for a newline-separated value.
        logging.push(("LogNote".to_string(), notes.join(" | ")));
    }
    let order = |tag: &str| LOGGING_ORDER.iter().position(|t| *t == tag).unwrap_or(usize::MAX);
    logging.sort_by_key(|(tag, _)| order(tag)); // stable, like Python's sort
    (logging, bag)
}

/// `_set_leaves`: replace or append leaf elements, keeping the parent's indentation.
fn set_leaves(tree: &mut Tree, parent: NodeId, pairs: &[(String, String)], fallback_indent: &str) {
    let (child_indent, closing): (String, Option<String>) = if tree.len(parent) > 0 {
        let indent = tree.text(parent).filter(|t| !t.is_empty()).unwrap_or(fallback_indent).to_string();
        let last = *tree.children(parent).last().unwrap();
        (indent, tree.tail(last).map(String::from))
    } else {
        let closing = &fallback_indent[..fallback_indent.len() - 1];
        tree.set_text(parent, Some(fallback_indent));
        (fallback_indent.to_string(), Some(closing.to_string()))
    };
    for (tag, text) in pairs {
        let existing = match tree.child_by_tag(parent, tag) {
            Some(e) => e,
            None => {
                if tree.len(parent) > 0 {
                    let last = *tree.children(parent).last().unwrap();
                    tree.set_tail(last, Some(child_indent.clone()));
                }
                let e = tree.sub_element(parent, tag, &[]);
                tree.set_tail(e, closing.clone());
                e
            }
        };
        tree.set_text(existing, Some(text.as_str()));
    }
}

/// `apply(project, item, avid)`: write the Avid metadata onto one project item.
pub fn apply(project: &mut Project, item: ItemId, avid: Option<&AvidInfo>) -> Result<()> {
    let Some(avid) = avid else { return Ok(()) };
    let (logging, bag) = premiere_fields(avid);
    let element = project.item(item).element;
    let Some(properties) = project.doc.tree.find1(element, "ProjectItem/Node/Properties") else {
        return value("project item has no property bag");
    };
    set_leaves(&mut project.doc.tree, properties, &bag, "\n\t\t\t\t\t");
    let Some(master_ref) = project.doc.tree.find1(element, "MasterClip") else {
        return value("project item has no MasterClip");
    };
    let master = project.doc.resolve(master_ref)?;
    let Some(logging_ref) = project.doc.tree.child_by_tag(master, "LoggingInfo") else {
        return value(format!("master clip of {} has no LoggingInfo", py_str_repr(&project.item_name(item))));
    };
    let info = project.doc.resolve(logging_ref)?;
    set_leaves(&mut project.doc.tree, info, &logging, "\n\t\t");
    Ok(())
}
