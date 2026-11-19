//! Synthetic-only original-file semantics and independent Python construction parity.
use prproj::convert::{model::*, prepared::*, writer::{
    MarkerProvenance, prepare_typed_project, prepare_typed_project_for_premiere_27,
    prepare_typed_project_with_marker_provenance, TypedMedia,
}};
use prproj::SeededUuids;
use std::{io::Read, path::PathBuf, process::Command};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "anynle-original-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn original(shape: &str) -> OriginalClip {
    let wav = shape.starts_with("wav");
    let prores = shape == "prores_hq";
    let mono = shape.starts_with("wav") && shape.ends_with("mono");
    let n = if prores { 0 } else if mono { 1 } else { 2 };
    let channels = (0..n)
        .map(|i| OriginalChannel {
            avid_track: i + 1,
            channel_index: if shape == "reverse" { 1 - i } else { i },
            gain_db: 0.0,
        })
        .collect();
    OriginalClip {
        separate_mono_streams: false,
        proxy_path: None,
        metadata: ClipMetadata {
            id: "original".into(),
            name: "Synthetic original".into(),
            material_umid: None,
            tape_name: None,
            avid: None,
            time_display: None,
            rate: Some(Fraction::new(24, 1)),
            markers: vec![],
        },
        path: std::env::temp_dir()
            .join(if wav {
                "anynle-synthetic-source.wav"
            } else if prores {
                "anynle-synthetic-source.mov"
            } else {
                "anynle-synthetic-source.mp4"
            })
            .to_string_lossy()
            .into_owned(),
        container: if wav { "wav" } else { "mov" }.into(),
        video: (!wav).then_some(OriginalVideo {
            codec: if prores { "apch" } else { "avc1" }.into(),
            width: 1920,
            height: 1080,
            rate: Fraction::new(24, 1),
            duration: 2 * TICKS_PER_SECOND,
            stream_index: 0,
            source_start: 0,
        }),
        audio: (!prores).then_some(OriginalAudio {
            source_start: 0,
            sample_rate: 48000,
            channels: n,
            duration: 2 * TICKS_PER_SECOND,
            stream_index: if wav { 0 } else { 1 },
            representation: if shape.starts_with("wav16") {
                SampleRepresentation::Pcm16
            } else if wav {
                SampleRepresentation::Pcm24
            } else {
                SampleRepresentation::Aac
            },
        }),
        channels,
        groups: if shape == "dualmono" {
            vec![vec![1], vec![2]]
        } else if prores {
            vec![]
        } else {
            vec![(1..=n).collect()]
        },
    }
}
fn input(c: OriginalClip) -> PreparedProjectInput {
    PreparedProjectInput {
        autosync_bins: Default::default(),
        texts: Vec::new(),
        multicams: Vec::new(),
        multicam_uses: Vec::new(),
        picture_subclips: Vec::new(),
        av_subclips: Vec::new(),
        av_subclip_uses: Vec::new(),
        constant_speeds: Vec::new(),
        frame_holds: Default::default(),
        picture_nests: Vec::new(),
        layered_submasters: Vec::new(),
        png_mattes: Default::default(),
        video_dissolves: Vec::new(),
        audio_edge_fades: Vec::new(),
        audio_crossfades: Vec::new(),
        audio_pans: Vec::new(),
        audio_mixers: Default::default(),
        track_states: Default::default(),
        timeline_marks: Default::default(),
        non_drop_sequences: Default::default(),
        picture_effects: Vec::new(),
        source_luts: Vec::new(),
        extended_sdr_grades: Default::default(),
        edit_labels: Vec::new(),
        disabled_edits: Default::default(),
        direct_dnxhr_origins: Default::default(),
        direct_dnxhd_pcm24: Default::default(),
        project_name: "Synthetic".into(),
        options: Options::default(),
        clips: vec![PreparedClip::Original(c)],
        bins: vec![Bin {
            name: "Sources".into(),
            clips: vec!["original".into()],
        }],
        sequences: vec![],
    }
}
fn xml(p: &PathBuf) -> String {
    let b = std::fs::read(p).unwrap();
    let mut s = String::new();
    flate2::read::GzDecoder::new(b.as_slice())
        .read_to_string(&mut s)
        .unwrap();
    s
}

#[test]
fn premiere_27_writers_change_only_the_seeded_project_version() {
    let scratch = Scratch::new();
    let path = scratch.0.join("project.prproj");
    let project = input(original("prores_hq"));
    let seed = || SeededUuids::new("project-version");
    prepare_typed_project(&project, &path, &mut seed(), &[], false)
        .unwrap().publish().unwrap();
    let legacy = xml(&path);
    std::fs::remove_file(&path).unwrap();

    prepare_typed_project_for_premiere_27(
        &project, &path, &mut seed(), TypedMedia::Bound { bindings: &[], native_av: false }, None,
    ).unwrap().publish().unwrap();
    let current = xml(&path);
    let version_45 = "<Project ObjectID=\"1\" ClassID=\"62ad66dd-0dcd-42da-a660-6d8fbde94876\" Version=\"45\">";
    let version_46 = version_45.replace("Version=\"45\"", "Version=\"46\"");
    assert!(legacy.contains(version_45));
    assert_eq!(current, legacy.replacen(version_45, &version_46, 1));
    std::fs::remove_file(&path).unwrap();

    let provenance = MarkerProvenance { custom_data: &Default::default(), original_colors: &Default::default() };
    prepare_typed_project_with_marker_provenance(&project, &path, &mut seed(), provenance)
        .unwrap().publish().unwrap();
    assert_eq!(xml(&path), current);
}

