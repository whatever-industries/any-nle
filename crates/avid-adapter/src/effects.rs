use super::*;
use avid_reader::extract::SeqEvent;
use avid_reader::source::PictureExtension;

pub(super) fn apply_picture_extension(
    effect: PictureExtension, raster: Option<(i64, i64)>, p: &mut Placement,
    out: &mut ReadResult, budget: &mut Budget,
) -> Result<(), String> {
    let (kind, params) = match effect {
        PictureExtension::Unsupported(reason) | PictureExtension::UnsupportedTransform(reason) => {
            return loss(out, budget, &p.id, "picture_effect", reason);
        }
        PictureExtension::Flip { horizontal, vertical } => {
            if p.transform.is_some() || !p.effects.is_empty() {
                return loss(out, budget, &p.id, "picture_effect", "Flip with another translated effect requires effect ordering qualification");
            }
            ("avid.flip", obj(vec![("horizontal", Value::Bool(horizontal)), ("vertical", Value::Bool(vertical))]))
        }
        PictureExtension::WarpTrim(trim) | PictureExtension::ResizeTrim(trim) => {
            let resize = matches!(effect, PictureExtension::ResizeTrim(_));
            let kind = match (resize, trim.crop.is_some()) {
                (true, true) => "avid.resize_trim_crop_linear",
                (true, false) => "avid.resize_trim_linear",
                (false, true) => "avid.warp_trim_crop_linear",
                (false, false) => "avid.warp_trim_linear",
            };
            if raster != Some((1920, 1080)) || p.transform.is_some() || !p.effects.is_empty()
                || trim.motion.iter().any(|k| k.position.iter().any(|x| x.abs() > 64.0))
            {
                return loss(out, budget, &p.id, "picture_effect", "Trimmed picture animation requires qualified 1080p geometry and an isolated effect");
            }
            p.transform = Some(Transform {
                position: trim.motion[0].position.into(),
                scale: (trim.width[0].1, trim.motion[0].height),
                rotation: 0.0,
                anchor: (0.0, 0.0),
                crop: [0.0; 4],
            });
            let motion = trim.motion.map(|k| obj(vec![
                ("frame", Value::Int(k.frame)),
                ("position", Value::Arr(k.position.into_iter().map(Value::Float).collect())),
                ("height", Value::Float(k.height)),
            ]));
            let width = trim.width.map(|(frame, value)| Value::Arr(vec![Value::Int(frame), Value::Float(value)]));
            let mut params = vec![
                ("motion", Value::Arr(motion.into())),
                ("width", Value::Arr(width.into())),
                ("fixed_width", Value::Bool(trim.fixed_width)),
            ];
            if let Some(crop) = trim.crop {
                let keys = crop.map(|(frame, edges)| obj(vec![
                    ("frame", Value::Int(frame)),
                    ("edges", Value::Arr(edges.into_iter().map(Value::Float).collect())),
                ]));
                params.push(("crop", Value::Arr(keys.into())));
            }
            (kind, obj(params))
        }
        PictureExtension::Resize(keys) | PictureExtension::ResizeCrop(keys) | PictureExtension::Warp(keys) => {
            let kind = match effect {
                PictureExtension::ResizeCrop(_) => "avid.resize_crop_linear",
                PictureExtension::Warp(_) if avid_reader::source::ResizeKey::has_crop(keys) => "avid.warp_crop_linear",
                PictureExtension::Warp(_) => "avid.warp_linear",
                _ => "avid.resize_linear",
            };
            let keyed_crop = matches!(kind, "avid.resize_crop_linear" | "avid.warp_crop_linear");
            if !p.effects.is_empty() || p.transform.as_ref().is_some_and(|t| t.crop != [0.0; 4]) {
                return loss(out, budget, &p.id, "picture_effect", "Animated picture motion with crop or another translated effect requires separate qualification");
            }
            if raster != Some((1920, 1080)) {
                return loss(out, budget, &p.id, "picture_effect", "Animated picture motion is qualified only for 1920x1080 geometry");
            }
            if keys.iter().any(|k| k.position.iter().any(|x| x.abs() > 64.0)) {
                return loss(out, budget, &p.id, "picture_effect", "Animated picture position is outside the qualified range");
            }
            let transform = p.transform.get_or_insert(Transform {
                position: (0.0, 0.0), scale: (100.0, 100.0), rotation: 0.0,
                anchor: (0.0, 0.0), crop: [0.0; 4],
            });
            transform.position = keys[0].position.into();
            transform.scale = keys[0].scale.into();
            let keys = keys.map(|k| {
                let mut params = vec![
                    ("frame", Value::Int(k.frame)),
                    ("position", Value::Arr(k.position.into_iter().map(Value::Float).collect())),
                    ("scale", Value::Arr(k.scale.into_iter().map(Value::Float).collect())),
                ];
                if keyed_crop {
                    params.push(("crop", Value::Arr(k.crop.into_iter().map(Value::Float).collect())));
                }
                obj(params)
            });
            (kind, obj(vec![("keys", Value::Arr(keys.into()))]))
        }
    };
    budget.value(&params)?;
    budget.charge(size_of::<Effect>() + kind.len())?;
    p.effects.push(Effect { kind: kind.into(), params });
    Ok(())
}

