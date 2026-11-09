use graph::source::*;
use graph::{MarkerOwner, Rational as Q, Sidecar, StreamKind, Value};
use premiere_adapter::{prepare, ClockProfile};
use prproj::convert::model::{parse_model_str, Options};

fn v(p: Vec<(&str, Value)>) -> Value {
    Value::Obj(p.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn clock(count: u64, base: i64, drop: bool) -> Value {
    v(vec![
        ("count", Value::Str(count.to_string())),
        ("rounded_base", Value::Int(base)),
        ("drop", Value::Bool(drop)),
    ])
}
fn fixture() -> SourceGraph {
    let m = parse_model_str(include_str!("../../../examples/opatom_clip.model.json")).unwrap();
    let (g, _) = bridge::to_graph(&m).unwrap();
    let mut sidecars = g.sidecars;
    let media = g
        .media
        .into_iter()
        .map(|m| {
            sidecars.push(Sidecar {
                adapter: "avid.source".into(),
                object: m.id.clone(),
                data: v(vec![
                    ("legacy_clock", Value::Null),
                    ("legacy_file_clock", clock(0, 24, false)),
                ]),
            });
            SourceMedia {
                clock: None,
                id: m.id,
                kind: m.kind,
                locations: m.locations,
                codec: m.codec,
                frame_size: m.frame_size,
                rate: m.rate,
                sample_rate: m.sample_rate,
                bits: m.bits,
                duration: m.duration,
                start: Some(m.start),
                stream: m.stream,
                online: m.online,
                original: None,
            }
        })
        .collect();
    let items = g
        .items
        .into_iter()
        .map(|i| {
            sidecars.push(Sidecar {
                adapter: "avid.source".into(),
                object: i.id.clone(),
                data: v(vec![("drop", Value::Bool(false))]),
            });
            SourceItem {
                id: i.id,
                name: i.name,
                media: i
                    .media
                    .into_iter()
                    .map(|im| SourceItemMedia {
                        media: im.media,
                        gain_db: im.gain_db,
                        source_offset: Some(Q::ZERO),
                    })
                    .collect(),
                umid: i.umid,
                tape: i.tape,
                rate: i.rate,
                color_rgb16: i.color_rgb16,
                stream_groups: i.stream_groups,
                markers: Vec::new(),
            }
        })
        .collect();
    let sequences = g
        .sequences
        .into_iter()
        .map(|s| {
            sidecars.push(Sidecar {
                adapter: "avid.source".into(),
                object: s.id.clone(),
                data: v(vec![(
                    "compatibility_start",
                    Value::Str(s.start.to_string()),
                )]),
            });
            let mut placements = Vec::new();
            for mut p in s.placements {
                for (i, &track) in p.audio_tracks.iter().enumerate() {
                    let mut a = p.clone();
                    a.id = format!("{}:A{track}", p.id);
                    a.video_track = None;
                    a.audio_tracks = vec![track];
                    a.audio_streams = vec![p.audio_streams.get(i).copied().unwrap_or(track)];
                    a.track_gains_db = Vec::new();
                    a.track_pans = Vec::new();
                    a.effects = Vec::new();
                    a.transform = None;
                    placements.push(a);
                }
                p.audio_tracks.clear();
                p.audio_streams.clear();
                p.track_gains_db.clear();
                p.track_pans.clear();
                p.grouped_audio.clear();
                placements.insert(0, p);
            }
            SourceSequence {
            texts: Vec::new(),
                clock: None,
                id: s.id,
                name: s.name,
                rate: s.rate,
                frame_size: Some(s.frame_size),
                start: None,
                tracks: s.tracks,
                placements,
                transitions: Vec::new(),
                markers: s
                    .markers
                    .into_iter()
                    .map(|m| SourceMarker {
                        name: m.name,
                        comment: m.comment,
                        user: m.user,
                        color: m.color,
                        start: Some(m.start),
                        duration: m.duration,
                        track: m.track,
                        owner: m.owner,
                        owner_occurrence: "test:sequence".into(),
                        owner_local: Some(m.start),
                        owner_rate: Some(s.rate),
                        binding: SourceBinding {
                            placements: Vec::new(),
                            reason: None,
                        },
                        source: m.source,
                        inherited: m.inherited,
                    })
                    .collect(),
            }
        })
        .collect();
    SourceGraph {
        project: g.project,
        multicams: Vec::new(),
        multicam_uses: Vec::new(),
        media,
        items,
        bins: g.bins,
        sequences,
        losses: g.losses,
        sidecars,
    }
}
fn q(n: i64) -> Q {
    Q::integer(n)
}
fn options() -> Options {
    Options::default()
}
fn two_edits() -> SourceGraph {
    let mut g = fixture();
    let s = &mut g.sequences[0];
    s.markers.clear();
    s.placements.retain(|p| p.video_track.is_some());
    s.placements[0].record_start = q(0);
    s.placements[0].duration = q(4);
    s.placements[0].source_in = q(1);
    let mut next = s.placements[0].clone();
    next.id = "incoming".into();
    next.record_start = q(3);
    next.source_in = q(1);
    next.duration = q(4);
    s.placements.push(next);
    s.transitions.push(SourceTransition {
        id: "transition:one".into(),
        kind: StreamKind::Video,
        track: 1,
        cut: Q::new(7, 2).unwrap(),
        duration: q(1),
        cut_offset: Q::new(1, 2).unwrap(),
        style: "native:dissolve".into(),
        outgoing: SourceBinding {
            placements: vec![s.placements[0].id.clone()],
            reason: None,
        },
        incoming: SourceBinding {
            placements: vec!["incoming".into()],
            reason: None,
        },
    });
    g
}

#[test]
fn ordinary_avid_dissolve_preserves_an_asymmetric_cut() {
    let mut g = two_edits();
    let t = &mut g.sequences[0].transitions[0];
    t.style = "avid.EFF2_BLEND_DISSOLVE".into();
    t.cut_offset = Q::new(1, 4).unwrap();
    t.cut = Q::new(13, 4).unwrap();
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let [d] = p.video_dissolves.as_slice() else { panic!("expected one prepared dissolve") };
    let unit = prproj::convert::model::TICKS_PER_SECOND;
    let events = &p.model.sequences[0].events;
    assert_eq!((d.duration, d.before_cut), (unit, unit / 4));
    assert_eq!(events[d.outgoing].duration, 13 * unit / 4);
    assert_eq!(events[d.incoming].source_in, 5 * unit / 4);

    g.sequences[0].transitions[0].style = "avid.EFF2_BLEND_DISSOLVE_LOG".into();
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(p.video_dissolves.is_empty());
    assert!(p.losses.iter().any(|l| l.reason.contains("prepared as a video cut")));
}

#[test]
fn avid_subframe_volume_uses_the_media_clock_and_reports_tick_rounding() {
    let mut g = fixture();
    let placement = g.sequences[0].placements.iter_mut()
        .find(|p| !p.audio_tracks.is_empty() && p.video_track.is_none()).unwrap();
    let at = Q::new(49152 + 12837, 24576).unwrap();
    placement.volume_keys.push(graph::VolumeKey { at, db: -6.0 });
    g.sidecars.push(Sidecar {
        adapter: "avid.source".into(), object: placement.id.clone(),
        data: v(vec![("pvol_volume_keys", Value::Bool(true))]),
    });
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let key = p.model.sequences[0].events.iter().flat_map(|e| &e.audio.volume_keys).next().unwrap();
    assert_eq!(key.ticks, 640714429688);
    assert!(p.losses.iter().any(|l| l.aspect == "volume_key_clock"));
    assert_eq!(g.sequences[0].placements.iter().find_map(|p| p.volume_keys.first()).unwrap().at, at);
}

#[test]
fn picture_effects_follow_the_bound_video_edit_and_its_source_clock() {
    use prproj::convert::prepared::PictureEffect;
    let mut g = fixture();
    let placement = g.sequences[0].placements.iter_mut().find(|p| p.video_track.is_some()).unwrap();
    placement.effects.push(graph::Effect {
        kind: "avid.flip".into(),
        params: v(vec![("horizontal", Value::Bool(true)), ("vertical", Value::Bool(false))]),
    });
    let id = placement.id.clone();
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let [effect] = p.picture_effects.as_slice() else { panic!("expected one Flip") };
    assert_eq!((effect.sequence, effect.event), p.ownership[&id]);
    assert_eq!(effect.effect, PictureEffect::Flip { horizontal: true, vertical: false });

    let placement = g.sequences[0].placements.iter_mut().find(|p| p.id == id).unwrap();
    let unit = prproj::convert::model::TICKS_PER_SECOND;
    let source = placement.source_in.to_units(unit).unwrap();
    let last_frame = placement.duration.to_units(24).unwrap() - 1;
    placement.transform = Some(graph::Transform {
        position: (0.0, 0.0), scale: (100.0, 100.0),
        rotation: 0.0, anchor: (0.0, 0.0), crop: [0.0; 4],
    });
    let key = |frame, x, width| v(vec![
        ("frame", Value::Int(frame)),
        ("position", Value::Arr(vec![Value::Float(x), Value::Float(0.0)])),
        ("scale", Value::Arr(vec![Value::Float(width), Value::Float(100.0)])),
    ]);
    placement.effects = vec![graph::Effect {
        kind: "avid.resize_linear".into(),
        params: v(vec![("keys", Value::Arr(vec![key(0, 0.0, 100.0), key(last_frame, 0.2, 125.0)]))]),
    }];
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let PictureEffect::Resize(keys) = p.picture_effects[0].effect else { panic!("expected Linear Resize") };
    assert_eq!(keys.map(|k| k.ticks), [source, source + last_frame * (unit / 24)]);
    assert_eq!(keys[1].scale, [125.0, 100.0]);
}

#[test]
fn unchanged_model_and_source_immutability() {
    let g = fixture();
    let before = g.to_json_text().unwrap();
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let mut expected =
        parse_model_str(include_str!("../../../examples/opatom_clip.model.json")).unwrap();
    expected.sequences[0].events[0].audio_channels = vec![1, 2];
    assert_eq!(p.model, expected);
    assert_eq!(g.to_json_text().unwrap(), before);
    assert_eq!(p.ownership.len(), 3);
}
#[test]
fn lane_specific_audio_trims_do_not_trim_video() {
    let mut g = two_edits();
    let s = &mut g.sequences[0];
    s.transitions[0].kind = StreamKind::Audio;
    for p in &mut s.placements {
        p.video_track = None;
        p.audio_tracks = vec![1];
        p.audio_streams = vec![1];
    }
    let mut video = s.placements[0].clone();
    video.id = "untrimmed-picture".into();
    video.video_track = Some(1);
    video.audio_tracks.clear();
    video.audio_streams.clear();
    s.placements.push(video);
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(
        p.model.sequences[0]
            .events
            .iter()
            .find(|e| e.video_track.is_some())
            .unwrap()
            .duration,
        4 * 254016000000
    );
    assert_eq!(p.model.sequences[0].audio_crossfades.len(), 1);
}
#[test]
fn chained_transitions_use_original_spans() {
    let mut g = two_edits();
    let s = &mut g.sequences[0];
    let mut third = s.placements[1].clone();
    third.id = "third".into();
    third.record_start = q(6);
    s.placements.push(third);
    let mut t = s.transitions[0].clone();
    t.id = "transition:two".into();
    t.cut = Q::new(13, 2).unwrap();
    t.outgoing.placements = vec!["incoming".into()];
    t.incoming.placements = vec!["third".into()];
    s.transitions.push(t);
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let es = &p.model.sequences[0].events;
    assert_eq!(es.len(), 3);
    assert_eq!(es[1].duration, 3 * 254016000000);
    assert_eq!(es[1].record_start, 7 * 254016000000 / 2);
}
#[test]
fn ambiguous_and_retimed_transition_refusals_are_named() {
    let mut g = two_edits();
    g.sequences[0].transitions[0].outgoing.reason = Some("ambiguous owner".into());
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("unresolved_transition_topology"));
    let mut g = two_edits();
    g.sequences[0].placements[0].speed = Some(graph::Speed {
        ratio: (1, 2),
        source_duration: q(8),
    });
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("retimed_transition_endpoint"));
}
#[test]
fn unknown_offsets_are_not_zero_and_absolute_maps_are_not_offset_twice() {
    let mut g = fixture();
    g.items[0].media[0].source_offset = None;
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("unresolved_source_offset"));
    g.items[0].media[0].source_offset = Some(q(1));
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("nonzero_source_offset"));
}
#[test]
fn exact_clock_is_not_rounded_to_reconstruct_legacy_clock() {
    let mut g = fixture();
    g.media[0].start = Some(Q::new(1001001, 24000).unwrap());
    let sc = g
        .sidecars
        .iter_mut()
        .find(|s| s.object == g.media[0].id && s.adapter == "avid.source")
        .unwrap();
    sc.data = v(vec![("legacy_clock", clock(99, 24, false))]);
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(
        p.model.clips["clip-1"].video.as_ref().unwrap().start,
        99 * 254016000000 / 24
    );
    sc_remove(&mut g, "legacy_clock");
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).is_err());
}
fn sc_remove(g: &mut SourceGraph, key_name: &str) {
    let sc = g
        .sidecars
        .iter_mut()
        .find(|s| s.object == g.media[0].id && s.adapter == "avid.source")
        .unwrap();
    if let Value::Obj(xs) = &mut sc.data {
        xs.retain(|(k, _)| k != key_name);
    }
}
#[test]
fn trimmed_marker_keeps_record_time_on_sequence() {
    let mut g = two_edits();
    let p = g.sequences[0].placements[0].clone();
    g.sequences[0].markers.push(SourceMarker {
        name: "Tail".into(),
        comment: "preserve".into(),
        user: "".into(),
        color: None,
        start: Some(Q::new(15, 4).unwrap()),
        duration: Q::ZERO,
        track: None,
        owner: MarkerOwner::Edit,
        owner_occurrence: "native:owner".into(),
        owner_local: Some(Q::new(15, 4).unwrap()),
        owner_rate: Some(q(24)),
        binding: SourceBinding {
            placements: vec![p.id.clone()],
            reason: None,
        },
        source: None,
        inherited: false,
    });
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = &r.model.sequences[0].markers[0];
    assert_eq!(m.owner, "sequence");
    assert_eq!(m.event, None);
    assert_eq!(m.start, 15 * 254016000000 / 4);
    assert!(r.losses.iter().any(|l| l.aspect == "marker_ownership"));
}
#[test]
fn unknown_marker_transform_is_scoped_loss() {
    let mut g = two_edits();
    let p = g.sequences[0].placements[0].clone();
    g.sequences[0].markers.push(SourceMarker {
        name: "Unresolved".into(),
        comment: "".into(),
        user: "".into(),
        color: None,
        start: None,
        duration: Q::ZERO,
        track: None,
        owner: MarkerOwner::Edit,
        owner_occurrence: "native:owner".into(),
        owner_local: Some(q(1)),
        owner_rate: Some(q(24)),
        binding: SourceBinding {
            placements: vec![p.id.clone()],
            reason: Some("retimed".into()),
        },
        source: None,
        inherited: false,
    });
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(r.model.sequences[0].markers.is_empty());
    assert!(r.losses.iter().any(|l| l.aspect == "marker_timing"));
}

