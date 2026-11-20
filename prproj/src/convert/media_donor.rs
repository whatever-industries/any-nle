//! Bounded Premiere 27 linked-media records for the internal import probe.
use crate::build::media::MONO_LAYOUT;
use crate::xml::{NodeId, PremiereDocument, Tree};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;

const NATIVE_IMPORTER: &str = "1fa18bfa-255c-44b1-ad73-56bcd99fceaf";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Essence {
    Picture {
        codec: u32,
        frame_ticks: i64,
        width: i64,
        height: i64,
    },
    MonoPcm {
        bits: i64,
        sample_ticks: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    pub essence: Essence,
    pub duration: i64,
    pub physical_start: i64,
    pub alternate_start: Option<i64>,
}

pub type ExpectedMedia = BTreeMap<String, FileFacts>;
type Result<T> = std::result::Result<T, crate::Error>;

#[derive(Clone)]
struct Record {
    media: NodeId,
    stream: NodeId,
    importer: String,
    orientation: Option<String>,
    codec: Option<u32>,
    color_space: Option<String>,
}
pub struct MediaRecords {
    records: HashMap<String, Record>,
}

fn invalid(message: impl Into<String>) -> crate::Error {
    crate::Error::Value(message.into())
}

fn one(tree: &Tree, node: NodeId, tag: &str) -> Result<Option<NodeId>> {
    let mut matches = tree
        .children(node)
        .iter()
        .copied()
        .filter(|&child| tree.tag(child) == tag);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(invalid(format!("ambiguous {tag} in Premiere media record")));
    }
    Ok(first)
}

fn leaf<'a>(tree: &'a Tree, node: NodeId, tag: &str) -> Result<Option<&'a str>> {
    let Some(child) = one(tree, node, tag)? else {
        return Ok(None);
    };
    if !tree.children(child).is_empty() || !tree[child].attrs.is_empty() {
        return Err(invalid(format!(
            "non-scalar {tag} in Premiere media record"
        )));
    }
    Ok(Some(tree[child].text_or_empty()))
}

fn number(tree: &Tree, node: NodeId, tag: &str) -> Result<Option<i64>> {
    leaf(tree, node, tag)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| invalid(format!("invalid {tag} in Premiere media record")))
        })
        .transpose()
}

fn read_document(path: &Path) -> Result<PremiereDocument> {
    // Apply the existing .prproj decompression and XML bounds after bounding the file read.
    const LIMIT: usize = 64 << 20;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(invalid("Premiere donor exceeds 64 MiB"));
    }
    PremiereDocument::from_bytes(&bytes)
}

fn validate_document(document: &PremiereDocument) -> Result<()> {
    let tree = &document.tree;
    let mut projects = tree
        .children(document.root)
        .iter()
        .copied()
        .filter(|&node| tree.tag(node) == "Project" && tree.get(node, "ObjectID").is_some());
    if !projects.next().is_some_and(|project| {
        tree.get(project, "ClassID") == Some("62ad66dd-0dcd-42da-a660-6d8fbde94876")
            && tree.get(project, "Version") == Some("46")
    }) || projects.next().is_some()
    {
        return Err(invalid("expected a Premiere 27 Project version 46 save"));
    }
    let mut object_ids = HashSet::new();
    let mut object_uids = HashSet::new();
    for &node in tree.children(document.root) {
        if tree
            .get(node, "ObjectID")
            .is_some_and(|id| !object_ids.insert(id))
            || tree
                .get(node, "ObjectUID")
                .is_some_and(|id| !object_uids.insert(id))
        {
            return Err(invalid(
                "duplicate object identity in Premiere media project",
            ));
        }
    }
    Ok(())
}

impl MediaRecords {
    pub fn read(path: &Path, expected: &ExpectedMedia) -> Result<Self> {
        Self::inspect(&read_document(path)?, expected)
    }

