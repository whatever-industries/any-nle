//! Match blocked media tracks to exact saved timeline occurrences for Review.
use super::{av, kind, original::OriginalIndex, rational, time, HashMap, HashSet, MediaEditDetails,
    MotionMxf, OriginalLocatorEdit, OriginalLocatorTimeline, SelectedMediaIndex};
use std::mem::size_of;

pub(super) fn collect_saved_paths(
    facts: &av::SourceProject,
    findings: impl IntoIterator<Item = (usize, usize)>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MediaEditDetails, String> {
    let mut paths = HashMap::<usize, HashSet<&[String]>>::new();
    for (master, track) in findings {
        let original = facts.masters[master].originals[track].as_ref()
            .expect("locator finding from a saved original track");
        paths.entry(master).or_default().insert(&original.locators);
    }
    let tracks = paths.into_iter().flat_map(|(master, paths)| {
        facts.masters[master].originals.iter().enumerate().filter_map(move |(track, original)| {
            original.as_ref().filter(|original| paths.contains(original.locators.as_slice()))
                .map(|_| (master, track))
        })
    });
    collect_tracks(facts, tracks, check)
}

pub(super) fn collect_package_conflicts(
    facts: &av::SourceProject,
    originals: &OriginalIndex,
    selected: &SelectedMediaIndex<'_>,
    motion: &MotionMxf<'_>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MediaEditDetails, String> {
    let mut tracks = Vec::new();
    for (master, source) in facts.masters.iter().enumerate() {
        check()?;
        if originals.omitted_unused.contains_key(&source.id)
            || originals.omitted_media.contains_key(&source.id)
            || motion.fallbacks.omitted_caches.contains(&source.master.mob_id)
        {
            continue;
        }
        for (ordinal, track) in source.master.tracks.iter().enumerate() {
            let package = track.file_mob.umid_hex();
            if selected.index.ambiguous.binary_search(&package).is_ok()
                && !selected.unreferenced_duplicates.contains(package.as_str())
                && !originals.tracks.contains_key(&(source.id.clone(), ordinal))
            {
                tracks.push((master, ordinal));
            }
        }
    }
    collect_tracks(facts, tracks, check)
}

pub(super) fn collect_tracks(
    facts: &av::SourceProject,
    findings: impl IntoIterator<Item = (usize, usize)>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<MediaEditDetails, String> {
    let mut affected = HashMap::<usize, HashSet<usize>>::new();
    for (master, track) in findings {
        affected.entry(master).or_default().insert(track);
    }
    let mut owners = HashMap::new();
    for &ordinal in affected.keys() {
        owners.insert(facts.masters[ordinal].master.mob_id, Some(ordinal));
    }
    for (ordinal, master) in facts.masters.iter().enumerate() {
        check()?;
        let mob = master.master.mob_id;
        if owners.get(&mob).is_some_and(|owner| *owner != Some(ordinal)) {
            owners.insert(mob, None);
        }
    }

    let mut tracks = HashMap::new();
    for (mob, ordinal) in owners {
        check()?;
        let Some(ordinal) = ordinal else { continue };
        let master = &facts.masters[ordinal];
        let mut labels = HashMap::new();
        for (index, track) in master.master.tracks.iter().enumerate() {
            let Some(kind) = kind(track.kind) else { continue };
            let blocked = affected[&ordinal].contains(&index).then_some(track);
            labels.entry((kind, track.index)).and_modify(|unique| *unique = None).or_insert(blocked);
        }
        for ((kind, label), blocked) in labels {
            if let Some(track) = blocked {
                tracks.insert((mob, kind, label), (master, track.file_mob));
            }
        }
    }
    let mut details = MediaEditDetails::default();
    if tracks.is_empty() { return Ok(details); }

    let mut bytes = 0;
    for sequence in &facts.sequences {
        check()?;
        let mut edits = Vec::new();
        for edit in &sequence.events {
            check()?;
            let event = &edit.event;
            let Some(kind) = kind(event.kind) else { continue };
            let Some((master, file_package)) = tracks.get(&(event.master, kind, event.master_track)) else {
                continue;
            };
            if edits.is_empty() {
                bytes += size_of::<OriginalLocatorTimeline>() + sequence.id.len()
                    + sequence.info.name.len();
            }
            // Names are repeated in the Review rows. Account for those strings
            // and the rendered lane and position in the same diagnostic budget.
            bytes += size_of::<OriginalLocatorEdit>() + edit.id.len() + master.id.len()
                + master.master.name.len() * 4 + sequence.info.name.len() + 192;
            if bytes > super::index::REVIEW_DETAIL_BUDGET {
                details.truncated = true;
                break;
            }
            let offset = edit.range.as_ref()
                .filter(|range| Some(range.record_rate) == sequence.rate)
                .and_then(|range| time(range.record_start, range.record_rate).ok());
            edits.push(OriginalLocatorEdit {
                id: edit.id.clone(), kind, track: event.track, offset,
                master_id: master.id.clone(), master_name: master.master.name.clone(),
                master_track: event.master_track,
                file_package: file_package.umid_hex(),
            });
        }
        if !edits.is_empty() {
            let clock = sequence.clock.filter(|clock| Some(clock.rate) == sequence.rate)
                .and_then(|clock| Some((time(i64::try_from(clock.count).ok()?, clock.rate).ok()?,
                    clock.display_base, clock.drop)));
            details.timelines.push(OriginalLocatorTimeline {
                id: sequence.id.clone(), name: sequence.info.name.clone(), bin: sequence.info.bin,
                rate: sequence.rate.map(rational), clock, edits,
            });
        }
        if details.truncated { break; }
    }
    Ok(details)
}
