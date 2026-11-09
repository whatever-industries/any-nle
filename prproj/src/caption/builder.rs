//! Synthesize the imported-SRT object graph: port of `models/caption_builder.py`.
//!
//! Every builder returns a detached element in the caller's [`Tree`], byte-identical to the
//! Python builder of the same name. UUIDs come from the one [`UuidSource`] of the write.

use super::template::{
    payload_head, payload_tail, COLLECTION_METADATA, SHIFT_DOWN, SHIFT_UP, STREAM_FORMAT, SYNTHETIC_STATE,
    TEMPLATE_PADDED, TEMPLATE_STYLE,
};
use super::{digits::DIGIT_BLOCKS, PyError, PyResult};
use crate::build::media::{basename, relpath, utf16le_b64, BMP_IMPLEMENTATION_ID, MEDIA_CLASS_ID};
use crate::build::time::TICKS_PER_SECOND;
use crate::build::{leaf, top};
use crate::convert::model::py_str_repr;
use crate::ids::UuidSource;
use crate::pystr::{is_py_space, py_strip};
use crate::xml::mutations::remove_child;
use crate::xml::{NodeId, Tree};

pub const TRANSCRIPT_CLASS_ID: &str = "9e0179bb-153c-4884-b34b-eb7082f34384";
pub const DATA_SOURCE_CLASS_ID: &str = "ff36343e-4ece-4d37-ab61-e99b758f9d30";
pub const COLLECTION_CLASS_ID: &str = "9b9f6927-2bdd-4874-bd23-1d8164bfbfcf";
pub const CAPTION_CLASS_ID: &str = "d4ca6d5e-d3fe-4cfb-af7c-c9aea9e54b66";
pub const DATA_STREAM_CLASS_ID: &str = "9e4e76eb-b72f-4b9b-9ea8-2887b1cc24fd";
pub const BLOCK_CLASS_ID: &str = "d3782b80-516f-47e3-a7e8-e83779f0ed01";

/// Caption streams count at 30 fps with timecode format 104 (29_captions).
pub const CAPTION_FRAME_RATE: i64 = 8467200000;
pub const CAPTION_TIMECODE_FORMAT: &str = "104";

/// The factory Captions label (Mango), used when no local prefs exist.
pub const CAPTIONS_LABEL_INDEX: i64 = 7;
pub const CAPTIONS_LABEL_COLOR: i64 = 277129;

const TICKS_PER_MILLISECOND: i64 = TICKS_PER_SECOND / 1000;

/// Timeline caption classes and constants (29_captions' createCaptionTrack output).
pub const CAPTION_TRACK_CLASS_ID: &str = "b9d20db2-f229-482d-87fd-10f1fa157107";
pub const CAPTION_ITEM_CLASS_ID: &str = "541ba122-fe61-4350-abe5-040ed395006e";
pub const DATA_CHAIN_CLASS_ID: &str = "1d83b349-453e-4099-801d-6b23edae1724";
pub const SUBCLIP_CLASS_ID: &str = "e0c58dc9-dbdd-4166-aef7-5db7e3f22e84";

/// The data (caption) media type GUID in a sequence's TrackGroups map.
pub const DATA_MEDIA_TYPE: &str = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5";

/// A synthetic caption's phantom source: the generator implementation (the Black Video one)
/// behind a fourcc token, spanning 12 hours.
pub const CAPTION_MEDIA_TOKEN: &str = "1396920390";
pub const GENERATOR_IMPLEMENTATION_ID: &str = "42008e7a-de6f-4270-96de-7e287abb9b4b";
pub const SYNTHETIC_DURATION: i64 = 43200 * TICKS_PER_SECOND;

/// One parsed cue: exact ticks (not `Time` objects) and the (possibly multi-line) text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrtCue {
    /// Ticks from the start of the caption stream.
    pub start: i64,
    /// Ticks from the start of the caption stream.
    pub end: i64,
    pub text: String,
}

