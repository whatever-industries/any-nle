//! Headless preflight of an existing saved job.
use crate::job::{self, CancelToken};
use serde::Serialize;
use std::{
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

fn report(value: &impl Serialize, ready: bool) -> ExitCode {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let result = serde_json::to_writer_pretty(&mut stdout, value)
        .map_err(io::Error::other)
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush());
    match result {
        Ok(()) if ready => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        Err(error) => {
            crate::application::fail(format!("could not write the preflight report: {error}"))
        }
    }
}

fn load_error(error: String) -> ExitCode {
    eprintln!("any-nle: {error}");
    ExitCode::from(2)
}

pub(crate) fn run(path: &Path) -> ExitCode {
    #[cfg(target_os = "macos")]
    match crate::resolve_job::load_if_avid_resolve(path) {
        Ok(Some(job)) => {
            let (value, ready) = crate::resolve_job::review_cli(job);
            return report(&value, ready);
        }
        Ok(None) => {}
        Err(error) => return load_error(error),
    }
    let spec = match job::load(path) {
        Ok(spec) => spec,
        Err(error) => return load_error(error),
    };
    let cancel = CancelToken::default();
    match job::prepare_for_review(spec.clone(), &cancel, &mut |_| {}) {
        Ok(prepared) => report(&prepared.preflight_report(), true),
        Err(blocker) => report(&job::blocked_preflight_report(&spec, &blocker), false),
    }
}
