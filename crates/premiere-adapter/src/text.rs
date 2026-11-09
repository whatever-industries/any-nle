//! Content/timing preparation; native appearance is a reported loss.
use super::*;
use graph::source::TextKind;
use prproj::convert::prepared::{PreparedText, PreparedTextKind};

pub(super) fn prepare(
    g: &SourceGraph,
    model: &mut PreparedProjectInput,
    losses: &mut Vec<Loss>,
) -> R<()> {
    let count = g
        .sequences
        .iter()
        .try_fold(0usize, |n, s| n.checked_add(s.texts.len()))
        .ok_or_else(|| error("text count overflow"))?;
    if count > model::MAX_ITEMS {
        return Err(error("Premiere text count exceeds limit"));
    }
    for (si, s) in g.sequences.iter().enumerate() {
        let mut captions = Vec::new();
        let mut media: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
        for e in &model.sequences[si].events {
            if let Some(track) = e.video_track {
                let end = e
                    .record_start
                    .checked_add(e.duration)
                    .ok_or_else(|| error("prepared media endpoint overflow"))?;
                media.entry(track).or_default().push((e.record_start, end));
            }
        }
        for spans in media.values_mut() {
            spans.sort_unstable();
            let mut furthest = 0;
            for (_, end) in spans {
                furthest = furthest.max(*end);
                *end = furthest;
            }
        }
        for t in &s.texts {
            let fail = |losses: &mut Vec<Loss>, reason: &str| loss(losses, &t.id, "text", reason);
            if let Some(reason) = &t.reason {
                fail(losses, reason);
                continue;
            }
            let (Some(content), Some(start), Some(duration)) =
                (&t.content, t.record_start, t.duration)
            else {
                fail(losses, "text content or record span unresolved");
                continue;
            };
            if content.is_empty() {
                continue;
            }
            let (Some(start), Some(duration), Some(frame)) = (
                start.to_units(TICKS_PER_SECOND),
                duration.to_units(TICKS_PER_SECOND),
                Rational::new(s.rate.den() as i128, s.rate.num() as i128)
                    .and_then(|r| r.to_units(TICKS_PER_SECOND)),
            ) else {
                fail(
                    losses,
                    "text span is not exactly representable on the destination tick clock",
                );
                continue;
            };
            let Some(end) = start.checked_add(duration) else {
                fail(losses, "text endpoint overflows");
                continue;
            };
            if frame <= 0
                || start % frame != 0
                || duration % frame != 0
                || start < 0
                || duration <= 0
                || end > prproj::caption::builder::SYNTHETIC_DURATION - 3600 * TICKS_PER_SECOND
                || !(1..=64).contains(&t.track)
            {
                fail(
                    losses,
                    "text span or lane is outside the qualified destination bounds",
                );
                continue;
            }
            if t.kind == TextKind::Graphic
                && media.get(&t.track).is_some_and(|spans| {
                    let count = spans.partition_point(|(at, _)| *at < end);
                    count > 0 && spans[count - 1].1 > start
                })
            {
                fail(losses, "title over nonempty picture requires separate compositing preparation; source text retained");
                continue;
            }
            let value = PreparedText {
                sequence: si,
                source_id: t.id.clone(),
                kind: match t.kind {
                    TextKind::Caption => PreparedTextKind::Caption,
                    TextKind::Graphic => PreparedTextKind::Graphic,
                },
                track: t.track,
                start,
                duration,
                content: content.clone(),
            };
            if t.kind == TextKind::Caption {
                captions.push(value);
            } else {
                model.texts.push(value);
            }
        }
        // Premiere enables one subtitle track at a time. Flatten disjoint source
        // caption lanes, and retain/report overlapping cues rather than hide one.
        captions.sort_by(|a, b| {
            (a.start, a.track, &a.source_id).cmp(&(b.start, b.track, &b.source_id))
        });
        let mut overlaps = vec![false; captions.len()];
        let mut furthest: Option<usize> = None;
        for i in 0..captions.len() {
            if let Some(j) = furthest {
                let end = captions[j].start + captions[j].duration;
                if captions[i].start < end {
                    overlaps[i] = true;
                    overlaps[j] = true;
                }
                if captions[i].start + captions[i].duration > end {
                    furthest = Some(i);
                }
            } else {
                furthest = Some(i);
            }
        }
        for (i, c) in captions.into_iter().enumerate() {
            if overlaps[i] {
                loss(losses, &c.source_id, "text", "overlapping SubCaps cannot share the single active Premiere subtitle track; source cue retained");
            } else {
                model.texts.push(c);
            }
        }
    }
    for t in &model.texts {
        loss(losses, &t.source_id, "text_destination_style", match t.kind {
            PreparedTextKind::Caption => "Subtitle text uses destination ArialMT/48 pt defaults; source styling is not reproduced",
            PreparedTextKind::Graphic => "Editable point text uses destination ArialMT/48 pt at normalized position (0.1,0.35); source text-box geometry and appearance are not reproduced",
        });
    }
    Ok(())
}