// ---- SRT parsing -----------------------------------------------------------------------------------

/// The decimal value of `c` if a str regex `\d` matches it (Unicode category Nd).
fn decimal(c: char) -> Option<u32> {
    let cp = c as u32;
    let i = DIGIT_BLOCKS.partition_point(|&s| s <= cp);
    if i == 0 {
        return None;
    }
    let value = cp - DIGIT_BLOCKS[i - 1];
    (value < 10).then_some(value)
}

/// Scan `\d{min,}` greedy at `*at`; the value as `i128` (`None` when it does not fit).
fn digits(line: &[char], at: &mut usize, count: Option<usize>) -> Option<Option<i128>> {
    let start = *at;
    let mut value: Option<i128> = Some(0);
    let mut end = start;
    while end < line.len() && decimal(line[end]).is_some() {
        if count.is_some_and(|c| end - start == c) {
            break;
        }
        let d = decimal(line[end]).expect("digit") as i128;
        value = value.and_then(|v| v.checked_mul(10)).and_then(|v| v.checked_add(d));
        end += 1;
    }
    if end == start || count.is_some_and(|c| end - start != c) {
        return None;
    }
    *at = end;
    Some(value)
}

fn expect(line: &[char], at: &mut usize, ok: impl Fn(char) -> bool) -> bool {
    if *at < line.len() && ok(line[*at]) {
        *at += 1;
        true
    } else {
        false
    }
}

fn skip_space(line: &[char], at: &mut usize) {
    while *at < line.len() && is_py_space(line[*at]) {
        *at += 1;
    }
}

/// `_CUE_TIMES.search(line)`: the eight fields (`None` inside when a field overflows `i128`).
fn match_cue_at(line: &[char], start: usize) -> Option<[Option<i128>; 8]> {
    let mut at = start;
    let mut fields = [Some(0i128); 8];
    for half in 0..2 {
        fields[half * 4] = digits(line, &mut at, None)?;
        if !expect(line, &mut at, |c| c == ':') {
            return None;
        }
        fields[half * 4 + 1] = digits(line, &mut at, Some(2))?;
        if !expect(line, &mut at, |c| c == ':') {
            return None;
        }
        fields[half * 4 + 2] = digits(line, &mut at, Some(2))?;
        if !expect(line, &mut at, |c| c == ',' || c == '.') {
            return None;
        }
        fields[half * 4 + 3] = digits(line, &mut at, Some(3))?;
        if half == 0 {
            skip_space(line, &mut at);
            for arrow in ['-', '-', '>'] {
                if !expect(line, &mut at, |c| c == arrow) {
                    return None;
                }
            }
            skip_space(line, &mut at);
        }
    }
    Some(fields)
}

fn match_cue(line: &[char]) -> Option<[Option<i128>; 8]> {
    (0..=line.len()).find_map(|start| match_cue_at(line, start))
}

/// `re.split(r"\n\s*\n", text)`.
fn split_blocks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut piece_start = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' {
            let mut run_end = i + 1;
            while run_end < chars.len() && is_py_space(chars[run_end]) {
                run_end += 1;
            }
            // `\s*` is greedy: back off to the last newline of the whitespace run.
            if let Some(last) = (i + 1..run_end).rev().find(|&e| chars[e] == '\n') {
                pieces.push(chars[piece_start..i].iter().collect());
                piece_start = last + 1;
                i = last + 1;
                continue;
            }
        }
        i += 1;
    }
    pieces.push(chars[piece_start..].iter().collect());
    pieces
}

