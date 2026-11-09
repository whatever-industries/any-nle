//! `pr-validate` (`cli/validate.py`): compare the parsed model against the ExtendScript ground
//! truth JSON exported by `scripts/jsx/export_project_json.jsx` (the DOM values Premiere itself
//! reports for the same file).
//!
//! ```text
//! prproj-validate file.prproj                (expects file.json)
//! prproj-validate file.prproj --json gt.json
//! prproj-validate a.prproj b.prproj c.prproj [--report-json R] [--coverage]
//! ```
//!
//! The ground truth is read with `json.loads` semantics (`convert::model::parse_json_python`).
//! Values the Python code compares with `==`/`repr` are modelled as [`Json`], which carries
//! Python's int/float/bool distinctions; ground truth of an exotic shape (a list where an
//! object is expected, ...) raises the same exception class as Python for the common cases.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use super::args::{self, Kind, Opt, Outcome, Spec};
use super::{os_error, path_name, path_str, report, say, with_suffix, PyErr, PyResult};
use crate::build::fmt::py_repr;
use crate::build::time::{Time, UNSET_TICKS};
use crate::convert::model::{parse_json_python, py_str_repr, Json};
use crate::model::application;
use crate::model::component::{
    param_class_id, param_display_name, param_is_time_varying, param_keys, param_value, param_value_at_key, ParamValue,
    VERIFIED_VALUE_CLASS_IDS,
};
use crate::model::marker as mk;
use crate::model::project::py_int;
use crate::model::{ItemId, Project, SequenceId, TrackId, TrackItemId};
use crate::xml::NodeId;

pub const SPEC: Spec = Spec {
    prog: "prproj-validate",
    usage: "usage: prproj-validate [-h] [--json JSON] [--report-json REPORT_JSON] [--coverage]\n                       files [files ...]",
    help: "Validate parsed output against ExtendScript ground-truth JSON.

positional arguments:
  files                 prproj file(s) to validate

options:
  -h, --help            show this help message and exit
  --json JSON           Ground-truth JSON (single file mode only; default:
                        sibling .json)
  --report-json REPORT_JSON
                        Write a machine-readable report (per file: status,
                        mismatches, asserted/ignored ground-truth keys) to
                        this path
  --coverage            Also print which ground-truth keys were never asserted

Ground truth is exported by scripts/jsx/export_project_json.jsx (run it via
scripts/run_in_ppro.ps1). By default <file>.json next to each .prproj is used.",
    opts: &[
        Opt { long: "--json", kind: Kind::Text },
        Opt { long: "--report-json", kind: Kind::Text },
        Opt { long: "--coverage", kind: Kind::Flag },
    ],
    positionals: &[("files", true)],
};

/// Ground-truth values that are NOT derivable from the project file (documented ES
/// divergences): (fixture, path, label, reason).
const KNOWN_DIVERGENCES: [(&str, &str, &str, &str); 8] = [
    (
        "34_generators",
        "rootItem/[5]",
        "colorLabel",
        "nothing stored; ES falls back to the live BE.Prefs.LabelDefaults.Still (3)",
    ),
    (
        "34_generators",
        "rootItem/[6]",
        "colorLabel",
        "nothing stored; ES falls back to BE.Prefs.LabelDefaults.AV (1)",
    ),
    (
        "34_generators",
        "rootItem/[8]",
        "outPoint",
        "IsContinuousTime media; ES asks the importer, the file stores 6 frames",
    ),
    (
        "48_smart_bin",
        "rootItem/[4]",
        "colorLabel",
        "nothing stored; ES falls back to BE.Prefs.LabelDefaults.Bin (7)",
    ),
    (
        "49_seq_settings",
        "sequences[1] ('Seq B')",
        "videoDisplayFormat",
        "110 (23.976 TC) is illegal for a 29.97 sequence; Premiere reverts to 102 on load while the file keeps 110",
    ),
    (
        "54_display_format",
        "sequences[1] ('Seq B')",
        "videoDisplayFormat",
        "same rejection, and here ExtendScript ITSELF wrote the 110 - the audioDisplayFormat 201 written in the same call does survive",
    ),
    (
        "51_ae_comp",
        "rootItem/[4]",
        "isOffline",
        "dynamic link reports offline until After Effects has rendered it; the file stores no OfflineReason",
    ),
    (
        "62_start_time",
        "rootItem/[0]",
        "startTime",
        "the file keeps the setStartTime AlternateStart (0x2h) but once media linking settles ES re-derives startTime from the embedded 1h timecode - an export of the same file BEFORE settle reports the stored value",
    ),
];

fn known_divergence(fixture: &str, path: &str, label: &str) -> Option<&'static str> {
    KNOWN_DIVERGENCES.iter().find(|(f, p, l, _)| *f == fixture && *p == path && *l == label).map(|e| e.3)
}

// ---- shape keys (the `re.sub` rules) -------------------------------------------------------------

/// `[^X]*` then `X`: the length of `'...'` starting at `at` (a quote), or `None`.
fn quoted_len(s: &[char], at: usize) -> Option<usize> {
    s[at + 1..].iter().position(|&c| c == '\'').map(|n| n + 2)
}

