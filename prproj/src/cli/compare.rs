//! `pr-compare` (`cli/compare.py`): compare two `.prproj` files structurally, aware of resave
//! churn. Top-level objects are matched by `ObjectUID`, then by a ref-aware signature that
//! ignores per-save integers, then pairwise by tag; children are aligned by longest common
//! subsequence (`difflib.SequenceMatcher`, ported below, no junk heuristics).
//!
//! ```text
//! prproj-compare a.prproj b.prproj [--filter TEXT] [--max-per-object N] [--show-churn]
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::Path;

use super::args::{self, Kind, Opt, Outcome, Spec};
use super::{path_name, path_str, report, say, PyResult};
use crate::convert::model::py_str_repr;
use crate::pystr::py_strip;
use crate::xml::{parse_prproj, NodeId, PremiereDocument, Tree};

pub const SPEC: Spec = Spec {
    prog: "prproj-compare",
    usage: "usage: prproj-compare [-h] [--filter FILTER] [--max-per-object N] [--show-churn]\n                      file1 file2",
    help: "Compare two Premiere Pro project files (.prproj).

positional arguments:
  file1                 First prproj file
  file2                 Second prproj file

options:
  -h, --help            show this help message and exit
  --filter FILTER       Only report objects whose tag or label contains this
                        substring
  --max-per-object N    Cap reported diff lines per object pair (default 25)
  --show-churn          Report raw ObjectID/ObjectRef renumbering instead of
                        resolving it

Examples:
    prproj-compare with_feature.prproj without_feature.prproj
    prproj-compare a.prproj b.prproj --filter Sequence
    prproj-compare a.prproj b.prproj --show-churn      (include ObjectID renumbering)

Objects are matched by ObjectUID, then by an ObjectID-insensitive signature,
then by tag in document order. Use prproj-inspect for single-file views.",
    opts: &[
        Opt { long: "--filter", kind: Kind::Text },
        Opt { long: "--max-per-object", kind: Kind::Int },
        Opt { long: "--show-churn", kind: Kind::Flag },
    ],
    positionals: &[("file1", false), ("file2", false)],
};

/// Attributes renumbered on every Premiere save; excluded from signatures.
const VOLATILE_ATTRS: [&str; 2] = ["ObjectID", "ObjectRef"];

/// How far a ref-aware signature follows a chain of UID-less targets.
const REF_DEPTH: i32 = 2;

#[derive(Default)]
struct Stats {
    diff_lines: usize,
    churn_suppressed: usize,
    formatting_suppressed: usize,
}

fn ref_identity(document: &PremiereDocument, reference: &str, depth: i32) -> String {
    let Some(&target) = document.by_object_id.get(reference) else { return "<dangling>".into() };
    if let Some(uid) = document.tree.get(target, "ObjectUID").filter(|u| !u.is_empty()) {
        return format!("{}[{uid}]", document.tree.tag(target));
    }
    if depth <= 0 {
        return document.tree.tag(target).to_string();
    }
    let mut parts = Vec::new();
    signature_parts(&document.tree, target, &mut parts, Some(document), depth - 1);
    parts.join("\0")
}

fn signature_parts(tree: &Tree, element: NodeId, out: &mut Vec<String>, document: Option<&PremiereDocument>, depth: i32) {
    out.push(tree.tag(element).to_string());
    let mut attrs: Vec<&(String, String)> = tree[element].attrs.iter().collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, value) in attrs {
        match document {
            Some(document) if key == "ObjectRef" => out.push(format!("ObjectRef->{}", ref_identity(document, value, depth))),
            _ if !VOLATILE_ATTRS.contains(&key.as_str()) => out.push(format!("{key}={value}")),
            _ => {}
        }
    }
    if let Some(text) = tree[element].text.as_deref() {
        let stripped = py_strip(text);
        if !stripped.is_empty() {
            out.push(stripped.to_string());
        }
    }
    out.push("(".into());
    for &child in tree.children(element) {
        signature_parts(tree, child, out, document, depth);
    }
    out.push(")".into());
}

fn signature(tree: &Tree, element: NodeId, document: Option<&PremiereDocument>) -> String {
    let mut parts = Vec::new();
    signature_parts(tree, element, &mut parts, document, REF_DEPTH);
    parts.join("\0")
}

