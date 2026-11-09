//! XML parser producing the same tree as Python's `ET.fromstring`.
//!
//! It mirrors what expat + `TreeBuilder` keep and drop:
//!
//! - `\r\n` and lone `\r` become `\n` (XML end-of-line handling);
//! - a literal tab/CR/LF inside an attribute value becomes a space
//!   (attribute-value normalization); character references are not normalized;
//! - the five predefined entities and decimal/hex character references are
//!   decoded; any other entity is an error;
//! - comments and processing instructions are dropped, CDATA sections become
//!   ordinary text, and text split by them is joined;
//! - character data that is empty stays `None`.
//!
//! Namespaces (prefixed names, `xmlns` attributes) and DOCTYPE declarations
//! are refused outright: ElementTree would rewrite or drop them, so a
//! document carrying them can never pass the round-trip self-check anyway.
//! Byte-level checks that are expat errors (invalid XML characters, `]]>` in
//! text, `--` in comments, duplicate attributes, mismatched tags, junk after
//! the root) are errors here too.

use super::name_tables::{NAME_CHAR, NAME_START};
use super::tree::{NodeId, Tree};
use crate::error::{Error, Result};

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// expat's name-start test: ASCII letters, `_`, `:`, and the non-ASCII
/// ranges measured from expat itself (`name_tables.rs`; expat follows the
/// older XML 1.0 Appendix B classes, not the 5th-edition ranges).
fn is_name_start(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic() || c == '_' || c == ':'
    } else {
        in_table(NAME_START, c)
    }
}

fn is_name_char(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.')
    } else {
        in_table(NAME_CHAR, c)
    }
}

fn is_xml_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

impl<'a> Parser<'a> {
    fn err<T>(&self, message: &str) -> Result<T> {
        let before = &self.b[..self.pos.min(self.b.len())];
        let line = before.iter().filter(|&&c| c == b'\n').count() + 1;
        let col = before.iter().rev().take_while(|&&c| c != b'\n').count();
        Err(Error::Parse(format!("{message}: line {line}, column {col}")))
    }

    fn starts(&self, s: &str) -> bool {
        self.b[self.pos..].starts_with(s.as_bytes())
    }

    fn skip_space(&mut self) -> bool {
        let start = self.pos;
        while self.pos < self.b.len() && is_space(self.b[self.pos]) {
            self.pos += 1;
        }
        self.pos > start
    }

    /// Validate raw characters in `self.src[start..end]` (bytes already known UTF-8).
    fn check_chars(&self, start: usize, end: usize) -> Result<()> {
        let s = &self.b[start..end];
        let mut i = 0;
        while i < s.len() {
            let c = s[i];
            if c < 0x20 && c != b'\t' && c != b'\n' && c != b'\r' {
                return Err(Error::Parse(format!("not well-formed (invalid token) at byte {}", start + i)));
            }
            if c == 0xEF && i + 2 < s.len() && s[i + 1] == 0xBF && (s[i + 2] == 0xBE || s[i + 2] == 0xBF) {
                return Err(Error::Parse(format!("not well-formed (invalid token) at byte {}", start + i)));
            }
            i += 1;
        }
        Ok(())
    }

