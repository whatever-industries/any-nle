# Premiere-assisted linking trial

`premiere-import-probe` is an internal executable from the `anynle-app` workspace package. It uses the existing Separate files writer for a selected graph/2 timeline. The trial covers straight cuts on V1, A1, and A2. The first media profiles are 1080p24 picture-only ProRes HQ with separate mono 48 kHz PCM16/24 WAV, and 1080p23.976 DNxHR LB OP-Atom with two mono PCM24 siblings.

Run the probe with a local UTF-8 JSON request. Use absolute project and media locations and an exact sequence ID from the graph.

```json
{
  "graph": "/path/to/source.graph.json",
  "sequence": "exact-graph-sequence-id",
  "output": "/path/to/new-output.prproj",
  "ffprobe": "/path/to/ffprobe",
  "donor": "/path/to/Donor.prproj"
}
```

An optional `original_sha256` object maps selected original paths to recorded SHA-256 digests. The probe rechecks file state before publication. `--review` checks the preparation and prints per-file link decisions without submitting a job to Premiere. `--replay-donor` requires a saved donor and exercises its records for each selected original, including media with a supported standalone encoding.

The standalone-first fallback currently resolves a saved ProRes codec family to the concrete Premiere `apch` tag after confirming the original is HQ and the donor agrees. The parser verifies the donor's exact absolute locators, importer, stream references, duration, raster, clock, and mono PCM layout. The writer uses these bounded media facts while retaining its own bins, source items, edit references, and channel assignments. The normalized codec and verified clock mapping live in destination preparation. The saved graph remains the record of the original edit.

## Automatic linking on macOS

