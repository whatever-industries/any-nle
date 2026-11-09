//! `pr-inspect` (`cli/inspect.py`): inspect `.prproj` structure.
//!
//! ```text
//! prproj-inspect file.prproj                (object summary)
//! prproj-inspect file.prproj --list         (top-level objects)
//! prproj-inspect file.prproj --tree [--depth N]
//! prproj-inspect file.prproj --dump 42      (object by ObjectID / ObjectUID / tag)
//! prproj-inspect file.prproj --xml [--out payload.xml]
//! ```

use std::io::Write;
use std::path::Path;

use super::args::{self, Kind, Opt, Outcome, Spec};
use super::{os_error, path_name, report, say, PyErr, PyResult};
use crate::convert::model::py_str_repr;
use crate::pystr::py_strip;
use crate::xml::serializer::serialize_element;
use crate::xml::{parse_prproj, NodeId, PremiereDocument};

pub const SPEC: Spec = Spec {
    prog: "prproj-inspect",
    usage: "usage: prproj-inspect [-h] [--list] [--tree] [--depth N] [--dump SELECTOR] [--xml]\n                      [--out FILE]\n                      file",
    help: "Inspect Premiere Pro project file (.prproj) structure.

positional arguments:
  file             prproj file to inspect

options:
  -h, --help       show this help message and exit
  --list           List top-level objects
  --tree           Print the element tree
  --depth N        Max tree depth (-1 for unlimited, use with --tree)
  --dump SELECTOR  Print one object's XML by ObjectID, ObjectUID, or top-level
                   tag
  --xml            Write the decompressed XML payload (to --out or stdout)
  --out FILE       Write --xml payload bytes to FILE instead of stdout

Examples:
    prproj-inspect file.prproj                 (object summary)
    prproj-inspect file.prproj --list          (top-level objects with IDs and names)
    prproj-inspect file.prproj --tree --depth 3
    prproj-inspect file.prproj --dump 42      (object with ObjectID 42)
    prproj-inspect file.prproj --dump Sequence
    prproj-inspect file.prproj --xml --out payload.xml

Use --out for byte-exact payload extraction; PowerShell's `>` redirection
re-encodes output and corrupts it.",
    opts: &[
        Opt { long: "--list", kind: Kind::Flag },
        Opt { long: "--tree", kind: Kind::Flag },
        Opt { long: "--depth", kind: Kind::Int },
        Opt { long: "--dump", kind: Kind::Text },
        Opt { long: "--xml", kind: Kind::Flag },
        Opt { long: "--out", kind: Kind::Text },
    ],
    positionals: &[("file", false)],
};

/// `_text_preview`: `repr(text)` cut to `limit` characters.
fn text_preview(text: &str, limit: usize) -> String {
    let preview = py_str_repr(text);
    if preview.chars().count() > limit {
        let head: String = preview.chars().take(limit - 3).collect();
        format!("{head}...")
    } else {
        preview
    }
}

fn element_label(doc: &PremiereDocument, element: NodeId) -> String {
    let el = &doc.tree[element];
    let attrs: String = el.attrs.iter().map(|(k, v)| format!(" {k}=\"{v}\"")).collect();
    format!("<{}{attrs}>", el.tag)
}

