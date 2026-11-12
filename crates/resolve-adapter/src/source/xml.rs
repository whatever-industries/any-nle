//! Bounded, non-extracting archive and XML reader for untrusted source projects.
use crate::Result;
use quick_xml::{events::Event, Reader, XmlVersion};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
};
use zip::ZipArchive;

const MAX_ARCHIVE: u64 = 32 << 20;
pub(super) const MAX_EXPANDED: u64 = 64 << 20;
const MAX_MEMBERS: usize = 1024;
const MAX_NODES: usize = 50_000;
const MAX_DEPTH: usize = 96;

#[derive(Debug)]
pub(super) struct Node {
    pub name: String,
    pub id: Option<String>,
    pub text: String,
    pub children: Vec<Node>,
    pub content: Range<usize>,
}

impl Node {
    pub fn id(&self) -> Result<&str> {
        self.id
            .as_deref()
            .ok_or_else(|| format!("missing {} DbId", self.name).into())
    }

    pub fn optional(&self, name: &str) -> Result<Option<&Self>> {
        let mut matches = self.children.iter().filter(|node| node.name == name);
        let found = matches.next();
        if matches.next().is_some() {
            return Err(format!("duplicate {name} in {}", self.name).into());
        }
        Ok(found)
    }

    pub fn one(&self, name: &str) -> Result<&Self> {
        self.optional(name)?
            .ok_or_else(|| format!("missing {name} in {}", self.name).into())
    }

    pub fn scalar(&self) -> Result<&str> {
        if !self.children.is_empty() {
            return Err(format!("expected scalar {}", self.name).into());
        }
        Ok(&self.text)
    }

    pub fn get(&self, name: &str) -> Result<&str> {
        self.one(name)?.scalar()
    }

    pub fn maybe(&self, name: &str) -> Result<Option<&str>> {
        self.optional(name)?.map(Self::scalar).transpose()
    }

    pub fn elements(&self) -> Result<impl Iterator<Item = &Self>> {
        if self.children.iter().any(|node| node.name != "Element") || !self.text.trim().is_empty() {
            return Err(format!("invalid {} collection", self.name).into());
        }
        Ok(self.children.iter())
    }

    pub fn entry(&self) -> Result<&Self> {
        match self.children.as_slice() {
            [node] if self.text.trim().is_empty() => Ok(node),
            _ => Err(format!("invalid {} wrapper", self.name).into()),
        }
    }
}

pub(super) fn read(path: &Path) -> Result<BTreeMap<String, Node>> {
    read_zip(open_file(path)?)
}

pub(super) fn open_file(path: &Path) -> Result<File> {
    if !std::fs::metadata(path)?.is_file() {
        return Err("expected a regular Resolve archive".into());
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_ARCHIVE {
        return Err("expected a regular Resolve archive no larger than 32 MiB".into());
    }
    Ok(file)
}

pub(super) fn checked_zip<R: Read + Seek>(mut input: R) -> Result<ZipArchive<R>> {
    let (member_count, central_start) = directory(&mut input)?;
    let mut archive = ZipArchive::new(input)?;
    // The ZIP library indexes by name. Compare with the on-disk count so an
    // exact duplicate cannot be silently collapsed by that index.
    if archive.len() != member_count
        || archive.central_directory_start() != central_start
        || archive.has_overlapping_files()?
    {
        return Err("duplicate or ambiguous Resolve archive directory".into());
    }
    Ok(archive)
}

pub(super) fn read_zip(input: impl Read + Seek) -> Result<BTreeMap<String, Node>> {
    let mut archive = checked_zip(input)?;
    let mut seen = HashSet::new();
    let mut members = BTreeMap::new();
    let mut remaining = MAX_EXPANDED;
    let mut nodes = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_owned();
        check_member(&entry, &mut seen, remaining)?;
        // Gallery assets are outside the source profile. Count their declared size too.
        if name != "project.xml"
            && !(name.starts_with("MediaPool/") && name.ends_with("/MpFolder.xml"))
            && !(name.starts_with("SeqContainer/") && name.ends_with(".xml"))
        {
            remaining -= entry.size();
            continue;
        }
        let xml = read_member(&mut entry, &mut remaining)?;
        check_version(&xml)?;
        members.insert(name, parse(&xml, &mut nodes)?);
    }
    Ok(members)
}