#[test]
fn off_clock_marker_duration_or_source_does_not_block_timeline() {
    for duration in [true, false] {
        let mut g = fixture();
        let m = &mut g.sequences[0].markers[0];
        if duration {
            m.duration = Q::new(1, 11).unwrap();
        } else {
            m.source = Some(Q::new(1, 11).unwrap());
        }
        let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
        assert_eq!(p.model.sequences[0].events.len(), 1);
        assert!(p.model.sequences[0].markers.is_empty());
        assert!(p.losses.iter().any(|l| l.aspect == "marker_timing"));
    }
}
#[test]
fn unrepresentable_item_streams_are_reported() {
    let mut g = fixture();
    g.items[0].stream_groups = vec![vec![1]];
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(p.losses.iter().any(|l| l.aspect == "stream_groups"));
    let mut g = fixture();
    let mut second = g.media[0].clone();
    second.id = "second-picture".into();
    g.media.push(second);
    g.items[0].media.push(SourceItemMedia {
        media: "second-picture".into(),
        gain_db: 0.0,
        source_offset: Some(Q::ZERO),
    });
    g.sequences[0].placements.clear();
    g.sequences[0].markers.clear();
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("multiple_picture_streams"));
}

#[test]
fn absolute_frame_map_is_preserved_without_adding_source_in() {
    let mut g = fixture();
    let s = &mut g.sequences[0];
    s.markers.clear();
    s.placements.retain(|p| p.video_track.is_some());
    let p = &mut s.placements[0];
    p.duration = Q::new(1, 12).unwrap();
    p.source_in = q(1);
    p.frame_map = Some(vec![25, 24]);
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(
        r.model.sequences[0].events[0].time_remap,
        Some(vec![25, 24])
    );
    assert_eq!(r.model.sequences[0].events[0].source_in, 254016000000);
}

