//! Native centered dissolves and their exact graph/2 endpoint overlaps.
use super::{EditRate, Result, Span};
use avid_reader::{
    document::AvbDocument,
    io::Uuid,
    objects::{Node, Value},
};
use graph::source::SourceTransition;
use std::collections::HashMap;

const VIDEO: &str = "EFF2_BLEND_DISSOLVE";
const AUDIO: &str = "BLEND_AUDIO_DISSOLVE";
const CURVE_ATTR: &str = "_ATN_AUDIO_DISSOLVE_CURVETYPE";
const CURVE_PARAM: u128 = 0x8134fc48_3bd6_4904_9520_7a6360cc7127;

#[derive(Clone, Copy, Debug)]
pub(super) struct Transition {
    pub after_clip: usize,
    pub duration: u32,
    pub cut_offset: u32,
}

pub(super) fn prepare<'a>(
    transitions: impl Iterator<Item = &'a SourceTransition>,
    spans: &[(&str, Span)],
    rate: EditRate,
    picture: bool,
    label: &str,
) -> Result<Vec<Transition>> {
    let mut transitions = transitions.peekable();
    let mut by_clip = HashMap::new();
    if transitions.peek().is_some() {
        let positions: HashMap<_, _> = spans
            .iter()
            .enumerate()
            .map(|(index, (id, _))| (*id, index))
            .collect();
        for t in transitions {
            let refuse = |message| format!("{}: {message}", t.id);
            let units = |time| rate.frames(time).and_then(|n| u32::try_from(n).ok());
            let (style, qualified_duration) = if picture {
                ("premiere.ADBE Cross Dissolve", 8)
            } else {
                ("premiere.Constant Power", 6)
            };
            let duration = units(t.duration)
                .filter(|n| *n == qualified_duration)
                .filter(|n| units(t.cut_offset) == Some(n / 2) && t.style == style)
                .ok_or_else(|| refuse("unsupported centered Avid transition profile"))?;
            let cut_offset = duration / 2;
            let begin = units(t.cut)
                .and_then(|cut| cut.checked_sub(cut_offset))
                .ok_or_else(|| refuse("transition cut is off the frame grid"))?;
            let end = begin
                .checked_add(duration)
                .ok_or_else(|| refuse("transition extent overflows"))?;
            let endpoint =
                |binding: &graph::source::SourceBinding| match binding.placements.as_slice() {
                    [id] if binding.reason.is_none() => positions.get(id.as_str()).copied(),
                    _ => None,
                };
            let outgoing =
                endpoint(&t.outgoing).ok_or_else(|| refuse("unresolved outgoing edit"))?;
            let incoming =
                endpoint(&t.incoming).ok_or_else(|| refuse("unresolved incoming edit"))?;
            let (_, left) = spans[outgoing];
            let (_, right) = spans[incoming];
            if incoming != outgoing + 1
                || left.record > begin
                || left.record + left.length != end
                || right.record != begin
                || right.length < duration
            {
                return Err(refuse(
                    "expected adjacent edits with exactly the saved transition overlap",
                )
                .into());
            }
            if by_clip
                .insert(
                    outgoing,
                    (
                        Transition {
                            after_clip: outgoing,
                            duration,
                            cut_offset,
                        },
                        begin,
                        end,
                    ),
                )
                .is_some()
            {
                return Err(refuse("outgoing edit owns multiple transitions").into());
            }
        }
    }

    let mut prepared = Vec::with_capacity(by_clip.len());
    let mut last_blend_end = 0;
    for (i, pair) in spans.windows(2).enumerate() {
        if let Some(&(transition, begin, end)) = by_clip.get(&i) {
            if begin < last_blend_end {
                return Err(format!("{label}: overlapping transition windows").into());
            }
            last_blend_end = end;
            prepared.push(transition);
        } else {
            let [(_, left), (_, right)] = pair else {
                unreachable!()
            };
            if left.record + left.length > right.record {
                return Err(format!("{label}: overlapping record edits").into());
            }
        }
    }
    Ok(prepared)
}

fn add(
    doc: &mut AvbDocument,
    class: [u8; 4],
    type_name: &'static str,
    fields: Vec<(&'static str, Value)>,
) -> Result<u32> {
    Ok(doc.add_object(Node {
        class,
        type_name,
        fields,
    })?)
}

fn embedded(class: [u8; 4], type_name: &'static str, fields: Vec<(&'static str, Value)>) -> Value {
    Value::Node(Box::new(Node {
        class,
        type_name,
        fields,
    }))
}