/// `\[\d+\]` at `at`: its length.
fn index_len(s: &[char], at: usize) -> Option<usize> {
    if s.get(at) != Some(&'[') {
        return None;
    }
    let digits = s[at + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
    (digits > 0 && s.get(at + 1 + digits) == Some(&']')).then_some(digits + 2)
}

fn starts_with(s: &[char], at: usize, literal: &str) -> bool {
    literal.chars().enumerate().all(|(k, c)| s.get(at + k) == Some(&c))
}

/// `re.sub(pattern, ...)` with a hand-written matcher: `matcher(chars, i)` returns the match
/// length and the replacement.
fn sub(text: &str, matcher: impl Fn(&[char], usize) -> Option<(usize, String)>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match matcher(&chars, i) {
            Some((len, replacement)) if len > 0 => {
                out.push_str(&replacement);
                i += len;
            }
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// `shape_key`: collapse a display path into the ground-truth key shape it asserts.
pub fn shape_key(path: &str, label: &str) -> String {
    // ^sequences\[\d+\] \([^)]*\)  ->  sequences[]
    let mut shape = path.to_string();
    {
        let chars: Vec<char> = shape.chars().collect();
        if starts_with(&chars, 0, "sequences") {
            if let Some(n) = index_len(&chars, 9) {
                let at = 9 + n;
                if starts_with(&chars, at, " (") {
                    if let Some(close) = chars[at + 2..].iter().position(|&c| c == ')') {
                        let rest: String = chars[at + 2 + close + 1..].iter().collect();
                        shape = format!("sequences[]{rest}");
                    }
                }
            }
        }
    }
    // /component '[^']*'  ->  /components[]
    shape = sub(&shape, |s, i| {
        if starts_with(s, i, "/component '") {
            quoted_len(s, i + 11).map(|n| (11 + n, "/components[]".to_string()))
        } else {
            None
        }
    });
    // /marker '[^']*'  ->  /markers[]
    shape = sub(&shape, |s, i| {
        if starts_with(s, i, "/marker '") {
            quoted_len(s, i + 8).map(|n| (8 + n, "/markers[]".to_string()))
        } else {
            None
        }
    });
    // /'[^']*'  ->  /params[]
    shape = sub(&shape, |s, i| {
        if starts_with(s, i, "/'") {
            quoted_len(s, i + 1).map(|n| (1 + n, "/params[]".to_string()))
        } else {
            None
        }
    });
    // /clips\[\d+\]  ->  /clips[]
    shape = sub(&shape, |s, i| {
        if starts_with(s, i, "/clips") {
            index_len(s, i + 6).map(|n| (6 + n, "/clips[]".to_string()))
        } else {
            None
        }
    });
    // /(videoTracks|audioTracks)\[\d+\]  ->  /\1[]
    shape = sub(&shape, |s, i| {
        for name in ["videoTracks", "audioTracks"] {
            if starts_with(s, i, &format!("/{name}")) {
                let at = i + 1 + name.len();
                if let Some(n) = index_len(s, at) {
                    return Some((1 + name.len() + n, format!("/{name}[]")));
                }
            }
        }
        None
    });
    // /\[\d+\]  ->  /children[]
    shape = sub(&shape, |s, i| {
        if s[i] == '/' {
            index_len(s, i + 1).map(|n| (1 + n, "/children[]".to_string()))
        } else {
            None
        }
    });
    if shape == "project" {
        return label.to_string();
    }
    format!("{}.{label}", shape.replace('/', "."))
}

fn covered(key: &str, asserted: &BTreeSet<String>) -> bool {
    if asserted.contains(key) {
        return true;
    }
    let parts: Vec<&str> = key.split('.').collect();
    (1..parts.len()).any(|index| asserted.contains(&parts[..index].join(".")))
}

fn flatten_keys(node: &Json, prefix: &str, found: &mut BTreeSet<String>) {
    match node {
        Json::Obj(pairs) => {
            for (key, value) in pairs {
                let child = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
                if matches!(value, Json::Obj(_) | Json::Arr(_)) {
                    flatten_keys(value, &child, found);
                } else {
                    found.insert(child);
                }
            }
        }
        Json::Arr(entries) => {
            for entry in entries {
                flatten_keys(entry, &format!("{prefix}[]"), found);
            }
        }
        _ => {}
    }
}

// ---- Python value semantics over Json ------------------------------------------------------------

static NULL: Json = Json::Null;

fn type_name(v: &Json) -> &'static str {
    match v {
        Json::Null => "NoneType",
        Json::Bool(_) => "bool",
        Json::Int(_) | Json::Big(_) => "int",
        Json::Float(_) => "float",
        Json::Str(_) => "str",
        Json::Arr(_) => "list",
        Json::Obj(_) => "dict",
    }
}

fn s(x: &str) -> Json {
    Json::Str(x.to_string())
}

fn i(x: impl Into<i128>) -> Json {
    Json::Int(x.into())
}

fn oi(x: Option<i64>) -> Json {
    x.map_or(Json::Null, |v| Json::Int(v as i128))
}

fn os(x: Option<String>) -> Json {
    x.map_or(Json::Null, Json::Str)
}

/// `repr(value)`.
fn repr(v: &Json) -> String {
    match v {
        Json::Null => "None".into(),
        Json::Bool(true) => "True".into(),
        Json::Bool(false) => "False".into(),
        Json::Int(n) => n.to_string(),
        Json::Big(neg) => format!("<{}big int>", if *neg { "-" } else { "" }),
        Json::Float(f) => py_repr(*f),
        Json::Str(t) => py_str_repr(t),
        Json::Arr(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Json::Obj(pairs) => {
            format!("{{{}}}", pairs.iter().map(|(k, v)| format!("{}: {}", py_str_repr(k), repr(v))).collect::<Vec<_>>().join(", "))
        }
    }
}

enum Number {
    Int(i128),
    Float(f64),
}

fn number(v: &Json) -> Option<Number> {
    match v {
        Json::Bool(b) => Some(Number::Int(*b as i128)),
        Json::Int(n) => Some(Number::Int(*n)),
        Json::Float(f) => Some(Number::Float(*f)),
        _ => None,
    }
}

fn int_eq_float(n: i128, f: f64) -> bool {
    f.is_finite() && f.fract() == 0.0 && f.abs() < 1.0e38 && f as i128 == n
}

/// Python `a == b` over decoded JSON values.
fn py_eq(a: &Json, b: &Json) -> bool {
    if let (Some(x), Some(y)) = (number(a), number(b)) {
        return match (x, y) {
            (Number::Int(p), Number::Int(q)) => p == q,
            (Number::Float(p), Number::Float(q)) => p == q,
            (Number::Int(p), Number::Float(q)) | (Number::Float(q), Number::Int(p)) => int_eq_float(p, q),
        };
    }
    match (a, b) {
        (Json::Null, Json::Null) => true,
        (Json::Str(x), Json::Str(y)) => x == y,
        (Json::Arr(x), Json::Arr(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Json::Obj(x), Json::Obj(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().find(|(k2, _)| k2 == k).is_some_and(|(_, v2)| py_eq(v, v2)))
        }
        _ => false,
    }
}

fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Int(n) => *n != 0,
        Json::Big(_) => true,
        Json::Float(f) => *f != 0.0,
        Json::Str(t) => !t.is_empty(),
        Json::Arr(a) => !a.is_empty(),
        Json::Obj(o) => !o.is_empty(),
    }
}

/// `expected.get(key)` (absent -> None).
fn get<'a>(v: &'a Json, key: &str) -> PyResult<&'a Json> {
    match v {
        Json::Obj(_) => Ok(v.get(key).unwrap_or(&NULL)),
        other => Err(PyErr::new("AttributeError", format!("'{}' object has no attribute 'get'", type_name(other)))),
    }
}

