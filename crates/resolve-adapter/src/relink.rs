//! Verified relocation of saved original media in the qualified native profiles.
mod premiere;

use crate::{media::probe_json, source::relink as resolve, Published, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const MAX_PROJECT: u64 = 32 << 20;
const MAX_PLAN: u64 = 1 << 20;
const MAX_MEDIA: usize = 1024;
const MANIFEST_SCHEMA: &str = "anynle.relink/1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    project_sha256: String,
    originals: Vec<Original>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub original: String,
    pub destination: String,
}

pub(crate) type Paths = BTreeSet<String>;
pub(crate) type Destinations = BTreeMap<String, String>;
pub(crate) type Conform = BTreeMap<String, Option<Identity>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Identity {
    pub reel: String,
    pub timecode: String,
}

pub(crate) fn remember(conform: &mut Conform, path: &str, identity: Option<Identity>) {
    conform
        .entry(path.into())
        .and_modify(|previous| {
            if *previous != identity {
                *previous = None;
            }
        })
        .or_insert(identity);
}

enum Document {
    Premiere(premiere::Document),
    Resolve(resolve::Document),
}

impl Document {
    fn read(input: &Path, bytes: &[u8]) -> Result<Self> {
        match input.extension().and_then(|ext| ext.to_str()) {
            Some("prproj") => Ok(Self::Premiere(premiere::Document::read(bytes)?)),
            Some("drp") => Ok(Self::Resolve(resolve::Document::read(bytes)?)),
            _ => Err("expected a .prproj or .drp input".into()),
        }
    }

    fn paths(&self) -> &Paths {
        match self {
            Self::Premiere(doc) => doc.paths(),
            Self::Resolve(doc) => doc.paths(),
        }
    }

    fn conform(&self) -> &Conform {
        match self {
            Self::Premiere(doc) => doc.conform(),
            Self::Resolve(doc) => doc.conform(),
        }
    }

    fn write(self, output: &Path, moves: &Destinations, file: &mut File) -> Result<()> {
        match self {
            Self::Premiere(doc) => doc.write(output, moves, file),
            Self::Resolve(doc) => doc.write(moves, file),
        }
    }
}

fn bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(format!("expected a regular file at {}", path.display()).into());
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(format!("{} exceeds the {} byte read limit", path.display(), max).into());
    }
    Ok(bytes)
}

fn plan<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&bounded(path, MAX_PLAN)?)?)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn original_path(path: &str) -> Result<&Path> {
    let parsed = Path::new(path);
    if !path.starts_with('/')
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || path.contains('\\')
        || path[1..]
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
        || parsed.extension().and_then(|ext| ext.to_str()) != Some("mov")
    {
        return Err(format!("expected a canonical absolute .mov locator: {path}").into());
    }
    Ok(parsed)
}

fn checked_paths(paths: &Paths) -> Result<()> {
    if !(1..=MAX_MEDIA).contains(&paths.len()) {
        return Err("expected between 1 and 1024 saved media originals".into());
    }
    for path in paths {
        original_path(path)?;
    }
    Ok(())
}

fn output_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
    let name = path.file_name().ok_or("missing output filename")?;
    let output = fs::canonicalize(parent.unwrap_or(Path::new(".")))?.join(name);
    match output.symlink_metadata() {
        Ok(_) => Err(format!("output already exists: {}", output.display()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output),
        Err(error) => Err(error.into()),
    }
}

fn publish(
    output: &Path,
    write: impl FnOnce(&mut File) -> Result<()>,
    check: impl FnOnce(&Path) -> Result<()>,
) -> Result<Published> {
    let mut stage = NamedTempFile::new_in(output.parent().ok_or("missing output directory")?)?;
    write(stage.as_file_mut())?;
    stage.as_file().sync_all()?;
    check(stage.path())?;
    fs::hard_link(stage.path(), output)?;
    let path = stage.path().display().to_string();
    let cleanup_warning = stage.close().err().map(|error| {
        format!(
            "published {}, retained stage {path}: {error}",
            output.display()
        )
    });
    Ok(Published { cleanup_warning })
}

