//! Direct Avid source adapter. No destination model or writer participates in extraction.
//! Source facts and graph construction each have a 64 MiB accounted budget; these are
//! distinct from the encoded graph ceiling and are not peak-RSS guarantees.
mod effects;
pub mod index;
pub mod original;
mod revision;
mod sequence;
mod multicam;
use avid_reader::{
    extract::{self, AvidMarker, Essence, KIND_PICTURE, KIND_SOUND},
    source::{self as av, Observed, Rate},
};
use graph::{source::*, *};
use std::{
    collections::{HashMap, HashSet},
    mem::size_of,
    path::PathBuf,
};

pub struct ReadResult {
    pub graph: SourceGraph,
    /// Exact original bytes pinned before/after stream probing and checked again at read completion.
    pub original_inputs: Vec<av::SourceInput>,
    pub diagnostics: Vec<String>,
    pub audio_group_issues: Vec<String>,
    pub scan_issues: Vec<String>,
    pub files_scanned: usize,
}
pub fn read(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
) -> Result<ReadResult, String> {
    let facts =
        av::extract_source_paths(inputs, av::SourceLimits::default()).map_err(|e| e.to_string())?;
    let wanted = facts
        .masters
        .iter()
        .flat_map(|m| m.master.tracks.iter().map(|t| t.file_mob.umid_hex()))
        .collect();
    let index = index::MediaIndex::scan(media_roots, &wanted)?;
    build(&facts, &index, project, bin_names)
}