/// `expected.get(key, default)`.
fn get_or<'a>(v: &'a Json, key: &str, default: &'a Json) -> PyResult<&'a Json> {
    match v {
        Json::Obj(_) => Ok(v.get(key).unwrap_or(default)),
        other => Err(PyErr::new("AttributeError", format!("'{}' object has no attribute 'get'", type_name(other)))),
    }
}

/// `key in expected`.
fn has(v: &Json, key: &str) -> bool {
    v.get(key).is_some()
}

/// `expected[key]`.
fn index<'a>(v: &'a Json, key: &str) -> PyResult<&'a Json> {
    match v {
        Json::Obj(_) => v.get(key).ok_or_else(|| PyErr::new("KeyError", py_str_repr(key))),
        Json::Arr(_) => Err(PyErr::new("TypeError", "list indices must be integers or slices, not str")),
        Json::Str(_) => Err(PyErr::new("TypeError", "string indices must be integers, not 'str'")),
        other => Err(PyErr::new("TypeError", format!("'{}' object is not subscriptable", type_name(other)))),
    }
}

/// `int(value)`.
fn to_int(v: &Json) -> PyResult<i128> {
    match v {
        Json::Int(n) => Ok(*n),
        Json::Bool(b) => Ok(*b as i128),
        Json::Float(f) if f.is_nan() => Err(PyErr::new("ValueError", "cannot convert float NaN to integer")),
        Json::Float(f) if f.is_infinite() => Err(PyErr::new("OverflowError", "cannot convert float infinity to integer")),
        Json::Float(f) => Ok(f.trunc() as i128),
        Json::Str(t) => py_int(t).map(i128::from).map_err(PyErr::from),
        other => Err(PyErr::new(
            "TypeError",
            format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", type_name(other)),
        )),
    }
}

/// `float(value)`.
fn to_float(v: &Json) -> PyResult<f64> {
    match v {
        Json::Int(n) => Ok(*n as f64),
        Json::Bool(b) => Ok(*b as i128 as f64),
        Json::Float(f) => Ok(*f),
        Json::Str(t) => crate::pystr::py_strip(t)
            .replace('_', "")
            .parse::<f64>()
            .map_err(|_| PyErr::new("ValueError", format!("could not convert string to float: {}", py_str_repr(t)))),
        other => Err(PyErr::new(
            "TypeError",
            format!("float() argument must be a string or a real number, not '{}'", type_name(other)),
        )),
    }
}

/// `len(value)`.
fn py_len(v: &Json) -> PyResult<usize> {
    match v {
        Json::Arr(a) => Ok(a.len()),
        Json::Obj(o) => Ok(o.len()),
        Json::Str(t) => Ok(t.chars().count()),
        other => Err(PyErr::new("TypeError", format!("object of type '{}' has no len()", type_name(other)))),
    }
}

/// `list(value)` (iteration).
fn py_iter(v: &Json) -> PyResult<Vec<Json>> {
    match v {
        Json::Arr(a) => Ok(a.clone()),
        Json::Obj(o) => Ok(o.iter().map(|(k, _)| Json::Str(k.clone())).collect()),
        Json::Str(t) => Ok(t.chars().map(|c| Json::Str(c.to_string())).collect()),
        other => Err(PyErr::new("TypeError", format!("'{}' object is not iterable", type_name(other)))),
    }
}

// ---- Problems ------------------------------------------------------------------------------------

#[derive(Default)]
struct Problems {
    list: Vec<String>,
    asserted: BTreeSet<String>,
    divergences: Vec<String>,
    fixture: String,
}

impl Problems {
    fn note(&mut self, path: &str, label: &str) {
        self.asserted.insert(shape_key(path, label));
    }

    /// File a mismatch, unless it is a documented ES divergence.
    fn record(&mut self, path: &str, label: &str, message: String) {
        match known_divergence(&self.fixture, path, label) {
            None => self.list.push(message),
            Some(reason) => self.divergences.push(format!("{message}  [{reason}]")),
        }
    }

    fn append(&mut self, message: String) {
        self.list.push(message);
    }
}

