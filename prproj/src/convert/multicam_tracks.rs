//! Editable multicam sources assembled from verified, ordinary linked pieces.
use super::{
    model::{self, ModelError},
    prepared::{PreparedClip, PreparedProjectInput},
    writer::{Result, WriteError},
};
use crate::{
    build::time::Time,
    ids::UuidSource,
    model::{ItemId, Project, TrackId},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedTrackMulticam {
    pub id: String,
    pub name: String,
    pub bin: usize,
    pub sequence: usize,
    pub origin: i64,
    pub duration: i64,
    pub picture: Vec<Vec<Piece>>,
    pub mono: Vec<Piece>,
    pub uses: Vec<Use>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub clip: String,
    pub start: i64,
    pub source_in: i64,
    pub duration: i64,
    /// Zero for picture. Audio uses the imported clip's logical mono label.
    pub channel: i64,
    pub fit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    /// None for the independent mono output.
    pub angle: Option<usize>,
    pub track: i64,
    pub start: i64,
    pub source_in: i64,
    pub duration: i64,
}

pub(super) fn validate(
    input: &PreparedProjectInput,
    clips: &HashMap<&str, &PreparedClip>,
) -> std::result::Result<(), ModelError> {
    let fail = |reason: &str| ModelError(format!("editable linked multicam: {reason}"));
    if input.multicam_tracks.len() > super::multicam::MAX_GROUPS {
        return Err(fail("source item count exceeds limit"));
    }
    let mut ids = HashSet::new();
    let mut ranges: HashMap<_, Vec<_>> = HashMap::new();
    let mut pieces_and_uses = 0usize;
    for group in &input.multicam_tracks {
        model::check_text(&group.id, "multicam identity", 256, false)?;
        model::check_text(&group.name, "multicam name", 4096, false)?;
        let sequence = input
            .sequences
            .get(group.sequence)
            .ok_or_else(|| fail("missing sequence"))?;
        let unit = model::ticks_per_unit(sequence.rate);
        let aligned = |value: i64| value >= 0 && value % unit == 0;
        if !ids.insert(&group.id)
            || clips.contains_key(group.id.as_str())
            || group.bin >= input.bins.len()
            || !aligned(group.origin)
            || !aligned(group.duration)
            || group.duration == 0
            || group.uses.is_empty()
            || group.picture.len() > 16
            || (group.picture.is_empty() && group.mono.is_empty())
        {
            return Err(fail("invalid source identity, extent or angle count"));
        }
        for (picture, track) in group
            .picture
            .iter()
            .map(|pieces| (true, pieces))
            .chain(std::iter::once((false, &group.mono)))
        {
            if picture && track.is_empty() {
                return Err(fail("empty picture angle"));
            }
            let mut end = 0;
            for piece in track {
                let Some(PreparedClip::Original(original)) =
                    clips.get(piece.clip.as_str()).copied()
                else {
                    return Err(fail(
                        "angle or mono input does not address a verified original",
                    ));
                };
                let extent = if picture {
                    original
                        .video
                        .as_ref()
                        .filter(|video| {
                            video.rate == sequence.rate
                                && piece.channel == 0
                                && (piece.fit
                                    || (video.width, video.height)
                                        == (sequence.width, sequence.height))
                        })
                        .map(|video| video.duration)
                } else {
                    original
                        .audio
                        .as_ref()
                        .filter(|audio| {
                            audio.sample_rate == 48_000
                                && original
                                    .groups
                                    .iter()
                                    .any(|group| group.as_slice() == [piece.channel])
                        })
                        .map(|audio| audio.duration)
                };
                if !aligned(piece.start)
                    || piece.start < end
                    || !aligned(piece.source_in)
                    || piece.duration <= 0
                    || !aligned(piece.duration)
                    || piece
                        .start
                        .checked_add(piece.duration)
                        .is_none_or(|out| out > group.duration)
                    || !extent.is_some_and(|extent| {
                        piece
                            .source_in
                            .checked_add(piece.duration)
                            .is_some_and(|out| out <= extent)
                    })
                {
                    return Err(fail("angle piece overlaps or exceeds its source extent"));
                }
                end = piece.start + piece.duration;
            }
            pieces_and_uses = pieces_and_uses.saturating_add(track.len());
        }
        pieces_and_uses = pieces_and_uses.saturating_add(group.uses.len());
        for edit in &group.uses {
            if !(1..=32).contains(&edit.track)
                || !aligned(edit.start)
                || !aligned(edit.source_in)
                || edit.duration <= 0
                || !aligned(edit.duration)
                || edit
                    .source_in
                    .checked_add(edit.duration)
                    .is_none_or(|out| out > group.duration)
                || edit
                    .angle
                    .map_or(group.mono.is_empty(), |angle| angle >= group.picture.len())
            {
                return Err(fail("invalid multicam edit timing, lane or angle"));
            }
            let end = edit
                .start
                .checked_add(edit.duration)
                .ok_or_else(|| fail("record extent overflow"))?;
            ranges
                .entry((group.sequence, edit.angle.is_some(), edit.track))
                .or_default()
                .push((edit.start, end));
        }
    }
    if pieces_and_uses > model::MAX_ITEMS {
        return Err(fail("piece and edit count exceeds limit"));
    }
    for ((sequence, picture, track), mut intervals) in ranges {
        for edit in &input.sequences[sequence].events {
            let same_track = if picture {
                edit.video_track == Some(track)
            } else {
                edit.audio_tracks.contains(&track)
                    || edit.stereo.iter().any(|route| route.audio_track == track)
            };
            if same_track {
                intervals.push((
                    edit.record_start,
                    edit.record_start
                        .checked_add(edit.duration)
                        .ok_or_else(|| fail("record extent overflow"))?,
                ));
            }
        }
        intervals.sort_unstable();
        if intervals.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(fail("overlapping destination edits"));
        }
    }
    Ok(())
}