fn verify_project(input: &Path, expected: &str) -> Result<()> {
    if sha256(&bounded(input, MAX_PROJECT)?) != expected {
        return Err("input project SHA-256 differs from the trusted manifest".into());
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    bytes: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn stamp(meta: Metadata) -> Result<FileStamp> {
    if !meta.is_file() {
        return Err("expected a regular media file".into());
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(FileStamp {
        bytes: meta.len(),
        modified: meta.modified()?,
        #[cfg(unix)]
        identity: (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()),
    })
}

fn media_stamp(path: &str) -> Result<FileStamp> {
    let path = original_path(path)?;
    if fs::canonicalize(path)?.as_os_str() != path.as_os_str() {
        return Err(format!(
            "media locator resolves through an alias: {}",
            path.display()
        )
        .into());
    }
    stamp(fs::metadata(path)?)
}

fn media_hash(path: &str, before: &FileStamp) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    if stamp(file.metadata()?)? != *before
        || io::copy(&mut file, &mut hash)? != before.bytes
        || stamp(file.metadata()?)? != *before
    {
        return Err(format!("media changed while hashing: {path}").into());
    }
    Ok(hex::encode(hash.finalize()))
}

fn recheck(stamps: &BTreeMap<String, FileStamp>) -> Result<()> {
    for (path, before) in stamps {
        if media_stamp(path)? != *before {
            return Err(format!("media changed before publication: {path}").into());
        }
    }
    Ok(())
}

fn media_identity(ffprobe: &Path, path: &str) -> Result<Identity> {
    let value = probe_json(
        ffprobe,
        Path::new(path),
        &[
            "-f",
            "mov",
            "-show_entries",
            "stream=codec_tag_string:stream_tags=reel_name,timecode",
        ],
    )
    .map_err(|error| format!("could not read MOV identity at {path}: {error}"))?;
    let streams = value["streams"]
        .as_array()
        .ok_or("missing MOV stream list")?;
    let mut tracks = streams
        .iter()
        .filter(|row| row["codec_tag_string"] == "tmcd");
    let track = tracks
        .next()
        .ok_or_else(|| format!("missing MOV timecode track at {path}"))?;
    if tracks.next().is_some() {
        return Err(format!("ambiguous MOV timecode tracks at {path}").into());
    }
    let tag = |key| {
        track["tags"][key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("missing MOV {key} tag at {path}"))
    };
    Ok(Identity {
        reel: tag("reel_name")?,
        timecode: tag("timecode")?,
    })
}

/// List the saved absolute paths without opening any media.
pub fn originals(input: &Path) -> Result<Vec<String>> {
    let document = Document::read(input, &bounded(input, MAX_PROJECT)?)?;
    checked_paths(document.paths())?;
    Ok(document.paths().iter().cloned().collect())
}

/// Capture optional checksums while the selected, known-good originals are online.
pub fn capture(input: &Path, selected: Option<&[String]>, output: &Path) -> Result<Published> {
    let output = output_path(output)?;
    let bytes = bounded(input, MAX_PROJECT)?;
    let document = Document::read(input, &bytes)?;
    checked_paths(document.paths())?;
    let paths = match selected {
        Some(selected) => {
            let paths: Paths = selected.iter().cloned().collect();
            if paths.len() != selected.len()
                || paths.is_empty()
                || !paths.is_subset(document.paths())
            {
                return Err("checksum selection must contain distinct saved original paths".into());
            }
            paths
        }
        None => document.paths().clone(),
    };
    let mut stamps = BTreeMap::new();
    let mut originals = Vec::new();
    for path in paths {
        let before = media_stamp(&path)?;
        let sha256 = media_hash(&path, &before)?;
        stamps.insert(path.clone(), before);
        originals.push(Original { path, sha256 });
    }
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA.into(),
        project_sha256: sha256(&bytes),
        originals,
    };
    let mut json = serde_json::to_vec_pretty(&manifest)?;
    json.push(b'\n');
    if json.len() as u64 > MAX_PLAN {
        return Err("manifest exceeds size limit".into());
    }
    publish(
        &output,
        |file| Ok(file.write_all(&json)?),
        |_| {
            verify_project(input, &manifest.project_sha256)?;
            recheck(&stamps)
        },
    )
}

pub fn read_moves(path: &Path) -> Result<Vec<Move>> {
    plan(path)
}

pub(crate) struct VerifiedMoves {
    pub(crate) destinations: Destinations,
    stamps: BTreeMap<String, FileStamp>,
}

impl VerifiedMoves {
    pub(crate) fn recheck(&self) -> Result<()> {
        recheck(&self.stamps)
    }
}

pub(crate) fn resolve_project_bytes(path: &Path) -> Result<Vec<u8>> {
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("drp"))
    {
        return Err("move verification requires a reference and candidate DRP".into());
    }
    bounded(path, MAX_PROJECT)
}

pub(crate) fn verify_resolve_moves(
    bytes: &[u8],
    requested: &[Move],
    ffprobe: &Path,
    manifest: Option<&Path>,
) -> Result<VerifiedMoves> {
    let document = Document::Resolve(resolve::Document::read(bytes)?);
    verify_moves(&document, &sha256(bytes), requested, ffprobe, manifest)
}

/// Write a new project using explicit locator moves. Optional checksums must
/// have been captured from the known-good originals before relocation.
pub fn apply(
    input: &Path,
    requested: &[Move],
    output: &Path,
    ffprobe: &Path,
    manifest: Option<&Path>,
) -> Result<Published> {
    let output = output_path(output)?;
    if input.extension() != output.extension() {
        return Err("relink output must have the same extension as the input".into());
    }
    let bytes = bounded(input, MAX_PROJECT)?;
    let digest = sha256(&bytes);
    let document = Document::read(input, &bytes)?;
    let verified = verify_moves(&document, &digest, requested, ffprobe, manifest)?;
    let expected_paths: Paths = verified.destinations.values().cloned().collect();
    publish(
        &output,
        |file| document.write(&output, &verified.destinations, file),
        |stage| {
            let staged = Document::read(&output, &bounded(stage, MAX_PROJECT)?)?;
            if staged.paths() != &expected_paths {
                return Err("staged project did not retain the verified media bindings".into());
            }
            verify_project(input, &digest)?;
            verified.recheck()
        },
    )
}

