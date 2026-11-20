//! File-backed imports use Premiere's encoding after checking portable stream facts.
use super::{invalid, leaf, number, one, read_document, validate_document, Result};
use crate::{
    build::media::{audio_channel_layout, AAC_SAMPLE_TYPE},
    convert::{
        model::{ticks_per_unit, Fraction},
        prepared::{OriginalClip, SampleRepresentation},
    },
    xml::{mutations, NodeId, PremiereDocument, Tree},
};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Picture {
    frame_ticks: i64,
    raster: String,
    duration: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sound {
    sample_ticks: i64,
    duration: i64,
    layout: String,
    sample_type: Option<&'static str>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Original {
    start: i64,
    picture: Option<Picture>,
    sound: Option<Sound>,
}

#[derive(Default)]
pub(crate) struct AssistedMedia {
    originals: BTreeMap<String, Original>,
}

const VIDEO_FIELDS: &[&str] = &[
    "CodecType",
    "OriginalColorSpace",
    "OriginalImageOrientationType",
    "FieldType",
    "FieldOrder",
    "FieldTypeIsUncertain",
    "IgnoreAlpha",
    "AlphaType",
    "AlphaInfoIsUncertain",
    "IsPremultiplied",
];

#[derive(PartialEq, Eq)]
struct Encoding {
    importer: String,
    video: Vec<Option<String>>,
}

pub(crate) struct ImportedEncoding(BTreeMap<String, Vec<Record>>);

struct Record {
    media: NodeId,
    video: Option<NodeId>,
    encoding: Encoding,
}

impl AssistedMedia {
    pub(crate) fn from_clips<'a>(
        clips: impl Iterator<Item = &'a OriginalClip> + Clone,
    ) -> Result<Self> {
        let mut originals = BTreeMap::new();
        for clip in clips.clone() {
            if !clip.premiere_assisted {
                continue;
            }
            let facts = Original::from_clip(clip);
            if originals
                .insert(clip.path.clone(), facts.clone())
                .is_some_and(|old| old != facts)
            {
                return Err(invalid(format!(
                    "{}: conflicting verified stream facts",
                    clip.path
                )));
            }
        }
        // A file may have distinct editorial masters, but one file cannot use
        // competing destination interpretations in a single conversion.
        if clips
            .into_iter()
            .any(|clip| !clip.premiere_assisted && originals.contains_key(&clip.path))
        {
            return Err(invalid(
                "one original has both direct and assisted media bindings",
            ));
        }
        Ok(Self { originals })
    }

    pub(crate) fn paths(&self) -> impl ExactSizeIterator<Item = &String> {
        self.originals.keys()
    }

    pub(crate) fn verify_draft(&self, document: &PremiereDocument) -> Result<()> {
        self.inspect(document, false).map(|_| ())
    }

    pub(crate) fn apply(
        &self,
        document: &mut PremiereDocument,
        path: &Path,
    ) -> Result<ImportedEncoding> {
        let donor = self.inspect(&read_document(path)?, true)?;
        let target = self.inspect(document, false)?;
        for (path, records) in &target {
            let encoding = &donor[path][0].encoding;
            for record in records {
                let tree = &mut document.tree;
                set_scalar(
                    tree,
                    record.media,
                    "ImplementationID",
                    Some(&encoding.importer),
                )?;
                if let Some(video) = record.video {
                    for (key, value) in VIDEO_FIELDS.iter().zip(&encoding.video) {
                        set_scalar(tree, video, key, value.as_deref())?;
                    }
                }
            }
        }
        Ok(ImportedEncoding(donor))
    }

    pub(crate) fn verify_encoding(
        &self,
        document: &PremiereDocument,
        donor: &ImportedEncoding,
    ) -> Result<()> {
        let staged = self.inspect(document, false)?;
        for (path, records) in &staged {
            if records
                .iter()
                .any(|record| record.encoding != donor.0[path][0].encoding)
            {
                return Err(invalid(format!(
                    "{path}: staged Premiere media encoding differs from import"
                )));
            }
        }
        Ok(())
    }

    fn inspect(
        &self,
        document: &PremiereDocument,
        donor: bool,
    ) -> Result<BTreeMap<String, Vec<Record>>> {
        validate_document(document)?;
        let tree = &document.tree;
        let mut matches: BTreeMap<String, Vec<Record>> = BTreeMap::new();
        let mut stream_objects = HashSet::new();
        for &media in tree.children(document.root) {
            if tree.tag(media) != "Media" {
                continue;
            }
            let saved = leaf(tree, media, "FilePath")?;
            let actual = leaf(tree, media, "ActualMediaFilePath")?;
            let Some(path) = saved
                .into_iter()
                .chain(actual)
                .find(|name| self.originals.contains_key(*name))
            else {
                continue;
            };
            let result = (|| {
                if saved != Some(path) || actual != Some(path) {
                    return Err(invalid("saved media locators disagree"));
                }
                let facts = &self.originals[path];
                let alternate = number(tree, media, "AlternateStart")?;
                let enabled = leaf(tree, media, "UseAlternateStart")?;
                let absent =
                    matches!(alternate, None | Some(0)) && matches!(enabled, None | Some("false"));
                // Premiere's importer can leave the logging clock unset. The
                // verified Avid/file clock stays in the staged project. An
                // explicit conflicting Premiere clock still refuses.
                let valid_clock = absent && (donor || facts.start == 0)
                    || facts.start != 0
                        && alternate == Some(facts.start)
                        && enabled == Some("true");
                if one(tree, media, "OfflineReason")?.is_some()
                    || number(tree, media, "Start")?.unwrap_or(0) != 0
                    || !valid_clock
                {
                    return Err(invalid(
                        "saved media is offline or its source clock differs",
                    ));
                }
                let importer = leaf(tree, media, "ImplementationID")?
                    .filter(|value| valid_guid(value))
                    .ok_or_else(|| invalid("invalid Premiere importer ID"))?;
                let mut stream = |tag, class, present| -> Result<Option<NodeId>> {
                    let node = one(tree, media, tag)?
                        .map(|reference| document.resolve(reference))
                        .transpose()?;
                    if node.is_some() != present
                        || node.is_some_and(|node| {
                            tree.tag(node) != tag
                                || tree.get(node, "ClassID") != Some(class)
                                || !stream_objects.insert(node)
                        })
                    {
                        return Err(invalid("unexpected, invalid, or shared saved media stream"));
                    }
                    Ok(node)
                };
                let video = stream(
                    "VideoStream",
                    "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5",
                    facts.picture.is_some(),
                )?;
                let audio = stream(
                    "AudioStream",
                    "0b5cf52f-2b85-4863-890b-8844b64ecfe9",
                    facts.sound.is_some(),
                )?;
                let mut fields = Vec::new();
                if let Some((video, picture)) = video.zip(facts.picture.as_ref()) {
                    if number(tree, video, "Duration")? != Some(picture.duration)
                        || number(tree, video, "FrameRate")? != Some(picture.frame_ticks)
                        || leaf(tree, video, "FrameRect")? != Some(picture.raster.as_str())
                        || !matches!(
                            leaf(tree, video, "OriginalImageOrientationType")?,
                            None | Some("1")
                        )
                    {
                        return Err(invalid(
                            "saved picture clock, extent, raster or orientation differs",
                        ));
                    }
                    for key in VIDEO_FIELDS {
                        let value = leaf(tree, video, key)?;
                        if value.is_some_and(|value| value.len() > 8192) {
                            return Err(invalid("saved picture encoding field exceeds limit"));
                        }
                        fields.push(value.map(str::to_owned));
                    }
                    if donor
                        && !fields[0]
                            .as_deref()
                            .is_some_and(|value| value.parse::<u32>().is_ok_and(|n| n != 0))
                    {
                        return Err(invalid("Premiere did not identify the picture codec"));
                    }
                }
                if let Some((audio, sound)) = audio.zip(facts.sound.as_ref()) {
                    if number(tree, audio, "Duration")? != Some(sound.duration)
                        || number(tree, audio, "FrameRate")? != Some(sound.sample_ticks)
                        || leaf(tree, audio, "AudioChannelLayout")? != Some(sound.layout.as_str())
                        || leaf(tree, audio, "SampleType")? != sound.sample_type
                    {
                        return Err(invalid("saved audio extent, sample clock, representation, or channel order differs"));
                    }
                }
                let records = matches.entry(path.to_owned()).or_default();
                if donor && !records.is_empty() {
                    return Err(invalid("ambiguous imported media records"));
                }
                records.push(Record {
                    media,
                    video,
                    encoding: Encoding {
                        importer: importer.into(),
                        video: fields,
                    },
                });
                Ok(())
            })();
            result.map_err(|error| invalid(format!("{path}: {error}")))?;
        }
        if let Some(path) = self
            .originals
            .keys()
            .find(|path| !matches.contains_key(*path))
        {
            return Err(invalid(format!("{path}: no exact Premiere media record")));
        }
        Ok(matches)
    }
}

