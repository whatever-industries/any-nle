//! File-relative binding for the zero-delay mono MP3 profile.
use super::{Rational, Result, SourceMedia, StreamKind, Value};
use crate::media::{stamp, FileStamp};
use avid_adapter::original::{inspect_mp3_presentation, Mp3Extent};
use graph::source::OriginalStream;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Original {
    path: PathBuf,
    identity: FileStamp,
    extent: Mp3Extent,
}

impl Original {
    pub fn open(locator: &Path, tool: &Path) -> Result<Self> {
        if !locator
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
        {
            return Err("linked MP3 needs a literal .mp3 path".into());
        }
        let path = fs::canonicalize(locator)?;
        let identity = stamp(fs::metadata(&path)?)?;
        let extent = inspect_mp3_presentation(&path, tool)?;
        if extent.sample_rate != 44_100
            || extent.channels != 1
            || extent.start_samples != 0
            || extent.samples != extent.coded_samples
        {
            return Err("the Resolve MP3 link profile requires one 44.1 kHz mono stream with zero decoder delay and padding".into());
        }
        let original = Self {
            path,
            identity,
            extent,
        };
        original.recheck(locator)?;
        Ok(original)
    }

    pub fn recheck(&self, locator: &Path) -> Result<()> {
        if fs::canonicalize(locator)? != self.path
            || stamp(fs::metadata(&self.path)?)? != self.identity
        {
            return Err(format!("verified Resolve MP3 changed: {}", locator.display()).into());
        }
        Ok(())
    }

    pub fn bind(&self, source: &mut SourceMedia, saved: Option<&Value>) -> Result<()> {
        let rate = self.extent.sample_rate;
        // Resolve's observed cache ends up to three samples short of the
        // decoded file. Keep the cached cut coordinates and verify the full
        // playable extent for the Premiere master.
        if source.kind != StreamKind::Audio
            || source.bits.is_some()
            || source.rate.is_some()
            || source.sample_rate != Some(rate)
            || source.start != Some(Rational::ZERO)
            || source.clock.is_some()
            || source.stream != Some(1)
            || !saved.is_some_and(|r| {
                ["saved_cache_track", "saved_selector", "saved_channel"]
                    .into_iter()
                    .all(|field| r.get(field) == Some(&Value::Int(0)))
            })
            || !source.duration.to_units(rate).is_some_and(|cached| {
                self.extent
                    .samples
                    .checked_sub(cached)
                    .is_some_and(|delta| (0..=3).contains(&delta))
            })
        {
            return Err("saved Resolve MP3 clock, mono routing or sample extent contradicts the verified file".into());
        }
        let original = OriginalStream {
            saved_locators: source.locations.clone(),
            container: "mp3".into(),
            stream_index: 0,
            channel_index: Some(0),
            channels: Some(1),
            time_base: Rational::new(1, rate.into()).ok_or("invalid MP3 sample clock")?,
            start_pts: 0,
            duration_ts: self.extent.samples,
            sample_representation: Some("mp3".into()),
        };
        if source
            .original
            .as_ref()
            .is_some_and(|saved| saved != &original)
        {
            return Err("contradictory original MP3 stream facts".into());
        }
        source.duration = Rational::from_units(self.extent.samples, rate)
            .ok_or("invalid MP3 playable duration")?;
        source.online = Some(true);
        source.original = Some(original);
        Ok(())
    }
}
