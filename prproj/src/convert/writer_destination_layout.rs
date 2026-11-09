//! Optional project-panel hierarchy; never a filesystem destination or model/1 field.
use super::{other, Result};
use crate::convert::model::Bin;

/// One import root, plus relative parent folders in the same order as the input bins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationBinLayout {
    pub root_name: String,
    pub parents: Vec<Vec<String>>,
}

const MAX_BYTES: usize = 64 << 20;
const MAX_NAME: usize = 4096;
// Conservative charges include vector/string references and the transient parent index.
const ROOT_REFS: usize = 64;
const BIN_REFS: usize = 64;
const COMPONENT_REFS: usize = 128;
fn add_bytes(total: &mut usize, count: usize) -> Result<()> {
    *total = total
        .checked_add(count)
        .ok_or_else(|| other("destination bin layout size overflow"))?;
    if *total > MAX_BYTES {
        return Err(other("destination bin layout exceeds 64 MiB"));
    }
    Ok(())
}
fn name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.len() > MAX_NAME
        || matches!(name, "." | "..")
        || name.chars().any(|c| {
            c == '/' || c == '\\' || c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}')
        })
    {
        return Err(other("invalid destination bin layout component"));
    }
    Ok(())
}
impl DestinationBinLayout {
    pub(super) fn validate(&self, bins: &[Bin]) -> Result<()> {
        if self.parents.len() != bins.len() {
            return Err(other("destination bin layout must match input bin count"));
        }
        name(&self.root_name)?;
        let mut bytes = ROOT_REFS;
        add_bytes(&mut bytes, self.root_name.len())?;
        for (parents, bin) in self.parents.iter().zip(bins) {
            // The import root and actual bin count toward the maximum depth.
            if parents.len() > 62 {
                return Err(other("destination bin layout exceeds 64 levels"));
            }
            add_bytes(&mut bytes, BIN_REFS)?;
            name(&bin.name)?;
            add_bytes(&mut bytes, COMPONENT_REFS + bin.name.len())?;
            for component in parents {
                name(component)?;
                add_bytes(&mut bytes, COMPONENT_REFS + component.len())?;
            }
        }
        Ok(())
    }
}
