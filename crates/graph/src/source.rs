//! Qualified source interpretation (`anynle.graph/2`). Unknown source facts are
//! explicit, and this type is never implicitly a legacy/prepared graph/1.
use super::*;
use std::io::Write;
mod decode;
mod encode;
mod multicam;
mod validate;
pub use multicam::{Multicam, MulticamLane, MulticamAlternative, MulticamPiece, MulticamUse};

pub const SCHEMA: &str = "anynle.graph/2";
pub const MAX_SOURCE_GRAPH_BYTES: usize = 256 << 20;
pub const AVID_SOURCE_COLUMNS_TIMECODE: &str = "avid_source_columns_verified_mov_timecode";
pub const AVID_LINKED_MOV_TIMECODE: &str = "avid_linked_mov_timecode";
pub const VERIFIED_LINKED_MOV_TIMECODE: &str = "verified_linked_mov_timecode";

/// Exact source timecode facts. Display base and physical counting rate are distinct.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceClock {
    pub effective_count: u64,
    pub rate: Rational,
    pub display_base: u16,
    pub drop: bool,
    pub provenance: SourceClockProvenance,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SourceClockProvenance {
    pub origin_count: u64,
    pub source_offset: i64,
    pub source_rate: Rational,
    pub source_package: String,
    pub timecode_track: String,
    pub route: String,
}
impl SourceClock {
    pub fn exact_start(&self) -> Option<Rational> {
        (self.rate.num() > 0)
            .then(|| {
                Rational::new(
                    self.effective_count as i128 * self.rate.den() as i128,
                    self.rate.num() as i128,
                )
            })
            .flatten()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceGraph {
    pub project: Project,
    pub media: Vec<SourceMedia>,
    pub items: Vec<SourceItem>,
    pub bins: Vec<Bin>,
    pub sequences: Vec<SourceSequence>,
    pub losses: Vec<Loss>,
    pub sidecars: Vec<Sidecar>,
    /// Full source angle layouts, distinct from selected leaf-media placements.
    pub multicams: Vec<Multicam>,
    pub multicam_uses: Vec<MulticamUse>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceMedia {
    pub id: String,
    pub kind: StreamKind,
    pub locations: Vec<String>,
    pub codec: Option<String>,
    pub frame_size: Option<(i64, i64)>,
    pub rate: Option<Rational>,
    pub sample_rate: Option<i64>,
    pub bits: Option<i64>,
    pub duration: Rational,
    pub start: Option<Rational>,
    pub clock: Option<SourceClock>,
    pub stream: Option<i64>,
    pub online: Option<bool>,
    /// Explicit physical original-file selection, independent of the logical lane label.
    pub original: Option<OriginalStream>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OriginalStream {
    pub saved_locators: Vec<String>,
    pub container: String,
    pub stream_index: i64,
    pub channel_index: Option<i64>,
    pub channels: Option<i64>,
    pub time_base: Rational,
    pub start_pts: i64,
    pub duration_ts: i64,
    /// Codec sample representation. AAC is compressed and has no PCM bit depth.
    pub sample_representation: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceItemMedia {
    pub media: String,
    pub gain_db: f64,
    /// Exact item-to-file source offset, or unresolved. Never infer zero from None.
    pub source_offset: Option<Rational>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceItem {
    pub id: String,
    pub name: String,
    pub media: Vec<SourceItemMedia>,
    pub umid: Option<String>,
    pub tape: Option<String>,
    pub rate: Option<Rational>,
    pub color_rgb16: Option<[i64; 3]>,
    pub stream_groups: Vec<Vec<i64>>,
    pub markers: Vec<SourceMarker>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceSequence {
    pub id: String,
    pub name: String,
    pub rate: Rational,
    /// Qualified native canvas; absence does not imply a destination default.
    pub frame_size: Option<(i64, i64)>,
    pub start: Option<Rational>,
    pub clock: Option<SourceClock>,
    pub tracks: Vec<SourceTrack>,
    pub placements: Vec<Placement>,
    /// Retained cuts explicitly disabled in the source timeline.
    pub disabled_placements: Vec<Placement>,
    pub transitions: Vec<SourceTransition>,
    pub markers: Vec<SourceMarker>,
    /// Source text occurrences, independent of underlying picture/media edits.
    pub texts: Vec<SourceText>,
}

/// A timeline lane, including declared lanes with no portable placements.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceTrack {
    pub kind: StreamKind,
    pub index: i64,
    pub level_db: Option<f64>,
    pub name: Option<String>,
    /// True only when the source confirms a plain empty lane. Absence of a
    /// portable placement does not establish that a lane was empty.
    pub empty: bool,
}

impl SourceTrack {
    pub fn new(kind: StreamKind, index: i64, level_db: Option<f64>) -> Self {
        Self { kind, index, level_db, name: None, empty: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind { Caption, Graphic }
impl TextKind {
    pub fn as_str(self) -> &'static str { match self { Self::Caption => "caption", Self::Graphic => "graphic" } }
}
/// Text content and its qualified record span. Appearance is not implied.
/// Missing content or timing is retained with a scoped explanation.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceText {
    pub id: String,
    pub kind: TextKind,
    pub track: i64,
    pub content: Option<String>,
    pub record_start: Option<Rational>,
    pub duration: Option<Rational>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceBinding {
    pub placements: Vec<String>,
    /// None means resolved; a nonempty reason preserves an unresolved binding.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceTransition {
    pub id: String,
    pub kind: StreamKind,
    pub track: i64,
    pub cut: Rational,
    pub duration: Rational,
    pub cut_offset: Rational,
    pub style: String,
    pub outgoing: SourceBinding,
    pub incoming: SourceBinding,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceMarker {
    pub name: String,
    pub comment: String,
    pub user: String,
    pub color: Option<Label>,
    /// Qualified record/item-relative time, or unresolved.
    pub start: Option<Rational>,
    pub duration: Rational,
    pub track: Option<TrackRef>,
    pub owner: MarkerOwner,
    /// Input-revision-scoped native occurrence, not an object address.
    pub owner_occurrence: String,
    pub owner_local: Option<Rational>,
    pub owner_rate: Option<Rational>,
    pub binding: SourceBinding,
    pub source: Option<Rational>,
    pub inherited: bool,
}

impl SourceGraph {
    pub fn sidecar(&self, adapter: &str, object: &str) -> Option<&Value> {
        self.sidecars
            .iter()
            .find(|s| s.adapter == adapter && s.object == object)
            .map(|s| &s.data)
    }
    pub fn losses_for<'a>(
        &'a self,
        object: &'a str,
        aspect: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.losses
            .iter()
            .filter(move |l| l.object == object && l.aspect == aspect)
            .map(|l| l.reason.as_str())
    }

    pub fn validate(&self) -> R<()> {
        validate::graph(self)
    }

    /// Count the bounded encoding without constructing a graph-wide Value tree.
    pub fn encoded_len(&self) -> R<usize> {
        self.validate()?;
        encode::encoded_len(self)
    }

    pub fn write_json(&self, writer: &mut impl Write) -> R<()> {
        self.encoded_len()?;
        encode::write_graph(self, writer)
    }

    pub fn to_json_text(&self) -> R<String> {
        let len = self.encoded_len()?;
        let mut bytes = Vec::with_capacity(len);
        encode::write_graph(self, &mut bytes)?;
        String::from_utf8(bytes).map_err(|_| GraphError("source graph output is not UTF-8".into()))
    }

    pub fn from_json_text(text: &str) -> R<Self> {
        decode::from_text(text)
    }

    pub fn from_json(value: &Value) -> R<Self> {
        decode::from_value(value)
    }
}

/// Encode one source receipt for the private native-conversion spool, with the
/// same number and string encodings and per-record bound used by graph/2.
pub fn write_source_receipt(writer: &mut impl Write, owner: &str, data: &Value) -> R<usize> {
    encode::write_receipt(writer, owner, data)
}

#[cfg(test)]
mod tests;
