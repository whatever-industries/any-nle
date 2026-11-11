//! The pinned Group skeleton owns the native two-angle topology and mixer.
//! Generate leaf bindings and edit occurrences from the prepared source facts.
use super::*;

const GROUP_SKELETON: &str = "374b1e3c0645ebf8927fdce6b3cca3b54c4814b4034666d4f7760ec0c2dfeb1a";
const CAMERA_BIN: &str = "MediaPool/Master/000_Camera sources/MpFolder.xml";
const GROUP_ORIGIN: i64 = 86400;

pub fn publish_group(
    prepared: &PreparedGroup,
    skeleton: &Path,
    output: &Path,
) -> Result<Published> {
    check_output(output)?;
    let members = build_group(prepared, load_skeleton(skeleton, GROUP_SKELETON, 7)?)?;
    write_archive(
        members,
        &[&prepared.cameras[0], &prepared.cameras[1]],
        output,
    )
}

fn at_head(s: &mut String, stop: &str, field: &str, value: &str) -> Result<()> {
    let end = s.find(stop).ok_or("missing native Group block")?;
    let mut head = s[..end].to_owned();
    set(&mut head, field, value)?;
    s.replace_range(..end, &head);
    Ok(())
}

fn timemap(frames: i64) -> String {
    let mut data = vec![2];
    data.extend_from_slice(&seconds(frames - 1).to_be_bytes());
    hex::encode(data)
}

fn media_parts(binding: &VerifiedBinding) -> Result<(&str, &str, &str)> {
    Ok((
        utf8_path(&binding.path)?,
        utf8_path(binding.path.parent().ok_or("missing camera directory")?)?,
        binding
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("missing camera file name")?,
    ))
}

fn sequence_member(m: &Members, sequence_id: &str, count: usize) -> Result<String> {
    let target = format!("<Sequence>{sequence_id}</Sequence>");
    let matches: Vec<_> = m
        .iter()
        .filter(|(n, s)| n.starts_with("SeqContainer/") && s.matches(&target).count() == count)
        .collect();
    let [(name, s)] = matches.as_slice() else {
        return Err("unresolved Group sequence container".into());
    };
    if **name != format!("SeqContainer/{}.xml", dbid(s)?) {
        return Err("Group sequence container identity does not match path".into());
    }
    Ok((*name).clone())
}

fn update_group_pool(p: &PreparedGroup, item: &mut String) -> Result<String> {
    let id = dbid(item)?.to_owned();
    at_head(item, "<PTZRPreset>", "Name", &xml(&p.group_name))?;
    set(item, "UniqueMediaPoolItemId", &uid())?;
    let old = block(item, "Sm2Sequence")?;
    let mut sequence = old.clone();
    if tag(&sequence, "Parent")? != id {
        return Err("Group parent identity mismatch".into());
    }
    let start = seconds(GROUP_ORIGIN);
    set(
        &mut sequence,
        "MediaExtents",
        &hex::encode(extent(start, seconds(GROUP_ORIGIN + p.length) - start)),
    )?;
    replace_once(item, &old, &sequence)?;

    // The reference-audio Group exports a single adaptive transport with two
    // virtual channels. The outer edits expose two distinct mono selectors.
    let range = extent(start, seconds(p.length));
    let left = qt(&[("MediaRef", Text(&id))]);
    let right = qt(&[("MediaRef", Text(&id)), ("ChannelIdx", Int(1))]);
    let channels = qt(&[("1", Bytes(&right)), ("0", Bytes(&left))]);
    let track = qt(&[
        ("Type", Uint(258)),
        ("MediaExtents", Bytes(&range)),
        ("ChannelVecBA", Bytes(&channels)),
    ]);
    let routes = qt(&[("0", Bytes(&track))]);
    let fields = Regex::new(r"<FieldsBlob>([0-9a-f]+)</FieldsBlob>")?;
    let field = fields
        .captures(item)
        .and_then(|c| c.get(1))
        .ok_or("missing Group routing blob")?;
    let rebuilt = replace_bytes(&unpack(field.as_str())?, 1, |a| {
        replace_bytes(a, 1, |b| replace_bytes(b, 5, |_| Ok(routes)))
    })?;
    item.replace_range(field.range(), &pack(&rebuilt)?);
    Ok(id)
}

fn write_inner(
    p: &PreparedGroup,
    mut sequence: String,
    old_media: &[String; 2],
    media_ids: &[String; 2],
) -> Result<String> {
    let item_lists = Regex::new(r"(?s)<Items>.*?</Items>")?;
    let spans: Vec<_> = item_lists.find_iter(&sequence).map(|m| m.range()).collect();
    if spans.len() != 4 {
        return Err("expected two video and two reference-audio inner tracks".into());
    }
    for (lane, span) in spans.into_iter().enumerate().rev() {
        // Native Reference Audio transports both channels of the first camera.
        // The observed inner native layout has two audio items with that binding.
        let camera = usize::from(lane == 1);
        let b = &p.cameras[camera];
        let (path, _, file) = media_parts(b)?;
        let mut item = sequence[span.clone()].to_owned();
        if tag(&item, "MediaRef")? != old_media[camera] {
            return Err("unexpected inner camera reference".into());
        }
        set(&mut item, "MediaRef", &media_ids[camera])?;
        set(&mut item, "Name", &xml(file))?;
        set(&mut item, "MediaFilePath", &xml(path))?;
        set(
            &mut item,
            "MediaStartTime",
            &seconds(b.timecode_frames).to_string(),
        )?;
        set(&mut item, "MediaTimemapBA", &timemap(b.frames))?;
        set(
            &mut item,
            "Start",
            &(GROUP_ORIGIN + p.offsets[camera]).to_string(),
        )?;
        set(&mut item, "Duration", &b.frames.to_string())?;
        sequence.replace_range(span, &item);
    }
    Ok(sequence)
}

