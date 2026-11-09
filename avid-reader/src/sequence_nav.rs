//! Sequence position helpers from pinned py-avb `components.py` (MIT; NOTICE).
//! They operate in integer edit units and preserve its transition-overlap and
//! boundary behavior. They do not assert Avid timeline/render equivalence.

use crate::file::AvbFile;
use crate::io::{Error, Result};
use crate::objects::{Node, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentSpan {
    pub object_id: u32,
    pub class: [u8; 4],
    pub length: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionedComponent {
    pub index: usize,
    pub position: i64,
    pub component: ComponentSpan,
}

/// Compute the same sequence positions as py-avb. TNFX subtracts its length
/// before yielding its position, so it overlaps preceding and following clips.
pub fn positions(spans: &[ComponentSpan]) -> Result<Vec<PositionedComponent>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut position = 0i64;
    for (index, &component) in spans.iter().enumerate() {
        if component.length < 0 {
            return Err(Error::new(format!("component {index} has negative length"), 0));
        }
        if component.class == *b"TNFX" {
            position = position.checked_sub(component.length)
                .ok_or_else(|| Error::new("sequence position underflow", 0))?;
        }
        out.push(PositionedComponent { index, position, component });
        if component.class != *b"TNFX" {
            position = position.checked_add(component.length)
                .ok_or_else(|| Error::new("sequence position overflow", 0))?;
        }
    }
    Ok(out)
}

pub fn sequence_length(spans: &[ComponentSpan]) -> Result<i64> {
    let mut total = 0i64;
    for item in spans {
        total = if item.class == *b"TNFX" {
            total.checked_sub(item.length)
        } else {
            total.checked_add(item.length)
        }.ok_or_else(|| Error::new("sequence length overflow", 0))?;
    }
    Ok(total)
}

/// Port of py-avb `Sequence.nearest_index_at_time`. At a non-transition
/// boundary it returns the preceding nonzero component, as py-avb does.
pub fn nearest_index_at_time(spans: &[ComponentSpan], edit_unit: i64) -> Result<Option<(usize, i64)>> {
    let mut last = None;
    for entry in positions(spans)? {
        if entry.component.length == 0 {
            continue;
        }
        if entry.component.class == *b"TNFX"
            && entry.position <= edit_unit
            && edit_unit < entry.position + entry.component.length
        {
            return Ok(Some((entry.index, entry.position)));
        }
        if last.is_some() && entry.position >= edit_unit {
            return Ok(last);
        }
        last = Some((entry.index, entry.position));
    }
    Ok(last)
}

/// Resolve a decoded SEQU's component lengths, including nested sequences.
/// A graph cycle or dangling reference is an error rather than recursion.
pub fn spans_from_file(file: &AvbFile, sequence_id: u32) -> Result<Vec<ComponentSpan>> {
    let sequence = file.node(sequence_id)
        .ok_or_else(|| Error::new(format!("missing sequence {sequence_id}"), 0))?;
    if sequence.class != *b"SEQU" {
        return Err(Error::new(format!("object {sequence_id} is not SEQU"), 0));
    }
    let Value::RefList(refs) = sequence.get("components")
        .ok_or_else(|| Error::new("sequence has no components", 0))? else {
        return Err(Error::new("sequence components are not a reference list", 0));
    };
    let mut result = Vec::with_capacity(refs.len());
    for reference in refs {
        let Value::Ref(id) = reference else {
            return Err(Error::new("sequence has a null component", 0));
        };
        let component = file.node(*id)
            .ok_or_else(|| Error::new(format!("missing component {id}"), 0))?;
        let mut visiting = HashSet::new();
        let length = component_length(file, component, *id, &mut visiting)?;
        result.push(ComponentSpan { object_id: *id, class: component.class, length });
    }
    Ok(result)
}

fn component_length(file: &AvbFile, node: &Node, id: u32, visiting: &mut HashSet<u32>) -> Result<i64> {
    if !visiting.insert(id) {
        return Err(Error::new(format!("component cycle at object {id}"), 0));
    }
    let answer = if node.class == *b"SEQU" {
        let Value::RefList(refs) = node.get("components")
            .ok_or_else(|| Error::new("nested sequence has no components", 0))? else {
            return Err(Error::new("nested sequence component list is malformed", 0));
        };
        let mut total = 0i64;
        for reference in refs {
            let Value::Ref(child_id) = reference else {
                return Err(Error::new("nested sequence has null component", 0));
            };
            let child = file.node(*child_id)
                .ok_or_else(|| Error::new(format!("missing nested component {child_id}"), 0))?;
            let length = component_length(file, child, *child_id, visiting)?;
            total = if child.class == *b"TNFX" { total.checked_sub(length) } else { total.checked_add(length) }
                .ok_or_else(|| Error::new("nested sequence length overflow", 0))?;
        }
        total
    } else {
        node.get("length").and_then(Value::as_int)
            .ok_or_else(|| Error::new(format!("component {id} has no length"), 0))?
    };
    visiting.remove(&id);
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_overlap_positions_and_nearest_follow_pyavb() {
        let spans = [
            ComponentSpan { object_id: 1, class: *b"SCLP", length: 50 },
            ComponentSpan { object_id: 2, class: *b"TNFX", length: 10 },
            ComponentSpan { object_id: 3, class: *b"FILL", length: 0 },
            ComponentSpan { object_id: 4, class: *b"SCLP", length: 40 },
        ];
        let found = positions(&spans).unwrap();
        assert_eq!(found.iter().map(|x| x.position).collect::<Vec<_>>(), [0, 40, 40, 40]);
        assert_eq!(sequence_length(&spans).unwrap(), 80);
        assert_eq!(nearest_index_at_time(&spans, 45).unwrap(), Some((1, 40)));
        assert_eq!(nearest_index_at_time(&spans, 80).unwrap(), Some((3, 40)));
        assert_eq!(nearest_index_at_time(&spans, 0).unwrap(), Some((0, 0)));
    }

    #[test]
    fn malformed_lengths_and_overflow_are_bounded() {
        let mut spans = [ComponentSpan { object_id: 1, class: *b"FILL", length: -1 }];
        assert!(positions(&spans).is_err());
        spans[0].length = i64::MAX;
        assert!(positions(&[spans[0], spans[0]]).is_err());
    }
}
