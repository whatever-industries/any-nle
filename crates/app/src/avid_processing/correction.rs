//! Ordinary Avid CC Effect parameters observed on a static timeline segment.
use avid_reader::io::{Error, Uuid};
use avid_reader::objects::{Node, Value};
use avid_reader::{
    source::{SourceClampedCdl, SourceColorRange},
    AvbDocument,
};

pub(super) const ADAPTER: &str = "EFF2_LUTSFX";
pub(super) const CORRECTION: &str = "EFF2_RGB_COLOR_CORRECTION";

const VIDEO_BLACK: f64 = SourceColorRange::VIDEO_BLACK as f64;
const VIDEO_WHITE: f64 = SourceColorRange::VIDEO_WHITE as f64;
const VIDEO_WIDTH: f64 = VIDEO_WHITE - VIDEO_BLACK;
const FULL_WHITE: f64 = ((1u32 << SourceColorRange::BIT_DEPTH) - 1) as f64;

// Avid's Master Curve floors and caps its RGB input domain. Reserve headroom
// with HSL Contrast before the curve floors the shadows, then restore the
// intended scale with a second correction. The lift leaves room for the first
// correction's output encoding before the restoration removes it.
const COMPRESSION_SCALE: f64 = 0.46;
const COMPRESSION_LIFT: f64 = 8.0;

#[derive(Clone, Copy)]
pub(super) enum Transform {
    Range(SourceColorRange),
    Cdl(SourceClampedCdl),
}

impl Transform {
    pub(super) fn from_saved_xml(xml: &str) -> Option<Self> {
        SourceColorRange::from_saved_xml(xml)
            .map(Self::Range)
            .or_else(|| {
                let cdl = SourceClampedCdl::from_saved_xml(xml)?;
                // Keep the trial within ordinary corrections with well-separated
                // toes and shoulders. The curve interpolation near a knee is
                // approximate even within these admitted values.
                cdl.slope
                    .iter()
                    .all(|v| (0.5..=2.0).contains(v))
                    .then_some(())?;
                cdl.offset
                    .iter()
                    .all(|v| (-0.25..=0.25).contains(v))
                    .then_some(())?;
                cdl.power
                    .iter()
                    .all(|v| (0.75..=1.5).contains(v))
                    .then_some(Self::Cdl(cdl))
            })
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Range(SourceColorRange::VideoToFull) => "Video levels to full range",
            Self::Range(SourceColorRange::FullToVideo) => "Full range to video levels",
            Self::Cdl(cdl) if cdl.is_affine() => "Affine ASC CDL",
            Self::Cdl(_) => "ASC CDL slope, offset and power",
        }
    }

    pub(super) fn is_approximation(self) -> bool {
        !matches!(self, Self::Range(SourceColorRange::VideoToFull))
    }

    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Range(SourceColorRange::VideoToFull) =>
                "Editable RGB Master Curve for video levels to full range.",
            Self::Range(SourceColorRange::FullToVideo) =>
                "Editable nested Color Correction trial. Preserves highlight headroom with measured rounding differences.",
            Self::Cdl(cdl) if cdl.is_affine() =>
                "Editable RGB Curves trial for clamped ASC CDL slope and offset. Rounding near black and white may differ.",
            Self::Cdl(_) =>
                "Editable RGB Curves trial for clamped ASC CDL slope, offset and power. The curved response is sampled.",
        }
    }
}

#[derive(Clone, Copy)]
struct CurveParams {
    points: u128,
    count: u128,
    enable: u128,
}

const MASTER_CURVE: CurveParams = CurveParams {
    points: 0x225fb7354acb45c7b9b6deacdaf96354,
    count: 0xd70971aa74e84ecc9214f0040fc48987,
    enable: 0xd87f4323ba3c4e2a92adf0afe57ba965,
};

const RGB_CURVES: [CurveParams; 3] = [
    CurveParams {
        points: 0x1d0d1e20aa18446caf28e99784c1b19e,
        count: 0xeb070511b53f40f18a1feb2e07b74635,
        enable: 0x77748d792c87407a9818f233d031ee07,
    },
    CurveParams {
        points: 0x2c1df123351141f7b4713ee88dc485ec,
        count: 0xd60fa4c6213f494daf870a134d4b1271,
        enable: 0x9c64744d8c5141bfbf1ccea6a4ffbcfb,
    },
    CurveParams {
        points: 0x97bb6a821af94439bf066e64dc62167f,
        count: 0xbe4aabae840d4dc4921976d8127ab4f4,
        enable: 0xf8500cbafe294a1d8a63066c14162f75,
    },
];

