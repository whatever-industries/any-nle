use graph::source::*;
use graph::{Label, MarkerOwner, Rational as Q, Sidecar, StreamKind, TrackRef, Value};
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
            disabled_placements: Vec::new(),
            texts: Vec::new(),
                clock: None,
                id: s.id,
                name: s.name,
                rate: s.rate,
                frame_size: Some(s.frame_size),
                start: None,
                tracks: s.tracks.into_iter().map(|t|
                    graph::source::SourceTrack::new(t.kind, t.index, t.level_db)).collect(),
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
fn invalid_avid_logging_text_is_omitted_only_from_premiere() {
    let mut graph = fixture();
    let item = graph.items[0].id.clone();
    let columns = v(vec![
        ("Scene", Value::Str("12A".into())),
        ("Log", Value::Str("Keep this note".into())),
        ("Log\u{7}", Value::Str("Invalid field name".into())),
        ("Take", Value::Str("2\u{0}".into())),
        ("Shoot Date", Value::Str(String::new())),
    ]);
    graph.sidecars.push(Sidecar {
        adapter: "avid".into(),
        object: item.clone(),
        data: v(vec![
            ("mob_id", Value::Str("synthetic-master".into())),
            ("columns", columns.clone()),
            ("start_timecode", Value::Null),
            ("aux_timecodes", Value::Arr(Vec::new())),
            ("source_file", Value::Null),
            ("source_path", Value::Null),
        ]),
    });

    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&graph, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(graph.sidecar("avid", &item).unwrap().get("columns"), Some(&columns));
    let avid = prepared.model.clips["clip-1"].avid.as_ref().unwrap();
    assert_eq!(avid.columns, [
        ("Scene".into(), "12A".into()),
        ("Log".into(), "Keep this note".into()),
        ("Shoot Date".into(), String::new()),
    ]);
    let losses: Vec<_> = prepared.losses.iter()
        .filter(|loss| loss.aspect == "avid_logging_column").collect();
    assert_eq!(losses.len(), 2);
    assert!(losses.iter().all(|loss| loss.object == item));
    assert!(losses[0].reason.contains(r#""Log\u{7}" has an invalid name"#));
    assert!(losses[1].reason.contains(r#""Take" has an invalid value"#));
}

#[test]
fn avid_source_color_transform_survives_graph_reload_and_reports_one_affected_clip() {
    let graph = fixture();
    let options = parse_model_str(include_str!("../../../examples/opatom_clip.model.json"))
        .unwrap().options;
    let baseline = prepare(&graph, &options, ClockProfile::LegacyImport).unwrap();
    let item_id = graph.items[0].id.clone();
    let wrap = |operation: &str| {
        format!("<ColorTransformationList><ColorTransformation>{operation}</ColorTransformation></ColorTransformationList>")
    };
    for (xml, expected) in [
        (wrap(""), "retained in graph/2 only"),
        (
            wrap("<ExternalLut><LutFileName>Camera &amp; Chart.cube</LutFileName><LutDimension>3D</LutDimension><InhibitScaling/></ExternalLut>"),
            "external source LUT \"Camera & Chart.cube\" on this master",
        ),
        (
            wrap("<ExternalLut><LutFileName>../other.cube</LutFileName><LutDimension>3D</LutDimension></ExternalLut>"),
            "retained in graph/2 only",
        ),
        (
            wrap("<ExternalLut><LutFileName>First.cube</LutFileName><LutFileName>Second.cube</LutFileName><LutDimension>3D</LutDimension></ExternalLut>"),
            "retained in graph/2 only",
        ),
        ("<ColorTransformationList>".into(), "retained in graph/2 only"),
    ] {
        let mut variant = graph.clone();
        let native = variant.sidecars.iter_mut()
            .find(|s| s.adapter == "avid.source" && s.object == item_id).unwrap();
        let Value::Obj(fields) = &mut native.data else { panic!("native facts must be an object") };
        let transforms = Value::Arr(vec![v(vec![
            ("track", Value::Int(1)),
            ("owner_mob", Value::Str("source-mob".into())),
            ("text", Value::Str(xml)),
        ])]);
        fields.push(("input_color_transforms".into(), transforms.clone()));
        let reloaded = SourceGraph::from_json_text(&variant.to_json_text().unwrap()).unwrap();
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
        assert!(loss.reason.contains(expected), "{}", loss.reason);
        assert!(loss.reason.contains("Color Adapter"), "{}", loss.reason);
    }
}
fn q(n: i64) -> Q {
    Q::integer(n)
}
fn options() -> Options {
    Options::default()
}

#[test]
fn direct_24_combined_preserves_media_state_and_discrete_audio() {
    let mut graph = fixture();
    let rate = q(24);
    graph.items[0].rate = Some(rate);
    for (index, media) in graph.media.iter_mut().enumerate() {
        let picture = media.kind == StreamKind::Video;
        let source_rate = if picture { rate } else { q(48_000) };
        media.online = Some(true);
        media.clock = Some(SourceClock {
            effective_count: 0,
            rate,
            display_base: 24,
            drop: false,
            provenance: SourceClockProvenance {
                origin_count: 0,
                source_offset: 0,
                source_rate,
                source_package: "synthetic source".into(),
                timecode_track: format!("physical track {index}"),
                route: "file_source_clip_to_physical_timecode_track".into(),
            },
        });
        let descriptor = if picture {
            v(vec![
                ("compression", Value::Str("AVHD".into())),
                ("resolution_id", Value::Int(1253)),
                ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)])),
            ])
        } else {
            v(vec![("bits", Value::Int(16)), ("channels", Value::Int(1))])
        };
        let facts = graph.sidecars.iter_mut().find(|s| s.object == media.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(0, 24, false)),
            ("file_package", Value::Str(format!("file {index}"))),
            ("descriptor_length", Value::Int(media.duration.to_units(source_rate.num()).unwrap())),
            ("descriptor", descriptor),
            ("picture_coding", if picture {
                Value::Str("060E2B340401010A0401020271130000".into())
            } else { Value::Null }),
        ]);
    }
    for mode in ["grouped", "merged"] {
        let opts = Options { bin_mode: mode.into(), ..options() };
        let prepared = premiere_adapter::prepare_originals(&graph, &opts, ClockProfile::LegacyImport).unwrap();
        assert_eq!(prepared.input.direct_dnxhr_origins.values().copied().collect::<Vec<_>>(), [0]);
        let out = std::env::temp_dir().join(format!("anynle-{mode}-media-state-{}-{}.prproj", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut ids = prproj::SeededUuids::new("combined-media-state");
        prproj::convert::writer::prepare_typed_project(&prepared.input, &out, &mut ids, &[], false)
            .unwrap().publish().unwrap();
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_file(out).unwrap();
        let doc = prproj::PremiereDocument::from_bytes(&bytes).unwrap();
        let tree = &doc.tree;
        let states: Vec<_> = tree.iter_tag(doc.root, "Media")
            .filter(|&id| tree.child_by_tag(id, "ActualMediaFilePath").is_some())
            .map(|id| tree.child_by_tag(id, "ModificationState").and_then(|state| tree.text(state)))
            .collect();
        assert_eq!(states.len(), 3);
        assert!(states.iter().all(|state| state.is_some_and(|blob| !blob.trim().is_empty())));
        assert!(tree.iter_tag(doc.root, "TimecodeFormat").any(|id| tree.text(id) == Some("100")));
        if mode == "grouped" {
            let pans: Vec<_> = tree.iter_tag(doc.root, "MonoToStereoPanProcessor")
                .map(|panner| {
                    let param = tree.find1(panner, "PanProcessor/AudioComponent/Component/Params/Param").unwrap();
                    let param = doc.resolve(param).unwrap();
                    assert_eq!(tree.findtext(param, "Name").unwrap().as_deref(), Some("Pan"));
                    tree.child_by_tag(param, "CurrentValue").and_then(|value| tree.text(value))
                })
                .collect();
            assert_eq!(pans, [Some("0"), None]);

        }
    }

    graph.media.iter_mut().find(|media| media.kind == StreamKind::Audio).unwrap().bits = Some(24);
    for mode in ["grouped", "merged"] {
        let opts = Options { bin_mode: mode.into(), ..options() };
        let changed = premiere_adapter::prepare_originals(&graph, &opts, ClockProfile::LegacyImport).unwrap();
        assert!(changed.input.direct_dnxhr_origins.is_empty());
    }
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
            v(vec![("channels", Value::Int(1)), ("bits", Value::Int(24))])
        };
        let facts = g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(1198, 24, false)),
            ("file_package", Value::Str(format!("{:064X}", i + 1))),
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
    let mut audio_only = g.clone();
    let picture = audio_only.media.iter().find(|media| media.kind == StreamKind::Video).unwrap().id.clone();
    let package = audio_only.sidecar("avid.source", &picture).unwrap().get("file_package").unwrap().clone();
    audio_only.media.retain(|media| media.id != picture);
    audio_only.items[0].media.retain(|binding| binding.media != picture);
    audio_only.sidecars.retain(|sidecar| sidecar.object != picture);
    audio_only.sidecars.push(Sidecar {
        adapter: "avid.missing_managed_picture".into(), object: audio_only.items[0].id.clone(),
        data: v(vec![("track", Value::Int(1)), ("file_package", package.clone())]),
    });
    let prepared = premiere_adapter::prepare_originals(&audio_only, &separate, ClockProfile::LegacyImport)
        .unwrap().input;
    assert_eq!(prepared.direct_dnxhr_origins.values().copied().collect::<Vec<_>>(), [origin]);
    let mut ids = prproj::SeededUuids::new("managed-audio-without-picture");
    let out = dir.join("managed-audio-without-picture.prproj");
    assert!(prepare_typed_project(&prepared, &out, &mut ids, &[], false).is_ok());
    for lane in [1, 2] {
        let mut one_mono = audio_only.clone();
        let missing = one_mono.media.iter().find(|media| media.stream == Some(lane)).unwrap().id.clone();
        let missing_package = one_mono.sidecar("avid.source", &missing).unwrap().get("file_package").unwrap().clone();
        one_mono.media.retain(|media| media.id != missing);
        one_mono.items[0].media.retain(|binding| binding.media != missing);
        one_mono.sidecars.retain(|sidecar| sidecar.object != missing);
        for (track, receipt_package, allowed) in [
            (lane, missing_package.clone(), true),
            (3 - lane, missing_package, false),
            (lane, package.clone(), false),
        ] {
            one_mono.sidecars.retain(|sidecar| sidecar.adapter != "avid.missing_managed_mono");
            one_mono.sidecars.push(Sidecar {
                adapter: "avid.missing_managed_mono".into(), object: one_mono.items[0].id.clone(),
                data: v(vec![("track", Value::Int(track)), ("file_package", receipt_package)]),
            });
            let saved = SourceGraph::from_json_text(&one_mono.to_json_text().unwrap()).unwrap();
            let result = premiere_adapter::prepare_originals(&saved, &separate, ClockProfile::LegacyImport);
            if allowed {
                let input = result.unwrap().input;
                assert_eq!(input.direct_dnxhr_origins.values().copied().collect::<Vec<_>>(), [origin]);
                let out = dir.join(format!("managed-a{}-without-picture.prproj", 3 - lane));
                assert!(prepare_typed_project(&input, &out, &mut ids, &[], false).is_ok());
            } else {
                assert!(result.err().unwrap().to_string().contains("matching file clocks and extents"));
            }
        }
    }
    let mono = audio_only.media.iter_mut().find(|media| media.stream == Some(2)).unwrap();
    let changed = mono.clock.as_mut().unwrap();
    changed.effective_count += 1;
    changed.provenance.origin_count += 1;
    mono.start = changed.exact_start();
    assert!(premiere_adapter::prepare_originals(&audio_only, &separate, ClockProfile::LegacyImport)
        .err().unwrap().to_string().contains("matching file clocks and extents"));

    for lane in [1, 2] {
        let mut partial = g.clone();
        let missing = partial.media.iter().find(|media| media.stream == Some(lane)).unwrap().id.clone();
        let package = partial.sidecar("avid.source", &missing).unwrap().get("file_package").unwrap().clone();
        partial.media.retain(|media| media.id != missing);
        partial.items[0].media.retain(|binding| binding.media != missing);
        partial.sidecars.retain(|sidecar| sidecar.object != missing);
        for (track, allowed) in [(lane, true), (3 - lane, false)] {
            partial.sidecars.retain(|sidecar| sidecar.adapter != "avid.missing_managed_mono");
            partial.sidecars.push(Sidecar {
                adapter: "avid.missing_managed_mono".into(), object: partial.items[0].id.clone(),
                data: v(vec![("track", Value::Int(track)), ("file_package", package.clone())]),
            });
            let prepared = premiere_adapter::prepare_originals(&partial, &separate, ClockProfile::LegacyImport)
                .unwrap().input;
            assert_eq!(!prepared.direct_dnxhr_origins.is_empty(), allowed);
            let mut ids = prproj::SeededUuids::new("partial-managed-mono");
            let out = dir.join(format!("partial-{lane}-{allowed}.prproj"));
            assert_eq!(prepare_typed_project(&prepared, &out, &mut ids, &[], false).is_ok(), allowed);
        }
    }
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

fn direct_23976_dnxhd_fixture(bits: i64) -> SourceGraph {
    let mut g = fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let frame = |n: i128| Q::new(n * 1001, 24_000).unwrap();
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
        if picture { m.rate = Some(rate); } else { m.bits = Some(bits); }
        m.clock = Some(make_clock(172_800, if picture { rate } else { q(48_000) }));
        let facts = g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == m.id).unwrap();
        facts.data = v(vec![
            ("legacy_clock", clock(172_800, 24, false)),
            ("file_package", Value::Str(format!("file {i}"))),
            ("descriptor_length", Value::Int(if picture { 96 } else { 192_192 })),
            ("descriptor", if picture {
                v(vec![("compression", Value::Str("AVHD".into())), ("resolution_id", Value::Int(1253)),
                    ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)]))])
            } else { v(vec![("channels", Value::Int(1)), ("bits", Value::Int(bits))]) }),
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
    g
}

