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

#[test]
fn avid_source_color_transform_survives_graph_reload_and_reports_one_affected_clip() {
    let mut graph = fixture();
    let options = parse_model_str(include_str!("../../../examples/opatom_clip.model.json"))
        .unwrap().options;
    let baseline = prepare(&graph, &options, ClockProfile::LegacyImport).unwrap();
    let item_id = graph.items[0].id.clone();
    let native = graph.sidecars.iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == item_id).unwrap();
    let Value::Obj(fields) = &mut native.data else { panic!("native facts must be an object") };
    let text = "<ColorTransformationList><ColorTransformation/></ColorTransformationList>";
    let transforms = Value::Arr(vec![v(vec![
        ("track", Value::Int(1)),
        ("owner_mob", Value::Str("source-mob".into())),
        ("text", Value::Str(text.into())),
    ])]);
    fields.push(("input_color_transforms".into(), transforms.clone()));
    let reloaded = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    assert_eq!(
        reloaded.sidecar("avid.source", &item_id).unwrap().get("input_color_transforms"),
        Some(&transforms),
    );
    let prepared = prepare(&reloaded, &options, ClockProfile::LegacyImport).unwrap();
    assert_eq!(baseline.model, prepared.model);
    let losses: Vec<_> = prepared.losses.iter()
        .filter(|l| l.aspect == "source_color_transform").collect();
    let [loss] = losses.as_slice() else { panic!("expected one loss for the affected clip") };
    assert_eq!(loss.object, item_id);
}
fn q(n: i64) -> Q {
    Q::integer(n)
}
fn options() -> Options {
    Options::default()
}

#[test]
fn direct_dnxhr_needs_the_native_codec_clock_and_discrete_siblings() {
    use prproj::convert::{
        model::{AudioCrossfade, Fraction, TICKS_PER_SECOND},
        writer::prepare_typed_project,
    };

    let mut g = fixture();
    g.sequences.clear();
    g.sidecars.retain(|s| g.media.iter().any(|m| m.id == s.object)
        || g.items.iter().any(|item| item.id == s.object));
    let rate = Q::new(24000, 1001).unwrap();
    let duration = Q::new(3003, 2000).unwrap(); // 36 pictures or 72,072 samples
    let start = Q::new(1001, 20).unwrap(); // frame 1,200 at the exact file rate
    g.items[0].rate = Some(rate);
    for (i, m) in g.media.iter_mut().enumerate() {
        let picture = m.kind == StreamKind::Video;
        let lane = m.stream;
        m.online = Some(true);
        m.duration = duration;
        m.start = Some(start);
        m.clock = Some(SourceClock {
            effective_count: 1200,
            rate,
            display_base: 24,
            drop: false,
            provenance: SourceClockProvenance {
                origin_count: 1198,
                source_offset: if picture { 2 } else { 4004 },
                source_rate: if picture { rate } else { q(48000) },
                source_package: "native source package".into(),
                timecode_track: format!("native timecode track {i}"),
                route: "file_source_clip_to_physical_timecode_track".into(),
            },
        });
        let descriptor = if picture {
            m.rate = Some(rate);
            v(vec![("compression", Value::Str("RI00".into())),
                   ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)]))])
        } else {
            assert!(matches!(lane, Some(1 | 2)));
            m.bits = Some(24);
            v(vec![("channels", Value::Int(1))])
        };
        let facts = g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(1198, 24, false)),
            ("file_package", Value::Str(format!("file package {i}"))),
            ("descriptor_length", Value::Int(if picture { 36 } else { 72072 })),
            ("descriptor", descriptor),
            ("picture_coding", if picture { Value::Str("060E2B340401010D0401020271280000".into()) } else { Value::Null }),
        ]);
    }
    let separate = Options { bin_mode: "separate".into(), ..options() };
    let prepared = prepare(&g, &separate, ClockProfile::LegacyImport).unwrap();
    let clip = prepared.model.clips.values().next().unwrap();
    let exact_ticks = start.to_units(TICKS_PER_SECOND).unwrap();
    let origin = Q::new(1198 * 1001, 24000).unwrap().to_units(TICKS_PER_SECOND).unwrap();
    assert_eq!(prepared.direct_dnxhr_origins.get(&clip.id), Some(&origin));
    assert_eq!(clip.video.as_ref().unwrap().start, exact_ticks);
    assert!(clip.audio.iter().all(|a| a.start == exact_ticks));
    let dir = std::env::temp_dir().join(format!("anynle-direct-dnxhr-{}-{}",
        std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let unit = TICKS_PER_SECOND / 24_000 * 1_001;
    let mut timeline = parse_model_str(include_str!("../../../examples/opatom_clip.model.json"))
        .unwrap().sequences.remove(0);
    timeline.rate = Fraction::new(24_000, 1_001);
    timeline.markers.clear();
    let first = &mut timeline.events[0];
    first.clip = clip.id.clone();
    first.source_in = 4 * unit;
    first.duration = 10 * unit;
    let mut second = first.clone();
    second.record_start = 10 * unit;
    second.source_in = 6 * unit;
    timeline.events.push(second);
    timeline.audio_crossfades.push(AudioCrossfade {
        track: 2,
        cut: 10 * unit,
        duration: 4 * unit,
        cut_offset: 2 * unit,
        style: "constant_power".into(),
    });
    for mode in ["separate", "grouped", "merged"] {
        let options = Options { bin_mode: mode.into(), ..options() };
        let mut input = premiere_adapter::prepare_originals(&g, &options, ClockProfile::LegacyImport).unwrap().input;
        assert_eq!(input.direct_dnxhr_origins, prepared.direct_dnxhr_origins);
        input.sequences.push(timeline.clone());
        let out = dir.join(format!("{mode}.prproj"));
        let mut ids = prproj::SeededUuids::new("synthetic-direct-dnxhr");
        let ready = prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap();
        assert!(!out.exists());
        ready.publish().unwrap();
        let mut doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert!(doc.tree.iter_tag(doc.root, "CodecType").any(|n| doc.tree.text(n) == Some("1096180840")));
        assert!(doc.tree.iter_tag(doc.root, "TimecodeFormat").any(|n| doc.tree.text(n) == Some("110")));
        assert_eq!(doc.tree.iter_tag(doc.root, "AudioTransitionTrackItem").count(), 1);
        let media_starts: Vec<_> = doc.tree.iter_tag(doc.root, "Media")
            .filter_map(|n| doc.tree.child_by_tag(n, "Start"))
            .map(|n| doc.tree.text(n).unwrap().parse::<i64>().unwrap())
            .collect();
        assert_eq!(media_starts, [origin; 3]);
        let modification_states: Vec<_> = doc.tree.iter_tag(doc.root, "Media")
            .filter_map(|n| doc.tree.child_by_tag(n, "ModificationState"))
            .collect();
        assert_eq!(modification_states.len(), 3);
        for state in modification_states {
            assert!(doc.payload(state).unwrap().is_some_and(|bytes| !bytes.is_empty()));
        }
        if mode == "grouped" {
            assert_eq!(doc.tree.iter_tag(doc.root, "MonoToStereoPanProcessor").count(), 2);
        }
        assert!(doc.tree.iter_tag(doc.root, "MediaInPoint")
            .any(|n| doc.tree.text(n).and_then(|v| v.parse::<i64>().ok()) == Some(exact_ticks)));
    }
    let mut fades = fixture();
    let mut sequence = fades.sequences.remove(0);
    sequence.rate = rate;
    sequence.markers.clear();
    let mut outgoing = sequence.placements.into_iter().find(|p| p.audio_tracks == [1]).unwrap();
    let frame = Q::new(1001, 24000).unwrap();
    outgoing.duration = Q::new(14 * 1001, 24000).unwrap();
    outgoing.source_in = frame;
    let mut incoming = outgoing.clone();
    incoming.id = "incoming-audio".into();
    incoming.record_start = Q::new(8 * 1001, 24000).unwrap();
    incoming.source_in = frame;
    sequence.transitions.push(SourceTransition {
        id: "transition:linear".into(),
        kind: StreamKind::Audio,
        track: 1,
        cut: Q::new(10 * 1001, 24000).unwrap(),
        duration: Q::new(6 * 1001, 24000).unwrap(),
        cut_offset: Q::new(2 * 1001, 24000).unwrap(),
        style: "avid.BLEND_AUDIO_DISSOLVE".into(),
        outgoing: SourceBinding { placements: vec![outgoing.id.clone()], reason: None },
        incoming: SourceBinding { placements: vec![incoming.id.clone()], reason: None },
    });
    sequence.placements = vec![outgoing, incoming];
    let sequence_id = sequence.id.clone();
    g.sequences.push(sequence);
    for (attribute, parameter, mode, expected) in [
        (Value::Int(0), Value::Int(0), "separate", "constant_gain"),
        (Value::Int(0), Value::Int(1), "separate", "constant_power"),
        (Value::Null, Value::Int(0), "separate", "constant_power"),
        (Value::Int(0), Value::Int(0), "merged", "constant_power"),
    ] {
        g.sidecars.retain(|s| s.object != sequence_id);
        g.sidecars.push(Sidecar {
            adapter: "avid.source".into(),
            object: sequence_id.clone(),
            data: v(vec![("compatibility_start", Value::Str("3600".into())), ("native_transitions", Value::Arr(vec![v(vec![
                ("id", Value::Str("transition:linear".into())),
                ("effect", Value::Str("BLEND_AUDIO_DISSOLVE".into())),
                ("audio_curve_attribute", attribute),
                ("audio_curve_parameter", parameter),
            ])]))]),
        });
        let options = Options { bin_mode: mode.into(), ..options() };
        let saved = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
        let prepared = prepare(&saved, &options, ClockProfile::LegacyImport).unwrap();
        assert_eq!(prepared.model.sequences[0].audio_crossfades[0].style, expected);
        if expected == "constant_gain" {
            let out = dir.join("linear.prproj");
            let mut ids = prproj::SeededUuids::new("synthetic-direct-dnxhr");
            let mut input = premiere_adapter::prepare_originals(&saved, &options, ClockProfile::LegacyImport).unwrap().input;
            let ready = prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap();
            ready.publish().unwrap();
            let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(out).unwrap()).unwrap();
            let fade = doc.tree.iter_tag(doc.root, "AudioTransitionTrackItem").next().unwrap();
            assert_eq!(doc.tree.findtext(fade, "TransitionTrackItem/MatchName").unwrap().as_deref(), Some("Constant Gain"));
            assert_eq!(doc.tree.findtext(fade, "FadeShapeValue").unwrap().as_deref(), Some("0"));
            let rejected = dir.join("linear-missing-handle.prproj");
            input.sequences[0].events[1].source_in = unit;
            let err = prepare_typed_project(&input, &rejected, &mut ids, &[], false).err().unwrap();
            assert!(err.to_string().contains("source handles"));
            assert!(!rejected.exists());
        }
    }
    g.sequences.clear();
    g.sidecars.retain(|s| s.object != sequence_id);
    let picture = g.media.iter().find(|m| m.kind == StreamKind::Video).unwrap();
    let facts = g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == picture.id).unwrap();
    if let Value::Obj(fields) = &mut facts.data {
        fields.retain(|(name, _)| name != "picture_coding");
    }
    for mode in ["separate", "grouped", "merged"] {
        let options = Options { bin_mode: mode.into(), ..options() };
        let input = premiere_adapter::prepare_originals(&g, &options, ClockProfile::LegacyImport).unwrap().input;
        assert!(input.direct_dnxhr_origins.is_empty());
        let refused = dir.join(format!("unqualified-{mode}.prproj"));
        let mut ids = prproj::SeededUuids::new("synthetic-direct-dnxhr");
        let error = prepare_typed_project(&input, &refused, &mut ids, &[], false).err().unwrap();
        assert!(error.to_string().contains("no Premiere-observed media timecode format"));
        assert!(!refused.exists());
    }
}