fn equal(actual: &Json, expected: &Json) -> bool {
    if py_eq(actual, expected) {
        return true;
    }
    // ExtendScript's JSON writer rounds a float to 15 significant digits.
    if let Json::Float(a) = actual {
        if let Some(e) = number(expected) {
            let e = match e {
                Number::Int(n) => n as f64,
                Number::Float(f) => f,
            };
            let scale = a.abs().max(e.abs()).max(1.0);
            return (a - e).abs() <= 1e-12 * scale;
        }
    }
    false
}

fn check(problems: &mut Problems, path: &str, label: &str, actual: Json, expected: &Json) {
    problems.note(path, label);
    if equal(&actual, expected) {
        return;
    }
    let message = format!("{path}: {label} {} != expected {}", repr(&actual), repr(expected));
    problems.record(path, label, message);
}

fn normalize_media_path(v: Option<&str>) -> String {
    v.map_or(String::new(), |t| t.replace('\\', "/"))
}

fn ticks_or_unset(t: Option<Time>) -> i128 {
    t.map_or(UNSET_TICKS, |t| t.ticks) as i128
}

fn validate_time(problems: &mut Problems, path: &str, label: &str, actual: Time, expected: &Json) -> PyResult<()> {
    check(problems, path, label, i(actual.ticks), &Json::Int(to_int(index(expected, "ticks")?)?));
    Ok(())
}

fn param_json(value: &ParamValue) -> Json {
    match value {
        ParamValue::Float(f) => Json::Float(*f),
        ParamValue::Bool(b) => Json::Bool(*b),
        ParamValue::List(l) => Json::Arr(l.iter().map(|&f| Json::Float(f)).collect()),
        ParamValue::Str(t) => Json::Str(t.clone()),
    }
}

/// `_values_close`.
fn values_close(actual: &Json, expected: &Json) -> bool {
    if matches!(actual, Json::Bool(_)) || matches!(expected, Json::Bool(_)) {
        return matches!((actual, expected), (Json::Bool(a), Json::Bool(b)) if a == b);
    }
    if let (Some(a), Some(e)) = (number(actual), number(expected)) {
        let f = |n: Number| match n {
            Number::Int(n) => n as f64,
            Number::Float(f) => f,
        };
        let (a, e) = (f(a), f(e));
        return (a - e).abs() <= 1e-6 * 1.0f64.max(a.abs()).max(e.abs());
    }
    if let (Json::Arr(a), Json::Arr(e)) = (actual, expected) {
        return a.len() == e.len() && a.iter().zip(e).all(|(x, y)| values_close(x, y));
    }
    py_eq(actual, expected)
}

fn validate_item(problems: &mut Problems, p: &Project, path: &str, item: ItemId, expected: &Json) -> PyResult<()> {
    check(problems, path, "name", s(&p.item_name(item)), get(expected, "name")?);
    check(problems, path, "type", i(p.item_type(item).value()), get(expected, "type")?);
    if has(expected, "treePath") {
        check(problems, path, "treePath", s(&p.tree_path(item)), index(expected, "treePath")?);
    }
    if has(expected, "isOffline") {
        check(problems, path, "isOffline", Json::Bool(p.is_offline(item)?), &Json::Bool(truthy(index(expected, "isOffline")?)));
    }
    if has(expected, "colorLabel") {
        check(problems, path, "colorLabel", i(p.color_label(item)), index(expected, "colorLabel")?);
    }
    if has(expected, "startTime") {
        let actual = p.item_start_time(item)?.ticks;
        check(problems, path, "startTime", i(actual), &Json::Int(to_int(index(index(expected, "startTime")?, "ticks")?)?));
    }
    let flags = [
        ("isAdjustmentLayer", p.is_adjustment_layer(item)?),
        ("isMergedClip", p.is_merged_clip(item)?),
        ("isMulticamClip", p.is_multicam_clip(item)?),
    ];
    for (key, actual) in flags {
        if has(expected, key) {
            check(problems, path, key, Json::Bool(actual), &Json::Bool(truthy(index(expected, key)?)));
        }
    }
    if has(expected, "hasProxy") {
        check(problems, path, "hasProxy", Json::Bool(p.has_proxy(item)?), &Json::Bool(truthy(index(expected, "hasProxy")?)));
    }
    if has(expected, "proxyPath") {
        // ExtendScript reports no proxy as an empty path, py as None.
        let actual = normalize_media_path(p.proxy_path(item)?.as_deref());
        let expected_path = match index(expected, "proxyPath")? {
            Json::Str(t) => normalize_media_path(Some(t)),
            _ => String::new(),
        };
        check(problems, path, "proxyPath", Json::Str(actual), &Json::Str(expected_path));
    }
    if has(expected, "inPoint") {
        let actual = ticks_or_unset(p.item_in_point(item)?);
        check(problems, path, "inPoint", Json::Int(actual), &Json::Int(to_int(index(index(expected, "inPoint")?, "ticks")?)?));
    }
    if has(expected, "outPoint") {
        let actual = ticks_or_unset(p.item_out_point(item)?);
        check(problems, path, "outPoint", Json::Int(actual), &Json::Int(to_int(index(index(expected, "outPoint")?, "ticks")?)?));
    }
    if has(expected, "mediaPath") {
        let actual = normalize_media_path(p.item_media_path(item));
        let want = match get(expected, "mediaPath")? {
            Json::Str(t) => normalize_media_path(Some(t)),
            _ => String::new(),
        };
        check(problems, path, "mediaPath", Json::Str(actual), &Json::Str(want));
    }
    let interpretation = p.footage_interpretation(item)?;
    if let (true, Some(fi)) = (has(expected, "interpretation"), interpretation) {
        let tree = &p.doc.tree;
        let want = index(expected, "interpretation")?;
        check(problems, path, "interpretation.alphaUsage", i(fi.alpha_usage(tree)?.value()), &Json::Int(to_int(index(want, "alphaUsage")?)?));
        check(problems, path, "interpretation.ignoreAlpha", Json::Bool(fi.ignore_alpha(tree)), &Json::Bool(truthy(index(want, "ignoreAlpha")?)));
        check(problems, path, "interpretation.invertAlpha", Json::Bool(fi.invert_alpha(tree)), &Json::Bool(truthy(index(want, "invertAlpha")?)));
        check(problems, path, "interpretation.fieldType", i(fi.field_type(tree)?.value()), &Json::Int(to_int(index(want, "fieldType")?)?));
        let rate = fi.frame_rate(tree)?;
        if rate.ticks != 0 {
            let actual = round6(1.0 / rate.seconds());
            let wanted = round6(to_float(index(want, "frameRate")?)?);
            check(problems, path, "interpretation.frameRate", Json::Float(actual), &Json::Float(wanted));
        }
        check(
            problems,
            path,
            "interpretation.pixelAspectRatio",
            Json::Float(fi.pixel_aspect_ratio(tree)?),
            &Json::Float(to_float(index(want, "pixelAspectRatio")?)?),
        );
    }
    let expected_children = get_or(expected, "children", &EMPTY_LIST)?;
    let children = p.children(item);
    check(problems, path, "child count", i(children.len() as i64), &Json::Int(py_len(expected_children)? as i128));
    for (n, (&child, expected_child)) in children.iter().zip(py_iter(expected_children)?).enumerate() {
        validate_item(problems, p, &format!("{path}/[{n}]"), child, &expected_child)?;
    }
    Ok(())
}