impl Original {
    fn from_clip(clip: &OriginalClip) -> Self {
        Self {
            start: clip.video.as_ref().map_or(0, |video| video.source_start),
            picture: clip.video.as_ref().map(|video| Picture {
                frame_ticks: ticks_per_unit(video.rate),
                raster: format!("0,0,{},{}", video.width, video.height),
                duration: video.duration,
            }),
            sound: clip.audio.as_ref().map(|audio| Sound {
                sample_ticks: ticks_per_unit(Fraction::new(audio.sample_rate, 1)),
                duration: audio.duration,
                layout: audio_channel_layout(audio.channels as u32),
                sample_type: match audio.representation {
                    SampleRepresentation::Pcm16 => Some("3"),
                    SampleRepresentation::Pcm16Be => Some("11"),
                    SampleRepresentation::Pcm24 => None,
                    SampleRepresentation::Aac => Some(AAC_SAMPLE_TYPE),
                    SampleRepresentation::Mp3 => {
                        crate::build::media::compressed_sample_type(".mp3")
                    }
                },
            }),
        }
    }
}

fn set_scalar(tree: &mut Tree, parent: NodeId, key: &str, value: Option<&str>) -> Result<()> {
    let node = one(tree, parent, key)?;
    match (node, value) {
        (Some(node), Some(value)) => tree.set_text(node, Some(value)),
        (Some(node), None) => mutations::remove_child(tree, parent, node)?,
        (None, Some(value)) => {
            let node = tree.new_element(key);
            tree.set_text(node, Some(value));
            mutations::append_uniform_child(tree, parent, node)?;
        }
        (None, None) => {}
    }
    Ok(())
}

fn valid_guid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