#[test]
fn direct_23976_dnxhd_pcm16_preserves_two_file_clocks_and_sample_types() {
    use prproj::convert::{model::TICKS_PER_SECOND, writer::prepare_typed_project};

    let mut g = fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let frame = |n: i128| Q::new(n * 1001, 24_000).unwrap();
    let ticks = |n| frame(n).to_units(TICKS_PER_SECOND).unwrap();
    g.items[0].rate = Some(rate);
    let make_clock = |count, source_rate| SourceClock {
        effective_count: count,
        rate,
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: count,
            source_offset: 0,
            source_rate,
            source_package: "synthetic physical package".into(),
            timecode_track: "TC1/synthetic".into(),
            route: "file_source_clip_to_physical_timecode_track".into(),
        },
    };
    for (i, m) in g.media.iter_mut().enumerate() {
        let picture = m.kind == StreamKind::Video;
        m.online = Some(true);
        m.start = Some(frame(172_800));
        m.duration = frame(96);
        if picture { m.rate = Some(rate); } else { m.bits = Some(16); }
        m.clock = Some(make_clock(172_800, if picture { rate } else { q(48_000) }));
        let facts = g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(172_800, 24, false)),
            ("file_package", Value::Str(format!("file {i}"))),
            ("descriptor_length", Value::Int(if picture { 96 } else { 192_192 })),
            ("descriptor", if picture {
                v(vec![("compression", Value::Str("AVHD".into())), ("resolution_id", Value::Int(1253)),
                    ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)]))])
            } else { v(vec![("channels", Value::Int(1)), ("bits", Value::Int(16))]) }),
            ("picture_coding", if picture { Value::Str("060E2B340401010A0401020271130000".into()) } else { Value::Null }),
        ]);
    }
    let s = &mut g.sequences[0];
    s.rate = rate;
    s.start = Some(frame(86_400));
    s.clock = Some(make_clock(86_400, rate));
    s.markers.clear();
    for p in &mut s.placements {
        p.record_start = Q::ZERO;
        p.source_in = frame(5);
        p.duration = frame(24);
    }
    let opts = Options { bin_mode: "separate".into(), ..options() };
    let g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let input = premiere_adapter::prepare_originals(&g, &opts, ClockProfile::LegacyImport).unwrap().input;
    assert_eq!(input.sequences[0].start, ticks(86_400));
    assert!(input.sequences[0].events.iter().all(|e| e.source_in == ticks(5)));
    let out = std::env::temp_dir().join(format!("anynle-direct-pcm16-{}-{}.prproj", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut ids = prproj::SeededUuids::new("synthetic-direct-pcm16");
    prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert!(doc.tree.iter_tag(doc.root, "CodecType").any(|n| doc.tree.text(n) == Some("1096180846")));
    assert!(doc.tree.iter_tag(doc.root, "TimecodeFormat").any(|n| doc.tree.text(n) == Some("110")));
    assert_eq!(doc.tree.iter_tag(doc.root, "SampleType").filter(|&n| doc.tree.text(n) == Some("3")).count(), 2);
    assert_eq!(doc.tree.iter_tag(doc.root, "Media").filter_map(|n| doc.tree.child_by_tag(n, "Start"))
        .filter_map(|n| doc.tree.text(n).and_then(|s| s.parse::<i64>().ok())).collect::<Vec<_>>(), [ticks(172_800); 3]);

    let mut mixed = g.clone();
    mixed.media.iter_mut().find(|m| m.kind == StreamKind::Audio).unwrap().bits = Some(24);
    for (graph, mode) in [(&g, "grouped"), (&g, "merged"), (&mixed, "separate")] {
        let options = Options { bin_mode: mode.into(), ..opts.clone() };
        let input = premiere_adapter::prepare_originals(graph, &options, ClockProfile::LegacyImport).unwrap().input;
        assert!(input.direct_dnxhr_origins.is_empty());
        assert!(prepare_typed_project(&input, &out, &mut ids, &[], false).is_err());
        assert!(!out.exists());
    }

    let mut ranged = g.clone();
    let parent = ranged.items[0].umid.as_ref().unwrap().clone();
    let native_id = "060A2B340101010501010F101300000000000000000000000000000000000002";
    let to_urn = |id: &str| format!("urn:smpte:umid:{}", id.as_bytes().chunks(8)
        .map(|s| std::str::from_utf8(s).unwrap().to_ascii_lowercase()).collect::<Vec<_>>().join("."));
    let track = |kind, label| v(vec![
        ("label", Value::Int(label)), ("kind", Value::Int(kind)),
        ("component_class", Value::Str("SCLP".into())),
        ("rate", Value::Str("24000/1001".into())), ("length", Value::Int(96)),
        ("target_mob", Value::Str(parent.clone())),
        ("target_track", Value::Int(label)), ("target_offset", Value::Int(0)),
    ]);
    let observed = |n| v(vec![("value", Value::Int(n))]);
    ranged.sidecars.push(Sidecar {
        adapter: "avid.subclips".into(), object: ranged.bins[0].id.clone(),
        data: Value::Arr(vec![v(vec![
            ("id", Value::Str("synthetic-subclip".into())), ("mob", Value::Str(native_id.into())),
            ("name", Value::Str("Reusable Subclip".into())), ("rate", Value::Str("24000/1001".into())),
            ("backing_length", Value::Int(96)), ("start", observed(0)), ("end", observed(96)),
            ("tracks", Value::Arr(vec![track(1, 1), track(2, 1), track(2, 2)])),
        ])]),
    });
    for placement in &ranged.sequences[0].placements {
        let kind = if placement.video_track.is_some() { 1 } else { 2 };
        let label = placement.audio_streams.first().copied().unwrap_or(1);
        let hop = |id, offset, length| v(vec![
            ("mob", Value::Str(to_urn(id))), ("track", Value::Int(label)),
            ("rate", Value::Str("24000/1001".into())),
            ("source_offset", observed(offset)), ("length", observed(length)),
        ]);
        ranged.sidecars.push(Sidecar {
            adapter: "avid.source".into(), object: placement.id.clone(),
            data: v(vec![("native_kind", Value::Int(kind)),
                ("route", Value::Arr(vec![hop(native_id, 5, 24), hop(&parent, 0, 96)]))]),
        });
    }
    let prepare = |graph| premiere_adapter::prepare_originals(graph, &opts, ClockProfile::LegacyImport).unwrap();
    let ranged = SourceGraph::from_json_text(&ranged.to_json_text().unwrap()).unwrap();
    let ready = prepare(&ranged);
    assert_eq!(ready.input.av_subclips.len(), 1);
    assert_eq!(ready.input.av_subclip_uses.len(), 3);
    prepare_typed_project(&ready.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    let tree = &doc.tree;
    let master = tree.iter_tag(doc.root, "MasterClip").find(|&m| {
        tree.findtext(m, "Name").unwrap().as_deref() == Some("Reusable Subclip")
    }).unwrap();
    assert_eq!(tree.findall(master, "Clips/Clip").unwrap().len(), 3);
    assert_eq!(tree.iter_tag(doc.root, "SubClip").filter(|&c| {
        tree.child_by_tag(c, "MasterClip").is_some_and(|r| doc.resolve(r) == Ok(master))
    }).count(), 3);

    let shorten = |graph: &mut SourceGraph, start, end| {
        let receipt = graph.sidecars.iter_mut().find(|s| s.adapter == "avid.subclips").unwrap();
        let Value::Arr(rows) = &mut receipt.data else { panic!("missing receipt rows") };
        let Value::Obj(fields) = &mut rows[0] else { panic!("missing receipt fields") };
        for (field, value) in fields {
            match field.as_str() {
                "start" => *value = observed(start),
                "end" => *value = observed(end),
                _ => {},
            }
        }
    };
    let mut shortened = ranged.clone();
    shorten(&mut shortened, 5, 29);
    let short = prepare(&shortened);
    assert_eq!((short.input.av_subclips[0].start, short.input.av_subclips[0].end), (ticks(5), ticks(29)));
    assert_eq!(short.input.av_subclip_uses.len(), 3);
    prepare_typed_project(&short.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    for (tag, value) in [("StartBoundary", ticks(5)), ("EndBoundary", ticks(29))] {
        let expected = value.to_string();
        assert_eq!(doc.tree.iter_tag(doc.root, tag)
            .filter(|&n| doc.tree.text(n) == Some(expected.as_str())).count(), 3);
    }

    let mut pcm24 = shortened.clone();
    for m in &mut pcm24.media {
        let facts = pcm24.sidecars.iter_mut()
            .find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        let Value::Obj(fields) = &mut facts.data else { panic!("missing media facts") };
        let mut set = |name, value| {
            fields.iter_mut().find(|(k, _)| k == name).unwrap().1 = value;
        };
        if m.kind == StreamKind::Video {
            set("descriptor", v(vec![
                ("compression", Value::Str("RI00".into())),
                ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)])),
            ]));
            set("picture_coding", Value::Str("060E2B340401010D0401020271280000".into()));
        } else {
            m.bits = Some(24);
            set("descriptor", v(vec![("channels", Value::Int(1)), ("bits", Value::Int(24))]));
        }
    }
    let pcm24_graph = SourceGraph::from_json_text(&pcm24.to_json_text().unwrap()).unwrap();
    let pcm24 = prepare(&pcm24_graph);
    assert_eq!((pcm24.input.av_subclips[0].start, pcm24.input.av_subclips[0].end), (ticks(5), ticks(29)));
    assert_eq!(pcm24.input.av_subclip_uses.len(), 3);
    prepare_typed_project(&pcm24.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    std::fs::remove_file(&out).unwrap();

    let mut nested = pcm24_graph.clone();
    nested.sidecars.retain(|s| s.adapter != "avid.subclips");
    let picture = nested.sequences[0].placements.iter_mut().find(|p| p.video_track.is_some()).unwrap();
    picture.record_start = frame(8);
    let picture_id = picture.id.clone();
    let receipt = nested.sidecars.iter_mut().find(|s| s.object == picture_id && s.adapter == "avid.source").unwrap();
    receipt.data = v(vec![
        ("plain_submaster", Value::Bool(true)),
        ("via", Value::Arr(vec![Value::Str("effect EFF_SUBMASTER not converted".into())])),
    ]);
    let nested = SourceGraph::from_json_text(&nested.to_json_text().unwrap()).unwrap();
    let make_nest = prepare(&nested);
    let [nest] = make_nest.input.picture_nests.as_slice() else { panic!("missing editable nest") };
    assert_eq!(make_nest.input.sequences[nest.sequence].events[nest.event].record_start, ticks(8));
    prepare_typed_project(&make_nest.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    let tree = &doc.tree;
    let child = tree.iter_tag(doc.root, "Sequence")
        .find(|&n| tree.findtext(n, "Name").unwrap().unwrap_or_default().contains("Submaster")).unwrap();
    assert_eq!(tree.findtext(child, "Node/Properties/MZ.ZeroPoint").unwrap(), Some(ticks(86_408).to_string()));
    let child_sources = tree.iter_tag(doc.root, "VideoSequenceSource").filter(|&n| {
        tree.find1(n, "SequenceSource/Sequence").is_some_and(|r| doc.resolve(r) == Ok(child))
    }).collect::<Vec<_>>();
    let [source] = child_sources.as_slice() else { panic!("missing child picture source") };
    assert_eq!(tree.findtext(*source, "OriginalDuration").unwrap(), Some(ticks(24).to_string()));
    assert!(tree.iter_tag(doc.root, "VideoClip").any(|n| {
        tree.find1(n, "Clip/Source").is_some_and(|r| doc.resolve(r) == Ok(*source))
            && tree.findtext(n, "Clip/InPoint").unwrap().as_deref() == Some("0")
            && tree.findtext(n, "Clip/OutPoint").unwrap() == Some(ticks(24).to_string())
    }));

    let mut malformed_nest = make_nest.input;
    let mut outside = malformed_nest.picture_nests[0];
    outside.event = usize::MAX;
    malformed_nest.picture_nests[0] = outside;
    assert!(prepare_typed_project(&malformed_nest, &out, &mut ids, &[], false).is_err());
    assert!(!out.exists());
    let mut unproven = nested.clone();
    let receipt = unproven.sidecars.iter_mut().find(|s| s.object == picture_id && s.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut receipt.data else { panic!("missing Submaster fields") };
    fields.retain(|(name, _)| name != "plain_submaster");
    let fallback = prepare(&unproven);
    assert!(fallback.input.picture_nests.is_empty());
    assert!(fallback.losses.iter().any(|l| l.object == picture_id && l.aspect == "picture_nest"));

    let mut mixed_writer_input = pcm24.input;
    let prproj::convert::prepared::PreparedClip::Legacy(parent) = &mut mixed_writer_input.clips[0]
        else { panic!("missing legacy backing master") };
    parent.audio[1].bits = 16;
    assert!(prepare_typed_project(&mixed_writer_input, &out, &mut ids, &[], false).is_err());
    assert!(!out.exists());

    let mut outside = shortened.clone();
    shorten(&mut outside, 6, 29);
    let outside = prepare(&outside);
    assert_eq!(outside.input.av_subclips.len(), 1);
    assert!(outside.input.av_subclip_uses.is_empty());
    assert_eq!(outside.losses.iter().filter(|l| l.aspect == "subclip_timeline_source").count(), 3);

    let mut ambiguous = ranged.clone();
    let receipt = ambiguous.sidecars.iter_mut().find(|s| s.adapter == "avid.subclips").unwrap();
    let Value::Arr(rows) = &mut receipt.data else { panic!("missing receipt rows") };
    let mut duplicate = rows[0].clone();
    let Value::Obj(fields) = &mut duplicate else { panic!("missing receipt fields") };
    *fields.iter_mut().find(|(k, _)| k == "id").map(|(_, value)| value).unwrap() = Value::Str("duplicate-mob".into());
    rows.push(duplicate);
    let result = prepare(&ambiguous);
    assert_eq!(result.input.av_subclips.len(), 2);
    assert!(result.input.av_subclip_uses.is_empty());
    assert_eq!(result.losses.iter().filter(|l| l.aspect == "subclip_timeline_source").count(), 3);

    let mut malformed = ready.input;
    malformed.av_subclip_uses[0].subclip = usize::MAX;
    assert!(prepare_typed_project(&malformed, &out, &mut ids, &[], false).err().unwrap()
        .to_string().contains("unknown prepared A/V subclip"));
    assert!(!out.exists());
}

#[test]
fn direct_dnxhd_drop_frame_requires_the_saved_profile_and_sequence_clock() {
    use prproj::convert::{model::TICKS_PER_SECOND, writer::prepare_typed_project};
    let mut g = fixture();
    let rate = Q::new(30_000, 1001).unwrap();
    let frame = |n: i128| Q::new(n * 1001, 30_000).unwrap();
    g.items[0].rate = Some(rate);
    let make_clock = |count, source_rate, route: &str, track: &str| SourceClock {
        effective_count: count,
        rate,
        display_base: 30,
        drop: true,
        provenance: SourceClockProvenance {
            origin_count: count,
            source_offset: 0,
            source_rate,
            source_package: "synthetic package".into(),
            timecode_track: track.into(),
            route: route.into(),
        },
    };
    for (i, m) in g.media.iter_mut().enumerate() {
        let picture = m.kind == StreamKind::Video;
        m.online = Some(true);
        m.start = Some(frame(215_784));
        m.duration = frame(150);
        if picture { m.rate = Some(rate); } else { m.bits = Some(24); }
        m.clock = Some(make_clock(215_784, if picture { rate } else { q(48_000) },
            "file_source_clip_to_physical_timecode_track", "physical TC"));
        let facts = g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(215_784, 30, true)),
            ("file_package", Value::Str(format!("file {i}"))),
            ("descriptor_length", Value::Int(if picture { 150 } else { 240_240 })),
            ("descriptor", if picture {
                v(vec![("compression", Value::Str("AVHD".into())),
                    ("resolution_id", Value::Int(1253)),
                    ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)]))])
            } else { v(vec![("channels", Value::Int(1))]) }),
            ("picture_coding", if picture { Value::Str("060E2B340401010A0401020271130000".into()) } else { Value::Null }),
        ]);
    }
    let s = &mut g.sequences[0];
    s.rate = rate;
    s.start = Some(frame(1795));
    s.clock = Some(make_clock(1795, rate, "direct sequence TC1", "TC1/synthetic"));
    s.markers.clear();
    for p in &mut s.placements {
        p.record_start = Q::ZERO;
        p.source_in = frame(10);
        p.duration = frame(5);
    }
    let separate = Options { bin_mode: "separate".into(), ..options() };
    let saved = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let input = premiere_adapter::prepare_originals(&saved, &separate, ClockProfile::LegacyImport).unwrap().input;
    let ticks = |n| frame(n).to_units(TICKS_PER_SECOND).unwrap();
    assert_eq!(input.sequences[0].start, ticks(1795));
    assert!(input.sequences[0].events.iter().all(|e| e.source_in == ticks(10)));
    let out = std::env::temp_dir().join(format!("anynle-direct-dnxhd-{}-{}.prproj",
        std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut ids = prproj::SeededUuids::new("synthetic-direct-dnxhd");
    prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert!(doc.tree.iter_tag(doc.root, "CodecType").any(|n| doc.tree.text(n) == Some("1096180846")));
    assert!(!doc.tree.iter_tag(doc.root, "CodecType").any(|n| doc.tree.text(n) == Some("1096180840")));
    assert!(doc.tree.iter_tag(doc.root, "TimecodeFormat").any(|n| doc.tree.text(n) == Some("102")));
    assert_eq!(doc.tree.iter_tag(doc.root, "Media").filter_map(|n| doc.tree.child_by_tag(n, "Start"))
        .filter_map(|n| doc.tree.text(n).and_then(|s| s.parse::<i64>().ok())).collect::<Vec<_>>(), [ticks(215_784); 3]);

    for mode in ["grouped", "merged"] {
        let options = Options { bin_mode: mode.into(), ..options() };
        let input = premiere_adapter::prepare_originals(&saved, &options, ClockProfile::LegacyImport).unwrap().input;
        assert!(input.direct_dnxhr_origins.is_empty());
        assert!(prepare_typed_project(&input, &out, &mut ids, &[], false).is_err());
        assert!(!out.exists());
    }
    let mut unknown = saved.clone();
    unknown.sequences[0].clock = None;
    assert!(premiere_adapter::prepare_originals(&unknown, &separate, ClockProfile::LegacyImport)
        .err().unwrap().to_string().contains("direct_dnxhd_sequence_clock"));
    let mut wrong_coding = saved;
    let picture = wrong_coding.media.iter().find(|m| m.kind == StreamKind::Video).unwrap();
    let facts = wrong_coding.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == picture.id).unwrap();
    let Value::Obj(fields) = &mut facts.data else { unreachable!() };
    fields.iter_mut().find(|(name, _)| name == "picture_coding").unwrap().1 =
        Value::Str("060E2B340401010D0401020271280000".into());
    let input = premiere_adapter::prepare_originals(&wrong_coding, &separate, ClockProfile::LegacyImport).unwrap().input;
    assert!(input.direct_dnxhr_origins.is_empty());
    assert!(prepare_typed_project(&input, &out, &mut ids, &[], false).is_err());
    assert!(!out.exists());
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
fn exact_tc1_changes_only_the_absolute_sequence_origin() {
    let mut g = fixture();
    let rate = Q::new(24000, 1001).unwrap();
    let origin = Q::new(18018, 5).unwrap();
    g.sequences[0].rate = rate;
    let at_record_rate = |t: Q| Q::new(t.num() as i128 * 1001, t.den() as i128 * 1000).unwrap();
    for p in &mut g.sequences[0].placements {
        p.record_start = at_record_rate(p.record_start);
        p.duration = at_record_rate(p.duration);
    }
    let id = &g.sequences[0].id;
    g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && &s.object == id)
        .unwrap().data = v(vec![("compatibility_start", Value::Str("3600".into()))]);
    let baseline = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let mut expected = baseline.model.sequences[0].clone();
    assert_eq!(expected.start, 914_457_600_000_000);
    expected.start = 915_372_057_600_000;

    let seq = &mut g.sequences[0];
    seq.start = Some(origin);
    seq.clock = Some(SourceClock {
        effective_count: 86400,
        rate,
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: 86400,
            source_offset: 0,
            source_rate: rate,
            source_package: "sequence".into(),
            timecode_track: "TC1/0".into(),
            route: "direct sequence TC1".into(),
        },
    });
    let corrected = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(corrected.model.sequences[0], expected);
    assert_eq!(corrected.ownership, baseline.ownership);
    assert!(corrected.diagnostics.iter().any(|s| s.contains("qualified 23.976 clock")));

    // A start without its qualified clock must never select corrected timing.
    g.sequences[0].clock = None;
    let fallback = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(fallback.model.sequences, baseline.model.sequences);
}

