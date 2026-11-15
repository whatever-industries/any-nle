//! Save the bounded eight-mono BWF import chain without opening the original file.
use super::*;

pub(super) fn capture(
    ctx: &Ctx<'_>,
    mob: &Node,
    master: &MasterClip,
    offsets: &[Observed<i64>],
    clocks: &[Option<SavedFileClock>],
    ambiguous: &HashSet<MobId>,
    budget: &SharedBudget,
) -> Option<Box<SavedPcmImport>> {
    let frames = master.length;
    if master.edit_rate != 24.0
        || frames <= 0
        || int(mob, "usage_code") != Some(0)
        || ambiguous.contains(&master.mob_id)
        || !master.stereo_pairs.is_empty()
        || !master.other_groups.is_empty()
        || master.tracks.len() != 8
        || offsets.len() != 8
        || clocks.len() != 8
        || ctx.tracks(mob).len() != 8
    {
        return None;
    }
    let settings = import_settings(ctx, mob)?;
    let setting = |name| unique_attribute(settings, name).and_then(Value::as_int);
    if setting("_ATN_IMPORT_AUD_BWF_MONO_GROUP") != Some(1)
        || setting("_ATN_IMPORT_AUD_USE_GAIN") != Some(0)
        || setting("_ATN_IMPORT_AUD_SAMPSIZE_CVT") != Some(0)
        || unique_attribute(settings, "_CHANNEL_GROUP_LIST").and_then(Value::as_str) != Some("")
    {
        return None;
    }
    let import_locator = ctx.deref(unique_attribute(settings, "_SRCFILE"))?;
    if import_locator.class != *b"FILE" {
        return None;
    }

    let mut lanes = [None; 8];
    let mut physical_package = None;
    let mut start_frame = None;
    for (i, (track, clock)) in master.tracks.iter().zip(clocks).enumerate() {
        let clock = clock.as_ref()?;
        let channel = usize::try_from(track.index.checked_sub(1)?).ok()?;
        if channel >= lanes.len()
            || track.kind != KIND_SOUND
            || track.clip_length != frames
            || track.length != frames.checked_mul(2000)?
            || track.edit_rate != 48000.0
            || !matches!(
                track.essence,
                Some(Essence::Sound {
                    sample_rate: 48000.0,
                    bits: 16,
                    channels: 1
                })
            )
            || !matches!(offsets[i], Observed::Known(0))
            || clock.rate != Rate::new(24, 1)?
            || clock.base != 24
            || clock.drop
            || clock.offset != 0
            || !(0..24 * 60 * 60 * 24).contains(&clock.count)
            || clock.file_track != track.index
            || ambiguous.contains(&track.file_mob)
            || ambiguous.contains(&clock.package)
            || track.file_mob == clock.package
            || track.file_mob == master.mob_id
            || master.mob_id == clock.package
            || physical_package.is_some_and(|id| id != clock.package)
            || start_frame.is_some_and(|value| value != clock.count)
            || lanes
                .iter()
                .flatten()
                .any(|lane: &SavedPcmImportLane| lane.file_package == track.file_mob)
        {
            return None;
        }
        let file = ctx.mobs.get(&track.file_mob)?;
        if int(file, "mob_type_id") != Some(MOB_SOURCE)
            || ctx
                .deref(file.get("descriptor"))
                .is_none_or(|d| d.class != *b"PCMA")
        {
            return None;
        }
        let tracks = ctx.tracks(file);
        let [file_track] = tracks.as_slice() else {
            return None;
        };
        let clip = ctx.deref(file_track.get("component"))?;
        // In this profile master label, file label and saved physical label
        // agree. A permutation remains unresolved until a separate fixture.
        if clip.class != *b"SCLP" || int(clip, "track_id") != Some(track.index) {
            return None;
        }
        let lane = SavedPcmImportLane {
            master_track: track.index,
            file_package: track.file_mob,
            file_track: clock.file_track,
            physical_track: track.index,
        };
        if lanes[channel].replace(lane).is_some() {
            return None;
        }
        physical_package = Some(clock.package);
        start_frame = Some(clock.count);
    }
    let physical_package = physical_package?;
    let physical = ctx.mobs.get(&physical_package)?;
    let mut seen = [false; 8];
    let mut timecode = false;
    let physical_tracks = ctx.tracks(physical);
    if physical_tracks.len() != 9 {
        return None;
    }
    for tr in physical_tracks {
        let index = int(tr, "index")?;
        let component = ctx.deref(tr.get("component"))?;
        if int(component, "length") != Some(frames) || float(component, "edit_rate") != Some(24.0) {
            return None;
        }
        match int(component, "media_kind_id")? {
            KIND_SOUND
                if component.class == *b"SCLP"
                    && int(component, "track_id") == Some(0)
                    && int(component, "start_time") == Some(0) =>
            {
                let sound = seen.get_mut(usize::try_from(index.checked_sub(1)?).ok()?)?;
                if std::mem::replace(sound, true) {
                    return None;
                }
            }
            KIND_TIMECODE if index == 1 && component.class == *b"TCCP" && !timecode => {
                timecode = true;
            }
            _ => return None,
        }
    }
    if !timecode || seen.contains(&false) {
        return None;
    }
    let descriptor = ctx.deref(physical.get("descriptor"))?;
    if descriptor.class != *b"MDES" || int(descriptor, "mob_kind") != Some(5) {
        return None;
    }
    let physical_locator = ctx.deref(descriptor.get("locator"))?;
    if physical_locator.class != *b"FILE" {
        return None;
    }
    let path = unique_wav_path(import_locator)?;
    if unique_wav_path(physical_locator)? != path {
        return None;
    }
    let [Some(a), Some(b), Some(c), Some(d), Some(e), Some(f), Some(g), Some(h)] = lanes else {
        return None;
    };
    if !charge(budget, size_of::<SavedPcmImport>()) {
        return None;
    }
    Some(Box::new(SavedPcmImport {
        locator: copy_text(budget, path),
        physical_package,
        start_frame: start_frame?,
        frames,
        lanes: [a, b, c, d, e, f, g, h],
    }))
}

fn unique_wav_path(locator: &Node) -> Option<&str> {
    let mut paths = ["path_posix", "path_utf8", "path"]
        .into_iter()
        .filter_map(|field| locator.get(field).and_then(Value::as_str))
        .filter(|path| !path.is_empty());
    let path = paths.next()?;
    if path.len() > 4096
        || !path
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("wav"))
        || paths.any(|other| other != path)
    {
        return None;
    }
    Some(path)
}
