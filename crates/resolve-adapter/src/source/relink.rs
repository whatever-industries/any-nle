//! Locate physical media through the saved pool identity and its timeline uses.
use super::{
    cache, containers,
    encoding::Blobs,
    expect, folders, same, tracks,
    xml::{self, Node},
};
use crate::{
    binary::{pack, replace_bytes},
    relink::{original_path, remember, Conform, Destinations, Identity, Paths},
    Result,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{Cursor, Write},
    ops::Range,
    path::Path,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

struct Patch {
    span: Range<usize>,
    original: String,
    cache: Option<Vec<u8>>,
}

pub(crate) struct Document {
    members: BTreeMap<String, String>,
    patches: BTreeMap<String, Vec<Patch>>,
    paths: Paths,
    conform: Conform,
}

impl Document {
    pub(crate) fn read(bytes: &[u8]) -> Result<Self> {
        let mut archive = xml::checked_zip(Cursor::new(bytes))?;
        let mut seen = HashSet::new();
        let mut remaining = xml::MAX_EXPANDED;
        let mut total_nodes = 0;
        let mut members = BTreeMap::new();
        let mut nodes = BTreeMap::new();
        let mut names = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            xml::check_member(&entry, &mut seen, remaining)?;
            let name = entry.name().to_owned();
            if name != "project.xml"
                && name != "Gallery.xml"
                && !(name.starts_with("MediaPool/") && name.ends_with("/MpFolder.xml"))
                && !(name.starts_with("SeqContainer/") && name.ends_with(".xml"))
            {
                return Err(format!("unsupported Resolve relink member {name}").into());
            }
            let text = xml::read_member(&mut entry, &mut remaining)?;
            xml::check_version(&text)?;
            let node = xml::parse(&text, &mut total_nodes)?;
            if names.insert(node.id()?.to_owned(), name.clone()).is_some() {
                return Err("duplicate Resolve archive root identity".into());
            }
            nodes.insert(name.clone(), node);
            members.insert(name, text);
        }

        let project = nodes
            .get("project.xml")
            .ok_or("missing Resolve project.xml")?;
        expect(project, "SM_Project")?;
        same(project.get("ProjectVersion")?, "17", "project version")?;
        let mut blobs = Blobs::default();
        let mut paths = Paths::new();
        let mut conform = Conform::new();
        let mut masters = HashMap::new();
        let mut timelines = HashMap::new();
        let mut patches = BTreeMap::new();
        let mut pool_ids = HashSet::new();

        for folder in folders(&nodes, project, &mut blobs)? {
            let name = names.get(folder.id()?).ok_or("missing Resolve bin XML")?;
            for entry in folder.one("MediaVec")?.elements()? {
                let clip = entry.entry()?;
                same(clip.get("MpFolder")?, folder.id()?, "pool item folder")?;
                if !pool_ids.insert(clip.id()?) {
                    return Err("duplicate Resolve pool item identity".into());
                }
                match clip.name.as_str() {
                    "Sm2MpVideoClip" => {
                        let master =
                            pool_path(clip, &mut blobs, patches.entry(name.clone()).or_default())?;
                        let video = clip.one("Video")?.entry()?;
                        remember(&mut conform, &master, identity(video, &mut blobs));
                        paths.insert(master.clone());
                        masters.insert(clip.id()?, master);
                    }
                    "Sm2MpTimelineClip" => {
                        let handle = clip.one("TimelineSharedHandle")?.entry()?;
                        expect(handle, "Sm2Timeline")?;
                        same(
                            handle.get("MpTimelineClip")?,
                            clip.id()?,
                            "timeline pool back-reference",
                        )?;
                        let sequence = handle.one("Sequence")?.entry()?;
                        expect(sequence, "Sm2Sequence")?;
                        same(sequence.get("Parent")?, handle.id()?, "sequence parent")?;
                        if timelines.insert(handle.id()?, sequence.id()?).is_some() {
                            return Err("duplicate Resolve timeline identity".into());
                        }
                    }
                    _ => {
                        return Err(
                            format!("unsupported Resolve relink pool item {}", clip.name).into(),
                        )
                    }
                }
            }
        }
        let mut containers = containers(&nodes)?;
        for registered in project.one("TimelineHandleVec")?.elements()? {
            let sequence = timelines
                .remove(registered.scalar()?)
                .ok_or("missing or duplicate Resolve timeline registration")?;
            let container = containers
                .remove(sequence)
                .ok_or("missing Resolve sequence container")?;
            let name = names
                .get(container.id()?)
                .ok_or("missing Resolve sequence XML")?;
            timeline_paths(
                container,
                &masters,
                patches.entry(name.clone()).or_default(),
            )?;
        }
        if !timelines.is_empty() || !containers.is_empty() {
            return Err("unregistered Resolve timelines or sequence containers".into());
        }
        Ok(Self {
            members,
            patches,
            paths,
            conform,
        })
    }

    pub(crate) fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn conform(&self) -> &Conform {
        &self.conform
    }

    pub(crate) fn write(mut self, moves: &Destinations, file: &mut File) -> Result<()> {
        let mut remaining = xml::MAX_EXPANDED as usize;
        for (name, member) in &mut self.members {
            if let Some(patches) = self.patches.remove(name) {
                *member = patch_xml(member, patches, moves, remaining)?;
            }
            remaining = remaining
                .checked_sub(member.len())
                .ok_or("rewritten Resolve archive exceeds expanded size limit")?;
        }
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, xml) in self.members {
            zip.start_file(name, options)?;
            zip.write_all(xml.as_bytes())?;
        }
        zip.finish()?;
        Ok(())
    }
}

