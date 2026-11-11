use crate::{
    binary::{pack, pb_bytes, pb_int, qt, replace_bytes, unpack, Qt::*},
    prepare::utf8_path,
    seconds,
    source::mixer::Mixer,
    timecode, PreparedGroup, PreparedResolve, Result, VerifiedBinding,
};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{Cursor, Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

mod group;
mod linked;
mod markers;
pub use group::publish_group;
pub use linked::publish_linked;

const MAX_ARCHIVE: u64 = 1 << 20;
const MAX_XML: u64 = 8 << 20;
const SKELETON_SHA256: &str = "957171ef74e1b1b36cb8824983f0f73c9185785a8b424f0d8f5dfd135a1725de";
const PROJECT: &str = "project.xml";
const POOL: &str = "MediaPool/Master/MpFolder.xml";
const SOURCE: &str = "MediaPool/Master/000_Source/MpFolder.xml";
const FRAME_RATE: &str = "872211b5dcf937400000000000000000";
const REC709_RADIOMETRY: &str = "800878100228107002789e01800166880132980100d80100";

type Members = BTreeMap<String, String>;

pub struct Published {
    pub cleanup_warning: Option<String>,
}

pub fn publish(prepared: &PreparedResolve, skeleton: &Path, output: &Path) -> Result<Published> {
    publish_checked(prepared, skeleton, output, || Ok(()))
}

pub(crate) fn publish_checked(
    prepared: &PreparedResolve,
    skeleton: &Path,
    output: &Path,
    check_source: impl FnOnce() -> Result<()>,
) -> Result<Published> {
    check_output(output)?;
    let members = build(prepared, load_skeleton(skeleton, SKELETON_SHA256, 6)?)?;
    write_checked_archive(members, output, |_| {
        for binding in &prepared.bindings {
            binding.recheck()?;
        }
        check_source()
    })
}

pub(crate) fn check_output(output: &Path) -> Result<()> {
    if output.extension().and_then(|s| s.to_str()) != Some("drp") {
        return Err("output must end in .drp".into());
    }
    match std::fs::symlink_metadata(output) {
        Ok(_) => return Err("project output already exists".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn write_archive(
    members: Members,
    bindings: &[&VerifiedBinding],
    output: &Path,
) -> Result<Published> {
    write_checked_archive(members, output, |_| {
        for binding in bindings {
            binding.recheck()?;
        }
        Ok(())
    })
}

fn write_checked_archive(
    members: Members,
    output: &Path,
    check: impl FnOnce(&Path) -> Result<()>,
) -> Result<Published> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut stage = NamedTempFile::new_in(parent)?;
    {
        let mut zip = ZipWriter::new(stage.as_file_mut());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, xml) in members {
            zip.start_file(name, options)?;
            zip.write_all(xml.as_bytes())?;
        }
        zip.finish()?;
    }
    stage.as_file().sync_all()?;
    check(stage.path())?;
    // Same-directory hard links publish a finished project and refuse a late incumbent.
    std::fs::hard_link(stage.path(), output)?;
    let stage_name = stage.path().display().to_string();
    let cleanup_warning = stage.close().err().map(|error| {
        format!(
            "published {}, retained stage {stage_name}: {error}",
            output.display()
        )
    });
    Ok(Published { cleanup_warning })
}

fn load_skeleton(path: &Path, digest: &str, members: usize) -> Result<Members> {
    if !std::fs::metadata(path)?.is_file() {
        return Err("skeleton must be a regular file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_ARCHIVE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ARCHIVE || hex::encode(Sha256::digest(&bytes)) != digest {
        return Err("expected the pinned Resolve Studio 21.2.0.0006 synthetic skeleton".into());
    }
    read_members(&bytes, members)
}

fn read_members(bytes: &[u8], count: usize) -> Result<Members> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() != count {
        return Err("unexpected skeleton member count".into());
    }
    let mut members = Members::new();
    let mut remaining = MAX_XML;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name().to_owned();
        if !entry.is_file()
            || entry.enclosed_name().is_none()
            || name.contains('\\')
            || !name.ends_with(".xml")
            || entry.size() > remaining
        {
            return Err("unsafe or over-budget skeleton member".into());
        }
        let mut xml = String::new();
        let size = entry.size();
        entry.take(size + 1).read_to_string(&mut xml)?;
        if xml.len() as u64 != size
            || !xml.contains("DbAppVer=\"21.2.0.0006\" DbPrjVer=\"17\"")
            || xml.contains("<!DOCTYPE")
            || members.insert(name, xml).is_some()
        {
            return Err("unexpected or ambiguous skeleton XML".into());
        }
        remaining -= size;
    }
    Ok(members)
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn one<'a>(re: &Regex, s: &'a str) -> Result<regex::Match<'a>> {
    let mut found = re.find_iter(s);
    let value = found
        .next()
        .ok_or_else(|| format!("required skeleton field absent for {re}"))?;
    if found.next().is_some() {
        return Err(format!("ambiguous skeleton field matching {re}").into());
    }
    Ok(value)
}

