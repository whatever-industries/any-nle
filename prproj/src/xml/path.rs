//! The subset of ElementTree's ElementPath that py_premiere uses:
//! `find`, `findall`, `findtext` with steps `tag`, `*`, `.`, `..`, `//tag`
//! and the predicates `[@a]`, `[@a='v']`, `[tag]`, `[tag='text']`,
//! `[.='text']`, `[n]`, `[last()]`, `[last()-n]`.
//!
//! Results follow ET's order and its quirks (no de-duplication for `//`,
//! `..` never climbs above the element the search started from, `[n]` counts
//! same-tag siblings).

use super::tree::{NodeId, Tree};
use crate::error::{Error, Result};

#[derive(Debug)]
enum Pred {
    HasAttr(String),
    AttrEq(String, String),
    HasChild(String),
    ChildText(String, String),
    SelfText(String),
    /// 0-based from the start (`[n]` gives n-1) or negative from the end
    /// (`last()` is -1).
    Position(i64),
}

#[derive(Debug)]
enum Step {
    Child(String),
    SelfNode,
    Parent,
    Descendant(String),
    Filter(Pred),
}

fn unquote(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'\'' || b[0] == b'"') && b[b.len() - 1] == b[0] {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

fn parse_pred(body: &str) -> Result<Pred> {
    let bad = || Error::Value(format!("invalid predicate [{body}]"));
    let body = body.trim();
    if let Some(rest) = body.strip_prefix('@') {
        return Ok(match rest.split_once('=') {
            None => Pred::HasAttr(rest.trim().to_string()),
            Some((k, v)) => Pred::AttrEq(k.trim().to_string(), unquote(v.trim()).ok_or_else(bad)?),
        });
    }
    if let Some(rest) = body.strip_prefix("last()") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Ok(Pred::Position(-1));
        }
        let n: i64 = rest.strip_prefix('-').ok_or_else(bad)?.trim().parse().map_err(|_| bad())?;
        return Ok(Pred::Position(-1 - n));
    }
    if body.bytes().all(|b| b.is_ascii_digit()) && !body.is_empty() {
        let n: i64 = body.parse().map_err(|_| bad())?;
        if n < 1 {
            return Err(Error::Value("XPath position >= 1 expected".into()));
        }
        return Ok(Pred::Position(n - 1));
    }
    match body.split_once('=') {
        None => Ok(Pred::HasChild(body.to_string())),
        Some((k, v)) => {
            let v = unquote(v.trim()).ok_or_else(bad)?;
            let k = k.trim();
            if k == "." {
                Ok(Pred::SelfText(v))
            } else {
                Ok(Pred::ChildText(k.to_string(), v))
            }
        }
    }
}

fn compile(path: &str) -> Result<Vec<Step>> {
    if path.starts_with('/') {
        return Err(Error::Value("cannot use absolute path on element".into()));
    }
    let b = path.as_bytes();
    let mut steps = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                i += 2;
                let start = i;
                while i < b.len() && b[i] != b'/' && b[i] != b'[' {
                    i += 1;
                }
                let name = &path[start..i];
                if name.is_empty() {
                    return Err(Error::Value("invalid descendant".into()));
                }
                steps.push(Step::Descendant(name.to_string()));
            }
            b'/' => i += 1,
            b'[' => {
                let start = i + 1;
                let mut quote = None;
                i += 1;
                while i < b.len() {
                    match (quote, b[i]) {
                        (None, b'\'' | b'"') => quote = Some(b[i]),
                        (Some(q), c) if c == q => quote = None,
                        (None, b']') => break,
                        _ => {}
                    }
                    i += 1;
                }
                if i >= b.len() {
                    return Err(Error::Value("unterminated predicate".into()));
                }
                steps.push(Step::Filter(parse_pred(&path[start..i])?));
                i += 1;
            }
            _ => {
                let start = i;
                while i < b.len() && b[i] != b'/' && b[i] != b'[' {
                    i += 1;
                }
                steps.push(match &path[start..i] {
                    "." => Step::SelfNode,
                    ".." => Step::Parent,
                    name => Step::Child(name.to_string()),
                });
            }
        }
    }
    Ok(steps)
}

/// `"".join(element.itertext())`, iteratively.
fn full_text(tree: &Tree, id: NodeId) -> String {
    enum Step {
        Enter(NodeId),
        Tail(NodeId),
    }
    let mut out = String::new();
    let mut stack = vec![Step::Enter(id)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Enter(n) => {
                if let Some(t) = tree.text(n) {
                    out.push_str(t);
                }
                for &c in tree.children(n).iter().rev() {
                    stack.push(Step::Tail(c));
                    stack.push(Step::Enter(c));
                }
            }
            Step::Tail(n) => {
                if let Some(t) = tree.tail(n) {
                    out.push_str(t);
                }
            }
        }
    }
    out
}

