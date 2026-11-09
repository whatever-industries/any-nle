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
    let mono = shape.starts_with("wav") && shape.ends_with("mono");
    let n = if mono { 1 } else { 2 };
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
            } else {
                "anynle-synthetic-source.mp4"
            })
            .to_string_lossy()
            .into_owned(),
        container: if wav { "wav" } else { "mov" }.into(),
        video: (!wav).then_some(OriginalVideo {
            codec: "avc1".into(),
            width: 1920,
            height: 1080,
            rate: Fraction::new(24, 1),
            duration: 2 * TICKS_PER_SECOND,
            stream_index: 0,
        }),
        audio: Some(OriginalAudio {
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
        picture_effects: Vec::new(),
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
    ] {
        let code = r#"import sys
from pathlib import Path
from fractions import Fraction
import py_premiere
from avid2prproj.original import Original,Video,Audio,Channel,add_item
from avid2prproj.determinism import seeded_ids
shape=sys.argv[2];wav=shape.startswith('wav');n=1 if wav and shape.endswith('mono') else 2
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