fn object_label(tree: &Tree, element: NodeId) -> PyResult<String> {
    if let Some(uid) = tree.get(element, "ObjectUID") {
        return Ok(format!("{}[{uid}]", tree.tag(element)));
    }
    if let Some(name) = tree.findtext(element, "Name")?.filter(|n| !n.is_empty()) {
        return Ok(format!("{}[{}]", tree.tag(element), py_str_repr(&name)));
    }
    Ok(format!("{}[ObjectID={}]", tree.tag(element), tree.get(element, "ObjectID").unwrap_or("None")))
}

fn ref_target(document: &PremiereDocument, reference: &str) -> String {
    let Some(&target) = document.by_object_id.get(reference) else { return format!("<dangling:{reference}>") };
    match document.tree.get(target, "ObjectUID").filter(|u| !u.is_empty()) {
        Some(uid) => format!("{}[{uid}]", document.tree.tag(target)),
        None => signature(&document.tree, target, Some(document)),
    }
}

fn truncate(value: &str) -> String {
    if value.chars().count() <= 80 {
        value.to_string()
    } else {
        let head: String = value.chars().take(77).collect();
        format!("{head}...")
    }
}

/// An insertion-ordered `dict[str, list]` (Python dict order).
struct Buckets<T> {
    order: Vec<String>,
    map: HashMap<String, VecDeque<T>>,
}

impl<T> Buckets<T> {
    fn new() -> Self {
        Buckets { order: Vec::new(), map: HashMap::new() }
    }

    fn push(&mut self, key: String, value: T) {
        if !self.map.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.map.entry(key).or_default().push_back(value);
    }

    fn pop_front(&mut self, key: &str) -> Option<T> {
        self.map.get_mut(key).and_then(VecDeque::pop_front)
    }

    fn drain_all(self) -> Vec<T> {
        let Buckets { order, mut map } = self;
        order.iter().flat_map(|k| map.remove(k).unwrap_or_default()).collect()
    }
}

type Matched = (Vec<(NodeId, NodeId)>, Vec<NodeId>, Vec<NodeId>);

/// `_match_objects`: pair top-level objects across the two documents (UID > signature > tag).
fn match_objects(doc1: &PremiereDocument, doc2: &PremiereDocument) -> Matched {
    let (t1, t2) = (&doc1.tree, &doc2.tree);
    let left: Vec<NodeId> = t1.children(doc1.root).to_vec();
    let right: Vec<NodeId> = t2.children(doc2.root).to_vec();
    let mut pairs = Vec::new();
    let mut matched_right: HashSet<NodeId> = HashSet::new();

    let mut right_by_uid: HashMap<&str, NodeId> = HashMap::new();
    for &el in &right {
        if let Some(uid) = t2.get(el, "ObjectUID").filter(|u| !u.is_empty()) {
            right_by_uid.insert(uid, el);
        }
    }
    let mut left_rest = Vec::new();
    for &el in &left {
        let uid = t1.get(el, "ObjectUID").filter(|u| !u.is_empty());
        match uid.and_then(|u| right_by_uid.get(u)) {
            Some(&partner) => {
                pairs.push((el, partner));
                matched_right.insert(partner);
            }
            None => left_rest.push(el),
        }
    }

    let mut right_by_sig: Buckets<NodeId> = Buckets::new();
    for &el in &right {
        if !matched_right.contains(&el) {
            right_by_sig.push(signature(t2, el, Some(doc2)), el);
        }
    }
    let mut left_rest2 = Vec::new();
    for &el in &left_rest {
        match right_by_sig.pop_front(&signature(t1, el, Some(doc1))) {
            Some(partner) => {
                pairs.push((el, partner));
                matched_right.insert(partner);
            }
            None => left_rest2.push(el),
        }
    }

    let mut right_by_tag: Buckets<NodeId> = Buckets::new();
    for &el in right.iter().filter(|e| !matched_right.contains(e)) {
        right_by_tag.push(t2.tag(el).to_string(), el);
    }
    let mut only_left = Vec::new();
    for &el in &left_rest2 {
        match right_by_tag.pop_front(t1.tag(el)) {
            Some(partner) => pairs.push((el, partner)),
            None => only_left.push(el),
        }
    }
    (pairs, only_left, right_by_tag.drain_all())
}