#[test]
fn linked_dual_mono_mov_writes_two_physical_audio_media_nodes() {
    let mut clip = original("dualmono");
    clip.separate_mono_streams = true;
    clip.path = clip.path.replace(".mp4", ".mov");
    clip.metadata.rate = Some(Fraction::new(25, 1));
    clip.video.as_mut().unwrap().rate = Fraction::new(25, 1);
    clip.audio.as_mut().unwrap().representation = SampleRepresentation::Pcm24;
    let mut project = input(clip);
    let scratch = Scratch::new();
    let path = scratch.0.join("dual-mono.prproj");
    prepare_typed_project(&project, &path, &mut SeededUuids::new("dual-mono"), &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&path).unwrap();
    let nodes = doc.tree.findall(doc.root, "Media").unwrap();
    assert_eq!(nodes.len(), 2);
    for (i, node) in nodes.iter().enumerate() {
        assert_eq!(doc.tree.findtext(*node, "StreamNumber").unwrap().as_deref(), [None, Some("1")][i]);
        let stream = doc.resolve(doc.tree.find1(*node, "AudioStream").unwrap()).unwrap();
        assert_eq!(doc.tree.findtext(stream, "AudioChannelLayout").unwrap().as_deref(),
            Some(r#"[{"channellabel":0}]"#));
    }
    assert_eq!(doc.tree.findtext(nodes[0], "FileKey").unwrap(),
        doc.tree.findtext(nodes[1], "FileKey").unwrap());
    let PreparedClip::Original(clip) = &mut project.clips[0] else { panic!("expected original clip"); };
    clip.groups = vec![vec![1, 2]];
    let invalid = scratch.0.join("invalid.prproj");
    assert!(prepare_typed_project(&project, &invalid, &mut SeededUuids::new("invalid"), &[], false).is_err());
    assert!(!invalid.exists());
}

#[test]
fn linked_avc_pcm_mp4_accepts_nonstandard_rasters_and_checks_the_pcm_extent() {
    for depth in [SampleRepresentation::Pcm16, SampleRepresentation::Pcm24] {
        let mut clip = original("movie");
        let video = clip.video.as_mut().unwrap();
        video.width = 3840;
        video.height = 2160;
        video.rate = Fraction::new(24_000, 1001);
        video.duration = 48 * ticks_per_unit(video.rate);
        let audio = clip.audio.as_mut().unwrap();
        audio.duration = video.duration;
        audio.representation = depth;
        validate_original(&clip).unwrap();

        let mut wide = clip.clone();
        let video = wide.video.as_mut().unwrap();
        video.width = 1600;
        video.height = 900;
        validate_original(&wide).unwrap();
        wide.video.as_mut().unwrap().width = 0;
        assert!(validate_original(&wide).is_err());
        clip.audio.as_mut().unwrap().duration -= ticks_per_unit(Fraction::new(48_000, 1));
        assert!(validate_original(&clip).is_err());
    }
}

#[test]
fn linked_picture_media_carries_its_saved_reel_and_clock() {
    let scratch = Scratch::new();
    let out = scratch.0.join("camera.prproj");
    let mut clip = original("prores_hq");
    clip.metadata.tape_name = Some("REEL_A_101".into());
    let start = 36_001 * TICKS_PER_SECOND;
    clip.video.as_mut().unwrap().source_start = start;
    prepare_typed_project(
        &input(clip),
        &out,
        &mut SeededUuids::new("camera reel"),
        &[],
        false,
    )
    .unwrap()
    .publish()
    .unwrap();

    let saved = xml(&out);
    let (_, media_and_after) = saved.split_once("<Media ObjectUID=").unwrap();
    let (media, _) = media_and_after.split_once("</Media>").unwrap();
    assert!(media.contains("<TapeName>REEL_A_101</TapeName>"));
    assert!(media.contains("<AlternateTapeName>REEL_A_101</AlternateTapeName>"));
    assert!(media.contains(&format!("<AlternateStart>{start}</AlternateStart>")));
    let (_, log_and_after) = saved.split_once("<ClipLoggingInfo ").unwrap();
    let (log, _) = log_and_after.split_once("</ClipLoggingInfo>").unwrap();
    assert!(log.contains("<TapeName>REEL_A_101</TapeName>"));
    assert!(log.contains(&format!("<MediaInPoint>{start}</MediaInPoint>")));
}

#[test]
fn extended_sdr_grades_share_the_table_and_refuse_a_combined_grade() {
    let scratch = Scratch::new();
    let out = scratch.0.join("grades.prproj");
    let invalid = scratch.0.join("invalid.prproj");
    let mut project = input(original("prores_hq"));
    let mut gain = event();
    gain.audio_tracks.clear();
    gain.stereo.clear();
    gain.color_correction = Some(ColorCorrection {
        tone: vec![("gain".into(), 150.0)],
        ..Default::default()
    });
    let mut repeated = gain.clone();
    repeated.record_start = gain.record_start + gain.duration;
    project.sequences.push(sequence(vec![gain, repeated]));
    project.extended_sdr_grades.extend([(0, 0), (0, 1)]);
    let mut ids = SeededUuids::new("extended-sdr-grade");
    prepare_typed_project(&project, &out, &mut ids, &[], false)
        .unwrap().publish().unwrap();

    let mut doc = prproj::xml::parse_prproj(&out).unwrap();
    let tables: Vec<_> = doc.tree.iter_tag(doc.root, "ArbVideoComponentParam")
        .filter(|&node| doc.tree.findtext(node, "Name").unwrap().as_deref() == Some("LUT Blob"))
        .map(|node| doc.tree.find1(node, "StartKeyframeValue").unwrap())
        .collect();
    assert_eq!(tables.len(), 2);
    assert_eq!(tables.iter().filter(|&&node| !doc.tree[node].text_or_empty().trim().is_empty()).count(), 1);
    let a = doc.payload(tables[0]).unwrap().unwrap();
    let b = doc.payload(tables[1]).unwrap().unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 40 + 65_usize.pow(3) * 12);
    assert_eq!(&a[..4], b"ITX3");
    assert_eq!(&a[16..20], &(-1.125_f32).to_le_bytes());
    assert_eq!(&a[28..32], &2.125_f32.to_le_bytes());
    let assets = scratch.0.join("grades LUTs");
    assert_eq!(std::fs::read_dir(&assets).unwrap().count(), 1);
    assert!(std::fs::read_to_string(assets.join("Avid Gain 150 Rec709 SDR.cube")).unwrap()
        .contains("DOMAIN_MIN -1.125 -1.125 -1.125\nDOMAIN_MAX 2.125 2.125 2.125\n"));

    project.sequences[0].events[1].color_correction.as_mut().unwrap().tone
        .push(("contrast".into(), 30.0));
    assert!(prepare_typed_project(&project, &invalid, &mut ids, &[], false).is_err());
    assert!(!invalid.exists());
    assert!(!scratch.0.join("invalid LUTs").exists());
}

#[test]
fn original_master_construction_matches_independent_python_and_sample_representation() {
    let scratch = Scratch::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let out = scratch.0.join("out.prproj");
    for shape in [
        "stereo",
        "reverse",
        "dualmono",
        "wav16mono",
        "wav16stereo",
        "wav24mono",
        "wav24stereo",
        "prores_hq",
    ] {
        let code = r#"import sys
from pathlib import Path
from fractions import Fraction
import py_premiere
from avid2prproj.original import Original,Video,Audio,Channel,add_item
from avid2prproj.determinism import seeded_ids
shape=sys.argv[2];wav=shape.startswith('wav');n=1 if wav and shape.endswith('mono') else 2
if shape=='prores_hq':
 c=Original(sys.argv[3],'mov',Video('apch',1920,1080,Fraction(24),508032000000),None,(),())
else:
 c=Original(sys.argv[3],'wav' if wav else 'mov',None if wav else Video('avc1',1920,1080,Fraction(24),508032000000),Audio(48000,n,508032000000,0 if wav else 1,'pcm16' if shape.startswith('wav16') else 'pcm24' if wav else 'aac'),tuple(Channel(i+1,1-i if shape=='reverse' else i) for i in range(n)),((1,),(2,)) if shape=='dualmono' else (tuple(range(1,n+1)),))
with seeded_ids('original-parity'):
 p=py_premiere.new().project;p._path=Path(sys.argv[1]);p._import_target=p.root_item.add_bin('Sources');add_item(p,c,'Synthetic original');p.save(sys.argv[1])
"#;
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                "python".into()
            } else {
                "python3".into()
            }
        });
        let source = original(shape).path;
        let p = Command::new(python)
            .args(["-c", code, out.to_str().unwrap(), shape, &source])
            .env(
                "PYTHONPATH",
                std::env::join_paths([root.join("src"), root.join("vendor")]).unwrap(),
            )
            .env("PYTHONHASHSEED", "0")
            .output()
            .unwrap();
        assert!(p.status.success(), "{}", String::from_utf8_lossy(&p.stderr));
        let expected = xml(&out);
        std::fs::remove_file(&out).unwrap();
        let mut ids = SeededUuids::new("original-parity");
        let prepared =
            prepare_typed_project(&input(original(shape)), &out, &mut ids, &[], false).unwrap();
        assert!(!out.exists());
        prepared.publish().unwrap();
        let actual = xml(&out);
        assert_eq!(actual, expected, "{shape}");
        assert!(!actual.contains("<Sequence "));
        assert_eq!(
            actual
                .lines()
                .filter(|s| s.contains("<Media ") && s.contains("ClassID="))
                .count(),
            1
        );
        if shape.starts_with("wav16") {
            assert!(actual.contains("<SampleType>3</SampleType>"));
        } else if shape.starts_with("wav24") {
            assert!(!actual.contains("<SampleType>"));
        } else if shape == "prores_hq" {
            assert!(!actual.contains("<AudioStream "));
            assert_eq!(actual.matches("<VideoStream ").count(), 2);
        } else {
            assert!(actual.contains("<SampleType>7</SampleType>"));
        }
        std::fs::remove_file(&out).unwrap();
    }
}
fn event() -> Event {
    Event {
        clip: "original".into(),
        record_start: 0,
        source_in: 0,
        duration: TICKS_PER_SECOND,
        video_track: Some(1),
        audio_tracks: vec![],
        audio: Default::default(),
        audio_channels: vec![],
        stereo: vec![StereoEdit {
            audio_track: 1,
            channels: vec![1, 2],
            gain_db: None,
            pans: vec![None, None],
        }],
        audio_gains: vec![],
        audio_pans: vec![],
        speed_ratio: None,
        source_duration: None,
        color_correction: None,
        transform: None,
        matte: None,
        time_remap: None,
    }
}
fn sequence(events: Vec<Event>) -> Sequence {
    Sequence {
        name: "Synthetic sequence".into(),
        rate: Fraction::new(24, 1),
        width: 1920,
        height: 1080,
        events,
        markers: vec![],
        track_levels: vec![],
        audio_crossfades: vec![],
        start: 0,
    }
}

