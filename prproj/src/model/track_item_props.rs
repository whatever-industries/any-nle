//! The rest of `TrackItem` (port of `py_premiere/models/track_item.py`): components, blend
//! mode (with the GUI's Opacity materialization), time remapping (read, set, clear), the
//! Essential Sound tag, time interpolation, enable/disable, `type` and the repr.
//!
//! Not ported here: `TrackItem.add_mask` (mask builder port).

use super::component::{param_display_name, param_value, ParamValue};
use super::enums::{BlendMode, TimeInterpolationType};
use super::named_list::NamedList;
use super::project::py_int;
use super::validators::{Num, POSITIVE_NUMBER};
use super::{ComponentData, ComponentId, Project, TrackItemId};
use crate::build::fmt::{fmt_f, format_remap_bound, format_remap_value};
use crate::build::merged::bag_node;
use crate::build::time::{round_half_even, Time, TICKS_PER_SECOND};
use crate::build::{leaf, top};
use crate::convert::model::py_str_repr;
use crate::error::{type_err, value, Error, Result};
use crate::xml::mutations::{remove_child, set_elided_flag};
use crate::xml::NodeId;

/// `_TIME_REMAP_CLASS_ID`.
pub const TIME_REMAP_CLASS_ID: &str = "ace5148e-9c9b-40ed-9f82-64cb67308464";
/// `_TIME_PARAM_CLASS_ID`.
pub const TIME_PARAM_CLASS_ID: &str = "278ae1f9-ab7b-4dff-a53c-21029a399a9d";
/// `_REMAP_START_KEYFRAME`.
pub const REMAP_START_KEYFRAME: &str = "-91445760000000000,0.000000000000000000000000,0,0,0,0,0,0";
const OPACITY_COMPONENT_CLASS_ID: &str = "d10da199-beea-4dd1-b941-ed3a78766d50";
const OPACITY_PARAM_CLASS_ID: &str = "fe47129e-6c94-4fc0-95d5-c056a517aaf3";
const BLEND_PARAM_CLASS_ID: &str = "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8";

/// `_DIALOGUE_ESSENTIAL_SOUND`: the bag the Essential Sound panel writes for Dialogue.
pub const DIALOGUE_ESSENTIAL_SOUND: [(&str, &str); 9] = [
    ("ESP.ClaritySection.2", "3"),
    ("ESP.SoundEffectsSection.3", "2"),
    ("ESP.RestorationSection.4", "7.14300060272"),
    ("ESP.RestorationSection.9", "60"),
    ("ESP.Tag", "dialog"),
    ("ESP.PresetGuid", "bb717b4a-c0b1-4def-9040-3fb8e83b8a91"),
    ("ESP.ClipMixTypeID", "1"),
    ("MZ.EssentialSound.AdjustmentModeWorkflowType", "1"),
    ("ESP.AdjustmentsModeID", "dialog"),
];

/// `_API_BLEND_VALUES`: the API twin's (`ParameterID` 2) value per mode.
pub fn api_blend_value(mode: BlendMode) -> i64 {
    use BlendMode::*;
    match mode {
        Normal => 18,
        Dissolve => 6,
        Darken => 3,
        Multiply => 17,
        ColorBurn => 1,
        DarkerColor => 4,
        Lighten => 11,
        Screen => 22,
        ColorDodge => 2,
        LighterColor => 12,
        LinearBurn => 13,
        LinearDodge => 14,
        Overlay => 19,
        SoftLight => 23,
        HardLight => 8,
        VividLight => 24,
        LinearLight => 15,
        PinLight => 20,
        HardMix => 9,
        Difference => 5,
        Exclusion => 7,
        Subtract => 25,
        Divide => 26,
        Hue => 10,
        Saturation => 21,
        Color => 0,
        Luminosity => 16,
    }
}

/// `_blend_keyframe`: popup values are bare integers.
fn blend_keyframe(v: i64) -> String {
    format!("-91445760000000000,{v},0,0,0,0,0,0")
}

impl Project {
    /// `TrackItem.components`: the materialized components, by display or match name.
    pub fn components(&self, item: TrackItemId) -> NamedList<ComponentId> {
        self.components_named(self.track_item(item).components.clone())
    }

    /// `TrackItem.selection_components`: clip-level masks.
    pub fn selection_components(&self, item: TrackItemId) -> NamedList<ComponentId> {
        self.components_named(self.track_item(item).selection_components.clone())
    }

