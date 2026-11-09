//! Clip gain, clip volume (+ keyframes), track level and audio crossfades (port of
//! `avid2prproj/audio.py`; shapes from research PRPROJ-0013).
//!
//! - clip volume: a materialized intrinsic "Internal Volume Mono" `AudioFilterComponent` with
//!   params Mute and Level; Level is linear with dB = 20*log10(v) + 15; keyframes are
//!   ';'-separated 8-field records in clip source time;
//! - audio gain: a linear `<Gain>` on the timeline `AudioClip`, after `AudioChannelLayout`;
//! - track level: the track `AudioFader`'s Volume param, linear (dB = 20*log10(v)).

use crate::build::fmt::{py_pow, py_repr};
use crate::build::media::audio_channel_layout;
use crate::build::time::Time;
use crate::build::{leaf, top};
use crate::convert::model::{py_str_repr, AudioCrossfade, EventAudio, VolumeKey};
use crate::error::{value, Result};
use crate::model::{Project, TrackId, TrackItemId};

pub const PREMIERE_VOLUME_MAX_DB: f64 = 6.0;
const FILTER_CLASS_ID: &str = "d77a90a0-6c9e-44bf-9b20-de8c21168fe1";
const MUTE_CLASS_ID: &str = "32657501-3aa4-445f-a49b-d09ecb9fa1ae";
const LEVEL_CLASS_ID: &str = "a714635e-a628-4b27-9d59-77eba47dbc1a";
const START: &str = "-91445760000000000";
const CHANNEL_CONFIG: &str =
    r#"{"in":[{"layout":[0],"name":"Mono In","type":0}],"out":[{"layout":[0],"name":"Mono Out","type":0}]}"#;

/// Premiere clip-volume Level for `db` (0 dB -> 0.177827939...).
pub fn level_value(db: f64) -> f64 {
    py_pow(10.0, (db - 15.0) / 20.0)
}

/// Linear gain for `db`.
pub fn linear(db: f64) -> f64 {
    py_pow(10.0, db / 20.0)
}

/// Python `repr(float(value))`.
pub fn fmt(value: f64) -> String {
    py_repr(value)
}

/// Premiere's clip volume tops out at +6 dB; Avid's goes higher. Volume and gain multiply, so
/// any excess moves into gain without changing what plays. Returns `(gain_db, volume, shift_db)`.
pub fn split_overflow(audio: &EventAudio) -> (f64, EventAudio, f64) {
    let mut peaks: Vec<f64> = audio.volume_keys.iter().map(|k| k.db).collect();
    if let Some(v) = audio.volume_db {
        peaks.push(v);
    }
    // Python `max(peaks, default=0.0)`: the first maximal value; `max(0.0, x)` likewise.
    let mut peak = 0.0;
    for (i, &p) in peaks.iter().enumerate() {
        if i == 0 || p > peak {
            peak = p;
        }
    }
    let excess = peak - PREMIERE_VOLUME_MAX_DB;
    let shift = if excess > 0.0 { excess } else { 0.0 };
    if shift == 0.0 {
        return (audio.gain_db, audio.clone(), 0.0);
    }
    let volume = EventAudio {
        gain_db: audio.gain_db,
        volume_db: audio.volume_db.map(|v| v - shift),
        volume_keys: audio.volume_keys.iter().map(|k| VolumeKey { ticks: k.ticks, db: k.db - shift }).collect(),
    };
    (audio.gain_db + shift, volume, shift)
}

/// `set_clip_gain(placed, db)`: linear `<Gain>` on the timeline `AudioClip` (zero = nothing).
pub fn set_clip_gain(project: &mut Project, placed: TrackItemId, db: f64) -> Result<()> {
    if db == 0.0 {
        return Ok(());
    }
    let clip = project.track_item(placed).clip;
    let tree = &mut project.doc.tree;
    let gain = match tree.child_by_tag(clip, "Gain") {
        Some(g) => g,
        None => {
            let g = tree.sub_element(clip, "Gain", &[]);
            let children = tree.children(clip);
            if children.len() < 2 {
                return value("list index out of range");
            }
            let previous = children[children.len() - 2];
            let tail = tree.tail(previous).map(String::from);
            tree.set_tail(g, tail);
            g
        }
    };
    tree.set_text(gain, Some(fmt(linear(db))));
    Ok(())
}