/// `parse_srt`: SRT cues; index lines are optional, times become exact ticks.
pub fn parse_srt(text: &str) -> PyResult<Vec<SrtCue>> {
    let mut cues = Vec::new();
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for block in split_blocks(py_strip(&normalized)) {
        let lines: Vec<&str> = block.split('\n').collect();
        let mut found = None;
        for (index, line) in lines.iter().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            if let Some(fields) = match_cue(&chars) {
                found = Some((index, fields));
                break;
            }
        }
        let Some((index, fields)) = found else { continue };
        let too_big = || PyError::Value("cue time is too large to express in ticks".to_string());
        let mut f = [0i128; 8];
        for (slot, field) in f.iter_mut().zip(fields) {
            *slot = field.ok_or_else(too_big)?;
        }
        let ticks = |h: i128, m: i128, s: i128, ms: i128| -> Option<i128> {
            h.checked_mul(3600)?
                .checked_add(m * 60)?
                .checked_add(s)?
                .checked_mul(1000)?
                .checked_add(ms)?
                .checked_mul(TICKS_PER_MILLISECOND as i128)
        };
        let start = ticks(f[0], f[1], f[2], f[3]);
        let end = ticks(f[4], f[5], f[6], f[7]);
        let body = lines[index + 1..].join("\n");
        let body = body.trim_matches('\n');
        if body.is_empty() {
            continue;
        }
        let (Some(start), Some(end)) = (start, end) else { return Err(too_big()) };
        if end <= start {
            return Err(PyError::Value(format!(
                "cue ends before it starts: {}",
                py_str_repr(py_strip(lines[index]))
            )));
        }
        let (Ok(start), Ok(end)) = (i64::try_from(start), i64::try_from(end)) else { return Err(too_big()) };
        cues.push(SrtCue { start, end, text: body.to_string() });
    }
    if cues.is_empty() {
        return Err(PyError::Value("no cues found in the SRT file".to_string()));
    }
    Ok(cues)
}

// ---- payload -----------------------------------------------------------------------------------------

/// `build_caption_payload`: the `FormattedTextData` FlatBuffer for `text` (a patch of the
/// fixture template).
pub fn build_caption_payload(text: &str) -> Vec<u8> {
    let encoded = text.as_bytes();
    let padded = ((encoded.len() + 1).div_ceil(4) * 4) as i64;
    let delta = padded - TEMPLATE_PADDED;
    let mut head = payload_head().to_vec();
    for position in SHIFT_UP {
        let value = u32::from_le_bytes(head[position..position + 4].try_into().expect("4 bytes")) as i64;
        head[position..position + 4].copy_from_slice(&(((value + delta) & 0xFFFF_FFFF) as u32).to_le_bytes());
    }
    for position in SHIFT_DOWN {
        let value = u32::from_le_bytes(head[position..position + 4].try_into().expect("4 bytes")) as i64;
        head[position..position + 4].copy_from_slice(&(((value - delta) & 0xFFFF_FFFF) as u32).to_le_bytes());
    }
    let mut out = head;
    out.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
    out.extend_from_slice(encoded);
    out.resize(out.len() + (padded as usize - encoded.len()), 0);
    out.extend_from_slice(payload_tail());
    out
}

// ---- element helpers ----------------------------------------------------------------------------------

fn top_uid(tree: &mut Tree, tag: &str, class_id: &str, version: &str, uid: &str) -> NodeId {
    top(tree, tag, class_id, version, if uid.is_empty() { None } else { Some(uid) })
}

/// `SubElement(parent, tag, attrs)` with text and tail.
fn sub(tree: &mut Tree, parent: NodeId, tag: &str, attrs: &[(&str, &str)], text: Option<&str>, tail: &str) -> NodeId {
    let e = tree.sub_element(parent, tag, attrs);
    tree.set_text(e, text);
    tree.set_tail(e, Some(tail));
    e
}

// ---- builders ------------------------------------------------------------------------------------------

/// `new_block`: a panel `Block` holding the styled-text payload (base64).
pub fn new_block(tree: &mut Tree, payload_b64: &str, binary_hash: &str) -> NodeId {
    let element = top(tree, "Block", BLOCK_CLASS_ID, "1", None);
    let data = tree.sub_element(element, "FormattedTextData", &[("Encoding", "base64"), ("BinaryHash", binary_hash)]);
    tree.set_text(data, Some(format!("{payload_b64}\n\t\t")));
    tree.set_tail(data, Some("\n\t"));
    element
}

