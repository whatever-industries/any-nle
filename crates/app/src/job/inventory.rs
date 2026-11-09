//! Read-only catalog. One decoded bin is live at a time. File presence/identity is not inferred.
use super::{BinEntry, CancelToken, EffectCapability, EffectMode, Inventory, JobSpec, Progress};
use avid_reader::bin_nav::BinNavigator;
use avid_reader::file::{AvbFile, Object};
use avid_reader::objects::{Node, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

const MAX_BINS: usize = 50_000;
const MAX_ENTRIES: usize = 500_000;
const MAX_DEPTH: usize = 64;
const MAX_ROUTE_VISITS: usize = 20_000_000;
const MAX_FACT_BYTES: usize = 64 << 20;
fn charge(total: &mut usize, bytes: usize) -> Result<(), String> {
    *total = total
        .checked_add(bytes)
        .filter(|n| *n <= MAX_FACT_BYTES)
        .ok_or("Inventory facts exceed the 64 MiB budget.")?;
    Ok(())
}
pub(crate) const PRESENCE_PENDING: &str =
    "Media presence is not checked by the inventory; run pre-flight.";
fn is_bin(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("avb"))
}
fn excluded(p: &Path) -> bool {
    p.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("trash") || s.eq_ignore_ascii_case("attic"))
    })
}
fn checked_file(p: &Path) -> Result<PathBuf, String> {
    let m = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if m.file_type().is_symlink() || !m.is_file() {
        return Err(format!(
            "{}: bins must be regular files, not symbolic links",
            p.display()
        ));
    }
    if !is_bin(p) {
        return Err(format!("{}: choose an .avb bin", p.display()));
    }
    std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}