pub(super) fn check_member<R: Read>(
    entry: &zip::read::ZipFile<'_, R>,
    seen: &mut HashSet<String>,
    remaining: u64,
) -> Result<()> {
    let name = entry.name();
    if !entry.is_file()
        || entry.enclosed_name().is_none()
        || name.len() > 1024
        || name.contains(['\\', ':', '\0'])
        || name.split('/').any(|part| matches!(part, "" | "." | ".."))
        || !seen.insert(name.to_ascii_lowercase())
    {
        return Err("unsafe or duplicate Resolve archive member".into());
    }
    if entry.size() > remaining {
        return Err("Resolve archive exceeds expanded size limit".into());
    }
    Ok(())
}

pub(super) fn read_member<R: Read>(
    entry: &mut zip::read::ZipFile<'_, R>,
    remaining: &mut u64,
) -> Result<String> {
    let mut xml = String::new();
    let expected = entry.size();
    entry.take(expected + 1).read_to_string(&mut xml)?;
    if xml.len() as u64 != expected {
        return Err("Resolve member size differs from ZIP directory".into());
    }
    *remaining -= expected;
    Ok(xml)
}

pub(super) fn check_version(xml: &str) -> Result<()> {
    let mut lines = xml.lines();
    if lines.next().is_none_or(|line| !line.starts_with("<?xml "))
        || lines.next() != Some("<!--DbAppVer=\"21.2.0.0006\" DbPrjVer=\"17\"-->")
    {
        return Err("expected Resolve Studio 21.2.0.0006 project XML".into());
    }
    Ok(())
}

fn directory(input: &mut (impl Read + Seek)) -> Result<(usize, u64)> {
    // The retained .drp cohort uses a regular single-disk ZIP32 directory.
    let length = input.seek(SeekFrom::End(0))?;
    let tail_size = length.min(22 + u64::from(u16::MAX));
    let tail_start = length - tail_size;
    input.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0; tail_size as usize];
    input.read_exact(&mut tail)?;
    let Some((offset, record)) = tail.windows(22).enumerate().rev().find(|(at, window)| {
        window[..4] == *b"PK\x05\x06"
            && *at + 22 + usize::from(u16::from_le_bytes([window[20], window[21]])) == tail.len()
    }) else {
        return Err("missing Resolve ZIP directory terminator".into());
    };
    let local = u16::from_le_bytes(record[8..10].try_into()?);
    let count = usize::from(u16::from_le_bytes(record[10..12].try_into()?));
    let bytes = u64::from(u32::from_le_bytes(record[12..16].try_into()?));
    let start = u64::from(u32::from_le_bytes(record[16..20].try_into()?));
    if record[4..8] != [0; 4]
        || usize::from(local) != count
        || !(1..=MAX_MEMBERS).contains(&count)
        || start + bytes != tail_start + offset as u64
    {
        return Err("unsupported or over-budget Resolve ZIP directory".into());
    }
    Ok((count, start))
}

