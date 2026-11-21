//! Saved edit routing for embedded audio whose playback map is unresolved.
use super::*;

pub(super) const ROUTE_ADAPTER: &str = "resolve.embedded_audio_routes";

pub(super) fn observe_edit(
    source_rate: Rational,
    node: &Node,
    track: &Node,
    index: i64,
    sequence: &SourceSequence,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<()> {
    let occurrence = node.id()?;
    let saved_selector = node.maybe("MediaTrackIdx")?;
    let encoded = node.maybe("VirtualAudioTrackBA")?
        .filter(|saved| !saved.is_empty() && saved.len() <= 512);
    let channels = match (saved_selector, encoded) {
        (Some("0"), Some(saved)) => channels(&blobs.qt(saved)?)?,
        _ => None,
    };
    let mut facts = vec![
        ("occurrence".into(), text(occurrence)),
        ("media_pool_item".into(), text(node.get("MediaRef")?)),
        ("track_id".into(), text(track.id()?)),
        ("track".into(), Value::Int(index)),
        ("saved_track_subtype".into(), text(track.get("SubType")?)),
        ("saved_track_flags".into(), text(track.get("Flags")?)),
        ("saved_clip_flags".into(), text(node.get("Flags")?)),
        ("saved_media_track_index".into(), saved_selector.map_or(Value::Null, text)),
        ("physical_channels".into(), channels.as_ref().map_or(Value::Null, |selected| {
            Value::Arr(selected.iter().copied().map(Value::Int).collect())
        })),
        ("saved_virtual_audio_track".into(), encoded.map_or(Value::Null, text)),
    ];
    for (tag, key) in [("MediaStartTime", "saved_media_start_time"), ("MediaFrameRate", "saved_media_frame_rate")] {
        if let Some(saved) = node.maybe(tag)? {
            facts.push((key.into(), text(saved)));
        }
    }
    let mut complete_timing = true;
    for tag in ["Start", "Duration", "In", "MediaTimemapBA"] {
        complete_timing &= node.maybe(tag)?.is_some();
    }
    if complete_timing {
        record_saved_timing(graph, node, &sequence.id, source_rate, sequence.rate, blobs)?;
    } else {
        for (tag, key) in [("Start", "saved_start"), ("Duration", "saved_duration"), ("In", "saved_in")] {
            if let Some(saved) = node.maybe(tag)? {
                facts.push((key.into(), text(saved)));
            }
        }
    }
    observations::append(graph, ROUTE_ADAPTER, &sequence.id, Value::Obj(facts));
    let (aspect, reason) = if channels.is_some() {
        ("timing", "Resolve embedded AAC routing is saved; playback timing or track mix is unqualified")
    } else {
        ("audio_routing", "Resolve embedded AAC edit has an unqualified saved channel selector")
    };
    omitted(graph, &sequence.id, occurrence, aspect, reason);
    Ok(())
}

pub(super) fn channels(route: &encoding::Map) -> Result<Option<&'static [i64]>> {
    Ok(match (route.int("AudioType")?, route.bytes("ChannelsBA")?) {
        (0, [0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0x40, 1, 0, 0, 0x40, 2]) => Some(&[1, 2]),
        (1, [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 1]) => Some(&[1]),
        (1, [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 2]) => Some(&[2]),
        _ => None,
    })
}