#[test]
fn linked_original_marker_conflicts_follow_the_selected_source_lane() {
    let scratch = Scratch::new();
    let when = TICKS_PER_SECOND / 4;
    let source = Marker {
        name: "Source audio note".into(),
        comment: String::new(),
        start: when,
        duration: TICKS_PER_SECOND / 24,
        user: String::new(),
        color: "Red".into(),
        track: Some("A1".into()),
        owner: "sequence".into(),
        event: None,
        source: None,
        inherited: false,
    };
    for (track, same_lane) in [("V1", false), ("A1", false), ("A2", true)] {
        for winner in ["source", "edit"] {
            let mut clip = original("dualmono");
            clip.metadata.markers.push(source.clone());
            let mut project = input(clip);
            project.options.marker_conflict_winner = winner.into();
            let mut edit = event();
            edit.stereo.clear();
            edit.audio_tracks = vec![1, 2];
            // The A2 timeline lane plays the source's A1 channel.
            edit.audio_channels = vec![2, 1];
            let mut timeline = sequence(vec![edit]);
            timeline.markers.push(Marker {
                name: "Editorial note".into(),
                track: Some(track.into()),
                owner: "edit".into(),
                event: Some(0),
                source: Some(when),
                ..source.clone()
            });
            project.sequences.push(timeline);
            let out = scratch.0.join(format!("{track}-{winner}.prproj"));
            prepare_typed_project(&project, &out, &mut SeededUuids::new("linked-marker-lanes"), &[], false)
                .unwrap().publish().unwrap();
            let doc = prproj::xml::parse_prproj(&out).unwrap();
            let count = |name: &str| doc.tree.iter_tag(doc.root, "DVAMarker")
                .filter(|&node| doc.tree.text(node).is_some_and(|text| text.contains(name)))
                .count();
            assert_eq!(count("Source audio note"), if same_lane && winner == "edit" { 1 } else { 2 }, "{track} {winner}");
            assert_eq!(count("Editorial note"), if same_lane && winner == "source" { 0 } else { 1 }, "{track} {winner}");
        }
    }
}

