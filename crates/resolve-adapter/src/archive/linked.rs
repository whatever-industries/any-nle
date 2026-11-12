//! Construct a project from prepared bin, media and timeline records.
use super::*;
use crate::dissolve::{self, Dissolve};
use crate::{
    linked::{mixer::DirectMixer, samples_per_frame, Media, PreparedProject, Profile, Timeline},
    turnover::Templates,
};
use graph::{Rational, Value};

const TEMPLATE_23976: &str = "4b2895fa6827801bce87bff7f657dae6454f7011459b689d1983bb11a017ccfc";
const TEMPLATE_24: &str = "e2a3a1ad16be932947914d866b3c8bb49b549ffd5869a06a9fc9eb576bf21bf5";
const TWO_BUS_TEMPLATE: &str = "51e9255ae6fed4f82b7a297531a7fab0bff8353153bfe39e7f797bb7296570cf";
const TWO_BUS_FOLDER: &str = "MediaPool/Master/000_Originals/MpFolder.xml";
const MEDIA_DONOR: &str = "MediaPool/Master/002_Linked/000_Audio/MpFolder.xml";
const TIMELINE_DONOR: &str = "SeqContainer/93a83077-933a-4ed8-9595-e12534b57e18.xml";
const PRORES_REC709: &str = "80087810027002789e01800166880132980100d80100";
const AVC_UNTAGGED: &str = "80089403100228108801329001d080a006980100d80100";
const AVC_REC709: &str = "80089403100228107002789e018001668801329001d080a006980100d80100";

pub fn publish_linked(
    prepared: &PreparedProject,
    layout_template: &Path,
    profile_template: &Path,
    output: &Path,
) -> Result<Published> {
    publish_turnover(
        prepared,
        layout_template,
        profile_template,
        None,
        None,
        output,
    )
}

pub fn publish_turnover(
    prepared: &PreparedProject,
    layout_template: &Path,
    profile_template: &Path,
    atom_template: Option<&Path>,
    group_template: Option<&Path>,
    output: &Path,
) -> Result<Published> {
    publish_linked_with_templates(
        prepared,
        &Templates {
            layout: layout_template.into(),
            profile: profile_template.into(),
            atom: atom_template.map(Path::to_owned),
            group: group_template.map(Path::to_owned),
            fairlight: None,
        },
        output,
    )
}

pub fn publish_linked_with_templates(
    prepared: &PreparedProject,
    templates: &Templates,
    output: &Path,
) -> Result<Published> {
    publish_turnover_checked(prepared, templates, output, || Ok(()))
}

pub(crate) fn publish_turnover_checked(
    prepared: &PreparedProject,
    templates: &Templates,
    output: &Path,
    recheck: impl FnOnce() -> Result<()>,
) -> Result<Published> {
    check_output(output)?;
    let (members, identities) = assemble_turnover(prepared, templates)?;
    write_checked_archive(members, output, |stage| {
        verify_stage(prepared, &identities, stage)?;
        for media in &prepared.media {
            media.recheck()?;
        }
        recheck()
    })
}

fn assemble_turnover(
    prepared: &PreparedProject,
    templates: &Templates,
) -> Result<(Members, Identities)> {
    let template = |role: &str, path: &Path, hash: &str, count| -> Result<Members> {
        load_skeleton(path, hash, count)
            .map_err(|error| format!("Resolve {role} template {}: {error}", path.display()).into())
    };
    let layout = template("layout", &templates.layout, TEMPLATE_23976, 13)?;
    let profile = if prepared.rate == Rational::integer(24) {
        template("profile", &templates.profile, TEMPLATE_24, 10)?
    } else {
        template("profile", &templates.profile, TEMPLATE_23976, 13)?
    };
    let atom_donor = if prepared
        .media
        .iter()
        .any(|m| matches!(m.profile, Profile::Atom(_)))
    {
        let path = templates
            .atom
            .as_deref()
            .ok_or("split MXF project requires its pinned media template")?;
        let members = template("split MXF", path, SKELETON_SHA256, 6)?;
        Some(block(member(&members, SOURCE)?, "Sm2MpVideoClip")?)
    } else {
        None
    };
    let fairlight = if prepared
        .timelines
        .iter()
        .any(|row| row.mixer.is_some_and(|m| m.buses == 2))
    {
        let path = templates
            .fairlight
            .as_deref()
            .ok_or("missing pinned two-bus Fairlight template")?;
        let donor = template("two-bus Fairlight", path, TWO_BUS_TEMPLATE, 7)?;
        let sequence = block(member(&donor, TWO_BUS_FOLDER)?, "Sm2Sequence")?;
        Some(
            Mixer::decode(&sequence_fields(&sequence)?)?
                .ok_or("missing persisted Fairlight model in the pinned template")?,
        )
    } else {
        None
    };
    build(
        prepared,
        &layout,
        &profile,
        atom_donor.as_deref(),
        templates.group.as_deref(),
        fairlight.as_ref(),
    )
}

pub(crate) fn review_turnover(
    prepared: &PreparedProject,
    templates: &crate::turnover::Templates,
    output: &Path,
) -> Result<()> {
    check_output(output)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !std::fs::metadata(parent)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Err("project output must have an existing parent directory".into());
    }
    assemble_turnover(prepared, templates)?;
    Ok(())
}

fn member<'a>(members: &'a Members, path: &str) -> Result<&'a str> {
    members
        .get(path)
        .map(String::as_str)
        .ok_or_else(|| format!("missing template member {path}").into())
}