/// Original paths are resolved only from saved locators and explicit prefix mappings.
pub fn read_with_originals(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
) -> Result<ReadResult, String> {
    read_with_originals_observed(
        inputs,
        media_roots,
        project,
        bin_names,
        options,
        &mut |_, _| Ok(()),
    )
}
/// Source progress is reported before each native bin open, without retaining decoded nodes.
pub fn read_with_originals_observed(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
    before: &mut dyn FnMut(usize, &std::path::Path) -> Result<(), String>,
) -> Result<ReadResult, String> {
    read_with_originals_controlled(
        inputs,
        media_roots,
        project,
        bin_names,
        options,
        before,
        &mut || Ok(()),
    )
}
/// The optional control is checked during original source hashing and between selected tracks.
pub fn read_with_originals_controlled(
    inputs: &[av::SourceInput],
    media_roots: &[PathBuf],
    project: &str,
    bin_names: &[String],
    options: &original::OriginalOptions,
    before: &mut dyn FnMut(usize, &std::path::Path) -> Result<(), String>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<ReadResult, String> {
    let facts = av::extract_source_paths_observed(inputs, av::SourceLimits::default(), before)
        .map_err(|e| e.to_string())?;
    let originals = original::OriginalIndex::scan_controlled(&facts, options, check)?;
    let wanted = facts
        .masters
        .iter()
        .flat_map(|m| {
            m.master.tracks.iter().enumerate().filter_map(|(i, t)| {
                (!originals.tracks.contains_key(&(m.id.clone(), i))).then(|| t.file_mob.umid_hex())
            })
        })
        .collect();
    let index = index::MediaIndex::scan_controlled(media_roots, &wanted, check)?;
    build_selected(&facts, &index, project, bin_names, &originals)
}

/// Bounded entry point also permits byte-free synthetic source-fact tests.
pub fn build(
    facts: &av::SourceProject,
    index: &index::MediaIndex,
    project: &str,
    bin_names: &[String],
) -> Result<ReadResult, String> {
    build_selected(
        facts,
        index,
        project,
        bin_names,
        &original::OriginalIndex::default(),
    )
}
fn build_selected(
    facts: &av::SourceProject,
    index: &index::MediaIndex,
    project: &str,
    bin_names: &[String],
    originals: &original::OriginalIndex,
) -> Result<ReadResult, String> {
    validate_rates(facts)?;
    if facts.accounted_bytes > av::MAX_SOURCE_FACT_BYTES {
        return Err("source facts exceed 64 MiB".into());
    }
    let mut budget = Budget::default();
    budget.charge(size_of::<SourceGraph>() + project.len())?;
    // Report strings are owned too; reserve escaped names before formatting.
    for skipped in &facts.skipped {
        budget.charge(
            skipped
                .name
                .len()
                .checked_mul(10)
                .and_then(|n| n.checked_add(skipped.reason.len() + 64))
                .ok_or("diagnostic budget overflow")?,
        )?;
    }
    for input in &originals.original_inputs {
        budget.charge(
            size_of::<av::SourceInput>() + input.path.as_os_str().len() + input.revision.len(),
        )?;
    }
    for note in &originals.notes {
        budget.charge(size_of::<String>() + note.len())?;
    }
    let mut out = ReadResult {
        original_inputs: originals.original_inputs.clone(),
        graph: SourceGraph {
            multicams: Vec::new(),
            multicam_uses: Vec::new(),
            project: Project {
                name: project.into(),
            },
            media: vec![],
            items: vec![],
            bins: vec![],
            sequences: vec![],
            losses: vec![],
            sidecars: vec![],
        },
        diagnostics: facts
            .skipped
            .iter()
            .map(|s| format!("{:?}: skipped ({})", s.name, s.reason))
            .chain(originals.notes.iter().cloned())
            .collect(),
        audio_group_issues: vec![],
        scan_issues: vec![],
        files_scanned: index.files_scanned,
    };
    for (i, name) in bin_names.iter().enumerate() {
        budget.charge(size_of::<Bin>() + name.len() + 32)?;
        out.graph.bins.push(Bin {
            id: format!("bin:{i}"),
            name: name.clone(),
            parent: None,
            items: vec![],
        });
    }
    for (name, bin) in &facts.duplicates {
        out.diagnostics.push(format!(
            "{name:?} is also in bin {:?}; a copy is imported there with that bin's attributes",
            bin_names
                .get(*bin)
                .ok_or("duplicate references unknown bin")?
        ));
    }
    for uid in &index.ambiguous {
        out.scan_issues
            .push(format!("ambiguous MXF file package {uid}"));
    }
    for (p, e) in &index.unreadable {
        out.scan_issues.push(format!("{}: {e}", p.display()));
    }
    let mut copies: HashMap<avid_reader::io::MobId, Vec<(usize, usize)>> = HashMap::new();
    for (n, source) in facts.masters.iter().enumerate() {
        let id = format!("clip:clip-{}", n + 1);
        let checkpoint = (
            out.graph.media.len(),
            out.graph.sidecars.len(),
            out.graph.losses.len(),
            budget.used,
        );
        match master(source, index, &id, &mut out, &mut budget, originals) {
            Ok(item) => {
                let bin = out
                    .graph
                    .bins
                    .get_mut(source.master.bin)
                    .ok_or("master references unknown bin")?;
                budget.charge(size_of::<String>() + id.len())?;
                bin.items.push(id);
                copies
                    .entry(source.master.mob_id)
                    .or_default()
                    .push((source.master.bin, out.graph.items.len()));
                out.graph.items.push(item);
            }
            Err(MasterError::Budget(e)) => return Err(e),
            Err(MasterError::Unsupported(e)) => {
                out.graph.media.truncate(checkpoint.0);
                out.graph.sidecars.truncate(checkpoint.1);
                out.graph.losses.truncate(checkpoint.2);
                budget.used = checkpoint.3;
                retain_unresolved_master(source, &e, &mut out, &mut budget)?;
                out.diagnostics
                    .push(format!("{:?}: skipped ({e})", source.master.name));
            }
        }
    }
    let multicams = multicam::append(&facts.groups, &copies, &mut out, &mut budget)?;
    let media_by_id: HashMap<_, _> = out.graph.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let item_routes: Vec<_> = out
        .graph
        .items
        .iter()
        .map(|i| {
            let mut audio = HashSet::new();
            let mut raster = None;
            for im in &i.media {
                let m = media_by_id[im.media.as_str()];
                if m.kind == StreamKind::Audio {
                    if let Some(n) = m.stream {
                        audio.insert(n);
                    }
                } else {
                    raster = m.frame_size;
                }
            }
            sequence::ItemRoutes { audio, raster }
        })
        .collect();
    for seq in &facts.sequences {
        sequence::append(seq, &copies, &item_routes, &multicams, &mut out, &mut budget)?;
    }
    out.graph.validate().map_err(|e| e.to_string())?;
    out.graph.encoded_len().map_err(|e| e.to_string())?;
    Ok(out)
}

#[derive(Default)]
struct Budget {
    used: usize,
}
impl Budget {
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.used = self
            .used
            .checked_add(n)
            .filter(|n| *n <= av::MAX_SOURCE_FACT_BYTES)
            .ok_or("source graph construction exceeds 64 MiB accounted budget")?;
        Ok(())
    }
    fn value(&mut self, v: &Value) -> Result<(), String> {
        self.charge(size_of::<Value>())?;
        self.value_payload(v)
    }
    fn value_payload(&mut self, v: &Value) -> Result<(), String> {
        match v {
            Value::Str(s) => self.charge(s.capacity()),
            Value::Arr(a) => {
                self.charge(a.capacity() * size_of::<Value>())?;
                for x in a {
                    self.value_payload(x)?;
                }
                Ok(())
            }
            Value::Obj(a) => {
                self.charge(a.capacity() * size_of::<(String, Value)>())?;
                for (k, v) in a {
                    self.charge(k.capacity())?;
                    self.value_payload(v)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
fn validate_rates(facts: &av::SourceProject) -> Result<(), String> {
    let rate = |r: Rate| {
        if Rate::new(r.num, r.den) == Some(r) {
            Ok(())
        } else {
            Err("malformed source rate witness".to_string())
        }
    };
    let route = |v: &[av::RouteWitness]| {
        for w in v {
            if let Some(r) = w.rate {
                rate(r)?;
            }
        }
        Ok::<_, String>(())
    };
    for m in &facts.masters {
        if let Some(r) = m.rate {
            rate(r)?;
        }
        for w in &m.marker_witnesses {
            if let Some(r) = w.local_rate {
                rate(r)?;
            }
        }
    }
    for g in &facts.groups {
        if let Some(r)=g.rate{rate(r)?;}
        for l in &g.lanes {
            if let Some(r)=l.rate{rate(r)?;}
            for a in &l.alternatives {
                if let Some(r)=a.endpoint.rate{rate(r)?;}
                for p in &a.pieces{if let av::SourceGroupPiece::Media{rate:r,..}=p{rate(*r)?;}}
            }
        }
    }
    for s in &facts.sequences {
        for u in &s.group_uses{
            if let Some(r)=u.rate{rate(r)?;}
            for e in &u.alternatives{if let Some(r)=e.rate{rate(r)?;}}
        }
        if let Some(r) = s.rate {
            rate(r)?;
        }
        for e in &s.events {
            if let Some(q) = &e.range {
                rate(q.record_rate)?;
                rate(q.source_rate)?;
                route(&q.route)?;
            }
        }
        for t in &s.transitions {
            if let Some(r) = t.rate {
                rate(r)?;
            }
        }
        for m in &s.markers {
            if let Some(r) = m.local_rate {
                rate(r)?;
            }
        }
        for i in &s.unresolved {
            if let Some(r) = i.record_rate {
                rate(r)?;
            }
            route(&i.route)?;
        }
    }
    Ok(())
}
fn reserve_marker(m: &AvidMarker, budget: &mut Budget) -> Result<(), String> {
    // The typed marker and raw receipt each own these texts; pre-charge both copies.
    for text in [&m.name, &m.comment, &m.user, &m.color, &m.id] {
        budget.charge(
            text.len()
                .checked_mul(2)
                .ok_or("marker text budget overflow")?,
        )?;
    }
    if let Some(t) = &m.track_attr {
        budget.charge(t.len())?;
    }
    budget.charge(2048)
}
fn s(v: impl ToString) -> Value {
    Value::Str(v.to_string())
}
fn raw_float(v: f64) -> Value {
    if v.is_finite() {
        Value::Float(v)
    } else {
        obj(vec![("f64_bits", s(format!("{:016X}", v.to_bits())))])
    }
}
fn obj(v: Vec<(&str, Value)>) -> Value {
    Value::Obj(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn opt_s(v: Option<&str>) -> Value {
    v.map(s).unwrap_or(Value::Null)
}
// Preserve type and bytes, without resolving an opaque bin-local reference or
// interpreting an unknown control as a default. Preflight before allocating hex.
fn output_mix(m: &extract::MasterOutputMix, budget: &mut Budget) -> Result<Value, String> {
    use extract::MasterOutputMix as M;
    let payload = match m {
        M::Text(v) => v.len(),
        M::Bytes(v) => v.len().checked_mul(2).ok_or("mix payload overflow")?,
        _ => 0,
    };
    let mut reservation = Budget { used: budget.used };
    reservation.charge(payload.checked_add(512).ok_or("mix payload overflow")?)?;
    let (kind, raw) = match m {
        M::Absent => ("absent", Value::Null),
        M::Integer(v) => ("integer", Value::Int(*v)),
        M::Text(v) => ("text", s(v)),
        M::Bytes(v) => {
            use std::fmt::Write;
            let mut hex = String::with_capacity(payload);
            for byte in v {
                write!(&mut hex, "{byte:02x}").unwrap();
            }
            ("bytes_hex", Value::Str(hex))
        }
        M::Reference(v) => ("bin_object_ref", Value::Int(i64::from(*v))),
        M::Null => ("null", Value::Null),
    };
    Ok(obj(vec![("type", s(kind)), ("raw", raw)]))
}
fn rational(r: Rate) -> Rational {
    Rational::new(r.num as i128, r.den as i128).expect("positive reduced i64 source rate")
}
fn time(n: i64, r: Rate) -> Result<Rational, String> {
    Rational::new(n as i128 * r.den as i128, r.num as i128)
        .ok_or("source time overflows rational".into())
}
fn kind(k: i64) -> Option<StreamKind> {
    match k {
        KIND_PICTURE => Some(StreamKind::Video),
        KIND_SOUND => Some(StreamKind::Audio),
        _ => None,
    }
}
fn observed(v: &Observed<i64>) -> Value {
    match v {
        Observed::Known(n) => obj(vec![("value", Value::Int(*n))]),
        Observed::Unresolved(r) => obj(vec![("reason", s(r))]),
    }
}
fn loss(
    out: &mut ReadResult,
    budget: &mut Budget,
    id: &str,
    aspect: &str,
    reason: impl Into<String>,
) -> Result<(), String> {
    let reason = reason.into();
    budget.charge(size_of::<Loss>() + id.len() + aspect.len() + reason.len())?;
    out.graph.losses.push(Loss {
        object: id.into(),
        aspect: aspect.into(),
        reason,
    });
    Ok(())
}
fn sidecar(
    out: &mut ReadResult,
    budget: &mut Budget,
    adapter: &str,
    id: &str,
    data: Value,
) -> Result<(), String> {
    budget.charge(size_of::<Sidecar>() + adapter.len() + id.len())?;
    budget.value(&data)?;
    out.graph.sidecars.push(Sidecar {
        adapter: adapter.into(),
        object: id.into(),
        data,
    });
    Ok(())
}
fn append_bin_receipt(
    out: &mut ReadResult,
    budget: &mut Budget,
    bin: usize,
    key: &str,
    data: Value,
) -> Result<(), String> {
    let id = out
        .graph
        .bins
        .get(bin)
        .ok_or("native fact references unknown bin")?
        .id
        .clone();
    if let Some(existing) = out
        .graph
        .sidecars
        .iter_mut()
        .find(|s| s.adapter == "avid.source" && s.object == id)
    {
        budget.value(&data)?;
        budget.charge(size_of::<Value>())?;
        let Value::Obj(fields) = &mut existing.data else {
            return Err("invalid bin source sidecar".into());
        };
        if let Some((_, v)) = fields.iter_mut().find(|(k, _)| k == key) {
            let Value::Arr(a) = v else {
                return Err("invalid bin receipt list".into());
            };
            a.push(data);
        } else {
            budget.charge(key.len() + size_of::<(String, Value)>())?;
            fields.push((key.into(), Value::Arr(vec![data])));
        }
        Ok(())
    } else {
        sidecar(
            out,
            budget,
            "avid.source",
            &id,
            obj(vec![(key, Value::Arr(vec![data]))]),
        )
    }
}
fn retain_unresolved_master(
    source: &av::SourceMaster,
    reason: &str,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<(), String> {
    let m = &source.master;
    // Guard all potentially large cloned payloads before building an opaque receipt.
    let mut reservation = Budget { used: budget.used };
    reservation.charge(
        m.name.len()
            + source.id.len()
            + m.tracks.len() * 2048
            + m.other_groups.iter().map(|s| s.len()).sum::<usize>(),
    )?;
    for (k, v) in &m.columns {
        reservation.charge(k.len() + v.len() + size_of::<(String, Value)>())?;
    }
    for (_, _, mk) in &m.markers {
        reserve_marker(mk, &mut reservation)?;
    }
    let markers = m
        .markers
        .iter()
        .enumerate()
        .map(|(i, (_, _, mk))| {
            let mut native = master_marker_native(source, i, mk);
            if let Value::Obj(fields) = &mut native {
                fields.extend([
                    ("name".into(), s(&mk.name)),
                    ("comment".into(), s(&mk.comment)),
                    ("user".into(), s(&mk.user)),
                    ("color".into(), s(&mk.color)),
                    ("graph_index".into(), Value::Null),
                ]);
            }
            native
        })
        .collect();
    let mix = output_mix(&m.output_mix, budget)?;
    let raw = obj(vec![
        ("occurrence", s(&source.id)),
        ("name", s(&m.name)),
        ("mob_id", s(m.mob_id.urn())),
        ("reason", s(reason)),
        ("native_rate", raw_float(m.edit_rate)),
        (
            "qualified_rate",
            source.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
        ),
        ("length", Value::Int(m.length)),
        (
            "color_rgb16",
            m.color
                .map(|c| Value::Arr(c.into_iter().map(Value::Int).collect()))
                .unwrap_or(Value::Null),
        ),
        (
            "start",
            m.start
                .as_ref()
                .map(|t| {
                    obj(vec![
                        ("count", Value::Int(t.frames)),
                        ("rounded_base", Value::Int(t.fps)),
                        ("drop", Value::Bool(t.drop)),
                    ])
                })
                .unwrap_or(Value::Null),
        ),
        (
            "aux_timecodes",
            Value::Arr(
                m.aux_timecodes
                    .iter()
                    .map(|a| {
                        obj(vec![
                            ("track_index", Value::Int(a.track_index)),
                            ("position", Value::Int(a.position)),
                            ("length", Value::Int(a.length)),
                            ("count", Value::Int(a.start)),
                            ("rounded_base", Value::Int(a.fps)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "columns",
            Value::Arr(
                m.columns
                    .iter()
                    .map(|(k, v)| Value::Arr(vec![s(k), s(v)]))
                    .collect(),
            ),
        ),
        ("source_file", opt_s(m.source_file.as_deref())),
        ("source_path", opt_s(m.source_path.as_deref())),
        (
            "tracks",
            Value::Arr(
                m.tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        obj(vec![
                            ("kind", Value::Int(t.kind)),
                            ("index", Value::Int(t.index)),
                            ("file_package", s(t.file_mob.umid_hex())),
                            ("native_rate", raw_float(t.edit_rate)),
                            ("length", Value::Int(t.length)),
                            ("clip_length", Value::Int(t.clip_length)),
                            (
                                "offset",
                                source.offsets.get(i).map(observed).unwrap_or(Value::Null),
                            ),
                            ("gain_db", t.gain_db.map(raw_float).unwrap_or(Value::Null)),
                            ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
                            ("descriptor", descriptor(&t.essence)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "stereo_pairs",
            Value::Arr(
                m.stereo_pairs
                    .iter()
                    .map(|(a, b)| Value::Arr(vec![Value::Int(*a), Value::Int(*b)]))
                    .collect(),
            ),
        ),
        (
            "raw_groups",
            Value::Arr(m.other_groups.iter().map(s).collect()),
        ),
        ("marker_native", Value::Arr(markers)),
        ("master_output_mix", mix),
    ]);
    append_bin_receipt(out, budget, m.bin, "unresolved_masters", raw)?;
    loss(
        out,
        budget,
        &format!("bin:{}", m.bin),
        "source_master",
        format!(
            "{}: {reason}; native master facts retained on bin",
            source.id
        ),
    )
}
fn clock(v: Option<(u64, u16, bool)>) -> Value {
    v.map(|(n, b, d)| {
        obj(vec![
            ("count", s(n)),
            ("rounded_base", Value::Int(b as i64)),
            ("drop", Value::Bool(d)),
        ])
    })
    .unwrap_or(Value::Null)
}
fn tc(n: i64, fps: i64) -> String {
    if fps <= 0 {
        return String::new();
    }
    let f = (n as i128).rem_euclid(86400i128 * fps as i128);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        f / (3600 * fps as i128),
        f / (60 * fps as i128) % 60,
        f / fps as i128 % 60,
        f % fps as i128
    )
}
fn marker_native(m: &AvidMarker) -> Value {
    obj(vec![
        ("id", s(&m.id)),
        ("frame", Value::Int(m.frame)),
        ("length", m.length.map(Value::Int).unwrap_or(Value::Null)),
        ("created", m.created.map(Value::Int).unwrap_or(Value::Null)),
        (
            "rgb16",
            Value::Arr(m.rgb16.into_iter().map(Value::Int).collect()),
        ),
        ("track_attr", opt_s(m.track_attr.as_deref())),
    ])
}
fn master_marker_native(source: &av::SourceMaster, index: usize, m: &AvidMarker) -> Value {
    let mut v = marker_native(m);
    if let Value::Obj(fields) = &mut v {
        if let Some(w) = source.marker_witnesses.get(index) {
            fields.extend([
                ("owner".into(), s(&w.owner)),
                ("kind".into(), Value::Int(w.kind)),
                ("track".into(), Value::Int(w.track)),
                ("local_offset".into(), observed(&w.local_offset)),
                (
                    "local_rate".into(),
                    w.local_rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                ),
                (
                    "master_frame".into(),
                    w.master_frame.map(Value::Int).unwrap_or(Value::Null),
                ),
            ]);
        }
    }
    v
}
fn marker(
    m: &AvidMarker,
    start: Option<Rational>,
    local: Option<Rational>,
    rate: Option<Rate>,
    owner: String,
    owner_kind: MarkerOwner,
    track: Option<TrackRef>,
    mut binding: SourceBinding,
) -> Result<SourceMarker, String> {
    if start.is_none() && local.is_none() && binding.reason.is_none() {
        binding.reason = Some("marker local and record clocks are unresolved".into());
    }
    Ok(SourceMarker {
        name: m.name.clone(),
        comment: m.comment.clone(),
        user: m.user.clone(),
        color: (!m.color.is_empty()).then(|| Label {
            palette: "avid".into(),
            name: m.color.clone(),
        }),
        start,
        duration: match (m.length, rate) {
            (Some(n), Some(r)) => time(n, r)?,
            _ => Rational::ZERO,
        },
        track,
        owner: owner_kind,
        owner_occurrence: owner,
        owner_local: local,
        owner_rate: rate.map(rational),
        binding,
        source: None,
        inherited: false,
    })
}
fn descriptor(e: &Option<Essence>) -> Value {
    match e {
        Some(Essence::Picture {
            compression,
            resolution_id,
            width,
            height,
            display_aspect,
        }) => obj(vec![
            ("compression", s(compression)),
            ("resolution_id", Value::Int(*resolution_id)),
            ("width", Value::Int(*width)),
            ("height", Value::Int(*height)),
            (
                "display_aspect",
                display_aspect
                    .map(|(n, d)| Value::Arr(vec![Value::Int(n), Value::Int(d)]))
                    .unwrap_or(Value::Null),
            ),
        ]),
        Some(Essence::Sound {
            sample_rate,
            bits,
            channels,
        }) => obj(vec![
            ("sample_rate", Value::Float(*sample_rate)),
            ("bits", Value::Int(*bits)),
            ("channels", Value::Int(*channels)),
        ]),
        _ => Value::Null,
    }
}
enum MasterError {
    Unsupported(String),
    Budget(String),
}
impl From<&str> for MasterError {
    fn from(e: &str) -> Self {
        Self::Unsupported(e.into())
    }
}
impl From<String> for MasterError {
    fn from(e: String) -> Self {
        Self::Unsupported(e)
    }
}
fn master(
    source: &av::SourceMaster,
    index: &index::MediaIndex,
    id: &str,
    out: &mut ReadResult,
    budget: &mut Budget,
    originals: &original::OriginalIndex,
) -> Result<SourceItem, MasterError> {
    let m = &source.master;
    budget
        .charge(size_of::<SourceItem>() + id.len() + m.name.len() + source.id.len())
        .map_err(MasterError::Budget)?;
    for (k, v) in &m.columns {
        budget
            .charge(k.len() + v.len() + size_of::<(String, Value)>())
            .map_err(MasterError::Budget)?;
    }
    for text in m
        .source_file
        .iter()
        .chain(m.source_path.iter())
        .chain(m.other_groups.iter())
    {
        budget.charge(text.len()).map_err(MasterError::Budget)?;
    }
    for (_, _, mk) in &m.markers {
        reserve_marker(mk, budget).map_err(MasterError::Budget)?;
    }
    let mut item = SourceItem {
        id: id.into(),
        name: m.name.clone(),
        media: vec![],
        umid: Some(m.mob_id.umid_hex()),
        tape: m
            .columns
            .iter()
            .find(|(k, _)| k == "Tape")
            .map(|(_, v)| v.clone()),
        rate: source.rate.map(rational),
        color_rgb16: m.color,
        stream_groups: vec![],
        markers: vec![],
    };
    let mut video = false;
    let mut labels = HashSet::new();
    let mut raw_tracks = vec![];
    let mut audio_ordinal = 0;
    for (tindex, t) in m.tracks.iter().enumerate() {
        let audio_index = audio_ordinal;
        if t.kind == KIND_SOUND {
            audio_ordinal += 1;
        }
        if let Some(selected) = originals.tracks.get(&(source.id.clone(), tindex)) {
            let stream_id = if selected.kind == StreamKind::Video {
                if video {
                    return Err("more than one original picture track".into());
                }
                video = true;
                format!("{id}:video")
            } else {
                labels.insert(t.index);
                format!("{id}:audio:{audio_index}")
            };
            let offset = match (source.offsets.get(tindex), source.rate) {
                (Some(Observed::Known(n)), Some(r)) => Some(time(*n, r)?),
                _ => None,
            };
            let original = &selected.stream;
            budget
                .charge(
                    size_of::<SourceMedia>()
                        + size_of::<SourceItemMedia>()
                        + size_of::<OriginalStream>()
                        + stream_id.len() * 2
                        + selected.path.as_os_str().len()
                        + original
                            .saved_locators
                            .iter()
                            .map(|s| s.capacity() + size_of::<String>())
                            .sum::<usize>()
                        + 512,
                )
                .map_err(MasterError::Budget)?;
            let raw = source.originals[tindex]
                .as_ref()
                .ok_or("original descriptor witness missing")?;
            sidecar(
                out,
                budget,
                "avid.source",
                &stream_id,
                obj(vec![
                    ("file_package", s(t.file_mob.umid_hex())),
                    ("descriptor_class", s(&raw.descriptor_class)),
                    ("descriptor_rate", Value::Float(raw.descriptor_rate)),
                    ("descriptor_length", Value::Int(raw.descriptor_length)),
                    (
                        "stored_quantization_bits",
                        raw.stored_quantization_bits
                            .map(Value::Int)
                            .unwrap_or(Value::Null),
                    ),
                    ("descriptor", descriptor(&t.essence)),
                    ("native_kind", Value::Int(t.kind)),
                    ("native_track", Value::Int(t.index)),
                    (
                        "file_track",
                        raw.file_track.map(Value::Int).unwrap_or(Value::Null),
                    ),
                ]),
            )
            .map_err(MasterError::Budget)?;
            out.graph.media.push(SourceMedia {
                id: stream_id.clone(),
                kind: selected.kind,
                locations: vec![selected
                    .path
                    .to_str()
                    .ok_or("original path is not UTF-8")?
                    .into()],
                codec: Some(selected.codec.clone()),
                frame_size: selected.raster,
                rate: selected.rate,
                sample_rate: selected.sample_rate,
                bits: selected.bits,
                duration: selected.duration,
                start: None,
                clock: None,
                stream: (selected.kind == StreamKind::Audio).then_some(t.index),
                online: Some(true),
                original: Some(original.clone()),
            });
            item.media.push(SourceItemMedia {
                media: stream_id,
                gain_db: t.gain_db.unwrap_or(0.0),
                source_offset: offset,
            });
            raw_tracks.push(obj(vec![
                ("kind", Value::Int(t.kind)),
                ("label", Value::Int(t.index)),
                ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
                (
                    "offset",
                    source
                        .offsets
                        .get(tindex)
                        .map(observed)
                        .unwrap_or(Value::Null),
                ),
            ]));
            continue;
        }
        let rate = Rate::from_avid(t.edit_rate)
            .ok_or_else(|| format!("unsupported edit rate {}", t.edit_rate))?;
        let Some(path) = index.files.get(&t.file_mob.umid_hex()) else {
            if t.kind == KIND_SOUND {
                out.diagnostics.push(format!(
                    "{:?} audio A{}: media for file mob {} not found; converted without it",
                    m.name,
                    t.index,
                    t.file_mob.umid_hex()
                ));
                continue;
            }
            return Err(format!("media for file mob {} not found", t.file_mob.umid_hex()).into());
        };
        let identity = index.identity(path)?;
        let (stream_id, stream_kind, codec, size, sample_rate, bits, stream) =
            match (t.kind, &t.essence) {
                (
                    KIND_PICTURE,
                    Some(Essence::Picture {
                        compression,
                        width,
                        height,
                        ..
                    }),
                ) => {
                    if video {
                        return Err("more than one picture track".to_string().into());
                    }
                    video = true;
                    let codec = match compression.as_str() {
                        "AVHD" => "AVdn",
                        "RI00"
                            if identity.picture_essence_coding
                                == Some(avid_reader::mxf::PRORES_422_LT_CODING) =>
                        {
                            "apcs"
                        }
                        "RI00"
                            if identity.picture_essence_coding
                                == Some(avid_reader::mxf::DNXHR_LB_CODING) =>
                        {
                            "AVdn"
                        }
                        _ => {
                            return Err(
                                format!("unsupported picture compression {compression:?}").into()
                            )
                        }
                    };
                    if !(16..=16384).contains(width) || !(16..=16384).contains(height) {
                        return Err("unsupported picture raster".to_string().into());
                    }
                    (
                        format!("{id}:video"),
                        StreamKind::Video,
                        Some(codec.into()),
                        Some((*width, *height)),
                        None,
                        None,
                        None,
                    )
                }
                (
                    KIND_SOUND,
                    Some(Essence::Sound {
                        sample_rate,
                        bits,
                        channels,
                    }),
                ) => {
                    if *channels != 1
                        || !matches!(*sample_rate, 44100.0 | 48000.0 | 96000.0)
                        || !matches!(*bits, 16 | 24)
                        || !(1..=64).contains(&t.index)
                    {
                        return Err("unsupported audio descriptor".to_string().into());
                    }
                    labels.insert(t.index);
                    (
                        format!("{id}:audio:{audio_index}"),
                        StreamKind::Audio,
                        None,
                        None,
                        Some(*sample_rate as i64),
                        Some(*bits),
                        Some(t.index),
                    )
                }
                _ => return Err(format!("track {} has an unsupported descriptor", t.index).into()),
            };
        let duration = match identity.file_tracks.as_slice() {
            [(n, d, len)] if *n > 0 && *d > 0 => {
                Rational::new(*len as i128 * *d as i128, *n as i128)
                    .ok_or("MXF duration overflow")?
            }
            _ => time(t.length, rate)?,
        };
        // The optional clock lives inline in the media record; precharge its owned
        // provenance payload before the reader creates the qualified owned witness.
        if let Some(q) = identity.qualified_file_clocks.first() {
            budget
                .charge(
                    q.source_package.capacity()
                        + q.timecode_track.capacity()
                        + q.route.capacity()
                        + size_of::<SourceClock>(),
                )
                .map_err(MasterError::Budget)?;
        }
        let source_clock = identity
            .qualified_file_clock()
            .map(|q| {
                Ok::<_, String>(SourceClock {
                    effective_count: q.effective_count,
                    rate: Rational::new(q.rate.0 as i128, q.rate.1 as i128)
                        .ok_or("invalid qualified clock rate")?,
                    display_base: q.rounded_base,
                    drop: q.drop,
                    provenance: SourceClockProvenance {
                        origin_count: q.origin_count,
                        source_offset: q.source_offset,
                        source_rate: Rational::new(
                            q.source_rate.0 as i128,
                            q.source_rate.1 as i128,
                        )
                        .ok_or("invalid qualified source rate")?,
                        source_package: q.source_package,
                        timecode_track: q.timecode_track,
                        route: q.route,
                    },
                })
            })
            .transpose()?;
        let start = source_clock
            .as_ref()
            .map(|c| c.exact_start().ok_or("qualified MXF clock overflow"))
            .transpose()?;
        let offset = match (source.offsets.get(tindex), source.rate) {
            (Some(Observed::Known(n)), Some(r)) => Some(time(*n, r)?),
            _ => None,
        };
        if offset.is_none() {
            loss(
                out,
                budget,
                id,
                "source_offset",
                format!("track {} item-to-file offset is unresolved", t.index),
            )
            .map_err(MasterError::Budget)?;
        }
        let legacy = identity.start_timecode();
        let legacy_start = match legacy {
            Some((n, b, false)) if b > 0 => {
                Rational::new(n as i128, b as i128).unwrap_or(Rational::ZERO)
            }
            _ => Rational::ZERO,
        };
        sidecar(
            out,
            budget,
            "avid.source",
            &stream_id,
            obj(vec![
                ("legacy_clock", clock(legacy)),
                ("legacy_start", s(legacy_start)),
                (
                    "legacy_start_provenance",
                    s("nominal non-drop compatibility aid; not target representability"),
                ),
                ("legacy_file_clock", clock(identity.file_start_timecode())),
                ("file_package", s(t.file_mob.umid_hex())),
                ("descriptor_rate", s(rational(rate))),
                ("descriptor_length", Value::Int(t.length)),
                (
                    "master_offset",
                    source
                        .offsets
                        .get(tindex)
                        .map(observed)
                        .unwrap_or(Value::Null),
                ),
                ("native_kind", Value::Int(t.kind)),
                ("native_track", Value::Int(t.index)),
                ("descriptor", descriptor(&t.essence)),
                (
                    "picture_coding",
                    identity
                        .picture_essence_coding
                        .map(|v| s(v.iter().map(|b| format!("{b:02X}")).collect::<String>()))
                        .unwrap_or(Value::Null),
                ),
            ]),
        )
        .map_err(MasterError::Budget)?;
        let media = SourceMedia {
            id: stream_id.clone(),
            kind: stream_kind,
            locations: vec![path.to_str().ok_or("media path is not UTF-8")?.into()],
            codec,
            frame_size: size,
            rate: (stream_kind == StreamKind::Video).then(|| rational(rate)),
            sample_rate,
            bits,
            duration,
            start,
            clock: source_clock,
            stream,
            online: Some(true),
            original: None,
        };
        budget
            .charge(
                size_of::<SourceMedia>()
                    + size_of::<SourceItemMedia>()
                    + stream_id.len() * 2
                    + path.as_os_str().len()
                    + 128,
            )
            .map_err(MasterError::Budget)?;
        item.media.push(SourceItemMedia {
            media: stream_id,
            gain_db: t.gain_db.unwrap_or(0.0),
            source_offset: offset,
        });
        out.graph.media.push(media);
        raw_tracks.push(obj(vec![
            ("kind", Value::Int(t.kind)),
            ("label", Value::Int(t.index)),
            ("pan_raw", t.pan_raw.map(Value::Int).unwrap_or(Value::Null)),
            (
                "offset",
                source
                    .offsets
                    .get(tindex)
                    .map(observed)
                    .unwrap_or(Value::Null),
            ),
        ]));
    }
    if item.media.is_empty() {
        return Err(
            "no media found for any clip track; clip and its edits left out"
                .to_string()
                .into(),
        );
    }
    for (a, b) in &m.stereo_pairs {
        if labels.contains(a) && labels.contains(b) {
            budget
                .charge(64 + 2 * size_of::<i64>())
                .map_err(MasterError::Budget)?;
            item.stream_groups.push(vec![*a, *b]);
        } else {
            out.audio_group_issues.push(format!(
                "{:?}: stereo A{a}/A{b} missing a channel; available media remains mono",
                m.name
            ));
        }
    }
    for g in &m.other_groups {
        match avid_reader::extract::parse_multichannel_group(g) {
            Some((_, channels)) if channels.iter().all(|n| labels.contains(n)) => {
                budget
                    .charge(64 + channels.capacity() * size_of::<i64>())
                    .map_err(MasterError::Budget)?;
                item.stream_groups.push(channels);
            }
            _ => out.audio_group_issues.push(format!(
                "{:?}: unsupported or incomplete native audio group {g}",
                m.name
            )),
        }
    }
    let mut marker_receipts = vec![];
    for (i, (_, _, mk)) in m.markers.iter().enumerate() {
        let witness = source.marker_witnesses.get(i);
        let rate = witness.and_then(|w| w.local_rate);
        let mut native = master_marker_native(source, i, mk);
        if mk.length.is_some_and(|n| n != 0) && rate.is_none() {
            if let Value::Obj(fields) = &mut native {
                fields.push(("graph_index".into(), Value::Null));
            }
            marker_receipts.push(native);
            loss(out,budget,id,"marker_timing","native spanned marker duration has no qualified owner rate; marker retained only as native facts").map_err(MasterError::Budget)?;
            continue;
        }
        if let Value::Obj(fields) = &mut native {
            fields.push(("graph_index".into(), Value::Int(item.markers.len() as i64)));
        }
        marker_receipts.push(native);
        let local = match (witness.map(|w| &w.local_offset), rate) {
            (Some(Observed::Known(n)), Some(r)) => Some(time(*n, r)?),
            _ => None,
        };
        let start = match (witness.and_then(|w| w.master_frame), source.rate) {
            (Some(n), Some(r)) => Some(time(n, r)?),
            _ => None,
        };
        item.markers.push(marker(
            mk,
            start,
            local,
            rate,
            witness
                .map(|w| w.owner.clone())
                .unwrap_or_else(|| source.id.clone()),
            MarkerOwner::Sequence,
            None,
            SourceBinding {
                placements: vec![],
                reason: None,
            },
        )?);
    }
    let avid = obj(vec![
        ("mob_id", s(m.mob_id.urn())),
        (
            "columns",
            Value::Obj(m.columns.iter().map(|(k, v)| (k.clone(), s(v))).collect()),
        ),
        (
            "start_timecode",
            m.start
                .as_ref()
                .map(|v| s(tc(v.frames, v.fps)))
                .unwrap_or(Value::Null),
        ),
        (
            "aux_timecodes",
            Value::Arr(
                m.aux_timecodes
                    .iter()
                    .map(|a| {
                        obj(vec![
                            ("track_index", Value::Int(a.track_index)),
                            ("timecode", s(tc(a.start, a.fps))),
                            ("frames", Value::Int(a.start)),
                            ("fps", Value::Int(a.fps)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("source_file", opt_s(m.source_file.as_deref())),
        ("source_path", opt_s(m.source_path.as_deref())),
    ]);
    sidecar(out, budget, "avid", id, avid).map_err(MasterError::Budget)?;
    let mix = output_mix(&m.output_mix, budget).map_err(MasterError::Budget)?;
    sidecar(
        out,
        budget,
        "avid.source",
        id,
        obj(vec![
            ("occurrence", s(&source.id)),
            (
                "native_rate",
                source.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
            ),
            (
                "drop",
                Value::Bool(m.start.as_ref().is_some_and(|t| t.drop)),
            ),
            (
                "raw_groups",
                Value::Arr(m.other_groups.iter().map(s).collect()),
            ),
            ("tracks", Value::Arr(raw_tracks)),
            ("marker_native", Value::Arr(marker_receipts)),
            ("master_output_mix", mix),
        ]),
    )
    .map_err(MasterError::Budget)?;
    budget
        .charge(
            size_of::<SourceItem>()
                + item.name.capacity()
                + id.len()
                + item.markers.len() * size_of::<SourceMarker>(),
        )
        .map_err(MasterError::Budget)?;
    Ok(item)
}

#[cfg(test)]
mod tests;