#[test]
fn avid_drop_frame_tc1_uses_the_saved_frame_count() {
    let mut g = fixture();
    let rate = Q::new(30_000, 1001).unwrap();
    let seq = &mut g.sequences[0];
    seq.rate = rate;
    // Use frame-aligned record ranges at 29.97 without changing source trims.
    let record = |time: Q| Q::new(time.num() as i128 * 1001, time.den() as i128 * 1000).unwrap();
    for edit in &mut seq.placements {
        edit.record_start = record(edit.record_start);
        edit.duration = record(edit.duration);
    }
    let sequence_id = seq.id.clone();
    g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == sequence_id)
        .unwrap().data = v(vec![("compatibility_start", Value::Str("0".into()))]);
    let baseline = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let mut expected = baseline.model.sequences[0].clone();
    assert_eq!(expected.start, 0);

    let seq = &mut g.sequences[0];
    let count = 1795; // 00:00:59;25, five actual frames before 00:01:00;02.
    let clock = SourceClock {
        effective_count: count,
        rate,
        display_base: 30,
        drop: true,
        provenance: SourceClockProvenance {
            origin_count: count,
            source_offset: 0,
            source_rate: rate,
            source_package: "sequence".into(),
            timecode_track: "TC1/0".into(),
            route: "direct sequence TC1".into(),
        },
    };
    seq.start = clock.exact_start();
    seq.clock = Some(clock);
    let saved = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let corrected = prepare(&saved, &options(), ClockProfile::LegacyImport).unwrap();
    expected.start = 15_213_822_624_000;
    assert_eq!(corrected.model.sequences[0], expected);
    assert_eq!(corrected.ownership, baseline.ownership);
    assert!(corrected.diagnostics.iter().any(|s| s.contains("qualified 29.97 DF clock")));

    g.sequences[0].clock.as_mut().unwrap().drop = false;
    let fallback = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(fallback.model.sequences, baseline.model.sequences);
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
fn corner_pin_round_trips_points_and_rejects_crossed_corners() {
    use prproj::convert::prepared::{CornerPin, PictureEffect};
    let mut g = fixture();
    let corners = [[-0.001, 0.1], [0.8, 0.05], [0.95, 0.8], [0.15, 0.95]];
    let effect = |corners: [[f64; 2]; 4]| graph::Effect {
        kind: "picture.corner_pin".into(),
        params: v(vec![("target_corners_tl_tr_br_bl", Value::Arr(
            corners.map(|point| Value::Arr(point.map(Value::Float).into())).into(),
        ))]),
    };
    let edit = &mut g.sequences[0].placements[0];
    assert!(edit.video_track.is_some());
    edit.effects = vec![effect(corners)];
    let id = edit.id.clone();
    let reloaded = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&reloaded, &options(), ClockProfile::LegacyImport).unwrap();
    let [pin] = prepared.picture_effects.as_slice() else { panic!("expected one Corner Pin") };
    assert_eq!((pin.sequence, pin.event), prepared.ownership[&id]);
    assert_eq!(pin.effect, PictureEffect::CornerPin(CornerPin { corners, source_crop: [0.0; 4] }));

    let source_crop = [0.1, 0.08, 0.15, 0.12];
    let mut cropped = effect(corners);
    let Value::Obj(params) = &mut cropped.params else { unreachable!() };
    params.push(("source_crop_ltrb".into(), Value::Arr(source_crop.map(Value::Float).into())));
    g.sequences[0].placements[0].effects = vec![cropped];
    let reloaded = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&reloaded, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(prepared.picture_effects[0].effect, PictureEffect::CornerPin(CornerPin { corners, source_crop }));
    // A reversed component list still parses in Premiere but crops the warped result.
    use prproj::convert::writer::{prepare_project_with_effects, Editorial};
    let dir = std::env::temp_dir().join(format!("anynle-crop-pin-{}-{}",
        std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("corner-pin.prproj");
    let editorial = Editorial {
        texts: &prepared.texts,
        video_dissolves: &prepared.video_dissolves,
        picture_effects: &prepared.picture_effects,
        direct_dnxhr_origins: &prepared.direct_dnxhr_origins,
        constant_speeds: &prepared.constant_speeds,
    };
    let mut ids = prproj::SeededUuids::new("static-crop-pin");
    prepare_project_with_effects(&prepared.model, editorial, &path, &mut ids, &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(path).unwrap()).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    let item = doc.tree.iter_tag(doc.root, "VideoClipTrackItem").next().unwrap();
    let chain = doc.resolve(doc.tree.find1(item, "ClipTrackItem/ComponentOwner/Components").unwrap()).unwrap();
    let names: Vec<_> = doc.tree.findall(chain, "ComponentChain/Components/Component").unwrap().into_iter()
        .map(|reference| doc.tree.findtext(doc.resolve(reference).unwrap(), "MatchName").unwrap().unwrap())
        .collect();
    assert_eq!(names, ["AE.ADBE Corner Pin", "AE.Impact_Crop_FX"]);

    let mut crossed = corners;
    crossed.swap(2, 3);
    g.sequences[0].placements[0].effects = vec![effect(crossed)];
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).err().unwrap().to_string()
        .contains("invalid Corner Pin points"));
    let mut invalid_crop = effect(corners);
    let Value::Obj(params) = &mut invalid_crop.params else { unreachable!() };
    params.push(("source_crop_ltrb".into(), Value::Arr([0.6, 0.0, 0.5, 0.0].map(Value::Float).into())));
    g.sequences[0].placements[0].effects = vec![invalid_crop];
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).err().unwrap().to_string()
        .contains("invalid Corner Pin points or source crop"));
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

    let reflection = graph::picture_chain::Reflection { horizontal: true, vertical: false }.effect();
    let mut shared = g.clone();
    let placement = shared.sequences[0].placements.iter_mut().find(|p| p.id == id).unwrap();
    placement.effects = vec![reflection];
    let shared_prepared = prepare(&shared, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(shared_prepared.picture_effects[0].effect, effect.effect);
    shared.sequences[0].placements.iter_mut().find(|p| p.id == id).unwrap().transform =
        Some(graph::Transform {
            position: (0.05, 0.0), scale: (100.0, 100.0), rotation: 0.0,
            anchor: (0.0, 0.0), crop: [0.0; 4],
        });
    let combined = prepare(&shared, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(combined.picture_effects.is_empty());
    assert!(combined.losses.iter().any(|loss| loss.object == id && loss.aspect == "picture_effect"));

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
fn ordered_picture_chain_projects_reflection_on_the_correct_side_of_motion() {
    use prproj::convert::prepared::PictureEffect;
    let mut g = fixture();
    let flip = v(vec![
        ("kind", Value::Str("avid.flip".into())),
        ("params", v(vec![("horizontal", Value::Bool(true)), ("vertical", Value::Bool(false))])),
    ]);
    let pair = |a, b| Value::Arr(vec![Value::Float(a), Value::Float(b)]);
    let motion = v(vec![
        ("kind", Value::Str("avid.warp_static".into())),
        ("params", v(vec![])),
        ("transform", v(vec![
            ("position", pair(0.05, -0.02)),
            ("scale", pair(90.0, 110.0)),
            ("anchor", pair(0.0, 0.0)),
            ("rotation", Value::Float(0.0)),
            ("crop", Value::Arr([0.01, 0.02, 0.04, 0.03].into_iter().map(Value::Float).collect())),
        ])),
    ]);
    let id = g.sequences[0].placements.iter().find(|p| p.video_track.is_some()).unwrap().id.clone();
    for (stages, x, edges) in [
        (vec![flip.clone(), motion.clone()], 0.05, [0.01, 0.02, 0.04, 0.03]),
        (vec![motion.clone(), flip.clone()], -0.05, [0.04, 0.02, 0.01, 0.03]),
    ] {
        let p = g.sequences[0].placements.iter_mut().find(|p| p.id == id).unwrap();
        p.effects = vec![graph::Effect {
            kind: graph::picture_chain::KIND.into(),
            params: v(vec![("stages", Value::Arr(stages))]),
        }];
        let prepared = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
        let [effect] = prepared.picture_effects.as_slice() else { panic!("expected one chain") };
        assert_eq!(effect.effect, PictureEffect::StaticMotion);
        assert_eq!(effect.reflection_before_motion, [true, false]);
        let transform = prepared.model.sequences[effect.sequence].events[effect.event].transform.as_ref().unwrap();
        assert_eq!(transform.position, (x, -0.02));
        assert_eq!(transform.crop, edges);
    }
    let p = g.sequences[0].placements.iter_mut().find(|p| p.id == id).unwrap();
    p.effects[0].params = v(vec![("stages", Value::Arr(vec![flip, motion.clone(), motion]))]);
    let prepared = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(prepared.picture_effects.is_empty());
    assert!(prepared.losses.iter().any(|l| l.object == id && l.reason.contains("more than one Motion")));
    let (seq, event) = prepared.ownership[&id];
    assert!(prepared.model.sequences[seq].events[event].transform.is_none());
}

#[test]
fn animated_crop_keeps_four_edges_on_the_source_frame_clock() {
    use prproj::convert::prepared::PictureEffect;
    let mut g = fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let media = g.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap();
    media.rate = Some(rate);
    media.duration = Q::new(240 * 1001, 24_000).unwrap();
    let s = &mut g.sequences[0];
    s.rate = rate;
    s.placements.retain(|p| p.video_track.is_some());
    let p = &mut s.placements[0];
    p.source_in = Q::new(24 * 1001, 24_000).unwrap();
    p.duration = Q::new(48 * 1001, 24_000).unwrap();
    p.transform = Some(graph::Transform {
        position: (0.0, 0.0), scale: (100.0, 100.0),
        rotation: 0.0, anchor: (0.0, 0.0), crop: [0.0; 4],
    });
    let key = |frame, crop: &[f64]| v(vec![
        ("frame", Value::Int(frame)),
        ("position", Value::Arr(vec![Value::Int(0), Value::Int(0)])),
        ("scale", Value::Arr(vec![Value::Int(125), Value::Int(90)])),
        ("crop", Value::Arr(crop.iter().copied().map(Value::Float).collect())),
    ]);
    let keys = |end: &[f64]| v(vec![("keys", Value::Arr(vec![key(0, &[0.0; 4]), key(47, end)]))]);
    p.effects = vec![graph::Effect {
        kind: "avid.warp_crop_linear".into(),
        params: keys(&[0.1, 0.2, 0.3, 0.4]),
    }];
    let prepared = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let PictureEffect::Resize(motion) = prepared.picture_effects[0].effect else {
        panic!("expected Linear Motion");
    };
    assert_eq!(motion.map(|k| k.ticks), [254270016000, 752215464000]);
    assert_eq!(motion.map(|k| k.crop), [Some([0.0; 4]), Some([0.1, 0.2, 0.3, 0.4])]);

    let p = &mut g.sequences[0].placements[0];
    p.effects[0].params = keys(&[0.1, 0.2, 0.3]);
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).is_err());

    g.sequences[0].placements[0].effects[0].params = keys(&[0.6, 0.2, 0.5, 0.4]);
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).is_err());

    let s = &mut g.sequences[0];
    s.placements[0].effects[0].params = keys(&[0.1, 0.2, 0.3, 0.4]);
    s.frame_size = Some((1280, 720));
    let refused = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(refused.picture_effects.is_empty());
    assert!(refused.model.sequences[0].events[0].transform.is_none());
}

