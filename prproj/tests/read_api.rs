//! py_premiere's read / object-model API against Python (`tools/expected_read.py`):
//!
//! - `dumps`: every corpus project parsed and described through the public API; the text must
//!   equal Python's dump line for line (`tests/data/read/dumps/*.txt.gz`).
//! - `preferences`, `validators`: the prefs reader and the validators' verdicts.
//! - `edits`: the operation lists of `tests/data/read/edits/*.json` replayed on their fixture
//!   (seeded UUIDs); each op's result, the dump after the edits and the saved gzip bytes must
//!   equal Python's.
//!
//! Regenerate the expected data with
//! `PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_read.py`.

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};

use common::{base, data, fixtures, Scratch, GZIP_BYTES_GUARANTEED};
use prproj::build::fmt::py_repr;
use prproj::build::time::Time;
use prproj::convert::model::{parse_json, py_str_repr, Json};
use prproj::model::application::{self, Application};
use prproj::model::color::Color;
use prproj::model::component::{
    param_class_id, param_color, param_display_name, param_find_next_key, param_find_nearest_key,
    param_find_previous_key, param_interpolation_at_key, param_is_time_varying, param_keys, param_out_interpolation_at_key,
    param_repr, param_spatial_tangents_at_key, param_value, param_value_at_key, ParamInput, ParamValue, PathVertex,
};
use prproj::model::enums::{AlphaUsage, BlendMode, KeyframeInterpolation, VideoFieldType};
use prproj::model::footage::ParInput;
use prproj::model::marker as mk;
use prproj::model::named_list::NamedList;
use prproj::model::preferences::Preferences;
use prproj::model::transition as tr;
use prproj::model::validators::{self as val, Num, PathRule};
use prproj::model::{ComponentId, ItemId, Project, SequenceId, TrackId, TrackItemId};
use prproj::xml::NodeId;
use prproj::{Error, Result, SeededUuids};

// ---- rendering (tools/expected_read.py `fmt`, `v`, `r`) --------------------------------------

fn err(e: &Error) -> String {
    format!("!{}: {}", e.py_class(), e).replace('\n', "\\n")
}

fn v<T, E: Into<Error>>(res: std::result::Result<T, E>, f: impl FnOnce(T) -> String) -> String {
    match res {
        Ok(x) => f(x),
        Err(e) => err(&e.into()),
    }
}

fn t(x: Time) -> String {
    format!("T{}", x.ticks)
}

fn ot(x: Option<Time>) -> String {
    x.map_or("None".into(), t)
}

fn b(x: bool) -> String {
    if x { "True" } else { "False" }.into()
}

fn s(x: &str) -> String {
    py_str_repr(x)
}

fn os(x: Option<String>) -> String {
    x.map_or("None".into(), |x| s(&x))
}

fn oi<T: std::fmt::Display>(x: Option<T>) -> String {
    x.map_or("None".into(), |x| x.to_string())
}

fn pair(x: Option<(i64, i64)>) -> String {
    x.map_or("None".into(), |(a, b)| format!("[{a},{b}]"))
}

fn color(c: Option<Color>) -> String {
    c.map_or("None".into(), |c| format!("Color({},{},{},{})", c.red, c.green, c.blue, c.alpha))
}

fn floats(xs: &[f64]) -> String {
    format!("[{}]", xs.iter().map(|x| py_repr(*x)).collect::<Vec<_>>().join(","))
}

fn pv(x: Option<ParamValue>) -> String {
    match x {
        None => "None".into(),
        Some(ParamValue::Float(f)) => py_repr(f),
        Some(ParamValue::Bool(v)) => b(v),
        Some(ParamValue::List(l)) => floats(&l),
        Some(ParamValue::Str(v)) => s(&v),
    }
}

fn interp(x: Option<KeyframeInterpolation>) -> String {
    x.map_or("None".into(), |i| format!("KeyframeInterpolation.{}", i.name()))
}

fn times(xs: &[Time]) -> String {
    format!("[{}]", xs.iter().map(|x| t(*x)).collect::<Vec<_>>().join(","))
}

fn vertex(p: &PathVertex) -> String {
    format!("PathVertex({})", [p.x, p.y, p.in_x, p.in_y, p.out_x, p.out_y, p.flag].map(py_repr).join(","))
}

// ---- the dump ----------------------------------------------------------------------------------

fn path_of(p: &Project, item: Option<ItemId>) -> String {
    item.map_or("None".into(), |i| s(&p.tree_path(i)))
}

fn dump_marker(out: &mut Vec<String>, indent: &str, p: &Project, m: NodeId) {
    let tree = &p.doc.tree;
    out.push(format!(
        "{indent}marker name={} comments={} start={} end={} type={} guid={} color_index={} url={} target={} repr={}",
        v(mk::marker_name(tree, m), |x| s(&x)),
        v(mk::marker_comments(tree, m), |x| s(&x)),
        v(mk::marker_start(tree, m), t),
        v(mk::marker_end(tree, m), t),
        v(mk::marker_type(tree, m), |x| s(&x)),
        v(mk::marker_guid(tree, m), |x| s(&x)),
        v(mk::marker_color_index(tree, m), oi),
        v(mk::marker_web_link_url(tree, m), |x| s(&x)),
        v(mk::marker_web_link_frame_target(tree, m), |x| s(&x)),
        v(mk::marker_repr(tree, m), |x| x),
    ));
}

fn dump_named<T: Copy + PartialEq>(out: &mut Vec<String>, indent: &str, label: &str, list: &NamedList<T>) {
    let position_of = |found: Option<T>| found.and_then(|f| list.index_of(f)).map_or("None".into(), |i: usize| i.to_string());
    for position in 0..list.len() {
        let names: Vec<String> = list.keys_of(position).iter().filter_map(|k| k.as_ref().ok().cloned()).collect();
        let hits: Vec<String> = names.iter().map(|n| format!("{}:{}", s(n), v(list.get(n), position_of))).collect();
        let all = names.iter().try_fold(true, |acc, n| list.contains_name(n).map(|c| acc && c));
        out.push(format!("{indent}{label}[{position}] {} in={}", hits.join(" "), v(all, b)));
    }
    out.push(format!(
        "{indent}{label} missing={} get={}",
        v(list.by_name("\u{0}missing").map(Some), position_of),
        v(list.get("nope"), position_of)
    ));
}

fn probe_times(p: &Project, param: NodeId) -> Vec<Time> {
    let keys = param_keys(&p.doc.tree, param).unwrap_or_default();
    let mut ticks: Vec<i64> = vec![0, -1];
    for k in keys.iter().take(6) {
        ticks.extend([k.ticks, k.ticks - 1, k.ticks + 1]);
    }
    ticks.sort();
    ticks.dedup();
    ticks.into_iter().map(Time::new).collect()
}

