# Any NLE native wizard

`any-nle-gui` is the native Rust desktop entry point, version `0.1.0-alpha.1`. It uses
eframe/egui 0.37 with the Glow renderer, AccessKit, bundled fonts, X11 and Wayland support.
It calls `anynle_app::job` in process. The CLI and GUI share the same job preparation and
publication engine; the GUI does not run the CLI as a subprocess.

The five steps are Project, Clips & media, Markers & colors, Effects, and Review.
The Project step starts with **Translate from** and **Translate to**. Both menus list
Avid Media Composer, Premiere, Resolve, Pro Tools and After Effects.
Avid Media Composer → Premiere is selected and is the only available direction.
To inspect the menus with a keyboard, Tab to each dropdown and press Enter or
Space to open it. Press Escape to close it.
The project step supports individual bins and project folders, the folder tree with
counts/search/checks, whole-project selection, Trash/Attic opt-in and sequence-source
dependencies. Media locations accept Avid roots, original-media roots and explicit
saved-prefix mappings. The clips step keeps
the chosen construction even when a qualified UME-only job bypasses native rewrapping.
New media uses a new folder; the visible notice is **Creates new media.**

Marker conflict rules, the 16 named Avid marker colors, all 32 clip palette swatches and
custom RGB mappings use the existing policy. Current effect handling is shown by type;
unsupported alternatives are not presented as working controls. Proxy attachment and
additional audio-to-source relinking remain unavailable and have no active controls.

On Review, **Run preflight** checks the job without creating outputs. The summary
shows project and media folder names; hover or use **Copy full path** from the
name's context menu for the exact location. A blocker appears at the top with
its cause and a way to fix it, such as opening media locations, opening
saved-path mappings, or choosing a new output. Preflight stops at the first
blocking stage. Run it again after making a correction.
If conversion can proceed with changes, Review shows the affected clips or
sequences where known. Warnings and the **Full preflight report** can be expanded
separately.
When ready, **Convert** uses that prepared job, and **Check again** refreshes it.
A prepared result belongs to one exact settings revision;
changing a path, translation choice, tool or preset invalidates it. Conversion moves that
same prepared value to the worker, which rechecks inputs before execution. Bounded progress
events keep the window responsive. Cancellation is cooperative; the UI waits for the
engine's completion result and reports retained files. Closing during a job can request
cancellation and waits for a safe completion boundary.

Open **Saved settings** in the sidebar to load or save a preset or job file. Presets
store translation choices and color mappings, without project/media/output paths.
Last choices are remembered locally. The shared job file stores exact paths for replay;
`any-nle-gui --job SETTINGS.json` loads it for a fresh check, and Review provides a CLI
equivalent. Settings schemas and bounds are validated. A preset save refuses an existing
file. No telemetry, updater or default AVB association is installed.

On completion, the native wizard shows actual output/receipt paths, retained media and a
searchable report. Open in Premiere is offered only when a local Premiere installation
is found on macOS or Windows; opening occurs only after the user clicks. Linux can run the
conversion engine and open output folders, but Premiere Pro is unavailable there.

Build with an isolated Rust 1.95 toolchain or newer:

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
