//! Preserve saved subclip observations without changing existing timeline routing.
use super::*;
use std::collections::BTreeMap;

pub(super) fn append(
    subclips: &[av::SourceSubclip],
    graph: &mut SourceGraph,
    budget: &mut Budget,
) -> Result<(), String> {
    let mut by_bin: BTreeMap<usize, Vec<Value>> = BTreeMap::new();
    let integer = |n: Option<i64>| n.map_or(Value::Null, Value::Int);
    let clock = |r: Option<Rate>| r.map_or(Value::Null, |r| s(rational(r)));
    let mob = |id: Option<avid_reader::io::MobId>| id.map_or(Value::Null, |id| s(id.umid_hex()));
    for subclip in subclips {
        if graph.bins.get(subclip.bin).is_none() {
            return Err("subclip references unknown bin".into());
        }
        // Include object fields, container slots and fixed-width class, rate
        // and UMID strings before allocating. The copied names and IDs vary.
        let size = subclip.tracks.len().checked_mul(1024)
            .and_then(|n| n.checked_add(2048))
            .and_then(|n| n.checked_add(subclip.id.len()))
            .and_then(|n| n.checked_add(subclip.name.len()))
            .ok_or("subclip graph accounting overflow")?;
        budget.charge(size)?;
        graph.losses.push(Loss {
            object: graph.bins[subclip.bin].id.clone(),
            aspect: "subclip_bin_item".into(),
            reason: format!("{}: saved subclip is retained as a source receipt", subclip.id),
        });
        by_bin.entry(subclip.bin).or_default().push(obj(vec![
            ("id", s(&subclip.id)),
            ("mob", mob(subclip.mob)),
            ("name", s(&subclip.name)),
            ("rate", clock(subclip.rate)),
            ("backing_length", integer(subclip.backing_length)),
            ("start", observed(&subclip.start)),
            ("end", observed(&subclip.end)),
            ("tracks", Value::Arr(subclip.tracks.iter().map(|t| obj(vec![
                ("label", integer(t.label)),
                ("component_class", t.component_class.map_or(Value::Null, |class| {
                    s(String::from_utf8_lossy(&class))
                })),
                ("kind", integer(t.kind)),
                ("rate", clock(t.rate)),
                ("length", integer(t.length)),
                ("target_mob", mob(t.target_mob)),
                ("target_track", integer(t.target_track)),
                ("target_offset", integer(t.target_offset)),
            ])).collect())),
        ]));
    }
    for (bin, subclips) in by_bin {
        graph.sidecars.push(Sidecar {
            adapter: "avid.subclips".into(),
            object: graph.bins[bin].id.clone(),
            data: Value::Arr(subclips),
        });
    }
    Ok(())
}
