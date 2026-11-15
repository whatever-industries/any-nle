//! Opt-in camera-original lookup for Avid-managed masters.
use super::*;
use avid_reader::extract::{Essence, KIND_PICTURE, KIND_SOUND};

fn saved_locator(master: &av::SourceMaster) -> Result<Option<String>, String> {
    let m = &master.master;
    let mut paths = m.columns.iter().filter(|(name, _)| name == "UNC Path");
    let full = paths.next().map(|(_, value)| value);
    if paths.next().is_some() {
        return Err("duplicate saved UNC Path values".into());
    }
    if m.source_path.is_none() && full.is_none() {
        return Ok(None);
    }
    let name = m
        .source_file
        .as_deref()
        .filter(|name| !matches!(*name, "" | "." | "..") && !name.contains(['/', '\\', ':']))
        .ok_or("Source File is missing or is not a literal filename")?;
    let joined = m
        .source_path
        .as_deref()
        .map(|dir| {
            saved_parts(dir)?;
            Ok::<_, String>(format!("{}/{name}", dir.trim_end_matches(['/', '\\'])))
        })
        .transpose()?;
    if let Some(full) = full {
        let full_parts = saved_parts(full)?;
        if full_parts.1.last().map(String::as_str) != Some(name)
            || joined
                .as_deref()
                .map(saved_parts)
                .transpose()?
                .is_some_and(|path| path != full_parts)
        {
            return Err("Source File / Source Path disagree with the saved UNC Path".into());
        }
        return Ok(Some(full.clone()));
    }
    if let Some(path) = &joined {
        saved_parts(path)?;
    }
    Ok(joined)
}

fn saved_picture(
    master: &av::SourceMaster,
    allow_stereo: bool,
) -> Result<(&av::SavedFileClock, i64), String> {
    let m = &master.master;
    let track = m.tracks.first().ok_or("no saved picture track")?;
    if m.tracks.len() != 1
        && !(allow_stereo
            && m.tracks.len() == 3
            && m.tracks[1..].iter().enumerate().all(|(i, t)| {
                t.kind == KIND_SOUND
                    && t.index == i as i64 + 1
                    && t.clip_length == track.length
                    && track.length.checked_mul(2002) == Some(t.length)
                    && av::Rate::from_avid(t.edit_rate) == Some(av::Rate { num: 48000, den: 1 })
                    && matches!(
                        t.essence,
                        Some(Essence::Sound {
                            sample_rate: 48000.0,
                            bits: 16 | 24,
                            channels: 1
                        })
                    )
            }))
    {
        return Err("camera-original audio requires one saved picture and two mono routes with identical extents".into());
    }
    let clock = master
        .file_clocks
        .first()
        .and_then(Option::as_ref)
        .ok_or("no qualified saved managed picture clock")?;
    if master.rendered_freeze
        || master.rendered_gain.is_some()
        || track.kind != KIND_PICTURE
        || track.index != 1
        || clock.file_track != 1
        || !matches!((clock.rate.num, clock.rate.den), (24, 1) | (24_000, 1001))
        || av::Rate::from_avid(track.edit_rate) != Some(clock.rate)
        || master.rate != Some(clock.rate)
        || (clock.base, clock.drop, clock.offset) != (24, false, 0)
        || !(0..24 * 60 * 60 * 24).contains(&clock.count)
        || m.start
            .as_ref()
            .is_none_or(|tc| tc.frames != clock.count || tc.fps != 24 || tc.drop)
        || master.offsets.first() != Some(&av::Observed::Known(0))
        || track.length <= 0
        || track.length != track.clip_length
        || track.length != m.length
        || !matches!(
            &track.essence,
            Some(Essence::Picture {
                width: 1920,
                height: 1080,
                display_aspect: Some((16, 9)),
                ..
            })
        )
    {
        return Err("the saved managed route requires full-range 1080p picture at 24 or 23.976, an exact non-drop origin and zero offsets".into());
    }
    Ok((clock, track.length))
}