fn identity(video: &Node, blobs: &mut Blobs) -> Option<Identity> {
    let time = blobs.qt(video.get("Time").ok()?).ok()?;
    let metadata = blobs.qt(video.get("VideoMetadata").ok()?).ok()?;
    let reel = metadata.text("ActualReelNumber").ok()??;
    let timecode = time.text("Timecode").ok()??;
    if reel.is_empty() || timecode.is_empty() {
        return None;
    }
    Some(Identity {
        reel: reel.into(),
        timecode: timecode.into(),
    })
}

fn pool_path(clip: &Node, blobs: &mut Blobs, patches: &mut Vec<Patch>) -> Result<String> {
    let video = clip.one("Video")?.entry()?;
    expect(video, "BtVideoInfo")?;
    same(video.get("Type")?, "STANDARD_CLIP", "video media type")?;
    if let Some(proxy) = video.maybe("Proxy")?.filter(|proxy| !proxy.is_empty()) {
        let proxy = blobs.qt(proxy)?;
        if proxy
            .keys()
            .any(|name| !matches!(name, "UniqueId" | "DbType" | "DataManagerID"))
            || proxy.text("DbType")? != Some("BtVideoProxy")
        {
            return Err("Resolve attached proxies are outside the verified relink profile".into());
        }
    }
    let (original, _) = cache(video, blobs)?;
    original_path(&original)?;
    add_cache(video, &original, blobs, patches)?;
    if let Some(audio) = clip.optional("EmbeddedAudioVec")? {
        for entry in audio.elements()? {
            let audio = entry.entry()?;
            expect(audio, "BtAudioInfo")?;
            same(&cache(audio, blobs)?.0, &original, "embedded audio locator")?;
            add_cache(audio, &original, blobs, patches)?;
        }
    }
    if clip.optional("Audio")?.is_some() {
        return Err("Resolve separately bound audio is outside the verified relink profile".into());
    }
    Ok(original)
}

fn add_cache(
    node: &Node,
    original: &str,
    blobs: &mut Blobs,
    patches: &mut Vec<Patch>,
) -> Result<()> {
    let clip = node.one("Clip")?;
    patches.push(Patch {
        span: clip.content.clone(),
        original: original.into(),
        cache: Some(blobs.envelope(clip.scalar()?)?),
    });
    Ok(())
}

fn timeline_paths(
    container: &Node,
    masters: &HashMap<&str, String>,
    patches: &mut Vec<Patch>,
) -> Result<()> {
    for (kind, track) in tracks(container)? {
        for entry in track.one("Items")?.elements()? {
            let item = entry.entry()?;
            let tag = match kind {
                graph::StreamKind::Video => "Sm2TiVideoClip",
                graph::StreamKind::Audio => "Sm2TiAudioClip",
            };
            expect(item, tag)?;
            let original = masters
                .get(item.get("MediaRef")?)
                .ok_or("unknown Resolve timeline media identity")?;
            let path = item.one("MediaFilePath")?;
            same(path.scalar()?, original, "timeline media locator")?;
            patches.push(Patch {
                span: path.content.clone(),
                original: original.clone(),
                cache: None,
            });
        }
    }
    Ok(())
}

fn patch_xml(
    xml: &str,
    mut patches: Vec<Patch>,
    moves: &Destinations,
    limit: usize,
) -> Result<String> {
    patches.sort_by_key(|patch| patch.span.start);
    let mut out = String::new();
    let mut prior = 0;
    let append = |out: &mut String, text: &str| -> Result<()> {
        if text.len() > limit.saturating_sub(out.len()) {
            return Err("rewritten Resolve archive exceeds expanded size limit".into());
        }
        out.push_str(text);
        Ok(())
    };
    for patch in patches {
        if patch.span.is_empty()
            || patch.span.start < prior
            || xml.get(patch.span.clone()).is_none()
        {
            return Err("invalid or overlapping Resolve locator span".into());
        }
        append(&mut out, &xml[prior..patch.span.start])?;
        prior = patch.span.end;
        let destination = moves
            .get(&patch.original)
            .ok_or("unmapped Resolve original")?;
        if *destination == patch.original {
            append(&mut out, &xml[patch.span])?;
            continue;
        }
        let text = match patch.cache {
            Some(cache) => {
                let folder = Path::new(destination)
                    .parent()
                    .and_then(Path::to_str)
                    .ok_or("missing Resolve destination directory")?;
                let body = replace_bytes(&cache, 1, |old| {
                    let mut new = folder.as_bytes().to_vec();
                    if old.ends_with(b"/") && !new.ends_with(b"/") {
                        new.push(b'/');
                    }
                    Ok(new)
                })?;
                pack(&body)?
            }
            None => quick_xml::escape::escape(destination).into_owned(),
        };
        append(&mut out, &text)?;
    }
    append(&mut out, &xml[prior..])?;
    Ok(out)
}
