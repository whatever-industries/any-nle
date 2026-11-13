//! Compare the supported static EQ and dynamics controls on their saved owners.
use super::{mono_row, Diff, Timeline};
use crate::{
    linked::mixer::{StripProcessing, PROCESSING_RECEIPT},
    source::mixer::{DIRECT_PROFILE, FAIRLIGHT_ADAPTER},
};
use graph::Value;
use serde_json::{json, Value as Json};

pub(super) fn compare(
    left: &Timeline<'_>,
    right: &Timeline<'_>,
    left_buses: Option<usize>,
    right_buses: Option<usize>,
    diff: &mut Diff,
) {
    let a = facts(left);
    let b = facts(right);
    // No added processing verdict for older receipts that never captured it.
    if !observes_processing(a) && !observes_processing(b) {
        return;
    }
    for track in 1..=2 {
        compare_strip(
            diff,
            &json!({"kind":"audio", "track":track}),
            track_processing(a, track),
            track_processing(b, track),
        );
    }
    for index in 0..left_buses.zip(right_buses).map_or(0, |(a, b)| a.min(b)) {
        compare_strip(
            diff,
            &json!({"bus":index + 1}),
            bus_processing(a, index, left_buses),
            bus_processing(b, index, right_buses),
        );
    }
}

fn observes_processing(data: Option<&Value>) -> bool {
    let Some(data) = data else { return false };
    data.get(PROCESSING_RECEIPT).is_some()
        || ["tracks", "buses"].into_iter().any(|key| {
            matches!(data.get(key), Some(Value::Arr(rows)) if rows.iter().any(|r| r.get("eq_dynamics_fields").is_some()))
        })
}

fn facts<'a>(timeline: &Timeline<'a>) -> Option<&'a Value> {
    timeline.graph.sidecar(FAIRLIGHT_ADAPTER, &timeline.seq.id)
}

fn track_processing(facts: Option<&Value>, track: i64) -> Option<StripProcessing> {
    let data = facts?;
    let row = mono_row(Some(data), track)?;
    if row.get("strip_id") != Some(&Value::Int(track - 1))
        || !StripProcessing::observes_saved_cells(row, false)
    {
        return None;
    }
    match data.get(PROCESSING_RECEIPT) {
        Some(Value::Arr(strips))
            if strips.len() == 2 && matches!(data.get(DIRECT_PROFILE), Some(Value::Int(1 | 2))) =>
        {
            // This receipt has fixed A1/A2 slots. Confirm each typed value
            // against the raw cells for the matching saved strip.
            StripProcessing::from_receipt(&strips[(track - 1) as usize], row).ok()
        }
        None if StripProcessing::saved_rows_are_default(row) => Some(StripProcessing::DEFAULT),
        _ => None,
    }
}

fn bus_processing(
    data: Option<&Value>,
    index: usize,
    count: Option<usize>,
) -> Option<StripProcessing> {
    let data = data?;
    let Value::Arr(buses) = data.get("buses")? else {
        return None;
    };
    let row = buses.get(index)?;
    if !StripProcessing::observes_saved_cells(row, true) {
        return None;
    }
    let value = StripProcessing::from_bus_receipt(row).ok()?;
    let direct =
        count.is_some_and(|count| data.get(DIRECT_PROFILE) == Some(&Value::Int(count as i64)));
    (direct || value == StripProcessing::DEFAULT).then_some(value)
}

fn compare_strip(
    diff: &mut Diff,
    owner: &Json,
    a: Option<StripProcessing>,
    b: Option<StripProcessing>,
) {
    let (Some(a), Some(b)) = (a, b) else {
        let reason = match (a.is_some(), b.is_some()) {
            (true, false) => "unresolved on the right",
            (false, true) => "unresolved on the left",
            _ => "unresolved on both sides",
        };
        diff.gap("fairlight_processing", owner, reason);
        return;
    };
    if a == b {
        return;
    }
    let (Value::Obj(left), Value::Obj(right)) = (a.receipt(), b.receipt()) else {
        unreachable!("strip receipts are objects");
    };
    for ((name, left), (_, right)) in left.into_iter().zip(right) {
        if left != right {
            let mut scope = owner.clone();
            scope["control"] = json!(name);
            diff.change(
                "fairlight_processing",
                &scope,
                crate::source::compare::graph_value(&left),
                crate::source::compare::graph_value(&right),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linked::mixer::PROCESSING_ROWS;

    fn row(track: i64, processing: StripProcessing) -> Value {
        Value::Obj(vec![
            ("track".into(), Value::Int(track)),
            ("strip_id".into(), Value::Int(track - 1)),
            ("channel_index".into(), Value::Int(0)),
            (
                "eq_dynamics_fields".into(),
                Value::Obj(
                    PROCESSING_ROWS
                        .iter()
                        .zip(processing.cells())
                        .map(|(row, v)| (format!("0x{:03x}", row.id), Value::Int(v.into())))
                        .collect(),
                ),
            ),
        ])
    }

    #[test]
    fn processing_receipts_are_checked_against_their_own_saved_strip() {
        let neutral = StripProcessing::DEFAULT;
        let mut cells = neutral.cells();
        let bell = PROCESSING_ROWS
            .iter()
            .position(|row| row.id == 0xe3)
            .unwrap();
        cells[bell] = 60;
        let bell = StripProcessing::from_cells(cells).unwrap();
        let mut data = Value::Obj(vec![
            // The raw rows are deliberately out of ordinal order.
            (
                "tracks".into(),
                Value::Arr(vec![row(2, bell), row(1, neutral)]),
            ),
            (DIRECT_PROFILE.into(), Value::Int(1)),
            (
                PROCESSING_RECEIPT.into(),
                Value::Arr(vec![neutral.receipt(), bell.receipt()]),
            ),
        ]);
        let mut diff = Diff::default();
        compare_strip(
            &mut diff,
            &json!({"kind":"audio", "track":2}),
            track_processing(Some(&data), 1),
            track_processing(Some(&data), 2),
        );
        assert_eq!(
            diff.differences,
            [
                json!({"field":"fairlight_processing","context":{"kind":"audio","track":2,"control":"bell_gain_tenths_db"},"left":0,"right":60})
            ]
        );
        assert!(diff.gaps.is_empty());

        let Value::Obj(fields) = &mut data else {
            unreachable!()
        };
        let Value::Arr(receipts) = &mut fields[2].1 else {
            unreachable!()
        };
        receipts[1] = neutral.receipt();
        assert_eq!(track_processing(Some(&data), 1), Some(neutral));
        assert_eq!(track_processing(Some(&data), 2), None);
    }
}