    /// `TrackItem.time_remapping`: the Speed parameter of a retimed clip, if any.
    pub fn time_remapping(&self, item: TrackItemId) -> Result<Option<NodeId>> {
        let tree = &self.doc.tree;
        let Some(core) = tree.child_by_tag(self.track_item(item).clip, "Clip") else { return Ok(None) };
        let Some(reference) = tree.child_by_tag(core, "TimeRemapping") else { return Ok(None) };
        let remap = self.doc.resolve(reference)?;
        let Some(keyframes) = self.doc.tree.child_by_tag(remap, "Keyframes") else { return Ok(None) };
        self.doc.resolve(keyframes).map(Some)
    }

    /// `TrackItem._blend_params`: the Opacity intrinsic's Blend Mode twins by `ParameterID`.
    fn blend_params(&self, item: TrackItemId) -> Result<Vec<(String, NodeId)>> {
        let mut params: Vec<(String, NodeId)> = Vec::new();
        let tree = &self.doc.tree;
        for &component in &self.track_item(item).components {
            let element = self.component(component).element;
            let Some(inner) = tree.child_by_tag(element, "Component") else { continue };
            if tree.findtext(inner, "DisplayName")?.as_deref() != Some("Opacity") {
                continue;
            }
            for &param in &self.component(component).properties {
                if param_display_name(tree, param) == "Blend Mode" {
                    let pid = tree.findtext(param, "ParameterID")?.unwrap_or_default();
                    match params.iter_mut().find(|(p, _)| *p == pid) {
                        Some(slot) => slot.1 = param,
                        None => params.push((pid, param)),
                    }
                }
            }
        }
        Ok(params)
    }

    /// `TrackItem.blend_mode` (`NORMAL` while the Opacity intrinsic is not materialized).
    pub fn blend_mode(&self, item: TrackItemId) -> Result<BlendMode> {
        let params = self.blend_params(item)?;
        let Some(&(_, param)) = params.iter().find(|(p, _)| p == "3") else { return Ok(BlendMode::Normal) };
        let v = match param_value(&self.doc.tree, param)? {
            Some(ParamValue::Float(f)) => super::track_item::py_float_to_int(f)?,
            Some(ParamValue::Bool(b)) => b as i64,
            Some(ParamValue::Str(s)) => py_int(&s)?,
            Some(ParamValue::List(_)) => {
                return type_err("int() argument must be a string, a bytes-like object or a real number, not 'list'")
            }
            None => {
                return type_err("int() argument must be a string, a bytes-like object or a real number, not 'NoneType'")
            }
        };
        BlendMode::from_value(v)
    }

    /// `TrackItem.blend_mode = mode`: rewrite both Blend Mode twins, or materialize the Opacity
    /// intrinsic as Premiere's GUI does.
    pub fn set_blend_mode(&mut self, item: TrackItemId, mode: BlendMode) -> Result<()> {
        let params = self.blend_params(item)?;
        if params.is_empty() {
            if mode == BlendMode::Normal {
                return Ok(());
            }
            return self.materialize_opacity(item, mode);
        }
        for (pid, stored) in [("2", api_blend_value(mode)), ("3", mode.value())] {
            let Some(&(_, param)) = params.iter().find(|(p, _)| p == pid) else { continue };
            if let Some(keyframe) = self.doc.tree.child_by_tag(param, "StartKeyframe") {
                self.doc.tree.set_text(keyframe, Some(blend_keyframe(stored)));
            }
        }
        Ok(())
    }