fn cdl_curve_points(slope: f64, offset: f64, power: f64) -> Vec<[f64; 2]> {
    let intercept = VIDEO_BLACK * (1.0 - slope) + VIDEO_WIDTH * offset;
    let black = (VIDEO_BLACK - intercept) / slope;
    let white = (VIDEO_WHITE - intercept) / slope;
    let mut xs = Vec::with_capacity(16);
    xs.push(0.0);
    for knee in [black, white] {
        if 0.0 < knee && knee < FULL_WHITE {
            // Adjacent samples constrain the native spline tangent to the
            // narrow neighborhood of the clipping boundary.
            xs.extend(
                [knee - 1.0, knee, knee + 1.0]
                    .into_iter()
                    .filter(|x| 0.0 < *x && *x < FULL_WHITE),
            );
        }
    }
    xs.push(FULL_WHITE);
    if power != 1.0 {
        let first = black.clamp(0.0, FULL_WHITE);
        let last = white.clamp(0.0, FULL_WHITE);
        // At most six clipping-point samples and eight interior samples.
        // Spend more points near the steep end of sub-unity power curves.
        for index in 1..=8 {
            let progress = f64::from(index) / 9.0;
            xs.push(first + (last - first) * progress.powi(2));
        }
        xs.sort_by(f64::total_cmp);
        xs.dedup();
    }
    xs.into_iter()
        .map(|x| {
            let clipped = (slope * x + intercept).clamp(VIDEO_BLACK, VIDEO_WHITE);
            let output = if power == 1.0 {
                clipped
            } else {
                VIDEO_BLACK + VIDEO_WIDTH * ((clipped - VIDEO_BLACK) / VIDEO_WIDTH).powf(power)
            };
            [x, output]
        })
        .collect()
}

fn hsl_controls(slope: f64, intercept: f64) -> (f64, f64) {
    let contrast = (slope.atan().to_degrees() - 45.0) / 0.45;
    // Contrast operates about the measured Rec.709 pivot. Brightness adds
    // luma after Contrast, using the UI control's transformation in 10-bit codes.
    let brightness =
        (intercept - (1.0 - slope) * (VIDEO_BLACK + 0.507 * VIDEO_WIDTH)) / (140.0 / 30.0);
    (contrast, brightness)
}

fn curve_points(points: &[[f64; 2]]) -> Vec<u8> {
    // AVUP uses Avid's 10-bit RGB units. A video code c is 256 + c/2
    // in that space. The remaining slots are inactive.
    let mut data = Vec::with_capacity(16 * 16);
    for value in points.iter().flatten() {
        data.extend((256.0 + value / 2.0).to_le_bytes());
    }
    data.resize(16 * 16, 0);
    data
}

pub(super) fn make(
    document: &mut AvbDocument,
    wrapper: Node,
    transform: Transform,
) -> Result<u32, Error> {
    match transform {
        Transform::Range(SourceColorRange::VideoToFull) => grade(
            document,
            wrapper,
            None,
            &[(
                MASTER_CURVE,
                &[[VIDEO_BLACK, 0.0], [VIDEO_WHITE, FULL_WHITE]],
            )],
        ),
        Transform::Range(SourceColorRange::FullToVideo) => {
            let inner = grade(
                document,
                wrapper.clone(),
                Some(hsl_controls(COMPRESSION_SCALE, 0.0)),
                &[(
                    MASTER_CURVE,
                    &[
                        [0.0, COMPRESSION_LIFT],
                        [FULL_WHITE, FULL_WHITE + COMPRESSION_LIFT],
                    ],
                )],
            )?;
            let restore = VIDEO_WIDTH / (FULL_WHITE * COMPRESSION_SCALE);
            let wrapper = nest_grade(document, wrapper, inner)?;
            grade(
                document,
                wrapper,
                Some(hsl_controls(
                    restore,
                    VIDEO_BLACK - restore * COMPRESSION_LIFT,
                )),
                &[],
            )
        }
        Transform::Cdl(cdl) => {
            let curves: [_; 3] = std::array::from_fn(|i| {
                cdl_curve_points(cdl.slope[i], cdl.offset[i], cdl.power[i])
            });
            let entries: [_; 3] = std::array::from_fn(|i| (RGB_CURVES[i], curves[i].as_slice()));
            grade(document, wrapper, None, &entries)
        }
    }
}

