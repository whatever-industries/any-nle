//! Editable Fusion operations on a linked picture edit.
use crate::{
    binary::{qt, Qt},
    Result,
};
use flate2::{write::ZlibEncoder, Compression};
use graph::picture_chain::{corner_pin_geometry, CORNER_PIN_KIND, KIND};
use mapping::picture_chain::{is_corner_pin_stage, read_ordered_corner_pin};
use prproj::convert::prepared::CornerPin;
use std::{fmt::Write as _, io::Write as _, path::Path};

pub(crate) const ADAPTER: &str = "resolve.clip_fusion";
pub(crate) const LOSS: &str = "Resolve clip Fusion composition requires a destination mapping";
pub(crate) const APPROXIMATION: &str = "color_adjust_approximation";

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Adjust {
    pub(crate) exposure: f64,
    pub(crate) contrast: f64,
    pub(crate) pivot: f64,
}

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Adjust(Adjust),
    CornerPin(CornerPin, Option<Flop>),
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Flop {
    BeforePin,
    AfterPin,
}

pub(crate) fn prepare_pin(effect: &graph::Effect) -> Result<Option<Operation>> {
    if effect.kind == CORNER_PIN_KIND {
        return Ok(Some(Operation::CornerPin(read_pin(&effect.params)?, None)));
    }
    if effect.kind != KIND {
        return Ok(None);
    }
    let Some(graph::Value::Arr(stages)) = effect.params.get("stages") else {
        return Ok(None);
    };
    if !stages.iter().any(is_corner_pin_stage) {
        return Ok(None);
    }
    let unsupported =
        "Resolve ordered Corner Pin requires one static pin and one horizontal reflection";
    let chain =
        read_ordered_corner_pin(stages, &effect.params)?.ok_or(unsupported)?;
    let pin = read_pin(chain.pin_params)?;
    if !chain
        .reflection
        .is_some_and(|axes| axes.horizontal && !axes.vertical)
    {
        return Err(unsupported.into());
    }
    let flop = if chain.reflection_before_pin {
        Flop::BeforePin
    } else {
        Flop::AfterPin
    };
    Ok(Some(Operation::CornerPin(pin, Some(flop))))
}

fn read_pin(params: &graph::Value) -> Result<CornerPin> {
    let (corners, source_crop) =
        corner_pin_geometry(params).ok_or("malformed Corner Pin coordinates")?;
    let pin = CornerPin {
        corners,
        source_crop,
    };
    if !pin.is_valid() {
        return Err("invalid Corner Pin geometry".into());
    }
    Ok(pin)
}

pub(crate) fn supports_media(media: &crate::linked::Media) -> bool {
    matches!(media.profile, crate::linked::Profile::Mov {
        video: crate::linked::MovieVideo::ProResHq, raster: (1920, 1080),
        aac: false, pcm: None, tagged_rec709: true, frames, ..
    } if i32::try_from(frames).is_ok())
        && media.native.is_none()
        && media.proxy.is_none()
        && supports_path(&media.path)
}

fn supports_path(path: &Path) -> bool {
    path.to_str().is_some_and(|path| {
        path.len() <= 4096 && !path.chars().any(|c| c.is_control() || c == '\\')
    })
}

pub(crate) fn encode_edit(
    operation: Operation,
    media: &crate::linked::Media,
    media_id: &str,
    source_in: i64,
    length: i64,
) -> Result<(String, Vec<u8>)> {
    let crate::linked::Profile::Mov { frames, .. } = media.profile else {
        return Err("Fusion composition requires verified MOV picture".into());
    };
    encode(operation, &media.path, media_id, source_in, length, frames)
}

fn compressed(raw: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw)?;
    Ok(encoder.finish()?)
}

