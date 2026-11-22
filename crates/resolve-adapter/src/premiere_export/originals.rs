//! Job-local physical proofs. A cached result never substitutes for checking
//! a new timeline's saved stream claims or its current file bindings.
mod bind;
pub(super) use bind::bind_originals;

use super::{mp3, red, still};
use crate::{
    linked::{self, MediaSpec, Profile},
    media::{check_wav_extent, media_digest, stamp, verify_group_binding, BindingSpec, FileStamp},
    Result, StreamSelector, VerifiedBinding,
};
use graph::Rational;
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

type Ordinary = (linked::Media, Rational, i64);

#[derive(Default)]
pub(super) struct VerifiedFiles {
    ordinary: HashMap<String, Arc<Ordinary>>,
    red: HashMap<String, Arc<red::Recording>>,
    stills: HashMap<String, Arc<still::Original>>,
    mp3: HashMap<String, Arc<mp3::Original>>,
}

impl VerifiedFiles {
    pub fn media(&self, locator: &str) -> Option<&linked::Media> {
        self.ordinary.get(locator).map(|binding| &binding.0)
    }

    pub fn recheck(&self) -> Result<()> {
        for (locator, binding) in &self.ordinary {
            let media = &binding.0;
            if fs::canonicalize(locator)? != media.path {
                return Err(format!("Resolve original locator changed: {locator}").into());
            }
            media.recheck()?;
        }
        for (locator, recording) in &self.red {
            recording.recheck_at(Path::new(locator))?;
        }
        for (locator, still) in &self.stills {
            still.recheck(Path::new(locator))?;
        }
        for (locator, original) in &self.mp3 {
            original.recheck(Path::new(locator))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProfileRequest {
    Linked,
    Camera,
    Avid,
    Red,
    Still(still::Format),
    Mp3,
    LegacyGroup,
}

enum Proof {
    Ordinary(Arc<Ordinary>),
    Red(Arc<red::Recording>),
    Still(Arc<still::Original>),
    Mp3(Arc<mp3::Original>),
    Legacy(Arc<LegacyOriginal>),
}

struct LegacyOriginal {
    binding: VerifiedBinding,
    identity: FileStamp,
}

struct Entry {
    profile: ProfileRequest,
    proof: std::result::Result<Proof, String>,
    reuse_count: usize,
}

pub(super) struct Originals<'a> {
    tool: &'a Path,
    announce: bool,
    locators: HashMap<String, PathBuf>,
    entries: BTreeMap<PathBuf, Entry>,
}

#[derive(Debug)]
pub(super) struct OriginalFailure {
    pub locator: String,
    pub owner: String,
    detail: String,
}

impl fmt::Display for OriginalFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.owner, self.locator, self.detail)
    }
}

impl std::error::Error for OriginalFailure {}

#[derive(serde::Serialize)]
pub(super) struct OriginalReview<'a> {
    locator: Cow<'a, str>,
    reuse_count: usize,
    verified: bool,
    diagnostic: Option<&'a str>,
}

impl<'a> Originals<'a> {
    pub fn new(tool: &'a Path, announce: bool) -> Self {
        Self {
            tool,
            announce,
            locators: HashMap::new(),
            entries: BTreeMap::new(),
        }
    }

