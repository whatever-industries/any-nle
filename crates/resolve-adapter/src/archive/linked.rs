//! Construct a project from prepared bin, media and timeline records.
use super::*;
use crate::linked::{samples_per_frame, Media, PreparedProject, Profile, Timeline};
use graph::{Rational, Value};

const TEMPLATE_23976: &str = "4b2895fa6827801bce87bff7f657dae6454f7011459b689d1983bb11a017ccfc";
const TEMPLATE_24: &str = "e2a3a1ad16be932947914d866b3c8bb49b549ffd5869a06a9fc9eb576bf21bf5";
const MEDIA_DONOR: &str = "MediaPool/Master/002_Linked/000_Audio/MpFolder.xml";
const TIMELINE_DONOR: &str = "SeqContainer/93a83077-933a-4ed8-9595-e12534b57e18.xml";
const PRORES_REC709: &str = "80087810027002789e01800166880132980100d80100";

pub fn publish_linked(
    prepared: &PreparedProject,
    layout_template: &Path,
    profile_template: &Path,
    output: &Path,
) -> Result<Published> {
    check_output(output)?;
    let layout = load_skeleton(layout_template, TEMPLATE_23976, 13)?;
    let profile = if prepared.rate == Rational::integer(24) {
        load_skeleton(profile_template, TEMPLATE_24, 10)?
    } else {
        load_skeleton(profile_template, TEMPLATE_23976, 13)?
    };
    let (members, identities) = build(prepared, &layout, &profile)?;
    write_checked_archive(members, output, |stage| {
        verify_stage(prepared, &identities, stage)?;
        for media in &prepared.media {
            media.recheck()?;
        }
        Ok(())
    })
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

fn clip_cache(media: &Media, video: bool) -> Result<String> {
    let folder = utf8_path(media.path.parent().ok_or("missing linked media parent")?)?;
    let file = media
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid linked basename")?;
    let mut data = Vec::new();
    pb_bytes(1, folder.as_bytes(), &mut data);
    pb_bytes(2, file.as_bytes(), &mut data);
    pb_bytes(5, if video { b"apch" } else { b"Linear PCM" }, &mut data);
    if video {
        pb_bytes(6, file.as_bytes(), &mut data);
        pb_bytes(7, uid().as_bytes(), &mut data);
    }
    pb_int(13, media.modified_micros, &mut data);
    if matches!(
        media.profile,
        Profile::Mov {
            quicktime_timecode: false,
            ..
        }
    ) {
        pb_int(14, 24584, &mut data);
    }
    pb_int(15, 8, &mut data);
    pb_int(16, 100, &mut data);
    pb_int(18, if video { 16384 } else { 32768 }, &mut data);
    pack(&data)
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
) -> Result<(String, String)> {
    let donor = match media.profile {
        Profile::Mov { .. } => named(source, "Sm2MpVideoClip", "Counter-23976.mov")?,
        Profile::Wav { bits, channels, .. } => {
            let name = format!(
                "{}-{bits}.wav",
                if channels == 1 { "Mono" } else { "Signals" }
            );
            named(source, "Sm2MpAudioClip", &name)?
        }
    };
    let ids = new_ids(&donor)?;
    let mut item = remap(donor, &ids);
    let id = dbid(&item)?.to_owned();
    set(&mut item, "MpFolder", bin)?;
    prefix_name(&mut item, &media.name)?;
    match media.profile {
        Profile::Mov {
            rate,
            frames,
            tagged_rec709,
            ..
        } => {
            let old = block(&item, "BtVideoInfo")?;
            let mut video = old.clone();
            set(&mut video, "Clip", &clip_cache(media, true)?)?;
            let timing = qt(&[
                ("UniqueId", Text(&uid())),
                ("StartFrame", Int(0)),
                ("NumFrames", Int(i32::try_from(frames)?)),
                ("FrameRate", Bytes(&rate_bytes(rate))),
                ("DbType", Text("BtVideoTime")),
            ]);
            set(&mut video, "Time", &hex::encode(timing))?;
            if tagged_rec709 {
                set(&mut video, "Radiometry", PRORES_REC709)?;
            }
            let metadata = qt(&[
                ("UniqueId", Text(&uid())),
                ("DbType", Text("BtVideoMetadata")),
            ]);
            set(&mut video, "VideoMetadata", &hex::encode(metadata))?;
            replace_once(&mut item, &old, &video)?;
        }
        Profile::Wav {
            samples,
            bits,
            channels,
        } => {
            let old = block(&item, "BtAudioInfo")?;
            let mut audio = old.clone();
            let audio_id = dbid(&audio)?.to_owned();
            let depth = if bits == 16 { 1 } else { 2 };
            set(&mut audio, "Clip", &clip_cache(media, false)?)?;
            let mut track = vec![
                ("UniqueId", Text(&audio_id)),
                ("StartTime", Double(0.0)),
                ("SampleRate", Uint(48000)),
                ("NumChannels", Int(channels as i32)),
                ("IdxTrack", Int(0)),
                ("Duration", Long(samples)),
                ("DbType", Text("BtAudioTrack")),
                ("CodecName", Text("Linear PCM")),
            ];
            if depth == 2 {
                track.push(("ChannelLayout", Int(channels as i32)));
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
            let channels_map = if channels == 2 {
                qt(&[("1", Bytes(&right)), ("0", Bytes(&left))])
            } else {
                qt(&[("0", Bytes(&left))])
            };
            let bounds = extent(0.0, samples as f64 / 48000.0);
            let mut route = Vec::new();
            if channels == 2 {
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
    }
    Ok((id, item))
}

fn write_timeline(
    timeline: &Timeline,
    rate: Rational,
    bin: &str,
    media: &[Media],
    media_ids: &[String],
    pool_donor: &str,
    container_donor: &str,
) -> Result<(String, String, String, String, String)> {
    let mut ids = new_ids(pool_donor)?;
    for (old, new) in new_ids(container_donor)? {
        if ids.insert(old, new).is_some() {
            return Err("colliding timeline template identities".into());
        }
    }
    let mut pool = remap(pool_donor.to_owned(), &ids);
    let mut container = remap(container_donor.to_owned(), &ids);
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
    let fields = tag(&fields_head, "FieldsBlob")?;
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
            let end = match source.profile {
                Profile::Mov { frames, .. } => (frames - 1) as f64 / fps(rate),
                Profile::Wav { samples, .. } => {
                    (samples - samples_per_frame(rate)) as f64 / 48000.0
                }
            };
            let mut map = vec![2];
            map.extend_from_slice(&end.to_be_bytes());
            set(&mut clip, "MediaTimemapBA", &hex::encode(map))?;
            if let Some(channel) = edit.channel {
                let mut channels = [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 1];
                channels[11] = u8::try_from(channel + 1)?;
                set(
                    &mut clip,
                    "VirtualAudioTrackBA",
                    &hex::encode(qt(&[
                        ("ChannelsBA", Bytes(&channels)),
                        ("AudioType", Int(1)),
                    ])),
                )?;
                set(&mut clip, "MediaTrackIdx", "0")?;
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
}

fn build(
    p: &PreparedProject,
    layout: &Members,
    profile: &Members,
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
    let container_donor = member(layout, TIMELINE_DONOR)?;
    for media in &p.media {
        let (id, item) = pool_item(donor, media, &bin_ids[media.bin], p.rate)?;
        remaining_xml = remaining_xml
            .checked_sub(item.len() + 19)
            .ok_or("media XML exceeds its construction budget")?;
        media_ids.push(id);
        children[media.bin].push_str(&format!("<Element>{item}</Element>"));
    }
    let mut members = Members::new();
    let mut handles = String::new();
    let mut sequence_ids = Vec::new();
    let mut project = member(profile, PROJECT)?.to_owned();
    set(&mut project, "ProjectName", &xml(&p.name))?;
    set(&mut project, "CurrentTimelineIndex", "0")?;
    container_field(&mut project, "LocableBlobSet", "")?;
    for timeline in &p.timelines {
        let (handle, sequence, container_id, pool, container) = write_timeline(
            timeline,
            p.rate,
            &bin_ids[timeline.bin],
            &p.media,
            &media_ids,
            &timeline_donor,
            container_donor,
        )?;
        remaining_xml = remaining_xml
            .checked_sub(pool.len() + container.len() + 19)
            .ok_or("project XML exceeds its construction budget")?;
        children[timeline.bin].push_str(&format!("<Element>{pool}</Element>"));
        handles.push_str(&format!("<Element>{handle}</Element>"));
        markers::write(&mut project, &sequence, &timeline.markers)?;
        sequence_ids.push(sequence);
        members.insert(format!("SeqContainer/{container_id}.xml"), container);
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
        },
    ))
}

fn verify_stage(p: &PreparedProject, ids: &Identities, path: &Path) -> Result<()> {
    let graph = crate::source::read(path)?;
    if graph.sequences.len() != p.timelines.len()
        || graph.items.len() != p.media.len()
        || graph.bins.len() != p.bins.len()
        || !graph.losses.is_empty()
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
    }
    for (index, (timeline, read)) in p.timelines.iter().zip(&graph.sequences).enumerate() {
        let count = timeline.lanes.iter().map(Vec::len).sum::<usize>();
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