fn collect(spec: &JobSpec, cancel: &CancelToken) -> Result<Vec<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    if let Some(root) = &spec.project_folder {
        let m = std::fs::symlink_metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err("Project folder must be a directory, not a symbolic link.".into());
        }
        let mut stack = vec![(root.clone(), 0)];
        let mut visited = 0;
        while let Some((dir, depth)) = stack.pop() {
            cancel.check()?;
            if depth > MAX_DEPTH {
                return Err("Project folder exceeds the 64-level traversal limit.".into());
            }
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                cancel.check()?;
                visited += 1;
                if visited > MAX_ENTRIES {
                    return Err("Project folder exceeds the 500000-entry traversal limit.".into());
                }
                entries.push(entry.map_err(|e| e.to_string())?);
            }
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                cancel.check()?;
                let p = entry.path();
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink()
                    || (!spec.include_trash_attic && excluded(p.strip_prefix(root).unwrap_or(&p)))
                {
                    continue;
                }
                if kind.is_dir() {
                    stack.push((p, depth + 1));
                } else if kind.is_file() && is_bin(&p) {
                    paths.insert(checked_file(&p)?);
                    if paths.len() > MAX_BINS {
                        return Err("Project exceeds the 50000-bin catalog limit.".into());
                    }
                }
            }
        }
    }
    for p in &spec.bins {
        cancel.check()?;
        let p = checked_file(p)?;
        if !spec.include_trash_attic
            && spec
                .project_folder
                .as_ref()
                .is_some_and(|root| p.strip_prefix(root).ok().is_some_and(excluded))
        {
            continue;
        }
        paths.insert(p);
        if paths.len() > MAX_BINS {
            return Err("Selection exceeds the 50000-bin catalog limit.".into());
        }
    }
    if paths.is_empty() {
        return Err("No Avid bins found in the selection.".into());
    }
    Ok(paths.into_iter().collect())
}
#[derive(Default)]
struct Routes {
    refs: HashMap<String, BTreeSet<String>>,
    masters: HashMap<String, BTreeSet<usize>>,
    roots: BTreeSet<String>,
    incomplete: bool,
}
fn mob_id(n: &Node) -> Option<String> {
    match n.get("mob_id") {
        Some(Value::MobId(id)) => Some(id.urn()),
        _ => None,
    }
}
fn source_refs(
    file: &AvbFile,
    start: u32,
    visits: &mut usize,
    cancel: &CancelToken,
) -> Result<(BTreeSet<String>, bool), String> {
    let (mut refs, mut seen, mut stack) = (BTreeSet::new(), HashSet::new(), vec![start]);
    let mut incomplete = false;
    while let Some(id) = stack.pop() {
        cancel.check()?;
        if seen.len() > 500_000 || stack.len() > 500_000 {
            return Err("One dependency route exceeds the 500000-object budget.".into());
        }
        if !seen.insert(id) {
            continue;
        }
        *visits += 1;
        if *visits > MAX_ROUTE_VISITS {
            return Err("Dependency graph exceeds the bounded traversal budget.".into());
        }
        let Some(n) = file.node(id) else {
            incomplete = true;
            continue;
        };
        if n.class == *b"SCLP" {
            if let Some(id) = mob_id(n) {
                refs.insert(id);
            }
            continue;
        }
        // Attributes/markers/descriptors cannot establish sequence source dependencies.
        for (field, v) in &n.fields {
            if matches!(
                *field,
                "attributes" | "descriptor" | "physical_media" | "locator"
            ) {
                continue;
            }
            match v {
                Value::Ref(i) => stack.push(*i),
                Value::RefList(items) | Value::List(items) => {
                    for v in items {
                        match v {
                            Value::Ref(i) => stack.push(*i),
                            Value::Node(n) => stack.extend(avid_reader::graph::direct_refs(n)),
                            _ => {}
                        }
                    }
                }
                Value::Node(n) => stack.extend(avid_reader::graph::direct_refs(n)),
                _ => {}
            }
        }
    }
    Ok((refs, incomplete))
}
fn original_locator(file: &AvbFile, descriptor: &Node) -> Option<String> {
    let mut current = Some(descriptor);
    let mut seen = HashSet::new();
    for _ in 0..8 {
        let node = current?;
        if let Some(locator) = node
            .get("locator")
            .and_then(Value::as_index)
            .and_then(|i| file.node(i))
            .filter(|n| n.class == *b"FILE")
        {
            for field in ["path_posix", "path_utf8", "path"] {
                if let Some(path) = locator
                    .get(field)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    return Some(path.to_owned());
                }
            }
        }
        let index = node.get("physical_media").and_then(Value::as_index)?;
        if !seen.insert(index) {
            return None;
        }
        current = file.node(index).filter(|n| n.class == *b"MDES");
    }
    None
}
fn effect(key: &str, count: usize) -> EffectCapability {
    let (label, mode, why) = match key {
        "EFF2_RGB_COLOR_CORRECTION" => (
            "Color Correction",
            EffectMode::Approximate,
            "The current qualified approximation writes a LUT; editable color controls are not offered.",
        ),
        "EFF2_BLEND_RESIZE" => (
            "Resize",
            EffectMode::Approximate,
            "Supported transform controls translate; unsupported controls and timing are reported.",
        ),
        "EFF2_SBLEND" => (
            "3D Warp / blend",
            EffectMode::Approximate,
            "Only the qualified 3D Warp subset translates; other blend controls are reported.",
        ),
        "EFF_ANIMATTE" => (
            "AniMatte",
            EffectMode::Approximate,
            "Qualified shape data translates; tracking, easing and unsupported geometry are reported.",
        ),
        "SPED" => (
            "Motion Effect",
            EffectMode::Approximate,
            "Constant picture speed and reverse translate; ramps and unsupported timing are reported.",
        ),
        "TNFX" => (
            "Transition",
            EffectMode::Approximate,
            "Only supported transition types translate; audio curves may be approximated and are reported.",
        ),
        "PVOL" => (
            "Audio volume / pan",
            EffectMode::Approximate,
            "Supported levels and pan translate; unsupported automation and channel combinations are reported.",
        ),
        "EFF2_AUDIO_CHANNEL_COMBINER" => (
            "Audio channel combiner",
            EffectMode::Convert,
            "Qualified declared channel groups translate; incomplete routing refuses preparation.",
        ),
        _ => (
            key,
            EffectMode::Skip,
            "No qualified translation is offered for this effect type; its details remain in the report.",
        ),
    };
    EffectCapability {
        key: key.into(),
        label: label.into(),
        count,
        modes: vec![mode],
        explanation: why.into(),
    }
}
fn dependency_bins(routes: &Routes, selected: &BTreeSet<usize>) -> (BTreeSet<usize>, usize) {
    let mut bins = BTreeSet::new();
    let mut unresolved = 0;
    let mut seen = HashSet::new();
    let mut pending = routes.roots.iter().cloned().collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(owners) = routes.masters.get(&id) {
            if !owners.iter().any(|b| selected.contains(b)) {
                if let Some(first) = owners.first() {
                    bins.insert(*first);
                }
            }
        }
        if let Some(children) = routes.refs.get(&id) {
            pending.extend(children.iter().cloned());
        } else if !routes.masters.contains_key(&id) {
            unresolved += 1;
        }
    }
    (bins, unresolved)
}
/// Catalog all project bins so a deselected bin can be shown when a selected sequence needs it.
/// This is not physical media qualification; `prepare` replaces candidate counts after probing.
pub fn inventory(
    spec: &JobSpec,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(Progress),
) -> Result<Inventory, String> {
    if spec.schema != "anynle.job/1" {
        return Err("Unsupported job schema.".into());
    }
    let paths = collect(spec, cancel)?;
    let selected_paths = spec
        .bins
        .iter()
        .map(|p| checked_file(p))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let selected = paths
        .iter()
        .enumerate()
        .filter(|(_, p)| spec.entire_project || selected_paths.contains(*p))
        .map(|(i, _)| i)
        .collect::<BTreeSet<_>>();
    let mut inv = Inventory::default();
    let mut routes = Routes::default();
    let mut visits = 0;
    let mut fact_bytes = 0;
    let (mut native, mut originals, mut effects, mut media) = (
        HashSet::new(),
        HashSet::new(),
        BTreeMap::<String, usize>::new(),
        HashSet::new(),
    );
    for (bin, path) in paths.iter().enumerate() {
        cancel.check()?;
        progress(Progress {
            stage: "Inventory".into(),
            current_bin: Some(path.clone()),
            completed: bin,
            total: paths.len(),
            message: "Reading one bin at a time; originals stay in place.".into(),
        });
        charge(&mut fact_bytes, 512 + 2 * path.as_os_str().len() + 64)?;
        let pins = crate::source_input::pin_checked(&[path.clone()], &mut || cancel.check())?;
        let file = AvbFile::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let nav = BinNavigator::new(&file).map_err(|e| format!("{}: {e:?}", path.display()))?;
        let masters = nav
            .mastermobs()
            .map_err(|e| format!("{}: {e:?}", path.display()))?;
        let sequences = nav
            .toplevel()
            .map_err(|e| format!("{}: {e:?}", path.display()))?;
        let relative = spec
            .project_folder
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(path.file_name().unwrap_or_default()));
        inv.bins.push(BinEntry {
            path: path.clone(),
            relative,
            clips: masters.len(),
            sequences: sequences.len(),
            dependency: false,
        });
        for master in &masters {
            if let Some(id) = mob_id(master.mob) {
                charge(&mut fact_bytes, 256 + id.len())?;
                routes.masters.entry(id).or_default().insert(bin);
            }
        }
        if selected.contains(&bin) {
            for seq in &sequences {
                if let Some(id) = mob_id(seq.mob) {
                    routes.roots.insert(id);
                }
            }
        }
        for (index, obj) in file.objects.iter().enumerate() {
            cancel.check()?;
            let Some(Object::Decoded(node)) = obj else {
                continue;
            };
            if node.class == *b"CMPO" && node.get("mob_type_id").and_then(Value::as_int) == Some(1)
            {
                if let Some(id) = mob_id(node) {
                    let (refs, incomplete) = source_refs(&file, index as u32, &mut visits, cancel)?;
                    charge(
                        &mut fact_bytes,
                        256 + id.len() + refs.iter().map(|s| 128 + s.len()).sum::<usize>(),
                    )?;
                    routes.incomplete |= incomplete;
                    routes.refs.entry(id).or_default().extend(refs);
                }
            }
        }
        if selected.contains(&bin) {
            let contents = avid_reader::extract::extract(&file);
            let mob_nodes = file
                .objects
                .iter()
                .filter_map(|o| match o {
                    Some(Object::Decoded(n)) if n.class == *b"CMPO" => mob_id(n).map(|id| (id, n)),
                    _ => None,
                })
                .collect::<HashMap<_, _>>();
            for master in &contents.masters {
                if let Some(rgb) = master.color {
                    let hex = prproj::convert::colors::rgb16_hex(rgb);
                    if !inv.clip_colors.contains(&hex) {
                        charge(&mut fact_bytes, 128 + hex.len())?;
                        inv.clip_colors.push(hex);
                    }
                }
                for (_, _, m) in &master.markers {
                    if !m.color.is_empty() && !inv.marker_colors.contains(&m.color) {
                        charge(&mut fact_bytes, 128 + m.color.len())?;
                        inv.marker_colors.push(m.color.clone());
                    }
                }
                for track in &master.tracks {
                    let id = track.file_mob.urn();
                    if media.insert(id.clone()) {
                        charge(&mut fact_bytes, 128 + id.len())?;
                    }
                    let locator = mob_nodes
                        .get(&id)
                        .and_then(|n| n.get("descriptor"))
                        .and_then(Value::as_index)
                        .and_then(|i| file.node(i))
                        .and_then(|d| original_locator(&file, d));
                    if let Some(p) = locator {
                        if originals.insert(p.clone()) {
                            charge(&mut fact_bytes, 128 + p.len())?;
                        }
                    } else {
                        native.insert(id);
                    }
                }
            }
            for obj in &file.objects {
                let Some(Object::Decoded(n)) = obj else {
                    continue;
                };
                let key = match &n.class {
                    b"TKFX" => n.get("effect_id").and_then(Value::as_str),
                    b"SPED" => Some("SPED"),
                    b"TNFX" => Some("TNFX"),
                    b"PVOL" => Some("PVOL"),
                    _ => None,
                };
                if let Some(key) = key {
                    if !effects.contains_key(key) {
                        charge(&mut fact_bytes, 256 + key.len())?;
                    }
                    *effects.entry(key.into()).or_default() += 1;
                }
            }
            if !contents.skipped.is_empty() {
                inv.notes.push(format!("{}: {} bin items need source-interpretation review; counts include stored bin entries.",relative_name(path),contents.skipped.len()));
            }
        }
        crate::source_input::recheck_checked(&pins, &mut || cancel.check())?;
        inv.bin_revisions
            .push((path.clone(), pins[0].revision.clone()));
        // file and all borrowed nodes are dropped here, before opening the next bin.
    }
    if spec.include_dependencies {
        let (dependencies, unresolved) = dependency_bins(&routes, &selected);
        for i in dependencies {
            inv.bins[i].dependency = true;
        }
        if unresolved > 0 {
            inv.notes.push(format!("{unresolved} sequence source routes have no cataloged master; pre-flight must resolve or report them."));
        }
        if routes.incomplete {
            inv.notes.push("Some dependency routes cross opaque or dangling object references; complete dependency closure is not established.".into());
        }
    }
    inv.total_media = media.len();
    inv.native_media = native.len();
    inv.ume_originals = originals.len();
    inv.found_media = 0;
    inv.effects = effects.into_iter().map(|(k, n)| effect(&k, n)).collect();
    inv.notes.push(PRESENCE_PENDING.into());
    if inv.bins.iter().any(|b| b.dependency) {
        inv.notes.push("Media and effect candidate totals cover explicit selections; dependent bins are included and fully checked during pre-flight.".into());
    }
    inv.notes.push("Native and linked-original counts are descriptor candidates until physical identity and stream qualification complete in pre-flight.".into());
    progress(Progress {
        stage: "Inventory".into(),
        current_bin: None,
        completed: paths.len(),
        total: paths.len(),
        message:
            "Bin catalog complete. Media presence and route qualification remain for pre-flight."
                .into(),
    });
    Ok(inv)
}
fn relative_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
/// Engine selection uses the same cataloged dependency facts reviewed by the UI.
pub(super) fn selected_paths(spec: &JobSpec, inv: &Inventory) -> Result<Vec<PathBuf>, String> {
    let explicit = spec
        .bins
        .iter()
        .map(|p| checked_file(p))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let paths = inv
        .bins
        .iter()
        .filter(|b| {
            spec.entire_project
                || explicit.contains(&b.path)
                || (spec.include_dependencies && b.dependency)
        })
        .map(|b| b.path.clone())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        Err("Select at least one bin.".into())
    } else {
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "anynle-inventory-{}-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn catalog_is_sorted_deduplicated_and_excludes_only_exact_trash_attic_components() {
        let t = Temp::new();
        for dir in ["Trash", "Attic", "Attic notes", "sub"] {
            std::fs::create_dir(t.0.join(dir)).unwrap();
        }
        for p in [
            "z.AVB",
            "sub/a.avb",
            "Trash/x.avb",
            "Attic/y.avb",
            "Attic notes/b.avb",
            "not.avb.lck",
        ] {
            std::fs::write(t.0.join(p), b"synthetic not decoded").unwrap();
        }
        let spec = JobSpec {
            project_folder: Some(t.0.clone()),
            bins: vec![t.0.join("z.AVB")],
            ..Default::default()
        };
        let paths = collect(&spec, &CancelToken::default()).unwrap();
        assert_eq!(paths.len(), 3);
        assert!(paths.windows(2).all(|w| w[0] < w[1]));
        let spec = JobSpec {
            include_trash_attic: true,
            ..spec
        };
        assert_eq!(collect(&spec, &CancelToken::default()).unwrap().len(), 5);
    }
    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_not_followed_and_explicit_links_refuse() {
        let t = Temp::new();
        std::fs::write(t.0.join("a.avb"), b"synthetic").unwrap();
        std::os::unix::fs::symlink(&t.0, t.0.join("loop")).unwrap();
        std::os::unix::fs::symlink(t.0.join("a.avb"), t.0.join("alias.avb")).unwrap();
        let spec = JobSpec {
            project_folder: Some(t.0.clone()),
            ..Default::default()
        };
        assert_eq!(collect(&spec, &CancelToken::default()).unwrap().len(), 1);
        assert!(checked_file(&t.0.join("alias.avb")).is_err());
    }
    #[test]
    fn source_fact_budget_refuses_before_overflow() {
        let mut bytes = MAX_FACT_BYTES - 1;
        charge(&mut bytes, 1).unwrap();
        assert!(charge(&mut bytes, 1).is_err());
        let mut bytes = 0;
        assert!(charge(&mut bytes, usize::MAX).is_err());
    }
    #[test]
    fn cancellation_stops_before_decoding() {
        let t = Temp::new();
        let c = CancelToken::default();
        c.cancel();
        assert!(collect(
            &JobSpec {
                project_folder: Some(t.0.clone()),
                ..Default::default()
            },
            &c
        )
        .unwrap_err()
        .contains("cancelled"));
    }
    #[test]
    fn dependencies_follow_ids_through_subclips_and_choose_one_original_bin() {
        let mut routes = Routes::default();
        routes.roots.insert("sequence".into());
        routes
            .refs
            .insert("sequence".into(), ["subclip".into()].into());
        routes
            .refs
            .insert("subclip".into(), ["master".into(), "missing".into()].into());
        routes.masters.insert("master".into(), [2, 3].into());
        assert_eq!(dependency_bins(&routes, &[0].into()), ([2].into(), 1));
        assert_eq!(
            dependency_bins(&routes, &[0, 3].into()),
            (BTreeSet::new(), 1)
        );
    }
    #[test]
    fn synthetic_project_catalog_counts_cross_bin_sources_and_preserves_inputs() {
        use avid_reader::document::{AvbDocument, MobKind};
        use avid_reader::io::MobId;
        let t = Temp::new();
        let master_id=MobId::from_urn("urn:smpte:umid:060a2b34.01010101.01010f20.13010203.00000000.00000000.00000000.00000001").unwrap();
        let seq_id=MobId::from_urn("urn:smpte:umid:060a2b34.01010101.01010f20.13010203.00000000.00000000.00000000.00000002").unwrap();
        let mut masters = AvbDocument::new_empty_bin(101, "Synthetic", 0, false);
        let master = masters
            .create_composition("Synthetic master", MobKind::Master, master_id, 25, 0)
            .unwrap();
        masters.add_bin_item(master).unwrap();
        let mut sequence = AvbDocument::new_empty_bin(102, "Synthetic", 0, false);
        let source = sequence
            .create_source_clip(25., 1, 25, master_id, 1, 0)
            .unwrap();
        let seq = sequence
            .create_composition("Synthetic sequence", MobKind::Composition, seq_id, 25, 0)
            .unwrap();
        sequence.append_track(seq, 1, source).unwrap();
        sequence.add_bin_item(seq).unwrap();
        let a = t.0.join("a-sequence.avb");
        let b = t.0.join("b-master.avb");
        sequence.write_new(&a).unwrap();
        masters.write_new(&b).unwrap();
        let before = [std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()];
        let spec = JobSpec {
            project_folder: Some(t.0.clone()),
            bins: vec![a.clone()],
            output: t.0.join("never-created.prproj"),
            ..Default::default()
        };
        let mut progress = Vec::new();
        let inv = inventory(&spec, &CancelToken::default(), &mut |p| progress.push(p)).unwrap();
        assert_eq!((inv.bins[0].clips, inv.bins[0].sequences), (0, 1));
        assert_eq!((inv.bins[1].clips, inv.bins[1].sequences), (1, 0));
        assert!(inv.bins[1].dependency);
        assert_eq!(
            selected_paths(&spec, &inv).unwrap(),
            vec![checked_file(&a).unwrap(), checked_file(&b).unwrap()]
        );
        assert!(inv.notes.iter().any(|n| n == PRESENCE_PENDING));
        assert_eq!(inv.found_media, 0);
        assert_eq!(progress.last().unwrap().completed, 2);
        assert_eq!(
            before,
            [std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()]
        );
        assert!(!spec.output.exists());
    }
    #[test]
    fn selection_includes_dependencies_without_importing_other_bins() {
        let t = Temp::new();
        for n in ["a.avb", "b.avb", "c.avb"] {
            std::fs::write(t.0.join(n), b"synthetic").unwrap();
        }
        let bins = ["a.avb", "b.avb", "c.avb"]
            .iter()
            .enumerate()
            .map(|(i, n)| BinEntry {
                path: checked_file(&t.0.join(n)).unwrap(),
                relative: (*n).into(),
                clips: 0,
                sequences: 0,
                dependency: i == 1,
            })
            .collect();
        let inv = Inventory {
            bins,
            ..Default::default()
        };
        let spec = JobSpec {
            bins: vec![t.0.join("a.avb")],
            ..Default::default()
        };
        assert_eq!(selected_paths(&spec, &inv).unwrap().len(), 2);
        assert_eq!(
            selected_paths(
                &JobSpec {
                    include_dependencies: false,
                    ..spec
                },
                &inv
            )
            .unwrap()
            .len(),
            1
        );
    }
}
