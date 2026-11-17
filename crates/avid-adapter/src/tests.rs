use super::*;
use avid_reader::{
    extract::{MasterClip, MediaTrack, SeqEvent},
    io::{MobId, Uuid},
    mxf::{MxfIdentity, QualifiedClock},
};
fn mob(n: u32) -> MobId {
    MobId {
        label: [0; 12],
        length: 19,
        instance: [0; 3],
        material: Uuid {
            data1: n,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
    }
}

#[test]
fn master_start_keeps_drop_frame_logging_separate_from_media_clock() {
    let (mut facts, index) = fixture(1);
    for (frames, fps, drop, expected) in [
        (1799, 30, true, Some("00:00:59;29")),
        (1800, 30, true, Some("00:01:00;02")),
        (215784, 30, true, Some("02:00:00;00")),
        (35964, 60, true, Some("00:10:00;00")),
        (215784, 30, false, Some("01:59:52:24")),
        (1800, 24, true, None),
    ] {
        facts.masters[0].master.start = Some(extract::Timecode { frames, fps, drop });
        let result = build(&facts, &index, "Synthetic", &["Bin".into()]).unwrap();
        let graph = SourceGraph::from_json_text(&result.graph.to_json_text().unwrap()).unwrap();
        let item = &graph.items[0];
        assert_eq!(
            graph.sidecar("avid", &item.id).unwrap().get("start_timecode"),
            Some(&expected.map(s).unwrap_or(Value::Null)),
        );
        assert_eq!(graph.media[0].clock, None);
    }
}

#[test]
fn distinguishes_selected_ambiguous_packages_from_other_scan_diagnostics() {
    let (facts, mut index) = fixture(1);
    let selected = facts.masters[0].master.tracks[0].file_mob.umid_hex();
    let unrelated = mob(999).umid_hex();
    index.files.remove(&selected);
    index.ambiguous = vec![selected.clone(), unrelated.clone()];
    let result = build(&facts, &index, "Synthetic", &["Bin".into()]).unwrap();
    assert_eq!(
        result.ambiguous_managed_packages,
        [format!("ambiguous MXF file package {selected}")]
    );
    assert_eq!(
        result.scan_issues,
        [
            format!("ambiguous MXF file package {selected}"),
            format!("ambiguous MXF file package {unrelated}"),
        ]
    );
}

#[test]
fn unused_managed_picture_with_unsupported_rate_gets_reported_omission() {
    let (mut facts, index) = fixture(2);
    let source = &mut facts.masters[0];
    source.id = "revision/bin/0/item/0".into();
    source.rate = None;
    source.master.edit_rate = 59.831;
    source.master.tracks.insert(0, MediaTrack {
        kind: KIND_PICTURE,
        index: 1,
        file_mob: mob(100),
        edit_rate: 59.831,
        length: 48,
        essence: Some(Essence::Picture {
            compression: "RI00".into(), resolution_id: 0, width: 1920, height: 1080,
            display_aspect: Some((16, 9)),
        }),
        offset: 0,
        clip_length: 48,
        gain_db: None,
        pan_raw: None,
    });
    source.offsets.push(Observed::Known(0));
    let prepare = |facts: &av::SourceProject, skip_unused: bool| {
        let originals = original::OriginalIndex {
            references: skip_unused.then(unused::References::default),
            ..Default::default()
        };
        build_selected(facts, &index, "Synthetic", &["Bin".into()], &originals, &MotionMxf::default()).unwrap()
    };
    let strict = prepare(&facts, false);
    assert_eq!(strict.graph.losses[0].aspect, "source_master");
    let omitted = prepare(&facts, true);
    assert_eq!(omitted.graph.losses[0].aspect, "unused_bin_item");
    assert!(omitted.graph.losses[0].reason.contains("unsupported edit rate 59.831"));
    facts.masters[0].master.edit_rate = 24.0;
    assert_eq!(prepare(&facts, true).graph.losses[0].aspect, "source_master");
}

#[test]
fn legacy_mxf_sibling_locators_are_retained_for_an_unused_omission() {
    let (mut facts, _) = fixture(8);
    let source = &mut facts.masters[0];
    source.id = "revision/bin/0/item/0".into();
    source.rate = Rate::new(24_000, 1001);
    source.master.edit_rate = 23.976;
    for track in &mut source.master.tracks { track.length = 48 * 2002; }
    source.master.tracks.insert(0, MediaTrack {
        kind: KIND_PICTURE, index: 1, file_mob: mob(100), edit_rate: 23.976, length: 48,
        essence: Some(Essence::Picture {
            compression: "AVHD".into(), resolution_id: 1253, width: 1920, height: 1080,
            display_aspect: Some((16, 9)),
        }),
        offset: 0, clip_length: 48, gain_db: None, pan_raw: None,
    });
    source.offsets.push(Observed::Known(0));
    source.originals = source.master.tracks.iter().enumerate().map(|(ordinal, track)| {
        let picture = ordinal == 0;
        Some(av::OriginalTrack {
            locators: vec![format!("G//Saved/Example/Lane {ordinal}.mxf")],
            descriptor_class: if picture { "CDCI" } else { "PCMA" }.into(),
            descriptor_rate: track.edit_rate, descriptor_length: track.length,
            channels: (!picture).then_some(1), sample_rate: (!picture).then_some(48_000.0),
            stored_quantization_bits: (!picture).then_some(24),
            display_raster: picture.then_some((1920, 1080)), file_track: Some(track.index),
            motion_audio_track_start: None,
        })
    }).collect();
    let check = |facts: &av::SourceProject, skip_unused: bool| {
        let options = original::OriginalOptions::default();
        let mut originals = original::OriginalIndex::scan_for_review_with_policy(facts, &options,
            true, None, &mut || Ok(())).map_err(SourceReadIssue::into_message)?;
        originals.references = skip_unused.then(unused::References::default);
        originals.resolve_legacy_mxf(facts, &Default::default(), &options, &mut || Ok(()))
            .map_err(SourceReadIssue::into_message)?;
        Ok::<_, String>(originals)
    };
    assert!(check(&facts, false).err().unwrap().contains("Legacy MXF locators"));
    let originals = check(&facts, true).unwrap();
    let result = build_selected(&facts, &Default::default(), "Synthetic", &["Bin".into()],
        &originals, &MotionMxf::default()).unwrap();
    assert_eq!(result.graph.losses[0].aspect, "unused_bin_item");
    let receipt = result.graph.sidecar("avid.source", "bin:0").unwrap();
    let Value::Arr(masters) = receipt.get("omitted_unused_masters").unwrap() else { panic!("missing omitted master") };
    let Value::Arr(tracks) = masters[0].get("tracks").unwrap() else { panic!("missing saved tracks") };
    assert_eq!(tracks.len(), 9);
    assert_eq!(tracks[8].get("saved_file_track"), Some(&Value::Int(8)));
    assert_eq!(tracks[8].get("saved_locators"), Some(&Value::Arr(vec![s("G//Saved/Example/Lane 8.mxf")])));

    let mut index = index::MediaIndex::default();
    let source = &facts.masters[0];
    for track in &source.master.tracks {
        let package = track.file_mob.umid_hex();
        let video = track.kind == KIND_PICTURE;
        let path = PathBuf::from(if video { "picture.mxf".into() } else { format!("audio-{}.mxf", track.index) });
        index.files.insert(package.clone(), path.clone());
        index.identities.insert(path, MxfIdentity {
            file_packages: vec![package], file_essence_tracks: 1,
            file_tracks: vec![(24_000, 1001, 48), if video { (24_000, 1001, 48) } else { (48_000, 1, 48 * 2002) }],
            ..Default::default()
        });
    }
    let mut probe = |path: &Path| graph::json::parse(if path == Path::new("picture.mxf") {
        r#"{"streams":[{"codec_type":"video","codec_name":"dnxhd","time_base":"1001/24000","start_pts":0,"duration_ts":48,"field_order":"progressive","sample_aspect_ratio":"1:1","width":1920,"height":1080}]}"#
    } else {
        r#"{"streams":[{"codec_type":"audio","codec_name":"pcm_s24le","time_base":"1/48000","start_pts":0,"duration_ts":96096,"sample_rate":"48000","channels":1}]}"#
    });
    let verify = |index: &index::MediaIndex, probe: &mut dyn FnMut(&Path) -> Result<Value, String>| {
        original::legacy_mxf::missing_siblings(source, index, &mut || Ok(()), probe)
    };
    assert!(verify(&index, &mut probe).unwrap().is_empty());
    index.identities.get_mut(Path::new("audio-8.mxf")).unwrap().file_tracks[0].2 -= 1;
    assert!(verify(&index, &mut probe).unwrap_err().contains("extent disagrees for A8"));

    let raw = facts.masters[0].originals[8].as_mut().unwrap();
    raw.locators[0] = "G//Saved/Example/../Lane 8.mxf".into();
    assert!(check(&facts, true).err().unwrap().contains("not a normalized absolute path"));
}

#[test]
fn graph_keeps_same_named_subclip_receipts_on_their_bin() {
    let (mut facts, index) = fixture(1);
    for (number, start) in [(2, 11), (3, 41)] {
        facts.subclips.push(av::SourceSubclip {
            id: format!("revision/bin/0/item/{number}"),
            bin: 0,
            mob: Some(mob(number)),
            name: "Same name".into(),
            rate: Rate::new(24, 1),
            backing_length: Some(100),
            start: Observed::Known(start),
            end: Observed::Known(start + 10),
            tracks: vec![av::SourceSubclipTrack {
                label: Some(1),
                component_class: Some(*b"SCLP"),
                kind: Some(KIND_SOUND),
                rate: Rate::new(24, 1),
                length: Some(100),
                target_mob: Some(mob(1)),
                target_track: Some(1),
                target_offset: Some(3),
            }],
        });
    }
    let result = build(&facts, &index, "Synthetic", &["Bin".into()]).unwrap();
    let parsed = SourceGraph::from_json_text(&result.graph.to_json_text().unwrap()).unwrap();
    assert_eq!(parsed.items.len(), 1);
    let data = parsed.sidecar("avid.subclips", "bin:0").unwrap();
    assert_eq!(Some(data), result.graph.sidecar("avid.subclips", "bin:0"));
    let Value::Arr(entries) = data else { panic!("expected subclip list") };
    assert_eq!(entries.len(), 2);
    let get = |i: usize, key: &str| {
        let Value::Obj(fields) = &entries[i] else { panic!("expected subclip object") };
        &fields.iter().find(|(name, _)| name == key).unwrap().1
    };
    assert_eq!(get(0, "name"), get(1, "name"));
    assert_ne!(get(0, "id"), get(1, "id"));
    assert_ne!(get(0, "mob"), get(1, "mob"));
    assert_eq!(get(0, "start"), &observed(&Observed::Known(11)));
    assert_eq!(get(1, "start"), &observed(&Observed::Known(41)));
    assert_eq!(get(1, "backing_length"), &Value::Int(100));
    facts.subclips[1].bin = 1;
    assert!(build(&facts, &index, "Synthetic", &["Bin".into()]).err().unwrap()
        .contains("unknown bin"));
}

fn fixture(channels: i64) -> (av::SourceProject, index::MediaIndex) {
    let rate = Rate::new(24, 1).unwrap();
    let mut index = index::MediaIndex::default();
    let mut tracks = vec![];
    for n in 1..=channels {
        let file = mob(100 + n as u32);
        let path = PathBuf::from(format!("/fake/A{n}.mxf"));
        index.files.insert(file.umid_hex(), path.clone());
        index.identities.insert(
            path,
            MxfIdentity {
                file_packages: vec![file.umid_hex()],
                file_tracks: vec![(48000, 1, 96000)],
                timecodes: vec![(240, 24, false)],
                ..Default::default()
            },
        );
        tracks.push(MediaTrack {
            kind: KIND_SOUND,
            index: n,
            file_mob: file,
            edit_rate: 48000.0,
            length: 96000,
            essence: Some(Essence::Sound {
                sample_rate: 48000.0,
                bits: 24,
                channels: 1,
            }),
            offset: 0,
            clip_length: 48,
            gain_db: None,
            pan_raw: None,
        });
    }
    let master = MasterClip {
        name: "source".into(),
        mob_id: mob(1),
        edit_rate: 24.0,
        length: 48,
        tracks,
        columns: vec![],
        color: None,
        start: None,
        aux_timecodes: vec![],
        source_file: None,
        source_path: None,
        bin: 0,
        stereo_pairs: if channels == 2 { vec![(1, 2)] } else { vec![] },
        other_groups: vec![],
        output_mix: Default::default(),
        markers: vec![],
    };
    let source = av::SourceMaster {
        id: "revision/master".into(),
        master,
        rate: Some(rate),
        offsets: (0..channels).map(|_| Observed::Known(0)).collect(),
        marker_witnesses: vec![],
        originals: vec![],
        file_clocks: vec![],
        pcm_prefix: None,
        managed_multistream: None,
        imported_audio: None,
        input_transforms: vec![],
        rendered_freeze: false,
        rendered_gain: None,
    };
    (
        av::SourceProject {
            groups: vec![],
            masters: vec![source],
            imported_stills: vec![],
            subclips: vec![],
            sequences: vec![],
            skipped: vec![],
            duplicates: vec![],
            accounted_bytes: 0,
        },
        index,
    )
}
fn event(
    id: &str,
    track: i64,
    channel: i64,
    start: i64,
    length: i64,
    combined: Option<(usize, usize)>,
) -> av::SourceEvent {
    let rate = Rate::new(24, 1).unwrap();
    av::SourceEvent {
        id: id.into(),
        volume_automation: None,
        pan_automation: None,
        subclip_center_pan_count: 0,
        subclip_gain: None,
        picture_extension: None,
        picture_chain: Vec::new(),
        motion_adapter_phase: None,
        motion_adapter_center_pan: false,
        saved_timewarp: None,
        dry_gain_bypass: false,
        equalizer: None,
        audiosuite: None,
        gain_dry_input: None,
        constant_speed: None,
        plain_submaster: false,
        owner: format!("owner-{id}"),
        event: SeqEvent {
            kind: KIND_SOUND,
            track,
            record_start: start,
            length,
            master: mob(1),
            master_track: channel,
            source_in: 0,
            via: vec![],
            speed: None,
            source_length: length,
            combined,
            edit_gain_db: Some(-3.0),
            edit_pan: None,
            color: None,
            picture: None,
            picture_nested: false,
            animatte: None,
        },
        range: Some(av::QualifiedRange {
            record_start: start,
            length,
            record_rate: rate,
            source_in: 0,
            source_length: length,
            source_rate: rate,
            route: vec![],
        }),
        reason: None,
    }
}
fn sequence(events: Vec<av::SourceEvent>) -> av::SourceSequence {
    let mut s = av::SourceSequence::new(
        "revision/seq".into(),
        "sequence".into(),
        0,
        Rate::new(24, 1),
        48,
    );
    for e in &events {
        if let Some((g, ch)) = e.event.combined {
            if s.combiner_channels.len() <= g {
                s.combiner_channels.resize(g + 1, 0);
            }
            s.combiner_channels[g] = s.combiner_channels[g].max(ch + 1);
        }
    }
    s.events = events;
    s
}

#[test]
fn timewarp_gap_keeps_the_saved_span_and_unrelated_edits() {
    let (mut facts, index) = fixture(1);
    let mut source = sequence(vec![event("audio", 1, 1, 0, 24, None)]);
    source.unresolved.push(av::SourceIssue {
        owner: "revision/seq/component/0".into(),
        reason: "unsupported Timewarp curve".into(),
        issue_kind: av::SourceIssueKind::Timewarp { length: 10 },
        route: vec![],
        record_start: Observed::Known(12),
        record_rate: Rate::new(24, 1),
        kind: KIND_PICTURE,
        track: 2,
        matte_inputs: None,
        matte_import: None,
        color_adapter: None,
        submaster_inputs: vec![],
        picture_profile: None,
        timewarp: None,
        nested_picture_motion: None,
    });
    facts.sequences.push(source);
    let graph = build(&facts, &index, "test", &["Bin".into()]).unwrap().graph;
    let [seq] = graph.sequences.as_slice() else { panic!("expected one sequence") };
    assert_eq!(seq.placements.len(), 1);
    assert_eq!(seq.placements[0].id, "audio");
    let [marker] = seq.markers.as_slice() else { panic!("expected one gap marker") };
    assert_eq!(marker.start, Rational::new(1, 2));
    assert_eq!(marker.duration, Rational::new(5, 12).unwrap());
    assert_eq!(marker.owner, MarkerOwner::Sequence);
    assert!(graph.losses.iter().any(|loss| loss.aspect == "timewarp_gap"
        && loss.reason.contains("component/0") && loss.reason.contains("V2")));
    assert!(!graph.losses.iter().any(|loss| loss.aspect == UNQUALIFIED_ROUTE_TIMING));
    facts.sequences[0].unresolved[0].record_rate = Rate::new(25, 1);
    let graph = build(&facts, &index, "test", &["Bin".into()]).unwrap().graph;
    assert!(graph.sequences[0].markers.is_empty());
    assert!(graph.losses.iter().any(|loss| loss.aspect == UNQUALIFIED_ROUTE_TIMING));
}

#[test]
fn unsupported_effect_marks_an_exact_gap_only_on_an_unoccupied_lane() {
    let (mut facts, index) = fixture(1);
    let mut source = sequence(vec![
        event("before", 1, 1, 0, 12, None),
        event("after", 1, 1, 24, 12, None),
    ]);
    source.unresolved.push(av::SourceIssue {
        owner: "revision/seq/component/1".into(),
        reason: "Gain render unavailable".into(),
        issue_kind: av::SourceIssueKind::UntranslatedEffect { effect: "AudioSuite Gain", length: 12 },
        route: vec![],
        record_start: Observed::Known(12),
        record_rate: Rate::new(24, 1),
        kind: KIND_SOUND,
        track: 1,
        matte_inputs: None,
        matte_import: None,
        color_adapter: None,
        submaster_inputs: vec![],
        picture_profile: None,
        timewarp: None,
        nested_picture_motion: None,
    });
    facts.sequences.push(source);
    let graph = build(&facts, &index, "test", &["Bin".into()]).unwrap().graph;
    let seq = &graph.sequences[0];
    assert_eq!(seq.placements.iter().map(|p| (p.id.as_str(), p.record_start)).collect::<Vec<_>>(),
        [("before", Rational::ZERO), ("after", Rational::new(1, 1).unwrap())]);
    let [marker] = seq.markers.as_slice() else { panic!("expected one gap marker") };
    assert_eq!(marker.name, "AudioSuite Gain omitted · A1");
    assert_eq!(marker.start, Rational::new(1, 2));
    assert_eq!(marker.duration, Rational::new(1, 2).unwrap());
    assert!(graph.losses.iter().any(|loss| loss.aspect == "effect_gap"));
    assert!(!graph.losses.iter().any(|loss| loss.aspect == UNQUALIFIED_ROUTE_TIMING));

    for (start, rate) in [(11, Rate::new(24, 1)), (12, Rate::new(25, 1))] {
        let issue = &mut facts.sequences[0].unresolved[0];
        issue.record_start = Observed::Known(start);
        issue.record_rate = rate;
        let graph = build(&facts, &index, "test", &["Bin".into()]).unwrap().graph;
        assert!(graph.sequences[0].markers.is_empty());
        assert!(graph.losses.iter().any(|loss| loss.aspect == UNQUALIFIED_ROUTE_TIMING));
    }

    let (mut grouped_facts, grouped_index) = fixture(2);
    let mut grouped = sequence(vec![
        event("left", 1, 1, 0, 24, Some((7, 0))),
        event("right", 1, 2, 0, 24, Some((7, 1))),
    ]);
    let mut issue = facts.sequences[0].unresolved.pop().unwrap();
    issue.record_rate = Rate::new(24, 1);
    grouped.unresolved.push(issue);
    grouped_facts.sequences.push(grouped);
    let graph = build(&grouped_facts, &grouped_index, "test", &["Bin".into()]).unwrap().graph;
    assert_eq!(graph.sequences[0].placements[0].grouped_audio[0].track, 1);
    assert!(graph.sequences[0].markers.is_empty());
    assert!(graph.losses.iter().any(|loss| loss.aspect == UNQUALIFIED_ROUTE_TIMING));
}

#[test]
fn matte_key_loss_is_locatable_after_graph_reload() {
    let (mut facts, index) = fixture(1);
    let mut source = sequence(vec![event("unchanged-audio", 1, 1, 0, 24, None)]);
    let start = Observed::Known(17);
    source.unresolved.push(av::SourceIssue {
        owner: "revision/seq/component/0".into(),
        reason: "Avid Matte Key (EFF2_2DMATTE) omitted".into(),
        issue_kind: av::SourceIssueKind::MatteKey { length: Some(3) },
        route: vec![],
        record_start: start.clone(),
        record_rate: Rate::new(24, 1),
        kind: KIND_PICTURE,
        track: 2,
        matte_inputs: Some(Box::new(av::MatteInputs {
            background: av::MatteBackground::LowerTrack,
            fill: Some(av::MatteInput {
                mob: mob(10), track: 1, start: 8, length: 3, rate: Rate::new(24, 1).unwrap(),
            }),
            matte: None,
        })),
        matte_import: Some(av::MatteImport {
            ignore_alpha: Some(false),
            invert_alpha: None,
            locators: vec!["/Synthetic/alpha.png".into()],
        }),
        color_adapter: None,
        submaster_inputs: Vec::new(),
        picture_profile: None,
        timewarp: None,
        nested_picture_motion: None,
    });
    facts.sequences.push(source);
    let built = run(&facts, &index).graph;
    let graph = SourceGraph::from_json_text(&built.to_json_text().unwrap()).unwrap();
    assert_eq!(graph.sequences[0].placements[0].id, "unchanged-audio");
    let loss = graph.losses.iter().find(|l| l.aspect == "picture_effect").unwrap();
    assert_eq!(loss.object, "revision/seq");
    assert!(loss.reason.contains("V2 at record frame 17 for 3 frames (24 fps)"));
    let receipt = graph.sidecar("avid.source", "revision/seq").unwrap();
    let Some(Value::Arr(issues)) = receipt.get("unresolved") else { panic!("expected issues") };
    assert_eq!(issues[0].get("effect_id"), Some(&s("EFF2_2DMATTE")));
    assert_eq!(issues[0].get("owner"), Some(&s("revision/seq/component/0")));
    assert_eq!(issues[0].get("record_start"), Some(&observed(&start)));
    assert_eq!(issues[0].get("record_length"), Some(&Value::Int(3)));
    let inputs = issues[0].get("matte_inputs").unwrap();
    assert_eq!(inputs.get("background"), Some(&s("lower_track")));
    assert_eq!(inputs.get("matte"), Some(&Value::Null));
    let fill = inputs.get("fill").unwrap();
    assert_eq!(fill.get("mob"), Some(&s(mob(10).urn())));
    assert_eq!(fill.get("start"), Some(&Value::Int(8)));
    let import = issues[0].get("matte_import").unwrap();
    assert_eq!(import.get("ignore_alpha"), Some(&Value::Bool(false)));
    assert_eq!(import.get("invert_alpha"), Some(&Value::Null));
    assert_eq!(import.get("saved_locators"), Some(&Value::Arr(vec![s("/Synthetic/alpha.png")])));
}

#[test]
fn color_adapter_receipt_binds_only_the_owned_picture_placement() {
    const RANGE: &str = concat!(
        "<ColorTransformationList><ColorTransformation><LinearLut>",
        "<Name>Levels scaling (video levels to full range)</Name><BitDepth>10</BitDepth>",
        "<Black>64</Black><White>940</White></LinearLut></ColorTransformation></ColorTransformationList>",
    );
    let (mut facts, mut index) = fixture(1);
    let track = &mut facts.masters[0].master.tracks[0];
    track.kind = KIND_PICTURE;
    track.edit_rate = 24.0;
    track.length = 48;
    track.essence = Some(Essence::Picture {
        compression: "AVHD".into(),
        resolution_id: 0,
        width: 1920,
        height: 1080,
        display_aspect: Some((16, 9)),
    });
    let file_mob = track.file_mob;
    index.identities.values_mut().next().unwrap().file_tracks = vec![(24, 1, 48)];
    let mut picture = event("picture", 1, 1, 0, 24, None);
    picture.event.kind = KIND_PICTURE;
    let mut source = sequence(vec![picture]);
    source.unresolved.push(av::SourceIssue {
        owner: "sequence/component/1".into(),
        reason: "Avid Color Adapter omitted".into(),
        issue_kind: av::SourceIssueKind::ColorAdapter { length: Some(24) },
        route: vec![],
        record_start: Observed::Known(0),
        record_rate: Rate::new(24, 1),
        kind: KIND_PICTURE,
        track: 1,
        matte_inputs: None,
        matte_import: None,
        color_adapter: Some(Box::new(av::SourceColorAdapter {
            centered_still_fit: false,
            master: mob(1),
            master_track: 1,
            source_in: 0,
            subclip_master: None,
            subclip_motion: None,
            picture_file: Observed::Known(file_mob),
            transform: RANGE.into(),
            neutral_wrapper: true,
            wrapper_clip_color: Some([48_896; 3]),
            managed_picture_span: None,
        })),
        submaster_inputs: vec![],
        picture_profile: None,
        timewarp: None,
        nested_picture_motion: None,
    });
    facts.sequences.push(source);
    for (owner, expected) in [
        ("sequence/component/1/component/0", Some("picture")),
        ("sequence/component/10/component/0", None),
    ] {
        facts.sequences[0].events[0].owner = owner.into();
        let built = run(&facts, &index).graph;
        let graph = SourceGraph::from_json_text(&built.to_json_text().unwrap()).unwrap();
        let sidecar = graph.sidecar("avid.source", "revision/seq").unwrap();
        let Some(Value::Arr(issues)) = sidecar.get("unresolved") else { panic!("expected issues") };
        let adapter = issues[0].get("color_adapter").unwrap();
        assert_eq!(adapter.get("placement"), Some(&expected.map(s).unwrap_or(Value::Null)));
        assert_eq!(adapter.get("wrapper_clip_color_rgb16"),
            Some(&Value::Arr(vec![Value::Int(48_896); 3])));
        assert_eq!(adapter.get("picture_file"), Some(&s(file_mob.urn())));
        assert_eq!(adapter.get("neutral_wrapper"), Some(&Value::Bool(true)));
        let range = adapter.get("linear_range").unwrap();
        assert_eq!(range.get("direction"), Some(&s("video_to_full")));
        assert_eq!(range.get("bit_depth"), Some(&Value::Int(10)));
        assert_eq!(range.get("video_black"), Some(&Value::Int(64)));
        assert_eq!(range.get("video_white"), Some(&Value::Int(940)));
        let loss = graph.losses.iter().find(|loss| loss.aspect == "source_color_adapter"
            && loss.object == expected.unwrap_or("revision/seq")).unwrap();
        assert_eq!(loss.reason.contains("unapplied"), expected.is_some());
    }
    let source = &mut facts.sequences[0];
    let adapter = source.unresolved[0].color_adapter.as_mut().unwrap();
    adapter.master = mob(2);
    adapter.master_track = 4;
    adapter.source_in = 3;
    adapter.subclip_master = Some(Box::new(av::ColorAdapterMaster {
        mob: mob(1), track: 1, source_in: 8,
    }));
    let event = &mut source.events[0];
    event.owner = "sequence/component/1/component/0".into();
    event.event.source_in = 8;
    let range = event.range.as_mut().unwrap();
    range.source_in = 8;
    range.route = vec![
        av::RouteWitness {
            mob: mob(2), track: 4, rate: Rate::new(24, 1),
            source_offset: Observed::Known(3), length: Observed::Known(24),
        },
        av::RouteWitness {
            mob: mob(1), track: 1, rate: Rate::new(24, 1),
            source_offset: Observed::Known(5), length: Observed::Known(40),
        },
    ];
    for (underlying_start, expected) in [(5, Some("picture")), (6, None)] {
        facts.sequences[0].events[0].range.as_mut().unwrap().route[1].source_offset =
            Observed::Known(underlying_start);
        let graph = run(&facts, &index).graph;
        let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
        let sidecar = graph.sidecar("avid.source", "revision/seq").unwrap();
        let Some(Value::Arr(issues)) = sidecar.get("unresolved") else { panic!("expected issues") };
        let adapter = issues[0].get("color_adapter").unwrap();
        assert_eq!(adapter.get("placement"), Some(&expected.map(s).unwrap_or(Value::Null)));
        assert_eq!(adapter.get("master"), Some(&s(mob(2).urn())));
        let backing = adapter.get("subclip_master").unwrap();
        assert_eq!(backing.get("mob"), Some(&s(mob(1).urn())));
        assert_eq!(backing.get("source_in"), Some(&Value::Int(8)));
    }
    let adapter = facts.sequences[0].unresolved[0].color_adapter.as_mut().unwrap();
    adapter.subclip_master = None;
    adapter.picture_file = Observed::Unresolved("unqualified retimed picture".into());
    adapter.subclip_motion = Some(Box::new(av::SourceSubclipMotion {
        input_mob: mob(1), input_track: 1, input_start: 5, input_length: 99, length: 50,
        advanced: false, span_ratio: (50, 99), speed: 2.0, speed_key_frame: (-3, 1),
        position: [0.0, 100.0], phase_offset: 2, offset_adjust: 6.0, render_mode: 2,
        ignore_render_setting: 0, new_source_calculation: true, graph_mode: Some(1),
        sampled_input_frames: None,
        sampled_picture_master: None,
        sampled_picture_file: None,
    }));
    let built = run(&facts, &index).graph;
    let graph = SourceGraph::from_json_text(&built.to_json_text().unwrap()).unwrap();
    let sidecar = graph.sidecar("avid.source", "revision/seq").unwrap();
    let Some(Value::Arr(issues)) = sidecar.get("unresolved") else { panic!("expected issues") };
    let adapter = issues[0].get("color_adapter").unwrap();
    assert_eq!(adapter.get("placement"), Some(&Value::Null));
    assert!(adapter.get("picture_file").is_none());
    let motion = adapter.get("subclip_motion").unwrap();
    assert_eq!(motion.get("speed"), Some(&Value::Float(2.0)));
    assert_eq!(motion.get("span_ratio"), Some(&Value::Arr(vec![Value::Int(50), Value::Int(99)])));
    assert_eq!(motion.get("speed_key_frame"), Some(&Value::Arr(vec![Value::Int(-3), Value::Int(1)])));
    assert!(motion.get("sampled_input_frames").is_none());
}

#[test]
fn layered_submaster_keeps_ordered_input_facts_without_placing_a_composite() {
    let (mut facts, index) = fixture(1);
    let mut source = sequence(vec![event("unchanged-audio", 1, 1, 0, 24, None)]);
    source.unresolved.push(av::SourceIssue {
        owner: "revision/seq/component/0".into(),
        reason: "Avid layered Submaster omitted".into(),
        issue_kind: av::SourceIssueKind::LayeredSubmaster { length: Some(48), input_count: 3 },
        route: Vec::new(),
        record_start: Observed::Known(0),
        record_rate: Rate::new(24, 1),
        kind: KIND_PICTURE,
        track: 1,
        matte_inputs: None,
        matte_import: None,
        color_adapter: None,
        submaster_inputs: [b"FILL", b"SCLP", b"TKFX"].into_iter().enumerate().map(|(i, tag)| {
            av::SubmasterInput {
                track: Some(i as i64 + 1),
                component_class: Some(*b"SEQU"),
                segment_class: Some(*tag),
                segment_effect: (tag == b"TKFX").then(|| "EFF2_BLEND_PIP".into()),
                picture_source: (tag == b"SCLP").then(|| av::SubmasterPictureInput {
                    mob: mob(10),
                    track: 1,
                    source_start: 17,
                    local_start: 4,
                    length: 40,
                    local_length: 48,
                    rate: Rate::new(24, 1).unwrap(),
                }),
            }
        }).collect(),
        picture_profile: None,
        timewarp: None,
        nested_picture_motion: None,
    });
    facts.sequences.push(source);
    let built = run(&facts, &index).graph;
    let graph = SourceGraph::from_json_text(&built.to_json_text().unwrap()).unwrap();
    let [edit] = graph.sequences[0].placements.as_slice() else { panic!("expected one audio edit") };
    assert_eq!(edit.id, "unchanged-audio");
    assert!(graph.losses.iter().any(|l| l.aspect == "picture_effect"
        && l.reason.contains("V1 at record frame 0 for 48 frames (24 fps)")));
    let receipt = graph.sidecar("avid.source", "revision/seq").unwrap();
    let Some(Value::Arr(issues)) = receipt.get("unresolved") else { panic!("expected issues") };
    assert_eq!(issues[0].get("effect_id"), Some(&s("EFF_SUBMASTER")));
    assert_eq!(issues[0].get("input_count"), Some(&Value::Int(3)));
    assert_eq!(issues[0].get("record_length"), Some(&Value::Int(48)));
    let Some(Value::Arr(inputs)) = issues[0].get("submaster_inputs") else { panic!("expected layers") };
    assert_eq!(inputs.len(), 3);
    for (i, (input, class)) in inputs.iter().zip(["FILL", "SCLP", "TKFX"]).enumerate() {
        assert_eq!(input.get("track"), Some(&Value::Int(i as i64 + 1)));
        assert_eq!(input.get("component_class"), Some(&s("SEQU")));
        assert_eq!(input.get("segment_class"), Some(&s(class)));
    }
    assert_eq!(inputs[2].get("segment_effect"), Some(&s("EFF2_BLEND_PIP")));
    assert!(inputs[0].get("picture_source").is_none() && inputs[2].get("picture_source").is_none());
    let saved_picture = inputs[1].get("picture_source").unwrap();
    assert_eq!(saved_picture.get("mob"), Some(&s(mob(10).urn())));
    assert_eq!(saved_picture.get("track"), Some(&Value::Int(1)));
    assert_eq!(saved_picture.get("source_start"), Some(&Value::Int(17)));
    assert_eq!(saved_picture.get("local_start"), Some(&Value::Int(4)));
    assert_eq!(saved_picture.get("length"), Some(&Value::Int(40)));
    assert_eq!(saved_picture.get("local_length"), Some(&Value::Int(48)));
    assert_eq!(saved_picture.get("rate"), Some(&s("24")));
}

#[test]
fn avid_pan_keys_survive_graph_reload_on_the_owning_placement() {
    let (mut facts, index) = fixture(1);
    let mut edit = event("pan-edit", 1, 1, 0, 24, None);
    edit.pan_automation = Some(av::PanAutomation::Keys(vec![
        av::PanPoint { frame_numerator: 0, frame_denominator: 1, raw_pan: 0 },
        av::PanPoint { frame_numerator: 25, frame_denominator: 2, raw_pan: 32768 },
    ]));
    facts.sequences.push(sequence(vec![edit]));
    let graph = run(&facts, &index).graph;
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let sequence = &graph.sequences[0];
    let placement = &sequence.placements[0];
    let receipt = graph.sidecar("avid.source", &sequence.id).unwrap();
    let Some(Value::Arr(events)) = receipt.get("native_events") else {
        panic!("expected native events");
    };
    assert_eq!(events[0].get("placement_id"), Some(&s(&placement.id)));
    let pan = events[0].get("controls").and_then(|c| c.get("pvol_pan")).unwrap();
    let Some(Value::Arr(keys)) = pan.get("keys") else {
        panic!("expected native pan keys");
    };
    assert_eq!(keys[0].get("raw_pan"), Some(&Value::Int(0)));
    assert_eq!(keys[1].get("frame_numerator"), Some(&Value::Int(25)));
    assert_eq!(keys[1].get("frame_denominator"), Some(&Value::Int(2)));
    assert_eq!(keys[1].get("raw_pan"), Some(&Value::Int(32768)));
    assert!(placement.volume_keys.is_empty());
    let [effect] = placement.effects.as_slice() else { panic!("expected one clip pan curve") };
    let keys = graph::audio_pan::read_linear(effect, placement.duration).unwrap();
    assert_eq!(keys[0].pan, Rational::integer(-1));
    assert_eq!(keys[1].at, Rational::new(25, 48).unwrap());
    assert_eq!(keys[1].pan, Rational::integer(1));
    assert!(!graph.losses.iter().any(|l| l.object == placement.id && l.aspect == "pan_automation"));
}
fn run(f: &av::SourceProject, i: &index::MediaIndex) -> ReadResult {
    build(f, i, "project", &["bin".into()]).unwrap()
}

#[test]
fn native_color_receipt_survives_an_unavailable_picture_route() {
    use avid_reader::color::{ColorCorrection, ParamValue};
    use avid_reader::transform::{CornerPin, Kind, PictureEffect};
    let (mut facts, index) = fixture(1);
    let audio = event("audio", 1, 1, 0, 24, None);
    let mut picture = event("picture", 1, 1, 0, 24, None);
    picture.event.kind = KIND_PICTURE;
    picture.event.master = mob(2);
    let pin = CornerPin {
        corners: [[0.1, 0.2], [0.8, 0.1], [0.9, 0.8], [0.2, 0.9]],
        crop: [0.05, 0.1, 0.15, 0.2],
        from_transform: false,
    };
    picture.event.picture = Some(PictureEffect {
        kind: Kind::Warp, params: ColorCorrection::default(), corner_pin: Some(pin),
        compact_affine_warp: false,
    });
    let opaque = vec![17; 4097];
    picture.event.color = Some(ColorCorrection {
        params: vec![
            ("scalar".into(), ParamValue::Float(-0.0)),
            ("payload".into(), ParamValue::Bytes(opaque)),
        ],
        ..Default::default()
    });
    facts.sequences.push(sequence(vec![audio, picture]));
    let graph = run(&facts, &index).graph;
    assert_eq!(graph.sequences[0].placements.len(), 1);
    let saved = graph.to_json_text().unwrap();
    let reopened = SourceGraph::from_json_text(&saved).unwrap();
    assert_eq!(graph, reopened);
    let source = reopened.sidecar("avid.source", &graph.sequences[0].id).unwrap();
    let Some(Value::Arr(events)) = source.get("native_events") else {
        panic!("missing native event receipts");
    };
    assert_eq!(events[0].get("placement_id"), Some(&s("audio")));
    assert_eq!(events[1].get("placement_id"), Some(&Value::Null));
    let corner = events[1].get("controls").and_then(|v| v.get("picture"))
        .and_then(|v| v.get("corner_pin")).unwrap();
    assert_eq!(corner.get("source_crop_ltrb"), Some(&Value::Arr(pin.crop.map(Value::Float).into())));
    let Some(Value::Arr(parameters)) = events[1].get("controls").and_then(|v| v.get("color")) else {
        panic!("missing native color parameters");
    };
    assert_eq!(
        parameters[0].get("value").and_then(|v| v.get("float_bits")),
        Some(&s("8000000000000000"))
    );
    let payload = parameters[1].get("value").unwrap();
    assert_eq!(payload.get("byte_count"), Some(&Value::Int(4097)));
    assert!(matches!(payload.get("sha256"), Some(Value::Str(hash)) if hash.len() == 64));
    assert!(payload.get("bytes_hex").is_none());
}

#[test]
fn ordered_projective_warp_receipt_survives_without_linked_picture() {
    use avid_reader::color::{ColorCorrection, ParamValue};
    use avid_reader::transform::{CornerPin, Kind, PictureEffect};
    let (mut facts, index) = fixture(1);
    let mut picture = event("picture", 1, 1, 0, 24, None);
    picture.event.kind = KIND_PICTURE;
    picture.event.master = mob(2);
    let pin = CornerPin {
        corners: [[0.1, 0.2], [0.8, 0.1], [0.9, 0.8], [0.2, 0.9]],
        crop: [0.05, 0.1, 0.15, 0.2],
        from_transform: true,
    };
    let yaw_id = "b981568d7e84d511935a50f857c10000";
    picture.picture_chain = vec![
        av::PictureStage::Extension(av::PictureExtension::Flip {
            native_id: "EFF2_BLEND_FLIP_HORZ", horizontal: true, vertical: false,
        }),
        av::PictureStage::Static(std::sync::Arc::new(PictureEffect {
            kind: Kind::Warp,
            params: ColorCorrection { params: vec![(yaw_id.into(), ParamValue::Float(30.0))], ..Default::default() },
            corner_pin: Some(pin),
            compact_affine_warp: false,
        })),
    ];
    facts.sequences.push(sequence(vec![event("audio", 1, 1, 0, 24, None), picture]));
    let graph = run(&facts, &index).graph;
    let reopened = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    assert_eq!(graph, reopened);
    let sequence = &reopened.sequences[0];
    assert_eq!(sequence.placements.len(), 1);
    let source = reopened.sidecar("avid.source", &sequence.id).unwrap();
    let Some(Value::Arr(events)) = source.get("native_events") else { panic!() };
    assert_eq!(events[1].get("placement_id"), Some(&Value::Null));
    let Some(Value::Arr(stages)) = events[1].get("controls").unwrap()
        .get("projective_picture_stages") else { panic!() };
    let [stage] = stages.as_slice() else { panic!() };
    assert_eq!(stage.get("stage"), Some(&Value::Int(1)));
    let controls = stage.get("controls").unwrap();
    let Some(Value::Arr(params)) = controls.get("params") else { panic!() };
    assert_eq!(params[0].get("id"), Some(&s(yaw_id)));
    assert_eq!(controls.get("projected_corners").unwrap().get("source_crop_ltrb"),
        Some(&Value::Arr(pin.crop.map(Value::Float).into())));
}

#[test]
fn source_resize_curves_override_the_cached_static_values() {
    use avid_reader::color::{ColorCorrection, ParamValue};
    use avid_reader::transform::{Kind, PictureEffect};
    let (mut f, mut i) = fixture(1);
    let track = &mut f.masters[0].master.tracks[0];
    track.kind = KIND_PICTURE;
    track.edit_rate = 24.0;
    track.length = 48;
    track.essence = Some(Essence::Picture {
        compression: "AVHD".into(), resolution_id: 0, width: 1920, height: 1080,
        display_aspect: Some((16, 9)),
    });
    i.identities.values_mut().next().unwrap().file_tracks = vec![(24, 1, 48)];
    let mut e = event("resize", 1, 1, 0, 48, None);
    e.event.kind = KIND_PICTURE;
    e.event.picture = Some(PictureEffect {
        kind: Kind::Resize,
        corner_pin: None,
        compact_affine_warp: false,
        params: ColorCorrection { params: vec![
            ("2581568d7e84d511935a50f857c10000".into(), ParamValue::Float(3000.0)),
        ], ..Default::default() },
    });
    e.picture_extension = Some(Box::new(av::PictureExtension::Resize([
        av::ResizeKey { frame: 0, position: [-0.15, 0.075], scale: [80.0, 120.0], crop: [0.0; 4] },
        av::ResizeKey { frame: 47, position: [0.2, -0.125], scale: [135.0, 65.0], crop: [0.0; 4] },
    ])));
    f.sequences.push(sequence(vec![e]));
    let out = run(&f, &i);
    let p = &out.graph.sequences[0].placements[0];
    assert_eq!(p.transform.as_ref().unwrap().position, (-0.15, 0.075));
    assert_eq!(p.transform.as_ref().unwrap().scale, (80.0, 120.0));
    assert_eq!(p.effects[0].kind, "avid.resize_linear");
}
#[test]
fn mixed_and_animated_animatte_controls_survive_graph_reload() {
    use avid_reader::animatte::{AniMatteData, AniMattePoint, AniMatteShape, AniMatteState};
    let (mut facts, mut index) = fixture(1);
    let track = &mut facts.masters[0].master.tracks[0];
    track.kind = KIND_PICTURE;
    track.edit_rate = 24.0;
    track.length = 48;
    track.essence = Some(Essence::Picture {
        compression: "AVHD".into(), resolution_id: 0, width: 1920, height: 1080,
        display_aspect: Some((16, 9)),
    });
    index.identities.values_mut().next().unwrap().file_tracks = vec![(24, 1, 48)];
    let rectangle = [[-120.0, -80.0], [0.0, -80.0], [0.0, 0.0], [-120.0, 0.0]];
    let inside = AniMatteShape {
        id: 1,
        key_out: false,
        opacity: 100.0,
        feather: [0.0, 0.0],
        points: rectangle.into_iter().map(|xy| AniMattePoint {
            xy, incoming: [0.0; 2], outgoing: [0.0; 2], corner: true,
        }).collect(),
    };
    let mut outside = inside.clone();
    outside.id = 2;
    outside.key_out = true;
    for point in &mut outside.points {
        point.xy[0] += 66.0;
    }
    let mut end = outside.clone();
    end.opacity = 46.0;
    end.feather = [21.0, 7.0];
    let mut edit = event("animatte-edit", 1, 1, 0, 48, None);
    edit.event.kind = KIND_PICTURE;
    edit.event.animatte = Some(AniMatteData {
        states: vec![
            AniMatteState { frame: 0, shapes: vec![inside.clone(), outside] },
            AniMatteState { frame: 47, shapes: vec![inside, end] },
        ],
        acceleration_q30: 0,
        tracker_xy: None,
        not_converted: vec![],
    });
    facts.sequences.push(sequence(vec![edit]));
    let graph = run(&facts, &index).graph;
    let graph = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
    let sequence = &graph.sequences[0];
    let placement = &sequence.placements[0];
    assert!(placement.effects.is_empty());
    let receipt = graph.sidecar("avid.source", &sequence.id).unwrap();
    let Some(Value::Arr(events)) = receipt.get("native_events") else { panic!("missing native events") };
    assert_eq!(events[0].get("placement_id"), Some(&s(&placement.id)));
    let matte = events[0].get("controls").and_then(|c| c.get("animatte")).unwrap();
    let Some(Value::Arr(states)) = matte.get("states") else { panic!("missing shape states") };
    assert_eq!(states.len(), 2);
    let Some(Value::Arr(shapes)) = states[1].get("shapes") else { panic!("missing shapes") };
    assert_eq!(states[1].get("frame"), Some(&Value::Int(47)));
    assert_eq!(shapes[0].get("key_out"), Some(&Value::Bool(false)));
    assert_eq!(shapes[1].get("key_out"), Some(&Value::Bool(true)));
    assert_eq!(shapes[1].get("opacity"), Some(&Value::Float(46.0)));
    assert_eq!(shapes[1].get("feather"), Some(&Value::Arr(vec![Value::Float(21.0), Value::Float(7.0)])));
    assert!(graph.losses.iter().any(|loss| loss.object == placement.id
        && loss.reason.contains("mixed or animated Key In/Out")));
}

#[test]
fn exact_clock_and_unknown_offset_are_distinct_from_compatibility() {
    let (mut f, mut i) = fixture(1);
    f.masters[0].offsets[0] = Observed::Unresolved("absent".into());
    let raw = i.identities.values_mut().next().unwrap();
    raw.file_essence_tracks = 1;
    raw.qualified_file_clocks.push(QualifiedClock {
        effective_count: 240,
        rate: (24000, 1001),
        rounded_base: 24,
        drop: false,
        source_offset: 0,
        source_rate: (24000, 1001),
        origin_count: 240,
        source_package: "package".into(),
        timecode_track: "track".into(),
        route: "qualified".into(),
    });
    let out = run(&f, &i);
    assert_eq!(out.graph.media[0].start, Rational::new(1001, 100));
    let clock = out.graph.media[0].clock.as_ref().unwrap();
    assert_eq!(clock.effective_count, 240);
    assert_eq!(clock.rate, Rational::new(24000, 1001).unwrap());
    assert_eq!(clock.display_base, 24);
    assert_eq!(clock.provenance.source_package, "package");
    assert!(out
        .graph
        .sidecar("avid.source", &out.graph.media[0].id)
        .unwrap()
        .get("qualified_clock")
        .is_none());
    assert_eq!(out.graph.items[0].media[0].source_offset, None);
    assert_eq!(
        out.graph
            .sidecar("avid.source", &out.graph.media[0].id)
            .unwrap()
            .get("legacy_start"),
        Some(&s("10"))
    );
    assert!(out.graph.losses.iter().any(|l| l.aspect == "source_offset"));
    let encoded = out.graph.to_json_text().unwrap();
    assert_eq!(SourceGraph::from_json_text(&encoded).unwrap(), out.graph);
}
#[test]
fn independent_lanes_remain_independent_and_sparse() {
    let (mut f, i) = fixture(2);
    f.sequences.push(sequence(vec![
        event("e1", 1, 1, 0, 24, None),
        event("e2", 11, 2, 0, 24, None),
    ]));
    let out = run(&f, &i);
    let seq = &out.graph.sequences[0];
    assert_eq!(seq.placements.len(), 2);
    assert_eq!(
        seq.tracks.iter().map(|t| t.index).collect::<Vec<_>>(),
        [1, 11]
    );
    assert_eq!(seq.start, None);
    assert_eq!(seq.clock, None);
    assert_eq!(seq.frame_size, None);
    assert_eq!(
        out.graph
            .sidecar("avid.source", &seq.id)
            .unwrap()
            .get("compatibility_frame_size"),
        Some(&Value::Arr(vec![Value::Int(1920), Value::Int(1080)]))
    );
}
#[test]
fn native_combiner_is_atomic_and_bindings_are_deduplicated() {
    let (mut f, i) = fixture(2);
    let mut seq = sequence(vec![
        event("e1", 1, 1, 0, 24, Some((7, 0))),
        event("e2", 1, 2, 0, 24, Some((7, 1))),
    ]);
    seq.markers.push(av::SourceMarker {
        id: "marker".into(),
        owner: "native-combiner".into(),
        kind: KIND_SOUND,
        track: 1,
        owner_kind: extract::MarkerOwner::Edit,
        marker: AvidMarker {
            frame: 3,
            name: "note".into(),
            ..Default::default()
        },
        local_offset: Observed::Known(3),
        local_rate: Rate::new(24, 1),
        record_frame: Some(3),
        binding: av::Binding {
            placements: vec!["e1".into(), "e2".into()],
            reason: None,
        },
    });
    f.sequences.push(seq);
    let out = run(&f, &i);
    let seq = &out.graph.sequences[0];
    assert_eq!(seq.placements.len(), 1);
    assert_eq!(seq.placements[0].grouped_audio[0].streams, [1, 2]);
    assert_eq!(seq.markers[0].binding.placements, ["e1"]);
    assert_eq!(seq.markers[0].source, Rational::new(1, 8));
    assert!(out.graph.sidecar("avid.source", "e2").is_none());
}
#[test]
fn missing_stereo_member_preserves_survivor_and_original_numbering() {
    let (mut f, mut i) = fixture(2);
    i.files.remove(&mob(101).umid_hex());
    f.sequences.push(sequence(vec![
        event("left", 1, 1, 0, 24, Some((7, 0))),
        event("right", 1, 2, 0, 24, Some((7, 1))),
    ]));
    let out = run(&f, &i);
    assert_eq!(out.graph.media[0].id, "clip:clip-1:audio:1");
    let p = &out.graph.sequences[0].placements[0];
    assert_eq!(p.audio_streams, [2]);
    assert_eq!(p.track_pans, [Some(100.0)]);
    assert!(!out.audio_group_issues.is_empty());
    let originals = original::OriginalIndex { allow_media_gaps: true, ..Default::default() };
    let linked = build_selected(&f, &i, "Synthetic", &["Bin".into()], &originals, &MotionMxf::default()).unwrap();
    assert!(linked.audio_group_issues.is_empty());
    assert_eq!(linked.graph.sequences[0].placements, out.graph.sequences[0].placements);
    assert!(linked.graph.items[0].stream_groups.is_empty());
    assert!(linked.graph.losses.iter().any(|l| l.aspect == "audio_group" && l.reason.contains("missing a channel")));
}
#[test]
fn unknown_marker_local_position_is_not_invented() {
    let (mut f, i) = fixture(1);
    f.masters[0]
        .master
        .markers
        .push((KIND_SOUND, 1, AvidMarker::default()));
    f.masters[0].marker_witnesses.push(av::MasterMarkerWitness {
        owner: "unknown-owner".into(),
        kind: KIND_SOUND,
        track: 1,
        local_offset: Observed::Unresolved("missing comp_offset".into()),
        local_rate: Rate::new(24, 1),
        master_frame: None,
    });
    let out = run(&f, &i);
    let m = &out.graph.items[0].markers[0];
    assert_eq!(m.start, None);
    assert_eq!(m.owner_local, None);
    assert!(m.binding.reason.is_some());
}
#[test]
fn transition_endpoints_keep_native_identity() {
    let (mut f, i) = fixture(1);
    let mut seq = sequence(vec![
        event("out", 1, 1, 0, 24, None),
        event("in", 1, 1, 18, 24, None),
    ]);
    seq.transitions.push(av::SourceTransition {
        id: "t".into(),
        kind: KIND_SOUND,
        track: 1,
        position: Observed::Known(18),
        length: Observed::Known(6),
        cutpoint: Observed::Known(3),
        rate: Rate::new(24, 1),
        effect: Some("DISSOLVE".into()),
        audio_curve_attribute: None,
        audio_curve_parameter: None,
        incoming_filler: None,
        gain_gap_edges: [None, None],
        outgoing: av::Binding {
            placements: vec!["out".into()],
            reason: None,
        },
        incoming: av::Binding {
            placements: vec!["in".into()],
            reason: None,
        },
    });
    f.sequences.push(seq);
    let out = run(&f, &i);
    let t = &out.graph.sequences[0].transitions[0];
    assert_eq!(t.cut, Rational::new(7, 8).unwrap());
    assert_eq!(t.outgoing.placements, ["out"]);
    assert_eq!(
        out.graph.sequences[0].placements[0].duration,
        Rational::integer(1)
    );
}
#[test]
fn oversized_facts_and_expansion_refuse() {
    let (mut f, i) = fixture(1);
    f.accounted_bytes = av::MAX_SOURCE_FACT_BYTES;
    assert!(build(&f, &i, "p", &["b".into()]).is_ok());
    f.accounted_bytes = av::MAX_SOURCE_FACT_BYTES + 1;
    assert!(build(&f, &i, "p", &["b".into()]).is_err());
    let mut b = Budget::default();
    assert!(b.charge(graph::source::MAX_SOURCE_GRAPH_BYTES).is_ok());
    assert!(b.charge(MAX_GRAPH_CONSTRUCTION_BYTES - graph::source::MAX_SOURCE_GRAPH_BYTES).is_ok());
    assert!(b.charge(1).is_err());
}

#[test]
fn native_only_controls_and_missing_marker_witness_survive() {
    use avid_reader::{
        color::{ColorCorrection, ParamValue},
        transform::{Kind, PictureEffect},
    };
    let (mut facts, index) = fixture(1);
    let mut e = event("e", 1, 1, 0, 24, None);
    e.event.picture = Some(PictureEffect {
        kind: Kind::Resize,
        corner_pin: None,
        compact_affine_warp: false,
        params: ColorCorrection {
            params: vec![(
                "unknown-control".into(),
                ParamValue::Bytes(vec![0, 1, 254, 255]),
            )],
            ..Default::default()
        },
    });
    let mut seq = sequence(vec![e]);
    seq.rate = None;
    seq.markers.push(av::SourceMarker {
        id: "mk".into(),
        owner: "owner".into(),
        kind: 0,
        track: 0,
        owner_kind: extract::MarkerOwner::Sequence,
        marker: AvidMarker {
            frame: 37,
            length: Some(5),
            ..Default::default()
        },
        local_offset: Observed::Unresolved("missing native comp_offset".into()),
        local_rate: None,
        record_frame: None,
        binding: av::Binding {
            placements: vec![],
            reason: Some("unqualified owner clock".into()),
        },
    });
    facts.sequences.push(seq);
    let out = run(&facts, &index);
    assert!(out.graph.sequences.is_empty());
    assert!(out
        .graph
        .losses
        .iter()
        .any(|l| l.object == "bin:0" && l.aspect == "source_sequence"));
    let text = out.graph.to_json_text().unwrap();
    for required in [
        "unknown-control",
        "0001FEFF",
        "missing native comp_offset",
        "unqualified owner clock",
        "\"frame\":37",
        "\"length\":5",
    ] {
        assert!(text.contains(required), "{required}");
    }
}

#[test]
fn malformed_rate_facts_refuse_without_panic() {
    let (mut facts, index) = fixture(1);
    facts.masters[0].rate = Some(Rate { num: 0, den: 0 });
    assert!(matches!(build(&facts,&index,"p",&["b".into()]),Err(e) if e.contains("rate")));
}

#[test]
fn spanned_marker_without_rate_stays_raw_and_indexes_surviving_markers() {
    let (mut facts, index) = fixture(1);
    let m = &mut facts.masters[0];
    for (n, length, local_rate) in [(0, Some(12), None), (1, None, Rate::new(24, 1))] {
        m.master.markers.push((
            KIND_SOUND,
            1,
            AvidMarker {
                frame: n,
                length,
                name: format!("m{n}"),
                ..Default::default()
            },
        ));
        m.marker_witnesses.push(av::MasterMarkerWitness {
            owner: format!("owner{n}"),
            kind: KIND_SOUND,
            track: 1,
            local_offset: Observed::Known(n),
            local_rate,
            master_frame: Some(n),
        });
    }
    let out = run(&facts, &index);
    assert_eq!(out.graph.items[0].markers.len(), 1);
    assert_eq!(out.graph.items[0].markers[0].name, "m1");
    assert_eq!(out.graph.items[0].markers[0].track, Some(TrackRef {
        kind: StreamKind::Audio,
        index: 1,
    }));
    let Value::Arr(native) = out
        .graph
        .sidecar("avid.source", "clip:clip-1")
        .unwrap()
        .get("marker_native")
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(native[0].get("graph_index"), Some(&Value::Null));
    assert_eq!(native[1].get("graph_index"), Some(&Value::Int(0)));
    assert!(out.graph.losses.iter().any(|l| l.aspect == "marker_timing"));
}

#[test]
fn original_effect_bytes_are_preserved_on_ordinary_sequences() {
    use avid_reader::color::{ColorCorrection, ParamValue};
    let (mut facts, index) = fixture(1);
    let mut e = event("event", 1, 1, 0, 24, None);
    e.event.color = Some(ColorCorrection {
        params: vec![("unknown-guid".into(), ParamValue::Bytes(vec![1, 128, 255]))],
        ..Default::default()
    });
    facts.sequences.push(sequence(vec![e]));
    let out = run(&facts, &index);
    let text = out.graph.to_json_text().unwrap();
    assert!(text.contains("unknown-guid"));
    assert!(text.contains("0180FF"));
}

#[test]
fn media_scan_deduplicates_roots_and_overlapping_files() {
    let path = std::env::temp_dir().join(format!(
        "anynle-avid-index-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let child = path.join("child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("bad.MXF"), b"not an MXF").unwrap();
    let index =
        index::MediaIndex::scan(&[path.clone(), path.clone(), child], &HashSet::new()).unwrap();
    assert_eq!(index.files_scanned, 1);
    assert_eq!(index.unreadable.len(), 1);
    assert!(index.identities.is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn unresolved_native_channel_prevents_false_two_channel_group() {
    let (mut facts, index) = fixture(2);
    let mut seq = sequence(vec![
        event("left", 1, 1, 0, 24, Some((0, 0))),
        event("right", 1, 2, 0, 24, Some((0, 2))),
    ]);
    seq.combiner_channels = vec![3];
    facts.sequences.push(seq);
    let out = run(&facts, &index);
    assert!(out.graph.sequences[0].placements.is_empty());
    assert!(out.graph.losses.iter().any(|l| l.aspect == "audio_routing"));
    let text = out.graph.to_json_text().unwrap();
    assert!(text.contains("\"combiner_channels\":[\"3\"]"));
    assert!(text.contains("left"));
    assert!(text.contains("right"));
}

#[test]
fn absent_expected_combiner_count_does_not_infer_completeness() {
    let (mut facts, index) = fixture(2);
    let mut seq = sequence(vec![
        event("left", 1, 1, 0, 24, Some((0, 0))),
        event("right", 1, 2, 0, 24, Some((0, 1))),
    ]);
    seq.combiner_channels.clear();
    facts.sequences.push(seq);
    let out = run(&facts, &index);
    assert!(out.graph.sequences[0].placements.is_empty());
}

#[test]
fn omitted_master_and_sequence_keep_distinct_receipts_on_one_bin() {
    let (mut facts, index) = fixture(1);
    facts.masters[0].master.tracks[0].essence = None;
    let mut seq = sequence(vec![event("native-event", 1, 1, 0, 24, None)]);
    seq.rate = None;
    facts.sequences.push(seq);
    let out = run(&facts, &index);
    assert!(out.graph.items.is_empty());
    assert!(out.graph.media.is_empty());
    assert_eq!(out.graph.sidecars.len(), 1);
    let bin = out.graph.sidecar("avid.source", "bin:0").unwrap();
    assert!(bin.get("unresolved_masters").is_some());
    assert!(bin.get("unresolved_sequences").is_some());
    assert!(out.graph.losses.iter().any(|l| l.aspect == "source_master"));
}

#[test]
fn omitted_media_marks_exact_spans_and_refuses_overlapping_edits() {
    let (mut facts, index) = fixture(1);
    let source_id = facts.masters[0].id.clone();
    let (mut retained, _) = fixture(1);
    let mut retained = retained.masters.pop().unwrap();
    retained.id = "revision/retained".into();
    retained.master.mob_id = mob(2);
    facts.masters.push(retained);
    let originals = original::OriginalIndex {
        omitted_media: HashMap::from([(source_id.clone(), "Test unsupported original".into())]),
        ..Default::default()
    };
    let mut kept = event("kept-edit", 2, 1, 0, 12, None);
    kept.event.master = mob(2);
    facts.sequences.push(sequence(vec![kept,
        event("missing-edit", 2, 1, 12, 24, None),
        event("other-lane", 3, 1, 12, 24, None),
    ]));
    let build = |facts: &av::SourceProject| build_selected(facts, &index, "test",
        &["bin".into()], &originals, &MotionMxf::default()).unwrap();
    let out = build(&facts);
    let seq = &out.graph.sequences[0];
    assert_eq!(seq.placements.len(), 1);
    assert_eq!(seq.placements[0].record_start, Rational::ZERO);
    assert_eq!(seq.placements[0].duration, Rational::new(1, 2).unwrap());
    assert_eq!(seq.markers.len(), 2);
    assert_eq!(seq.markers[0].name, "Media omitted · A2");
    assert_eq!(seq.markers[0].owner_occurrence, "missing-edit");
    assert_eq!(seq.markers[0].start, Rational::new(1, 2));
    assert_eq!(seq.markers[0].duration, Rational::integer(1));
    assert!(seq.tracks.iter().any(|t| t.kind == StreamKind::Audio && t.index == 2));
    let receipt = out.graph.sidecar("avid.source", "bin:0").unwrap();
    assert!(receipt.get("omitted_media_masters").is_some());
    assert_eq!(out.graph.losses.iter().filter(|l| l.aspect == "media_gap").count(), 2);
    assert!(!out.graph.losses.iter().any(|l| l.aspect == "source_master"));

    facts.sequences[0].events.push(event("overlap", 2, 1, 24, 24, None));
    let out = build(&facts);
    assert_eq!(out.graph.sequences[0].markers.len(), 1);
    assert_eq!(out.graph.losses.iter().filter(|l| l.aspect == UNQUALIFIED_ROUTE_TIMING).count(), 2);
}

#[test]
fn absent_pcm_prefix_siblings_can_be_omitted_but_present_unverified_files_refuse() {
    let (mut facts, present) = fixture(2);
    facts.masters[0].pcm_prefix = Some(av::SourcePcmPrefix { frames: 47, tail_package: mob(4) });
    facts.sequences.push(sequence(vec![event("missing-prefix", 1, 1, 0, 24, None)]));
    let originals = original::OriginalIndex { allow_media_gaps: true, ..Default::default() };
    let run = |index: &index::MediaIndex| build_selected(&facts, index, "test",
        &["bin".into()], &originals, &MotionMxf::default()).unwrap();
    let omitted = run(&Default::default());
    assert!(omitted.graph.losses.iter().any(|l| l.aspect == "media_bin_omission"));
    assert_eq!(omitted.graph.sequences[0].markers[0].duration, Rational::integer(1));
    let unverified = run(&present);
    assert!(unverified.graph.losses.iter().any(|l| l.aspect == "source_master"));
    assert!(unverified.graph.sequences[0].markers.is_empty());
}

#[test]
fn master_output_mix_preserves_types_absence_and_unresolved_receipts() {
    use avid_reader::extract::MasterOutputMix as M;
    for (input, kind, raw) in [
        (M::Absent,"absent",Value::Null),
        (M::Integer(4),"integer",Value::Int(4)),
        (M::Integer(5),"integer",Value::Int(5)),
        (M::Integer(-91),"integer",Value::Int(-91)),
        (M::Text("future".into()),"text",s("future")),
        (M::Bytes(vec![0,128,255]),"bytes_hex",s("0080ff")),
        (M::Reference(9),"bin_object_ref",Value::Int(9)),
        (M::Null,"null",Value::Null),
    ] {
        let (mut facts,index)=fixture(2);
        facts.masters[0].master.output_mix=input;
        let out=run(&facts,&index);
        let expected=obj(vec![("type",s(kind)),("raw",raw)]);
        let source=out.graph.sidecar("avid.source",&out.graph.items[0].id).unwrap();
        assert_eq!(source.get("master_output_mix"),Some(&expected));
        assert!(out.graph.losses.iter().all(|l|l.aspect!="master_output_mix"));
        // An unresolved master must not discard the control either.
        facts.masters[0].master.tracks[0].essence=None;
        let out=run(&facts,&index);
        let bin=out.graph.sidecar("avid.source","bin:0").unwrap();
        let Value::Arr(rows)=bin.get("unresolved_masters").unwrap() else {panic!()};
        assert_eq!(rows[0].get("master_output_mix"),Some(&expected));
    }
}

#[test]
fn master_output_mix_payload_is_budgeted_before_graph_copy() {
    let mut budget=Budget {used:MAX_GRAPH_CONSTRUCTION_BYTES-256, ..Default::default()};
    assert!(output_mix(&extract::MasterOutputMix::Bytes(vec![7;1024]),&mut budget).is_err());
    assert!(output_mix(&extract::MasterOutputMix::Text("x".repeat(1024)),&mut budget).is_err());
}

#[test]
fn named_aac_and_smpte_ds_source_groups_preserve_declared_order() {
    for (count,raw,channels) in [(6,"51:A2A1A3A4A5A6",vec![2,1,3,4,5,6]),(8,"71:A1A3A2A5A6A7A8A4",vec![1,3,2,5,6,7,8,4])] {
        let (mut facts,index)=fixture(count);
        facts.masters[0].master.other_groups=vec![raw.into()];
        let out=run(&facts,&index);
        assert_eq!(out.graph.items[0].stream_groups,vec![channels]);
        assert_eq!(out.graph.sidecar("avid.source",&out.graph.items[0].id).unwrap().get("raw_groups"),Some(&Value::Arr(vec![s(raw)])));
        assert!(out.audio_group_issues.is_empty());
    }
}

#[test]
fn explicit_root_mappings_keep_same_named_originals_distinct() {
    let root = std::env::temp_dir().join(format!("anynle-original-collision-{}", std::process::id()));
    let a = root.join("a");
    let b = root.join("b");
    for dir in [&a, &b] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("source.wav"), dir.as_os_str().as_encoded_bytes()).unwrap();
    }
    let (mut facts, _) = fixture(1);
    let (mut second, _) = fixture(1);
    let mut second = second.masters.remove(0);
    second.id = "revision/second".into();
    second.master.mob_id = mob(2);
    facts.masters.push(second);
    for (master, root) in facts.masters.iter_mut().zip(["/saved/a", "/saved/b"]) {
        master.originals = vec![Some(av::OriginalTrack {
            locators: vec![format!("{root}/source.wav")],
            descriptor_class: "PCMA".into(),
            descriptor_rate: 48000.0,
            descriptor_length: 96000,
            channels: Some(1),
            sample_rate: Some(48000.0),
            stored_quantization_bits: Some(24),
            display_raster: None,
            file_track: Some(1),
            motion_audio_track_start: None,
        })];
    }
    let metadata = graph::json::parse(r#"{
        "format":{"format_name":"wav"},
        "streams":[{"index":0,"codec_type":"audio","codec_name":"pcm_s24le",
          "sample_rate":"48000","channels":1,"time_base":"1/48000","duration_ts":96000}]
    }"#).unwrap();
    let mut options = original::OriginalOptions {
        root_mappings: vec![
            original::RootMapping { saved_root: "/saved/a".into(), current_root: a.clone() },
            original::RootMapping { saved_root: "/saved/b".into(), current_root: b.clone() },
        ],
        ..Default::default()
    };
    let scan = |facts: &av::SourceProject, options: &original::OriginalOptions| {
        original::OriginalIndex::scan_with(facts, options, |_| Ok(metadata.clone()))
    };
    let distinct = scan(&facts, &options).unwrap();
    assert_eq!(distinct.original_inputs.len(), 2);
    assert_eq!(distinct.tracks[&("revision/master".into(), 0)].path, a.join("source.wav").canonicalize().unwrap());
    assert_eq!(distinct.tracks[&("revision/second".into(), 0)].path, b.join("source.wav").canonicalize().unwrap());

    let mut repeated = fixture(1).0.masters.remove(0);
    repeated.id = "revision/repeated".into();
    repeated.master.mob_id = mob(3);
    repeated.originals = facts.masters[0].originals.clone();
    facts.masters.push(repeated);
    let first = a.join("source.wav").canonicalize().unwrap();
    let second = b.join("source.wav").canonicalize().unwrap();
    let mut visited = Vec::new();
    let interleaved = original::OriginalIndex::scan_with(&facts, &options, |path| {
        visited.push(path.to_owned());
        Ok(metadata.clone())
    }).unwrap();
    assert_eq!(visited, [first.clone(), second.clone()]);
    assert_eq!(interleaved.original_inputs.len(), 2);
    assert_eq!(interleaved.tracks[&("revision/repeated".into(), 0)].path, first);

    let changed = original::OriginalIndex::scan_with(&facts, &options, |path| {
        if path == second {
            std::fs::write(&first, b"changed during the other file's probe").unwrap();
        }
        Ok(metadata.clone())
    }).err().expect("an interleaved cache hit must not hide a file change");
    assert!(changed.contains("changed before source read completed"));
    std::fs::write(&first, a.as_os_str().as_encoded_bytes()).unwrap();
    facts.masters.pop();

    options.root_mappings[1].current_root = a.clone();
    let error = scan(&facts, &options).err().expect("distinct paths must not collapse");
    assert!(error.contains("/saved/b/source.wav"));
    assert!(error.contains("root mappings resolve distinct saved originals"));

    #[cfg(unix)]
    {
        // Equal bytes on separate files are valid. A hard link across two
        // mapped folders is the same file and cannot stand for both originals.
        let second_file = b.join("source.wav");
        let first_file = a.join("source.wav");
        options.root_mappings[1].current_root = b;
        std::fs::copy(&first_file, &second_file).unwrap();
        assert_eq!(scan(&facts, &options).unwrap().original_inputs.len(), 2);
        std::fs::remove_file(&second_file).unwrap();
        std::fs::hard_link(&first_file, &second_file).unwrap();
        let error = scan(&facts, &options).err().expect("hard links must not collapse originals");
        assert!(error.contains("/saved/b/source.wav"));
        assert!(error.contains("root mappings resolve distinct saved originals"));
    }

    // Separate bin items and even different mob IDs can legitimately reuse one locator.
    facts.masters[1].originals = facts.masters[0].originals.clone();
    let reused = scan(&facts, &options).unwrap();
    assert_eq!(reused.original_inputs.len(), 1);
    assert_eq!(reused.tracks.len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn controlled_original_pcm_and_compressed_clocks_use_physical_streams_without_mxf_ids(){
    let dir=std::env::temp_dir().join(format!("anynle-original-stream-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();
    for (codec,bits,representation) in [("pcm_s16le",16,"pcm16"),("pcm_s24le",24,"pcm24"),("aac",16,"aac")] {
        let compressed=codec=="aac";let path=dir.join(if compressed{"source.mp4"}else{"source.wav"});std::fs::write(&path,b"synthetic probe witness").unwrap();
        let (mut facts,_)=fixture(2);let m=&mut facts.masters[0];
        m.originals=(1..=2).map(|channel|Some(av::OriginalTrack{locators:vec![path.to_str().unwrap().into()],descriptor_class:if compressed{"MPGA"}else{"PCMA"}.into(),descriptor_rate:if compressed{46.875}else{48000.0},descriptor_length:96000,channels:Some(2),sample_rate:Some(48000.0),stored_quantization_bits:Some(bits),display_raster:None,file_track:Some(channel),motion_audio_track_start:None})).collect();
        for t in &mut m.master.tracks {t.edit_rate=if compressed{46.875}else{48000.0};t.essence=if compressed{Some(Essence::Other("MPGA".into()))}else{Some(Essence::Sound{sample_rate:48000.,bits,channels:2})};}
        let mut streams=vec![];
        if compressed{streams.push(graph::json::parse(r#"{"index":0,"codec_type":"video","codec_name":"h264","codec_tag_string":"avc1","profile":"High","level":40,"pix_fmt":"yuv420p","width":1920,"height":1080,"avg_frame_rate":"24/1","r_frame_rate":"24/1","field_order":"progressive","sample_aspect_ratio":"1:1"}"#).unwrap());}
        let mut audio=vec![("index".into(),Value::Int(if compressed{1}else{0})),("codec_type".into(),Value::Str("audio".into())),("codec_name".into(),Value::Str(codec.into())),("sample_rate".into(),Value::Str("48000".into())),("channels".into(),Value::Int(2)),("time_base".into(),Value::Str("1/48000".into())),("duration_ts".into(),Value::Int(96000))];if compressed{audio.extend([("start_pts".into(),Value::Int(0)),("profile".into(),s("LC")),("codec_tag_string".into(),s("mp4a")),("channel_layout".into(),s("stereo"))]);}streams.push(Value::Obj(audio));
        let probe=Value::Obj(vec![("streams".into(),Value::Arr(streams)),("format".into(),Value::Obj(vec![("format_name".into(),Value::Str(if compressed{"mov,mp4,m4a,3gp,3g2,mj2"}else{"wav"}.into()))]))]);
        let mut calls=0;let original=original::OriginalIndex::scan_with(&facts,&Default::default(),|_|{calls+=1;Ok(probe.clone())}).unwrap();assert_eq!(calls,1);
        assert_eq!(original.original_inputs.len(),1);assert_eq!(original.original_inputs[0].revision.len(),64);
        let out=build_selected(&facts,&Default::default(),"Synthetic",&["Sources".into()],&original,&MotionMxf::default()).unwrap();assert_eq!(out.graph.media.len(),2);assert_eq!(out.original_inputs,original.original_inputs);
        if compressed{assert_eq!(original.notes.len(),1);assert!(out.diagnostics.iter().any(|s|s.contains("default")&&s.contains("not observed source color")));}else{assert!(original.notes.is_empty());}
        for (channel,m) in out.graph.media.iter().enumerate(){assert_eq!(m.duration,Rational::integer(2));assert_eq!(m.sample_rate,Some(48000));assert_eq!(m.bits,if compressed{None}else{Some(bits)});let o=m.original.as_ref().unwrap();assert_eq!(o.channel_index,Some(channel as i64));assert_eq!(o.sample_representation.as_deref(),Some(representation));}
        let mut bad=probe.clone();if let Value::Obj(root)=&mut bad{if let Some((_,Value::Arr(rows)))=root.iter_mut().find(|(k,_)|k=="streams"){if let Value::Obj(row)=rows.last_mut().unwrap(){row.iter_mut().find(|(k,_)|k=="channels").unwrap().1=Value::Int(3);}}}
        assert!(original::OriginalIndex::scan_with(&facts,&Default::default(),|_|Ok(bad.clone())).is_err());
        if compressed {
            // The injected full-file probe is checked even though this master selects
            // audio lanes only; a matching audio codec cannot bypass the picture gate.
            for (stream_index,key,value) in [(0,"profile",s("High 10")),(0,"color_transfer",s("smpte2084")),(0,"tags",graph::json::parse(r#"{"rotate":"90"}"#).unwrap()),(1,"profile",s("HE-AAC")),(1,"codec_tag_string",s("unknown"))] {
                let mut bad=probe.clone();let Value::Obj(root)=&mut bad else{panic!()};let Value::Arr(rows)=&mut root.iter_mut().find(|(k,_)|k=="streams").unwrap().1 else{panic!()};let Value::Obj(row)=&mut rows[stream_index] else{panic!()};if let Some((_,old))=row.iter_mut().find(|(k,_)|k==key){*old=value;}else{row.push((key.into(),value));}
                assert!(original::OriginalIndex::scan_with(&facts,&Default::default(),|_|Ok(bad.clone())).is_err(),"profile bypass for {key}");
            }
            let mut tagged=probe.clone();let Value::Obj(root)=&mut tagged else{panic!()};let Value::Arr(rows)=&mut root.iter_mut().find(|(k,_)|k=="streams").unwrap().1 else{panic!()};let Value::Obj(row)=&mut rows[0] else{panic!()};
            row.extend([("color_space".into(),s("bt709")),("color_transfer".into(),s("bt709")),("color_primaries".into(),s("bt709")),("color_range".into(),s("tv"))]);
            let tagged=original::OriginalIndex::scan_with(&facts,&Default::default(),|_|Ok(tagged.clone())).unwrap();assert!(tagged.notes.is_empty());
        }

        // Physical-family mismatch must refuse even when the stream supplies zero
        // PTS, rather than only when the WAV origin fallback is needed.
        for zero_pts in [false,true] {
            let mut candidate=probe.clone();let Value::Obj(root)=&mut candidate else{panic!()};let Value::Arr(streams)=&mut root.iter_mut().find(|(k,_)|k=="streams").unwrap().1 else{panic!()};let Value::Obj(audio)=streams.last_mut().unwrap() else{panic!()};audio.retain(|(k,_)|k!="start_pts");if zero_pts{audio.push(("start_pts".into(),Value::Int(0)));}
            if !compressed||zero_pts{assert!(original::OriginalIndex::scan_with(&facts,&Default::default(),|_|Ok(candidate.clone())).is_ok());}
            for family in [Some(if compressed{"wav"}else{"mov,mp4,m4a,3gp,3g2,mj2"}),Some("matroska,webm"),Some("unknown"),None] {
                let mut bad=candidate.clone();let Value::Obj(root)=&mut bad else{panic!()};root.retain(|(k,_)|k!="format");if let Some(family)=family{root.push(("format".into(),Value::Obj(vec![("format_name".into(),s(family))])));}
                let error=original::OriginalIndex::scan_with(&facts,&Default::default(),|_|Ok(bad.clone())).err().expect("mislabeled physical family was admitted");assert!(error.contains("physical container"),"wrong refusal: {error}");
            }
        }

        let before=std::fs::metadata(&path).unwrap().modified().unwrap();
        let changed=original::OriginalIndex::scan_with(&facts,&Default::default(),|p|{
            std::fs::write(p,b"synthetic Probe witness").unwrap();
            std::fs::OpenOptions::new().write(true).open(p).unwrap().set_times(std::fs::FileTimes::new().set_modified(before)).unwrap();
            Ok(probe.clone())
        });
        assert!(changed.err().unwrap().contains("changed during stream probe"));
        std::fs::write(&path,b"synthetic probe witness").unwrap();
        if !compressed {
            let other=dir.join("other.wav");std::fs::write(&other,b"synthetic probe witness").unwrap();
            let mut second=fixture(2).0.masters.remove(0);second.id="revision/second".into();second.originals=facts.masters[0].originals.clone();
            for raw in &mut second.originals{raw.as_mut().unwrap().locators=vec![other.to_str().unwrap().into()];}facts.masters.push(second);
            let other_canonical=std::fs::canonicalize(&other).unwrap();
            let changed=original::OriginalIndex::scan_with(&facts,&Default::default(),|p|{if p==other_canonical{std::fs::write(&path,b"synthetic Probe witness").unwrap();}Ok(probe.clone())});
            assert!(changed.err().unwrap().contains("changed before source read completed"));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn source_group(id:&str)->av::SourceGroup{
    let r=Rate::new(24,1).unwrap();
    let alternatives=(0..2).map(|i|av::SourceGroupAlternative{
        endpoint:av::GroupEndpoint{class:*b"SCLP",kind:Some(KIND_SOUND),rate:Some(r),mob:Some(mob(10+i)),track:Some(1),start:Some(0),length:Some(48)},
        pieces:if i==0{vec![av::SourceGroupPiece::Gap{start:0,length:24},av::SourceGroupPiece::Media{start:24,length:24,master:mob(1),track:1,source_in:0,rate:r}]}else{vec![av::SourceGroupPiece::Media{start:0,length:48,master:mob(1),track:1,source_in:0,rate:r}]},reason:None,
    }).collect();
    av::SourceGroup{id:id.into(),bin:0,mob:Some(mob(20)),name:"Group A".into(),rate:Some(r),stored_rate:Some(24.0),length:Some(48),lanes:vec![av::SourceGroupLane{is_ganged:Some(true),kind:Some(KIND_SOUND),label:Some(1),rate:Some(r),length:Some(48),selected:Some(0),alternatives,reason:None}],marks:vec![],markers:vec![],reason:None}
}
fn group_use(g:&av::SourceGroup,selected:i64,bindings:&[&str])->av::SourceGroupUse{
    av::SourceGroupUse{is_ganged:Some(true),warp_picture_input:false,id:"group-use".into(),kind:KIND_SOUND,track:1,position:Some(0),length:Some(12),rate:g.rate,selected:Some(selected),alternatives:g.lanes[0].alternatives.iter().map(|a|av::GroupEndpoint{length:Some(12),..a.endpoint}).collect(),binding:av::Binding{placements:bindings.iter().map(|s|s.to_string()).collect(),reason:None},reason:None}
}
#[test]
fn group_layouts_keep_gaps_and_bind_exact_occurrences_instead_of_first_timestamp(){
    let (mut f,i)=fixture(1);let g=source_group("group");let u=group_use(&g,1,&["second"]);
    let mut seq=sequence(vec![event("first",1,1,0,12,None),event("second",2,1,0,12,None)]);seq.group_uses.push(u);f.groups.push(g);f.sequences.push(seq);
    let out=run(&f,&i);assert_eq!(out.graph.multicams.len(),1);assert!(out.graph.bins[0].items.iter().any(|s|s=="group"));
    assert!(out.graph.multicams[0].lanes[0].alternatives[0].pieces[0].item.is_none());
    let u=&out.graph.multicam_uses[0];assert_eq!(u.candidates,vec!["group"]);assert_eq!(u.selected,Some(1));assert_eq!(u.binding.placements,vec!["second"]);assert_eq!(u.source_in,Some(Rational::ZERO));assert!(u.reason.is_none());
    assert_eq!(SourceGraph::from_json_text(&out.graph.to_json_text().unwrap()).unwrap(),out.graph);
}
#[test]
fn selected_group_gap_ambiguity_missing_member_and_bad_clock_are_explicit(){
    let (mut f,i)=fixture(1);let g=source_group("group");let u=group_use(&g,0,&[]);let mut seq=sequence(vec![]);seq.group_uses.push(u);f.groups.push(g);f.sequences.push(seq);
    let out=run(&f,&i);assert!(out.graph.multicam_uses[0].binding.placements.is_empty());assert!(out.graph.multicam_uses[0].reason.is_none());
    f.groups.push(source_group("same-endpoints"));let out=run(&f,&i);assert_eq!(out.graph.multicam_uses[0].candidates.len(),2);assert!(out.graph.multicam_uses[0].source_in.is_none());assert!(out.graph.multicam_uses[0].reason.is_some());
    f.groups.pop();if let av::SourceGroupPiece::Media{master,..}=&mut f.groups[0].lanes[0].alternatives[1].pieces[0]{*master=mob(99);}
    let out=run(&f,&i);assert!(out.graph.multicams[0].lanes[0].alternatives[1].reason.as_ref().unwrap().contains("missing"));
    f.groups[0].rate=Some(Rate{num:0,den:1});assert!(build(&f,&i,"bad",&["bin".into()]).err().unwrap().contains("rate"));
}
#[test]
fn unknown_sequence_clock_retains_group_uses_on_bin_receipt(){
    let (mut f,i)=fixture(1);let g=source_group("group");let u=group_use(&g,0,&[]);let mut seq=sequence(vec![]);seq.rate=None;seq.group_uses.push(u);f.groups.push(g);f.sequences.push(seq);
    let out=run(&f,&i);assert!(out.graph.sequences.is_empty());assert!(out.graph.multicam_uses.is_empty());
    let text=out.graph.to_json_text().unwrap();assert!(text.contains("native_group_uses"));assert!(text.contains("group-use"));
}

#[test]
fn unresolved_group_owner_or_lane_keeps_empty_alternatives_as_explicit_unknowns(){
    for owner_unknown in [false,true]{
        let (mut f,i)=fixture(1);let mut g=source_group("group");
        for a in &mut g.lanes[0].alternatives{a.pieces.clear();}
        if owner_unknown{g.rate=None;g.stored_rate=Some(f64::NAN);g.reason=Some("native group clock is unknown".into());}
        else{g.lanes[0].selected=Some(-1);g.lanes[0].reason=Some("native selected index is unresolved".into());}
        f.groups.push(g);let out=run(&f,&i);
        assert!(out.graph.multicams[0].lanes[0].alternatives.iter().all(|a|a.pieces.is_empty()&&a.reason.is_some()));
        let text=out.graph.to_json_text().unwrap();assert_eq!(SourceGraph::from_json_text(&text).unwrap(),out.graph);
        if !owner_unknown{let native=out.graph.sidecars.iter().find(|s|s.adapter=="avid.group").unwrap();assert!(format!("{:?}",native.data).contains("Int(-1)"));}
    }
}
#[test]
fn identical_endpoint_lanes_keep_lane_ambiguity_without_duplicate_group_references(){
    let (mut f,i)=fixture(1);let mut g=source_group("group");let u=group_use(&g,0,&[]);
    let mut second=source_group("temporary").lanes.remove(0);second.label=Some(2);g.lanes.push(second);
    let mut seq=sequence(vec![]);seq.group_uses.push(u);f.groups.push(g);f.sequences.push(seq);
    let out=run(&f,&i);let u=&out.graph.multicam_uses[0];assert_eq!(u.candidates,vec!["group"]);assert!(u.lane.is_none());assert!(u.reason.is_some());
}