#[test]
fn bin_only_offsets_cannot_be_silently_assembled_at_zero() {
    for mode in ["separate", "grouped", "merged"] {
        for offset in [None, Some(q(1))] {
            let mut g = fixture();
            g.sequences.clear();
            g.sidecars.retain(|s| s.object != "sequence:0");
            g.items[0].media[0].source_offset = offset;
            let mut o = options();
            o.bin_mode = mode.into();
            let message = prepare(&g, &o, ClockProfile::LegacyImport).err().unwrap().0;
            assert!(
                message.contains(if offset.is_none() {
                    "unresolved_source_offset"
                } else {
                    "nonzero_source_offset"
                }),
                "{message}"
            );
        }
    }
}
#[test]
fn unresolved_native_transition_cannot_disappear_behind_a_loss() {
    let mut g = two_edits();
    g.sequences[0].transitions.clear();
    let seq = &g.sequences[0].id.clone();
    let sc = g
        .sidecars
        .iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == *seq)
        .unwrap();
    if let Value::Obj(fields) = &mut sc.data {
        fields.push((
            "native_transitions".into(),
            Value::Arr(vec![v(vec![
                ("id", Value::Str("native:unknown-transition".into())),
                (
                    "position",
                    v(vec![("reason", Value::Str("unknown owner rate".into()))]),
                ),
            ])]),
        ));
    }
    g.validate().unwrap();
    let before = g.to_json_text().unwrap();
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("unresolved_transition_topology"));
    assert_eq!(g.to_json_text().unwrap(), before);
}

