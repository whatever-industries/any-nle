//! Verify the physical PCM siblings of an Avid master with an MPEG tail.
use crate::{index::MediaIndex, original, KIND_SOUND};
use avid_reader::{io::MobId, source::SourceProject};
use graph::Value;
use std::{collections::HashSet, path::Path};

pub(super) fn verify(
    facts: &SourceProject,
    index: &MediaIndex,
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<HashSet<(MobId, i64)>, String> {
    let mut checked = HashSet::new();
    let mut verified = HashSet::new();
    for m in facts.masters.iter().filter(|m| m.pcm_prefix.is_some()) {
        for track in m.master.tracks.iter().filter(|t| t.kind == KIND_SOUND) {
            let witness = (track.file_mob, track.length);
            if !checked.insert(witness) {
                continue;
            }
            let package = track.file_mob.umid_hex();
            let Some(path) = index.files.get(&package) else {
                continue;
            };
            check()?;
            let id = index.identity(path)?;
            if id.file_packages != [package]
                || id.file_essence_tracks != 1
                || id.file_tracks != [(48_000, 1, track.length)]
            {
                continue;
            }
            let Ok(probe) = original::probe_offline_atom(path, ffprobe) else {
                continue;
            };
            let ul = probe
                .get("format")
                .and_then(|v| v.get("tags"))
                .and_then(|tags| original::text(tags, "operational_pattern_ul"));
            let Some(Value::Arr(streams)) = probe.get("streams") else {
                continue;
            };
            let mut essence = streams
                .iter()
                .filter(|s| original::text(s, "codec_type").as_deref() != Some("data"));
            let Some(audio) = essence.next() else {
                continue;
            };
            if !matches!(
                ul.as_deref(),
                Some("060e2b34.04010102.0d010201.10010000" | "060e2b34.04010102.0d010201.10030000")
            ) || essence.next().is_some()
                || original::text(audio, "codec_type").as_deref() != Some("audio")
                || original::text(audio, "codec_name").as_deref() != Some("pcm_s16le")
                || original::text(audio, "time_base").as_deref() != Some("1/48000")
                || original::number(audio, "sample_rate") != Some(48_000)
                || original::number(audio, "channels") != Some(1)
                || original::number(audio, "start_pts") != Some(0)
                || original::number(audio, "duration_ts") != Some(track.length)
                || !original::complete_mono_pcm_packets(path, ffprobe, track.length, 2)
            {
                continue;
            }
            verified.insert(witness);
        }
    }
    Ok(verified)
}