    /// `TrackItem._materialize_opacity(mode)`: the chain loses `DefaultOpacity` and gains the
    /// `AE.ADBE Opacity` component (percentage param plus both Blend Mode twins).
    fn materialize_opacity(&mut self, item: TrackItemId, mode: BlendMode) -> Result<()> {
        let element = self.track_item(item).element;
        let Some(chain_ref) = self.doc.tree.find1(element, "ClipTrackItem/ComponentOwner/Components") else {
            return value("track item has no component chain");
        };
        let chain = self.doc.resolve(chain_ref)?;
        let Some(inner) = self.doc.tree.child_by_tag(chain, "ComponentChain") else {
            return value("component chain has no ComponentChain core");
        };
        for tag in ["DefaultOpacity", "DefaultOpacityComponentID"] {
            if let Some(flag) = self.doc.tree.child_by_tag(chain, tag) {
                remove_child(&mut self.doc.tree, chain, flag)?;
            }
        }
        let tree = &mut self.doc.tree;
        let opacity = top(tree, "VideoComponentParam", OPACITY_PARAM_CLASS_ID, "10", None);
        leaf(tree, opacity, "Name", "Opacity", "\n\t\t");
        leaf(tree, opacity, "ParameterID", "1", "\n\t\t");
        leaf(tree, opacity, "StartKeyframe", "-91445760000000000,100.,0,0,0,0,0,0", "\n\t\t");
        leaf(tree, opacity, "CurrentValue", "100", "\n\t\t");
        leaf(tree, opacity, "LowerBound", "0", "\n\t\t");
        leaf(tree, opacity, "UpperBound", "100", "\n\t");
        let api = top(tree, "VideoComponentParam", BLEND_PARAM_CLASS_ID, "10", None);
        leaf(tree, api, "Name", "Blend Mode", "\n\t\t");
        leaf(tree, api, "ParameterControlType", "10", "\n\t\t");
        leaf(tree, api, "DiscontinuousInterpolate", "true", "\n\t\t");
        leaf(tree, api, "ParameterID", "2", "\n\t\t");
        leaf(tree, api, "StartKeyframe", &blend_keyframe(api_blend_value(mode)), "\n\t\t");
        leaf(tree, api, "LowerBound", "0", "\n\t\t");
        leaf(tree, api, "UpperBound", "27", "\n\t");
        let gui = top(tree, "VideoComponentParam", BLEND_PARAM_CLASS_ID, "10", None);
        leaf(tree, gui, "Name", "Blend Mode", "\n\t\t");
        leaf(tree, gui, "DiscontinuousInterpolate", "true", "\n\t\t");
        leaf(tree, gui, "ParameterID", "3", "\n\t\t");
        leaf(tree, gui, "StartKeyframe", &blend_keyframe(mode.value()), "\n\t\t");
        leaf(tree, gui, "LowerBound", "0", "\n\t\t");
        leaf(tree, gui, "UpperBound", "31", "\n\t");
        let mut param_ids = Vec::new();
        for p in [opacity, api, gui] {
            param_ids.push(self.doc.add_object(p)?);
        }

        let tree = &mut self.doc.tree;
        let component = top(tree, "VideoFilterComponent", OPACITY_COMPONENT_CLASS_ID, "9", None);
        let core = tree.sub_element(component, "Component", &[("Version", "7")]);
        tree.set_text(core, Some("\n\t\t\t"));
        tree.set_tail(core, Some("\n\t\t"));
        let holder = tree.sub_element(core, "Params", &[("Version", "1")]);
        tree.set_text(holder, Some("\n\t\t\t\t"));
        tree.set_tail(holder, Some("\n\t\t\t"));
        for (index, param_id) in param_ids.iter().enumerate() {
            let i = index.to_string();
            let entry = tree.sub_element(holder, "Param", &[("Index", &i), ("ObjectRef", param_id)]);
            tree.set_tail(entry, Some(if index < param_ids.len() - 1 { "\n\t\t\t\t" } else { "\n\t\t\t" }));
        }
        leaf(tree, core, "ID", "2", "\n\t\t\t");
        leaf(tree, core, "DisplayName", "Opacity", "\n\t\t\t");
        leaf(tree, core, "Intrinsic", "true", "\n\t\t");
        leaf(tree, component, "MatchName", "AE.ADBE Opacity", "\n\t\t");
        leaf(tree, component, "VideoFilterType", "2", "\n\t");
        let component_id = self.doc.add_object(component)?;

        let tree = &mut self.doc.tree;
        let components = match tree.child_by_tag(inner, "Components") {
            Some(c) => c,
            None => {
                let c = tree.new_element_with("Components", &[("Version", "1")]);
                tree.set_text(c, Some("\n\t\t\t\t"));
                tree.set_tail(c, Some("\n\t\t"));
                match tree.children(inner).last().copied() {
                    Some(last) => tree.set_tail(last, Some("\n\t\t\t")),
                    None => tree.set_text(inner, Some("\n\t\t\t")),
                }
                tree.append(inner, c);
                c
            }
        };
        let entries: Vec<NodeId> = tree.children(components).iter().copied().filter(|&c| tree.tag(c) == "Component").collect();
        if let Some(&last) = entries.last() {
            tree.set_tail(last, Some("\n\t\t\t\t"));
        }
        let index = entries.len().to_string();
        let entry = tree.sub_element(components, "Component", &[("Index", &index), ("ObjectRef", &component_id)]);
        tree.set_tail(entry, Some("\n\t\t\t"));

        let properties = param_ids.iter().map(|id| self.doc.by_object_id[id.as_str()]).collect();
        let id = self.push_component(ComponentData { element: component, track_item: item, properties, sub_components: Vec::new() });
        self.track_item_mut(item).components.push(id);
        Ok(())
    }

