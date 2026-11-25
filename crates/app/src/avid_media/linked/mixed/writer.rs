//! Stage all native source chains before writing and checking the mixed timeline.
use super::super::{
    managed_atom, mp4, native_bin as saved, original, verify, Plan, Result, SourceKind,
};
use crate::{backend_bundle::BackendTools, source_input};
use avid_reader::{
    io::MobId,
    objects::{Node, Value},
    AvbDocument,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

enum Verified<'a> {
    AvcAac(mp4::Encoding),
    ManagedAtom(managed_atom::Verified<'a>),
}

pub(in super::super) fn write(
    plan: &Plan<'_>,
    destination: &Path,
    tools: &BackendTools,
    recheck_source: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut seen = HashSet::new();
    let mut stamps = Vec::new();
    let mut verified = Vec::with_capacity(plan.sources.len());
    for source in &plan.sources {
        // The qualified MP4 group shares one file; each OP-Atom slot has its own file.
        let originals = match source.kind {
            SourceKind::AvcAac { .. } => &source.routes[..1],
            SourceKind::ManagedAtom => &source.routes[..],
            SourceKind::Separate => {
                return Err("unsupported profile in mixed-source writing".into())
            }
        };
        for route in originals {
            let path = Path::new(route.path);
            if fs::canonicalize(path)? != path || !seen.insert(path) {
                return Err(format!(
                    "{}: conflicting or noncanonical mixed-source original",
                    route.path
                )
                .into());
            }
            stamps.push(source_input::original_stamp(path)?);
        }
        verified.push(match source.kind {
            SourceKind::AvcAac { .. } => {
                Verified::AvcAac(verify::avc_aac(source, plan.rate, tools)?)
            }
            SourceKind::ManagedAtom => Verified::ManagedAtom(managed_atom::prepare(source, tools)?),
            SourceKind::Separate => unreachable!("separate sources already refused"),
        });
    }

    let (mut doc, timestamp) = original::new_bin()?;
    let masters = plan
        .sources
        .iter()
        .zip(&verified)
        .map(|(source, verified)| match verified {
            Verified::AvcAac(avc) => mp4::source_chain(&mut doc, source, plan.rate, avc, timestamp),
            Verified::ManagedAtom(atom) => {
                atom.source_chain(&mut doc, source, plan.rate, timestamp)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    saved::write_timeline(&mut doc, plan, &masters, timestamp)?;
    let decoded = AvbDocument::from_bytes(doc.to_bytes()?)?;
    check(&decoded, plan, &verified, &masters)?;

    recheck_source()?;
    source_input::recheck_originals(&stamps)?;
    for (source, verified) in plan.sources.iter().zip(&verified) {
        if let Verified::ManagedAtom(atom) = verified {
            atom.recheck(source)?;
        }
    }
    decoded.write_new(destination)?;
    Ok(())
}

fn check(
    doc: &AvbDocument,
    plan: &Plan<'_>,
    verified: &[Verified<'_>],
    masters: &[[MobId; 3]],
) -> Result<()> {
    let mut mobs = HashMap::new();
    let mut user_masters = HashSet::new();
    let mut timeline = None;
    let root = doc.root().ok_or("missing mixed-source bin root")?;
    for item in saved::list(root, "items")? {
        let item = saved::embedded(item)?;
        let id = saved::index(item, "mob")?;
        let mob = saved::node(doc, id)?;
        let identity = saved::mob_id(mob)?;
        if mobs.insert(identity, (id, mob)).is_some() {
            return Err("duplicate saved mixed-source package".into());
        }
        match (saved::number(mob, "mob_type_id")?, item.get("user_placed")) {
            (1, Some(Value::Bool(true))) if timeline.is_none() => timeline = Some(id),
            (2, Some(Value::Bool(true))) => {
                user_masters.insert(identity);
            }
            (3, Some(Value::Bool(false))) => {}
            _ => return Err("unexpected mixed-source bin item".into()),
        }
    }
    let expected_masters: HashSet<_> = masters.iter().flatten().copied().collect();
    let dependencies = verified
        .iter()
        .map(|source| match source {
            Verified::AvcAac(_) => 3,
            Verified::ManagedAtom(_) => 4,
        })
        .sum::<usize>();
    if user_masters != expected_masters || mobs.len() != expected_masters.len() + dependencies + 1 {
        return Err("saved mixed-source bin has unexpected masters or dependencies".into());
    }
    let by_id = |id| -> Result<(u32, &Node)> {
        mobs.get(&id)
            .copied()
            .ok_or_else(|| "missing saved mixed-source package".into())
    };
    for ((source, verified), ids) in plan.sources.iter().zip(verified).zip(masters) {
        match verified {
            Verified::AvcAac(avc) => {
                let mut registered = vec![by_id(ids[0])?];
                if source.split_avc_items() {
                    registered.push(by_id(ids[1])?);
                }
                mp4::check_sources(doc, source, plan.rate, avc, &registered, by_id)?;
            }
            Verified::ManagedAtom(atom) => atom.check_sources(doc, source, plan.rate, by_id)?,
        }
    }
    let timeline = timeline.ok_or("saved mixed-source bin needs one timeline")?;
    saved::check_timeline(doc, plan, timeline, masters)
}