static EMPTY_LIST: Json = Json::Arr(Vec::new());

/// Python `round(x, 6)`.
fn round6(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.6}").parse().unwrap_or(x)
}

fn validate_param_values(problems: &mut Problems, p: &Project, param_path: &str, param: NodeId, expected: &Json) -> PyResult<()> {
    let tree = &p.doc.tree;
    let varying = param_is_time_varying(tree, param);
    check(problems, param_path, "isTimeVarying", Json::Bool(varying), get(expected, "isTimeVarying")?);
    // Value semantics are asserted only for ClassIDs whose decoding is corpus-verified.
    if !VERIFIED_VALUE_CLASS_IDS.contains(&param_class_id(tree, param).as_str()) {
        return Ok(());
    }
    problems.note(param_path, "value");
    problems.note(param_path, "keys[]");
    if !varying {
        let value = param_value(tree, param)?;
        let expected_value = get(expected, "value")?;
        if let Some(value) = value {
            let actual = param_json(&value);
            if !values_close(&actual, expected_value) {
                problems.append(format!("{param_path}: value {} != expected {}", repr(&actual), repr(expected_value)));
            }
        }
        return Ok(());
    }
    let expected_keys = get_or(expected, "keys", &EMPTY_LIST)?;
    let keys = param_keys(tree, param)?;
    check(problems, param_path, "key count", i(keys.len() as i64), &Json::Int(py_len(expected_keys)? as i128));
    for (key_time, expected_key) in keys.iter().zip(py_iter(expected_keys)?) {
        let expected_ticks = to_int(index(index(&expected_key, "time")?, "ticks")?)?;
        if key_time.ticks as i128 != expected_ticks {
            problems.append(format!("{param_path}: key at {} != expected {expected_ticks}", key_time.ticks));
            continue;
        }
        let value = param_value_at_key(tree, param, *key_time)?;
        let expected_value = get(&expected_key, "value")?;
        if let Some(value) = value {
            let actual = param_json(&value);
            if !values_close(&actual, expected_value) {
                problems.append(format!(
                    "{param_path}: value at key {} {} != expected {}",
                    key_time.ticks,
                    repr(&actual),
                    repr(expected_value)
                ));
            }
        }
    }
    Ok(())
}

fn validate_components(problems: &mut Problems, p: &Project, path: &str, clip: TrackItemId, expected: &Json) -> PyResult<()> {
    let mut remaining = py_iter(expected)?;
    for &component in p.components(clip).items() {
        let match_name = p.component_match_name(component);
        let component_path = format!("{path}/component {}", py_str_repr(&match_name));
        problems.note(&component_path, "matchName");
        let mut entry = None;
        for (n, candidate) in remaining.iter().enumerate() {
            if py_eq(get(candidate, "matchName")?, &s(&match_name)) {
                entry = Some(candidate.clone());
                remaining.drain(..=n);
                break;
            }
        }
        let Some(entry) = entry else {
            problems.append(format!("{component_path}: not reported by ExtendScript"));
            continue;
        };
        check(problems, &component_path, "matchName", s(&match_name), get(&entry, "matchName")?);
        let display_name = p.component_display_name(component);
        if !display_name.is_empty() {
            // Audio components store no display name; only matchName is comparable there.
            check(problems, &component_path, "displayName", s(&display_name), get(&entry, "displayName")?);
        }
        let expected_params = py_iter(get_or(&entry, "params", &EMPTY_LIST)?)?;
        let mut pointer = 0;
        for &param in p.component_properties(component).items() {
            let name = param_display_name(&p.doc.tree, param);
            let mut expected_param = None;
            for (k, candidate) in expected_params.iter().enumerate().skip(pointer) {
                if py_eq(get(candidate, "displayName")?, &s(&name)) {
                    expected_param = Some(candidate);
                    pointer = k + 1;
                    break;
                }
            }
            // Stored but not scriptable: nothing to compare against.
            let Some(expected_param) = expected_param else { continue };
            let param_path = format!("{component_path}/{}", py_str_repr(&name));
            problems.note(&param_path, "displayName");
            validate_param_values(problems, p, &param_path, param, expected_param)?;
        }
    }
    Ok(())
}