#[allow(clippy::too_many_arguments)]
fn diff_attrs(
    doc1: &PremiereDocument,
    doc2: &PremiereDocument,
    el1: NodeId,
    el2: NodeId,
    path: &str,
    lines: &mut Vec<String>,
    stats: &mut Stats,
    show_churn: bool,
) {
    let (t1, t2) = (&doc1.tree, &doc2.tree);
    let mut keys: Vec<&str> = t1[el1].attrs.iter().chain(t2[el2].attrs.iter()).map(|(k, _)| k.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    let repr = |v: Option<&str>| v.map_or("None".to_string(), py_str_repr);
    for key in keys {
        let (v1, v2) = (t1.get(el1, key), t2.get(el2, key));
        if v1 == v2 {
            continue;
        }
        if VOLATILE_ATTRS.contains(&key) && !show_churn {
            let (Some(a), Some(b)) = (v1, v2) else {
                // A reference added or removed is structural, not churn.
                lines.push(format!("  {path} @{key}: {} vs {}", repr(v1), repr(v2)));
                continue;
            };
            if key == "ObjectRef" {
                let (r1, r2) = (ref_target(doc1, a), ref_target(doc2, b));
                if r1 != r2 {
                    lines.push(format!("  {path} @{key}: -> {} vs -> {}", truncate(&r1), truncate(&r2)));
                    continue;
                }
            }
            stats.churn_suppressed += 1;
            continue;
        }
        lines.push(format!("  {path} @{key}: {} vs {}", repr(v1), repr(v2)));
    }
}

fn child_key(tree: &Tree, element: NodeId) -> String {
    let uid = tree.get(element, "ObjectUID").filter(|u| !u.is_empty()).or_else(|| tree.get(element, "ObjectURef").filter(|u| !u.is_empty()));
    match uid {
        Some(uid) => format!("{}\0{uid}", tree.tag(element)),
        None => tree.tag(element).to_string(),
    }
}

/// `_align_children`: pair up children by longest common subsequence, not by position.
fn align_children(doc1: &PremiereDocument, el1: NodeId, doc2: &PremiereDocument, el2: NodeId) -> Vec<(Option<usize>, Option<usize>)> {
    let keys1: Vec<String> = doc1.tree.children(el1).iter().map(|&c| child_key(&doc1.tree, c)).collect();
    let keys2: Vec<String> = doc2.tree.children(el2).iter().map(|&c| child_key(&doc2.tree, c)).collect();
    let mut aligned = Vec::new();
    for (opcode, start1, end1, start2, end2) in get_opcodes(&keys1, &keys2) {
        if opcode == Op::Equal {
            aligned.extend((start1..end1).zip(start2..end2).map(|(a, b)| (Some(a), Some(b))));
            continue;
        }
        let overlap = (end1 - start1).min(end2 - start2);
        aligned.extend((0..overlap).map(|k| (Some(start1 + k), Some(start2 + k))));
        aligned.extend((start1 + overlap..end1).map(|i| (Some(i), None)));
        aligned.extend((start2 + overlap..end2).map(|j| (None, Some(j))));
    }
    aligned
}

#[allow(clippy::too_many_arguments)]
fn diff_pair(
    doc1: &PremiereDocument,
    doc2: &PremiereDocument,
    el1: NodeId,
    el2: NodeId,
    path: &str,
    lines: &mut Vec<String>,
    stats: &mut Stats,
    show_churn: bool,
) {
    let (t1, t2) = (&doc1.tree, &doc2.tree);
    if t1.tag(el1) != t2.tag(el2) {
        lines.push(format!("  {path}: tag <{}> vs <{}>", t1.tag(el1), t2.tag(el2)));
        return;
    }
    diff_attrs(doc1, doc2, el1, el2, path, lines, stats, show_churn);
    let (text1, text2) = (t1[el1].text_or_empty(), t2[el2].text_or_empty());
    if text1 != text2 {
        let is_leaf = t1.children(el1).is_empty() && t2.children(el2).is_empty();
        if !py_strip(text1).is_empty() || !py_strip(text2).is_empty() || is_leaf {
            lines.push(format!("  {path}: text {} vs {}", py_str_repr(&truncate(text1)), py_str_repr(&truncate(text2))));
        } else {
            stats.formatting_suppressed += 1;
        }
    }
    let (c1, c2) = (t1.children(el1), t2.children(el2));
    if c1.len() != c2.len() {
        let count = |tree: &Tree, children: &[NodeId]| {
            let mut m: HashMap<String, usize> = HashMap::new();
            for &c in children {
                *m.entry(tree.tag(c).to_string()).or_default() += 1;
            }
            m
        };
        let (n1, n2) = (count(t1, c1), count(t2, c2));
        let mut tags: Vec<&String> = n1.keys().chain(n2.keys()).collect();
        tags.sort();
        tags.dedup();
        let delta: Vec<String> = tags
            .into_iter()
            .filter_map(|tag| {
                let (a, b) = (n1.get(tag).copied().unwrap_or(0), n2.get(tag).copied().unwrap_or(0));
                (a != b).then(|| format!("{}: ({a}, {b})", py_str_repr(tag)))
            })
            .collect();
        lines.push(format!("  {path}: child count {} vs {} {{{}}}", c1.len(), c2.len(), delta.join(", ")));
    }
    for (index1, index2) in align_children(doc1, el1, doc2, el2) {
        match (index1, index2) {
            (Some(i1), Some(i2)) => {
                let label = format!("{path}/{}[{i1}]", t1.tag(c1[i1]));
                diff_pair(doc1, doc2, c1[i1], c2[i2], &label, lines, stats, show_churn);
            }
            (Some(i1), None) => lines.push(format!("  {path}: only in file1 <{}>[{i1}]", t1.tag(c1[i1]))),
            (None, Some(i2)) => lines.push(format!("  {path}: only in file2 <{}>[{i2}]", t2.tag(c2[i2]))),
            (None, None) => {}
        }
    }
}

// ---- difflib.SequenceMatcher (autojunk=False, no isjunk) ---------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Equal,
    Other,
}