fn write_outer(p: &PreparedGroup, mut sequence: String, group_id: &str) -> Result<String> {
    let item_lists = Regex::new(r"(?s)<Items>.*?</Items>")?;
    let spans: Vec<_> = item_lists.find_iter(&sequence).map(|m| m.range()).collect();
    if spans.len() != 3 {
        return Err("expected one picture and two mono outer tracks".into());
    }
    let attributes = Regex::new(r#"DbId="[0-9a-f-]{36}""#)?;
    let selector_bytes = timemap(p.length);
    for (lane, span) in spans.into_iter().enumerate().rev() {
        let base = block(&sequence[span.clone()], "Element")?;
        if tag(&base, "MediaRef")? != group_id
            || (lane != 0 && tag(&base, "MediaTrackIdx")? != (lane - 1).to_string())
        {
            return Err("outer Group link or channel selector mismatch".into());
        }
        let mut list = String::from("<Items>");
        for g in &p.lanes[lane] {
            let mut item = attributes
                .replace_all(&base, |_: &regex::Captures| format!("DbId=\"{}\"", uid()))
                .into_owned();
            let label: &[u8] = if g.camera == 1 {
                b"Camera 2"
            } else {
                b"Camera 1"
            };
            let fields = tag(&item, "FieldsBlob")?;
            let changed = replace_bytes(&unpack(&fields)?, 1, |data| {
                replace_bytes(data, 3, |old| {
                    if old != b"Camera 1" {
                        return Err("unexpected default angle selector".into());
                    }
                    Ok(label.to_vec())
                })
            })?;
            replace_once(
                &mut item,
                &format!("<FieldsBlob>{fields}</FieldsBlob>"),
                &format!("<FieldsBlob>{}</FieldsBlob>", pack(&changed)?),
            )?;
            set(&mut item, "Name", &xml(&p.group_name))?;
            set(&mut item, "Start", &(p.origin + g.edit.start).to_string())?;
            set(&mut item, "In", &g.edit.source_in.to_string())?;
            set(&mut item, "Duration", &g.edit.length.to_string())?;
            set(&mut item, "MediaTimemapBA", &selector_bytes)?;
            list.push_str(&item);
        }
        list.push_str("</Items>");
        sequence.replace_range(span, &list);
    }
    Ok(sequence)
}

fn build_group(p: &PreparedGroup, mut m: Members) -> Result<Members> {
    let mut project = m.remove(PROJECT).ok_or("missing Group project")?;
    let mut pool = m.remove(POOL).ok_or("missing Group pool")?;
    let mut cameras = m.remove(CAMERA_BIN).ok_or("missing camera bin")?;
    let old_group = block(&pool, "Sm2MpMulticamClip")?;
    let group_seq = block(&old_group, "Sm2Sequence")?;
    let inner = sequence_member(&m, dbid(&group_seq)?, 4)?;
    let old_timeline = block(&pool, "Sm2MpTimelineClip")?;
    let outer_seq = block(&old_timeline, "Sm2Sequence")?;
    let outer = sequence_member(&m, dbid(&outer_seq)?, 3)?;
    if inner == outer {
        return Err("Group and timeline container alias".into());
    }

    let mut group = old_group.clone();
    let group_id = update_group_pool(p, &mut group)?;
    replace_once(&mut pool, &old_group, &group)?;
    let mut timeline = old_timeline.clone();
    if tag(&block(&project, "TimelineHandleVec")?, "Element")?
        != dbid(&block(&timeline, "Sm2Timeline")?)?
    {
        return Err("unresolved outer timeline registration".into());
    }
    at_head(
        &mut timeline,
        "<PTZRPreset>",
        "Name",
        &xml(&p.sequence_name),
    )?;
    let old_handle = block(&timeline, "Sm2Timeline")?;
    let mut handle = old_handle.clone();
    at_head(&mut handle, "<Sequence>", "Name", &xml(&p.sequence_name))?;
    replace_once(&mut timeline, &old_handle, &handle)?;
    set(
        &mut timeline,
        "MediaExtents",
        &hex::encode(extent(
            seconds(p.origin),
            seconds(p.end) - seconds(p.origin),
        )),
    )?;
    set(&mut timeline, "CurPlayheadPosition", "0")?;
    replace_once(&mut pool, &old_timeline, &timeline)?;
    set(&mut project, "ProjectName", &xml(&p.project_name))?;

    let masters = Regex::new(r"(?s)<Sm2MpVideoClip [^>]+>.*?</Sm2MpVideoClip>")?;
    let originals: [String; 2] = masters
        .find_iter(&cameras)
        .map(|m| m.as_str().to_owned())
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "expected exactly two synthetic camera masters")?;
    let old_media = [
        dbid(&originals[0])?.to_owned(),
        dbid(&originals[1])?.to_owned(),
    ];
    let media_ids = [uid(), uid()];
    for camera in 0..2 {
        let binding = &p.cameras[camera];
        let (_, folder, file) = media_parts(binding)?;
        let old = &originals[camera];
        if tag(old, "MpFolder")? != dbid(&cameras)? {
            return Err("unresolved camera bin link".into());
        }
        let bound = bind_pool_item(old, binding, folder, file, &media_ids[camera])?;
        replace_once(&mut cameras, old, &bound)?;
    }
    let inner_xml = write_inner(
        p,
        m.remove(&inner).ok_or("missing Group container")?,
        &old_media,
        &media_ids,
    )?;
    let outer_xml = write_outer(
        p,
        m.remove(&outer).ok_or("missing timeline container")?,
        &group_id,
    )?;
    let count: usize = p.lanes.iter().map(Vec::len).sum();
    if inner_xml
        .matches(&format!("<MediaRef>{}</MediaRef>", media_ids[0]))
        .count()
        != 3
        || inner_xml
            .matches(&format!("<MediaRef>{}</MediaRef>", media_ids[1]))
            .count()
            != 1
        || outer_xml
            .matches(&format!("<MediaRef>{group_id}</MediaRef>"))
            .count()
            != count
    {
        return Err("generated Group references do not resolve".into());
    }
    m.insert(PROJECT.into(), project);
    m.insert(POOL.into(), pool);
    m.insert(CAMERA_BIN.into(), cameras);
    m.insert(inner, inner_xml);
    m.insert(outer, outer_xml);
    if m.values().map(|s| s.len() as u64).sum::<u64>() > MAX_XML {
        return Err("generated Group exceeds the XML budget".into());
    }
    Ok(m)
}

