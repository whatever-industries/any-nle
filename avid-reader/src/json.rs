//! Minimal JSON writer (std-only).

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    /// Insertion-ordered object.
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Object(Vec::new())
    }

    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    /// Append a key to an object (no-op on other variants).
    pub fn with(mut self, key: impl Into<String>, value: Json) -> Json {
        if let Json::Object(entries) = &mut self {
            entries.push((key.into(), value));
        }
        self
    }

    pub fn push(&mut self, key: impl Into<String>, value: Json) {
        if let Json::Object(entries) = self {
            entries.push((key.into(), value));
        }
    }

    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, indent: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => write!(out, "{}", i).unwrap(),
            Json::UInt(i) => write!(out, "{}", i).unwrap(),
            Json::Float(f) if f.is_finite() => write!(out, "{:?}", f).unwrap(),
            Json::Float(_) => out.push_str("null"),
            Json::Str(s) => escape(s, out),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Object(items) if items.is_empty() => out.push_str("{}"),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    pad(out, indent + 1);
                    item.write(out, indent + 1);
                }
                out.push('\n');
                pad(out, indent);
                out.push(']');
            }
            Json::Object(items) => {
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    pad(out, indent + 1);
                    escape(k, out);
                    out.push_str(": ");
                    v.write(out, indent + 1);
                }
                out.push('\n');
                pad(out, indent);
                out.push('}');
            }
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_formats() {
        let j = Json::obj().with("a\"", Json::Array(vec![Json::Float(24.0), Json::Float(f64::NAN), Json::str("\u{1}")]));
        assert_eq!(j.to_pretty(), "{\n \"a\\\"\": [\n  24.0,\n  null,\n  \"\\u0001\"\n ]\n}\n");
    }
}
