//! Disk storage for bulky native receipts during linked whole-project preparation.
use super::*;
use std::{fs::File, io::{BufWriter, Seek, SeekFrom, Write}};

const MAX_SPOOL_BYTES: u64 = 16 << 30;

pub(super) struct ReceiptSpool {
    writer: BufWriter<File>,
    pub(super) bytes: u64,
    pub(super) records: usize,
    pub(super) losses: usize,
}

impl ReceiptSpool {
    pub(super) fn new() -> Result<Self, String> {
        let file = tempfile::tempfile().map_err(|e| format!("Create graph receipt spool: {e}"))?;
        Ok(Self { writer: BufWriter::new(file), bytes: 0, records: 0, losses: 0 })
    }

    pub(super) fn archive(&mut self, owner: &str, data: &Value) -> Result<(), String> {
        let mut remaining = LimitedWrite { writer: &mut self.writer, remaining: MAX_SPOOL_BYTES - self.bytes };
        let bytes = write_source_receipt(&mut remaining, owner, data).map_err(|e| e.to_string())?;
        self.bytes += bytes as u64;
        self.records += 1;
        Ok(())
    }

    pub(super) fn archive_loss(&mut self, owner: &str, aspect: &str, reason: String) -> Result<(), String> {
        self.archive(owner, &obj(vec![("aspect", s(aspect)), ("reason", Value::Str(reason))]))?;
        self.losses += 1;
        Ok(())
    }

    /// Returns true when native PNG recovery may still consume source losses.
    pub(super) fn compact_timeline(&mut self, id: &str, rate: Rational, rows: &mut [Sidecar]) -> Result<bool, String> {
        let Some(receipt) = rows.iter_mut().find(|s| s.adapter == "avid.source" && s.object == id) else {
            return Ok(false);
        };
        self.archive(id, &receipt.data)?;
        let Value::Obj(fields) = &mut receipt.data else {
            return Err("Native sequence receipt must be an object".into());
        };
        let issues = fields.iter().find(|(key, _)| key == "unresolved").map(|(_, value)| value);
        let native_png = matches!(issues, Some(Value::Arr(issues)) if issues.iter().any(|issue|
            matches!(issue.get("picture_profile"), Some(Value::Str(profile)) if profile == "imported_png_matte_controls")));
        let needs_neighbor_issues = native_png || matches!(issues, Some(Value::Arr(issues)) if issues.iter().any(|issue|
            issue.get("disabled_clip").is_some() || matches!(issue.get("picture_profile"), Some(Value::Str(profile))
                if profile == "plain_layered_submaster")));
        // These issue consumers check all neighbors. A Color Adapter lookup
        // only consumes its own record, and only with an existing placement.
        if !needs_neighbor_issues {
            if let Some((_, Value::Arr(issues))) = fields.iter_mut().find(|(key, _)| key == "unresolved") {
                issues.retain(|row|
                    matches!(row.get("picture_profile"), Some(Value::Str(profile))
                        if profile == "imported_png_matte_overlay")
                    || (rate == Rational::integer(24) && matches!(
                        row.get("color_adapter").and_then(|facts| facts.get("placement")), Some(Value::Str(_)))));
                issues.shrink_to_fit();
            }
        }
        fields.retain(|(key, _)| key != "extraction_issues");
        // The native event consumers require exact 24 fps. Pan, gain and
        // Color Adapter select by portable placement identity. Native PNG is
        // the exception: it checks the entire unresolved event list.
        if rate != Rational::integer(24) {
            fields.retain(|(key, _)| key != "native_events");
        } else {
            if !native_png {
                if let Some((_, Value::Arr(events))) = fields.iter_mut().find(|(key, _)| key == "native_events") {
                    events.retain(|row| matches!(row.get("placement_id"), Some(Value::Str(_))));
                    events.shrink_to_fit();
                }
            }
        }
        for (key, data) in fields {
            let Value::Arr(rows) = data else { continue };
            let read_keys: &[&str] = match key.as_str() {
                "marker_native" => &["graph_index", "created"],
                "native_transitions" => &["id", "effect", "audio_curve_attribute", "audio_curve_parameter",
                    "outgoing_filler_frames", "incoming_filler_frames", "gain_gap_edges"],
                _ => continue,
            };
            for row in rows {
                if let Value::Obj(fields) = row {
                    let has_filler = key == "native_transitions"
                        && fields.iter().any(|(key, _)|
                            matches!(key.as_str(), "outgoing_filler_frames" | "incoming_filler_frames"));
                    fields.retain(|(key, _)| read_keys.contains(&key.as_str())
                        || (has_filler && key == "rate"));
                    fields.shrink_to_fit();
                }
            }
        }
        Ok(native_png)
    }

    pub(super) fn finish(&mut self) -> Result<(), String> {
        self.writer.flush().map_err(|e| format!("Flush graph receipt spool: {e}"))
    }

    pub(super) fn into_reader(self) -> Result<File, String> {
        let mut file = self.writer.into_inner().map_err(|e| format!("Flush graph receipt spool: {e}"))?;
        file.seek(SeekFrom::Start(0)).map_err(|e| format!("Rewind graph receipt spool: {e}"))?;
        Ok(file)
    }
}

// The spool limit is enforced before each write, including for partial records.
struct LimitedWrite<'a, W> {
    writer: &'a mut W,
    remaining: u64,
}