/// `new_caption`: one cue of the imported stream (ticks).
pub fn new_caption(tree: &mut Tree, block_id: &str, start_ticks: i64, end_ticks: i64) -> NodeId {
    let element = top(tree, "Caption", CAPTION_CLASS_ID, "1", None);
    let vector = sub(tree, element, "BlockVector", &[("Version", "1")], Some("\n\t\t\t"), "\n\t\t");
    let entry = tree.sub_element(vector, "BlockVectorItem", &[("Index", "0"), ("ObjectRef", block_id)]);
    tree.set_tail(entry, Some("\n\t\t"));
    leaf(tree, element, "TimeStart", &start_ticks.to_string(), "\n\t\t");
    leaf(tree, element, "TimeEnd", &end_ticks.to_string(), "\n\t");
    element
}

/// `new_caption_collection`: the cue map, `(end ticks, caption id)` pairs keyed by cue END.
pub fn new_caption_collection(tree: &mut Tree, entries: &[(i64, &str)]) -> NodeId {
    let element = top(tree, "CaptionCollection", COLLECTION_CLASS_ID, "1", None);
    let caption_map = sub(tree, element, "CaptionMap", &[("Version", "1")], Some("\n\t\t\t"), "\n\t\t");
    for (index, (end_ticks, caption_id)) in entries.iter().enumerate() {
        let i = index.to_string();
        let tail = if index + 1 < entries.len() { "\n\t\t\t" } else { "\n\t\t" };
        let item = sub(tree, caption_map, "CaptionMapItem", &[("Version", "1"), ("Index", &i)], Some("\n\t\t\t\t"), tail);
        leaf(tree, item, "First", &end_ticks.to_string(), "\n\t\t\t\t");
        let second = tree.sub_element(item, "Second", &[("ObjectRef", caption_id)]);
        tree.set_tail(second, Some("\n\t\t\t"));
    }
    leaf(tree, element, "Metadata", COLLECTION_METADATA, "\n\t\t");
    leaf(tree, element, "StreamFormat", STREAM_FORMAT, "\n\t");
    element
}

/// `new_data_media`: the movie Media shape with a DataStream reference and none of the
/// audio/timecode extras (29_captions). Draws the ModificationState hash, then the FileKey.
pub fn new_data_media(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    media_uid: &str,
    stream_id: &str,
    file_path: &str,
    project_dir: &str,
    content_state: &str,
) -> NodeId {
    let element = top_uid(tree, "Media", MEDIA_CLASS_ID, "30", media_uid);
    let stream = tree.sub_element(element, "DataStream", &[("ObjectRef", stream_id)]);
    tree.set_tail(stream, Some("\n\t\t"));
    let blob = utf16le_b64(content_state);
    let hash = ids.uuid4().to_string();
    sub(
        tree,
        element,
        "ModificationState",
        &[("Encoding", "base64"), ("BinaryHash", &hash)],
        Some(&format!("{blob}\n\t\t")),
        "\n\t\t",
    );
    leaf(tree, element, "RelativePath", &relpath(file_path, project_dir), "\n\t\t");
    leaf(tree, element, "FilePath", file_path, "\n\t\t");
    leaf(tree, element, "ImplementationID", BMP_IMPLEMENTATION_ID, "\n\t\t");
    leaf(tree, element, "Title", basename(file_path), "\n\t\t");
    leaf(tree, element, "FileKey", &ids.uuid4().to_string(), "\n\t\t");
    leaf(tree, element, "ContentAndMetadataState", content_state, "\n\t\t");
    leaf(tree, element, "ActualMediaFilePath", file_path, "\n\t");
    element
}

