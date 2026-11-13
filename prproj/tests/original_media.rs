//! Synthetic-only original-file semantics and independent Python construction parity.
use prproj::convert::{model::*, prepared::*, writer::prepare_typed_project};
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
        }),
        audio: (!prores).then_some(OriginalAudio {
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
        texts: Vec::new(),
        multicams: Vec::new(),
        multicam_uses: Vec::new(),
        video_dissolves: Vec::new(),
        audio_edge_fades: Vec::new(),
        audio_crossfades: Vec::new(),
        audio_mixers: Default::default(),
        picture_effects: Vec::new(),
        direct_dnxhr_origins: Default::default(),
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
fn left_aligned_dissolve_uses_only_incoming_preroll() {
    let scratch = Scratch::new();
    let frame = TICKS_PER_SECOND / 24;
    let mut project = input(original("prores_hq"));
    let mut outgoing = event();
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
        c.audio.as_mut().unwrap().representation = SampleRepresentation::Pcm16
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