#[test]
fn audio_source_subframes_remain_exact_but_unsupported_record_subframes_refuse() {
    let mut g = fixture();
    g.sequences[0].markers.clear();
    g.sequences[0]
        .placements
        .retain(|p| p.audio_tracks == vec![1]);
    g.sequences[0].placements[0].source_in = Q::new(1, 48000).unwrap();
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(
        r.model.sequences[0].events[0].source_in,
        254016000000 / 48000
    );
    g.sequences[0].placements[0].record_start = Q::new(1, 48000).unwrap();
    g.validate().unwrap();
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("destination_record_grid"));
}

#[test]
fn marker_inheritance_uses_exact_source_time_and_creation_content() {
    let mut g = fixture();
    let video = g.sequences[0].placements[0].id.clone();
    let mut source = g.sequences[0].markers[0].clone();
    source.start = Some(q(3));
    source.owner_local = Some(q(3));
    source.owner_rate = Some(q(24));
    source.owner_occurrence = "native:master".into();
    g.items[0].markers.push(source);
    let m = &mut g.sequences[0].markers[0];
    m.owner = MarkerOwner::Edit;
    m.owner_occurrence = "native:edit".into();
    m.track = Some(graph::TrackRef {
        kind: StreamKind::Video,
        index: 1,
    });
    m.binding.placements = vec![video];
    for object in [&g.items[0].id, &g.sequences[0].id] {
        let sc = g
            .sidecars
            .iter_mut()
            .find(|s| s.adapter == "avid.source" && s.object == *object)
            .unwrap();
        if let Value::Obj(fields) = &mut sc.data {
            fields.push((
                "marker_native".into(),
                Value::Arr(vec![v(vec![
                    ("graph_index", Value::Int(0)),
                    ("created", Value::Int(100)),
                ])]),
            ));
        }
    }
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = &r.model.sequences[0].markers[0];
    assert_eq!(m.source, Some(3 * 254016000000));
    assert!(m.inherited);
    g.items[0].markers[0].start = Some(Q::new(73, 24).unwrap());
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(!r.model.sequences[0].markers[0].inherited);
}

#[test]
fn one_to_many_marker_binding_maps_through_assembly_without_timestamp_guessing() {
    let mut g = fixture();
    let ids = g.sequences[0]
        .placements
        .iter()
        .map(|p| p.id.clone())
        .collect();
    let m = &mut g.sequences[0].markers[0];
    m.owner = MarkerOwner::Edit;
    m.binding.placements = ids;
    m.owner_occurrence = "native:expanded-owner".into();
    m.track = Some(graph::TrackRef {
        kind: StreamKind::Video,
        index: 1,
    });
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(r.model.sequences[0].markers[0].event, Some(0));
    assert_eq!(r.ownership.len(), 3);
}

#[test]
fn unsupported_palette_and_unattached_item_color_have_scoped_losses() {
    let mut g = fixture();
    g.items[0].color_rgb16 = Some([65535, 0, 0]);
    g.sequences[0].markers[0].color = Some(graph::Label {
        palette: "other-source".into(),
        name: "Red".into(),
    });
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(r.losses.iter().any(|l| l.aspect == "marker_color"));
    assert!(r.losses.iter().any(|l| l.aspect == "color"));
}

#[test]
fn edit_marker_without_a_qualified_track_keeps_record_time_as_sequence_marker() {
    let mut g = fixture();
    let id = g.sequences[0].placements[0].id.clone();
    let m = &mut g.sequences[0].markers[0];
    m.owner = MarkerOwner::Edit;
    m.binding.placements = vec![id];
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = &r.model.sequences[0].markers[0];
    assert_eq!(m.owner, "sequence");
    assert_eq!(m.start, 2 * 254016000000);
    assert!(r.losses.iter().any(|l| l.aspect == "marker_ownership"));
}

#[test]
fn overlapping_marker_owners_are_not_resolved_by_first_timestamp() {
    let mut g = fixture();
    let mut second = g.sequences[0].placements[0].clone();
    second.id = "overlapping-owner".into();
    second.video_track = Some(2);
    let first = g.sequences[0].placements[0].id.clone();
    g.sequences[0].placements.push(second);
    let m = &mut g.sequences[0].markers[0];
    m.owner = MarkerOwner::Edit;
    m.binding.placements = vec![first, "overlapping-owner".into()];
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = &r.model.sequences[0].markers[0];
    assert_eq!(m.owner, "sequence");
    assert_eq!(m.event, None);
    assert!(r.losses.iter().any(|l| l.aspect == "marker_ownership"));
}