fn tag(s: &str, name: &str) -> Result<String> {
    let re = Regex::new(&format!(r"(?s)<{name}>[^<]*</{name}>"))?;
    let found = one(&re, s)?.as_str();
    Ok(found[name.len() + 2..found.len() - name.len() - 3].to_owned())
}

fn set(s: &mut String, name: &str, value: &str) -> Result<()> {
    let re = Regex::new(&format!(r"(?s)<{name}(?:/>|>[^<]*</{name}>)"))?;
    let found = one(&re, s)?;
    s.replace_range(found.range(), &format!("<{name}>{value}</{name}>"));
    Ok(())
}

fn block(s: &str, name: &str) -> Result<String> {
    let re = Regex::new(&format!(r"(?s)<{name}(?: [^>]+)?>.*?</{name}>"))?;
    Ok(one(&re, s)?.as_str().to_owned())
}

fn replace_once(s: &mut String, old: &str, new: &str) -> Result<()> {
    let mut hits = s.match_indices(old);
    let (index, _) = hits.next().ok_or("required skeleton block absent")?;
    if hits.next().is_some() {
        return Err("ambiguous skeleton block".into());
    }
    s.replace_range(index..index + old.len(), new);
    Ok(())
}

fn dbid(s: &str) -> Result<&str> {
    let (_, tail) = s.split_once("DbId=\"").ok_or("missing object identity")?;
    let (id, _) = tail.split_once('"').ok_or("invalid object identity")?;
    Uuid::parse_str(id)?;
    Ok(id)
}

fn uid() -> String {
    Uuid::new_v4().to_string()
}

fn extent(start: f64, duration: f64) -> [u8; 16] {
    let mut data = [0; 16];
    data[..8].copy_from_slice(&start.to_le_bytes());
    data[8..].copy_from_slice(&duration.to_le_bytes());
    data
}

fn metadata(package: &[u8; 32], master: &[u8; 32]) -> Vec<u8> {
    qt(&[
        ("tapeMobId", Bytes(&[0; 32])),
        ("sourceMobId", Bytes(package)),
        ("masterMobId", Bytes(master)),
    ])
}

fn clip_cache(
    modified_micros: u64,
    folder: &str,
    file: &str,
    video: bool,
    atom: bool,
) -> Result<String> {
    let mut body = Vec::new();
    pb_bytes(1, folder.as_bytes(), &mut body);
    pb_bytes(2, file.as_bytes(), &mut body);
    pb_bytes(
        5,
        if video { b"dnxhd_lb" } else { b"Linear PCM" },
        &mut body,
    );
    if video {
        pb_bytes(6, file.as_bytes(), &mut body);
        pb_bytes(7, uid().as_bytes(), &mut body);
    }
    pb_int(13, modified_micros, &mut body);
    pb_int(15, 8, &mut body);
    pb_int(16, 100, &mut body);
    pb_int(18, if atom { 16388 } else { 16396 }, &mut body);
    if !video && !atom {
        pb_int(19, 4, &mut body);
    }
    pack(&body)
}

