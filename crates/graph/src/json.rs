//! A small strict JSON value, parser and writer (std only, like the rest of the converter).
//!
//! Objects keep their key order. Numbers written with a fraction or exponent are `Float`, all
//! others `Int` (an integer that does not fit `i64` is an error, not a rounded float). Floats are
//! written in Rust's shortest round-trip form with a `.0` for whole numbers, so a value written
//! and read back is equal, kind included.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Nesting limit for parsing (the graph itself is at most a few levels deep; effect parameters
/// add a few more).
const MAX_DEPTH: usize = 64;

pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { b: text.as_bytes(), at: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.at != p.b.len() {
        return Err(p.err("trailing data"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("JSON: {what} at byte {}", self.at)
    }

    fn ws(&mut self) {
        while self.at < self.b.len() && matches!(self.b[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.b[self.at..].starts_with(lit.as_bytes()) {
            self.at += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        match self.b.get(self.at) {
            None => Err(self.err("unexpected end")),
            Some(b'{') => {
                self.at += 1;
                let mut pairs: Vec<(String, Value)> = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Ok(Value::Obj(pairs));
                }
                loop {
                    self.ws();
                    if self.b.get(self.at) != Some(&b'"') {
                        return Err(self.err("expected a key"));
                    }
                    let k = self.string()?;
                    if pairs.iter().any(|(e, _)| *e == k) {
                        return Err(self.err(&format!("repeated key {k:?}")));
                    }
                    self.ws();
                    if !self.eat(":") {
                        return Err(self.err("expected ':'"));
                    }
                    self.ws();
                    let v = self.value(depth + 1)?;
                    pairs.push((k, v));
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Value::Obj(pairs));
                    }
                    return Err(self.err("expected ',' or '}'"));
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Value::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Value::Arr(items));
                    }
                    return Err(self.err("expected ',' or ']'"));
                }
            }
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') if self.eat("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Value::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        if self.b[self.at] == b'-' {
            self.at += 1;
        }
        let digits = |p: &mut Self| {
            let s = p.at;
            while p.at < p.b.len() && p.b[p.at].is_ascii_digit() {
                p.at += 1;
            }
            p.at - s
        };
        let int_start = self.at;
        let n = digits(self);
        if n == 0 || (n > 1 && self.b[int_start] == b'0') {
            return Err(self.err("bad number"));
        }
        let mut float = false;
        if self.b.get(self.at) == Some(&b'.') {
            self.at += 1;
            float = true;
            if digits(self) == 0 {
                return Err(self.err("bad number"));
            }
        }
        if matches!(self.b.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            float = true;
            if matches!(self.b.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if digits(self) == 0 {
                return Err(self.err("bad number"));
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.at]).expect("ascii");
        if float {
            let f: f64 = text.parse().map_err(|_| self.err("bad number"))?;
            if !f.is_finite() {
                return Err(self.err("number out of range"));
            }
            Ok(Value::Float(f))
        } else {
            text.parse::<i64>().map(Value::Int).map_err(|_| self.err("integer out of range"))
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.at..self.at + 4).ok_or_else(|| self.err("bad escape"))?;
        let s = std::str::from_utf8(h).map_err(|_| self.err("bad escape"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("bad escape"))?;
        self.at += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.at;
            while self.at < self.b.len() && !matches!(self.b[self.at], b'"' | b'\\') && self.b[self.at] >= 0x20 {
                self.at += 1;
            }
            out.push_str(std::str::from_utf8(&self.b[start..self.at]).map_err(|_| self.err("invalid UTF-8"))?);
            match self.b.get(self.at) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let c = *self.b.get(self.at).ok_or_else(|| self.err("bad escape"))?;
                    self.at += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if !self.eat("\\u") {
                                    return Err(self.err("lone surrogate"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("lone surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(code).ok_or_else(|| self.err("lone surrogate"))?);
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                Some(_) => return Err(self.err("control character in string")),
            }
        }
    }
}

/// Indented JSON text (one space per level, like the converter's model files), ending in `\n`.
pub fn write(v: &Value) -> String {
    let mut out = String::new();
    emit(v, 0, &mut out);
    out.push('\n');
    out
}

fn emit(v: &Value, depth: usize, out: &mut String) {
    let pad = |out: &mut String, d: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', d));
    };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => write!(out, "{n}").expect("string"),
        Value::Float(f) => write!(out, "{f:?}").expect("string"),
        Value::Str(s) => quote(s, out),
        Value::Arr(items) if items.is_empty() => out.push_str("[]"),
        Value::Arr(items) if items.iter().all(|i| !matches!(i, Value::Arr(_) | Value::Obj(_))) => {
            // Short scalar lists (points, pairs, frame maps) stay on one line.
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit(item, depth + 1, out);
            }
            out.push(']');
        }
        Value::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                emit(item, depth + 1, out);
            }
            pad(out, depth);
            out.push(']');
        }
        Value::Obj(pairs) if pairs.is_empty() => out.push_str("{}"),
        Value::Obj(pairs) => {
            out.push('{');
            for (i, (k, item)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                quote(k, out);
                out.push_str(": ");
                emit(item, depth + 1, out);
            }
            pad(out, depth);
            out.push('}');
        }
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).expect("string"),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_kinds_and_text() {
        let v = Value::Obj(vec![
            ("a".into(), Value::Arr(vec![Value::Int(-3), Value::Float(100.0), Value::Float(0.1), Value::Float(-0.0), Value::Float(1e300)])),
            ("s".into(), Value::Str("tab\t \"q\" \u{1} é 𝄞".into())),
            ("n".into(), Value::Null),
            ("o".into(), Value::Obj(vec![("b".into(), Value::Bool(true))])),
        ]);
        let text = write(&v);
        assert_eq!(parse(&text).unwrap(), v);
        assert_eq!(parse("\"\\ud834\\udd1e\"").unwrap(), Value::Str("𝄞".into()));
    }

    #[test]
    fn refuses_what_it_cannot_keep() {
        for bad in ["01", "1.", "9223372036854775808", "{\"a\":1,\"a\":2}", "[1,]", "\"\\ud800\"", "1e999", "nul"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