    fn inspect(document: &PremiereDocument, expected: &ExpectedMedia) -> Result<Self> {
        let tree = &document.tree;
        validate_document(document)?;
        let mut records = HashMap::new();
        let mut stream_objects = HashSet::new();
        for &media in tree.children(document.root) {
            if tree.tag(media) != "Media" {
                continue;
            }
            let file = leaf(tree, media, "FilePath")?;
            let actual = leaf(tree, media, "ActualMediaFilePath")?;
            let Some(path) = file
                .filter(|p| expected.contains_key(*p))
                .or_else(|| actual.filter(|p| expected.contains_key(*p)))
            else {
                continue;
            };
            let facts = &expected[path];
            let record = (|| {
                if file != Some(path) || actual != Some(path) {
                    return Err(invalid("saved media locators disagree"));
                }
                if one(tree, media, "OfflineReason")?.is_some()
                    || number(tree, media, "Start")?.unwrap_or(0) != facts.physical_start
                {
                    return Err(invalid(
                        "saved media is offline or its physical start differs",
                    ));
                }
                let alternate = number(tree, media, "AlternateStart")?;
                let use_alternate = leaf(tree, media, "UseAlternateStart")?;
                if match facts.alternate_start {
                    Some(start) => alternate != Some(start) || use_alternate != Some("true"),
                    None => {
                        !matches!(alternate, None | Some(0))
                            || !matches!(use_alternate, None | Some("false"))
                    }
                } {
                    return Err(invalid("saved alternate source clock differs"));
                }
                let importer = leaf(tree, media, "ImplementationID")?
                    .filter(|value| *value == NATIVE_IMPORTER)
                    .ok_or_else(|| invalid("unqualified Premiere media importer"))?;
                let (tag, other, class) = match facts.essence {
                    Essence::Picture { .. } => (
                        "VideoStream",
                        "AudioStream",
                        "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5",
                    ),
                    Essence::MonoPcm { .. } => (
                        "AudioStream",
                        "VideoStream",
                        "0b5cf52f-2b85-4863-890b-8844b64ecfe9",
                    ),
                };
                if one(tree, media, other)?.is_some() {
                    return Err(invalid("unexpected additional saved media stream"));
                }
                let reference = one(tree, media, tag)?
                    .ok_or_else(|| invalid("saved media stream is absent"))?;
                let stream = document.resolve(reference)?;
                if tree.tag(stream) != tag
                    || tree.get(stream, "ClassID") != Some(class)
                    || !stream_objects.insert(stream)
                {
                    return Err(invalid("invalid or shared saved media stream reference"));
                }
                if number(tree, stream, "Duration")? != Some(facts.duration) {
                    return Err(invalid("saved media extent differs from the original"));
                }
                let (orientation, codec, color_space) = match facts.essence {
                    Essence::Picture {
                        codec,
                        frame_ticks,
                        width,
                        height,
                    } => {
                        let saved_codec = number(tree, stream, "CodecType")?
                            .and_then(|value| u32::try_from(value).ok());
                        if saved_codec != Some(codec)
                            || number(tree, stream, "FrameRate")? != Some(frame_ticks)
                            || leaf(tree, stream, "FrameRect")?
                                != Some(&format!("0,0,{width},{height}"))
                        {
                            return Err(invalid(
                                "saved picture codec, frame clock or raster differs",
                            ));
                        }
                        let orientation = leaf(tree, stream, "OriginalImageOrientationType")?;
                        if !matches!(orientation, None | Some("1")) {
                            return Err(invalid("saved picture orientation is unqualified"));
                        }
                        let color = if codec == u32::from_be_bytes(*b"apcs") {
                            let saved = leaf(tree, stream, "OriginalColorSpace")?
                                .filter(|value| *value == r#"{"baseColorProfile":{"colorProfileName":"BT.709 RGB Full"},"baseProfileType":1}"#)
                                .ok_or_else(|| invalid("saved ProRes LT color interpretation differs"))?;
                            Some(saved.to_owned())
                        } else {
                            None
                        };
                        (orientation.map(str::to_owned), saved_codec, color)
                    }
                    Essence::MonoPcm { bits, sample_ticks } => {
                        if leaf(tree, stream, "AudioChannelLayout")? != Some(MONO_LAYOUT)
                            || number(tree, stream, "FrameRate")? != Some(sample_ticks)
                            || number(tree, stream, "SampleType")?
                                != match bits {
                                    16 => Some(3),
                                    24 => None,
                                    _ => return Err(invalid("unqualified PCM depth")),
                                }
                        {
                            return Err(invalid(
                                "saved mono PCM layout, depth or sample clock differs",
                            ));
                        }
                        (None, None, None)
                    }
                };
                Ok(Record {
                    media,
                    stream,
                    importer: importer.into(),
                    orientation,
                    codec,
                    color_space,
                })
            })()
            .map_err(|e| invalid(format!("{path}: {e}")))?;
            if records.insert(path.to_owned(), record).is_some() {
                return Err(invalid(format!(
                    "{path}: ambiguous saved Premiere media record"
                )));
            }
        }
        if let Some(path) = expected.keys().find(|path| !records.contains_key(*path)) {
            return Err(invalid(format!(
                "{path}: no matching saved Premiere media record"
            )));
        }
        Ok(Self { records })
    }

    pub(crate) fn apply(
        &self,
        document: &mut PremiereDocument,
        expected: &ExpectedMedia,
    ) -> Result<()> {
        let target = Self::inspect(document, expected)?;
        for (path, donor) in &self.records {
            let saved = target
                .records
                .get(path)
                .ok_or_else(|| invalid("donor path is outside staged media"))?;
            let tree = &mut document.tree;
            let implementation = one(tree, saved.media, "ImplementationID")?
                .ok_or_else(|| invalid("staged importer is absent"))?;
            tree.set_text(implementation, Some(donor.importer.as_str()));
            if let Some(codec) = donor.codec {
                let node = one(tree, saved.stream, "CodecType")?
                    .ok_or_else(|| invalid("staged picture codec is absent"))?;
                tree.set_text(node, Some(&codec.to_string()));
            }
            if let Some(color) = &donor.color_space {
                let node = one(tree, saved.stream, "OriginalColorSpace")?
                    .ok_or_else(|| invalid("staged picture color field is absent"))?;
                tree.set_text(node, Some(color));
            }
            if let Some(orientation) = &donor.orientation {
                let node = match one(tree, saved.stream, "OriginalImageOrientationType")? {
                    Some(node) => node,
                    None => {
                        let node = tree.new_element("OriginalImageOrientationType");
                        crate::xml::mutations::append_uniform_child(tree, saved.stream, node)?;
                        node
                    }
                };
                tree.set_text(node, Some(orientation.as_str()));
            }
        }
        Ok(())
    }
}

pub(crate) fn verify(document: &PremiereDocument, expected: &ExpectedMedia) -> Result<()> {
    MediaRecords::inspect(document, expected).map(|_| ())
}

mod assisted;
pub(super) use assisted::AssistedMedia;
