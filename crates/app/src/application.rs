//! any-nle: one command from source program to target program, through the graph.
//!
//! Avid source facts -> graph/2 -> Premiere preparation -> one encoded document.
//! The held FCPXML route retains its explicit legacy graph/1 input profile.


use std::path::{Path, PathBuf};
use std::process::ExitCode;

use avid_reader::extract::extract_project_paths;
use avid_reader::model::{build, MediaIndex};
use graph::{Graph, Loss};
use mapping::{AvidToPremiere, BinMode};
use crate::premiere_route::PremiereRoute;
use crate::{media_prompt, output, rewrap, source_input};
use prproj::convert::model::{parse_model_str, MAX_MODEL_BYTES};
use prproj::{RandomUuids, SeededUuids, UuidSource};

pub(crate) const USAGE: &str = "usage:
  any-nle run --job SETTINGS.json [--output NEW.prproj] [--new-media NEW_DIR]
      Run a saved native-wizard job with the same read-only preparation and explicit execution.
  any-nle convert --from avid BIN.avb... --media DIR [--media DIR...] [--project NAME]
                  --to premiere OUTPUT.prproj [--policy POLICY.json] [--bin-mode separate|grouped|merged]
                  [--match-avid-default-pan] [--graph GRAPH.json] [--seed SEED]
                  [--rewrap-op1a NEW_DIR | --rewrap-op1a-prompt | --rewrap-media NEW_DIR] [--rewrap-dry-run] [--rewrap-native-master] [--ffmpeg PATH] [--ffprobe PATH]
      Convert Avid bins to a Premiere project. Prints what was converted, the reader's problems
      and every loss. --graph saves the qualified source anynle.graph/2 (Premiere route).
      --rewrap-media NEW_DIR: Creates new media. DNx to MXF OP1a; qualified ProRes to MOV. Requires --rewrap-native-master.
      --rewrap-op1a: Creates new media. Copies supported OP-Atom streams into NEW_DIR.
      --rewrap-op1a-prompt: macOS folder picker and space review before creating media.
      --rewrap-dry-run: preview only; creates no media/project/graph. Requires a rewrap destination option.
      --rewrap-native-master: experimental file-backed AV masters preserving bin audio groups; replaces bin mode.
      Qualified MXF rates (including 23.976), DNxHD/DNxHR 8-bit 4:2:2 + up to 32 mono PCM48k inputs.
      Requires FFmpeg/ffprobe 8.1; bounded native read-backs pass; wider qualification is pending.
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
    pub(crate) rewrap_media: bool,
    pub(crate) rewrap_dry_run: bool,
    pub(crate) rewrap_prompt: bool,
    pub(crate) rewrap_native_master: bool,
    pub(crate) ffmpeg: Option<PathBuf>,
    pub(crate) ffprobe: Option<PathBuf>,
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
        let value = it
            .next()
            .cloned()
            .ok_or_else(|| format!("{arg} needs a value"))?;
        if matches!(arg.as_str(), "--rewrap-op1a" | "--rewrap-media" | "--ffmpeg" | "--ffprobe")
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
            "--rewrap-op1a" | "--rewrap-media" => {
                if a.rewrap.is_some() {
                    return Err("rewrap destination given more than once; choose one media mode".into());
                }
                a.rewrap_media = arg=="--rewrap-media";
                a.rewrap = Some(PathBuf::from(value));
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
    if a.rewrap_prompt && a.rewrap.is_some() {
        return Err("choose an explicit rewrap directory or --rewrap-op1a-prompt, not both".into());
    }
    if !a.wants_rewrap()
        && (a.rewrap_dry_run || a.rewrap_native_master || a.ffmpeg.is_some() || a.ffprobe.is_some())
    {
        return Err("rewrap settings require --rewrap-op1a NEW_DIR, --rewrap-media NEW_DIR or --rewrap-op1a-prompt".into());
    }
    if a.rewrap_media && !a.rewrap_native_master { return Err("--rewrap-media requires --rewrap-native-master".into()); }
    if a.rewrap_native_master && a.bin_mode.is_some() {
        return Err("--rewrap-native-master replaces bin mode; omit --bin-mode".into());
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

pub(crate) fn read_avid_source(
    a: &Args,
) -> Result<
    (
        graph::source::SourceGraph,
        Vec<String>,
        Vec<avid_reader::source::SourceInput>,
    ),
    String,
> {
    if a.from.as_deref() != Some("avid") {
        return Err("--from: only avid is supported so far".into());
    }
    if a.bins.is_empty() || a.media.is_empty() {
        return Err("give at least one BIN.avb and one --media folder".into());
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
    let ffprobe = a.ffprobe.as_deref().unwrap_or_else(|| Path::new("ffprobe"));
    let result = avid_adapter::read_with_ffprobe(&inputs, &roots, &project, &names, ffprobe)?;
    source_input::recheck(&inputs)?;
    let mut problems = result.diagnostics;
    problems.extend(result.scan_issues);
    problems.extend(result.audio_group_issues);
    if a.wants_rewrap() {
        // Retain the existing strict rewrap policy: an omitted source route or
        // unsupported source control cannot disappear behind a reduced model.
        let text_ids: std::collections::HashSet<&str> = result.graph.sequences.iter()
            .flat_map(|s| s.texts.iter().map(|t| t.id.as_str())).collect();
        problems.extend(
            result
                .graph
                .losses
                .iter()
                // Text appearance is intentionally represented by destination
                // defaults. It cannot change essence selection or routing; keep
                // reporting it, without treating it as a media-preflight error.
                .filter(|l| l.aspect != "text_appearance" || !text_ids.contains(l.object.as_str()))
                .map(|l| format!("{} [{}]: {}", l.object, l.aspect, l.reason)),
        );
        if !problems.is_empty() {
            return Err(format!("OP1a preparation requires a reader/media scan without problems; no media created:\n  {}",problems.join("\n  ")));
        }
    }
    problems.extend(
        selected
            .skipped
            .iter()
            .map(|p| format!("repeated bin path skipped: {}", p.display())),
    );
    // Enforce the encoded ceiling before any conversion outputs, even without a snapshot.
    result.graph.encoded_len().map_err(|e| e.to_string())?;
    Ok((result.graph, problems, inputs))
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
    let route = if a.rewrap_media {PremiereRoute::NativeMedia}else{PremiereRoute::resolve(a.rewrap_native_master, policy.bin_mode)};
    let (graph, problems, inputs) = read_avid_source(a)?;
    if let Some(issue) = crate::job::sequence_timing_blocker(&graph) {
        return Err(issue.cli_summary());
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
    let clock = if a.wants_rewrap() { premiere_adapter::ClockProfile::VerifiedRewrap }
        else { premiere_adapter::ClockProfile::LegacyImport };
    let (model, texts, video_dissolves, picture_effects, mut typed, preparation_notes, bridge_losses) =
        if !gain.is_empty() || !graph.multicams.is_empty() || !graph.multicam_uses.is_empty() {
            if matches!(route, PremiereRoute::Bin(_)) {
                return Err("multicam_native_route_required: editable Groups require the native OP1a rewrap route".into());
            }
            let p = premiere_adapter::prepare_originals(&graph, &policy.to_options(), clock)
                .map_err(|e| format!("Premiere preparation failed: {e}"))?;
            let media = p.input.build_native_media_model().map_err(|e| e.to_string())?;
            (media, Vec::new(), Vec::new(), Vec::new(), Some(p.input), p.diagnostics, p.losses)
        } else {
            let p = premiere_adapter::prepare(&graph, &policy.to_options(), clock)
                .map_err(|e| format!("Premiere preparation failed: {e}"))?;
            (p.model, p.texts, p.video_dissolves, p.picture_effects, None, p.diagnostics, p.losses)
        };
    let prepared_event_count: usize = typed.as_ref().map_or(&model.sequences, |p| &p.sequences)
        .iter().map(|s| s.events.len()).sum();
    let native_media = a.rewrap_media.then(|| rewrap::NativeMediaPreparation {
        model: &model,
        writer_input: typed.as_mut(),
        gain: &gain,
    });
    if let Some(native) = &native_media {
        native.preflight()?;
    } else {
        route.preflight(&model).map_err(|e| e.to_string())?;
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
                &model, dir, &a.media, &output, a.graph_out.as_deref(), ffmpeg, ffprobe,
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
    let mut ids: Box<dyn UuidSource> = match &a.seed {
        Some(s) => Box::new(SeededUuids::new(s)),
        None => Box::new(RandomUuids::new()),
    };
    let mut prepared = match &typed {
        Some(p) => prproj::convert::writer::prepare_typed_project(p, &output, ids.as_mut(), &bindings, true),
        None => route.prepare(&model, &output, ids.as_mut(), &bindings, prproj::convert::writer::Editorial {
            texts: &texts, video_dissolves: &video_dissolves, picture_effects: &picture_effects,
        }),
    }.map_err(|e| format!("Premiere document preparation failed: {e}"))?;
    drop(typed);
    if let Some(path) = &a.graph_out {
        prepared
            .check_reserved_output(path)
            .map_err(|e| e.to_string())?;
    }
    drop(model);
    if a.rewrap_dry_run {
        for problem in &problems {
            println!("Reader note: {problem}");
        }
        for note in &preparation_notes {
            println!("Preparation note: {note}");
        }
        print_losses("Not converted (from the source)", &graph.losses);
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
    source_input::recheck(&inputs).map_err(|e| {
        publication_failure(
            "source stability check failed",
            e,
            &prepared,
            preparation.as_ref(),
            None,
        )
    })?;
    prepared.publish_assets().map_err(|e| {
        publication_failure(
            "asset publication failed",
            e,
            &prepared,
            preparation.as_ref(),
            None,
        )
    })?;
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
        graph.items.len(),
        graph.bins.len(),
        graph.sequences.len(),
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
    print_losses("Not converted (from the source)", &graph.losses);
    print_losses("Not converted (colour policy)", &policy_losses);
    print_losses("Not converted (writer input)", &bridge_losses);
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
        .take(graph::MAX_GRAPH_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{path}: {e}"))?;
    if bytes.len() > graph::MAX_GRAPH_BYTES {
        return Err("graph exceeds 64 MiB".into());
    }
    let text = String::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))?;
    if text.len() > graph::MAX_GRAPH_BYTES {
        return Err("graph exceeds 64 MiB".into());
    }
    let value = graph::json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let policy = policy(a)?;
    let preview = match value.get("schema") {
        Some(graph::Value::Str(schema)) if schema == graph::SCHEMA => {
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
    if command != "convert"
        && rest
            .iter()
            .any(|a| a.starts_with("--rewrap") || a == "--ffmpeg" || a == "--ffprobe")
    {
        return usage("rewrap options are only supported by convert --to premiere");
    }
    let result = match command.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "run" => {
            if rest.len()<2 || !matches!(rest[0].as_str(),"--job"|"--job-json"|"--job-base64") {return usage("run --job SETTINGS.json [--output NEW.prproj] [--new-media NEW_DIR]");}
            let loaded=if rest[0]=="--job"{crate::job::load(Path::new(&rest[1]))}else if rest[0]=="--job-base64"{crate::job::load_encoded(&rest[1])}else if rest[1].len()>1<<20{Err("Job settings exceed 1 MiB".into())}else{serde_json::from_str(&rest[1]).map_err(|e|format!("Invalid job settings: {e}"))};
            loaded.and_then(|mut spec:crate::job::JobSpec|{
                let mut flags=rest[2..].chunks_exact(2);
                for pair in flags.by_ref(){match pair[0].as_str(){"--output"=>spec.output=PathBuf::from(&pair[1]),"--new-media"=>spec.new_media=Some(PathBuf::from(&pair[1])),other=>return Err(format!("Unknown run override {other}"))}}
                if !flags.remainder().is_empty(){return Err("Run override needs a value".into())}
                let cancel=crate::job::CancelToken::default();
                let mut progress=|p:crate::job::Progress|eprintln!("{}: {}",p.stage,p.message);
                let prepared=crate::job::prepare(spec,&cancel,&mut progress)?;
                let done=crate::job::execute(prepared,&cancel,&mut progress);
                println!("{}",serde_json::to_string_pretty(&done).map_err(|e|e.to_string())?);
                if done.status==crate::job::CompletionStatus::Complete {Ok(())}else{Err(done.message)}
            })
        },
        "policy" => {
            print!("{}", AvidToPremiere::default().to_json_text());
            Ok(())
        }
        "convert" => match parse(rest) {
            Ok(a) => convert(&a),
            Err(e) => return usage(&e),
        },
        "graph" => match parse(rest) {
            Ok(a) => read_avid_source(&a)
                .map(|(g, problems, _inputs)| {
                    for p in problems {
                        eprintln!("  {p}");
                    }
                    g.write_json(&mut std::io::stdout().lock())
                        .map_err(|e| e.to_string())
                })
                .and_then(|r| r),
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
