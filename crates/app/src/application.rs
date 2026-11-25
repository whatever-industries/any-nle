//! any-nle: one command from source program to target program, through the graph.
//!
//! Avid source facts -> graph/2 -> Premiere preparation -> one encoded document.
//! The held FCPXML route retains its explicit legacy graph/1 input profile.


use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use avid_reader::extract::extract_project_paths;
use avid_reader::model::{build, MediaIndex};
use graph::{Graph, Loss};
use mapping::{AvidToPremiere, BinMode};
use crate::premiere_route::PremiereRoute;
use crate::{media_prompt, output, premiere_preparation, rewrap, source_input};
use prproj::convert::{
    model::{parse_model_str, Model, MAX_MODEL_BYTES},
    prepared::PreparedProjectInput,
    writer::Editorial,
};
use prproj::{RandomUuids, SeededUuids, UuidSource};

pub(crate) const USAGE: &str = "usage:
  any-nle review --job SETTINGS.json
      Print the saved job's read-only preflight report (JSON).
  any-nle mcp --diagnostics REPORT.json [--job SETTINGS.json]
                  [--relink-report RELINK.json --moves MOVES.json [--checksums TRUSTED.json]]
      Serve redacted support diagnostics to a local MCP client over stdio.
  any-nle run --job SETTINGS.json [--output NEW_PROJECT] [--new-media NEW_DIR]
      Run a saved native-wizard job with the same read-only preparation and explicit execution.
  any-nle verify-offline-relink REPORT.json MOVES.json [--checksums TRUSTED.json] [--ffprobe PATH]
      Verify proposed files before using Premiere's Link Media. Writes no files.
  any-nle convert --from avid BIN.avb... --media DIR [--media DIR...] [--project NAME]
                  --to premiere OUTPUT.prproj [--policy POLICY.json] [--bin-mode separate|grouped|merged]
                  [--match-avid-default-pan] [--graph GRAPH.json] [--seed SEED]
                  [--link-originals [--map-original-root SAVED_ROOT CURRENT_ROOT]...]
                  [--prefer-source-media [--attach-avid-proxies [--rewrap-avid-proxies EXISTING_DIR]]]
                  [--allow-offline-media]
                  [--rewrap-op1a NEW_DIR | --rewrap-op1a-prompt | --rewrap-media NEW_DIR | --rewrap-needed NEW_DIR] [--rewrap-dry-run] [--rewrap-native-master] [--ffmpeg PATH] [--ffprobe PATH]
      Convert Avid bins to a Premiere project. Prints what was converted, the reader's problems
      and every loss. --graph saves the qualified source anynle.graph/2 (Premiere route).
      --link-originals: Resolve qualified saved file links alongside direct Avid media; --media is optional for original-only bins.
      Linked conversions report omitted edits and mark verified missing spans.
      --prefer-source-media: Try Avid Source File / Source Path for managed clips before linking their managed media. Implies --link-originals.
      --attach-avid-proxies: Attach matching Avid MXF by package identity. Implies --prefer-source-media.
      --rewrap-avid-proxies EXISTING_DIR: Creates new media when stereo requires it.
      --map-original-root: Map a saved path prefix to an absolute current folder. Repeat as needed.
      --rewrap-media NEW_DIR: Creates new media. DNx to MXF OP1a; qualified ProRes to MOV. Requires --rewrap-native-master.
      --rewrap-needed NEW_DIR: Keep qualified direct links and rewrap only the managed clips that need it.
      --rewrap-op1a: Creates new media. Copies supported OP-Atom streams into NEW_DIR.
      --rewrap-op1a-prompt: macOS folder picker and space review before creating media.
      --rewrap-dry-run: preview only; creates no media/project/graph. Requires a rewrap destination option.
      --rewrap-native-master: experimental file-backed AV masters preserving bin audio groups; replaces bin mode.
      Selective rewrap accepts verified DNx and ProRes picture and up to 64 mono PCM48k inputs.
      Rewrap requires FFmpeg/ffprobe 8.1; bounded native read-backs pass.
  any-nle convert --from avid BIN.avb... --media DIR [--media DIR...] [--project NAME]
                  --to fcpxml OUTPUT.fcpxml [--graph GRAPH.json]
      Convert Avid bins to FCPXML 1.9 (DaVinci Resolve, Final Cut Pro).
  any-nle graph --from avid BIN.avb... --media DIR [--media DIR...] [--project NAME]
      Print the qualified source anynle.graph/2 of the bins (JSON).
  any-nle colours GRAPH.json [--policy POLICY.json]
      The colour mapper's table: what each Avid marker and clip colour becomes in Premiere.
  any-nle policy
      Print the default Avid -> Premiere policy (JSON), to edit and pass with --policy.";

pub(crate) fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("any-nle: {message}");
    ExitCode::from(1)
}

pub(crate) fn usage(message: &str) -> ExitCode {
    eprintln!("{USAGE}");
    if !message.is_empty() {
        eprintln!("any-nle: error: {message}");
    }
    ExitCode::from(2)
}

#[derive(Default, Clone)]
pub(crate) struct Args {
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) bins: Vec<String>,
    pub(crate) media: Vec<String>,
    pub(crate) project: Option<String>,
    pub(crate) policy: Option<PathBuf>,
    pub(crate) bin_mode: Option<String>,
    pub(crate) match_pan: bool,
    pub(crate) graph_out: Option<PathBuf>,
    pub(crate) seed: Option<String>,
    pub(crate) rewrap: Option<PathBuf>,
    pub(crate) rewrap_needed: Option<PathBuf>,
    pub(crate) rewrap_media: bool,
    pub(crate) rewrap_dry_run: bool,
    pub(crate) rewrap_prompt: bool,
    pub(crate) rewrap_native_master: bool,
    pub(crate) ffmpeg: Option<PathBuf>,
    pub(crate) ffprobe: Option<PathBuf>,
    pub(crate) link_originals: bool,
    pub(crate) prefer_source_media: bool,
    pub(crate) attach_avid_proxies: bool,
    pub(crate) proxy_rewrap_dir: Option<PathBuf>,
    pub(crate) allow_offline_media: bool,
    pub(crate) original_mappings: Vec<avid_adapter::original::RootMapping>,
}