/// `new_data_stream`.
pub fn new_data_stream(tree: &mut Tree, duration_ticks: i64) -> NodeId {
    let element = top(tree, "DataStream", DATA_STREAM_CLASS_ID, "1", None);
    leaf(tree, element, "FrameRate", &CAPTION_FRAME_RATE.to_string(), "\n\t\t");
    leaf(tree, element, "Duration", &duration_ticks.to_string(), "\n\t");
    element
}

/// `new_data_media_source`.
pub fn new_data_media_source(tree: &mut Tree, media_uid: &str, duration_ticks: i64) -> NodeId {
    let element = top(tree, "DataMediaSource", DATA_SOURCE_CLASS_ID, "1", None);
    let source = sub(tree, element, "MediaSource", &[("Version", "4")], Some("\n\t\t\t"), "\n\t\t");
    sub(tree, source, "Content", &[("Version", "10")], Some("\n\t\t\t"), "\n\t\t\t");
    let media = tree.sub_element(source, "Media", &[("ObjectURef", media_uid)]);
    tree.set_tail(media, Some("\n\t\t"));
    leaf(tree, element, "OriginalDuration", &duration_ticks.to_string(), "\n\t");
    element
}

/// `new_empty_logging_info`: a synthetic caption's master carries a logging info with no fields.
pub fn new_empty_logging_info(tree: &mut Tree) -> NodeId {
    let element = top(tree, "ClipLoggingInfo", crate::build::media::LOGGING_CLASS_ID, "10", None);
    tree.set_text(element, Some("\n\t"));
    element
}

/// `new_synthetic_stream`.
pub fn new_synthetic_stream(tree: &mut Tree, timebase: i64) -> NodeId {
    let element = top(tree, "DataStream", DATA_STREAM_CLASS_ID, "1", None);
    leaf(tree, element, "FrameRate", &timebase.to_string(), "\n\t\t");
    leaf(tree, element, "Duration", &SYNTHETIC_DURATION.to_string(), "\n\t");
    element
}

/// `new_synthetic_media`: `state_stored` is whether the constant state blob already sits in the
/// document (only the first occurrence carries it).
pub fn new_synthetic_media(tree: &mut Tree, media_uid: &str, stream_id: &str, state_stored: bool) -> NodeId {
    let element = top_uid(tree, "Media", MEDIA_CLASS_ID, "30", media_uid);
    let stream = tree.sub_element(element, "DataStream", &[("ObjectRef", stream_id)]);
    tree.set_tail(stream, Some("\n\t\t"));
    let state = tree.sub_element(element, "ModificationState", &[("Encoding", "base64"), ("BinaryHash", SYNTHETIC_STATE.0)]);
    if !state_stored {
        tree.set_text(state, Some(format!("{}\n\t\t", SYNTHETIC_STATE.1)));
    }
    tree.set_tail(state, Some("\n\t\t"));
    leaf(tree, element, "FilePath", CAPTION_MEDIA_TOKEN, "\n\t\t");
    leaf(tree, element, "ImplementationID", GENERATOR_IMPLEMENTATION_ID, "\n\t\t");
    leaf(tree, element, "Title", "SyntheticCaption", "\n\t\t");
    leaf(tree, element, "Infinite", "true", "\n\t\t");
    leaf(tree, element, "ActualMediaFilePath", CAPTION_MEDIA_TOKEN, "\n\t");
    element
}

/// `new_synthetic_template_clip`: the synthetic master's own template clip.
pub fn new_synthetic_template_clip(tree: &mut Tree, source_id: &str, clip_id: &str) -> NodeId {
    let element = top(tree, "TranscriptClip", TRANSCRIPT_CLASS_ID, "2", None);
    let data_clip = sub(tree, element, "DataClip", &[("Version", "1")], Some("\n\t\t\t"), "\n\t");
    let core = sub(tree, data_clip, "Clip", &[("Version", "18")], Some("\n\t\t\t\t"), "\n\t\t");
    let source = tree.sub_element(core, "Source", &[("ObjectRef", source_id)]);
    tree.set_tail(source, Some("\n\t\t\t\t"));
    leaf(tree, core, "ClipID", clip_id, "\n\t\t\t\t");
    leaf(tree, core, "InUse", "false", "\n\t\t\t");
    element
}