/// `find_longest_match(alo, ahi, blo, bhi)`: earliest-in-a, then earliest-in-b longest match.
fn find_longest_match(a: &[String], b2j: &HashMap<&str, Vec<usize>>, lb: usize, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
    // j2len[j] = length of the match ending at (i-1, j); entries absent from the dict are 0.
    let mut j2len = vec![0usize; lb + 1];
    let mut touched: Vec<usize> = Vec::new();
    let mut new_j2len = vec![0usize; lb + 1];
    let mut new_touched: Vec<usize> = Vec::new();
    for (i, item) in a.iter().enumerate().take(ahi).skip(alo) {
        if let Some(js) = b2j.get(item.as_str()) {
            for &j in js {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = if j > 0 { j2len[j - 1] } else { 0 } + 1;
                new_j2len[j] = k;
                new_touched.push(j);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        for &j in &touched {
            j2len[j] = 0;
        }
        touched.clear();
        std::mem::swap(&mut j2len, &mut new_j2len);
        std::mem::swap(&mut touched, &mut new_touched);
    }
    (besti, bestj, bestsize)
}

fn get_matching_blocks(a: &[String], b: &[String]) -> Vec<(usize, usize, usize)> {
    let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, elt) in b.iter().enumerate() {
        b2j.entry(elt.as_str()).or_default().push(j);
    }
    let (la, lb) = (a.len(), b.len());
    let mut queue = vec![(0, la, 0, lb)];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(a, &b2j, lb, alo, ahi, blo, bhi);
        if k != 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();
    let (mut i1, mut j1, mut k1) = (0, 0, 0);
    let mut merged = Vec::new();
    for (i2, j2, k2) in blocks {
        if i1 + k1 == i2 && j1 + k1 == j2 {
            k1 += k2;
        } else {
            if k1 != 0 {
                merged.push((i1, j1, k1));
            }
            (i1, j1, k1) = (i2, j2, k2);
        }
    }
    if k1 != 0 {
        merged.push((i1, j1, k1));
    }
    merged.push((la, lb, 0));
    merged
}

fn get_opcodes(a: &[String], b: &[String]) -> Vec<(Op, usize, usize, usize, usize)> {
    let (mut i, mut j) = (0, 0);
    let mut answer = Vec::new();
    for (ai, bj, size) in get_matching_blocks(a, b) {
        if i < ai || j < bj {
            answer.push((Op::Other, i, ai, j, bj));
        }
        (i, j) = (ai + size, bj + size);
        if size != 0 {
            answer.push((Op::Equal, ai, i, bj, j));
        }
    }
    answer
}

fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> PyResult<i32> {
    let parsed = match args::parse(&SPEC, argv, out, err) {
        Outcome::Run(p) => p,
        Outcome::Exit(code) => return Ok(code),
    };
    let (file1, file2) = (parsed.positionals[0][0].as_str(), parsed.positionals[1][0].as_str());
    let max_per_object = parsed.int("--max-per-object").unwrap_or(25);
    let show_churn = parsed.flag("--show-churn");
    for path in [file1, file2] {
        if !Path::new(path).exists() {
            say!(err, "Error: File not found: {}", path_str(path));
            return Ok(1);
        }
    }
    let doc1 = parse_prproj(file1)?;
    let doc2 = parse_prproj(file2)?;
    let (pairs, only_left, only_right) = match_objects(&doc1, &doc2);

    let filter = parsed.text("--filter").filter(|f| !f.is_empty());
    let needle = filter.map(str::to_lowercase);
    let wanted = |label: &str| needle.as_ref().is_none_or(|n| label.to_lowercase().contains(n.as_str()));

    let scope = filter.map_or(String::new(), |f| format!(" (filter: {f})"));
    say!(out, "Comparing {} vs {}{scope}", path_name(file1), path_name(file2));
    say!(
        out,
        "  top-level objects: {} vs {} ({} matched)",
        doc1.tree.children(doc1.root).len(),
        doc2.tree.children(doc2.root).len(),
        pairs.len()
    );

    let mut stats = Stats::default();
    let mut differing = 0usize;
    // Python slices with a possibly negative or zero limit: lines[:n] / lines[n:].
    let limit = |len: usize| -> usize {
        if max_per_object >= 0 {
            (max_per_object as usize).min(len)
        } else {
            len.saturating_sub(max_per_object.unsigned_abs() as usize)
        }
    };
    for (el1, el2) in pairs {
        let label = object_label(&doc1.tree, el1)?;
        if !wanted(&label) {
            continue;
        }
        let mut lines = Vec::new();
        diff_pair(&doc1, &doc2, el1, el2, &label, &mut lines, &mut stats, show_churn);
        if !lines.is_empty() {
            differing += 1;
            say!(out, "\n{label}: {} difference(s)", lines.len());
            let shown = limit(lines.len());
            for line in &lines[..shown] {
                say!(out, "{line}");
            }
            if lines.len() as i64 > max_per_object {
                say!(out, "  ... {} more", lines.len() as i64 - max_per_object);
            }
            stats.diff_lines += lines.len();
        }
    }

    let mut shown_only_left = Vec::new();
    for &el in &only_left {
        let label = object_label(&doc1.tree, el)?;
        if wanted(&label) {
            shown_only_left.push(label);
        }
    }
    let mut shown_only_right = Vec::new();
    for &el in &only_right {
        let label = object_label(&doc2.tree, el)?;
        if wanted(&label) {
            shown_only_right.push(label);
        }
    }
    if !shown_only_left.is_empty() {
        say!(out, "\nOnly in {} ({}):", path_name(file1), shown_only_left.len());
        for label in &shown_only_left {
            say!(out, "  {label}");
        }
    }
    if !shown_only_right.is_empty() {
        say!(out, "\nOnly in {} ({}):", path_name(file2), shown_only_right.len());
        for label in &shown_only_right {
            say!(out, "  {label}");
        }
    }
    say!(
        out,
        "\nSummary{scope}: {differing} differing object(s), {} only in file1, {} only in file2, {} ObjectID/ObjectRef churn suppressed, {} formatting-only text diffs suppressed",
        shown_only_left.len(),
        shown_only_right.len(),
        stats.churn_suppressed,
        stats.formatting_suppressed
    );
    if stats.churn_suppressed != 0 && !show_churn {
        say!(out, "(use --show-churn to see raw renumbering)");
    }
    Ok(if differing != 0 || !shown_only_left.is_empty() || !shown_only_right.is_empty() { 1 } else { 0 })
}

/// The `pr-compare` entry point: `argv` without the program name; returns the exit status.
pub fn main(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match run(argv, out, err) {
        Ok(code) => code,
        Err(e) => {
            report(err, &e);
            1
        }
    }
}
