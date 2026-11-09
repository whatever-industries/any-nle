//! The py_premiere command-line tools as library functions (port of `py_premiere/cli`).
//!
//! | Python | Rust | binary |
//! |---|---|---|
//! | `pr-inspect` (`cli/inspect.py`) | [`inspect::main`] | `prproj-inspect` |
//! | `pr-compare` (`cli/compare.py`) | [`compare::main`] | `prproj-compare` |
//! | `pr-validate` (`cli/validate.py`) | [`validate::main`] | `prproj-validate` |
//! | `pr-visualize` (`cli/visualize.py`) | [`visualize::main`] | `prproj-visualize` |
//!
//! Each `main(argv, out, err) -> i32` takes the arguments after the program name and writes
//! stdout/stderr to the given writers, returning the exit code. An exception Python would not
//! catch (a refused project, a malformed ground-truth file, ...) is reported on `err` as
//! `Class: message` (Python prints a traceback ending in that line) with exit status 1; what
//! was printed before it stays printed, as in Python.

pub mod args;
pub mod compare;
pub mod inspect;
pub mod validate;
pub mod visualize;

use std::io::Write;
use std::path::Path;

use crate::convert::model::py_str_repr;
use crate::error::Error;
use crate::model::project::py_path;

/// A Python exception: class name and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyErr {
    pub class: String,
    pub message: String,
}

impl PyErr {
    pub fn new(class: &str, message: impl Into<String>) -> PyErr {
        PyErr { class: class.into(), message: message.into() }
    }
}

impl From<Error> for PyErr {
    fn from(e: Error) -> PyErr {
        PyErr { class: e.py_class().into(), message: e.to_string() }
    }
}

impl From<crate::caption::PyError> for PyErr {
    fn from(e: crate::caption::PyError) -> PyErr {
        PyErr::from(Error::from(e))
    }
}

impl From<std::io::Error> for PyErr {
    fn from(e: std::io::Error) -> PyErr {
        PyErr::from(Error::from(e))
    }
}

pub type PyResult<T> = std::result::Result<T, PyErr>;

/// An `OSError` as Python words it for a path: `[Errno 2] No such file or directory: 'x'`.
pub fn os_error(path: &Path, e: &std::io::Error) -> Error {
    let text = e.to_string();
    let message = text.split(" (os error").next().unwrap_or(&text).to_string();
    match e.raw_os_error() {
        Some(n) => {
            let text = format!("[Errno {n}] {message}: {}", py_str_repr(&path.to_string_lossy()));
            if n == 2 {
                Error::FileNotFound(text)
            } else {
                Error::Io(text)
            }
        }
        None => Error::Io(e.to_string()),
    }
}

/// Write to a stream and ignore failures (a closed pipe must not panic).
pub(crate) fn put(w: &mut dyn Write, text: &str) {
    let _ = w.write_all(text.as_bytes());
}

/// `print(...)`.
macro_rules! say {
    ($w:expr, $($arg:tt)*) => {{
        let mut s = format!($($arg)*);
        s.push('\n');
        $crate::cli::put($w, &s);
    }};
}
pub(crate) use say;

/// Report an uncaught exception the way the last line of a Python traceback does.
pub(crate) fn report(err: &mut dyn Write, e: &PyErr) {
    if e.message.is_empty() {
        say!(err, "{}", e.class);
    } else {
        say!(err, "{}: {}", e.class, e.message);
    }
}

/// `Path(text).name`.
pub fn path_name(text: &str) -> String {
    let normalized = py_path(text).to_string_lossy().into_owned();
    if normalized == "." || normalized.chars().all(|c| c == '/') {
        return String::new();
    }
    normalized.rsplit('/').next().unwrap_or("").to_string()
}

/// `str(Path(text))`.
pub fn path_str(text: &str) -> String {
    py_path(text).to_string_lossy().into_owned()
}

/// `Path(text).with_suffix(suffix)`.
pub fn with_suffix(text: &str, suffix: &str) -> PyResult<String> {
    let normalized = path_str(text);
    let name = path_name(text);
    if name.is_empty() {
        return Err(PyErr::new("ValueError", format!("{} has an empty name", repr_path(&normalized))));
    }
    let stem = match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[..i],
        _ => name.as_str(),
    };
    let parent = &normalized[..normalized.len() - name.len()];
    Ok(format!("{parent}{stem}{suffix}"))
}

fn repr_path(normalized: &str) -> String {
    format!("PosixPath({})", py_str_repr(normalized))
}