fn encode(
    operation: Operation,
    path: &Path,
    media_id: &str,
    source_in: i64,
    length: i64,
    frames: i64,
) -> Result<(String, Vec<u8>)> {
    if !supports_path(path)
        || source_in < 0
        || length <= 0
        || source_in.checked_add(length).is_none_or(|end| end > frames)
        || i32::try_from(frames).is_err()
    {
        return Err("unqualified Fusion composition file or source extent".into());
    }
    let path = serde_json::to_string(path.to_str().ok_or("Fusion media path must be UTF-8")?)?;
    // Resolve opens embedded compositions at frame 1. Store the same
    // origin and inclusive source extent in every saved time field so
    // revisiting Fusion after a DRP reopen keeps the source frame fixed.
    let first = 1 - i32::try_from(source_in)?;
    let last = frames - source_in;
    let end = length;
    let unsigned_first = first as u32;
    let (effect_tools, output, tool_count) = match operation {
        Operation::Adjust(adjust) => (
            format!(
                concat!(
                    "CustomTool1 = Custom {{ Inputs = {{ {}",
                    "Image1 = Input {{ SourceOp = \"MediaIn1\", Source = \"Output\", }}, }}, }}, "
                ),
                adjust_inputs(adjust)?
            ),
            "CustomTool1",
            3,
        ),
        Operation::CornerPin(pin, flop) => pin_tools(pin, flop)?,
    };
    let tools = format!(concat!(
        "{{ MediaIn1 = MediaIn {{ ExtentSet = true, CustomData = {{ MediaProps = {{ ",
        "MEDIA_FORMAT_TYPE = \"QuickTime\", MEDIA_WIDTH = 1920, MEDIA_HEIGHT = 1080, ",
        "MEDIA_ID = \"{id}\", MEDIA_IS_SOURCE_RES = true, MEDIA_MARK_IN = 1, MEDIA_MARK_OUT = {end}, ",
        "MEDIA_NAME = \"Linked picture\", MEDIA_NUM_FRAMES = {frames}, MEDIA_NUM_LAYERS = 1, ",
        "MEDIA_PAR = 1, MEDIA_PATH = {path}, MEDIA_SRC_FRAME_RATE = 24, MEDIA_START_FRAME = {first} }}, }}, ",
        "Inputs = {{ GlobalIn = Input {{ Value = {first}, }}, GlobalOut = Input {{ Value = {last}, }}, ",
        "Layer = Input {{ Value = \"0\", }}, ClipTimeStart = Input {{ Value = {first}, }}, ",
        "ClipTimeEnd = Input {{ Value = {last}, }}, }}, Version = 1 }}, ",
        "{effect_tools}",
        "MediaOut1 = MediaOut {{ Inputs = {{ Index = Input {{ Value = \"0\", }}, ",
        "Input = Input {{ SourceOp = \"{output}\", Source = \"Output\", }}, }}, }} }}\0"
    ), id=media_id, end=end, frames=frames, path=path, first=first,
        last=last, effect_tools=effect_tools, output=output);
    let header = format!(concat!(
        "Composition {{ CurrentTime = 1, RenderRange = {{ 1, {end} }}, GlobalRange = {{ {first}, {last} }}, ",
        "CurrentID = {tool_count}, HiQ = true, Frames = {{ }}, Prefs = {{ Comp = {{ FrameFormat = {{ Rate = 24, }}, ",
        "Unsorted = {{ GlobalStart = {unsigned_first}, GlobalEnd = {last} }}, }} }}, Compressed = true, }}\0"
    ), end=end, first=first, last=last, unsigned_first=unsigned_first, tool_count=tool_count);
    let mut body = header.into_bytes();
    body.extend(u32::try_from(tools.len())?.to_le_bytes());
    body.extend(compressed(tools.as_bytes())?);
    let outer = qt(&[
        ("count", Qt::Int(1)),
        ("0_name", Qt::Text("Composition 1")),
        ("0_mediaIdList", Qt::EmptyStringList),
        ("0_data", Qt::Bytes(&body)),
    ]);
    let mut payload = u32::try_from(outer.len())?.to_be_bytes().to_vec();
    payload.extend(compressed(&outer)?);
    let fields = qt(&[("FusionStartFrameOffset", Qt::Int(first))]);
    Ok((hex::encode(fields), payload))
}

fn adjust_inputs(adjust: Adjust) -> Result<String> {
    let mut controls = String::new();
    for (i, (name, value)) in [
        ("Exposure", adjust.exposure),
        ("Contrast", adjust.contrast),
        ("Pivot", adjust.pivot),
    ]
    .into_iter()
    .enumerate()
    {
        let slot = i + 1;
        write!(controls, "NameforNumber{slot} = Input {{ Value = \"{name}\", }}, NumberIn{slot} = Input {{ Value = {value}, }}, ")?;
    }
    // Premiere's measured Rec.709 response, with each channel using the
    // same luminance multiplier and the input alpha left intact.
    for (name, channel) in [("Red", 'r'), ("Green", 'g'), ("Blue", 'b')] {
        write!(
            controls,
            "{name}Expression = Input {{ Value = \"{channel}1 * 2^(n1/2) * \
             (max(2^n1 * (0.2126*r1*r1+0.7152*g1*g1+0.0722*b1*b1),0.00000001)/(0.1786*2^n3))\
             ^((if(n2>1, 1+3*(n2-1), n2)-1)/2)\", }}, "
        )?;
    }
    controls.push_str(" AlphaExpression = Input { Value = \"a1\", }, ");
    Ok(controls)
}

