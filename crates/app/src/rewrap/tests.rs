//! Existing media-preparation checks, retained across the module split.
use super::backend::{bounded, integer, probe, run};
use super::source::pcm_essence_bytes;
use super::validation::timecode;
use super::{normalize_source_clocks, validate_bin_audio, Plan, R};
use graph::json::{self, Value};
use prproj::build::time::Time;
use prproj::convert::model::{ticks_per_unit, Model, TICKS_PER_SECOND as TPS};
use prproj::convert::rewrap::display_format;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use prproj::convert::model::{parse_model_str, Fraction};
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let n = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "anynle-op1a-{}-{n}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
    fn fixture(&self) -> Model {
        let v = self.0.join("v.mxf");
        let a = self.0.join("a.mxf");
        let b = self.0.join("b.mxf");
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=24",
                "-frames:v",
                "12",
                "-c:v",
                "dnxhd",
                "-profile:v",
                "dnxhr_lb",
                "-timecode",
                "01:00:00:00",
                "-f",
                "mxf_opatom",
            ])
            .arg(&v))
        .unwrap();
        for (p, f) in [(&a, "440"), (&b, "880")] {
            run(Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={f}:sample_rate=48000:duration=0.5"),
                    "-c:a",
                    "pcm_s24le",
                    "-timecode",
                    "01:00:00:00",
                    "-mxf_audio_edit_rate",
                    "24",
                    "-f",
                    "mxf_opatom",
                ])
                .arg(p))
            .unwrap();
        }
        let mut m =
            parse_model_str(include_str!("../../../../examples/opatom_clip.model.json")).unwrap();
        m.sequences.clear();
        let c = m.clips.get_mut("clip-1").unwrap();
        let v1 = c.video.as_mut().unwrap();
        v1.path = v.to_string_lossy().into();
        v1.width = 320;
        v1.height = 180;
        v1.duration = TPS / 2;
        v1.start = TPS * 3600;
        c.rate = Some(Fraction::new(24, 1));
        for (i, p) in [a, b].iter().enumerate() {
            let a = &mut c.audio[i];
            a.path = p.to_string_lossy().into();
            a.bits = 24;
            a.sample_rate = 48000;
            a.duration = TPS / 2;
            a.start = TPS * 3600;
            a.avid_track = Some(i as i64 + 1);
        }
        m
    }
    fn rate_fixture(&self, rate: Fraction, count: usize, drop: bool) -> Model {
        let gcd = |mut a: i64, mut b: i64| {
            while b != 0 {
                let r = a % b;
                a = b;
                b = r;
            }
            a
        };
        let cycle = rate.num / gcd(48000 * rate.den, rate.num);
        let frames = cycle * ((rate.num + 5 * rate.den * cycle - 1) / (5 * rate.den * cycle));
        let samples = frames * 48000 * rate.den / rate.num;
        let tc = if drop {
            "01:01:00;12"
        } else if rate.num / rate.den > 100 {
            "01:00:00:006"
        } else {
            "01:00:00:06"
        };
        let label = format!("{}/{}", rate.num, rate.den);
        let v = self.0.join("rate-v.mxf");
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size=320x180:rate={label}"),
                "-frames:v",
                &frames.to_string(),
                "-c:v",
                "dnxhd",
                "-profile:v",
                "dnxhr_lb",
                "-timecode",
                tc,
                "-f",
                "mxf_opatom",
            ])
            .arg(&v))
        .unwrap();
        let mut model =
            parse_model_str(include_str!("../../../../examples/opatom_clip.model.json")).unwrap();
        model.sequences.clear();
        let c = model.clips.get_mut("clip-1").unwrap();
        c.rate = Some(rate);
        c.time_display = Some(i64::from(display_format(rate, drop).unwrap()));
        let video = c.video.as_mut().unwrap();
        video.path = v.to_string_lossy().into();
        video.rate = rate;
        video.width = 320;
        video.height = 180;
        video.duration = frames * ticks_per_unit(rate);
        video.start = Time::from_timecode(tc, ticks_per_unit(rate)).unwrap().ticks;
        let template = c.audio[0].clone();
        c.audio.clear();
        for i in 0..count {
            let p = self.0.join(format!("rate-a{i:02}.mxf"));
            let bits = 24;
            run(Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={}:sample_rate=48000", 400 + i * 37),
                    "-af",
                    &format!("atrim=end_sample={samples}"),
                    "-c:a",
                    &format!("pcm_s{bits}le"),
                    "-timecode",
                    tc,
                    "-mxf_audio_edit_rate",
                    &label,
                    "-f",
                    "mxf_opatom",
                ])
                .arg(&p))
            .unwrap();
            let mut a = template.clone();
            a.path = p.to_string_lossy().into();
            a.bits = bits;
            a.avid_track = Some(i as i64 + 1);
            a.start = video.start;
            a.duration = video.duration;
            c.audio.push(a);
        }
        model
    }
    fn plan(&self, m: &Model) -> R<Plan> {
        Plan::prepare(
            m,
            &self.0.join("output"),
            &[],
            &self.0.join("project.prproj"),
            None,
            Path::new("ffmpeg"),
            Path::new("ffprobe"),
        )
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
#[test]
#[ignore = "requires qualified external FFmpeg/ffprobe 8.1"]
fn native_master_routes_mixed_groups_without_changing_samples_or_sources() {
    let s = Scratch::new();
    let rate = Fraction::new(24000, 1001);
    let mut m = s.rate_fixture(rate, 3, false);
    let c = m.clips.get_mut("clip-1").unwrap();
    c.stereo_pairs = vec![(3, 1)];
    c.audio[0].gain_db = -6.0;
    c.audio[2].gain_db = -6.0;
    c.audio[1].gain_db = -3.0;
    let originals: Vec<_> = std::iter::once(&c.video.as_ref().unwrap().path)
        .chain(c.audio.iter().map(|a| &a.path))
        .map(|p| (p.clone(), fs::read(p).unwrap()))
        .collect();
    let template =
        parse_model_str(include_str!("../../../../examples/opatom_clip.model.json")).unwrap();
    let mut seq = template.sequences[0].clone();
    seq.rate = rate;
    seq.width = 320;
    seq.height = 180;
    seq.markers.clear();
    let e = &mut seq.events[0];
    e.source_in = ticks_per_unit(rate);
    e.record_start = 0;
    e.duration = 3 * ticks_per_unit(rate);
    e.audio_tracks = vec![4];
    e.audio_channels = vec![2];
    e.audio_gains = vec![Some(0.0)];
    e.audio_pans = vec![Some(-25.0)];
    e.stereo = vec![prproj::convert::model::StereoEdit {
        audio_track: 2,
        channels: vec![3, 1],
        gain_db: Some(0.0),
        pans: vec![Some(-100.0), Some(100.0)],
    }];
    m.sequences = vec![seq];
    prproj::convert::native_av::preflight(&m).unwrap();
    let p = s.plan(&m).unwrap();
    let maps = p.execute().unwrap();
    let out = s.0.join("project.prproj");
    prproj::convert::writer::write_native_op1a_project(
        &m,
        &out,
        &mut prproj::SeededUuids::new("native-backend"),
        &maps,
    )
    .unwrap();
    p.record_project_with_mode(&out, "experimental native OP1a AV master")
        .unwrap();
    let (xml, _) = prproj::xml::decompress_prproj(&fs::read(&out).unwrap()).unwrap();
    let xml = String::from_utf8(xml).unwrap();
    assert!(!xml.contains("BE.Sequence.IsMulticam"));
    assert!(!xml.contains("BE.Sequence.IsMergedClip"));
    assert!(xml.contains("<mSourceChannelIndex>2</mSourceChannelIndex>"));
    assert!(!xml.contains("<SingleAudioChannel>"));
    assert!(xml.contains("<OrigChGrp>1</OrigChGrp>"));
    let probe = probe(Path::new("ffprobe"), &s.0.join("output/media-0001.mxf")).unwrap();
    assert_eq!(probe.streams.len(), 4);
    for (path, bytes) in originals {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(fs::read_to_string(s.0.join("output/conversion.json"))
        .unwrap()
        .contains("experimental native OP1a AV master"));
}
#[test]
fn pcm_counter_rejects_truncated_klv_and_counts_payload_bytes() {
    let s = Scratch::new();
    let p = s.0.join("sound.mxf");
    let mut b = vec![
        6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 22, 1, 1, 0, 3, 1, 2, 3,
    ];
    fs::write(&p, &b).unwrap();
    assert_eq!(pcm_essence_bytes(&p).unwrap(), 3);
    b.pop();
    fs::write(&p, &b).unwrap();
    assert!(pcm_essence_bytes(&p).is_err());
}

#[test]
fn private_klv_keys_are_allowed_only_inside_declared_header_metadata() {
    let s = Scratch::new();
    let p = s.0.join("header.mxf");
    let partition = [6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1];
    let mut metadata = partition.to_vec();
    metadata.extend_from_slice(&[5, 1, 0, 8, 0, 0, 0, 0, 0, 0, 0, 18]); // Primer.
    let private = [0x80u8; 16];
    metadata.extend(private);
    metadata.extend_from_slice(&[3, 9, 8, 7]);
    let mut b = partition.to_vec();
    b.extend_from_slice(&[2, 4, 0, 88]);
    let mut pack = [0u8; 88];
    pack[32..40].copy_from_slice(&(metadata.len() as u64).to_be_bytes());
    b.extend(pack);
    b.extend(metadata);
    b.extend_from_slice(&[
        6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 22, 1, 1, 0, 3, 1, 2, 3,
    ]);
    fs::write(&p, &b).unwrap();
    assert_eq!(pcm_essence_bytes(&p).unwrap(), 3);
    // Shorter header declaration must not make a private key outside it acceptable.
    let mut short = b.clone();
    short[17 + 32..17 + 40].copy_from_slice(&25u64.to_be_bytes());
    fs::write(&p, &short).unwrap();
    assert!(pcm_essence_bytes(&p).is_err());
    // No unknown-key scanning/resynchronization after essence or before header.
    let mut trailing = b.clone();
    trailing.extend(private);
    trailing.push(0);
    fs::write(&p, &trailing).unwrap();
    assert!(pcm_essence_bytes(&p).is_err());
    let mut runin = vec![0x80; 17];
    runin.extend(&b);
    fs::write(&p, &runin).unwrap();
    assert!(pcm_essence_bytes(&p).is_err());
    // Valid key/BER framing cannot legitimize a length past EOF.
    b.pop();
    fs::write(&p, &b).unwrap();
    assert!(pcm_essence_bytes(&p).is_err());
}

#[cfg(unix)]
#[test]
fn a_packet_bearing_data_stream_cannot_hide_behind_reference_tags() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new();
    let tool = s.0.join("probe");
    let raw = format!(
        r#"{{"format":{{"format_name":"mxf","tags":{{"operational_pattern_ul":"{}"}}}},"streams":[
      {{"index":0,"codec_type":"data","tags":{{"data_type":"video","file_package_umid":"0x{}"}}}},
      {{"index":1,"codec_type":"audio","codec_name":"pcm_s24le","time_base":"1/48000","duration_ts":9600,"nb_read_packets":5,
        "tags":{{"file_package_umid":"0x{}","timecode":"01:00:00:00"}}}}]}}"#,
        super::backend::OP_ATOM,
        "11".repeat(32),
        "22".repeat(32)
    );
    // A controlled backend lies by omitting the data-stream count in its summary.
    // The second actual-packet scan must catch the contradiction before reading MXF bytes.
    fs::write(&tool, format!("#!/bin/sh\ncase \"$*\" in\n *-show_packets*) printf '0\\n' ;;\n *) cat <<'JSON'\n{raw}\nJSON\n ;;\nesac\n")).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    let error = probe(&tool, &s.0.join("not-opened.mxf")).unwrap_err();
    assert!(error.contains("ancillary data has packets"), "{error}");
}
#[test]
fn exact_timecode_and_invalid_origins() {
    assert_eq!(
        timecode(TPS * 3600, Fraction::new(24, 1), false).unwrap(),
        "01:00:00:00"
    );
    assert!(timecode(-1, Fraction::new(24, 1), false).is_err());
    assert!(timecode(1, Fraction::new(24, 1), false).is_err());
    assert!(timecode(TPS * 86400, Fraction::new(24, 1), false).is_err());
}
#[test]
fn bounded_backend_output_drains_but_caps_memory() {
    let (v, large) = bounded(&b"123456"[..], 3).unwrap();
    assert_eq!(v, b"123");
    assert!(large);
}
#[test]
fn review_shows_actual_preflight_values_without_writing() {
    let s = Scratch::new();
    let plan = Plan {
        mono: vec![],
        destination: s.0.join("new media"),
        inputs: vec![],
        jobs: vec![],
        ffmpeg: "ffmpeg".into(),
        ffprobe: "ffprobe".into(),
        versions: vec![],
        estimate: 3 << 30,
        free: 5 << 30,
    };
    let text = plan.review_text();
    assert!(text.contains("Creates new media."));
    assert!(text.contains(plan.destination.to_str().unwrap()));
    assert!(text.contains("3.00 GiB (3221225472 bytes)"));
    assert!(text.contains("5.00 GiB (5368709120 bytes)"));
    assert!(text.contains("Actual size may differ"));
    assert!(!plan.destination.exists());
}
#[test]
fn existing_destination_is_refused_before_backend_or_inputs() {
    let s = Scratch::new();
    fs::create_dir(s.0.join("output")).unwrap();
    let m = parse_model_str(include_str!("../../../../examples/opatom_clip.model.json")).unwrap();
    assert!(s.plan(&m).err().unwrap().contains("already exists"));
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn real_two_mono_stream_copy_dry_run_refusals_and_project_binding() {
    let s = Scratch::new();
    let m = s.fixture();
    let originals: Vec<_> = m.clips["clip-1"]
        .audio
        .iter()
        .map(|a| fs::read(&a.path).unwrap())
        .collect();
    let p = s.plan(&m).unwrap();
    p.describe();
    assert!(!s.0.join("output").exists());
    let maps = p.execute().unwrap();
    assert_eq!(maps.len(), 3);
    assert_eq!(maps[2].audio_stream, Some(1));
    assert!(p.execute().is_err());
    let probe = probe(Path::new("ffprobe"), &s.0.join("output/media-0001.mxf")).unwrap();
    assert_eq!(probe.streams.len(), 3);
    assert_eq!(probe.tc.as_deref(), Some("01:00:00:00"));
    assert!(s.0.join("output/manifest.json").is_file());
    let mut ids = prproj::SeededUuids::new("op1a");
    prproj::convert::writer::write_project_with_media(
        &m,
        s.0.join("project.prproj"),
        &mut ids,
        &maps,
    )
    .unwrap();
    p.record_project(&s.0.join("project.prproj")).unwrap();
    for (a, b) in m.clips["clip-1"].audio.iter().zip(originals) {
        assert_eq!(fs::read(&a.path).unwrap(), b);
    }
    let mut bad = m.clone();
    bad.clips.get_mut("clip-1").unwrap().audio[0].duration -= 1;
    assert!(Plan::prepare(
        &bad,
        &s.0.join("bad"),
        &[],
        &s.0.join("x.prproj"),
        None,
        Path::new("ffmpeg"),
        Path::new("ffprobe")
    )
    .is_err());
    assert!(!s.0.join("bad").exists());
    assert!(Plan::prepare(
        &m,
        &s.0.join("nested"),
        &[s.0.to_string_lossy().into()],
        &s.0.join("y.prproj"),
        None,
        Path::new("ffmpeg"),
        Path::new("ffprobe")
    )
    .is_err());
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn late_collision_and_changed_input_do_not_replace_files() {
    let collision = Scratch::new();
    let cm = collision.fixture();
    let cp = collision.plan(&cm).unwrap();
    fs::create_dir(&cp.destination).unwrap();
    let protected = cp.destination.join("media-0001.mxf");
    fs::write(&protected, b"late producer bytes").unwrap();
    assert!(cp.execute_inner().is_err());
    assert_eq!(fs::read(&protected).unwrap(), b"late producer bytes");
    assert!(!cp.destination.join("manifest.json").exists());
    let s = Scratch::new();
    let m = s.fixture();
    let p = s.plan(&m).unwrap();
    fs::write(&p.inputs[0].path, b"changed test-owned bytes").unwrap();
    assert!(p.execute().is_err());
    assert!(!s.0.join("output/media-0001.mxf").exists());
    assert!(s.0.join("output/failed.json").exists());
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn qualified_profiles_mono16_video_only_and_padding_refusal() {
    for profile in ["dnxhr_sq", "dnxhr_hq", "dnxhd"] {
        let s = Scratch::new();
        let mut m = s.fixture();
        let is_hd = profile == "dnxhd";
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            if is_hd {
                "testsrc2=size=1920x1080:rate=24"
            } else {
                "testsrc2=size=320x180:rate=24"
            },
            "-frames:v",
            "12",
            "-threads",
            "1",
            "-c:v",
            "dnxhd",
        ]);
        if is_hd {
            cmd.args(["-b:v", "36M"]);
        } else {
            cmd.args(["-profile:v", profile]);
        }
        cmd.args(["-timecode", "01:00:00:00", "-f", "mxf_opatom"])
            .arg(&m.clips["clip-1"].video.as_ref().unwrap().path);
        run(&mut cmd).unwrap();
        let c = m.clips.get_mut("clip-1").unwrap();
        if is_hd {
            let v = c.video.as_mut().unwrap();
            v.width = 1920;
            v.height = 1080;
        }
        c.audio.truncate(1);
        c.audio[0].bits = 16;
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=0.5",
                "-c:a",
                "pcm_s16le",
                "-timecode",
                "01:00:00:00",
                "-mxf_audio_edit_rate",
                "24",
                "-f",
                "mxf_opatom",
            ])
            .arg(&c.audio[0].path))
        .unwrap();
        assert_eq!(s.plan(&m).unwrap().execute().unwrap().len(), 2);
        m.clips.get_mut("clip-1").unwrap().audio.clear();
        let video = Plan::prepare(
            &m,
            &s.0.join("video-only"),
            &[],
            &s.0.join("v.prproj"),
            None,
            Path::new("ffmpeg"),
            Path::new("ffprobe"),
        )
        .unwrap();
        assert_eq!(video.execute().unwrap().len(), 1);
    }
    let s = Scratch::new();
    let m = s.fixture();
    // A non-integral 25fps audio edit span pads this clip-wrapped payload.
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=0.5",
            "-c:a",
            "pcm_s24le",
            "-timecode",
            "01:00:00:00",
            "-mxf_audio_edit_rate",
            "25",
            "-f",
            "mxf_opatom",
        ])
        .arg(&m.clips["clip-1"].audio[0].path))
    .unwrap();
    let error = s.plan(&m).err().unwrap();
    assert!(
        error.contains("physical sample span") || error.contains("unequal source spans"),
        "{error}"
    );
    assert!(!s.0.join("output").exists());
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn all_mxf_rates_preserve_exact_clocks_and_uniform_pcm() {
    for &(n, d) in prproj::convert::rewrap::RATES {
        let s = Scratch::new();
        let rate = Fraction::new(n, d);
        let mut m = s.rate_fixture(rate, 2, false);
        // Exercise the rounded reader clock correction, including nonzero subsecond TC.
        let expected = m.clips["clip-1"].video.as_ref().unwrap().start;
        {
            let c = m.clips.get_mut("clip-1").unwrap();
            c.video.as_mut().unwrap().start = 0;
            for a in &mut c.audio {
                a.start = 0;
            }
        }
        assert!(!normalize_source_clocks(&mut m).unwrap().is_empty());
        assert_eq!(m.clips["clip-1"].video.as_ref().unwrap().start, expected);
        let (graph, _) = bridge::to_graph(&m).unwrap();
        let (roundtrip, _) = bridge::to_model(&graph, &m.options).unwrap();
        assert_eq!(m, roundtrip, "exact clock graph roundtrip {rate}");
        let p = s.plan(&m).unwrap_or_else(|e| panic!("rate {rate}: {e}"));
        let maps = p.execute().unwrap_or_else(|e| panic!("rate {rate}: {e}"));
        let out = probe(Path::new("ffprobe"), &s.0.join("output/media-0001.mxf")).unwrap();
        assert_eq!(out.streams[0].rate, format!("{n}/{d}"));
        let mut ids = prproj::SeededUuids::new("all-rates");
        prproj::convert::writer::write_project_with_media(
            &m,
            s.0.join("p.prproj"),
            &mut ids,
            &maps,
        )
        .unwrap();
    }
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn drop_frame_and_32_discrete_sources_keep_stream_routing() {
    for (n, d, count) in [
        (30000, 1001, 32),
        (60000, 1001, 2),
        (90000, 1001, 2),
        (120000, 1001, 2),
    ] {
        let s = Scratch::new();
        let mut m = s.rate_fixture(Fraction::new(n, d), count, true);
        m.clips.get_mut("clip-1").unwrap().audio.reverse();
        if count == 32 {
            m.clips.get_mut("clip-1").unwrap().stereo_pairs = vec![(3, 1)];
        }
        normalize_source_clocks(&mut m).unwrap();
        let p = s.plan(&m).unwrap();
        let maps = p.execute().unwrap();
        assert_eq!(maps.len(), count + 1);
        if count == 32 {
            let receipt =
                json::parse(&fs::read_to_string(s.0.join("output/media-0001.json")).unwrap())
                    .unwrap();
            let Some(Value::Arr(masters)) = receipt.get("masters") else {
                panic!("missing logical masters")
            };
            assert_eq!(
                masters[0].get("stereo_pairs"),
                Some(&Value::Arr(vec![Value::Arr(vec![
                    Value::Int(3),
                    Value::Int(1)
                ])]))
            );
            let Some(Value::Arr(map)) = masters[0].get("audio_channel_map") else {
                panic!("missing channel map")
            };
            assert_eq!(integer(&map[0], "avid_track").unwrap(), 1);
            assert_eq!(integer(&map[2], "avid_track").unwrap(), 3);
            assert_eq!(integer(&map[2], "output_audio_ordinal").unwrap(), 2);
            assert_eq!(integer(&map[2], "output_stream_index").unwrap(), 3);
        }
        let c = &m.clips["clip-1"];
        for a in &c.audio {
            assert_eq!(
                maps.iter()
                    .find(|b| b.source_path == a.path)
                    .unwrap()
                    .audio_stream,
                Some((a.avid_track.unwrap() - 1) as u32)
            );
        }
        let mut ids = prproj::SeededUuids::new("wide-DF");
        prproj::convert::writer::write_project_with_media(
            &m,
            s.0.join("p.prproj"),
            &mut ids,
            &maps,
        )
        .unwrap();
    }
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn unsupported_audio_and_rate_refuse_before_media_creation() {
    let s = Scratch::new();
    let m = s.rate_fixture(Fraction::new(24000, 1001), 2, false);
    let mut mixed = m.clone();
    mixed.clips.get_mut("clip-1").unwrap().audio[1].bits = 16;
    assert!(s
        .plan(&mixed)
        .err()
        .unwrap()
        .contains("mixed PCM bit depths"));
    assert!(!s.0.join("output").exists());
    for (sample, bits) in [(44100, 16), (96000, 24), (48000, 32)] {
        let mut bad = m.clone();
        let a = &mut bad.clips.get_mut("clip-1").unwrap().audio[0];
        a.sample_rate = sample;
        a.bits = bits;
        assert!(s.plan(&bad).is_err());
        assert!(!s.0.join("output").exists());
    }
    let mut bad = m.clone();
    bad.clips
        .get_mut("clip-1")
        .unwrap()
        .video
        .as_mut()
        .unwrap()
        .rate = Fraction::new(27, 1);
    assert!(s.plan(&bad).is_err());
    assert!(!s.0.join("output").exists());
    let mut bad = m.clone();
    let c = bad.clips.get_mut("clip-1").unwrap();
    while c.audio.len() < 33 {
        c.audio.push(c.audio[0].clone());
    }
    assert!(s.plan(&bad).is_err());
    assert!(!s.0.join("output").exists());
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn dnxhd_common_rate_profiles_copy_without_clock_changes() {
    for (n, d, bitrate) in [
        (24000, 1001, "36M"),
        (24, 1, "36M"),
        (25, 1, "36M"),
        (30000, 1001, "45M"),
        (30, 1, "45M"),
        (50, 1, "90M"),
        (60000, 1001, "90M"),
        (60, 1, "90M"),
    ] {
        let s = Scratch::new();
        let rate = Fraction::new(n, d);
        let mut m = s.rate_fixture(rate, 2, false);
        let v = m.clips.get_mut("clip-1").unwrap().video.as_mut().unwrap();
        v.width = 1920;
        v.height = 1080;
        let frames = v.duration / ticks_per_unit(rate);
        let tc = timecode(v.start, rate, false).unwrap();
        // Replace only this test's freshly generated synthetic picture source.
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=size=1920x1080:rate={n}/{d}"),
                "-frames:v",
                &frames.to_string(),
                "-c:v",
                "dnxhd",
                "-threads:v",
                "1",
                "-b:v",
                bitrate,
                "-timecode",
                &tc,
                "-f",
                "mxf_opatom",
            ])
            .arg(&v.path))
        .unwrap();
        s.plan(&m).unwrap().execute().unwrap();
    }
}
#[test]
fn fractional_drop_and_three_digit_frame_labels_roundtrip_exactly() {
    for (rate, label, drop) in [
        (Fraction::new(24000, 1001), "01:00:00:06", false),
        (Fraction::new(30000, 1001), "01:01:00;02", true),
        (Fraction::new(60000, 1001), "01:01:00;04", true),
        (Fraction::new(120000, 1001), "01:00:00:119", false),
    ] {
        let ticks = Time::from_timecode(label, ticks_per_unit(rate))
            .unwrap()
            .ticks;
        assert_eq!(timecode(ticks, rate, drop).unwrap(), label);
    }
    assert!(timecode(0, Fraction::new(24000, 1001), true).is_err());
}
#[test]
fn unsupported_output_timeline_refuses_before_source_access() {
    let s = Scratch::new();
    let mut m =
        parse_model_str(include_str!("../../../../examples/opatom_clip.model.json")).unwrap();
    m.sequences[0].rate = Fraction::new(27, 1);
    assert!(s
        .plan(&m)
        .err()
        .unwrap()
        .contains("output timeline rate/raster"));
    assert!(!s.0.join("output").exists());
    m.sequences[0].rate = Fraction::new(24, 1);
    m.sequences[0].events[0].audio_tracks.push(5);
    assert!(s
        .plan(&m)
        .err()
        .unwrap()
        .contains("timeline layout exceeds"));
    assert!(!s.0.join("output").exists());
}
#[test]
fn bin_grouping_refuses_unrepresented_or_ambiguous_layouts() {
    use avid_reader::extract::{BinContents, Essence, MasterClip, MediaTrack, KIND_SOUND};
    use avid_reader::io::{MobId, Uuid};
    let id = MobId {
        label: [0; 12],
        length: 0x13,
        instance: [0; 3],
        material: Uuid {
            data1: 1,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
    };
    let master = MasterClip {
        name: "Synthetic channel grouping".into(),
        mob_id: id,
        edit_rate: 24.0,
        length: 24,
        tracks: (1..=3)
            .map(|i| MediaTrack {
                kind: KIND_SOUND,
                index: i,
                file_mob: id,
                edit_rate: 48000.0,
                length: 48000,
                essence: Some(Essence::Sound {
                    sample_rate: 48000.0,
                    bits: 24,
                    channels: 1,
                }),
                offset: 0,
                clip_length: 24,
                gain_db: None,
                pan_raw: None,
            })
            .collect(),
        columns: vec![],
        color: None,
        start: None,
        aux_timecodes: vec![],
        source_file: None,
        source_path: None,
        bin: 0,
        stereo_pairs: vec![],
        other_groups: vec![],
        output_mix: Default::default(),
        markers: vec![],
    };
    let mut bin = BinContents {
        masters: vec![master],
        ..Default::default()
    };
    validate_bin_audio(&bin).unwrap(); // Three unpaired mono sources are not guessed stereo.
    bin.masters[0].stereo_pairs = vec![(3, 1)];
    validate_bin_audio(&bin).unwrap();
    assert_eq!(bin.masters[0].stereo_pairs, vec![(3, 1)]); // Order is semantic.
    for raw in ["51:A1A2A3A4A5A6,", "ST:A1A2A3,"] {
        let (pairs, other) = avid_reader::extract::parse_channel_groups(raw);
        bin.masters[0].stereo_pairs = pairs;
        bin.masters[0].other_groups = other;
        let error=validate_bin_audio(&bin).unwrap_err();
        assert!(error.contains(if raw.starts_with("51:") {"missing or overlapping multichannel members"}else{"unsupported bin group"}),"{error}");
    }
    bin.masters[0].other_groups.clear();
    for pairs in [vec![(1, 1)], vec![(1, 4)], vec![(1, 2), (2, 3)]] {
        bin.masters[0].stereo_pairs = pairs;
        assert!(validate_bin_audio(&bin).is_err());
    }
}
#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; synthetic media only"]
fn sparse_bin_track_labels_are_not_container_stream_indices() {
    let s = Scratch::new();
    let mut m = s.rate_fixture(Fraction::new(24000, 1001), 2, false);
    let c = m.clips.get_mut("clip-1").unwrap();
    c.audio[0].avid_track = Some(7);
    c.audio[1].avid_track = Some(3);
    c.stereo_pairs = vec![(7, 3)];
    let left = c.audio[0].path.clone();
    let right = c.audio[1].path.clone();
    let maps = s.plan(&m).unwrap().execute().unwrap();
    assert_eq!(
        maps.iter()
            .find(|b| b.source_path == left)
            .unwrap()
            .audio_stream,
        Some(1)
    );
    assert_eq!(
        maps.iter()
            .find(|b| b.source_path == right)
            .unwrap()
            .audio_stream,
        Some(0)
    );
    let receipt =
        json::parse(&fs::read_to_string(s.0.join("output/media-0001.json")).unwrap()).unwrap();
    let Some(Value::Arr(masters)) = receipt.get("masters") else {
        panic!("masters")
    };
    assert_eq!(
        masters[0].get("stereo_pairs"),
        Some(&Value::Arr(vec![Value::Arr(vec![
            Value::Int(7),
            Value::Int(3)
        ])]))
    );
    let mut ids = prproj::SeededUuids::new("sparse-pair");
    prproj::convert::writer::write_project_with_media(&m, s.0.join("p.prproj"), &mut ids, &maps)
        .unwrap();
}

// Modify generated test media only. Preserve BER widths, footer span, partition
// pointers and RIP; these controls are not Avid-saved fixtures.
fn generated_terminal(path: &Path, frames: usize, tail: &[u8]) {
    #[derive(Clone)]
    struct Row {
        key: Vec<u8>,
        width: usize,
        value: Vec<u8>,
        at: usize,
    }
    impl Row {
        fn encode(&self) -> Vec<u8> {
            let mut b = self.key.clone();
            if self.width == 0 {
                assert!(self.value.len() < 128);
                b.push(self.value.len() as u8);
            } else {
                b.push(0x80 | self.width as u8);
                b.extend(&(self.value.len() as u64).to_be_bytes()[8 - self.width..]);
            }
            b.extend(&self.value);
            b
        }
        fn partition(&self) -> bool {
            self.key[..13] == [6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1]
                && (2..=4).contains(&self.key[13])
        }
    }
    let original = fs::read(path).unwrap();
    let mut rows = Vec::new();
    let mut at = 0;
    while at < original.len() {
        let key = original[at..at + 16].to_vec();
        let first = original[at + 16];
        let width = if first < 128 {
            0
        } else {
            (first & 127) as usize
        };
        let start = at + 17 + width;
        let size = if width == 0 {
            first as usize
        } else {
            original[at + 17..start]
                .iter()
                .fold(0usize, |a, x| (a << 8) | *x as usize)
        };
        rows.push(Row {
            key,
            width,
            value: original[start..start + size].to_vec(),
            at,
        });
        at = start + size;
    }
    let e = rows
        .iter_mut()
        .find(|r| r.key[..12] == [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1])
        .unwrap();
    assert_eq!(e.value.len(), frames * 188416);
    e.value.extend(tail);
    let idx = rows
        .iter()
        .position(|r| r.key == [6, 14, 43, 52, 2, 83, 1, 1, 13, 1, 2, 1, 1, 16, 1, 0])
        .unwrap();
    let old = rows[idx].value.len();
    let mut local = Vec::new();
    let mut p = 0;
    while p < old {
        let tag = u16::from_be_bytes(rows[idx].value[p..p + 2].try_into().unwrap());
        let size = u16::from_be_bytes(rows[idx].value[p + 2..p + 4].try_into().unwrap()) as usize;
        p += 4;
        let value = match tag {
            0x3f05 => vec![0; 4],
            0x3f0d => (frames as u64 + 1).to_be_bytes().to_vec(),
            _ => rows[idx].value[p..p + size].to_vec(),
        };
        assert_ne!(tag, 0x3f0a);
        local.extend(tag.to_be_bytes());
        local.extend((value.len() as u16).to_be_bytes());
        local.extend(value);
        p += size;
    }
    let mut array = Vec::from((frames as u32 + 1).to_be_bytes());
    array.extend(11u32.to_be_bytes());
    for i in 0..=frames {
        array.extend([0, 0, 128]);
        array.extend((i as u64 * 188416).to_be_bytes());
    }
    local.extend(0x3f0au16.to_be_bytes());
    local.extend((array.len() as u16).to_be_bytes());
    local.extend(array);
    let extra = local.len() - old;
    rows[idx].value = local;
    let filler = &mut rows[idx + 1];
    assert_eq!(filler.key[8..13], [3, 1, 2, 16, 1]);
    let len = filler.value.len();
    assert!(len > extra);
    filler.value.truncate(len - extra);
    let mut at = 0;
    for r in &mut rows {
        r.at = at;
        at += r.encode().len();
    }
    let parts: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.partition())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(parts.len(), 3);
    let footer = rows[*parts.last().unwrap()].at as u64;
    let rip = rows
        .iter()
        .position(|r| r.key[..15] == [6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1, 17, 1])
        .unwrap();
    for (n, i) in parts.iter().enumerate() {
        let previous = if n == 0 {
            0
        } else {
            rows[parts[n - 1]].at as u64
        };
        let offset = rows[*i].at as u64;
        let r = &mut rows[*i];
        assert_eq!(offset % 512, 0);
        r.value[8..16].copy_from_slice(&offset.to_be_bytes());
        r.value[16..24].copy_from_slice(&previous.to_be_bytes());
        r.value[24..32].copy_from_slice(&footer.to_be_bytes());
    }
    let index_bytes = (rows[rip].at - rows[idx].at) as u64;
    rows[*parts.last().unwrap()].value[40..48].copy_from_slice(&index_bytes.to_be_bytes());
    let rip_size = rows[rip].encode().len() as u32;
    let mut value = Vec::new();
    for i in parts {
        value.extend(&rows[i].value[60..64]);
        value.extend((rows[i].at as u64).to_be_bytes());
    }
    value.extend(rip_size.to_be_bytes());
    rows[rip].value = value;
    let bytes: Vec<u8> = rows.iter().flat_map(Row::encode).collect();
    fs::write(path, bytes).unwrap();
}

