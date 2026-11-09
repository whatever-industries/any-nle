//! Decoders for the AVB object classes the converter needs.
//! Field names, order and tag checks follow pyavb's `read()` methods (MIT, Mark Reid;
//! see NOTICE): components.py, trackgroups.py, bin.py, essence.py, misc.py, attributes.py.

use crate::io::{MobId, Reader, Result, Uuid};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// Object-table index (never 0; 0 decodes as Null).
    Ref(u32),
    RefList(Vec<Value>),
    Bytes(Vec<u8>),
    MobId(MobId),
    Uuid(Uuid),
    /// Seconds since the Unix epoch.
    Time(u32),
    List(Vec<Value>),
    Node(Box<Node>),
    /// An ATTR object: named int / string / object-ref / bytes values, in file order.
    Attrs(Vec<(String, Value)>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_index(&self) -> Option<u32> {
        match self {
            Value::Ref(i) => Some(*i),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub class: [u8; 4],
    /// pyavb's Python class name, so dumps line up with the oracle.
    pub type_name: &'static str,
    pub fields: Vec<(&'static str, Value)>,
}

impl Node {
    fn new(class: [u8; 4], type_name: &'static str) -> Node {
        Node { class, type_name, fields: Vec::new() }
    }

    fn set(&mut self, name: &'static str, value: Value) {
        match self.fields.iter_mut().find(|(k, _)| *k == name) {
            Some(slot) => slot.1 = value,
            None => self.fields.push((name, value)),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| *k == name).map(|(_, v)| v)
    }

    pub fn class_str(&self) -> String {
        String::from_utf8_lossy(&self.class).into_owned()
    }
}

/// Reader plus the object-table size, so every reference is range-checked like pyavb's
/// `check_refs`.
pub struct Decoder<'a> {
    pub r: Reader<'a>,
    pub num_objects: u32,
}

impl<'a> Decoder<'a> {
    fn object_ref(&mut self) -> Result<Value> {
        let at = self.r.offset();
        let index = self.r.object_ref()?;
        if index == 0 {
            return Ok(Value::Null);
        }
        if index > self.num_objects {
            return Err(crate::io::Error::new(format!("bad object index {}", index), at));
        }
        Ok(Value::Ref(index))
    }

    fn opt_string(&mut self) -> Result<Value> {
        let s = self.r.string()?;
        Ok(if s.is_empty() { Value::Null } else { Value::Str(s) })
    }

    fn tags(&mut self, a: u8, b: u8) -> Result<()> {
        self.r.assert_tag(a)?;
        self.r.assert_tag(b)
    }

    fn unknown_ext<T>(&self, n: &Node, tag: u8) -> Result<T> {
        self.r.err(format!("{}: unknown ext tag 0x{:02X}", n.class_str(), tag))
    }

    fn count(&mut self, n: i64, min_item_size: usize) -> Result<usize> {
        if n < 0 || (n as u128) * (min_item_size as u128) > self.r.remaining() as u128 {
            return self.r.err(format!("count {} does not fit the payload", n));
        }
        Ok(n as usize)
    }
}

pub const KNOWN_CLASSES: &[&[u8; 4]] = &[
    b"COMP", b"SEQU", b"CLIP", b"SCLP", b"TCCP", b"ECCP", b"TRKR", b"PRCL", b"CTRL", b"FILL", b"TRKG", b"CMPO", b"ABIN", b"BINF", b"ASET", b"BVst", b"ASPI", b"EQMB", b"REPT", b"STRB",
    b"ATTR", b"MDES", b"MDTP", b"MDFM", b"MDNG", b"MDFL", b"MULD", b"PCMA", b"WAVE", b"AIFC", b"MPGA", b"DATD", b"ANCD", b"DIDD", b"CDCI", b"MPGI", b"JPED", b"RGBA", b"MSML", b"FILE", b"WINF", b"URLL",
    b"MCBR", b"MCMR", b"TMBC", b"APOS", b"ABOB", b"DIDP", b"TMCS", b"PRLS", b"TKFX", b"PVOL", b"TNFX",
    b"SLCT", b"RSET", b"WARP", b"SPED", b"MASK", b"PRIT", b"AVUP", b"CCFX", b"GRFX", b"MPGP", b"SHLP",
    b"TKMN", b"TKDS", b"TKPS", b"TKDA", b"TKPA", b"FXPS",
];

/// Classes pyavb represents as plain lists of references (AVBRefList); decoded as a Node
/// with one "items" field and dumped as a bare list.
pub const LIST_CLASSES: &[&[u8; 4]] = &[b"TMCS", b"PRLS"];

/// Decode one object payload. Ok(None) for a class this port does not implement.
pub fn decode(class: [u8; 4], d: &mut Decoder) -> Result<Option<Node>> {
    let type_name = match &class {
        b"COMP" => "Component",
        b"SEQU" => "Sequence",
        b"CLIP" => "Clip",
        b"SCLP" => "SourceClip",
        b"TCCP" => "Timecode",
        b"ECCP" => "Edgecode",
        b"TRKR" => "TrackRef",
        b"PRCL" => "ParamClip",
        b"CTRL" => "ControlClip",
        b"FILL" => "Filler",
        b"TRKG" => "TrackGroup",
        b"CMPO" => "Composition",
        b"ABIN" => "Bin",
        b"BINF" => "BinFirst",
        b"ASET" => "Setting",
        b"BVst" => "BinViewSetting",
        b"ATTR" => "Attributes",
        b"MDES" => "MediaDescriptor",
        b"MDTP" => "TapeDescriptor",
        b"MDFM" => "FilmDescriptor",
        b"MDNG" => "NagraDescriptor",
        b"MDFL" => "MediaFileDescriptor",
        b"MULD" => "MultiDescriptor",
        b"PCMA" => "PCMADescriptor",
        b"WAVE" => "WaveDescriptor",
        b"AIFC" => "AIFCDescriptor",
        b"MPGA" => "MPGADescriptor",
        b"DATD" => "DataDescriptor",
        b"ANCD" => "ANCDataDescriptor",
        b"DIDD" => "DIDDescriptor",
        b"CDCI" => "CDCIDescriptor",
        b"MPGI" => "MPGIDescriptor",
        b"JPED" => "JPEGDescriptor",
        b"RGBA" => "RGBADescriptor",
        b"MSML" => "MSMLocator",
        b"FILE" => "MacFileLocator",
        b"WINF" => "WinFileLocator",
        b"URLL" => "URLLocator",
        b"MCBR" => "BinRef",
        b"MCMR" => "MobRef",
        b"TMBC" => "Marker",
        b"TMCS" => "TimeCrumbList",
        b"PRLS" => "ParameterList",
        b"TKFX" => "TrackEffect",
        b"ASPI" => "AudioSuitePluginEffect",
        b"EQMB" => "EqualizerMultiBand",
        b"PVOL" => "PanVolumeEffect",
        b"TNFX" => "TransitionEffect",
        b"SLCT" => "Selector",
        b"RSET" => "EssenceGroup",
        b"WARP" => "TimeWarp",
        b"REPT" => "Repeat",
        b"STRB" => "StrobeEffect",
        b"SPED" => "MotionEffect",
        b"MASK" => "CaptureMask",
        b"APOS" => "Position",
        b"ABOB" => "BOBPosition",
        b"DIDP" => "DIDPosition",
        b"MPGP" => "MPGPosition",
        b"CCFX" => "ColorCorrectionEffect",
        b"GRFX" => "GraphicEffect",
        b"SHLP" => "ShapeList",
        b"PRIT" => "ParameterItem",
        b"AVUP" => "CFUserParam",
        b"FXPS" => "EffectParamList",
        b"TKMN" => "TrackerManager",
        b"TKDS" => "TrackerDataSlot",
        b"TKPS" => "TrackerParameterSlot",
        b"TKDA" => "TrackerData",
        b"TKPA" => "TrackerParameter",
        _ => return Ok(None),
    };
    let mut n = Node::new(class, type_name);
    match &class {
        b"COMP" => component(d, &mut n)?,
        b"SEQU" => sequence(d, &mut n)?,
        b"CLIP" => clip(d, &mut n)?,
        b"SCLP" => source_clip(d, &mut n)?,
        b"TCCP" => timecode(d, &mut n)?,
        b"ECCP" => edgecode(d, &mut n)?,
        b"TRKR" => track_ref(d, &mut n)?,
        b"PRCL" => param_clip(d, &mut n)?,
        b"CTRL" => control_clip(d, &mut n)?,
        b"FILL" => {
            clip(d, &mut n)?;
            d.tags(0x02, 0x01)?;
            d.r.assert_tag(0x03)?;
        }
        b"TRKG" => track_group(d, &mut n)?,
        b"CMPO" => composition(d, &mut n)?,
        b"ABIN" => bin(d, &mut n)?,
        b"BINF" => bin_first(d, &mut n)?,
        b"ASET" => setting(d, &mut n)?,
        b"BVst" => bin_view_setting(d, &mut n)?,
        b"ATTR" => return Ok(Some(attributes(d)?)),
        b"MDES" => media_descriptor(d, &mut n)?,
        b"MDTP" => tape_descriptor(d, &mut n)?,
        b"MDFM" | b"MDNG" => film_or_nagra_descriptor(d, &mut n)?,
        b"MDFL" => media_file_descriptor(d, &mut n)?,
        b"MULD" => multi_descriptor(d, &mut n)?,
        b"PCMA" => pcma(d, &mut n)?,
        b"WAVE" => wave(d, &mut n)?,
        b"AIFC" => aifc(d, &mut n)?,
        b"MPGA" => mpga(d, &mut n)?,
        b"DATD" | b"ANCD" => data_descriptor(d, &mut n)?,
        b"DIDD" => did(d, &mut n)?,
        b"CDCI" => cdci(d, &mut n)?,
        b"MPGI" => mpgi(d, &mut n)?,
        b"JPED" => jped(d, &mut n)?,
        b"RGBA" => rgba(d, &mut n)?,
        b"MSML" => msm_locator(d, &mut n)?,
        b"FILE" | b"WINF" => file_locator(d, &mut n)?,
        b"URLL" => d.r.assert_tag(0x03)?,
        b"MCBR" => bin_ref(d, &mut n)?,
        b"MCMR" | b"TMBC" => mob_ref(d, &mut n)?,
        b"TMCS" | b"PRLS" => ref_list(d, &mut n)?,
        b"TKFX" | b"PVOL" => track_effect(d, &mut n)?,
        b"ASPI" => audio_suite_plugin_effect(d, &mut n)?,
        b"EQMB" => equalizer_multi_band(d, &mut n)?,
        b"TNFX" => transition(d, &mut n)?,
        b"SLCT" => selector(d, &mut n)?,
        b"RSET" => essence_group(d, &mut n)?,
        b"WARP" | b"SPED" | b"MASK" | b"REPT" | b"STRB" => time_warp(d, &mut n)?,
        b"APOS" | b"ABOB" | b"DIDP" | b"MPGP" => position(d, &mut n)?,
        b"PRIT" => parameter_item(d, &mut n)?,
        b"AVUP" => user_param(d, &mut n)?,
        b"FXPS" => effect_param_list(d, &mut n)?,
        b"CCFX" | b"GRFX" | b"SHLP" => opaque_effect_data(d, &mut n)?,
        b"TKMN" => tracker_manager(d, &mut n)?,
        b"TKDS" => tracker_data_slot(d, &mut n)?,
        b"TKPS" => tracker_parameter_slot(d, &mut n)?,
        b"TKDA" => tracker_data(d, &mut n)?,
        b"TKPA" => tracker_parameter(d, &mut n)?,
        _ => unreachable!(),
    }
    Ok(Some(n))
}

fn is(n: &Node, class: &[u8; 4]) -> bool {
    &n.class == class
}

fn component(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x03)?;
    n.set("left_bob", d.object_ref()?);
    n.set("right_bob", d.object_ref()?);
    n.set("media_kind_id", Value::Int(d.r.s16()? as i64));
    n.set("edit_rate", Value::Float(d.r.exp10_float()?));
    n.set("name", d.opt_string()?);
    n.set("effect_id", d.opt_string()?);
    n.set("attributes", d.object_ref()?);
    n.set("session_attrs", d.object_ref()?);
    n.set("precomputed", d.object_ref()?);
    n.set("param_list", Value::Null);
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(72)?;
                n.set("param_list", d.object_ref()?);
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    Ok(())
}

fn sequence(d: &mut Decoder, n: &mut Node) -> Result<()> {
    component(d, n)?;
    d.tags(0x02, 0x03)?;
    let count = d.r.u32()? as i64;
    let count = d.count(count, 4)?;
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        refs.push(d.object_ref()?);
    }
    n.set("components", Value::RefList(refs));
    d.r.assert_tag(0x03)
}