/// The label `Node/Properties` block shared by the two labelled transcript clips.
fn label_node(tree: &mut Tree, core: NodeId, label_index: i64, label_color: i64) {
    let node = sub(tree, core, "Node", &[("Version", "1")], Some("\n\t\t\t\t\t"), "\n\t\t\t\t");
    let properties = sub(tree, node, "Properties", &[("Version", "1")], Some("\n\t\t\t\t\t\t"), "\n\t\t\t\t");
    leaf(tree, properties, "asl.clip.label.color", &label_color.to_string(), "\n\t\t\t\t\t\t");
    leaf(tree, properties, "asl.clip.label.name", &format!("BE.Prefs.LabelColors.{label_index}"), "\n\t\t\t\t\t");
}

/// `new_timeline_caption_clip`: the placed instance, labelled, with in/out inside the synthetic
/// source's 12-hour span.
pub fn new_timeline_caption_clip(
    tree: &mut Tree,
    source_id: &str,
    in_ticks: i64,
    out_ticks: i64,
    clip_id: &str,
    label_index: i64,
    label_color: i64,
) -> NodeId {
    let element = top(tree, "TranscriptClip", TRANSCRIPT_CLASS_ID, "2", None);
    let data_clip = sub(tree, element, "DataClip", &[("Version", "1")], Some("\n\t\t\t"), "\n\t");
    let core = sub(tree, data_clip, "Clip", &[("Version", "18")], Some("\n\t\t\t\t"), "\n\t\t");
    label_node(tree, core, label_index, label_color);
    let source = tree.sub_element(core, "Source", &[("ObjectRef", source_id)]);
    tree.set_tail(source, Some("\n\t\t\t\t"));
    leaf(tree, core, "OutPoint", &out_ticks.to_string(), "\n\t\t\t\t");
    leaf(tree, core, "InPoint", &in_ticks.to_string(), "\n\t\t\t\t");
    leaf(tree, core, "ClipID", clip_id, "\n\t\t\t");
    element
}

/// `new_caption_subclip`.
pub fn new_caption_subclip(tree: &mut Tree, clip_ref: &str, master_uid: &str) -> NodeId {
    let element = top(tree, "SubClip", SUBCLIP_CLASS_ID, "6", None);
    let clip = tree.sub_element(element, "Clip", &[("ObjectRef", clip_ref)]);
    tree.set_tail(clip, Some("\n\t\t"));
    let master = tree.sub_element(element, "MasterClip", &[("ObjectURef", master_uid)]);
    tree.set_tail(master, Some("\n\t\t"));
    leaf(tree, element, "Name", "SyntheticCaption", "\n\t\t");
    leaf(tree, element, "OrigChGrp", "0", "\n\t");
    element
}

/// `new_data_chain`.
pub fn new_data_chain(tree: &mut Tree) -> NodeId {
    let element = top(tree, "DataComponentChain", DATA_CHAIN_CLASS_ID, "1", None);
    sub(tree, element, "ComponentChain", &[("Version", "3")], Some("\n\t\t"), "\n\t");
    element
}

/// `new_timeline_block`: the timeline copy hash-references the panel block's payload.
pub fn new_timeline_block(tree: &mut Tree, binary_hash: &str) -> NodeId {
    let element = top(tree, "Block", BLOCK_CLASS_ID, "1", None);
    let data = tree.sub_element(element, "FormattedTextData", &[("Encoding", "base64"), ("BinaryHash", binary_hash)]);
    tree.set_tail(data, Some("\n\t"));
    element
}

