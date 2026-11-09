//! Read-only, source-pinned py-avb property definitions.
//!
//! These rows describe py-avb's inherited `AVBPropertyDef` declarations. A
//! schema default is distinct from a constructor/factory side effect, and an
//! unobserved field is not evidence that AVB bytes contain that field.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyDefault {
    /// The pinned definition declares no default; it must not be guessed.
    Unspecified,
    Null,
    Bool(bool),
    Int(i64),
    IntList(&'static [i64]),
    Bytes(&'static [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyDefinition {
    pub class_id: [u8; 4],
    pub name: &'static str,
    pub value_type: &'static str,
    /// py-avb class that originally declares this inherited property.
    pub introduced_by: &'static str,
    pub default: PropertyDefault,
    /// True when the pinned fixture corpus actually contains this property.
    pub fixture_observed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassProperties {
    pub class_id: [u8; 4],
    pub registered: bool,
    pub start: usize,
    pub len: usize,
}

mod data {
    use super::{ClassProperties, PropertyDefault, PropertyDefinition};
    include!("property_registry_data.rs");
}

pub use data::{FIELD_LEDGER_SHA256, PYAVB_COMMIT};

pub fn classes() -> &'static [ClassProperties] {
    &data::CLASS_PROPERTIES
}

pub fn property_definitions(class_id: [u8; 4]) -> Option<&'static [PropertyDefinition]> {
    let index = data::CLASS_PROPERTIES
        .binary_search_by_key(&class_id, |row| row.class_id)
        .ok()?;
    let row = &data::CLASS_PROPERTIES[index];
    Some(&data::PROPERTY_DEFS[row.start..row.start + row.len])
}

pub fn property_definition(class_id: [u8; 4], name: &str) -> Option<&'static PropertyDefinition> {
    property_definitions(class_id)?
        .iter()
        .find(|row| row.name == name)
}