#[test]
fn spanned_marker_crossing_a_trimmed_owner_keeps_its_complete_record_span() {
    let mut g = two_edits();
    let mut m = fixture().sequences.remove(0).markers.remove(0);
    m.start = Some(Q::new(13, 4).unwrap());
    m.duration = Q::new(1, 2).unwrap();
    m.owner = MarkerOwner::Edit;
    m.track = Some(graph::TrackRef {
        kind: StreamKind::Video,
        index: 1,
    });
    m.binding.placements = vec![g.sequences[0].placements[0].id.clone()];
    g.sequences[0].markers.push(m);
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = &r.model.sequences[0].markers[0];
    assert_eq!(m.owner, "sequence");
    assert_eq!(m.start, 13 * 254016000000 / 4);
    assert_eq!(m.duration, 254016000000 / 2);
}

#[test]
fn incompatible_chain_trims_refuse_unavailable_handles() {
    let mut g = two_edits();
    let s = &mut g.sequences[0];
    let mut third = s.placements[1].clone();
    third.id = "third".into();
    s.placements.push(third);
    let mut t = s.transitions[0].clone();
    t.id = "overlapping-transition".into();
    t.cut = Q::new(13, 4).unwrap();
    t.cut_offset = Q::new(1, 4).unwrap();
    t.duration = q(4);
    t.outgoing.placements = vec!["incoming".into()];
    t.incoming.placements = vec!["third".into()];
    s.transitions.push(t);
    g.validate().unwrap();
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("transition_handles_unavailable"));
}

#[test]
fn native_only_audio_groups_cannot_disappear_in_saved_source_preparation() {
    let mut g = fixture();
    let sc = g
        .sidecars
        .iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == g.items[0].id)
        .unwrap();
    if let Value::Obj(fields) = &mut sc.data {
        fields.push((
            "raw_groups".into(),
            Value::Arr(vec![Value::Str("unqualified native group".into())]),
        ));
    }
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(p.losses.iter().any(|l| l.aspect == "audio_grouping"));
    assert!(prepare(&g, &options(), ClockProfile::VerifiedRewrap)
        .err()
        .unwrap()
        .0
        .contains("unsupported_native_audio_grouping"));
}

#[test]
fn unknown_canvas_stays_unknown_while_explicit_compatibility_raster_prepares() {
    let mut g = fixture();
    g.sequences[0].frame_size = None;
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport)
        .err()
        .unwrap()
        .0
        .contains("unresolved_sequence_geometry"));
    let sc = g
        .sidecars
        .iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == g.sequences[0].id)
        .unwrap();
    if let Value::Obj(fields) = &mut sc.data {
        fields.push((
            "compatibility_frame_size".into(),
            Value::Arr(vec![Value::Int(1920), Value::Int(1080)]),
        ));
    }
    let before = g.to_json_text().unwrap();
    for clock in [ClockProfile::LegacyImport, ClockProfile::VerifiedRewrap] {
        let p = prepare(&g, &options(), clock).unwrap();
        assert_eq!(
            (p.model.sequences[0].width, p.model.sequences[0].height),
            (1920, 1080)
        );
    }
    assert_eq!(g.to_json_text().unwrap(), before);
    assert_eq!(g.sequences[0].frame_size, None);
    g.sequences[0].frame_size = Some((3840, 2160));
    let p = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(
        (p.model.sequences[0].width, p.model.sequences[0].height),
        (1920, 1080)
    );
}

#[test]
fn explicit_surround_declaration_needs_complete_source_members_and_witness() {
    let mut g=fixture();
    let audio=g.items[0].media.iter().find(|m|g.media.iter().any(|p|p.id==m.media && p.kind==StreamKind::Audio)).unwrap().clone();
    let original=g.media.iter().find(|m|m.id==audio.media).unwrap().clone();
    for label in 3..=8 {
        let mut m=original.clone();m.id=format!("extra-audio-{label}");m.stream=Some(label);
        g.media.push(m.clone());let mut binding=audio.clone();binding.media=m.id.clone();g.items[0].media.push(binding);
        g.sidecars.push(Sidecar{adapter:"avid.source".into(),object:m.id,data:v(vec![("legacy_clock",Value::Null),("legacy_file_clock",clock(0,24,false))])});
    }
    let channels=vec![1,3,2,7,8,5,6,4];g.items[0].stream_groups=vec![channels.clone()];
    let sc=g.sidecars.iter_mut().find(|s|s.adapter=="avid.source" && s.object==g.items[0].id).unwrap();
    if let Value::Obj(fields)=&mut sc.data {fields.push(("raw_groups".into(),Value::Arr(vec![Value::Str("71:A1A3A2A7A8A5A6A4".into())])));}
    let p=prepare(&g,&options(),ClockProfile::VerifiedRewrap).unwrap();
    assert_eq!(p.model.clips.values().next().unwrap().multichannel_groups[0].channels,channels);
    let snapshot=g.clone();
    let mut ambiguous=g.clone();ambiguous.items[0].stream_groups[0].reverse();
    assert!(prepare(&ambiguous,&options(),ClockProfile::VerifiedRewrap).is_err());
    assert_eq!(g,snapshot);
    let mut duplicate=g.clone();
    let sc=duplicate.sidecars.iter_mut().find(|s|s.adapter=="avid.source" && s.object==duplicate.items[0].id).unwrap();
    if let Value::Obj(fields)=&mut sc.data {
        if let Some((_,Value::Arr(rows)))=fields.iter_mut().find(|(k,_)|k=="raw_groups") {rows.push(rows[0].clone());}
    }
    assert!(prepare(&duplicate,&options(),ClockProfile::VerifiedRewrap).err().unwrap().0.contains("duplicate native"));
}