fn fail(message: &str) -> WriteError {
    WriteError::Other(crate::Error::Value(format!(
        "editable linked multicam: {message}"
    )))
}

pub(super) fn build<'a>(
    project: &mut Project,
    group: &PreparedTrackMulticam,
    sequence: &model::Sequence,
    target: ItemId,
    ids: &mut dyn UuidSource,
    leaf: impl Fn(&str) -> Option<(ItemId, &'a super::prepared::OriginalClip)>,
) -> Result<ItemId> {
    let unit = model::ticks_per_unit(sequence.rate);
    let display = super::rewrap::display_format(sequence.rate, false)
        .ok_or_else(|| fail("unknown display clock"))?;
    let fragment = crate::build::multicam::build_multicam_mono_audio_with_outputs(
        &mut project.doc.tree,
        ids,
        &group.name,
        unit,
        sequence.width,
        sequence.height,
        i64::from(display),
        group.duration,
        group.picture.len(),
        1,
        std::iter::once(0),
    )?;
    let nodes = project.splice(fragment, ids)?;
    let (Some(sequence_element), Some(item_element), Some(master)) = (
        nodes.get("Sequence"),
        nodes.get("ClipProjectItem"),
        nodes.get("MasterClip"),
    ) else {
        return Err(fail("missing source fragment objects"));
    };
    let seq = project.parse_sequence(sequence_element)?;
    project.set_zero_point(seq, Time::new(group.origin))?;
    super::combined::premiere_balances(project, sequence_element)?;
    let video = project.video_tracks(seq).to_vec();
    let audio = project.audio_tracks(seq)[0];
    for (track, pieces) in video.into_iter().zip(&group.picture) {
        for piece in pieces {
            let (item, _) =
                leaf(&piece.clip).ok_or_else(|| fail("picture original was not imported"))?;
            let placed = project.add_clip(track, item, Time::new(piece.start), ids)?;
            if piece.fit {
                super::writer::fit_picture_edit(project, placed);
            }
            trim(
                project,
                placed,
                piece.start,
                piece.source_in,
                piece.duration,
                None,
            )?;
        }
    }
    for piece in &group.mono {
        let (item, original) =
            leaf(&piece.clip).ok_or_else(|| fail("mono original was not imported"))?;
        let channel = original
            .groups
            .iter()
            .position(|group| group.as_slice() == [piece.channel])
            .ok_or_else(|| fail("mono original has no selected group"))?;
        let placed = super::native_av::place_audio(
            project,
            audio,
            item,
            channel,
            Time::new(piece.start),
            ids,
        )?;
        super::combined::set_clip_pan(project, placed, super::combined::Pan::Left)?;
        trim(
            project,
            placed,
            piece.start,
            piece.source_in,
            piece.duration,
            None,
        )?;
    }
    super::multicam::attach_source(
        project,
        target,
        group.duration,
        sequence_element,
        item_element,
        master,
        !group.picture.is_empty(),
        !group.mono.is_empty(),
    )
}

pub(super) fn place(
    project: &mut Project,
    source: ItemId,
    track: TrackId,
    edit: &Use,
    rect: &str,
    ids: &mut dyn UuidSource,
) -> Result<()> {
    let item = project.add_clip(track, source, Time::new(edit.start), ids)?;
    if edit.angle.is_none() {
        project.narrow_multicam_audio(item, source, 0)?;
    }
    trim(
        project,
        item,
        edit.start,
        edit.source_in,
        edit.duration,
        edit.angle.map(|_| rect),
    )?;
    if let Some(angle) = edit.angle {
        super::multicam::select_picture(project, item, angle)?;
    }
    Ok(())
}

fn trim(
    project: &mut Project,
    item: crate::model::TrackItemId,
    start: i64,
    source_in: i64,
    duration: i64,
    rect: Option<&str>,
) -> Result<()> {
    project.set_in_point(item, Time::new(source_in))?;
    project.set_out_point(item, Time::new(source_in + duration))?;
    project.set_end(item, Time::new(start + duration))?;
    super::placement::normalize_placement(project, item, rect)?;
    Ok(())
}