pub(super) struct TurnoverGroup {
    pub(super) id: String,
    pub(super) pool: String,
    pub(super) timeline: String,
    pub(super) handle: String,
    pub(super) sequence: String,
    pub(super) containers: Members,
}

pub(super) fn build_turnover_group(
    p: &PreparedGroup,
    skeleton: &Path,
    group_bin: &str,
    timeline_bin: &str,
    media_ids: [&str; 2],
) -> Result<TurnoverGroup> {
    let mut m = load_skeleton(skeleton, GROUP_SKELETON, 7)?;
    let pool = m.remove(POOL).ok_or("missing Group pool")?;
    let cameras = m.remove(CAMERA_BIN).ok_or("missing camera bin")?;
    let mut group = block(&pool, "Sm2MpMulticamClip")?;
    let mut timeline = block(&pool, "Sm2MpTimelineClip")?;
    let group_seq = block(&group, "Sm2Sequence")?;
    let timeline_seq = block(&timeline, "Sm2Sequence")?;
    let sequence = dbid(&timeline_seq)?.to_owned();
    let inner = sequence_member(&m, dbid(&group_seq)?, 4)?;
    let outer = sequence_member(&m, &sequence, 3)?;
    if inner == outer {
        return Err("Group and timeline container alias".into());
    }
    let id = update_group_pool(p, &mut group)?;
    set(&mut group, "MpFolder", group_bin)?;
    set(&mut timeline, "MpFolder", timeline_bin)?;
    at_head(
        &mut timeline,
        "<PTZRPreset>",
        "Name",
        &xml(&p.sequence_name),
    )?;
    let old_handle = block(&timeline, "Sm2Timeline")?;
    let mut handle = old_handle.clone();
    at_head(&mut handle, "<Sequence>", "Name", &xml(&p.sequence_name))?;
    let handle_id = dbid(&handle)?.to_owned();
    replace_once(&mut timeline, &old_handle, &handle)?;
    set(
        &mut timeline,
        "MediaExtents",
        &hex::encode(extent(
            seconds(p.origin),
            seconds(p.end) - seconds(p.origin),
        )),
    )?;
    set(&mut timeline, "CurPlayheadPosition", "0")?;
    let pattern = Regex::new(r"(?s)<Sm2MpVideoClip [^>]+>.*?</Sm2MpVideoClip>")?;
    let old_media: [String; 2] = pattern
        .find_iter(&cameras)
        .map(|item| dbid(item.as_str()).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .map_err(|_| "expected two synthetic camera masters")?;
    let inner_xml = write_inner(
        p,
        m.remove(&inner).ok_or("missing Group container")?,
        &old_media,
        &media_ids.map(str::to_owned),
    )?;
    let outer_xml = write_outer(
        p,
        m.remove(&outer).ok_or("missing timeline container")?,
        &id,
    )?;
    Ok(TurnoverGroup {
        id,
        pool: group,
        timeline,
        handle: handle_id,
        sequence,
        containers: Members::from([(inner, inner_xml), (outer, outer_xml)]),
    })
}