#[test]
fn master_output_mix_is_reported_without_changing_routing_or_source() {
    for profile in [ClockProfile::LegacyImport,ClockProfile::VerifiedRewrap] {
        let baseline=prepare(&fixture(),&options(),profile).unwrap();
        for (kind,raw,present) in [
            ("absent",Value::Null,false),("integer",Value::Int(4),true),
            ("integer",Value::Int(5),true),("integer",Value::Int(-7),true),
            ("text",Value::Str("unknown".into()),true),
            ("bytes_hex",Value::Str("0080ff".into()),true),
            ("bin_object_ref",Value::Int(9),true),("null",Value::Null,true),
        ] {
            let mut g=fixture();
            let id=g.items[0].id.clone();
            let sc=g.sidecars.iter_mut().find(|s|s.adapter=="avid.source"&&s.object==id).unwrap();
            let Value::Obj(fields)=&mut sc.data else {panic!()};
            fields.push(("master_output_mix".into(),v(vec![("type",Value::Str(kind.into())),("raw",raw)])));
            let before=g.clone();
            let result=prepare(&g,&options(),profile).unwrap();
            assert_eq!(result.model,baseline.model);
            let notes:Vec<_>=result.losses.iter().filter(|l|l.aspect=="master_output_mix").collect();
            assert_eq!(notes.len(),usize::from(present));
            if present {assert!(notes[0].reason.contains("Avid output mixing is not applied"));}
            assert_eq!(g,before);
        }
        for (kind,raw) in [("integer",Value::Null),("absent",Value::Int(4)),("bytes_hex",Value::Str("0xz".into())),("bin_object_ref",Value::Int(0)),("future",Value::Null)] {
            let mut g=fixture();let id=g.items[0].id.clone();
            let sc=g.sidecars.iter_mut().find(|s|s.adapter=="avid.source"&&s.object==id).unwrap();
            let Value::Obj(fields)=&mut sc.data else {panic!()};
            fields.push(("master_output_mix".into(),v(vec![("type",Value::Str(kind.into())),("raw",raw)])));
            assert!(prepare(&g,&options(),profile).err().unwrap().0.contains("malformed"));
        }
    }
}

#[test]
fn named_aac_and_smpte_ds_prepare_only_with_matching_witnesses() {
    for (count,raw,channels) in [(6,"51:A2A1A3A4A5A6",vec![2,1,3,4,5,6]),(8,"71:A1A3A2A5A6A7A8A4",vec![1,3,2,5,6,7,8,4])] {
        let mut g=fixture();
        let audio=g.items[0].media.iter().find(|m|g.media.iter().any(|p|p.id==m.media && p.kind==StreamKind::Audio)).unwrap().clone();
        let original=g.media.iter().find(|m|m.id==audio.media).unwrap().clone();
        for label in 3..=count {
            let mut m=original.clone();m.id=format!("extra-audio-{label}");m.stream=Some(label);
            g.media.push(m.clone());let mut binding=audio.clone();binding.media=m.id.clone();g.items[0].media.push(binding);
            g.sidecars.push(Sidecar{adapter:"avid.source".into(),object:m.id,data:v(vec![("legacy_clock",Value::Null),("legacy_file_clock",clock(0,24,false))])});
        }
        g.items[0].stream_groups=vec![channels.clone()];
        let sc=g.sidecars.iter_mut().find(|s|s.adapter=="avid.source" && s.object==g.items[0].id).unwrap();
        if let Value::Obj(fields)=&mut sc.data {fields.push(("raw_groups".into(),Value::Arr(vec![Value::Str(raw.into())])));}
        let before=g.clone();
        let prepared=prepare(&g,&options(),ClockProfile::VerifiedRewrap).unwrap();
        assert_eq!(prepared.model.clips.values().next().unwrap().multichannel_groups[0].channels,channels);
        assert_eq!(g,before);
        g.items[0].stream_groups[0].reverse();
        assert!(prepare(&g,&options(),ClockProfile::VerifiedRewrap).is_err());
    }
}

#[test]
fn linked_originals_use_typed_codec_channels_and_keep_legacy_projection_strict(){
    use prproj::convert::prepared::{PreparedClip,SampleRepresentation};
    let mut g=fixture();
    for m in &mut g.media {
        let audio=m.kind==StreamKind::Audio;
        if audio{m.bits=None;m.codec=Some("aac".into());}else{m.codec=Some("avc1".into());}
        m.locations=vec!["/Synthetic/source.mp4".into()];
        let clock=if audio{48000}else{24};
        m.original=Some(OriginalStream{saved_locators:vec!["/Saved/source.mp4".into()],container:"mov".into(),stream_index:if audio{1}else{0},channel_index:if audio{Some(m.stream.unwrap()-1)}else{None},channels:audio.then_some(2),time_base:Q::new(1,clock).unwrap(),start_pts:0,duration_ts:m.duration.to_units(clock as i64).unwrap(),sample_representation:audio.then_some("aac".into())});
    }
    g.validate().unwrap();let before=g.to_json_text().unwrap();
    assert!(prepare(&g,&options(),ClockProfile::LegacyImport).err().unwrap().to_string().contains("typed prepared route"));
    let prepared=premiere_adapter::prepare_originals(&g,&options(),ClockProfile::LegacyImport).unwrap();
    let PreparedClip::Original(c)=&prepared.input.clips[0]else{panic!("typed original missing")};
    assert_eq!(c.audio.as_ref().unwrap().representation,SampleRepresentation::Aac);assert_eq!(c.channels.iter().map(|c|c.channel_index).collect::<Vec<_>>(),vec![0,1]);
    assert_eq!(g.to_json_text().unwrap(),before);assert!(prepared.input.build_native_media_model().unwrap().clips.is_empty());
    g.media[2].original.as_mut().unwrap().channel_index=Some(0);
    assert!(premiere_adapter::prepare_originals(&g,&options(),ClockProfile::LegacyImport).is_err());
}