fn component(rate: f64, kind: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("left_bob", Value::Null),
        ("right_bob", Value::Null),
        ("media_kind_id", Value::Int(kind)),
        ("edit_rate", Value::Float(rate)),
        ("name", Value::Null),
        ("effect_id", Value::Null),
        ("attributes", Value::Null),
        ("session_attrs", Value::Null),
        ("precomputed", Value::Null),
        ("param_list", Value::Null),
    ]
}

fn uuid(value: u128) -> Uuid {
    Uuid {
        data1: (value >> 96) as u32,
        data2: (value >> 80) as u16,
        data3: (value >> 64) as u16,
        data4: (value as u64).to_be_bytes(),
    }
}

fn point(frame: i64, value: f64, handles: Option<[f64; 4]>) -> Value {
    let mut properties = Vec::new();
    if let Some(handles) = handles {
        for (code, value) in (5..=8).zip(handles) {
            properties.push(embedded(
                [0; 4],
                "ParamControlPointProperty",
                vec![
                    ("code", Value::Int(code)),
                    ("type", Value::Int(2)),
                    ("value", Value::Float(value)),
                ],
            ));
        }
        properties.push(embedded(
            [0; 4],
            "ParamControlPointProperty",
            vec![
                ("code", Value::Int(9)),
                ("type", Value::Int(1)),
                ("value", Value::Int(1)),
            ],
        ));
    }
    embedded(
        [0; 4],
        "ParamControlPoint",
        vec![
            (
                "offset",
                Value::List(vec![Value::Int(frame), Value::Int(1)]),
            ),
            ("timescale", Value::Int(4)),
            ("value", Value::Float(value)),
            ("pp", Value::List(properties)),
        ],
    )
}

fn parameters(doc: &mut AvbDocument, rate: f64, picture: bool, duration: i64) -> Result<u32> {
    let mut fields = component(rate, 10);
    let points = if picture {
        vec![
            point(-1, 0.0, Some([-3.6, -0.0, 3.6, 0.0])),
            point(duration, 100.0, Some([-3.6, 0.0, 3.6, -0.0])),
        ]
    } else {
        // The native six-frame effect retains the default level envelope's
        // 48-unit endpoint. Equal Power is selected by separate controls.
        vec![point(0, 0.0, None), point(48, 1.0, None)]
    };
    fields.extend([
        ("length", Value::Int(duration)),
        ("interp_kind", Value::Int(3)),
        ("value_type", Value::Int(2)),
        ("control_points", Value::List(points)),
        ("extrap_kind", Value::Int(1)),
        ("fields", Value::Int(1)),
    ]);
    let curve = add(doc, *b"PRCL", "ParamClip", fields)?;
    let specs = if picture {
        vec![
            (
                0x8d56812b_847e_11d5_935a_50f857c10000,
                Value::Int(1),
                Value::Null,
                true,
            ),
            (
                0x8d56813d_847e_11d5_935a_50f857c10000,
                Value::Float(0.0),
                Value::Ref(curve),
                true,
            ),
            (
                0x8d56814d_847e_11d5_935a_50f857c10000,
                Value::Int(0),
                Value::Null,
                true,
            ),
            (
                0x8d568153_847e_11d5_935a_50f857c10000,
                Value::Int(0),
                Value::Null,
                true,
            ),
        ]
    } else {
        vec![
            (
                0x8ba0916a_7c0b_41e4_b2da_c817048cbf5d,
                Value::Float(0.0),
                Value::Ref(curve),
                true,
            ),
            (CURVE_PARAM, Value::Int(1), Value::Null, true),
            (
                0x8af7a5dc_367b_4919_afc7_5ca00a5bd558,
                Value::Int(1),
                Value::Null,
                false,
            ),
        ]
    };
    let mut items = Vec::with_capacity(specs.len());
    for (id, value, track, signature) in specs {
        let kind = if matches!(value, Value::Float(_)) {
            2
        } else {
            1
        };
        items.push(Value::Ref(add(
            doc,
            *b"PRIT",
            "ParameterItem",
            vec![
                ("uuid", Value::Uuid(uuid(id))),
                ("value_type", Value::Int(kind)),
                ("value", value),
                ("name", Value::Str(String::new())),
                ("enable", Value::Bool(true)),
                ("control_track", track),
                ("contribs_to_sig", Value::Bool(signature)),
            ],
        )?));
    }
    add(
        doc,
        *b"PRLS",
        "ParameterList",
        vec![("items", Value::RefList(items))],
    )
}

