fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(request) = args.next() else {
        eprintln!("usage: premiere-import-probe REQUEST.json [--review | --replay-donor]");
        return std::process::ExitCode::FAILURE;
    };
    let mode = match args.next().as_deref() {
        None => anynle_app::premiere_import::Mode::Convert,
        Some(flag) if flag == "--review" => anynle_app::premiere_import::Mode::Review,
        Some(flag) if flag == "--replay-donor" => anynle_app::premiere_import::Mode::ReplayDonor,
        _ => {
            eprintln!("expected --review or --replay-donor");
            return std::process::ExitCode::FAILURE;
        }
    };
    if args.next().is_some() {
        eprintln!("unexpected argument");
        return std::process::ExitCode::FAILURE;
    }
    match anynle_app::premiere_import::run(request.as_ref(), mode) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("report is serializable")
            );
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("Premiere import probe: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