fn dump_param(out: &mut Vec<String>, indent: &str, p: &mut Project, param: NodeId) {
    let tree = &p.doc.tree;
    out.push(format!(
        "{indent}param {} class={} varying={} value={} color={} repr={}",
        s(&param_display_name(tree, param)),
        s(&param_class_id(tree, param)),
        b(param_is_time_varying(tree, param)),
        v(param_value(tree, param), pv),
        v(param_color(tree, param), color),
        param_repr(tree, param),
    ));
    let keys = param_keys(tree, param);
    out.push(format!("{indent}  keys={}", v(keys.clone(), |k| times(&k))));
    let text = p.param_text(param);
    let font = p.param_font_family(param);
    let path = p.param_path(param);
    out.push(format!(
        "{indent}  text={} font={} path={}",
        v(text, os),
        v(font, os),
        path.map_or("None".into(), |vs| format!("[{}]", vs.iter().map(vertex).collect::<Vec<_>>().join(",")))
    ));
    let tree = &p.doc.tree;
    for &key in keys.unwrap_or_default().iter().take(12) {
        out.push(format!(
            "{indent}  at {} value={} in={} out={} spatial={}",
            t(key),
            v(param_value_at_key(tree, param, key), pv),
            v(param_interpolation_at_key(tree, param, key), interp),
            v(param_out_interpolation_at_key(tree, param, key), interp),
            v(param_spatial_tangents_at_key(tree, param, key), spatial),
        ));
    }
    for time in probe_times(p, param) {
        out.push(format!(
            "{indent}  probe {} nearest={} near1={} next={} prev={} value={} in={} spatial={}",
            t(time),
            v(param_find_nearest_key(tree, param, time, None), ot),
            v(param_find_nearest_key(tree, param, time, Some(Time::new(1))), ot),
            v(param_find_next_key(tree, param, time), ot),
            v(param_find_previous_key(tree, param, time), ot),
            v(param_value_at_key(tree, param, time), pv),
            v(param_interpolation_at_key(tree, param, time), interp),
            v(param_spatial_tangents_at_key(tree, param, time), spatial),
        ));
    }
}

fn spatial(x: Option<prproj::model::component::SpatialTangents>) -> String {
    x.map_or("None".into(), |x| format!("SpatialTangents({})", [x.in_x, x.in_y, x.out_x, x.out_y].map(py_repr).join(",")))
}

fn dump_component(out: &mut Vec<String>, indent: &str, p: &mut Project, c: ComponentId) {
    out.push(format!(
        "{indent}component {} match={} len={} repr={}",
        s(&p.component_display_name(c)),
        s(&p.component_match_name(c)),
        p.component(c).properties.len(),
        p.component_repr(c)
    ));
    let properties = p.component_properties(c);
    for &param in properties.items() {
        let name = param_display_name(&p.doc.tree, param);
        let found = properties.get(&name).unwrap().and_then(|f| properties.index_of(f));
        out.push(format!("{indent}  named {} -> {}", s(&name), oi(found)));
        dump_param(out, &format!("{indent}  "), p, param);
    }
    for sub in p.component(c).sub_components.clone() {
        dump_component(out, &format!("{indent}  sub "), p, sub);
    }
}

fn dump_clip(out: &mut Vec<String>, indent: &str, p: &mut Project, c: TrackItemId) {
    out.push(format!(
        "{indent}clip name={} start={} end={} duration={} in={} out={} speed={} reversed={} interp={} disabled={} media={} type={} source={} es_tag={} blend={} repr={}",
        v(p.track_item_name(c), |x| s(&x)),
        v(p.start(c), t),
        v(p.end(c), t),
        v(p.duration(c), t),
        v(p.in_point(c), t),
        v(p.out_point(c), t),
        v(p.speed(c), py_repr),
        v(p.is_speed_reversed(c), b),
        v(p.time_interpolation_type(c), |x| format!("TimeInterpolationType.{}", x.name())),
        v(p.is_disabled(c), b),
        s(p.track_item_media_type(c).as_str()),
        p.track_item_type(c),
        path_of(p, p.track_item_source(c)),
        v(p.essential_sound_tag(c), os),
        v(p.blend_mode(c), |x| format!("BlendMode.{}", x.name())),
        v(p.track_item_repr(c), |x| x),
    ));
    let components = p.components(c);
    dump_named(out, &format!("{indent}  "), "components", &components);
    for &component in components.items() {
        dump_component(out, &format!("{indent}  "), p, component);
    }
    for component in p.track_item(c).selection_components.clone() {
        dump_component(out, &format!("{indent}  selection "), p, component);
    }
    if let Ok(Some(remap)) = p.time_remapping(c) {
        dump_param(out, &format!("{indent}  remap "), p, remap);
    }
}

fn dump_transition(out: &mut Vec<String>, indent: &str, p: &Project, x: NodeId) {
    let tree = &p.doc.tree;
    out.push(format!(
        "{indent}transition name={} match={} start={} end={} duration={} offset={} cut={} border={} border_color={} reversed={} direction={} aa={} incoming={} outgoing={} repr={}",
        v(tr::transition_name(tree, x), |x| s(&x)),
        v(tr::transition_match_name(tree, x), |x| s(&x)),
        v(tr::transition_start(tree, x), t),
        v(tr::transition_end(tree, x), t),
        v(tr::transition_duration(tree, x), t),
        v(tr::transition_cut_point_offset(tree, x), t),
        v(tr::transition_cut_point(tree, x), t),
        v(tr::transition_border_width(tree, x), |x| x.to_string()),
        v(tr::transition_border_color(tree, x), color),
        v(tr::transition_is_reversed(tree, x), b),
        v(tr::transition_direction(tree, x), |x| x.to_string()),
        v(tr::transition_anti_alias_quality(tree, x), |x| x.to_string()),
        v(tr::transition_has_incoming_clip(tree, x), b),
        v(tr::transition_has_outgoing_clip(tree, x), b),
        v(tr::transition_repr(tree, x), |x| x),
    ));
}

fn dump_track(out: &mut Vec<String>, p: &mut Project, track: TrackId) {
    out.push(format!(
        "  track {} index={} id={} media={} muted={} locked={} sync={} len={} repr={}",
        s(&p.track_name(track)),
        p.track_index(track),
        p.track_id(track),
        s(p.track_media_type(track).as_str()),
        v(p.is_muted(track), b),
        v(p.is_locked(track), b),
        v(p.is_sync_locked(track), b),
        p.track_clips(track).len(),
        p.track_repr(track)
    ));
    let clips = p.track_clips_named(track);
    dump_named(out, "    ", "clips", &clips);
    for &clip in clips.items() {
        dump_clip(out, "    ", p, clip);
    }
    let transitions = p.transitions_named(track);
    dump_named(out, "    ", "transitions", &transitions);
    for &x in transitions.items() {
        dump_transition(out, "    ", p, x);
    }
}