    fn name(&mut self) -> Result<&'a str> {
        let start = self.pos;
        let rest = &self.src[start..];
        let mut chars = rest.char_indices();
        match chars.next() {
            Some((_, c)) if is_name_start(c) => {}
            _ => return self.err("not well-formed (invalid token)"),
        }
        let mut end = rest.len();
        for (i, c) in chars {
            if !is_name_char(c) {
                end = i;
                break;
            }
        }
        self.pos = start + end;
        let name = &self.src[start..start + end];
        if name.contains(':') {
            return self.err("namespaced names are not supported");
        }
        Ok(name)
    }

    /// Decode a reference at `self.pos` (which is at `&`), appending to `out`.
    fn reference(&mut self, out: &mut String) -> Result<()> {
        let start = self.pos + 1;
        let Some(semi) = self.b[start..].iter().position(|&c| c == b';') else {
            return self.err("not well-formed (invalid token)");
        };
        let body = &self.src[start..start + semi];
        let decoded = match body {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ if body.starts_with("#x") => {
                let v = u32::from_str_radix(&body[2..], 16).ok().filter(|_| body.len() > 2 && body[2..].bytes().all(|c| c.is_ascii_hexdigit()));
                match v.filter(|&v| is_xml_char(v)).and_then(char::from_u32) {
                    Some(c) => c,
                    None => return self.err("reference to invalid character number"),
                }
            }
            _ if body.starts_with('#') => {
                let v = body[1..].parse::<u32>().ok().filter(|_| body.len() > 1 && body[1..].bytes().all(|c| c.is_ascii_digit()));
                match v.filter(|&v| is_xml_char(v)).and_then(char::from_u32) {
                    Some(c) => c,
                    None => return self.err("reference to invalid character number"),
                }
            }
            _ => return self.err("undefined entity"),
        };
        out.push(decoded);
        self.pos = start + semi + 1;
        Ok(())
    }

    /// Skip a comment starting at `<!--`.
    fn comment(&mut self) -> Result<()> {
        let start = self.pos + 4;
        let Some(end) = find(&self.b[start..], b"--") else {
            return self.err("unclosed token");
        };
        if self.b.get(start + end + 2) != Some(&b'>') {
            self.pos = start + end;
            return self.err("not well-formed (invalid token)");
        }
        self.check_chars(start, start + end)?;
        self.pos = start + end + 3;
        Ok(())
    }

    /// Skip a processing instruction starting at `<?` (not the XML declaration).
    fn pi(&mut self) -> Result<()> {
        self.pos += 2;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return self.err("XML or text declaration not at start of entity");
        }
        let start = self.pos;
        let Some(end) = find(&self.b[start..], b"?>") else {
            return self.err("unclosed token");
        };
        if end > 0 && !is_space(self.b[start]) {
            return self.err("not well-formed (invalid token)");
        }
        self.check_chars(start, start + end)?;
        self.pos = start + end + 2;
        Ok(())
    }

    /// Comments, PIs and whitespace outside the root element.
    fn misc(&mut self) -> Result<()> {
        loop {
            self.skip_space();
            if self.starts("<!--") {
                self.comment()?;
            } else if self.starts("<?") {
                self.pi()?;
            } else if self.starts("<!DOCTYPE") {
                return self.err("DOCTYPE declarations are not supported");
            } else {
                return Ok(());
            }
        }
    }

    /// Append raw character data `src[start..end]` to `out` with end-of-line
    /// normalization.
    fn push_text(&self, out: &mut String, start: usize, end: usize) {
        let s = &self.src[start..end];
        if !s.contains('\r') {
            out.push_str(s);
            return;
        }
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            } else {
                out.push(c);
            }
        }
    }

    fn start_tag(&mut self, tree: &mut Tree) -> Result<(NodeId, bool)> {
        self.pos += 1;
        let tag = self.name()?;
        let id = tree.new_element(tag);
        loop {
            let spaced = self.skip_space();
            if self.starts("/>") {
                self.pos += 2;
                return Ok((id, true));
            }
            if self.starts(">") {
                self.pos += 1;
                return Ok((id, false));
            }
            if self.pos >= self.b.len() {
                return self.err("unclosed token");
            }
            if !spaced {
                return self.err("not well-formed (invalid token)");
            }
            let name = self.name()?;
            if name == "xmlns" {
                return self.err("namespace declarations are not supported");
            }
            self.skip_space();
            if !self.starts("=") {
                return self.err("not well-formed (invalid token)");
            }
            self.pos += 1;
            self.skip_space();
            let quote = match self.b.get(self.pos) {
                Some(&q @ (b'"' | b'\'')) => q,
                _ => return self.err("not well-formed (invalid token)"),
            };
            self.pos += 1;
            let mut value = String::new();
            loop {
                let run = self.pos;
                while self.pos < self.b.len()
                    && !matches!(self.b[self.pos], b'<' | b'&' | b'\t' | b'\n' | b'\r')
                    && self.b[self.pos] != quote
                {
                    self.pos += 1;
                }
                self.check_chars(run, self.pos)?;
                value.push_str(&self.src[run..self.pos]);
                match self.b.get(self.pos) {
                    None => return self.err("unclosed token"),
                    Some(&b'<') => return self.err("not well-formed (invalid token)"),
                    Some(&b'&') => self.reference(&mut value)?,
                    Some(&b'\r') => {
                        self.pos += 1;
                        if self.b.get(self.pos) == Some(&b'\n') {
                            self.pos += 1;
                        }
                        value.push(' ');
                    }
                    Some(&b'\t' | &b'\n') => {
                        self.pos += 1;
                        value.push(' ');
                    }
                    Some(_) => {
                        self.pos += 1;
                        break;
                    }
                }
            }
            if tree[id].has_attr(name) {
                return self.err("duplicate attribute");
            }
            tree[id].attrs.push((name.to_string(), value));
        }
    }

    fn document(&mut self, tree: &mut Tree) -> Result<NodeId> {
        if self.starts("\u{feff}") {
            self.pos += 3;
        }
        if self.starts("<?xml") && self.b.get(self.pos + 5).is_some_and(|&c| is_space(c) || c == b'?') {
            let Some(end) = find(&self.b[self.pos..], b"?>") else {
                return self.err("unclosed token");
            };
            let body = &self.b[self.pos + 5..self.pos + end];
            if !xml_decl_ok(body) {
                return self.err("XML declaration not well-formed");
            }
            self.pos += end + 2;
        }
        self.misc()?;
        if !(self.starts("<") && self.pos + 1 < self.b.len()) {
            return self.err("no element found");
        }
        let mut stack: Vec<NodeId> = Vec::new();
        let mut pending = String::new();
        let flush = |tree: &mut Tree, stack: &[NodeId], pending: &mut String| {
            if pending.is_empty() {
                return;
            }
            let Some(&top) = stack.last() else { return };
            let text = std::mem::take(pending);
            match tree.children(top).last() {
                Some(&last) => tree[last].tail = Some(text),
                None => tree[top].text = Some(text),
            }
        };
        let (root, empty) = self.start_tag(tree)?;
        if empty {
            self.misc()?;
            return self.finish(root);
        }
        stack.push(root);
        while let Some(&top) = stack.last() {
            match self.b.get(self.pos) {
                None => return self.err("no element found"),
                Some(b'<') => {
                    if self.starts("</") {
                        self.pos += 2;
                        let name = self.name()?;
                        self.skip_space();
                        if !self.starts(">") {
                            return self.err("not well-formed (invalid token)");
                        }
                        if name != tree.tag(top) {
                            return self.err("mismatched tag");
                        }
                        self.pos += 1;
                        flush(tree, &stack, &mut pending);
                        stack.pop();
                    } else if self.starts("<!--") {
                        self.comment()?;
                    } else if self.starts("<![CDATA[") {
                        let start = self.pos + 9;
                        let Some(end) = find(&self.b[start..], b"]]>") else {
                            return self.err("unclosed CDATA section");
                        };
                        self.check_chars(start, start + end)?;
                        self.push_text(&mut pending, start, start + end);
                        self.pos = start + end + 3;
                    } else if self.starts("<?") {
                        self.pi()?;
                    } else if self.starts("<!") {
                        return self.err("not well-formed (invalid token)");
                    } else {
                        flush(tree, &stack, &mut pending);
                        let (child, empty) = self.start_tag(tree)?;
                        tree.append_fresh(top, child);
                        if !empty {
                            stack.push(child);
                        }
                    }
                }
                Some(b'&') => self.reference(&mut pending)?,
                Some(_) => {
                    let start = self.pos;
                    while self.pos < self.b.len() && !matches!(self.b[self.pos], b'<' | b'&') {
                        if self.b[self.pos] == b']' && self.starts("]]>") {
                            return self.err("not well-formed (invalid token)");
                        }
                        self.pos += 1;
                    }
                    self.check_chars(start, self.pos)?;
                    self.push_text(&mut pending, start, self.pos);
                }
            }
        }
        self.misc()?;
        self.finish(root)
    }

    fn finish(&self, root: NodeId) -> Result<NodeId> {
        if self.pos < self.b.len() {
            return self.err("junk after document element");
        }
        Ok(root)
    }
}

