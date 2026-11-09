//! Bin grouping and native-clock validation before media planning.
use super::R;
use prproj::build::time::Time;
use prproj::convert::model::{ticks_per_unit, Fraction, Model};
use prproj::convert::rewrap::{display_format, supported_rate};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub(super) fn timecode(ticks: i64, rate: Fraction, drop: bool) -> R<String> {
    if !supported_rate(rate) || display_format(rate, drop).is_none() {
        return Err("unsupported rate/drop-frame combination".into());
    }
    let unit = ticks_per_unit(rate);
    if ticks < 0 || ticks % unit != 0 {
        return Err("source timecode is not on the exact picture clock".into());
    }
    let tc = Time::new(ticks)
        .to_timecode(unit, drop)
        .map_err(|e| e.to_string())?;
    if tc
        .split(':')
        .next()
        .unwrap_or("")
        .parse::<u32>()
        .map_err(|_| "invalid timecode hour")?
        >= 24
    {
        return Err("timecode outside one day".into());
    }
    if (rate.num + rate.den / 2) / rate.den > 100 {
        let split = tc.rfind([':', ';']).ok_or("invalid timecode separator")? + 1;
        return Ok(format!(
            "{}{:03}",
            &tc[..split],
            tc[split..]
                .parse::<u32>()
                .map_err(|_| "invalid frame number")?
        ));
    }
    Ok(tc)
}
/// The bin owns logical channel grouping. Do not let unsupported declarations
/// disappear while projecting to model/1's narrower mono/stereo representation.
pub fn validate_bin_audio(contents: &avid_reader::extract::BinContents) -> R<()> {
    use avid_reader::extract::KIND_SOUND;
    for master in &contents.masters {
        let mut labels = HashSet::new();
        for track in master.tracks.iter().filter(|t| t.kind == KIND_SOUND) {
            if track.index <= 0 || !labels.insert(track.index) {
                return Err(format!(
                    "rewrap: clip {:?} has ambiguous audio track labels",
                    master.name
                ));
            }
        }
        let mut paired = HashSet::new();
        for &(left, right) in &master.stereo_pairs {
            if left == right
                || !labels.contains(&left)
                || !labels.contains(&right)
                || !paired.insert(left)
                || !paired.insert(right)
            {
                return Err(format!(
                    "rewrap: clip {:?} has missing, repeated or overlapping stereo members",
                    master.name
                ));
            }
        }
        for raw in &master.other_groups {
            let (_,group)=avid_reader::extract::parse_multichannel_group(raw).ok_or_else(||format!("rewrap: unsupported bin group {raw:?}"))?;
            for label in group {
                if !labels.contains(&label) || !paired.insert(label) {
                    return Err(format!("rewrap: clip {:?} has missing or overlapping multichannel members",master.name));
                }
            }
        }
    }
    Ok(())
}

/// Opt-in reader correction: use native frame counts at the picture edit rate, not
/// the rounded display base. The standalone reader/default conversion is unchanged.
pub fn normalize_source_clocks(model: &mut Model) -> R<Vec<String>> {
    let mut notes = Vec::new();
    let mut shared = HashMap::new();
    for id in model
        .clips
        .values()
        .map(|c| c.id.clone())
        .collect::<Vec<_>>()
    {
        let clip = model.clips.get_mut(&id).unwrap();
        if clip.audio.len() > 32 {
            return Err("rewrap supports at most 32 discrete mono PCM sources".into());
        }
        let Some(video) = &mut clip.video else {
            continue;
        };
        if !supported_rate(video.rate) {
            return Err(format!("unsupported MXF rewrap rate {}", video.rate));
        }
        let rate = video.rate;
        let unit = ticks_per_unit(rate);
        let mut drop = false;
        for (at, (path, start)) in std::iter::once((&video.path, &mut video.start))
            .chain(clip.audio.iter_mut().map(|a| (&a.path, &mut a.start)))
            .enumerate()
        {
            let id = avid_reader::mxf::read_identity(Path::new(path)).map_err(|e| e.to_string())?;
            let (frames, base, df) = id
                .file_start_timecode()
                .ok_or("missing/ambiguous native source timecode")?;
            if i128::from(base)
                != (i128::from(rate.num) + i128::from(rate.den) / 2) / i128::from(rate.den)
            {
                return Err("source timecode base disagrees with picture rate".into());
            }
            if at == 0 {
                drop = df;
            } else if df != drop {
                return Err("source drop-frame flags disagree".into());
            }
            let exact = i64::try_from(i128::from(frames) * i128::from(unit))
                .map_err(|_| "source start overflow")?;
            timecode(exact, rate, df)?;
            if let Some(prior) = shared.insert(
                fs::canonicalize(path).map_err(|e| e.to_string())?,
                (rate, exact),
            ) {
                if prior != (rate, exact) {
                    return Err("shared source has conflicting picture clocks".into());
                }
            }
            if *start != exact {
                notes.push(format!(
                    "rewrap source clock normalized from native frame count at {rate} fps: {path}"
                ));
                *start = exact;
            }
        }
        clip.time_display = Some(i64::from(
            display_format(rate, drop).ok_or("unsupported drop-frame rate")?,
        ));
    }
    Ok(notes)
}
