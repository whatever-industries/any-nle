//! Execute a prepared plan, verify copied essence and publish outputs exclusively.
use super::backend::{probe, run};
use super::receipts::{obj, strings, text};
use super::source::{free_space, id_value, pcm_essence_bytes, stamp};
use super::{Plan, R};
use graph::json::Value;
use prproj::convert::rewrap::MediaRelink;
use std::fs;
use std::path::Path;
use std::process::Command;

impl Plan {
    fn hashes(
        &self,
        path: &Path,
        indices: &[usize],
        terminal: Option<&super::terminal::Terminal>,
    ) -> R<Vec<String>> {
        let mut command = Command::new(&self.ffmpeg);
        command
            .args([
                "-v",
                "error",
                "-nostdin",
                "-protocol_whitelist",
                "file,pipe",
                "-f",
                "mxf",
                "-i",
            ])
            .arg(path);
        for index in indices {
            command.args(["-map", &format!("0:{index}")]);
        }
        command.args(["-c", "copy"]);
        if let Some(tail) = terminal {
            command.args(["-bsf:v:0", &tail.filter()]);
        }
        let output = run(command.args(["-f", "streamhash", "-hash", "sha256", "-"]))?;
        let mut hashes = Vec::new();
        for (i, line) in output.lines().enumerate() {
            let parts: Vec<_> = line.split(',').collect();
            if parts.len() != 3
                || parts[0] != i.to_string()
                || !parts[2].starts_with("SHA256=")
                || parts[2].len() != 71
                || !parts[2][7..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("invalid backend stream fingerprint".into());
            }
            hashes.push(parts[2][7..].to_string());
        }
        if hashes.is_empty() {
            return Err("missing stream fingerprints".into());
        }
        Ok(hashes)
    }
    pub fn execute(&self) -> R<Vec<MediaRelink>> {
        self.execute_controlled(&mut |_, _| Ok(()))
    }
    /// Check cancellation/progress before each output and between input verification stages.
    /// A backend invocation already in flight completes before the next safe boundary.
    pub fn execute_controlled(
        &self,
        control: &mut dyn FnMut(usize, usize) -> R<()>,
    ) -> R<Vec<MediaRelink>> {
        self.execute_observed(control, &mut false)
    }
    /// Record directory ownership only after exclusive creation succeeds.
    pub fn execute_observed(
        &self,
        control: &mut dyn FnMut(usize, usize) -> R<()>,
        created: &mut bool,
    ) -> R<Vec<MediaRelink>> {
        control(0, self.jobs.len())?;
        if self.inputs.iter().any(|i| i.terminal.is_some()) {
            super::terminal::require_filter(&self.ffmpeg)?;
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&self.destination)
            .map_err(|e| format!("rewrap destination: {e}"))?;
        *created = true;
        let result = self.execute_inner_controlled(control);
        if let Err(e) = &result {
            if let Err(receipt_error)=self.write_receipt("failed.json",obj(vec![("status",text("failed; original inputs untouched; partial/new media retained for inspection")),("error",text(e)),("tools",strings(&self.versions))])) {
                eprintln!("media preparation failed: {e}; failure receipt could not be written: {receipt_error}; partial/new media retained at {}",self.destination.display());
            }
        }
        result
    }
    #[cfg(test)]
    pub(super) fn execute_inner(&self) -> R<Vec<MediaRelink>> {
        self.execute_inner_controlled(&mut |_, _| Ok(()))
    }
    fn execute_inner_controlled(
        &self,
        control: &mut dyn FnMut(usize, usize) -> R<()>,
    ) -> R<Vec<MediaRelink>> {
        let mut mappings = Vec::new();
        let mut records = Vec::new();
        for (j, job) in self.jobs.iter().enumerate() {
            control(j, self.jobs.len())?;
            println!(
                "{} rewrap {}/{}",
                if job.mov { "MOV" } else { "OP1a" },
                j + 1,
                self.jobs.len()
            );
            let partial = self.destination.join(format!(
                ".partial-{:04}.{}",
                j + 1,
                if job.mov { "mov" } else { "mxf" }
            ));
            let target = self.destination.join(format!(
                "media-{:04}.{}",
                j + 1,
                if job.mov { "mov" } else { "mxf" }
            ));
            let mut hashes = Vec::new();
            let mut inputs = Vec::new();
            for i in &job.inputs {
                control(j, self.jobs.len())?;
                let input = &self.inputs[*i];
                if stamp(&input.path)? != input.stamp {
                    return Err("input changed since preflight".into());
                }
                if let Some(tail) = &input.terminal {
                    let again = super::terminal::Terminal::qualify(
                        &input.path,
                        &input.probe,
                        &self.ffprobe,
                        job.rate,
                    )?;
                    if &again != tail {
                        return Err("terminal boundary changed since preflight".into());
                    }
                }
                let hs = self.hashes(&input.path, &input.probe.stream_indices, None)?;
                if hs.len() != 1 {
                    return Err("input changed stream cardinality".into());
                }
                let copied = if let Some(tail) = &input.terminal {
                    let filtered =
                        self.hashes(&input.path, &input.probe.stream_indices, Some(tail))?;
                    if filtered.len() != 1 {
                        return Err("filtered stream cardinality changed".into());
                    }
                    filtered[0].clone()
                } else {
                    hs[0].clone()
                };
                hashes.push(copied.clone());
                let mut facts = vec![
                    ("path", text(input.path.to_string_lossy())),
                    ("aliases", strings(&input.aliases)),
                    ("bytes", text(input.stamp.len.to_string())),
                    ("essence_sha256", text(&hs[0])),
                    (
                        "pcm_payload_bytes",
                        text(
                            if input.probe.streams[0].kind == "audio" {
                                pcm_essence_bytes(&input.path)?
                            } else {
                                0
                            }
                            .to_string(),
                        ),
                    ),
                    ("identity", input.identity.clone()),
                    ("probe", input.probe.raw.clone()),
                    (
                        "selected_stream_index",
                        Value::Int(input.probe.stream_indices[0] as i64),
                    ),
                    (
                        "reference_only_streams",
                        Value::Arr(
                            input
                                .probe
                                .references
                                .iter()
                                .map(|(kind, id)| {
                                    obj(vec![("kind", text(kind)), ("file_package", text(id))])
                                })
                                .collect(),
                        ),
                    ),
                ];
                if let Some(tail) = &input.terminal {
                    facts.push(("copied_essence_sha256", text(copied)));
                    facts.push(("terminal_exclusion", tail.receipt()));
                }
                inputs.push(obj(facts));
            }
            let remaining = self.jobs[j..].iter().try_fold(0u64, |sum, job| {
                job.inputs.iter().try_fold(
                    sum.checked_add(8 << 20).ok_or("estimate overflow")?,
                    |s, i| {
                        s.checked_add(
                            self.inputs[*i]
                                .stamp
                                .len
                                .checked_mul(
                                    if job.mov && self.inputs[*i].probe.streams[0].kind == "audio" {
                                        2
                                    } else {
                                        1
                                    },
                                )
                                .ok_or("estimate overflow")?,
                        )
                        .ok_or("estimate overflow")
                    },
                )
            })?;
            if free_space(&self.destination)? < remaining {
                return Err(
                    "available disk space fell below conservative preflight estimate".into(),
                );
            }
            self.write_receipt(
                &format!("inputs-{:04}.json", j + 1),
                obj(vec![
                    (
                        "status",
                        text("input fingerprints verified; output NOT complete"),
                    ),
                    ("output", text(target.to_string_lossy())),
                    ("inputs", Value::Arr(inputs.clone())),
                    ("masters", Value::Arr(job.clips.clone())),
                    ("timecode", text(&job.tc)),
                    ("tools", strings(&self.versions)),
                ]),
            )?;
            let output_raw = if job.mov {
                super::mov::execute(self, job, &partial, &hashes)?
            } else {
                let mut command = Command::new(&self.ffmpeg);
                command.args(["-v", "error", "-nostdin", "-n"]);
                for i in &job.inputs {
                    command
                        .args(["-protocol_whitelist", "file,pipe", "-f", "mxf", "-i"])
                        .arg(&self.inputs[*i].path);
                }
                for (n, i) in job.inputs.iter().enumerate() {
                    command.args([
                        "-map",
                        &format!("{n}:{}", self.inputs[*i].probe.stream_indices[0]),
                    ]);
                }
                if let Some(tail) = &self.inputs[job.inputs[0]].terminal {
                    command.args(["-bsf:v:0", &tail.filter()]);
                }
                command
                    .args([
                        "-r:v",
                        &format!("{}/{}", job.rate.num, job.rate.den),
                        "-map_metadata",
                        "0",
                        "-c",
                        "copy",
                        "-timecode",
                        &job.tc,
                        "-f",
                        "mxf",
                    ])
                    .arg(&partial);
                run(&mut command)?;
                let output = probe(&self.ffprobe, &partial)?;
                if output.op != "060e2b34040101010d01020101010900"
                    || output.tc.as_deref() != Some(&job.tc)
                    || output.streams.len() != job.inputs.len()
                {
                    return Err("output OP1a/stream/timecode validation failed".into());
                }
                for (s, i) in output.streams.iter().zip(&job.inputs) {
                    let mut expected = self.inputs[*i].probe.streams[0].clone();
                    if s.kind == "audio" {
                        expected.packets = s.packets;
                    } else if let Some(tail) = &self.inputs[*i].terminal {
                        expected.packets = tail.frames;
                    }
                    if s != &expected {
                        return Err("output stream descriptors/duration/frame count changed".into());
                    }
                }
                if let Some(tail) = &self.inputs[job.inputs[0]].terminal {
                    tail.verify_output(&partial)?;
                }
                let sound_bytes = job.inputs.iter().skip(1).try_fold(0u64, |n, i| {
                    pcm_essence_bytes(&self.inputs[*i].path)
                        .and_then(|x| n.checked_add(x).ok_or("PCM size overflow".into()))
                })?;
                if pcm_essence_bytes(&partial)? != sound_bytes {
                    return Err("output PCM physical sample count changed".into());
                }
                if self.hashes(&partial, &output.stream_indices, None)? != hashes {
                    return Err("copied essence SHA-256 verification failed".into());
                }
                output.raw
            };
            for i in &job.inputs {
                let input = &self.inputs[*i];
                if let Some(tail) = &input.terminal {
                    let again = super::terminal::Terminal::qualify(
                        &input.path,
                        &input.probe,
                        &self.ffprobe,
                        job.rate,
                    )?;
                    if &again != tail {
                        return Err("source terminal boundary changed while rewrapping".into());
                    }
                }
                if stamp(&input.path)? != input.stamp || id_value(&input.path)? != input.identity {
                    return Err("input changed while rewrapping".into());
                }
            }
            // Hard-link publication refuses even a late file/dangling symlink, atomically.
            fs::hard_link(&partial, &target)
                .map_err(|e| format!("refusing to replace output {}: {e}", target.display()))?;
            fs::remove_file(&partial).map_err(|e| e.to_string())?;
            for (n, i) in job.inputs.iter().enumerate() {
                for path in &self.inputs[*i].aliases {
                    mappings.push(MediaRelink {
                        source_path: path.clone(),
                        target_path: target.to_string_lossy().into_owned(),
                        audio_stream: if n == 0 { None } else { Some((n - 1) as u32) },
                    })
                }
            }
            let record=obj(vec![("status",text("verified media; Premiere application read-back pending")),("output",text(target.to_string_lossy())),("output_probe",output_raw),("essence_sha256",strings(&hashes)),("inputs",Value::Arr(inputs)),("masters",Value::Arr(job.clips.clone())),("metadata_losses",text("Container/package identities regenerated. Sidecar preserves source facts. Optional metadata not represented by FFmpeg must be recovered from this receipt; native interpretation remains unqualified."))]);
            self.write_receipt(&format!("media-{:04}.json", j + 1), record.clone())?;
            records.push(record);
            control(j + 1, self.jobs.len())?;
        }
        self.write_receipt(
            "manifest.json",
            obj(vec![
                ("schema", text(if self.jobs.iter().any(|j|j.mov) {"anynle.media-preparation/1"}else{"anynle.op1a/1"})),
                (
                    "status",
                    text("media prepared; project completion recorded separately"),
                ),
                ("notice", text("Creates new media.")),
                ("tools", strings(&self.versions)),
                ("clock_handling",text("Native MXF timecode frame counts at exact picture edit rate; no rounded display-base timing. Picture packet time base, span and count validated before and after muxing.")),
                ("jobs", Value::Arr(records)),
            ]),
        )?;
        Ok(mappings)
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn exclusive_creation_reports_only_the_owned_media_directory() {
        let root = std::env::temp_dir().join(format!(
            "anynle-media-ownership-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let destination = root.join("new-media");
        let plan = Plan {
            destination: destination.clone(),
            inputs: vec![],
            jobs: vec![],
            ffmpeg: "unused".into(),
            ffprobe: "unused".into(),
            versions: vec![],
            estimate: 0,
            free: 0,
        };
        let mut owned = false;
        plan.execute_observed(&mut |_, _| Ok(()), &mut owned)
            .unwrap();
        assert!(owned);
        assert!(destination.join("manifest.json").is_file());
        let mut second_owned = false;
        assert!(plan
            .execute_observed(&mut |_, _| Ok(()), &mut second_owned)
            .is_err());
        assert!(!second_owned);
        assert!(destination.join("manifest.json").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
