//! Prepare once, with no filesystem outputs; publication consumes the same payload.

mod common;

use common::Scratch;
use prproj::convert::model::{
    parse_model_str, ticks_per_unit, AudioCrossfade, ColorCorrection, Model,
};
use prproj::convert::writer::{prepare_project, write_project};
use prproj::{SeededUuids, Uuid, UuidSource};

struct Count {
    inner: SeededUuids,
    calls: usize,
}
impl Count {
    fn new() -> Self {
        Self {
            inner: SeededUuids::new("prepared"),
            calls: 0,
        }
    }
}
impl UuidSource for Count {
    fn uuid4(&mut self) -> Uuid {
        self.calls += 1;
        self.inner.uuid4()
    }
}
fn example(color: bool) -> Model {
    let mut m = parse_model_str(include_str!("../../examples/opatom_clip.model.json")).unwrap();
    if color {
        m.sequences[0].events[0].color_correction = Some(ColorCorrection {
            saturation: Some(125.0),
            ..Default::default()
        });
    }
    m
}

#[test]
fn prepared_document_publishes_without_more_ids_or_rebuilding() {
    let scratch = Scratch::new("anynle-prepared");
    let out = scratch.path().join("out.prproj");
    let m = example(true);
    let mut ids = Count::new();
    let mut p = prepare_project(&m, &out, &mut ids, &[], false).unwrap();
    let count = ids.calls;
    assert!(count > 0);
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    assert!(!p.is_published());
    p.publish_assets().unwrap();
    assert!(!out.exists());
    let report = p.publish_project().unwrap();
    assert!(p.is_published());
    assert_eq!(ids.calls, count);
    let bytes = std::fs::read(&out).unwrap();
    let cube = std::fs::read_dir(scratch.path().join("out LUTs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let cube_bytes = std::fs::read(&cube).unwrap();
    std::fs::remove_file(&out).unwrap();
    std::fs::remove_dir_all(scratch.path().join("out LUTs")).unwrap();
    let expected = write_project(&m, &out, &mut SeededUuids::new("prepared")).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), bytes);
    assert_eq!(std::fs::read(&cube).unwrap(), cube_bytes);
    assert_eq!(report, expected);
}

#[test]
fn snapshot_cannot_reserve_a_planned_lut_file_or_directory() {
    let scratch = Scratch::new("anynle-prepared");
    let out = scratch.path().join("out.prproj");
    let m = example(true);
    let mut p = prepare_project(&m, &out, &mut SeededUuids::new("prepared"), &[], false).unwrap();
    assert!(p.check_reserved_output(&scratch.path().join("out LUTs")).is_err());
    p.publish_assets().unwrap();
    let cube = std::fs::read_dir(scratch.path().join("out LUTs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(p.check_reserved_output(&cube).is_err());
    assert!(p.check_reserved_output(&scratch.path().join("source.json")).is_ok());
}

#[test]
fn malformed_typed_native_inputs_refuse_without_panicking_or_outputs() {
    for short_pan in [false, true] {
        let scratch = Scratch::new("anynle-prepared");
        let out = scratch.path().join("out.prproj");
        let mut m = example(false);
        if short_pan {
            let e = &mut m.sequences[0].events[0];
            e.audio_tracks.clear();
            e.audio_channels.clear();
            e.stereo.push(prproj::convert::model::StereoEdit {
                audio_track: 1,
                channels: vec![1, 2],
                gain_db: None,
                pans: vec![Some(0.0)],
            });
        } else {
            m.sequences[0].events[0].clip = "missing".into();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare_project(&m, &out, &mut SeededUuids::new("typed-refusal"), &[], true)
        }));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prproj::convert::native_av::preflight(&m)
        }));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    }
}

#[test]
fn late_project_or_lut_is_preserved_without_replanning() {
    let scratch = Scratch::new("anynle-prepared");
    let out = scratch.path().join("out.prproj");
    let m = example(false);
    let mut p = prepare_project(&m, &out, &mut SeededUuids::new("prepared"), &[], false).unwrap();
    p.publish_assets().unwrap();
    std::fs::write(&out, b"incumbent").unwrap();
    assert!(p.publish_project().is_err());
    assert!(!p.is_published());
    assert_eq!(std::fs::read(&out).unwrap(), b"incumbent");
    std::fs::remove_file(&out).unwrap();
    let m = example(true);
    let mut p = prepare_project(&m, &out, &mut SeededUuids::new("prepared"), &[], false).unwrap();
    let dir = scratch.path().join("out LUTs");
    std::fs::create_dir(&dir).unwrap();
    // Obtain the planned name through the unchanged legacy naming rule by using
    // the source sequence and first edit time, rather than renaming after preparation.
    let tc = prproj::convert::writer::timecode(
        m.sequences[0].events[0].record_start,
        m.sequences[0].rate,
        m.sequences[0].start,
    )
    .unwrap();
    let safe: String = m.sequences[0]
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    let cube = dir.join(format!("{safe} {} avid cc.cube", tc.replace(':', "-")));
    std::fs::write(&cube, b"late LUT").unwrap();
    assert!(p.publish_assets().is_err());
    assert!(!out.exists());
    assert_eq!(std::fs::read(&cube).unwrap(), b"late LUT");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn predictable_crossfade_failure_does_not_create_luts_or_project() {
    let scratch = Scratch::new("anynle-prepared");
    let out = scratch.path().join("out.prproj");
    let mut m = example(true);
    let seq = &mut m.sequences[0];
    let unit = ticks_per_unit(seq.rate);
    let mut incoming = seq.events[0].clone();
    incoming.record_start = seq.events[0].record_start + seq.events[0].duration;
    incoming.source_in = 0;
    incoming.color_correction = None;
    seq.audio_crossfades.push(AudioCrossfade {
        track: 1,
        cut: incoming.record_start,
        duration: incoming.duration + 2 * unit,
        cut_offset: unit,
        style: "constant_power".into(),
    });
    seq.events.push(incoming);
    assert!(prepare_project(&m, &out, &mut SeededUuids::new("prepared"), &[], false).is_err());
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}

#[test]
fn planned_lut_collisions_get_distinct_paths_without_writing() {
    let scratch = Scratch::new("anynle-prepared");
    let out = scratch.path().join("out.prproj");
    let mut m = example(true);
    let second = m.sequences[0].clone();
    m.sequences.push(second);
    let mut p = prepare_project(&m, &out, &mut SeededUuids::new("prepared"), &[], false).unwrap();
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    p.publish_assets().unwrap();
    p.publish_project().unwrap();
    let files = std::fs::read_dir(scratch.path().join("out LUTs"))
        .unwrap()
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|e| e
        .as_ref()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("(2)")));
}