fn bind_pool_item(
    old: &str,
    binding: &VerifiedBinding,
    folder: &str,
    file: &str,
    new_id: &str,
) -> Result<String> {
    let mut item = old.to_owned();
    let old_id = dbid(&item)?.to_owned();
    replace_once(&mut item, &old_id, new_id)?;
    let ptzr = item.find("<PTZRPreset>").ok_or("missing media preset")?;
    let mut head = item[..ptzr].to_owned();
    set(&mut head, "Name", &xml(file))?;
    item.replace_range(..ptzr, &head);
    set(&mut item, "UniqueMediaPoolItemId", &uid())?;

    let start = seconds(binding.timecode_frames);
    let duration = seconds(binding.frames);
    let media_extent = extent(start, duration);
    let atom = binding.atom.as_ref();
    let master = atom.map_or(&[0; 32], |files| &files.master_package);
    let mxf = metadata(&binding.source_package, master);
    let rate = hex::decode(FRAME_RATE)?;
    let video_old = block(&item, "BtVideoInfo")?;
    let mut video = video_old.clone();
    let v_id = dbid(&video)?.to_owned();
    replace_once(&mut video, &v_id, &uid())?;
    set(
        &mut video,
        "Clip",
        &clip_cache(binding.modified_micros, folder, file, true, atom.is_some())?,
    )?;
    let time_id = uid();
    let tc = timecode(binding.timecode_frames);
    let mut fields = vec![("UniqueId", Text(&time_id))];
    if binding.timecode_frames != 0 {
        fields.push(("Timecode", Text(&tc)));
    }
    fields.extend([
        ("StartFrame", Int(0)),
        ("NumFrames", Int(i32::try_from(binding.frames)?)),
        ("FrameRate", Bytes(&rate)),
        ("DbType", Text("BtVideoTime")),
    ]);
    set(&mut video, "Time", &hex::encode(qt(&fields)))?;
    if binding.tagged_rec709 {
        set(&mut video, "Radiometry", REC709_RADIOMETRY)?;
    }
    let metadata_id = uid();
    let mut fields = vec![
        ("UniqueId", Text(&metadata_id)),
        ("MXFMetadata", Bytes(&mxf)),
        ("DbType", Text("BtVideoMetadata")),
    ];
    if let Some(atom) = atom.filter(|a| !a.reel.is_empty()) {
        fields.push(("ActualReelNumber", Text(&atom.reel)));
    }
    set(&mut video, "VideoMetadata", &hex::encode(qt(&fields)))?;
    replace_once(&mut item, &video_old, &video)?;

    let audio_wrapper = block(&item, "EmbeddedAudioVec")?;
    let audio_base = block(&audio_wrapper, "BtAudioInfo")?;
    let mut routes = Vec::new();
    if let Some(atom) = atom {
        let mut wrapper = String::from("<EmbeddedAudioVec>");
        for sibling in &atom.files[1..] {
            let audio_id = uid();
            let (folder, file) = media_path_parts(&sibling.path)?;
            let cache = clip_cache(sibling.modified_micros, folder, file, false, true)?;
            let mxf = metadata(&sibling.package, master);
            let track = mono_track(start, binding.frames, 0, &mxf);
            let tracks = qt(&[("0", Bytes(&track))]);
            wrapper.push_str("<Element>");
            wrapper.push_str(&bind_audio(&audio_base, &audio_id, &cache, &tracks)?);
            wrapper.push_str("</Element>");
            routes.push(mono_route(&media_extent, &audio_id, 0));
        }
        wrapper.push_str("</EmbeddedAudioVec>");
        replace_once(&mut item, &audio_wrapper, &wrapper)?;
    } else {
        let audio_id = uid();
        let tracks = [0, 1].map(|index| mono_track(start, binding.frames, index, &mxf));
        let tracks = qt(&[("1", Bytes(&tracks[1])), ("0", Bytes(&tracks[0]))]);
        let cache = clip_cache(binding.modified_micros, folder, file, false, false)?;
        let audio = bind_audio(&audio_base, &audio_id, &cache, &tracks)?;
        replace_once(&mut item, &audio_base, &audio)?;
        routes.extend([0, 1].map(|index| mono_route(&media_extent, &audio_id, index)));
    }

    let fields = Regex::new(r"<FieldsBlob>([0-9a-f]+)</FieldsBlob>")?;
    let field = fields.captures(&item).ok_or("missing pool FieldsBlob")?;
    let payload = field.get(1).ok_or("missing pool blob payload")?;
    let routing = qt(&[("1", Bytes(&routes[1])), ("0", Bytes(&routes[0]))]);
    let rebuilt = replace_bytes(&unpack(payload.as_str())?, 1, |a| {
        replace_bytes(a, 1, |b| replace_bytes(b, 5, |_| Ok(routing)))
    })?;
    item.replace_range(payload.range(), &pack(&rebuilt)?);
    Ok(item)
}