fn blocks(name: &str) -> Result<Regex> {
    Ok(Regex::new(&format!(
        r"(?s)<{name}(?: [^>]+)?>.*?</{name}>"
    ))?)
}

fn named(source: &str, class: &str, name: &str) -> Result<String> {
    let re = blocks(class)?;
    let label = format!("<Name>{}</Name>", xml(name));
    let mut hits = re
        .find_iter(source)
        .filter(|item| item.as_str().contains(&label));
    let found = hits
        .next()
        .ok_or_else(|| format!("missing pinned {class} {name}"))?;
    if hits.next().is_some() {
        return Err("ambiguous pinned template object name".into());
    }
    Ok(found.as_str().to_owned())
}

fn container_field(s: &mut String, name: &str, contents: &str) -> Result<()> {
    let re = Regex::new(&format!(r"(?s)<{name}(?:/>|>.*?</{name}>)"))?;
    let range = one(&re, s)?.range();
    s.replace_range(range, &format!("<{name}>{contents}</{name}>"));
    Ok(())
}

fn new_ids(xml: &str) -> Result<BTreeMap<String, String>> {
    let re = Regex::new(
        r#"DbId="([a-f0-9-]{36})"|<Unique(?:SequenceId|MediaPoolItemId)>([a-f0-9-]{36})</"#,
    )?;
    let mut ids = BTreeMap::new();
    for m in re.captures_iter(xml) {
        let id = m
            .get(1)
            .or_else(|| m.get(2))
            .ok_or("missing template object capture")?;
        Uuid::parse_str(id.as_str())?;
        if ids.insert(id.as_str().to_owned(), uid()).is_some() {
            return Err("duplicate template object identity".into());
        }
    }
    Ok(ids)
}

fn remap(mut s: String, ids: &BTreeMap<String, String>) -> String {
    for (old, new) in ids {
        s = s.replace(old, new);
    }
    s
}

fn remap_blob(mut data: Vec<u8>, ids: &BTreeMap<String, String>) -> Vec<u8> {
    for (old, new) in ids {
        let utf16 = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>()
        };
        for (from, to) in [
            (old.as_bytes().to_vec(), new.as_bytes().to_vec()),
            (utf16(old), utf16(new)),
        ] {
            let mut offset = 0;
            while let Some(index) = data[offset..].windows(from.len()).position(|x| x == from) {
                offset += index;
                data[offset..offset + to.len()].copy_from_slice(&to);
                offset += to.len();
            }
        }
    }
    data
}

fn rate_bytes(rate: Rational) -> [u8; 16] {
    let mut encoded = [0; 16];
    encoded[..8].copy_from_slice(&fps(rate).to_le_bytes());
    encoded
}

fn fps(rate: Rational) -> f64 {
    rate.num() as f64 / rate.den() as f64
}

fn prefix_name(item: &mut String, name: &str) -> Result<()> {
    let end = item.find("<PTZRPreset>").unwrap_or(item.len());
    let mut head = item[..end].to_owned();
    set(&mut head, "Name", &xml(name))?;
    item.replace_range(..end, &head);
    Ok(())
}

fn clip_cache(media: &Media, codec: &[u8], video: bool) -> Result<String> {
    let folder = utf8_path(media.path.parent().ok_or("missing linked media parent")?)?;
    let file = media
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid linked basename")?;
    let mut data = Vec::new();
    pb_bytes(1, folder.as_bytes(), &mut data);
    pb_bytes(2, file.as_bytes(), &mut data);
    pb_bytes(5, codec, &mut data);
    if video {
        pb_bytes(6, file.as_bytes(), &mut data);
        pb_bytes(7, uid().as_bytes(), &mut data);
    }
    pb_int(13, media.modified_micros, &mut data);
    if matches!(
        media.profile,
        Profile::Mov {
            quicktime_timecode: false,
            h264: false,
            ..
        }
    ) {
        pb_int(14, 24584, &mut data);
    }
    pb_int(15, 8, &mut data);
    pb_int(16, 100, &mut data);
    pb_int(
        18,
        if video || codec == b"AAC" {
            16384
        } else {
            32768
        },
        &mut data,
    );
    pack(&data)
}

fn embedded_aac(item: &mut String, source: &str, media: &Media, frames: i64) -> Result<()> {
    let donor = named(source, "Sm2MpAudioClip", "Signals-24.wav")?;
    let base = block(&donor, "BtAudioInfo")?;
    let id = uid();
    let samples = frames * 2000;
    let track = qt(&[
        ("UniqueId", Text(&uid())),
        ("StartTime", Double(0.0)),
        ("SampleRate", Uint(48000)),
        ("NumChannels", Int(2)),
        ("IdxTrack", Int(0)),
        ("Duration", Long(samples)),
        ("DbType", Text("BtAudioTrack")),
        ("CodecName", Text("AAC")),
        ("ChannelLayout", Int(2)),
        ("BitDepth", Uint(3)),
    ]);
    let audio = bind_audio(
        &base,
        &id,
        &clip_cache(media, b"AAC", false)?,
        &qt(&[("0", Bytes(&track))]),
    )?;
    container_field(
        item,
        "EmbeddedAudioVec",
        &format!("<Element>{audio}</Element>"),
    )?;
    let mono_left = mono_selector(0, false)?;
    let mono_right = mono_selector(1, false)?;
    set(
        item,
        "VirtualAudioTracksBA",
        &hex::encode(qt(&[("1", Bytes(&mono_right)), ("0", Bytes(&mono_left))])),
    )?;
    let left = qt(&[("MediaRef", Text(&id))]);
    let right = qt(&[("MediaRef", Text(&id)), ("ChannelIdx", Int(1))]);
    let channels = qt(&[("1", Bytes(&right)), ("0", Bytes(&left))]);
    let range = extent(0.0, samples as f64 / 48000.0);
    let route = qt(&[
        ("Type", Uint(0)),
        ("MediaExtents", Bytes(&range)),
        ("CodecName", Text("AAC")),
        ("ChannelVecBA", Bytes(&channels)),
    ]);
    let routes = qt(&[("0", Bytes(&route))]);
    replace_fields(item, |data| {
        replace_bytes(&data, 1, |a| {
            replace_bytes(a, 1, |b| {
                crate::binary::rewrite_bytes(b, 5, |_| Ok(Some(routes)))
            })
        })
    })
}