pub(crate) fn parse(rest: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            a.bins.push(arg.clone());
            continue;
        }
        if arg == "--rewrap-native-master" {
            if a.rewrap_native_master {
                return Err("--rewrap-native-master given twice".into());
            }
            a.rewrap_native_master = true;
            continue;
        }
        if arg == "--rewrap-op1a-prompt" {
            if a.rewrap_prompt {
                return Err("--rewrap-op1a-prompt given twice".into());
            }
            a.rewrap_prompt = true;
            continue;
        }
        if arg == "--rewrap-dry-run" {
            a.rewrap_dry_run = true;
            continue;
        }
        if arg == "--match-avid-default-pan" {
            a.match_pan = true;
            continue;
        }
        if arg == "--link-originals" {
            if a.link_originals {
                return Err("--link-originals given twice".into());
            }
            a.link_originals = true;
            continue;
        }
        if arg == "--prefer-source-media" {
            if a.prefer_source_media {
                return Err("--prefer-source-media given twice".into());
            }
            a.prefer_source_media = true;
            continue;
        }
        if arg == "--attach-avid-proxies" {
            if a.attach_avid_proxies { return Err("--attach-avid-proxies given twice".into()); }
            a.attach_avid_proxies = true;
            continue;
        }
        if arg == "--allow-offline-media" {
            if a.allow_offline_media {
                return Err("--allow-offline-media given twice".into());
            }
            a.allow_offline_media = true;
            continue;
        }
        let value = it
            .next()
            .cloned()
            .ok_or_else(|| format!("{arg} needs a value"))?;
        if matches!(arg.as_str(), "--rewrap-op1a" | "--rewrap-media" | "--rewrap-needed" | "--ffmpeg" | "--ffprobe" | "--map-original-root")
            && value.starts_with("--")
        {
            return Err(format!("{arg} needs a path, not another option"));
        }
        match arg.as_str() {
            "--from" => a.from = Some(value),
            "--to" => {
                a.to = Some(value);
                match it.next() {
                    Some(out) if !out.starts_with("--") => a.output = Some(PathBuf::from(out)),
                    _ => {
                        return Err(
                            "--to PROGRAM needs OUTPUT after it (--to premiere OUTPUT.prproj)"
                                .into(),
                        )
                    }
                }
            }
            "--media" => a.media.push(value),
            "--project" => a.project = Some(value),
            "--policy" => a.policy = Some(PathBuf::from(value)),
            "--bin-mode" => a.bin_mode = Some(value),
            "--graph" => a.graph_out = Some(PathBuf::from(value)),
            "--seed" => a.seed = Some(value),
            "--map-original-root" => {
                let current = it.next().filter(|path| !path.starts_with("--"))
                    .ok_or("--map-original-root needs SAVED_ROOT CURRENT_ROOT")?;
                let current_root = PathBuf::from(current);
                if !current_root.is_absolute()
                    || current_root.components().any(|c| c == std::path::Component::ParentDir)
                {
                    return Err("--map-original-root needs an absolute CURRENT_ROOT without parent escapes".into());
                }
                a.original_mappings.push(avid_adapter::original::RootMapping {
                    saved_root: value.into(),
                    current_root,
                });
            }
            "--rewrap-avid-proxies" => {
                if a.proxy_rewrap_dir.replace(PathBuf::from(value)).is_some() {
                    return Err("--rewrap-avid-proxies given twice".into());
                }
            }
            "--rewrap-op1a" | "--rewrap-media" => {
                if a.rewrap.is_some() {
                    return Err("rewrap destination given more than once; choose one media mode".into());
                }
                a.rewrap_media = arg=="--rewrap-media";
                a.rewrap = Some(PathBuf::from(value));
            }
            "--rewrap-needed" => {
                if a.rewrap_needed.replace(PathBuf::from(value)).is_some() {
                    return Err("--rewrap-needed given twice".into());
                }
            }
            "--ffmpeg" => {
                if a.ffmpeg.is_some() {
                    return Err("--ffmpeg given twice".into());
                }
                a.ffmpeg = Some(PathBuf::from(value));
            }
            "--ffprobe" => {
                if a.ffprobe.is_some() {
                    return Err("--ffprobe given twice".into());
                }
                a.ffprobe = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if a.proxy_rewrap_dir.is_some() && !a.attach_avid_proxies {
        return Err("--rewrap-avid-proxies requires --attach-avid-proxies; creates new media".into());
    }
    a.prefer_source_media |= a.attach_avid_proxies;
    if a.prefer_source_media {
        if a.wants_rewrap() || a.to.as_deref() != Some("premiere") {
            return Err("--prefer-source-media requires direct Avid to Premiere conversion without rewrap".into());
        }
        a.link_originals = true;
    }
    if a.rewrap_prompt && a.rewrap.is_some() {
        return Err("choose an explicit rewrap directory or --rewrap-op1a-prompt, not both".into());
    }
    if a.rewrap_needed.is_some() && (a.wants_rewrap() || a.rewrap_native_master
        || a.to.as_deref() != Some("premiere") || a.seed.is_some() || a.graph_out.is_some())
    {
        return Err("--rewrap-needed requires Avid to Premiere linked conversion and cannot combine with full rewrap, --seed or --graph".into());
    }
    if !a.wants_rewrap() && a.rewrap_needed.is_none()
        && (a.rewrap_dry_run || a.rewrap_native_master || (a.ffmpeg.is_some() && a.proxy_rewrap_dir.is_none()))
    {
        return Err("rewrap settings require --rewrap-op1a NEW_DIR, --rewrap-media NEW_DIR or --rewrap-op1a-prompt".into());
    }
    if a.ffprobe.is_some() && a.to.as_deref() != Some("premiere") {
        return Err("--ffprobe is only supported for Avid → Premiere conversion".into());
    }
    if a.rewrap_media && !a.rewrap_native_master { return Err("--rewrap-media requires --rewrap-native-master".into()); }
    if a.rewrap_native_master && a.bin_mode.is_some() {
        return Err("--rewrap-native-master replaces bin mode; omit --bin-mode".into());
    }
    if a.allow_offline_media && (a.wants_rewrap() || a.to.as_deref() != Some("premiere")) {
        return Err("--allow-offline-media requires direct Avid → Premiere conversion in Separate, Grouped or Merged mode".into());
    }
    if !a.original_mappings.is_empty() && !a.link_originals && !a.allow_offline_media {
        return Err("--map-original-root requires --link-originals or --allow-offline-media".into());
    }
    if a.link_originals && (a.wants_rewrap() || a.to.as_deref() != Some("premiere")) {
        return Err("--link-originals requires direct convert --to premiere without rewrap".into());
    }
    Ok(a)
}

impl Args {
    pub(crate) fn wants_rewrap(&self) -> bool {
        self.rewrap.is_some() || self.rewrap_prompt
    }
}

/// Read Avid bins into the graph. Returns the graph and the reader's problems.
pub(crate) fn read_avid_legacy(a: &Args) -> Result<(Graph, Vec<String>), String> {
    if a.from.as_deref() != Some("avid") {
        return Err("--from: only avid is supported so far".into());
    }
    if a.bins.is_empty() || a.media.is_empty() {
        return Err("give at least one BIN.avb and one --media folder".into());
    }
    let selected = avid_reader::inputs::unique_bins(&a.bins);
    let contents = extract_project_paths(&selected.paths).map_err(|e| e.to_string())?;
    if a.wants_rewrap() {
        rewrap::validate_bin_audio(&contents)?;
    }
    let roots: Vec<&Path> = a.media.iter().map(Path::new).collect();
    let index = MediaIndex::scan_all(&roots).map_err(|e| e.to_string())?;
    let bin_names: Vec<String> = selected
        .paths
        .iter()
        .map(|b| {
            b.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let project = a.project.clone().unwrap_or_else(|| bin_names[0].clone());
    // The reader's bin mode only fills model/1's options, which the policy replaces.
    let conversion = build(&contents, &index, &project, &bin_names, "separate");
    let mut problems = conversion.problems;
    problems.extend(
        selected
            .skipped
            .iter()
            .map(|p| format!("repeated bin path skipped: {}", p.display())),
    );
    problems.extend(
        index
            .unreadable
            .iter()
            .map(|(p, e)| format!("unreadable MXF {}: {e}", p.display())),
    );
    problems.extend(
        index
            .ambiguous
            .iter()
            .map(|a| format!("ambiguous media identity: {a}")),
    );
    // Ordinary conversion may omit missing sound or skip unsupported clips. A rewrap
    // must not mistake that reduced model for the complete selected source set.
    if a.wants_rewrap() {
        let substantive: Vec<_> = problems
            .iter()
            .filter(|p| !p.starts_with("repeated bin path skipped: "))
            .collect();
        if !substantive.is_empty() {
            return Err(format!(
                "OP1a preparation requires a reader/media scan without problems; no media created:\n  {}",
                substantive.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
            ));
        }
    }
    let text = conversion.model.to_pretty();
    // The same limit prproj-write's load_model applies, so both pipelines refuse alike.
    if text.len() as u64 > MAX_MODEL_BYTES {
        return Err("reader output: model file exceeds 16 MiB".into());
    }
    let mut model = parse_model_str(&text).map_err(|e| format!("reader output: {e}"))?;
    if a.wants_rewrap() {
        problems.extend(rewrap::normalize_source_clocks(&mut model)?);
    }
    let (graph, _reader_options) = bridge::to_graph(&model).map_err(|e| e.to_string())?;
    Ok((graph, problems))
}

struct AvidRead {
    result: avid_adapter::ReadResult,
    inputs: Vec<avid_reader::source::SourceInput>,
    skipped_bins: Vec<PathBuf>,
}

fn read_avid_source(a: &Args, link_managed_op1a: bool) -> Result<AvidRead, String> {
    if a.from.as_deref() != Some("avid") {
        return Err("--from: only avid is supported so far".into());
    }
    if a.bins.is_empty() || (a.media.is_empty() && !a.link_originals && !a.allow_offline_media) {
        return Err("give at least one BIN.avb and a --media folder or --link-originals".into());
    }
    let selected = avid_reader::inputs::unique_bins(&a.bins);
    let inputs = source_input::pin(&selected.paths)?;
    let names: Vec<String> = selected
        .paths
        .iter()
        .map(|b| {
            b.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let project = a.project.clone().unwrap_or_else(|| names[0].clone());
    let roots: Vec<PathBuf> = a.media.iter().map(PathBuf::from).collect();
    let link_dry_gain = a.to.as_deref() == Some("premiere") && !a.wants_rewrap();
    let result = if a.link_originals || a.allow_offline_media {
        let tools = crate::backend_bundle::resolve(a.ffmpeg.as_deref(), a.ffprobe.as_deref(), a.proxy_rewrap_dir.is_some())?;
        avid_adapter::read_with_media_policy(
            &inputs, &roots, &project, &names,
            &avid_adapter::original::OriginalOptions {
                link_managed_op1a,
                link_dry_gain,
                root_mappings: a.original_mappings.clone(),
                ffprobe: tools.ffprobe,
                prefer_source_media: a.prefer_source_media,
                attach_avid_proxies: a.attach_avid_proxies,
                proxy_rewrap_dir: a.proxy_rewrap_dir.clone(),
                ffmpeg: tools.ffmpeg,
                allow_media_gaps: link_dry_gain,
                preserve_managed_audio_without_picture: link_managed_op1a && a.link_originals,
                collect_blocked_locators: false,
                verify_linked_imports: false,
            }, a.allow_offline_media, link_dry_gain, &mut |_, _| Ok(()), &mut || Ok(()),
        ).map_err(avid_adapter::SourceReadIssue::into_message)?
    } else {
        let ffprobe = if link_managed_op1a {
            match a.ffprobe.as_ref() {
                Some(path) => path.clone(),
                None => crate::backend_bundle::resolve(None, None, false)?.ffprobe,
            }
        } else {
            a.ffprobe.clone().unwrap_or_else(|| PathBuf::from("ffprobe"))
        };
        avid_adapter::read_managed_for_linking(
            &inputs, &roots, &project, &names, &ffprobe, link_managed_op1a, link_dry_gain,
        )?
    };
    source_input::recheck(&inputs)?;
    // Enforce the encoded ceiling before any conversion outputs, even without a snapshot.
    result.graph.encoded_len().map_err(|e| e.to_string())?;
    Ok(AvidRead { result, inputs, skipped_bins: selected.skipped })
}

fn source_notes(a: &Args, read: &mut AvidRead, matte: &crate::png_mattes::Plan) -> Result<Vec<String>, String> {
    let result = &mut read.result;
    if a.to.as_deref() == Some("premiere") && !result.ambiguous_managed_packages.is_empty() {
        return Err(format!(
            "Media scan or channel routing needs resolution: {}",
            result.ambiguous_managed_packages.join("; ")
        ));
    }
    if a.link_originals || a.allow_offline_media {
        let issues = result.scan_issues.iter()
            .chain(result.audio_group_issues.iter().map(|issue| &issue.message));
        let unresolved = result.graph.losses.iter().filter(|loss| loss.aspect == "source_master" && matte.keeps_loss(loss));
        let mut blockers: Vec<_> = issues.cloned().chain(unresolved.map(|loss| {
            format!("{}: {}", loss.object, loss.reason)
        })).collect();
        if !blockers.is_empty() {
            if a.prefer_source_media {
                blockers.extend(result.diagnostics.iter().filter(|note|
                    note.contains(avid_adapter::original::SOURCE_MEDIA_UNAVAILABLE)).cloned());
            }
            return Err(format!("Linked media preparation failed:\n  {}", blockers.join("\n  ")));
        }
    } else if a.to.as_deref() == Some("premiere") && result.has_unresolved_original_links {
        return Err("Selected bins contain unresolved original-file links. Use --link-originals for direct linking, or use a saved job to combine original links and rewrap.".into());
    }
    let mut problems = std::mem::take(&mut result.diagnostics);
    problems.retain(|note| matte.keeps_reader_note(note));
    problems.append(&mut result.scan_issues);
    problems.extend(result.audio_group_issues.drain(..).map(|issue| issue.message));
    if a.wants_rewrap() {
        problems.extend(result.missing_managed_audio.iter().cloned());
        // Retain the existing strict rewrap policy: an omitted source route or
        // unsupported source control cannot disappear behind a reduced model.
        let text_ids: std::collections::HashSet<&str> = result.graph.sequences.iter()
            .flat_map(|s| s.texts.iter().map(|t| t.id.as_str())).collect();
        problems.extend(
            result
                .graph
                .losses
                .iter()
                // These notices describe omitted appearance or bin items. Source selection
                // and routing are validated separately by the media preflight.
                .filter(|l| (l.aspect != "text_appearance" || !text_ids.contains(l.object.as_str()))
                    && l.aspect != "source_color_adapter"
                    && !(a.rewrap_native_master && l.aspect == "subclip_bin_item"))
                .map(|l| format!("{} [{}]: {}", l.object, l.aspect, l.reason)),
        );
        if !problems.is_empty() {
            return Err(format!("OP1a preparation requires a reader/media scan without problems; no media created:\n  {}",problems.join("\n  ")));
        }
    }
    problems.extend(
        read.skipped_bins
            .iter()
            .map(|p| format!("repeated bin path skipped: {}", p.display())),
    );
    Ok(problems)
}

pub(crate) fn policy(a: &Args) -> Result<AvidToPremiere, String> {
    let mut p = match &a.policy {
        Some(path) => {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            AvidToPremiere::from_json_text(&text).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => AvidToPremiere::default(),
    };
    if let Some(m) = &a.bin_mode {
        p.bin_mode = BinMode::parse(m).ok_or("--bin-mode: separate, grouped or merged")?;
    }
    if a.match_pan {
        p.match_avid_default_pan = true;
    }
    Ok(p)
}

pub(crate) fn print_losses(title: &str, losses: &[Loss]) {
    if losses.is_empty() {
        return;
    }
    println!("{title} ({}):", losses.len());
    for l in losses {
        println!("  {} [{}]: {}", l.object, l.aspect, l.reason);
    }
}

pub(crate) fn publication_failure(
    stage: &str,
    error: impl std::fmt::Display,
    prepared: &prproj::convert::writer::PreparedProject,
    media: Option<&rewrap::Plan>,
    snapshot: Option<&Path>,
) -> String {
    let mut retained: Vec<String> = prepared
        .retained_paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    if let Some(plan) = media {
        if std::fs::symlink_metadata(plan.directory()).is_ok() {
            retained.push(plan.directory().display().to_string());
        }
    }
    if let Some(path) = snapshot {
        retained.push(path.display().to_string());
    }
    let message = format!(
        "{stage}: {error}; retained outputs: {}",
        if retained.is_empty() {
            "none".into()
        } else {
            retained.join(", ")
        }
    );
    if let Some(plan) = media {
        plan.record_failure(&message);
    }
    message
}

enum DirectPremiereInput {
    Compatibility(premiere_adapter::PreparedPremiere),
    Typed {
        input: PreparedProjectInput,
        media: Model,
    },
}

impl DirectPremiereInput {
    fn media_parts(&mut self) -> (&Model, Option<&mut PreparedProjectInput>) {
        match self {
            Self::Compatibility(prepared) => (&prepared.model, None),
            Self::Typed { input, media } => (media, Some(input)),
        }
    }
}

pub(crate) fn convert(a: &Args) -> Result<(), String> {
    if a.wants_rewrap() && a.to.as_deref() != Some("premiere") {
        return Err("media rewrap is only supported for Avid -> Premiere".into());
    }
    match a.to.as_deref() {
        Some("premiere") => {}
        Some("fcpxml") => return convert_fcpxml(a),
        _ => return Err("--to: premiere OUTPUT.prproj or fcpxml OUTPUT.fcpxml".into()),
    }
    let output = a
        .output
        .clone()
        .ok_or("--to premiere needs OUTPUT.prproj")?;
    output::preflight(&output, a.graph_out.as_deref())?;
    let policy = policy(a)?;
    if let Some(folder) = &a.rewrap_needed {
        use crate::job::{self, ClipMode, JobSpec};
        if a.from.as_deref() != Some("avid") {
            return Err("--rewrap-needed requires --from avid".into());
        }
        let spec = JobSpec {
            bins: a.bins.iter().map(PathBuf::from).collect(),
            output,
            media_roots: a.media.iter().map(PathBuf::from).collect(),
            project_name: a.project.clone(),
            mode: match policy.bin_mode {
                BinMode::Separate => ClipMode::Separate,
                BinMode::Grouped => ClipMode::Grouped,
                BinMode::Merged => ClipMode::Merged,
            },
            policy_json: policy.to_json_text(),
            rewrap_needed: Some(folder.clone()),
            allow_offline_media: a.allow_offline_media,
            prefer_source_media: a.prefer_source_media,
            attach_avid_proxies: a.attach_avid_proxies,
            proxy_rewrap_dir: a.proxy_rewrap_dir.clone(),
            ffmpeg: a.ffmpeg.clone(),
            ffprobe: a.ffprobe.clone(),
            path_mappings: a.original_mappings.iter().map(|mapping| job::PathMapping {
                saved_prefix: mapping.saved_root.clone(), actual_root: mapping.current_root.clone(),
            }).collect(),
            ..JobSpec::default()
        };
        let cancel = job::CancelToken::default();
        let mut progress = |stage: job::Progress| eprintln!("{}", stage.message);
        let prepared = job::prepare(spec, &cancel, &mut progress)?;
        if a.rewrap_dry_run {
            for note in &prepared.review().notes { println!("{}", note.message); }
            return Ok(());
        }
        let done = job::execute(prepared, &cancel, &mut progress);
        println!("{}", serde_json::to_string_pretty(&done).map_err(|e| e.to_string())?);
        return if done.status == job::CompletionStatus::Complete { Ok(()) } else { Err(done.message) };
    }
    let route = if a.rewrap_media {PremiereRoute::NativeMedia}else{PremiereRoute::resolve(a.rewrap_native_master, policy.bin_mode)};
    let mut read = read_avid_source(a, !a.wants_rewrap() && policy.bin_mode == BinMode::Separate)?;
    if a.link_originals && policy.bin_mode == BinMode::Separate && !a.wants_rewrap() {
        crate::wav_original::select(
            &mut read.result, &a.original_mappings,
            &crate::backend_bundle::resolve(None, a.ffprobe.as_deref(), false)?.ffprobe,
            &mut || Ok(()),
        )?;
    }
    let mut matte = crate::png_mattes::prepare(
        &read.result, &a.original_mappings, !a.wants_rewrap() && policy.bin_mode == BinMode::Separate,
        a.ffprobe.as_deref().unwrap_or(Path::new("ffprobe")),
        &mut || Ok(()),
    ).map_err(|issue| issue.message)?;
    let has_matte = !matte.is_empty();
    let problems = source_notes(a, &mut read, &matte)?;
    let AvidRead { result, inputs, .. } = read;
    let avid_adapter::ReadResult { graph, mut original_inputs, proxy_bindings, .. } = result;
    original_inputs.append(&mut matte.inputs);
    let managed_stamps = if a.wants_rewrap() {
        Vec::new()
    } else {
        // Direct managed links already have a resolved path. Observe their file
        // state while preserving the direct CLI's existing qualification policy.
        let mut paths: Vec<_> = graph.media.iter()
            .filter(|media| media.original.is_none() && media.online == Some(true))
            .flat_map(|media| &media.locations)
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths.into_iter()
            .map(|path| source_input::original_stamp(Path::new(path)))
            .collect::<Result<Vec<_>, _>>()?
    };
    if !a.rewrap_media {
        let affected: Vec<_> = premiere_preparation::managed_rewrap_candidates(&graph, a.wants_rewrap())
            .into_iter().filter(|clip| !matte.managed_fill_only.contains(clip.item))
            .map(|clip| clip.detail).collect();
        if !affected.is_empty() {
            return Err(format!(
                "Selected Avid-managed OP-Atom clips need rewrap for Premiere. Use --rewrap-media NEW_DIR --rewrap-native-master.\n{}",
                affected.join("\n"),
            ));
        }
    }
    if a.wants_rewrap() {
        if let Some(issue) = crate::job::sequence_timing_blocker(&graph, None) {
            return Err(issue.cli_summary());
        }
        crate::offline::check_edit_routes(&graph)?;
    }
    let gain = rewrap::gain_paths(&graph);
    if !gain.is_empty() && !a.rewrap_media {
        return Err("AudioSuite Gain requires --rewrap-media and --rewrap-native-master to copy the saved render to WAV".into());
    }
    let selected_directory = if a.rewrap_prompt {
        match media_prompt::choose_directory(&output)? {
            Some(dir) => Some(dir),
            None => {
                println!("Cancelled: no media, project or graph created.");
                return Ok(());
            }
        }
    } else {
        a.rewrap.clone()
    };
    let policy_losses = policy.losses_source(&graph);
    let typed_disabled = !a.wants_rewrap() && policy.bin_mode == BinMode::Separate
        && premiere_adapter::has_disabled_edit_candidates(&graph);
    let typed_pan = !a.wants_rewrap()
        && premiere_adapter::has_managed_pan_candidates(&graph);
    let typed_track_names = !a.wants_rewrap() && premiere_adapter::has_track_name_candidates(&graph);
    // Premiere preparation reports the appearance of each prepared cue using
    // its destination styling, and forwards source appearance for omitted cues.
    let mut source_losses: Vec<_> = graph.losses.iter()
        .filter(|loss| (loss.aspect != "subclip_bin_item" || a.rewrap_native_master)
            && loss.aspect != "text_appearance"
            && loss.aspect != premiere_adapter::LAYERED_LOSS
            && !(typed_disabled && loss.aspect == "disabled_clip")
            && loss.aspect != avid_adapter::PAN_AUTOMATION_LOSS && matte.keeps_loss(loss))
        .cloned().collect();
    source_losses.extend(matte.overlay_omissions.iter().cloned());
    let clock = premiere_preparation::clock_profile(a.wants_rewrap());
    let color_media = if a.rewrap_media {
        crate::source_luts::ColorMedia::Rewrap
    } else {
        crate::source_luts::ColorMedia::Linked
    };
    let has_source_lut = (!a.wants_rewrap() || a.rewrap_media) && policy.bin_mode == BinMode::Separate
        && crate::source_luts::has_candidates(&graph, color_media);
    let typed_link = has_matte || typed_disabled || typed_pan || typed_track_names
        || a.link_originals || a.allow_offline_media || has_source_lut;
    let editable_groups = !graph.multicams.is_empty() || !graph.multicam_uses.is_empty();
    let linked_structures = !a.wants_rewrap() && policy.bin_mode == BinMode::Separate
        && (premiere_adapter::has_layered_candidates(&graph) || graph.sidecars.iter().any(|sidecar| match sidecar.adapter.as_str() {
            "avid.subclips" => matches!(&sidecar.data, graph::Value::Arr(rows) if !rows.is_empty()),
            "avid.source" => sidecar.data.get("plain_submaster") == Some(&graph::Value::Bool(true)),
            _ => false,
        }));
    let has_managed_op1a = crate::managed_op1a::has_verified_streams(&graph);
    let use_typed = typed_link || linked_structures || has_managed_op1a
        || !gain.is_empty() || editable_groups || premiere_adapter::has_avid_filler_fade(&graph);
    let mut lut_pictures = Vec::new();
    let mut managed_lt_pictures = rewrap::LtRangePictures::new();
    let (mut destination, preparation_notes, mut bridge_losses) = if use_typed {
        let ffprobe = if has_source_lut || a.link_originals || a.allow_offline_media {
            crate::backend_bundle::resolve(None, a.ffprobe.as_deref(), false)?.ffprobe
        } else {
            PathBuf::new()
        };
        let (p, mut luts) = premiere_preparation::prepare_typed(
            &graph, &policy.to_options(), clock, a.rewrap_media, &mut matte, &inputs, &ffprobe, false, false, None, &mut || Ok(()),
        ).map_err(|issue| issue.message)?;
        source_losses.retain(|loss| luts.keeps_loss(loss));
        original_inputs.append(&mut luts.inputs);
        lut_pictures = luts.pictures;
        managed_lt_pictures = luts.managed_pictures;
        let media = p.input.build_native_media_model().map_err(|e| e.to_string())?;
        (DirectPremiereInput::Typed { input: p.input, media }, p.diagnostics, p.losses)
    } else {
        let mut p = premiere_adapter::prepare(&graph, &policy.to_options(), clock)
            .map_err(|e| format!("Premiere preparation failed: {e}"))?;
        let diagnostics = std::mem::take(&mut p.diagnostics);
        let losses = std::mem::take(&mut p.losses);
        drop(std::mem::take(&mut p.ownership));
        (DirectPremiereInput::Compatibility(p), diagnostics, losses)
    };
    let reported_source_losses: HashSet<_> = source_losses.iter()
        .map(|loss| (loss.object.as_str(), loss.aspect.as_str(), loss.reason.as_str()))
        .collect();
    bridge_losses.retain(|loss| {
        !reported_source_losses.contains(&(loss.object.as_str(), loss.aspect.as_str(), loss.reason.as_str()))
    });
    let (model, typed) = destination.media_parts();
    let prepared_sequences = typed.as_ref().map_or(&model.sequences, |p| &p.sequences);
    let prepared_sequence_count = prepared_sequences.len();
    let prepared_event_count: usize = prepared_sequences.iter().map(|s| s.events.len()).sum::<usize>()
        + typed.as_ref().map_or(0, |p| p.png_mattes.edits.len() + p.png_mattes.split_edits.len() + p.layered_submasters.len());
    let prepared_stills = typed.as_ref().map_or(0, |p| p.png_mattes.stills.len()
        + p.png_mattes.jpeg_fills.iter().filter(|fill| fill.existing_clip.is_none()).count());
    let native_media = a.rewrap_media.then(|| rewrap::NativeMediaPreparation {
        model,
        source: &graph,
        writer_input: typed,
        gain: &gain,
        managed_pictures: managed_lt_pictures,
    });
    if let Some(native) = &native_media {
        native.preflight()?;
    } else {
        route.preflight(model).map_err(|e| e.to_string())?;
    }
    if gain.is_empty() {
        if let Some(notice) = route.notice() {
            println!("{notice}");
        }
    }
    let preparation = if let Some(dir) = &selected_directory {
        println!("Creates new media.");
        let ffmpeg = a.ffmpeg.as_deref().unwrap_or(Path::new("ffmpeg"));
        let ffprobe = a.ffprobe.as_deref().unwrap_or(Path::new("ffprobe"));
        let plan = match native_media {
            Some(native) => native
                .prepare(dir, &a.media, &output, a.graph_out.as_deref(), ffmpeg, ffprobe)
                .map_err(rewrap::PrepareIssue::into_message)?,
            None => rewrap::Plan::prepare(
                model, dir, &a.media, &output, a.graph_out.as_deref(), ffmpeg, ffprobe,
            )?,
        };
        plan.describe();
        Some(plan)
    } else {
        None
    };
    let bindings = preparation
        .as_ref()
        .map(|p| p.bindings())
        .unwrap_or_default();
    let dnxhr_444_rgb_sources = preparation.as_ref()
        .map(|plan| plan.dnxhr_444_rgb_sources()).unwrap_or_default();
    let mut ids: Box<dyn UuidSource> = match &a.seed {
        Some(s) => Box::new(SeededUuids::new(s)),
        None => Box::new(RandomUuids::new()),
    };
    let offline = crate::offline::paths(&graph, &Default::default());
    let offline_relink = crate::offline::report(&graph, &output, &Default::default())?;
    if let Some((path, _)) = &offline_relink {
        output::preflight(&output, Some(path))?;
        if let Some(snapshot) = &a.graph_out {
            output::preflight(snapshot, Some(path))?;
        }
    }
    let mut prepared = match &mut destination {
        DirectPremiereInput::Typed { input, .. } if has_managed_op1a =>
            crate::managed_op1a::prepare(&graph, input, &output, ids.as_mut(), None, &offline),
        DirectPremiereInput::Typed { input, .. } => premiere_preparation::prepare_document(
            input, &output, ids.as_mut(), &bindings, matches!(route, PremiereRoute::NativeOp1a | PremiereRoute::NativeMedia), None, &offline,
            &dnxhr_444_rgb_sources,
        ),
        DirectPremiereInput::Compatibility(p) => route.prepare(
            &p.model, &output, ids.as_mut(), &bindings,
            Editorial {
                texts: &p.texts,
                video_dissolves: &p.video_dissolves,
                picture_effects: &p.picture_effects,
                direct_dnxhr_origins: &p.direct_dnxhr_origins,
                constant_speeds: &p.constant_speeds,
            },
            &dnxhr_444_rgb_sources,
        ),
    }.map_err(|e| {
        let mut message = format!("Premiere document preparation failed: {e}");
        for problem in problems.iter().filter(|note|
            note.contains(avid_adapter::original::SOURCE_MEDIA_UNAVAILABLE))
        {
            message.push('\n');
            message.push_str(problem);
        }
        message
    })?;
    if let Some((path, _)) = &offline_relink {
        prepared.check_reserved_output(path).map_err(|e| e.to_string())?;
    }
    avid_adapter::proxy::review(&proxy_bindings)?;
    for binding in proxy_bindings.iter().filter(|b| b.audio.is_some()) {
        output::preflight(&output, Some(&binding.path))?;
        if let Some(snapshot) = &a.graph_out {
            output::preflight(snapshot, Some(&binding.path))?;
        }
        prepared.check_reserved_output(&binding.path).map_err(|e|e.to_string())?;
    }
    if let Some(path) = &a.graph_out {
        prepared
            .check_reserved_output(path)
            .map_err(|e| e.to_string())?;
    }
    drop(destination);
    if a.rewrap_dry_run {
        for problem in &problems {
            println!("Reader note: {problem}");
        }
        for note in &preparation_notes {
            println!("Preparation note: {note}");
        }
        print_losses("Not converted (from the source)", &source_losses);
        print_losses("Not converted (colour policy)", &policy_losses);
        print_losses("Not converted (writer input)", &bridge_losses);
        println!("Dry run: no media, project or graph created.");
        return Ok(());
    }
    if let Some(plan) = &preparation {
        if a.rewrap_prompt && !media_prompt::confirm(&plan.review_text())? {
            println!("Cancelled: no media, project or graph created.");
            return Ok(());
        }
        prepared.recheck_outputs().map_err(|e| e.to_string())?;
        output::preflight(&output, a.graph_out.as_deref())?;
        source_input::recheck(&inputs)?;
        let mut media_created = false;
        let actual = plan
            .execute_observed(&mut |_, _| Ok(()), &mut media_created)
            .map_err(|e| {
                let retained = if media_created {
                    plan.directory().display().to_string()
                } else {
                    "none".into()
                };
                format!("media preparation failed: {e}; retained media outputs: {retained}")
            })?;
        if actual != bindings {
            let error =
                "executed media bindings differ from the prepared project; verified media retained";
            return Err(publication_failure(
                "media binding verification failed",
                error,
                &prepared,
                Some(plan),
                None,
            ));
        }
    }
    let recheck = |pins: &[avid_reader::source::SourceInput]| {
        pins.iter()
            .map(|pin| source_input::recheck_file_and_stamp(pin, &mut || Ok(())))
            .collect::<Result<Vec<_>, _>>()
    };
    let mut source_stamps = recheck(&inputs).map_err(|e| {
        publication_failure(
            "source stability check failed",
            e,
            &prepared,
            preparation.as_ref(),
            None,
        )
    })?;
    let mut original_stamps = recheck(&original_inputs)
        .and_then(|stamps| source_input::recheck_originals(&lut_pictures).map(|()| stamps))
        .map_err(|e| {
            publication_failure("original media stability check failed", e, &prepared, preparation.as_ref(), None)
        })?;
    let mut auxiliary_outputs = Vec::new();
    if proxy_bindings.iter().any(|b| b.audio.is_some()) {
        println!("Creates new media. Rewrapping verified Avid proxies.");
        avid_adapter::proxy::execute_observed(&proxy_bindings, &mut |path| {
            auxiliary_outputs.push(path.to_owned());
        })?;
        (|| {
            source_stamps = recheck(&inputs)?;
            original_stamps = recheck(&original_inputs)?;
            source_input::recheck_originals(&lut_pictures)
        })()
            .map_err(|e| format!("{e}; verified proxy files retained"))?;
    }
    source_stamps.extend(original_stamps);
    source_stamps.extend(lut_pictures);
    source_stamps.extend(managed_stamps);
    let check_source_state = |prepared: &prproj::convert::writer::PreparedProject, snapshot| {
        source_input::recheck_originals(&source_stamps).map_err(|e| {
            let error = publication_failure(
                "source stability check failed", e, prepared, preparation.as_ref(), snapshot,
            );
            if auxiliary_outputs.is_empty() {
                error
            } else {
                format!("{error}; verified proxy files retained: {auxiliary_outputs:?}")
            }
        })
    };
    check_source_state(&prepared, None)?;
    prepared.publish_assets().map_err(|e| {
        publication_failure(
            "asset publication failed",
            e,
            &prepared,
            preparation.as_ref(),
            None,
        )
    })?;
    check_source_state(&prepared, None)?;
    let mut published_snapshot = None;
    if let Some(path) = &a.graph_out {
        output::write_source_snapshot(path, &graph).map_err(|e| {
            publication_failure(
                "snapshot publication failed",
                e.message,
                &prepared,
                preparation.as_ref(),
                e.retained.then_some(path.as_path()),
            )
        })?;
        published_snapshot = Some(path.as_path());
        check_source_state(&prepared, published_snapshot)?;
    }
    let report = prepared.publish_project().map_err(|e| {
        publication_failure(
            "project publication failed",
            e,
            &prepared,
            preparation.as_ref(),
            published_snapshot,
        )
    })?;
    if let Some((path, text)) = &offline_relink {
        output::write_relink_report(path, text, &mut auxiliary_outputs).map_err(|e| {
            let retained = prepared.retained_paths().iter().map(PathBuf::as_path)
                .chain(published_snapshot)
                .chain(auxiliary_outputs.iter().map(PathBuf::as_path))
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>();
            format!("Project published at {}; {e}; retained outputs: {}",
                output.display(), retained.join(", "))
        })?;
        println!("Offline relink report: {}", path.display());
    }
    if let Some(plan) = &preparation {
        plan.record_project_with_mode(&output,plan.mode(route.receipt_mode())).map_err(|e|{
            // record_project already records the completed outcome accurately.
            format!("{e}; project/assets retained: {:?}; source snapshot: {:?}; publication notes: {}",prepared.retained_paths(),published_snapshot,report.notes.join("; "))
        })?;
    }
    let placements = prepared_event_count;
    println!(
        "any-nle: wrote {} ({} clip(s), {} bin(s), {} sequence(s), {} edit(s); {})",
        output.display(),
        graph.items.len() + prepared_stills,
        graph.bins.len(),
        prepared_sequence_count,
        placements,
        preparation.as_ref().map_or_else(|| route.to_string(), |p| p.mode(route.receipt_mode()).to_owned())
    );
    for note in &preparation_notes {
        println!("Preparation note: {note}");
    }
    if !problems.is_empty() {
        println!("Reader problems ({}):", problems.len());
        for p in &problems {
            println!("  {p}");
        }
    }
    print_losses("Not converted (from the source)", &source_losses);
    print_losses("Not converted (colour policy)", &policy_losses);
    print_losses("Not converted (writer input)", &bridge_losses);
    print_losses(
        "Marker ownership changes",
        &premiere_preparation::grouped_picture_marker_losses(&graph, &report),
    );
    for (title, lines) in [
        ("Writer notes", &report.notes),
        ("Unverified Premiere shapes", &report.unverified),
    ] {
        if !lines.is_empty() {
            println!("{title} ({}):", lines.len());
            for l in lines {
                println!("  {l}");
            }
        }
    }
    Ok(())
}

pub(crate) fn convert_fcpxml(a: &Args) -> Result<(), String> {
    let output = a.output.clone().ok_or("--to fcpxml needs OUTPUT.fcpxml")?;
    output::preflight(&output, a.graph_out.as_deref())?;
    let (graph, problems) = read_avid_legacy(a)?;
    if let Some(path) = &a.graph_out {
        output::write_snapshot(path, &graph.to_json_text().map_err(|e| e.to_string())?)?;
    }
    let (text, losses) = fcpxml::write(&graph).map_err(|e| e.to_string())?;
    std::fs::write(&output, text).map_err(|e| format!("{}: {e}", output.display()))?;
    let placements: usize = graph.sequences.iter().map(|s| s.placements.len()).sum();
    println!(
        "any-nle: wrote {} (FCPXML {}; {} clip(s), {} bin(s), {} sequence(s), {} edit(s))",
        output.display(),
        fcpxml::FCPXML_VERSION,
        graph.items.len(),
        graph.bins.len(),
        graph.sequences.len(),
        placements
    );
    if !problems.is_empty() {
        println!("Reader problems ({}):", problems.len());
        for p in &problems {
            println!("  {p}");
        }
    }
    print_losses("Not converted", &losses);
    Ok(())
}

pub(crate) fn colours(path: &str, a: &Args) -> Result<(), String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| format!("{path}: {e}"))?
        .take(graph::source::MAX_SOURCE_GRAPH_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{path}: {e}"))?;
    if bytes.len() > graph::source::MAX_SOURCE_GRAPH_BYTES {
        return Err("source graph exceeds 256 MiB".into());
    }
    let text = String::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))?;
    let value = graph::json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let policy = policy(a)?;
    let preview = match value.get("schema") {
        Some(graph::Value::Str(schema)) if schema == graph::SCHEMA => {
            if text.len() > graph::MAX_GRAPH_BYTES {
                return Err("graph/1 exceeds 64 MiB".into());
            }
            policy.colour_preview(&Graph::from_json(&value).map_err(|e| e.to_string())?)
        }
        Some(graph::Value::Str(schema)) if schema == graph::source::SCHEMA => policy
            .colour_preview_source(
                &graph::source::SourceGraph::from_json(&value).map_err(|e| e.to_string())?,
            ),
        _ => return Err("colours: expected anynle.graph/1 or anynle.graph/2".into()),
    };
    println!("Marker colours (Avid -> Premiere):");
    for m in &preview.markers {
        let target = m.premiere.as_ref().map_or(
            "Premiere default (not an Avid colour)".to_string(),
            |(i, n)| format!("{n} ({i})"),
        );
        println!("  {:<10} -> {target}   x{}", m.avid, m.uses);
    }
    println!("Clip colours (Avid -> Premiere label):");
    for l in &preview.labels {
        println!(
            "  {} -> {} ({})   x{}",
            l.avid, l.premiere.1, l.premiere.0, l.uses
        );
    }
    Ok(())
}

pub(crate) fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        return usage("");
    };
    if !matches!(command.as_str(), "convert" | "verify-offline-relink")
        && rest
            .iter()
            .any(|a| a.starts_with("--rewrap") || matches!(a.as_str(),
                "--ffmpeg" | "--ffprobe" | "--link-originals" | "--prefer-source-media" | "--map-original-root" | "--allow-offline-media"))
    {
        return usage("media preparation options are only supported by convert --to premiere");
    }
    let result = match command.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "review" => {
            let [flag, job] = rest else {
                return usage("review --job SETTINGS.json");
            };
            if flag != "--job" {
                return usage("review --job SETTINGS.json");
            }
            return crate::job_review::run(Path::new(job));
        }
        "mcp" => crate::mcp::run(rest),
        "run" => {
            if rest.len()<2 || !matches!(rest[0].as_str(),"--job"|"--job-json"|"--job-base64") {return usage("run --job SETTINGS.json [--output NEW.prproj] [--new-media NEW_DIR]");}
            #[cfg(target_os = "macos")]
            if rest[0] == "--job" {
                match crate::resolve_job::load_if_avid_resolve(Path::new(&rest[1])) {
                    Ok(Some(job)) => return match crate::resolve_job::run_cli(job, &rest[2..]) {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(error) => { eprintln!("{error}"); ExitCode::FAILURE }
                    },
                    Ok(None) => {}
                    Err(error) => { eprintln!("{error}"); return ExitCode::FAILURE; }
                }
            }
            let loaded=if rest[0]=="--job"{crate::job::load(Path::new(&rest[1]))}else if rest[0]=="--job-base64"{crate::job::load_encoded(&rest[1])}else if rest[1].len()>1<<20{Err("Job settings exceed 1 MiB".into())}else{serde_json::from_str(&rest[1]).map_err(|e|format!("Invalid job settings: {e}"))};
            loaded.and_then(|mut spec:crate::job::JobSpec|{
                let mut flags=rest[2..].chunks_exact(2);
                for pair in flags.by_ref(){match pair[0].as_str(){"--output"=>spec.output=PathBuf::from(&pair[1]),"--new-media"=>spec.new_media=Some(PathBuf::from(&pair[1])),other=>return Err(format!("Unknown run override {other}"))}}
                if !flags.remainder().is_empty(){return Err("Run override needs a value".into())}
                let cancel=crate::job::CancelToken::default();
                let mut progress=|p:crate::job::Progress|eprintln!("{}: {}",p.stage,p.message);
                let prepared=crate::job::prepare(spec,&cancel,&mut progress)?;
                let done=crate::job::execute(prepared,&cancel,&mut progress);
                use std::io::{BufWriter, Write};
                let mut stdout = BufWriter::with_capacity(64 * 1024, std::io::stdout().lock());
                let write_error = |e| format!("Could not write the completion report to stdout: {e}");
                let printed = serde_json::to_writer_pretty(&mut stdout, &done)
                    .map_err(|e| write_error(e.to_string()))
                    .and_then(|_| stdout.write_all(b"\n").and_then(|_| stdout.flush())
                        .map_err(|e| write_error(e.to_string())));
                match (done.status, printed) {
                    (crate::job::CompletionStatus::Complete, result) => result,
                    (_, Ok(())) => Err(done.message),
                    (_, Err(e)) => Err(format!("{}\n{e}", done.message)),
                }
            })
        },
        "policy" => {
            print!("{}", AvidToPremiere::default().to_json_text());
            Ok(())
        }
        "verify-offline-relink" => {
            let [report, moves, options @ ..] = rest else {
                return usage("verify-offline-relink needs REPORT.json and MOVES.json");
            };
            let mut checksums = None;
            let mut ffprobe = None;
            let mut pairs = options.chunks_exact(2);
            for pair in &mut pairs {
                let slot = match pair[0].as_str() {
                    "--checksums" => &mut checksums,
                    "--ffprobe" => &mut ffprobe,
                    _ => return usage("unknown verify-offline-relink option"),
                };
                if slot.replace(Path::new(&pair[1])).is_some() {
                    return usage("duplicate verify-offline-relink option");
                }
            }
            if !pairs.remainder().is_empty() {
                return usage("verify-offline-relink option needs a path");
            }
            crate::backend_bundle::resolve(None, ffprobe, false).and_then(|tools| {
                crate::offline::verify(Path::new(report), Path::new(moves), checksums, &tools.ffprobe)
            })
        },
        "convert" => match parse(rest) {
            Ok(a) => convert(&a),
            Err(e) => return usage(&e),
        },
        "graph" => match parse(rest) {
            Ok(a) => read_avid_source(&a, false)
                .and_then(|mut read| {
                    for p in source_notes(&a, &mut read, &crate::png_mattes::Plan::default())? {
                        eprintln!("  {p}");
                    }
                    read.result.graph.write_json(&mut std::io::stdout().lock())
                        .map_err(|e| e.to_string())
                }),
            Err(e) => return usage(&e),
        },
        "colours" | "colors" => match rest.split_first() {
            Some((path, more)) => match parse(more) {
                Ok(a) => colours(path, &a),
                Err(e) => return usage(&e),
            },
            None => return usage("colours needs GRAPH.json"),
        },
        other => return usage(&format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