fn dump_sequence(out: &mut Vec<String>, p: &mut Project, q: SequenceId) {
    out.push(format!(
        "sequence name={} id={} timebase={} frame={} audio_rate={} channels={} vdisplay={} adisplay={} end={} zero={} in={} out={} work_in={} work_out={} playhead={} item={} repr={}",
        s(&p.sequence_name(q)),
        os(p.sequence_id(q)),
        oi(p.timebase(q)),
        pair(p.frame_size(q)),
        oi(p.audio_frame_rate(q)),
        oi(p.audio_channel_count(q)),
        v(p.video_display_format(q), oi),
        v(p.audio_display_format(q), oi),
        v(p.sequence_end(q), t),
        v(p.zero_point(q), t),
        v(p.sequence_in_point(q), ot),
        v(p.sequence_out_point(q), ot),
        v(p.work_area_in(q), t),
        v(p.work_area_out(q), t),
        v(p.playhead(q), t),
        path_of(p, p.sequence_item(q)),
        p.sequence_repr(q),
    ));
    out.push(format!(
        "  settings mode={} bitdepth={} quality={} codec={} preset={} preview={} vrate={} vsize={} channels={} arate={} repr={}",
        os(p.settings_editing_mode(q)),
        b(p.settings_max_bit_depth(q)),
        b(p.settings_max_render_quality(q)),
        v(p.settings_preview_codec(q), oi),
        os(p.settings_preview_preset_path(q)),
        v(p.settings_preview_frame_size(q), pair),
        ot(p.settings_video_frame_rate(q)),
        pair(p.frame_size(q)),
        oi(p.audio_channel_count(q)),
        ot(p.settings_audio_sample_rate(q)),
        p.settings_repr(q),
    ));
    for m in p.sequence_markers(q).to_vec() {
        dump_marker(out, "  ", p, m);
    }
    dump_named(out, "  ", "markers", &p.sequence_markers_named(q));
    let video = p.tracks_named(p.video_tracks(q));
    let audio = p.tracks_named(p.audio_tracks(q));
    dump_named(out, "  ", "video_tracks", &video);
    dump_named(out, "  ", "audio_tracks", &audio);
    for &track in video.items().iter().chain(audio.items()) {
        dump_track(out, p, track);
    }
    let clips = p.sequence_clips(q);
    let names: Vec<String> = clips.iter().map(|&c| v(p.track_item_name(c), |x| s(&x))).collect();
    out.push(format!("  clips={} {}", clips.len(), names.join(" ")));
    for ct in p.caption_tracks(q) {
        out.push(format!(
            "  caption_track id={} index={} format={} repr={}",
            v(p.caption_track_id(ct), |x| x.to_string()),
            v(p.caption_track_index(ct), |x| x.to_string()),
            v(p.caption_track_format(ct), |x| format!("CaptionFormat.{}", x.name())),
            v(p.caption_track_repr(ct), |x| x),
        ));
        for c in p.captions(ct) {
            out.push(format!(
                "    caption text={} start={} end={} source_start={} source_end={} repr={}",
                s(p.caption_text(c)),
                v(p.caption_start(c), t),
                v(p.caption_end(c), t),
                t(p.caption_source_start(c)),
                t(p.caption_source_end(c)),
                v(p.caption_repr(c), |x| x),
            ));
        }
    }
}

fn dump_item(out: &mut Vec<String>, p: &Project, i: ItemId) {
    out.push(format!(
        "item {} type=ProjectItemType.{} name={} node={} seq={} label={} in={} out={} media={} offline={} gen_id={} gen={} adj={} multicam={} mgt={} merged={} start={} proxy={} proxy_path={} subclip={} sub_in={} sub_out={} hard={} scale={} len={} repr={}",
        path_of(p, Some(i)),
        p.item_type(i).name(),
        s(&p.item_name(i)),
        os(p.node_id(i)),
        b(p.is_sequence_item(i)),
        p.color_label(i),
        v(p.item_in_point(i), ot),
        v(p.item_out_point(i), ot),
        os(p.item_media_path(i).map(str::to_string)),
        v(p.is_offline(i), b),
        v(p.generator_id(i), os),
        v(p.generator_type(i), |g| g.map_or("None".into(), |g| format!("GeneratorType.{}", g.name()))),
        v(p.is_adjustment_layer(i), b),
        v(p.is_multicam_clip(i), b),
        b(p.is_mgt(i)),
        v(p.is_merged_clip(i), b),
        v(p.item_start_time(i), t),
        v(p.has_proxy(i), b),
        v(p.proxy_path(i), os),
        v(p.is_subclip(i), b),
        v(p.subclip_in_point(i), ot),
        v(p.subclip_out_point(i), ot),
        v(p.has_hard_boundaries(i), b),
        v(p.scale_to_frame_size(i), b),
        p.children(i).len(),
        p.item_repr(i),
    ));
    match p.footage_interpretation(i) {
        Err(e) => out.push(format!("  footage {}", err(&e))),
        Ok(None) => out.push("  footage None".into()),
        Ok(Some(fi)) => {
            let tree = &p.doc.tree;
            out.push(format!(
                "  footage rate={} alpha={} ignore={} invert={} field={} par={} repr={}",
                v(fi.frame_rate(tree), t),
                v(fi.alpha_usage(tree), |x| format!("AlphaUsage.{}", x.name())),
                b(fi.ignore_alpha(tree)),
                b(fi.invert_alpha(tree)),
                v(fi.field_type(tree), |x| format!("VideoFieldType.{}", x.name())),
                v(fi.pixel_aspect_ratio(tree), py_repr),
                v(fi.repr(tree), |x| x),
            ));
        }
    }
    for m in p.item_markers(i).to_vec() {
        dump_marker(out, "  ", p, m);
    }
    dump_named(out, "  ", "children", &p.item_children(i));
}