/// `new_caption_track_item`: a `CaptionDataClipTrackItem` (ticks on the sequence timeline).
pub fn new_caption_track_item(
    tree: &mut Tree,
    chain_id: &str,
    subclip_id: &str,
    block_id: &str,
    start_ticks: i64,
    end_ticks: i64,
) -> NodeId {
    let element = top(tree, "CaptionDataClipTrackItem", CAPTION_ITEM_CLASS_ID, "3", None);
    let data_item = sub(tree, element, "DataClipTrackItem", &[("Version", "1")], Some("\n\t\t\t"), "\n\t\t");
    let clip_item = sub(tree, data_item, "ClipTrackItem", &[("Version", "8")], Some("\n\t\t\t\t"), "\n\t\t");
    let owner = sub(tree, clip_item, "ComponentOwner", &[("Version", "1")], Some("\n\t\t\t\t\t"), "\n\t\t\t\t");
    let components = tree.sub_element(owner, "Components", &[("ObjectRef", chain_id)]);
    tree.set_tail(components, Some("\n\t\t\t\t"));
    let track_item = sub(tree, clip_item, "TrackItem", &[("Version", "4")], Some("\n\t\t\t\t\t"), "\n\t\t\t\t");
    if start_ticks != 0 {
        leaf(tree, track_item, "Start", &start_ticks.to_string(), "\n\t\t\t\t\t");
    }
    let end = leaf(tree, track_item, "End", &end_ticks.to_string(), "\n\t\t\t\t\t");
    // Fix the last child's tail to close TrackItem at its own depth.
    tree.set_tail(end, Some("\n\t\t\t\t"));
    let subclip = tree.sub_element(clip_item, "SubClip", &[("ObjectRef", subclip_id)]);
    tree.set_tail(subclip, Some("\n\t\t\t"));
    let vector = sub(tree, element, "BlockVector", &[("Version", "1")], Some("\n\t\t\t"), "\n\t");
    let entry = tree.sub_element(vector, "BlockVectorItem", &[("Index", "0"), ("ObjectRef", block_id)]);
    tree.set_tail(entry, Some("\n\t\t"));
    element
}

/// `new_caption_track`: a `CaptionDataClipTrack`. `style_stored` is whether the default style
/// template payload already sits in the document.
pub fn new_caption_track(
    tree: &mut Tree,
    track_uid: &str,
    track_id: i64,
    item_ids: &[&str],
    style_stored: bool,
    caption_format: i64,
) -> PyResult<NodeId> {
    let element = top_uid(tree, "CaptionDataClipTrack", CAPTION_TRACK_CLASS_ID, "1", track_uid);
    let data_track = sub(tree, element, "DataClipTrack", &[("Version", "1")], Some("\n\t\t\t"), "\n\t\t");
    let clip_track = sub(tree, data_track, "ClipTrack", &[("Version", "2")], Some("\n\t\t\t\t"), "\n\t\t");
    let track = sub(tree, clip_track, "Track", &[("Version", "4")], Some("\n\t\t\t\t\t"), "\n\t\t\t\t");
    leaf(tree, track, "ID", &track_id.to_string(), "\n\t\t\t\t\t");
    leaf(tree, track, "MediaType", DATA_MEDIA_TYPE, "\n\t\t\t\t\t");
    leaf(tree, track, "Index", "0", "\n\t\t\t\t");
    let clip_items = sub(tree, clip_track, "ClipItems", &[("Version", "3")], Some("\n\t\t\t\t\t"), "\n\t\t\t\t");
    let track_items = sub(tree, clip_items, "TrackItems", &[("Version", "1")], Some("\n\t\t\t\t\t\t"), "\n\t\t\t\t\t");
    for (index, item_id) in item_ids.iter().enumerate() {
        let i = index.to_string();
        let entry = tree.sub_element(track_items, "TrackItem", &[("Index", &i), ("ObjectRef", item_id)]);
        let tail = if index + 1 < item_ids.len() { "\n\t\t\t\t\t\t" } else { "\n\t\t\t\t\t" };
        tree.set_tail(entry, Some(tail));
    }
    leaf(tree, clip_items, "MediaType", DATA_MEDIA_TYPE, "\n\t\t\t\t\t");
    leaf(tree, clip_items, "Index", "0", "\n\t\t\t\t");
    let transitions = sub(tree, clip_track, "TransitionItems", &[("Version", "3")], Some("\n\t\t\t\t\t"), "\n\t\t\t");
    leaf(tree, transitions, "MediaType", DATA_MEDIA_TYPE, "\n\t\t\t\t\t");
    leaf(tree, transitions, "Index", "0", "\n\t\t\t\t");
    let style = tree.sub_element(
        element,
        "CaptionDataTemplateStyle",
        &[("Encoding", "base64"), ("BinaryHash", TEMPLATE_STYLE.0)],
    );
    if !style_stored {
        tree.set_text(style, Some(format!("{}\n\t\t", TEMPLATE_STYLE.1)));
    }
    tree.set_tail(style, Some("\n\t"));
    // `Format` is the API constant's low word and `SubFormat` its high one; Subtitle (0) elides
    // both.
    write_track_format(tree, element, caption_format)?;
    Ok(element)
}