fn text_fact(id: &str, kind: TextKind, track: i64, start: Q, duration: Q) -> SourceText {
    SourceText { id:id.into(),kind,track,content:Some("Synthetic text café\nSecond line".into()),record_start:Some(start),duration:Some(duration),reason:None }
}
#[test]
fn source_text_roundtrip_preparation_and_scoped_refusals_preserve_source() {
    let mut g=fixture();
    g.sequences[0].texts=vec![text_fact("text:caption",TextKind::Caption,2,Q::integer(2),Q::integer(1)),text_fact("text:title",TextKind::Graphic,4,Q::integer(2),Q::integer(1))];
    let before=g.to_json_text().unwrap();
    assert_eq!(SourceGraph::from_json_text(&before).unwrap(),g);
    let p=prepare(&g,&options(),ClockProfile::LegacyImport).unwrap();
    assert_eq!(p.texts.len(),2);assert_eq!(g.to_json_text().unwrap(),before);
    assert_eq!(p.texts[0].start,2*prproj::convert::model::TICKS_PER_SECOND);assert!(p.texts.iter().all(|t|t.content.contains("café\n")));
    let mut bad=g.clone();bad.sequences[0].texts[0].record_start=None;
    assert!(bad.validate().is_err());
    bad.sequences[0].texts[0].reason=Some("owner clock unresolved".into());
    let p=prepare(&bad,&options(),ClockProfile::LegacyImport).unwrap();assert_eq!(p.texts.len(),1);assert!(p.losses.iter().any(|l|l.object=="text:caption"&&l.reason.contains("clock")));
    bad=g.clone();bad.sequences[0].texts[0].duration=Some(Q::new(1,25).unwrap());
    let p=prepare(&bad,&options(),ClockProfile::LegacyImport).unwrap();assert_eq!(p.texts.len(),1);assert!(p.losses.iter().any(|l|l.object=="text:caption"&&l.reason.contains("bounds")));
    bad=g.clone();let mut t=bad.sequences[0].texts[0].clone();t.id="text:overlap".into();t.track=3;bad.sequences[0].texts.push(t);
    let p=prepare(&bad,&options(),ClockProfile::LegacyImport).unwrap();assert_eq!(p.texts.len(),1);assert!(p.losses.iter().filter(|l|l.reason.contains("overlapping SubCaps")).count()==2);
    bad=g.clone();bad.sequences[0].texts[1].record_start=Some(Q::ZERO);bad.sequences[0].texts[1].track=1;
    let p=prepare(&bad,&options(),ClockProfile::LegacyImport).unwrap();assert_eq!(p.texts.len(),1);assert!(p.losses.iter().any(|l|l.object=="text:title"&&l.reason.contains("nonempty picture")));
    let duplicate=before.replace("\"texts\":[", "\"texts\":[],\"texts\":[");assert!(SourceGraph::from_json_text(&duplicate).is_err());
    let unknown=before.replace("\"kind\":\"caption\"", "\"kind\":\"mystery\"");assert!(SourceGraph::from_json_text(&unknown).is_err());
}

#[test]
fn source_multicam_refuses_before_any_destination_work_instead_of_flattening_silently(){
    let mut g=fixture();g.multicams.push(graph::source::Multicam{id:"unresolved-group".into(),name:"Native group".into(),rate:None,duration:None,lanes:vec![],reason:Some("source clock unresolved".into())});
    g.bins[0].items.push("unresolved-group".into());let before=g.to_json_text().unwrap();
    assert!(prepare(&g,&options(),ClockProfile::LegacyImport).err().unwrap().to_string().contains("multicam_native_route_required"));
    assert!(premiere_adapter::prepare_originals(&g,&options(),ClockProfile::VerifiedRewrap).err().unwrap().to_string().contains("unsupported_editable_multicam"));
    assert_eq!(g.to_json_text().unwrap(),before);
}

fn fixed_audio_group() -> SourceGraph {
    let mut g = fixture();
    let f = |n: i128| Q::new(n * 1001, 24000).unwrap();
    let rate = Q::new(24000, 1001).unwrap();
    g.items[0].rate = Some(rate);
    let mut b = g.items[0].clone();
    b.id = "clip:camera-b".into();
    b.name = "Synthetic Camera B".into();
    b.umid = Some("060A2B340101010501010F101300000000000000000000000000000000000002".into());
    for (i, m) in g.media.iter_mut().enumerate() {
        m.duration = f(60);
        if m.kind == StreamKind::Video {
            m.rate = Some(rate);
        } else {
            m.stream = Some(i as i64);
            m.bits = Some(24);
        }
    }
    for (binding, m) in b.media.iter_mut().zip(g.media.clone()) {
        let mut m = m;
        m.id.push_str(":b");
        m.locations[0] = format!("/Synthetic/{}/B.mxf", m.id);
        binding.media = m.id.clone();
        g.sidecars.push(Sidecar {
            adapter: "avid.source".into(), object: m.id.clone(),
            data: v(vec![("legacy_clock", Value::Null), ("legacy_file_clock", clock(0, 24, false))]),
        });
        g.media.push(m);
    }
    let a_id = g.items[0].id.clone();
    let b_id = b.id.clone();
    g.items.push(b);
    g.bins[0].items.push("synthetic-group".into());
    let gap = |start, duration| MulticamPiece {
        start: f(start), duration: f(duration), item: None, stream: None, source_in: None,
    };
    let content = |item: &str, stream, start| MulticamPiece {
        start: f(start), duration: f(60), item: Some(item.into()), stream: Some(stream), source_in: Some(Q::ZERO),
    };
    let mut lanes = Vec::new();
    for (kind, track) in [(StreamKind::Video, 1), (StreamKind::Audio, 1), (StreamKind::Audio, 2)] {
        let a = MulticamAlternative { id: format!("group:{kind:?}:{track}:a"), pieces: vec![gap(0, 3), content(&a_id, track, 3)], reason: None };
        let b = MulticamAlternative { id: format!("group:{kind:?}:{track}:b"), pieces: vec![content(&b_id, track, 0), gap(60, 3)], reason: None };
        // Audio order intentionally differs from camera order.
        let (selected, alternatives) = if kind == StreamKind::Video {(0, vec![a,b])} else {(1, vec![b,a])};
        lanes.push(MulticamLane { kind, index: track, selected: Some(selected), alternatives, reason: None });
    }
    g.multicams.push(Multicam { id: "synthetic-group".into(), name: "Synthetic Group".into(), rate: Some(rate), duration: Some(f(63)), lanes, reason: None });
    let s = &mut g.sequences[0];
    s.rate = rate;
    s.markers.clear();
    for p in &mut s.placements {
        p.record_start = Q::ZERO;
        p.duration = f(10);
        p.transform = None;
        p.effects.clear();
        let (kind, track) = if p.video_track.is_some() {
            p.item = b_id.clone();
            p.source_in = f(8);
            (StreamKind::Video, 1)
        } else {
            p.source_in = f(5);
            let track = p.audio_tracks[0];
            p.audio_streams = vec![track];
            (StreamKind::Audio, track)
        };
        g.multicam_uses.push(MulticamUse {
            id: format!("use:{kind:?}:{track}"), sequence: s.id.clone(), candidates: vec!["synthetic-group".into()],
            kind, lane: Some(track), track, selected: Some(1),
            record_start: Some(Q::ZERO), duration: Some(f(10)), source_in: Some(f(8)),
            binding: SourceBinding { placements: vec![p.id.clone()], reason: None }, reason: None,
        });
    }
    g
}