fn mono_selector(channel: usize, atom: bool) -> Result<Vec<u8>> {
    let mut channels = [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 1];
    let index = u8::try_from(channel + 1)?;
    if atom {
        channels[10] = 0x40 * index;
    } else {
        channels[11] = index;
    }
    Ok(qt(&[
        ("ChannelsBA", Bytes(&channels)),
        ("AudioType", Int(1)),
    ]))
}

fn replace_fields(
    item: &mut String,
    update: impl FnOnce(Vec<u8>) -> Result<Vec<u8>>,
) -> Result<()> {
    let re = Regex::new(r"<FieldsBlob>([a-f0-9]+)</FieldsBlob>")?;
    let payload = re
        .captures(item)
        .and_then(|m| m.get(1))
        .ok_or("missing template FieldsBlob")?;
    let updated = pack(&update(unpack(payload.as_str())?)?)?;
    item.replace_range(payload.range(), &updated);
    Ok(())
}

fn pool_item(
    source: &str,
    media: &Media,
    bin: &str,
    project_rate: Rational,
    atom_donor: Option<&str>,
) -> Result<(String, String)> {
    let donor = match &media.profile {
        Profile::Mov { .. } => named(source, "Sm2MpVideoClip", "Counter-23976.mov")?,
        Profile::Wav { bits, channels, .. } => {
            let name = format!(
                "{}-{bits}.wav",
                if *channels == 1 { "Mono" } else { "Signals" }
            );
            named(source, "Sm2MpAudioClip", &name)?
        }
        Profile::Atom(binding) => {
            let donor = atom_donor.ok_or("missing split MXF template")?;
            let id = uid();
            let (folder, file) = media_path_parts(&binding.path)?;
            let mut item = bind_pool_item(donor, binding, folder, file, &id)?;
            set(&mut item, "MpFolder", bin)?;
            prefix_name(&mut item, &media.name)?;
            return Ok((id, item));
        }
    };
    let ids = new_ids(&donor)?;
    let mut item = remap(donor, &ids);
    let id = dbid(&item)?.to_owned();
    set(&mut item, "MpFolder", bin)?;
    prefix_name(&mut item, &media.name)?;
    match &media.profile {
        Profile::Mov {
            rate,
            frames,
            tagged_rec709,
            h264,
            aac,
            ..
        } => {
            let old = block(&item, "BtVideoInfo")?;
            let mut video = old.clone();
            set(
                &mut video,
                "Clip",
                &clip_cache(media, if *h264 { b"avc1" } else { b"apch" }, true)?,
            )?;
            let timing = qt(&[
                ("UniqueId", Text(&uid())),
                ("StartFrame", Int(0)),
                ("NumFrames", Int(i32::try_from(*frames)?)),
                ("FrameRate", Bytes(&rate_bytes(*rate))),
                ("DbType", Text("BtVideoTime")),
            ]);
            set(&mut video, "Time", &hex::encode(timing))?;
            if *h264 {
                let mut resolution = [0; 16];
                resolution[..8].copy_from_slice(&1920u64.to_be_bytes());
                resolution[8..].copy_from_slice(&1080u64.to_be_bytes());
                let geometry = qt(&[
                    ("UniqueId", Text(&uid())),
                    ("Resolution", Bytes(&resolution)),
                    ("FrameSize", Int(3126784)),
                    ("DbType", Text("BtGeometry")),
                ]);
                set(&mut video, "Geometry", &hex::encode(geometry))?;
                set(
                    &mut video,
                    "Radiometry",
                    if *tagged_rec709 {
                        AVC_REC709
                    } else {
                        AVC_UNTAGGED
                    },
                )?;
            } else if *tagged_rec709 {
                set(&mut video, "Radiometry", PRORES_REC709)?;
            }
            let metadata = qt(&[
                ("UniqueId", Text(&uid())),
                ("DbType", Text("BtVideoMetadata")),
            ]);
            set(&mut video, "VideoMetadata", &hex::encode(metadata))?;
            replace_once(&mut item, &old, &video)?;
            if *aac {
                embedded_aac(&mut item, source, media, *frames)?;
            }
        }
        Profile::Wav {
            samples,
            bits,
            channels,
        } => {
            let old = block(&item, "BtAudioInfo")?;
            let mut audio = old.clone();
            let audio_id = dbid(&audio)?.to_owned();
            let depth = if *bits == 16 { 1 } else { 2 };
            set(
                &mut audio,
                "Clip",
                &clip_cache(media, b"Linear PCM", false)?,
            )?;
            let mut track = vec![
                ("UniqueId", Text(&audio_id)),
                ("StartTime", Double(0.0)),
                ("SampleRate", Uint(48000)),
                ("NumChannels", Int(*channels as i32)),
                ("IdxTrack", Int(0)),
                ("Duration", Long(*samples)),
                ("DbType", Text("BtAudioTrack")),
                ("CodecName", Text("Linear PCM")),
            ];
            if depth == 2 {
                track.push(("ChannelLayout", Int(*channels as i32)));
            }
            track.push(("BitDepth", Uint(depth)));
            set(
                &mut audio,
                "TracksBA",
                &hex::encode(qt(&[("0", Bytes(&qt(&track)))])),
            )?;
            replace_once(&mut item, &old, &audio)?;
            let left = qt(&[("MediaRef", Text(&audio_id))]);
            let right = qt(&[("MediaRef", Text(&audio_id)), ("ChannelIdx", Int(1))]);
            let channels_map = if *channels == 2 {
                qt(&[("1", Bytes(&right)), ("0", Bytes(&left))])
            } else {
                qt(&[("0", Bytes(&left))])
            };
            let bounds = extent(0.0, *samples as f64 / 48000.0);
            let mut route = Vec::new();
            if *channels == 2 {
                route.push(("Type", Uint(0)));
            }
            route.extend([
                ("MediaExtents", Bytes(&bounds)),
                ("CodecName", Text("Linear PCM")),
                ("ChannelVecBA", Bytes(&channels_map)),
                ("BitDepth", Uint(depth)),
            ]);
            let routes = qt(&[("0", Bytes(&qt(&route)))]);
            replace_fields(&mut item, |data| {
                let with_routes =
                    replace_bytes(&data, 1, |outer| replace_bytes(outer, 5, |_| Ok(routes)))?;
                replace_bytes(&with_routes, 2, |_| {
                    let mut clock = vec![9];
                    clock.extend_from_slice(&fps(project_rate).to_le_bytes());
                    Ok(clock)
                })
            })?;
        }
        Profile::Atom(_) => return Err("unexpected split MXF template profile".into()),
    }
    Ok((id, item))
}