    pub fn reviews(&self) -> impl Iterator<Item = OriginalReview<'_>> {
        self.entries.iter().map(|(locator, entry)| OriginalReview {
            locator: locator.to_string_lossy(),
            reuse_count: entry.reuse_count,
            verified: entry.proof.is_ok(),
            diagnostic: entry.proof.as_ref().err().map(String::as_str),
        })
    }

    fn lookup(
        &mut self,
        locator: &str,
        owner: &str,
        profile: ProfileRequest,
        inspect: impl FnOnce(&Path) -> Result<Proof>,
    ) -> Result<&Proof> {
        let fail = |detail: String| OriginalFailure {
            locator: locator.to_owned(),
            owner: owner.to_owned(),
            detail,
        };
        let saved = Path::new(locator);
        if !saved.is_absolute() {
            return Err(fail("linked media binding needs an absolute path".into()).into());
        }
        let key = self.locators.entry(locator.into()).or_insert_with(|| {
            if profile == ProfileRequest::Red {
                // A RED original's identity includes its sibling paths. A
                // symlink to just part one does not establish that family.
                saved
                    .parent()
                    .and_then(|parent| fs::canonicalize(parent).ok())
                    .zip(saved.file_name())
                    .map(|(parent, name)| parent.join(name))
            } else {
                fs::canonicalize(saved).ok()
            }
            .unwrap_or_else(|| saved.to_owned())
        });
        let entry = match self.entries.entry(key.clone()) {
            std::collections::btree_map::Entry::Occupied(row) => {
                let entry = row.into_mut();
                if entry.profile != profile {
                    return Err(
                        fail("conflicting verification profiles for one original".into()).into(),
                    );
                }
                entry.reuse_count += 1;
                entry
            }
            std::collections::btree_map::Entry::Vacant(row) => {
                if self.announce {
                    eprintln!("Resolve project Review · verifying {}", key.display());
                }
                row.insert(Entry {
                    profile,
                    proof: inspect(saved).map_err(|error| error.to_string()),
                    reuse_count: 0,
                })
            }
        };
        Ok(entry.proof.as_ref().map_err(|error| fail(error.clone()))?)
    }

    pub fn ordinary(
        &mut self,
        locator: &str,
        owner: &str,
        premiere: bool,
        camera: bool,
    ) -> Result<Arc<Ordinary>> {
        let profile = if camera {
            ProfileRequest::Camera
        } else if premiere {
            ProfileRequest::Linked
        } else {
            ProfileRequest::Avid
        };
        let tool = self.tool;
        let Proof::Ordinary(binding) = self.lookup(locator, owner, profile, |path| {
            let spec = MediaSpec {
                item_id: String::new(),
                bin_id: String::new(),
                path: path.to_owned(),
                sha256: String::new(),
                streams: vec![],
            };
            let binding = if premiere || camera {
                linked::probe_resolve_original(&spec, tool, camera)?
            } else {
                linked::probe_resolve_original_by_path(&spec, tool)?
            };
            if let Profile::Wav {
                bits,
                channels,
                samples,
            } = binding.0.profile
            {
                check_wav_extent(path, bits, channels, samples)?;
            }
            Ok(Proof::Ordinary(Arc::new(binding)))
        })?
        else {
            unreachable!("ordinary proof kind")
        };
        Ok(Arc::clone(binding))
    }

    pub fn red(&mut self, locator: &str, owner: &str) -> Result<Arc<red::Recording>> {
        let Proof::Red(recording) = self.lookup(locator, owner, ProfileRequest::Red, |path| {
            Ok(Proof::Red(Arc::new(red::Recording::open(path)?)))
        })?
        else {
            unreachable!("RED proof kind")
        };
        Ok(Arc::clone(recording))
    }

    pub fn still(&mut self, locator: &str, owner: &str, format: still::Format) -> Result<Arc<still::Original>> {
        let tool = self.tool;
        let Proof::Still(still) = self.lookup(locator, owner, ProfileRequest::Still(format), |path| {
            Ok(Proof::Still(Arc::new(still::Original::open(path, tool, format)?)))
        })? else {
            unreachable!("still proof kind")
        };
        Ok(Arc::clone(still))
    }

    pub fn mp3(&mut self, locator: &str, owner: &str) -> Result<Arc<mp3::Original>> {
        let tool = self.tool;
        let Proof::Mp3(original) = self.lookup(locator, owner, ProfileRequest::Mp3, |path| {
            Ok(Proof::Mp3(Arc::new(mp3::Original::open(path, tool)?)))
        })? else {
            unreachable!("MP3 proof kind")
        };
        Ok(Arc::clone(original))
    }

    pub fn legacy_camera(
        &mut self,
        locator: &str,
        item: &str,
        streams: [StreamSelector; 3],
    ) -> Result<Arc<VerifiedBinding>> {
        let tool = self.tool;
        let Proof::Legacy(legacy) =
            self.lookup(locator, item, ProfileRequest::LegacyGroup, |path| {
                let canonical = fs::canonicalize(path)?;
                let identity = stamp(fs::metadata(&canonical)?)?;
                let digest = media_digest(&canonical).map_err(|error| {
                    format!("could not read camera original {locator}: {error}")
                })?;
                let binding = verify_group_binding(
                    BindingSpec {
                        item_id: item.into(),
                        path: canonical.clone(),
                        sha256: hex::encode(digest),
                        streams: std::array::from_fn(|i| StreamSelector {
                            media_id: streams[i].media_id.clone(),
                            index: streams[i].index,
                            channel: streams[i].channel,
                        }),
                    },
                    tool,
                )
                .map_err(|error| format!("camera original verification failed: {error}"))?;
                if fs::canonicalize(path)? != canonical
                    || stamp(fs::metadata(&canonical)?)? != identity
                {
                    return Err("OP1a original changed during verification".into());
                }
                Ok(Proof::Legacy(Arc::new(LegacyOriginal {
                    binding,
                    identity,
                })))
            })?
        else {
            unreachable!("legacy camera proof kind")
        };
        let b = &legacy.binding;
        Ok(Arc::new(VerifiedBinding {
            item_id: item.into(),
            path: b.path.clone(),
            streams,
            frames: b.frames,
            timecode_frames: b.timecode_frames,
            source_package: b.source_package,
            modified_micros: b.modified_micros,
            tagged_rec709: b.tagged_rec709,
            digest: b.digest,
            atom: None,
        }))
    }

    pub fn recheck_legacy(&self, locator: &str) -> Result<()> {
        let key = self
            .locators
            .get(locator)
            .ok_or("missing verified OP1a locator")?;
        let Some(Ok(Proof::Legacy(legacy))) = self.entries.get(key).map(|entry| &entry.proof)
        else {
            return Err("missing verified OP1a proof".into());
        };
        let path = &legacy.binding.path;
        if fs::canonicalize(locator)? != *path || stamp(fs::metadata(path)?)? != legacy.identity {
            return Err(format!("verified OP1a original changed: {locator}").into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_proof_is_reused_through_aliases_with_the_current_source_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.mov");
        fs::write(&path, b"incomplete").unwrap();
        fs::create_dir(dir.path().join("child")).unwrap();
        let alias = dir.path().join("child/../original.mov");
        let mut originals = Originals::new(Path::new("unneeded"), false);
        let first = originals
            .lookup(
                path.to_str().unwrap(),
                "first",
                ProfileRequest::Linked,
                |_| Err("short packet extent".into()),
            )
            .err()
            .unwrap();
        let second = originals
            .lookup(
                alias.to_str().unwrap(),
                "second",
                ProfileRequest::Linked,
                |_| panic!("an alias must reuse the original's verification result"),
            )
            .err()
            .unwrap();
        assert!(first.to_string().ends_with("short packet extent"));
        let failure = second.downcast_ref::<OriginalFailure>().unwrap();
        assert_eq!(failure.owner, "second");
        assert_eq!(failure.locator, alias.to_str().unwrap());
        let rows: Vec<_> = originals.reviews().collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reuse_count, 1);
        assert!(!rows[0].verified);
    }

    #[test]
    fn a_verified_original_is_reused_but_a_later_file_change_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.wav");
        fs::write(&path, b"sample").unwrap();
        let canonical = fs::canonicalize(&path).unwrap();
        let locator = path.to_str().unwrap();
        let identity = stamp(fs::metadata(&path).unwrap()).unwrap();
        let mut originals = Originals::new(Path::new("unneeded"), false);
        let original = originals
            .lookup(locator, "clip", ProfileRequest::Linked, |_| {
                Ok(Proof::Ordinary(Arc::new((
                    linked::Media {
                        name: String::new(),
                        bin: 0,
                        path: canonical,
                        profile: Profile::Wav {
                            bits: 16,
                            channels: 1,
                            samples: 1,
                        },
                        modified_micros: 0,
                        calendar_seconds: None,
                        identity: linked::MediaIdentity::File(identity),
                        proxy: None,
                        native: None,
                    },
                    Rational::new(1, 48000).unwrap(),
                    1,
                ))))
            })
            .unwrap();
        let Proof::Ordinary(first) = original else {
            panic!("expected PCM proof")
        };
        let first = Arc::clone(first);
        let second = originals
            .ordinary(locator, "other clip", true, false)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let files = VerifiedFiles {
            ordinary: [(locator.into(), second)].into(),
            ..VerifiedFiles::default()
        };
        files.recheck().unwrap();
        fs::write(path, b"replacement").unwrap();
        assert!(files.recheck().unwrap_err().to_string().contains("changed"));
    }
}