fn validate_track(problems: &mut Problems, p: &Project, path: &str, track: TrackId, expected: &Json) -> PyResult<()> {
    check(problems, path, "id", i(p.track_id(track)), get(expected, "id")?);
    check(problems, path, "mediaType", s(p.track_media_type(track).as_str()), get(expected, "mediaType")?);
    if has(expected, "name") {
        check(problems, path, "name", s(&p.track_name(track)), index(expected, "name")?);
    }
    if has(expected, "isMuted") {
        check(problems, path, "isMuted", Json::Bool(p.is_muted(track)?), &Json::Bool(truthy(index(expected, "isMuted")?)));
    }
    if has(expected, "isLocked") {
        check(problems, path, "isLocked", Json::Bool(p.is_locked(track)?), &Json::Bool(truthy(index(expected, "isLocked")?)));
    }
    let expected_clips = get_or(expected, "clips", &EMPTY_LIST)?;
    let clips = p.track_clips(track);
    check(problems, path, "clip count", i(clips.len() as i64), &Json::Int(py_len(expected_clips)? as i128));
    for (n, (&clip, expected_clip)) in clips.iter().zip(py_iter(expected_clips)?).enumerate() {
        let clip_path = format!("{path}/clips[{n}]");
        check(problems, &clip_path, "name", s(&p.track_item_name(clip)?), get(&expected_clip, "name")?);
        check(problems, &clip_path, "type", i(p.track_item_type(clip)), get(&expected_clip, "type")?);
        if has(&expected_clip, "clipMediaType") {
            check(problems, &clip_path, "clipMediaType", s(p.track_item_media_type(clip).as_str()), index(&expected_clip, "clipMediaType")?);
        }
        validate_time(problems, &clip_path, "start", p.start(clip)?, index(&expected_clip, "start")?)?;
        validate_time(problems, &clip_path, "end", p.end(clip)?, index(&expected_clip, "end")?)?;
        validate_time(problems, &clip_path, "inPoint", p.in_point(clip)?, index(&expected_clip, "inPoint")?)?;
        validate_time(problems, &clip_path, "outPoint", p.out_point(clip)?, index(&expected_clip, "outPoint")?)?;
        validate_time(problems, &clip_path, "duration", p.duration(clip)?, index(&expected_clip, "duration")?)?;
        validate_components(problems, p, &clip_path, clip, get_or(&expected_clip, "components", &EMPTY_LIST)?)?;
    }
    Ok(())
}

fn validate_markers(problems: &mut Problems, p: &Project, path: &str, markers: &[NodeId], expected: &Json) -> PyResult<()> {
    let tree = &p.doc.tree;
    check(problems, path, "marker count", i(markers.len() as i64), &Json::Int(py_len(expected)? as i128));
    // {m.get("guid"): m for m in expected}: a repeated guid keeps its first position, last value.
    let mut by_guid: Vec<(Json, Json)> = Vec::new();
    for m in py_iter(expected)? {
        let guid = get(&m, "guid")?.clone();
        match by_guid.iter_mut().find(|(k, _)| py_eq(k, &guid)) {
            Some(slot) => slot.1 = m,
            None => by_guid.push((guid, m)),
        }
    }
    for &marker in markers {
        let marker_path = format!("{path}/marker {}", py_str_repr(&mk::marker_name(tree, marker)?));
        problems.note(&marker_path, "guid"); // the matching key
        let guid = mk::marker_guid(tree, marker)?;
        let Some((_, entry)) = by_guid.iter().find(|(k, _)| py_eq(k, &s(&guid))) else {
            problems.append(format!("{marker_path}: guid {} not in ground truth", py_str_repr(&guid)));
            continue;
        };
        check(problems, &marker_path, "name", s(&mk::marker_name(tree, marker)?), get(entry, "name")?);
        check(problems, &marker_path, "comments", s(&mk::marker_comments(tree, marker)?), get(entry, "comments")?);
        check(problems, &marker_path, "type", s(&mk::marker_type(tree, marker)?), get(entry, "type")?);
        let start = mk::marker_start(tree, marker)?.ticks;
        check(problems, &marker_path, "start", i(start), &Json::Int(to_int(index(index(entry, "start")?, "ticks")?)?));
        let end = mk::marker_end(tree, marker)?.ticks;
        check(problems, &marker_path, "end", i(end), &Json::Int(to_int(index(index(entry, "end")?, "ticks")?)?));
        if has(entry, "webLinkURL") {
            check(problems, &marker_path, "webLinkURL", s(&mk::marker_web_link_url(tree, marker)?), index(entry, "webLinkURL")?);
        }
        if has(entry, "webLinkFrameTarget") {
            check(
                problems,
                &marker_path,
                "webLinkFrameTarget",
                s(&mk::marker_web_link_frame_target(tree, marker)?),
                index(entry, "webLinkFrameTarget")?,
            );
        }
        if has(entry, "colorIndex") {
            let actual = mk::marker_color_index(tree, marker)?.map(|n| n as i64);
            check(problems, &marker_path, "colorIndex", oi(actual), &Json::Int(to_int(index(entry, "colorIndex")?)?));
        }
    }
    Ok(())
}

