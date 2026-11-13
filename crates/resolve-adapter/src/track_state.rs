//! Resolve track state, bound to timeline and lane.
use crate::Result;
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, StreamKind, Value,
};

pub(crate) const ADAPTER: &str = "resolve.track_state";
pub(crate) const PREMIERE_ADAPTER: &str = "premiere.track_state";
pub(crate) const LOSS: (&str, &str) = (
    "track_state",
    "Resolve track state is retained in source metadata",
);
pub(crate) const LANES: [(StreamKind, i64); 3] = [
    (StreamKind::Video, 1),
    (StreamKind::Audio, 1),
    (StreamKind::Audio, 2),
];

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Track {
    pub(crate) name: String,
    pub(crate) locked: bool,
    pub(crate) disabled: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TrackState(pub(crate) [Track; 3]);

impl TrackState {
    pub(crate) fn prepare(graph: &SourceGraph, sequence: &SourceSequence) -> Result<Option<Self>> {
        Self::parse(graph.sidecar(ADAPTER, &sequence.id), sequence)
    }

    pub(crate) fn prepare_for_resolve(
        graph: &SourceGraph,
        sequence: &SourceSequence,
    ) -> Result<Option<Self>> {
        let resolve = graph.sidecar(ADAPTER, &sequence.id);
        let premiere = graph.sidecar(PREMIERE_ADAPTER, &sequence.id);
        if premiere.is_some() && (resolve.is_some() || !sequence.clock.as_ref()
            .is_some_and(|clock| clock.provenance.route == "Premiere sequence ticks"))
        {
            return Err(format!("{}: conflicting or unqualified Premiere track state", sequence.id).into());
        }
        Self::parse(resolve.or(premiere), sequence)
    }

    fn parse(data: Option<&Value>, sequence: &SourceSequence) -> Result<Option<Self>> {
        let Some(data) = data else { return Ok(None) };
        let invalid = || format!("{}: unsupported timeline track state", sequence.id);
        let Value::Arr(rows) = data else {
            return Err(invalid().into());
        };
        if !sequence
            .tracks
            .iter()
            .map(|track| (track.kind, track.index))
            .eq(LANES)
        {
            return Err(format!(
                "{}: track names, locks and picture output require V1 and two mono tracks",
                sequence.id
            )
            .into());
        }
        let mut seen = [false; 3];
        let mut controls: [Track; 3] = Default::default();
        for row in rows {
            let (
                Some(Value::Str(kind)),
                Some(Value::Int(index)),
                Some(Value::Str(name)),
                Some(Value::Bool(locked)),
            ) = (
                row.get("kind"),
                row.get("track"),
                row.get("name"),
                row.get("locked"),
            )
            else {
                return Err(invalid().into());
            };
            let lane = LANES
                .iter()
                .position(|(k, n)| kind == k.as_str() && index == n)
                .ok_or_else(invalid)?;
            let disabled = match row.get("disabled") {
                None | Some(Value::Bool(false)) => false,
                Some(Value::Bool(true)) if lane == 0 => true,
                _ => return Err(invalid().into()),
            };
            if seen[lane] || !valid_name(name) || name.is_empty() && !locked && !disabled {
                return Err(invalid().into());
            }
            seen[lane] = true;
            controls[lane] = Track {
                name: name.clone(),
                locked: *locked,
                disabled,
            };
        }
        if rows.is_empty() {
            return Err(invalid().into());
        }
        Ok(Some(Self(controls)))
    }

    pub(crate) fn recover(loss: &Loss, sequence: &str) -> bool {
        loss.object == sequence
            && loss.aspect == LOSS.0
            && (loss.reason == LOSS.1
                || loss.reason == "Resolve track names and locks are retained in source metadata")
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    name.len() <= 255 && !name.chars().any(char::is_control)
}