fn dump(app: &mut Application) -> Vec<String> {
    let repr = app.repr();
    let p = app.project_mut();
    let mut out = vec![
        format!("application {repr}"),
        format!(
            "project name={} document_id={} sequences={} active={} repr={}",
            s(&p.name()),
            os(p.document_id()),
            p.sequences().len(),
            os(p.active_sequence().and_then(|q| p.sequence_id(q))),
            p.repr()
        ),
        format!("presets [{}]", Project::sequence_presets().iter().map(|n| s(n)).collect::<Vec<_>>().join(",")),
    ];
    if let Some(root) = p.root_item() {
        let mut items = vec![root];
        items.extend(p.walk(root));
        for i in items {
            dump_item(&mut out, p, i);
        }
    }
    dump_named(&mut out, "", "sequences", &p.sequences_named());
    for q in p.sequences() {
        dump_sequence(&mut out, p, q);
    }
    out
}

// ---- dumps ---------------------------------------------------------------------------------------

fn gunzip(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    let mut text = String::new();
    flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut text).unwrap();
    text
}

#[test]
fn dumps() {
    let index = std::fs::read_to_string(data("read/index.tsv")).unwrap();
    let (mut compared, mut skipped, mut failed) = (0, 0, Vec::new());
    let mut scratch = None;
    for line in index.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let path = base().join(fields[0]);
        if !path.exists() {
            skipped += 1;
            continue;
        }
        let parsed = application::parse(&path, None);
        if fields[1] == "-" {
            match parsed {
                Err(e) => assert_eq!(&err(&e)[..err(&e).len().min(300)], fields[2], "{}", fields[0]),
                Ok(_) => failed.push(format!("{}: Python refused, Rust parsed", fields[0])),
            }
            compared += 1;
            continue;
        }
        let mut app = match parsed {
            Ok(a) => a,
            Err(e) => {
                failed.push(format!("{}: Rust refused: {}", fields[0], err(&e)));
                continue;
            }
        };
        let text = dump(&mut app).join("\n") + "\n";
        let expected = gunzip(&data(&format!("read/dumps/{}", fields[1])));
        compared += 1;
        if text != expected {
            let dir = scratch.get_or_insert_with(|| Scratch::new("prproj-read-rs-dumps")).path();
            let stem = fields[1].trim_end_matches(".txt.gz");
            std::fs::write(dir.join(format!("{stem}.rust.txt")), &text).unwrap();
            std::fs::write(dir.join(format!("{stem}.python.txt")), &expected).unwrap();
            let first = text.lines().zip(expected.lines()).position(|(a, b)| a != b);
            let detail = match first {
                Some(n) => format!("line {}:\n  rust:   {}\n  python: {}", n + 1, text.lines().nth(n).unwrap(), expected.lines().nth(n).unwrap()),
                None => format!("lengths differ ({} vs {} lines)", text.lines().count(), expected.lines().count()),
            };
            failed.push(format!("{} ({}): {detail}", fields[0], dir.join(stem).display()));
        }
    }
    eprintln!("read dumps: {compared} compared, {skipped} skipped (absent)");
    assert!(failed.is_empty(), "{} of {compared} dumps differ:\n{}", failed.len(), failed.join("\n"));
}

// ---- preferences -------------------------------------------------------------------------------