Build the worker's CCX with `python3 tools/premiere-import-uxp/package_worker.py "/path/to/Any NLE Media Linking.ccx"`. Install the package in Creative Cloud or with Adobe's Unified Plugin Installer Agent. Start Premiere 27 and leave it at Home with projects closed to allow assisted linking. The installed worker starts in Premiere and uses its own per-user plugin data folder. [Adobe installation guide](https://developer.adobe.com/premiere-pro/uxp/plugins/distribution/install/) · [Startup plugins](https://developer.adobe.com/premiere-pro/uxp/plugins/#invisible-plugins)

Omit `donor` from the request to use the worker automatically when the standalone writer needs a Premiere media record. Open `any-nle-gui --premiere-import-job REQUEST.json` for **Review → Status → Media linking** or run the probe directly. Review lists direct and assisted originals and offers **Launch Premiere** when useful. If a project is open, the worker waits for the user to close it. Convert rechecks eligibility before it queues a request. The worker imports into a new disposable project, saves the donor, and closes its own project. The converter verifies the returned media and publishes the destination exclusively. Standalone conversions run with Premiere closed.

## Manual donor capture

Use Premiere 27 and an authorized local development helper. The panel uses Adobe UXP directly. [Adobe Project API](https://developer.adobe.com/premiere-pro/uxp/ppro-reference/classes/project) · [Adobe ingest settings API](https://developer.adobe.com/premiere-pro/uxp/ppro-reference/classes/ingestsettings)

1. Open an empty, saved disposable project named `Any NLE Import Worker ‹label›.prproj`. Read its exact path and GUID with **Read scratch identity**.
2. Prepare a private request with `schema` set to `anynle.premiere-import/1`, a fresh UUID in `request_id`, the reported `scratch_path` and `scratch_guid`, and an `originals` array of distinct absolute paths.
3. Use **Import originals from request** to select that JSON and a private results folder. The helper checks the active scratch identity, disables ingest in that project if needed, imports each named file, and checks every imported path and online status. The results folder contains `Donor.prproj` and `capture.json` after success. An incomplete import records a failure. Use a fresh empty scratch project for a retry.

## Avid to Premiere wizard

On macOS, **Separate files**, **Grouped**, and **Merged** use the installed helper automatically when a verified original needs a Premiere media record. **Review → Media linking** shows which originals need Premiere. Open Premiere at Home to enable conversion. The same fallback runs with `any-nle run --job` using the wizard's saved settings. Use worker 0.2.0 or later for the general fallback.

Supported standalone originals link directly. For a file that needs Premiere, Any NLE checks the Avid locator, descriptor, and physical stream facts first. The generic MOV/MP4 source check covers one progressive square-pixel video stream with a constant frame clock and optional stereo PCM. It verifies the entire picture and PCM packet clocks. The fallback accepts Premiere's native media encoding after checking the saved path, picture geometry and extent, sample clock, and channel order. If Premiere leaves the logging clock unset, the independently verified source timecode stays in the project. Graph/2 owns the edits and selectors. Avid-managed OP-Atom stays on Any NLE's managed-media routes.

The local installed-helper check uses a disposable saved job with one qualified original, a fresh destination, and Premiere at Home. Set `ANYNLE_WIZARD_ASSISTED_JOB` to that job and run the check from the converter root.

```sh
cargo test --locked --manifest-path crates/Cargo.toml -p anynle-gui \
  --bin any-nle-gui --features live-premiere-tests qualify_installed_premiere_helper_from_wizard
```

The test exercises the normal wizard in process without pointer or keyboard access. The explicit feature keeps the editor requirement out of the offline test suites.

All three modes passed the same Review and Premiere 27.0.0 build 88 Save As and reopen checks. Each mode matched all 32 decoded frames of the retained native Premiere reference at source frames 5–36. Grouped and Merged also refused publication when the helper was unavailable.

The general fallback also passed a Separate files test with 24 fps ProRes LT MOV and embedded stereo PCM24. After Save As and reopen, all 36 decoded picture frames and both isolated 72,000-sample lanes matched the native Premiere reference exactly. A linked HEVC MP4 with the same stream layout passed assisted import and structural read-back through the same code.

## Qualification

Retained Premiere 27 ProRes/WAV and OP-Atom saves provide structural donor evidence. Replay the test sequence from each graph, verify every staged edit, and compare source clock, alternate or physical start, logging timecode, and file-relative selectors independently. The earlier ProRes LT OP-Atom playback failure remains a control for the limits of saved-project checks.

The live UXP capture routine passed on macOS in Premiere 27.0.0 build 88. Adobe UXP Developer Tool loaded a disposable harness that called the helper's capture routine directly. It imported all six originals online into two empty projects and saved both donors. Save As, close, and reopen preserved the cuts, independent trims, clocks, and physical audio selectors. A second pass reproduced the picture and isolated audio results.

| Profile | Reopened picture | Reopened audio |
| --- | --- | --- |
| 24 fps ProRes HQ with mono PCM16 and PCM24 WAVs | All 48 decoded frames matched the native Premiere control. | All 96,000 PCM24 samples per lane matched the standalone conversion exactly. The Premiere-authored control used a different center-pan level. After accounting for the converter's −3.01 dB center gain, both lanes matched the native control within one PCM24 count. |
| 23.976 fps DNxHR LB with two mono PCM24 OP-Atom siblings | All 72 decoded frames matched the retained direct-link control. | Both 144,144-sample PCM24 lanes matched the direct-link control exactly. |

The normal fallback used the live donor for ProRes picture and linked the WAVs directly. The DNxHR case linked all three originals directly by default. Explicit donor replay also passed for every original in both cases. The trial remains internal.

The CCX-installed worker passed a separate qualification on Premiere 27.0.0 build 88 with Developer Tool closed. The automatic conversion requested only the ProRes picture. Repeating the conversion recreated the same links. The OP-Atom control used standalone linking. After Save As, close, and reopen, both projects matched the prior direct-link picture and isolated PCM references sample for sample. With an editing project open, the installed worker refused an import and the active project, sequence, playhead, and saved file hash stayed the same. The qualification used Adobe UXP and local file exchange throughout.