/// A parsed pseudo-attribute: name, value, rest of the declaration.
type Pseudo<'a> = (&'a [u8], &'a [u8], &'a [u8]);

/// One pseudo-attribute of the XML declaration (expat `parsePseudoAttribute`):
/// `Ok(None)` at the end, `Err(())` when malformed.
fn pseudo_attribute(s: &[u8]) -> std::result::Result<Option<Pseudo<'_>>, ()> {
    if s.is_empty() {
        return Ok(None);
    }
    if !is_space(s[0]) {
        return Err(());
    }
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    if i == s.len() {
        return Ok(None);
    }
    let name_start = i;
    let name_end;
    loop {
        match s.get(i) {
            None => return Err(()),
            Some(b'=') => {
                name_end = i;
                break;
            }
            Some(&c) if is_space(c) => {
                name_end = i;
                while i < s.len() && is_space(s[i]) {
                    i += 1;
                }
                if s.get(i) != Some(&b'=') {
                    return Err(());
                }
                break;
            }
            Some(_) => i += 1,
        }
    }
    if name_end == name_start {
        return Err(());
    }
    i += 1; // '='
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let open = match s.get(i) {
        Some(&q @ (b'"' | b'\'')) => q,
        _ => return Err(()),
    };
    i += 1;
    let value_start = i;
    loop {
        match s.get(i) {
            None => return Err(()),
            Some(&c) if c == open => break,
            Some(&c) if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_') => i += 1,
            Some(_) => return Err(()),
        }
    }
    Ok(Some((&s[name_start..name_end], &s[value_start..i], &s[i + 1..])))
}