fn dump_preferences(out: &mut Vec<String>, label: &str, prefs: Option<&Preferences>) {
    let Some(prefs) = prefs else {
        out.push(format!("{label} repr=None"));
        return;
    };
    out.push(format!("{label} repr={}", prefs.repr()));
    out.push(format!("  names=[{}]", prefs.label_names().iter().map(|n| s(n)).collect::<Vec<_>>().join(",")));
    out.push(format!(
        "  colors={}",
        v(prefs.label_colors(), |c| format!("[{}]", c.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")))
    ));
    for kind in ["Bin", "Sequence", "Video", "Audio", "AV", "Still", "DynamicLink", "Captions", "Nope"] {
        out.push(format!("  default {kind}={}", v(prefs.label_default(kind), oi)));
    }
    out.push(format!(
        "  still_rate={} still_out={}",
        v(prefs.still_frame_rate(), oi),
        v(prefs.still_default_out_ticks(), oi)
    ));
    for key in ["BE.Prefs.LabelColors.0", "BE.Prefs.StillImages.DurationInSeconds", "MZ.Nope", "Neg", "Word", "Flag", "Off"] {
        out.push(format!(
            "  get {key}={} int={} bool={}",
            os(prefs.get(key)),
            v(prefs.get_int(key), oi),
            prefs.get_bool(key).map_or("None".into(), b)
        ));
    }
}

const CRAFTED_PREFS: [(&str, &str); 5] = [
    ("minimal", "<PremiereData Version=\"3\"><Preferences ObjectID=\"1\" ClassID=\"x\" Version=\"1\"><Properties Version=\"1\"><BE.Prefs.LabelColors.3>255</BE.Prefs.LabelColors.3><BE.Prefs.LabelDefaults.Video>3</BE.Prefs.LabelDefaults.Video><BE.Prefs.LabelDefaults.Audio>16</BE.Prefs.LabelDefaults.Audio><BE.Prefs.LabelDefaults.Still>-1</BE.Prefs.LabelDefaults.Still><BE.Prefs.LabelNames.1>Iris</BE.Prefs.LabelNames.1><Neg>-12</Neg><Word>abc</Word><Flag>true</Flag><Off>false</Off><BE.Prefs.StillImages.DefaultFramerate>8475667200</BE.Prefs.StillImages.DefaultFramerate><BE.Prefs.StillImages.DurationInSeconds>5</BE.Prefs.StillImages.DurationInSeconds></Properties></Preferences></PremiereData>"),
    ("no_rate", "<PremiereData><Preferences ObjectID=\"7\"><Properties><BE.Prefs.StillImages.DurationInSeconds>5</BE.Prefs.StillImages.DurationInSeconds></Properties></Preferences></PremiereData>"),
    ("no_bag", "<PremiereData><Preferences ObjectID=\"\"><Properties/></Preferences></PremiereData>"),
    ("not_xml", "<PremiereData><Preferences>"),
    ("empty_root", "<PremiereData/>"),
];

// Minimal synthetic profile for the defaults recorded by the Python oracle.
fn recorded_preferences_xml() -> String {
    let names = [
        "Violet", "Iris", "Caribbean", "Lavender", "Cerulean", "Forest", "Rose", "Mango",
        "Purple", "Blue", "Teal", "Magenta", "Tan", "Green", "Brown", "Yellow",
    ];
    let colors = [
        11405886, 6769408, 480554, 8851829, 5985541, 19005, 3474060, 277129,
        11993185, 10104082, 4541953, 5770628, 4545135, 2579725, 408413, 1664623,
    ];
    let defaults = [
        ("Bin", 7), ("Sequence", 5), ("Video", 0), ("Audio", 2),
        ("AV", 1), ("Still", 3), ("DynamicLink", 6), ("Captions", 7),
    ];
    let mut xml = String::from("<PremiereData><Preferences ObjectID=\"1\"><Properties>");
    for (index, (name, color)) in names.into_iter().zip(colors).enumerate() {
        xml.push_str(&format!("<BE.Prefs.LabelNames.{index}>{name}</BE.Prefs.LabelNames.{index}>"));
        xml.push_str(&format!("<BE.Prefs.LabelColors.{index}>{color}</BE.Prefs.LabelColors.{index}>"));
    }
    for (kind, index) in defaults {
        xml.push_str(&format!("<BE.Prefs.LabelDefaults.{kind}>{index}</BE.Prefs.LabelDefaults.{kind}>"));
    }
    xml.push_str(concat!(
        "<BE.Prefs.StillImages.DefaultFramerate>10594584000</BE.Prefs.StillImages.DefaultFramerate>",
        "<BE.Prefs.StillImages.DurationInSeconds>5</BE.Prefs.StillImages.DurationInSeconds>",
        "</Properties></Preferences></PremiereData>",
    ));
    xml
}

#[test]
fn preferences() {
    let mut out = Vec::new();
    let scratch = Scratch::new("prproj-read-rs-prefs");
    let dir = scratch.path();
    let profile = dir.join("recorded-default");
    std::fs::write(&profile, recorded_preferences_xml()).unwrap();
    let mut default = Preferences::new(&profile).unwrap();
    default.set_path("/recorded/default");
    dump_preferences(&mut out, "default", Some(&default));
    for (name, text) in CRAFTED_PREFS {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        match Preferences::new(&path) {
            Err(e) => out.push(format!("{name} {}", err(&e))),
            Ok(mut prefs) => {
                prefs.set_path(Path::new("/crafted").join(name));
                dump_preferences(&mut out, name, Some(&prefs));
            }
        }
    }
    let expected = std::fs::read_to_string(data("read/preferences.txt")).unwrap();
    let (host_repr, values) = expected.split_once('\n').unwrap();
    assert!(host_repr.starts_with("default repr=Preferences(path="));
    assert_eq!(out.join("\n") + "\n", format!("default repr=Preferences(path='/recorded/default')\n{values}"));
}

// ---- validators --------------------------------------------------------------------------------

fn num_repr(n: Num) -> String {
    n.to_string()
}

#[test]
fn validators() {
    let expected = std::fs::read_to_string(data("read/validators.tsv")).unwrap();
    let rules = [
        ("number", val::NUMBER),
        ("positive_number", val::POSITIVE_NUMBER),
        ("int", val::INT),
        ("positive_int", val::POSITIVE_INT),
        ("track_count", val::TRACK_COUNT),
        ("color_label", val::COLOR_LABEL),
        ("marker_color_index", val::MARKER_COLOR_INDEX),
        ("float32", val::FLOAT32),
        ("packed_color", val::PACKED_COLOR),
    ];
    let scratch = Scratch::new("prproj-read-rs-validators");
    let dir = scratch.path();
    let tmp = dir.to_string_lossy().into_owned();
    std::fs::write(dir.join("file.txt"), "x").unwrap();
    let mut checked = 0;
    for line in expected.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (label, value, verdict) = (f[0], f[1], f[2]);
        let got: Option<Result<()>> = if let Some(&(_, rule)) = rules.iter().find(|(l, _)| *l == label) {
            let n = parse_num(value);
            assert_eq!(num_repr(n), value);
            Some(rule.check(n))
        } else if label.starts_with("string") {
            if value == "'\\ud800'" {
                None // an unpaired surrogate cannot be a Rust string
            } else {
                let text = unrepr(value);
                Some(match label {
                    "string" => val::validate_string(&text),
                    "string_nonempty" => val::validate_nonempty_string(&text),
                    _ => val::validate_string_with(&text, true, Some(3)),
                })
            }
        } else if let Some(name) = label.strip_prefix("enum_") {
            let n: i64 = value.parse().unwrap();
            let valid = match name {
                "AlphaUsage" => AlphaUsage::from_value(n).is_ok(),
                "BlendMode" => BlendMode::from_value(n).is_ok(),
                "VideoFieldType" => VideoFieldType::from_value(n).is_ok(),
                _ => KeyframeInterpolation::from_value(n).is_ok(),
            };
            Some(val::validate_enum_value(n, name, valid))
        } else if label == "one_of" {
            Some(val::validate_one_of(&unrepr(value), &["Comment", "Chapter", "Segmentation", "WebLink"]))
        } else if let Some(rule) = label.strip_prefix("path_") {
            let path = match value {
                "file" => dir.join("file.txt"),
                "dir" => dir.to_path_buf(),
                _ => dir.join("missing"),
            };
            let rule = match rule {
                "exists_file" => PathRule::MustExist { must_be_file: true },
                "exists" => PathRule::MustExist { must_be_file: false },
                "absent" => PathRule::MustNotExist,
                _ => PathRule::Any,
            };
            Some(val::validate_path(&path, rule).map_err(|e| match e {
                Error::Value(m) => Error::Value(m.replace(&tmp, "<TMP>")),
                Error::FileExists(m) => Error::FileExists(m.replace(&tmp, "<TMP>")),
                e => e,
            }))
        } else if label == "vector2" {
            let inner = value.trim_start_matches('[').trim_end_matches(']');
            let parts: Vec<Num> = inner.split(',').map(parse_num).collect();
            Some(val::validate_vector2([parts[0], parts[1]]))
        } else if label == "color" {
            let inner = value.trim_start_matches("Color(").trim_end_matches(')');
            let c: Vec<i64> = inner.split(',').map(|x| x.parse().unwrap()).collect();
            Some(val::validate_color([c[0], c[1], c[2], c[3]]))
        } else if label == "time_from_seconds" {
            let n = parse_num(value).as_f64();
            let got = Time::from_seconds(n).map(|x| x.ticks.to_string()).unwrap_or_else(|e| err(&e));
            assert_eq!(got, verdict, "{line}");
            checked += 1;
            None
        } else {
            panic!("unknown validator case {line}");
        };
        if let Some(got) = got {
            let got = match got {
                Ok(()) => "ok".to_string(),
                Err(e) => err(&e),
            };
            assert_eq!(got, verdict, "{line}");
            checked += 1;
        }
    }
    assert!(checked > 250, "{checked}");
}

fn parse_num(text: &str) -> Num {
    match text {
        "nan" => Num::Float(f64::NAN),
        "inf" => Num::Float(f64::INFINITY),
        "-inf" => Num::Float(f64::NEG_INFINITY),
        t if t.contains(['.', 'e']) => Num::Float(t.parse().unwrap()),
        t => Num::Int(t.parse().unwrap()),
    }
}

/// Undo `repr()` of the simple strings the validator cases use.
fn unrepr(text: &str) -> String {
    let inner = &text[1..text.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next().unwrap() {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '\\' => out.push('\\'),
            '\'' => out.push('\''),
            'x' => {
                let h: String = chars.by_ref().take(2).collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).unwrap()).unwrap());
            }
            other => panic!("escape {other}"),
        }
    }
    out
}

