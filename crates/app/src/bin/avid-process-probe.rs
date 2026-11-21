use std::path::PathBuf;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: avid-process-probe INPUT.avb OUTPUT.avb (--review | --write-trial) [--sequence MOBID]... [--rec709-sdr] [--accept-approximations]";
    let source = PathBuf::from(args.next().ok_or(usage)?);
    let output = PathBuf::from(args.next().ok_or(usage)?);
    let (mut review, mut trial, mut rec709, mut approve) = (false, false, false, false);
    let mut selected = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--review") => review = true,
            Some("--write-trial") => trial = true,
            Some("--rec709-sdr") => rec709 = true,
            Some("--accept-approximations") => approve = true,
            Some("--sequence") => selected.push(
                args.next()
                    .and_then(|value| value.into_string().ok())
                    .filter(|value| value.starts_with("urn:smpte:umid:"))
                    .ok_or("--sequence requires a saved sequence MobID")?,
            ),
            _ => return Err(usage.into()),
        }
    }
    if review == trial {
        return Err(usage.into());
    }
    let report =
        anynle_app::avid_processing::run(&source, &output, &selected, review, rec709, approve)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Avid Processing: {error}");
            ExitCode::FAILURE
        }
    }
}