fn verify_moves(
    document: &Document,
    digest: &str,
    requested: &[Move],
    ffprobe: &Path,
    manifest: Option<&Path>,
) -> Result<VerifiedMoves> {
    let paths = document.paths();
    checked_paths(paths)?;
    let moves = bindings(paths, requested)?;
    let trusted = manifest
        .map(|path| checksums(path, digest, paths))
        .transpose()?;
    let mut stamps = BTreeMap::new();
    let mut basenames: HashMap<_, Vec<_>> = HashMap::new();
    for (path, target) in &moves {
        let stamp = media_stamp(target)
            .map_err(|error| format!("could not verify media at {target}: {error}"))?;
        stamps.insert(target.clone(), stamp);
        basenames
            .entry(original_path(path)?.file_name().ok_or("missing basename")?)
            .or_default()
            .push(path);
    }
    for same_name in basenames.values().filter(|rows| rows.len() > 1) {
        if matches!(document, Document::Premiere(_)) {
            // The retained Premiere 27 failing control had equal size and
            // timestamps alongside its shared cache ID. Keep that unqualified
            // stat cohort out even when the bytes can be verified.
            let mut seen = BTreeSet::new();
            for &original in same_name {
                let s = &stamps[&moves[original]];
                let seconds = s.modified.duration_since(std::time::UNIX_EPOCH)?.as_secs();
                if !seen.insert((s.bytes, seconds)) {
                    return Err("Premiere same-name candidates share file size and modification second; picture cache behavior is unqualified".into());
                }
            }
        }
        if trusted.is_none() && same_name.iter().any(|&path| &moves[path] != path) {
            let mut seen = BTreeSet::new();
            for &original in same_name {
                let expected = document.conform().get(original).and_then(Option::as_ref)
                    .ok_or_else(|| format!("same-name sources need a saved reel and source timecode, or a trusted checksum: {original}"))?;
                if !seen.insert((&expected.reel, &expected.timecode)) {
                    return Err(
                        "same-name sources share reel and timecode; use trusted checksums".into(),
                    );
                }
                let target = &moves[original];
                if target != original && media_identity(ffprobe, target)? != *expected {
                    return Err(format!(
                        "same-name media reel or source timecode mismatch: {target}"
                    )
                    .into());
                }
            }
        }
    }
    if let Some(trusted) = trusted {
        for move_ in requested {
            let target = &moves[&move_.original];
            let expected = trusted
                .get(&move_.original)
                .ok_or_else(|| format!("checksum manifest does not cover {}", move_.original))?;
            if media_hash(target, &stamps[target])? != *expected {
                return Err(format!("media SHA-256 mismatch: {target}").into());
            }
        }
    }
    Ok(VerifiedMoves {
        destinations: moves,
        stamps,
    })
}

fn checksums(path: &Path, project_sha: &str, paths: &Paths) -> Result<BTreeMap<String, String>> {
    let manifest: Manifest = plan(path)?;
    if manifest.schema != MANIFEST_SCHEMA || manifest.project_sha256 != project_sha {
        return Err("manifest version or trusted input project SHA-256 does not match".into());
    }
    let mut trusted = BTreeMap::new();
    for original in manifest.originals {
        if !paths.contains(&original.path)
            || original.sha256.len() != 64
            || hex::decode(&original.sha256).is_err()
            || trusted
                .insert(original.path, original.sha256.to_ascii_lowercase())
                .is_some()
        {
            return Err("invalid original, SHA-256 or duplicate path in manifest".into());
        }
    }
    Ok(trusted)
}

fn bindings(paths: &Paths, requested: &[Move]) -> Result<Destinations> {
    if requested.is_empty() || requested.len() > paths.len() {
        return Err("select at least one saved original to move".into());
    }
    let mut moves: Destinations = paths
        .iter()
        .map(|path| (path.clone(), path.clone()))
        .collect();
    let mut selected = BTreeSet::new();
    for move_ in requested {
        let old = original_path(&move_.original)?;
        let new = original_path(&move_.destination)?;
        if old.file_name() != new.file_name() || old == new || !selected.insert(old) {
            return Err(
                "each move must change only the folder of a distinct saved original".into(),
            );
        }
        *moves
            .get_mut(&move_.original)
            .ok_or("unknown original in move list")? = move_.destination.clone();
    }
    if moves.values().collect::<BTreeSet<_>>().len() != moves.len() {
        return Err("different originals cannot share a destination path".into());
    }
    Ok(moves)
}