#[test]
fn editable_multicam_resolves_independent_audio_order_and_refuses_unsupported_uses() {
    use prproj::convert::multicam::FRAME;
    let g = fixed_audio_group();
    let before = g.to_json_text().unwrap();
    let make = |g| premiere_adapter::prepare_originals(g, &options(), ClockProfile::VerifiedRewrap);
    let p = make(&g).unwrap();
    assert_eq!(p.input.multicams.len(), 1);
    assert_eq!(p.input.multicams[0].cameras[0].start, 3 * FRAME);
    assert_eq!(p.input.multicams[0].cameras[1].clip, "camera-b");
    assert_eq!(p.input.multicam_uses.len(), 2);
    assert!(p.input.multicam_uses.iter().any(|u| u.video == Some(1) && u.audio.is_empty()));
    assert!(p.input.multicam_uses.iter().any(|u| u.video.is_none() && u.audio == [0,1]));
    assert!(p.input.multicam_uses.iter().all(|u| u.source_in == 8 * FRAME));
    let media = p.input.build_native_media_model().unwrap();
    assert_eq!(media.clips.len(), 2);
    assert!(media.sequences.is_empty());
    assert_eq!(before, g.to_json_text().unwrap());

    let mut switched = g.clone();
    switched.multicam_uses.iter_mut().find(|u| u.kind == StreamKind::Audio).unwrap().selected = Some(0);
    assert!(make(&switched).err().unwrap().0.contains("independently selected group audio"));
    let mut missing = g.clone();
    missing.multicam_uses[0].binding.placements.clear();
    assert!(make(&missing).err().unwrap().0.contains("exact bound leaf placement"));
    let mut inconsistent = g.clone();
    inconsistent.multicam_uses[0].source_in = Some(Q::ZERO);
    assert!(make(&inconsistent).err().unwrap().0.contains("contradicts the bound leaf placement"));
    let mut mixed_depth = g.clone();
    mixed_depth.media.iter_mut().find(|m| m.kind == StreamKind::Audio).unwrap().bits = Some(16);
    assert!(make(&mixed_depth).err().unwrap().0.contains("PCM24/48k"));
    assert!(premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).err().unwrap().0.contains("multicam_native_route_required"));
}

#[test]
fn editable_multicam_writes_reusable_source_with_two_output_lanes() {
    use prproj::convert::{multicam::FRAME, rewrap::MediaRelink, writer::prepare_typed_project};
    let g = fixed_audio_group();
    let p = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::VerifiedRewrap).unwrap();
    let media = p.input.build_native_media_model().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "anynle-editable-group-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let out = dir.join("Group.prproj");
    let mut ids = prproj::SeededUuids::new("synthetic-editable-group");
    assert!(prepare_typed_project(&p.input, &out, &mut ids, &[], false).err().unwrap().to_string().contains("multicam_native_route_required"));
    let mut bindings = Vec::new();
    for leaf in media.clips.values() {
        let dst = dir.join(format!("{}.mxf", leaf.id)).to_string_lossy().into_owned();
        bindings.push(MediaRelink { source_path:leaf.video.as_ref().unwrap().path.clone(), target_path:dst.clone(), audio_stream:None });
        for a in &leaf.audio {
            bindings.push(MediaRelink { source_path:a.path.clone(), target_path:dst.clone(), audio_stream:Some((a.avid_track.unwrap()-1) as u32) });
        }
    }
    let ready = prepare_typed_project(&p.input, &out, &mut ids, &bindings, true).unwrap();
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    ready.publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let source = project.sequences().iter().copied().find(|&s| project.sequence_name(s) == "Synthetic Group").unwrap();
    assert_eq!(project.sequence_end(source).unwrap().ticks, 63*FRAME);
    assert_eq!(project.video_tracks(source).len(), 2);
    let audio = project.audio_tracks(source);
    assert_eq!(audio.len(),4);
    assert_eq!(audio.iter().map(|&t| project.is_muted(t).unwrap()).collect::<Vec<_>>(), [false,false,true,true]);
    assert_eq!(project.start(project.track_clips(project.video_tracks(source)[0])[0]).unwrap().ticks, 3*FRAME);
    let tree = &project.doc.tree;
    assert_eq!(tree.iter_tag(project.doc.root,"SelectedTrackIndex").map(|n|tree.text(n).unwrap()).collect::<Vec<_>>(), ["1"]);
    assert_eq!(tree.iter_tag(project.doc.root,"MonoToStereoPanProcessor").count(), 2);
    let panel = project.sequence_item(source).unwrap();
    assert_eq!(project.item_name(panel), "Synthetic Group");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}
