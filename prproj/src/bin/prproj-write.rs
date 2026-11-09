//! prproj-write: write a native Premiere project from a neutral edit model (JSON).
//!
//!     prproj-write MODEL.json OUTPUT.prproj [--bin-mode separate|merged|grouped]
//!                  [--match-avid-default-pan] [--seed SEED]
//!
//! The Rust port of `python -m avid2prproj` (same arguments, same JSON report on stdout, exit
//! status 2 for a refused model, an unverified Premiere shape, an existing output or a missing
//! output folder). `--seed SEED` draws every UUID from the deterministic sequence of the Python
//! writer's `seeded_ids(SEED)`, so the output is byte-identical to Python's under that seed;
//! without it IDs are random (version-4 UUIDs).

use std::path::PathBuf;
use std::process::ExitCode;

use prproj::convert::model::{load_model, BIN_MODES};
use prproj::convert::writer::write_project;
use prproj::model::project::py_path;
use prproj::{RandomUuids, SeededUuids, UuidSource};

const PROG: &str = "prproj-write";
/// argparse's usage, wrapped as argparse wraps it (`--seed` is the Rust-only extra).
const USAGE: &str = "\
usage: prproj-write [-h] [--bin-mode {separate,grouped,merged}]
                    [--match-avid-default-pan] [--seed SEED]
                    model output";

const HELP: &str = "\
prproj-write: write a native Premiere project from a neutral edit model (JSON).

prproj-write MODEL.json OUTPUT.prproj [--bin-mode separate|merged|grouped]

positional arguments:
  model
  output

options:
  -h, --help            show this help message and exit
  --bin-mode {separate,grouped,merged}
                        override the model's bin mode
  --match-avid-default-pan
                        pan mono edits with no explicit Avid pan the way Avid
                        plays them (odd tracks left, even right)
  --seed SEED           deterministic IDs: the UUID sequence of the Python
                        writer's seeded_ids(SEED) (Rust only; default random)";

struct Args {
    model: PathBuf,
    output: PathBuf,
    bin_mode: Option<String>,
    match_avid_default_pan: bool,
    seed: Option<String>,
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("{USAGE}");
    eprintln!("{PROG}: error: {message}");
    ExitCode::from(2)
}

/// argparse-style parsing: long options may be abbreviated to a unique prefix and take their
/// value as `--opt VALUE` or `--opt=VALUE`; `--` ends the options.
fn parse_args(argv: &[String]) -> Result<Args, ExitCode> {
    const OPTIONS: [&str; 4] = ["--bin-mode", "--match-avid-default-pan", "--seed", "--help"];
    let mut positional = Vec::new();
    let (mut bin_mode, mut match_pan, mut seed) = (None, false, None);
    let mut i = 0;
    let mut options_done = false;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if options_done || !arg.starts_with('-') || arg == "-" {
            positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        if arg == "-h" {
            println!("{USAGE}\n\n{HELP}");
            return Err(ExitCode::SUCCESS);
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if arg.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let candidates: Vec<&str> = if OPTIONS.contains(&name) {
            vec![name]
        } else {
            OPTIONS.iter().copied().filter(|o| name.starts_with("--") && name.len() > 2 && o.starts_with(name)).collect()
        };
        let option = match candidates.as_slice() {
            [one] => *one,
            [] => return Err(usage_error(&format!("unrecognized arguments: {arg}"))),
            many => {
                return Err(usage_error(&format!(
                    "ambiguous option: {name} could match {}",
                    many.join(", ")
                )))
            }
        };
        let mut take_value = |inline: Option<String>| -> Result<String, ExitCode> {
            if let Some(v) = inline {
                return Ok(v);
            }
            match argv.get(i) {
                Some(v) if !v.starts_with('-') || v == "-" => {
                    i += 1;
                    Ok(v.clone())
                }
                _ => Err(usage_error(&format!("argument {option}: expected one argument"))),
            }
        };
        match option {
            "--help" => {
                println!("{USAGE}\n\n{HELP}");
                return Err(ExitCode::SUCCESS);
            }
            "--bin-mode" => {
                let v = take_value(inline)?;
                if !BIN_MODES.contains(&v.as_str()) {
                    return Err(usage_error(&format!(
                        "argument --bin-mode: invalid choice: '{v}' (choose from separate, grouped, merged)"
                    )));
                }
                bin_mode = Some(v);
            }
            "--seed" => seed = Some(take_value(inline)?),
            _ => {
                if inline.is_some() {
                    return Err(usage_error(&format!("argument {option}: ignored explicit argument '{}'", inline.unwrap())));
                }
                match_pan = true;
            }
        }
    }
    match positional.len() {
        0 => Err(usage_error("the following arguments are required: model, output")),
        1 => Err(usage_error("the following arguments are required: output")),
        2 => Ok(Args {
            model: PathBuf::from(&positional[0]),
            output: PathBuf::from(&positional[1]),
            bin_mode,
            match_avid_default_pan: match_pan,
            seed,
        }),
        _ => Err(usage_error(&format!("unrecognized arguments: {}", positional[2..].join(" ")))),
    }
}

/// Python `json.dumps(str)` (ensure_ascii).
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Python `json.dumps(list_of_str, indent=1)` at nesting level 1.
fn json_list(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let lines: Vec<String> = items.iter().map(|s| format!("  {}", json_string(s))).collect();
    format!("[\n{}\n ]", lines.join(",\n"))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let mut model = match load_model(&args.model) {
        Ok(m) => m,
        Err(error) => {
            eprintln!("{PROG}: {error}");
            return ExitCode::from(2);
        }
    };
    if let Some(mode) = args.bin_mode {
        model.options.bin_mode = mode;
    }
    if args.match_avid_default_pan {
        model.options.match_avid_default_pan = true;
    }
    let mut seeded;
    let mut random;
    let ids: &mut dyn UuidSource = match &args.seed {
        Some(seed) => {
            seeded = SeededUuids::new(seed.clone());
            &mut seeded
        }
        None => {
            random = RandomUuids::new();
            &mut random
        }
    };
    match write_project(&model, &args.output, ids) {
        Ok(report) => {
            let output = py_path(&args.output.to_string_lossy()).to_string_lossy().into_owned();
            println!(
                "{{\n \"output\": {},\n \"notes\": {},\n \"unverified\": {}\n}}",
                json_string(&output),
                json_list(&report.notes),
                json_list(&report.unverified)
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{PROG}: {error}");
            ExitCode::from(if error.is_reported() { 2 } else { 1 })
        }
    }
}