// ---- edits ---------------------------------------------------------------------------------------

/// The op runner's own lookups raise Python `LookupError` / `IndexError`.
fn lookup(message: String) -> Error {
    Error::Key(format!("\u{1}LookupError\u{1}{message}"))
}

fn err_op(e: &Error) -> String {
    let text = err(e);
    if let Some(rest) = text.strip_prefix("!KeyError: \u{1}LookupError\u{1}") {
        return format!("!LookupError: {rest}");
    }
    text
}

fn index<T: Copy>(list: &[T], i: &Json) -> Result<T> {
    let i = int(i) as usize;
    list.get(i).copied().ok_or_else(|| Error::Index("list index out of range".into()))
}

fn int(j: &Json) -> i64 {
    match j {
        Json::Int(i) => *i as i64,
        other => panic!("not an int: {other:?}"),
    }
}

fn text(j: &Json) -> String {
    match j {
        Json::Str(x) => x.clone(),
        other => panic!("not a string: {other:?}"),
    }
}

fn boolean(j: &Json) -> bool {
    match j {
        Json::Bool(x) => *x,
        other => panic!("not a bool: {other:?}"),
    }
}

fn num(j: &Json) -> Num {
    match j {
        Json::Int(i) => Num::Int(*i),
        Json::Float(f) => Num::Float(*f),
        other => panic!("not a number: {other:?}"),
    }
}

fn arr(j: &Json) -> &[Json] {
    match j {
        Json::Arr(a) => a,
        other => panic!("not a list: {other:?}"),
    }
}

fn opt_time(j: &Json) -> Option<Time> {
    match j {
        Json::Null => None,
        j => Some(Time::new(int(j))),
    }
}

fn item_at(p: &Project, path: &str) -> Result<ItemId> {
    let root = p.root_item().unwrap();
    let mut items = vec![root];
    items.extend(p.walk(root));
    items.into_iter().find(|&i| p.tree_path(i) == path).ok_or_else(|| lookup(format!("no item {path}")))
}

fn track_at(p: &Project, target: &[Json]) -> Result<TrackId> {
    let q = index(&p.sequences(), &target[0])?;
    let tracks = if text(&target[1]) == "V" { p.video_tracks(q) } else { p.audio_tracks(q) };
    index(tracks, &target[2])
}

fn clip_at(p: &Project, target: &[Json]) -> Result<TrackItemId> {
    let track = track_at(p, &target[..3])?;
    index(p.track_clips(track), &target[3])
}

fn param_at(p: &Project, target: &Json) -> Result<NodeId> {
    let t = arr(target);
    let clip = clip_at(p, &t[..4])?;
    if t[4] == Json::Str("remap".into()) {
        return p.time_remapping(clip)?.ok_or_else(|| Error::Attribute("'NoneType' object has no attribute".into()));
    }
    let list = if text(&t[4]) == "c" { p.components(clip) } else { p.selection_components(clip) };
    let mut component = index(list.items(), &t[5])?;
    for sub in &t[6..t.len() - 1] {
        component = index(p.component_sub_components(component).items(), sub)?;
    }
    index(p.component_properties(component).items(), &t[t.len() - 1])
}

fn marker_owner_markers(p: &Project, target: &[Json]) -> Result<Vec<NodeId>> {
    if text(&target[0]) == "seq" {
        let q = index(&p.sequences(), &target[1])?;
        Ok(p.sequence_markers(q).to_vec())
    } else {
        Ok(p.item_markers(item_at(p, &text(&target[1]))?).to_vec())
    }
}

fn tuple(j: &Json) -> Option<Vec<Num>> {
    j.get("tuple").map(|t| arr(t).iter().map(num).collect())
}

fn param_input(j: &Json) -> ParamInput {
    match j {
        Json::Bool(x) => ParamInput::Bool(*x),
        Json::Arr(a) => ParamInput::List(a.iter().map(num).collect()),
        j => ParamInput::Num(num(j)),
    }
}

/// `validate_enum(Enum)` on a raw int, then `Enum(value)`.
fn enum_value<T>(v: i64, name: &str, parse: impl Fn(i64) -> Result<T>) -> Result<T> {
    parse(v).map_err(|_| Error::Value(format!("{v} is not a valid {name} value")))
}