fn nest_grade(document: &mut AvbDocument, mut wrapper: Node, inner: u32) -> Result<Node, Error> {
    let invalid = || Error::new("The prepared Color Adapter input changed shape", 0);
    let Some((_, Value::List(tracks))) =
        wrapper.fields.iter_mut().find(|(key, _)| *key == "tracks")
    else {
        return Err(invalid());
    };
    let [Value::Node(track)] = tracks.as_mut_slice() else {
        return Err(invalid());
    };
    let (_, component) = track
        .fields
        .iter_mut()
        .find(|(key, _)| *key == "component")
        .ok_or_else(invalid)?;
    let source_track = component.as_index().ok_or_else(invalid)?;
    let Some(Value::RefList(parts)) = document
        .node(source_track)
        .and_then(|node| node.get("components"))
    else {
        return Err(invalid());
    };
    let [lead, _, tail] = parts.as_slice() else {
        return Err(invalid());
    };
    let lead = lead.as_index().ok_or_else(invalid)?;
    let tail = tail.as_index().ok_or_else(invalid)?;
    // Avid disposes these sequence children with their owning sequence.
    let lead = document.duplicate_saved_object(lead, *b"FILL")?;
    let tail = document.duplicate_saved_object(tail, *b"FILL")?;
    let parts = vec![Value::Ref(lead), Value::Ref(inner), Value::Ref(tail)];
    let copied = document.duplicate_saved_object(source_track, *b"SEQU")?;
    document.set_field(copied, "components", Value::RefList(parts))?;
    *component = Value::Ref(copied);
    Ok(wrapper)
}

