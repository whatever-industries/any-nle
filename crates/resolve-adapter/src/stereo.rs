//! Saved routing for neutral stereo A1 edits of linked PCM.
use graph::{GroupedAudio, Placement, Rational};
use std::path::Path;

pub(crate) const STREAMS: [i64; 2] = [1, 2];
pub(crate) const CLIP_FIELDS: &str = "0000000200000007800a0220017804";
pub(crate) const SELECTOR: &str = concat!(
    "000000010000000200000014004300680061006e006e0065006c007300420041",
    "0000000c000000001000000002000000020000400100004002",
    "000000120041007500640069006f0054007900700065000000020000000000",
);

pub(crate) fn clock(rate: Rational, size: (i64, i64)) -> bool {
    rate == Rational::integer(25) && size == (1920, 1080)
}

pub(crate) fn mov(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("mov"))
}

pub(crate) fn group() -> GroupedAudio {
    GroupedAudio { track: 1, streams: STREAMS.to_vec(), gain_db: None, pans: vec![] }
}

pub(crate) fn placement(edit: &Placement) -> bool {
    matches!(edit.grouped_audio.as_slice(), [row]
        if row.track == 1 && row.streams == STREAMS && row.gain_db.is_none() && row.pans.is_empty())
        && edit.video_track.is_none()
        && edit.audio_tracks.is_empty()
        && edit.audio_streams.is_empty()
}