fn sequence_fields(sequence: &str) -> Result<String> {
    let end = sequence
        .find("<UniqueSequenceId>")
        .ok_or("missing sequence identity")?;
    tag(&sequence[..end], "FieldsBlob")
}

struct TimelineDonors<'a> {
    pool: &'a str,
    container: &'a str,
    fairlight: Option<&'a Mixer>,
}

fn write_timeline(
    timeline: &Timeline,
    rate: Rational,
    bin: &str,
    media: &[Media],
    media_ids: &[String],
    donors: &TimelineDonors<'_>,
) -> Result<(String, String, String, String, String)> {
    let mut ids = new_ids(donors.pool)?;
    for (old, new) in new_ids(donors.container)? {
        if ids.insert(old, new).is_some() {
            return Err("colliding timeline template identities".into());
        }
    }
    let mut pool = remap(donors.pool.to_owned(), &ids);
    let mut container = remap(donors.container.to_owned(), &ids);
    set(&mut pool, "MpFolder", bin)?;
    set(&mut pool, "CurPlayheadPosition", "0")?;
    prefix_name(&mut pool, &timeline.name)?;
    replace_fields(&mut pool, |data| Ok(remap_blob(data, &ids)))?;
    let old = block(&pool, "Sm2Timeline")?;
    let mut handle = old.clone();
    let head_end = handle
        .find("<Sequence>")
        .ok_or("missing timeline sequence wrapper")?;
    let mut head = handle[..head_end].to_owned();
    set(&mut head, "Name", &xml(&timeline.name))?;
    handle.replace_range(..head_end, &head);
    let handle_id = dbid(&handle)?.to_owned();
    let old_sequence = block(&handle, "Sm2Sequence")?;
    let mut sequence = old_sequence.clone();
    let sequence_id = dbid(&sequence)?.to_owned();
    let rate_hex = hex::encode(rate_bytes(rate));
    set(&mut sequence, "FrameRate", &rate_hex)?;
    set(
        &mut sequence,
        "MediaExtents",
        &hex::encode(extent(
            timeline.origin as f64 / fps(rate),
            timeline.length as f64 / fps(rate),
        )),
    )?;
    let fields_end = sequence
        .find("<UniqueSequenceId>")
        .ok_or("missing sequence identity")?;
    let mut fields_head = sequence[..fields_end].to_owned();
    let fields = sequence_fields(&sequence)?;
    let fields = if let Some(mixer) = timeline.mixer {
        Mixer::decode(&fields)?
            .ok_or("missing qualified Fairlight mixer in timeline template")?
            .write_direct(donors.fairlight, mixer)?
    } else {
        fields
    };
    set(
        &mut fields_head,
        "FieldsBlob",
        &hex::encode(remap_blob(hex::decode(fields)?, &ids)),
    )?;
    sequence.replace_range(..fields_end, &fields_head);
    replace_once(&mut handle, &old_sequence, &sequence)?;
    replace_once(&mut pool, &old, &handle)?;
    let item_lists = blocks("Items")?;
    let spans: Vec<_> = item_lists
        .find_iter(&container)
        .map(|m| m.range())
        .collect();
    if spans.len() != 3 {
        return Err("expected V1 and two native mono track templates".into());
    }
    for (lane, span) in spans.into_iter().enumerate().rev() {
        let base = blocks(if lane == 0 {
            "Sm2TiVideoClip"
        } else {
            "Sm2TiAudioClip"
        })?
        .find(&container[span.clone()])
        .ok_or("missing native edit template")?
        .as_str()
        .to_owned();
        let mut list = String::from("<Items>");
        for edit in &timeline.lanes[lane] {
            if let Some(dissolve) = timeline
                .dissolve
                .as_ref()
                .filter(|dissolve| lane == 0 && edit.start == dissolve.cut)
            {
                write_dissolve(&mut list, timeline.origin, dissolve);
            }
            let source = &media[edit.media];
            let mut clip = base.clone();
            let old_id = dbid(&clip)?.to_owned();
            replace_once(&mut clip, &old_id, &uid())?;
            set(&mut clip, "Name", &xml(&source.name))?;
            set(&mut clip, "MediaRef", &media_ids[edit.media])?;
            set(&mut clip, "MediaFilePath", &xml(utf8_path(&source.path)?))?;
            set(&mut clip, "MediaFrameRate", &rate_hex)?;
            set(
                &mut clip,
                "Start",
                &(timeline.origin + edit.start).to_string(),
            )?;
            set(&mut clip, "In", &edit.source_in.to_string())?;
            set(&mut clip, "Duration", &edit.length.to_string())?;
            if let Some(video) = &edit.video {
                set(&mut clip, "EffectFiltersBA", &video.encoded)?;
            }
            if let Some(db) = edit.volume {
                let envelope = crate::audio::Envelope::StaticVolume(db);
                set(&mut clip, "EffectFiltersBA", &envelope.encode()?)?;
            }
            let atom = match &source.profile {
                Profile::Atom(binding) => Some(binding),
                _ => None,
            };
            let end = match &source.profile {
                Profile::Mov { frames, .. } => (frames - 1) as f64 / fps(rate),
                Profile::Wav { samples, .. } => {
                    (samples - samples_per_frame(rate)) as f64 / 48000.0
                }
                Profile::Atom(binding) => {
                    set(
                        &mut clip,
                        "MediaStartTime",
                        &seconds(binding.timecode_frames).to_string(),
                    )?;
                    seconds(binding.frames - 1)
                }
            };
            let mut map = vec![2];
            map.extend_from_slice(&end.to_be_bytes());
            set(&mut clip, "MediaTimemapBA", &hex::encode(map))?;
            if let Some(channel) = edit.channel {
                let selector = if atom.is_some() { channel } else { 0 };
                set(
                    &mut clip,
                    "VirtualAudioTrackBA",
                    &hex::encode(mono_selector(channel, atom.is_some())?),
                )?;
                set(&mut clip, "MediaTrackIdx", &selector.to_string())?;
            }
            if list.len().saturating_add(clip.len()).saturating_add(32) as u64 > MAX_XML {
                return Err("timeline XML exceeds its construction budget".into());
            }
            list.push_str("<Element>");
            list.push_str(&clip);
            list.push_str("</Element>");
        }
        list.push_str("</Items>");
        container.replace_range(span, &list);
    }
    Ok((
        handle_id,
        sequence_id,
        dbid(&container)?.to_owned(),
        pool,
        container,
    ))
}