fn summary(out: &mut dyn Write, path: &str, document: &PremiereDocument) -> PyResult<i32> {
    let payload = document.to_xml_bytes()?;
    let size = std::fs::metadata(path).map_err(|e| PyErr::from(os_error(Path::new(path), &e)))?.len();
    say!(out, "{}: {} bytes on disk, {} bytes XML", path_name(path), size, payload.len());
    let tree = &document.tree;
    let version = tree.get(document.root, "Version").map_or("None".to_string(), str::to_string);
    say!(
        out,
        "PremiereData Version={}, {} top-level objects ({} with ObjectID, {} with ObjectUID)",
        version,
        tree.children(document.root).len(),
        document.by_object_id.len(),
        document.by_object_uid.len()
    );
    // Counter.most_common(): by count, descending, ties in first-seen order.
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for &element in tree.children(document.root) {
        let tag = tree.tag(element);
        match counts.iter_mut().find(|(t, _)| *t == tag) {
            Some(slot) => slot.1 += 1,
            None => counts.push((tag, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    for (tag, count) in counts {
        say!(out, "  {count:5}  {tag}");
    }
    Ok(0)
}

fn list_objects(out: &mut dyn Write, document: &PremiereDocument) -> PyResult<i32> {
    let tree = &document.tree;
    for (index, &element) in tree.children(document.root).iter().enumerate() {
        let object_id = tree.get(element, "ObjectID").unwrap_or("");
        let object_uid = tree.get(element, "ObjectUID").unwrap_or("");
        let name = tree.findtext(element, "Name")?.unwrap_or_default();
        let ids = [object_id, object_uid].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" ");
        let detail = if name.is_empty() { String::new() } else { format!("  {}", py_str_repr(&name)) };
        say!(out, "  [{index:4}] {} {ids}{detail}", tree.tag(element));
    }
    Ok(0)
}

fn print_tree(out: &mut dyn Write, document: &PremiereDocument, root: NodeId, max_depth: i64) {
    let tree = &document.tree;
    let mut stack = vec![(root, 0i64)];
    while let Some((element, depth)) = stack.pop() {
        if 0 <= max_depth && max_depth < depth {
            continue;
        }
        let mut detail = String::new();
        if tree.children(element).is_empty() {
            if let Some(text) = tree[element].text.as_deref() {
                if !py_strip(text).is_empty() {
                    detail = format!(" = {}", text_preview(text, 60));
                }
            }
        }
        say!(out, "{}{}{detail}", "  ".repeat(depth as usize), element_label(document, element));
        for &child in tree.children(element).iter().rev() {
            stack.push((child, depth + 1));
        }
    }
}

fn dump(out: &mut dyn Write, err: &mut dyn Write, document: &PremiereDocument, selector: &str) -> PyResult<i32> {
    let tree = &document.tree;
    let found = document.by_object_id.get(selector).or_else(|| document.by_object_uid.get(selector)).copied();
    if let Some(element) = found {
        let _ = out.write_all(&serialize_element(tree, element)?);
        let _ = out.write_all(b"\n");
        return Ok(0);
    }
    let matches: Vec<NodeId> = tree.children(document.root).iter().copied().filter(|&e| tree.tag(e) == selector).collect();
    if matches.is_empty() {
        say!(err, "No top-level object matches {} ", py_str_repr(selector));
        say!(err, "(try an ObjectID, an ObjectUID, or a top-level tag name)");
        return Ok(1);
    }
    if matches.len() > 1 {
        say!(err, "# {} <{selector}> objects", matches.len());
    }
    for element in matches {
        let _ = out.write_all(&serialize_element(tree, element)?);
        let _ = out.write_all(b"\n");
    }
    Ok(0)
}

fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> PyResult<i32> {
    let parsed = match args::parse(&SPEC, argv, out, err) {
        Outcome::Run(p) => p,
        Outcome::Exit(code) => return Ok(code),
    };
    let path = parsed.positionals[0][0].as_str();
    if !Path::new(path).exists() {
        say!(err, "Error: File not found: {}", super::path_str(path));
        return Ok(1);
    }
    let document = parse_prproj(path)?;
    if parsed.flag("--xml") {
        let payload = document.to_xml_bytes()?;
        match parsed.text("--out") {
            Some(target) => std::fs::write(target, &payload).map_err(|e| PyErr::from(os_error(Path::new(target), &e)))?,
            None => {
                let _ = out.write_all(&payload);
            }
        }
        return Ok(0);
    }
    if parsed.flag("--tree") {
        print_tree(out, &document, document.root, parsed.int("--depth").unwrap_or(-1));
        return Ok(0);
    }
    if let Some(selector) = parsed.text("--dump") {
        return dump(out, err, &document, selector);
    }
    if parsed.flag("--list") {
        return list_objects(out, &document);
    }
    summary(out, path, &document)
}

/// The `pr-inspect` entry point: `argv` without the program name; returns the exit status.
pub fn main(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match run(argv, out, err) {
        Ok(code) => code,
        Err(e) => {
            report(err, &e);
            1
        }
    }
}