#[test]
fn direct_23976_dnxhd_pcm24_writes_ordinary_discrete_edits() {
    use prproj::convert::{model::TICKS_PER_SECOND, writer::prepare_typed_project};

    let frame = |n: i128| Q::new(n * 1001, 24_000).unwrap();
    let ticks = |n| frame(n).to_units(TICKS_PER_SECOND).unwrap();
    let mut graph = direct_23976_dnxhd_fixture(24);
    for edit in &mut graph.sequences[0].placements {
        edit.source_in = frame(5 + i128::from(edit.audio_tracks.first().copied().unwrap_or(0)));
    }
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let out = std::env::temp_dir().join(format!("anynle-direct-pcm24-{}-{}.prproj", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut ids = prproj::SeededUuids::new("synthetic-direct-pcm24");
    let prepare = |g, mode: &str| premiere_adapter::prepare_originals(
        g, &Options { bin_mode: mode.into(), ..options() }, ClockProfile::LegacyImport);

    let ready = prepare(&graph, "separate").unwrap();
    assert_eq!(ready.input.direct_dnxhd_pcm24.len(), 1);
    assert!(ready.input.multicams.is_empty());
    prepare_typed_project(&ready.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let tree = &doc.tree;
    assert!(tree.iter_tag(doc.root, "CodecType").any(|n| tree.text(n) == Some("1096180846")));
    assert!(tree.iter_tag(doc.root, "TimecodeFormat").any(|n| tree.text(n) == Some("110")));
    assert_eq!(tree.iter_tag(doc.root, "Media").filter_map(|n| tree.child_by_tag(n, "Start"))
        .filter_map(|n| tree.text(n).and_then(|s| s.parse::<i64>().ok())).collect::<Vec<_>>(), [ticks(172_800); 3]);
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let [sequence] = project.sequences().try_into().unwrap();
    let video = project.video_tracks(sequence);
    let audio = project.audio_tracks(sequence);
    let used_tracks = |tracks: &[prproj::model::TrackId]| tracks.iter()
        .copied().filter(|&track| !project.track(track).clips.is_empty()).count();
    assert_eq!((used_tracks(video), used_tracks(audio)), (1, 2));
    for (lane, &track) in [&video[0], &audio[0], &audio[1]].into_iter().enumerate() {
        let [edit] = project.track(track).clips.as_slice() else { panic!("expected one edit per lane") };
        assert_eq!(project.start(*edit).unwrap().ticks, 0);
        assert_eq!(project.in_point(*edit).unwrap().ticks, ticks(5 + lane as i128));
        assert_eq!(project.out_point(*edit).unwrap().ticks, ticks(29 + lane as i128));
        assert_eq!(project.item_media_path(project.track_item_source(*edit).unwrap()),
            Some(graph.media[lane].locations[0].as_str()));
    }
    std::fs::remove_file(&out).unwrap();

    let mut bad_extent = graph.clone();
    let media_id = &bad_extent.media[2].id;
    let side = bad_extent.sidecars.iter_mut().find(|s| s.object == *media_id && s.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut side.data else { panic!("expected media evidence") };
    fields.iter_mut().find(|(field, _)| field == "descriptor_length").unwrap().1 = Value::Int(192_191);
    for (g, mode) in [(&bad_extent, "separate"), (&graph, "grouped"), (&graph, "merged")] {
        let input = prepare(g, mode).unwrap().input;
        assert!(input.direct_dnxhd_pcm24.is_empty());
        assert!(prepare_typed_project(&input, &out, &mut ids, &[], false).is_err());
        assert!(!out.exists());
    }
}

#[test]
fn direct_23976_dnxhd_pcm24_preserves_a_saved_stereo_master_and_discrete_edits() {
    use prproj::convert::{model::TICKS_PER_SECOND, writer::prepare_typed_project};

    let frame = |n: i128| Q::new(n * 1001, 24_000).unwrap();
    let ticks = |n| frame(n).to_units(TICKS_PER_SECOND).unwrap();
    let mut graph = direct_23976_dnxhd_fixture(24);
    graph.items[0].stream_groups = vec![vec![1, 2]];
    for edit in &mut graph.sequences[0].placements {
        edit.source_in = frame(5 + i128::from(edit.audio_tracks.first().copied().unwrap_or(0)));
    }
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let out = std::env::temp_dir().join(format!("anynle-direct-pcm24-stereo-{}-{}.prproj", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let opts = Options { bin_mode: "separate".into(), ..options() };
    let prepare = |g| premiere_adapter::prepare_originals(g, &opts, ClockProfile::LegacyImport);
    let mut ids = prproj::SeededUuids::new("synthetic-direct-pcm24-stereo");
    let input = prepare(&graph).unwrap().input;
    assert_eq!(input.direct_dnxhd_pcm24.len(), 1);
    prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    assert_eq!(project.sequences().len(), 2);
    for sequence in project.sequences() {
        let stereo = project.sequence_name(sequence).ends_with("_A01A02");
        assert_eq!(project.zero_point(sequence).unwrap().ticks, ticks(if stereo { 172_800 } else { 86_400 }));
        let audio = project.audio_tracks(sequence);
        assert!(audio.len() >= 2);
        for (lane, &track) in audio.iter().take(2).enumerate() {
            let [edit] = project.track(track).clips.as_slice() else { panic!("expected one edit per mono lane") };
            assert_eq!(project.in_point(*edit).unwrap().ticks, ticks(if stereo { 0 } else { 6 + lane as i128 }));
            assert_eq!(project.out_point(*edit).unwrap().ticks, ticks(if stereo { 96 } else { 30 + lane as i128 }));
            assert_eq!(project.item_media_path(project.track_item_source(*edit).unwrap()),
                Some(graph.media[lane + 1].locations[0].as_str()));
        }
    }

    let mut reversed = graph.clone();
    reversed.items[0].stream_groups[0].reverse();
    let input = prepare(&reversed).unwrap().input;
    assert!(input.direct_dnxhd_pcm24.is_empty());
    assert!(prepare_typed_project(&input, &out, &mut ids, &[], false).is_err());
    assert!(!out.exists());
}

#[test]
fn direct_23976_dnxhd_pcm16_preserves_two_file_clocks_and_sample_types() {
    use prproj::convert::{model::TICKS_PER_SECOND, writer::prepare_typed_project};

    let g = direct_23976_dnxhd_fixture(16);
    let rate = g.sequences[0].rate;
    let frame = |n: i128| Q::new(n * 1001, 24_000).unwrap();
    let ticks = |n| frame(n).to_units(TICKS_PER_SECOND).unwrap();
    let opts = Options { bin_mode: "separate".into(), ..options() };
    let g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let out = std::env::temp_dir().join(format!("anynle-direct-pcm16-{}-{}.prproj", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut ids = prproj::SeededUuids::new("synthetic-direct-pcm16");
    for mode in ["separate", "grouped", "merged"] {
        let options = Options { bin_mode: mode.into(), ..opts.clone() };
        let input = premiere_adapter::prepare_originals(&g, &options, ClockProfile::LegacyImport).unwrap().input;
        assert_eq!(input.sequences[0].start, ticks(86_400));
        assert!(input.sequences[0].events.iter().all(|e| e.source_in == ticks(5)));
        prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
        let mut doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        std::fs::remove_file(&out).unwrap();
        assert!(doc.tree.iter_tag(doc.root, "CodecType").any(|n| doc.tree.text(n) == Some("1096180846")));
        assert!(doc.tree.iter_tag(doc.root, "TimecodeFormat").any(|n| doc.tree.text(n) == Some("110")));
        assert_eq!(doc.tree.iter_tag(doc.root, "SampleType").filter(|&n| doc.tree.text(n) == Some("3")).count(), 2);
        assert_eq!(doc.tree.iter_tag(doc.root, "Media").filter_map(|n| doc.tree.child_by_tag(n, "Start"))
            .filter_map(|n| doc.tree.text(n).and_then(|s| s.parse::<i64>().ok())).collect::<Vec<_>>(), [ticks(172_800); 3]);
        let mut starts: Vec<_> = doc.tree.iter_tag(doc.root, "MZ.ZeroPoint")
            .map(|n| doc.tree.text(n).unwrap().parse::<i64>().unwrap()).collect();
        starts.sort_unstable();
        let expected = if mode == "separate" {
            vec![ticks(86_400)]
        } else {
            vec![ticks(86_400), ticks(172_800)]
        };
        assert_eq!(starts, expected);
        let states: Vec<_> = doc.tree.iter_tag(doc.root, "Media")
            .filter_map(|n| doc.tree.child_by_tag(n, "ModificationState"))
            .collect();
        assert_eq!(states.len(), 3);
        for state in states {
            assert!(doc.payload(state).unwrap().is_some_and(|bytes| !bytes.is_empty()));
        }
        if mode == "grouped" {
            assert!(doc.tree.iter_tag(doc.root, "MonoToStereoPanProcessor").count() >= 2);

            use prproj::convert::{model::Marker, prepared::PreparedClip};
            let mut marked = input.clone();
            let sequence = &mut marked.sequences[0];
            let (index, edit) = sequence.events.iter().enumerate()
                .find(|(_, e)| e.video_track == Some(1) && e.audio_tracks.contains(&1)).unwrap();
            let name = sequence.name.clone();
            for (label, track) in [("Picture range", "V1"), ("Mono range", "A1")] {
                sequence.markers.push(Marker {
                    name: label.into(), comment: "Review".into(), user: String::new(),
                    color: "Red".into(), track: Some(track.into()), owner: "edit".into(),
                    start: edit.record_start, duration: ticks(1),
                    event: Some(index as i64), source: Some(edit.source_in), inherited: false,
                });
            }
            let source_start = edit.source_in;
            let PreparedClip::Legacy(source_clip) = &mut marked.clips[0] else {
                panic!("expected a linked DNxHD master");
            };
            for (name, track) in [("Source A1 range", "A1"), ("Source A2 range", "A2")] {
                source_clip.markers.push(Marker {
                    name: name.into(), comment: "Source review".into(), user: String::new(),
                    color: "Red".into(), track: Some(track.into()), owner: "sequence".into(),
                    start: source_start, duration: ticks(1), event: None, source: None, inherited: false,
                });
            }
            let out = std::env::temp_dir().join(format!("anynle-grouped-markers-{}-{}.prproj", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            let report = prproj::convert::writer::prepare_typed_project(&marked, &out, &mut ids, &[], false)
                .unwrap().publish().unwrap();
            let doc = prproj::xml::parse_prproj(&out).unwrap();
            std::fs::remove_file(out).unwrap();
            let tree = &doc.tree;
            let sequence = tree.iter_tag(doc.root, "Sequence")
                .find(|&node| tree.findtext(node, "Name").unwrap().as_deref() == Some(name.as_str())).unwrap();
            let collection = doc.resolve(tree.find1(sequence, "MarkerOwner/Markers").unwrap()).unwrap();
            let saved = doc.resolve(tree.find1(collection, "Markers/Marker/Second").unwrap()).unwrap();
            let marker = tree.findtext(saved, "DVAMarker").unwrap().unwrap();
            assert!(marker.contains("Picture range"));
            let resolve = |node, path| tree.find1(node, path).map(|r| doc.resolve(r).unwrap());
            let audio = tree.iter_tag(doc.root, "AudioClipTrackItem").filter_map(|item| {
                let subclip = resolve(item, "ClipTrackItem/SubClip")?;
                let clip = resolve(subclip, "Clip")?;
                let collection = resolve(clip, "Clip/MarkerOwner/Markers")?;
                let markers: Vec<_> = tree.findall(collection, "Markers/Marker/Second").unwrap().iter()
                    .map(|&r| tree.findtext(doc.resolve(r).unwrap(), "DVAMarker").unwrap().unwrap())
                    .collect();
                markers.iter().any(|text| text.contains("Mono range")).then_some(markers)
            }).next().unwrap();
            assert!(audio.iter().any(|text| text.contains("Source A2 range")));
            assert!(!audio.iter().any(|text| text.contains("Source A1 range")));
            let [marker_move] = report.grouped_picture_marker_moves.as_slice() else {
                panic!("expected a Grouped picture marker ownership change");
            };
            assert_eq!((marker_move.sequence, marker_move.record_ticks, marker_move.marker_name.as_str()),
                (0, marked.sequences[0].events[index].record_start, "Picture range"));
        }
    }

    let mut mixed = g.clone();
    mixed.media.iter_mut().find(|m| m.kind == StreamKind::Audio).unwrap().bits = Some(24);
    for mode in ["separate", "grouped", "merged"] {
        let options = Options { bin_mode: mode.into(), ..opts.clone() };
        let input = premiere_adapter::prepare_originals(&mixed, &options, ClockProfile::LegacyImport).unwrap().input;
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

    let mut source_marked = ranged.clone();
    let item = &mut source_marked.items[0];
    item.markers.push(SourceMarker {
        name: "Backing source range".into(),
        comment: "Keep the source annotation".into(),
        user: String::new(),
        color: Some(Label { palette: "avid".into(), name: "Red".into() }),
        start: Some(frame(10)),
        duration: frame(3),
        track: Some(TrackRef { kind: StreamKind::Video, index: 1 }),
        owner: MarkerOwner::Sequence,
        owner_occurrence: item.id.clone(),
        owner_local: Some(frame(10)),
        owner_rate: Some(rate),
        binding: SourceBinding { placements: vec![], reason: None },
        source: None,
        inherited: false,
    });
    let source_input = prepare(&source_marked);
    assert_eq!(source_input.input.av_subclip_uses, ready.input.av_subclip_uses);
    prepare_typed_project(&source_input.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert_eq!(doc.tree.iter_tag(doc.root, "DVAMarker").count(), 2);

    let mut two_bins = source_marked.clone();
    let mut second_master = two_bins.items[0].clone();
    second_master.id = "duplicate-backing-master".into();
    second_master.markers.clear();
    two_bins.bins.push(graph::Bin {
        id: "other-backing-bin".into(),
        name: "Other bin with the same backing mob".into(),
        parent: None,
        items: vec![second_master.id.clone()],
    });
    two_bins.items.push(second_master.clone());
    let prepared = prepare(&two_bins);
    assert_eq!(prepared.input.av_subclips, source_input.input.av_subclips);
    assert_eq!(prepared.input.av_subclip_uses, source_input.input.av_subclip_uses);

    let mut ambiguous_here = two_bins.clone();
    ambiguous_here.bins[0].items.push(second_master.id);
    let refused = prepare(&ambiguous_here);
    assert!(refused.input.av_subclips.is_empty());
    assert!(refused.losses.iter().any(|l| l.aspect == "subclip_bin_item"
        && l.reason.contains("more than one selected master")));

    let mut ambiguous_elsewhere = two_bins.clone();
    let scoped_items = std::mem::take(&mut ambiguous_elsewhere.bins[0].items);
    ambiguous_elsewhere.bins[1].items.extend(scoped_items);
    let refused = prepare(&ambiguous_elsewhere);
    assert!(refused.input.av_subclips.is_empty());
    assert!(refused.losses.iter().any(|l| l.aspect == "subclip_bin_item"
        && l.reason.contains("more than one selected master")));

    let mut crossing = source_marked.clone();
    crossing.items[0].markers[0].duration = frame(87);
    let outside_subclip = prepare(&crossing);
    assert!(outside_subclip.input.av_subclips.is_empty());
    assert!(outside_subclip.losses.iter().any(|l| l.aspect == "subclip_bin_item"
        && l.reason.contains("one V1 range contained in the subclip")));

    let mut marked = ranged.clone();
    let picture_id = marked.sequences[0].placements.iter()
        .find(|p| p.video_track.is_some()).unwrap().id.clone();
    marked.sequences[0].markers.push(SourceMarker {
        name: "Subclip review".into(),
        comment: "Keep this range".into(),
        user: String::new(),
        color: Some(Label { palette: "avid".into(), name: "Red".into() }),
        start: Some(frame(2)),
        duration: frame(3),
        track: Some(TrackRef { kind: StreamKind::Video, index: 1 }),
        owner: MarkerOwner::Edit,
        owner_occurrence: "synthetic:subclip-marker".into(),
        owner_local: Some(frame(2)),
        owner_rate: Some(rate),
        binding: SourceBinding { placements: vec![picture_id.clone()], reason: None },
        source: None,
        inherited: false,
    });
    let marked_input = prepare(&marked);
    assert_eq!(marked_input.input.av_subclip_uses, ready.input.av_subclip_uses);
    assert!(marked_input.losses.iter().any(|l| l.aspect == "marker_ownership"
        && l.reason.contains("Subclip review")));
    let marker = &marked_input.input.sequences[0].markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("sequence", ticks(2), ticks(3)));
    prepare_typed_project(&marked_input.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    std::fs::remove_file(&out).unwrap();

    let mut both = marked.clone();
    both.items[0].markers = source_marked.items[0].markers.clone();
    both.items[0].markers[0].start = Some(frame(7));
    both.items[0].markers[0].owner_local = Some(frame(7));
    let conflicting = prepare(&both);
    assert_eq!(conflicting.input.av_subclips.len(), 1);
    assert!(conflicting.input.av_subclip_uses.is_empty());
    assert_eq!(conflicting.input.sequences[0].markers[0].owner, "edit");

    let mut distinct = marked.clone();
    distinct.items[0].markers = source_marked.items[0].markers.clone();
    let distinct = prepare(&distinct);
    assert_eq!(distinct.input.av_subclip_uses, ready.input.av_subclip_uses);
    let marker = &distinct.input.sequences[0].markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("sequence", ticks(2), ticks(3)));
    prepare_typed_project(&distinct.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&out).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert_eq!(doc.tree.iter_tag(doc.root, "DVAMarker").count(), 3);

    let mut incomplete = marked.clone();
    incomplete.sidecars.retain(|s| s.object != picture_id);
    let incomplete = prepare(&incomplete);
    assert!(incomplete.input.av_subclip_uses.is_empty());
    assert_eq!(incomplete.input.sequences[0].markers[0].owner, "edit");

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

    let mut outside_marked = marked.clone();
    shorten(&mut outside_marked, 5, 29);
    let template = &source_marked.items[0].markers[0];
    for (name, start, duration) in [("Ends at In", 3, 2), ("Point at Out", 29, 0)] {
        let start = frame(start);
        outside_marked.items[0].markers.push(SourceMarker {
            name: name.into(),
            start: Some(start),
            duration: frame(duration),
            owner_local: Some(start),
            ..template.clone()
        });
    }
    let outside_graph = SourceGraph::from_json_text(&outside_marked.to_json_text().unwrap()).unwrap();
    let outside = prepare(&outside_graph);
    assert_eq!(outside.input.av_subclips, short.input.av_subclips);
    assert_eq!(outside.input.av_subclip_uses, short.input.av_subclip_uses);
    assert_eq!(outside.input.sequences[0].markers[0].owner, "sequence");
    prepare_typed_project(&outside.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&out).unwrap();
    std::fs::remove_file(&out).unwrap();
    assert_eq!(doc.tree.iter_tag(doc.root, "DVAMarker").count(), 3);

    let mut at_in = outside_marked.clone();
    at_in.items[0].markers.push(SourceMarker {
        name: "Point at In".into(),
        start: Some(frame(5)),
        duration: Q::ZERO,
        owner_local: Some(frame(5)),
        ..template.clone()
    });
    let touching = prepare(&at_in);
    assert_eq!(touching.input.av_subclips.len(), 1);
    assert_eq!(touching.input.av_subclip_uses, short.input.av_subclip_uses);
    assert_eq!(touching.input.sequences[0].markers[0].owner, "sequence");
    prepare_typed_project(&touching.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&out).unwrap();
    std::fs::remove_file(&out).unwrap();
    let markers: Vec<_> = doc.tree.iter_tag(doc.root, "DVAMarker")
        .filter_map(|id| doc.tree.text(id)).collect();
    assert_eq!(markers.len(), 5);
    assert_eq!(markers.iter().filter(|marker| marker.contains("Point at In")).count(), 2);

    outside_marked.items[0].markers[0].duration = frame(3);
    let crossing_in = prepare(&outside_marked);
    assert!(crossing_in.input.av_subclips.is_empty());
    assert!(crossing_in.losses.iter().any(|l| l.aspect == "subclip_bin_item"));

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

    let mut marked_nest = nested.clone();
    marked_nest.sequences[0].markers.push(SourceMarker {
        name: "Marked Submaster".into(),
        comment: "Retain this range on the parent".into(),
        user: String::new(),
        color: Some(Label { palette: "avid".into(), name: "Red".into() }),
        start: Some(frame(10)),
        duration: frame(3),
        track: Some(TrackRef { kind: StreamKind::Video, index: 1 }),
        owner: MarkerOwner::Edit,
        owner_occurrence: "synthetic:marked-submaster".into(),
        owner_local: Some(frame(2)),
        owner_rate: Some(rate),
        binding: SourceBinding { placements: vec![picture_id.clone()], reason: None },
        source: None,
        inherited: false,
    });
    let marked = prepare(&marked_nest);
    let [marked_child] = marked.input.picture_nests.as_slice() else { panic!("marked nest was flattened") };
    assert_eq!((marked_child.sequence, marked_child.event), (nest.sequence, nest.event));
    let marker = &marked.input.sequences[marked_child.sequence].markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("sequence", ticks(10), ticks(3)));
    assert!(marked.losses.iter().any(|l| l.aspect == "marker_ownership" && l.reason.contains("Marked Submaster")));
    prepare_typed_project(&marked.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    std::fs::remove_file(&out).unwrap();

    let mut record_only_nest = marked_nest.clone();
    record_only_nest.sequences[0].markers[0].source = Some(Q::new(1, 11).unwrap());
    let saved = SourceGraph::from_json_text(&record_only_nest.to_json_text().unwrap()).unwrap();
    let record_only = prepare(&saved);
    assert_eq!(record_only.input.picture_nests, marked.input.picture_nests);
    let marker = &record_only.input.sequences[0].markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("sequence", ticks(10), ticks(3)));
    assert_eq!(record_only.losses.iter().filter(|l| l.aspect == "marker_ownership").count(), 1);
    prepare_typed_project(&record_only.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    std::fs::remove_file(&out).unwrap();

    let mut outside_nest = record_only_nest.clone();
    outside_nest.sequences[0].markers[0].duration = frame(40);
    let outside = prepare(&outside_nest);
    assert!(outside.input.picture_nests.is_empty());
    assert_eq!(outside.input.sequences[0].markers[0].owner, "sequence");

    let mut unresolved_record_only = record_only_nest;
    unresolved_record_only.sequences[0].markers[0].binding.reason = Some("unknown native owner".into());
    assert!(prepare(&unresolved_record_only).input.picture_nests.is_empty());

    let mut upper = marked_nest.clone();
    let sequence = &mut upper.sequences[0];
    sequence.placements.iter_mut().find(|p| p.id == picture_id).unwrap().video_track = Some(5);
    sequence.markers[0].track = Some(TrackRef { kind: StreamKind::Video, index: 5 });
    let upper = prepare(&upper);
    let [upper_child] = upper.input.picture_nests.as_slice() else { panic!("upper nest was flattened") };
    let sequence = &upper.input.sequences[upper_child.sequence];
    assert_eq!(sequence.events[upper_child.event].video_track, Some(5));
    let marker = &sequence.markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("sequence", ticks(10), ticks(3)));
    assert!(upper.losses.iter().any(|l| l.aspect == "marker_ownership" && l.reason.contains("on V5")));
    prepare_typed_project(&upper.input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    std::fs::remove_file(&out).unwrap();

    let mut unresolved = marked_nest.clone();
    unresolved.sequences[0].markers[0].binding.reason = Some("unknown native owner".into());
    let unresolved = prepare(&unresolved);
    assert!(unresolved.input.picture_nests.is_empty());
    assert_eq!(unresolved.input.sequences[0].markers[0].owner, "sequence");

    let mut inherited_nest = marked_nest.clone();
    let mut source_marker = inherited_nest.sequences[0].markers[0].clone();
    let edit = inherited_nest.sequences[0].placements.iter().find(|p| p.id == picture_id).unwrap();
    source_marker.start = Some(edit.source_in.checked_add(frame(2)).unwrap());
    source_marker.owner_local = source_marker.start;
    source_marker.owner = MarkerOwner::Sequence;
    source_marker.owner_occurrence = "synthetic:backing-picture".into();
    source_marker.binding.placements.clear();
    inherited_nest.items[0].markers.push(source_marker);
    for id in [&inherited_nest.items[0].id, &inherited_nest.sequences[0].id] {
        let sidecar = inherited_nest.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && &s.object == id).unwrap();
        let Value::Obj(fields) = &mut sidecar.data else { panic!("missing Avid source fields") };
        fields.push(("marker_native".into(), Value::Arr(vec![v(vec![
            ("graph_index", Value::Int(0)), ("created", Value::Int(100)),
        ])])));
    }
    let inherited = prepare(&inherited_nest);
    assert_eq!(inherited.input.picture_nests.len(), 1);
    let marker = &inherited.input.sequences[0].markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("sequence", ticks(10), ticks(3)));
    assert!(!marker.inherited);
    assert_eq!(inherited.losses.iter().filter(|l| l.aspect == "marker_ownership").count(), 1);

    let mut mixed_rate_nest = make_nest.input.clone();
    mixed_rate_nest.sequences[nest.sequence].rate = prproj::convert::model::Fraction::new(24, 1);
    assert!(prepare_typed_project(&mixed_rate_nest, &out, &mut ids, &[], false).is_err());
    assert!(!out.exists());

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
fn marked_constant_retime_preserves_picture_speed_and_the_full_record_marker() {
    let mut graph = fixture();
    let frame = |n| Q::new(n, 24).unwrap();
    let rate = q(24);
    graph.items[0].rate = Some(rate);
    for (index, media) in graph.media.iter_mut().enumerate() {
        let picture = media.kind == StreamKind::Video;
        media.online = Some(true);
        media.start = Some(Q::ZERO);
        media.duration = frame(242);
        if !picture {
            media.bits = Some(16);
        }
        media.clock = Some(SourceClock {
            effective_count: 0,
            rate,
            display_base: 24,
            drop: false,
            provenance: SourceClockProvenance {
                origin_count: 0,
                source_offset: 0,
                source_rate: if picture { rate } else { q(48_000) },
                source_package: format!("physical {index}"),
                timecode_track: format!("TC {index}"),
                route: "file_source_clip_to_physical_timecode_track".into(),
            },
        });
        let facts = graph.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == media.id).unwrap();
        let descriptor = if picture {
            v(vec![("compression", Value::Str("AVHD".into())),
                   ("resolution_id", Value::Int(1253)),
                   ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)]))])
        } else {
            v(vec![("channels", Value::Int(1)), ("bits", Value::Int(16))])
        };
        facts.data = v(vec![
            ("legacy_clock", clock(0, 24, false)),
            ("descriptor", descriptor),
            ("descriptor_length", Value::Int(if picture { 242 } else { 484_000 })),
            ("file_package", Value::Str(format!("file {index}"))),
            ("picture_coding", if picture { Value::Str("060E2B340401010A0401020271130000".into()) } else { Value::Null }),
        ]);
    }
    let sequence = &mut graph.sequences[0];
    let picture = sequence.placements.iter_mut().find(|p| p.video_track == Some(1)).unwrap();
    picture.record_start = frame(50);
    picture.source_in = frame(42);
    picture.duration = frame(50);
    picture.speed = Some(graph::Speed { ratio: (50, 99), source_duration: frame(99) });
    let parent = picture.id.clone();
    let marker = &mut sequence.markers[0];
    marker.owner = MarkerOwner::Edit;
    marker.track = Some(graph::TrackRef { kind: StreamKind::Video, index: 1 });
    marker.start = Some(frame(56));
    marker.source = Some(frame(54));
    marker.duration = frame(3);
    marker.binding.placements = vec![parent.clone()];
    let mut unresolved = marker.clone();
    unresolved.start = None;
    sequence.markers.push(unresolved);
    graph.sidecars.push(Sidecar {
        adapter: "avid.source".into(),
        object: parent,
        data: v(vec![("constant_speed", Value::Arr(vec![Value::Int(2), Value::Int(1)]))]),
    });
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let options = Options { bin_mode: "separate".into(), ..options() };
    let compat = prepare(&graph, &options, ClockProfile::LegacyImport).unwrap();
    let typed = premiere_adapter::prepare_originals(&graph, &options, ClockProfile::LegacyImport).unwrap();
    assert_eq!(compat.constant_speeds, typed.input.constant_speeds);
    assert_eq!(compat.constant_speeds.len(), 1);
    assert_eq!(compat.constant_speeds[0].ratio, (2, 1));
    let [saved] = typed.input.sequences[0].markers.as_slice() else {
        panic!("expected the one marker with qualified record time");
    };
    assert_eq!(saved.owner, "sequence");
    assert_eq!(saved.start, frame(56).to_units(prproj::convert::model::TICKS_PER_SECOND).unwrap());
    assert_eq!(saved.duration, frame(3).to_units(prproj::convert::model::TICKS_PER_SECOND).unwrap());
    assert_eq!(saved.event, None);
    assert_eq!(saved.source, None);
    assert!(typed.losses.iter().any(|l| l.aspect == "marker_ownership" && l.reason.contains("corrected constant retime")));
    assert!(typed.losses.iter().any(|l| l.aspect == "marker_timing"));
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
        assert_eq!(input.direct_dnxhr_origins.values().copied().collect::<Vec<_>>(), [ticks(215_784)]);
        assert_eq!(input.sequences[0].start, ticks(1795));
        prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
        let mut doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        std::fs::remove_file(&out).unwrap();
        let displays: Vec<_> = doc.tree.iter_tag(doc.root, "MZ.Sequence.VideoTimeDisplayFormat")
            .map(|n| doc.tree.text(n).unwrap()).collect();
        assert_eq!(displays, ["102", "102"]);
        // The reusable bin item and the editorial timeline have independent clocks.
        let mut starts: Vec<_> = doc.tree.iter_tag(doc.root, "MZ.ZeroPoint")
            .map(|n| doc.tree.text(n).unwrap().parse::<i64>().unwrap()).collect();
        starts.sort_unstable();
        assert_eq!(starts, [ticks(1795), ticks(215_784)]);
        let states: Vec<_> = doc.tree.iter_tag(doc.root, "Media")
            .filter_map(|n| doc.tree.child_by_tag(n, "ModificationState")).collect();
        assert_eq!(states.len(), 3);
        for state in states {
            assert!(doc.payload(state).unwrap().is_some_and(|bytes| !bytes.is_empty()));
        }
    }
    let mut unknown = saved.clone();
    unknown.sequences[0].clock = None;
    for mode in ["separate", "grouped", "merged"] {
        let options = Options { bin_mode: mode.into(), ..options() };
        assert!(premiere_adapter::prepare_originals(&unknown, &options, ClockProfile::LegacyImport)
            .err().unwrap().to_string().contains("direct_dnxhd_sequence_clock"));
    }
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
fn avid_25_fps_tc1_changes_only_the_absolute_sequence_origin() {
    let mut g = fixture();
    let rate = q(25);
    let seq = &mut g.sequences[0];
    seq.rate = rate;
    let sequence_id = seq.id.clone();
    g.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == sequence_id)
        .unwrap().data = v(vec![("compatibility_start", Value::Str("3600".into()))]);
    let baseline = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let mut expected = baseline.model.sequences[0].clone();
    let count = 93_079; // 01:02:03:04 at 25 fps.
    let clock = SourceClock {
        effective_count: count,
        rate,
        display_base: 25,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: count,
            source_offset: 0,
            source_rate: rate,
            source_package: sequence_id,
            timecode_track: "TC1/0".into(),
            route: "direct sequence TC1".into(),
        },
    };
    g.sequences[0].start = clock.exact_start();
    g.sequences[0].clock = Some(clock);
    let saved = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let corrected = prepare(&saved, &options(), ClockProfile::LegacyImport).unwrap();
    expected.start = 945_742_210_560_000;
    assert_eq!(corrected.model.sequences[0], expected);
    assert_eq!(corrected.ownership, baseline.ownership);
    assert!(corrected.diagnostics.iter().any(|s| s.contains("qualified 25 clock")));

    g.sequences[0].clock.as_mut().unwrap().display_base = 24;
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
fn linked_corner_pin_keeps_points_and_checks_matching_raster() {
    use prproj::convert::prepared::{CornerPin, PictureEffect};

    let mut g = linked_avc_pcm_picture_fixture();
    let corners = [[0.1, 0.2], [0.8, 0.1], [0.9, 0.8], [0.2, 0.9]];
    let crop = [0.05, 0.1, 0.15, 0.2];
    let params = v(vec![
        ("target_corners_tl_tr_br_bl", Value::Arr(corners.map(|xy|
            Value::Arr(xy.map(Value::Float).into())).into())),
        ("source_crop_ltrb", Value::Arr(crop.map(Value::Float).into())),
    ]);
    let axes = graph::picture_chain::Reflection { horizontal: true, vertical: false }.effect();
    let edit = &mut g.sequences[0].placements[0];
    let id = edit.id.clone();
    edit.effects = vec![graph::Effect {
        kind: graph::picture_chain::KIND.into(),
        params: v(vec![("stages", Value::Arr(vec![
            v(vec![("kind", Value::Str("picture.corner_pin".into())), ("params", params)]),
            v(vec![("kind", Value::Str(axes.kind)), ("params", axes.params)]),
        ]))]),
    }];

    let mut prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    prepared.input.validate().unwrap();
    let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected one pin with Flop") };
    assert_eq!(effect.effect, PictureEffect::CornerPinWithFlop {
        pin: CornerPin { corners, source_crop: crop }, flop_before_pin: false,
    });

    let mut picture_only = g.clone();
    picture_only.sidecars.retain(|sidecar| !g.media.iter().any(|m|
        m.kind == StreamKind::Audio && m.id == sidecar.object));
    picture_only.media.retain(|m| m.kind == StreamKind::Video);
    for item in &mut picture_only.items {
        item.stream_groups.clear();
        item.media.retain(|binding| picture_only.media.iter().any(|m| m.id == binding.media));
    }
    for sequence in &mut picture_only.sequences {
        sequence.placements.retain(|p| p.video_track.is_some());
        sequence.tracks.retain(|track| track.kind == StreamKind::Video);
    }
    let picture_only = premiere_adapter::prepare_originals(
        &picture_only, &options(), ClockProfile::LegacyImport,
    ).unwrap();
    picture_only.input.validate().unwrap();
    let [picture_pin] = picture_only.input.picture_effects.as_slice() else {
        panic!("expected Corner Pin on picture-only linked AVC");
    };
    assert_eq!(picture_pin.effect, effect.effect);

    let mut aac = g.clone();
    for media in aac.media.iter_mut().filter(|m| m.kind == StreamKind::Audio) {
        media.bits = None;
        media.codec = Some("aac".into());
        media.original.as_mut().unwrap().sample_representation = Some("aac".into());
    }
    let aac = premiere_adapter::prepare_originals(&aac, &options(), ClockProfile::LegacyImport).unwrap();
    aac.input.validate().unwrap();
    let [aac_pin] = aac.input.picture_effects.as_slice() else {
        panic!("expected Corner Pin on linked AVC with AAC");
    };
    assert_eq!(aac_pin.effect, effect.effect);

    let mut prores = g.clone();
    for media in &mut prores.media {
        media.locations = vec!["/Synthetic/source.mov".into()];
        media.original.as_mut().unwrap().saved_locators = vec!["/Saved/source.mov".into()];
        if media.kind == StreamKind::Video {
            media.codec = Some("apch".into());
        }
    }
    let prores = premiere_adapter::prepare_originals(
        &prores, &options(), ClockProfile::LegacyImport,
    ).unwrap();
    prores.input.validate().unwrap();
    let [prores_pin] = prores.input.picture_effects.as_slice() else {
        panic!("expected Corner Pin on linked ProRes HQ");
    };
    assert_eq!(prores_pin.effect, effect.effect);

    prepared.input.sequences[0].width = 1280;
    assert!(prepared.input.validate().unwrap_err().to_string().contains("invalid static Corner Pin"));

    g.sequences[0].frame_size = Some((1920, 1080));
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(prepared.input.picture_effects.is_empty());
    assert!(prepared.losses.iter().any(|loss| loss.object == id && loss.aspect == "picture_effect"));
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

fn linked_avc_pcm_picture_fixture() -> SourceGraph {
    let mut g = original_fixture();
    for media in &mut g.media {
        if media.kind == StreamKind::Video {
            media.frame_size = Some((1440, 1080));
        } else {
            media.bits = Some(24);
            media.codec = Some("pcm_s24le".into());
            media.original.as_mut().unwrap().sample_representation = Some("pcm24".into());
        }
    }
    g.sequences[0].frame_size = Some((1440, 1080));
    g
}

#[test]
fn static_picture_chain_requires_a_matching_raster_for_linked_avc_pcm() {
    use prproj::convert::prepared::PictureEffect;

    let mut g = linked_avc_pcm_picture_fixture();
    let edit = g.sequences[0].placements.iter_mut().find(|p| p.video_track.is_some()).unwrap();
    let axes = graph::picture_chain::Reflection { horizontal: true, vertical: false }.effect();
    edit.effects = vec![graph::Effect {
        kind: graph::picture_chain::KIND.into(),
        params: v(vec![("stages", Value::Arr(vec![
            v(vec![
                ("kind", Value::Str("avid.warp_static".into())),
                ("params", v(vec![])),
                ("transform", v(vec![
                    ("position", Value::Arr(vec![Value::Float(0.1), Value::Float(0.0)])),
                    ("scale", Value::Arr(vec![Value::Float(80.0), Value::Float(90.0)])),
                    ("anchor", Value::Arr(vec![Value::Float(0.0); 2])),
                    ("rotation", Value::Float(0.0)),
                    ("crop", Value::Arr([0.1, 0.0, 0.2, 0.0].map(Value::Float).into())),
                ])),
            ]),
            v(vec![("kind", Value::Str(axes.kind)), ("params", axes.params)]),
        ]))]),
    }];
    let id = edit.id.clone();
    let g_json = g.to_json_text().unwrap();
    let mut g = SourceGraph::from_json_text(&g_json).unwrap();
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected one chain") };
    assert_eq!(effect.effect, PictureEffect::StaticMotion);
    assert_eq!(effect.reflection_before_motion, [true, false]);
    let transform = prepared.input.sequences[effect.sequence].events[effect.event].transform.as_ref().unwrap();
    assert_eq!(transform.position, (-0.1, 0.0));
    assert_eq!(transform.crop, [0.2, 0.0, 0.1, 0.0]);

    g.sequences[0].frame_size = Some((1920, 1080));
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(prepared.input.picture_effects.is_empty());
    assert!(prepared.losses.iter().any(|loss| loss.object == id && loss.aspect == "picture_effect"));
}

#[test]
fn wide_linear_motion_preserves_media_keys_and_reflection_order() {
    use prproj::convert::prepared::PictureEffect;

    let mut g = linked_avc_pcm_picture_fixture();
    let edit = g.sequences[0].placements.iter_mut().find(|p| p.video_track.is_some()).unwrap();
    let id = edit.id.clone();
    let unit = prproj::convert::model::TICKS_PER_SECOND;
    let start = edit.source_in.to_units(unit).unwrap();
    let last = edit.duration.to_units(24).unwrap() - 1;
    let pair = |x, y| Value::Arr([x, y].map(Value::Float).into());
    let key = |frame, x, w| v(vec![
        ("frame", Value::Int(frame)),
        ("position", pair(x, 0.025)),
        ("scale", pair(w, 100.0)),
    ]);
    let params = v(vec![("keys", Value::Arr(vec![key(0, -0.1, 80.0), key(last, 0.2, 90.0)]))]);
    let axes = graph::picture_chain::Reflection { horizontal: true, vertical: false }.effect();
    let chain = graph::Effect {
        kind: graph::picture_chain::KIND.into(),
        params: v(vec![("stages", Value::Arr(vec![
            v(vec![
                ("kind", Value::Str("avid.warp_linear".into())),
                ("params", params.clone()),
                ("transform", v(vec![
                    ("position", pair(-0.1, 0.025)), ("scale", pair(80.0, 100.0)),
                    ("anchor", pair(0.0, 0.0)), ("rotation", Value::Float(0.0)),
                    ("crop", Value::Arr(vec![Value::Float(0.0); 4])),
                ])),
            ]),
            v(vec![("kind", Value::Str(axes.kind)), ("params", axes.params)]),
        ]))]),
    };
    edit.effects = vec![chain];
    let raw = g.to_json_text().unwrap();
    let mut g = SourceGraph::from_json_text(&raw).unwrap();
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected one chain") };
    let PictureEffect::Resize(keys) = effect.effect else { panic!("expected Linear Motion") };
    assert_eq!(keys.map(|key| key.ticks), [start, start + last * (unit / 24)]);
    assert_eq!(keys.map(|key| key.position), [[0.1, 0.025], [-0.2, 0.025]]);
    assert_eq!(effect.reflection_before_motion, [true, false]);

    let edit = g.sequences[0].placements.iter_mut().find(|p| p.id == id).unwrap();
    edit.transform = Some(graph::Transform {
        position: (-0.1, 0.025), scale: (80.0, 100.0),
        anchor: (0.0, 0.0), rotation: 0.0, crop: [0.0; 4],
    });
    edit.effects = vec![graph::Effect { kind: "avid.resize_linear".into(), params }];
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected single Motion") };
    let PictureEffect::Resize(keys) = effect.effect else { panic!("expected Linear Motion") };
    assert_eq!(keys.map(|key| key.ticks), [start, start + last * (unit / 24)]);
    assert_eq!(keys.map(|key| key.position), [[-0.1, 0.025], [0.2, 0.025]]);

    g.sequences[0].frame_size = Some((1920, 1080));
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(prepared.input.picture_effects.is_empty());
    assert!(prepared.losses.iter().any(|loss| loss.object == id && loss.aspect == "picture_effect"));
    let (sequence, event) = prepared.ownership[&id];
    assert!(prepared.input.sequences[sequence].events[event].transform.is_none());
}

#[test]
fn spectramatte_under_static_warp_keeps_key_color_and_drops_unqualified_overlays() {
    use prproj::convert::prepared::PictureEffect;
    let mut graph = fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let media = graph.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap();
    media.rate = Some(rate);
    media.duration = Q::new(240 * 1001, 24_000).unwrap();
    let sequence = &mut graph.sequences[0];
    sequence.rate = rate;
    sequence.placements.retain(|p| p.video_track.is_some());
    sequence.tracks.push(SourceTrack::new(StreamKind::Video, 2, None));
    let placement = &mut sequence.placements[0];
    let id = placement.id.clone();
    placement.video_track = Some(2);
    placement.duration = Q::new(48 * 1001, 24_000).unwrap();
    placement.record_start = Q::ZERO;
    placement.source_in = Q::ZERO;
    placement.transform = None;
    sequence.markers.clear();
    let pair = |a, b| Value::Arr(vec![Value::Float(a), Value::Float(b)]);
    let keyer = |rgb: [i64; 3], square_pixels| v(vec![
        ("kind", Value::Str("avid.spectramatte".into())),
        ("params", v(vec![
            ("key_rgb8", Value::Arr(rgb.map(Value::Int).into())),
            ("square_pixels_verified", Value::Bool(square_pixels)),
            ("saved_progress_end_frame", Value::Int(47)),
        ])),
    ]);
    let warp = v(vec![
        ("kind", Value::Str("avid.warp_static".into())),
        ("params", v(vec![])),
        ("transform", v(vec![
            ("position", pair(-0.1, 0.0)),
            ("scale", pair(125.0, 125.0)),
            ("rotation", Value::Float(0.0)),
            ("anchor", pair(0.0, 0.0)),
            ("crop", Value::Arr([Value::Float(0.0), Value::Float(0.0), Value::Float(0.0), Value::Float(0.0)].into())),
        ])),
    ]);

    for (stages, qualified) in [
        (vec![keyer([20, 180, 100], true), warp.clone()], true),
        (vec![warp.clone(), keyer([20, 180, 100], true)], false),
        (vec![keyer([20, 180, 100], false), warp.clone()], false),
        (vec![keyer([20, 180, 200], true), warp], false),
    ] {
        graph.sequences[0].placements[0].effects = vec![graph::Effect {
            kind: graph::picture_chain::KIND.into(),
            params: v(vec![("stages", Value::Arr(stages))]),
        }];
        let saved = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
        let prepared = prepare(&saved, &options(), ClockProfile::LegacyImport).unwrap();
        if qualified {
            let [effect] = prepared.picture_effects.as_slice() else { panic!("expected editable key") };
            assert_eq!(effect.effect, PictureEffect::UltraKey { rgb: [20, 180, 100] });
            let transform = prepared.model.sequences[effect.sequence].events[effect.event].transform.as_ref().unwrap();
            assert_eq!(transform.scale, (125.0, 125.0));
            assert!(prepared.losses.iter().any(|l| l.object == id && l.aspect == "picture_effect_approximation"));
        } else {
            assert!(prepared.picture_effects.is_empty());
            assert!(!prepared.ownership.contains_key(&id));
            assert!(prepared.losses.iter().any(|l| l.object == id && l.reason.contains("Omitted SpectraMatte overlay")));
        }
    }
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

fn linked_avc_pcm_picture_at_rate(num: i128, den: i128) -> SourceGraph {
    let mut g = linked_avc_pcm_picture_fixture();
    let rate = Q::new(num, den).unwrap();
    let at_frame = |frame| Q::new(frame * den, num).unwrap();
    for media in &mut g.media {
        let original = media.original.as_mut().unwrap();
        media.duration = at_frame(240);
        if media.kind == StreamKind::Video {
            media.rate = Some(rate);
            original.time_base = at_frame(1);
            original.duration_ts = 240;
        } else {
            original.duration_ts = media.duration.to_units(48_000).unwrap();
        }
    }
    g.items[0].rate = Some(rate);
    let sequence = &mut g.sequences[0];
    sequence.rate = rate;
    sequence.placements.retain(|p| p.video_track.is_some());
    let p = &mut sequence.placements[0];
    p.source_in = at_frame(6);
    p.duration = at_frame(36);
    p.transform = Some(graph::Transform {
        position: (0.0, 0.0), scale: (100.0, 100.0),
        rotation: 0.0, anchor: (0.0, 0.0), crop: [0.0; 4],
    });
    g
}

#[test]
fn animated_crop_checks_the_linked_raster_in_preparation_and_writer_validation() {
    use prproj::convert::prepared::PictureEffect;

    for (num, den) in [(24, 1), (24_000, 1001), (25, 1), (30_000, 1001)] {
        let mut g = linked_avc_pcm_picture_at_rate(num, den);
        let p = &mut g.sequences[0].placements[0];
        let key = |frame, crop: [f64; 4]| v(vec![
            ("frame", Value::Int(frame)),
            ("position", Value::Arr(vec![Value::Int(0); 2])),
            ("scale", Value::Arr(vec![Value::Int(100); 2])),
            ("crop", Value::Arr(crop.map(Value::Float).into())),
        ]);
        let end = [0.1, 0.2, 0.3, 0.4];
        p.effects = vec![graph::Effect {
            kind: "avid.warp_crop_linear".into(),
            params: v(vec![("keys", Value::Arr(vec![key(0, [0.0; 4]), key(35, end)]))]),
        }];
        let id = p.id.clone();
        let mut prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
        prepared.input.validate().unwrap();
        let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected one crop") };
        let PictureEffect::Resize(keys) = effect.effect else { panic!("expected two Linear keys") };
        let frame = prproj::convert::model::TICKS_PER_SECOND / num as i64 * den as i64;
        assert_eq!(keys.map(|k| k.ticks), [6, 41].map(|n| n * frame));
        assert_eq!(keys.map(|k| k.crop), [Some([0.0; 4]), Some(end)]);

        prepared.input.sequences[0].width = 1280;
        assert!(prepared.input.validate().unwrap_err().to_string().contains("invalid Linear Motion"));

        g.sequences[0].frame_size = Some((1920, 1080));
        let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
        assert!(prepared.input.picture_effects.is_empty());
        assert!(prepared.losses.iter().any(|loss| loss.object == id && loss.aspect == "picture_effect"));
    }
}

#[test]
fn wide_trimmed_crop_keeps_independent_motion_and_crop_handles() {
    use prproj::convert::prepared::PictureEffect;

    let mut g = linked_avc_pcm_picture_at_rate(25, 1);
    let edit = &mut g.sequences[0].placements[0];
    let id = edit.id.clone();
    let motion: Vec<_> = [0, 18, 35].into_iter().map(|frame| v(vec![
        ("frame", Value::Int(frame)),
        ("position", Value::Arr(vec![Value::Int(0); 2])),
        ("height", Value::Int(100)),
    ])).collect();
    let width: Vec<_> = [0, 18, 44].into_iter().map(|frame|
        Value::Arr(vec![Value::Int(frame), Value::Int(100)])).collect();
    let params = |last| {
        let crop = [0, 20, last].into_iter().zip([0.0, 0.04, 0.1]).map(|(frame, edge)| v(vec![
            ("frame", Value::Int(frame)),
            ("edges", Value::Arr(vec![Value::Float(edge); 4])),
        ])).collect();
        v(vec![("motion", Value::Arr(motion.clone())), ("width", Value::Arr(width.clone())),
            ("fixed_width", Value::Bool(true)), ("crop", Value::Arr(crop))])
    };
    edit.effects = vec![graph::Effect { kind: "avid.warp_trim_crop_linear".into(), params: params(47) }];
    let mut prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    prepared.input.validate().unwrap();
    let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected a trimmed Warp") };
    let PictureEffect::TrimmedWarp(trim) = effect.effect else { panic!("expected trimmed Warp") };
    let frame = 10_160_640_000;
    assert_eq!(trim.motion.map(|key| key.ticks), [6, 24, 41].map(|n| n * frame));
    assert_eq!(trim.width.map(|key| key.0), [6, 24, 50].map(|n| n * frame));
    assert_eq!(trim.crop.unwrap().map(|key| key.0), [6, 26, 53].map(|n| n * frame));
    prepared.input.sequences[0].width = 1280;
    assert!(prepared.input.validate().unwrap_err().to_string().contains("invalid trimmed Linear Motion"));

    // The visible edit still fits when only the last crop key moves off media.
    g.sequences[0].placements[0].effects[0].params = params(234);
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(prepared.input.picture_effects.is_empty());
    assert!(prepared.losses.iter().any(|loss| loss.object == id
        && loss.reason.contains("qualified source-media handles")));
    let (sequence, event) = prepared.ownership[&id];
    assert_eq!(prepared.input.sequences[sequence].events[event].duration, 36 * frame);
    assert!(prepared.input.sequences[sequence].events[event].transform.is_none());
}

fn warp_roll_fixture(mut g: SourceGraph) -> SourceGraph {
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
    g
}

#[test]
fn warp_interpolation_uses_the_exact_source_key_times() {
    use prproj::convert::prepared::{PictureEffect, WarpRollInterpolation};
    let mut g = warp_roll_fixture(fixture());
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
fn linked_warp_roll_requires_matching_picture_geometry() {
    use prproj::convert::prepared::{PictureEffect, WarpRollInterpolation};

    let mut g = warp_roll_fixture(linked_avc_pcm_picture_fixture());
    let id = g.sequences[0].placements[0].id.clone();
    let mut prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    prepared.input.validate().unwrap();
    let [effect] = prepared.input.picture_effects.as_slice() else { panic!("expected one roll") };
    let PictureEffect::WarpRoll { keys, interpolation: WarpRollInterpolation::Hold } = effect.effect
        else { panic!("expected Hold roll keys") };
    assert_eq!(keys.map(|k| k.ticks), [2, 7, 14].map(|n| n * 10_584_000_000));
    assert_eq!(keys.map(|k| k.rotation), [-15.0, 25.0, -10.0]);
    prepared.input.sequences[0].width = 1280;
    assert!(prepared.input.validate().unwrap_err().to_string().contains("invalid Warp Rotation"));

    g.sequences[0].frame_size = Some((1920, 1080));
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(prepared.input.picture_effects.is_empty());
    assert!(prepared.losses.iter().any(|loss| loss.object == id && loss.aspect == "picture_effect"));
}

#[test]
fn warp_rotation_uses_the_selected_source_frame_grid() {
    use prproj::convert::prepared::{PictureEffect, WarpRollInterpolation};

    for (num, den) in [(24, 1), (24_000, 1001), (25, 1), (30_000, 1001)] {
        let mut g = warp_roll_fixture(linked_avc_pcm_picture_at_rate(num, den));
        let edit = &mut g.sequences[0].placements[0];
        edit.source_in = Q::new(17 * den, num).unwrap();
        edit.duration = Q::new(13 * den, num).unwrap();
        let frame = prproj::convert::model::TICKS_PER_SECOND / num as i64 * den as i64;
        for (kind, expected) in [
            ("avid.warp_roll_linear", WarpRollInterpolation::Linear),
            ("avid.warp_roll_shelf", WarpRollInterpolation::Hold),
            ("avid.warp_roll_bezier", WarpRollInterpolation::FlatBezier),
            ("avid.warp_roll_spline", WarpRollInterpolation::FlatBezier),
        ] {
            g.sequences[0].placements[0].effects[0].kind = kind.into();
            let mut prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
            prepared.input.validate().unwrap();
            let [picture] = prepared.input.picture_effects.as_mut_slice() else {
                panic!("expected one Warp rotation for {kind} at {num}/{den}");
            };
            let PictureEffect::WarpRoll { keys, interpolation } = &mut picture.effect else {
                panic!("expected editable Warp rotation");
            };
            assert_eq!(*interpolation, expected);
            assert_eq!(keys.map(|k| k.ticks), [17, 22, 29].map(|n| n * frame));
            keys[1].ticks += 1;
            assert!(prepared.input.validate().unwrap_err().to_string().contains("invalid Warp Rotation"));
        }
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

    for (num, den) in [(24, 1), (25, 1), (30_000, 1001)] {
        let mut at_rate = g.clone();
        let at_frame = |frames| Q::new(frames * den, num).unwrap();
        let rate = Q::new(num, den).unwrap();
        let media = at_rate.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap();
        media.rate = Some(rate);
        media.duration = at_frame(120);
        let seq = &mut at_rate.sequences[0];
        seq.rate = rate;
        seq.placements[0].source_in = at_frame(36);
        seq.placements[0].duration = at_frame(36);
        let prepared = prepare(&at_rate, &options(), ClockProfile::LegacyImport).unwrap();
        let PictureEffect::TrimmedWarp(trim) = prepared.picture_effects[0].effect else {
            panic!("expected trimmed Warp at {num}/{den}");
        };
        let frame = prproj::convert::model::TICKS_PER_SECOND / num as i64 * den as i64;
        assert_eq!(trim.motion.map(|k| k.ticks), [36, 54, 71].map(|n| n * frame));
        assert_eq!(trim.width.map(|(ticks, _)| ticks), [24, 47, 71].map(|n| n * frame));
    }

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
fn linked_audio_dissolve_into_saved_filler_keeps_the_exact_cut() {
    let mut g = two_edits();
    let s = &mut g.sequences[0];
    let mut later = s.placements.pop().unwrap();
    later.record_start = q(6);
    let p = &mut s.placements[0];
    for edit in [p, &mut later] {
        edit.video_track = None;
        edit.audio_tracks = vec![1];
        edit.audio_streams = vec![1];
    }
    s.placements.push(later);
    let t = &mut s.transitions[0];
    t.kind = StreamKind::Audio;
    t.cut = q(4);
    t.cut_offset = q(1);
    t.style = "avid.BLEND_AUDIO_DISSOLVE".into();
    t.incoming = SourceBinding {
        placements: vec![],
        reason: Some("native owner produced no source placements".into()),
    };
    let id = t.id.clone();
    let receipt = v(vec![
        ("id", Value::Str(id.clone())),
        ("effect", Value::Str("BLEND_AUDIO_DISSOLVE".into())),
        ("rate", Value::Str(s.rate.to_string())),
        ("audio_curve_attribute", Value::Int(1)),
        ("audio_curve_parameter", Value::Int(1)),
        ("incoming_filler_frames", Value::Int(48)),
    ]);
    let sidecar = g.sidecars.iter_mut().find(|sc| sc.object == s.id && sc.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut sidecar.data else { panic!("expected sequence facts") };
    fields.push(("native_transitions".into(), Value::Arr(vec![receipt])));

    let g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let clock = ClockProfile::LegacyImport;
    let clean_cuts = || {
        let mut clean = g.clone();
        clean.sequences[0].transitions.clear();
        let sidecar = clean.sidecars.iter_mut()
            .find(|sc| sc.object == clean.sequences[0].id && sc.adapter == "avid.source").unwrap();
        let Value::Obj(fields) = &mut sidecar.data else { panic!("expected sequence facts") };
        fields.retain(|(key, _)| key != "native_transitions");
        prepare(&clean, &options(), clock).unwrap()
    };
    for spooled in [false, true] {
        let p = premiere_adapter::prepare_linked_originals(&g, &options(), clock, spooled).unwrap();
        assert_eq!(p.input.sequences[0], clean_cuts().model.sequences[0]);
        assert!(p.input.audio_edge_fades.is_empty());
        assert!(p.losses.iter().any(|loss| loss.object == id && loss.aspect == "transition_cut"));
    }
    let refusal = |candidate: &SourceGraph| {
        assert!(premiere_adapter::prepare_linked_originals(candidate, &options(), clock, false)
            .err().unwrap().0.contains("unresolved_transition_topology"));
    };
    assert!(prepare(&g, &options(), clock).err().unwrap().0.contains("unresolved_transition_topology"));
    let mut wrong_cut = g.clone();
    wrong_cut.sequences[0].transitions[0].cut = q(5);
    refusal(&wrong_cut);
    let mut unrelated_owner = g.clone();
    unrelated_owner.sequences[0].transitions[0].incoming.reason = Some("ambiguous owner".into());
    refusal(&unrelated_owner);
    let mut wrong_rate = g.clone();
    let sidecar = wrong_rate.sidecars.iter_mut()
        .find(|sc| sc.object == g.sequences[0].id && sc.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut sidecar.data else { panic!("expected sequence facts") };
    let (_, Value::Arr(rows)) = fields.iter_mut().find(|(key, _)| key == "native_transitions").unwrap()
        else { panic!("expected transition rows") };
    let Value::Obj(row) = &mut rows[0] else { panic!("expected transition facts") };
    row.iter_mut().find(|(key, _)| key == "rate").unwrap().1 = Value::Str("25".into());
    refusal(&wrong_rate);
    let mut overlapping = g.clone();
    let mut duplicate = overlapping.sequences[0].transitions[0].clone();
    duplicate.id = "transition:two".into();
    overlapping.sequences[0].transitions.push(duplicate);
    refusal(&overlapping);
    let mut occupied = g;
    occupied.sequences[0].placements[1].record_start = Q::new(7, 2).unwrap();
    refusal(&occupied);
    occupied.sequences[0].placements[1].record_start = q(4);
    refusal(&occupied);
}

#[test]
fn linked_audio_dissolve_from_saved_filler_keeps_the_incoming_trim() {
    let mut g = two_edits();
    let s = &mut g.sequences[0];
    for edit in &mut s.placements {
        edit.video_track = None;
        edit.audio_tracks = vec![1];
        edit.audio_streams = vec![1];
    }
    s.placements[0].duration = q(1);
    let t = &mut s.transitions[0];
    t.kind = StreamKind::Audio;
    t.cut = q(3);
    t.cut_offset = Q::ZERO;
    t.style = "avid.BLEND_AUDIO_DISSOLVE".into();
    t.outgoing = SourceBinding {
        placements: vec![],
        reason: Some("native owner produced no source placements".into()),
    };
    let id = t.id.clone();
    let mut clean = g.clone();
    clean.sequences[0].transitions.clear();
    let clock = ClockProfile::LegacyImport;
    let base = prepare(&clean, &options(), clock).unwrap();
    let refusal = |candidate: &SourceGraph| {
        assert!(premiere_adapter::prepare_linked_originals(candidate, &options(), clock, false)
            .err().unwrap().0.contains("unresolved_transition_topology"));
    };
    refusal(&g);
    let sidecar = g.sidecars.iter_mut()
        .find(|sc| sc.object == g.sequences[0].id && sc.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut sidecar.data else { panic!("expected sequence facts") };
    fields.push(("native_transitions".into(), Value::Arr(vec![v(vec![
        ("id", Value::Str(id.clone())),
        ("effect", Value::Str("BLEND_AUDIO_DISSOLVE".into())),
        ("rate", Value::Str(g.sequences[0].rate.to_string())),
        ("outgoing_filler_frames", Value::Int(72)),
    ])])));
    let g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    for spooled in [false, true] {
        let prepared = premiere_adapter::prepare_linked_originals(&g, &options(), clock, spooled).unwrap();
        assert_eq!(prepared.input.sequences[0], base.model.sequences[0]);
        assert!(prepared.input.audio_edge_fades.is_empty());
        assert!(prepared.losses.iter().any(|loss| loss.object == id && loss.aspect == "transition_cut"
            && loss.reason.contains("kept the incoming edit")));
    }
    let mut mismatched_cut = g.clone();
    mismatched_cut.sequences[0].placements[1].record_start = Q::new(7, 2).unwrap();
    refusal(&mismatched_cut);
    let mut neighbor = g;
    neighbor.sequences[0].placements[0].duration = q(3);
    refusal(&neighbor);
    neighbor.sequences[0].placements[0].duration = Q::new(7, 2).unwrap();
    refusal(&neighbor);
}

#[test]
fn linked_picture_filler_dissolves_keep_existing_cuts_on_both_layers() {
    let effects = ["EFF2_BLEND_DISSOLVE", "EFF2_BLEND_DISSOLVE_LOG", "EFF2_BLEND_DISSOLVE_LIN"];
    for (effect, from_filler) in effects.into_iter().flat_map(|effect| [(effect, true), (effect, false)]) {
        let mut g = two_edits();
        let s = &mut g.sequences[0];
        s.placements[0].duration = q(8);
        s.placements[1].video_track = Some(2);
        s.tracks.push(SourceTrack::new(StreamKind::Video, 2, None));
        let t = &mut s.transitions[0];
        t.track = 2;
        t.style = format!("avid.{effect}");
        let filler = SourceBinding {
            placements: vec![],
            reason: Some("native owner produced no source placements".into()),
        };
        let mut edge = "outgoing_filler_frames";
        if from_filler {
            t.cut = q(3);
            t.cut_offset = Q::ZERO;
            t.outgoing = filler;
        } else {
            t.cut = q(7);
            t.cut_offset = t.duration;
            t.outgoing = t.incoming.clone();
            t.incoming = filler;
            edge = "incoming_filler_frames";
        }
        let id = t.id.clone();
        let clock = ClockProfile::LegacyImport;
        let mut clean = g.clone();
        clean.sequences[0].transitions.clear();
        let expected = prepare(&clean, &options(), clock).unwrap();
        let sidecar = g.sidecars.iter_mut()
            .find(|sc| sc.object == g.sequences[0].id && sc.adapter == "avid.source").unwrap();
        let Value::Obj(fields) = &mut sidecar.data else { panic!("expected sequence facts") };
        fields.push(("native_transitions".into(), Value::Arr(vec![v(vec![
            ("id", Value::Str(id.clone())),
            ("effect", Value::Str(effect.into())),
            ("rate", Value::Str(g.sequences[0].rate.to_string())),
            (edge, Value::Int(48)),
        ])])));
        let mut g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
        for spooled in [false, true] {
            let prepared = premiere_adapter::prepare_linked_originals(&g, &options(), clock, spooled).unwrap();
            assert_eq!(prepared.input.sequences[0], expected.model.sequences[0]);
            assert!(prepared.input.video_dissolves.is_empty());
            assert!(prepared.losses.iter().any(|loss| loss.object == id
                && loss.aspect == "transition_cut" && loss.reason.contains("V2")));
        }
        assert!(prepare(&g, &options(), clock).err().unwrap().0.contains("unresolved_transition_topology"));
        let mut wrong_effect = g.clone();
        wrong_effect.sequences[0].transitions[0].style = if effect == "EFF2_BLEND_DISSOLVE" {
            "avid.EFF2_BLEND_DISSOLVE_LOG".into()
        } else {
            "avid.EFF2_BLEND_DISSOLVE".into()
        };
        assert!(premiere_adapter::prepare_linked_originals(&wrong_effect, &options(), clock, false)
            .err().unwrap().0.contains("unresolved_transition_topology"));
        g.sequences[0].placements[0].video_track = Some(2);
        assert!(premiere_adapter::prepare_linked_originals(&g, &options(), clock, false)
            .err().unwrap().0.contains("unresolved_transition_topology"));
    }
}

#[test]
fn dissolve_at_gain_gap_keeps_both_channels_of_the_surviving_edit() {
    let mut g = two_edits();
    let seq = &mut g.sequences[0];
    seq.placements.remove(0);
    let p = &mut seq.placements[0];
    p.video_track = None;
    p.audio_tracks.clear();
    p.audio_streams.clear();
    p.grouped_audio = vec![graph::GroupedAudio {
        track: 1, streams: vec![1, 2], gain_db: Some(-3.0), pans: vec![Some(-100.0), Some(100.0)],
    }];
    let t = &mut seq.transitions[0];
    t.kind = StreamKind::Audio;
    t.style = "avid.BLEND_AUDIO_DISSOLVE".into();
    t.outgoing.placements.clear();
    t.outgoing.reason = Some("native owner includes an unresolved source branch".into());
    seq.markers.push(SourceMarker {
        name: "AudioSuite Gain omitted · A1".into(),
        comment: "unavailable".into(), user: String::new(), color: None,
        start: Some(Q::ZERO), duration: t.cut, track: None,
        owner: MarkerOwner::Sequence, owner_occurrence: "omitted-gain".into(),
        owner_local: Some(Q::ZERO), owner_rate: Some(seq.rate),
        binding: SourceBinding { placements: vec![], reason: None },
        source: None, inherited: false,
    });
    let receipt = g.sidecars.iter_mut().find(|p| p.object == seq.id && p.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut receipt.data else { unreachable!() };
    fields.push(("native_transitions".into(), Value::Arr(vec![v(vec![
        ("id", Value::Str(t.id.clone())),
        ("gain_gap_edges", Value::Arr(vec![Value::Str("omitted-gain".into()), Value::Null])),
    ])])));
    let mut g = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let seq = &prepared.model.sequences[0];
    let [edit] = seq.events.as_slice() else { panic!("expected one surviving edit"); };
    let hz = prproj::convert::model::TICKS_PER_SECOND;
    assert_eq!((edit.record_start, edit.source_in, edit.duration), (7 * hz / 2, 3 * hz / 2, 7 * hz / 2));
    assert!(seq.audio_crossfades.is_empty());
    assert_eq!(edit.stereo[0].channels, [1, 2]);
    assert_eq!(edit.stereo[0].gain_db, Some(-3.0));
    assert_eq!(edit.stereo[0].pans, [Some(-100.0), Some(100.0)]);
    assert!(prepared.losses.iter().any(|loss| loss.aspect == "transition_cut"));

    g.sequences[0].markers[0].duration = q(3);
    assert!(prepare(&g, &options(), ClockProfile::LegacyImport).err().unwrap().0
        .contains("range marker disagrees with the saved cut"));
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
    let unresolved = SourceMarker {
        name: String::new(),
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
    };
    let sequence = &mut g.sequences[0];
    let marker_index = sequence.markers.len();
    sequence.markers.push(unresolved.clone());
    sequence.markers.push(unresolved);
    let sequence_id = sequence.id.clone();
    let r = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(r.model.sequences[0].markers.is_empty());
    let losses: Vec<_> = r.losses.iter().filter(|l| l.aspect == "marker_timing").collect();
    let [first, second] = losses.as_slice() else {
        panic!("expected one timing loss per marker: {losses:?}");
    };
    for (index, entry) in [(marker_index, first), (marker_index + 1, second)] {
        assert_eq!(entry.object, sequence_id);
        assert!(entry.reason.starts_with(&format!("{sequence_id}/marker/{index}: ")));
        assert!(entry.reason.contains("owner-to-record transform unresolved"));
    }
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
fn off_clock_edit_marker_source_keeps_its_exact_sequence_range() {
    let mut g = coincident_marker_fixture();
    let source = &mut g.sequences[0].markers[0];
    source.source = Some(Q::new(1, 11).unwrap());
    source.duration = Q::new(3, 24).unwrap();
    source.inherited = true;
    let saved = SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap();
    let source = &saved.sequences[0].markers[0];

    let prepared = premiere_adapter::prepare_originals(&saved, &options(), ClockProfile::LegacyImport)
        .unwrap();
    let sequence = &prepared.input.sequences[0];
    assert_eq!(sequence.events.len(), 1);
    let [marker] = sequence.markers.as_slice() else {
        panic!("expected the exact edit marker range on the sequence");
    };
    let second = prproj::convert::model::TICKS_PER_SECOND;
    assert_eq!(marker.start, source.start.unwrap().to_units(second).unwrap());
    assert_eq!(marker.duration, 3 * (second / 24));
    assert_eq!((&marker.name, &marker.comment), (&source.name, &source.comment));
    assert_eq!(marker.track.as_deref(), Some("V1"));
    assert_eq!(marker.owner, "sequence");
    assert_eq!((marker.event, marker.source, marker.inherited), (None, None, false));
    let marker_losses: Vec<_> = prepared.losses.iter().filter(|l| l.aspect.starts_with("marker_")).collect();
    let [entry] = marker_losses.as_slice() else { panic!("expected one marker loss"); };
    assert_eq!(entry.aspect, "marker_ownership");
    assert!(entry.reason.starts_with(&format!("{}/marker/0:", saved.sequences[0].id)));
    assert!(entry.reason.contains("marker source: 1/11 is not on the Premiere tick clock"));

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let out = std::env::temp_dir().join(format!("anynle-edit-marker-fallback-{}-{timestamp}.prproj", std::process::id()));
    let mut ids = prproj::ids::SeededUuids::new("edit-marker-fallback");
    prproj::convert::writer::prepare_typed_project(&prepared.input, &out, &mut ids, &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&out).unwrap();
    std::fs::remove_file(&out).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let [stored] = project.sequence_markers(project.sequences()[0]) else {
        panic!("expected one saved sequence marker");
    };
    use prproj::model::marker::{marker_end, marker_name, marker_start};
    assert_eq!(marker_name(&project.doc.tree, *stored).unwrap(), source.name);
    assert_eq!(marker_start(&project.doc.tree, *stored).unwrap().ticks, marker.start);
    assert_eq!(marker_end(&project.doc.tree, *stored).unwrap().ticks, marker.start + marker.duration);
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
fn linked_preparation_omits_a_dissolve_only_when_the_unresolved_span_is_empty() {
    let mut g = two_edits();
    let sequence = &mut g.sequences[0];
    let mut transition = sequence.transitions.pop().unwrap();
    transition.style = "avid.EFF2_BLEND_DISSOLVE".into();
    transition.outgoing = SourceBinding {
        placements: vec![], reason: Some("native owner produced no source placements".into()),
    };
    transition.incoming = SourceBinding {
        placements: vec![], reason: Some("one or more native endpoints were not portable".into()),
    };
    sequence.transitions.clear();
    sequence.transitions.push(transition.clone());
    assert!(premiere_adapter::prepare_linked_originals(&g, &options(), ClockProfile::LegacyImport, true)
        .err().unwrap().0.contains("unresolved_transition_topology"));

    let sequence = &mut g.sequences[0];
    sequence.placements.clear();
    sequence.markers.clear();
    let out = premiere_adapter::prepare_linked_originals(&g, &options(), ClockProfile::LegacyImport, true).unwrap();
    assert!(out.input.sequences[0].audio_crossfades.is_empty());
    assert!(out.input.video_dissolves.is_empty());
    assert!(out.losses.iter().any(|l| l.object == transition.id && l.aspect == "transition_omitted"));
    assert_eq!(g.sequences[0].transitions, vec![transition]);
}

fn add_unqualified_offline_master(g: &mut SourceGraph) -> String {
    let mut unused = g.items[0].clone();
    unused.id = "clip:unused-offline".into();
    unused.name = "Unused offline".into();
    for binding in &mut unused.media {
        let mut offline = g.media.iter().find(|m| m.id == binding.media).unwrap().clone();
        offline.id.push_str(":offline");
        offline.online = Some(false);
        binding.media = offline.id.clone();
        g.media.push(offline);
    }
    g.bins[0].items.push(unused.id.clone());
    let id = unused.id.clone();
    g.items.push(unused);
    id
}

#[test]
fn spooled_linked_preparation_omits_offline_managed_masters_and_marks_exact_gaps() {
    let mut g = fixture();
    let offline = add_unqualified_offline_master(&mut g);
    let out = premiere_adapter::prepare_linked_originals(
        &g, &options(), ClockProfile::LegacyImport, true,
    ).unwrap();
    assert_eq!(out.input.clips.len(), 1);
    assert!(!out.input.bins[0].clips.iter().any(|id| id == "unused-offline"));
    assert!(out.losses.iter().any(|l| l.object == offline && l.aspect == "omitted_offline_bin_item"));
    let edit = &mut g.sequences[0].placements[0];
    edit.item = offline;
    let omitted = edit.id.clone();
    let range = (edit.record_start, edit.duration);
    let out = premiere_adapter::prepare_linked_originals(
        &g, &options(), ClockProfile::LegacyImport, true,
    ).unwrap();
    let marker = out.input.sequences[0].markers.iter()
        .find(|m| m.name == "Media omitted · V1").unwrap();
    let ticks = prproj::convert::model::TICKS_PER_SECOND;
    assert_eq!((Some(marker.start), Some(marker.duration)), (range.0.to_units(ticks), range.1.to_units(ticks)));
    assert!(out.losses.iter().any(|l| l.object == omitted && l.aspect == "media_gap"));
    assert!(premiere_adapter::prepare_linked_originals(
        &g, &options(), ClockProfile::LegacyImport, false,
    ).err().unwrap().0.contains("offline managed clips require"));

    let mut overlap = g.sequences[0].placements[0].clone();
    overlap.id = "overlap".into();
    overlap.item = g.items[0].id.clone();
    g.sequences[0].placements.push(overlap);
    let out = premiere_adapter::prepare_linked_originals(
        &g, &options(), ClockProfile::LegacyImport, true,
    ).unwrap();
    assert!(!out.input.sequences[0].markers.iter().any(|m| m.name == "Media omitted · V1"));
    assert!(out.ownership.contains_key("overlap"));
    assert!(out.losses.iter().any(|l| l.object == omitted && l.aspect == "offline_edit_omitted"));
}

#[test]
fn spooled_linked_preparation_keeps_the_saved_cut_beside_an_offline_endpoint() {
    let mut g = two_edits();
    let offline = add_unqualified_offline_master(&mut g);
    let sequence = &mut g.sequences[0];
    sequence.placements[1].item = offline;
    sequence.transitions[0].style = "avid.EFF2_BLEND_DISSOLVE".into();
    let transition = sequence.transitions[0].id.clone();
    let cut = sequence.transitions[0].cut;
    let out = premiere_adapter::prepare_linked_originals(
        &g, &options(), ClockProfile::LegacyImport, true,
    ).unwrap();
    let ticks = prproj::convert::model::TICKS_PER_SECOND;
    let sequence = &out.input.sequences[0];
    let [event] = sequence.events.as_slice() else { panic!("expected only the surviving edit") };
    assert_eq!(Some(event.record_start + event.duration), cut.to_units(ticks));
    let [gap] = sequence.markers.as_slice() else { panic!("expected one offline gap") };
    assert_eq!(Some(gap.start), cut.to_units(ticks));
    assert_eq!(Some(gap.start + gap.duration), q(7).to_units(ticks));
    assert!(out.input.video_dissolves.is_empty());
    assert!(out.losses.iter().any(|l| l.object == transition && l.aspect == "transition_omitted"));
}

#[test]
fn linked_preparation_reports_a_group_without_camera_media_or_bound_edits() {
    let mut g = fixture();
    let id = "multicam:offline".to_string();
    g.bins[0].items.push(id.clone());
    g.multicams.push(Multicam {
        id: id.clone(), name: "Offline cameras".into(), rate: Some(Q::integer(24)),
        duration: Some(Q::integer(1)), lanes: vec![], reason: Some("Camera routes unavailable".into()),
    });
    g.multicam_uses.push(MulticamUse {
        id: "multicam:offline-use".into(), sequence: g.sequences[0].id.clone(),
        candidates: vec![id.clone()], kind: StreamKind::Audio, lane: Some(1), track: 1,
        selected: None, record_start: None, duration: None, source_in: None,
        binding: SourceBinding { placements: vec![], reason: Some("No portable edit".into()) },
        reason: Some("Unresolved native range".into()),
    });
    let out = premiere_adapter::prepare_linked_originals(&g, &options(), ClockProfile::LegacyImport, true).unwrap();
    assert!(out.input.multicams.is_empty());
    assert!(out.losses.iter().any(|l| l.object == id && l.aspect == "group_omitted"));
    assert!(out.losses.iter().any(|l| l.object == "multicam:offline-use" && l.aspect == "group_use_omitted"));
    assert!(!out.input.sequences[0].events.is_empty());
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

fn coincident_marker_fixture() -> SourceGraph {
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
    g
}

#[test]
fn marker_inheritance_uses_exact_source_time_and_creation_content() {
    let mut g = coincident_marker_fixture();
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

    let mut undated = g.clone();
    for object in [&undated.items[0].id, &undated.sequences[0].id] {
        let sidecar = undated.sidecars.iter_mut()
            .find(|s| s.adapter == "avid.source" && s.object == *object).unwrap();
        let Value::Obj(fields) = &mut sidecar.data else { panic!("expected source receipt") };
        let Value::Arr(rows) = &mut fields.iter_mut().find(|(k, _)| k == "marker_native").unwrap().1
            else { panic!("expected native markers") };
        let Value::Obj(fields) = &mut rows[0] else { panic!("expected native marker") };
        fields.iter_mut().find(|(k, _)| k == "created").unwrap().1 = Value::Null;
    }
    let range = Q::new(1, 24).unwrap();
    undated.items[0].markers[0].duration = range;
    undated.sequences[0].markers[0].duration = range;
    let undated = SourceGraph::from_json_text(&undated.to_json_text().unwrap()).unwrap();
    let r = prepare(&undated, &options(), ClockProfile::LegacyImport).unwrap();
    let marker = &r.model.sequences[0].markers[0];
    assert_eq!(marker.owner, "edit");
    assert_eq!(marker.source, Some(3 * 254016000000));
    assert_eq!(marker.duration, 254016000000 / 24);
    assert!(!marker.inherited);
    assert_eq!(r.model.clips["clip-1"].markers[0].duration, marker.duration);

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
fn ambiguous_coincident_markers_reach_the_conflict_preference() {
    use prproj::convert::writer::prepare_typed_project;

    let g = coincident_marker_fixture();
    for duplicate_source in [true, false] {
        let mut graph = g.clone();
        let (object, markers) = if duplicate_source {
            let item = &mut graph.items[0];
            (&item.id, &mut item.markers)
        } else {
            let sequence = &mut graph.sequences[0];
            (&sequence.id, &mut sequence.markers)
        };
        let mut second = markers[0].clone();
        second.owner_occurrence.push_str("/second");
        markers.push(second);
        let receipt = graph.sidecars.iter_mut()
            .find(|sidecar| sidecar.adapter == "avid.source" && &sidecar.object == object)
            .unwrap();
        let Value::Obj(fields) = &mut receipt.data else { panic!("expected source receipt") };
        let Value::Arr(rows) = &mut fields.iter_mut().find(|(k, _)| k == "marker_native").unwrap().1
            else { panic!("expected native markers") };
        let mut second = rows[0].clone();
        let Value::Obj(fields) = &mut second else { panic!("expected native marker") };
        fields.iter_mut().find(|(k, _)| k == "graph_index").unwrap().1 = Value::Int(1);
        rows.push(second);

        let saved = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
        for winner in ["source", "edit"] {
            let opts = Options {
                marker_conflict_winner: winner.into(),
                marker_conflict_loser_to_sequence: true,
                ..options()
            };
            let prepared = prepare(&saved, &opts, ClockProfile::LegacyImport).unwrap();
            let source = &prepared.model.clips["clip-1"].markers;
            let edits = &prepared.model.sequences[0].markers;
            assert_eq!(source.len(), if duplicate_source { 2 } else { 1 });
            assert_eq!(edits.len(), if duplicate_source { 1 } else { 2 });
            assert!(edits.iter().all(|marker| !marker.inherited), "duplicate source: {duplicate_source}");

            let path = std::env::temp_dir().join(format!(
                "anynle-ambiguous-markers-{duplicate_source}-{winner}-{}-{}.prproj",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            ));
            let adapted = premiere_adapter::prepare_originals(&saved, &opts, ClockProfile::LegacyImport).unwrap();
            let mut ids = prproj::SeededUuids::new("ambiguous-marker-conflict");
            let report = prepare_typed_project(&adapted.input, &path, &mut ids, &[], false)
                .unwrap().publish().unwrap();
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            assert!(report.notes.iter().any(|note| note.contains(&format!(
                "the {winner} marker is kept, the other placed on the sequence"
            ))));
            let doc = prproj::PremiereDocument::from_bytes(&bytes).unwrap();
            let named_markers = doc.tree.iter_tag(doc.root, "DVAMarker").count();
            assert_eq!(named_markers, source.len() * 2 + edits.len());
        }
    }
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
    assert!(!r.losses.iter().any(|loss| loss.reason.contains("spans multiple edits")));

    let placements = &g.sequences[0].placements;
    let second = placements[1].id.clone();
    g.sequences[0].markers[0].binding.placements.push(second);
    let bound_range = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(bound_range.model, r.model);
    assert!(bound_range.losses.iter().any(|loss| {
        loss.aspect == "marker_ownership" && loss.reason.contains("spans multiple edits on V1")
            && loss.reason.contains("full range is preserved as a sequence marker")
    }));
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
fn linked_av_subclip_applies_the_common_backing_offset_once() {
    let mut graph = original_fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let frame = |count: i128| Q::new(count * 1001, 24_000).unwrap();
    let path = "/Synthetic/camera.mov";
    let parent = &mut graph.items[0];
    parent.rate = Some(rate);
    parent.stream_groups = vec![vec![1, 2]];
    let mob = format!("{:064X}", 1);
    parent.umid = Some(mob.clone());
    for media in &mut graph.media {
        let audio = media.kind == StreamKind::Audio;
        media.locations = vec![path.into()];
        media.online = Some(true);
        media.duration = frame(192);
        media.codec = Some(if audio { "pcm_s24le" } else { "apch" }.into());
        media.bits = audio.then_some(24);
        if !audio {
            media.rate = Some(rate);
            media.start = Some(frame(105_120));
            media.clock = Some(SourceClock {
                effective_count: 105_120, rate, display_base: 24, drop: false,
                provenance: SourceClockProvenance {
                    origin_count: 105_120, source_offset: 0, source_rate: rate,
                    source_package: mob.clone(), timecode_track: "TC1".into(),
                    route: AVID_LINKED_MOV_TIMECODE.into(),
                },
            });
        }
        let original = media.original.as_mut().unwrap();
        original.saved_locators = vec![path.into()];
        original.time_base = Q::new(1, if audio { 48_000 } else { 24_000 }).unwrap();
        original.duration_ts = media.duration.to_units(original.time_base.den()).unwrap();
        original.sample_representation = audio.then(|| "pcm24".into());
    }
    let subclip_receipt = |right_offset| Sidecar {
        adapter: "avid.subclips".into(),
        object: graph.bins[0].id.clone(),
        data: Value::Arr(vec![v(vec![
            ("id", Value::Str("bin/range".into())),
            ("name", Value::Str("Selected Range".into())),
            ("mob", Value::Str(format!("{:064X}", 2))),
            ("rate", Value::Str(rate.to_string())),
            ("backing_length", Value::Int(174)),
            ("start", v(vec![("value", Value::Int(2))])),
            ("end", v(vec![("value", Value::Int(172))])),
            ("tracks", Value::Arr([(1, 1, 8), (2, 1, 8), (2, 2, right_offset)]
                .into_iter().map(|(kind, lane, offset)| v(vec![
                    ("kind", Value::Int(kind)),
                    ("label", Value::Int(lane)),
                    ("target_track", Value::Int(lane)),
                    ("target_offset", Value::Int(offset)),
                    ("component_class", Value::Str("SCLP".into())),
                    ("rate", Value::Str(rate.to_string())),
                    ("length", Value::Int(174)),
                    ("target_mob", Value::Str(mob.clone())),
                ])).collect())),
        ])]),
    };
    let correct = subclip_receipt(8);
    let conflicting = subclip_receipt(9);
    let sequence = &mut graph.sequences[0];
    sequence.rate = rate;
    sequence.markers.clear();
    let to_urn = |id| format!("urn:smpte:umid:00000000.00000000.00000000.00000000.00000000.00000000.00000000.{id:08x}");
    for edit in &mut sequence.placements {
        let (kind, lane) = match edit.video_track {
            Some(_) => (1, 1),
            None => (2, edit.audio_streams[0]),
        };
        let local_in = if lane == 2 { 13 } else { 10 };
        edit.record_start = Q::ZERO;
        edit.source_in = frame(local_in + 8);
        edit.duration = frame(24);
        let hop = |id, start, length| v(vec![
            ("mob", Value::Str(to_urn(id))),
            ("rate", Value::Str(rate.to_string())),
            ("track", Value::Int(lane)),
            ("source_offset", v(vec![("value", Value::Int(start))])),
            ("length", v(vec![("value", Value::Int(length))])),
        ]);
        graph.sidecars.push(Sidecar {
            adapter: "avid.source".into(),
            object: edit.id.clone(),
            data: v(vec![
                ("native_kind", Value::Int(kind)),
                ("route", Value::Arr(vec![hop(2, local_in as i64, 24), hop(1, 8, 174)])),
            ]),
        });
    }
    graph.sidecars.push(correct);
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepare = |graph| premiere_adapter::prepare_avid_job(
        graph, &options(), ClockProfile::LegacyImport, false, true, None,
    ).unwrap_or_else(|issue| panic!("{}", issue.message));
    let prepared = prepare(&graph);
    let [item] = prepared.input.av_subclips.as_slice() else { panic!("{:?}", prepared.losses) };
    let ticks = prproj::convert::model::TICKS_PER_SECOND / 24_000 * 1001;
    assert_eq!((item.bin, item.start, item.end), (0, 10 * ticks, 180 * ticks));
    assert_eq!(prepared.input.av_subclip_uses.len(), 3);
    for usage in &prepared.input.av_subclip_uses {
        let event = &prepared.input.sequences[usage.sequence].events[usage.event];
        let source = if usage.audio_track == Some(2) { 21 } else { 18 };
        assert_eq!((usage.subclip, event.source_in, event.duration), (0, source * ticks, 24 * ticks));
    }
    prepared.input.validate().unwrap();
    let mut bad_route = graph.clone();
    let a2 = bad_route.sequences[0].placements.iter().find(|p| p.audio_tracks == [2]).unwrap();
    let source = bad_route.sidecars.iter_mut().find(|s| s.object == a2.id && s.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut source.data else { unreachable!() };
    let Value::Arr(route) = &mut fields.iter_mut().find(|(k, _)| k == "route").unwrap().1 else { unreachable!() };
    let Value::Obj(hop) = &mut route[1] else { unreachable!() };
    hop.iter_mut().find(|(k, _)| k == "source_offset").unwrap().1 = v(vec![("value", Value::Int(7))]);
    let refused_use = prepare(&bad_route);
    assert_eq!(refused_use.input.av_subclips, prepared.input.av_subclips);
    assert_eq!(refused_use.input.av_subclip_uses.len(), 2);
    assert!(refused_use.losses.iter().any(|loss| loss.aspect == "subclip_timeline_source" && loss.object == a2.id));
    assert_eq!(refused_use.input.sequences, prepared.input.sequences);
    let mut invalid = graph.clone();
    *invalid.sidecars.last_mut().unwrap() = conflicting;
    let prepared = prepare(&invalid);
    assert!(prepared.input.av_subclips.is_empty());
    assert!(prepared.losses.iter().any(|loss| loss.aspect == "subclip_bin_item"
        && loss.reason.contains("common backing offset")));
}

#[test]
fn retime_refusals_keep_the_saved_edit_and_original_diagnostic() {
    use premiere_adapter::{prepare_avid_job, prepare_linked_originals, PreparationOccurrence};

    let mut g = original_fixture();
    g.sequences[0].markers.clear();
    g.sequences[0].placements.retain(|edit| edit.video_track.is_some());
    let mut second = g.sequences[0].clone();
    second.id = "second".into();
    second.placements[0].id = "second/retime".into();
    second.placements[0].record_start = q(2);
    second.placements[0].source_in = q(0);
    second.placements[0].duration = q(1);
    g.sequences.push(second);
    g.sidecars.push(Sidecar {
        adapter: "avid.source".into(),
        object: "second".into(),
        data: v(vec![("compatibility_start", Value::Str("3600".into()))]),
    });
    let options = options();
    let profile = ClockProfile::LegacyImport;
    let prepare = |g: &SourceGraph| prepare_avid_job(g, &options, profile, false, false, None);
    if let Err(issue) = prepare(&g) {
        panic!("the straight-cut control must prepare: {}", issue.message);
    }

    g.sequences[1].placements[0].speed = Some(graph::Speed { ratio: (1, 2), source_duration: q(2) });
    let check = |g: &SourceGraph, message| {
        let issue = prepare(g).err().expect("retime must refuse preparation");
        let legacy = prepare_linked_originals(g, &options, profile, false).err().unwrap();
        assert_eq!(issue.message, message);
        assert_eq!(issue.message, legacy.to_string());
        assert_eq!(issue.sequence.as_deref(), Some("second"));
        assert_eq!(issue.occurrence, Some(PreparationOccurrence::Placement("second/retime".into())));
    };
    check(&g, "original-file retiming is not qualified");

    g.sidecars.push(Sidecar {
        adapter: "avid.source".into(),
        object: "second/retime".into(),
        data: v(vec![("constant_speed", Value::Arr(vec![Value::Int(2)]))]),
    });
    check(&g, "invalid saved Avid constant-speed witness");
}

#[test]
fn linked_aac_origin_preserves_stereo_source_and_independent_audio_edits() {
    use prproj::convert::prepared::PreparedClip;

    let mut g = original_fixture();
    g.items[0].stream_groups = vec![vec![1, 2]];
    for m in g.media.iter().filter(|m| m.kind == StreamKind::Audio) {
        let side = g.sidecars.iter_mut().find(|sc|
            sc.adapter == "avid.source" && sc.object == m.id).unwrap();
        let Value::Obj(fields) = &mut side.data else { panic!("expected receipt") };
        fields.extend([
            ("same_rate_aac_file_track_start".into(), Value::Int(1)),
            ("verified_aac_priming_samples".into(), Value::Int(1024)),
        ]);
    }
    let frame = Q::new(1, 24).unwrap();
    let mut expected = Vec::new();
    for p in &mut g.sequences[0].placements {
        if let [channel] = p.audio_streams.as_slice() {
            // Use different source trims on the two lanes to catch accidental
            // recombination into one stereo edit using just one trim.
            if *channel == 2 {
                p.source_in = p.source_in.checked_add(frame).unwrap();
            }
            let source = p.source_in.checked_add(frame).unwrap();
            expected.push((p.id.clone(), source, *channel));
            g.sidecars.push(Sidecar {
                adapter: "avid.same_rate_aac".into(),
                object: p.id.clone(),
                data: v(vec![
                    ("file_track_start", Value::Int(1)),
                    ("saved_source_in", Value::Str(p.source_in.to_string())),
                ]),
            });
            p.source_in = source;
        }
    }
    assert_eq!(expected.len(), 2);
    let encoded = g.to_json_text().unwrap();
    let mut g = SourceGraph::from_json_text(&encoded).unwrap();
    let prepared = premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let [PreparedClip::Original(mono), PreparedClip::Original(stereo)] = prepared.input.clips.as_slice()
        else { panic!("expected two prepared views of one original") };
    assert_eq!(mono.groups, [vec![1], vec![2]]);
    assert_eq!(stereo.groups, [vec![1, 2]]);
    assert_eq!(mono.path, stereo.path);
    for (id, source, channel) in &expected {
        let (s, e) = prepared.ownership[id];
        let edit = &prepared.input.sequences[s].events[e];
        assert_eq!(edit.clip, mono.metadata.id);
        assert_eq!(edit.source_in, source.to_units(prproj::convert::model::TICKS_PER_SECOND).unwrap());
        assert_eq!(edit.audio_channels, [*channel]);
    }
    assert!(prepared.losses.iter().any(|loss| loss.aspect == "original_audio_insertion"));
    let receipt = g.sidecars.iter_mut().find(|sc| sc.object == expected[0].0
        && sc.adapter == "avid.same_rate_aac").unwrap();
    receipt.data = v(vec![("file_track_start", Value::Int(0))]);
    assert!(premiere_adapter::prepare_originals(&g, &options(), ClockProfile::LegacyImport)
        .err().unwrap().to_string().contains("verified one-frame offset"));
}

#[test]
fn timecoded_hd_aac_keeps_file_clock_and_picture_markers_separate_from_edit_trims() {
    use prproj::convert::{model::TICKS_PER_SECOND, prepared::PreparedClip};

    let mut graph = original_fixture();
    let rate = Q::new(24_000, 1001).unwrap();
    let frame = |count: i128| Q::new(count * 1001, 24_000).unwrap();
    let ticks = |count| frame(count).to_units(TICKS_PER_SECOND).unwrap();
    graph.items[0].rate = Some(rate);
    graph.items[0].stream_groups.clear();
    graph.items[0].markers.push(SourceMarker {
        name: "Source cue".into(), comment: String::new(), user: String::new(),
        color: Some(Label { palette: "avid".into(), name: "Green".into() }),
        start: Some(frame(30)), duration: Q::ZERO,
        track: Some(TrackRef { kind: StreamKind::Video, index: 1 }),
        owner: MarkerOwner::Sequence, owner_occurrence: "test:source".into(),
        owner_local: Some(frame(30)), owner_rate: Some(rate),
        binding: SourceBinding { placements: Vec::new(), reason: None },
        source: None, inherited: false,
    });
    for media in &mut graph.media {
        let original = media.original.as_mut().unwrap();
        media.duration = frame(192);
        if media.kind == StreamKind::Video {
            media.rate = Some(rate);
            media.start = Some(frame(102_240));
            media.clock = Some(SourceClock {
                effective_count: 102_240, rate, display_base: 24, drop: false,
                provenance: SourceClockProvenance {
                    origin_count: 102_240, source_offset: 0, source_rate: rate,
                    source_package: "synthetic".into(), timecode_track: "tmcd/2".into(),
                    route: VERIFIED_LINKED_MOV_TIMECODE.into(),
                },
            });
            original.time_base = Q::new(1, 24_000).unwrap();
        } else {
            let receipt = graph.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && s.object == media.id).unwrap();
            let Value::Obj(fields) = &mut receipt.data else { panic!("expected media receipt") };
            fields.extend([
                ("same_rate_aac_file_track_start".into(), Value::Int(1)),
                ("verified_aac_priming_samples".into(), Value::Int(1024)),
            ]);
        }
        original.duration_ts = media.duration.to_units(original.time_base.den()).unwrap();
    }
    let sequence = &mut graph.sequences[0];
    sequence.rate = rate;
    sequence.markers.clear();
    for edit in &mut sequence.placements {
        edit.record_start = Q::ZERO;
        edit.duration = frame(168);
        let source = match edit.audio_tracks.first() {
            Some(&lane) => {
                let saved = frame(12 + i128::from(lane));
                graph.sidecars.push(Sidecar {
                    adapter: "avid.same_rate_aac".into(), object: edit.id.clone(),
                    data: v(vec![("file_track_start", Value::Int(1)),
                        ("saved_source_in", Value::Str(saved.to_string()))]),
                });
                13 + i128::from(lane)
            }
            None => 12,
        };
        edit.source_in = frame(source);
    }
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepare = |g| premiere_adapter::prepare_originals(g, &options(), ClockProfile::LegacyImport);
    let prepared = prepare(&graph).unwrap();
    prepared.input.validate().unwrap();
    let [PreparedClip::Original(mono), PreparedClip::Original(master)] = prepared.input.clips.as_slice()
        else { panic!("expected a picture master and a mono edit view") };
    assert_eq!(master.video.as_ref().unwrap().source_start, ticks(102_240));
    assert_eq!(master.metadata.markers[0].start, ticks(30));
    assert_eq!(master.metadata.markers[0].track.as_deref(), Some("V1"));
    assert!(mono.metadata.markers.is_empty());
    for edit in &graph.sequences[0].placements {
        let (sequence, occurrence) = prepared.ownership[&edit.id];
        let output = &prepared.input.sequences[sequence].events[occurrence];
        assert_eq!(output.source_in, edit.source_in.to_units(TICKS_PER_SECOND).unwrap());
        assert_eq!(output.audio_channels, edit.audio_streams);
    }
    let mut unverified = graph.clone();
    unverified.media.iter_mut().find(|m| m.kind == StreamKind::Video).unwrap()
        .clock.as_mut().unwrap().provenance.timecode_track = "unverified".into();
    assert!(prepare(&unverified).err().unwrap().to_string().contains("verified same-rate source"));
}

#[test]
fn animatte_acceleration50_samples_paths_without_mutating_the_source_graph() {
    let mut g = fixture();
    let scene = &mut g.sequences[0];
    let frame = Q::new(1, 24).unwrap();
    scene.frame_size = Some((1920, 1080));
    let p = scene.placements.iter_mut().find(|p| p.video_track.is_some()).unwrap();
    p.duration = Q::new(5, 24).unwrap();
    let point = |x| Value::Arr([x, 0.25, 0.0, 0.0, 0.0, 0.0].map(Value::Float).into());
    let key = |f, x| v(vec![
        ("frame", Value::Int(f)),
        ("points", Value::Arr(vec![point(x), point(x + 0.1), point(x + 0.2)])),
    ]);
    let shape = |keyframes| v(vec![
        ("opacity", Value::Float(100.0)),
        ("feather", Value::Arr(vec![Value::Float(0.0); 2])),
        ("keyframes", Value::Arr(keyframes)),
    ]);
    let matte = |inverted| graph::Effect {
        kind: "avid.animatte".into(),
        params: v(vec![
            ("inverted", Value::Bool(inverted)),
            ("acceleration_q30", Value::Int(1 << 29)),
            ("shapes", Value::Arr(vec![
                shape(vec![key(0, 0.0), key(4, 1.0)]),
                shape(vec![key(0, 0.1)]),
            ])),
        ]),
    };
    p.source_in = frame;
    p.effects = vec![matte(false)];
    let source_text = g.to_json_text().unwrap();
    let saved = SourceGraph::from_json_text(&source_text).unwrap();
    let prepared = prepare(&saved, &options(), ClockProfile::LegacyImport).unwrap();
    let event = prepared.model.sequences[0].events.iter()
        .find(|e| e.matte.is_some()).unwrap();
    let m = event.matte.as_ref().unwrap();
    assert_eq!(m.shapes[0].iter().map(|(f, _)| *f).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
    assert!(m.shapes[0][1].1[0][0] < 0.25);
    assert_eq!(m.shapes[0][2].1[0][0], 0.5);
    assert_eq!(m.shapes[0][4].1[0][0], 1.0);
    assert_eq!(m.shapes[1].len(), 1);
    assert!(prepared.losses.iter().any(|l| l.aspect == "matte_approximation"));
    assert_eq!(g.to_json_text().unwrap(), source_text);

    g.sequences[0].placements.iter_mut().find(|p| p.video_track.is_some()).unwrap()
        .effects = vec![matte(true)];
    let refused = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
    let m = refused.model.sequences[0].events.iter()
        .find_map(|e| e.matte.as_ref()).unwrap();
    assert_eq!(m.shapes[0].len(), 2);
    let owner = g.sequences[0].placements.iter().find(|p| p.video_track.is_some()).unwrap();
    assert!(refused.losses.iter().any(|l| l.aspect == "matte"
        && l.object == owner.id && l.reason.contains("omits this multi-shape AniMatte Key Out")));
    assert!(!refused.losses.iter().any(|l| l.aspect == "matte_approximation"));
}

#[test]
fn two_static_hard_key_out_shapes_write_an_ordered_mask_union() {
    use prproj::convert::prepared::PictureEffect;
    use prproj::convert::writer::prepare_typed_project;

    let mut graph = fixture();
    let scene = &mut graph.sequences[0];
    scene.frame_size = Some((1920, 1080));
    let placement = scene.placements.iter_mut().find(|p| p.video_track.is_some()).unwrap();
    placement.transform = None;
    let rectangle = |left, top| {
        let points = [(left, top), (left + 0.4, top), (left + 0.4, top + 0.4), (left, top + 0.4)]
            .map(|(x, y)| Value::Arr([x, y, 0.0, 0.0, 0.0, 0.0].map(Value::Float).into()));
        v(vec![
            ("opacity", Value::Int(100)),
            ("feather", Value::Arr(vec![Value::Int(0); 2])),
            ("keyframes", Value::Arr(vec![v(vec![
                ("frame", Value::Int(0)),
                ("points", Value::Arr(points.into())),
            ])])),
        ])
    };
    placement.effects = vec![graph::Effect {
        kind: "avid.animatte".into(),
        params: v(vec![
            ("inverted", Value::Bool(true)),
            ("shapes", Value::Arr(vec![rectangle(0.1, 0.2), rectangle(0.3, 0.3)])),
        ]),
    }];
    let prepare = |g| premiere_adapter::prepare_originals(g, &options(), ClockProfile::LegacyImport);
    let saved = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let input = prepare(&saved).unwrap().input;
    assert_eq!(input.picture_effects.len(), 1);
    assert_eq!(input.picture_effects[0].effect, PictureEffect::MatteKeyOutUnion);

    let mut bad = input.clone();
    let effect = &bad.picture_effects[0];
    bad.sequences[effect.sequence].events[effect.event].matte.as_mut().unwrap().opacities[1] = 50.0;
    assert!(bad.validate().is_err());

    let dir = std::env::temp_dir().join(format!("anynle-two-shape-key-out-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("Mask union.prproj");
    prepare_typed_project(&input, &path, &mut prproj::SeededUuids::new("key-out-union"), &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
    let masks: Vec<_> = doc.tree.iter_tag(doc.root, "VideoFilterComponent")
        .filter(|&node| doc.tree.findtext(node, "MatchName").unwrap().as_deref() == Some("AE.ADBE AEMask2"))
        .collect();
    let saved_setting = |mask, id: &str| {
        doc.tree.findall(mask, "Component/Params/Param").unwrap().into_iter()
            .map(|reference| doc.resolve(reference).unwrap())
            .find(|&param| doc.tree.findtext(param, "ParameterID").unwrap().as_deref() == Some(id))
            .and_then(|param| doc.tree.findtext(param, "StartKeyframe").unwrap())
            .map(|start| start.split(',').nth(1).unwrap().to_owned()).unwrap()
    };
    assert_eq!(masks.len(), 2);
    assert_eq!(saved_setting(masks[0], "17"), "true");
    assert_eq!(saved_setting(masks[0], "21"), "0");
    assert_eq!(saved_setting(masks[1], "17"), "false");
    assert_eq!(saved_setting(masks[1], "21"), "1");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn partial_animatte_key_out_keeps_its_mask_and_reports_the_compositing_difference() {
    for (inverted, opacity, expect_notice) in [
        (true, 50, true),
        (true, 100, false),
        (false, 50, false),
    ] {
        let mut g = fixture();
        let placement = g.sequences[0].placements.iter_mut()
            .find(|p| p.video_track.is_some()).unwrap();
        let owner = placement.id.clone();
        let points = [[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]]
            .map(|[x, y]| Value::Arr([x, y, 0.0, 0.0, 0.0, 0.0].map(Value::Float).into()));
        placement.effects = vec![graph::Effect {
            kind: "avid.animatte".into(),
            params: v(vec![
                ("inverted", Value::Bool(inverted)),
                ("shapes", Value::Arr(vec![v(vec![
                    ("opacity", Value::Int(opacity)),
                    ("feather", Value::Arr(vec![Value::Int(0); 2])),
                    ("keyframes", Value::Arr(vec![v(vec![
                        ("frame", Value::Int(0)),
                        ("points", Value::Arr(points.into())),
                    ])])),
                ])])),
            ]),
        }];
        let prepared = prepare(&g, &options(), ClockProfile::LegacyImport).unwrap();
        let (sequence, event) = prepared.ownership[&owner];
        let matte = prepared.model.sequences[sequence].events[event].matte.as_ref().unwrap();
        assert_eq!(matte.inverted, inverted);
        assert_eq!(matte.opacities, [opacity as f64]);
        let notices = prepared.losses.iter().filter(|loss| {
            loss.aspect == "matte_approximation" && loss.reason.starts_with("Partial AniMatte Key Out")
        }).collect::<Vec<_>>();
        assert_eq!(notices.len(), usize::from(expect_notice));
        if let Some(notice) = notices.first() {
            assert_eq!(notice.object, owner);
        }
    }
}

#[test]
fn graph_media_round_trips_beyond_linked_destination_profiles() {
    fn check(graph: SourceGraph, owner: usize, reason: &str) {
        let saved = graph.to_json_text().unwrap();
        let reloaded = SourceGraph::from_json_text(&saved).unwrap();
        assert_eq!(reloaded, graph);
        let err = premiere_adapter::prepare_originals(&reloaded, &options(), ClockProfile::LegacyImport)
            .err().unwrap();
        assert_eq!(err.to_string(), format!("source media {:?}: {reason}", graph.media[owner].id));
    }

    let mut graph = original_fixture();
    graph.media[0].original.as_mut().unwrap().start_pts = -1024;
    check(graph, 0, "unsupported original stream origin or extent");

    let mut graph = original_fixture();
    let media = &mut graph.media[1];
    media.bits = Some(24);
    media.codec = Some("pcm_s24le".into());
    let original = media.original.as_mut().unwrap();
    original.sample_representation = Some("pcm24".into());
    original.container = "wav".into();
    original.stream_index = 0;
    original.channels = Some(8);
    original.channel_index = Some(7);
    check(graph, 1, "invalid physical audio channel");

    for representation in [None, Some("pcm32")] {
        let mut graph = original_fixture();
        let media = &mut graph.media[1];
        media.bits = Some(32);
        media.codec = Some("pcm_s32le".into());
        media.original.as_mut().unwrap().sample_representation = representation.map(str::to_owned);
        check(graph, 1, "audio sample representation contradicts bit depth or media profile");
    }

    let mut graph = original_fixture();
    let media = &mut graph.media[0];
    media.codec = Some("tiff".into());
    media.frame_size = Some((2048, 1556));
    media.rate = Some(q(25));
    let original = media.original.as_mut().unwrap();
    original.container = "tiff".into();
    original.time_base = Q::new(1, 25).unwrap();
    original.duration_ts = 1;
    check(graph, 0, "invalid original still or editorial hold");
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
    g.losses.extend(g.sequences[0].texts.iter().map(|text| graph::Loss {
        object: text.id.clone(),
        aspect: "text_appearance".into(),
        reason: "Original appearance is retained in a native receipt.".into(),
    }));
    let before=g.to_json_text().unwrap();
    assert_eq!(SourceGraph::from_json_text(&before).unwrap(),g);
    let p=prepare(&g,&options(),ClockProfile::LegacyImport).unwrap();
    assert_eq!(p.texts.len(),2);assert_eq!(g.to_json_text().unwrap(),before);
    assert_eq!(p.texts[0].start,2*prproj::convert::model::TICKS_PER_SECOND);assert!(p.texts.iter().all(|t|t.content.contains("café\n")));
    for cue in &g.sequences[0].texts {
        let reports: Vec<_> = p.losses.iter().filter(|loss| loss.object == cue.id).collect();
        let [report] = reports.as_slice() else { panic!("expected one report for {}", cue.id) };
        assert_eq!(report.aspect, "text_destination_style");
    }
    let mut bad=g.clone();bad.sequences[0].texts[0].record_start=None;
    assert!(bad.validate().is_err());
    bad.sequences[0].texts[0].reason=Some("owner clock unresolved".into());
    let p=prepare(&bad,&options(),ClockProfile::LegacyImport).unwrap();assert_eq!(p.texts.len(),1);assert!(p.losses.iter().any(|l|l.object=="text:caption"&&l.reason.contains("clock")));
    assert!(p.losses.contains(&g.losses[0]));
    assert!(!p.losses.iter().any(|loss| loss.object == "text:caption" && loss.aspect == "text_destination_style"));
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
fn static_motion_on_multicam_requires_the_avid_warp_input_witness() {
    let mut graph = fixed_audio_group();
    let usage = graph.multicam_uses.iter().find(|usage| usage.kind == StreamKind::Video).unwrap();
    let usage_id = usage.id.clone();
    let placement = graph.sequences[0].placements.iter_mut()
        .find(|placement| usage.binding.placements.contains(&placement.id)).unwrap();
    placement.transform = Some(graph::Transform {
        position: (0.0, 0.0),
        scale: (63.0, 63.0),
        rotation: 0.0,
        anchor: (0.0, 0.0),
        crop: [0.0; 4],
    });
    let prepare = |graph: &SourceGraph| {
        premiere_adapter::prepare_originals(graph, &options(), ClockProfile::VerifiedRewrap)
    };
    assert!(prepare(&graph).is_err());
    graph.sidecars.push(Sidecar {
        adapter: "avid.group_use".into(),
        object: usage_id,
        data: v(vec![("warp_picture_input", Value::Bool(true))]),
    });
    let loaded = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&loaded).unwrap();
    let video = prepared.input.multicam_uses.iter().find(|usage| usage.video.is_some()).unwrap();
    assert!(video.warp_picture_input);
    assert_eq!(video.video, Some(1));
    assert!(prepared.input.sequences[video.sequence].events[video.event].transform.is_some());
}

#[test]
fn one_mono_group_preserves_second_default_picture_and_first_camera_audio() {
    use prproj::convert::multicam::PreparedGroupAudio::Group;
    let mut graph = fixed_audio_group();
    let lanes = &mut graph.multicams[0].lanes;
    lanes[0].selected = Some(1);
    lanes.retain(|lane| lane.kind != StreamKind::Audio || lane.index == 1);
    graph.multicam_uses.retain(|usage| usage.kind != StreamKind::Audio || usage.track == 1);
    graph.sequences[0].placements.retain(|edit| edit.audio_tracks != [2]);
    let prepared = premiere_adapter::prepare_originals(&graph, &options(), ClockProfile::VerifiedRewrap)
        .unwrap();
    let [group] = prepared.input.multicams.as_slice() else { panic!("one native Group") };
    assert_eq!(group.cameras[0].clip, "camera-b");
    assert_eq!(group.audio_cameras, [1]);
    assert!(prepared.input.multicam_uses.iter().any(|usage| usage.video == Some(0)));
    assert!(prepared.input.multicam_uses.iter().any(|usage| usage.audio == [Group(0)]));
}

fn linked_group_camera_fixture() -> SourceGraph {
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
    g
}

#[test]
fn layered_submaster_preserves_two_source_ranges_and_declines_an_overlap() {
    let rate_23976 = Q::new(24000, 1001).unwrap();
    for (rate, dnxhd) in [(rate_23976, false), (rate_23976, true), (q(24), true)] {
        check_layered_submaster(rate, dnxhd);
    }
}

fn check_layered_submaster(rate: Q, dnxhd: bool) {
    let mut graph = linked_group_camera_fixture();
    graph.multicams.clear();
    graph.multicam_uses.clear();
    graph.bins[0].items.retain(|id| id != "synthetic-group");
    let frame = |n: i128| Q::new(n * i128::from(rate.den()), i128::from(rate.num())).unwrap();
    let exact_24 = rate == q(24);
    if dnxhd {
        let sample_count = if exact_24 { 120000 } else { 120120 };
        let bits = if exact_24 { 16 } else { 24 };
        for item in &mut graph.items {
            item.rate = Some(rate);
        }
        for m in &mut graph.media {
            let picture = m.kind == StreamKind::Video;
            m.duration = frame(60);
            let clock = m.clock.as_mut().unwrap();
            clock.rate = rate;
            let descriptor = if picture {
                m.rate = Some(rate);
                clock.provenance.source_rate = rate;
                v(vec![
                    ("compression", Value::Str("AVHD".into())),
                    ("resolution_id", Value::Int(1253)),
                    ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)])),
                ])
            } else {
                m.bits = Some(bits);
                v(vec![("channels", Value::Int(1)), ("bits", Value::Int(bits))])
            };
            let side = graph.sidecars.iter_mut().find(|s| s.object == m.id && s.adapter == "avid.source").unwrap();
            let Value::Obj(fields) = &mut side.data else { unreachable!() };
            for (key, value) in fields {
                match key.as_str() {
                    "descriptor" => *value = descriptor.clone(),
                    "descriptor_length" if !picture => *value = Value::Int(sample_count),
                    "picture_coding" if picture => *value = Value::Str("060E2B340401010A0401020271130000".into()),
                    _ => {}
                }
            }
        }
    }
    let sequence = &mut graph.sequences[0];
    sequence.rate = rate;
    for p in &mut sequence.placements {
        p.duration = frame(10);
        p.source_in = frame(if p.video_track.is_some() { 8 } else { 5 });
    }
    sequence.start = Some(frame(86_400));
    sequence.clock = Some(SourceClock {
        effective_count: 86_400,
        rate,
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: 86_400,
            source_offset: 0,
            source_rate: rate,
            source_package: sequence.id.clone(),
            timecode_track: "TC1/1".into(),
            route: "direct sequence TC1".into(),
        },
    });
    let sequence_id = sequence.id.clone();
    let saved_rate = rate.to_string();
    let str_value = |s: &str| Value::Str(s.into());
    let layer = |track, item: &graph::source::SourceItem, start, source_in, duration, bound_item: Option<Value>| {
        let mob = item.umid.as_ref().unwrap();
        let parts = mob.as_bytes().chunks(8).map(|b| std::str::from_utf8(b).unwrap()).collect::<Vec<_>>();
        let mut source = vec![
            ("mob", Value::Str(format!("urn:smpte:umid:{}", parts.join(".").to_ascii_lowercase()))),
            ("track", Value::Int(1)),
            ("local_length", Value::Int(20)),
            ("local_start", Value::Int(start)),
            ("source_start", Value::Int(source_in)),
            ("length", Value::Int(duration)),
            ("rate", str_value(&saved_rate)),
        ];
        if let Some(bound_item) = bound_item {
            source.push(("item", bound_item));
        }
        v(vec![("track", Value::Int(track)), ("picture_source", v(source))])
    };
    let owner = |index| format!("{sequence_id}/component/{index}");
    let issue = |at, index, upper_source, upper_item| v(vec![
        ("effect_id", str_value("EFF_SUBMASTER")),
        ("picture_profile", str_value("plain_layered_submaster")),
        ("kind", Value::Int(1)), ("track", Value::Int(1)),
        ("record_rate", str_value(&saved_rate)),
        ("record_start", v(vec![("value", Value::Int(at))])),
        ("record_length", Value::Int(20)), ("input_count", Value::Int(3)),
        ("owner", Value::Str(owner(index))),
        ("reason", str_value("Saved picture layers")),
        ("submaster_inputs", Value::Arr(vec![
            v(vec![("track", Value::Int(1)), ("segment_class", str_value("FILL"))]),
            layer(2, &graph.items[0], 0, 5, 20, None),
            layer(3, &graph.items[1], 4, upper_source, 9, upper_item),
        ])),
    ]);
    let rows = |next, origin: &str, upper_item: Option<Value>| v(vec![
        ("compatibility_start", str_value(origin)),
        ("unresolved", Value::Arr(vec![
            issue(12, 0, 31, upper_item.clone()), issue(next, 1, 35, upper_item.clone()),
            issue(52, 2, 60, upper_item),
            v(vec![("kind", Value::Int(2))]),
        ])),
    ]);
    let candidate = rows(32, "3600", None);
    let overlap = rows(31, "3600", None);
    let wrong_origin = rows(32, "3599", None);
    let substituted = rows(32, "3600", Some(str_value("clip:camera-b-copy")));
    let wrong_mob = rows(32, "3600", Some(str_value(&graph.items[0].id)));
    let unresolved_copy = rows(32, "3600", Some(Value::Null));
    let loss_reasons: Vec<_> = (0..3).map(|index| format!("V1 at {}: Saved picture layers", owner(index))).collect();
    graph.sidecars.retain(|s| s.adapter != "avid.source" || s.object != sequence_id);
    graph.sidecars.push(Sidecar { adapter: "avid.source".into(), object: sequence_id.clone(), data: candidate });
    graph.losses.extend(loss_reasons.iter().map(|reason| graph::Loss {
        object: sequence_id.clone(), aspect: premiere_adapter::LAYERED_LOSS.into(), reason: reason.clone(),
    }));
    let sequence = &mut graph.sequences[0];
    let video = sequence.placements.iter().find(|p| p.video_track == Some(1)).unwrap().clone();
    sequence.markers.push(SourceMarker {
        name: "Review preceding cut".into(),
        comment: String::new(),
        user: String::new(),
        color: None,
        start: Some(frame(2)),
        duration: frame(1),
        track: Some(TrackRef { kind: StreamKind::Video, index: 1 }),
        owner: MarkerOwner::Edit,
        owner_occurrence: "preceding-source-marker".into(),
        owner_local: Some(frame(2)),
        owner_rate: Some(rate),
        binding: SourceBinding { placements: vec![video.id.clone()], reason: None },
        source: Some(frame(10)),
        inherited: false,
    });
    let mut outgoing = video.clone();
    outgoing.id = "following-cut-a".into();
    outgoing.record_start = frame(60);
    outgoing.duration = frame(12);
    let mut incoming = video;
    incoming.id = "following-cut-b".into();
    incoming.record_start = frame(68);
    incoming.duration = frame(12);
    sequence.transitions.push(SourceTransition {
        id: "following-dissolve".into(),
        kind: StreamKind::Video,
        track: 1,
        cut: frame(70),
        duration: frame(4),
        cut_offset: frame(2),
        style: "avid.EFF2_BLEND_DISSOLVE".into(),
        outgoing: SourceBinding { placements: vec![outgoing.id.clone()], reason: None },
        incoming: SourceBinding { placements: vec![incoming.id.clone()], reason: None },
    });
    sequence.placements.extend([outgoing, incoming]);
    sequence.texts.push(graph::source::SourceText {
        id: "following-graphic".into(), kind: graph::source::TextKind::Graphic, track: 1,
        content: Some("Following graphic".into()), record_start: Some(frame(80)),
        duration: Some(frame(5)), reason: None,
    });
    let prepare = |graph| premiere_adapter::prepare_originals(graph, &options(), ClockProfile::LegacyImport)
        .unwrap();
    let reloaded = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepared = prepare(&reloaded);
    let [nest, next] = prepared.input.layered_submasters.as_slice() else { panic!("expected two layered Submasters") };
    let ticks = |n| frame(n).to_units(prproj::convert::model::TICKS_PER_SECOND).unwrap();
    assert_eq!((nest.start, nest.duration), (ticks(12), ticks(20)));
    assert_eq!(nest.layers.each_ref().map(|l| (l.start, l.source_in, l.duration)),
        [(0, ticks(5), ticks(20)), (ticks(4), ticks(31), ticks(9))]);
    assert_ne!(nest.layers[0].clip, nest.layers[1].clip);
    assert_eq!((next.start, next.layers[1].source_in), (ticks(32), ticks(35)));
    let remaining = |losses: &[graph::Loss]| losses.iter().filter(|l| l.aspect == premiere_adapter::LAYERED_LOSS)
        .map(|l| l.reason.clone()).collect::<Vec<_>>();
    assert_eq!(remaining(&prepared.losses), loss_reasons[2..]);
    assert_eq!(prepared.input.video_dissolves.len(), 1);
    assert_eq!(prepared.input.texts[0].start, ticks(80));
    let marker = &prepared.input.sequences[0].markers[0];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration), ("edit", ticks(2), ticks(1)));
    prepared.input.validate().unwrap();
    let mut invalid = prepared.input;
    invalid.layered_submasters[0].layers[1].source_in = ticks(60);
    assert!(invalid.validate().is_err());
    let mut marked = graph.clone();
    marked.sequences[0].markers.push(SourceMarker {
        name: "Review nested edit".into(),
        comment: String::new(),
        user: String::new(),
        color: None,
        start: Some(frame(19)),
        duration: frame(6),
        track: Some(TrackRef { kind: StreamKind::Video, index: 1 }),
        owner: MarkerOwner::Edit,
        owner_occurrence: format!("{sequence_id}/component/0"),
        owner_local: Some(frame(7)),
        owner_rate: Some(rate),
        binding: SourceBinding {
            placements: vec![],
            reason: Some("native owner includes an unresolved source branch".into()),
        },
        source: None,
        inherited: false,
    });
    let restored = premiere_adapter::prepare_originals(&marked, &options(), ClockProfile::LegacyImport)
        .unwrap();
    assert_eq!(restored.input.layered_submasters.len(), 2);
    let record_marker = &restored.input.sequences[0].markers[1];
    assert_eq!((record_marker.owner.as_str(), record_marker.start, record_marker.duration),
        ("sequence", ticks(19), ticks(6)));
    let mut shared_cut = marked.clone();
    shared_cut.sequences[0].markers[1].start = Some(frame(30));
    shared_cut.sequences[0].markers[1].owner_local = Some(frame(18));
    let surviving = prepare(&shared_cut);
    assert_eq!(remaining(&surviving.losses), loss_reasons[2..]);
    assert_eq!(surviving.input.layered_submasters.len(), 2);
    let marker = &surviving.input.sequences[0].markers[1];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration),
        ("sequence", ticks(30), ticks(6)));
    let mut uncertain = shared_cut.clone();
    uncertain.sequences[0].markers[1].owner_local = Some(frame(17));
    let refused = prepare(&uncertain);
    assert!(refused.input.layered_submasters.is_empty());
    assert_eq!(remaining(&refused.losses), loss_reasons);
    // A marker on an inner effect does not prove ownership of the nest.
    marked.sequences[0].markers[1].owner_occurrence.push_str("/component/4");
    let unresolved = premiere_adapter::prepare_originals(&marked, &options(), ClockProfile::LegacyImport)
        .unwrap();
    assert_eq!(unresolved.input.layered_submasters.len(), 1);
    assert_eq!(unresolved.input.layered_submasters[0].start, ticks(32));
    // The parent sequence can carry the full range even when its end
    // exceeds the nest. Its start must still belong to the saved owner.
    let range = &mut marked.sequences[0].markers[1];
    range.owner_occurrence = format!("{sequence_id}/component/1");
    range.start = Some(frame(49));
    range.owner_local = Some(frame(17));
    let spanning = prepare(&marked);
    assert_eq!(spanning.input.layered_submasters.len(), 2);
    assert_eq!(remaining(&spanning.losses), loss_reasons[2..]);
    let marker = &spanning.input.sequences[0].markers[1];
    assert_eq!((marker.owner.as_str(), marker.start, marker.duration),
        ("sequence", ticks(49), ticks(6)));
    let mut beyond_owner = marked.clone();
    beyond_owner.sequences[0].markers[1].start = Some(frame(52));
    beyond_owner.sequences[0].markers[1].owner_local = Some(frame(20));
    let outside = prepare(&beyond_owner);
    assert_eq!(outside.input.layered_submasters.len(), 1);
    assert_eq!(outside.input.layered_submasters[0].start, ticks(12));
    let mut with_copy = graph.clone();
    let mut copy = graph.items[1].clone();
    copy.id = "clip:camera-b-copy".into();
    with_copy.bins.push(graph::Bin {
        id: "other-bin".into(), name: "Other bin".into(), parent: None, items: vec![copy.id.clone()],
    });
    with_copy.items.push(copy);
    let unbound = premiere_adapter::prepare_originals(&with_copy, &options(), ClockProfile::LegacyImport)
        .unwrap();
    assert!(unbound.input.layered_submasters.is_empty());
    with_copy.sidecars.last_mut().unwrap().data = substituted;
    let saved = SourceGraph::from_json_text(&with_copy.to_json_text().unwrap()).unwrap();
    let restored = premiere_adapter::prepare_originals(&saved, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(restored.input.layered_submasters.len(), 2);
    assert!(restored.input.layered_submasters.iter().all(|nest| nest.layers[1].clip == "camera-b-copy"));
    if dnxhd && !exact_24 {
        assert!(restored.input.direct_dnxhd_pcm24.contains("camera-b"));
        assert!(restored.input.direct_dnxhd_pcm24.contains("camera-b-copy"));
    }
    assert_eq!(remaining(&restored.losses), loss_reasons[2..]);
    for invalid in [wrong_mob, unresolved_copy] {
        let mut with_wrong_item = graph.clone();
        with_wrong_item.sidecars.last_mut().unwrap().data = invalid;
        let declined = premiere_adapter::prepare_originals(&with_wrong_item, &options(), ClockProfile::LegacyImport)
            .unwrap();
        assert!(declined.input.layered_submasters.is_empty());
        assert_eq!(remaining(&declined.losses), loss_reasons);
    }
    if exact_24 {
        let mut disagreeing = graph.clone();
        disagreeing.sidecars.last_mut().unwrap().data = wrong_origin;
        let refused = premiere_adapter::prepare_originals(&disagreeing, &options(), ClockProfile::LegacyImport)
            .unwrap();
        assert!(refused.input.layered_submasters.is_empty());
        assert_eq!(remaining(&refused.losses), loss_reasons);
    }
    let mut title_overlap = graph.clone();
    title_overlap.sequences[0].texts[0].record_start = Some(frame(27));
    let with_title = prepare(&title_overlap);
    assert_eq!(with_title.input.layered_submasters.len(), 1);
    assert_eq!(with_title.input.layered_submasters[0].start, ticks(32));
    assert_eq!(remaining(&with_title.losses), [loss_reasons[0].clone(), loss_reasons[2].clone()]);
    graph.sidecars.last_mut().unwrap().data = overlap;
    let declined = prepare(&graph);
    assert!(declined.input.layered_submasters.is_empty());
    assert_eq!(remaining(&declined.losses), loss_reasons);
}

#[test]
fn linked_three_mono_group_keeps_default_and_independent_third_lane() {
    use prproj::convert::{multicam::{PreparedGroupAudio, FRAME}, writer::prepare_typed_project};

    let mut graph = linked_group_camera_fixture();
    for (i, media) in graph.media.iter().enumerate() {
        let facts = graph.sidecars.iter_mut()
            .find(|s| s.adapter == "avid.source" && s.object == media.id).unwrap();
        let Value::Obj(fields) = &mut facts.data else { panic!("missing native facts") };
        fields.iter_mut().find(|(key, _)| key == "file_package").unwrap().1 =
            Value::Str(format!("{:064X}", i + 1));
        if media.kind == StreamKind::Audio {
            fields.iter_mut().find(|(key, _)| key == "descriptor").unwrap().1 =
                v(vec![("channels", Value::Int(1)), ("bits", Value::Int(24))]);
        }
    }
    let without_a3 = graph.clone();
    for (i, item) in graph.items.iter_mut().enumerate() {
        let mut binding = item.media.last().unwrap().clone();
        let mut media = graph.media.iter()
            .find(|m| m.id == binding.media).unwrap().clone();
        media.id.push_str(":third");
        media.locations[0].push_str(".third.mxf");
        media.stream = Some(3);
        let mut facts = graph.sidecars.iter()
            .find(|s| s.adapter == "avid.source" && s.object == binding.media).unwrap().clone();
        facts.object = media.id.clone();
        let Value::Obj(fields) = &mut facts.data else { panic!("missing native facts") };
        fields.iter_mut().find(|(key, _)| key == "file_package").unwrap().1 =
            Value::Str(format!("{:064X}", i + 7));
        graph.sidecars.push(facts);
        binding.media = media.id.clone();
        graph.media.push(media);
        item.media.push(binding);
    }
    let group = &mut graph.multicams[0];
    let mut lane = group.lanes[2].clone();
    lane.index = 3;
    lane.selected = Some(1);
    for alternative in &mut lane.alternatives {
        alternative.id.push_str(":third");
        for piece in &mut alternative.pieces {
            if piece.item.is_some() {
                piece.stream = Some(3);
            }
        }
    }
    group.lanes.push(lane);

    let sequence = &mut graph.sequences[0];
    let audio = sequence.placements.iter().find(|p| p.audio_tracks == [2]).unwrap().clone();
    let use_ = graph.multicam_uses.iter()
        .find(|u| u.kind == StreamKind::Audio && u.track == 2).unwrap().clone();
    let frame = |n: i128| Q::new(n * 1001, 24000).unwrap();
    for (start, end, alternate) in [(0, 3, false), (3, 6, true), (6, 10, false)] {
        let mut edit = audio.clone();
        edit.id = format!("third:{start}");
        edit.record_start = frame(start);
        edit.source_in = frame(if alternate { 8 + start } else { 5 + start });
        edit.duration = frame(end - start);
        edit.audio_tracks = vec![3];
        edit.audio_streams = vec![3];
        let mut pick = use_.clone();
        pick.id = format!("use:third:{start}");
        pick.track = 3;
        pick.lane = Some(3);
        pick.selected = Some(if alternate { 0 } else { 1 });
        pick.record_start = Some(edit.record_start);
        pick.source_in = Some(frame(8 + start));
        pick.duration = Some(edit.duration);
        pick.binding.placements = vec![edit.id.clone()];
        if alternate {
            edit.item = without_a3.items[1].id.clone();
        }
        sequence.placements.push(edit);
        graph.multicam_uses.push(pick);
    }

    let saved = graph.to_json_text().unwrap();
    let loaded = SourceGraph::from_json_text(&saved).unwrap();
    assert_eq!(loaded, graph);
    let prepared = premiere_adapter::prepare_originals(&loaded, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(prepared.input.multicams[0].audio_cameras, [0, 0, 0]);
    assert_eq!(prepared.input.multicams[0].cameras[0].audio_labels, [1, 2, 3]);
    assert_eq!(prepared.input.multicam_uses.iter().filter(|u|
        u.audio.contains(&PreparedGroupAudio::LinkedAlternate(2))).count(), 1);
    assert!(premiere_adapter::prepare_originals(&loaded, &options(), ClockProfile::VerifiedRewrap).is_err());
    let merged = Options { bin_mode: "merged".into(), ..options() };
    let refused = premiere_adapter::prepare_originals(&loaded, &merged, ClockProfile::LegacyImport).err().unwrap();
    assert!(refused.0.contains("Separate files"), "{refused:?}");

    let mut offline = loaded.clone();
    offline.media.iter_mut().find(|m| m.stream == Some(3)).unwrap().online = Some(false);
    assert!(premiere_adapter::prepare_originals(&offline, &options(), ClockProfile::LegacyImport).is_err());
    let mut duplicate = loaded.clone();
    let second = duplicate.sidecars.iter()
        .find(|side| side.object == duplicate.items[0].media[2].media && side.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &second.data else { panic!("missing native facts") };
    let package = fields.iter().find(|(key, _)| key == "file_package").unwrap().1.clone();
    let third = duplicate.sidecars.iter_mut()
        .find(|side| side.object == duplicate.items[0].media[3].media && side.adapter == "avid.source").unwrap();
    let Value::Obj(fields) = &mut third.data else { panic!("missing native facts") };
    fields.iter_mut().find(|(key, _)| key == "file_package").unwrap().1 = package;
    let refused = premiere_adapter::prepare_originals(&duplicate, &options(), ClockProfile::LegacyImport).err().unwrap();
    assert!(refused.0.contains("three original mono PCM24 siblings"), "{refused:?}");
    let mut wrong = loaded.clone();
    let middle = wrong.multicam_uses.iter_mut().find(|p| p.id == "use:third:3").unwrap();
    middle.selected = Some(1);
    let refused = premiere_adapter::prepare_originals(&wrong, &options(), ClockProfile::LegacyImport).err().unwrap();
    assert!(refused.0.contains("audio choice contradicts"), "{refused:?}");

    let dir = std::env::temp_dir().join(format!("anynle-three-mono-group-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let out = dir.join("Independent A3.prproj");
    prepare_typed_project(&prepared.input, &out, &mut prproj::SeededUuids::new("three-mono-group"), &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let tree = &project.doc.tree;
    assert_eq!(tree.iter_tag(project.doc.root, "CodecType").filter_map(|n| tree.text(n)).collect::<Vec<_>>(),
        ["1096180840", "1096180840"]);
    let group = project.sequences().into_iter().find(|&s| project.sequence_name(s) == "Synthetic Group").unwrap();
    assert_eq!(project.sequence(group).audio_channel_count, Some(8));
    let tracks = project.audio_tracks(group);
    assert_eq!(tracks.iter().map(|&t| project.is_muted(t).unwrap()).collect::<Vec<_>>(),
        [false, false, false, true, true, true]);
    let third = project.track(tracks[2]).element;
    let panner = project.doc.resolve(tree.find1(third, "AudioTrack/Panner").unwrap()).unwrap();
    assert_eq!(tree.findtext(panner, "DirectPanProcessor/Matrix").unwrap().as_deref(), Some("[[0,[2]],[1,[3]]]"));
    let timeline = project.sequences().into_iter().find(|&s| project.sequence_name(s) == loaded.sequences[0].name).unwrap();
    let a3 = &project.track(project.audio_tracks(timeline)[2]).clips;
    assert_eq!(a3.len(), 3);
    for &edit in &[a3[0], a3[2]] {
        let placed = project.track_item(edit);
        assert_eq!(tree.findtext(placed.subclip, "OrigChGrp").unwrap().as_deref(), Some("2"));
        let references = tree.findall(placed.clip, "SecondaryContents/SecondaryContentItem").unwrap();
        let [reference] = references.as_slice() else { panic!("expected one mono content") };
        let content = project.doc.resolve(*reference).unwrap();
        assert_eq!(tree.findtext(content, "ChannelIndex").unwrap().as_deref(), Some("3"));
    }
    let middle_source = project.track_item_source(a3[1]).unwrap();
    let camera_b = &loaded.items[1];
    let third_b = loaded.media.iter().find(|m| m.id == camera_b.media[3].media).unwrap();
    assert_eq!(project.item_media_path(middle_source), Some(third_b.locations[0].as_str()));
    assert_eq!(project.start(a3[1]).unwrap().ticks, 3 * FRAME);
    assert_eq!(project.in_point(a3[1]).unwrap().ticks, 11 * FRAME);
    assert_eq!(loaded.to_json_text().unwrap(), saved);

    // Avid's DNx LB preset can save DNxHD as well as DNxHR. The picture
    // template must follow its saved coding label in the three-lane path.
    let mut dnxhd = loaded.clone();
    for picture in dnxhd.media.iter().filter(|m| m.kind == StreamKind::Video) {
        let side = dnxhd.sidecars.iter_mut()
            .find(|side| side.object == picture.id && side.adapter == "avid.source").unwrap();
        let Value::Obj(fields) = &mut side.data else { panic!("missing native facts") };
        fields.iter_mut().find(|(key, _)| key == "descriptor").unwrap().1 = v(vec![
            ("compression", Value::Str("AVHD".into())),
            ("resolution_id", Value::Int(1253)),
            ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)])),
        ]);
        fields.iter_mut().find(|(key, _)| key == "picture_coding").unwrap().1 =
            Value::Str("060E2B340401010A0401020271130000".into());
    }
    let candidate = premiere_adapter::prepare_originals(&dnxhd, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(candidate.input.direct_dnxhd_pcm24.len(), 2);
    let out = dir.join("Independent A3 DNxHD.prproj");
    prepare_typed_project(&candidate.input, &out, &mut prproj::SeededUuids::new("three-mono-dnxhd"), &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(out).unwrap()).unwrap();
    assert_eq!(doc.tree.iter_tag(doc.root, "CodecType").filter_map(|n| doc.tree.text(n)).collect::<Vec<_>>(),
        ["1096180846", "1096180846"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn linked_group_selects_picture_templates_independently_of_pcm_depth() {
    use prproj::convert::writer::prepare_typed_project;
    let mut graph = linked_group_camera_fixture();
    let first_id = graph.media.iter().find(|m| m.kind == StreamKind::Video).unwrap().id.clone();
    let facts = graph.sidecars.iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == first_id).unwrap();
    let Value::Obj(fields) = &mut facts.data else { panic!("missing media facts") };
    for (key, value) in fields {
        if key == "descriptor" {
            *value = v(vec![
                ("compression", Value::Str("AVHD".into())),
                ("resolution_id", Value::Int(1253)),
                ("display_aspect", Value::Arr(vec![Value::Int(16), Value::Int(9)])),
            ]);
        }
        if key == "picture_coding" {
            *value = Value::Str("060E2B340401010A0401020271130000".into());
        }
    }
    let item = &graph.items[0];
    let audio_ids: Vec<_> = item.media.iter().map(|b| &b.media).filter(|id| **id != first_id).collect();
    for side in graph.sidecars.iter_mut().filter(|s| audio_ids.contains(&&s.object)) {
        let Value::Obj(fields) = &mut side.data else { panic!("missing audio facts") };
        let desc = fields.iter_mut().find(|(key, _)| key == "descriptor").unwrap();
        desc.1 = v(vec![("channels", Value::Int(1)), ("bits", Value::Int(24))]);
    }
    let prepare = |graph: &SourceGraph| premiere_adapter::prepare_originals(graph, &options(), ClockProfile::LegacyImport);
    let prepared = prepare(&graph).unwrap();
    assert_eq!(prepared.input.direct_dnxhd_pcm24, [item.id.strip_prefix("clip:").unwrap().into()].into());
    assert_eq!(prepared.input.direct_dnxhr_origins.len(), 2);
    let dir = std::env::temp_dir().join(format!("anynle-mixed-dnx-group-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let out = dir.join("Mixed DNx.prproj");
    prepare_typed_project(&prepared.input, &out, &mut prproj::SeededUuids::new("mixed-dnx-group"), &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(out).unwrap()).unwrap();
    let types: std::collections::HashSet<_> = doc.tree.iter_tag(doc.root, "CodecType")
        .filter_map(|node| doc.tree.text(node)).collect();
    assert!(types.contains("1096180846") && types.contains("1096180840"), "{types:?}");
    graph.media.iter_mut().find(|m| m.id == first_id).unwrap().online = Some(false);
    assert!(prepare(&graph).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn linked_mov_group_preserves_independently_selected_mono_channels() {
    use prproj::convert::{multicam::PreparedGroupAudio, writer::prepare_typed_project};

    let mut graph = fixed_audio_group();
    for item in &mut graph.items {
        item.stream_groups = vec![vec![1, 2]];
    }
    for (index, media) in graph.media.iter_mut().enumerate() {
        let audio = media.kind == StreamKind::Audio;
        let path = format!("/Synthetic/Camera-{}.mov", index / 3);
        media.locations = vec![path.clone()];
        media.codec = Some(if audio { "pcm_s24le" } else { "apch" }.into());
        media.online = Some(true);
        let clock = if audio { 48_000 } else { 24_000 };
        media.original = Some(OriginalStream {
            saved_locators: vec![path],
            container: "mov".into(),
            stream_index: if audio { 1 } else { 0 },
            channel_index: media.stream.filter(|_| audio).map(|label| label - 1),
            channels: audio.then_some(2),
            time_base: Q::new(1, clock).unwrap(),
            start_pts: 0,
            duration_ts: media.duration.to_units(clock as i64).unwrap(),
            sample_representation: audio.then(|| "pcm24".into()),
        });
    }
    let camera_b = graph.items[1].id.clone();
    let sequence = &mut graph.sequences[0];
    let a1 = sequence.placements.iter_mut().find(|edit| edit.audio_tracks == [1]).unwrap();
    a1.item = camera_b;
    a1.source_in = Q::new(8 * 1001, 24_000).unwrap();
    let a1_use = graph.multicam_uses.iter_mut()
        .find(|usage| usage.kind == StreamKind::Audio && usage.track == 1).unwrap();
    a1_use.selected = Some(0);
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepared = premiere_adapter::prepare_avid_job(
        &graph, &options(), ClockProfile::LegacyImport, false, true, None,
    ).unwrap_or_else(|error| panic!("{}", error.message));
    let input = prepared.input;
    assert_eq!(input.multicams[0].audio_cameras, [0, 0]);
    assert!(input.multicams[0].cameras.iter().all(|camera| camera.mono_view.is_some()));
    assert!(input.multicam_uses.iter().any(|usage|
        usage.audio == [PreparedGroupAudio::LinkedAlternate(0)]));

    let dir = std::env::temp_dir().join(format!("anynle-linked-mov-group-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let out = dir.join("Group.prproj");
    let mut ids = prproj::SeededUuids::new("synthetic-linked-mov-group");
    let ready = prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap();
    assert_eq!(ready.assisted_originals().map(String::as_str).collect::<Vec<_>>(),
        ["/Synthetic/Camera-0.mov", "/Synthetic/Camera-1.mov"]);
    assert!(ready.publish().err().unwrap().to_string().contains("not been verified"));
    assert!(!out.exists());

    let mut invalid = input.clone();
    invalid.multicams[0].cameras[1].mono_view = input.multicams[0].cameras[0].mono_view.clone();
    assert!(prepare_typed_project(&invalid, &out, &mut ids, &[], false)
        .err().unwrap().to_string().contains("distinct MOV paths"));
    invalid = input;
    invalid.multicams[0].cameras[1].audio_labels = vec![1, 1];
    let out = dir.join("Invalid Group.prproj");
    assert!(prepare_typed_project(&invalid, &out, &mut ids, &[], false).is_err());
    assert!(!out.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn linked_editable_multicam_uses_the_six_original_streams() {
    use premiere_adapter::{prepare_avid_job, PreparationOccurrence};
    use prproj::convert::{multicam::FRAME, writer::prepare_typed_project};

    let g = linked_group_camera_fixture();
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
    let group_error = prepare_linked(&missing, &options()).err().unwrap();
    assert!(group_error.0.contains("two original mono PCM24 siblings"));
    let scoped = |g: &SourceGraph| prepare_avid_job(
        g, &options(), ClockProfile::LegacyImport, false, false, None,
    );
    let failure = scoped(&missing).err().unwrap();
    assert_eq!(failure.message, group_error.to_string());
    assert!(failure.sequence.is_none());
    assert_eq!(failure.occurrence, Some(PreparationOccurrence::Group(g.multicams[0].id.clone())));

    let mut invalid_use = g.clone();
    let usage = invalid_use.multicam_uses.iter_mut().find(|u| u.kind == StreamKind::Audio).unwrap();
    usage.binding.reason = Some("unresolved synthetic leaf".into());
    let (sequence_id, use_id) = (usage.sequence.clone(), usage.id.clone());
    let failure = scoped(&invalid_use).err().unwrap();
    assert_eq!(failure.message, prepare_linked(&invalid_use, &options()).err().unwrap().to_string());
    assert!(failure.message.contains("group use or leaf binding has an unresolved witness"));
    assert_eq!(failure.sequence, Some(sequence_id));
    assert_eq!(failure.occurrence, Some(PreparationOccurrence::GroupUse(use_id)));

    let dir = std::env::temp_dir().join(format!("anynle-linked-group-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let out = dir.join("Linked Group.prproj");
    let mut ids = prproj::SeededUuids::new("synthetic-linked-group");
    let ready = prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap();
    assert!(!out.exists());
    let report = ready.publish().unwrap();
    assert!(report.notes.iter().any(|note| note.contains("two default mono outputs from its first camera")));
    assert!(!report.notes.iter().any(|note| note.contains("alternate Group audio timeline edit")));
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
    let rate = switched.sequences[0].rate;
    let marker_start = Q::new(2 * 1001, 24000).unwrap();
    let marker_duration = Q::new(3 * 1001, 24000).unwrap();
    let marker_source = Q::new(10 * 1001, 24000).unwrap();
    let video_placement = switched.multicam_uses.iter().find(|u| u.kind == StreamKind::Video)
        .unwrap().binding.placements[0].clone();
    for (name, kind, placement) in [
        ("Angle note", StreamKind::Video, video_placement),
        ("Inherited audio note", StreamKind::Audio, binding.clone()),
        ("Alternate audio note", StreamKind::Audio, binding),
    ] {
        switched.sequences[0].markers.push(SourceMarker {
            name: name.into(),
            comment: "Keep this review range".into(),
            user: String::new(),
            color: Some(Label { palette: "avid".into(), name: "Red".into() }),
            start: Some(marker_start),
            duration: marker_duration,
            track: Some(TrackRef { kind, index: 1 }),
            owner: MarkerOwner::Edit,
            owner_occurrence: format!("marker:{name}"),
            owner_local: Some(marker_start),
            owner_rate: Some(rate),
            binding: SourceBinding { placements: vec![placement], reason: None },
            source: None,
            inherited: false,
        });
    }
    let native_marker = |i| v(vec![
        ("graph_index", Value::Int(i)), ("created", Value::Int(100)),
    ]);
    let item = switched.items.iter_mut().find(|i| i.id == "clip:camera-b").unwrap();
    for marker in &switched.sequences[0].markers[..2] {
        let mut source = marker.clone();
        source.start = Some(marker_source);
        source.owner_local = source.start;
        source.owner = MarkerOwner::Sequence;
        source.binding.placements.clear();
        source.owner_occurrence = format!("master:{}", source.name);
        item.markers.push(source);
    }
    for object in [&item.id, &switched.sequences[0].id] {
        let rows = Value::Arr(vec![native_marker(0), native_marker(1)]);
        if let Some(sidecar) = switched.sidecars.iter_mut().find(|s| s.adapter == "avid.source" && &s.object == object) {
            let Value::Obj(fields) = &mut sidecar.data else { panic!("expected Avid source facts") };
            fields.push(("marker_native".into(), rows));
        } else {
            switched.sidecars.push(Sidecar {
                adapter: "avid.source".into(), object: object.clone(), data: v(vec![("marker_native", rows)]),
            });
        }
    }
    let prepared = prepare_linked(&switched, &options()).unwrap();
    assert!(prepared.losses.iter().any(|l| l.object == occurrence && l.aspect == "group_audio_editability"));
    assert!(prepared.input.multicam_uses.iter().any(|u| u.audio.contains(&LinkedAlternate(0))));
    assert!(prepared.input.multicam_uses.iter().any(|u| u.audio.contains(&Group(1))));
    let out = dir.join("Alternate Audio.prproj");
    let report = prepare_typed_project(&prepared.input, &out, &mut ids, &[], false)
        .unwrap().publish().unwrap();
    assert!(report.notes.iter().any(|note| note.starts_with("1 alternate Group audio timeline edit")));
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let sequence = project.sequences().into_iter()
        .find(|&s| project.sequence_name(s) == switched.sequences[0].name).unwrap();
    use prproj::model::marker::{
        marker_color_index, marker_comments, marker_end, marker_name, marker_start as saved_start,
    };
    let tree = &project.doc.tree;
    assert_eq!(tree.iter_tag(project.doc.root, "SelectedTrackIndex")
        .map(|node| tree.text(node).unwrap()).collect::<Vec<_>>(), ["1"]);
    let video = project.video_tracks(sequence)[0];
    let v1 = project.track(video).clips[0];
    assert_eq!(project.item_name(project.track_item_source(v1).unwrap()), "Synthetic Group");
    let markers = project.sequence_markers(sequence);
    assert_eq!(markers.len(), 2);
    let mut names = Vec::new();
    for &marker in markers {
        names.push(marker_name(tree, marker).unwrap());
        assert_eq!(saved_start(tree, marker).unwrap().ticks, 2 * FRAME);
        assert_eq!(marker_end(tree, marker).unwrap().ticks, 5 * FRAME);
        assert!(marker_comments(tree, marker).unwrap().contains("Keep this review range"));
        assert_eq!(marker_color_index(tree, marker).unwrap(), Some(1));
    }
    names.sort_unstable();
    assert_eq!(names, ["Alternate audio note", "Angle note"]);
    assert_eq!(prepared.losses.iter().filter(|l| l.aspect == "marker_ownership").count(), 2);
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
    let [marker] = project.item_markers(a1_source) else { panic!("expected the linked alternate's source marker") };
    assert_eq!(marker_name(tree, *marker).unwrap(), "Inherited audio note");
    assert_eq!(saved_start(tree, *marker).unwrap().ticks, 10 * FRAME);
    assert_eq!(marker_end(tree, *marker).unwrap().ticks, 13 * FRAME);
    assert_eq!(project.item_name(a2_source), "Synthetic Group");
    for item in [v1, a1, a2] {
        assert_eq!(project.in_point(item).unwrap().ticks, 8 * FRAME);
        assert_eq!(project.out_point(item).unwrap().ticks, 18 * FRAME);
    }

    let mut split_default = g.clone();
    let lane = split_default.multicams[0].lanes.iter_mut()
        .find(|lane| lane.kind == StreamKind::Audio && lane.index == 1).unwrap();
    // The fixture stores the second picture camera at audio alternative zero.
    lane.selected = Some(0);
    let mut input = prepare_linked(&split_default, &options()).unwrap().input;
    assert_eq!(input.multicams[0].audio_cameras, [1, 0]);
    let out = dir.join("Split Group Default.prproj");
    prepare_typed_project(&input, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let group = project.sequences().into_iter()
        .find(|&s| project.sequence_name(s) == "Synthetic Group").unwrap();
    let tracks = project.audio_tracks(group);
    let mutes: Vec<_> = tracks.iter().map(|&t| project.is_muted(t).unwrap()).collect();
    assert_eq!(mutes, [false, false, true, true]);
    let source_path = |track| {
        let item = project.track(track).clips[0];
        project.item_media_path(project.track_item_source(item).unwrap()).unwrap()
    };
    assert_eq!(source_path(tracks[0]), alternate_path);
    let camera_a = &split_default.items[0];
    let right = split_default.media.iter().find(|media| {
        media.kind == StreamKind::Audio && media.stream == Some(2)
            && camera_a.media.iter().any(|binding| binding.media == media.id)
    }).unwrap();
    assert_eq!(source_path(tracks[1]), right.locations[0]);

    input.multicams[0].audio_cameras[0] = 2;
    let invalid = dir.join("Invalid Group Default.prproj");
    assert!(prepare_typed_project(&input, &invalid, &mut ids, &[], false).is_err());
    assert!(!invalid.exists());
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
        bindings.push(MediaRelink { source_path:leaf.video.as_ref().unwrap().path.clone(), target_path:dst.clone(), audio_stream:None, separate_mono: false, pixel_aspect: None });
        for a in &leaf.audio {
            bindings.push(MediaRelink { source_path:a.path.clone(), target_path:dst.clone(), audio_stream:Some((a.avid_track.unwrap()-1) as u32), separate_mono: false, pixel_aspect: None });
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

fn autosync_fixture(duplicate_package: bool) -> SourceGraph {
    let mut graph = fixture();
    graph.sequences.clear();
    graph.sidecars.clear();
    graph.losses.clear();
    let camera_mob = format!("{:064X}", 1);
    let sound_mob = format!("{:064X}", 2);
    let sync_mob = format!("{:064X}", 3);
    let mut picture = graph.media.iter().find(|m| m.kind == StreamKind::Video).unwrap().clone();
    let audio_template = graph.media.iter().find(|m| m.kind == StreamKind::Audio).unwrap().clone();
    picture.locations = vec!["/Synthetic/Camera.mov".into()];
    picture.codec = Some("avc1".into());
    picture.rate = Some(Q::integer(24));
    picture.duration = Q::integer(2);
    picture.online = Some(true);
    picture.original = Some(OriginalStream {
        saved_locators: picture.locations.clone(),
        container: "mov".into(), stream_index: 0, channel_index: None, channels: None,
        time_base: Q::new(1, 24).unwrap(), start_pts: 0, duration_ts: 48,
        sample_representation: None,
    });
    let source_ref = |m: &SourceMedia| SourceItemMedia {
        media: m.id.clone(), gain_db: 0.0, source_offset: Some(Q::ZERO),
    };
    let mut camera = graph.items[0].clone();
    camera.umid = Some(camera_mob.clone());
    camera.rate = Some(Q::integer(24));
    camera.stream_groups.clear();
    camera.media = vec![source_ref(&picture)];
    let mut sound = camera.clone();
    sound.id = "controlled-sound".into();
    sound.name = "Controlled sound".into();
    sound.umid = Some(sound_mob.clone());
    sound.media.clear();
    graph.media = vec![picture];
    let track = |kind, label, mob: &str, offset| v(vec![
        ("kind", Value::Int(kind)), ("label", Value::Int(label)),
        ("component_class", Value::Str("SCLP".into())), ("rate", Value::Str("24".into())),
        ("length", Value::Int(48)), ("target_mob", Value::Str(mob.into())),
        ("target_track", Value::Int(label)), ("target_offset", Value::Int(offset)),
    ]);
    let mut tracks = vec![track(1, 1, &camera_mob, 0)];
    for lane in 1..=8 {
        let mut mono = audio_template.clone();
        mono.id = format!("controlled-sound-{lane}");
        mono.locations = vec![format!("/Synthetic/Mic{lane}.mxf")];
        mono.stream = Some(lane);
        mono.duration = Q::integer(3);
        mono.online = Some(true);
        mono.bits = Some(16);
        sound.media.push(source_ref(&mono));
        let package = if duplicate_package && lane == 8 { 17 } else { 16 + lane };
        graph.sidecars.push(Sidecar {
            object: mono.id.clone(), adapter: "avid.source".into(),
            data: v(vec![
                ("file_package", Value::Str(format!("{package:064X}"))),
                ("legacy_clock", clock(0, 24, false)),
                ("legacy_file_clock", clock(0, 24, false)),
            ]),
        });
        graph.media.push(mono);
        tracks.push(track(2, lane, &sound_mob, 2));
    }
    graph.bins[0].items = vec![camera.id.clone(), sound.id.clone()];
    graph.items = vec![camera, sound];
    graph.sidecars.push(Sidecar {
        adapter: "avid.subclips".into(),
        object: graph.bins[0].id.clone(),
        data: Value::Arr(vec![v(vec![
            ("id", Value::Str("source/bin/sync".into())),
            ("mob", Value::Str(sync_mob)),
            ("name", Value::Str("Synced interview".into())),
            ("rate", Value::Str("24".into())),
            ("backing_length", Value::Int(48)),
            ("start", v(vec![("value", Value::Int(4))])),
            ("end", v(vec![("value", Value::Int(28))])),
            ("tracks", Value::Arr(tracks)),
        ])]),
    });
    graph
}

#[test]
fn autosync_subclip_uses_exact_separate_picture_and_mono_source_ranges() {
    let graph = autosync_fixture(false);
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepared = premiere_adapter::prepare_originals(&graph, &options(), ClockProfile::LegacyImport).unwrap();
    assert_eq!(prepared.input.autosync_bins.get(&0), Some(&0));
    let [source] = &prepared.input.sequences[..] else { panic!("expected reusable sequence"); };
    let frame = prproj::convert::model::TICKS_PER_SECOND / 24;
    assert_eq!(source.events.len(), 9);
    assert_eq!(source.events[0].video_track, Some(1));
    assert_eq!(source.events[0].source_in, 4 * frame);
    for (lane, event) in (1..=8).zip(&source.events[1..]) {
        assert_eq!(event.audio_tracks, [lane]);
        assert_eq!(event.audio_channels, [lane]);
        assert_eq!(event.source_in, 6 * frame);
        assert_eq!(event.clip, source.events[1].clip);
    }
    assert!(source.events.iter().all(|e| e.record_start == 0 && e.duration == 24 * frame));
    assert_ne!(source.events[0].clip, source.events[1].clip);
    assert!(prepared.losses.iter().any(|loss| loss.aspect == "subclip_timecode"));
    assert!(!prepared.losses.iter().any(|loss| loss.aspect == "subclip_bin_item"));

    let duplicate = autosync_fixture(true);
    let p = premiere_adapter::prepare_originals(&duplicate, &options(), ClockProfile::LegacyImport).unwrap();
    assert!(p.input.autosync_bins.is_empty());
    assert!(p.input.sequences.is_empty());
    assert!(p.losses.iter().any(|loss| loss.aspect == "subclip_bin_item"
        && loss.reason.contains("distinct PCM16 mono siblings")));
}

#[test]
fn linked_autosync_binds_two_mono_wav_masters_with_separate_offsets() {
    let mut graph = autosync_fixture(false);
    let rate = Q::new(24_000, 1001).unwrap();
    let picture = &mut graph.media[0];
    picture.codec = Some("apch".into());
    picture.rate = Some(rate);
    picture.duration = Q::new(52 * 1001, 24_000).unwrap();
    let timecode = SourceClock {
        effective_count: 105_120,
        rate,
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: 105_120,
            source_offset: 0,
            source_rate: rate,
            source_package: format!("{:064X}", 8),
            timecode_track: "TC1".into(),
            route: AVID_LINKED_MOV_TIMECODE.into(),
        },
    };
    picture.start = timecode.exact_start();
    picture.clock = Some(timecode);
    let original = picture.original.as_mut().unwrap();
    original.time_base = Q::new(1, 24_000).unwrap();
    original.duration_ts = 52 * 1001;
    graph.items[0].rate = Some(rate);
    graph.media.truncate(3);
    let mut second = graph.items[1].clone();
    second.id = "second-sound".into();
    second.umid = Some(format!("{:064X}", 4));
    second.media = vec![graph.items[1].media[1].clone()];
    second.rate = Some(rate);
    graph.items[1].rate = Some(rate);
    graph.items[1].media.truncate(1);
    graph.items.push(second);
    graph.bins[0].items.push("second-sound".into());
    for (index, media) in graph.media[1..].iter_mut().enumerate() {
        media.locations = vec![format!("/Synthetic/Mono-{}.wav", index + 1)];
        media.codec = Some("pcm_s24le".into());
        media.stream = Some(1);
        media.bits = Some(24);
        media.start = None;
        media.original = Some(OriginalStream {
            saved_locators: media.locations.clone(),
            container: "wav".into(),
            stream_index: 0,
            channel_index: Some(0),
            channels: Some(1),
            time_base: Q::new(1, 48_000).unwrap(),
            start_pts: 0,
            duration_ts: 144_000,
            sample_representation: Some("pcm24".into()),
        });
    }
    graph.sidecars.retain(|s| s.adapter == "avid.subclips");
    let Value::Arr(rows) = &mut graph.sidecars[0].data else { unreachable!() };
    let Value::Obj(row) = &mut rows[0] else { unreachable!() };
    let field = |key: &str, value| (key.to_owned(), value);
    row.retain(|(key, _)| !matches!(key.as_str(), "rate" | "tracks"));
    row.push(field("rate", Value::Str(rate.to_string())));
    let track = |kind, lane, mob: &str, offset| v(vec![
        ("kind", Value::Int(kind)), ("label", Value::Int(lane)),
        ("component_class", Value::Str("SCLP".into())),
        ("rate", Value::Str(rate.to_string())),
        ("length", Value::Int(48)), ("target_mob", Value::Str(mob.into())),
        ("target_track", Value::Int(1)), ("target_offset", Value::Int(offset)),
    ]);
    row.push(field("tracks", Value::Arr(vec![
        track(1, 1, graph.items[0].umid.as_deref().unwrap(), 3),
        track(2, 1, graph.items[2].umid.as_deref().unwrap(), 5),
        track(2, 2, graph.items[1].umid.as_deref().unwrap(), 2),
    ])));
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let prepare = |graph| premiere_adapter::prepare_avid_job(
        graph, &options(), ClockProfile::LegacyImport, false, true, None,
    ).unwrap_or_else(|issue| panic!("{}", issue.message));
    let prepared = prepare(&graph);
    let frame = prproj::convert::model::TICKS_PER_SECOND / 24_000 * 1001;
    let [source] = &prepared.input.sequences[..] else { panic!("{:?}", prepared.losses) };
    assert_eq!(prepared.input.autosync_bins.get(&0), Some(&0));
    assert_eq!(source.start, (105_120 + 7) * frame);
    assert_eq!(source.events.len(), 3);
    assert_eq!(source.events[0].source_in, 7 * frame);
    assert!(source.events.iter().all(|e| e.record_start == 0 && e.duration == 24 * frame));
    let [first, second] = &source.events[1..] else { unreachable!() };
    assert_eq!((&*first.clip, first.source_in), ("second-sound", 9 * frame));
    assert_eq!((&*second.clip, second.source_in), ("controlled-sound", 6 * frame));
    assert_eq!((first.audio_tracks.as_slice(), second.audio_tracks.as_slice()), (&[1][..], &[2][..]));
    assert_eq!((first.audio_channels.as_slice(), second.audio_channels.as_slice()), (&[1][..], &[1][..]));

    let mut wrong = graph.clone();
    let row = &mut wrong.sidecars[0].data;
    let Value::Arr(rows) = row else { unreachable!() };
    let Value::Obj(fields) = &mut rows[0] else { unreachable!() };
    let Value::Arr(tracks) = &mut fields.iter_mut().find(|(k, _)| k == "tracks").unwrap().1
        else { unreachable!() };
    let Value::Obj(fields) = &mut tracks[1] else { unreachable!() };
    fields.iter_mut().find(|(k, _)| k == "target_track").unwrap().1 = Value::Int(2);
    let refused = prepare(&wrong);
    assert!(refused.input.autosync_bins.is_empty());
    assert!(refused.losses.iter().any(|loss| loss.aspect == "subclip_bin_item"
        && loss.reason.contains("not a direct same-rate source reference")));
}