fn qualify(
    clock: &av::SavedFileClock,
    frames: i64,
    saved: String,
    path: PathBuf,
    tool: &Path,
    stereo: bool,
    reel: Option<&str>,
) -> Result<(Selected, Vec<Selected>, bool), String> {
    let rate = Rational::new(clock.rate.num.into(), clock.rate.den.into())
        .ok_or("invalid camera original rate")?;
    let expected = crate::tc(clock.count, 24);
    let probe = probe_metadata(&path, tool)?;
    qualify_container(&path, &probe)?;
    let Some(Value::Arr(streams)) = probe.get("streams") else {
        return Err("camera original has no probe streams".into());
    };
    let (defaulted, audio) = if stereo {
        let [video, audio, tc] = streams.as_slice() else {
            return Err(
                "original requires picture, stereo PCM and one matching timecode track".into(),
            );
        };
        let defaulted = qualify_prores_hq_parts(&path, &probe, video, tc, 2, rate, &expected)?;
        let bits = match text(audio, "codec_name").as_deref() {
            Some("pcm_s16le") => 16,
            Some("pcm_s24le") => 24,
            _ => return Err("original PCM representation is not supported".into()),
        };
        let samples = frames
            .checked_mul(2002)
            .ok_or("original PCM extent overflows")?;
        let tb = Rational::new(1, 48000).unwrap();
        if rate != Rational::new(24000, 1001).unwrap()
            || number(audio, "index") != Some(1)
            || text(audio, "codec_type").as_deref() != Some("audio")
            || number(audio, "channels") != Some(2)
            || number(audio, "sample_rate") != Some(48000)
            || text(audio, "channel_layout").is_some_and(|v| v != "stereo" && v != "unknown")
            || number(audio, "bits_per_sample") != Some(bits)
            || ratio(audio, "time_base") != Some(tb)
            || number(audio, "duration_ts") != Some(samples)
            || number(audio, "start_pts") != Some(0)
        {
            return Err("original stereo PCM layout, phase or extent is unqualified".into());
        }
        let audio = (0..2)
            .map(|channel| Selected {
                path: path.clone(),
                online: true,
                stream: OriginalStream {
                    saved_locators: vec![saved.clone()],
                    container: "mov".into(),
                    stream_index: 1,
                    channel_index: Some(channel),
                    channels: Some(2),
                    time_base: tb,
                    start_pts: 0,
                    duration_ts: samples,
                    sample_representation: Some(format!("pcm{bits}")),
                },
                kind: StreamKind::Audio,
                codec: format!("pcm_s{bits}le"),
                raster: None,
                rate: None,
                sample_rate: Some(48000),
                bits: Some(bits),
                duration: Rational::new(samples.into(), 48000).unwrap(),
                motion_audio_origin: None,
                same_rate_aac_origin: false,
                column_timecode: None,
                embedded_mov_timecode: None,
            })
            .collect();
        (defaulted, audio)
    } else {
        (
            qualify_prores_hq_timecode(&path, &probe, streams, rate, &expected)?,
            vec![],
        )
    };
    let embedded_reel = streams
        .last()
        .and_then(|stream| stream.get("tags"))
        .and_then(|tags| text(tags, "reel_name"))
        .filter(|value| !value.is_empty());
    if let (Some(saved_reel), Some(file_reel)) = (reel, embedded_reel.as_deref()) {
        if saved_reel != file_reel {
            return Err(format!(
                "camera reel {file_reel:?} differs from saved Avid Tape {saved_reel:?}"
            ));
        }
    }
    let video = &streams[0];
    let time_base = ratio(video, "time_base").ok_or("camera original time base is absent")?;
    let duration_ts = number(video, "duration_ts").ok_or("camera original duration is absent")?;
    let duration = Rational::new(
        i128::from(duration_ts) * i128::from(time_base.num()),
        time_base.den().into(),
    )
    .ok_or("camera original duration overflows")?;
    if number(video, "start_pts") != Some(0)
        || number(video, "nb_frames") != Some(frames)
        || Some(duration)
            != Rational::new(
                i128::from(frames) * i128::from(rate.den()),
                rate.num().into(),
            )
        || probe
            .get("format")
            .and_then(|f| f.get("tags"))
            .and_then(|tags| text(tags, "timecode"))
            .is_some_and(|tc| tc != expected)
        || text(video, "color_space").as_deref() != Some("bt709")
        || text(video, "color_range").as_deref() != Some("tv")
        || text(video, "color_primaries") != text(video, "color_transfer")
        || video
            .get("tags")
            .and_then(|tags| tags.get("rotate"))
            .is_some()
    {
        return Err("camera original clock, exact extent, orientation or BT.709 tags disagree with the supported profile".into());
    }
    prove_prores_cadence(&path, tool, video, rate)?;
    Ok((
        Selected {
            path,
            online: true,
            stream: OriginalStream {
                saved_locators: vec![saved],
                container: "mov".into(),
                stream_index: 0,
                channel_index: None,
                channels: None,
                time_base,
                start_pts: 0,
                duration_ts,
                sample_representation: None,
            },
            kind: StreamKind::Video,
            codec: "apch".into(),
            raster: Some((1920, 1080)),
            rate: Some(rate),
            sample_rate: None,
            bits: None,
            duration,
            motion_audio_origin: None,
            same_rate_aac_origin: false,
            column_timecode: Some(clock.clone()),
            embedded_mov_timecode: None,
        },
        audio,
        defaulted,
    ))
}