fn matches_tag(tree: &Tree, id: NodeId, tag: &str) -> bool {
    tag == "*" || tree.tag(id) == tag
}

fn within(tree: &Tree, start: NodeId, mut node: NodeId) -> bool {
    loop {
        if node == start {
            return true;
        }
        match tree.parent(node) {
            Some(p) => node = p,
            None => return false,
        }
    }
}

impl Tree {
    /// Python `element.findall(path)`.
    pub fn findall(&self, id: NodeId, path: &str) -> Result<Vec<NodeId>> {
        let steps = compile(path)?;
        let mut result = vec![id];
        for step in &steps {
            let mut next = Vec::new();
            match step {
                Step::Child(tag) => {
                    for &e in &result {
                        next.extend(self.children(e).iter().copied().filter(|&c| matches_tag(self, c, tag)));
                    }
                }
                Step::SelfNode => next = result.clone(),
                Step::Parent => {
                    let mut seen = std::collections::HashSet::new();
                    for &e in &result {
                        if e == id {
                            continue;
                        }
                        if let Some(p) = self.parent(e) {
                            if within(self, id, p) && seen.insert(p) {
                                next.push(p);
                            }
                        }
                    }
                }
                Step::Descendant(tag) => {
                    for &e in &result {
                        next.extend(self.iter(e).filter(|&d| d != e && matches_tag(self, d, tag)));
                    }
                }
                Step::Filter(pred) => {
                    for &e in &result {
                        let keep = match pred {
                            Pred::HasAttr(k) => self.get(e, k).is_some(),
                            Pred::AttrEq(k, v) => self.get(e, k) == Some(v.as_str()),
                            Pred::HasChild(tag) => self.children(e).iter().any(|&c| matches_tag(self, c, tag)),
                            Pred::ChildText(tag, v) => self
                                .children(e)
                                .iter()
                                .any(|&c| matches_tag(self, c, tag) && full_text(self, c) == *v),
                            Pred::SelfText(v) => full_text(self, e) == *v,
                            Pred::Position(n) => match self.parent(e) {
                                Some(p) if e != id => {
                                    let tag = self.tag(e);
                                    let same: Vec<NodeId> = self
                                        .children(p)
                                        .iter()
                                        .copied()
                                        .filter(|&c| self.tag(c) == tag)
                                        .collect();
                                    let idx = if *n >= 0 { *n } else { same.len() as i64 + *n };
                                    idx >= 0 && same.get(idx as usize) == Some(&e)
                                }
                                _ => false,
                            },
                        };
                        if keep {
                            next.push(e);
                        }
                    }
                }
            }
            result = next;
        }
        Ok(result)
    }

    /// Python `element.find(path)`.
    pub fn find(&self, id: NodeId, path: &str) -> Result<Option<NodeId>> {
        Ok(self.findall(id, path)?.into_iter().next())
    }

    /// Python `element.find(path)` for paths known to be valid (panics on a
    /// malformed path, never on a missing element).
    pub fn find1(&self, id: NodeId, path: &str) -> Option<NodeId> {
        self.find(id, path).expect("invalid element path")
    }

    /// Python `element.findtext(path)`: `None` when nothing matches, else the
    /// element's text or `""`.
    pub fn findtext(&self, id: NodeId, path: &str) -> Result<Option<String>> {
        Ok(self.find(id, path)?.map(|e| self[e].text_or_empty().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let mut t = Tree::new();
        let root = t.parse_fragment(
            "<R><Component><Params><Param Index='0' ObjectRef='5'/><Param Index='1' ObjectRef='6'/></Params></Component><X>hi</X></R>",
        )
        .unwrap();
        let p = t.findall(root, "Component/Params/Param").unwrap();
        assert_eq!(p.len(), 2);
        let one = t.find(root, "Component/Params/Param[@Index='1']").unwrap().unwrap();
        assert_eq!(t.get(one, "ObjectRef"), Some("6"));
        assert_eq!(t.findtext(root, "X").unwrap().as_deref(), Some("hi"));
        assert_eq!(t.findtext(root, "Y").unwrap(), None);
        assert_eq!(t.findall(root, ".//Param").unwrap().len(), 2);
        assert_eq!(t.findall(root, "Component/Params/Param[last()]").unwrap(), vec![one]);
        assert_eq!(t.findall(root, "*[X]").unwrap().len(), 0);
        assert_eq!(t.findall(root, "Component[Params]").unwrap().len(), 1);
        assert_eq!(t.findall(root, "X[.='hi']").unwrap().len(), 1);
    }
}