fn clip(d: &mut Decoder, n: &mut Node) -> Result<()> {
    component(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("length", Value::Int(d.r.u32()? as i64));
    Ok(())
}

fn source_clip(d: &mut Decoder, n: &mut Node) -> Result<()> {
    clip(d, n)?;
    d.tags(0x02, 0x03)?;
    let _mob_id_hi = d.r.u32()?;
    let _mob_id_lo = d.r.u32()?;
    n.set("track_id", Value::Int(d.r.s16()? as i64));
    n.set("start_time", Value::Int(d.r.s32()? as i64));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => n.set("mob_id", Value::MobId(d.r.mob_id()?)),
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn timecode(d: &mut Decoder, n: &mut Node) -> Result<()> {
    clip(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("flags", Value::Int(d.r.u32()? as i64));
    n.set("fps", Value::Int(d.r.u16()? as i64));
    d.r.bytes(6)?;
    n.set("start", Value::Int(d.r.u32()? as i64));
    d.r.assert_tag(0x03)
}

fn edgecode(d: &mut Decoder, n: &mut Node) -> Result<()> {
    clip(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("header", Value::Bytes(d.r.bytes(8)?.to_vec()));
    n.set("film_kind", Value::Int(d.r.u8()? as i64));
    n.set("code_format", Value::Int(d.r.u8()? as i64));
    n.set("base_perf", Value::Int(d.r.u16()? as i64));
    d.r.u32()?; // Unused field in pyavb's Edgecode.read().
    n.set("start_ec", Value::Int(d.r.s32()? as i64));
    d.r.assert_tag(0x03)
}

fn rational_pair(d: &mut Decoder) -> Result<Value> {
    Ok(Value::List(vec![
        Value::Int(d.r.s32()? as i64),
        Value::Int(d.r.s32()? as i64),
    ]))
}

fn param_clip(d: &mut Decoder, n: &mut Node) -> Result<()> {
    clip(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("interp_kind", Value::Int(d.r.s32()? as i64));
    let value_type = d.r.s16()?;
    if !matches!(value_type, 1 | 2 | 4) {
        return d.r.err(format!("PRCL: unknown value type {value_type}"));
    }
    n.set("value_type", Value::Int(value_type as i64));
    let count = d.r.s32()? as i64;
    // Offset, timescale, value and property count; checked before any allocation.
    let min_point = if value_type == 2 { 22 } else { 18 };
    let count = d.count(count, min_point)?;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let mut point = Node::new(*b"\0\0\0\0", "ParamControlPoint");
        point.set("offset", rational_pair(d)?);
        point.set("timescale", Value::Int(d.r.s32()? as i64));
        let value = match value_type {
            1 => Value::Int(d.r.s32()? as i64),
            2 => Value::Float(d.r.f64()?),
            4 => d.object_ref()?,
            _ => unreachable!(),
        };
        point.set("value", value);
        let pp_count_raw = d.r.s16()? as i64;
        let pp_count = d.count(pp_count_raw, 8)?;
        let mut props = Vec::with_capacity(pp_count);
        for _ in 0..pp_count {
            let mut prop = Node::new(*b"\0\0\0\0", "ParamControlPointProperty");
            prop.set("code", Value::Int(d.r.s16()? as i64));
            let kind = d.r.s16()?;
            prop.set("type", Value::Int(kind as i64));
            prop.set(
                "value",
                match kind {
                    1 => Value::Int(d.r.s32()? as i64),
                    2 => Value::Float(d.r.f64()?),
                    _ => return d.r.err(format!("PRCL: unknown point property type {kind}")),
                },
            );
            props.push(Value::Node(Box::new(prop)));
        }
        point.set("pp", Value::List(props));
        points.push(Value::Node(Box::new(point)));
    }
    n.set("control_points", Value::List(points));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(71)?;
                n.set("extrap_kind", Value::Int(d.r.s32()? as i64));
            }
            0x02 => {
                d.r.assert_tag(71)?;
                n.set("fields", Value::Int(d.r.s32()? as i64));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn control_clip(d: &mut Decoder, n: &mut Node) -> Result<()> {
    clip(d, n)?;
    d.tags(0x02, 0x03)?;
    n.set("interp_kind", Value::Int(d.r.s32()? as i64));
    let count_raw = d.r.s32()? as i64;
    let count = d.count(count_raw, 23)?;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let mut point = Node::new(*b"\0\0\0\0", "ControlPoint");
        point.set("offset", rational_pair(d)?);
        point.set("time_scale", Value::Int(d.r.s32()? as i64));
        if !d.r.bool()? {
            return d.r.err("CTRL: control point has no value");
        }
        point.set("value", rational_pair(d)?);
        let pp_count_raw = d.r.s16()? as i64;
        let pp_count = d.count(pp_count_raw, 10)?;
        let mut props = Vec::with_capacity(pp_count);
        for _ in 0..pp_count {
            let mut prop = Node::new(*b"\0\0\0\0", "ControlPointProperty");
            prop.set("code", Value::Int(d.r.s16()? as i64));
            prop.set("value", rational_pair(d)?);
            props.push(Value::Node(Box::new(prop)));
        }
        point.set("pp", Value::List(props));
        points.push(Value::Node(Box::new(point)));
    }
    n.set("control_points", Value::List(points));
    d.r.assert_tag(0x03)
}

fn track_ref(d: &mut Decoder, n: &mut Node) -> Result<()> {
    clip(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("relative_scope", Value::Int(d.r.s16()? as i64));
    n.set("relative_track", Value::Int(d.r.s16()? as i64));
    d.r.assert_tag(0x03)
}

const TRACK_LABEL: u16 = 1 << 0;
const TRACK_ATTRIBUTES: u16 = 1 << 1;
const TRACK_COMPONENT: u16 = 1 << 2;
const TRACK_FILLER_PROXY: u16 = 1 << 3;
const TRACK_BOB_DATA: u16 = 1 << 4;
const TRACK_CONTROL_CODE: u16 = 1 << 5;
const TRACK_CONTROL_SUB_CODE: u16 = 1 << 6;
const TRACK_START_POS: u16 = 1 << 7;
const TRACK_READ_ONLY: u16 = 1 << 8;
const TRACK_SESSION_ATTR: u16 = 1 << 9;
const TRACK_UNKNOWN: u16 = 0xFC00;

fn track_group(d: &mut Decoder, n: &mut Node) -> Result<()> {
    component(d, n)?;
    d.tags(0x02, 0x08)?;
    n.set("mc_mode", Value::Int(d.r.u8()? as i64));
    n.set("length", Value::Int(d.r.s32()? as i64));
    n.set("num_scalars", Value::Int(d.r.s32()? as i64));
    let count = d.r.s32()? as i64;
    let count = d.count(count, 2)?;
    let mut tracks = Vec::with_capacity(count);
    for _ in 0..count {
        let flags = d.r.u16()?;
        if flags & TRACK_UNKNOWN != 0 {
            return d.r.err(format!("unknown track flags 0x{:04X}", flags));
        }
        // Read order is pyavb's, which differs from the bit order for session_attr.
        let mut t = Node::new(*b"\0\0\0\0", "Track");
        if flags & TRACK_LABEL != 0 {
            t.set("index", Value::Int(d.r.s16()? as i64));
        }
        if flags & TRACK_ATTRIBUTES != 0 {
            t.set("attributes", d.object_ref()?);
        }
        if flags & TRACK_SESSION_ATTR != 0 {
            t.set("session_attr", d.object_ref()?);
        }
        if flags & TRACK_COMPONENT != 0 {
            t.set("component", d.object_ref()?);
        }
        if flags & TRACK_FILLER_PROXY != 0 {
            t.set("filler_proxy", d.object_ref()?);
        }
        if flags & TRACK_BOB_DATA != 0 {
            t.set("bob_data", d.object_ref()?);
        }
        if flags & TRACK_CONTROL_CODE != 0 {
            t.set("control_code", Value::Int(d.r.s16()? as i64));
        }
        if flags & TRACK_CONTROL_SUB_CODE != 0 {
            t.set("control_sub_code", Value::Int(d.r.s16()? as i64));
        }
        if flags & TRACK_START_POS != 0 {
            t.set("start_pos", Value::Int(d.r.s32()? as i64));
        }
        if flags & TRACK_READ_ONLY != 0 {
            t.set("read_only", Value::Bool(d.r.bool()?));
        }
        tracks.push(t);
    }
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                for t in tracks.iter_mut() {
                    d.r.assert_tag(69)?;
                    t.set("lock_number", Value::Int(d.r.s16()? as i64));
                }
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    n.set("tracks", Value::List(tracks.into_iter().map(|t| Value::Node(Box::new(t))).collect()));
    Ok(())
}

fn composition(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_group(d, n)?;
    d.tags(0x02, 0x02)?;
    let _mob_id_lo = d.r.u32()?;
    let _mob_id_hi = d.r.u32()?;
    n.set("last_modified", Value::Time(d.r.u32()?));
    n.set("mob_type_id", Value::Int(d.r.u8()? as i64));
    n.set("usage_code", Value::Int(d.r.s32()? as i64));
    n.set("descriptor", d.object_ref()?);
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(71)?;
                n.set("creation_time", Value::Time(d.r.u32()?));
            }
            0x02 => n.set("mob_id", Value::MobId(d.r.mob_id()?)),
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn bin(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.r.assert_tag(0x02)?;
    let version = d.r.u8()?;
    if version != 0x0e && version != 0x0f {
        return d.r.err(format!("unknown bin version 0x{:02X}", version));
    }
    n.set("large_bin", Value::Bool(version == 0x0f));
    n.set("view_setting", d.object_ref()?);
    n.set("uid", Value::UInt(d.r.u64()?));
    let count = if version == 0x0e { d.r.u16()? as i64 } else { d.r.u32()? as i64 };
    let count = d.count(count, 13)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        let mut item = Node::new(*b"\0\0\0\0", "BinItem");
        item.set("mob", d.object_ref()?);
        item.set("x", Value::Int(d.r.s16()? as i64));
        item.set("y", Value::Int(d.r.s16()? as i64));
        item.set("keyframe", Value::Int(d.r.s32()? as i64));
        item.set("user_placed", Value::Bool(d.r.bool()?));
        items.push(Value::Node(Box::new(item)));
    }
    n.set("items", Value::List(items));
    n.set("display_mask", Value::Int(d.r.s32()? as i64));
    n.set("display_mode", Value::Int(d.r.s16()? as i64));
    n.set("sifted", Value::Bool(d.r.bool()?));
    let mut sifted = Vec::new();
    for _ in 0..6 {
        let method = d.r.s16()? as i64;
        let string = d.r.string()?;
        let column = d.r.string()?;
        sifted.push(Value::List(vec![Value::Int(method), Value::Str(string), Value::Str(column)]));
    }
    n.set("sifted_settings", Value::List(sifted));
    let sort_count = d.r.s16()? as i64;
    let sort_count = d.count(sort_count, 3)?;
    let mut sort = Vec::new();
    for _ in 0..sort_count {
        let direction = d.r.u8()? as i64;
        sort.push(Value::List(vec![Value::Int(direction), Value::Str(d.r.string()?)]));
    }
    n.set("sort_columns", Value::List(sort));
    n.set("mac_font", Value::Int(d.r.s16()? as i64));
    n.set("mac_font_size", Value::Int(d.r.s16()? as i64));
    n.set("mac_image_scale", Value::Int(d.r.s16()? as i64));
    n.set("home_rect", Value::List(d.r.rect()?.iter().map(|&v| Value::Int(v as i64)).collect()));
    n.set("background_color", Value::List(d.r.rgb_color()?.iter().map(|&v| Value::Int(v as i64)).collect()));
    n.set("forground_color", Value::List(d.r.rgb_color()?.iter().map(|&v| Value::Int(v as i64)).collect()));
    n.set("ql_image_scale", Value::Int(d.r.s16()? as i64));
    n.set("attributes", d.object_ref()?);
    n.set("was_iconic", Value::Bool(d.r.bool()?));
    if n.class == *b"ABIN" {
        d.r.assert_tag(0x03)?;
    }
    Ok(())
}

fn bin_first(d: &mut Decoder, n: &mut Node) -> Result<()> {
    bin(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("unknown_s32", Value::Int(d.r.s32()? as i64));
    d.r.assert_tag(0x03)
}

fn setting(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x06)?;
    n.set("name", Value::Str(d.r.string()?));
    n.set("kind", Value::Str(d.r.string()?));
    n.set("attr_count", Value::Int(d.r.s16()? as i64));
    n.set("attr_type", Value::Int(d.r.s16()? as i64));
    n.set("attributes", d.object_ref()?);
    Ok(())
}

fn bin_view_setting(d: &mut Decoder, n: &mut Node) -> Result<()> {
    setting(d, n)?;
    d.tags(0x02, 10)?;
    let count_raw = d.r.u16()? as i64;
    let count = d.count(count_raw, 7)?;
    let mut columns = Vec::with_capacity(count);
    for _ in 0..count {
        let mut column = Node::new(*b"\0\0\0\0", "BinViewColumn");
        column.set("title", Value::Str(d.r.string()?));
        column.set("format", Value::Int(d.r.s16()? as i64));
        column.set("type", Value::Int(d.r.s16()? as i64));
        column.set("hidden", Value::Bool(d.r.bool()?));
        columns.push(Value::Node(Box::new(column)));
    }
    n.set("columns", Value::List(columns));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(69)?;
                let count_raw = d.r.s16()? as i64;
                // E + id + G + size + L + ignored s32 + payload (possibly empty).
                let count = d.count(count_raw, 13)?;
                let mut descriptors = Vec::with_capacity(count);
                for _ in 0..count {
                    d.r.assert_tag(69)?;
                    let id = d.r.s16()? as i64;
                    d.r.assert_tag(71)?;
                    let size_raw = d.r.s32()? as i64;
                    d.r.assert_tag(76)?;
                    d.r.s32()?; // Prefix ignored by pyavb's BinViewSetting.read().
                    let size = d.count(size_raw, 1)?;
                    let bytes = d.r.bytes(size)?;
                    let value = std::str::from_utf8(bytes)
                        .map_err(|_| crate::io::Error::new("BVst: invalid UTF-8 format descriptor", d.r.offset() - size))?;
                    let mut descriptor = Node::new(*b"\0\0\0\0", "BinViewFormatDescriptor");
                    descriptor.set("vcid_free_column_id", Value::Int(id));
                    descriptor.set("format_descriptor", Value::Str(value.to_owned()));
                    descriptors.push(Value::Node(Box::new(descriptor)));
                }
                n.set("format_descriptors", Value::List(descriptors));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn attributes(d: &mut Decoder) -> Result<Node> {
    d.tags(0x02, 0x01)?;
    let count = d.r.u32()? as i64;
    let count = d.count(count, 6)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = d.r.u32()?;
        let name = d.r.string()?;
        let value = match kind {
            1 => Value::Int(d.r.s32()? as i64),
            2 => Value::Str(d.r.string()?),
            3 => d.object_ref()?,
            4 => {
                let size = d.r.u32()? as usize;
                Value::Bytes(d.r.bytes(size)?.to_vec())
            }
            _ => return d.r.err(format!("attribute {:?}: unknown type {}", name, kind)),
        };
        entries.push((name, value));
    }
    d.r.assert_tag(0x03)?;
    let mut n = Node::new(*b"ATTR", "Attributes");
    n.set("entries", Value::Attrs(entries));
    Ok(n)
}

fn media_descriptor(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x03)?;
    n.set("mob_kind", Value::Int(d.r.u8()? as i64));
    n.set("locator", d.object_ref()?);
    n.set("intermediate", Value::Bool(d.r.bool()?));
    n.set("physical_media", d.object_ref()?);
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(65)?;
                if d.r.s32()? != 16 {
                    return d.r.err("descriptor UUID length is not 16");
                }
                n.set("uuid", Value::Uuid(d.r.raw_uuid()?));
            }
            0x02 => {
                d.r.assert_tag(65)?;
                let size = d.r.s32()? as i64;
                let size = d.count(size, 1)?;
                n.set("wchar", Value::Bytes(d.r.bytes(size)?.to_vec()));
            }
            0x03 => {
                d.r.assert_tag(72)?;
                n.set("attributes", d.object_ref()?);
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    if is(n, b"MDES") {
        d.r.assert_tag(0x03)?;
    }
    Ok(())
}

fn media_file_descriptor(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_descriptor(d, n)?;
    d.tags(0x02, 0x03)?;
    n.set("edit_rate", Value::Float(d.r.exp10_float()?));
    n.set("length", Value::Int(d.r.s32()? as i64));
    n.set("is_omfi", Value::Int(d.r.s16()? as i64));
    n.set("data_offset", Value::Int(d.r.s32()? as i64));
    if is(n, b"MDFL") {
        d.r.assert_tag(0x03)?;
    }
    Ok(())
}

fn tape_descriptor(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_descriptor(d, n)?;
    d.tags(0x02, 0x02)?;
    n.set("cframe", Value::Int(d.r.s16()? as i64));
    d.r.assert_tag(0x03)
}

fn film_or_nagra_descriptor(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    d.r.assert_tag(0x03)
}

fn multi_descriptor(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    let count_raw = d.r.s32()? as i64;
    let count = d.count(count_raw, 4)?;
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        refs.push(d.object_ref()?);
    }
    n.set("descriptors", Value::RefList(refs));
    d.r.assert_tag(0x03)
}

fn aifc(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    if d.r.bytes(4)? != b"FORM" {
        return d.r.err("AIFC summary does not start with FORM");
    }
    let size_bytes = d.r.bytes(4)?;
    let size = u32::from_be_bytes(size_bytes.try_into().unwrap()) as usize;
    n.set("summary", Value::Bytes(d.r.bytes(size)?.to_vec()));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(71)?;
                n.set("data_pos", Value::Int(d.r.s32()? as i64));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn mpga(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("channels", Value::Int(d.r.u16()? as i64));
    n.set("quantization_bits", Value::Int(d.r.u16()? as i64));
    n.set("sample_rate", Value::Float(d.r.exp10_float()?));
    n.set("locked", Value::Bool(d.r.bool()?));
    n.set("audio_ref_level", Value::Int(d.r.s16()? as i64));
    n.set("electro_spatial_formulation", Value::Int(d.r.s32()? as i64));
    n.set("dial_norm", Value::Int(d.r.u16()? as i64));
    n.set("coding_format", Value::Int(d.r.u32()? as i64));
    n.set("bit_rate", Value::Int(d.r.u32()? as i64));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(77)?;
                n.set("sub_frame_alignment", Value::UInt(d.r.u64()?));
            }
            0x02 => {
                d.r.assert_tag(77)?;
                n.set("origin", Value::UInt(d.r.u64()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn data_descriptor(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("is_offset_to_frame_indexes_valid", Value::Bool(d.r.bool()?));
    n.set("offset_to_frame_indexes", Value::UInt(d.r.u64()?));
    n.set("first_frame_offset", Value::Int(d.r.s32()? as i64));
    n.set("min_sample_size", Value::Int(d.r.s32()? as i64));
    n.set("max_sample_size", Value::Int(d.r.s32()? as i64));
    if is(n, b"ANCD") {
        d.tags(0x02, 0x01)?;
        n.set("manifest_element_count", Value::Int(d.r.s32()? as i64));
        d.r.assert_tag(0x03)?;
    }
    Ok(()) // pyavb's bare DATD has no terminator.
}

fn pcma(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("channels", Value::Int(d.r.u16()? as i64));
    n.set("quantization_bits", Value::Int(d.r.u16()? as i64));
    n.set("sample_rate", Value::Float(d.r.exp10_float()?));
    n.set("locked", Value::Bool(d.r.bool()?));
    n.set("audio_ref_level", Value::Int(d.r.s16()? as i64));
    n.set("electro_spatial_formulation", Value::Int(d.r.s32()? as i64));
    n.set("dial_norm", Value::Int(d.r.u16()? as i64));
    n.set("coding_format", Value::Int(d.r.u32()? as i64));
    n.set("block_align", Value::Int(d.r.u32()? as i64));
    n.set("sequence_offset", Value::Int(d.r.u16()? as i64));
    n.set("average_bps", Value::Int(d.r.u32()? as i64));
    n.set("has_peak_envelope_data", Value::Bool(d.r.bool()?));
    n.set("peak_envelope_version", Value::Int(d.r.s32()? as i64));
    n.set("peak_envelope_format", Value::Int(d.r.s32()? as i64));
    n.set("points_per_peak_value", Value::Int(d.r.s32()? as i64));
    n.set("peak_envelope_block_size", Value::Int(d.r.s32()? as i64));
    n.set("peak_channel_count", Value::Int(d.r.s32()? as i64));
    n.set("peak_frame_count", Value::Int(d.r.s32()? as i64));
    n.set("peak_of_peaks_offset", Value::UInt(d.r.u64()?));
    n.set("peak_envelope_timestamp", Value::Int(d.r.s32()? as i64));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(77)?;
                n.set("ebu_timestamp", Value::Int(d.r.s64()?));
            }
            0x03 => {
                d.r.assert_tag(76)?;
                n.set("timecode_framerate", Value::Str(d.r.string()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn wave(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x01)?;
    if d.r.bytes(4)? != b"RIFF" {
        return d.r.err("WAVE summary does not start with RIFF");
    }
    let raw = d.r.bytes(4)?;
    let size = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    n.set("summary", Value::Bytes(d.r.bytes(size)?.to_vec()));
    d.r.assert_tag(0x03)
}

fn tagged_s32(d: &mut Decoder) -> Result<i64> {
    d.r.assert_tag(71)?;
    Ok(d.r.s32()? as i64)
}

fn tagged_points(d: &mut Decoder, count: usize) -> Result<Value> {
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let x = tagged_s32(d)?;
        let y = tagged_s32(d)?;
        points.push(Value::List(vec![Value::Int(x), Value::Int(y)]));
    }
    Ok(Value::List(points))
}

fn did(d: &mut Decoder, n: &mut Node) -> Result<()> {
    media_file_descriptor(d, n)?;
    d.tags(0x02, 0x02)?;
    for name in [
        "stored_height", "stored_width", "sampled_height", "sampled_width", "sampled_x_offset",
        "sampled_y_offset", "display_height", "display_width", "display_x_offset", "display_y_offset",
    ] {
        n.set(name, Value::Int(d.r.s32()? as i64));
    }
    n.set("frame_layout", Value::Int(d.r.s16()? as i64));
    let num = d.r.s32()? as i64;
    let den = d.r.s32()? as i64;
    n.set("aspect_ratio", Value::List(vec![Value::Int(num), Value::Int(den)]));
    let line_map_size = d.r.s32()? as i64;
    let line_map_count = d.count(line_map_size.max(0) / 4, 4)?;
    let mut line_map = Vec::with_capacity(line_map_count);
    for _ in 0..line_map_count {
        line_map.push(Value::Int(d.r.s32()? as i64));
    }
    n.set("line_map", Value::List(line_map));
    n.set("alpha_transparency", Value::Int(d.r.s32()? as i64));
    n.set("uniformness", Value::Bool(d.r.bool()?));
    n.set("did_image_size", Value::Int(d.r.s32()? as i64));
    n.set("next_did_desc", d.object_ref()?);
    let fourcc = d.r.fourcc()?;
    n.set("compress_method", Value::Bytes(fourcc.to_vec()));
    n.set("resolution_id", Value::Int(d.r.s32()? as i64));
    n.set("image_alignment_factor", Value::Int(d.r.s32()? as i64));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(69)?;
                n.set("frame_index_byte_order", Value::Int(d.r.s16()? as i64));
            }
            0x02 => n.set("frame_sample_size", Value::Int(tagged_s32(d)?)),
            0x03 => n.set("first_frame_offset", Value::Int(tagged_s32(d)?)),
            0x04 => {
                n.set("client_fill_start", Value::Int(tagged_s32(d)?));
                n.set("client_fill_end", Value::Int(tagged_s32(d)?));
            }
            0x05 => n.set("offset_to_rle_frame_index", Value::Int(tagged_s32(d)?)),
            0x06 => n.set("frame_start_offset", Value::Int(tagged_s32(d)?)),
            0x08 => {
                n.set("valid_box", tagged_points(d, 4)?);
                n.set("essence_box", tagged_points(d, 4)?);
                n.set("source_box", tagged_points(d, 4)?);
            }
            0x09 => {
                n.set("framing_box", tagged_points(d, 4)?);
                n.set("reformatting_option", Value::Int(tagged_s32(d)?));
            }
            0x0A => {
                d.r.assert_tag(80)?;
                n.set("transfer_characteristic", Value::Uuid(d.r.raw_uuid()?));
            }
            0x0B => {
                d.r.assert_tag(80)?;
                n.set("color_primaries", Value::Uuid(d.r.raw_uuid()?));
                d.r.assert_tag(80)?;
                n.set("coding_equations", Value::Uuid(d.r.raw_uuid()?));
            }
            0x0C => {
                d.r.assert_tag(80)?;
                n.set("essence_compression", Value::Uuid(d.r.raw_uuid()?));
            }
            0x0E => {
                d.r.assert_tag(68)?;
                n.set("essence_element_size_kind", Value::Int(d.r.u8()? as i64));
            }
            0x0F => {
                d.r.assert_tag(66)?;
                n.set("frame_checked_with_mapper", Value::Bool(d.r.bool()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    if is(n, b"DIDD") {
        d.r.assert_tag(0x03)?;
    }
    Ok(())
}

fn cdci(d: &mut Decoder, n: &mut Node) -> Result<()> {
    did(d, n)?;
    d.tags(0x02, 0x02)?;
    n.set("horizontal_subsampling", Value::Int(d.r.u32()? as i64));
    n.set("vertical_subsampling", Value::Int(d.r.u32()? as i64));
    n.set("component_width", Value::Int(d.r.s32()? as i64));
    n.set("color_sitting", Value::Int(d.r.s16()? as i64));
    n.set("black_ref_level", Value::Int(d.r.u32()? as i64));
    n.set("white_ref_level", Value::Int(d.r.u32()? as i64));
    n.set("color_range", Value::Int(d.r.u32()? as i64));
    n.set("frame_index_offset", Value::Int(d.r.s64()?));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(72)?;
                n.set("alpha_sampled_width", Value::Int(d.r.u32()? as i64));
            }
            0x02 => {
                d.r.assert_tag(72)?;
                n.set("ignore_bw", Value::Int(d.r.u32()? as i64));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    if is(n, b"CDCI") {
        d.r.assert_tag(0x03)?;
    }
    Ok(())
}

fn mpgi(d: &mut Decoder, n: &mut Node) -> Result<()> {
    cdci(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("mpeg_version", Value::Int(d.r.u8()? as i64));
    n.set("profile", Value::Int(d.r.u8()? as i64));
    n.set("gop_structure", Value::Int(d.r.u8()? as i64));
    n.set("stream_type", Value::Int(d.r.u8()? as i64));
    n.set("random_access", Value::Bool(d.r.bool()?));
    n.set("leading_discard", Value::Bool(d.r.bool()?));
    n.set("trailing_discard", Value::Bool(d.r.bool()?));
    n.set("min_gop_length", Value::Int(d.r.u16()? as i64));
    n.set("max_gop_length", Value::Int(d.r.u16()? as i64));
    let len_raw = d.r.s32()? as i64;
    let len = d.count(len_raw, 1)?;
    n.set("sequence_hdr", Value::Bytes(d.r.bytes(len)?.to_vec()));
    d.r.assert_tag(0x03)
}

fn jped(d: &mut Decoder, n: &mut Node) -> Result<()> {
    cdci(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("jpeg_table_id", Value::Int(d.r.s32()? as i64));
    n.set("jpeg_frame_index_offset", Value::UInt(d.r.u64()?));
    let len_raw = d.r.s32()? as i64;
    let len = d.count(len_raw, 1)?;
    n.set("quantization_tables", Value::Bytes(d.r.bytes(len)?.to_vec()));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(71)?;
                n.set("image_start_align", Value::Int(d.r.s32()? as i64));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn rgba(d: &mut Decoder, n: &mut Node) -> Result<()> {
    did(d, n)?;
    d.tags(0x02, 0x01)?;
    let layout_len = d.r.u32()? as usize;
    let layout = d.r.bytes(layout_len)?;
    let sizes_len = d.r.u32()? as usize;
    if sizes_len != layout_len {
        return d.r.err("RGBA pixel layout/structure lengths differ");
    }
    let sizes = d.r.bytes(sizes_len)?;
    let mut elements = Vec::with_capacity(layout_len);
    for (&code, &size) in layout.iter().zip(sizes) {
        let mut element = Node::new(*b"\0\0\0\0", "PixelLayoutElement");
        element.set("Code", Value::Int(code as i64));
        element.set("Size", Value::Int(size as i64));
        elements.push(Value::Node(Box::new(element)));
    }
    n.set("pixel_layout", Value::List(elements));
    for name in ["palette layout", "palette structure", "palette"] {
        if d.r.u32()? != 0 {
            return d.r.err(format!("RGBA nonzero {name} size"));
        }
    }
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(77)?;
                n.set("frame_index_offset", Value::UInt(d.r.u64()?));
            }
            0x02 => {
                d.r.assert_tag(66)?;
                n.set("has_comp_min_ref", Value::Bool(d.r.bool()?));
                d.r.assert_tag(72)?;
                n.set("comp_min_ref", Value::Int(d.r.u32()? as i64));
                d.r.assert_tag(66)?;
                n.set("has_comp_max_ref", Value::Bool(d.r.bool()?));
                d.r.assert_tag(72)?;
                n.set("comp_max_ref", Value::Int(d.r.u32()? as i64));
            }
            0x03 => {
                d.r.assert_tag(72)?;
                n.set("alpha_min_ref", Value::Int(d.r.u32()? as i64));
                d.r.assert_tag(72)?;
                n.set("alpha_max_ref", Value::Int(d.r.u32()? as i64));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn msm_locator(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x02)?;
    let _mob_id_hi = d.r.u32()?;
    let _mob_id_lo = d.r.u32()?;
    n.set("last_known_volume", Value::Str(d.r.string()?));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => n.set("domain_type", Value::Int(tagged_s32(d)?)),
            0x02 => n.set("mob_id", Value::MobId(d.r.mob_id()?)),
            0x03 => {
                d.r.assert_tag(76)?;
                n.set("last_known_volume_utf8", Value::Str(d.r.string_utf8()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn file_locator(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x02)?;
    n.set("path", Value::Str(d.r.string()?));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(76)?;
                n.set("path_posix", Value::Str(d.r.string()?));
            }
            0x02 => {
                d.r.assert_tag(76)?;
                n.set("path_utf8", Value::Str(d.r.string_utf8()?));
            }
            0x03 => {
                d.r.assert_tag(76)?;
                n.set("path2_utf8", Value::Str(d.r.string_utf8()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn bin_ref(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    n.set("uid_high", Value::Int(d.r.s32()? as i64));
    n.set("uid_low", Value::Int(d.r.s32()? as i64));
    n.set("name", Value::Str(d.r.string()?));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(76)?;
                n.set("name_utf8", Value::Str(d.r.string_utf8()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn mob_ref(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    let _mob_hi = d.r.u32()?;
    let _mob_lo = d.r.u32()?;
    n.set("position", Value::Int(d.r.s32()? as i64));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => n.set("mob_id", Value::MobId(d.r.mob_id()?)),
            _ => return d.unknown_ext(n, tag),
        }
    }
    if is(n, b"MCMR") {
        return d.r.assert_tag(0x03);
    }
    // TMBC (Marker) continues.
    d.tags(0x02, 0x03)?;
    n.set("comp_offset", Value::Int(d.r.s32()? as i64));
    n.set("attributes", d.object_ref()?);
    if d.r.s16()? != 1 {
        return d.r.err("marker color version is not 1");
    }
    n.set("color", Value::List(vec![
        Value::Int(d.r.u16()? as i64),
        Value::Int(d.r.u16()? as i64),
        Value::Int(d.r.u16()? as i64),
    ]));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(66)?;
                n.set("handled_codes", Value::Bool(d.r.bool()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

/// pyavb misc.EffectParamList. Variable-sized user_param is kept opaque (including AniMatte
/// shape data); file.rs also retains the complete raw FXPS chunk for the existing extractor.
fn effect_param_list(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x12)?;
    n.set("orig_length", Value::Int(d.r.s32()? as i64));
    n.set("window_offset", Value::Int(d.r.s32()? as i64));
    let raw_count = d.r.s32()?;
    n.set("keyframe_size", Value::Int(d.r.s32()? as i64));
    // Minimum per parameter: 20 scalar s32, 2 length s32, 4 bool, 4 s16,
    // one s8, and one final bool = 102 bytes.
    let count = d.count(raw_count as i64, 102)?;
    let mut parameters = Vec::with_capacity(count);
    for _ in 0..count {
        let mut p = Node::new(*b"FXPS", "EffectParam");
        for field in [
            "percent_time", "level", "pos_x", "floor_x", "ceil_x", "pos_y", "floor_y",
            "ceil_y", "scale_x", "scale_y", "crop_left", "crop_right", "crop_top",
            "crop_bottom",
        ] {
            p.set(field, Value::Int(d.r.s32()? as i64));
        }
        let mut box_values = Vec::with_capacity(4);
        for _ in 0..4 {
            box_values.push(Value::Int(d.r.s32()? as i64));
        }
        p.set("box", Value::List(box_values));
        for field in ["box_xscale", "box_yscale", "box_xpos", "box_ypos"] {
            p.set(field, Value::Bool(d.r.bool()?));
        }
        for field in ["border_width", "border_soft"] {
            p.set(field, Value::Int(d.r.s32()? as i64));
        }
        for field in ["splill_gain2", "splill_gain", "splill_soft2", "splill_soft"] {
            p.set(field, Value::Int(d.r.s16()? as i64));
        }
        p.set("enable_key_flags", Value::Int(d.r.u8()? as i8 as i64));
        let raw_color_count = d.r.s32()?;
        let color_count = d.count(raw_color_count as i64, 4)?;
        let mut colors = Vec::with_capacity(color_count);
        for _ in 0..color_count {
            colors.push(Value::Int(d.r.s32()? as i64));
        }
        p.set("colors", Value::List(colors));
        let raw_param_size = d.r.s32()?;
        let param_size = d.count(raw_param_size as i64, 1)?;
        p.set("user_param", Value::Bytes(d.r.bytes(param_size)?.to_vec()));
        p.set("selected", Value::Bool(d.r.bool()?));
        parameters.push(Value::Node(Box::new(p)));
    }
    n.set("parameters", Value::List(parameters));
    d.r.assert_tag(0x03)
}

/// pyavb misc.ParameterItem: an effect parameter (GUID, then an int, double or object value).
fn parameter_item(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x02)?;
    n.set("uuid", Value::Uuid(d.r.raw_uuid()?));
    let value_type = d.r.s16()?;
    n.set("value_type", Value::Int(value_type as i64));
    let value = match value_type {
        1 => Value::Int(d.r.s32()? as i64),
        2 => Value::Float(d.r.f64()?),
        4 => d.object_ref()?,
        other => return d.r.err(format!("unknown value_type: {}", other)),
    };
    n.set("value", value);
    // pyavb read_string: 0xFFFF reads as "".
    n.set("name", Value::Str(d.r.string()?));
    n.set("enable", Value::Bool(d.r.bool()?));
    n.set("control_track", d.object_ref()?);
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(66)?;
                n.set("contribs_to_sig", Value::Bool(d.r.bool()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

/// pyavb misc.CFUserParam: a typed binary parameter payload (e.g. Color Correction curves).
fn user_param(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    let byte_order = d.r.s16()?;
    if byte_order != 0x4949 {
        return d.r.err(format!("AVUP byte order {:#06x}", byte_order));
    }
    n.set("byte_order", Value::Int(byte_order as i64));
    n.set("uuid", Value::Uuid(d.r.raw_uuid()?));
    let size1 = d.r.s32()?;
    let size2 = d.r.s32()?;
    if size2 as i64 != size1 as i64 - 4 || size2 < 0 {
        return d.r.err(format!("AVUP value sizes {} / {}", size1, size2));
    }
    let size = d.count(size2 as i64, 1)?;
    n.set("data", Value::Bytes(d.r.bytes(size)?.to_vec()));
    d.r.assert_tag(0x03)
}

/// TMCS counts with an s16, PRLS with an s32.
fn ref_list(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    let count = if is(n, b"TMCS") { d.r.s16()? as i64 } else { d.r.s32()? as i64 };
    let count = d.count(count, 4)?;
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        refs.push(d.object_ref()?);
    }
    n.set("items", Value::RefList(refs));
    d.r.assert_tag(0x03)
}

/// APOS -> ABOB -> DIDP; each level closes with 0x03 only when it is the concrete class.
fn position(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    let _mob_id_hi = d.r.u32()?;
    let _mob_id_lo = d.r.u32()?;
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => n.set("mob_id", Value::MobId(d.r.mob_id()?)),
            _ => return d.unknown_ext(n, tag),
        }
    }
    if is(n, b"APOS") {
        return d.r.assert_tag(0x03);
    }
    d.tags(0x02, 0x01)?;
    n.set("sample_num", Value::Int(d.r.s32()? as i64));
    n.set("length", Value::Int(d.r.s32()? as i64));
    n.set("track_type", Value::Int(d.r.s16()? as i64));
    n.set("track_index", Value::Int(d.r.s16()? as i64));
    if is(n, b"ABOB") {
        return d.r.assert_tag(0x03);
    }
    d.tags(0x02, 0x01)?;
    n.set("strip", Value::Int(d.r.s32()? as i64));
    n.set("offset", Value::UInt(d.r.u64()?));
    n.set("byte_length", Value::UInt(d.r.u64()?));
    n.set("spos_invalid", Value::Bool(d.r.bool()?));
    if is(n, b"DIDP") {
        return d.r.assert_tag(0x03);
    }
    // MPGPosition extends DIDPosition with a bounded picture-type/length table.
    d.tags(0x02, 0x01)?;
    n.set("trailing_discards", Value::Int(d.r.s16()? as i64));
    n.set("need_seq_hdr", Value::Bool(d.r.bool()?));
    let leader_length = d.r.s16()? as i64;
    if leader_length < 0 {
        return d.r.err(format!("negative MPGP leader length {leader_length}"));
    }
    let mut fields = Vec::new();
    if leader_length > 0 {
        // pyavb reads this leading-discard count but does not expose it.
        let _leading_discard_fields = d.r.s16()?;
        let count = d.count(leader_length, 5)?;
        fields.reserve(count);
        for _ in 0..count {
            fields.push(Value::List(vec![
                Value::Int(d.r.u8()? as i64),
                Value::Int(d.r.u32()? as i64),
            ]));
        }
    }
    n.set("fields", Value::List(fields));
    d.r.assert_tag(0x03)
}

/// The pyavb classes expose these blocks as opaque bytes, not decoded effects/shapes.
/// The signed byte count is checked against the enclosing object payload before allocation.
fn opaque_effect_data(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    let size = if is(n, b"CCFX") { d.r.s16()? as i64 } else { d.r.s32()? as i64 };
    let size = d.count(size, 1)?;
    let field = if is(n, b"CCFX") {
        "color_correction"
    } else if is(n, b"GRFX") {
        "pict_data"
    } else {
        "shape_data"
    };
    n.set(field, Value::Bytes(d.r.bytes(size)?.to_vec()));
    d.r.assert_tag(0x03)
}

/// The TKFX/TNFX shared block after the class tags (pyavb TrackEffect / TransitionEffect).
fn effect_info(d: &mut Decoder, n: &mut Node) -> Result<()> {
    n.set("left_length", Value::Int(d.r.s32()? as i64));
    n.set("right_length", Value::Int(d.r.s32()? as i64));
    n.set("info_version", Value::Int(d.r.s16()? as i64));
    n.set("info_current", Value::Int(d.r.s32()? as i64));
    n.set("info_smooth", Value::Int(d.r.s32()? as i64));
    n.set("info_color_item", Value::Int(d.r.s16()? as i64));
    n.set("info_quality", Value::Int(d.r.s16()? as i64));
    n.set("info_is_reversed", Value::Int(d.r.u8()? as i8 as i64));
    n.set("info_aspect_on", Value::Bool(d.r.bool()?));
    n.set("keyframes", d.object_ref()?);
    n.set("info_force_software", Value::Bool(d.r.bool()?));
    n.set("info_never_hardware", Value::Bool(d.r.bool()?));
    Ok(())
}

fn track_effect(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_group(d, n)?;
    d.tags(0x02, 0x06)?;
    effect_info(d, n)?;
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x02 => {
                d.r.assert_tag(72)?;
                n.set("trackman", d.object_ref()?);
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    if is(n, b"TKFX") {
        return d.r.assert_tag(0x03);
    }
    if is(n, b"ASPI") || is(n, b"EQMB") {
        return Ok(());
    }
    // PVOL continues.
    d.tags(0x02, 0x05)?;
    n.set("level", Value::Int(d.r.s32()? as i64));
    n.set("pan", Value::Int(d.r.s32()? as i64));
    n.set("suppress_validation", Value::Bool(d.r.bool()?));
    n.set("level_set", Value::Bool(d.r.bool()?));
    n.set("pan_set", Value::Bool(d.r.bool()?));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => n.set("supports_seperate_gain", Value::Int(tagged_s32(d)?)),
            0x02 => n.set("is_trim_gain_effect", Value::Int(tagged_s32(d)?)),
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

/// ASPI carries plugin and opaque preset bytes in addition to the shared TKFX fields.
/// pyavb's reader asserts one plugin; the serialized count is honored here so a
/// valid multi-plugin payload remains inspectable. Each count is bounded by its
/// minimum serialized item size before allocating.
fn audio_suite_plugin_effect(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_effect(d, n)?;
    d.tags(0x02, 0x01)?;
    let count = d.r.s32()? as i64;
    let count = d.count(count, 18)?; // string length, three IDs, chunk count
    let mut plugins = Vec::with_capacity(count);
    for _ in 0..count {
        let mut plugin = Node::new(*b"\0\0\0\0", "ASPIPlugin");
        plugin.set("name", Value::Str(d.r.string()?));
        plugin.set("manufacturer_id", Value::UInt(d.r.u32()? as u64));
        plugin.set("product_id", Value::UInt(d.r.u32()? as u64));
        plugin.set("plugin_id", Value::UInt(d.r.u32()? as u64));
        let chunks_count = d.r.s32()? as i64;
        let chunks_count = d.count(chunks_count, 26)?;
        let mut chunks = Vec::with_capacity(chunks_count);
        for _ in 0..chunks_count {
            let size = d.r.s32()? as i64;
            let mut chunk = Node::new(*b"\0\0\0\0", "ASPIPluginChunk");
            chunk.set("version", Value::Int(d.r.s32()? as i64));
            chunk.set("manufacturer_id", Value::UInt(d.r.u32()? as u64));
            chunk.set("product_id", Value::UInt(d.r.u32()? as u64));
            chunk.set("plugin_id", Value::UInt(d.r.u32()? as u64));
            chunk.set("chunk_id", Value::UInt(d.r.u32()? as u64));
            chunk.set("name", Value::Str(d.r.string()?));
            let size = d.count(size, 1)?;
            chunk.set("data", Value::Bytes(d.r.bytes(size)?.to_vec()));
            chunks.push(Value::Node(Box::new(chunk)));
        }
        plugin.set("chunks", Value::List(chunks));
        plugins.push(Value::Node(Box::new(plugin)));
    }
    n.set("plugins", Value::List(plugins));
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                n.set("mob_hi", Value::Int(tagged_s32(d)?));
                n.set("mob_lo", Value::Int(tagged_s32(d)?));
            }
            0x02 => { d.r.assert_tag(77)?; n.set("mark_in", Value::Int(d.r.s64()?)); }
            0x03 => { d.r.assert_tag(77)?; n.set("mark_out", Value::Int(d.r.s64()?)); }
            0x04 => { d.r.assert_tag(72)?; n.set("tracks_to_affect", Value::UInt(d.r.u32()? as u64)); }
            0x05 => n.set("rendering_mode", Value::Int(tagged_s32(d)?)),
            0x06 => n.set("padding_secs", Value::Int(tagged_s32(d)?)),
            0x08 => n.set("mob_id", Value::MobId(d.r.mob_id()?)),
            0x09 => {
                d.r.assert_tag(72)?;
                let first_size = d.r.u32()? as usize;
                let data = if first_size == 0 {
                    Vec::new()
                } else {
                    d.r.assert_tag(65)?;
                    let second_size = d.r.u32()? as usize;
                    if first_size != second_size {
                        return d.r.err("ASPI preset path lengths differ");
                    }
                    d.r.bytes(first_size)?.to_vec()
                };
                n.set("preset_path", Value::Bytes(data));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn equalizer_multi_band(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_effect(d, n)?;
    d.tags(0x02, 0x05)?;
    let count = d.r.s32()? as i64;
    let count = d.count(count, 17)?; // four s32 values and one bool
    let mut bands = Vec::with_capacity(count);
    for _ in 0..count {
        let mut band = Node::new(*b"\0\0\0\0", "EqualizerBand");
        band.set("type", Value::Int(d.r.s32()? as i64));
        band.set("freq", Value::Int(d.r.s32()? as i64));
        band.set("gain", Value::Int(d.r.s32()? as i64));
        band.set("q", Value::Int(d.r.s32()? as i64));
        band.set("enable", Value::Bool(d.r.bool()?));
        bands.push(Value::Node(Box::new(band)));
    }
    n.set("bands", Value::List(bands));
    n.set("effect_enable", Value::Bool(d.r.bool()?));
    n.set("filter_name", Value::Str(d.r.string()?));
    d.r.assert_tag(0x03)
}

fn transition(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_group(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("cutpoint", Value::Int(d.r.s32()? as i64));
    d.tags(0x02, 0x05)?;
    effect_info(d, n)?;
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(72)?;
                n.set("trackman", d.object_ref()?);
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn selector(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_group(d, n)?;
    d.tags(0x02, 0x01)?;
    n.set("is_ganged", Value::Bool(d.r.bool()?));
    let selected = d.r.u16()? as usize;
    let tracks = match n.get("tracks") {
        Some(Value::List(t)) => t.len(),
        _ => 0,
    };
    if selected >= tracks {
        return d.r.err(format!("selector picks track {} of {}", selected, tracks));
    }
    n.set("selected", Value::Int(selected as i64));
    d.r.assert_tag(0x03)
}

fn essence_group(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_group(d, n)?;
    d.tags(0x02, 0x01)?;
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => n.set("rep_set_type", Value::Int(tagged_s32(d)?)),
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

/// WARP -> SPED (motion effect) / MASK (capture mask), pyavb TimeWarp and subclasses.
fn time_warp(d: &mut Decoder, n: &mut Node) -> Result<()> {
    track_group(d, n)?;
    d.tags(0x02, 0x02)?;
    n.set("phase_offset", Value::Int(d.r.s32()? as i64));
    match &n.class {
        b"SPED" => {
            d.tags(0x02, 0x03)?;
            let num = d.r.s32()? as i64;
            let den = d.r.s32()? as i64;
            n.set("speed_ratio", Value::List(vec![Value::Int(num), Value::Int(den)]));
            while let Some(tag) = d.r.next_ext()? {
                match tag {
                    0x01 => {
                        d.r.assert_tag(75)?;
                        n.set("offset_adjust", Value::Float(d.r.f64()?));
                    }
                    0x02 => {
                        d.r.assert_tag(72)?;
                        n.set("source_param_list", d.object_ref()?);
                    }
                    0x03 => {
                        d.r.assert_tag(66)?;
                        n.set("new_source_calculation", Value::Bool(d.r.bool()?));
                    }
                    _ => return d.unknown_ext(n, tag),
                }
            }
            d.r.assert_tag(0x03)
        }
        b"MASK" => {
            d.tags(0x02, 0x01)?;
            n.set("is_double", Value::Bool(d.r.bool()?));
            n.set("mask_bits", Value::Int(d.r.u32()? as i64));
            d.r.assert_tag(0x03)
        }
        b"REPT" => {
            d.tags(0x02, 0x01)?;
            d.r.assert_tag(0x03)
        }
        b"STRB" => {
            d.tags(0x02, 0x01)?;
            n.set("strobe_value", Value::Int(d.r.s32()? as i64));
            d.r.assert_tag(0x03)
        }
        _ => Ok(()), // pyavb's WARP read ends here without a closing tag
    }
}

// Tracker objects are also retained verbatim by AvbFile::raw for the narrowly scoped
// AniMatte extractor. These typed fields describe the reference graph and outer settings;
// they do not interpret tracker algorithm settings or evaluate the curves.
fn tracker_settings(d: &mut Decoder) -> Result<Value> {
    let size = d.r.s16()? as i64;
    let size = d.count(size, 1)?;
    Ok(Value::Bytes(d.r.bytes(size)?.to_vec()))
}

fn tracker_refs(d: &mut Decoder, count: i64) -> Result<Value> {
    let count = d.count(count, 4)?;
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        refs.push(d.object_ref()?);
    }
    Ok(Value::RefList(refs))
}

fn tracker_manager(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    n.set("data_slots", d.object_ref()?);
    n.set("param_slots", d.object_ref()?);
    d.r.assert_tag(0x03)
}

fn tracker_data_slot(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    let count = d.r.s32()? as i64;
    n.set("tracker_data", tracker_refs(d, count)?);
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => {
                d.r.assert_tag(66)?;
                n.set("track_fg", Value::Bool(d.r.bool()?));
            }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn tracker_parameter_slot(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    n.set("settings", tracker_settings(d)?);
    let count = d.r.s32()? as i64;
    n.set("params", tracker_refs(d, count)?);
    d.r.assert_tag(0x03)
}

fn tracker_data(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    n.set("settings", tracker_settings(d)?);
    n.set("clip_version", Value::UInt(d.r.u32()? as u64));
    let count = d.r.s16()? as i64;
    n.set("clips", tracker_refs(d, count)?);
    while let Some(tag) = d.r.next_ext()? {
        match tag {
            0x01 => { d.r.assert_tag(72)?; n.set("offset_tracking", Value::UInt(d.r.u32()? as u64)); }
            0x02 => { d.r.assert_tag(72)?; n.set("smoothing", Value::UInt(d.r.u32()? as u64)); }
            0x03 => { d.r.assert_tag(72)?; n.set("jitter_removal", Value::UInt(d.r.u32()? as u64)); }
            0x04 => { d.r.assert_tag(75)?; n.set("filter_amount", Value::Float(d.r.f64()?)); }
            0x05 => { d.r.assert_tag(72)?; n.set("clip5", d.object_ref()?); }
            0x06 => { d.r.assert_tag(72)?; n.set("clip6", d.object_ref()?); }
            _ => return d.unknown_ext(n, tag),
        }
    }
    d.r.assert_tag(0x03)
}

fn tracker_parameter(d: &mut Decoder, n: &mut Node) -> Result<()> {
    d.tags(0x02, 0x01)?;
    n.set("settings", tracker_settings(d)?);
    d.r.assert_tag(0x03)
}

#[cfg(test)]
mod tracker_tests {
    use super::*;

    fn read(class: [u8; 4], payload: &[u8]) -> Result<Node> {
        let mut d = Decoder { r: Reader::new(payload, 0, false), num_objects: 100_000 };
        let node = decode(class, &mut d)?.expect("known tracker class");
        if d.r.remaining() != 0 {
            return d.r.err("unread bytes");
        }
        Ok(node)
    }

    #[test]
    fn manager_references_and_exact_boundary() {
        let mut payload = vec![2, 1];
        payload.extend_from_slice(&123u32.to_le_bytes());
        payload.extend_from_slice(&456u32.to_le_bytes());
        payload.push(3);
        let node = read(*b"TKMN", &payload).unwrap();
        assert_eq!(node.get("data_slots"), Some(&Value::Ref(123)));
        assert_eq!(node.get("param_slots"), Some(&Value::Ref(456)));
        for end in 0..payload.len() {
            assert!(read(*b"TKMN", &payload[..end]).is_err(), "accepted prefix {end}");
        }
        payload.push(0);
        assert!(read(*b"TKMN", &payload).is_err());
    }

    /// Chunks from pinned pyavb ff64e06; use PYAVB_CHUNKS=.../tests/chunks.
    #[test]
    fn pinned_pyavb_tracker_chunks_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let root = std::path::PathBuf::from(root);
        for (class, name) in [
            ("TKDA", "tkda_01.chunk"), ("TKDS", "tkds_01.chunk"),
            ("TKPA", "tkpa_01.chunk"), ("TKPS", "tkps_01.chunk"),
        ] {
            let bytes = std::fs::read(root.join(class).join(name)).unwrap();
            let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            assert_eq!(size, bytes.len() - 8, "{name}");
            let mut id: [u8; 4] = bytes[..4].try_into().unwrap();
            id.reverse();
            assert_eq!(&id, class.as_bytes(), "{name}");
            let payload = &bytes[8..];
            let node = read(id, payload).unwrap_or_else(|e| panic!("{name}: {e}"));
            for end in 0..payload.len() {
                assert!(read(id, &payload[..end]).is_err(), "{name} accepted prefix {end}");
            }
            match class {
                "TKDA" => {
                    assert_eq!(node.get("clip_version"), Some(&Value::UInt(65536)));
                    assert_eq!(node.get("offset_tracking"), Some(&Value::UInt(1)));
                    assert_eq!(node.get("smoothing"), Some(&Value::UInt(0)));
                    assert_eq!(node.get("filter_amount"), Some(&Value::Float(0.5)));
                    let Some(Value::RefList(clips)) = node.get("clips") else { panic!("clips") };
                    assert_eq!(clips.len(), 11);
                }
                "TKDS" => {
                    assert_eq!(node.get("tracker_data"), Some(&Value::RefList(vec![Value::Ref(159)])));
                    assert_eq!(node.get("track_fg"), Some(&Value::Bool(true)));
                }
                "TKPA" => {
                    let Some(Value::Bytes(settings)) = node.get("settings") else { panic!("settings") };
                    assert_eq!(settings.len(), 96);
                }
                "TKPS" => {
                    let Some(Value::Bytes(settings)) = node.get("settings") else { panic!("settings") };
                    assert_eq!(settings.len(), 34);
                    let Some(Value::RefList(params)) = node.get("params") else { panic!("params") };
                    assert_eq!(params.len(), 2);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn rejects_impossible_tracker_sizes_counts_and_extensions() {
        for class in [*b"TKPA", *b"TKPS", *b"TKDA"] {
            let payload = [2, 1, 0xff, 0x7f, 3];
            assert!(read(class, &payload).is_err(), "oversize {class:?}");
            let payload = [2, 1, 0xff, 0xff, 3];
            assert!(read(class, &payload).is_err(), "negative size {class:?}");
        }
        let mut slot = vec![2, 1];
        slot.extend_from_slice(&i32::MAX.to_le_bytes());
        slot.push(3);
        assert!(read(*b"TKDS", &slot).is_err());
        slot[2..6].copy_from_slice(&0i32.to_le_bytes());
        slot.extend_from_slice(&[1, 0x7f]);
        assert!(read(*b"TKDS", &slot).is_err());

        let mut data = vec![2, 1, 0, 0];
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&i16::MAX.to_le_bytes());
        data.push(3);
        assert!(read(*b"TKDA", &data).is_err());
    }
}

#[cfg(test)]
mod component_curve_tests {
    use super::*;

    fn base_clip() -> Vec<u8> {
        let mut p = vec![2, 3];
        p.extend_from_slice(&[0; 8]); // left/right BOB refs
        p.extend_from_slice(&10i16.to_le_bytes());
        p.extend_from_slice(&23976i32.to_le_bytes());
        p.extend_from_slice(&(-3i16).to_le_bytes());
        p.extend_from_slice(&0xffffu16.to_le_bytes()); // name
        p.extend_from_slice(&0xffffu16.to_le_bytes()); // effect ID
        p.extend_from_slice(&[0; 12]); // attributes, session attrs, precomputed
        p.extend_from_slice(&[2, 1]);
        p.extend_from_slice(&24u32.to_le_bytes());
        p
    }

    fn read(class: [u8; 4], payload: &[u8]) -> Result<Node> {
        let mut d = Decoder {
            r: Reader::new(payload, 0, false),
            num_objects: 100_000,
        };
        let n = decode(class, &mut d)?.expect("known class");
        if d.r.remaining() != 0 {
            return d.r.err("unread bytes");
        }
        Ok(n)
    }

    #[test]
    fn prcl_double_value_and_point_property() {
        let mut p = base_clip();
        p.extend_from_slice(&[2, 1]);
        p.extend_from_slice(&3i32.to_le_bytes()); // linear interpolation
        p.extend_from_slice(&2i16.to_le_bytes()); // double values
        p.extend_from_slice(&1i32.to_le_bytes());
        p.extend_from_slice(&7i32.to_le_bytes());
        p.extend_from_slice(&2i32.to_le_bytes());
        p.extend_from_slice(&24i32.to_le_bytes());
        p.extend_from_slice(&1.5f64.to_le_bytes());
        p.extend_from_slice(&1i16.to_le_bytes());
        p.extend_from_slice(&5i16.to_le_bytes());
        p.extend_from_slice(&2i16.to_le_bytes());
        p.extend_from_slice(&0.25f64.to_le_bytes());
        p.extend_from_slice(&[1, 1, 71]);
        p.extend_from_slice(&2i32.to_le_bytes());
        p.push(3);
        let n = read(*b"PRCL", &p).unwrap();
        assert_eq!(n.get("value_type"), Some(&Value::Int(2)));
        let Value::List(points) = n.get("control_points").unwrap() else {
            panic!("points")
        };
        let Value::Node(point) = &points[0] else {
            panic!("point")
        };
        assert_eq!(
            point.get("offset"),
            Some(&Value::List(vec![Value::Int(7), Value::Int(2)]))
        );
        assert_eq!(point.get("value"), Some(&Value::Float(1.5)));
        let Value::List(props) = point.get("pp").unwrap() else {
            panic!("props")
        };
        let Value::Node(prop) = &props[0] else {
            panic!("prop")
        };
        assert_eq!(prop.get("value"), Some(&Value::Float(0.25)));
        assert_eq!(n.get("extrap_kind"), Some(&Value::Int(2)));
        for end in 0..p.len() {
            assert!(read(*b"PRCL", &p[..end]).is_err(), "accepted prefix {end}");
        }
        let mut bad = p.clone();
        bad[base_clip().len() + 2 + 4..base_clip().len() + 2 + 6]
            .copy_from_slice(&3i16.to_le_bytes());
        assert!(read(*b"PRCL", &bad).is_err());
        let mut bad_property = p.clone();
        let property_kind = base_clip().len() + 2 + 4 + 2 + 4 + 12 + 8 + 2 + 2;
        bad_property[property_kind..property_kind + 2].copy_from_slice(&4i16.to_le_bytes());
        assert!(read(*b"PRCL", &bad_property).is_err());
    }

    #[test]
    fn ctrl_rational_point_and_bounded_counts() {
        let mut p = base_clip();
        p.extend_from_slice(&[2, 3]);
        p.extend_from_slice(&3i32.to_le_bytes());
        p.extend_from_slice(&1i32.to_le_bytes());
        p.extend_from_slice(&(-12i32).to_le_bytes());
        p.extend_from_slice(&1i32.to_le_bytes());
        p.extend_from_slice(&1i32.to_le_bytes());
        p.push(1); // has_value
        p.extend_from_slice(&536870912i32.to_le_bytes());
        p.extend_from_slice(&1i32.to_le_bytes());
        p.extend_from_slice(&1i16.to_le_bytes());
        p.extend_from_slice(&14i16.to_le_bytes());
        p.extend_from_slice(&5i32.to_le_bytes());
        p.extend_from_slice(&2i32.to_le_bytes());
        p.push(3);
        let n = read(*b"CTRL", &p).unwrap();
        let Value::List(points) = n.get("control_points").unwrap() else {
            panic!("points")
        };
        let Value::Node(point) = &points[0] else {
            panic!("point")
        };
        assert_eq!(
            point.get("value"),
            Some(&Value::List(vec![Value::Int(536870912), Value::Int(1)]))
        );
        let Value::List(props) = point.get("pp").unwrap() else {
            panic!("props")
        };
        let Value::Node(prop) = &props[0] else {
            panic!("prop")
        };
        assert_eq!(prop.get("code"), Some(&Value::Int(14)));
        assert_eq!(
            prop.get("value"),
            Some(&Value::List(vec![Value::Int(5), Value::Int(2)]))
        );
        for end in 0..p.len() {
            assert!(read(*b"CTRL", &p[..end]).is_err(), "accepted prefix {end}");
        }
        let mut bad = p.clone();
        bad[base_clip().len() + 2 + 4..base_clip().len() + 2 + 8]
            .copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(read(*b"CTRL", &bad).is_err());
        let mut missing_value = p.clone();
        let value_flag = base_clip().len() + 2 + 4 + 4 + 12;
        missing_value[value_flag] = 0;
        assert!(read(*b"CTRL", &missing_value).is_err());
    }

    #[test]
    fn eccp_preserves_header_and_rejects_truncation() {
        let mut p = base_clip();
        p.extend_from_slice(&[2, 1]);
        p.extend_from_slice(b"ABCDEFGH");
        p.push(1);
        p.push(2);
        p.extend_from_slice(&4u16.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&(-25i32).to_le_bytes());
        p.push(3);
        let n = read(*b"ECCP", &p).unwrap();
        assert_eq!(n.get("header"), Some(&Value::Bytes(b"ABCDEFGH".to_vec())));
        assert_eq!(n.get("film_kind"), Some(&Value::Int(1)));
        assert_eq!(n.get("code_format"), Some(&Value::Int(2)));
        assert_eq!(n.get("base_perf"), Some(&Value::Int(4)));
        assert_eq!(n.get("start_ec"), Some(&Value::Int(-25)));
        for end in 0..p.len() {
            assert!(read(*b"ECCP", &p[..end]).is_err(), "accepted prefix {end}");
        }
    }

    /// Run with PYAVB_CHUNKS pointing to the pinned ff64e06 pyavb/tests/chunks directory.
    #[test]
    fn pinned_pyavb_chunks_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let cases = [
            ("CTRL", "ctrl_01.chunk"),
            ("PRCL", "prcl_01.chunk"),
            ("PRCL", "prcl_02.chunk"),
            ("PRCL", "prcl_03.chunk"),
        ];
        for (class, name) in cases {
            let bytes = std::fs::read(root.join(class).join(name)).unwrap();
            let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            assert_eq!(size, bytes.len() - 8, "{name}");
            let mut id: [u8; 4] = bytes[..4].try_into().unwrap();
            id.reverse();
            assert_eq!(&id, class.as_bytes(), "{name}");
            let n = read(id, &bytes[8..]).unwrap();
            assert_eq!(n.get("edit_rate"), Some(&Value::Float(23.976)), "{name}");
            assert_eq!(n.get("media_kind_id"), Some(&Value::Int(10)), "{name}");
            match name {
                "ctrl_01.chunk" => {
                    assert_eq!(n.get("interp_kind"), Some(&Value::Int(3)));
                    assert_eq!(n.get("length"), Some(&Value::Int(19)));
                    let Value::List(points) = n.get("control_points").unwrap() else {
                        panic!("points")
                    };
                    let Value::Node(point) = &points[0] else {
                        panic!("point")
                    };
                    assert_eq!(
                        point.get("offset"),
                        Some(&Value::List(vec![Value::Int(-12), Value::Int(1)]))
                    );
                    assert_eq!(
                        point.get("value"),
                        Some(&Value::List(vec![Value::Int(536870912), Value::Int(1)]))
                    );
                }
                "prcl_01.chunk" => {
                    assert_eq!(n.get("value_type"), Some(&Value::Int(1)));
                    assert_eq!(n.get("extrap_kind"), Some(&Value::Int(1)));
                    assert_eq!(n.get("fields"), Some(&Value::Int(1)));
                }
                "prcl_02.chunk" => {
                    assert_eq!(n.get("value_type"), Some(&Value::Int(4)));
                    assert_eq!(n.get("control_points"), Some(&Value::List(vec![])));
                }
                "prcl_03.chunk" => {
                    let Value::List(points) = n.get("control_points").unwrap() else {
                        panic!("points")
                    };
                    let Value::Node(point) = &points[0] else {
                        panic!("point")
                    };
                    assert_eq!(point.get("value"), Some(&Value::Ref(65197)));
                }
                _ => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod bin_view_tests {
    use super::*;

    fn read(class: [u8; 4], payload: &[u8]) -> Result<Node> {
        let mut d = Decoder { r: Reader::new(payload, 0, false), num_objects: 100_000 };
        let node = decode(class, &mut d)?.expect("known class");
        if d.r.remaining() != 0 {
            return d.r.err("unread bytes");
        }
        Ok(node)
    }

    fn string(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
    }

    fn small_bin_first() -> Vec<u8> {
        let mut p = vec![2, 14];
        p.extend_from_slice(&0u32.to_le_bytes()); // view setting
        p.extend_from_slice(&123u64.to_le_bytes());
        p.extend_from_slice(&0u16.to_le_bytes()); // bin items
        p.extend_from_slice(&98999i32.to_le_bytes());
        p.extend_from_slice(&0i16.to_le_bytes());
        p.push(0); // sifted
        for _ in 0..6 {
            p.extend_from_slice(&1i16.to_le_bytes());
            string(&mut p, "");
            string(&mut p, "Any");
        }
        p.extend_from_slice(&0i16.to_le_bytes()); // sorted columns
        for value in [1i16, 11, 5] {
            p.extend_from_slice(&value.to_le_bytes());
        }
        for value in [1i16, 0, 0, 300, 600] { // rectangle version and coordinates
            p.extend_from_slice(&value.to_le_bytes());
        }
        for color in [[45568u16; 3], [3328u16; 3]] {
            p.extend_from_slice(&1i16.to_le_bytes()); // color version
            for value in color {
                p.extend_from_slice(&value.to_le_bytes());
            }
        }
        p.extend_from_slice(&6i16.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes()); // attributes
        p.push(0); // was iconic
        p.extend_from_slice(&[2, 1]);
        p.extend_from_slice(&39i32.to_le_bytes());
        p.push(3);
        p
    }

    fn small_bin_view() -> Vec<u8> {
        let mut p = vec![2, 6];
        string(&mut p, "Test");
        string(&mut p, "Bin View");
        p.extend_from_slice(&1i16.to_le_bytes());
        p.extend_from_slice(&20i16.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&[2, 10]);
        p.extend_from_slice(&1u16.to_le_bytes());
        string(&mut p, "Scene");
        p.extend_from_slice(&2i16.to_le_bytes());
        p.extend_from_slice(&11i16.to_le_bytes());
        p.push(1);
        p.extend_from_slice(&[1, 1, 69]);
        p.extend_from_slice(&1i16.to_le_bytes());
        p.push(69);
        p.extend_from_slice(&123i16.to_le_bytes());
        p.push(71);
        p.extend_from_slice(&3i32.to_le_bytes());
        p.push(76);
        p.extend_from_slice(&5i32.to_le_bytes()); // pyavb ignores this prefix
        p.extend_from_slice(b"abc");
        p.push(3);
        p
    }

    #[test]
    fn bin_first_extends_bin_without_an_inner_terminator() {
        let p = small_bin_first();
        let node = read(*b"BINF", &p).unwrap();
        assert_eq!(node.get("uid"), Some(&Value::UInt(123)));
        assert_eq!(node.get("items"), Some(&Value::List(vec![])));
        assert_eq!(node.get("unknown_s32"), Some(&Value::Int(39)));
        for end in 0..p.len() {
            assert!(read(*b"BINF", &p[..end]).is_err(), "accepted prefix {end}");
        }
        let mut bad = p.clone();
        bad[14..16].copy_from_slice(&u16::MAX.to_le_bytes()); // impossible bin item count
        assert!(read(*b"BINF", &bad).is_err());
    }

    #[test]
    fn bin_view_columns_descriptors_and_bounds() {
        let p = small_bin_view();
        let node = read(*b"BVst", &p).unwrap();
        assert_eq!(node.get("name"), Some(&Value::Str("Test".into())));
        let Value::List(columns) = node.get("columns").unwrap() else { panic!("columns") };
        let Value::Node(column) = &columns[0] else { panic!("column") };
        assert_eq!(column.get("title"), Some(&Value::Str("Scene".into())));
        assert_eq!(column.get("type"), Some(&Value::Int(11)));
        assert_eq!(column.get("hidden"), Some(&Value::Bool(true)));
        let Value::List(descriptors) = node.get("format_descriptors").unwrap() else { panic!("descriptors") };
        let Value::Node(descriptor) = &descriptors[0] else { panic!("descriptor") };
        assert_eq!(descriptor.get("vcid_free_column_id"), Some(&Value::Int(123)));
        assert_eq!(descriptor.get("format_descriptor"), Some(&Value::Str("abc".into())));
        for end in 0..p.len() {
            assert!(read(*b"BVst", &p[..end]).is_err(), "accepted prefix {end}");
        }
        let mut huge_columns = p.clone();
        let col_count = 2 + 2 + 4 + 2 + 8 + 2 + 2 + 4 + 2;
        huge_columns[col_count..col_count + 2].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(read(*b"BVst", &huge_columns).is_err());
        let mut bad_size = p.clone();
        let size_at = p.windows(4).position(|w| w == [71, 3, 0, 0]).unwrap() + 1;
        bad_size[size_at..size_at + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(read(*b"BVst", &bad_size).is_err());
        let mut invalid_utf8 = p.clone();
        invalid_utf8[p.len() - 2] = 0xff;
        assert!(read(*b"BVst", &invalid_utf8).is_err());
    }

    /// Optional comparison against the pinned ff64e06 pyavb chunk fixtures.
    #[test]
    fn pinned_pyavb_bin_chunks_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let root = std::path::PathBuf::from(root);
        let cases = [
            ("BINF", "binf_01.chunk", 5, 39, 4119274651029482u64, 46),
            ("BINF", "binf_02.chunk", 0, 2, 6824502752094109u64, 4),
            ("BINF", "binf_03.chunk", 6, 91, 4118939643580394u64, 99),
            ("BVst", "bvst_01.chunk", 282, 0, 0, 0),
            ("BVst", "bvst_02.chunk", 292, 1, 0, 0),
            ("BVst", "bvst_03.chunk", 299, 14, 0, 0),
        ];
        for (class, name, count, other, uid, viewref) in cases {
            let bytes = std::fs::read(root.join(class).join(name)).unwrap();
            let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            assert_eq!(size, bytes.len() - 8, "{name}");
            let mut id: [u8; 4] = bytes[..4].try_into().unwrap();
            id.reverse();
            assert_eq!(&id, class.as_bytes(), "{name}");
            let node = read(id, &bytes[8..]).unwrap();
            if class == "BINF" {
                let Value::List(items) = node.get("items").unwrap() else { panic!("items") };
                assert_eq!(items.len(), count, "{name}");
                assert_eq!(node.get("unknown_s32"), Some(&Value::Int(other)), "{name}");
                assert_eq!(node.get("uid"), Some(&Value::UInt(uid)), "{name}");
                assert_eq!(node.get("view_setting"), Some(&Value::Ref(viewref)), "{name}");
                assert_eq!(node.get("sifted"), Some(&Value::Bool(false)), "{name}");
            } else {
                let Value::List(columns) = node.get("columns").unwrap() else { panic!("columns") };
                assert_eq!(columns.len(), count, "{name}");
                let actual_formats = match node.get("format_descriptors") {
                    Some(Value::List(items)) => items.len(),
                    None => 0,
                    _ => panic!("format descriptors"),
                };
                assert_eq!(actual_formats, other as usize, "{name}");
                assert_eq!(node.get("kind"), Some(&Value::Str("Bin View".into())), "{name}");
                let Value::Node(first) = &columns[0] else { panic!("first column") };
                let expected = if name == "bvst_01.chunk" { "AFD" } else { "Color" };
                assert_eq!(first.get("title"), Some(&Value::Str(expected.into())), "{name}");
                let first_format = if name == "bvst_01.chunk" { 0 } else { 105 };
                let first_type = if name == "bvst_01.chunk" { 131 } else { 51 };
                assert_eq!(first.get("format"), Some(&Value::Int(first_format)), "{name}");
                assert_eq!(first.get("type"), Some(&Value::Int(first_type)), "{name}");
                assert_eq!(first.get("hidden"), Some(&Value::Bool(name != "bvst_02.chunk")), "{name}");
                if other > 0 {
                    let Value::List(descriptors) = node.get("format_descriptors").unwrap() else { panic!("descriptors") };
                    let Value::Node(first) = &descriptors[0] else { panic!("descriptor") };
                    let (id, size) = if name == "bvst_02.chunk" { (27649, 1166) } else { (27637, 1675) };
                    assert_eq!(first.get("vcid_free_column_id"), Some(&Value::Int(id)), "{name}");
                    let Some(Value::Str(text)) = first.get("format_descriptor") else { panic!("descriptor text") };
                    assert_eq!(text.len(), size, "{name}");
                    assert!(text.starts_with("{\"properties\":[{"), "{name}");
                }
            }
        }
    }
}

#[cfg(test)]
mod media_descriptor_tests {
    use super::*;

    fn read(class: [u8; 4], payload: &[u8]) -> Result<Node> {
        let mut d = Decoder { r: Reader::new(payload, 0, false), num_objects: 1_000_000 };
        let node = decode(class, &mut d)?.expect("known descriptor class");
        if d.r.remaining() != 0 {
            return d.r.err("unread bytes");
        }
        Ok(node)
    }

    fn data_descriptor_payload(anc: bool) -> Vec<u8> {
        let mut p = vec![2, 3, 4]; // MDES tag/version and mob kind
        p.extend_from_slice(&0u32.to_le_bytes()); // locator
        p.push(0); // intermediate
        p.extend_from_slice(&0u32.to_le_bytes()); // physical media
        p.extend_from_slice(&[2, 3]); // MDFL tag/version
        p.extend_from_slice(&24i32.to_le_bytes());
        p.extend_from_slice(&0i16.to_le_bytes());
        p.extend_from_slice(&42i32.to_le_bytes());
        p.extend_from_slice(&0i16.to_le_bytes());
        p.extend_from_slice(&0i32.to_le_bytes());
        p.extend_from_slice(&[2, 1]); // DATD tag/version
        p.push(1);
        p.extend_from_slice(&123u64.to_le_bytes());
        for value in [5i32, 10, 20] {
            p.extend_from_slice(&value.to_le_bytes());
        }
        if anc {
            p.extend_from_slice(&[2, 1]);
            p.extend_from_slice(&7i32.to_le_bytes());
            p.push(3);
        }
        p
    }

    #[test]
    fn data_and_anc_data_have_distinct_terminators_and_bounded_reads() {
        for (class, anc) in [(*b"DATD", false), (*b"ANCD", true)] {
            let p = data_descriptor_payload(anc);
            let node = read(class, &p).unwrap();
            assert_eq!(node.get("mob_kind"), Some(&Value::Int(4)));
            assert_eq!(node.get("length"), Some(&Value::Int(42)));
            assert_eq!(node.get("offset_to_frame_indexes"), Some(&Value::UInt(123)));
            assert_eq!(node.get("min_sample_size"), Some(&Value::Int(10)));
            assert_eq!(node.get("max_sample_size"), Some(&Value::Int(20)));
            if anc {
                assert_eq!(node.get("manifest_element_count"), Some(&Value::Int(7)));
            } else {
                assert_eq!(node.get("manifest_element_count"), None);
            }
            for end in 0..p.len() {
                assert!(read(class, &p[..end]).is_err(), "{anc} accepted prefix {end}");
            }
        }
    }

    /// Optional fixture parity against pyavb ff64e06; pass PYAVB_CHUNKS to enable.
    #[test]
    fn pinned_pyavb_media_chunks_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let root = std::path::PathBuf::from(root);
        let cases: &[(&str, &[&str])] = &[
            ("AIFC", &["aifc_01.chunk"]),
            ("ANCD", &["ancd_01.chunk"]),
            ("JPED", &["jped_01.chunk", "jped_02.chunk"]),
            ("MDFM", &["mdfm_01.chunk"]),
            ("MDNG", &["mdng_01.chunk"]),
            ("MDTP", &["mdtp_01.chunk"]),
            ("MPGA", &["mpga_01.chunk"]),
            ("MPGI", &["mpgi_01.chunk", "mpgi_02.chunk"]),
            ("MULD", &["muld_01.chunk", "muld_02.chunk"]),
            ("RGBA", &["rgba_01.chunk", "rgba_02.chunk", "rgba_03.chunk", "rgba_04.chunk", "rgba_05.chunk"]),
        ];
        for &(class, files) in cases {
            for &name in files {
                let bytes = std::fs::read(root.join(class).join(name)).unwrap();
                let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
                assert_eq!(size, bytes.len() - 8, "{name}");
                let mut id: [u8; 4] = bytes[..4].try_into().unwrap();
                id.reverse();
                assert_eq!(&id, class.as_bytes(), "{name}");
                let node = read(id, &bytes[8..]).unwrap_or_else(|e| panic!("{name}: {e}"));
                for end in 0..bytes.len() - 8 {
                    assert!(read(id, &bytes[8..8 + end]).is_err(), "{name} accepted prefix {end}");
                }
                match class {
                    "AIFC" => {
                        assert_eq!(node.get("length"), Some(&Value::Int(6016010)));
                        assert_eq!(node.get("data_pos"), Some(&Value::Int(4096)));
                    }
                    "ANCD" => assert_eq!(node.get("length"), Some(&Value::Int(2106))),
                    "MDFM" => assert_eq!(node.get("mob_kind"), Some(&Value::Int(3))),
                    "MDNG" => assert_eq!(node.get("mob_kind"), Some(&Value::Int(4))),
                    "MDTP" => assert_eq!(node.get("cframe"), Some(&Value::Int(0))),
                    "MPGA" => {
                        assert_eq!(node.get("channels"), Some(&Value::Int(2)));
                        assert_eq!(node.get("sample_rate"), Some(&Value::Float(44100.0)));
                        assert_eq!(node.get("origin"), Some(&Value::UInt(2112)));
                    }
                    "JPED" => assert_eq!(node.get("image_start_align"), Some(&Value::Int(if name == "jped_01.chunk" { 512 } else { 131072 }))),
                    "MPGI" => {
                        let Some(Value::Bytes(hdr)) = node.get("sequence_hdr") else { panic!("header") };
                        assert_eq!(hdr.len(), if name == "mpgi_01.chunk" { 0 } else { 512 });
                    }
                    "MULD" => {
                        let Some(Value::RefList(refs)) = node.get("descriptors") else { panic!("refs") };
                        assert_eq!(refs.len(), 2);
                    }
                    "RGBA" => {
                        let Some(Value::List(layout)) = node.get("pixel_layout") else { panic!("layout") };
                        assert_eq!(layout.len(), if name == "rgba_04.chunk" { 1 } else if name == "rgba_05.chunk" { 4 } else { 2 });
                        assert_eq!(node.get("stored_width"), Some(&Value::Int(1920)));
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn pinned_variable_lengths_reject_impossible_sizes_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let root = std::path::PathBuf::from(root);
        for (class, name) in [
            ("AIFC", "aifc_01.chunk"),
            ("MULD", "muld_01.chunk"),
            ("MPGI", "mpgi_02.chunk"),
            ("JPED", "jped_01.chunk"),
            ("RGBA", "rgba_01.chunk"),
        ] {
            let bytes = std::fs::read(root.join(class).join(name)).unwrap();
            let mut p = bytes[8..].to_vec();
            let mut d = Decoder { r: Reader::new(&p, 0, false), num_objects: 1_000_000 };
            let id: [u8; 4] = class.as_bytes().try_into().unwrap();
            let mut n = Node::new(id, "TestDescriptor");
            match class {
                "AIFC" => {
                    media_file_descriptor(&mut d, &mut n).unwrap();
                    d.tags(2, 1).unwrap();
                    assert_eq!(d.r.bytes(4).unwrap(), b"FORM");
                }
                "MULD" => {
                    media_file_descriptor(&mut d, &mut n).unwrap();
                    d.tags(2, 1).unwrap();
                }
                "MPGI" => {
                    cdci(&mut d, &mut n).unwrap();
                    d.tags(2, 1).unwrap();
                    d.r.bytes(11).unwrap();
                }
                "JPED" => {
                    cdci(&mut d, &mut n).unwrap();
                    d.tags(2, 1).unwrap();
                    d.r.bytes(12).unwrap();
                }
                "RGBA" => {
                    did(&mut d, &mut n).unwrap();
                    d.tags(2, 1).unwrap();
                }
                _ => unreachable!(),
            }
            let at = d.r.offset();
            let impossible = if class == "AIFC" { u32::MAX.to_be_bytes() } else { i32::MAX.to_le_bytes() };
            p[at..at + 4].copy_from_slice(&impossible);
            assert!(read(id, &p).is_err(), "{name} accepted impossible length at {at}");
        }
    }
}

#[cfg(test)]
mod misc_effect_parity_tests {
    use super::*;

    fn read(class: [u8; 4], payload: &[u8]) -> Result<Node> {
        let mut d = Decoder { r: Reader::new(payload, 0, false), num_objects: 100_000 };
        let node = decode(class, &mut d)?.expect("known class");
        if d.r.remaining() != 0 {
            return d.r.err("unread bytes");
        }
        Ok(node)
    }

    fn blob(class: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut p = vec![2, 1];
        if class == *b"CCFX" {
            p.extend_from_slice(&(body.len() as i16).to_le_bytes());
        } else {
            p.extend_from_slice(&(body.len() as i32).to_le_bytes());
        }
        p.extend_from_slice(body);
        p.push(3);
        p
    }

    #[test]
    fn bounded_opaque_effect_blocks() {
        for (class, field) in [
            (*b"CCFX", "color_correction"),
            (*b"GRFX", "pict_data"),
            (*b"SHLP", "shape_data"),
        ] {
            let p = blob(class, &[0, 3, 0xff, 1]);
            let node = read(class, &p).unwrap();
            assert_eq!(node.get(field), Some(&Value::Bytes(vec![0, 3, 0xff, 1])));
            for end in 0..p.len() {
                assert!(read(class, &p[..end]).is_err(), "{class:?} accepted prefix {end}");
            }
            let mut oversized = p.clone();
            let size_width = if class == *b"CCFX" { 2 } else { 4 };
            oversized[2..2 + size_width].fill(0x7f);
            assert!(read(class, &oversized).is_err());
            let mut negative = p.clone();
            negative[2..2 + size_width].fill(0xff);
            assert!(read(class, &negative).is_err());
            let mut wrong_end = p.clone();
            *wrong_end.last_mut().unwrap() = 4;
            assert!(read(class, &wrong_end).is_err());
        }
    }

    /// Optional pinned source fixtures from pyavb ff64e06, supplied via PYAVB_CHUNKS.
    #[test]
    fn pinned_misc_chunks_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let root = std::path::PathBuf::from(root);
        for (class, name) in [
            ("CCFX", "ccfx_01.chunk"),
            ("MPGP", "mpgp_01.chunk"),
            ("MPGP", "mpgp_02.chunk"),
            ("SHLP", "shlp_01.chunk"),
        ] {
            let bytes = std::fs::read(root.join(class).join(name)).unwrap();
            let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            assert_eq!(size, bytes.len() - 8, "{name}");
            let mut id: [u8; 4] = bytes[..4].try_into().unwrap();
            id.reverse();
            assert_eq!(&id, class.as_bytes(), "{name}");
            let node = read(id, &bytes[8..]).unwrap();
            match name {
                "ccfx_01.chunk" => {
                    let Value::Bytes(data) = node.get("color_correction").unwrap() else { panic!("bytes") };
                    assert_eq!(data.len(), 75);
                }
                "shlp_01.chunk" => {
                    let Value::Bytes(data) = node.get("shape_data").unwrap() else { panic!("bytes") };
                    assert_eq!(data.len(), 346);
                    assert_eq!(&data[..4], b"XFZB");
                }
                "mpgp_01.chunk" => {
                    assert_eq!(node.get("offset"), Some(&Value::UInt(2_097_152)));
                    let Value::List(fields) = node.get("fields").unwrap() else { panic!("fields") };
                    assert_eq!(fields.len(), 6);
                    assert_eq!(fields[0], Value::List(vec![Value::Int(0), Value::Int(422_803)]));
                }
                "mpgp_02.chunk" => {
                    assert_eq!(node.get("offset"), Some(&Value::UInt(8_388_608)));
                    assert_eq!(node.get("fields"), Some(&Value::List(vec![])));
                }
                _ => unreachable!(),
            }
            for end in 0..size {
                assert!(read(id, &bytes[8..8 + end]).is_err(), "{name} accepted prefix {end}");
            }
        }
    }

    #[test]
    fn mpgp_rejects_impossible_field_count() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let bytes = std::fs::read(std::path::Path::new(&root).join("MPGP/mpgp_01.chunk")).unwrap();
        let payload = &bytes[8..];
        // The final MPGP layer begins at the last 40 bytes in this pinned chunk:
        // [02,01, trailing:i16, flag:u8, leader:i16, leading:i16, 6 * (u8,u32), 03].
        let layer = payload.len() - 40;
        assert_eq!(&payload[layer..layer + 2], &[2, 1]);
        let mut impossible = payload.to_vec();
        impossible[layer + 5..layer + 7].copy_from_slice(&i16::MAX.to_le_bytes());
        assert!(read(*b"MPGP", &impossible).is_err());
        let mut negative = payload.to_vec();
        negative[layer + 5..layer + 7].copy_from_slice(&(-1i16).to_le_bytes());
        assert!(read(*b"MPGP", &negative).is_err());
    }
}

#[cfg(test)]
mod fxps_parity_tests {
    use super::*;

    fn read(payload: &[u8]) -> Result<Node> {
        let mut d = Decoder { r: Reader::new(payload, 0, false), num_objects: 100_000 };
        let n = decode(*b"FXPS", &mut d)?.expect("known FXPS class");
        if d.r.remaining() != 0 {
            return d.r.err("unread FXPS bytes");
        }
        Ok(n)
    }

    #[test]
    fn empty_fxps_and_malformed_lengths() {
        let mut p = vec![2, 0x12];
        p.extend_from_slice(&(-1i32).to_le_bytes());
        p.extend_from_slice(&0i32.to_le_bytes());
        p.extend_from_slice(&0i32.to_le_bytes());
        p.extend_from_slice(&168i32.to_le_bytes());
        p.push(3);
        assert_eq!(read(&p).unwrap().get("parameters"), Some(&Value::List(vec![])));
        for end in 0..p.len() {
            assert!(read(&p[..end]).is_err(), "accepted prefix {end}");
        }
        for count in [-1i32, i32::MAX] {
            let mut bad = p.clone();
            bad[10..14].copy_from_slice(&count.to_le_bytes());
            assert!(read(&bad).is_err(), "accepted count {count}");
        }
        let mut bad = p.clone();
        bad[1] = 0x13;
        assert!(read(&bad).is_err());
        let mut bad = p;
        *bad.last_mut().unwrap() = 4;
        assert!(read(&bad).is_err());
    }

    /// Two pinned upstream chunks from pyavb ff64e06; values checked against EffectParamList.read.
    #[test]
    fn pinned_fxps_chunks_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let root = std::path::PathBuf::from(root).join("FXPS");
        for (name, original_length, keyframe_size, user_size) in [
            ("fxps_01.chunk", -1, 168, 0),
            ("fxps_02.chunk", 136, 762, 594),
        ] {
            let bytes = std::fs::read(root.join(name)).unwrap();
            assert_eq!(&bytes[0..4], b"SPXF");
            let payload = &bytes[8..];
            let node = read(payload).unwrap();
            assert_eq!(node.get("orig_length"), Some(&Value::Int(original_length)));
            assert_eq!(node.get("window_offset"), Some(&Value::Int(0)));
            assert_eq!(node.get("keyframe_size"), Some(&Value::Int(keyframe_size)));
            let Some(Value::List(parameters)) = node.get("parameters") else { panic!("missing parameters") };
            assert_eq!(parameters.len(), 2);
            for (i, value) in parameters.iter().enumerate() {
                let Value::Node(param) = value else { panic!("parameter is not a node") };
                assert_eq!(param.type_name, "EffectParam");
                assert_eq!(param.get("percent_time"), Some(&Value::Int(if i == 0 { 0 } else { 1 << 30 })));
                assert_eq!(param.get("selected"), Some(&Value::Bool(true)));
                for (field, expected) in [
                    ("level", if user_size == 0 { 536_870_912 } else { 1 << 30 }),
                    ("pos_x", 0), ("floor_x", 0), ("ceil_x", 1000),
                    ("pos_y", 0), ("floor_y", 0), ("ceil_y", 1000),
                    ("scale_x", 65_536), ("scale_y", 65_536),
                    ("crop_left", 0), ("crop_right", 0),
                    ("crop_top", 0), ("crop_bottom", 0),
                    ("border_width", 0), ("border_soft", 0),
                    ("splill_gain2", if user_size == 0 { 0 } else { 32 }),
                    ("splill_gain", 0),
                    ("splill_soft2", if user_size == 0 { 0 } else { 32 }),
                    ("splill_soft", 0), ("enable_key_flags", 0),
                ] {
                    assert_eq!(param.get(field), Some(&Value::Int(expected)), "{name} {field}");
                }
                for field in ["box_xscale", "box_yscale", "box_xpos", "box_ypos"] {
                    assert_eq!(param.get(field), Some(&Value::Bool(false)));
                }
                assert_eq!(param.get("box"), Some(&Value::List(vec![
                    Value::Int(-32_768_000), Value::Int(32_768_000),
                    Value::Int(-32_768_000), Value::Int(32_768_000),
                ])));
                let Some(Value::Bytes(user)) = param.get("user_param") else { panic!("missing user_param") };
                assert_eq!(user.len(), user_size);
                if user_size == 0 {
                    assert_eq!(param.get("colors"), Some(&Value::List(vec![])));
                } else {
                    assert_eq!(param.get("colors"), Some(&Value::List(vec![
                        Value::Int(16_777_215), Value::Int(8_388_607),
                    ])));
                    assert_eq!(&user[..8], b"BZFX\0\0\0\0");
                    assert_eq!(&user[user.len() - 8..], &[0xbf, 0xfc, 0x9f, 0xc8, 0xd8, 0x66, 0xb0, 0xa0]);
                }
            }
            for end in 0..payload.len() {
                assert!(read(&payload[..end]).is_err(), "{name} accepted prefix {end}");
            }
        }
    }

    #[test]
    fn pinned_fxps_rejects_bad_nested_counts_when_available() {
        let Some(root) = std::env::var_os("PYAVB_CHUNKS") else { return };
        let bytes = std::fs::read(std::path::PathBuf::from(root).join("FXPS/fxps_01.chunk")).unwrap();
        let payload = &bytes[8..];
        // First parameter starts at byte 18. Its 14 scalar values, box, flags,
        // border and spill values place color_count at 111 and param_size at 115.
        for (at, replacement) in [(111, -1i32), (111, i32::MAX), (115, -1), (115, i32::MAX)] {
            let mut bad = payload.to_vec();
            bad[at..at + 4].copy_from_slice(&replacement.to_le_bytes());
            assert!(read(&bad).is_err(), "accepted malformed nested length at {at}");
        }
    }
}
