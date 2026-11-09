//! `pr-visualize` (`cli/visualize.py`): print the parsed object model, the project-panel item
//! tree and every sequence with its tracks and clips.
//!
//! ```text
//! prproj-visualize file.prproj [--items] [--sequences]
//! ```

use std::io::Write;
use std::path::Path;

use super::args::{self, Kind, Opt, Outcome, Spec};
use super::{path_str, report, say, PyResult};
use crate::build::fmt::fmt_f;
use crate::build::time::{round_half_even, Time, TICKS_PER_SECOND};
use crate::model::application;
use crate::model::{ItemId, Project, SequenceId};
use crate::pynum::true_div;

pub const SPEC: Spec = Spec {
    prog: "prproj-visualize",
    usage: "usage: prproj-visualize [-h] [--items] [--sequences] file",
    help: "Visualize the parsed object model of a .prproj file.

positional arguments:
  file         prproj file to visualize

options:
  -h, --help   show this help message and exit
  --items      Item tree only
  --sequences  Sequences only",
    opts: &[Opt { long: "--items", kind: Kind::Flag }, Opt { long: "--sequences", kind: Kind::Flag }],
    positionals: &[("file", false)],
};

fn or_default(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn print_item(out: &mut dyn Write, p: &Project, item: ItemId, depth: usize) {
    let detail = p.item_media_path(item).map_or(String::new(), |m| format!("  ({m})"));
    say!(out, "{}{} [{}]{detail}", "  ".repeat(depth), or_default(p.item_name(item), "(unnamed)"), p.item_type(item).name());
    for &child in p.children(item) {
        print_item(out, p, child, depth + 1);
    }
}

fn secs(t: Time) -> f64 {
    t.seconds()
}

fn print_sequence(out: &mut dyn Write, p: &Project, sequence: SequenceId) -> PyResult<()> {
    let mut details = Vec::new();
    if let Some((w, h)) = p.frame_size(sequence) {
        details.push(format!("{w}x{h}"));
    }
    if let Some(timebase) = p.timebase(sequence).filter(|&t| t != 0) {
        details.push(format!("{} fps", fmt_f(true_div(TICKS_PER_SECOND as i128, timebase as i128), 3)));
    }
    if let Some(rate) = p.audio_frame_rate(sequence).filter(|&r| r != 0) {
        details.push(format!("{} Hz", round_half_even(true_div(TICKS_PER_SECOND as i128, rate as i128))?));
    }
    let suffix = if details.is_empty() { String::new() } else { format!(" ({})", details.join(", ")) };
    say!(out, "\n{}{suffix}", or_default(p.sequence_name(sequence), "(unnamed sequence)"));
    for (prefix, tracks) in [("V", p.video_tracks(sequence)), ("A", p.audio_tracks(sequence))] {
        for &track in tracks {
            let clips = p.track_clips(track);
            say!(out, "  {prefix}{} [{} clip(s)]", p.track_index(track) + 1, clips.len());
            for &clip in clips {
                let line = format!(
                    "    {:>8}s - {:>8}s  {}  (in {}s, out {}s)",
                    fmt_f(secs(p.start(clip)?), 3),
                    fmt_f(secs(p.end(clip)?), 3),
                    or_default(p.track_item_name(clip)?, "(unnamed)"),
                    fmt_f(secs(p.in_point(clip)?), 3),
                    fmt_f(secs(p.out_point(clip)?), 3),
                );
                say!(out, "{line}");
            }
        }
    }
    for caption_track in p.caption_tracks(sequence) {
        let captions = p.captions(caption_track);
        say!(out, "  C{} [{} caption(s)]", p.caption_track_index(caption_track)? + 1, captions.len());
        for caption in captions {
            let text = p.caption_text(caption);
            let line = format!(
                "    {:>8}s - {:>8}s  {}",
                fmt_f(secs(p.caption_start(caption)?), 3),
                fmt_f(secs(p.caption_end(caption)?), 3),
                if text.is_empty() { "(no text)" } else { text },
            );
            say!(out, "{line}");
        }
    }
    Ok(())
}

fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> PyResult<i32> {
    let parsed = match args::parse(&SPEC, argv, out, err) {
        Outcome::Run(p) => p,
        Outcome::Exit(code) => return Ok(code),
    };
    let file = parsed.positionals[0][0].as_str();
    if !Path::new(file).exists() {
        say!(err, "Error: File not found: {}", path_str(file));
        return Ok(1);
    }
    let application = application::parse(file, None)?;
    let project = application.project();
    say!(out, "{}  ({} sequence(s))", project.name(), project.sequences().len());
    let (items, sequences) = (parsed.flag("--items"), parsed.flag("--sequences"));
    let show_items = items || !sequences;
    let show_sequences = sequences || !items;
    if show_items {
        if let Some(root) = project.root_item() {
            say!(out, "");
            print_item(out, project, root, 0);
        }
    }
    if show_sequences {
        for sequence in project.sequences() {
            print_sequence(out, project, sequence)?;
        }
    }
    Ok(0)
}

/// The `pr-visualize` entry point: `argv` without the program name; returns the exit status.
pub fn main(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match run(argv, out, err) {
        Ok(code) => code,
        Err(e) => {
            report(err, &e);
            1
        }
    }
}