/// `write_track_format`: set (or clear) a caption track's `Format`/`SubFormat` pair.
pub fn write_track_format(tree: &mut Tree, track: NodeId, caption_format: i64) -> PyResult<()> {
    for tag in ["SubFormat", "Format"] {
        if let Some(existing) = tree.child_by_tag(track, tag) {
            remove_child(tree, track, existing)?;
        }
    }
    let last = |tree: &Tree| -> PyResult<NodeId> {
        tree.children(track).last().copied().ok_or_else(|| PyError::Index("list index out of range".into()))
    };
    let l = last(tree)?;
    tree.set_tail(l, Some("\n\t"));
    let low = caption_format & 0xFFFF;
    let high = caption_format >> 16;
    if low != 0 {
        let l = last(tree)?;
        tree.set_tail(l, Some("\n\t\t"));
        leaf(tree, track, "Format", &low.to_string(), "\n\t");
    }
    if high != 0 {
        let l = last(tree)?;
        tree.set_tail(l, Some("\n\t\t"));
        leaf(tree, track, "SubFormat", &high.to_string(), "\n\t");
    }
    Ok(())
}

/// `new_transcript_clip`: the caption item's template clip nests its core one level deeper than
/// a media clip's and adds the TranscriptTextSegments reference beside the DataClip.
pub fn new_transcript_clip(
    tree: &mut Tree,
    markers_id: &str,
    source_id: &str,
    collection_id: &str,
    label_index: i64,
    label_color: i64,
    clip_id: &str,
) -> NodeId {
    let element = top(tree, "TranscriptClip", TRANSCRIPT_CLASS_ID, "2", None);
    let data_clip = sub(tree, element, "DataClip", &[("Version", "1")], Some("\n\t\t\t"), "\n\t\t");
    let core = sub(tree, data_clip, "Clip", &[("Version", "18")], Some("\n\t\t\t\t"), "\n\t\t");
    label_node(tree, core, label_index, label_color);
    let owner = sub(tree, core, "MarkerOwner", &[("Version", "1")], Some("\n\t\t\t\t\t"), "\n\t\t\t\t");
    let markers = tree.sub_element(owner, "Markers", &[("ObjectRef", markers_id)]);
    tree.set_tail(markers, Some("\n\t\t\t\t\t"));
    let source = tree.sub_element(core, "Source", &[("ObjectRef", source_id)]);
    tree.set_tail(source, Some("\n\t\t\t\t"));
    leaf(tree, core, "ClipID", clip_id, "\n\t\t\t\t");
    leaf(tree, core, "InUse", "false", "\n\t\t\t");
    let segments = tree.sub_element(element, "TranscriptTextSegments", &[("ObjectRef", collection_id)]);
    tree.set_tail(segments, Some("\n\t"));
    element
}
