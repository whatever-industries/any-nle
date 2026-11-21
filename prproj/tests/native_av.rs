//! Native-route structural/routing checks + exact Python parity. No application claim.
use prproj::convert::{
    model::load_model,
    rewrap::{MediaRelink, RATES},
    writer::write_native_op1a_project,
};
use prproj::SeededUuids;
use std::{path::PathBuf, process::Command};

#[test]
fn native_av_groups_routing_and_all_clocks_match_python() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tmp = std::env::temp_dir().join(format!(
        "anynle-native-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&tmp).unwrap();
    let python = |code: &str, args: &[String]| {
        let p = Command::new("python3")
            .arg("-c")
            .arg(code)
            .args(args)
            .env(
                "PYTHONPATH",
                format!(
                    "{}:{}:{}",
                    root.join("src").display(),
                    root.join("vendor").display(),
                    root.join("tests").display()
                ),
            )
            .env("PYTHONHASHSEED", "0")
            .output()
            .unwrap();
        assert!(p.status.success(), "{}", String::from_utf8_lossy(&p.stderr));
    };
    let mut cases = RATES
        .iter()
        .map(|&(n, d)| ("mono", n, d))
        .collect::<Vec<_>>();
    for (n, d) in [(24, 1), (24000, 1001), (60000, 1001)] {
        for shape in ["stereo", "reverse", "mixed", "sparse", "copies", "edit"] {
            cases.push((shape, n, d));
        }
    }
    for &(n, d) in RATES {
        for shape in ["surround51","surround71","surround51reordered","surround71reordered","surround51reorderedmov","surround71reorderedmov","surround51aac","surround51aacmov","surround71smpte","surround71smptemov"] {
            cases.push((shape,n,d));
        }
    }
    for shape in ["eightmono","eightmonomov","pairmono","pairmonomov","sparse99","stereo99","surround5199","level99","fade99"] {
        cases.push((shape,30000,1001));
    }
    cases.push(("many", 24, 1));
    cases.push(("pcm16", 24000, 1001));
    cases.push(("video", 24, 1));
    cases.push(("copies", 120000, 1001));
    for (case, (shape, n, d)) in cases.iter().enumerate() {
        let fixture = tmp.join("model.json");
        let out = tmp.join("out.prproj");
        let target = tmp.join(if shape.ends_with("mov") {"synthetic.mov"}else{"synthetic.mxf"});
        python(
            r#"import json,sys
from pathlib import Path
from native_av_cases import model_case,bindings,verify
from avid2prproj.model import parse_model
from avid2prproj.writer import write_project
from avid2prproj.determinism import seeded_ids
v=model_case(sys.argv[4],(int(sys.argv[5]),int(sys.argv[6])));Path(sys.argv[1]).write_text(json.dumps(v))
with seeded_ids('native-av-parity'): r=write_project(parse_model(v),Path(sys.argv[2]),media_relinks=bindings(v,sys.argv[3]),native_op1a_master=True)
verify(sys.argv[2],v,sys.argv[3]);Path(sys.argv[2]+'.report').write_text('\n'.join(r.notes+r.unverified))
"#,
            &[
                fixture.display().to_string(),
                out.display().to_string(),
                target.display().to_string(),
                shape.to_string(),
                n.to_string(),
                d.to_string(),
            ],
        );
        let expected = std::fs::read(&out).unwrap();
        std::fs::remove_file(&out).unwrap();
        let model = load_model(&fixture).unwrap();
        let mut mappings = Vec::new();
        let mut clips = model.clips.values().collect::<Vec<_>>();
        clips.sort_by_key(|c| &c.id);
        for c in clips {
            mappings.push(MediaRelink {
                source_path: c.video.as_ref().unwrap().path.clone(),
                target_path: target.display().to_string(),
                audio_stream: None,
                separate_mono: false,
                pixel_aspect: None,
            });
            let mut audio = c.audio.iter().collect::<Vec<_>>();
            audio.sort_by_key(|a| a.avid_track);
            for (i, a) in audio.iter().enumerate() {
                mappings.push(MediaRelink {
                    source_path: a.path.clone(),
                    target_path: target.display().to_string(),
                    audio_stream: Some(i as u32),
                    separate_mono: false,
                    pixel_aspect: None,
                });
            }
        }
        let r = write_native_op1a_project(
            &model,
            &out,
            &mut SeededUuids::new("native-av-parity"),
            &mappings,
        )
        .unwrap();
        assert_eq!(
            expected,
            std::fs::read(&out).unwrap(),
            "case {case}: {shape} {n}/{d}; artifacts at {}",
            tmp.display()
        );
        let mut report = r.notes;
        report.extend(r.unverified);
        assert_eq!(
            report.join("\n"),
            std::fs::read_to_string(tmp.join("out.prproj.report")).unwrap()
        );
        // Rust refuses mismatched bindings before saving a new project.
        if let Some(m) = mappings.iter_mut().find(|m| m.audio_stream.is_some()) {
            m.audio_stream = Some(31);
        } else {
            mappings.clear();
        }
        let refused = tmp.join("never.prproj");
        assert!(write_native_op1a_project(
            &model,
            &refused,
            &mut SeededUuids::new("refuse"),
            &mappings
        )
        .is_err());
        assert!(!refused.exists());
        if *shape == "mixed" {
            let mut variants = Vec::new();
            let mut bad = model.clone();
            bad.sequences[0].events[0].audio.volume_db = Some(-3.0);
            variants.push(bad);
            let mut bad = model.clone();
            bad.clips.get_mut("clip-1").unwrap().audio[0].gain_db = -12.0;
            variants.push(bad);
            let mut bad = model.clone();
            bad.clips.get_mut("clip-1").unwrap().audio[0].start += 1;
            variants.push(bad);
            let mut bad = model.clone();
            bad.sequences[0].events[0].audio_channels = vec![3];
            variants.push(bad);
            let mut bad = model.clone();
            bad.sequences[0].events[0].stereo[0].channels.reverse();
            variants.push(bad);
            let mut bad = model.clone();
            bad.sequences[0].events[0].stereo[0].pans = vec![Some(0.0), Some(100.0)];
            variants.push(bad);
            for bad in variants {
                assert!(prproj::convert::native_av::preflight(&bad).is_err());
                assert!(!refused.exists());
            }
        }
        std::fs::remove_file(out).unwrap();
    }
    std::fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn grouped_presets_keep_base_settings_and_refuse_incompatible_wrappers() {
    use prproj::build::sequence::format_by_name;
    for base in ["1080p2997","1080p5994","2160p5994hdr","anynle-op1a/5292000000/1920/1080/113"] {
        for width in [6,8] {
            let mut expected=format_by_name(base).unwrap();expected.grouped_channels=width;
            assert_eq!(format_by_name(&format!("anynle-grouped/{width}/{base}")),Some(expected));
        }
    }
    for bad in ["anynle-grouped/7/1080p2997","anynle-grouped/6/anynle-grouped/8/1080p2997","anynle-grouped/6/broadcast8mono","anynle-grouped/8/broadcast4mono","anynle-grouped/8/missing"] {
        assert!(format_by_name(bad).is_none(),"{bad}");
    }
}

#[test]
fn audio_track_capacity_keeps_clock_and_bus_and_is_bounded() {
    use prproj::build::sequence::format_by_name;
    for base in ["1080p2997","1080p5994","anynle-grouped/6/1080p2997","anynle-grouped/8/1080p5994"] {
        for count in [6,8,99] {
            let mut expected=format_by_name(base).unwrap();expected.audio_tracks=count;
            assert_eq!(format_by_name(&format!("anynle-tracks/{count}/{base}")),Some(expected));
        }
    }
    for bad in ["anynle-tracks/0/1080p2997","anynle-tracks/3/1080p2997","anynle-tracks/100/1080p2997","anynle-tracks/99/broadcast4mono","anynle-tracks/8/anynle-tracks/6/1080p2997","anynle-grouped/6/anynle-tracks/8/1080p2997","anynle-tracks/8/anynle-grouped/6/anynle-tracks/8/1080p2997"] {
        assert!(format_by_name(bad).is_none(),"{bad}");
    }
}
