//! Internal Resolve-to-Premiere path for picture-only 24 fps linked ProRes.
use crate::{
    linked::{self, MediaSpec, Profile},
    media::media_digest,
    Result, StreamSelector,
};
use graph::{
    source::{OriginalStream, SourceGraph, TextKind},
    Rational, StreamKind,
};
use prproj::{
    convert::{model::Options, writer::prepare_typed_project},
    RandomUuids,
};
use std::{collections::HashSet, path::Path};

pub fn write(
    mut graph: SourceGraph,
    sequence_id: &str,
    ffprobe: &Path,
    output: &Path,
) -> Result<Vec<graph::Loss>> {
    graph.validate()?;
    let index = graph
        .sequences
        .iter()
        .position(|seq| seq.id == sequence_id)
        .ok_or("selected Resolve sequence is absent")?;
    let sequence = &graph.sequences[index];
    let qualified_clock = sequence.clock.as_ref().is_some_and(|clock| {
        clock.provenance.route == "Resolve archive clock"
            && clock.provenance.source_package == sequence.id
            && clock.provenance.timecode_track == "Sm2Sequence.MediaExtents"
            && clock.provenance.source_offset == 0
            && clock.provenance.origin_count == clock.effective_count
            && clock.rate == sequence.rate
            && clock.display_base == 24
            && !clock.drop
            && clock.exact_start() == sequence.start
    });
    if sequence.rate != Rational::integer(24)
        || sequence.frame_size != Some((1920, 1080))
        || !qualified_clock
        || !sequence
            .tracks
            .iter()
            .filter(|track| track.kind == StreamKind::Video)
            .map(|track| track.index)
            .eq([1])
        || sequence.tracks.iter().any(|track| track.level_db.is_some())
        || !sequence.transitions.is_empty()
        || sequence.placements.is_empty()
        || sequence.placements.iter().any(|edit| {
            edit.video_track != Some(1)
                || !edit.audio_tracks.is_empty()
                || !edit.grouped_audio.is_empty()
                || edit.transform.is_some()
                || !edit.effects.is_empty()
                || edit.frame_map.is_some()
                || graph.sidecar(crate::grade::ADAPTER, &edit.id).is_some()
        })
        || sequence
            .texts
            .iter()
            .any(|text| text.kind != TextKind::Caption)
        || graph
            .sidecar(crate::generator::ADAPTER, sequence_id)
            .is_some()
        || graph.sidecar("resolve.nested_uses", sequence_id).is_some()
        || graph
            .multicam_uses
            .iter()
            .any(|usage| usage.sequence == sequence_id)
    {
        return Err(
            "expected a 24 fps 1080p Resolve picture-only sequence with a qualified origin".into(),
        );
    }
    let used: HashSet<_> = sequence.placements.iter().map(|p| p.item.clone()).collect();
    let mut owned = used.clone();
    owned.insert(sequence_id.into());
    owned.extend(sequence.placements.iter().map(|p| p.id.clone()));
    owned.extend(sequence.texts.iter().map(|text| text.id.clone()));
    graph.items.retain(|item| used.contains(&item.id));
    let used_media: HashSet<_> = graph
        .items
        .iter()
        .flat_map(|item| item.media.iter().map(|link| link.media.clone()))
        .collect();
    owned.extend(used_media.iter().cloned());
    if let Some(loss) = graph
        .losses
        .iter()
        .find(|loss| loss.object == "project" || owned.contains(&loss.object))
    {
        return Err(format!("Resolve source loss on {}: {}", loss.object, loss.reason).into());
    }
    graph.media.retain(|media| used_media.contains(&media.id));
    graph
        .bins
        .iter_mut()
        .for_each(|bin| bin.items.retain(|item| used.contains(item)));
    graph.sequences = vec![graph.sequences.swap_remove(index)];
    graph.multicams.clear();
    graph.multicam_uses.clear();
    graph.losses.clear();
    graph
        .sidecars
        .retain(|receipt| owned.contains(&receipt.object));
    let mut verified = Vec::new();
    for item in &graph.items {
        let [link] = item.media.as_slice() else {
            return Err("Resolve picture must bind one original stream".into());
        };
        let source = graph
            .media
            .iter_mut()
            .find(|source| source.id == link.media)
            .ok_or("Resolve original stream is absent")?;
        let [locator] = source.locations.as_slice() else {
            return Err("Resolve picture must have one saved locator".into());
        };
        let path = Path::new(locator);
        let selector = StreamSelector {
            media_id: source.id.clone(),
            index: 0,
            channel: None,
        };
        let spec = MediaSpec {
            item_id: item.id.clone(),
            bin_id: String::new(),
            path: path.to_owned(),
            sha256: hex::encode(media_digest(path)?),
            streams: vec![],
        };
        let (media, time_base, duration_ts) = linked::probe(&spec, ffprobe, item.name.clone(), 0)?;
        if !matches!(
            media.profile,
            Profile::Mov {
                h264: false,
                aac: false,
                ..
            }
        ) || source.codec.as_deref() != Some("apch")
        {
            return Err("Resolve-to-Premiere picture requires verified ProRes HQ MOV".into());
        }
        linked::check_stream(source, &selector, &media, time_base, duration_ts)?;
        source.original = Some(OriginalStream {
            saved_locators: source.locations.clone(),
            container: "mov".into(),
            stream_index: 0,
            channel_index: None,
            channels: None,
            time_base,
            start_pts: 0,
            duration_ts,
            sample_representation: None,
        });
        verified.push(media);
    }
    let prepared = premiere_adapter::prepare_resolve_originals(&graph, &Options::default())?;
    let output =
        prepare_typed_project(&prepared.input, output, &mut RandomUuids::new(), &[], false)?;
    for media in &verified {
        media.recheck()?;
    }
    output.publish()?;
    Ok(prepared.losses)
}
