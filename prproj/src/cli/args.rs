//! A small `argparse` emulation for the four tools: long options (unique-prefix abbreviations,
//! `--opt VALUE` and `--opt=VALUE`), `-h`, `--`, negative numbers as values, positional runs
//! consumed the way argparse consumes them, and argparse's exit-2 errors. Help text is
//! hand-written (options and behaviour match Python's; wrapping need not).

use std::io::Write;

use super::say;
use crate::convert::model::py_str_repr;
use crate::model::project::py_int;

/// What an option takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `store_true`.
    Flag,
    /// `type=int`.
    Int,
    /// `type=str` / `type=Path`.
    Text,
}

pub struct Opt {
    pub long: &'static str,
    pub kind: Kind,
}

pub struct Spec {
    pub prog: &'static str,
    /// The whole `usage: ...` block.
    pub usage: &'static str,
    /// Everything `--help` prints after the usage block.
    pub help: &'static str,
    pub opts: &'static [Opt],
    /// Positional names and whether each is `nargs="+"`.
    pub positionals: &'static [(&'static str, bool)],
}

#[derive(Default)]
pub struct Parsed {
    flags: Vec<&'static str>,
    values: Vec<(&'static str, String)>,
    /// The arguments each positional collected, in spec order.
    pub positionals: Vec<Vec<String>>,
}

impl Parsed {
    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        self.values.iter().rev().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    /// An `int` option (already validated while parsing).
    pub fn int(&self, name: &str) -> Option<i64> {
        self.text(name).and_then(|v| py_int(v).ok())
    }
}

pub enum Outcome {
    Run(Parsed),
    Exit(i32),
}

enum Class {
    Positional,
    Option(Option<usize>, Option<String>),
    Help(Option<String>),
}

fn is_negative_number(arg: &str) -> bool {
    let Some(rest) = arg.strip_prefix('-') else { return false };
    if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    match rest.split_once('.') {
        Some((int, frac)) => int.bytes().all(|b| b.is_ascii_digit()) && !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

struct Failure(String);

fn classify(spec: &Spec, arg: &str) -> Result<Class, Failure> {
    if arg.is_empty() || !arg.starts_with('-') {
        return Ok(Class::Positional);
    }
    if arg == "-h" || arg == "--help" {
        return Ok(Class::Help(None));
    }
    if let Some(i) = spec.opts.iter().position(|o| o.long == arg) {
        return Ok(Class::Option(Some(i), None));
    }
    if arg.len() == 1 {
        return Ok(Class::Positional);
    }
    if arg.starts_with("--") {
        let (name, explicit) = match arg.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (arg, None),
        };
        if name == "--help" {
            return Ok(Class::Help(explicit));
        }
        if let Some(i) = spec.opts.iter().position(|o| o.long == name) {
            return Ok(Class::Option(Some(i), explicit));
        }
        let mut hits: Vec<(Option<usize>, &str)> = spec
            .opts
            .iter()
            .enumerate()
            .filter(|(_, o)| o.long.starts_with(name))
            .map(|(i, o)| (Some(i), o.long))
            .collect();
        if "--help".starts_with(name) {
            hits.insert(0, (None, "--help"));
        }
        match hits.len() {
            0 => {}
            1 => {
                return Ok(match hits[0].0 {
                    Some(i) => Class::Option(Some(i), explicit),
                    None => Class::Help(explicit),
                })
            }
            _ => {
                let names: Vec<&str> = hits.iter().map(|h| h.1).collect();
                return Err(Failure(format!("ambiguous option: {name} could match {}", names.join(", "))));
            }
        }
    }
    if is_negative_number(arg) || arg.contains(' ') {
        return Ok(Class::Positional);
    }
    Ok(Class::Option(None, None))
}

/// `parser.parse_args(argv)`.
pub fn parse(spec: &Spec, argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Outcome {
    let fail = |err: &mut dyn Write, message: &str| {
        say!(err, "{}", spec.usage);
        say!(err, "{}: error: {message}", spec.prog);
        Outcome::Exit(2)
    };
    // Classify everything first, as argparse does (an ambiguous abbreviation is reported
    // before anything else, even before --help).
    let mut classes = Vec::with_capacity(argv.len());
    let mut after_separator = false;
    for arg in argv {
        if after_separator {
            classes.push(Class::Positional);
        } else if arg == "--" {
            after_separator = true;
            classes.push(Class::Option(None, Some("--".into())));
        } else {
            match classify(spec, arg) {
                Ok(c) => classes.push(c),
                Err(Failure(m)) => return fail(err, &m),
            }
        }
    }
    let mut parsed = Parsed { positionals: vec![Vec::new(); spec.positionals.len()], ..Default::default() };
    let mut next_positional = 0;
    let mut extras: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        match &classes[i] {
            Class::Positional => {
                let mut j = i;
                while j < argv.len() && matches!(classes[j], Class::Positional) {
                    j += 1;
                }
                let mut run = &argv[i..j];
                while !run.is_empty() && next_positional < spec.positionals.len() {
                    let (_, plus) = spec.positionals[next_positional];
                    let take = if plus { run.len() } else { 1 };
                    parsed.positionals[next_positional].extend_from_slice(&run[..take]);
                    run = &run[take..];
                    next_positional += 1;
                }
                extras.extend_from_slice(run);
                i = j;
            }
            Class::Help(explicit) => {
                if let Some(e) = explicit {
                    return fail(err, &format!("argument -h/--help: ignored explicit argument {}", py_str_repr(e)));
                }
                say!(out, "{}\n\n{}", spec.usage, spec.help);
                return Outcome::Exit(0);
            }
            Class::Option(None, Some(sep)) if sep == "--" => i += 1,
            Class::Option(None, _) => {
                extras.push(argv[i].clone());
                i += 1;
            }
            Class::Option(Some(index), explicit) => {
                let opt = &spec.opts[*index];
                i += 1;
                if opt.kind == Kind::Flag {
                    if let Some(e) = explicit {
                        return fail(err, &format!("argument {}: ignored explicit argument {}", opt.long, py_str_repr(e)));
                    }
                    parsed.flags.push(opt.long);
                    continue;
                }
                let value = match explicit {
                    Some(v) => v.clone(),
                    None => match classes.get(i) {
                        Some(Class::Positional) => {
                            i += 1;
                            argv[i - 1].clone()
                        }
                        _ => return fail(err, &format!("argument {}: expected one argument", opt.long)),
                    },
                };
                if opt.kind == Kind::Int && py_int(&value).is_err() {
                    return fail(err, &format!("argument {}: invalid int value: {}", opt.long, py_str_repr(&value)));
                }
                parsed.values.push((opt.long, value));
            }
        }
    }
    let missing: Vec<&str> =
        spec.positionals.iter().zip(&parsed.positionals).filter(|(_, got)| got.is_empty()).map(|(p, _)| p.0).collect();
    if !missing.is_empty() {
        return fail(err, &format!("the following arguments are required: {}", missing.join(", ")));
    }
    if !extras.is_empty() {
        return fail(err, &format!("unrecognized arguments: {}", extras.join(" ")));
    }
    Outcome::Run(parsed)
}