fn grade(
    document: &mut AvbDocument,
    mut wrapper: Node,
    hsl: Option<(f64, f64)>,
    curves: &[(CurveParams, &[[f64; 2]])],
) -> Result<u32, Error> {
    let grade_attrs = add(
        document,
        *b"ATTR",
        "Attributes",
        vec![(
            "entries",
            Value::Attrs(vec![
                attr("_EFFECT_PLUGIN_TYPE", "AVX2"),
                attr("_EFFECT_PLUGIN_NAME", "Color Correction"),
                attr("_EFFECT_PLUGIN_CLASS", "Image"),
                attr(
                    "_EFFECT_PLUGIN_MANUFACTURER_ID",
                    "ccbf7662-ac25-4f8f-8abb-0a32df222f71",
                ),
                attr("_EFFECT_PLUGIN_MANUFACTURER_NAME", "Avid Technology, Inc."),
                attr(
                    "_EFFECT_PLUGIN_PRODUCT_ID",
                    "409578e0-2ee2-4f08-ac91-19df6b1cc737",
                ),
                attr("_EFFECT_PLUGIN_PRODUCT_NAME", ""),
                attr("_EFFECT_PLUGIN_VERSION", "1.0"),
                attr("_EFFECT_PLUGIN_SKU", ""),
                ("_BORDER_FIXED".into(), Value::Int(0)),
                ("_NEW_SHAPE_REFERENCE_FRAME".into(), Value::Int(1)),
                ("_IGNORE_TRACKING".into(), Value::Int(0)),
            ]),
        )],
    )?;
    let empty_params = add(
        document,
        *b"PRLS",
        "ParameterList",
        vec![("items", Value::RefList(Vec::new()))],
    )?;
    let mut empty_curve = wrapper.clone();
    empty_curve.class = *b"PRCL";
    empty_curve.type_name = "ParamClip";
    empty_curve.fields.retain(|(key, _)| {
        matches!(
            *key,
            "left_bob"
                | "right_bob"
                | "media_kind_id"
                | "edit_rate"
                | "name"
                | "effect_id"
                | "attributes"
                | "session_attrs"
                | "precomputed"
                | "param_list"
                | "length"
        )
    });
    for (key, value) in &mut empty_curve.fields {
        match *key {
            "media_kind_id" => *value = Value::Int(10),
            "effect_id" | "attributes" => *value = Value::Null,
            "param_list" => *value = Value::Ref(empty_params),
            _ => {}
        }
    }
    empty_curve.fields.extend([
        ("interp_kind", Value::Int(2)),
        ("value_type", Value::Int(4)),
        ("control_points", Value::List(Vec::new())),
        ("extrap_kind", Value::Int(1)),
        ("fields", Value::Int(1)),
    ]);
    let curve = document.add_object(empty_curve)?;

    let mut parameters = vec![
        (
            0x91d210eab6ce4a95bf31522c9c23f30a,
            4,
            Value::Null,
            Value::Ref(curve),
            true,
            true,
        ),
        (
            0x8af7a5dc367b4919afc75ca00a5bd558,
            1,
            Value::Int(1),
            Value::Null,
            true,
            false,
        ),
    ];
    // Avid 25.12 migrates HSL-only effects to schema 11 with the same
    // control values. Mixed HSL and Curves retain the saved schema 5 IDs.
    let (channels, contrast_id, brightness_id, schema_id, schema, signature_id) =
        if !curves.is_empty() {
            (
                [
                    0xeb27c63d5ac441a1b44fbd6cbb5c739f,
                    0x105874f7208d4b649b37f5422042bdfe,
                    0x6e11f54198ff40f88ea8d52a9d211833,
                ],
                0xf91052d568c34558970a894366d3c41a,
                0x685a2d0287bb4068ac5b79c9ef4bb025,
                0x3b3b5d0619844f068c19aa62a1e7616d,
                5,
                0x02435b330a97466a86dc0eaf9570b807,
            )
        } else {
            (
                [
                    0xeae3e18fc9334670b9c3606b6d69f44b,
                    0xa91ade368f7a4633995ea6aa96c09218,
                    0x2848905f773049a9a16f34ce3cbe3721,
                ],
                0x5e1a4611ba2643a9a22334eb095df41e,
                0x4fcfb16cc6ad4d0e82c9efb72c1f701e,
                0x4b6ab71e34a34c2485306693e2f8a2eb,
                11,
                0x484f9a96686d440e9360990be1936e1d,
            )
        };
    parameters.extend(channels.map(|id| (id, 1, Value::Int(1), Value::Null, false, true)));
    if let Some((contrast, brightness)) = hsl {
        parameters.extend([
            (
                brightness_id,
                2,
                Value::Float(brightness),
                Value::Null,
                true,
                true,
            ),
            (
                contrast_id,
                2,
                Value::Float(contrast),
                Value::Null,
                true,
                true,
            ),
        ]);
    }
    parameters.extend([
        (schema_id, 1, Value::Int(schema), Value::Null, true, false),
        (
            signature_id,
            1,
            Value::Int(1794296171),
            Value::Null,
            true,
            true,
        ),
    ]);
    for (controls, coordinates) in curves {
        let payload = add(
            document,
            *b"AVUP",
            "CFUserParam",
            vec![
                ("byte_order", Value::Int(0x4949)),
                (
                    "uuid",
                    Value::Uuid(uuid(0xe1c94795a4eb46a081329085c235079a)),
                ),
                ("data", Value::Bytes(curve_points(coordinates))),
            ],
        )?;
        parameters.extend([
            (
                controls.points,
                4,
                Value::Ref(payload),
                Value::Null,
                true,
                true,
            ),
            (controls.enable, 1, Value::Int(1), Value::Null, false, true),
            (
                controls.count,
                1,
                Value::Int(coordinates.len() as i64),
                Value::Null,
                false,
                true,
            ),
        ]);
    }
    if !curves.is_empty() {
        // Enable the editable Curves tab for the written channel controls.
        parameters.push((
            0x741340f126d040e2be9bc293a6f8a830,
            1,
            Value::Int(1),
            Value::Null,
            false,
            true,
        ));
    }
    let mut references = Vec::with_capacity(parameters.len());
    for (id, kind, value, control, enabled, signature) in parameters {
        references.push(Value::Ref(add(
            document,
            *b"PRIT",
            "ParameterItem",
            vec![
                ("uuid", Value::Uuid(uuid(id))),
                ("value_type", Value::Int(kind)),
                ("value", value),
                ("name", Value::Str(String::new())),
                ("enable", Value::Bool(enabled)),
                ("control_track", control),
                ("contribs_to_sig", Value::Bool(signature)),
            ],
        )?));
    }
    let params = add(
        document,
        *b"PRLS",
        "ParameterList",
        vec![("items", Value::RefList(references))],
    )?;
    for (key, value) in &mut wrapper.fields {
        match *key {
            "effect_id" => *value = Value::Str(CORRECTION.into()),
            "attributes" => *value = Value::Ref(grade_attrs),
            "param_list" => *value = Value::Ref(params),
            "keyframes" => {
                // Each TKFX owns its saved keyframe list, even for neutral controls.
                if let Some(saved) = value.as_index() {
                    *value = Value::Ref(document.duplicate_saved_object(saved, *b"FXPS")?);
                }
            }
            "tracks" => {
                let Value::List(tracks) = value else {
                    return Err(Error::new("The prepared Color Adapter track is missing", 0));
                };
                let [Value::Node(track)] = tracks.as_mut_slice() else {
                    return Err(Error::new("The prepared Color Adapter track is missing", 0));
                };
                // The filler proxy belongs to the effect track and is also
                // disposed when that track closes.
                if let Some((_, proxy)) = track
                    .fields
                    .iter_mut()
                    .find(|(key, _)| *key == "filler_proxy")
                {
                    if let Some(saved) = proxy.as_index() {
                        *proxy = Value::Ref(document.duplicate_saved_object(saved, *b"TRKR")?);
                    }
                }
            }
            _ => {}
        }
    }
    document.add_object(wrapper)
}