#[test]
fn warp_interpolation_uses_the_exact_source_key_times() {
    use prproj::convert::prepared::{PictureEffect, WarpRollInterpolation};
    let mut g = fixture();
    let sequence = &mut g.sequences[0];
    sequence.placements.retain(|p| p.video_track.is_some());
    let p = &mut sequence.placements[0];
    p.source_in = Q::new(2, 24).unwrap();
    p.duration = Q::new(13, 24).unwrap();
    p.transform = None;
    let key = |frame, x, width, rotation| v(vec![
        ("frame", Value::Int(frame)), ("x", Value::Float(x)),
        ("width", Value::Float(width)), ("rotation", Value::Float(rotation)),
    ]);
    p.effects = vec![graph::Effect {
        kind: "avid.warp_roll_shelf".into(),
        params: v(vec![("keys", Value::Arr(vec![
            key(0, -60.0 / 999.0, 70.0, -15.0),
            key(5, 90.0 / 999.0, 100.0, 25.0),
            key(12, -30.0 / 999.0, 80.0, -10.0),
        ]))]),
    }];
    let reopened = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&reopened, &options(), ClockProfile::LegacyImport).unwrap();
    let [picture] = prepared.picture_effects.as_slice() else { panic!("expected one Warp") };
    let PictureEffect::WarpRoll { keys, interpolation: WarpRollInterpolation::Hold } = picture.effect else { panic!("expected Hold keys") };
    let frame = prproj::convert::model::TICKS_PER_SECOND / 24;
    assert_eq!(keys.map(|k| k.ticks), [2 * frame, 7 * frame, 14 * frame]);
    for (kind, interpolation) in [
        ("avid.warp_roll_linear", WarpRollInterpolation::Linear),
        ("avid.warp_roll_bezier", WarpRollInterpolation::FlatBezier),
        ("avid.warp_roll_spline", WarpRollInterpolation::FlatBezier),
    ] {
        g.sequences[0].placements[0].effects[0].kind = kind.into();
        let prepared = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
        assert_eq!(prepared.picture_effects[0].effect, PictureEffect::WarpRoll { keys, interpolation });
    }
}

