use super::*;

fn example() -> SourceGraph {
    SourceGraph::from_json_text(r#"{
      "schema":"anynle.graph/2","project":{"name":"Source Δ"},
      "media":[
        {"id":"v","kind":"video","locations":["/v.mxf"],"rate":"24","duration":"4","start":null},
        {"id":"a1","kind":"audio","locations":["/a1.mxf"],"sample_rate":48000,"bits":24,"stream":1,"duration":"4","start":null},
        {"id":"a2","kind":"audio","locations":["/a2.mxf"],"sample_rate":48000,"bits":24,"stream":2,"duration":"4","start":"0"}],
      "items":[{"id":"i","name":"Master","media":[{"media":"v","source_offset":"0"},{"media":"a1","source_offset":null},{"media":"a2","source_offset":"0"}],"stream_groups":[[1,2]]}],
      "bins":[{"id":"b","name":"Bin","items":["i"]}],
      "sequences":[{"id":"s","name":"Original overlap","rate":"24","frame_size":[1920,1080],"start":null,
        "tracks":[{"kind":"audio","index":9}],
        "placements":[
          {"id":"picture","item":"i","record_start":"0","source_in":"0","duration":"2","video_track":1},
          {"id":"out","item":"i","record_start":"0","source_in":"0","duration":"5/4","grouped_audio":[{"track":3,"streams":[1,2]}]},
          {"id":"in","item":"i","record_start":"3/4","source_in":"2","duration":"5/4","grouped_audio":[{"track":3,"streams":[1,2]}]}],
        "transitions":[{"id":"t","kind":"audio","track":3,"cut":"1","duration":"1/2","cut_offset":"1/4","style":"avid.DISSOLVE","outgoing":{"placements":["out"]},"incoming":{"placements":["in"]}}],
        "markers":[{"name":"Unresolved owner clock","duration":"0","owner":"edit","owner_occurrence":"sha256:fixture/SEQU/2/TNFX/child","owner_local":"-1/24","owner_rate":"24","start":null,"binding":{"placements":["out","in"],"reason":"retimed owner clock is unqualified"}}]}],
      "sidecars":[{"adapter":"avid.source","object":"s","data":null}]
    }"#).unwrap()
}
#[test]
fn named_empty_source_track_round_trips_and_refuses_a_conflicting_edit() {
    let mut graph = example();
    assert!(!graph.sequences[0].tracks[0].empty);
    assert_eq!(graph.sequences[0].tracks[0].name, None);
    graph.sequences[0].tracks.push(SourceTrack {
        kind: StreamKind::Video,
        index: 5,
        name: Some("Graphics & café".into()),
        empty: true,
        level_db: None,
    });
    let saved = graph.to_json_text().unwrap();
    assert_eq!(SourceGraph::from_json_text(&saved).unwrap(), graph);

    graph.sequences[0].tracks[1].index = 1;
    let error = graph.to_json_text().unwrap_err();
    assert!(error.to_string().contains("placement contradicts an empty source track"));
}

#[test]
fn edit_local_pan_rejects_duplicate_curves_and_keys_outside_the_span() {
    use crate::audio_pan::{linear_effect, PanKey};
    let mut graph = example();
    let edit = &mut graph.sequences[0].placements[1];
    edit.grouped_audio.clear();
    edit.audio_tracks = vec![3];
    edit.audio_streams = vec![1];
    let keys = [
        PanKey { at: Rational::ZERO, pan: Rational::integer(-1) },
        PanKey { at: edit.duration, pan: Rational::integer(1) },
    ];
    edit.effects.push(linear_effect(&keys));
    graph.validate().unwrap();
    let edit = &mut graph.sequences[0].placements[1];
    edit.effects.push(linear_effect(&keys));
    assert!(graph.validate().unwrap_err().to_string().contains("Linear pan"));
    let edit = &mut graph.sequences[0].placements[1];
    edit.effects = vec![linear_effect(&[
        keys[0], PanKey { at: Rational::integer(2), ..keys[1] },
    ])];
    assert!(graph.validate().unwrap_err().to_string().contains("Linear pan"));
}

#[test]
fn disabled_placement_survives_reload_and_uses_a_distinct_id() {
    let mut graph = example();
    let sequence = &mut graph.sequences[0];
    let mut disabled = sequence.placements[0].clone();
    disabled.id = "disabled-picture".into();
    sequence.disabled_placements.push(disabled);
    let text = graph.to_json_text().unwrap();
    assert_eq!(SourceGraph::from_json_text(&text).unwrap(), graph);
    graph.sequences[0].disabled_placements[0].id = "picture".into();
    assert!(graph.validate().is_err());
}

