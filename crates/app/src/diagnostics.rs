//! Shareable diagnostics. Only typed facts and application-owned text cross this boundary.
use crate::job::{
    self, BlockerKind, Completion, CompletionStatus, ReportNote, Review, ReviewBlocker, ReviewGroup,
};
use graph::{Rational, StreamKind};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs::File, io::Read, path::Path};

const MAX_FINDINGS: usize = 4096;
const MAX_BYTES: u64 = 4 << 20;

pub(crate) fn app_version() -> &'static str {
    // The GUI package is the release version authority for the bundled CLI too.
    include_str!("../../gui/Cargo.toml")
        .lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("GUI package version")
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum Schema {
    #[serde(rename = "anynle.diagnostics/1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Route {
    AvidToPremiere,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Review,
    Conversion,
    RelinkVerification,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Review => "Preflight Review",
            Self::Conversion => "Latest conversion",
            Self::RelinkVerification => "Relink verification",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ready,
    Blocked,
    Complete,
    Failed,
    Cancelled,
    Unknown,
    Verified,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Blocked => "Blocked",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Unknown => "Completion unknown",
            Self::Verified => "Verified",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    MissingOriginal,
    OriginalLocator,
    OriginalMedia,
    ManagedMedia,
    LegacyMxfPackage,
    MxfPackageMismatch,
    RelinkCandidateMissing,
    RelinkRejected,
    OfflineMedia,
    MediaGap,
    EffectOmission,
    TranslationLoss,
    Warning,
    PublicationWarning,
    Source,
    Destination,
    NewMedia,
    MediaTools,
    SeparateFiles,
    Timing,
    Retry,
    ConversionError,
    WorkerStopped,
}

impl Reason {
    fn from_blocker(kind: BlockerKind) -> Self {
        use BlockerKind::*;
        match kind {
            ProjectSource | SourceContent => Self::Source,
            ProjectOutput => Self::Destination,
            AvidMedia | RewrapMissingMedia => Self::ManagedMedia,
            LegacyMxfPackage => Self::LegacyMxfPackage,
            RewrapMissingOriginal => Self::MissingOriginal,
            OriginalLocator => Self::OriginalLocator,
            OriginalMedia => Self::OriginalMedia,
            NewMedia | ProxyMedia | Rewrap => Self::NewMedia,
            MediaTools => Self::MediaTools,
            Effects => Self::EffectOmission,
            GroupClipMode => Self::SeparateFiles,
            SequenceTiming => Self::Timing,
            Retry => Self::Retry,
        }
    }

    fn from_note(note: &ReportNote) -> Option<Self> {
        match (note.severity.as_str(), note.why.as_str()) {
            ("loss", "media_gap") => Some(Self::MediaGap),
            ("loss", "effect" | "effect_order" | "transition_effect" | "picture_effect") => {
                Some(Self::EffectOmission)
            }
            ("loss", _) => Some(Self::TranslationLoss),
            ("warning", "Link the verified source in Premiere when it is available.") => {
                Some(Self::OfflineMedia)
            }
            (
                "warning",
                "Successful project publication may retain a staging file after cleanup failure.",
            ) => Some(Self::PublicationWarning),
            ("warning", _) => Some(Self::Warning),
            ("error", _) => Some(Self::ConversionError),
            _ => None,
        }
    }