fn write_dissolve(list: &mut String, origin: i64, transition: &Dissolve) {
    use std::fmt::Write as _;
    // The saved record is between the two clips. Its range ends at the cut;
    // the incoming clip's saved Start, In and Duration remain its visible edit.
    let start = origin + transition.cut - dissolve::DURATION;
    write!(
        list,
        concat!(
            "<Element><Sm2TiTransition DbId=\"{id}\">",
            "<FieldsBlob>{fields}</FieldsBlob><PrettyType>{kind}</PrettyType>",
            "<Name>Transition</Name><Start>{start}</Start><Duration>{duration}</Duration>",
            "<LinkedItemSync/><WasDisbanded>false</WasDisbanded><MarkersBA/>",
            "<UiMemento>0</UiMemento><Flags>0</Flags><PriorityIndex>0</PriorityIndex>",
            "<EffectFiltersBA>{effect}</EffectFiltersBA><ImportExportMetadataBA/>",
            "<RenderTextEnabled>true</RenderTextEnabled><RenderTextGanged>true</RenderTextGanged>",
            "<RenderTextPrefixed>true</RenderTextPrefixed>",
            "<AlignmentType>{alignment}</AlignmentType><Position>{position}</Position>",
            "</Sm2TiTransition></Element>"
        ),
        id = uid(),
        fields = dissolve::FIELDS,
        kind = dissolve::TYPE,
        start = start,
        duration = dissolve::DURATION,
        effect = dissolve::EFFECT,
        alignment = dissolve::ALIGNMENT,
        position = dissolve::POSITION,
    )
    .expect("writing to a String cannot fail");
}

fn folder_path(index: usize, project: &PreparedProject) -> Result<String> {
    let mut path = Vec::new();
    let mut current = index;
    while let Some(parent) = project.bins[current].parent {
        if path.len() == 16 {
            return Err("Resolve bin nesting exceeds project profile".into());
        }
        let sibling = project.bins[..current]
            .iter()
            .filter(|bin| bin.parent == Some(parent))
            .count();
        let name: String = project.bins[current]
            .name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '_' | '-') {
                    ch
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        path.push(format!("{sibling:03}_{name}"));
        current = parent;
    }
    let mut result = String::from("MediaPool/Master");
    for part in path.iter().rev() {
        result.push('/');
        result.push_str(part);
    }
    Ok(result + "/MpFolder.xml")
}

struct Identities {
    bins: Vec<String>,
    media: Vec<String>,
    sequences: Vec<String>,
    group: Option<String>,
}