    /// `TrackItem.essential_sound_tag` (`ESP.Tag`, `None` when untagged).
    pub fn essential_sound_tag(&self, item: TrackItemId) -> Result<Option<String>> {
        self.doc.tree.findtext(self.track_item(item).element, "ClipTrackItem/TrackItem/Node/Properties/ESP.Tag")
    }

    /// `TrackItem.tag_as_dialogue()`: the Essential Sound panel's Dialogue bag.
    pub fn tag_as_dialogue(&mut self, item: TrackItemId) -> Result<()> {
        let element = self.track_item(item).element;
        let Some(inner) = self.doc.tree.find1(element, "ClipTrackItem/TrackItem") else {
            return value("track item has no TrackItem core");
        };
        if self.doc.tree.child_by_tag(inner, "Node").is_some() {
            return value("track item already carries a property bag");
        }
        let node = bag_node(&mut self.doc.tree, 4, &DIALOGUE_ESSENTIAL_SOUND);
        self.doc.tree.insert(inner, 0, node);
        Ok(())
    }

    /// `TrackItem.set_time_remap(keys)`: set (or create) the time-remap curve; `keys` map
    /// clip-relative timeline time to SOURCE seconds. An unfinished curve gains a terminal key at
    /// the source end. Returns the Speed parameter.
    pub fn set_time_remap(&mut self, item: TrackItemId, keys: &[(Time, Num)]) -> Result<NodeId> {
        if keys.len() < 2 {
            return value("a time-remap curve needs at least two keyframes");
        }
        for &(time, v) in keys {
            POSITIVE_NUMBER.check(v)?;
            if time.ticks < 0 {
                return value("keyframe times are clip-relative, not negative");
            }
        }
        let mut ordered: Vec<(Time, Num)> = keys.to_vec();
        ordered.sort_by_key(|k| k.0.ticks);
        for pair in ordered.windows(2) {
            if pair[0].0.ticks == pair[1].0.ticks {
                return value(format!("duplicate keyframe time {}", pair[0].0.ticks));
            }
        }
        let clip = self.track_item(item).clip;
        let Some(core) = self.doc.tree.child_by_tag(clip, "Clip") else { return value("track item has no clip core") };
        let Some(source_ref) = self.doc.tree.child_by_tag(core, "Source") else {
            return value("track item has no media source");
        };
        let source = self.doc.resolve(source_ref)?;
        let duration_text = self.doc.tree.findtext(source, "OriginalDuration")?.unwrap_or_default();
        if duration_text.is_empty() {
            return value("media source has no duration to bound the curve");
        }
        let upper_bound = py_int(&duration_text)? as f64 / TICKS_PER_SECOND as f64;
        let (last_time, last_value) = *ordered.last().expect("two keys");
        match last_value.py_cmp(Num::Float(upper_bound)) {
            Some(std::cmp::Ordering::Greater) => return value("keyframe values cannot exceed the source duration"),
            Some(std::cmp::Ordering::Less) => {
                let gap = round_half_even((upper_bound - last_value.as_f64()) * TICKS_PER_SECOND as f64)?;
                ordered.push((last_time + Time::new(gap), Num::Float(upper_bound)));
            }
            _ => {}
        }
        let entries: Vec<String> =
            ordered.iter().map(|(t, v)| format!("{},{},6,0,0,0,0,0", t.ticks, format_remap_value(v.as_f64()))).collect();
        let entries = entries.join(";") + ";";
        if let Some(existing) = self.time_remapping(item)? {
            let Some(keyframes) = self.doc.tree.child_by_tag(existing, "Keyframes") else {
                return value("existing Speed parameter has no keyframe list");
            };
            self.doc.tree.set_text(keyframes, Some(entries));
            return Ok(existing);
        }
        let tree = &mut self.doc.tree;
        let param = top(tree, "TimeComponentParam", TIME_PARAM_CLASS_ID, "9", None);
        leaf(tree, param, "Name", "Speed", "\n\t\t");
        leaf(tree, param, "IsTimeVarying", "true", "\n\t\t");
        leaf(tree, param, "IsLocked", "false", "\n\t\t");
        leaf(tree, param, "DiscontinuousInterpolate", "false", "\n\t\t");
        leaf(tree, param, "LowerBound", "0", "\n\t\t");
        leaf(tree, param, "UpperBound", &format_remap_bound(upper_bound)?, "\n\t\t");
        leaf(tree, param, "StartKeyframe", REMAP_START_KEYFRAME, "\n\t\t");
        leaf(tree, param, "Keyframes", &entries, "\n\t\t");
        leaf(tree, param, "CurrentValue", "0", "\n\t\t");
        leaf(tree, param, "ParameterID", "-1", "\n\t");
        let param_id = self.doc.add_object(param)?;
        let tree = &mut self.doc.tree;
        let remap = top(tree, "TimeRemapping", TIME_REMAP_CLASS_ID, "2", None);
        let reference = tree.sub_element(remap, "Keyframes", &[("ObjectRef", &param_id)]);
        tree.set_tail(reference, Some("\n\t"));
        let remap_id = self.doc.add_object(remap)?;
        let tree = &mut self.doc.tree;
        let reference = tree.new_element_with("TimeRemapping", &[("ObjectRef", &remap_id)]);
        let tail = tree.tail(source_ref).map(str::to_string);
        tree.set_tail(reference, tail);
        let position = tree.children(core).iter().position(|&c| c == source_ref).expect("child");
        tree.insert(core, position + 1, reference);
        Ok(param)
    }