fn validate_sequence(problems: &mut Problems, p: &Project, path: &str, sequence: SequenceId, expected: &Json) -> PyResult<()> {
    check(problems, path, "name", s(&p.sequence_name(sequence)), get(expected, "name")?);
    check(problems, path, "sequenceID", os(p.sequence_id(sequence)), get(expected, "sequenceID")?);
    let timebase = get(expected, "timebase")?;
    if !matches!(timebase, Json::Null) {
        check(problems, path, "timebase", oi(p.timebase(sequence)), &Json::Int(to_int(timebase)?));
    }
    validate_markers(problems, p, path, p.sequence_markers(sequence), get_or(expected, "markers", &EMPTY_LIST)?)?;
    let frame_size = p.frame_size(sequence);
    let (width, height) = (oi(frame_size.map(|f| f.0)), oi(frame_size.map(|f| f.1)));
    check(problems, path, "frameSizeHorizontal", width.clone(), get(expected, "frameSizeHorizontal")?);
    check(problems, path, "frameSizeVertical", height.clone(), get(expected, "frameSizeVertical")?);
    if has(expected, "end") {
        check(problems, path, "end", i(p.sequence_end(sequence)?.ticks), &Json::Int(to_int(index(expected, "end")?)?));
    }
    if has(expected, "zeroPoint") {
        let raw = index(expected, "zeroPoint")?;
        // Older exports serialized this as an (empty) object; skip those.
        let expected_zero = if matches!(raw, Json::Obj(_)) { get(raw, "ticks")? } else { raw };
        if !matches!(expected_zero, Json::Null) {
            check(problems, path, "zeroPoint", i(p.zero_point(sequence)?.ticks), &Json::Int(to_int(expected_zero)?));
        }
    }
    if has(expected, "workInPoint") {
        let actual = ticks_or_unset(p.sequence_in_point(sequence)?);
        check(problems, path, "workInPoint", Json::Int(actual), &Json::Int(to_int(index(index(expected, "workInPoint")?, "ticks")?)?));
    }
    if has(expected, "workOutPoint") {
        let actual = ticks_or_unset(p.sequence_out_point(sequence)?);
        check(problems, path, "workOutPoint", Json::Int(actual), &Json::Int(to_int(index(index(expected, "workOutPoint")?, "ticks")?)?));
    }
    if has(expected, "videoDisplayFormat") {
        check(problems, path, "videoDisplayFormat", oi(p.video_display_format(sequence)?), index(expected, "videoDisplayFormat")?);
    }
    if has(expected, "audioDisplayFormat") {
        check(problems, path, "audioDisplayFormat", oi(p.audio_display_format(sequence)?), index(expected, "audioDisplayFormat")?);
    }
    let settings = get(expected, "settings")?;
    if !matches!(settings, Json::Null) {
        check(problems, path, "settings.videoFrameWidth", width, get(settings, "videoFrameWidth")?);
        check(problems, path, "settings.videoFrameHeight", height, get(settings, "videoFrameHeight")?);
        check(
            problems,
            path,
            "settings.videoFrameRate",
            oi(p.timebase(sequence)),
            &Json::Int(to_int(index(index(settings, "videoFrameRate")?, "ticks")?)?),
        );
        check(
            problems,
            path,
            "settings.audioSampleRate",
            oi(p.audio_frame_rate(sequence)),
            &Json::Int(to_int(index(index(settings, "audioSampleRate")?, "ticks")?)?),
        );
        check(problems, path, "settings.audioChannelCount", oi(p.audio_channel_count(sequence)), get(settings, "audioChannelCount")?);
    }
    let groups = [
        ("videoTracks", p.video_tracks(sequence), get_or(expected, "videoTracks", &EMPTY_LIST)?),
        ("audioTracks", p.audio_tracks(sequence), get_or(expected, "audioTracks", &EMPTY_LIST)?),
    ];
    for (label, tracks, expected_tracks) in groups {
        check(problems, path, &format!("{label} count"), i(tracks.len() as i64), &Json::Int(py_len(expected_tracks)? as i128));
        for (n, (&track, expected_track)) in tracks.iter().zip(py_iter(expected_tracks)?).enumerate() {
            validate_track(problems, p, &format!("{path}/{label}[{n}]"), track, &expected_track)?;
        }
    }
    Ok(())
}

/// Python `read_text(encoding="utf-8")` + `json.loads`.
fn load_ground_truth(path: &str) -> PyResult<Json> {
    let bytes = std::fs::read(path).map_err(|e| PyErr::from(os_error(Path::new(path), &e)))?;
    let text = String::from_utf8(bytes).map_err(|e| {
        let u = e.utf8_error();
        PyErr::new(
            "UnicodeDecodeError",
            format!(
                "'utf-8' codec can't decode byte 0x{:02x} in position {}: invalid start byte",
                e.as_bytes()[u.valid_up_to()],
                u.valid_up_to()
            ),
        )
    })?;
    // Universal newlines.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    parse_json_python(&text).map_err(|e| PyErr::new("JSONDecodeError", e.to_string()))
}

/// `validate_file`: the mismatches between parse output and ground truth.
fn validate_file(prproj: &str, json_path: &str) -> PyResult<(Problems, Json)> {
    let application = application::parse(prproj, None)?;
    let expected = load_ground_truth(json_path)?;
    let project = application.project();
    let mut problems = Problems { fixture: stem(&path_name(prproj)), ..Default::default() };

    if has(&expected, "name") {
        check(&mut problems, "project", "name", s(&project.name()), index(&expected, "name")?);
    }
    if has(&expected, "documentID") {
        check(&mut problems, "project", "documentID", os(project.document_id()), index(&expected, "documentID")?);
    }
    // activeSequence is session UI state without a dedicated stored key, so it is not asserted.
    let expected_root = get(&expected, "rootItem")?;
    if !matches!(expected_root, Json::Null) {
        match project.root_item() {
            None => problems.append("rootItem: missing in parsed output".into()),
            Some(root) => validate_item(&mut problems, project, "rootItem", root, expected_root)?,
        }
    }
    let sequences = project.sequences();
    let expected_sequences = py_iter(get_or(&expected, "sequences", &EMPTY_LIST)?)?;
    check(&mut problems, "project", "sequence count", i(sequences.len() as i64), &Json::Int(expected_sequences.len() as i128));
    // {s.get("sequenceID"): s for s in expected_sequences if s.get("sequenceID")}
    let mut by_id: Vec<(Json, &Json)> = Vec::new();
    for entry in &expected_sequences {
        let id = get(entry, "sequenceID")?;
        if truthy(id) {
            match by_id.iter_mut().find(|(k, _)| py_eq(k, id)) {
                Some(slot) => slot.1 = entry,
                None => by_id.push((id.clone(), entry)),
            }
        }
    }
    for (n, &sequence) in sequences.iter().enumerate() {
        let sequence_id = os(project.sequence_id(sequence));
        let mut entry = by_id.iter().find(|(k, _)| py_eq(k, &sequence_id)).map(|(_, v)| *v);
        if entry.is_none() {
            match expected_sequences.get(n) {
                Some(e) => entry = Some(e),
                None => {
                    problems.append(format!("sequences[{n}] ({}): no ground-truth entry", py_str_repr(&project.sequence_name(sequence))));
                    continue;
                }
            }
        }
        let path = format!("sequences[{n}] ({})", py_str_repr(&project.sequence_name(sequence)));
        validate_sequence(&mut problems, project, &path, sequence, entry.unwrap())?;
    }
    Ok((problems, expected))
}