#[test]
#[ignore = "requires FFmpeg/ffprobe8.1; generated media only"]
fn zero_tail_roundtrip_keeps_coded_pictures_audio_and_refuses_real_extra_data() {
    let scratch = Scratch::new();
    let rate = Fraction::new(24000, 1001);
    let mut model = scratch.rate_fixture(rate, 2, false);
    let video = model
        .clips
        .get_mut("clip-1")
        .unwrap()
        .video
        .as_mut()
        .unwrap();
    let old = PathBuf::from(&video.path);
    let p = scratch.0.join("scope1080.mxf");
    let frames = (video.duration / ticks_per_unit(rate)) as usize;
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1920x1080:rate=24000/1001",
            "-frames:v",
            &frames.to_string(),
            "-c:v",
            "dnxhd",
            "-profile:v",
            "dnxhr_lb",
            "-timecode",
            "01:00:00:06",
            "-f",
            "mxf_opatom",
        ])
        .arg(&p))
    .unwrap();
    video.path = p.to_string_lossy().into();
    video.width = 1920;
    video.height = 1080;
    let original = fs::read(&p).unwrap();
    let _baseline = scratch.plan(&model).unwrap();
    let hash = |p: &Path| {
        run(Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(p)
            .args([
                "-map",
                "0:v:0",
                "-c",
                "copy",
                "-f",
                "streamhash",
                "-hash",
                "sha256",
                "-",
            ]))
        .unwrap()
    };
    let expected = hash(&p);
    generated_terminal(&p, frames, &vec![0; 512]);
    let with_tail = fs::read(&p).unwrap();
    let plan = scratch.plan(&model).unwrap();
    assert!(plan.review_text().contains("verified zero-only terminal"));
    assert!(plan.inputs[0].terminal.is_some());
    plan.execute().unwrap();
    let output = scratch.0.join("output/media-0001.mxf");
    assert_eq!(hash(&output), expected);
    assert_eq!(fs::read(&p).unwrap(), with_tail);
    let receipt = fs::read_to_string(scratch.0.join("output/media-0001.json")).unwrap();
    assert!(receipt.contains("terminal_exclusion") && receipt.contains("copied_essence_sha256"));
    for (n, tail) in [vec![1; 512], Vec::new()].into_iter().enumerate() {
        // The valid extra frame is taken from the picture KLV, not the file header.
        fs::write(&p, &original).unwrap();
        let extra = if n == 1 {
            let layout = super::source::layout(&p).unwrap();
            let e = &layout.essence[0];
            original[e.offset as usize..e.offset as usize + 188416].to_vec()
        } else {
            tail
        };
        generated_terminal(&p, frames, &extra);
        let result = Plan::prepare(
            &model,
            &scratch.0.join(format!("refused-{n}")),
            &[],
            &scratch.0.join("p.prproj"),
            None,
            Path::new("ffmpeg"),
            Path::new("ffprobe"),
        );
        assert!(result.is_err());
        assert!(!scratch.0.join(format!("refused-{n}")).exists());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&p, &with_tail).unwrap();
        let fake = scratch.0.join("without-filter");
        fs::write(&fake,"#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then echo 'ffmpeg version 8.1 scoped-test'; fi\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let missing = Plan::prepare(
            &model,
            &scratch.0.join("missing-filter"),
            &[],
            &scratch.0.join("p.prproj"),
            None,
            &fake,
            Path::new("ffprobe"),
        );
        let error = missing.err().unwrap();
        assert!(error.contains("filter unavailable"), "{error}");
        assert!(!scratch.0.join("missing-filter").exists());
    }
    // A changed source after preflight must fail before writing any new MXF.
    fs::write(&p, &with_tail).unwrap();
    let stale = Plan::prepare(
        &model,
        &scratch.0.join("stale"),
        &[],
        &scratch.0.join("p.prproj"),
        None,
        Path::new("ffmpeg"),
        Path::new("ffprobe"),
    )
    .unwrap();
    fs::write(&p, &original).unwrap();
    assert!(stale.execute().is_err());
    assert!(!scratch.0.join("stale/.partial-0001.mxf").exists());
    assert!(old.exists());
}

