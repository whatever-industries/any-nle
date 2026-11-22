//! Reconcile each saved stream with the verified file for this timeline.
use super::{red, still, Ordinary, Originals, VerifiedFiles};
use crate::{
    linked::{self, Profile},
    source::ADAPTER,
    Result, StreamSelector,
};
use graph::{
    source::{OriginalStream, SourceGraph, SourceMedia},
    Rational, StreamKind, Value,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(in crate::premiere_export) fn bind_originals(
    graph: &mut SourceGraph,
    originals: &mut Originals<'_>,
    premiere_destination: bool,
) -> Result<VerifiedFiles> {
    let receipts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|r| r.adapter == ADAPTER)
        .map(|r| (r.object.as_str(), &r.data))
        .collect();
    let mut verified = VerifiedFiles::default();
    let cameras: HashSet<_> = graph
        .media
        .iter()
        .filter(|source| {
            source.kind == StreamKind::Video
                && source.codec.as_deref() == Some("avc1")
                && source.rate == Some(Rational::integer(24))
                && source.frame_size == Some((4096, 2160))
        })
        .flat_map(|source| source.locations.clone())
        .collect();
    for source in &mut graph.media {
        let [locator] = source.locations.as_slice() else {
            return Err(format!("{}: Resolve original needs one saved locator", source.id).into());
        };
        if premiere_destination && source.codec.as_deref() == Some("mp3") {
            let proof = retain(&mut verified.mp3, locator, || {
                originals.mp3(locator, &source.id)
            })?;
            let id = source.id.clone();
            proof
                .bind(source, receipts.get(id.as_str()).copied())
                .map_err(|error| format!("{id}: {error}"))?;
            continue;
        }
        if let Some(format) = source
            .original
            .as_ref()
            .filter(|_| premiere_destination)
            .and_then(|o| still::Format::from_codec(Some(&o.container)))
        {
            let proof = retain(&mut verified.stills, locator, || {
                originals.still(locator, &source.id, format)
            })?;
            if proof.format != format || source.frame_size != Some(proof.raster) {
                return Err(format!(
                    "{}: saved Resolve format or raster contradicts the linked still",
                    source.id
                )
                .into());
            }
            continue;
        }
        if premiere_destination && red::is_red(source) {
            let recording = retain(&mut verified.red, locator, || {
                originals.red(locator, &source.id)
            })?;
            let path = locator.clone();
            let id = source.id.clone();
            recording
                .bind(source)
                .map_err(|error| format!("{id} ({path}): {error}"))?;
            continue;
        }
        let binding = retain(&mut verified.ordinary, locator, || {
            originals.ordinary(
                locator,
                &source.id,
                premiere_destination,
                cameras.contains(locator),
            )
        })?;
        let receipt = receipts.get(source.id.as_str()).copied();
        bind_ordinary(source, binding, receipt)?;
    }
    Ok(verified)
}

fn bind_ordinary(
    source: &mut SourceMedia,
    binding: &Ordinary,
    receipt: Option<&Value>,
) -> Result<()> {
    let locator = &source.locations[0];
    let file_error =
        |error: Box<dyn std::error::Error>| format!("{} ({locator}): {error}", source.id);
    let (media, time_base, duration_ts) = binding;
    let camera = media.profile.timecoded_camera_pcm();
    let (container, channel, channels, representation) = match (&media.profile, source.kind) {
        (Profile::Mov { .. }, StreamKind::Video) if camera => ("mov", None, None, None),
        (Profile::Mov { .. }, StreamKind::Audio) if camera => {
            let channel = match saved_channel(receipt) {
                Some(channel @ (0 | 1)) if source.stream == Some(channel + 1) => channel,
                _ => {
                    return Err(
                        format!("{}: unresolved embedded PCM physical selector", source.id).into(),
                    )
                }
            };
            ("mov", Some(channel), Some(2), Some("pcm16be".into()))
        }
        (
            Profile::Mov {
                video: crate::linked::MovieVideo::ProResHq,
                aac: false,
                ..
            },
            StreamKind::Video,
        ) if source.codec.as_deref() == Some("apch") => ("mov", None, None, None),
        (Profile::Wav { bits, channels, .. }, StreamKind::Audio)
            if source.codec.as_deref() == Some("pcm") =>
        {
            let channel = saved_channel(receipt)
                .filter(|channel| (0..*channels).contains(channel))
                .ok_or_else(|| format!("{}: unresolved WAV physical selector", source.id))?;
            let representation = if *bits == 16 { "pcm16" } else { "pcm24" };
            (
                "wav",
                Some(channel),
                Some(*channels),
                Some(representation.into()),
            )
        }
        _ => {
            return Err(format!(
                "{} ({locator}): expected verified ProRes HQ MOV or PCM16/PCM24 WAV",
                source.id
            )
            .into())
        }
    };
    let (index, time_base, duration_ts) = if camera && source.kind == StreamKind::Audio {
        let time_base = Rational::new(1, 48_000).unwrap();
        (
            1,
            time_base,
            source
                .duration
                .to_units(48_000)
                .ok_or("invalid embedded PCM extent")?,
        )
    } else {
        (0, *time_base, *duration_ts)
    };
    let selector = StreamSelector {
        media_id: source.id.clone(),
        index,
        channel: channel.map(|c| c as usize),
    };
    linked::check_stream(source, &selector, media, time_base, duration_ts).map_err(file_error)?;
    if camera && source.kind == StreamKind::Video {
        let expected = match media.profile {
            Profile::Mov {
                quicktime_timecode: Some(start),
                ..
            } => u64::try_from(start)?,
            _ => return Err("missing verified original MOV clock".into()),
        };
        let clock = source
            .clock
            .as_mut()
            .filter(|clock| {
                clock.effective_count == expected
                    && !clock.drop
                    && clock.rate == Rational::integer(24)
                    && clock.exact_start() == source.start
            })
            .ok_or("saved Resolve camera timecode contradicts the verified MOV clock")?;
        clock.provenance.route = graph::source::VERIFIED_LINKED_MOV_TIMECODE.into();
    }
    let original = OriginalStream {
        saved_locators: source.locations.clone(),
        container: container.into(),
        stream_index: index as i64,
        channel_index: channel,
        channels,
        time_base,
        start_pts: 0,
        duration_ts,
        sample_representation: representation,
    };
    if source
        .original
        .as_ref()
        .is_some_and(|saved| saved != &original)
    {
        return Err(format!("{}: contradictory original stream facts", source.id).into());
    }
    source.original = Some(original);
    Ok(())
}

fn saved_channel(receipt: Option<&Value>) -> Option<i64> {
    let receipt = receipt?;
    match (
        receipt.get("saved_cache_track")?,
        receipt.get("saved_channel")?,
    ) {
        (Value::Int(0), Value::Int(channel)) => Some(*channel),
        _ => None,
    }
}

fn retain<'a, T>(
    files: &'a mut HashMap<String, Arc<T>>,
    locator: &str,
    inspect: impl FnOnce() -> Result<Arc<T>>,
) -> Result<&'a T> {
    if !files.contains_key(locator) {
        files.insert(locator.to_owned(), inspect()?);
    }
    Ok(files[locator].as_ref())
}
