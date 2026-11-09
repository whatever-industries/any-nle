//! prproj-visualize: the Rust port of py_premiere's `pr-visualize` (see `prproj::cli::visualize`).

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut err = stderr.lock();
    let code = prproj::cli::visualize::main(&argv, &mut out, &mut err);
    let _ = out.flush();
    ExitCode::from(code as u8)
}