fn keyframes(doc: &mut AvbDocument, picture: bool) -> Result<u32> {
    let mut parameters = Vec::new();
    for time in [0, 1 << 30] {
        let mut fields = vec![
            ("percent_time", Value::Int(time)),
            ("level", Value::Int(time)),
            ("pos_x", Value::Int(0)),
            ("floor_x", Value::Int(0)),
            ("ceil_x", Value::Int(1000)),
            ("pos_y", Value::Int(0)),
            ("floor_y", Value::Int(0)),
            ("ceil_y", Value::Int(1000)),
            ("scale_x", Value::Int(65536)),
            ("scale_y", Value::Int(65536)),
        ];
        for field in ["crop_left", "crop_right", "crop_top", "crop_bottom"] {
            fields.push((field, Value::Int(0)));
        }
        let bounds = if picture && time == 0 {
            [0; 4]
        } else {
            [-32768000, 32768000, -32768000, 32768000]
        };
        fields.push((
            "box",
            Value::List(bounds.into_iter().map(Value::Int).collect()),
        ));
        for field in ["box_xscale", "box_yscale", "box_xpos", "box_ypos"] {
            fields.push((field, Value::Bool(picture)));
        }
        for field in [
            "border_width",
            "border_soft",
            "splill_gain2",
            "splill_gain",
            "splill_soft2",
            "splill_soft",
            "enable_key_flags",
        ] {
            fields.push((field, Value::Int(0)));
        }
        fields.extend([
            ("colors", Value::List(vec![])),
            ("user_param", Value::Bytes(vec![])),
            ("selected", Value::Bool(true)),
        ]);
        parameters.push(embedded(*b"FXPS", "EffectParam", fields));
    }
    add(
        doc,
        *b"FXPS",
        "EffectParamList",
        vec![
            ("orig_length", Value::Int(-1)),
            ("window_offset", Value::Int(0)),
            ("keyframe_size", Value::Int(168)),
            ("parameters", Value::List(parameters)),
        ],
    )
}

pub(super) fn write(doc: &mut AvbDocument, rate: f64, kind: i64, t: Transition) -> Result<u32> {
    let picture = kind == 1;
    let name = if picture {
        "Dissolve"
    } else {
        "Audio Dissolve"
    };
    let publisher = if picture { "Avid Technology, Inc." } else { "" };
    let maker = if picture {
        "ccbf7662-ac25-4f8f-8abb-0a32df222f71"
    } else {
        ""
    };
    let product = if picture {
        "409578e0-2ee2-4f08-ac91-19df6b1cc737"
    } else {
        ""
    };
    let mut attrs: Vec<_> = [
        ("_EFFECT_PLUGIN_TYPE", "AVX2"),
        ("_EFFECT_PLUGIN_NAME", name),
        ("_EFFECT_PLUGIN_CLASS", "Blend"),
        ("_EFFECT_PLUGIN_MANUFACTURER_ID", maker),
        ("_EFFECT_PLUGIN_MANUFACTURER_NAME", publisher),
        ("_EFFECT_PLUGIN_PRODUCT_ID", product),
        ("_EFFECT_PLUGIN_PRODUCT_NAME", ""),
        ("_EFFECT_PLUGIN_VERSION", "1.0"),
        ("_EFFECT_PLUGIN_SKU", ""),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), Value::Str(value.into())))
    .collect();
    if !picture {
        attrs.push((CURVE_ATTR.into(), Value::Int(1)));
        attrs.push((
            "_EFFECT_PLUGIN_CACHE_INFO".into(),
            Value::Str("AVX2AudioDissolve ### Blend ### AVX2 ### Version 1.0".into()),
        ));
    }
    let attributes = add(
        doc,
        *b"ATTR",
        "Attributes",
        vec![("entries", Value::Attrs(attrs))],
    )?;
    let duration = i64::from(t.duration);
    let params = parameters(doc, rate, picture, duration)?;
    let keyframes = keyframes(doc, picture)?;
    let track = embedded([0; 4], "Track", vec![("lock_number", Value::Int(0))]);
    let mut fields = component(rate, kind);
    for (key, value) in &mut fields {
        match *key {
            "effect_id" => *value = Value::Str(if picture { VIDEO } else { AUDIO }.into()),
            "attributes" => *value = Value::Ref(attributes),
            "param_list" => *value = Value::Ref(params),
            _ => {}
        }
    }
    // Leave render/cache references empty. They are tied to the source bin's media.
    fields.extend([
        ("mc_mode", Value::Int(1)),
        ("length", Value::Int(duration)),
        ("num_scalars", Value::Int(0)),
        ("tracks", Value::List(vec![track.clone(), track])),
        ("cutpoint", Value::Int(i64::from(t.cut_offset))),
        ("left_length", Value::Int(15)),
        ("right_length", Value::Int(15)),
        ("info_version", Value::Int(2)),
        ("info_current", Value::Int(0)),
        ("info_smooth", Value::Int(0)),
        ("info_color_item", Value::Int(1)),
        ("info_quality", Value::Int(1)),
        ("info_is_reversed", Value::Int(0)),
        ("info_aspect_on", Value::Bool(picture)),
        ("keyframes", Value::Ref(keyframes)),
        ("info_force_software", Value::Bool(false)),
        ("info_never_hardware", Value::Bool(false)),
    ]);
    add(doc, *b"TNFX", "TransitionEffect", fields)
}