fn attr(key: &str, value: &str) -> (String, Value) {
    (key.into(), Value::Str(value.into()))
}

fn add(
    document: &mut AvbDocument,
    class: [u8; 4],
    type_name: &'static str,
    fields: Vec<(&'static str, Value)>,
) -> Result<u32, avid_reader::io::Error> {
    document.add_object(Node {
        class,
        type_name,
        fields,
    })
}

pub(super) fn uuid(value: u128) -> Uuid {
    Uuid {
        data1: (value >> 96) as u32,
        data2: (value >> 80) as u16,
        data3: (value >> 64) as u16,
        data4: (value as u64).to_be_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_trial_encodes_two_active_curve_points() {
        let points = curve_points(&[[VIDEO_BLACK, 0.0], [VIDEO_WHITE, FULL_WHITE]]);
        assert_eq!(points.len(), 256);
        let active: Vec<_> = points[..32]
            .chunks_exact(8)
            .map(|bytes| f64::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        assert_eq!(active, [288.0, 256.0, 726.0, 767.5]);
        assert!(points[32..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn compression_stages_apply_the_affine_range_and_black_floor() {
        let restore = VIDEO_WIDTH / (FULL_WHITE * COMPRESSION_SCALE);
        let forward = |(contrast, brightness): (f64, f64), input: f64| {
            let slope = (45.0 + 0.45 * contrast).to_radians().tan();
            let intercept =
                (1.0 - slope) * (VIDEO_BLACK + 0.507 * VIDEO_WIDTH) + brightness * (140.0 / 30.0);
            slope * input + intercept
        };
        let compress = hsl_controls(COMPRESSION_SCALE, 0.0);
        let expand = hsl_controls(restore, VIDEO_BLACK - restore * COMPRESSION_LIFT);
        for input in [-800.0f64, 0.0, 64.0, 940.0, FULL_WHITE, 1970.0] {
            let reserved = forward(compress, input).max(0.0);
            assert!(reserved + COMPRESSION_LIFT < FULL_WHITE);
            let actual = forward(expand, reserved + COMPRESSION_LIFT);
            let expected = VIDEO_BLACK + input.max(0.0) * VIDEO_WIDTH / FULL_WHITE;
            assert!((actual - expected).abs() < 1e-9);
        }
    }

    #[test]
    fn cdl_curve_keeps_ordered_points_at_both_video_rails() {
        let points = cdl_curve_points(1.12, -0.04, 1.0);
        assert_eq!(points.len(), 8);
        assert_eq!(points[0], [0.0, VIDEO_BLACK]);
        assert_eq!(points[7], [FULL_WHITE, VIDEO_WHITE]);
        assert!(points
            .windows(2)
            .all(|p| p[0][0] < p[1][0] && p[0][1] <= p[1][1]));
        assert_eq!(points[1][1], VIDEO_BLACK);
        assert!(points[3][1] > VIDEO_BLACK);
        assert!(points[4][1] < VIDEO_WHITE);
        assert_eq!(points[6][1], VIDEO_WHITE);
    }

    #[test]
    fn saved_cdl_requires_the_applied_affine_payload() {
        let xml = concat!(
            "<ColorTransformationList><ColorTransformation><ColorDecision>",
            "<BitDepth>10</BitDepth><SOPNode><Slope>1.18 0.87 1.07 </Slope>",
            "<Offset>-0.02 0.03 0.01 </Offset><Power>1</Power></SOPNode>",
            "<Black>64</Black><White>940</White><Clamp>true</Clamp>",
            "</ColorDecision></ColorTransformation></ColorTransformationList>",
        );
        let parsed = SourceClampedCdl::from_saved_xml(xml).unwrap();
        assert_eq!(parsed.slope, [1.18, 0.87, 1.07]);
        assert_eq!(parsed.offset, [-0.02, 0.03, 0.01]);
        assert_eq!(parsed.power, [1.0; 3]);
        assert!(parsed.is_affine());
        let per_channel = xml.replace("<Power>1", "<Power>0.84 1.11 1.23 ");
        let scalar = xml.replace("<Power>1", "<Power>1.20");
        assert_eq!(
            SourceClampedCdl::from_saved_xml(&per_channel)
                .unwrap()
                .power,
            [0.84, 1.11, 1.23]
        );
        assert_eq!(
            SourceClampedCdl::from_saved_xml(&scalar).unwrap().power,
            [1.2; 3]
        );
        assert!(Transform::from_saved_xml(xml).unwrap().is_approximation());
        for (before, after) in [
            ("<Power>1</Power>", "<Power>0 1.0 1.1</Power>"),
            ("<Power>1</Power>", "<Power>0.8 1.0</Power>"),
            ("<Clamp>true</Clamp>", "<Clamp>false</Clamp>"),
            ("<White>940</White>", "<White>939</White>"),
            ("<Slope>1.18", "<Slope>NaN"),
            ("1.18 0.87 1.07 ", "1.18 0.87 "),
            ("<SOPNode>", "<SOPNode><Slope>1 1 1</Slope>"),
            (
                "</SOPNode>",
                "</SOPNode><SatNode><Saturation>0.75</Saturation></SatNode>",
            ),
        ] {
            assert!(SourceClampedCdl::from_saved_xml(&xml.replace(before, after)).is_none());
        }
        assert!(Transform::from_saved_xml(&xml.replace("<Power>1", "<Power>2")).is_none());
        assert!(Transform::from_saved_xml(&xml.replace("1.18", "4.0")).is_none());
        assert!(Transform::from_saved_xml("(1.18 0.87 1.07) (-0.02 0.03 0.01) (1 1 1)").is_none());
    }

    #[test]
    fn cdl_power_sampling_stays_within_the_native_point_budget() {
        for power in [0.75, 1.5] {
            let points = cdl_curve_points(1.12, -0.04, power);
            assert_eq!(points.len(), 16);
            assert_eq!(points[0], [0.0, VIDEO_BLACK]);
            assert_eq!(points[15], [FULL_WHITE, VIDEO_WHITE]);
            assert!(points
                .windows(2)
                .all(|p| p[0][0] < p[1][0] && p[0][1] <= p[1][1]));
            let [x, y] = points[8];
            let normalized = (1.12 * (x - VIDEO_BLACK) / VIDEO_WIDTH - 0.04).clamp(0.0, 1.0);
            assert!((y - (VIDEO_BLACK + VIDEO_WIDTH * normalized.powf(power))).abs() < 1e-9);
        }
    }
}
