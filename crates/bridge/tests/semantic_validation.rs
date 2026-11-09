//! Projected graphs must meet model/1's placement rules before reaching a writer.
use bridge::{to_graph, to_model};
use graph::{Graph, GroupedAudio, Rational, StreamKind};
use prproj::convert::model::{parse_model_str, Options, TICKS_PER_SECOND};

fn example() -> (Graph, Options) {
    let model = parse_model_str(include_str!("../../../examples/opatom_clip.model.json")).unwrap();
    to_graph(&model).unwrap()
}

fn refused(graph: &Graph, options: &Options, message: &str) {
    graph
        .validate()
        .expect("the neutral graph itself permits this projection");
    let error = to_model(graph, options).unwrap_err().to_string();
    assert!(
        error.contains(message),
        "expected {message:?}, got {error:?}"
    );
}

fn neutral_refused(graph: &Graph, options: &Options, message: &str) {
    let error=graph.validate().unwrap_err().to_string();
    assert!(error.contains(message), "expected {message:?}, got {error:?}");
    assert_eq!(to_model(graph,options).unwrap_err().to_string(), error);
}

#[test]
fn tick_aligned_but_off_frame_record_positions_and_durations_refuse() {
    for duration in [false, true] {
        let (mut graph, options) = example();
        let tick = Rational::from_units(1, TICKS_PER_SECOND).unwrap();
        if duration {
            graph.sequences[0].placements[0].duration = tick;
        } else {
            graph.sequences[0].placements[0].record_start = tick;
        }
        refused(&graph, &options, "is not on the sequence frame grid");
    }
}

#[test]
fn projected_source_range_must_fit_the_media() {
    let (mut graph, options) = example();
    graph.sequences[0].placements[0].source_in = Rational::integer(20);
    neutral_refused(&graph, &options, "source range");
}

#[test]
fn speed_uses_the_source_span_in_the_range_check() {
    let (mut graph, options) = example();
    let p = &mut graph.sequences[0].placements[0];
    p.audio_tracks.clear();
    p.audio_streams.clear();
    p.track_gains_db.clear();
    p.track_pans.clear();
    p.speed = Some(graph::Speed {
        ratio: (1, 2),
        source_duration: Rational::integer(20),
    });
    neutral_refused(&graph, &options, "source range");
}

#[test]
fn projected_audio_tracks_and_streams_must_exist() {
    let (mut graph, options) = example();
    graph.sequences[0].placements[0].audio_tracks = vec![1, 2, 3];
    graph.sequences[0].placements[0].audio_streams.clear();
    graph.sequences[0].placements[0].track_gains_db.clear();
    graph.sequences[0].placements[0].track_pans.clear();
    neutral_refused(&graph, &options, "more audio tracks than source streams");
    let (mut graph, options) = example();
    graph.sequences[0].placements[0].audio_streams = vec![99, 2];
    neutral_refused(
        &graph,
        &options,
        "audio routing names a stream the item lacks",
    );
    let (mut graph, options) = example();
    graph.sequences[0].placements[0]
        .grouped_audio
        .push(GroupedAudio {
            track: 3,
            streams: vec![1, 99],
            gain_db: None,
            pans: vec![None, None],
        });
    neutral_refused(&graph, &options, "grouped audio names a stream the item lacks");
}

#[test]
fn picture_placement_requires_a_picture_stream() {
    let (mut graph, options) = example();
    let video = graph
        .media
        .iter()
        .find(|m| m.kind == StreamKind::Video)
        .unwrap()
        .id
        .clone();
    graph.items[0].media.retain(|m| m.media != video);
    neutral_refused(&graph, &options, "picture placement needs one unambiguous picture stream");
}

#[test]
fn projection_must_not_collapse_distinct_item_ids() {
    let (mut graph, options) = example();
    let mut second = graph.items[0].clone();
    second.id = second.id.strip_prefix("clip:").unwrap().to_string();
    graph.items.push(second);
    refused(&graph, &options, "clip id collision");
}

#[test]
fn invalid_sequence_rates_refuse_before_tick_arithmetic() {
    for rate in [Rational::ZERO, Rational::integer(-24)] {
        let (mut graph, options) = example();
        graph.sequences[0].rate = rate;
        neutral_refused(&graph, &options, "rate must be positive");
    }
    for rate in [Rational::integer(1001), Rational::new(23, 7).unwrap()] {
        let (mut graph, options) = example();
        graph.sequences[0].rate = rate;
        refused(&graph, &options, "rate");
    }
}

#[test]
fn exact_media_end_and_subframe_source_offsets_remain_valid() {
    let (mut graph, options) = example();
    let media_duration = graph.media.iter().map(|m| m.duration).max().unwrap();
    let p = &mut graph.sequences[0].placements[0];
    p.source_in = media_duration.checked_sub(p.duration).unwrap();
    to_model(&graph, &options).unwrap();
    // The model/1 rule grids record start and duration, not source In; retain that contract.
    graph.sequences[0].placements[0].source_in = Rational::from_units(1, TICKS_PER_SECOND).unwrap();
    to_model(&graph, &options).unwrap();
}