fn media_path_parts(path: &Path) -> Result<(&str, &str)> {
    let folder = utf8_path(path.parent().ok_or("missing media parent")?)?;
    let file = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("missing media name")?;
    Ok((folder, file))
}

fn bind_audio(base: &str, id: &str, cache: &str, tracks: &[u8]) -> Result<String> {
    let mut audio = base.to_owned();
    replace_once(&mut audio, dbid(base)?, id)?;
    set(&mut audio, "Clip", cache)?;
    set(&mut audio, "TracksBA", &hex::encode(tracks))?;
    Ok(audio)
}

fn mono_track(start: f64, frames: i64, index: i32, mxf: &[u8]) -> Vec<u8> {
    qt(&[
        ("UniqueId", Text(&uid())),
        ("StartTime", Double(start)),
        ("SampleRate", Uint(48000)),
        ("NumChannels", Int(1)),
        ("MXFMetadata", Bytes(mxf)),
        ("IdxTrack", Int(index)),
        ("Duration", Long(frames * 2002)),
        ("DbType", Text("BtAudioTrack")),
        ("CodecName", Text("Linear PCM")),
        ("BitDepth", Uint(2)),
    ])
}

fn mono_route(media_extent: &[u8], audio_id: &str, track: i32) -> Vec<u8> {
    let mut fields = Vec::new();
    if track != 0 {
        fields.push(("TrackIdx", Int(track)));
    }
    fields.push(("MediaRef", Text(audio_id)));
    let channel = qt(&fields);
    let channels = qt(&[("0", Bytes(&channel))]);
    qt(&[
        ("MediaExtents", Bytes(media_extent)),
        ("CodecName", Text("Linear PCM")),
        ("ChannelVecBA", Bytes(&channels)),
        ("BitDepth", Uint(2)),
    ])
}

struct PoolLink<'a> {
    id: String,
    path: &'a str,
    file: &'a str,
    start: String,
    timemap: String,
}