#[test]
fn fixed_warp_width_keeps_its_independent_source_handles() {
    use prproj::convert::prepared::PictureEffect;
    let mut g = fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let time = |frames: i128| Q::new(frames * 1001, 24_000).unwrap();
    let media = g.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap();
    media.rate = Some(rate);
    media.duration = time(120);
    let seq = &mut g.sequences[0];
    seq.rate = rate;
    seq.placements.retain(|p| p.video_track.is_some());
    let p = &mut seq.placements[0];
    p.source_in = time(36);
    p.duration = time(36);
    p.transform = Some(graph::Transform {
        position: (0.0, 0.0), scale: (80.0, 100.0), rotation: 0.0,
        anchor: (0.0, 0.0), crop: [0.0; 4],
    });
    let params = |motion_times: [i64; 3], width: [i64; 3], fixed| {
        let motion = motion_times.into_iter().map(|frame| v(vec![
            ("frame", Value::Int(frame)),
            ("position", Value::Arr(vec![Value::Int(0), Value::Int(0)])),
            ("height", Value::Int(100)),
        ])).collect();
        let width = width.into_iter().zip([80, 160, 135]).map(|(frame, value)|
            Value::Arr(vec![Value::Int(frame), Value::Int(value)])).collect();
        v(vec![("motion", Value::Arr(motion)), ("width", Value::Arr(width)), ("fixed_width", Value::Bool(fixed))])
    };
    p.effects.push(graph::Effect {
        kind: "avid.warp_trim_linear".into(),
        params: params([0, 18, 35], [-12, 11, 35], true),
    });
    let result = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let PictureEffect::TrimmedWarp(trim) = result.picture_effects[0].effect else {
        panic!("expected trimmed Warp");
    };
    let unit = prproj::convert::model::TICKS_PER_SECOND / 24_000 * 1001;
    assert_eq!(trim.motion.map(|k| k.ticks), [36 * unit, 54 * unit, 71 * unit]);
    assert_eq!(trim.width, [(24 * unit, 80.0), (47 * unit, 160.0), (71 * unit, 135.0)]);

    g.sequences[0].placements[0].effects[0].params = params([0, 18, 35], [-12, 11, 35], false);
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).is_err());

    g.sequences[0].placements[0].effects[0].params = params([0, 18, 35], [-37, 11, 35], true);
    let refused = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(refused.picture_effects.is_empty());
    assert!(refused.model.sequences[0].events[0].transform.is_none());

    g.sequences[0].placements[0].effects[0].params = params([0, 18, 35], [0, 23, 84], true);
    let refused = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(refused.picture_effects.is_empty());
    assert!(refused.model.sequences[0].events[0].transform.is_none());

    let edit = &mut g.sequences[0].placements[0];
    edit.source_in = time(32);
    edit.duration = time(24);
    edit.effects[0] = graph::Effect {
        kind: "avid.resize_trim_linear".into(),
        params: params([-6, 7, 23], [-23, -1, 23], true),
    };
    let result = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let PictureEffect::TrimmedResize(trim) = result.picture_effects[0].effect else {
        panic!("expected trimmed Resize");
    };
    assert_eq!(trim.motion.map(|k| k.ticks), [26 * unit, 39 * unit, 55 * unit]);
    assert_eq!(trim.width, [(9 * unit, 80.0), (31 * unit, 160.0), (55 * unit, 135.0)]);

    let edit = &mut g.sequences[0].placements[0];
    edit.source_in = time(8);
    edit.effects[0].params = params([0, 15, 31], [0, 18, 23], true);
    g.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap().duration = time(39);
    let refused = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(refused.picture_effects.is_empty());
    assert!(refused.model.sequences[0].events[0].transform.is_none());

    let crop = |times: [i64; 3], middle: [f64; 4]| {
        let mut effect = params([0, 15, 31], [0, 18, 23], true);
        let Value::Obj(ref mut fields) = effect else { unreachable!() };
        fields.push(("crop".into(), Value::Arr(times.into_iter().zip([
            [0.0; 4], middle, [0.1, 0.2, 0.3, 0.4],
        ]).map(|(frame, edges)| v(vec![
            ("frame", Value::Int(frame)),
            ("edges", Value::Arr(edges.into_iter().map(Value::Float).collect())),
        ])).collect())));
        effect
    };
    g.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap().duration = time(60);
    g.sequences[0].placements[0].effects[0] = graph::Effect {
        kind: "avid.resize_trim_crop_linear".into(),
        params: crop([0, 18, 51], [0.02; 4]),
    };
    let result = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let PictureEffect::TrimmedResize(trim) = result.picture_effects[0].effect else {
        panic!("expected trimmed Resize with crop");
    };
    assert_eq!(trim.crop.unwrap().map(|(t, _)| t), [8 * unit, 26 * unit, 59 * unit]);

    g.sequences[0].placements[0].effects[0].params = crop([0, 18, 52], [0.02; 4]);
    let refused = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(refused.picture_effects.is_empty());
    assert!(refused.model.sequences[0].events[0].transform.is_none());

    g.sequences[0].placements[0].effects[0].params = crop([0, 18, 51], [0.7; 4]);
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).is_err());
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
fn resolve_sequence_marker_palette_survives_graph_reload() {
    let mut graph = fixture();
    let template = graph.sequences[0].markers[0].clone();
    let colors = [
        ("Green", "Green", Some(0)),
        ("Red", "Red", Some(1)),
        ("Blue", "Blue", Some(6)),
        ("Cyan", "Cyan", Some(7)),
        ("Yellow", "Yellow", Some(4)),
        ("Lavender", "Magenta", Some(2)),
        ("Sand", "Orange", Some(3)),
        ("Cream", "White", Some(5)),
        ("Pink", "", None),
    ];
    graph.sequences[0].markers = colors.iter().enumerate().map(|(i, (source, _, _))| {
        let start = Q::new(i as i128, 24).unwrap();
        SourceMarker {
            name: "Same name".into(),
            comment: "One\nDeux café".into(),
            color: Some(graph::Label { palette: "resolve".into(), name: (*source).into() }),
            start: Some(start),
            owner_local: Some(start),
            owner: MarkerOwner::Sequence,
            owner_occurrence: format!("test:sequence/marker/{i}"),
            duration: Q::new(1, 24).unwrap(),
            track: None,
            source: None,
            ..template.clone()
        }
    }).collect();
    let saved = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    for input in [&graph, &saved] {
        let result = premiere_adapter::prepare_resolve_originals(input, &options()).unwrap();
        let markers = &result.input.sequences[0].markers;
        assert_eq!(markers.len(), colors.len());
        let frame = prproj::convert::model::TICKS_PER_SECOND / 24;
        for (i, (marker, &(_, name, index))) in markers.iter().zip(&colors).enumerate() {
            assert_eq!(marker.color, name);
            assert_eq!(prproj::convert::colors::marker_color(&marker.color, &[]), index);
            assert_eq!(marker.start, i as i64 * frame);
            assert_eq!(marker.duration, frame);
            assert_eq!(marker.comment, "One\nDeux café");
        }
        let losses: Vec<_> = result.losses.iter().filter(|l| l.aspect == "marker_color").collect();
        assert_eq!(losses.len(), 4);
        for (i, entry) in (5..9).zip(losses) {
            assert!(entry.reason.starts_with(&format!("test:sequence/marker/{i}:")));
        }
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
    source.track = Some(graph::TrackRef { kind: StreamKind::Video, index: 1 });
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
                    ("kind", Value::Int(1)),
                    ("track", Value::Int(1)),
                ])]),
            ));
        }
    }
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = &r.model.sequences[0].markers[0];
    assert_eq!(m.source, Some(3 * 254016000000));
    assert!(m.inherited);
    g.items[0].markers[0].track = None;
    let saved = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let r = prepare(&saved, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(r.model.clips["clip-1"].markers[0].track.as_deref(), Some("V1"));
    assert!(r.model.sequences[0].markers[0].inherited);
    g.items[0].markers[0].track = Some(graph::TrackRef { kind: StreamKind::Video, index: 1 });

    // An edit-local range change must not be discarded as an inherited copy.
    g.sequences[0].markers[0].duration = Q::new(1, 24).unwrap();
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(!r.model.sequences[0].markers[0].inherited);
    g.sequences[0].markers[0].duration = g.items[0].markers[0].duration;

    // Matching content and time on a different source lane are independent.
    let saved_track = g.items[0].markers[0].track;
    g.items[0].markers[0].track = Some(graph::TrackRef { kind: StreamKind::Audio, index: 1 });
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(!r.model.sequences[0].markers[0].inherited);
    g.items[0].markers[0].track = saved_track;

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

fn original_fixture() -> SourceGraph {
    let mut g=fixture();
    for m in &mut g.media {
        let audio=m.kind==StreamKind::Audio;
        if audio{m.bits=None;m.codec=Some("aac".into());}else{m.codec=Some("avc1".into());}
        m.locations=vec!["/Synthetic/source.mp4".into()];
        let clock=if audio{48000}else{24};
        m.original=Some(OriginalStream{saved_locators:vec!["/Saved/source.mp4".into()],container:"mov".into(),stream_index:if audio{1}else{0},channel_index:if audio{Some(m.stream.unwrap()-1)}else{None},channels:audio.then_some(2),time_base:Q::new(1,clock).unwrap(),start_pts:0,duration_ts:m.duration.to_units(clock as i64).unwrap(),sample_representation:audio.then_some("aac".into())});
    }
    g
}

#[test]
fn linked_originals_use_typed_codec_channels_and_keep_legacy_projection_strict() {
    use prproj::convert::prepared::{PreparedClip, SampleRepresentation};
    let mut g = original_fixture();
    g.validate().unwrap();let before=g.to_json_text().unwrap();
    assert!(prepare(&g,&options(),ClockProfile::LegacyImport).err().unwrap().to_string().contains("typed prepared route"));
    for profile in [ClockProfile::LegacyImport, ClockProfile::VerifiedRewrap] {
        let prepared = premiere_adapter::prepare_originals(&g, &options(), profile).unwrap();
        let PreparedClip::Original(c) = &prepared.input.clips[0] else {
            panic!("typed original missing")
        };
        assert_eq!(c.audio.as_ref().unwrap().representation, SampleRepresentation::Aac);
        assert_eq!(c.channels.iter().map(|c| c.channel_index).collect::<Vec<_>>(), vec![0, 1]);
        assert!(prepared.input.build_native_media_model().unwrap().clips.is_empty());
    }
    assert_eq!(g.to_json_text().unwrap(), before);
    g.media[2].original.as_mut().unwrap().channel_index=Some(0);
    assert!(premiere_adapter::prepare_originals(&g,&options(),ClockProfile::LegacyImport).is_err());
}

#[test]
fn linked_mono_wav_keeps_source_gain_without_changing_the_older_edit() {
    use prproj::convert::prepared::PreparedClip;

    let mut g = original_fixture();
    g.media.retain(|m| m.kind == StreamKind::Audio && m.stream == Some(1));
    let m = &mut g.media[0];
    m.codec = Some("pcm_s24le".into());
    m.bits = Some(24);
    m.online = Some(true);
    m.locations = vec!["/Synthetic/source.wav".into()];
    let original = m.original.as_mut().unwrap();
    original.saved_locators = m.locations.clone();
    original.container = "wav".into();
    original.stream_index = 0;
    original.channels = Some(1);
    original.sample_representation = Some("pcm24".into());

    g.items[0].media.retain(|a| a.media == m.id);
    g.items[0].media[0].gain_db = 3.0;
    g.items[0].rate = Some(q(24));
    let item = g.items[0].id.clone();
    let mob = "urn:smpte:umid:060a2b34.01010101.01010f00.13000000.00000001.00000002.00000003.00000004";
    g.sidecars.retain(|sc| !(sc.object == item && sc.adapter == "avid"));
    g.sidecars.push(Sidecar {
        adapter: "avid".into(), object: item, data: v(vec![
            ("mob_id", Value::Str(mob.into())),
            ("columns", v(vec![])),
            ("start_timecode", Value::Null),
            ("aux_timecodes", Value::Arr(vec![])),
            ("source_file", Value::Null),
            ("source_path", Value::Null),
        ]),
    });

    let seq = &mut g.sequences[0];
    seq.rate = q(24);
    seq.frame_size = Some((1920, 1080));
    seq.markers.clear();
    let mut dry = seq.placements.iter().find(|p| p.audio_tracks == [1]).unwrap().clone();
    dry.record_start = q(0);
    dry.source_in = q(2);
    dry.duration = q(2);
    dry.gain_db = 0.0;
    dry.volume_db = None;
    dry.volume_keys.clear();
    dry.track_gains_db.clear();
    dry.track_pans.clear();
    dry.speed = None;
    dry.frame_map = None;
    let receipt = |p: &graph::Placement, gain: Value, native_mob: &str| v(vec![
        ("id", Value::Str(p.id.clone())),
        ("placement_id", Value::Str(p.id.clone())),
        ("master", Value::Str(native_mob.into())),
        ("kind", Value::Int(2)),
        ("track", Value::Int(1)),
        ("master_track", Value::Int(1)),
        ("combined", Value::Null),
        ("edit_gain_db", gain),
        ("edit_pan", Value::Null),
        ("reason", Value::Null),
        ("via", Value::Arr(vec![])),
        ("controls", v(vec![("speed", Value::Null)])),
        ("range", v(vec![
            ("record_rate", Value::Str("24".into())),
            ("source_rate", Value::Str("24".into())),
            ("record_start", Value::Int(p.record_start.to_units(24).unwrap())),
            ("source_in", Value::Int(48)),
            ("record_length", Value::Int(48)),
            ("source_length", Value::Int(48)),
            ("route", Value::Arr(vec![v(vec![
                ("mob", Value::Str(native_mob.into())),
                ("track", Value::Int(1)),
                ("rate", Value::Str("24".into())),
                ("source_offset", v(vec![("value", Value::Int(48))])),
                ("length", v(vec![("value", Value::Int(48))])),
            ])])),
        ])),
    ]);
    let mut copied = dry.clone();
    copied.id = "copied-gain".into();
    copied.record_start = q(2);
    copied.track_gains_db = vec![Some(3.0)];
    let mut mismatched = dry.clone();
    mismatched.id = "other-master".into();
    mismatched.record_start = q(4);
    let events = vec![
        receipt(&dry, Value::Null, mob),
        receipt(&copied, Value::Float(3.0), mob),
        receipt(&mismatched, Value::Null, "another-master"),
    ];
    seq.placements = vec![dry, copied, mismatched];
    let sc = g.sidecars.iter_mut().find(|sc| sc.object == seq.id && sc.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut sc.data else { unreachable!() };
    fields.push(("native_events".into(), Value::Arr(events)));
    let seq = &g.sequences[0];
    g.sidecars.retain(|sc| sc.object == seq.id || sc.object == g.items[0].id
        || sc.object == g.media[0].id || seq.placements.iter().any(|p| p.id == sc.object));
    g.losses.clear();

    let json = g.to_json_text().unwrap();
    let saved = SourceGraph::from_json_text(&json).unwrap();
    for g in [&g, &saved] {
        let prepared = premiere_adapter::prepare_originals(g, &options(), ClockProfile::LegacyImport).unwrap();
        let PreparedClip::Original(master) = &prepared.input.clips[0] else { panic!("expected a file-backed master") };
        assert_eq!(master.channels[0].gain_db, 3.0);
        let events = &prepared.input.sequences[0].events;
        assert_eq!(events[0].audio_gains, vec![Some(0.0)]);
        assert_eq!(events[1].audio_gains, vec![Some(3.0)]);
        assert!(events[2].audio_gains.is_empty(), "contradictory receipt does not override gain");
    }
}

#[test]
fn source_stereo_is_kept_for_fresh_edits_without_merging_existing_mono_edits() {
    use prproj::convert::prepared::PreparedClip;
    let mut g = original_fixture();
    let item = g.items[0].id.clone();
    let witness = |pan| {
        let track = |label, pan| v(vec![("kind", Value::Int(2)), ("label", Value::Int(label)), ("pan_raw", pan)]);
        v(vec![
            ("drop", Value::Bool(false)),
            ("raw_groups", Value::Arr(vec![])),
            ("master_output_mix", v(vec![("type", Value::Str("absent".into())), ("raw", Value::Null)])),
            ("tracks", Value::Arr(vec![track(1, pan), track(2, Value::Null)])),
        ])
    };
    let set_witness = |g: &mut SourceGraph, data| {
        g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == item).unwrap().data = data;
    };
    let groups = |g: &SourceGraph| {
        let prepared = premiere_adapter::prepare_originals(g, &options(), ClockProfile::VerifiedRewrap).unwrap();
        let PreparedClip::Original(clip) = &prepared.input.clips[0] else {
            panic!("typed original missing")
        };
        clip.groups.clone()
    };
    set_witness(&mut g, witness(Value::Null));
    assert_eq!(groups(&g), vec![vec![1], vec![2]]);

    let sequences: Vec<_> = g.sequences.drain(..).map(|s| s.id).collect();
    g.sidecars.retain(|sidecar| !sequences.contains(&sidecar.object));
    let before = g.clone();
    assert_eq!(groups(&g), vec![vec![1, 2]]);
    assert_eq!(g, before);

    set_witness(&mut g, witness(Value::Int(0)));
    assert_eq!(groups(&g), vec![vec![1], vec![2]]);
    set_witness(&mut g, witness(Value::Null));
    g.items[0].stream_groups = vec![vec![2, 1]];
    assert_eq!(groups(&g), vec![vec![2, 1]]);
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
    use prproj::convert::multicam::PreparedGroupAudio::Group;
    assert!(p.input.multicam_uses.iter().any(|u| u.video.is_none() && u.audio == [Group(0), Group(1)]));
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
    assert!(premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).err().unwrap().0.contains("two original mono PCM24 siblings"));
}

