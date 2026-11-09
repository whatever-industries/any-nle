//! Read-only, identity-based media lookup. Detailed headers are retained only for referenced mobs.
use avid_reader::mxf::{self, MxfIdentity};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct MediaIndex {
    pub files: HashMap<String, PathBuf>,
    pub identities: HashMap<PathBuf, MxfIdentity>,
    pub ambiguous: Vec<String>,
    pub unreadable: Vec<(PathBuf, String)>,
    pub files_scanned: usize,
}
impl MediaIndex {
    pub fn scan(roots: &[PathBuf], wanted: &HashSet<String>) -> Result<Self, String> {
        Self::scan_controlled(roots, wanted, &mut || Ok(()))
    }
    pub fn scan_controlled(
        roots: &[PathBuf],
        wanted: &HashSet<String>,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        let mut out = Self::default();
        let mut seen = HashMap::<String, Option<PathBuf>>::new();
        let mut roots_seen = HashSet::new();
        let mut files_seen = HashSet::new();
        let mut retained_budget = super::Budget::default();
        for root in roots {
            check()?;
            let root =
                std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
            if !roots_seen.insert(root.clone()) {
                continue;
            }
            for path in mxf::find_mxf_files(&root, 6, 200_000).map_err(|e| e.to_string())? {
                check()?;
                if !files_seen.insert(path.clone()) {
                    continue;
                }
                out.files_scanned += 1;
                match mxf::read_identity(&path) {
                    Ok(mut id) => {
                        id.file_packages.sort_unstable();
                        id.file_packages.dedup();
                        let retained = id.file_packages.iter().any(|x| wanted.contains(x));
                        for uid in &id.file_packages {
                            // Keep legacy scan diagnostics even for unselected media.
                            // Only referenced files retain detailed header facts below.
                            match seen.entry(uid.clone()) {
                                std::collections::hash_map::Entry::Vacant(e) => {
                                    e.insert(Some(path.clone()));
                                }
                                std::collections::hash_map::Entry::Occupied(mut e) => {
                                    e.insert(None);
                                }
                            }
                        }
                        if retained {
                            retained_budget.charge(
                                std::mem::size_of::<MxfIdentity>() + path.as_os_str().len(),
                            )?;
                            for strings in [
                                &id.material_packages,
                                &id.source_packages,
                                &id.file_packages,
                            ] {
                                retained_budget
                                    .charge(strings.capacity() * std::mem::size_of::<String>())?;
                                for s in strings {
                                    retained_budget.charge(s.capacity())?;
                                }
                            }
                            retained_budget.charge(
                                id.timecodes.capacity() * std::mem::size_of::<(u64, u16, bool)>()
                                    + id.file_timecodes.capacity()
                                        * std::mem::size_of::<(u64, u16, bool)>()
                                    + id.file_tracks.capacity()
                                        * std::mem::size_of::<(i64, i64, i64)>()
                                    + id.qualified_file_clocks.capacity()
                                        * std::mem::size_of::<avid_reader::mxf::QualifiedClock>(),
                            )?;
                            for c in &id.qualified_file_clocks {
                                retained_budget.charge(
                                    c.source_package.capacity()
                                        + c.timecode_track.capacity()
                                        + c.route.capacity(),
                                )?;
                            }
                            out.identities.insert(path, id);
                        }
                    }
                    Err(e) => out.unreadable.push((path, e.to_string())),
                }
            }
        }
        for (uid, path) in seen {
            match path {
                Some(p) => {
                    if wanted.contains(&uid) {
                        out.files.insert(uid, p);
                    }
                }
                None => out.ambiguous.push(uid),
            }
        }
        out.ambiguous.sort();
        Ok(out)
    }
    pub fn identity(&self, path: &Path) -> Result<&MxfIdentity, String> {
        self.identities
            .get(path)
            .ok_or_else(|| format!("missing retained MXF identity for {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unreferenced_duplicate_identity_keeps_legacy_scan_warning_without_header_retention() {
        let root = std::env::temp_dir().join(format!(
            "anynle-index-diagnostics-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        fn set(class: u8, tag: u16) -> Vec<u8> {
            let mut bytes = vec![6, 14, 43, 52, 2, 83, 1, 1, 13, 1, 1, 1, 1, 1, class, 0, 36];
            bytes.extend_from_slice(&tag.to_be_bytes());
            bytes.extend_from_slice(&32u16.to_be_bytes());
            bytes.extend_from_slice(&[0xBB; 32]);
            bytes
        }
        let mut body = vec![
            6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1, 5, 1, 0, 8, 0, 0, 0, 0, 0, 0, 0, 18,
        ];
        body.extend(set(0x37, 0x4401));
        body.extend(set(0x23, 0x2701));
        let mut bytes = vec![6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1, 2, 4, 0, 88];
        let mut pack = [0u8; 88];
        pack[32..40].copy_from_slice(&(body.len() as u64).to_be_bytes());
        bytes.extend(pack);
        bytes.extend(body);
        std::fs::write(root.join("one.mxf"), &bytes).unwrap();
        std::fs::write(root.join("two.MXF"), &bytes).unwrap();
        let index = MediaIndex::scan(&[root.clone(), root.clone()], &HashSet::new()).unwrap();
        let legacy = avid_reader::model::MediaIndex::scan_all(&[&root]).unwrap();
        assert_eq!(index.files_scanned, 2);
        assert!(index.unreadable.is_empty());
        assert_eq!(index.ambiguous, legacy.ambiguous);
        assert_eq!(index.ambiguous, vec!["BB".repeat(32)]);
        assert!(index.files.is_empty());
        assert!(index.identities.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