impl<W: Write> Write for LimitedWrite<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other("temporary graph receipts exceed 16 GiB"));
        }
        let sent = self.writer.write(bytes)?;
        self.remaining -= sent as u64;
        Ok(sent)
    }

    fn flush(&mut self) -> std::io::Result<()> { self.writer.flush() }
}

fn slots<T>(rows: &Vec<T>) -> usize {
    rows.capacity() * size_of::<T>()
}

fn strings(rows: &Vec<String>) -> usize {
    slots(rows) + rows.iter().map(String::capacity).sum::<usize>()
}

fn opt(value: &Option<String>) -> usize {
    value.as_ref().map_or(0, String::capacity)
}

fn binding(b: &SourceBinding) -> usize {
    strings(&b.placements) + opt(&b.reason)
}

/// Recount live graph data after timeline-local scratch estimates and
/// archived payloads have gone away. Charge project Vec slack per row.
pub(super) fn charge_timeline(
    budget: &mut Budget, seq: &SourceSequence, sidecars: &[Sidecar],
    losses: &[Loss], uses: &[MulticamUse], diagnostics: &[String],
) -> Result<(), String> {
    budget.charge(2 * size_of::<SourceSequence>() + seq.id.capacity() + seq.name.capacity()
        + slots(&seq.tracks) + slots(&seq.placements) + slots(&seq.disabled_placements)
        + slots(&seq.transitions) + slots(&seq.markers) + slots(&seq.texts))?;
    if let Some(c) = &seq.clock {
        budget.charge(c.provenance.source_package.capacity() + c.provenance.timecode_track.capacity()
            + c.provenance.route.capacity())?;
    }
    for track in &seq.tracks {
        budget.charge(opt(&track.name))?;
    }
    for p in seq.placements.iter().chain(&seq.disabled_placements) {
        budget.charge(p.id.capacity() + p.item.capacity() + slots(&p.audio_tracks)
            + slots(&p.audio_streams) + slots(&p.grouped_audio) + slots(&p.volume_keys)
            + slots(&p.track_gains_db) + slots(&p.track_pans) + slots(&p.effects)
            + p.frame_map.as_ref().map_or(0, slots))?;
        for group in &p.grouped_audio {
            budget.charge(slots(&group.streams) + slots(&group.pans))?;
        }
        for effect in &p.effects {
            budget.charge(effect.kind.capacity())?;
            budget.value_payload(&effect.params)?;
        }
    }
    for t in &seq.transitions {
        budget.charge(t.id.capacity() + t.style.capacity() + binding(&t.outgoing) + binding(&t.incoming))?;
    }
    for m in &seq.markers {
        budget.charge(m.name.capacity() + m.comment.capacity() + m.user.capacity()
            + m.owner_occurrence.capacity() + binding(&m.binding)
            + m.color.as_ref().map_or(0, |c| c.name.capacity() + c.palette.capacity()))?;
    }
    for text in &seq.texts {
        budget.charge(text.id.capacity() + opt(&text.content) + opt(&text.reason))?;
    }
    for row in sidecars {
        budget.charge(2 * size_of::<Sidecar>() + row.adapter.capacity() + row.object.capacity())?;
        budget.value_payload(&row.data)?;
    }
    for row in losses {
        budget.charge(2 * size_of::<Loss>() + row.object.capacity() + row.aspect.capacity() + row.reason.capacity())?;
    }
    for row in uses {
        budget.charge(2 * size_of::<MulticamUse>() + row.id.capacity() + row.sequence.capacity()
            + strings(&row.candidates) + binding(&row.binding) + opt(&row.reason))?;
    }
    for row in diagnostics {
        budget.charge(2 * size_of::<String>() + row.capacity())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};

    #[test]
    fn disk_receipts_preserve_native_events_while_working_projection_respects_rate() {
        let data = obj(vec![
            ("native_events", Value::Arr(vec![
                obj(vec![("placement_id", s("clip-a")), ("route", s("one\ntwo"))]),
                obj(vec![("placement_id", Value::Null)]),
            ])),
            ("compatibility_start", s("3600")),
        ]);
        let mut rows = [Sidecar {
            adapter: "avid.source".into(), object: "sequence:0".into(), data: data.clone(),
        }];
        let mut spool = ReceiptSpool::new().unwrap();
        spool.compact_timeline("sequence:0", Rational::integer(24), &mut rows).unwrap();
        let Some(Value::Arr(kept)) = rows[0].data.get("native_events") else { panic!("missing event view") };
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].get("placement_id"), Some(&s("clip-a")));
        let mut other = [Sidecar { adapter: "avid.source".into(), object: "sequence:0".into(), data: data.clone() }];
        spool.compact_timeline("sequence:0", Rational::new(24000, 1001).unwrap(), &mut other).unwrap();
        assert_eq!(other[0].data, obj(vec![("compatibility_start", s("3600"))]));
        spool.finish().unwrap();
        let mut file = spool.writer.into_inner().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text.len() as u64, spool.bytes);
        assert_eq!(text.lines().count(), spool.records);
        for line in text.lines() {
            let recovered = graph::json::parse(line).unwrap();
            assert_eq!(recovered.get("owner"), Some(&s("sequence:0")));
            assert_eq!(recovered.get("data"), Some(&data));
        }
    }
}