pub(super) fn select(
    facts: &av::SourceProject,
    options: &OriginalOptions,
    out: &mut OriginalIndex,
    claims: &mut HashMap<LocatorClaimKey, LocatorClaim>,
    pinned: &mut HashMap<PathBuf, String>,
    used: &mut usize,
    check: &mut dyn FnMut() -> Result<(), String>,
    index: Option<&crate::index::MediaIndex>,
) -> Result<(), SourceReadIssue> {
    // These structures use managed-master identities in destination preparation.
    let managed_masters: HashSet<_> = facts
        .subclips
        .iter()
        .flat_map(|subclip| subclip.tracks.iter().filter_map(|track| track.target_mob))
        .chain(
            facts
                .groups
                .iter()
                .flat_map(|group| &group.lanes)
                .flat_map(|lane| &lane.alternatives)
                .flat_map(|alternative| {
                    alternative.pieces.iter().filter_map(|piece| match piece {
                        av::SourceGroupPiece::Media { master, .. } => Some(*master),
                        _ => None,
                    })
                }),
        )
        .collect();
    if let Some(dir) = options
        .proxy_rewrap_dir
        .as_ref()
        .filter(|_| index.is_some())
    {
        if !dir.is_dir() {
            return Err(format!(
                "proxy output folder {} must be an existing directory",
                dir.display()
            )
            .into());
        }
    }
    for master in &facts.masters {
        check()?;
        if master.originals.iter().any(Option::is_some) {
            continue;
        }
        let candidate = (|| {
            let Some(saved) = saved_locator(master)? else {
                return Ok(None);
            };
            if managed_masters.contains(&master.master.mob_id) {
                return Err(
                    "camera-original replacement for reusable subclips and Groups is not qualified"
                        .into(),
                );
            }
            let (clock, frames) = saved_picture(master, index.is_some())?;
            let stereo = master.master.tracks.len() == 3;
            if stereo && options.proxy_rewrap_dir.is_none() {
                return Err("stereo proxy needs opt-in rewrap; kept Avid media links".into());
            }
            let Some((path, claim)) =
                resolve_with_claim(std::slice::from_ref(&saved), &options.root_mappings)?
            else {
                return Err(format!(
                    "{saved}: no exact file at the saved or mapped path"
                ));
            };
            let key = locator_claim_key(&path)?;
            if claims
                .get(&key)
                .is_some_and(|old| old.saved_paths.is_disjoint(&claim.saved_paths))
            {
                return Err("different saved original paths resolve to the same file".into());
            }
            if !path
                .extension()
                .and_then(|suffix| suffix.to_str())
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case("mov"))
            {
                return Err("the camera-original profile requires ProRes HQ MOV".into());
            }
            // Verify the source state around the probe and pin only files selected for output.
            let revision = crate::revision::file_revision_checked(&path, check)
                .map_err(|error| error.to_string())?;
            let reel = master
                .master
                .columns
                .iter()
                .find(|(key, value)| key == "Tape" && !value.is_empty())
                .map(|(_, value)| value.as_str());
            let (selected, audio, defaulted) =
                qualify(clock, frames, saved, path, &options.ffprobe, stereo, reel)?;
            let proxy = index
                .map(|index| {
                    crate::proxy::Binding::verify(
                        master,
                        &selected,
                        audio.first().and_then(|a| a.bits),
                        index,
                        options,
                        check,
                    )
                })
                .transpose()?;
            if pinned
                .get(&selected.path)
                .is_some_and(|old| old != &revision)
                || crate::revision::file_revision_checked(&selected.path, check)
                    .map_err(|error| error.to_string())?
                    != revision
            {
                return Err("camera original changed while it was being checked".into());
            }
            if locator_claim_key(&selected.path)? != key {
                return Err("camera original file identity changed while it was being checked".into());
            }
            Ok(Some((selected, audio, key, claim, revision, defaulted, proxy)))
        })();
        check()?;
        let note = match candidate {
            Ok(None) => continue,
            Err(why) => format!(
                "{:?}: {SOURCE_MEDIA_UNAVAILABLE} ({why}); using managed-media policy",
                master.master.name
            ),
            Ok(Some((selected, audio, key, claim, revision, defaulted, proxy))) => {
                let path = &selected.path;
                let cost = std::mem::size_of::<Selected>()
                    + master.id.len()
                    + path.as_os_str().len() * 4
                    + revision.len() * 2
                    + selected.stream.saved_locators[0].len() * 2
                    + 1536
                    + audio.len()
                        * (path.as_os_str().len() + selected.stream.saved_locators[0].len())
                    + proxy.as_ref().map_or(0, |(_, pins)| {
                        pins.iter()
                            .map(|p| p.path.as_os_str().len() * 4 + 512)
                            .sum()
                    });
                *used = used
                    .checked_add(cost)
                    .filter(|n| *n <= av::MAX_SOURCE_FACT_BYTES)
                    .ok_or("camera-original index exceeds 64 MiB")?;
                let saved_path_note = if claim.mapped {
                    format!("; saved Avid locator {}", selected.stream.saved_locators[0])
                } else {
                    String::new()
                };
                match claims.entry(key) {
                    Entry::Occupied(mut entry) => entry.get_mut().intersect(&claim),
                    Entry::Vacant(entry) => {
                        entry.insert(claim);
                    }
                }
                if let Entry::Vacant(entry) = pinned.entry(path.clone()) {
                    entry.insert(revision.clone());
                    out.original_inputs.push(av::SourceInput {
                        path: path.clone(),
                        revision,
                    });
                }
                let note = format!(
                    "{:?}: linked source media from Avid columns at {}{}{}",
                    master.master.name,
                    path.display(),
                    if defaulted {
                        " (some color tags unspecified)"
                    } else {
                        ""
                    },
                    saved_path_note
                );
                out.tracks.insert((master.id.clone(), 0), selected);
                for (i, a) in audio.into_iter().enumerate() {
                    out.tracks.insert((master.id.clone(), i + 1), a);
                }
                if let Some((binding, pins)) = proxy {
                    for pin in pins {
                        if let Some(old) = pinned.get(&pin.path) {
                            if old != &pin.revision {
                                return Err("proxy input changed".into());
                            }
                        } else {
                            pinned.insert(pin.path.clone(), pin.revision.clone());
                            out.original_inputs.push(pin);
                        }
                    }
                    let note = format!(
                        "{note}; Avid proxy {} {}",
                        if binding.audio.is_some() {
                            "will be rewrapped to"
                        } else {
                            "linked at"
                        },
                        binding.path.display()
                    );
                    out.proxies.insert(master.id.clone(), binding);
                    note
                } else {
                    note
                }
            }
        };
        *used = used
            .checked_add(note.len() + std::mem::size_of::<String>())
            .filter(|n| *n <= av::MAX_SOURCE_FACT_BYTES)
            .ok_or("camera-original notes exceed 64 MiB")?;
        out.notes.push(note);
    }
    Ok(())
}