/// `set_clip_volume(project, placed, audio)`: materialize the clip Volume intrinsic, static or
/// keyframed. Nothing when the event has no volume.
pub fn set_clip_volume(project: &mut Project, placed: TrackItemId, audio: &EventAudio) -> Result<()> {
    if audio.volume_db.is_none() && audio.volume_keys.is_empty() {
        return Ok(());
    }
    let element = project.track_item(placed).element;
    let Some(chain_ref) = project.doc.tree.find1(element, "ClipTrackItem/ComponentOwner/Components") else {
        return value("clip has no ComponentOwner/Components");
    };
    let chain = project.doc.resolve(chain_ref)?;
    let tree = &mut project.doc.tree;
    for tag in ["DefaultVol", "DefaultVolumeComponentID", "DefaultChannelVolumeComponentID"] {
        if let Some(e) = tree.child_by_tag(chain, tag) {
            tree.remove(chain, e);
        }
    }
    let inner = match tree.child_by_tag(chain, "ComponentChain") {
        Some(i) => i,
        None => tree.sub_element(chain, "ComponentChain", &[("Version", "3")]),
    };
    let components = match tree.child_by_tag(inner, "Components") {
        Some(c) => c,
        None => tree.sub_element(inner, "Components", &[("Version", "1")]),
    };

    let mute = top(tree, "AudioComponentParam", MUTE_CLASS_ID, "10", None);
    leaf(tree, mute, "IsTimeVarying", "false", "\n\t\t");
    leaf(tree, mute, "Name", "Mute", "\n\t\t");
    leaf(tree, mute, "RangeLocked", "false", "\n\t");
    let level = top(tree, "AudioComponentParam", LEVEL_CLASS_ID, "10", None);
    if let Some(first_key) = audio.volume_keys.first() {
        let first = level_value(first_key.db);
        leaf(tree, level, "StartKeyframe", &format!("{START},{},0,0,0,0,0,0", fmt(first)), "\n\t\t");
        leaf(tree, level, "CurrentValue", &fmt(first), "\n\t\t");
        // Linear keys: speeds 0, influence 1/6 (Premiere's default influence).
        let keys: String = audio
            .volume_keys
            .iter()
            .map(|k| format!("{},{},0,0,0,0.16666666666666666,0,0.16666666666666666;", k.ticks, fmt(level_value(k.db))))
            .collect();
        leaf(tree, level, "Keyframes", &keys, "\n\t\t"); // no IsTimeVarying: Premiere omits it here
    } else {
        let v = level_value(audio.volume_db.expect("checked above"));
        leaf(tree, level, "StartKeyframe", &format!("{START},{},0,0,0,0,0,0", fmt(v)), "\n\t\t");
        leaf(tree, level, "CurrentValue", &fmt(v), "\n\t\t");
    }
    leaf(tree, level, "Name", "Level", "\n\t\t");
    leaf(tree, level, "UnitsString", "dB", "\n\t");
    let mute_id = project.register(mute)?;
    let level_id = project.register(level)?;

    let tree = &mut project.doc.tree;
    let component = top(tree, "AudioFilterComponent", FILTER_CLASS_ID, "4", None);
    let audio_component = tree.sub_element(component, "AudioComponent", &[("Version", "3")]);
    let core = tree.sub_element(audio_component, "Component", &[("Version", "7")]);
    let params = tree.sub_element(core, "Params", &[("Version", "1")]);
    tree.sub_element(params, "Param", &[("Index", "0"), ("ObjectRef", &mute_id)]);
    tree.sub_element(params, "Param", &[("Index", "1"), ("ObjectRef", &level_id)]);
    leaf(tree, core, "ID", "1", "");
    leaf(tree, core, "Intrinsic", "true", "");
    leaf(tree, audio_component, "AudioChannelLayout", &audio_channel_layout(1), "");
    leaf(tree, audio_component, "AudioComponentType", "0", "");
    leaf(tree, audio_component, "FrameRate", "5292000", "");
    leaf(tree, audio_component, "ChannelType", "0", "");
    leaf(tree, component, "FilterPreset", "0", "");
    leaf(tree, component, "ChannelConfigData", CHANNEL_CONFIG, "");
    leaf(tree, component, "FilterMatchName", "Internal Volume Mono", "");
    leaf(tree, component, "FilterIndex", "-1", "");
    let component_id = project.register(component)?;
    let tree = &mut project.doc.tree;
    let index = tree.len(components).to_string();
    tree.sub_element(components, "Component", &[("Index", &index), ("ObjectRef", &component_id)]);
    Ok(())
}