fn multicam_example()->SourceGraph{
    let mut g=example();
    let media=|stream|MulticamPiece{start:Rational::new(1,1).unwrap(),duration:Rational::new(3,1).unwrap(),item:Some("i".into()),stream:Some(stream),source_in:Some(Rational::ZERO)};
    let gap=MulticamPiece{start:Rational::ZERO,duration:Rational::new(1,1).unwrap(),item:None,stream:None,source_in:None};
    g.multicams.push(Multicam{id:"group".into(),name:"Offset group".into(),rate:Some(Rational::new(24,1).unwrap()),duration:Some(Rational::new(4,1).unwrap()),reason:None,lanes:vec![
        MulticamLane{kind:StreamKind::Video,index:1,selected:Some(0),reason:None,alternatives:vec![
            MulticamAlternative{id:"angle-a".into(),pieces:vec![gap.clone(),media(1)],reason:None},
            MulticamAlternative{id:"angle-b".into(),pieces:vec![MulticamPiece{start:Rational::ZERO,duration:Rational::new(4,1).unwrap(),..media(1)}],reason:None}]},
        // The one audio alternative is independent of the two video angles.
        MulticamLane{kind:StreamKind::Audio,index:3,selected:Some(0),reason:None,alternatives:vec![MulticamAlternative{id:"audio-only-choice".into(),pieces:vec![gap,media(2)],reason:None}]},
    ]});
    g.bins[0].items.push("group".into());
    g.multicam_uses.push(MulticamUse{id:"use".into(),sequence:"s".into(),candidates:vec!["group".into()],kind:StreamKind::Video,lane:Some(1),track:1,selected:Some(0),record_start:Some(Rational::ZERO),duration:Some(Rational::new(1,1).unwrap()),source_in:Some(Rational::ZERO),binding:SourceBinding{placements:vec![],reason:None},reason:None});
    g
}
#[test]
fn multicam_round_trip_preserves_independent_audio_and_selected_gap(){
    let mut g=multicam_example();let text=g.to_json_text().unwrap();
    assert_eq!(text.len(),g.encoded_len().unwrap());assert_eq!(SourceGraph::from_json_text(&text).unwrap(),g);
    // Known source gap emits no leaf placement but remains a real occurrence.
    assert!(g.multicam_uses[0].binding.placements.is_empty());
    assert_eq!(g.multicams[0].lanes[1].alternatives.len(),1);
    g.multicams[0].lanes[0].alternatives[0].pieces.clear();
    g.multicams[0].lanes[0].alternatives[0].reason=Some("mixed clock remains native-only".into());
    g.multicam_uses[0].reason=Some("selected layout unresolved".into());
    assert_eq!(SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap(),g);
}
#[test]
fn malformed_multicams_refuse_through_typed_value_and_text_apis(){
    let g=multicam_example();
    let mut bad=g.clone();bad.multicams[0].lanes[0].alternatives[0].pieces[0].item=Some("i".into());refuses_all(&bad,"gap/source fields");
    let mut bad=g.clone();bad.multicams[0].lanes[0].alternatives[0].pieces[1].start=Rational::ZERO;refuses_all(&bad,"contiguous partition");
    let mut bad=g.clone();bad.multicams[0].lanes[1].alternatives[0].pieces[1].stream=Some(99);refuses_all(&bad,"matching source stream");
    let mut bad=g.clone();bad.multicams[0].lanes[0].selected=Some(2);refuses_all(&bad,"selected alternative");
    let mut bad=g.clone();bad.multicams[0].lanes[0].alternatives[1].id="i".into();refuses_all(&bad,"duplicate");
    let mut bad=g.clone();bad.multicam_uses[0].binding.placements=vec!["missing".into()];refuses_all(&bad,"placement binding");
    let mut bad=g.clone();bad.multicam_uses[0].source_in=Some(Rational::new(4,1).unwrap());refuses_all(&bad,"source extent");
    let mut bad=g.clone();bad.multicam_uses[0].candidates.clear();refuses_all(&bad,"requires a reason");
    let mut bad=g.clone();bad.multicams[0].rate=Some(Rational::new(25,1).unwrap());refuses_all(&bad,"rate differs");
    let mut bad=g;bad.multicams[0].lanes[0].alternatives[0].pieces[1].source_in=Some(Rational::new(i64::MAX as i128,1).unwrap());refuses_all(&bad,"source range");
}
fn raw_json(g: &SourceGraph) -> String {
    let mut bytes = Vec::new();
    encode::write_graph(g, &mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}
fn refuses_all(g: &SourceGraph, expected: &str) {
    let text = raw_json(g);
    for e in [
        g.validate(),
        SourceGraph::from_json(&json::parse(&text).unwrap()).map(|_| ()),
        SourceGraph::from_json_text(&text).map(|_| ()),
        g.to_json_text().map(|_| ()),
    ] {
        let e = e.unwrap_err().to_string();
        assert!(e.contains(expected), "{e}, expected {expected}");
    }
}
#[test]
fn original_stream_clocks_channels_and_compressed_representation_round_trip(){
    let mut g=example();
    for (channel,m) in g.media.iter_mut().filter(|m|m.kind==StreamKind::Audio).enumerate(){
        m.bits=None;m.codec=Some("aac".into());m.locations=vec!["/Synthetic/source.mp4".into()];m.original=Some(OriginalStream{saved_locators:vec!["/Saved/source.mp4".into()],container:"mov".into(),stream_index:1,channel_index:Some(channel as i64),channels:Some(2),time_base:Rational::new(1,48000).unwrap(),start_pts:0,duration_ts:192000,sample_representation:Some("aac".into())});
    }
    let encoded=g.to_json_text().unwrap();assert_eq!(SourceGraph::from_json_text(&encoded).unwrap(),g);
    assert_eq!(g.media[1].stream,Some(1));assert_eq!(g.media[2].original.as_ref().unwrap().channel_index,Some(1));
    let mut bad=g.clone();bad.media[1].bits=Some(16);refuses_all(&bad,"sample representation");
    let mut bad=g.clone();bad.media[2].original.as_mut().unwrap().channel_index=Some(2);refuses_all(&bad,"physical audio channel");
    let mut bad=g;bad.media[1].original.as_mut().unwrap().duration_ts=191999;refuses_all(&bad,"physical duration");
}
#[test]
fn source_round_trip_preserves_unknowns_native_owners_and_original_overlaps() {
    let mut g = example();
    g.project.name = "quotes \" \n \\ and control \u{0001} Δ".into();
    let text = g.to_json_text().unwrap();
    assert_eq!(g.encoded_len().unwrap(), text.len());
    assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
    assert_eq!(
        SourceGraph::from_json_text(&text)
            .unwrap()
            .to_json_text()
            .unwrap(),
        text
    );
    assert_eq!(g.media[0].start, None);
    assert_eq!(g.media[2].start, Some(Rational::ZERO));
    assert_eq!(g.items[0].media[1].source_offset, None);
    assert_eq!(g.items[0].media[2].source_offset, Some(Rational::ZERO));
    assert_eq!(g.sequences[0].start, None);
    assert_eq!(g.sequences[0].markers[0].start, None);
    assert_eq!(
        g.sequences[0].placements[1].duration,
        Rational::new(5, 4).unwrap()
    );
    assert_eq!(
        g.sequences[0].placements[2].record_start,
        Rational::new(3, 4).unwrap()
    );
    assert!(Graph::from_json_text(&text).is_err());
    let old=json::parse(r#"{"schema":"anynle.graph/1","project":{"name":"Legacy"},"media":[],"items":[],"bins":[],"sequences":[]}"#).unwrap();
    assert!(Graph::from_json(&old).is_ok());
    assert!(SourceGraph::from_json(&old).is_err());
}
#[test]
fn unresolved_bindings_are_data_but_false_resolved_references_are_errors() {
    let mut g = example();
    g.sequences[0].transitions[0].incoming = SourceBinding {
        placements: vec![],
        reason: Some("native child expands ambiguously".into()),
    };
    g.validate().unwrap();
    g.sequences[0].transitions[0].incoming.reason = None;
    refuses_all(&g, "resolved endpoint needs");
    let mut g = example();
    g.sequences[0].transitions[0].incoming.placements = vec!["ghost".into()];
    refuses_all(&g, "unknown placement");
    let mut g = example();
    g.sequences[0].transitions[0].incoming.placements = vec!["picture".into()];
    refuses_all(&g, "different track");
    let mut g = example();
    g.sequences[0].transitions[0].incoming.placements = vec!["out".into()];
    refuses_all(&g, "endpoints overlap");
    let mut g = example();
    g.sequences[0].markers[0].binding = SourceBinding {
        placements: vec![],
        reason: Some("owner timing is unknown".into()),
    };
    g.validate().unwrap();
    g.sequences[0].markers[0].owner_local = None;
    g.validate().unwrap();
    assert_eq!(
        SourceGraph::from_json_text(&g.to_json_text().unwrap())
            .unwrap()
            .sequences[0]
            .markers[0]
            .owner_local,
        None
    );
    g.sequences[0].markers[0].binding.reason = Some(String::new());
    refuses_all(&g, "needs a reason");
}
#[test]
fn known_offsets_bound_actual_selected_streams_unknown_offsets_are_not_zero() {
    let mut g = example();
    for m in &mut g.items[0].media[1..] {
        m.source_offset = None;
    }
    g.sequences[0].placements[1].source_in = Rational::integer(100);
    g.validate().unwrap();
    g.items[0].media[2].source_offset = Some(Rational::ZERO);
    refuses_all(&g, "source range exceeds selected media");
    g.items[0].media[2].source_offset = Some(Rational::integer(-100));
    g.sequences[0].placements[2].source_in = Rational::integer(102);
    g.validate().unwrap();
}
#[test]
fn schema_and_native_payload_shadowing_cannot_bypass_value_entrypoint() {
    let g = example();
    let mut v = json::parse(&g.to_json_text().unwrap()).unwrap();
    if let Value::Obj(p) = &mut v {
        p.push(("schema".into(), Value::Str(SCHEMA.into())));
    }
    assert!(SourceGraph::from_json(&v)
        .unwrap_err()
        .to_string()
        .contains("repeated JSON key"));
    let mut g = example();
    g.sidecars.push(g.sidecars[0].clone());
    refuses_all(&g, "duplicate adapter/object");
    let mut g = example();
    g.sidecars[0].data = Value::Float(f64::NAN);
    assert!(g.validate().is_err());
    assert!(g.to_json_text().is_err());
}

#[test]
fn owned_json_preserves_sidecar_null_and_refusal_order() {
    let graph = example();
    let encoded = graph.to_json_text().unwrap();
    assert_eq!(graph.sidecars[0].data, Value::Null);
    let parsed = json::parse(&encoded).unwrap();
    assert_eq!(SourceGraph::from_json(&parsed).unwrap(), graph);
    assert_eq!(SourceGraph::from_json_text(&encoded).unwrap(), graph);

    let bad_reference = encoded.replacen("\"item\":\"i\"", "\"item\":\"missing\"", 1);
    let two_defects = bad_reference.replace(",\"data\":null", "");
    for (input, expected) in [
        (bad_reference, "unknown item"),
        (two_defects, "source graph.sidecars[0]: missing \"data\""),
    ] {
        let value = json::parse(&input).unwrap();
        let borrowed = SourceGraph::from_json(&value).unwrap_err().to_string();
        let owned = SourceGraph::from_json_text(&input).unwrap_err().to_string();
        assert_eq!(owned, borrowed);
        assert!(owned.contains(expected), "{owned}");
    }
}
#[test]
fn source_lanes_are_independent_and_native_grouped_channels_stay_atomic() {
    let mut g = example();
    g.sequences[0].placements[0].audio_tracks = vec![1];
    refuses_all(&g, "exactly one lane");
    let mut g = example();
    g.sequences[0].placements[1].grouped_audio[0].streams = vec![1, 2];
    g.validate().unwrap();
    g.sequences[0].placements[1].audio_tracks = vec![1];
    refuses_all(&g, "exactly one lane");
}
#[test]
fn linear_picture_keys_round_trip_on_the_item_source_clock() {
    use crate::picture_motion::{linear_effect, read_linear, MotionKey};
    let mut g = example();
    let keys = [
        MotionKey {
            at: Rational::new(1001, 4800).unwrap(),
            position: (0.0, 0.0),
            scale_percent: 100.0,
        },
        MotionKey {
            at: Rational::new(7007, 6000).unwrap(),
            position: (0.05, -0.025),
            scale_percent: 112.5,
        },
    ];
    g.sequences[0].placements[0].effects = vec![linear_effect(keys)];
    let text = g.to_json_text().unwrap();
    let decoded = SourceGraph::from_json_text(&text).unwrap();
    assert_eq!(decoded, g);
    assert_eq!(
        read_linear(&decoded.sequences[0].placements[0].effects[0]),
        Some(keys),
    );

    let mut reversed = keys;
    reversed.reverse();
    g.sequences[0].placements[0].effects = vec![linear_effect(reversed)];
    refuses_all(&g, "two ordered item-source keys");
}
#[test]
fn resolved_single_pair_covers_the_whole_transition_interval() {
    let g = example();
    g.validate().unwrap(); // Both original edits cover .75..1.25.
    let mut bad = g.clone();
    bad.sequences[0].transitions[0].cut = Rational::integer(5);
    refuses_all(&bad, "does not cover the transition interval");
    let mut bad = g.clone();
    bad.sequences[0].placements[1].duration = Rational::integer(1);
    refuses_all(&bad, "does not cover the transition interval");
    let mut bad = g.clone();
    bad.sequences[0].placements[2].record_start = Rational::integer(1);
    refuses_all(&bad, "does not cover the transition interval");
    // A candidate interval is not claimed qualified by an unresolved binding.
    let mut unresolved = bad.clone();
    unresolved.sequences[0].transitions[0].incoming.reason =
        Some("native expansion needs qualification".into());
    unresolved.validate().unwrap();
    // Structurally resolved multi-piece relationships remain source facts;
    // destination preparation is responsible for qualifying their coverage.
    let mut multi = bad;
    let mut second = multi.sequences[0].placements[1].clone();
    second.id = "out-second".into();
    multi.sequences[0].placements.push(second);
    multi.sequences[0].transitions[0]
        .outgoing
        .placements
        .push("out-second".into());
    multi.validate().unwrap();
    let mut overflow = g.clone();
    overflow.sequences[0].transitions[0].cut = Rational::integer(i64::MAX);
    refuses_all(&overflow, "transition time overflow");
}
#[test]
fn invalid_graphs_and_over_budget_encodings_write_no_caller_bytes() {
    let mut g = example();
    g.sequences[0].rate = Rational::ZERO;
    let mut bytes = Vec::new();
    assert!(g.write_json(&mut bytes).is_err());
    assert!(bytes.is_empty());
    let mut g = example();
    g.sidecars[0].data = Value::Str("x".repeat(MAX_SOURCE_GRAPH_BYTES));
    assert!(g
        .write_json(&mut bytes)
        .unwrap_err()
        .to_string()
        .contains("exceeds 256 MiB"));
    assert!(bytes.is_empty());
}
#[test]
fn streaming_writer_propagates_io_failures_without_panicking() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("controlled failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let e = example().write_json(&mut Broken).unwrap_err();
    assert!(e.to_string().contains("controlled failure"));
}
fn clock_example() -> SourceClock {
    SourceClock {
        effective_count: 240,
        rate: Rational::integer(24),
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: 238,
            source_offset: 4000,
            source_rate: Rational::integer(48000),
            source_package: "physical-package".into(),
            timecode_track: "timecode-track".into(),
            route: "file-source-clip".into(),
        },
    }
}
#[test]
fn typed_clocks_round_trip_and_unknown_starts_remain_unknown() {
    let mut g = example();
    let c = clock_example();
    g.media[0].start = c.exact_start();
    g.media[0].clock = Some(c.clone());
    g.sequences[0].start = c.exact_start();
    g.sequences[0].clock = Some(c);
    let text = g.to_json_text().unwrap();
    assert!(text.contains("\"effective_count\":\"240\""));
    assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
    let unknown = example();
    assert!(unknown.media[0].clock.is_none());
    assert!(unknown.media[0].start.is_none());
    assert!(unknown.media[2].clock.is_none());
    assert_eq!(unknown.media[2].start, Some(Rational::ZERO));
    // Old, pre-integration source audits omit clock and remain readable as unknown.
    let old = unknown
        .to_json_text()
        .unwrap()
        .replace("\"clock\":null,", "");
    assert_eq!(SourceGraph::from_json_text(&old).unwrap(), unknown);
}
#[test]
fn clock_contradictions_refuse_through_typed_value_and_text_apis() {
    let mut g = example();
    let c = clock_example();
    g.media[0].start = c.exact_start();
    g.media[0].clock = Some(c);
    let mut bad = g.clone();
    bad.media[0].start = Some(Rational::ZERO);
    refuses_all(&bad, "start contradicts");
    let mut bad = g.clone();
    bad.media[0].start = None;
    refuses_all(&bad, "start contradicts");
    let mut bad = g.clone();
    bad.media[0].clock.as_mut().unwrap().effective_count += 1;
    refuses_all(&bad, "count contradicts");
    let mut bad = g.clone();
    bad.media[0]
        .clock
        .as_mut()
        .unwrap()
        .provenance
        .source_offset = 1;
    refuses_all(&bad, "whole timecode count");
    let mut bad = g.clone();
    bad.media[0].clock.as_mut().unwrap().display_base = 0;
    refuses_all(&bad, "base must be positive");
    let mut bad = g.clone();
    bad.media[0].clock.as_mut().unwrap().rate = Rational::ZERO;
    refuses_all(&bad, "rate must be positive");
    let mut bad = g.clone();
    bad.media[0]
        .clock
        .as_mut()
        .unwrap()
        .provenance
        .route
        .clear();
    refuses_all(&bad, "provenance must be nonempty");
    let mut bad = g;
    bad.media[0]
        .clock
        .as_mut()
        .unwrap()
        .provenance
        .source_package = "x".repeat(32769);
    refuses_all(&bad, "text");
}
#[test]
fn u64_counts_and_cancelled_offset_products_are_exact_without_panics() {
    let mut g = example();
    let mut c = clock_example();
    c.effective_count = u64::MAX;
    c.rate = Rational::integer(3);
    c.provenance.origin_count = u64::MAX;
    c.provenance.source_offset = 0;
    c.provenance.source_rate = Rational::integer(3);
    g.media[0].start = c.exact_start();
    g.media[0].clock = Some(c.clone());
    assert_eq!(
        g.media[0].start,
        Some(Rational::integer(6148914691236517205))
    );
    let text = g.to_json_text().unwrap();
    assert!(text.contains("18446744073709551615"));
    assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
    c.rate = Rational::new(i64::MAX as i128, i64::MAX as i128 - 1).unwrap();
    c.provenance.source_rate = c.rate;
    c.provenance.origin_count = 0;
    c.provenance.source_offset = i64::MAX;
    c.effective_count = i64::MAX as u64;
    g.media[0].start = c.exact_start();
    g.media[0].clock = Some(c.clone());
    assert_eq!(g.media[0].start, Some(Rational::integer(i64::MAX - 1)));
    g.validate().unwrap();
    c.rate = Rational::integer(1);
    c.provenance.source_rate = Rational::integer(1);
    c.provenance.origin_count = u64::MAX;
    c.provenance.source_offset = i64::MIN;
    c.effective_count = i64::MAX as u64;
    g.media[0].start = c.exact_start();
    g.media[0].clock = Some(c);
    g.validate().unwrap();
    g.media[0].clock.as_mut().unwrap().provenance.source_offset = 1;
    refuses_all(&g, "count contradicts");
    let mut c = clock_example();
    c.effective_count = u64::MAX;
    c.provenance.origin_count = u64::MAX;
    c.provenance.source_offset = 0;
    c.rate = Rational::integer(1);
    g.media[0].clock = Some(c);
    refuses_all(&g, "not representable");
}
#[test]
fn clock_wire_counts_require_canonical_u64_strings() {
    let mut g = example();
    let c = clock_example();
    g.media[0].start = c.exact_start();
    g.media[0].clock = Some(c);
    let original = g.to_json_text().unwrap();
    for bad in [
        "\"01\"",
        "\"-1\"",
        "\"+1\"",
        "\"1.0\"",
        "\"18446744073709551616\"",
        "240",
    ] {
        let text = original.replacen(
            "\"effective_count\":\"240\"",
            &format!("\"effective_count\":{bad}"),
            1,
        );
        assert!(SourceGraph::from_json_text(&text).is_err(), "{bad}");
        assert!(
            SourceGraph::from_json(&json::parse(&text).unwrap()).is_err(),
            "{bad}"
        );
    }
}
#[test]
fn unknown_sequence_canvas_round_trips_without_inventing_a_default() {
    let mut g = example();
    g.sequences[0].frame_size = None;
    let text = g.to_json_text().unwrap();
    assert!(text.contains("\"frame_size\":null"));
    assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
    assert_eq!(
        SourceGraph::from_json(&json::parse(&text).unwrap()).unwrap(),
        g
    );
    let missing = text.replace("\"frame_size\":null,", "");
    assert_eq!(SourceGraph::from_json_text(&missing).unwrap(), g);
    g.sequences[0].frame_size = Some((2048, 1080));
    assert_eq!(
        SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap(),
        g
    );
    g.sequences[0].frame_size = Some((0, 1080));
    let text = raw_json(&g);
    assert!(g.validate().is_err());
    assert!(g.to_json_text().is_err());
    assert!(SourceGraph::from_json_text(&text).is_err());
    assert!(SourceGraph::from_json(&json::parse(&text).unwrap()).is_err());
}