#[test]
fn mono_clip_pan_targets_one_edit_and_uses_the_file_clock() {
    let scratch = Scratch::new();
    let out = scratch.0.join("clip-pan.prproj");
    let mut project = input(original("wav24mono"));
    project.options.match_avid_default_pan = true;
    let mut mono = event();
    mono.video_track = None;
    mono.stereo.clear();
    mono.audio_tracks = vec![1];
    mono.source_in = TICKS_PER_SECOND / 2;
    project.sequences.push(sequence(vec![mono]));
    let pan = PreparedAudioPan {
        sequence: 0,
        event: 0,
        track: 1,
        keys: vec![
            AudioPanKey { ticks: TICKS_PER_SECOND / 2, value: 0.0 },
            AudioPanKey { ticks: TICKS_PER_SECOND + 1, value: 1.0 },
        ],
    };
    project.audio_pans.push(pan.clone());
    let mut ids = SeededUuids::new("mono-clip-pan");
    prepare_typed_project(&project, &out, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&out).unwrap();
    let clip_pan = doc.tree.find1(doc.root, "AudioClipTrackItem/Panner").unwrap();
    let processor = doc.resolve(clip_pan).unwrap();
    assert_eq!(doc.tree.tag(processor), "MonoToStereoPanProcessor");
    let reference = doc.tree.find1(processor, "PanProcessor/AudioComponent/Component/Params/Param").unwrap();
    let param = doc.resolve(reference).unwrap();
    assert_eq!(doc.tree.findtext(param, "Name").unwrap().as_deref(), Some("Pan"));
    assert!(doc.tree.find1(param, "IsTimeVarying").is_none());
    let keys = doc.tree.findtext(param, "Keyframes").unwrap().unwrap();
    let ticks: Vec<_> = keys.split_terminator(';').map(|key| {
        key.split_once(',').unwrap().0.parse::<i64>().unwrap()
    }).collect();
    assert_eq!(ticks, [TICKS_PER_SECOND / 2, TICKS_PER_SECOND + 1]);
    project.audio_pans.push(pan);
    assert!(project.validate().is_err());
    project.audio_pans.pop();
    project.audio_pans[0].track = 2;
    assert!(project.validate().is_err());
    project.audio_pans[0].track = 1;
    project.audio_pans[0].keys[1].ticks = 2 * TICKS_PER_SECOND;
    assert!(project.validate().is_err());
}

#[test]
fn track_state_keeps_locked_edits_and_does_not_mute_audio() {
    let scratch = Scratch::new();
    let path = scratch.0.join("track-state.prproj");
    let mut input = input(original("prores_hq"));
    let mut wav = original("wav24mono");
    wav.metadata.id = "sound".into();
    input.clips.push(PreparedClip::Original(wav));
    input.bins[0].clips.push("sound".into());
    let mut picture = event();
    picture.stereo.clear();
    let mut a1 = picture.clone();
    a1.clip = "sound".into();
    a1.video_track = None;
    a1.audio_tracks = vec![1];
    let mut a2 = a1.clone();
    a2.audio_tracks = vec![2];
    input.sequences.push(sequence(vec![picture, a1, a2]));
    let states = vec![
        PreparedTrackState {
            lane: PreparedTrackLane::Picture(1), name: Some("Image & café".into()), locked: true, picture_disabled: true, create_empty: false,
        },
        PreparedTrackState {
            lane: PreparedTrackLane::Audio(1), name: Some("Dialogue".into()), locked: false, picture_disabled: false, create_empty: false,
        },
        PreparedTrackState {
            lane: PreparedTrackLane::Audio(2), name: Some("Dialogue".into()), locked: true, picture_disabled: false, create_empty: false,
        },
        PreparedTrackState {
            lane: PreparedTrackLane::Picture(5), name: Some("Graphics".into()), locked: false, picture_disabled: false, create_empty: true,
        },
        PreparedTrackState {
            lane: PreparedTrackLane::Audio(6), name: Some("Voiceover".into()), locked: false, picture_disabled: false, create_empty: true,
        },
    ];
    let mut invalid = states.clone();
    invalid[2].lane = PreparedTrackLane::Audio(1);
    input.track_states.insert(0, invalid);
    let mut ids = SeededUuids::new("track-state");
    assert!(prepare_typed_project(&input, &path, &mut ids, &[], false).is_err());
    assert!(!path.exists());
    let mut invalid = states.clone();
    invalid[2].picture_disabled = true;
    input.track_states.insert(0, invalid);
    assert!(input.validate().is_err());
    let mut invalid = states.clone();
    invalid[1].create_empty = true;
    input.track_states.insert(0, invalid);
    assert!(input.validate().is_err());
    input.track_states.insert(0, states.clone());
    prepare_typed_project(&input, &path, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&path).unwrap();
    let project = prproj::model::Project::from_document(doc, &path).unwrap();
    let seq = project.sequences()[0];
    let video = project.video_tracks(seq);
    let audio = project.audio_tracks(seq);
    assert_eq!((video.len(), audio.len()), (5, 6));
    for state in states {
        let (tracks, lane, tag) = match state.lane {
            PreparedTrackLane::Picture(lane) => (&video, lane, "VideoClipTrack"),
            PreparedTrackLane::Audio(lane) => (&audio, lane, "AudioClipTrack"),
        };
        let index = (lane - 1) as usize;
        let track = tracks[index];
        assert_eq!(project.track_clips(track).len(), usize::from(!state.create_empty));
        assert_eq!(project.is_locked(track).unwrap(), state.locked);
        assert_eq!(project.is_muted(track).unwrap(), state.picture_disabled);
        let tree = &project.doc.tree;
        let node = tree.findall(project.doc.root, tag).unwrap().into_iter()
            .find(|&track| tree.findtext(track, "ClipTrack/Track/Index").unwrap() == Some(index.to_string()))
            .unwrap();
        assert_eq!(tree.findtext(node, "ClipTrack/Track/Node/Properties/MZ.TrackName").unwrap(), state.name);
    }
    assert!(!project.is_locked(video[1]).unwrap());
    assert!(!project.is_muted(video[1]).unwrap());
}