/// `set_track_level(project, track, db)`: set an audio track's fader Volume (linear).
pub fn set_track_level(project: &mut Project, track: TrackId, db: f64) -> Result<()> {
    let element = project.track(track).element;
    let Some(chain_ref) = project.doc.tree.find1(element, "AudioTrack/ComponentOwner/Components") else {
        return value("audio track has no ComponentOwner/Components");
    };
    let chain = project.doc.resolve(chain_ref)?;
    for reference in project.doc.tree.findall(chain, "ComponentChain/Components/Component")? {
        let component = project.doc.resolve(reference)?;
        if project.doc.tree.tag(component) != "AudioFader" {
            continue;
        }
        let params: Vec<_> = project.doc.tree.iter_tag(component, "Param").collect();
        for p in params {
            let param = project.doc.resolve(p)?;
            if project.doc.tree.findtext(param, "Name")?.as_deref() == Some("Volume") {
                let v = fmt(linear(db));
                let tree = &mut project.doc.tree;
                for (tag, text) in [("StartKeyframe", format!("{START},{v},0,0,0,0,0,0")), ("CurrentValue", v.clone())] {
                    let e = match tree.child_by_tag(param, tag) {
                        Some(e) => e,
                        None => {
                            let e = tree.new_element(tag);
                            let tail = tree.children(param).first().and_then(|&c| tree.tail(c)).map(String::from);
                            tree.set_tail(e, tail);
                            tree.insert(param, 0, e);
                            e
                        }
                    };
                    tree.set_text(e, Some(text));
                }
                return Ok(());
            }
        }
    }
    value("audio track has no fader Volume parameter")
}

/// `add_crossfade(track, fade, style)`: an audio crossfade over the cut at `fade.cut`,
/// positioned exactly: Start = cut - offset, End = Start + duration, Alignment = offset.
/// `style`: `"constant_power"` (py_premiere's 26.3-verified shape) or `"custom_fade"` (what
/// Premiere 26.5.2 saved for a Constant Power crossfade: "Custom Fade" names plus
/// `FadeShapeValue` 14; PRPROJ-0013).
pub fn add_crossfade(project: &mut Project, track: TrackId, fade: &AudioCrossfade, style: &str) -> Result<()> {
    let clips: Vec<TrackItemId> = project.track_clips(track).to_vec();
    let mut outgoing = None;
    for &c in &clips {
        if project.end(c)?.ticks == fade.cut {
            outgoing = Some(c);
            break;
        }
    }
    let mut incoming = None;
    for &c in &clips {
        if project.start(c)?.ticks == fade.cut {
            incoming = Some(c);
            break;
        }
    }
    let (Some(outgoing), Some(incoming)) = (outgoing, incoming) else {
        return value(format!("no cut at {} ticks on {}", fade.cut, project.track_name(track)));
    };
    if fade.cut_offset > project.duration(outgoing)?.ticks || fade.duration - fade.cut_offset > project.duration(incoming)?.ticks {
        return value(format!("crossfade at {} is longer than the clips around it", fade.cut));
    }
    let element = project.add_transition(track, incoming, "Constant Power", true, Some(Time::new(fade.duration)), None)?;
    let tree = &mut project.doc.tree;
    let Some(span) = tree.find1(element, "TransitionTrackItem/TrackItem") else {
        return value("transition has no TransitionTrackItem/TrackItem");
    };
    let start = fade.cut - fade.cut_offset;
    for (tag, v) in [("Start", start), ("End", start + fade.duration)] {
        let e = match tree.child_by_tag(span, tag) {
            Some(e) => e,
            None => tree.sub_element(span, tag, &[]),
        };
        tree.set_text(e, Some(v.to_string()));
    }
    if let Some(alignment) = tree.find1(element, "TransitionTrackItem/Alignment") {
        tree.set_text(alignment, Some(fade.cut_offset.to_string()));
    }
    if style == "custom_fade" {
        for tag in ["DisplayName", "MatchName"] {
            if let Some(e) = tree.find1(element, &format!("TransitionTrackItem/{tag}")) {
                tree.set_text(e, Some("Custom Fade"));
            }
        }
        let shape = match tree.child_by_tag(element, "FadeShapeValue") {
            Some(s) => s,
            None => {
                let s = tree.sub_element(element, "FadeShapeValue", &[]);
                let children = tree.children(element);
                if children.len() < 2 {
                    return value("list index out of range");
                }
                let previous = children[children.len() - 2];
                let tail = tree.tail(previous).map(String::from);
                tree.set_tail(s, tail);
                s
            }
        };
        tree.set_text(shape, Some("14"));
    } else if style != "constant_power" {
        return value(format!("unknown crossfade style {}", py_str_repr(style)));
    }
    Ok(())
}