#[test]
fn linked_editable_multicam_uses_the_six_original_streams() {
    use prproj::convert::{multicam::FRAME, writer::prepare_typed_project};

    let mut g = fixed_audio_group();
    let rate = Q::new(24000, 1001).unwrap();
    for (i, m) in g.media.iter_mut().enumerate() {
        let picture = m.kind == StreamKind::Video;
        m.online = Some(true);
        m.start = Some(Q::ZERO);
        m.clock = Some(SourceClock {
            effective_count: 0,
            rate,
            display_base: 24,
            drop: false,
            provenance: SourceClockProvenance {
                origin_count: 0,
                source_offset: 0,
                source_rate: if picture { rate } else { q(48000) },
                source_package: format!("physical package {i}"),
                timecode_track: format!("TC1/{i}"),
                route: "file_source_clip_to_physical_timecode_track".into(),
            },
        });
        let descriptor = if picture {
            v(vec![
                ("compression", Value::Str("RI00".into())),
                ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)])),
            ])
        } else {
            v(vec![("channels", Value::Int(1))])
        };
        let facts = g.sidecars.iter_mut()
            .find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(0, 24, false)),
            ("file_package", Value::Str(format!("file package {i}"))),
            ("descriptor_length", Value::Int(if picture { 60 } else { 120120 })),
            ("descriptor", descriptor),
            ("picture_coding", if picture { Value::Str("060E2B340401010D0401020271280000".into()) } else { Value::Null }),
        ]);
    }
    let g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let prepare_linked = |g: &SourceGraph, options: &Options| {
        premiere_adapter::prepare_originals(g, options, ClockProfile::LegacyImport)
    };
    let input = prepare_linked(&g, &options()).unwrap().input;
    assert_eq!(input.direct_dnxhr_origins.len(), 2);
    let merged = Options { bin_mode: "merged".into(), ..options() };
    assert!(prepare_linked(&g, &merged).err().unwrap().0.contains("Separate files"));
    let mut missing = g.clone();
    missing.media[5].bits = Some(16);
    assert!(prepare_linked(&missing, &options()).err().unwrap().0.contains("two original mono PCM24 siblings"));

    let dir = std::env::temp_dir().join(format!("anynle-linked-group-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let out = dir.join("Linked Group.prproj");
    let mut ids = prproj::SeededUuids::new("synthetic-linked-group");
    let ready = prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap();
    assert!(!out.exists());
    ready.publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let tree = &project.doc.tree;
    let paths: std::collections::HashSet<_> = tree.iter_tag(project.doc.root, "ActualMediaFilePath")
        .map(|n| tree.text(n).unwrap()).collect();
    assert_eq!(paths, g.media.iter().map(|m| m.locations[0].as_str()).collect());
    let source = project.sequences().iter().copied()
        .find(|&s| project.sequence_name(s) == "Synthetic Group").unwrap();
    assert_eq!(project.sequence_end(source).unwrap().ticks, 63 * FRAME);
    let audio = project.audio_tracks(source);
    assert_eq!(audio.iter().map(|&t| project.is_muted(t).unwrap()).collect::<Vec<_>>(), [false,false,true,true]);
    assert_eq!(tree.iter_tag(project.doc.root, "MonoToStereoPanProcessor").count(), 2);
    assert_eq!(tree.iter_tag(project.doc.root, "SelectedTrackIndex")
        .map(|n| tree.text(n).unwrap()).collect::<Vec<_>>(), ["1"]);

    let mut ranged = g.clone();
    let parent = ranged.items.iter().find(|i| i.id == "clip:camera-b").unwrap()
        .umid.as_ref().unwrap().clone();
    let range = |n: i64, start, end| v(vec![
        ("id", Value::Str(format!("range:{n}"))),
        ("mob", Value::Str(format!("{n:064X}"))),
        ("name", Value::Str("Same range name".into())),
        ("rate", Value::Str("24000/1001".into())),
        ("backing_length", Value::Int(60)),
        ("start", v(vec![("value", Value::Int(start))])),
        ("end", v(vec![("value", Value::Int(end))])),
        ("tracks", Value::Arr(vec![v(vec![
            ("label", Value::Int(1)),
            ("kind", Value::Int(1)),
            ("component_class", Value::Str("SCLP".into())),
            ("rate", Value::Str("24000/1001".into())),
            ("length", Value::Int(60)),
            ("target_mob", Value::Str(parent.clone())),
            ("target_track", Value::Int(1)),
            ("target_offset", Value::Int(0)),
        ])])),
    ]);
    ranged.sidecars.push(Sidecar {
        adapter: "avid.subclips".into(), object: ranged.bins[0].id.clone(),
        data: Value::Arr(vec![range(1, 5, 13), range(2, 17, 29), range(3, 17, 61)]),
    });
    let ranged = SourceGraph::from_json_text(&ranged.to_json_text().unwrap()).unwrap();
    let prepared = prepare_linked(&ranged, &options()).unwrap();
    assert_eq!(prepared.input.picture_subclips.len(), 2);
    assert!(prepared.losses.iter().any(|loss| loss.object == "range:3"
        && loss.aspect == "subclip_bin_item" && loss.reason.contains("outside the usable backing range")));
    let out = dir.join("Picture Subclips.prproj");
    prepare_typed_project(&prepared.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let ranges: Vec<_> = project.walk(project.root_item().unwrap()).into_iter()
        .filter(|&item| project.item_name(item) == "Same range name")
        .map(|item| (
            project.media_element(item).unwrap().unwrap(),
            project.subclip_in_point(item).unwrap().unwrap().ticks,
            project.subclip_out_point(item).unwrap().unwrap().ticks,
        )).collect();
    assert_eq!(ranges.len(), 2);
    let mut bounds: Vec<_> = ranges.iter().map(|&(_, a, b)| (a, b)).collect();
    bounds.sort_unstable();
    assert_eq!(bounds, [(5 * FRAME, 13 * FRAME), (17 * FRAME, 29 * FRAME)]);
    assert_eq!(ranges[0].0, ranges[1].0);

    use prproj::convert::multicam::PreparedGroupAudio::{Group, LinkedAlternate};
    let mut switched = g.clone();
    let alternate = switched.multicam_uses.iter_mut()
        .find(|u| u.kind == StreamKind::Audio && u.track == 1).unwrap();
    alternate.selected = Some(0); // Audio's alternate ordering differs from picture.
    let occurrence = alternate.id.clone();
    let binding = alternate.binding.placements[0].clone();
    assert!(prepare_linked(&switched, &options()).err().unwrap().0.contains("contradicts the bound leaf"));
    let placement = switched.sequences[0].placements.iter_mut()
        .find(|p| p.id == binding).unwrap();
    placement.item = "clip:camera-b".into();
    placement.source_in = Q::new(8 * 1001, 24000).unwrap();
    let prepared = prepare_linked(&switched, &options()).unwrap();
    assert!(prepared.losses.iter().any(|l| l.object == occurrence && l.aspect == "group_audio_editability"));
    assert!(prepared.input.multicam_uses.iter().any(|u| u.audio.contains(&LinkedAlternate(0))));
    assert!(prepared.input.multicam_uses.iter().any(|u| u.audio.contains(&Group(1))));
    let out = dir.join("Alternate Audio.prproj");
    prepare_typed_project(&prepared.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let sequence = project.sequences().into_iter()
        .find(|&s| project.sequence_name(s) == switched.sequences[0].name).unwrap();
    let audio = project.audio_tracks(sequence);
    let a1 = project.track(audio[0]).clips[0];
    let a2 = project.track(audio[1]).clips[0];
    let a1_source = project.track_item_source(a1).unwrap();
    let a2_source = project.track_item_source(a2).unwrap();
    let camera_b = switched.items.iter().find(|item| item.id == "clip:camera-b").unwrap();
    let alternate_path = switched.media.iter().find(|media| {
        media.kind == StreamKind::Audio && media.stream == Some(1)
            && camera_b.media.iter().any(|binding| binding.media == media.id)
    }).unwrap().locations[0].as_str();
    assert_eq!(project.item_media_path(a1_source), Some(alternate_path));
    assert_eq!(project.item_name(a2_source), "Synthetic Group");
    for item in [a1, a2] {
        assert_eq!(project.in_point(item).unwrap().ticks, 8 * FRAME);
        assert_eq!(project.out_point(item).unwrap().ticks, 18 * FRAME);
    }
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