fn write_edits(sequence: &mut String, p: &PreparedResolve, links: &[PoolLink<'_>]) -> Result<()> {
    let item_lists = Regex::new(r"(?s)<Items>.*?</Items>")?;
    let spans: Vec<_> = item_lists.find_iter(sequence).map(|m| m.range()).collect();
    if spans.len() != 3 {
        return Err("expected one video and two mono skeleton tracks".into());
    }
    let attributes = Regex::new(r#"DbId="[0-9a-f-]{36}""#)?;
    for (lane, span) in spans.into_iter().enumerate().rev() {
        let source = &sequence[span.clone()];
        let base = block(source, "Element")?;
        let typ = if lane == 0 {
            "Sm2TiVideoClip"
        } else {
            "Sm2TiAudioClip"
        };
        let clip = block(&base, typ)?;
        if lane != 0 && tag(&clip, "MediaTrackIdx")? != (lane - 1).to_string() {
            return Err("unexpected synthetic lane selector".into());
        }
        let mut list = String::from("<Items>");
        for edit in &p.lanes[lane] {
            let mut item = attributes
                .replace_all(&base, |_: &regex::Captures| format!("DbId=\"{}\"", uid()))
                .into_owned();
            let link = &links[edit.binding];
            set(&mut item, "Name", &xml(link.file))?;
            set(&mut item, "MediaFilePath", &xml(link.path))?;
            set(&mut item, "MediaRef", &link.id)?;
            set(&mut item, "MediaStartTime", &link.start)?;
            set(&mut item, "MediaTimemapBA", &link.timemap)?;
            set(&mut item, "Start", &(p.origin + edit.start).to_string())?;
            set(&mut item, "In", &edit.source_in.to_string())?;
            set(&mut item, "Duration", &edit.length.to_string())?;
            if let Some(envelope) = edit.envelope {
                set(&mut item, "EffectFiltersBA", &envelope.encode()?)?;
            }
            list.push_str(&item);
        }
        list.push_str("</Items>");
        sequence.replace_range(span, &list);
    }
    if p.track_mutes != [false; 2] {
        let track_headers = Regex::new(
            r"<Sm2TiTrack [^>]+>\s*<FieldsBlob>[0-9a-f]+</FieldsBlob>\s*<Type>1</Type>\s*<SubType>1</SubType>\s*<Flags>(0)</Flags>",
        )?;
        let flags: Vec<_> = track_headers
            .captures_iter(sequence)
            .map(|m| m.get(1).expect("track flags capture").range())
            .collect();
        if flags.len() != 2 {
            return Err("expected two neutral mono track headers for mute".into());
        }
        for (flag, mute) in flags.into_iter().zip(p.track_mutes) {
            if mute {
                sequence.replace_range(flag, "2");
            }
        }
    }
    Ok(())
}

fn build(p: &PreparedResolve, mut m: Members) -> Result<Members> {
    let seqs: Vec<_> = m
        .keys()
        .filter(|name| name.starts_with("SeqContainer/"))
        .cloned()
        .collect();
    let [seq] = seqs.as_slice() else {
        return Err("expected exactly one native sequence container".into());
    };
    let mut project = m.remove(PROJECT).ok_or("missing project member")?;
    let mut pool = m.remove(POOL).ok_or("missing master pool")?;
    let mut source = m.remove(SOURCE).ok_or("missing source pool")?;
    let mut timeline = m.remove(seq).ok_or("missing timeline")?;
    if format!("SeqContainer/{}.xml", dbid(&timeline)?) != *seq {
        return Err("sequence container identity does not match archive path".into());
    }
    let timeline_shell = block(&pool, "Sm2Timeline")?;
    let old_timeline_id = dbid(&timeline_shell)?.to_owned();
    let timeline_id = uid();
    let handles = block(&project, "TimelineHandleVec")?;
    if tag(&handles, "Element")? != old_timeline_id {
        return Err("timeline registration does not identify the timeline".into());
    }
    let seq_shell = block(&timeline_shell, "Sm2Sequence")?;
    let seq_id = dbid(&seq_shell)?;
    if timeline
        .matches(&format!("<Sequence>{seq_id}</Sequence>"))
        .count()
        != 3
        || tag(&seq_shell, "Parent")? != old_timeline_id
    {
        return Err("unresolved timeline, sequence or track references".into());
    }
    let parent_id = dbid(&pool)?;
    let media_item = block(&source, "Sm2MpVideoClip")?;
    if tag(
        &source[..source.find(&media_item).ok_or("missing media item")?],
        "MpFolder",
    )? != parent_id
        || tag(&media_item, "MpFolder")? != dbid(&source)?
    {
        return Err("unresolved source bin or media references".into());
    }
    let old_media_vec = block(&source, "MediaVec")?;
    let mut media_vec = String::from("<MediaVec>");
    let mut links = Vec::with_capacity(p.bindings.len());
    for binding in &p.bindings {
        let id = uid();
        let path = utf8_path(&binding.path)?;
        let (folder, file) = media_path_parts(&binding.path)?;
        let bound = bind_pool_item(&media_item, binding, folder, file, &id)?;
        if dbid(&bound)? != id {
            return Err("generated Resolve pool identity mismatch".into());
        }
        media_vec.push_str("<Element>");
        media_vec.push_str(&bound);
        media_vec.push_str("</Element>");
        let mut timemap = vec![2];
        timemap.extend_from_slice(&seconds(binding.frames - 1).to_be_bytes());
        links.push(PoolLink {
            id,
            path,
            file,
            start: seconds(binding.timecode_frames).to_string(),
            timemap: hex::encode(timemap),
        });
    }
    media_vec.push_str("</MediaVec>");
    replace_once(&mut source, &old_media_vec, &media_vec)?;
    write_edits(&mut timeline, p, &links)?;

    set(&mut project, "ProjectName", &xml(&p.project_name))?;
    markers::write(&mut project, seq_id, &p.markers)?;
    if p.track_faders != [0, 0] || p.main_bus_fader != 0 || p.track_mutes != [false; 2] {
        let fields = Regex::new(r"^<Sm2Sequence [^>]+>\s*<FieldsBlob>([0-9a-f]+)</FieldsBlob>")?;
        let payload = fields
            .captures(&seq_shell)
            .and_then(|m| m.get(1))
            .ok_or("missing sequence Fairlight fields")?;
        let mixer = Mixer::decode(payload.as_str())?.ok_or("missing qualified Fairlight mixer")?;
        let mut patched = seq_shell.clone();
        patched.replace_range(
            payload.range(),
            &mixer.write(p.track_faders, p.main_bus_fader, p.track_mutes)?,
        );
        replace_once(&mut pool, &seq_shell, &patched)?;
    }
    pool = pool.replace(
        "<Name>Synthetic core skeleton</Name>",
        &format!("<Name>{}</Name>", xml(&p.sequence_name)),
    );
    project = project.replace(&old_timeline_id, &timeline_id);
    pool = pool.replace(&old_timeline_id, &timeline_id);
    set(
        &mut pool,
        "MediaExtents",
        &hex::encode(extent(
            seconds(p.origin),
            seconds(p.end) - seconds(p.origin),
        )),
    )?;
    set(&mut pool, "CurPlayheadPosition", "0")?;
    // Check generated placement IDs and all file references before writing the ZIP.
    let edit_ids = Regex::new(r#"<Sm2Ti(?:Video|Audio)Clip DbId="([0-9a-f-]{36})""#)?;
    let mut distinct = HashSet::new();
    let count: usize = p.lanes.iter().map(Vec::len).sum();
    for found in edit_ids.captures_iter(&timeline) {
        if !distinct.insert(found[1].to_owned()) {
            return Err("duplicate generated edit identity".into());
        }
    }
    let mut uses = vec![0; links.len()];
    for edit in p.lanes.iter().flatten() {
        uses[edit.binding] += 1;
    }
    for (link, uses) in links.iter().zip(uses) {
        if uses == 0
            || timeline
                .matches(&format!("<MediaRef>{}</MediaRef>", link.id))
                .count()
                != uses
        {
            return Err("generated Resolve media reference mismatch".into());
        }
    }
    if distinct.len() != count
        || tag(&block(&project, "TimelineHandleVec")?, "Element")? != timeline_id
        || dbid(&block(&pool, "Sm2Timeline")?)? != timeline_id
    {
        return Err("generated archive reference mismatch".into());
    }
    m.insert(PROJECT.into(), project);
    m.insert(POOL.into(), pool);
    m.insert(SOURCE.into(), source);
    m.insert(seq.clone(), timeline);
    if m.values().map(|text| text.len() as u64).sum::<u64>() > MAX_XML {
        return Err("generated archive exceeds the XML budget".into());
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_archive_is_refused() {
        assert!(read_members(b"PK\x03\x04broken", 6).is_err());
    }

    #[test]
    fn duplicate_field_does_not_patch() {
        let mut input = String::from("<In>1</In><In>2</In>");
        assert!(set(&mut input, "In", "3").is_err());
        assert_eq!(input, "<In>1</In><In>2</In>");
    }
}
