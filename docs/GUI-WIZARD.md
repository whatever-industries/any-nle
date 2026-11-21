# Any NLE native wizard

`any-nle-gui` is the native Rust desktop entry point. Run `any-nle-gui --version` to see its build version. It uses
eframe/egui 0.37 with the Glow renderer, AccessKit, bundled fonts, X11 and Wayland support.
It calls `anynle_app::job` in process. The CLI and GUI share the same job preparation and
publication engine; the GUI does not run the CLI as a subprocess.

The guided steps are **Project**, **Media**, and **Review**. **Advanced settings**
in the sidebar opens **Markers & colors** and **Effects**.
Avid Media Composer → Premiere is the default. Avid Media Composer → Resolve
is available for [local macOS qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#avid-to-resolve-in-the-macos-wizard).
Choose individual bins or a project folder under **Source bins**. The scrollable
tree lists paths and folders before the app opens any bin. Search reveals matching
branches in large projects. Each checkbox selects the whole bin. **Source options**
contains the Trash/Attic opt-in and sequence-source dependency settings.
Media locations accept Avid roots, original-media roots and explicit
saved-prefix mappings. The clips step keeps
the chosen construction even when a qualified UME-only job bypasses native rewrapping.
New media uses a new folder; the visible notice is **Creates new media.**

Marker conflict rules, the 16 named Avid marker colors, all 32 clip palette swatches and
custom RGB mappings use the existing policy. Current effect handling is shown by type;
unsupported alternatives are not presented as working controls.

In source builds with the offline and proxy controls, **Separate files**,
**Grouped**, and **Merged** offer the options below. Check the
[release notes](../RELEASE_NOTES.md) for packaged support.
**Allow offline media** lets qualified missing sources become relinkable items.
Under **Primary media**, choose **Use the media in the Avid edit** or
**Prefer camera originals for picture**. When you prefer camera originals,
Any NLE tries Avid's saved source path and falls back to qualified Avid media.
To locate moved originals, enter a saved folder prefix and choose its current folder under
**Saved-path mappings**.

**Attach matching Avid media as proxies** checks the Avid media against the
camera original. **Rewrap proxy for compatibility** uses an existing output
folder for a verified stereo proxy and displays **Creates new media.**
Review lists the matched camera file, channels used from that file, and the
proxy path. **Rewrap to native clips** uses its own media folder and requires
the source media online.
[Verification and relink scope](VERIFIED-RELINK.md)

**Run review** checks the job without creating outputs. The summary
shows project and media folder names; hover or use **Copy full path** from the
name's context menu for the exact location. A blocker appears at the top with
its cause and a way to fix it, such as opening media locations, opening
saved-path mappings, or choosing a new output. Preflight stops at the first
blocking stage. Run it again after making a correction.
If conversion can proceed with changes, Review shows the affected clips or
sequences where known. Warnings and the **Full preflight report** can be expanded
separately. **Save preflight report** writes a new JSON file. After a successful
preflight, it includes inventory notes and all findings, including those hidden
by the current search or filters. You can convert from the same Review after
saving. If preflight stops at a blocker, the report includes the blocker
category, reason, and the details collected for that stage. If a saved path
is missing, malformed, or ambiguous in **Rewrap to native clips**, Review
checks the remaining saved original paths with the current mappings. It reports
other locator problems without probing those files or preparing a project.
When legacy MXF verification blocks Rewrap, Review uses the existing media
index to report other absent or ambiguous package IDs on the affected clip
and later clips. For duplicates, it shows up to eight conflicting file paths
per package and the indexed count. The same path details are available for
duplicate managed packages reported by the linked clip modes. An indexed
package still needs stream verification.
The linked clip modes can continue past duplicate packages outside the
selected bins' saved dependencies when the dependency check is complete.
Review records those duplicates as warnings and can still use a qualified
omission or gap for a separate missing clip.
For a legacy package problem, **Open media locations** takes you to the Avid
media folders. For an unavailable linked original, **Open saved-path mappings**
takes you to the original's path mappings. The report flags any diagnostic
size limit reached. Reports can contain local paths and clip names.
For a longer blocker list, use **Search affected details** to find a bin,
clip, or package ID. Review shows up to 25 matching details per page. **Copy
details** and **Save preflight report** include the full list. Open **Activity** at
the bottom of the sidebar for the session's operational errors and full diagnostics.
In **Full preflight report**, the **All timelines** menu can narrow a ready
Avid to Premiere report to one timeline. The menu shows the source bin path
beside each name. The timeline choice works with search and the change and
warning filters. Saved and command-line preflight reports include
`timeline_groups` when there are scoped findings. Each group has an exact
timeline ID, its source bin when known, and zero-based indices into `report`.
Use **All source clips** in the same panel to see findings attached to
one clip or its timeline edits. This filter combines with timeline,
severity, and search. Saved and CLI reports include `source_groups` with
the source clip ID, bin when known, and indices into the same `report`.
For a sequence timing blocker, **Affected details** offers the same
timeline filter and text search. The saved and CLI reports put those
groups under `blocker.timeline_groups`, where the indices refer to
`blocker.affected`. **Copy details** still includes every finding.
For an unresolved-master blocker, **Affected details** also lists edits
with a unique saved master match. Filter them by timeline to see the
affected track, position when saved, and edit ID. Bin-level findings and
the first 1,024 matched edits are included in the saved and CLI reports,
with a total when the preview limit is reached.
Missing or ambiguous saved original paths also show saved edit references
in **Affected details** when the original's master and track identify them
uniquely. The timeline filter shows the edit's lane, position when known,
and source bin. Ambiguous master identities stay in the bin-level report.
For a legacy MXF sibling-package blocker, the same filter links missing,
ambiguous, or rejected packages to the edits that use the affected source
tracks. The edit's timeline lane can differ from the source track.
For a duplicate managed MXF package, Review shows the competing paths and
lists the edits that reference the exact package through a saved master
and source track. The timeline filter includes the package ID and the
record position when available.
When native rewrap requires a missing managed picture or audio package,
Review identifies the edits that use the missing source track. Missing and
duplicate package findings share the same timeline filter.
In **Separate files**, a verified 23.976 DNxHR LB clip can keep its picture
and any available PCM24 mono source linked when one or both mono siblings
are missing. Review lists the unavailable edits and the Premiere timeline
marks their verified spans.
When the picture is missing, the same mode can keep one or both verified
mono siblings on their original lanes and mark where picture or audio is missing.
When ready, **Convert** uses that prepared job, and **Check again** refreshes it.
A prepared result belongs to one exact settings revision;
changing a path, translation choice, tool or preset invalidates it. Conversion moves that
same prepared value to the worker, which rechecks inputs before execution. Bounded progress
events keep the window responsive. Cancellation is cooperative; the UI waits for the
engine's completion result and reports retained files. Closing during a job can request
cancellation and waits for a safe completion boundary.
While reading Avid bins, progress shows the findings from bins already read. Review
includes the checks that run after reading, such as cross-bin references and media linking.

Open **Saved settings** in the sidebar to load or save a preset or job file. Presets
store translation choices and color mappings, without project/media/output paths.
Last choices are remembered locally. The shared job file stores exact paths for replay;
`any-nle-gui --job SETTINGS.json` loads it for a fresh check, and Review provides a CLI
equivalent. Settings schemas and bounds are validated. A preset save refuses an existing
file. No telemetry, updater or default AVB association is installed.

## Automation

Use the wizard's **Save job settings** action once and replay the same job
headlessly. Review reads the job and prints a JSON report to stdout without
creating the project or media, launching an editor, or importing media.

```sh
any-nle review --job SETTINGS.json
any-nle run --job SETTINGS.json
```

The report uses `anynle.preflight-report/1`. Avid to Premiere uses the same
report as **Save preflight report**. Its `media_linking.premiere_files` lists
any originals that still need Premiere at conversion time. Local macOS Avid
to Resolve jobs report direct links, Resolve-assisted originals and planned
rewrap size where applicable. When Resolve is needed, the headless report
stops at media linking with `requires_editor`.

| Exit | Result |
| --- | --- |
| 0 | `ready`. Preflight completed. Check reported losses, media requirements and any required approval before running. |
| 1 | `blocked` or `requires_editor`. Inspect the blocker or media requirements in the report. A report write failure is diagnosed on stderr. |
| 2 | Invalid arguments or unreadable job settings, diagnosed on stderr. |

For desktop interface checks, load the job with
`any-nle-gui --job SETTINGS.json` and inspect the accessible controls. Review
and conversion recheck their inputs when run separately.

### Background wizard scenarios

Build the developer driver with
`cargo build --locked --manifest-path crates/Cargo.toml -p anynle-gui --features wizard-automation`.
Run the resulting `any-nle-gui --automation SCENARIO.json` from the local checkout.
The driver initializes one isolated Avid → Premiere wizard session and emits
one JSON response to stdout. It can run while another app has focus.

```json
{
  "schema": "anynle.wizard-automation/1",
  "actions": [
    {"action": "load_job", "path": "SETTINGS.json"},
    {"action": "find_bins"},
    {"action": "set_entire_project", "enabled": false},
    {"action": "set_folder_expanded", "path": "Editorial", "expanded": true},
    {"action": "set_bin_selected", "path": "Editorial/Selects.avb", "selected": true},
    {"action": "capture_project_layout"},
    {"action": "capture_screenshot", "path": "project.png"},
    {"action": "open_media"},
    {"action": "capture_screenshot", "path": "media.png"},
    {"action": "run_review"},
    {"action": "capture_screenshot", "path": "review.png"},
    {"action": "snapshot"}
  ]
}
```

Use a saved Avid → Premiere job with a project folder and output path. Bin
selectors use exact project-relative or absolute catalog paths. Folder selectors
use project-relative paths, with one action per folder to open a deep branch.
`snapshot` records the catalog, selection, effective job, and current preflight
report. `capture_project_layout` renders Project offscreen at 760 × 520 and
reports the bounds of the bin list and the Destination and Continue labels.
`open_media` moves to Media with the selected job settings. `run_review` runs
preflight and leaves Review open with its current findings and recovery actions.
In the macOS developer build, `capture_screenshot` renders the current Project,
Media, or Review page at 880 × 620 points (1760 × 1240 pixels). Project capture
uses the populated bin picker and its current selection and expanded folders.
`capture_project_screenshot` still works on Project. The driver returns the PNG
path and dimensions in `screenshots` and refuses to replace an existing file.
Rendering uses no native window or screen recording. Omit screenshot actions
when replaying on other platforms.
A blocked Review is a successful scenario result. An invalid scenario exits
with code 2 and a diagnostic. Runs leave source files and user preferences
untouched, and create no conversion outputs. Keep reports containing real
project paths in private test storage.

The [bin picker](../crates/gui/src/wizard/bin_picker.rs) shares catalog selection
and folder expansion with the background driver. The
[Review renderer](../crates/gui/src/wizard/review.rs) displays findings and
returns actions to the wizard controller. The controller owns the job inputs,
worker revisions, cancellation, and conversion.

On completion, the native wizard shows actual output/receipt paths, retained media and a
searchable report. Open in Premiere is offered only when a local Premiere installation
is found on macOS or Windows; opening occurs only after the user clicks. Linux can run the
conversion engine and open output folders, but Premiere Pro is unavailable there.

Build with the [pinned Rust toolchain](../rust-toolchain.toml).

```sh
cargo build --locked --release --manifest-path crates/Cargo.toml -p anynle-gui
cargo test --locked --manifest-path crates/Cargo.toml -p anynle-gui
```

`--help` and `--version` return without creating a window. Packaging places
`any-nle-gui`, `any-nle`, FFmpeg, ffprobe and the verified backend manifest together, as
specified by the release package. The brand asset is embedded into the executable.
Compilation, model tests and UI interaction do not establish new native NLE compatibility;
Windows/Linux execution and wider source profiles require their own acceptance evidence.

The copied command embeds the current job settings, so it works before conversion without
requiring a sidecar file. It uses a base64 settings argument, avoiding shell quoting of paths and JSON.
Commands beyond the portable size ceiling are disabled; use Save job settings instead.
`any-nle run --job SETTINGS.json --output NEW.prproj --new-media NEW_FOLDER` can replay a
completed job with new destinations. Existing outputs are never replaced.

Original links use exact saved locators and explicit saved-prefix mappings. There is no
basename search or unsupported original-folder scan control. Pre-flight hashes referenced
media in place and rechecks content before publication; large sources therefore take time.
The original-stream probe has a 30-second deadline. Cancellation is checked between bin reads,
media hash reads and media preparation jobs; a backend operation already running completes or
reaches its deadline before the next safe boundary.