pub(super) fn parse(text: &str, total_nodes: &mut usize) -> Result<Node> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    loop {
        let position = usize::try_from(reader.buffer_position())?;
        match reader.read_event()? {
            Event::Start(tag) => {
                *total_nodes += 1;
                if *total_nodes > MAX_NODES || stack.len() >= MAX_DEPTH {
                    return Err("Resolve XML exceeds node or depth limit".into());
                }
                let mut id = None;
                for attr in tag.attributes() {
                    let attr = attr?;
                    let value = attr
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())?;
                    if attr.key.as_ref() == b"DbId" {
                        if value.len() != 36 || uuid::Uuid::parse_str(&value).is_err() {
                            return Err("invalid Resolve DbId".into());
                        }
                        id = Some(value.to_ascii_lowercase());
                    }
                }
                let content_start = usize::try_from(reader.buffer_position())?;
                stack.push(Node {
                    name: std::str::from_utf8(tag.name().as_ref())?.into(),
                    id,
                    text: String::new(),
                    children: Vec::new(),
                    content: content_start..content_start,
                });
            }
            Event::End(_) => {
                let mut node = stack.pop().ok_or("unexpected XML end tag")?;
                node.content.end = position;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else if root.replace(node).is_some() {
                    return Err("multiple Resolve XML roots".into());
                }
            }
            Event::Text(value) => {
                let value = value.xml_content(XmlVersion::Implicit1_0)?;
                append(&mut stack, &value)?;
            }
            Event::CData(value) => {
                let value = value.xml_content(XmlVersion::Implicit1_0)?;
                append(&mut stack, &value)?;
            }
            Event::GeneralRef(value) => {
                let name = value.decode()?;
                let character = match name.as_ref() {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "apos" => '\'',
                    "quot" => '"',
                    _ => value
                        .resolve_char_ref()?
                        .ok_or("unsupported Resolve XML entity")?,
                };
                append(&mut stack, character.encode_utf8(&mut [0; 4]))?;
            }
            Event::Decl(declaration) => {
                if let Some(encoding) = declaration.encoding() {
                    if !encoding?.eq_ignore_ascii_case(b"utf-8") {
                        return Err("Resolve XML must be UTF-8".into());
                    }
                }
            }
            Event::Comment(_) => {}
            Event::DocType(_) | Event::PI(_) => {
                return Err("Resolve XML declarations of external content are unsupported".into())
            }
            Event::Eof => break,
            Event::Empty(_) => return Err("unexpected XML reader state".into()),
        }
    }
    if !stack.is_empty() {
        return Err("truncated Resolve XML".into());
    }
    root.ok_or_else(|| "missing Resolve XML root".into())
}

fn append(stack: &mut [Node], text: &str) -> Result<()> {
    if let Some(node) = stack.last_mut() {
        node.text.push_str(text);
    } else if !text.trim().is_empty() {
        return Err("content outside Resolve XML root".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn structural_xml_handles_native_names_and_rejects_entities() {
        let tree = parse(
            "<ListMgt::Native><A>&amp;&#xE9;</A><B/></ListMgt::Native>",
            &mut 0,
        )
        .unwrap();
        assert_eq!(tree.get("A").unwrap(), "&é");
        assert_eq!(tree.get("B").unwrap(), "");
        for bad in [
            "<A>&unknown;</A>",
            "<!DOCTYPE A SYSTEM 'file:///tmp/a'><A/>",
            "<A><B></A>",
            "<A/><B/>",
            "<A>",
        ] {
            assert!(parse(bad, &mut 0).is_err(), "{bad}");
        }
        assert!(parse(
            &format!(
                "{}{}",
                "<A>".repeat(MAX_DEPTH + 1),
                "</A>".repeat(MAX_DEPTH + 1)
            ),
            &mut 0
        )
        .is_err());
        assert!(parse("<A><B/><B/></A>", &mut 0).unwrap().one("B").is_err());
    }

    #[test]
    fn archive_never_accepts_path_aliases_or_duplicate_members() {
        for names in [
            ["project.xml", "PROJECT.XML"],
            ["project.xml", "../x.xml"],
            ["project.xml", "MediaPool//MpFolder.xml"],
        ] {
            let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
            for name in names {
                zip.start_file(name, SimpleFileOptions::default()).unwrap();
                zip.write_all(b"<?xml version=\"1.0\"?>\n<!--DbAppVer=\"21.2.0.0006\" DbPrjVer=\"17\"-->\n<Root/>").unwrap();
            }
            assert!(read_zip(Cursor::new(zip.finish().unwrap().into_inner())).is_err());
        }
    }
    #[test]
    fn exact_duplicate_member_is_refused_before_xml_parse() {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for name in ["project.xml", "PROJECT.xml"] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"<Root/>").unwrap();
        }
        let mut bytes = zip.finish().unwrap().into_inner();
        let names: Vec<_> = bytes
            .windows(11)
            .enumerate()
            .filter_map(|(i, window)| (window == b"PROJECT.xml").then_some(i))
            .collect();
        assert_eq!(names.len(), 2);
        for i in names {
            bytes[i..i + 11].copy_from_slice(b"project.xml");
        }
        assert!(read_zip(Cursor::new(bytes))
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
    }
}