fn run_op(p: &mut Project, op: &[Json], ids: &mut SeededUuids) -> Result<String> {
    use prproj::build::marker::MarkerSpec;
    use prproj::model::component as cp;
    let name = text(&op[0]);
    let a = &op[1..];
    match name.as_str() {
        "item_name" => p.set_item_name(item_at(p, &text(&a[0]))?, &text(&a[1]))?,
        "item_label" => p.set_color_label(item_at(p, &text(&a[0]))?, int(&a[1]))?,
        "item_scale" => p.set_scale_to_frame_size(item_at(p, &text(&a[0]))?, boolean(&a[1]))?,
        "item_start" => p.set_item_start_time(item_at(p, &text(&a[0]))?, Time::new(int(&a[1])))?,
        "item_move" => {
            let (item, parent) = (item_at(p, &text(&a[0]))?, item_at(p, &text(&a[1]))?);
            p.move_item(item, parent)?
        }
        "add_bin" => {
            let parent = item_at(p, &text(&a[0]))?;
            let bin = p.add_bin(parent, &text(&a[1]), ids)?;
            return Ok(s(&p.tree_path(bin)));
        }
        "remove_bin" => {
            let (parent, child) = (item_at(p, &text(&a[0]))?, item_at(p, &text(&a[1]))?);
            p.remove_bin(parent, child)?
        }
        "remove_item" => {
            let (parent, child) = (item_at(p, &text(&a[0]))?, item_at(p, &text(&a[1]))?);
            p.remove_item(parent, child)?
        }
        "footage" => {
            let item = item_at(p, &text(&a[0]))?;
            let Some(fi) = p.footage_interpretation(item)? else {
                return Err(Error::Attribute(format!("'NoneType' object has no attribute '{}'", text(&a[1]))));
            };
            let tree = &mut p.doc.tree;
            match text(&a[1]).as_str() {
                "frame_rate" => fi.set_frame_rate(tree, opt_time(&a[2]))?,
                "alpha_usage" => fi.set_alpha_usage_int(tree, int(&a[2]))?,
                "alpha_usage_enum" => fi.set_alpha_usage(tree, AlphaUsage::from_name(&text(&a[2]))?)?,
                "field_type_enum" => fi.set_field_type(tree, VideoFieldType::from_name(&text(&a[2]))?)?,
                "field_type" => fi.set_field_type_int(tree, int(&a[2]))?,
                "ignore_alpha" => fi.set_ignore_alpha(tree, boolean(&a[2]))?,
                "invert_alpha" => fi.set_invert_alpha(tree, boolean(&a[2]))?,
                "pixel_aspect_ratio" => {
                    let input = match &a[2] {
                        Json::Null => ParInput::Clear,
                        j => match tuple(j) {
                            Some(t) => ParInput::Pair(t[0], t[1]),
                            None => ParInput::Ratio(num(j)),
                        },
                    };
                    fi.set_pixel_aspect_ratio(tree, input)?
                }
                other => panic!("footage field {other}"),
            }
        }
        "item_add_marker" | "seq_add_marker" => {
            let (n, c, ty) = (text(&a[1]), text(&a[3]), text(&a[4]));
            let spec =
                MarkerSpec { name: &n, start: Time::new(int(&a[2])), comments: &c, marker_type: &ty, duration: opt_time(&a[5]) };
            let marker = if name == "item_add_marker" {
                let item = item_at(p, &text(&a[0]))?;
                p.add_item_marker(item, &spec, ids)?
            } else {
                let q = index(&p.sequences(), &a[0])?;
                p.add_sequence_marker(q, &spec, ids)?
            };
            return Ok(s(&mk::marker_guid(&p.doc.tree, marker)?));
        }
        "remove_marker" => {
            let owner = arr(&a[0]);
            let marker = index(&marker_owner_markers(p, owner)?, &a[1])?;
            if text(&owner[0]) == "seq" {
                let q = index(&p.sequences(), &owner[1])?;
                p.remove_sequence_marker(q, marker)?
            } else {
                let item = item_at(p, &text(&owner[1]))?;
                p.remove_item_marker(item, marker)?
            }
        }
        "marker" => {
            let marker = index(&marker_owner_markers(p, arr(&a[0]))?, &a[1])?;
            let tree = &mut p.doc.tree;
            match text(&a[2]).as_str() {
                "name" => mk::set_marker_name(tree, marker, &text(&a[3]))?,
                "comments" => mk::set_marker_comments(tree, marker, &text(&a[3]))?,
                "start" => mk::set_marker_start(tree, marker, Time::new(int(&a[3])))?,
                "end" => mk::set_marker_end(tree, marker, Time::new(int(&a[3])))?,
                "color_index" => mk::set_marker_color_index(tree, marker, int(&a[3]), ids)?,
                "type" => match &a[3] {
                    Json::Arr(link) => mk::set_marker_type_as_web_link(tree, marker, &text(&link[0]), &text(&link[1]))?,
                    kind => mk::set_marker_type(tree, marker, &text(kind))?,
                },
                other => panic!("marker field {other}"),
            }
        }
        "seq" => {
            let q = index(&p.sequences(), &a[0])?;
            let v = &a[2];
            match text(&a[1]).as_str() {
                "name" => p.set_sequence_name(q, &text(v))?,
                "zero_point" => p.set_zero_point(q, Time::new(int(v)))?,
                "in_point" => p.set_sequence_in_point(q, opt_time(v))?,
                "out_point" => p.set_sequence_out_point(q, opt_time(v))?,
                "work_area_in" => p.set_work_area_in(q, Time::new(int(v)))?,
                "work_area_out" => p.set_work_area_out(q, Time::new(int(v)))?,
                "playhead" => p.set_playhead(q, Time::new(int(v)))?,
                "video_display_format" => p.set_video_display_format(q, int(v))?,
                "audio_display_format" => p.set_audio_display_format(q, int(v))?,
                other => panic!("seq field {other}"),
            }
        }
        "settings" => {
            let q = index(&p.sequences(), &a[0])?;
            let v = &a[2];
            match text(&a[1]).as_str() {
                "editing_mode" => p.set_settings_editing_mode(q, &text(v))?,
                "max_bit_depth" => p.set_settings_max_bit_depth(q, boolean(v))?,
                "max_render_quality" => p.set_settings_max_render_quality(q, boolean(v))?,
                "preview_codec" => p.set_settings_preview_codec(q, int(v))?,
                "preview_rendering_preset_path" => p.set_settings_preview_preset_path(q, &text(v))?,
                "preview_frame_size" => {
                    let t = tuple(v).unwrap();
                    p.set_settings_preview_frame_size(q, (t[0], t[1]))?
                }
                other => panic!("settings field {other}"),
            }
        }
        "track" => {
            let track = track_at(p, arr(&a[0]))?;
            let on = boolean(&a[2]);
            match text(&a[1]).as_str() {
                "is_muted" => p.set_muted(track, on)?,
                "is_locked" => p.set_locked(track, on)?,
                "is_sync_locked" => p.set_sync_locked(track, on)?,
                other => panic!("track field {other}"),
            }
        }
        "clip" => {
            let clip = clip_at(p, arr(&a[0]))?;
            let v = &a[2];
            match text(&a[1]).as_str() {
                "is_disabled" => p.set_disabled(clip, boolean(v))?,
                "in_point" => p.set_in_point(clip, Time::new(int(v)))?,
                "out_point" => p.set_out_point(clip, Time::new(int(v)))?,
                "start" => p.set_start(clip, Time::new(int(v)))?,
                "end" => p.set_end(clip, Time::new(int(v)))?,
                "name" => p.set_track_item_name(clip, &text(v))?,
                "blend_mode" => {
                    let mode = enum_value(int(v), "BlendMode", BlendMode::from_value)?;
                    p.set_blend_mode(clip, mode)?
                }
                other => panic!("clip field {other}"),
            }
        }
        "tag_dialogue" => p.tag_as_dialogue(clip_at(p, arr(&a[0]))?)?,
        "time_remap" => {
            let clip = clip_at(p, arr(&a[0]))?;
            let keys: Vec<(Time, Num)> = arr(&a[1]).iter().map(|k| (Time::new(int(&arr(k)[0])), num(&arr(k)[1]))).collect();
            let param = p.set_time_remap(clip, &keys)?;
            return Ok(times(&param_keys(&p.doc.tree, param)?));
        }
        "clear_remap" => p.clear_time_remap(clip_at(p, arr(&a[0]))?)?,
        "remove_clip" => {
            let track = track_at(p, arr(&a[0]))?;
            let clip = index(p.track_clips(track), &a[1])?;
            p.remove_clip(track, clip)?
        }
        "move_clip" => {
            let track = track_at(p, arr(&a[0]))?;
            let clip = index(p.track_clips(track), &a[1])?;
            let target = track_at(p, arr(&a[2]))?;
            p.move_clip(track, clip, target, opt_time(&a[3]))?
        }
        "add_clip" | "insert_clip" => {
            let track = track_at(p, arr(&a[0]))?;
            let item = item_at(p, &text(&a[1]))?;
            let start = Time::new(int(&a[2]));
            let placed =
                if name == "add_clip" { p.add_clip(track, item, start, ids)? } else { p.insert_clip(track, item, start, ids)? };
            return Ok(s(&p.track_item_name(placed)?));
        }
        "add_linked" => {
            let q = index(&p.sequences(), &a[0])?;
            let item = item_at(p, &text(&a[1]))?;
            let (v, au) = p.add_linked_clip(q, item, Time::new(int(&a[2])), int(&a[3]), int(&a[4]), ids)?;
            return Ok(format!("[{},{}]", s(&p.track_item_name(v)?), s(&p.track_item_name(au)?)));
        }
        "add_transition" => {
            let track = track_at(p, arr(&a[0]))?;
            let clip = index(p.track_clips(track), &a[1])?;
            let display = match &a[5] {
                Json::Null => None,
                j => Some(text(j)),
            };
            let x = p.add_transition(track, clip, &text(&a[2]), boolean(&a[3]), opt_time(&a[4]), display.as_deref())?;
            let tree = &p.doc.tree;
            return Ok(format!("[{},{}]", t(tr::transition_start(tree, x)?), t(tr::transition_end(tree, x)?)));
        }
        "remove_transition" => {
            let track = track_at(p, arr(&a[0]))?;
            let x = index(p.track_transitions(track), &a[1])?;
            p.remove_transition(track, x)?
        }
        "param_value" => {
            let param = param_at(p, &a[0])?;
            cp::set_param_value(&mut p.doc.tree, param, &param_input(&a[1]))?
        }
        "param_color" => {
            let param = param_at(p, &a[0])?;
            let c: Vec<i64> = arr(a[1].get("color").unwrap()).iter().map(int).collect();
            cp::set_param_color(&mut p.doc.tree, param, Color::rgba(c[0], c[1], c[2], c[3]))?
        }
        "param_keys" => {
            let param = param_at(p, &a[0])?;
            let mut keys = Vec::new();
            for k in arr(&a[1]) {
                let k = arr(k);
                keys.push((Time::new(int(&k[0])), num(&k[1]), KeyframeInterpolation::from_name(&text(&k[2]))?));
            }
            cp::set_param_keyframes(&mut p.doc.tree, param, &keys)?
        }
        "param_text" => {
            let param = param_at(p, &a[0])?;
            p.set_param_text(param, &text(&a[1]))?
        }
        "param_font" => {
            let param = param_at(p, &a[0])?;
            p.set_param_font_family(param, &text(&a[1]))?
        }
        "param_path" => {
            let param = param_at(p, &a[0])?;
            let vertices: Vec<PathVertex> = arr(&a[1])
                .iter()
                .map(|v| {
                    let f: Vec<f64> = arr(v).iter().map(|x| num(x).as_f64()).collect();
                    PathVertex { x: f[0], y: f[1], in_x: f[2], in_y: f[3], out_x: f[4], out_y: f[5], flag: f[6] }
                })
                .collect();
            p.set_param_path(param, &vertices, ids)?
        }
        "add_sequence" => {
            let q = p.add_sequence(&text(&a[0]), &text(&a[1]), None, ids)?;
            return Ok(s(&p.sequence_name(q)));
        }
        other => panic!("unknown op {other}"),
    }
    Ok("ok".into())
}