#[test]
#[ignore = "requires qualified external FFmpeg/ffprobe 8.1"]
fn multichannel_dnx_op1a_and_prores_mov_preserve_all_pcm_lanes() {
    let root=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let scratch=Scratch::new();
    let parent=std::env::var_os("ANYNLE_MULTICHANNEL_OUTPUT").map(PathBuf::from).unwrap_or_else(||scratch.0.clone());
    let fixtures=if let Some(path)=std::env::var_os("ANYNLE_MULTICHANNEL_FIXTURES") {PathBuf::from(path)}else{
        let fixtures=parent.join("generated");
        let generated=Command::new("python3").arg(root.join("tests/multichannel_media_cases.py")).arg(&fixtures)
            .env("PYTHONPATH",format!("{}:{}:{}",root.join("src").display(),root.join("vendor").display(),root.join("tests").display())).output().unwrap();
        assert!(generated.status.success(),"{}",String::from_utf8_lossy(&generated.stderr));fixtures
    };
    let Value::Arr(cases)=json::parse(&fs::read_to_string(fixtures.join("cases.json")).unwrap()).unwrap()else{panic!()};
    assert_eq!(cases.len(),15);
    for path in cases {
        let Value::Str(path)=path else{panic!()};let model=prproj::convert::model::load_model(Path::new(&path)).unwrap();
        let name=&model.sequences[0].name;let destination=parent.join(name);let project=parent.join(format!("{name}.prproj"));
        let plan=Plan::prepare_media(&model,&destination,&[],&project,None,Path::new("ffmpeg"),Path::new("ffprobe"),true).unwrap();
        let bindings=plan.bindings();
        let prepared=prproj::convert::writer::prepare_project(&model,&project,&mut prproj::SeededUuids::new(&format!("multichannel-media-{name}")),&bindings,true).unwrap();
        assert!(!destination.exists() && !project.exists());
        assert_eq!(plan.execute().unwrap(),bindings);
        let record=json::parse(&fs::read_to_string(destination.join("media-0001.json")).unwrap()).unwrap();
        let Value::Arr(masters)=record.get("masters").unwrap() else{panic!()};
        let Value::Arr(routing)=masters[0].get("premiere_channel_routing").unwrap() else{panic!()};
        let c=model.clips.values().next().unwrap();
        assert_eq!(routing.len(),c.multichannel_groups.len());
        if let Some(group)=c.multichannel_groups.first() {
            let labels=group.premiere_channels();
            assert_eq!(routing[0].get("destination_source_labels"),Some(&Value::Arr(labels.iter().map(|n|Value::Int(*n)).collect())));
            assert_eq!(routing[0].get("destination_physical_ordinals"),Some(&Value::Arr(labels.iter().map(|n|Value::Int(n-1)).collect())));
            assert_eq!(routing[0].get("premiere_speaker_layout"),Some(&if group.format=="5.1" {Value::Str("5.1".into())}else{Value::Null}));
            if group.format=="7.1" {
                assert_eq!(routing[0].get("destination_channel_type"),Some(&Value::Str("Adaptive".into())));
                assert_eq!(routing[0].get("destination_channel_roles"),Some(&Value::Arr(["L","C","R","Lss","Rss","Lsr","Rsr","LFE"].iter().map(|s|Value::Str((*s).into())).collect())));
            }
        }
        prepared.publish().unwrap();
        assert!(project.is_file());
        let ext=if name.starts_with("dnx") {"mxf"}else{"mov"};
        assert!(destination.join(format!("media-0001.{ext}")).is_file());
        assert_eq!(model.clips.values().next().unwrap().audio.len(),if name.ends_with("pairs8") {8}else{name.rsplit('-').next().unwrap().trim_end_matches("ch").parse::<usize>().unwrap()});
        if !model.clips.values().next().unwrap().audio.is_empty() {
        let mut bad=model.clone();bad.clips.get_mut("clip-1").unwrap().audio[0].bits=if name.contains("pcm24") {16}else{24};
        let never=parent.join(format!("refused-{name}"));
        assert!(Plan::prepare_media(&bad,&never,&[],&project,None,Path::new("ffmpeg"),Path::new("ffprobe"),true).is_err());
        assert!(!never.exists());
        }
        let never=parent.join(format!("refused-default-{name}"));
        if name.starts_with("prores") { assert!(Plan::prepare(&model,&never,&[],&project,None,Path::new("ffmpeg"),Path::new("ffprobe")).is_err()); }
    }
}

#[test]
fn mov_lane_admission_refuses_unqualified_width_before_source_access() {
    let scratch=Scratch::new();
    let mut model=parse_model_str(include_str!("../../../../examples/opatom_clip.model.json")).unwrap();
    model.sequences.clear();let c=model.clips.get_mut("clip-1").unwrap();c.video.as_mut().unwrap().codec="apcs".into();
    let audio=c.audio[0].clone();c.audio.clear();
    for n in 1..=9 {let mut a=audio.clone();a.avid_track=Some(n);a.path=format!("/Unavailable/A{n}.mxf");c.audio.push(a);}
    let never=scratch.0.join("never");
    let error=Plan::prepare_media(&model,&never,&[],&scratch.0.join("test.prproj"),None,Path::new("missing-backend"),Path::new("missing-backend"),true).err().unwrap();
    assert!(error.contains("at most eight"),"{error}");assert!(!never.exists());
}