#[test]
fn edit_labels_change_only_the_selected_clip_copy() {
    let scratch = Scratch::new();
    let mut input = input(original("prores_hq"));
    let mut first = event();
    first.stereo.clear();
    let mut second = first.clone();
    second.record_start = TICKS_PER_SECOND;
    input.sequences.push(sequence(vec![first, second]));

    let output = scratch.0.join("labeled.prproj");
    let label = PreparedEditLabel { sequence: 0, event: 1, audio_track: None, index: 13 };
    input.edit_labels.extend([label, label]);
    assert!(prepare_typed_project(&input, &output, &mut SeededUuids::new("label"), &[], false).is_err());
    assert!(!output.exists());
    input.edit_labels.pop();
    input.edit_labels[0].audio_track = Some(1);
    assert!(input.validate().is_err());
    input.edit_labels[0] = label;
    prepare_typed_project(&input, &output, &mut SeededUuids::new("label"), &[], false)
        .unwrap().publish().unwrap();

    let doc = prproj::PremiereDocument::from_bytes(&std::fs::read(&output).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &output).unwrap();
    let track = project.video_tracks(project.sequences()[0])[0];
    let items = project.track_clips(track);
    assert_eq!(items.len(), 2);
    let labels = |clip| {
        let tree = &project.doc.tree;
        let properties = tree.find(clip, "Clip/Node/Properties").unwrap().unwrap();
        ["asl.clip.label.name", "asl.clip.label.color"]
            .map(|field| tree.findtext(properties, field).unwrap().unwrap())
    };
    let first = project.track_item(items[0]);
    let second = project.track_item(items[1]);
    let master_ref = project.doc.tree.find(second.subclip, "MasterClip").unwrap().unwrap();
    let master = project.doc.resolve(master_ref).unwrap();
    let clip_ref = project.doc.tree.find(master, "Clips/Clip").unwrap().unwrap();
    assert_eq!(labels(first.clip), labels(project.doc.resolve(clip_ref).unwrap()));
    assert_eq!(labels(second.clip), ["BE.Prefs.LabelColors.13", "2579725"]);
}

#[test]
fn left_aligned_dissolve_uses_only_incoming_preroll() {
    let scratch = Scratch::new();
    let frame = TICKS_PER_SECOND / 24;
    let mut project = input(original("prores_hq"));
    let mut outgoing = event();
    outgoing.video_track = Some(5);
    outgoing.stereo.clear();
    outgoing.source_in = 24 * frame;
    let mut incoming = outgoing.clone();
    incoming.record_start = 24 * frame;
    incoming.source_in = 8 * frame;
    project.sequences.push(sequence(vec![outgoing, incoming]));
    project.video_dissolves.push(PreparedVideoDissolve {
        sequence: 0, outgoing: 0, incoming: 1, duration: 8 * frame, before_cut: 8 * frame,
    });
    let output = scratch.0.join("left.prproj");
    let mut ids = SeededUuids::new("left-dissolve");
    prepare_typed_project(&project, &output, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&output).unwrap();
    let tr = doc.tree.find1(doc.root, "VideoTransitionTrackItem/TransitionTrackItem").unwrap();
    for (field, value) in [
        ("TrackItem/Start", 16 * frame), ("TrackItem/End", 24 * frame), ("Alignment", 8 * frame),
    ] {
        assert_eq!(doc.tree.findtext(tr, field).unwrap(), Some(value.to_string()));
    }
    let project_readback = prproj::model::Project::from_document(doc, &output).unwrap();
    let tracks = project_readback.video_tracks(project_readback.sequences()[0]);
    assert_eq!(tracks.len(), 5);
    assert!(tracks[..4].iter().all(|&track| project_readback.track_transitions(track).is_empty()));
    assert_eq!(project_readback.track_transitions(tracks[4]).len(), 1);
    assert_eq!(project_readback.track_clips(tracks[4]).len(), 2);
    project.sequences[0].events[1].source_in = 7 * frame;
    let invalid = scratch.0.join("missing-handle.prproj");
    assert!(prepare_typed_project(&project, &invalid, &mut ids, &[], false).is_err());
    assert!(!invalid.exists());
}

#[test]
fn linked_mono_crossfade_uses_both_edit_ids_and_requires_source_handles() {
    let frame = TICKS_PER_SECOND / 24;
    let mut project = input(original("wav24mono"));
    let mut outgoing = event();
    outgoing.video_track = None;
    outgoing.audio_tracks = vec![1];
    outgoing.stereo.clear();
    outgoing.source_in = 8 * frame;
    let mut incoming = outgoing.clone();
    incoming.record_start = 24 * frame;
    project.sequences.push(sequence(vec![outgoing, incoming]));
    project.audio_crossfades.push(PreparedAudioCrossfade {
        sequence: 0, track: 1, outgoing: 0, incoming: 1, duration: 8 * frame, before_cut: 4 * frame,
    });

    let scratch = Scratch::new();
    let path = scratch.0.join("linked-mono-crossfade.prproj");
    let mut ids = SeededUuids::new("linked-mono-crossfade");
    prepare_typed_project(&project, &path, &mut ids, &[], false).unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&path).unwrap();
    let tree = &doc.tree;
    let clips = tree.findall(doc.root, "AudioClipTrackItem").unwrap();
    let [out, inc] = clips.as_slice() else { panic!("expected two audio edits") };
    let transition = tree.find1(doc.root, "AudioTransitionTrackItem").unwrap();
    for (edit, edge) in [(*out, "TailTransition"), (*inc, "HeadTransition")] {
        let edge = tree.find1(edit, &format!("ClipTrackItem/{edge}")).unwrap();
        assert_eq!(doc.resolve(edge).unwrap(), transition);
    }
    for (field, value) in [
        ("MatchName", "Constant Gain".into()),
        ("TrackItem/Start", (20 * frame).to_string()),
        ("TrackItem/End", (28 * frame).to_string()),
        ("Alignment", (4 * frame).to_string()),
    ] {
        assert_eq!(tree.findtext(transition, &format!("TransitionTrackItem/{field}")).unwrap(), Some(value));
    }
    let seq = &mut project.sequences[0];
    seq.events[1].source_in = 3 * frame;
    assert!(project.validate().is_err());
    project.sequences[0].events[1].source_in = 8 * frame;
    project.sequences[0].events[0].source_in = 21 * frame;
    assert!(project.validate().is_err());
}