/// `Path.stem`.
fn stem(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

// ---- json.dumps(report, indent=1) ----------------------------------------------------------------

fn dumps_str(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn dumps(v: &Json, level: usize, out: &mut String) {
    let pad = " ".repeat(level + 1);
    match v {
        Json::Str(t) => out.push_str(&dumps_str(t)),
        Json::Arr(items) if items.is_empty() => out.push_str("[]"),
        Json::Arr(items) => {
            out.push_str("[\n");
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&pad);
                dumps(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(level));
            out.push(']');
        }
        Json::Obj(pairs) if pairs.is_empty() => out.push_str("{}"),
        Json::Obj(pairs) => {
            out.push_str("{\n");
            for (n, (k, item)) in pairs.iter().enumerate() {
                if n > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&pad);
                out.push_str(&dumps_str(k));
                out.push_str(": ");
                dumps(item, level + 1, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(level));
            out.push('}');
        }
        other => out.push_str(&repr(other)),
    }
}

fn str_list(items: impl IntoIterator<Item = String>) -> Json {
    Json::Arr(items.into_iter().map(Json::Str).collect())
}

fn set_entry(report: &mut Vec<(String, Json)>, name: String, entry: Json) {
    match report.iter_mut().find(|(k, _)| *k == name) {
        Some(slot) => slot.1 = entry,
        None => report.push((name, entry)),
    }
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> PyResult<i32> {
    let parsed = match args::parse(&SPEC, argv, out, err) {
        Outcome::Run(p) => p,
        Outcome::Exit(code) => return Ok(code),
    };
    let files = &parsed.positionals[0];
    let json_arg = parsed.text("--json");
    let report_path = parsed.text("--report-json");
    let coverage = parsed.flag("--coverage");
    if json_arg.is_some() && files.len() > 1 {
        say!(err, "Error: --json only applies to a single file");
        return Ok(2);
    }

    let mut failures = 0;
    let mut report_entries: Vec<(String, Json)> = Vec::new();
    for prproj in files {
        let json_path = match json_arg {
            Some(j) => path_str(j),
            None => with_suffix(prproj, ".json")?,
        };
        let name = path_name(prproj);
        if !Path::new(prproj).exists() {
            say!(err, "Error: File not found: {}", path_str(prproj));
            set_entry(&mut report_entries, name, obj(vec![("status", s("missing"))]));
            failures += 1;
            continue;
        }
        if !Path::new(&json_path).exists() {
            say!(out, "{name}: SKIP (no ground truth at {})", path_name(&json_path));
            set_entry(&mut report_entries, name, obj(vec![("status", s("skipped")), ("reason", s("no ground truth"))]));
            continue;
        }
        let (problems, ground_truth) = validate_file(prproj, &json_path)?;
        let mut ignored: Vec<String> = Vec::new();
        if coverage || report_path.is_some() {
            let mut keys = BTreeSet::new();
            flatten_keys(&ground_truth, "", &mut keys);
            ignored = keys.into_iter().filter(|key| !covered(key, &problems.asserted)).collect();
        }
        if !problems.list.is_empty() {
            failures += 1;
            say!(out, "{name}: {} mismatch(es)", problems.list.len());
            for problem in &problems.list {
                say!(out, "  {problem}");
            }
        } else {
            say!(out, "{name}: OK");
        }
        for divergence in &problems.divergences {
            // Never silent: a documented divergence is still reported, just not a failure.
            say!(out, "  known divergence: {divergence}");
        }
        if coverage {
            say!(out, "  asserted {} key shape(s); {} never asserted", problems.asserted.len(), ignored.len());
            for key in &ignored {
                say!(out, "    not asserted: {key}");
            }
        }
        set_entry(
            &mut report_entries,
            name,
            obj(vec![
                ("status", s(if problems.list.is_empty() { "ok" } else { "fail" })),
                ("mismatches", str_list(problems.list.clone())),
                ("divergences", str_list(problems.divergences.clone())),
                ("asserted", str_list(problems.asserted.iter().cloned())),
                ("ignored", str_list(ignored)),
            ]),
        );
    }
    if let Some(target) = report_path {
        let mut text = String::new();
        dumps(&Json::Obj(report_entries), 0, &mut text);
        std::fs::write(target, text).map_err(|e| PyErr::from(os_error(Path::new(target), &e)))?;
        say!(out, "report written to {}", path_str(target));
    }
    Ok(if failures != 0 { 1 } else { 0 })
}

/// The `pr-validate` entry point: `argv` without the program name; returns the exit status.
pub fn main(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match run(argv, out, err) {
        Ok(code) => code,
        Err(e) => {
            report(err, &e);
            1
        }
    }
}
