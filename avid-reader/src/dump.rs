//! A pyavb-shaped dump of a bin's mobs, for differential testing against the oracle
//! (`converter/local/oracle/pyavb_dump.py`): every object becomes
//! {"_type", "_id", "_class", fields...}, repeats become {"_ref": id}.

use std::collections::HashMap;

use crate::file::{AvbFile, Object};
use crate::json::Json;
use crate::objects::{Node, Value, LIST_CLASSES};

const MAX_DEPTH: usize = 512;

pub struct Dumper<'a> {
    file: &'a AvbFile,
    seen: HashMap<u32, String>,
    next_id: usize,
}

impl<'a> Dumper<'a> {
    pub fn new(file: &'a AvbFile) -> Dumper<'a> {
        Dumper { file, seen: HashMap::new(), next_id: 0 }
    }

    /// {"mobs": [...]} in bin-item order, like pyavb `Bin.mobs`.
    pub fn mobs(mut self) -> Json {
        let mut mobs = Vec::new();
        if let Some(root) = self.file.root() {
            if let Some(Value::List(items)) = root.get("items") {
                for item in items {
                    if let Value::Node(item) = item {
                        mobs.push(self.value(item.get("mob").unwrap_or(&Value::Null), 0));
                    }
                }
            }
        }
        Json::obj().with("mobs", Json::Array(mobs))
    }

    fn reference(&mut self, index: u32, depth: usize) -> Json {
        if let Some(id) = self.seen.get(&index) {
            return Json::obj().with("_ref", Json::str(id.clone()));
        }
        match self.file.get(index) {
            Some(Object::Decoded(node)) if &node.class == b"ATTR" => self.attrs_node(node, depth),
            Some(Object::Decoded(node)) if LIST_CLASSES.contains(&&node.class) => {
                let items = node.get("items").cloned().unwrap_or(Value::Null);
                self.value(&items, depth)
            }
            Some(Object::Decoded(node)) => {
                let id = self.new_id(node);
                self.seen.insert(index, id.clone());
                self.node(node, Some(id), depth)
            }
            Some(Object::Unsupported { class, .. }) => {
                Json::obj().with("_unsupported", Json::str(String::from_utf8_lossy(class)))
            }
            None => Json::obj().with("_missing", Json::Int(index as i64)),
        }
    }

    fn new_id(&mut self, node: &Node) -> String {
        self.next_id += 1;
        format!("{}#{}", node.type_name, self.next_id - 1)
    }

    fn attrs_node(&mut self, node: &Node, depth: usize) -> Json {
        match node.get("entries") {
            Some(v) => self.value(v, depth),
            None => Json::obj(),
        }
    }

    fn node(&mut self, node: &Node, id: Option<String>, depth: usize) -> Json {
        let mut out = Json::obj().with("_type", Json::str(node.type_name));
        // Inline nodes (Track, BinItem) get ids too, as pyavb AVBObjects do.
        let id = id.unwrap_or_else(|| self.new_id(node));
        out.push("_id", Json::str(id));
        if node.class != [0; 4] {
            out.push("_class", Json::str(node.class_str()));
        }
        for (k, v) in &node.fields {
            let value = self.value(v, depth + 1);
            out.push(*k, value);
        }
        out
    }

    fn value(&mut self, v: &Value, depth: usize) -> Json {
        if depth > MAX_DEPTH {
            return Json::obj().with("_truncated", Json::Bool(true));
        }
        match v {
            Value::Null => Json::Null,
            Value::Int(i) => Json::Int(*i),
            Value::UInt(i) => Json::UInt(*i),
            Value::Float(f) => Json::Float(*f),
            Value::Bool(b) => Json::Bool(*b),
            Value::Str(s) => Json::str(s.clone()),
            Value::Ref(i) => self.reference(*i, depth + 1),
            Value::RefList(items) | Value::List(items) => {
                Json::Array(items.iter().map(|x| self.value(x, depth + 1)).collect())
            }
            Value::Bytes(b) => Json::obj().with("_bytes", Json::str(hex(b))),
            Value::MobId(m) => Json::obj().with("_str", Json::str(m.urn())),
            Value::Uuid(u) => Json::obj().with("_str", Json::str(u.to_string())),
            Value::Time(t) => Json::obj().with("_unix", Json::Int(*t as i64)),
            Value::Node(n) => self.node(n, None, depth + 1),
            Value::Attrs(entries) => {
                let mut out = Json::obj();
                for (k, x) in entries {
                    let value = self.value(x, depth + 1);
                    out.push(k.clone(), value);
                }
                out
            }
        }
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}
