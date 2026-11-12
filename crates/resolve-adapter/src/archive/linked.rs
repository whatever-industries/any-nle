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
const THREE_MONO_TEMPLATE: &str = "346c93372828c5068323833437cbb4331ea1b8520d546e75850ea522926b771b";
const THREE_MONO_SEQUENCE: &str = "SeqContainer/2ce73793-25f7-49d0-88d6-e3c95a01841f.xml";
const NEST_TEMPLATE: &str = "48aa85bd7184d72532b52f993158e7c515b9e820887855f4e1065798b7e5263a";
const NEST_PARENT: &str = "SeqContainer/d35c263e-bf97-4a72-8876-c35e133c6601.xml";
const NEST_COMPOUND: &str = "SeqContainer/bdfee1f6-00ff-4a76-94fb-662551ca1846.xml";
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
            nested: None,
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
    let two_bus = prepared
        .timelines
        .iter()
        .any(|row| row.mixer.is_some_and(|m| m.buses == 2));
    let three_mono = prepared.timelines.iter().any(|row| !row.lanes[4].is_empty());
    if two_bus && three_mono {
        return Err("two-bus and three-mono Resolve templates cannot be combined".into());
    }
    let fairlight = if two_bus {
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
    let wider_layout = if three_mono {
        let path = templates.fairlight.as_deref().ok_or("missing pinned three-mono layout template")?;
        Some(template("three-mono layout", path, THREE_MONO_TEMPLATE, 4)?)
    } else {
        None
    };
    let nest_template = if prepared.timelines.iter().any(|row| !row.nests.is_empty()) {
        let path = templates.nested.as_deref().ok_or("missing pinned Resolve nesting template")?;
        Some(template("nesting", path, NEST_TEMPLATE, 9)?)
    } else { None };
    build(
        prepared,
        &layout,
        &profile,
        atom_donor.as_deref(),
        templates.group.as_deref(),
        fairlight.as_ref(),
        wider_layout.as_ref(),
        nest_template.as_ref(),
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
    labels: &mut Vec<(String, crate::clip_label::ClipLabel)>,
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
    prefix_name(&mut pool, &timeline.name)?;
    if let Some(marks) = timeline.marks {
        for (tag, frame) in marks.fields() {
            set(&mut pool, tag, &frame.to_string())?;
        }
    }
    replace_fields(&mut pool, |data| Ok(remap_blob(data, &ids)))?;
    let (old, mut handle, handle_id) = if timeline.compound {
        (String::new(), String::new(), String::new())
    } else {
        set(&mut pool, "CurPlayheadPosition", "0")?;
        let old = block(&pool, "Sm2Timeline")?;
        let mut handle = old.clone();
        let head_end = handle.find("<Sequence>").ok_or("missing timeline sequence wrapper")?;
        let mut head = handle[..head_end].to_owned();
        set(&mut head, "Name", &xml(&timeline.name))?;
        handle.replace_range(..head_end, &head);
        let id = dbid(&handle)?.to_owned();
        (old, handle, id)
    };
    let old_sequence = block(if timeline.compound { &pool } else { &handle }, "Sm2Sequence")?;
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
    let fields = if let Some(tracks) = timeline.track_state.as_ref().filter(|s| {
        s.0[1..].iter().any(|track| !track.name.is_empty() || track.locked)
    }) {
        Mixer::decode(&fields)?.ok_or("missing Fairlight track-state template")?
            .write_track_state(&tracks.0[1..])?
    } else {
        fields
    };
    set(
        &mut fields_head,
        "FieldsBlob",
        &hex::encode(remap_blob(hex::decode(fields)?, &ids)),
    )?;
    sequence.replace_range(..fields_end, &fields_head);
    if timeline.compound {
        replace_once(&mut pool, &old_sequence, &sequence)?;
        let old = tag(&pool, "VideoMetadata")?;
        let new = remap_blob(hex::decode(old)?, &ids);
        set(&mut pool, "VideoMetadata", &hex::encode(new))?;
    } else {
        replace_once(&mut handle, &old_sequence, &sequence)?;
        replace_once(&mut pool, &old, &handle)?;
    }
    let second_picture = !timeline.lanes[3].is_empty();
    let third_mono = !timeline.lanes[4].is_empty();
    if second_picture && !third_mono {
        let video = block(&container, "VideoTrackVec")?;
        let track = block(&video, "Sm2TiTrack")?;
        let mut extra = track.clone();
        replace_once(&mut extra, dbid(&track)?, &uid())?;
        let mut expanded = video.clone();
        replace_once(&mut expanded, "</VideoTrackVec>", &format!("<Element>{extra}</Element></VideoTrackVec>"))?;
        replace_once(&mut container, &video, &expanded)?;
    }
    let item_lists = Regex::new(r"(?s)<Items(?:/>|>.*?</Items>)")?;
    let spans: Vec<_> = item_lists
        .find_iter(&container)
        .map(|m| m.range())
        .collect();
    let lane_order: &[usize] = if timeline.compound {
        &[0, 1]
    } else if third_mono {
        &[0, 3, 1, 2, 4]
    } else if second_picture {
        &[0, 3, 1, 2]
    } else {
        &[0, 1, 2]
    };
    if spans.len() != lane_order.len() {
        return Err("expected V1 and two native mono track templates".into());
    }
    for (&lane, span) in lane_order.iter().zip(spans).rev() {
        let base = if timeline.lanes[lane].is_empty() {
            ""
        } else {
            blocks(if matches!(lane, 0 | 3) { "Sm2TiVideoClip" } else { "Sm2TiAudioClip" })?
                .find(&container[span.clone()])
                .ok_or("missing native edit template")?.as_str()
        };
        let mut list = String::from("<Items>");
        for edit in &timeline.lanes[lane] {
            if let Some(dissolve) = timeline
                .dissolve
                .as_ref()
                .filter(|dissolve| lane == usize::from(dissolve.audio) && edit.start == dissolve.cut)
            {
                write_dissolve(&mut list, timeline.origin, dissolve);
            }
            let source = &media[edit.media];
            let mut clip = base.to_owned();
            let old_id = dbid(&clip)?.to_owned();
            let clip_id = uid();
            replace_once(&mut clip, &old_id, &clip_id)?;
            if let Some(label) = edit.label {
                labels.push((clip_id, label));
            }
            set(&mut clip, "Name", &xml(&source.name))?;
            set(&mut clip, "MediaRef", &media_ids[edit.media])?;
            set(&mut clip, "MediaFilePath", &xml(utf8_path(&source.path)?))?;
            set(&mut clip, "MediaFrameRate", &rate_hex)?;
            set(
                &mut clip,
                "Start",
                &(timeline.origin + edit.start).to_string(),
            )?;
            set(&mut clip, "Duration", &edit.length.to_string())?;
            if let Some(video) = &edit.video {
                set(&mut clip, "EffectFiltersBA", &video.encoded)?;
            }
            if let Some(envelope) = edit.envelope {
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
            let (saved_in, map) = match (edit.speed, &source.profile) {
                (Some(speed), Profile::Mov { frames, .. }) => {
                    let saved_in = speed.record_source_in(edit.source_in, edit.length, *frames)
                        .ok_or("invalid Resolve retime source trim")?;
                    set(&mut clip, "EffectFiltersBA", crate::retime::NEAREST)?;
                    (saved_in, speed.encode(*frames, &uid())?)
                }
                (Some(_), _) => return Err("Resolve picture speed requires linked MOV media".into()),
                (None, _) => {
                    let mut map = vec![2];
                    map.extend_from_slice(&end.to_be_bytes());
                    (edit.source_in, map)
                }
            };
            set(&mut clip, "In", &saved_in.to_string())?;
            set(&mut clip, "MediaTimemapBA", &hex::encode(map))?;
            if let Some((_, body)) = &edit.grade {
                write_grade(&mut clip, body)?;
            }
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
        if lane == 0 {
            if let Some(solid) = &timeline.solid {
                use std::fmt::Write as _;
                write!(
                    list,
                    concat!(
                        "<Element><Sm2TiGenerator DbId=\"{}\"><FieldsBlob/><PrettyType>Solid Color</PrettyType>",
                        "<Name>Solid Color</Name><Start>{}</Start><Duration>{}</Duration><LinkedItemSync/>",
                        "<WasDisbanded>false</WasDisbanded><MarkersBA/><UiMemento>0</UiMemento><Flags>0</Flags>",
                        "<PriorityIndex>0</PriorityIndex><EffectFiltersBA>{}</EffectFiltersBA>",
                        "<ImportExportMetadataBA/><RenderTextEnabled>true</RenderTextEnabled>",
                        "<RenderTextGanged>true</RenderTextGanged><RenderTextPrefixed>true</RenderTextPrefixed>",
                        "<In/></Sm2TiGenerator></Element>",
                    ),
                    uid(), timeline.origin + solid.start, solid.duration, solid.effect(),
                )?;
            }
        }
        list.push_str("</Items>");
        container.replace_range(span, &list);
    }
    if let Some(state) = &timeline.track_state {
        let pattern = blocks("Sm2TiTrack")?;
        let spans: Vec<_> = pattern.find_iter(&container).map(|row| row.range()).collect();
        if spans.len() != state.0.len() {
            return Err("unexpected Resolve track-state template layout".into());
        }
        for (span, track) in spans.into_iter().zip(&state.0).rev() {
            let mut xml_track = container[span.clone()].to_owned();
            let at = xml_track.find("<Items").ok_or("missing Resolve track items")?;
            if track.locked || track.disabled {
                let mut head = xml_track[..at].to_owned();
                let flags: u32 = tag(&head, "Flags")?.parse()?;
                let flags = flags | u32::from(track.locked) | (u32::from(track.disabled) << 1);
                set(&mut head, "Flags", &flags.to_string())?;
                xml_track.replace_range(..at, &head);
            }
            if !track.name.is_empty() {
                set(&mut xml_track, "UserDefinedName", &xml(&track.name))?;
            }
            container.replace_range(span, &xml_track);
        }
    }
    if !timeline.captions.is_empty() {
        write_captions(&mut container, timeline, &sequence_id)?;
    }
    Ok((
        handle_id,
        sequence_id,
        dbid(&container)?.to_owned(),
        pool,
        container,
    ))
}

fn write_nested_uses(
    parent: &Timeline,
    project: &PreparedProject,
    pools: &[Option<String>],
    template: &str,
    container: &mut String,
    remaining_xml: &mut usize,
) -> Result<()> {
    let old_video = block(container, "VideoTrackVec")?;
    let old_track = block(&old_video, "Sm2TiTrack")?;
    let clip_pattern = blocks("Sm2TiVideoClip")?;
    let clips = clip_pattern.find_iter(&old_track).map(|m| m.as_str());
    let mut edits: Vec<_> = parent.lanes[0].iter().map(|edit| edit.start).zip(clips)
        .map(|(start, clip)| (start, clip.to_owned())).collect();
    if edits.len() != parent.lanes[0].len() {
        return Err("generated Resolve parent V1 layout differs from preparation".into());
    }
    for usage in &parent.nests {
        let child = &project.timelines[usage.child];
        let pool = pools[usage.child].as_ref().ok_or("missing generated child pool identity")?;
        let mut clip = remap(template.to_owned(), &new_ids(template)?);
        for (key, value) in [
            ("Name", xml(&child.name)),
            ("Start", (parent.origin + usage.start).to_string()),
            ("Duration", usage.length.to_string()),
            ("In", usage.source_in.to_string()),
            ("MediaRef", pool.to_owned()),
            ("MediaStartTime", (usage.origin as f64 / 24.0).to_string()),
            ("MediaTimemapBA", hex::encode(crate::nests::compact_map(usage.child_length))),
        ] {
            set(&mut clip, key, &value)?;
        }
        *remaining_xml = remaining_xml.checked_sub(clip.len() + 19)
            .ok_or("nested edit XML exceeds its construction budget")?;
        edits.push((usage.start, clip));
    }
    edits.sort_unstable_by_key(|(start, _)| *start);
    let list = edits.into_iter().map(|(_, xml)| format!("<Element>{xml}</Element>")).collect::<String>();
    let mut track = old_track.clone();
    container_field(&mut track, "Items", &list)?;
    let mut video = old_video.clone();
    replace_once(&mut video, &old_track, &track)?;
    replace_once(container, &old_video, &video)
}

fn write_grade(clip: &mut String, body: &str) -> Result<()> {
    use crate::binary::Qt;
    let active = uid();
    let table = uid();
    let fields = hex::encode(qt(&[("LastChangedTime", Qt::Long(0)), ("ActiveVersionType", Qt::Int(0))]));
    let grade = format!(
        concat!(
            "<pLmVerTable><ListMgt::LmVersionTable DbId=\"{table}\"><FieldsBlob>{fields}</FieldsBlob>",
            "<VerType>0</VerType><pActive>{active}</pActive><Locals><Element>",
            "<ListMgt::LmVersion DbId=\"{active}\"><FieldsBlob/><Name>Version 1</Name>",
            "<HasCorrection>true</HasCorrection><VerType>0</VerType><ImplVersion>1</ImplVersion>",
            "<IncludedInRecording>true</IncludedInRecording><FlatPassEnabled>false</FlatPassEnabled>",
            "<RGBAOutputEnabled>false</RGBAOutputEnabled><Body>{body}</Body>",
            "<UseVersionClipProcParams>true</UseVersionClipProcParams></ListMgt::LmVersion>",
            "</Element></Locals><DbSavedTime>0</DbSavedTime></ListMgt::LmVersionTable></pLmVerTable>"
        ),
        table=table, fields=fields, active=active, body=body
    );
    if clip.contains("<pLmVerTable>") {
        return Err("Resolve skeleton already contains a clip grade".into());
    }
    let anchor = clip.find("<LastChangedTime>").ok_or("missing Resolve grade insertion anchor")?;
    clip.insert_str(anchor, &grade);
    Ok(())
}

fn write_captions(container: &mut String, timeline: &Timeline, sequence_id: &str) -> Result<()> {
    use std::fmt::Write as _;
    let mut track = format!(
        "<SubtitleTrackVec><Element><Sm2TiTrack DbId=\"{}\"><FieldsBlob>{}</FieldsBlob>\
         <Type>2</Type><SubType>0</SubType><Flags>0</Flags><Sequence>{sequence_id}</Sequence><Items>",
        uid(), crate::captions::TRACK_FIELDS,
    );
    for caption in &timeline.captions {
        if track.len().saturating_add(caption.text.len().saturating_mul(6) + 640) as u64 > MAX_XML {
            return Err("Resolve subtitle track exceeds its construction budget".into());
        }
        write!(
            track,
            concat!(
                "<Element><Sm2TiGenerator DbId=\"{}\"><FieldsBlob/><PrettyType>Subtitle</PrettyType>",
                "<Name>{}</Name><Start>{}</Start><Duration>{}</Duration><LinkedItemSync/>",
                "<WasDisbanded>false</WasDisbanded><MarkersBA/><UiMemento>0</UiMemento><Flags>0</Flags>",
                "<PriorityIndex>0</PriorityIndex><EffectFiltersBA/><ImportExportMetadataBA/>",
                "<RenderTextEnabled>true</RenderTextEnabled><RenderTextGanged>true</RenderTextGanged>",
                "<RenderTextPrefixed>true</RenderTextPrefixed><In/></Sm2TiGenerator></Element>",
            ),
            uid(), xml(&caption.text), timeline.origin + caption.start, caption.length,
        )?;
    }
    track.push_str("</Items><UserDefinedName/><LayersVec/></Sm2TiTrack></Element></SubtitleTrackVec>");
    let empty = one(&Regex::new(r"<SubtitleTrackVec(?:\s*/>|>\s*</SubtitleTrackVec>)")?, container)?;
    container.replace_range(empty.range(), &track);
    Ok(())
}

fn write_dissolve(list: &mut String, origin: i64, transition: &Dissolve) {
    use std::fmt::Write as _;
    // Resolve keeps the visible edit ranges and stores overlap on the transition.
    let start = origin + transition.window().start;
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
        fields = if transition.audio { dissolve::AUDIO_FIELDS } else { dissolve::FIELDS },
        kind = transition.type_label(),
        start = start,
        duration = dissolve::DURATION,
        effect = if transition.audio { "" } else { dissolve::EFFECT },
        alignment = transition.alignment(),
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
    timeline_pools: Vec<Option<String>>,
    group: Option<String>,
}

fn build(
    p: &PreparedProject,
    layout: &Members,
    profile: &Members,
    atom_donor: Option<&str>,
    group_template: Option<&Path>,
    fairlight: Option<&Mixer>,
    wider_layout: Option<&Members>,
    nest_template: Option<&Members>,
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
    let wide_pool = wider_layout.map(|members|
        named(member(members, POOL)?, "Sm2MpTimelineClip", "Native lanes")).transpose()?;
    let wider_donors = match (wider_layout, wide_pool.as_deref()) {
        (Some(members), Some(pool)) => Some(TimelineDonors {
            pool, container: member(members, THREE_MONO_SEQUENCE)?, fairlight: None,
        }),
        _ => None,
    };
    let compound_pool = nest_template.map(|members| block(member(members, POOL)?, "Sm2MpCompoundClip")).transpose()?;
    let compound_donors = match (nest_template, compound_pool.as_deref()) {
        (Some(members), Some(pool)) => Some(TimelineDonors {
            pool, container: member(members, NEST_COMPOUND)?, fairlight: None,
        }),
        _ => None,
    };
    let nest_edit = nest_template.map(|members|
        named(member(members, NEST_PARENT)?, "Sm2TiVideoClip", "Witness A")).transpose()?;
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
    let mut timeline_pools = Vec::new();
    let mut timeline_containers = Vec::new();
    let mut group_id = None;
    let mut labels = Vec::new();
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
            timeline_pools.push(None);
            timeline_containers.push(None);
            group_id = Some(built.id);
            for (name, contents) in built.containers {
                if members.insert(name, contents).is_some() {
                    return Err("Group sequence container identity collision".into());
                }
            }
            continue;
        }
        let donors = if timeline.compound {
            compound_donors.as_ref().ok_or("missing native compound template")?
        } else if timeline.lanes[4].is_empty() {
            &donors
        } else {
            wider_donors.as_ref().ok_or("missing three-mono timeline layout")?
        };
        let (handle, sequence, container_id, pool, container) = write_timeline(
            timeline,
            p.rate,
            &bin_ids[timeline.bin],
            &p.media,
            &media_ids,
            donors,
            &mut labels,
        )?;
        remaining_xml = remaining_xml
            .checked_sub(pool.len() + container.len() + 19)
            .ok_or("project XML exceeds its construction budget")?;
        children[timeline.bin].push_str(&format!("<Element>{pool}</Element>"));
        if !timeline.compound {
            handles.push_str(&format!("<Element>{handle}</Element>"));
            markers::write(&mut project, &sequence, &timeline.markers)?;
        }
        timeline_pools.push(Some(dbid(&pool)?.to_owned()));
        let path = format!("SeqContainer/{container_id}.xml");
        timeline_containers.push(Some(path.clone()));
        sequence_ids.push(sequence);
        if members
            .insert(path, container)
            .is_some()
        {
            return Err("sequence container identity collision".into());
        }
    }
    if let Some(template) = nest_edit.as_deref() {
        for (timeline, path) in p.timelines.iter().zip(timeline_containers) {
            if timeline.nests.is_empty() { continue; }
            let target = path.and_then(|path| members.get_mut(&path)).ok_or("missing parent container")?;
            write_nested_uses(timeline, p, &timeline_pools, template, target, &mut remaining_xml)?;
        }
    }
    markers::write_labels(&mut project, &labels)?;
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
            timeline_pools,
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
    let registered = p.timelines.iter().zip(&ids.sequences)
        .filter(|(timeline, _)| !timeline.compound).map(|(_, id)| id);
    if !graph.sequences.iter().filter(|s| !crate::nests::compound(&graph,&s.id))
        .map(|s| &s.id).eq(registered.clone())
    {
        return Err("generated Resolve registered timeline order differs from preparation".into());
    }
    let expected_id = registered.clone().nth(p.active_timeline_index)
        .ok_or("generated Resolve active timeline index is absent")?;
    if active.get("saved_index") != Some(&expected_index)
        || !matches!(active.get("sequence_id"), Some(Value::Str(id)) if id == expected_id)
        || active.get("unavailable_reason") != Some(&Value::Null)
    {
        return Err("generated Resolve active timeline differs from preparation".into());
    }
    let readbacks: Vec<_> = ids.sequences.iter().map(|id|
        graph.sequences.iter().find(|sequence| sequence.id == *id)
            .ok_or("generated Resolve sequence ID is absent")).collect::<std::result::Result<_, _>>()?;
    let mut dissolves = Vec::with_capacity(p.timelines.len());
    let mut nested_ids = std::collections::HashSet::new();
    for (timeline, &read) in p.timelines.iter().zip(&readbacks) {
        use crate::nests::{int, string, ADAPTER, PROFILE};
        match graph.sidecar(ADAPTER, &read.id) {
            None if timeline.nests.is_empty() => {}
            Some(Value::Arr(rows)) if rows.len() == timeline.nests.len() => {
                for (row, expected) in rows.iter().zip(&timeline.nests) {
                    let child = &ids.sequences[expected.child];
                    let pool = ids.timeline_pools[expected.child].as_deref().ok_or("missing staged child pool")?;
                    let id = string(row,"id").ok_or("missing staged nested occurrence ID")?;
                    if !nested_ids.insert((read.id.as_str(), id))
                        || string(row,"profile") != Some(PROFILE)
                        || string(row,"child_sequence") != Some(child.as_str())
                        || string(row,"pool_id") != Some(pool)
                        || string(row,"kind") != Some(expected.kind.saved())
                        || string(row,"track_kind") != Some("video") || int(row,"track") != Some(1)
                        || string(row,"record_rate") != Some("24") || string(row,"child_rate") != Some("24")
                        || int(row,"child_origin_frames") != Some(expected.origin)
                        || int(row,"record_start_frames") != Some(expected.start)
                        || int(row,"record_duration_frames") != Some(expected.length)
                        || int(row,"saved_source_in_frames") != Some(expected.source_in)
                    {
                        return Err("generated Resolve nested binding or trim differs from preparation".into());
                    }
                }
            }
            _ => return Err("generated Resolve nested use count differs from preparation".into()),
        }
        let saved = dissolve::prepare(&graph, read)?;
        let window = |dissolve: &Dissolve| (dissolve.audio, dissolve.window());
        if saved.as_ref().map(window) != timeline.dissolve.as_ref().map(window) {
            return Err("generated Resolve dissolve differs from preparation".into());
        }
        dissolves.push(saved);
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
    let mut recovered_audio = std::collections::HashMap::new();
    let mut recovered_labels = HashSet::new();
    for (index, (timeline, &read)) in p.timelines.iter().zip(&readbacks).enumerate() {
        let count = if let Some(group) = &timeline.group {
            group.prepared.lanes.iter().map(Vec::len).sum::<usize>()
        } else {
            timeline.lanes.iter().map(Vec::len).sum::<usize>()
        };
        let owner = graph.sidecar("resolve.source", &read.id).ok_or("generated Resolve sequence owner is absent")?;
        let folder = owner.get("bin_id");
        if timeline.compound && (crate::nests::string(owner,"kind") != Some("compound")
            || owner.get("container_ref_agrees") != Some(&Value::Bool(true))
            || crate::nests::string(owner,"media_pool_clip") != ids.timeline_pools[index].as_deref())
        {
            return Err("generated Resolve compound ownership or container reference differs".into());
        }
        if read.id != ids.sequences[index]
            || read.name != timeline.name
            || read.rate != p.rate
            || read.frame_size != Some((1920, 1080))
            || read.clock.as_ref().map(|clock| clock.effective_count)
                != Some(timeline.origin as u64)
            || owner.get("record_length") != Some(&Value::Int(timeline.length))
            || read.placements.len() != count
            || crate::prepare::prepare_markers(&graph, read, timeline.length)?.0 != timeline.markers
            || read.markers.len() != timeline.markers.len()
            || crate::timeline_marks::TimelineMarks::prepare(&graph, &read.id, timeline.length)?
                != timeline.marks
            || crate::track_state::TrackState::prepare(&graph, read)? != timeline.track_state
            || read.texts.len() != timeline.captions.len()
            || crate::generator::prepare(&graph, read)? != timeline.solid
            || !matches!(folder, Some(Value::Str(id)) if *id == ids.bins[timeline.bin])
        {
            return Err(format!(
                "generated Resolve timeline disagrees with {}",
                timeline.name
            )
            .into());
        }
        for (saved, expected) in read.texts.iter().zip(&timeline.captions) {
            let frame = |time| crate::prepare::frames_on(time, p.rate);
            if saved.track != 1
                || saved.kind != graph::source::TextKind::Caption
                || saved.reason.is_some()
                || saved.content.as_deref() != Some(expected.text.as_str())
                || saved.record_start.map(frame).transpose()? != Some(expected.start)
                || saved.duration.map(frame).transpose()? != Some(expected.length)
            {
                return Err("generated Resolve subtitle differs from preparation".into());
            }
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
                3 => edit.video_track == Some(2),
                4 => edit.audio_tracks.as_slice() == [3],
                audio => edit.audio_tracks.as_slice() == [audio as i64],
            });
            for (edit, wanted) in actual.zip(expected) {
                if let Some(loss) = wanted.envelope.and_then(crate::audio::Envelope::recovered_loss) {
                    recovered_audio.insert(edit.id.as_str(), loss);
                }
                if wanted.label.is_some() {
                    recovered_labels.insert(edit.id.as_str());
                }
                let frame = |time| crate::prepare::frames_on(time, p.rate);
                if edit.item != ids.media[wanted.media]
                    || frame(edit.record_start)? != wanted.start
                    || frame(edit.source_in)? != wanted.source_in
                    || frame(edit.duration)? != wanted.length
                    || edit.speed != wanted.speed.and_then(|speed| speed.graph(wanted.length))
                    || crate::grade::prepare(&graph, &edit.id)?.map(|grade| grade.0)
                        != wanted.grade.as_ref().map(|grade| grade.0)
                    || !crate::video::matches_readback(edit, wanted.video.as_ref())
                    || crate::clip_label::ClipLabel::prepare(&graph, &edit.id)? != wanted.label
                    || (matches!(lane, 1 | 2 | 4)
                        && crate::audio::prepare(
                            edit, wanted.source_in, wanted.length, p.rate,
                            graph.sidecar(crate::source::ADAPTER, &edit.id),
                        )? != wanted.envelope)
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
                    3 => edit.video_track == Some(2),
                    4 => edit.audio_tracks.as_slice() == [3],
                    audio => edit.audio_tracks.as_slice() == [audio as i64],
                })
                .count()
                != expected.len()
            {
                return Err("generated Resolve track count differs from preparation".into());
            }
        }
    }
    if graph.losses.iter().any(|loss| {
        if recovered_labels.contains(loss.object.as_str()) && crate::clip_label::ClipLabel::recover(loss) {
            return false;
        }
        if recovered_audio
            .get(loss.object.as_str())
            .is_some_and(|(aspect, reason)| loss.aspect == *aspect && loss.reason == *reason)
        {
            return false;
        }
        if loss.aspect == "placement" && loss.reason.split_once(": ").is_some_and(|(id, reason)|
            reason == crate::nests::LOSS && nested_ids.contains(&(loss.object.as_str(), id)))
        {
            return false;
        }
        !p.timelines
            .iter()
            .zip(&readbacks)
            .zip(&dissolves)
            .any(|((timeline, read), dissolve)| {
                timeline.mixer.is_some() && DirectMixer::recover(loss, &read.id)
                    || timeline.track_state.is_some() && crate::track_state::TrackState::recover(loss, &read.id)
                    || timeline.marks.is_some() && crate::timeline_marks::TimelineMarks::recover(loss, &read.id)
                    || dissolve.as_ref().is_some_and(|d| d.recover(loss, &read.id))
                    || crate::nests::recover_empty_stereo(&graph, loss, &read.id)
            })
    })
    {
        return Err(format!(
            "generated Resolve reference graph differs from prepared project: {:?}",
            graph.losses
        )
        .into());
    }
    Ok(())
}