    fn help(self) -> (&'static str, &'static str) {
        match self {
            Self::MissingOriginal => ("A saved original is unavailable.", "Open saved-path mappings in Media, then check again."),
            Self::OriginalLocator => ("The saved original path could not be resolved uniquely.", "Open saved-path mappings in Media."),
            Self::OriginalMedia => ("The candidate did not pass the original-media checks.", "Open saved-path mappings and check the source file."),
            Self::ManagedMedia => ("A required Avid-managed source or sibling could not be verified.", "Open media locations and check the Avid media folders."),
            Self::LegacyMxfPackage => ("A saved MXF package could not be resolved.", "Open media locations and locate the matching Avid MXF."),
            Self::MxfPackageMismatch => ("The candidate MXF does not have the expected package identity and single essence track.", "Choose the matching Avid MXF sibling and verify it again."),
            Self::RelinkCandidateMissing => ("A proposed file is unavailable.", "Make the source available or choose its current location."),
            Self::RelinkRejected => ("The proposed files did not pass relink verification.", "Run the local relink verifier for detailed reasons before linking in Premiere."),
            Self::OfflineMedia => ("The project carries an offline media reference.", "Verify the source when it arrives, then use Link Media in Premiere."),
            Self::MediaGap => ("A media occurrence was omitted and its saved span is reported as a gap.", "Inspect the marked edit and make the required media available."),
            Self::EffectOmission => ("An effect could not be carried over as saved.", "Inspect the affected edit in the full preflight report."),
            Self::TranslationLoss => ("Review recorded a conversion change or omission.", "Inspect the affected edit in the full preflight report."),
            Self::Warning => ("Review recorded a warning.", "Open the full preflight report."),
            Self::PublicationWarning => ("Project publication recorded a cleanup warning.", "Look at the conversion report for retained files."),
            Self::Source => ("The selected Avid project or edit content needs attention.", "Check the source selection and affected details in Review."),
            Self::Destination => ("The project destination failed preflight.", "Choose a new project path."),
            Self::NewMedia => ("The selected rewrap or proxy settings need attention.", "Check the media folders and rewrap settings in Media."),
            Self::MediaTools => ("The media tools are unavailable or unsuitable.", "Open media tools and check the installed backend."),
            Self::SeparateFiles => ("The selected layout cannot carry these editable Groups.", "Switch to Separate files."),
            Self::Timing => ("The saved timing could not be represented safely.", "Inspect the affected timelines in Review."),
            Self::Retry => ("Preflight could not finish.", "Inspect Activity and check again."),
            Self::ConversionError => ("Conversion reported an execution error.", "Open Activity and check the destination for retained outputs."),
            Self::WorkerStopped => ("The conversion worker stopped before reporting the result.", "Inspect the destination and engine receipts before retrying."),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Warning,
    Loss,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
    Caption,
}

impl From<StreamKind> for TrackKind {
    fn from(kind: StreamKind) -> Self {
        match kind {
            StreamKind::Video => Self::Video,
            StreamKind::Audio => Self::Audio,
        }
    }
}

impl TrackKind {
    fn prefix(self) -> char {
        match self {
            Self::Video => 'V',
            Self::Audio => 'A',
            Self::Caption => 'C',
        }
    }
}

// Reduced exact seconds, or frames per second, without free-form input text.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fraction {
    numerator: i64,
    denominator: i64,
}

impl From<Rational> for Fraction {
    fn from(value: Rational) -> Self {
        Self {
            numerator: value.num(),
            denominator: value.den(),
        }
    }
}

impl std::fmt::Display for Fraction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.denominator == 1 {
            write!(f, "{}", self.numerator)
        } else {
            write!(f, "{}/{}", self.numerator, self.denominator)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedLocation {
    pub(crate) kind: Option<TrackKind>,
    pub(crate) track: Option<i64>,
    pub(crate) record_offset: Option<Fraction>,
    pub(crate) timeline_start: Option<Fraction>,
    pub(crate) rate: Option<Fraction>,
    pub(crate) display_base: Option<u16>,
    pub(crate) drop_frame: Option<bool>,
}

impl SavedLocation {
    pub(crate) fn from_sequence(
        sequence: &graph::source::SourceSequence,
        kind: impl Into<TrackKind>,
        track: i64,
        offset: Option<Rational>,
    ) -> Self {
        let clock = sequence
            .clock
            .as_ref()
            .and_then(|clock| Some((clock.exact_start()?, clock.display_base, clock.drop)));
        let mut location = Self::new(kind, track, offset, Some(sequence.rate), clock);
        location.timeline_start = sequence.start.map(Into::into).or(location.timeline_start);
        location
    }

    pub(crate) fn new(
        kind: impl Into<TrackKind>,
        track: i64,
        offset: Option<Rational>,
        rate: Option<Rational>,
        clock: Option<(Rational, u16, bool)>,
    ) -> Self {
        let (start, base, drop) = clock.map_or((None, None, None), |(start, base, drop)| {
            (Some(start), Some(base), Some(drop))
        });
        Self {
            kind: Some(kind.into()),
            track: Some(track),
            record_offset: offset.map(Into::into),
            rate: rate.map(Into::into),
            timeline_start: start.map(Into::into),
            display_base: base,
            drop_frame: drop,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Media {
    pub direct_files: usize,
    pub needs_premiere: usize,
    pub premiere_linked: usize,
    pub rewrap_available: usize,
    pub rewrap_selected: usize,
    pub conversion_pending: usize,
    pub camera_originals: usize,
    pub attached_proxies: usize,
    pub offline_streams: usize,
    pub managed_rewrap_planned: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub id: usize,
    pub timeline: Option<usize>,
    pub source: Option<usize>,
    location: Option<SavedLocation>,
    severity: Severity,
    reason: Reason,
    explanation: String,
    wizard_action: String,
}

impl Finding {
    pub fn explanation(&self) -> &'static str {
        self.reason.help().0
    }
    pub fn action(&self) -> &'static str {
        self.reason.help().1
    }

    pub fn subject(&self) -> String {
        let mut label = self
            .timeline
            .map(|id| format!("Timeline {id}"))
            .unwrap_or_default();
        if let Some(location) = self.location {
            if let (Some(kind), Some(track)) = (location.kind, location.track) {
                if !label.is_empty() {
                    label.push_str(" · ");
                }
                label.push_str(&format!("{}{track}", kind.prefix()));
            }
            if let Some(offset) = location.record_offset {
                label.push_str(&format!(" · {offset} s from timeline start"));
            }
        }
        if let Some(id) = self.source {
            if !label.is_empty() {
                label.push_str(" · ");
            }
            label.push_str(&format!("Source {id}"));
        }
        label
    }

    pub(crate) fn tool_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("typed finding serializes")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Diagnostics {
    schema: Schema,
    app_version: String,
    route: Route,
    pub stage: Stage,
    pub outcome: Outcome,
    pub project_published: Option<bool>,
    pub omitted_findings: usize,
    media: Option<Media>,
    pub findings: Vec<Finding>,
    verified_files: Option<usize>,
}

impl Diagnostics {
    fn new(stage: Stage, outcome: Outcome) -> Self {
        Self {
            schema: Schema::V1,
            app_version: app_version().into(),
            route: Route::AvidToPremiere,
            stage,
            outcome,
            project_published: None,
            omitted_findings: 0,
            media: None,
            findings: Vec::new(),
            verified_files: None,
        }
    }

    fn push(
        &mut self,
        reason: Reason,
        severity: Severity,
        timeline: Option<usize>,
        source: Option<usize>,
        location: Option<SavedLocation>,
    ) {
        if self.findings.len() == MAX_FINDINGS {
            self.omitted_findings += 1;
            return;
        }
        let (explanation, wizard_action) = reason.help();
        self.findings.push(Finding {
            id: self.findings.len() + 1,
            timeline,
            source,
            location,
            severity,
            reason,
            explanation: explanation.into(),
            wizard_action: wizard_action.into(),
        });
    }

    pub fn ready(review: &Review) -> Self {
        let mut result = Self::new(Stage::Review, Outcome::Ready);
        let linked = review.media_linking.as_ref();
        let offline_streams = review
            .notes
            .iter()
            .filter(|note| Reason::from_note(note) == Some(Reason::OfflineMedia))
            .count();
        result.media = Some(Media {
            direct_files: linked.map_or(0, |media| media.direct_files),
            needs_premiere: linked.map_or(0, |media| media.premiere_files.len()),
            premiere_linked: 0,
            rewrap_available: linked.map_or(0, |media| media.rewrap_available.len()),
            rewrap_selected: linked.map_or(0, |media| media.rewrapped_clips.len()),
            conversion_pending: linked.map_or(0, |media| media.conversion_pending.len()),
            camera_originals: review.camera_originals.len(),
            attached_proxies: review
                .camera_originals
                .iter()
                .filter(|link| link.proxy.is_some())
                .count(),
            offline_streams,
            managed_rewrap_planned: linked.is_none() && !review.rewrap_sources.is_empty(),
        });
        let timelines = aliases(&review.timeline_groups);
        let sources = aliases(&review.source_groups);
        for (index, note) in review.notes.iter().enumerate() {
            let Some(reason) = Reason::from_note(note) else {
                continue;
            };
            let severity = if note.severity == "loss" {
                Severity::Loss
            } else {
                Severity::Warning
            };
            result.push(
                reason,
                severity,
                timelines.get(&index).copied(),
                sources.get(&index).copied(),
                review.diagnostic_locations.get(&index).copied(),
            );
        }
        result
    }

    pub fn blocked(blocker: &ReviewBlocker) -> Self {
        let mut result = Self::new(Stage::Review, Outcome::Blocked);
        let reason = blocker
            .diagnostic_reason
            .unwrap_or_else(|| Reason::from_blocker(blocker.kind));
        let timelines = aliases(&blocker.timeline_groups);
        if blocker.affected.is_empty() {
            result.push(reason, Severity::Error, None, None, None);
        }
        for index in 0..blocker.affected.len() {
            result.push(
                reason,
                Severity::Error,
                timelines.get(&index).copied(),
                None,
                blocker.diagnostic_locations.get(&index).copied(),
            );
        }
        result
    }

    pub fn completion(mut self, completion: &Completion) -> Self {
        self.stage = Stage::Conversion;
        self.outcome = match completion.status {
            CompletionStatus::Complete => Outcome::Complete,
            CompletionStatus::Failed => Outcome::Failed,
            CompletionStatus::Cancelled => Outcome::Cancelled,
        };
        self.project_published = Some(completion.output.is_some());
        if let Some(media) = self.media.as_mut() {
            media.premiere_linked = completion.report.iter().filter(|note|
                note.why == "Verified against the Avid graph and the installed helper's saved media record."
            ).count();
            media.needs_premiere = media.needs_premiere.saturating_sub(media.premiere_linked);
        }
        for note in &completion.report {
            if Reason::from_note(note) == Some(Reason::PublicationWarning) {
                self.push(
                    Reason::PublicationWarning,
                    Severity::Warning,
                    None,
                    None,
                    None,
                );
            }
        }
        if completion.status == CompletionStatus::Failed {
            self.push(Reason::ConversionError, Severity::Error, None, None, None);
        }
        self
    }

    pub fn worker_stopped(mut self) -> Self {
        self.stage = Stage::Conversion;
        self.outcome = Outcome::Unknown;
        self.push(Reason::WorkerStopped, Severity::Error, None, None, None);
        self
    }

    pub(crate) fn relink(result: Result<usize, Reason>) -> Self {
        match result {
            Ok(count) => {
                let mut report = Self::new(Stage::RelinkVerification, Outcome::Verified);
                report.verified_files = Some(count);
                report
            }
            Err(reason) => {
                let mut report = Self::new(Stage::RelinkVerification, Outcome::Blocked);
                report.push(reason, Severity::Error, None, None, None);
                report
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            return Err("Choose a .json path for diagnostics".into());
        }
        job::write_json(path, self, &mut Vec::new())
    }

    pub(crate) fn load(path: &Path) -> Result<Self, &'static str> {
        if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
            return Err("Diagnostics must be a regular file.");
        }
        let file =
            File::open(path).map_err(|_| "Could not open the selected diagnostics report.")?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read the selected diagnostics report.")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Diagnostics exceed 4 MiB.");
        }
        let report: Self =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid Any NLE diagnostics report.")?;
        let bad_finding = report.findings.iter().enumerate().any(|(index, finding)| {
            finding.id != index + 1
                || finding.timeline == Some(0)
                || finding.source == Some(0)
                || (finding.explanation.as_str(), finding.wizard_action.as_str())
                    != finding.reason.help()
                || finding.location.is_some_and(|location| {
                    [
                        location.record_offset,
                        location.timeline_start,
                        location.rate,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|value| value.denominator <= 0)
                        || location.rate.is_some_and(|rate| rate.numerator <= 0)
                })
        });
        if report.findings.len() > MAX_FINDINGS
            || !valid_version(&report.app_version)
            || bad_finding
        {
            return Err("Invalid Any NLE diagnostics report.");
        }
        Ok(report)
    }

    pub(crate) fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": self.schema, "app_version": self.app_version, "route": self.route,
            "stage": self.stage, "outcome": self.outcome, "project_published": self.project_published,
            "media": self.media, "verified_files": self.verified_files,
            "findings": self.findings.len(), "omitted_findings": self.omitted_findings,
        })
    }

    pub fn media_summary(&self) -> Option<String> {
        let m = self.media?;
        let mut parts = Vec::new();
        if m.managed_rewrap_planned {
            parts.push("Managed media rewrap planned".into());
        }
        for (count, label) in [
            (m.direct_files, "direct links"),
            (m.needs_premiere, "need Premiere"),
            (m.premiere_linked, "linked by Premiere"),
            (m.offline_streams, "offline streams"),
            (m.camera_originals, "camera originals"),
            (m.attached_proxies, "proxies"),
            (m.rewrap_selected, "selected for rewrap"),
            (m.rewrap_available, "rewrap candidates"),
            (m.conversion_pending, "pending"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {label}"));
            }
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

fn valid_version(version: &str) -> bool {
    let (core, suffix) = version
        .split_once('-')
        .map_or((version, None), |(core, suffix)| (core, Some(suffix)));
    let numeric = |s: &str| !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit());
    core.split('.').count() == 3
        && core.split('.').all(numeric)
        && suffix.is_none_or(|s| {
            s.split_once('.')
                .is_some_and(|(tag, n)| matches!(tag, "alpha" | "beta" | "rc") && numeric(n))
        })
}

fn aliases(groups: &[ReviewGroup]) -> HashMap<usize, usize> {
    let mut result = HashMap::new();
    for (index, group) in groups.iter().enumerate() {
        for &finding in &group.note_indices {
            result.entry(finding).or_insert(index + 1);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_uses_only_allowlisted_fields_and_preserves_exact_clock() {
        let private =
            r#"Private "marker" — C:\shoot\jobs\secret.avb /Volumes/takes/private"?token=secret"#;
        let mut blocker = ReviewBlocker::new(BlockerKind::OriginalLocator, private);
        blocker.diagnostic_reason = Some(Reason::MissingOriginal);
        blocker.affected.push(private.into());
        blocker.push_diagnostic_edit(
            private.into(),
            SavedLocation::new(
                StreamKind::Video,
                2,
                Rational::new(1001, 30000),
                Rational::new(30000, 1001),
                Some((Rational::integer(3600), 30, true)),
            ),
        );
        blocker.timeline_groups.push(ReviewGroup {
            id: private.into(),
            name: private.into(),
            bin: Some(private.into()),
            note_indices: vec![1],
        });
        let report = Diagnostics::blocked(&blocker);
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["outcome"], "blocked");
        assert_eq!(value["findings"][0]["location"], serde_json::Value::Null);
        assert_eq!(
            value["findings"][1]["location"]["record_offset"]["numerator"],
            1001
        );
        assert_eq!(
            value["findings"][1]["location"]["record_offset"]["denominator"],
            30000
        );
        assert_eq!(value["findings"][1]["location"]["drop_frame"], true);
        assert_eq!(
            report.findings[1].subject(),
            "Timeline 1 · V2 · 1001/30000 s from timeline start"
        );
        assert!(!value.to_string().contains("secret"));
        let legacy = serde_json::to_value(&blocker).unwrap();
        assert!(legacy.get("diagnostic_locations").is_none());
        assert!(legacy.get("diagnostic_reason").is_none());

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("support.json");
        report.save(&path).unwrap();
        assert_eq!(Diagnostics::load(&path).unwrap(), report);
        let before = std::fs::read(&path).unwrap();
        assert!(report.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let mut malicious = value;
        malicious["findings"][0]["explanation"] = private.into();
        std::fs::write(&path, malicious.to_string()).unwrap();
        assert_eq!(
            Diagnostics::load(&path).unwrap_err(),
            "Invalid Any NLE diagnostics report."
        );
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert_eq!(
            Diagnostics::load(&path).unwrap_err(),
            "Diagnostics exceed 4 MiB."
        );
    }
}