    /// `TrackItem.clear_time_remap()`.
    pub fn clear_time_remap(&mut self, item: TrackItemId) -> Result<()> {
        let clip = self.track_item(item).clip;
        let Some(core) = self.doc.tree.child_by_tag(clip, "Clip") else { return Ok(()) };
        let Some(reference) = self.doc.tree.child_by_tag(core, "TimeRemapping") else { return Ok(()) };
        let remap = self.doc.resolve(reference)?;
        if let Some(param_ref) = self.doc.tree.child_by_tag(remap, "Keyframes") {
            let param = self.doc.resolve(param_ref)?;
            self.doc.remove_object(param)?;
        }
        self.doc.remove_object(remap)?;
        remove_child(&mut self.doc.tree, core, reference)
    }

    /// `TrackItem.time_interpolation_type` (`FRAME_SAMPLING` when elided).
    pub fn time_interpolation_type(&self, item: TrackItemId) -> Result<TimeInterpolationType> {
        let stored = self.doc.tree.findtext(self.track_item(item).clip, "TimeInterpolationType")?.unwrap_or_default();
        TimeInterpolationType::from_value(if stored.is_empty() { 0 } else { py_int(&stored)? })
    }

    /// `TrackItem.is_disabled` (`ClipTrackItem/IsMuted`).
    pub fn is_disabled(&self, item: TrackItemId) -> Result<bool> {
        Ok(self.doc.tree.findtext(self.track_item(item).element, "ClipTrackItem/IsMuted")?.as_deref() == Some("true"))
    }

    /// `TrackItem.is_disabled = value`.
    pub fn set_disabled(&mut self, item: TrackItemId, disabled: bool) -> Result<()> {
        let element = self.track_item(item).element;
        let Some(core) = self.doc.tree.child_by_tag(element, "ClipTrackItem") else {
            return Err(Error::Value("track item has no ClipTrackItem".into()));
        };
        set_elided_flag(&mut self.doc.tree, core, "IsMuted", disabled, "true")
    }

    /// `TrackItem.type`: always 1 (clips).
    pub fn track_item_type(&self, _item: TrackItemId) -> i64 {
        1
    }

    /// `repr(track_item)`.
    pub fn track_item_repr(&self, item: TrackItemId) -> Result<String> {
        Ok(format!(
            "TrackItem(name={}, start={}s, end={}s)",
            py_str_repr(&self.track_item_name(item)?),
            fmt_f(self.start(item)?.seconds(), 3),
            fmt_f(self.end(item)?.seconds(), 3)
        ))
    }
}