/// Original owned parser facts. These are opaque witnesses, not additional interpretations.
pub(super) fn receipt(event: &SeqEvent) -> Value {
    fn params(cc: &avid_reader::color::ColorCorrection) -> Value {
        use avid_reader::color::ParamValue;
        Value::Arr(
            cc.params
                .iter()
                .map(|(id, value)| {
                    obj(vec![
                        ("id", s(id)),
                        (
                            "value",
                            match value {
                                ParamValue::Int(n) => obj(vec![("int", Value::Int(*n))]),
                                ParamValue::Float(n) => obj(vec![("float", raw_float(*n))]),
                                ParamValue::Bytes(b) => obj(vec![(
                                    "bytes_hex",
                                    s(b.iter().map(|x| format!("{x:02X}")).collect::<String>()),
                                )]),
                                ParamValue::Other => {
                                    obj(vec![("uninterpreted", Value::Bool(true))])
                                }
                            },
                        ),
                    ])
                })
                .collect(),
        )
    }
    let point = |p: &avid_reader::animatte::AniMattePoint| {
        obj(vec![
            ("xy", Value::Arr(p.xy.into_iter().map(raw_float).collect())),
            (
                "incoming",
                Value::Arr(p.incoming.into_iter().map(raw_float).collect()),
            ),
            (
                "outgoing",
                Value::Arr(p.outgoing.into_iter().map(raw_float).collect()),
            ),
            ("corner", Value::Bool(p.corner)),
        ])
    };
    obj(vec![
        (
            "color",
            event.color.as_ref().map(params).unwrap_or(Value::Null),
        ),
        (
            "picture",
            event
                .picture
                .as_ref()
                .map(|p| {
                    obj(vec![
                        (
                            "kind",
                            s(match p.kind {
                                avid_reader::transform::Kind::Resize => "resize",
                                avid_reader::transform::Kind::Warp => "warp",
                            }),
                        ),
                        ("params", params(&p.params)),
                        ("nested", Value::Bool(event.picture_nested)),
                    ])
                })
                .unwrap_or(Value::Null),
        ),
        (
            "animatte",
            event
                .animatte
                .as_ref()
                .map(|a| {
                    obj(vec![
                        (
                            "states",
                            Value::Arr(
                                a.states
                                    .iter()
                                    .map(|st| {
                                        obj(vec![
                                            ("frame", Value::Int(st.frame as i64)),
                                            (
                                                "shapes",
                                                Value::Arr(
                                                    st.shapes
                                                        .iter()
                                                        .map(|sh| {
                                                            obj(vec![
                                                                ("id", Value::Int(sh.id as i64)),
                                                                (
                                                                    "key_out",
                                                                    Value::Bool(sh.key_out),
                                                                ),
                                                                ("opacity", raw_float(sh.opacity)),
                                                                (
                                                                    "feather",
                                                                    Value::Arr(
                                                                        sh.feather
                                                                            .into_iter()
                                                                            .map(raw_float)
                                                                            .collect(),
                                                                    ),
                                                                ),
                                                                (
                                                                    "points",
                                                                    Value::Arr(
                                                                        sh.points
                                                                            .iter()
                                                                            .map(point)
                                                                            .collect(),
                                                                    ),
                                                                ),
                                                            ])
                                                        })
                                                        .collect(),
                                                ),
                                            ),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                        (
                            "tracker_xy",
                            a.tracker_xy
                                .as_ref()
                                .map(|p| {
                                    Value::Arr(
                                        p.iter()
                                            .map(|xy| {
                                                Value::Arr(
                                                    xy.iter().copied().map(raw_float).collect(),
                                                )
                                            })
                                            .collect(),
                                    )
                                })
                                .unwrap_or(Value::Null),
                        ),
                        (
                            "not_converted",
                            Value::Arr(a.not_converted.iter().map(s).collect()),
                        ),
                    ])
                })
                .unwrap_or(Value::Null),
        ),
        (
            "speed",
            event
                .speed
                .map(|(n, d)| Value::Arr(vec![Value::Int(n), Value::Int(d)]))
                .unwrap_or(Value::Null),
        ),
    ])
}
pub(super) fn reserve_receipt(event: &SeqEvent, budget: &mut Budget) -> Result<(), String> {
    use avid_reader::color::ParamValue;
    budget.charge(2048)?;
    for cc in event
        .color
        .iter()
        .chain(event.picture.iter().map(|p| &p.params))
    {
        for (id, v) in &cc.params {
            budget.charge(1024 + id.len())?;
            if let ParamValue::Bytes(b) = v {
                budget.charge(
                    b.len()
                        .checked_mul(2)
                        .ok_or("raw effect byte budget overflow")?,
                )?;
            }
        }
    }
    if let Some(a) = &event.animatte {
        for st in &a.states {
            budget.charge(512)?;
            for sh in &st.shapes {
                budget.charge(1024)?;
                budget.charge(
                    sh.points
                        .len()
                        .checked_mul(1024)
                        .ok_or("raw matte point budget overflow")?,
                )?;
            }
        }
        if let Some(t) = &a.tracker_xy {
            budget.charge(
                t.len()
                    .checked_mul(4 * size_of::<Value>())
                    .ok_or("raw tracker budget overflow")?,
            )?;
        }
        for n in &a.not_converted {
            budget.charge(n.len() + size_of::<Value>())?;
        }
    }
    Ok(())
}

pub(super) fn apply(
    event: &SeqEvent,
    static_picture: bool,
    raster: Option<(i64, i64)>,
    p: &mut Placement,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<(), String> {
    if let Some(cc) = &event.color {
        let it = cc.interpret();
        let params = obj(vec![
            (
                "saturation",
                it.saturation.map(Value::Float).unwrap_or(Value::Null),
            ),
            (
                "tone",
                Value::Obj(
                    it.tone
                        .iter()
                        .map(|(k, v)| (k.to_lowercase(), Value::Float(*v)))
                        .collect(),
                ),
            ),
            (
                "curves",
                Value::Obj(
                    it.curves
                        .iter()
                        .map(|(k, pts)| {
                            (
                                k.to_string(),
                                Value::Arr(
                                    pts.iter()
                                        .map(|(x, y)| {
                                            Value::Arr(vec![Value::Float(*x), Value::Float(*y)])
                                        })
                                        .collect(),
                                ),
                            )
                        })
                        .collect(),
                ),
            ),
        ]);
        budget.value(&params)?;
        p.effects.push(Effect {
            kind: "avid.color_correction".into(),
            params,
        });
        for reason in it.not_converted {
            loss(out, budget, &p.id, "color_correction", reason)?;
        }
        for note in it.notes {
            out.diagnostics
                .push(format!("{} color correction: {note}", p.id));
        }
    }
    if let Some(fx) = event.picture.as_ref().filter(|_| static_picture) {
        let t = fx.interpret();
        if !t.is_identity() || !t.not_converted.is_empty() || event.picture_nested {
            p.transform = Some(Transform {
                position: t.position,
                scale: t.scale,
                rotation: t.rotation,
                anchor: t.anchor,
                crop: t.crop,
            });
        }
        for reason in t.not_converted {
            loss(out, budget, &p.id, "transform", reason)?;
        }
        if event.picture_nested {
            loss(
                out,
                budget,
                &p.id,
                "transform",
                format!(
                    "{} over another Resize/3D Warp (only the inner one is converted)",
                    fx.name()
                ),
            )?;
        }
    }
    if let Some(data) = &event.animatte {
        if raster != Some((1920, 1080)) {
            loss(
                out,
                budget,
                &p.id,
                "matte",
                "AniMatte is calibrated only for 1920x1080 sources",
            )?;
            return Ok(());
        }
        let Some(first) = data.states.first().filter(|s| !s.shapes.is_empty()) else {
            loss(
                out,
                budget,
                &p.id,
                "matte",
                "AniMatte contains no qualified shapes",
            )?;
            return Ok(());
        };
        if data
            .states
            .iter()
            .any(|s| s.shapes.len() != first.shapes.len())
            || data.tracker_xy.as_ref().is_some_and(|s| s.is_empty())
        {
            loss(
                out,
                budget,
                &p.id,
                "matte",
                "AniMatte states/tracker topology is unresolved",
            )?;
            return Ok(());
        }
        // Reserve the full expansion before allocating keyframe/point values. Each normalized
        // point owns a Vec and six Value slots. This deliberately overcharges repeated states.
        let keys = data.tracker_xy.as_ref().map_or(data.states.len(), Vec::len);
        let points = first
            .shapes
            .iter()
            .try_fold(0usize, |sum, s| sum.checked_add(s.points.len()))
            .ok_or("AniMatte point budget overflow")?;
        let estimate = points
            .checked_mul(keys)
            .and_then(|n| n.checked_mul(8 * size_of::<Value>()))
            .and_then(|n| n.checked_add(keys.checked_mul(first.shapes.len())?.checked_mul(512)?))
            .ok_or("AniMatte expansion budget overflow")?;
        budget.charge(estimate)?;
        let point = |p: &avid_reader::animatte::AniMattePoint, shift: [f64; 2]| {
            let h = if p.corner {
                [0.0; 4]
            } else {
                [
                    p.incoming[0] / 720.0,
                    p.incoming[1] / 243.0,
                    p.outgoing[0] / 720.0,
                    p.outgoing[1] / 243.0,
                ]
            };
            Value::Arr(
                [
                    0.5 + p.xy[0] / 720.0 + shift[0],
                    0.5 + p.xy[1] / 243.0 + shift[1],
                    h[0],
                    h[1],
                    h[2],
                    h[3],
                ]
                .into_iter()
                .map(Value::Float)
                .collect(),
            )
        };
        let key = |frame: u32, shape: &avid_reader::animatte::AniMatteShape, shift: [f64; 2]| {
            obj(vec![
                ("frame", Value::Int(frame as i64)),
                (
                    "points",
                    Value::Arr(shape.points.iter().map(|p| point(p, shift)).collect()),
                ),
            ])
        };
        let shapes = first
            .shapes
            .iter()
            .enumerate()
            .map(|(i, shape)| {
                let keys = match &data.tracker_xy {
                    Some(samples) => {
                        let origin = samples[0];
                        samples
                            .iter()
                            .enumerate()
                            .map(|(n, xy)| {
                                key(n as u32, shape, [xy[0] - origin[0], xy[1] - origin[1]])
                            })
                            .collect()
                    }
                    None if data.states.iter().all(|s| s.shapes[i] == *shape) => {
                        vec![key(0, shape, [0.0; 2])]
                    }
                    None => data
                        .states
                        .iter()
                        .map(|s| key(s.frame, &s.shapes[i], [0.0; 2]))
                        .collect(),
                };
                obj(vec![
                    ("opacity", Value::Float(shape.opacity)),
                    (
                        "feather",
                        Value::Arr(shape.feather.into_iter().map(Value::Float).collect()),
                    ),
                    ("keyframes", Value::Arr(keys)),
                ])
            })
            .collect();
        p.effects.push(Effect {
            kind: "avid.animatte".into(),
            params: obj(vec![
                ("inverted", Value::Bool(first.shapes[0].key_out)),
                ("shapes", Value::Arr(shapes)),
            ]),
        });
        for reason in &data.not_converted {
            loss(out, budget, &p.id, "matte", reason.clone())?;
        }
    }
    Ok(())
}