fn pin_inputs(pin: CornerPin) -> Result<String> {
    if !pin.is_valid() {
        return Err("invalid Fusion Corner Pin geometry".into());
    }
    let mut inputs = String::from("MappingType = Input { Value = 1, }, ");
    for (name, [x, y]) in ["TopLeft", "TopRight", "BottomRight", "BottomLeft"]
        .into_iter()
        .zip(pin.corners)
    {
        // Fusion uses normalized Y from the bottom edge.
        write!(
            inputs,
            "{name} = Input {{ Value = {{ {x}, {} }}, }}, ",
            1.0 - y
        )?;
    }
    Ok(inputs)
}

fn pin_tools(pin: CornerPin, flop: Option<Flop>) -> Result<(String, &'static str, u8)> {
    let mut nodes = String::new();
    let mut upstream = "MediaIn1";
    let mut count = 3;
    if flop == Some(Flop::BeforePin) {
        write_flop(&mut nodes, upstream)?;
        upstream = "Transform1";
    }
    if pin.source_crop != [0.0; 4] {
        nodes.push_str(&crop_tools(pin.source_crop, upstream));
        upstream = "MatteControl1";
        count += 2;
    }
    write!(
        nodes,
        concat!(
            "CornerPositioner1 = CornerPositioner {{ Inputs = {{ {}",
            "Input = Input {{ SourceOp = \"{}\", Source = \"Output\", }}, }}, }}, "
        ),
        pin_inputs(pin)?,
        upstream
    )?;
    let mut output = "CornerPositioner1";
    if flop == Some(Flop::AfterPin) {
        write_flop(&mut nodes, output)?;
        output = "Transform1";
    }
    Ok((nodes, output, count + u8::from(flop.is_some())))
}

fn write_flop(nodes: &mut String, upstream: &str) -> Result<()> {
    write!(
        nodes,
        concat!(
            "Transform1 = Transform {{ Inputs = {{ FlipHoriz = Input {{ Value = 1, }}, ",
            "Input = Input {{ SourceOp = \"{}\", Source = \"Output\", }}, }}, }}, "
        ),
        upstream
    )?;
    Ok(())
}

fn crop_tools([left, top, right, bottom]: [f64; 4], upstream: &str) -> String {
    let x = (1.0 + left - right) / 2.0;
    let y = (1.0 + bottom - top) / 2.0;
    let width = 1.0 - left - right;
    let height = 1.0 - top - bottom;
    // A garbage matte removes the outside while keeping the original image
    // raster. The Corner Positioner then projects from the full source frame.
    format!(concat!(
        "Rectangle1 = RectangleMask {{ Inputs = {{ ",
        "Filter = Input {{ Value = FuID {{ \"Fast Gaussian\" }}, }}, ",
        "MaskWidth = Input {{ Value = 1920, }}, MaskHeight = Input {{ Value = 1080, }}, ",
        "PixelAspect = Input {{ Value = {{ 1, 1 }}, }}, UseFrameFormatSettings = Input {{ Value = 1, }}, ",
        "ClippingMode = Input {{ Value = FuID {{ \"None\" }}, }}, ",
        "Center = Input {{ Value = {{ {x}, {y} }}, }}, ",
        "Width = Input {{ Value = {width}, }}, Height = Input {{ Value = {height}, }}, }}, }}, ",
        "MatteControl1 = MatteControl {{ Inputs = {{ ",
        "Filter = Input {{ Value = FuID {{ \"Fast Gaussian\" }}, }}, ",
        "[\"Garbage.Matte\"] = Input {{ SourceOp = \"Rectangle1\", Source = \"Mask\", }}, ",
        "[\"Garbage.MaskInverted\"] = Input {{ Value = 1, }}, ",
        "MultiplyGarbageMatte = Input {{ Value = 1, }}, ",
        "Background = Input {{ SourceOp = \"{upstream}\", Source = \"Output\", }}, }}, }}, "
    ), x=x, y=y, width=width, height=height, upstream=upstream)
}
