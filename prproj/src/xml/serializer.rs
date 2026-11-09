//! Premiere-style serializer (port of `py_premiere/xml/serializer.py`).
//!
//! Rules, exactly as Python applies them:
//!
//! - prolog `<?xml version="1.0" encoding="UTF-8" ?>\n`, epilogue `\n\n`;
//! - attributes in stored order, values escaped `&amp; &lt; &gt; &quot; &#13;
//!   &#10; &#9;` (no `&apos;`);
//! - an element with no children and falsy text (`None` or `""`) is written
//!   self-closing `<Tag/>`;
//! - leaf text: pure whitespace containing a newline is formatting and goes
//!   out raw; otherwise a trailing `\n\t+` run goes out raw and the rest is
//!   escaped `&amp; &lt; &gt; &#13; &#10;`;
//! - branch text and tails go out raw and must be whitespace (Python
//!   `str.isspace`), else "mixed content" is refused.
//!
//! Written iteratively so pathological nesting cannot overflow the stack.

use super::tree::{NodeId, Tree};
use crate::error::{Error, Result};
use crate::pystr::py_isspace;

/// The fixed XML declaration Premiere writes.
pub const PROLOG: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n";
/// The fixed bytes after the root element.
pub const EPILOGUE: &[u8] = b"\n\n";

fn escape_text(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            '\n' => out.push_str("&#10;"),
            c => out.push(c),
        }
    }
}

fn escape_attr(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
}

/// Python `_is_formatting_only`.
fn is_formatting_only(text: &str) -> bool {
    text.contains('\n') && py_isspace(text)
}

/// Start of the `\n\t+\Z` run, if the text ends with one.
fn trailing_formatting(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = b.len();
    while i > 0 && b[i - 1] == b'\t' {
        i -= 1;
    }
    (i < b.len() && i > 0 && b[i - 1] == b'\n').then(|| i - 1)
}

fn write_leaf_text(out: &mut String, text: &str) {
    if is_formatting_only(text) {
        out.push_str(text);
        return;
    }
    match trailing_formatting(text) {
        Some(at) => {
            escape_text(out, &text[..at]);
            out.push_str(&text[at..]);
        }
        None => escape_text(out, text),
    }
}

/// Write the start of `id`; returns true when the element stays open
/// (children/end tag still to come).
fn open(out: &mut String, tree: &Tree, id: NodeId) -> Result<bool> {
    let el = &tree[id];
    out.push('<');
    out.push_str(&el.tag);
    for (name, value) in &el.attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        escape_attr(out, value);
        out.push('"');
    }
    let text = el.text.as_deref().unwrap_or("");
    if el.children().is_empty() && text.is_empty() {
        out.push_str("/>");
        return Ok(false);
    }
    out.push('>');
    if !text.is_empty() {
        if !el.children().is_empty() {
            if !py_isspace(text) {
                return Err(Error::Serialize(format!(
                    "mixed content in <{}>: text alongside child elements",
                    el.tag
                )));
            }
            out.push_str(text);
        } else {
            write_leaf_text(out, text);
        }
    }
    Ok(true)
}

fn write_tail(out: &mut String, tree: &Tree, parent: NodeId, child: NodeId) -> Result<()> {
    if let Some(tail) = tree[child].tail.as_deref() {
        if !tail.is_empty() {
            if !py_isspace(tail) {
                return Err(Error::Serialize(format!(
                    "mixed content in <{}>: text after <{}>",
                    tree[parent].tag, tree[child].tag
                )));
            }
            out.push_str(tail);
        }
    }
    Ok(())
}

/// Python `_write_element`: the element and its subtree (not its own tail).
pub fn write_element(out: &mut String, tree: &Tree, id: NodeId) -> Result<()> {
    if !open(out, tree, id)? {
        return Ok(());
    }
    let mut stack: Vec<(NodeId, usize)> = vec![(id, 0)];
    while let Some(top) = stack.last_mut() {
        let (node, i) = *top;
        if let Some(&child) = tree[node].children().get(i) {
            top.1 += 1;
            if open(out, tree, child)? {
                stack.push((child, 0));
            } else {
                write_tail(out, tree, node, child)?;
            }
        } else {
            out.push_str("</");
            out.push_str(&tree[node].tag);
            out.push('>');
            stack.pop();
            if let Some(&(parent, _)) = stack.last() {
                write_tail(out, tree, parent, node)?;
            }
        }
    }
    Ok(())
}

/// Python `serialize_document`: prolog + root + epilogue, UTF-8.
pub fn serialize_document(tree: &Tree, root: NodeId) -> Result<Vec<u8>> {
    let mut s = String::new();
    write_element(&mut s, tree, root)?;
    let mut out = Vec::with_capacity(PROLOG.len() + s.len() + EPILOGUE.len());
    out.extend_from_slice(PROLOG);
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(EPILOGUE);
    Ok(out)
}

/// Python `serialize_element`: one subtree, no prolog/epilogue.
pub fn serialize_element(tree: &Tree, id: NodeId) -> Result<Vec<u8>> {
    let mut s = String::new();
    write_element(&mut s, tree, id)?;
    Ok(s.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(xml: &str) -> String {
        let mut t = Tree::new();
        let r = t.parse_fragment(xml).unwrap();
        String::from_utf8(serialize_element(&t, r).unwrap()).unwrap()
    }

    #[test]
    fn leaf_rules() {
        assert_eq!(rt("<a><b></b><c>\n\t</c><d>x&#10;\n\t\t</d><e>&#10;</e></a>"), "<a><b/><c>\n\t</c><d>x&#10;\n\t\t</d><e>\n</e></a>"); // pure-whitespace leaf text is formatting: Python quirk
        assert_eq!(rt("<a v=\"&#9;&#10;&#13;&quot;'&lt;&gt;\"/>"), "<a v=\"&#9;&#10;&#13;&quot;'&lt;&gt;\"/>");
        assert_eq!(rt("<a>\"'&gt;</a>"), "<a>\"'&gt;</a>");
    }

    #[test]
    fn mixed_content_refused() {
        let mut t = Tree::new();
        let r = t.parse_fragment("<a>x<b/></a>").unwrap();
        assert!(serialize_element(&t, r).is_err());
        let r = t.parse_fragment("<a><b/>x</a>").unwrap();
        assert!(serialize_element(&t, r).is_err());
    }
}
