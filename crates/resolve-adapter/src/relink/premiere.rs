use super::{original_path, remember, Conform, Destinations, Identity, Paths};
use crate::Result;
use prproj::xml::{
    mutations::{append_uniform_child, remove_child},
    GzipFraming, NodeId, PremiereDocument, Tree,
};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_XML: u64 = 64 << 20;

struct Locator {
    original: String,
    media: NodeId,
    absolute: [NodeId; 2],
    relative: Vec<NodeId>,
}

pub(super) struct Document {
    native: PremiereDocument,
    locators: Vec<Locator>,
    paths: Paths,
    conform: Conform,
}

impl Document {
    pub(super) fn read(input: &[u8]) -> Result<Self> {
        let mut decoder = flate2::bufread::GzDecoder::new(input);
        let mut xml = Vec::new();
        (&mut decoder).take(MAX_XML + 1).read_to_end(&mut xml)?;
        if xml.len() as u64 > MAX_XML || !decoder.get_ref().is_empty() {
            return Err("Premiere XML exceeds limit or has trailing gzip content".into());
        }
        let native = PremiereDocument::from_xml_bytes(&xml, Some(GzipFraming::default()))?;
        let tree = &native.tree;
        let mut project_refs =
            tree.children(native.root).iter().copied().filter(|&node| {
                tree.tag(node) == "Project" && tree.get(node, "ObjectRef").is_some()
            });
        let project_ref = project_refs
            .next()
            .ok_or("missing Premiere project reference")?;
        if project_refs.next().is_some() {
            return Err("duplicate Premiere project reference".into());
        }
        let project = native.resolve(project_ref)?;
        if tree.tag(native.root) != "PremiereData"
            || tree.tag(project) != "Project"
            || tree.get(project, "Version") != Some("46")
        {
            return Err("expected Premiere 27 project version 46".into());
        }
        if tree
            .iter(native.root)
            .any(|node| matches!(tree.tag(node), "ProxyMedia" | "IsProxy"))
        {
            return Err("Premiere proxy media is outside the verified relink profile".into());
        }
        let mut locators = Vec::new();
        let mut paths = Paths::new();
        let mut conform = Conform::new();
        let mut keys = HashSet::new();
        let mut states = HashMap::new();
        for &media in tree.children(native.root) {
            if tree.tag(media) != "Media" {
                continue;
            }
            let file = unique_child(tree, media, "FilePath")?;
            let actual = unique_child(tree, media, "ActualMediaFilePath")?;
            let relative: Vec<_> = children(tree, media, "RelativePath").collect();
            let first = *relative.first().ok_or("missing Premiere RelativePath")?;
            let value = scalar(tree, first)?;
            for &node in &relative[1..] {
                if scalar(tree, node)? != value {
                    return Err("contradictory Premiere RelativePath fields".into());
                }
            }
            let original = scalar(tree, file)?;
            original_path(original)?;
            if scalar(tree, actual)? != original {
                return Err("Premiere original and actual media locators disagree".into());
            }
            let key = scalar(tree, unique_child(tree, media, "FileKey")?)?;
            let state = scalar(tree, unique_child(tree, media, "ContentAndMetadataState")?)?;
            if !keys.insert(key)
                || states
                    .insert(state, original)
                    .is_some_and(|old| old != original)
            {
                return Err(
                    "Premiere media objects share an ambiguous FileKey or content cache state"
                        .into(),
                );
            }
            paths.insert(original.into());
            remember(&mut conform, original, identity(&native, media));
            locators.push(Locator {
                original: original.into(),
                media,
                absolute: [file, actual],
                relative,
            });
        }
        Ok(Self {
            native,
            locators,
            paths,
            conform,
        })
    }

    pub(super) fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(super) fn conform(&self) -> &Conform {
        &self.conform
    }

    pub(super) fn write(
        mut self,
        output: &Path,
        moves: &Destinations,
        file: &mut File,
    ) -> Result<()> {
        let parent = output
            .parent()
            .ok_or("missing Premiere project directory")?;
        for locator in self.locators {
            let new = moves
                .get(&locator.original)
                .ok_or("unmapped Premiere original")?;
            if new != &locator.original {
                let tree = &mut self.native.tree;
                // A native Premiere 27 relink drops the old file-modification
                // cache observation and records the previous saved path.
                if let Some(mod_time) = optional_child(tree, locator.media, "CCFileModTime")? {
                    remove_child(tree, locator.media, mod_time)?;
                }
                if optional_child(tree, locator.media, "MediaFileHistory0")?.is_none() {
                    let history = tree.new_element("MediaFileHistory0");
                    tree.set_text(history, Some(locator.original.as_str()));
                    append_uniform_child(tree, locator.media, history)?;
                }
            }
            for node in locator.absolute {
                self.native.tree.set_text(node, Some(new.as_str()));
            }
            let relative = relative_path(parent, Path::new(new));
            let mut relative_fields = locator.relative.into_iter();
            let first = relative_fields
                .next()
                .ok_or("missing Premiere RelativePath")?;
            self.native.tree.set_text(
                first,
                Some(
                    relative
                        .to_str()
                        .ok_or("non-UTF-8 relative media locator")?,
                ),
            );
            for redundant in relative_fields {
                remove_child(&mut self.native.tree, locator.media, redundant)?;
            }
        }
        Ok(file.write_all(&self.native.to_bytes()?)?)
    }
}

fn identity(native: &PremiereDocument, media: NodeId) -> Option<Identity> {
    let tree = &native.tree;
    let read = |owner, tag| scalar(tree, unique_child(tree, owner, tag).ok()?).ok();
    if read(media, "UseAlternateStart")? != "true" {
        return None;
    }
    let video = native
        .resolve(unique_child(tree, media, "VideoStream").ok()?)
        .ok()?;
    let frame_ticks = read(video, "FrameRate")?.parse::<i64>().ok()?;
    let start = read(media, "AlternateStart")?.parse::<i64>().ok()?;
    // First metadata comparison uses exact 24 fps non-drop source clocks.
    if frame_ticks != 10_584_000_000 || start < 0 || start % frame_ticks != 0 {
        return None;
    }
    Some(Identity {
        reel: read(media, "AlternateTapeName")?.into(),
        timecode: crate::timecode(start / frame_ticks),
    })
}

fn unique_child(tree: &Tree, owner: NodeId, tag: &str) -> Result<NodeId> {
    optional_child(tree, owner, tag)?.ok_or_else(|| format!("missing Premiere {tag}").into())
}

fn optional_child(tree: &Tree, owner: NodeId, tag: &str) -> Result<Option<NodeId>> {
    let mut nodes = children(tree, owner, tag);
    let node = nodes.next();
    if nodes.next().is_some() {
        return Err(format!("duplicate Premiere {tag}").into());
    }
    Ok(node)
}

fn children<'a>(tree: &'a Tree, owner: NodeId, tag: &'a str) -> impl Iterator<Item = NodeId> + 'a {
    tree.children(owner)
        .iter()
        .copied()
        .filter(move |&node| tree.tag(node) == tag)
}

fn scalar(tree: &Tree, node: NodeId) -> Result<&str> {
    if !tree.children(node).is_empty() {
        return Err("expected a scalar Premiere media field".into());
    }
    tree.text(node)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("empty Premiere {}", tree.tag(node)).into())
}

fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let common = from
        .components()
        .zip(to.components())
        .take_while(|(a, b)| a == b)
        .count();
    from.components()
        .skip(common)
        .map(|_| std::ffi::OsStr::new(".."))
        .chain(
            to.components()
                .skip(common)
                .map(|component| component.as_os_str()),
        )
        .collect()
}
