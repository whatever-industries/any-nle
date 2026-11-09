//! Transactional edits to source-pinned py-avb `Attributes` (`ATTR`).
//!
//! The serialized mapping is ordered. Replacing or renaming a key keeps its
//! position; adding a key appends it. Object targets must be decoded and
//! present. Opaque BOB bytes can be stored, but subgraph copying will still
//! refuse to guess whether those bytes contain references.

use std::collections::HashSet;

use crate::document::AvbDocument;
use crate::io::{Error, Result};
use crate::objects::Value;

fn fail(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    Integer(i32),
    Text(String),
    Object(Option<u32>),
    Blob(Vec<u8>),
}

pub trait DocumentAttributesExt {
    /// Set or append a key. A replacement returns the previous stored value.
    fn set_attribute(
        &mut self,
        attributes_id: u32,
        name: &str,
        value: AttributeValue,
    ) -> Result<Option<Value>>;

    /// Remove a key without deleting any object it referenced.
    fn remove_attribute(&mut self, attributes_id: u32, name: &str) -> Result<Value>;

    /// Rename a key at its current position. Existing destination names fail.
    fn rename_attribute(&mut self, attributes_id: u32, from: &str, to: &str) -> Result<()>;
}

impl DocumentAttributesExt for AvbDocument {
    fn set_attribute(
        &mut self,
        attributes_id: u32,
        name: &str,
        value: AttributeValue,
    ) -> Result<Option<Value>> {
        let old = checked_entries(self, attributes_id)?;
        let value = match value {
            AttributeValue::Integer(value) => Value::Int(i64::from(value)),
            AttributeValue::Text(value) => Value::Str(value),
            AttributeValue::Object(Some(id)) => {
                if id == 0 || self.node(id).is_none() {
                    return Err(fail(format!("attribute target {id} is missing or opaque")));
                }
                Value::Ref(id)
            }
            AttributeValue::Object(None) => Value::Null,
            AttributeValue::Blob(value) => Value::Bytes(value),
        };
        let mut edited = old.clone();
        let prior = if let Some((_, current)) = edited.iter_mut().find(|(key, _)| key == name) {
            Some(std::mem::replace(current, value))
        } else {
            edited.push((name.to_owned(), value));
            None
        };
        commit(self, attributes_id, old, edited)?;
        Ok(prior)
    }

    fn remove_attribute(&mut self, attributes_id: u32, name: &str) -> Result<Value> {
        let old = checked_entries(self, attributes_id)?;
        let index = old
            .iter()
            .position(|(key, _)| key == name)
            .ok_or_else(|| fail(format!("attribute {name} is absent")))?;
        let mut edited = old.clone();
        let (_, removed) = edited.remove(index);
        commit(self, attributes_id, old, edited)?;
        Ok(removed)
    }

    fn rename_attribute(&mut self, attributes_id: u32, from: &str, to: &str) -> Result<()> {
        let old = checked_entries(self, attributes_id)?;
        let index = old
            .iter()
            .position(|(key, _)| key == from)
            .ok_or_else(|| fail(format!("attribute {from} is absent")))?;
        if from == to {
            return Ok(());
        }
        if old.iter().any(|(key, _)| key == to) {
            return Err(fail(format!("attribute {to} already exists")));
        }
        let mut edited = old.clone();
        edited[index].0 = to.to_owned();
        commit(self, attributes_id, old, edited)
    }
}

fn checked_entries(doc: &AvbDocument, attributes_id: u32) -> Result<Vec<(String, Value)>> {
    let node = doc.node(attributes_id).ok_or_else(|| {
        fail(format!(
            "attributes object {attributes_id} is missing or opaque"
        ))
    })?;
    if node.class != *b"ATTR" {
        return Err(fail(format!("object {attributes_id} is not ATTR")));
    }
    let Some(Value::Attrs(entries)) = node.get("entries") else {
        return Err(fail(format!("ATTR {attributes_id} has no decoded entries")));
    };
    let mut names = HashSet::new();
    for (name, value) in entries {
        if !names.insert(name) {
            return Err(fail(format!(
                "ATTR {attributes_id} has duplicate key {name}"
            )));
        }
        match value {
            Value::Int(value) if i32::try_from(*value).is_ok() => {}
            Value::Str(_) | Value::Bytes(_) | Value::Null => {}
            Value::Ref(id) if *id != 0 && doc.node(*id).is_some() => {}
            _ => {
                return Err(fail(format!(
                    "ATTR {attributes_id} has unsupported or opaque value for {name}"
                )))
            }
        }
    }
    Ok(entries.clone())
}

fn commit(
    doc: &mut AvbDocument,
    attributes_id: u32,
    old: Vec<(String, Value)>,
    edited: Vec<(String, Value)>,
) -> Result<()> {
    doc.set_field(attributes_id, "entries", Value::Attrs(edited))?;
    if let Err(error) = doc.to_bytes() {
        doc.set_field(attributes_id, "entries", Value::Attrs(old))
            .map_err(|rollback| {
                fail(format!(
                    "attribute edit failed ({error}); rollback failed ({rollback})"
                ))
            })?;
        return Err(error);
    }
    Ok(())
}
