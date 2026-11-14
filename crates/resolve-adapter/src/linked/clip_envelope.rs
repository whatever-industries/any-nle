//! Prepare clip gain, volume keys and fades for linked Resolve timelines.
use super::Profile;
use crate::{
    audio::{self, Envelope, Fades},
    Result,
};
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Placement, Rational, Value,
};
use std::collections::{HashMap, HashSet};

pub(super) struct ClipEnvelopePreparation<'a> {
    graph: &'a SourceGraph,
    rate: Rational,
    premiere_clock: bool,
    has_a1_crossfade: bool,
    premiere_transition_ids: HashSet<String>,
    unsupported_components: HashSet<&'a str>,
    recovered: HashMap<&'a str, (&'static str, &'static str)>,
}

impl<'a> ClipEnvelopePreparation<'a> {
    pub(super) fn new(
        graph: &'a SourceGraph,
        sequence: &SourceSequence,
        has_a1_crossfade: bool,
    ) -> Self {
        Self {
            graph,
            rate: sequence.rate,
            premiere_clock: sequence
                .clock
                .as_ref()
                .is_some_and(|clock| clock.provenance.route == "Premiere sequence ticks"),
            has_a1_crossfade,
            premiere_transition_ids: sequence.transitions.iter().map(|t| t.id.clone()).collect(),
            unsupported_components: graph
                .losses
                .iter()
                .filter(|loss| loss.aspect == "premiere.appearance")
                .map(|loss| loss.object.as_str())
                .collect(),
            recovered: HashMap::new(),
        }
    }

    // Check fade ownership and lane eligibility before changing the visible range.
    pub(super) fn check_premiere_fades(
        &self,
        edit: &Placement,
        lane: usize,
        profile: &Profile,
    ) -> Result<Option<&'a Value>> {
        let fades = self.graph.sidecar(audio::PREMIERE_FADE_ADAPTER, &edit.id);
        if fades.is_some()
            && (!matches!(lane, 1 | 2)
                || !matches!(profile, Profile::Wav { .. })
                || !self.premiere_clock
                || self.unsupported_components.contains(edit.id.as_str())
                || (self.has_a1_crossfade && lane == 1))
        {
            return Err(format!("{}: Premiere clip-edge fades require a verified mono WAV edit with supported clip processing and no crossfade on its lane", edit.id).into());
        }
        Ok(fades)
    }

    pub(super) fn prepare(
        &mut self,
        edit: &'a Placement,
        lane: usize,
        profile: &Profile,
        source_in: i64,
        length: i64,
        premiere_fades: Option<&Value>,
    ) -> Result<Option<Envelope>> {
        if !matches!(lane, 1 | 2 | 4) {
            return Ok(None);
        }
        if self
            .graph
            .sidecar(audio::UNQUALIFIED_FADE_ADAPTER, &edit.id)
            .is_some()
        {
            return Err(format!("{}: unqualified Resolve clip fade points", edit.id).into());
        }
        let receipt = self.graph.sidecar(crate::source::ADAPTER, &edit.id);
        if (receipt.is_some() || edit.volume_db.is_some() || !edit.volume_keys.is_empty())
            && !matches!(profile, Profile::Wav { .. })
        {
            return Err(
                format!("{}: clip envelopes require a verified WAV source", edit.id).into(),
            );
        }
        if self.rate != Rational::integer(24)
            && (!edit.volume_keys.is_empty()
                || receipt.is_some_and(|r| {
                    r.get("fade_in_point").is_some() || r.get("fade_out_point").is_some()
                }))
        {
            return Err(format!("{}: custom fades and volume keys require 24 fps", edit.id).into());
        }
        let envelope = if let Some(fades) = premiere_fades {
            if receipt.is_some() {
                return Err(
                    format!("{}: conflicting Premiere and Resolve clip fades", edit.id).into(),
                );
            }
            Some(Fades::from_premiere(
                fades,
                edit,
                length,
                self.rate,
                &mut self.premiere_transition_ids,
            )?)
        } else {
            audio::prepare(edit, source_in, length, self.rate, receipt)?
        };
        if envelope
            .and_then(|e| e.fades)
            .is_some_and(Fades::fractional)
            && (!matches!(lane, 1 | 2) || (lane == 1 && self.has_a1_crossfade))
        {
            return Err(format!(
                "{}: fractional clip fades require A1 or A2 without a crossfade on that lane",
                edit.id
            )
            .into());
        }
        if let Some(loss) = envelope.and_then(Envelope::recovered_loss) {
            self.recovered.insert(edit.id.as_str(), loss);
        }
        Ok(envelope.filter(|control| *control != Envelope::static_volume(0.0)))
    }

    pub(super) fn recovers(&self, loss: &Loss) -> bool {
        self.recovered
            .get(loss.object.as_str())
            .is_some_and(|(aspect, reason)| loss.aspect == *aspect && loss.reason == *reason)
    }
}
