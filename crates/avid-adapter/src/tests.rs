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
    };
    (
        av::SourceProject {
            groups: vec![],
            masters: vec![source],
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
        picture_extension: None,
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
fn run(f: &av::SourceProject, i: &index::MediaIndex) -> ReadResult {
    build(f, i, "project", &["bin".into()]).unwrap()
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
        params: ColorCorrection { params: vec![
            ("2581568d7e84d511935a50f857c10000".into(), ParamValue::Float(3000.0)),
        ] },
    });
    e.picture_extension = Some(av::PictureExtension::Resize([
        av::ResizeKey { frame: 0, position: [-0.15, 0.075], scale: [80.0, 120.0] },
        av::ResizeKey { frame: 47, position: [0.2, -0.125], scale: [135.0, 65.0] },
    ]));
    f.sequences.push(sequence(vec![e]));
    let out = run(&f, &i);
    let p = &out.graph.sequences[0].placements[0];
    assert_eq!(p.transform.as_ref().unwrap().position, (-0.15, 0.075));
    assert_eq!(p.transform.as_ref().unwrap().scale, (80.0, 120.0));
    assert_eq!(p.effects[0].kind, "avid.resize_linear");
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
    f.accounted_bytes = av::MAX_SOURCE_FACT_BYTES + 1;
    assert!(build(&f, &i, "p", &["b".into()]).is_err());
    let mut b = Budget::default();
    assert!(b.charge(av::MAX_SOURCE_FACT_BYTES).is_ok());
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
        params: ColorCorrection {
            params: vec![(
                "unknown-control".into(),
                ParamValue::Bytes(vec![0, 1, 254, 255]),
            )],
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
    let mut budget=Budget {used:av::MAX_SOURCE_FACT_BYTES-256};
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
fn controlled_original_pcm_and_compressed_clocks_use_physical_streams_without_mxf_ids(){
    let dir=std::env::temp_dir().join(format!("anynle-original-stream-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();
    for (codec,bits,representation) in [("pcm_s16le",16,"pcm16"),("pcm_s24le",24,"pcm24"),("aac",16,"aac")] {
        let compressed=codec=="aac";let path=dir.join(if compressed{"source.mp4"}else{"source.wav"});std::fs::write(&path,b"synthetic probe witness").unwrap();
        let (mut facts,_)=fixture(2);let m=&mut facts.masters[0];
        m.originals=(1..=2).map(|channel|Some(av::OriginalTrack{locators:vec![path.to_str().unwrap().into()],descriptor_class:if compressed{"MPGA"}else{"PCMA"}.into(),descriptor_rate:if compressed{46.875}else{48000.0},descriptor_length:96000,channels:Some(2),sample_rate:Some(48000.0),stored_quantization_bits:Some(bits),file_track:Some(channel)})).collect();
        for t in &mut m.master.tracks {t.edit_rate=if compressed{46.875}else{48000.0};t.essence=if compressed{Some(Essence::Other("MPGA".into()))}else{Some(Essence::Sound{sample_rate:48000.,bits,channels:2})};}
        let mut streams=vec![];
        if compressed{streams.push(graph::json::parse(r#"{"index":0,"codec_type":"video","codec_name":"h264","codec_tag_string":"avc1","profile":"High","level":40,"pix_fmt":"yuv420p","width":1920,"height":1080,"avg_frame_rate":"24/1","r_frame_rate":"24/1","field_order":"progressive","sample_aspect_ratio":"1:1"}"#).unwrap());}
        let mut audio=vec![("index".into(),Value::Int(if compressed{1}else{0})),("codec_type".into(),Value::Str("audio".into())),("codec_name".into(),Value::Str(codec.into())),("sample_rate".into(),Value::Str("48000".into())),("channels".into(),Value::Int(2)),("time_base".into(),Value::Str("1/48000".into())),("duration_ts".into(),Value::Int(96000))];if compressed{audio.extend([("start_pts".into(),Value::Int(0)),("profile".into(),s("LC")),("codec_tag_string".into(),s("mp4a")),("channel_layout".into(),s("stereo"))]);}streams.push(Value::Obj(audio));
        let probe=Value::Obj(vec![("streams".into(),Value::Arr(streams)),("format".into(),Value::Obj(vec![("format_name".into(),Value::Str(if compressed{"mov,mp4,m4a,3gp,3g2,mj2"}else{"wav"}.into()))]))]);
        let mut calls=0;let original=original::OriginalIndex::scan_with(&facts,&Default::default(),|_|{calls+=1;Ok(probe.clone())}).unwrap();assert_eq!(calls,1);
        assert_eq!(original.original_inputs.len(),1);assert_eq!(original.original_inputs[0].revision.len(),64);
        let out=build_selected(&facts,&Default::default(),"Synthetic",&["Sources".into()],&original).unwrap();assert_eq!(out.graph.media.len(),2);assert_eq!(out.original_inputs,original.original_inputs);
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
    av::SourceGroupUse{is_ganged:Some(true),id:"group-use".into(),kind:KIND_SOUND,track:1,position:Some(0),length:Some(12),rate:g.rate,selected:Some(selected),alternatives:g.lanes[0].alternatives.iter().map(|a|av::GroupEndpoint{length:Some(12),..a.endpoint}).collect(),binding:av::Binding{placements:bindings.iter().map(|s|s.to_string()).collect(),reason:None},reason:None}
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