fn build(
    p: &PreparedProject,
    layout: &Members,
    profile: &Members,
    atom_donor: Option<&str>,
    group_template: Option<&Path>,
    fairlight: Option<&Mixer>,
) -> Result<(Members, Identities)> {
    let root = member(profile, POOL)?;
    let root_id = dbid(root)?.to_owned();
    let media_pool_id = tag(root, "MediaPool")?;
    let mut bin_ids = vec![root_id];
    bin_ids.extend((1..p.bins.len()).map(|_| uid()));
    let mut children = vec![String::new(); p.bins.len()];
    let mut media_ids = Vec::new();
    let mut remaining_xml = MAX_XML as usize;
    let donor = member(layout, MEDIA_DONOR)?;
    let timeline_donor = named(member(layout, POOL)?, "Sm2MpTimelineClip", "Linked A")?;
    let donors = TimelineDonors {
        pool: &timeline_donor,
        container: member(layout, TIMELINE_DONOR)?,
        fairlight,
    };
    for media in &p.media {
        let (id, item) = pool_item(donor, media, &bin_ids[media.bin], p.rate, atom_donor)?;
        remaining_xml = remaining_xml
            .checked_sub(item.len() + 19)
            .ok_or("media XML exceeds its construction budget")?;
        media_ids.push(id);
        children[media.bin].push_str(&format!("<Element>{item}</Element>"));
    }
    let mut members = Members::new();
    let mut handles = String::new();
    let mut sequence_ids = Vec::new();
    let mut group_id = None;
    let mut project = member(profile, PROJECT)?.to_owned();
    set(&mut project, "ProjectName", &xml(&p.name))?;
    set(
        &mut project,
        "CurrentTimelineIndex",
        &p.active_timeline_index.to_string(),
    )?;
    container_field(&mut project, "LocableBlobSet", "")?;
    for timeline in &p.timelines {
        if let Some(group) = &timeline.group {
            let built = super::group::build_turnover_group(
                &group.prepared,
                group_template.ok_or("Group project requires its pinned Group template")?,
                &bin_ids[group.bin],
                &bin_ids[timeline.bin],
                group.cameras.map(|index| media_ids[index].as_str()),
            )?;
            let added = built.pool.len()
                + built.timeline.len()
                + 38
                + built.containers.values().map(String::len).sum::<usize>();
            remaining_xml = remaining_xml
                .checked_sub(added)
                .ok_or("Group XML exceeds its construction budget")?;
            children[group.bin].push_str(&format!("<Element>{}</Element>", built.pool));
            children[timeline.bin].push_str(&format!("<Element>{}</Element>", built.timeline));
            handles.push_str(&format!("<Element>{}</Element>", built.handle));
            sequence_ids.push(built.sequence);
            group_id = Some(built.id);
            for (name, contents) in built.containers {
                if members.insert(name, contents).is_some() {
                    return Err("Group sequence container identity collision".into());
                }
            }
            continue;
        }
        let (handle, sequence, container_id, pool, container) = write_timeline(
            timeline,
            p.rate,
            &bin_ids[timeline.bin],
            &p.media,
            &media_ids,
            &donors,
        )?;
        remaining_xml = remaining_xml
            .checked_sub(pool.len() + container.len() + 19)
            .ok_or("project XML exceeds its construction budget")?;
        children[timeline.bin].push_str(&format!("<Element>{pool}</Element>"));
        handles.push_str(&format!("<Element>{handle}</Element>"));
        markers::write(&mut project, &sequence, &timeline.markers)?;
        sequence_ids.push(sequence);
        if members
            .insert(format!("SeqContainer/{container_id}.xml"), container)
            .is_some()
        {
            return Err("sequence container identity collision".into());
        }
    }
    container_field(&mut project, "TimelineHandleVec", &handles)?;
    members.insert(PROJECT.into(), project);
    members.insert("Gallery.xml".into(), member(profile, "Gallery.xml")?.into());
    for (index, bin) in p.bins.iter().enumerate() {
        let mut folder = if let Some(parent) = bin.parent {
            let mut folder = donor.to_owned();
            let old_id = dbid(&folder)?.to_owned();
            container_field(&mut folder, "MediaVec", "")?;
            replace_once(&mut folder, &old_id, &bin_ids[index])?;
            set(&mut folder, "MpFolder", &bin_ids[parent])?;
            set(&mut folder, "MediaPool", &media_pool_id)?;
            set(&mut folder, "UniqueMediaPoolItemId", &uid())?;
            folder
        } else {
            root.to_owned()
        };
        container_field(&mut folder, "MediaVec", &children[index])?;
        let end = folder
            .find("<MediaVec>")
            .ok_or("missing folder member list")?;
        let mut head = folder[..end].to_owned();
        set(&mut head, "Name", &xml(&bin.name))?;
        folder.replace_range(..end, &head);
        let children: Vec<_> = p
            .bins
            .iter()
            .enumerate()
            .filter(|(_, row)| row.parent == Some(index))
            .map(|(child, _)| bin_ids[child].as_str())
            .enumerate()
            .map(|(ordinal, id)| (ordinal.to_string(), id))
            .collect();
        let route: Vec<_> = children
            .iter()
            .rev()
            .map(|(key, id)| (key.as_str(), Text(id)))
            .collect();
        replace_fields(&mut folder, |data| {
            crate::binary::rewrite_bytes(&data, 2, |_| Ok((!route.is_empty()).then(|| qt(&route))))
        })?;
        members.insert(folder_path(index, p)?, folder);
    }
    if members.values().map(|s| s.len() as u64).sum::<u64>() > MAX_XML {
        return Err("generated project XML exceeds its budget".into());
    }
    Ok((
        members,
        Identities {
            bins: bin_ids,
            media: media_ids,
            sequences: sequence_ids,
            group: group_id,
        },
    ))
}