/// expat `doParseXmlDecl` for a document entity: `version` required, then
/// optional `encoding` (value starting with a letter), then optional
/// `standalone` (`yes`/`no`), whitespace-separated, nothing else. The
/// encoding name itself is not acted on: input is decoded as UTF-8, and a
/// document whose declaration is not Premiere's exact prolog fails the
/// round-trip self-check anyway (as in Python).
fn xml_decl_ok(body: &[u8]) -> bool {
    let Ok(Some((name, _, rest))) = pseudo_attribute(body) else { return false };
    if name != b"version" {
        return false;
    }
    let mut next = match pseudo_attribute(rest) {
        Ok(None) => return true,
        Ok(Some(n)) => n,
        Err(()) => return false,
    };
    if next.0 == b"encoding" {
        if !next.1.first().is_some_and(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        next = match pseudo_attribute(next.2) {
            Ok(None) => return true,
            Ok(Some(n)) => n,
            Err(()) => return false,
        };
    }
    if next.0 != b"standalone" || !(next.1 == b"yes" || next.1 == b"no") {
        return false;
    }
    next.2.iter().all(|&c| is_space(c))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Parse `xml` into `tree` the way `ET.fromstring` would; returns the root.
pub fn parse_into(tree: &mut Tree, xml: &str) -> Result<NodeId> {
    Parser { src: xml, b: xml.as_bytes(), pos: 0 }.document(tree)
}

/// Parse UTF-8 bytes into `tree` (invalid UTF-8 is a parse error, as in expat).
pub fn parse_bytes_into(tree: &mut Tree, xml: &[u8]) -> Result<NodeId> {
    match std::str::from_utf8(xml) {
        Ok(s) => parse_into(tree, s),
        Err(e) => Err(Error::Parse(format!(
            "not well-formed (invalid token) at byte {}",
            e.valid_up_to()
        ))),
    }
}

impl Tree {
    /// Parse an XML string (a builder template, say) into this arena as a
    /// detached subtree; Python `ET.fromstring(text)`.
    pub fn parse_fragment(&mut self, xml: &str) -> Result<NodeId> {
        parse_into(self, xml)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn et_like_tree() {
        let mut t = Tree::new();
        let r = t
            .parse_fragment("<?xml version='1.0'?>\n<!-- c --><a x=\"1&#9;\n2\" y='&quot;'>t<!--x-->u<b/>tail<![CDATA[<z>]]><c></c>\r\n</a>\n")
            .unwrap();
        assert_eq!(t.tag(r), "a");
        assert_eq!(t[r].attrs, vec![("x".into(), "1\t 2".into()), ("y".into(), "\"".into())]);
        assert_eq!(t.text(r), Some("tu"));
        let kids = t.children(r).to_vec();
        assert_eq!(t.tail(kids[0]), Some("tail<z>"));
        assert_eq!(t.text(kids[1]), None);
        assert_eq!(t.tail(kids[1]), Some("\n"));
    }

    #[test]
    fn refusals() {
        for bad in [
            "<a><b></a>",
            "<a>&nbsp;</a>",
            "<a x='1' x='2'/>",
            "<a/>junk",
            "<a>\u{1}</a>",
            "<a>]]></a>",
            "<p:a/>",
            "<a xmlns='u'/>",
            "<a>&#0;</a>",
            "<a",
            "",
        ] {
            assert!(Tree::new().parse_fragment(bad).is_err(), "{bad:?}");
        }
    }
}
