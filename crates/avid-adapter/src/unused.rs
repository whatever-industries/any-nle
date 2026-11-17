//! Proof for omitting an unlinked bin master from the selected conversion.
use avid_reader::{
    file::{AvbFile, Object},
    io::MobId,
    objects::{Node, Value},
    source::SourceMaster,
};
use std::collections::HashSet;

const MAX_TARGETS: usize = 1_000_000;

#[derive(Default)]
pub(super) struct References {
    targets: HashSet<MobId>,
    incomplete: Option<String>,
}

impl References {
    pub(super) fn observe(
        &mut self, file: &AvbFile, check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        self.observe_objects(&file.objects, check)
    }

    fn observe_objects(
        &mut self, objects: &[Option<Object>], check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.incomplete.is_some() {
            return Ok(());
        }
        let mut pending = Vec::new();
        let mut seen = vec![false; objects.len()];
        for (index, object) in objects.iter().enumerate().skip(1) {
            check()?;
            let Some(Object::Decoded(node)) = object else { continue };
            // Protect references even in hidden clips, subclips and rejected
            // routes. A CMPO's own Mob ID is its identity, not a reference.
            if node.class == *b"CMPO" {
                pending.push(index as u32);
            } else if let Some(Value::MobId(id)) = node.get("mob_id") {
                self.targets.insert(*id);
                if self.targets.len() > MAX_TARGETS {
                    self.incomplete = Some("source references exceed the dependency proof budget".into());
                    return Ok(());
                }
            }
        }
        // A missing or opaque editorial component can hide a source reference.
        // Walk all saved Mob bodies, including unselected angles and precomputes.
        while let Some(index) = pending.pop() {
            check()?;
            let Some(visited) = seen.get_mut(index as usize) else {
                self.incomplete = Some("a saved editorial reference is out of range".into());
                break;
            };
            if std::mem::replace(visited, true) {
                continue;
            }
            let node = match objects[index as usize].as_ref() {
                Some(Object::Decoded(node)) => node,
                Some(Object::Unsupported { class, .. }) => {
                    self.incomplete = Some(format!(
                        "an opaque {} component prevents complete dependency proof",
                        String::from_utf8_lossy(class)));
                    break;
                }
                None => {
                    self.incomplete = Some("a saved editorial reference is missing".into());
                    break;
                }
            };
            if node.class == *b"SCLP" && node.get("mob_id").is_none()
                && node.get("length").and_then(Value::as_int) != Some(0)
            {
                self.incomplete = Some("a source clip lacks a full saved package ID".into());
                break;
            }
            for (field, value) in &node.fields {
                // These are annotations or file descriptions. The scan above
                // still protects every decoded Mob ID referenced by them.
                if !matches!(*field, "attributes" | "descriptor" | "physical_media" | "locator") {
                    append_references(value, &mut pending, &seen);
                }
            }
            if pending.len() > objects.len().saturating_mul(4) {
                self.incomplete = Some("the editorial reference walk exceeds its queue budget".into());
                break;
            }
        }
        Ok(())
    }

    pub(super) fn can_omit(&self, master: &SourceMaster) -> bool {
        self.incomplete.is_none()
            && !self.targets.contains(&master.master.mob_id)
            && master.id.contains("/item/")
            && !master.rendered_freeze && master.rendered_gain.is_none()
            && master.pcm_prefix.is_none()
    }

    pub(super) fn incomplete(&self) -> Option<&str> {
        self.incomplete.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avid_reader::io::Uuid;

    #[test]
    fn cross_bin_references_and_opaque_edit_paths_prevent_omission() {
        let id = MobId {
            label: [1; 12], length: 19, instance: [0; 3],
            material: Uuid { data1: 7, data2: 0, data3: 0, data4: [0; 8] },
        };
        let object = |class, fields| Some(Object::Decoded(Node { class, type_name: "Test", fields }));
        let mut refs = References::default();
        refs.observe_objects(&[None, object(*b"CMPO", vec![("mob_id", Value::MobId(id))])],
            &mut || Ok(())).unwrap();
        assert!(!refs.targets.contains(&id));
        refs.observe_objects(&[
            None,
            object(*b"CMPO", vec![("components", Value::Ref(2))]),
            object(*b"SCLP", vec![("mob_id", Value::MobId(id)), ("length", Value::Int(8))]),
        ], &mut || Ok(())).unwrap();
        assert!(refs.targets.contains(&id));
        assert!(refs.incomplete().is_none());
        refs.observe_objects(&[
            None,
            object(*b"CMPO", vec![("components", Value::Ref(2))]),
            Some(Object::Unsupported { class: *b"TEST", size: 4 }),
        ], &mut || Ok(())).unwrap();
        assert!(refs.incomplete().unwrap().contains("opaque TEST"));
    }
}

fn append_references(value: &Value, pending: &mut Vec<u32>, seen: &[bool]) {
    match value {
        Value::Ref(index) if !seen.get(*index as usize).copied().unwrap_or(false) => pending.push(*index),
        Value::RefList(values) | Value::List(values) => {
            for value in values {
                append_references(value, pending, seen);
            }
        }
        Value::Node(node) => {
            let Node { fields, .. } = node.as_ref();
            for (_, value) in fields {
                append_references(value, pending, seen);
            }
        }
        Value::Attrs(entries) => {
            for (_, value) in entries {
                append_references(value, pending, seen);
            }
        }
        _ => {}
    }
}