#[test]
fn edits() {
    let dir = data("read/edits");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    cases.sort();
    let scratch = Scratch::new("prproj-read-rs-edits");
    let save_dir = scratch.path();
    let (mut compared, mut ops_run, mut failed) = (0, 0, Vec::new());
    for case_path in cases {
        let case = case_path.file_stem().unwrap().to_string_lossy().into_owned();
        let spec = parse_json(&std::fs::read_to_string(&case_path).unwrap()).unwrap();
        let fixture = fixtures().join(text(spec.get("fixture").unwrap()));
        if !fixture.exists() {
            eprintln!("SKIP: fixture absent: {}", fixture.display());
            continue;
        }
        let mut ids = SeededUuids::new(format!("read_edits:{case}"));
        let mut app = application::parse(&fixture, None).unwrap();
        let mut log = Vec::new();
        for op in arr(spec.get("ops").unwrap()) {
            let result = run_op(app.project_mut(), arr(op), &mut ids).unwrap_or_else(|e| err_op(&e));
            log.push(result);
            ops_run += 1;
        }
        let target = save_dir.join(format!("{case}.prproj"));
        app.project_mut().save(&target).unwrap();
        log.extend(dump(&mut app));
        let text_log = log.join("\n") + "\n";
        let expected_log = gunzip(&dir.join(format!("{case}.log.gz")));
        compared += 1;
        if text_log != expected_log {
            std::fs::write(save_dir.join(format!("{case}.rust.log")), &text_log).unwrap();
            std::fs::write(save_dir.join(format!("{case}.python.log")), &expected_log).unwrap();
            let n = text_log.lines().zip(expected_log.lines()).position(|(a, b)| a != b).unwrap_or(0);
            failed.push(format!(
                "{case}: log line {}:\n  rust:   {}\n  python: {}",
                n + 1,
                text_log.lines().nth(n).unwrap_or(""),
                expected_log.lines().nth(n).unwrap_or("")
            ));
            continue;
        }
        let ours = std::fs::read(&target).unwrap();
        let theirs = std::fs::read(dir.join(format!("{case}.prproj"))).unwrap();
        let (xml_ours, _) = prproj::xml::decompress_prproj(&ours).unwrap();
        let (xml_theirs, _) = prproj::xml::decompress_prproj(&theirs).unwrap();
        if xml_ours != xml_theirs {
            failed.push(format!("{case}: saved XML differs ({})", target.display()));
        } else if GZIP_BYTES_GUARANTEED && ours != theirs {
            failed.push(format!("{case}: gzip bytes differ"));
        }
    }
    eprintln!("read edits: {compared} cases, {ops_run} operations");
    assert!(failed.is_empty(), "{} edit cases differ:\n{}", failed.len(), failed.join("\n"));
}