fn verify_stage(p: &PreparedProject, ids: &Identities, path: &Path) -> Result<()> {
    let graph = crate::source::read(path)?;
    if graph.sequences.len() != p.timelines.len()
        || graph.items.len() != p.media.len()
        || graph.bins.len() != p.bins.len()
    {
        return Err("generated Resolve reference counts differ from preparation".into());
    }
    let active = graph
        .sidecar(crate::source::ACTIVE_TIMELINE_ADAPTER, &ids.bins[0])
        .ok_or("generated Resolve active timeline receipt missing")?;
    let expected_index = Value::Int(i64::try_from(p.active_timeline_index)?);
    let expected_id = &ids.sequences[p.active_timeline_index];
    if active.get("saved_index") != Some(&expected_index)
        || !matches!(active.get("sequence_id"), Some(Value::Str(id)) if id == expected_id)
        || active.get("unavailable_reason") != Some(&Value::Null)
    {
        return Err("generated Resolve active timeline differs from preparation".into());
    }
    let mut dissolves = Vec::with_capacity(p.timelines.len());
    for (timeline, read) in p.timelines.iter().zip(&graph.sequences) {
        let saved = dissolve::prepare(&graph, read)?;
        let cut = |dissolve: &Dissolve| dissolve.cut;
        if saved.as_ref().map(cut) != timeline.dissolve.as_ref().map(cut) {
            return Err("generated Resolve dissolve differs from preparation".into());
        }
        dissolves.push(saved);
    }
    if graph.losses.iter().any(|loss| {
        !p.timelines
            .iter()
            .zip(&graph.sequences)
            .zip(&dissolves)
            .any(|((timeline, read), dissolve)| {
                timeline.mixer.is_some() && DirectMixer::recover(loss, &read.id)
                    || dissolve.as_ref().is_some_and(|d| d.recover(loss, &read.id))
            })
    })
    {
        return Err(format!(
            "generated Resolve reference graph differs from prepared project: {:?}",
            graph.losses
        )
        .into());
    }
    for (index, bin) in p.bins.iter().enumerate() {
        let actual = graph
            .bins
            .iter()
            .find(|row| row.id == ids.bins[index])
            .ok_or("generated bin missing")?;
        let expected_items: Vec<_> = p
            .media
            .iter()
            .zip(&ids.media)
            .filter(|(media, _)| media.bin == index)
            .map(|(_, id)| id)
            .collect();
        if actual.name != bin.name
            || actual.parent.as_ref() != bin.parent.map(|parent| &ids.bins[parent])
            || !actual.items.iter().eq(expected_items)
        {
            return Err("generated Resolve bin ownership differs from preparation".into());
        }
        let expected_children = p
            .bins
            .iter()
            .zip(&ids.bins)
            .filter(|(child, _)| child.parent == Some(index))
            .map(|(_, id)| id);
        let actual_children = graph
            .bins
            .iter()
            .filter(|child| child.parent.as_ref() == Some(&actual.id))
            .map(|child| &child.id);
        let has_children = expected_children.clone().next().is_some();
        let order = graph.sidecar(crate::source::BIN_ORDER_ADAPTER, &actual.id);
        let saved_order = order.is_some_and(|fields| {
            matches!(fields.get("source"), Some(Value::Str(source)) if source == "fields_blob")
                && fields.get("archive_order_matches") == Some(&Value::Bool(true))
        });
        if !actual_children.eq(expected_children) || (has_children && !saved_order) {
            return Err("generated Resolve child bin order differs from preparation".into());
        }
    }
    for (media, id) in p.media.iter().zip(&ids.media) {
        if let Profile::Mov {
            h264: true,
            rate,
            frames,
            aac,
            ..
        } = &media.profile
        {
            let item = graph
                .items
                .iter()
                .find(|item| item.id == *id)
                .ok_or("generated AVC media item absent")?;
            let duration = Rational::new(
                i128::from(*frames) * i128::from(rate.den()),
                rate.num().into(),
            )
            .ok_or("generated AVC duration overflow")?;
            let file = utf8_path(&media.path)?;
            let expected = if *aac {
                [Some("avc1"), Some("aac"), Some("aac")]
            } else {
                [Some("avc1"), None, None]
            };
            let mut streams = item.media.iter();
            for codec in expected.into_iter().flatten() {
                let saved = streams
                    .next()
                    .and_then(|link| graph.media.iter().find(|row| row.id == link.media))
                    .ok_or("generated AVC media stream absent")?;
                if saved.locations.as_slice() != [file]
                    || saved.duration != duration
                    || saved.codec.as_deref() != Some(codec)
                {
                    return Err(
                        "generated AVC source clock or locator differs from preparation".into(),
                    );
                }
            }
            if streams.next().is_some() {
                return Err("generated AVC media has an unexpected stream".into());
            }
        }
        let Profile::Atom(binding) = &media.profile else {
            continue;
        };
        let files = binding
            .atom
            .as_ref()
            .ok_or("split MXF verification absent")?;
        let item = graph
            .items
            .iter()
            .find(|item| item.id == *id)
            .ok_or("generated split MXF item missing")?;
        if item.umid.as_deref() != Some(&hex::encode_upper(files.master_package))
            || item.media.len() != files.files.len()
        {
            return Err("generated split MXF master identity differs from preparation".into());
        }
        let start = Rational::new(i128::from(binding.timecode_frames) * 1001, 24000);
        for (index, file) in files.files.iter().enumerate() {
            let stream_id = if index == 0 {
                format!("{id}/video")
            } else {
                format!("{id}/audio/{index}")
            };
            let stream = graph.media.iter().find(|row| row.id == stream_id);
            let receipt = graph.sidecar("resolve.source", &stream_id);
            let file_path = utf8_path(&file.path)?;
            if !item.media.iter().any(|link| link.media == stream_id)
                || !stream.is_some_and(|row| {
                    row.locations.as_slice() == [file_path] && row.start == start
                })
                || receipt.and_then(|fields| fields.get("saved_file_package"))
                    != Some(&Value::Str(hex::encode_upper(file.package)))
            {
                return Err("generated split MXF file identity differs from preparation".into());
            }
        }
    }
    for (index, (timeline, read)) in p.timelines.iter().zip(&graph.sequences).enumerate() {
        let count = if let Some(group) = &timeline.group {
            group.prepared.lanes.iter().map(Vec::len).sum::<usize>()
        } else {
            timeline.lanes.iter().map(Vec::len).sum::<usize>()
        };
        let folder = graph
            .sidecar("resolve.source", &read.id)
            .and_then(|r| r.get("bin_id"));
        if read.id != ids.sequences[index]
            || read.name != timeline.name
            || read.rate != p.rate
            || read.frame_size != Some((1920, 1080))
            || read.clock.as_ref().map(|clock| clock.effective_count)
                != Some(timeline.origin as u64)
            || read.placements.len() != count
            || read.markers.len() != timeline.markers.len()
            || !matches!(folder, Some(Value::Str(id)) if *id == ids.bins[timeline.bin])
        {
            return Err(format!(
                "generated Resolve timeline disagrees with {}",
                timeline.name
            )
            .into());
        }
        if DirectMixer::prepare(&graph, read)? != timeline.mixer {
            return Err(
                format!("generated Fairlight mixer disagrees with {}", timeline.name).into(),
            );
        }
        if let Some(group) = &timeline.group {
            let id = ids
                .group
                .as_deref()
                .ok_or("generated Group identity absent")?;
            if graph.multicams.len() != 1
                || graph.multicams[0].id != id
                || graph.multicams[0].name != group.prepared.group_name
            {
                return Err("generated Group definition differs from preparation".into());
            }
            let uses: Vec<_> = graph
                .multicam_uses
                .iter()
                .filter(|u| u.sequence == read.id)
                .collect();
            if uses.len() != count {
                return Err("generated Group use count differs from preparation".into());
            }
            for (lane, edits) in group.prepared.lanes.iter().enumerate() {
                let kind = if lane == 0 {
                    graph::StreamKind::Video
                } else {
                    graph::StreamKind::Audio
                };
                let track = lane.max(1) as i64;
                let native = uses.iter().filter(|u| u.kind == kind && u.track == track);
                for (saved, wanted) in native.zip(edits) {
                    let frame = |time| crate::prepare::frames_on(time, p.rate);
                    let [placement] = saved.binding.placements.as_slice() else {
                        return Err("generated Group use lacks one leaf placement".into());
                    };
                    let leaf = read
                        .placements
                        .iter()
                        .find(|p| &p.id == placement)
                        .ok_or("generated Group leaf placement absent")?;
                    if saved.candidates.as_slice() != [id]
                        || saved.selected != Some(wanted.camera as i64)
                        || saved.source_in.map(frame).transpose()? != Some(wanted.edit.source_in)
                        || frame(leaf.record_start)? != wanted.edit.start
                        || frame(leaf.source_in)?
                            != wanted.edit.source_in - group.prepared.offsets[wanted.camera]
                        || frame(leaf.duration)? != wanted.edit.length
                        || leaf.item != ids.media[group.cameras[wanted.camera]]
                        || (lane != 0 && leaf.audio_streams.as_slice() != [lane as i64])
                    {
                        return Err(
                            "generated Group angle, range or channel differs from preparation"
                                .into(),
                        );
                    }
                }
                if uses
                    .iter()
                    .filter(|u| u.kind == kind && u.track == track)
                    .count()
                    != edits.len()
                {
                    return Err("generated Group lane count differs from preparation".into());
                }
            }
            continue;
        }
        for (lane, expected) in timeline.lanes.iter().enumerate() {
            let actual = read.placements.iter().filter(|edit| match lane {
                0 => edit.video_track == Some(1),
                audio => edit.audio_tracks.as_slice() == [audio as i64],
            });
            for (edit, wanted) in actual.zip(expected) {
                let frame = |time| crate::prepare::frames_on(time, p.rate);
                if edit.item != ids.media[wanted.media]
                    || frame(edit.record_start)? != wanted.start
                    || frame(edit.source_in)? != wanted.source_in
                    || frame(edit.duration)? != wanted.length
                    || !crate::video::matches_readback(edit, wanted.video.as_ref())
                    || edit.volume_db != wanted.volume
                    || !edit.volume_keys.is_empty()
                    || !edit
                        .audio_streams
                        .iter()
                        .copied()
                        .eq(wanted.channel.map(|index| index as i64 + 1))
                {
                    return Err("generated Resolve edit differs from preparation".into());
                }
            }
            if read
                .placements
                .iter()
                .filter(|edit| match lane {
                    0 => edit.video_track == Some(1),
                    audio => edit.audio_tracks.as_slice() == [audio as i64],
                })
                .count()
                != expected.len()
            {
                return Err("generated Resolve track count differs from preparation".into());
            }
        }
    }
    Ok(())
}
