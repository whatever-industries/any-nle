//! Opt-in rates: exact Rust/Python bytes, source clocks and normal-path refusal controls.
use prproj::convert::{
    model::load_model,
    rewrap::{MediaRelink, RATES},
    writer::{write_project, write_project_with_media},
};
use prproj::SeededUuids;
use std::{path::PathBuf, process::Command};

#[test]
fn all_rates_and_clip_modes_match_python_without_widening_default_acceptance() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tmp = std::env::temp_dir().join(format!(
        "op1a-rate-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&tmp).unwrap();
    let create = r#"import json,sys,math
from pathlib import Path
from fractions import Fraction
from avid2prproj.rewrap import display_format
v=json.loads(Path(sys.argv[1]).read_text());n,d=map(int,sys.argv[3:5]);mode,_,shape=sys.argv[5].partition(':');rate=Fraction(n,d);unit=254016000000*d//n
c=v['clips'][0];c['rate']=f'{n}/{d}';c['time_display']=display_format(rate)
c['video'].update(rate=f'{n}/{d}',duration_ticks=unit*n,start_ticks=unit*(round(rate)*3600+6))
for i,a in enumerate(c['audio']): a.update(duration_ticks=unit*n,start_ticks=c['video']['start_ticks'],avid_track=i+1,bits=24)
if shape:
    base=c['audio'][0].copy();c['audio']=[]
    for track in ([7,3] if shape=='sparse' else [3,1,2]):
        a=base.copy();a.update(path=f'/Synthetic/A{track}.mxf',avid_track=track,bits=24);c['audio'].append(a)
    c['stereo_pairs']=[[7,3]] if shape=='sparse' else [[3,1]]
v['options']['bin_mode']=mode
s=v['sequences'][0];s.update(rate=f'{n}/{d}');s['events'][0].update(source_in_ticks=6*unit,duration_ticks=12*unit);s['markers'][0]['start_ticks']=2*unit
if shape:
    s['events'][0].update(audio_tracks=[] if shape=='sparse' else [2],audio_channels=[] if shape=='sparse' else [2],stereo=[dict(audio_track=1,channels=c['stereo_pairs'][0])])
    s['stereo_audio_tracks']=[1]
Path(sys.argv[2]).write_text(json.dumps(v))
"#;
    let write = r#"import sys,json
from pathlib import Path
from avid2prproj.model import parse_model
from avid2prproj.writer import write_project
from avid2prproj.determinism import seeded_ids
v=json.loads(Path(sys.argv[1]).read_text());c=v['clips'][0];target=sys.argv[3]
m=[dict(source_path=c['video']['path'],target_path=target,audio_stream=None)]+[dict(source_path=a['path'],target_path=target,audio_stream=sorted(x['avid_track'] for x in c['audio']).index(a['avid_track'])) for a in c['audio']]
with seeded_ids('rewrap-rate-parity'): write_project(parse_model(v),Path(sys.argv[2]),media_relinks=m)
"#;
    let python = |code: &str, args: &[String]| {
        let p = Command::new("python3")
            .arg("-c")
            .arg(code)
            .args(args)
            .env(
                "PYTHONPATH",
                format!(
                    "{}:{}",
                    root.join("src").display(),
                    root.join("vendor").display()
                ),
            )
            .env("PYTHONHASHSEED", "0")
            .output()
            .unwrap();
        assert!(p.status.success(), "{}", String::from_utf8_lossy(&p.stderr));
    };
    let cases = RATES
        .iter()
        .map(|&(n, d)| (n, d, ""))
        .chain([(24000, 1001, "mixed"), (24000, 1001, "sparse")]);
    for (n, d, shape) in cases {
        for mode in ["separate", "grouped", "merged"] {
            let fixture = tmp.join("fixture.json");
            let out = tmp.join("out.prproj");
            let target = tmp.join("new.mxf").to_string_lossy().to_string();
            python(
                create,
                &[
                    root.join("examples/opatom_clip.model.json")
                        .display()
                        .to_string(),
                    fixture.display().to_string(),
                    n.to_string(),
                    d.to_string(),
                    if shape.is_empty() {
                        mode.into()
                    } else {
                        format!("{mode}:{shape}")
                    },
                ],
            );
            let model = load_model(&fixture).unwrap();
            let c = &model.clips["clip-1"];
            let mut maps = vec![MediaRelink {
                source_path: c.video.as_ref().unwrap().path.clone(),
                target_path: target.clone(),
                audio_stream: None,
                separate_mono: false,
            }];
            let mut labels: Vec<_> = c.audio.iter().map(|a| a.avid_track.unwrap()).collect();
            labels.sort();
            for a in &c.audio {
                maps.push(MediaRelink {
                    source_path: a.path.clone(),
                    target_path: target.clone(),
                    audio_stream: Some(
                        labels
                            .iter()
                            .position(|x| Some(*x) == a.avid_track)
                            .unwrap() as u32,
                    ),
                    separate_mono: false,
                });
            }
            if (n, d) != (24, 1) {
                let mut ids = SeededUuids::new("default-control");
                assert!(write_project(&model, tmp.join("refused.prproj"), &mut ids).is_err());
                assert!(!tmp.join("refused.prproj").exists());
            }
            let mut ids = SeededUuids::new("rewrap-rate-parity");
            write_project_with_media(&model, &out, &mut ids, &maps)
                .unwrap_or_else(|e| panic!("{n}/{d} {mode}: {e}"));
            let rust = std::fs::read(&out).unwrap();
            std::fs::remove_file(&out).unwrap();
            python(
                write,
                &[
                    fixture.display().to_string(),
                    out.display().to_string(),
                    target,
                ],
            );
            assert_eq!(rust, std::fs::read(&out).unwrap(), "{n}/{d} {mode}");
            std::fs::remove_file(out).unwrap();
        }
    }
    std::fs::remove_dir_all(tmp).unwrap();
}