pub(super) fn matches(doc: &AvbDocument, node: &Node, picture: bool, t: Transition) -> bool {
    let reference = |owner: &Node, field| {
        owner
            .get(field)
            .and_then(Value::as_index)
            .and_then(|id| doc.node(id))
    };
    if node.class != *b"TNFX"
        || node.get("effect_id").and_then(Value::as_str)
            != Some(if picture { VIDEO } else { AUDIO })
        || node.get("length") != Some(&Value::Int(i64::from(t.duration)))
        || node.get("cutpoint") != Some(&Value::Int(i64::from(t.cut_offset)))
        || reference(node, "keyframes").is_none_or(|keys| keys.class != *b"FXPS")
    {
        return false;
    }
    let Some(Value::RefList(params)) = reference(node, "param_list").and_then(|p| p.get("items"))
    else {
        return false;
    };
    if picture {
        return params.len() == 4;
    }
    let curve_value = reference(node, "attributes").and_then(|attr| attr.get("entries"));
    matches!(curve_value, Some(Value::Attrs(entries))
        if entries.iter().filter(|(field, _)| field == CURVE_ATTR).map(|(_, v)| v).eq([&Value::Int(1)]))
        && params
            .iter()
            .filter_map(Value::as_index)
            .filter_map(|id| doc.node(id))
            .filter(|param| param.get("uuid") == Some(&Value::Uuid(uuid(CURVE_PARAM))))
            .map(|param| param.get("value"))
            .eq([Some(&Value::Int(1))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::{source::SourceBinding, Rational, StreamKind};

    #[test]
    fn writes_centered_transition_with_owned_source_handles() {
        let timing = |frames| Rational::new(frames, 24).unwrap();
        let binding = |id: &str| SourceBinding {
            placements: vec![id.into()],
            reason: None,
        };
        let mut transition = SourceTransition {
            id: "transition".into(),
            kind: StreamKind::Audio,
            track: 2,
            style: "premiere.Constant Power".into(),
            cut: timing(8),
            duration: timing(6),
            cut_offset: timing(3),
            outgoing: binding("out"),
            incoming: binding("in"),
        };
        let spans = [
            (
                "out",
                Span {
                    record: 0,
                    source: 8,
                    length: 11,
                },
            ),
            (
                "in",
                Span {
                    record: 5,
                    source: 2,
                    length: 12,
                },
            ),
        ];
        let prepared = prepare(
            [&transition].into_iter(),
            &spans,
            EditRate::Fps24,
            false,
            "A2",
        )
        .unwrap();
        let [prepared] = prepared.as_slice() else {
            panic!("expected one prepared crossfade");
        };
        let mut doc = AvbDocument::new_empty_bin(1, "transition test", 0, false);
        let id = write(&mut doc, 24.0, 2, *prepared).unwrap();
        let readback = AvbDocument::from_bytes(doc.to_bytes().unwrap()).unwrap();
        assert!(matches(
            &readback,
            readback.node(id).unwrap(),
            false,
            *prepared
        ));

        transition.incoming = binding("out");
        assert!(prepare(
            [&transition].into_iter(),
            &spans,
            EditRate::Fps24,
            false,
            "A2"
        )
        .unwrap_err()
        .to_string()
        .contains("exactly the saved transition overlap"));
    }
}