#[test]
fn typed_mixed_timeline_and_master_copies_keep_sources_and_channels_separate() {
    let scratch = Scratch::new();
    let out = scratch.0.join("mixed.prproj");
    let mut p = input(original("reverse"));
    let mut copy = original("stereo");
    copy.metadata.id = "copy".into();
    p.clips.push(PreparedClip::Original(copy));
    p.bins.push(Bin {
        name: "Copies".into(),
        clips: vec!["copy".into()],
    });
    let legacy = parse_model_str(include_str!("../../examples/opatom_clip.model.json")).unwrap();
    let mut c = legacy.clips.values().next().unwrap().clone();
    c.id = "native".into();
    p.clips.push(PreparedClip::Legacy(c));
    p.bins[0].clips.push("native".into());
    let mut le = legacy.sequences[0].events[0].clone();
    le.clip = "native".into();
    le.record_start = TICKS_PER_SECOND;
    le.duration = TICKS_PER_SECOND;
    p.sequences.push(sequence(vec![event(), le]));
    p.validate().unwrap();
    let native = p.build_native_media_model().unwrap();
    assert_eq!(native.clips.len(), 1);
    assert!(native.sequences.is_empty());
    assert_eq!(native.bins[1].clips.len(), 0);
    let mut ids = SeededUuids::new("original-mixed");
    prepare_typed_project(&p, &out, &mut ids, &[], false)
        .unwrap()
        .publish()
        .unwrap();
    let actual = xml(&out);
    assert_eq!(
        actual
            .lines()
            .filter(|s| s.contains("<Sequence ") && s.contains("ClassID="))
            .count(),
        1
    );
    assert!(actual.contains("<mSourceChannelIndex>1</mSourceChannelIndex>"));
    let source = original("stereo").path;
    assert!(actual.contains(&source.replace('&', "&amp;")));
}
#[test]
fn mono_wav_edge_fades_and_volume_keys_keep_their_own_clocks() {
    let scratch = Scratch::new();
    let out = scratch.0.join("clip-fades.prproj");
    let mut project = input(original("wav16mono"));
    let mut audio = event();
    audio.video_track = None;
    audio.audio_tracks = vec![1];
    audio.stereo.clear();
    audio.source_in = TICKS_PER_SECOND / 4;
    let mut neighbor = audio.clone();
    neighbor.record_start = TICKS_PER_SECOND;
    audio.audio.volume_keys = vec![
        VolumeKey { ticks: TICKS_PER_SECOND / 2, db: -6.0 },
        VolumeKey { ticks: TICKS_PER_SECOND, db: 3.0 },
    ];
    let mut timeline = sequence(vec![audio, neighbor]);
    timeline.track_levels.push(TrackLevel { track: 1, db: -3.0 });
    project.sequences.push(timeline);
    project.audio_mixers.insert(0, PreparedAudioMixer {
        mix_level_db: Some(-3.7),
        muted_tracks: vec![1],
        mono_track_pans: None,
    });
    let duration = TICKS_PER_SECOND / 4 + 1;
    let fade = PreparedAudioEdgeFade {
        sequence: 0,
        event: 0,
        track: 1,
        head: 0,
        tail: duration,
    };
    project.audio_edge_fades.push(fade.clone());
    let mut ids = SeededUuids::new("mono-wav-clip-fades");
    prepare_typed_project(&project, &out, &mut ids, &[], false)
        .unwrap()
        .publish()
        .unwrap();
    let actual = xml(&out);
    assert_eq!(actual.matches("<TailTransition ObjectRef=").count(), 1);
    assert!(!actual.contains("<HeadTransition ObjectRef="));
    assert!(actual.contains("<HasIncomingClip>false</HasIncomingClip>"));
    assert!(actual.contains("<HasOutgoingClip>true</HasOutgoingClip>"));
    assert!(actual.contains(&format!("<Alignment>{duration}</Alignment>")));
    assert!(actual.contains(&format!("<Start>{}</Start>", TICKS_PER_SECOND - duration)));
    let keys = actual.lines().find_map(|line| line.split_once("<Keyframes>")).unwrap().1;
    assert!(keys.starts_with(&format!("{},", TICKS_PER_SECOND / 2)));
    assert!(keys.contains(&format!(";{},", TICKS_PER_SECOND)));
    assert!(actual.contains("<CurrentValue>0.7079457843841379</CurrentValue>"));
    assert!(!actual.contains("<Gain>"));
    let doc = prproj::xml::parse_prproj(&out).unwrap();
    let master = doc.tree.find1(doc.root, "AudioTrackGroup/MasterTrack").unwrap();
    let mix = doc.resolve(master).unwrap();
    assert_eq!(doc.tree.tag(mix), "AudioMixTrack");
    let owner = doc.tree.find1(mix, "AudioTrack/ComponentOwner/Components").unwrap();
    let chain = doc.resolve(owner).unwrap();
    let fader = doc.tree.findall(chain, "ComponentChain/Components/Component").unwrap()
        .into_iter().map(|node| doc.resolve(node).unwrap())
        .find(|&node| doc.tree.tag(node) == "AudioFader").unwrap();
    let volume = doc.tree.iter_tag(fader, "Param").map(|node| doc.resolve(node).unwrap())
        .find(|&node| doc.tree.findtext(node, "Name").unwrap().as_deref() == Some("Volume")).unwrap();
    let value = doc.tree.findtext(volume, "CurrentValue").unwrap().unwrap();
    assert!((value.parse::<f64>().unwrap() - 10.0_f64.powf(-3.7 / 20.0)).abs() < 1e-14);
    let reopened = prproj::model::Project::from_document(doc, &out).unwrap();
    let sequence = reopened.sequences().iter().next().copied().unwrap();
    let tracks = reopened.audio_tracks(sequence);
    assert!(reopened.is_muted(tracks[0]).unwrap());
    assert!(!reopened.is_muted(tracks[1]).unwrap());
    project.audio_mixers.insert(1, PreparedAudioMixer::default());
    assert!(project.validate().is_err());
    project.audio_mixers.remove(&1);
    project.audio_edge_fades.push(fade);
    assert!(project.validate().is_err());
    project.audio_edge_fades.pop();
    project.audio_edge_fades[0].tail = TICKS_PER_SECOND + 1;
    assert!(project.validate().is_err());

    project.audio_edge_fades[0].tail = duration;
    project.audio_mixers.get_mut(&0).unwrap().mono_track_pans = Some([Some(-100.0), Some(100.0)]);
    let panned = scratch.0.join("track-pan.prproj");
    prepare_typed_project(&project, &panned, &mut ids, &[], false)
        .unwrap()
        .publish()
        .unwrap();
    let doc = prproj::xml::parse_prproj(&panned).unwrap();
    assert_eq!(doc.tree.findtext(doc.root, "AudioTransitionTrackItem/AudioChannelLayout").unwrap()
        .as_deref(), Some(r#"[{"channellabel":0}]"#));
    let group = doc.tree.find1(doc.root, "AudioTrackGroup").unwrap();
    let tracks = doc.tree.findall(group, "TrackGroup/Tracks/Track").unwrap();
    for (reference, want) in tracks.into_iter().zip([0.0, 1.0]) {
        let track = doc.resolve(reference).unwrap();
        let panner = doc.resolve(doc.tree.find1(track, "AudioTrack/Panner").unwrap()).unwrap();
        assert_eq!(doc.tree.tag(panner), "MonoToStereoPanProcessor");
        let param = doc.resolve(doc.tree.find1(
            panner, "PanProcessor/AudioComponent/Component/Params/Param",
        ).unwrap()).unwrap();
        let value = doc.tree.findtext(param, "CurrentValue").unwrap().unwrap();
        assert_eq!(value.parse::<f64>().unwrap(), want);
    }
}

#[test]
fn original_prores_speed_preserves_raw_file_trim() {
    let scratch = Scratch::new();
    let frame = TICKS_PER_SECOND / 24;
    for (name, first, length, ratio, span, playback) in [
        ("double", 13, 12, (1, 2), 24 * frame, 2.0),
        ("half-at-tail", 36, 23, (2, 1), 23 * frame / 2, 0.5),
    ] {
        let mut project = input(original("prores_hq"));
        let mut edit = event();
        edit.stereo.clear();
        edit.source_in = first * frame;
        edit.duration = length * frame;
        edit.speed_ratio = Some(ratio);
        edit.source_duration = Some(span);
        project.sequences.push(sequence(vec![edit]));
        let path = scratch.0.join(format!("{name}.prproj"));
        let mut ids = SeededUuids::new(name);

        prepare_typed_project(&project, &path, &mut ids, &[], false)
            .unwrap()
            .publish()
            .unwrap();
        let doc = prproj::xml::parse_prproj(&path).unwrap();
        let track_item_subclip = doc
            .tree
            .find1(doc.root, "VideoClipTrackItem/ClipTrackItem/SubClip")
            .unwrap();
        let subclip = doc.resolve(track_item_subclip).unwrap();
        let clip = doc.resolve(doc.tree.find1(subclip, "Clip").unwrap()).unwrap();
        let saved = |tag| doc.tree.findtext(clip, tag).unwrap().unwrap();
        assert_eq!(saved("Clip/PlaybackSpeed").parse::<f64>().unwrap(), playback);
        assert_eq!(saved("Clip/InPoint").parse::<i64>().unwrap(), first * frame);
        assert_eq!(saved("Clip/OutPoint").parse::<i64>().unwrap(), first * frame + span);

        let edit = &mut project.sequences[0].events[0];
        edit.source_duration = Some(span - frame / 2);
        let invalid = scratch.0.join(format!("{name}-contradictory.prproj"));
        assert!(prepare_typed_project(&project, &invalid, &mut ids, &[], false).is_err());
        assert!(!invalid.exists());
    }
}

#[test]
fn frame_hold_belongs_to_the_placed_picture_and_preserves_edit_bounds() {
    let scratch = Scratch::new();
    let frame = TICKS_PER_SECOND / 24;
    let mut project = input(original("prores_hq"));
    let mut moving = event();
    moving.stereo.clear();
    moving.source_in = 12 * frame;
    moving.duration = 5 * frame;
    let mut held = moving.clone();
    held.record_start += moving.duration;
    held.source_in = 17 * frame;
    held.duration = 18 * frame;
    project.sequences.push(sequence(vec![moving, held]));
    project.frame_holds.insert((0, 1), 47 * frame);

    let path = scratch.0.join("frame-hold.prproj");
    let mut ids = SeededUuids::new("frame-hold");
    prepare_typed_project(&project, &path, &mut ids, &[], false)
        .unwrap()
        .publish()
        .unwrap();
    let doc = prproj::xml::parse_prproj(&path).unwrap();
    let edits = doc.tree.children(doc.root).iter().copied()
        .filter(|&node| doc.tree.tag(node) == "VideoClipTrackItem");
    let mut saved = Vec::new();
    for edit in edits {
        let subclip = doc.resolve(doc.tree.find1(edit, "ClipTrackItem/SubClip").unwrap()).unwrap();
        let clip = doc.resolve(doc.tree.find1(subclip, "Clip").unwrap()).unwrap();
        let end = doc.tree.findtext(edit, "ClipTrackItem/TrackItem/End").unwrap().unwrap();
        let hold = doc.tree.findtext(clip, "FrameHold").unwrap();
        let hold_start = doc.tree.findtext(clip, "FrameHoldStart").unwrap();
        saved.push((end.parse::<i64>().unwrap(), hold, hold_start));
    }
    saved.sort_by_key(|edit| edit.0);
    assert_eq!(saved, vec![
        (5 * frame, None, None),
        (23 * frame, Some("5".into()), Some((47 * frame).to_string())),
    ]);

    project.frame_holds.insert((0, 1), 48 * frame);
    let invalid = scratch.0.join("hold-past-file.prproj");
    assert!(prepare_typed_project(&project, &invalid, &mut ids, &[], false).is_err());
    assert!(!invalid.exists());

    project.frame_holds.insert((0, 1), 47 * frame);
    project.frame_holds.insert((0, 2), 47 * frame);
    let invalid = scratch.0.join("unknown-hold.prproj");
    assert!(prepare_typed_project(&project, &invalid, &mut ids, &[], false).is_err());
    assert!(!invalid.exists());
}

#[test]
fn sequence_marks_keep_zero_and_an_exclusive_out_without_moving_the_edit() {
    let scratch = Scratch::new();
    let frame = TICKS_PER_SECOND / 24;
    let mut project = input(original("prores_hq"));
    let mut clip = event();
    clip.stereo.clear();
    clip.record_start = 0;
    clip.source_in = 4 * frame;
    clip.duration = 12 * frame;
    let mut timeline = sequence(vec![clip]);
    timeline.start = 86401 * frame;
    project.sequences.push(timeline);
    let mut ids = SeededUuids::new("timeline-marks");
    let output = scratch.0.join("marks.prproj");
    project.timeline_marks.insert(0, PreparedTimelineMarks {
        in_ticks: Some(0),
        out_ticks: Some(12 * frame),
    });
    prepare_typed_project(&project, &output, &mut ids, &[], false)
        .unwrap().publish().unwrap();
    let doc = prproj::xml::parse_prproj(&output).unwrap();
    let seq = doc.tree.children(doc.root).iter().copied()
        .find(|&node| doc.tree.tag(node) == "Sequence").unwrap();
    let saved = |key| {
        doc.tree.findtext(seq, key).unwrap().unwrap().parse::<i64>().unwrap()
    };
    assert_eq!(saved("Node/Properties/MZ.ZeroPoint"), 86401 * frame);
    assert_eq!(saved("Node/Properties/MZ.InPoint"), 0);
    assert_eq!(saved("Node/Properties/MZ.OutPoint"), 12 * frame);

    let invalid = scratch.0.join("invalid.prproj");
    for (start, end) in [(Some(frame), Some(frame)), (None, Some(frame + 1)),
        (Some(12 * frame), None), (None, Some(13 * frame))]
    {
        project.timeline_marks.insert(0, PreparedTimelineMarks { in_ticks: start, out_ticks: end });
        assert!(prepare_typed_project(&project, &invalid, &mut ids, &[], false).is_err());
        assert!(!invalid.exists());
    }
}

#[test]
fn original_refusals_are_atomic_and_legacy_model_remains_strict() {
    let scratch = Scratch::new();
    let out = scratch.0.join("never.prproj");
    let base = input(original("stereo"));
    let mut variants = vec![];
    let mutate = |change: fn(&mut OriginalClip)| {
        let mut p = base.clone();
        if let PreparedClip::Original(c) = &mut p.clips[0] {
            change(c)
        }
        p
    };
    variants.push(mutate(|c| c.channels[1].channel_index = 0));
    variants.push(mutate(|c| {
        let audio = c.audio.as_mut().unwrap();
        audio.representation = SampleRepresentation::Pcm16;
        audio.duration -= ticks_per_unit(Fraction::new(48_000, 1));
    }));
    variants.push(mutate(|c| c.video.as_mut().unwrap().stream_index = 2));
    variants.push(mutate(|c| c.groups = vec![vec![1]]));
    variants.push(mutate(|c| c.path = "relative.mp4".into()));
    let mut p = base.clone();
    p.sequences.push(sequence(vec![event()]));
    p.sequences[0].events[0].stereo[0].channels.reverse();
    variants.push(p);
    let mut p = base.clone();
    p.sequences.push(sequence(vec![event()]));
    p.sequences[0].events[0].source_in = 2 * TICKS_PER_SECOND;
    variants.push(p);
    let mut p = base.clone();
    p.sequences.push(sequence(vec![event()]));
    p.sequences[0].events[0].speed_ratio = Some((1, 2));
    p.sequences[0].events[0].source_duration = Some(TICKS_PER_SECOND);
    variants.push(p);
    for p in variants {
        let mut ids = SeededUuids::new("refuse");
        assert!(prepare_typed_project(&p, &out, &mut ids, &[], false).is_err());
        assert_eq!(ids.count(), 0);
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
    }
    let mut legacy =
        parse_model_str(include_str!("../../examples/opatom_clip.model.json")).unwrap();
    let id = legacy.clips.values().next().unwrap().id.clone();
    let c = legacy.clips.get_mut(&id).unwrap();
    c.audio[0].bits = 0;
    assert!(validate_model(&legacy).is_err());
}

#[test]
fn disabled_mono_edit_round_trips_and_leaves_neighbors_enabled() {
    let scratch = Scratch::new();
    let mut input = input(original("wav16mono"));
    let mut first = event();
    first.video_track = None;
    first.stereo.clear();
    first.audio_tracks = vec![1];
    first.audio_channels = vec![1];
    let mut second = first.clone();
    second.record_start = TICKS_PER_SECOND;
    input.options.bin_mode = "separate".into();
    input.sequences.push(sequence(vec![first, second]));
    let target = scratch.0.join("disabled.prproj");
    input.disabled_edits.insert((0, 2));
    assert!(prepare_typed_project(&input, &target, &mut SeededUuids::new("disabled"), &[], false).is_err());
    assert!(!target.exists());
    input.disabled_edits.clear();
    input.disabled_edits.insert((0, 1));
    prepare_typed_project(&input, &target, &mut SeededUuids::new("disabled"), &[], false)
        .unwrap().publish().unwrap();

    let project = prproj::model::Project::from_document(
        prproj::xml::parse_prproj(&target).unwrap(), &target,
    ).unwrap();
    let track = project.audio_tracks(project.sequences()[0])[0];
    let [a, b] = project.track_clips(track) else { panic!("expected two mono placements") };
    assert!(!project.is_disabled(*a).unwrap());
    assert!(project.is_disabled(*b).unwrap());
    assert!(!project.is_muted(track).unwrap());
}
