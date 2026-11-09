# Optional OP-Atom to OP1a media preparation

2026-11-04. CLI implementation with structural/synthetic verification; native Premiere
checks now cover bounded audio-routing fixtures and the
Avid-saved native-source trial; User acceptance
and wider profiles remain pending. The command-line app offers a macOS folder
picker and media review. The native wizard also reviews the destination and disk
space before conversion. Both display **“Creates new media.”** Source media stays
in place.

## Multichannel / ProRes extension (2026-11-07)

`--rewrap-media NEW_DIR --rewrap-native-master` selects DNx OP1a or bounded ProRes LT
MOV automatically by source codec. Declared `51:`/`71:` groups now reach native
5.1/eight-channel Adaptive masters and matching timeline outputs. See the
current scope and tests. The older DNx-only mode
and folder prompt below retain their meaning. Surround is never inferred from count.

## Current CLI

```sh
any-nle convert --from avid BIN.avb --media SOURCE_FOLDER --to premiere OUTPUT.prproj \
  --rewrap-op1a NEW_MEDIA_DIRECTORY --rewrap-dry-run
# Repeat without --rewrap-dry-run to create media and the project.
```

### Native macOS folder prompt

Replace `--rewrap-op1a NEW_MEDIA_DIRECTORY` with `--rewrap-op1a-prompt`.
Choose an existing **parent location** in the native folder picker. Any NLE proposes a new
`OP1a Media` subfolder (`OP1a Media 2`, etc. if occupied), without creating it yet.
The review displays **“Creates new media.”**, the full destination, new MXF count,
conservative additional size in readable units and bytes, and available disk space on the
chosen volume. Choose **Create media** to continue or **Cancel** to leave without outputs.
Both picker cancellation and review cancellation leave media, project and graph unwritten.
All existing preflight refusals still apply; a disallowed location or insufficient space
returns an error without starting conversion. Choose another location on the next run.
The estimate covers media (whole input sizes plus 8 MiB per output), not project/graph size;
it is not an exact prediction. Available space is rechecked during writing.

With `--rewrap-dry-run`, the picker still opens, but the review is printed in the terminal;
there is no Create media dialog and nothing is written. The two destination options are
mutually exclusive. Explicit `--rewrap-op1a NEW_DIR` remains noninteractive. Other systems
must use the explicit destination option; native prompting currently requires macOS.
The picker uses synchronous main-thread [rfd 0.17.2](https://docs.rs/rfd/0.17.2/rfd/);
review uses AppKit NSAlert through objc2/objc2-app-kit/objc2-foundation bindings
(MIT licenses; Cargo.lock pins packages/checksums). The CLI finishes AppKit launch once
so modal dialogs remain responsive to accessibility. The picker cannot create folders.
No scripting bridge or editor is required.
This UI work does not qualify or change Premiere source-channel mapping.

In the CLI, `--rewrap-op1a` is optional and off by default; its directory must NOT already exist.
The parent must exist. It must be outside scanned source-media roots and Avid MediaFiles,
and must not overlap the project/graph paths. Both preflight and execution show the exact
notice. Dry run writes no media, project or graph. This is an Avid→Premiere-only option;
it is rejected by graph/colors/FCPXML commands. The graph snapshot remains the original
source graph; the output media receipts describe physical path/stream substitutions.

Provide installed FFmpeg 8.1 and ffprobe 8.1 on PATH or use `--ffmpeg PATH --ffprobe PATH`.
Tools are external, not bundled or auto-installed. Record their version in receipts;
distribution/license review is required before bundling any build. Free-space preflight
uses `df -Pk` on macOS/Linux; Windows preparation is currently refused. Estimates are
conservative input sizes plus header allowance and are rechecked while writing.

The synthetic backend cohort covers DNxHD/DNxHR LB/SQ/HQ, 8-bit 4:2:2 picture,
with video-only or 1–32 separate mono PCM 48 kHz, 16/24-bit inputs. Each output uses uniform 16-bit or uniform 24-bit audio; mixed-depth
assembly is refused. They remain discrete streams; no surround layout is inferred.
Explicit unique Avid source-track numbers determine order, never filenames.
Full spans, zero packet origins, native timecode bases/drop flags and exact source
clocks must agree. Padded/unequal spans and conflicting physical recipes refuse.

Native-source preflight also recognizes the exact observed RI00/DNxHR LB coding label,
reference-only sibling-package entries and nonzero demuxer essence indexes. It resolves
qualified file-source timecode offsets instead of confusing physical-origin timecode with
file start. The selected Avid-created fixture now passes the
guarded terminal-zero handling and bounded native
Premiere trial. Other terminal layouts and source profiles remain unqualified; the
native MXF preflight report preserves its earlier
blocked state as history.

Supported exact picture rates (decimal labels are shorthand):

| Rates | Exact fractional rates | Project display |
|---|---|---|
| 23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60 | 24000/1001, 30000/1001, 60000/1001 where applicable | Adobe timecode-display enums; bounded 23.976 trials and selected29.97/59.94 surround read-back pass;59.94 short-loop performance remains open; wider profiles/rates and User acceptance remain pending |
| 47.952, 71.928, 72, 75, 89.91, 90, 95.904, 96, 100, 119.88, 120 | 48000/1001, 72000/1001, 90000/1001, 96000/1001, 120000/1001 where applicable | Experimental frame-count display (109), explicitly reported |

The 1001-based 29.97/59.94/89.91/119.88 clocks also accept drop-frame source labels.
Drop-frame changes labels, not playback rate or PCM samples. Starts must fit one
24-hour timecode day. The rewrap route derives starts from native MXF frame counts
at the exact picture edit rate; this corrects the ordinary reader's rounded-base
start-clock handling without changing that default path. Source offsets remain relative.
The source graph snapshot retains original container paths and these exact source clocks.

FFprobe's guessed `r_frame_rate`/average rate is not authoritative here: MXF picture
packet time base, duration and packet count must agree with the model. The muxer receives
that same validated rational rate; output clocks and copied essence are checked again.
The extended-rate cohort does not establish correct per-frame System Item timecode,
interlaced interpretation, or native Premiere handling. Those remain qualification gaps.

| Audio type/layout | Current behavior |
|---|---|
| 48 kHz signed little-endian PCM, 16/24-bit, one channel per OP-Atom file | Copy 1–32 streams at one depth per output; refuse mixed depths; preserve sample bytes/order |
| 44.1/96 kHz, float/32-bit PCM, compressed audio or AES3 needing conversion | Refuse; no implicit resampling, decoding/re-encoding or repacking |
| Interleaved multi-channel input, audio-only OP1a output | Refuse in this backend route; no splitting, mixdown, dummy picture or guessed layout |

Timeline preparation allocates audio tracks up to lane 99 where the selected bus
allows expansion. Expanded layouts still need native read-back. A source bin may
contain up to 32 discrete sound files. Audio-only masters remain outside this
picture-led rewrap route. Track capacity and limits.
The initial CLI conservatively refuses reader/media-scan problems (including omitted
sound or skipped clips, even if another source is usable); repeated input-bin aliases
are allowed. Dry runs also show translation losses.
PCM duration is additionally checked from actual bounded KLV sound payload sizes: a
reported duration alone cannot justify copying padded/truncated material.

Each output uses `-c copy`, all explicitly selected streams, forced local MXF demuxing and
local file/pipe protocols. Output operational pattern, all stream facts, picture packet
count, timecode, physical PCM bytes and copied-stream SHA-256 must match. Package identities
are regenerated and recorded. A source changed during preparation refuses. Final media
names are published by an exclusive hard link; late existing files/symlinks are preserved.
A job failure retains partial/verified outputs with failure/input receipts, and returns an
error without a new completed project. Existing media directories are never resumed or
reused automatically. Cancellation can leave uncommitted `.partial-*` files; these are not
marked complete. Never delete retained outputs automatically as if they were originals.

`inputs-NNNN.json` records pre-mux fingerprints and original metadata; `media-NNNN.json`
records successful output facts; `manifest.json` means media preparation completed.
`conversion.json` records a completed project write. Separate failure receipts distinguish
media preparation from later project failure. Identical physical recipes are deduplicated
while logical master clips remain distinct. The bin-derived channel configuration assessment
separates storage order, stereo pairs and Premiere source-channel mapping. Unsupported
bin group declarations refuse before projection; receipts retain per-master track maps
and ordered stereo pairs. File and audio-stream bindings live outside
frozen model/1; Rust and Python writers match exactly. The native route uses full-source
templates and explicit physical-channel vectors. Legacy clip constructions use flattened
SingleAudioChannel/SingleChannel views. StreamNumber is not used for OP1a.
[Scoped native results and remaining qualification](NATIVE-OP1A-MASTERS.md) are explicit.

## Optional native AV-master construction

`--rewrap-native-master` explicitly selects experimental ordinary file-backed AV masters
in place of Grouped/Merged/Separate construction. It requires either rewrap destination option; omit
`--bin-mode`. Supported bin mono/stereo groups and sequence routing are preserved, with
lossy combinations refused before media creation. Bounded native save/reopen/export and fresh-bin-insertion checks now cover mono, stereo,
reversed and mixed groups; User acceptance and wider profiles remain pending. [Full use, limits and evidence](NATIVE-OP1A-MASTERS.md).
`conversion.json` records the chosen construction; media receipts retain logical master
channel maps/pairs. Rewrap without the flag retains the existing clip-build choice.

## Import option and review

The wizard puts native rewrap first and selects it for new or reset settings.
Grouped, Merged and Separate follow. Remembered choices and presets still apply.
The standalone `convert` CLI selects rewrap through explicit flags.

The wizard's native route prepares DNx as OP1a MXF and admitted ProRes LT as MOV.
Review shows **“Creates new media.”**, the destination and estimated and available
space before execution. Premiere links to verified outputs, while the source
media stays in place. Choose a new output folder outside Avid’s managed MediaFiles
tree. [Wizard flow and qualification](GUI-WIZARD.md).

The broader repair option for other formats remains in the
[roadmap](ROADMAP.md#media-linking-user-2026-10-14).

## Broader requirements and follow-up scope

The CLI and wizard implement a bounded subset of these requirements. Wider metadata,
additional audio types and layouts, and verified resume or reuse remain follow-up work.

1. Resolve the exact OP-Atom picture/sound set using full identities and explicit source-track
   routing. Preserve distinct masters. Do not group by V/A prefixes, basenames, shared origin
   alone or a guessed universal UME classification. Refuse missing/ambiguous sources.
2. Build a preflight recipe for an explicitly qualified codec/profile and stream layout.
   Initial scope should be ordinary video with mono/stereo PCM, plus video-only if qualified;
   audio-only and wider layouts need separate qualification. Preserve full source ranges
   initially so clip offsets and handles need no newly invented trim arithmetic.
3. Choose and pin a muxing backend/version, including its license/distribution requirements.
   Copy existing compressed video essence; preserve PCM sample values and explicit channel
   order if container packing changes. Do not silently encode, resample, remix or retime.
   Refuse an unsupported copy/mux combination before starting media output.
4. Stage job-owned output files in the chosen destination, validate them, and publish without
   overwriting. Cancellation/failure must leave original files untouched and must never label
   a partial file as complete. Keep the previous valid project intact on failure. Initial
   behavior should refuse unsupported selected recipes rather than silently claim success.
5. Reprobe the completed container and bind its actual stream identifiers/order to the project.
   Preserve exact rates, source offsets, timecode/drop-frame, channel names, audio phase/sync,
   pixel aspect and color metadata where supported. Record unrepresentable metadata as loss.
6. Keep a sidecar/import receipt with original path/identities, input/output fingerprints,
   track mapping, original/output clock facts, conversion recipe/backend and metadata losses.
   Container/package IDs can change: record a mapping, never pretend the new file is identical
   to the original. Reuse a prior output only after its exact inputs, recipe and bytes verify.

Backend capability outside the implemented cohort remains an investigation. FFmpeg's documented
[stream-copy mode](https://ffmpeg.org/ffmpeg.html#Streamcopy) avoids decoding/filtering/encoding
for copied streams; this does not establish that its MXF muxer accepts every selected codec,
audio layout or metadata field. The installed 8.1 synthetic cohort above is tested; other builds/profiles are not qualified. See rate/audio verification for the bounded matrix and provenance.

## Evidence and integration boundaries

Avid knowledgebase revision: `a166c881e78af7ee515d7e0940a992b2858e4c9b`.
The following exact finding hashes were reviewed for this plan; CONV-0030 now pins these references for the bounded implementation, without declaring native Premiere support:

- AVB-0021: `de0bc531ec9a98b60e939f84d44982758fb5af751acb7a9ae1b94af8bfd0dfc5`.
- AVB-0023: `094de93b46a9ddd419eba1fa65a92663342e9e0eae0c1fa3461963111f494a82`.
- PMR-0003: `cfafa3d52e842e76207c70e77e377e7477289352107dddc555bfdcfb0cfae8db`.

`AVB-0021` scopes native/linked identities and
`PMR-0003` scopes package identity joins.
`AVB-0023` covers native managed OP1a;
its descriptor-duration counterexample prohibits blindly treating a duration field divided
by audio sample rate as an authoritative length. It does not qualify converter-written OP1a.

Media preparation belongs in the app's conversion orchestration, with a preflight/job manifest
and explicit path/stream substitution before project writing. Keep muxing out of the pure
Premiere writers; assess model/graph path, source-range and stream-mapping interfaces before
changing them. Preserve Rust writer/Python oracle parity for shared project-output
changes. The wizard and saved-job CLI share the preparation engine. The standalone
conversion CLI uses the same media planner. Wider native qualification remains open.

## Acceptance and remaining native qualification

- With the option disabled, existing graph/model/project outputs and clip choices are unchanged.
- With it enabled, synthetic mono/stereo PCM and qualified compressed-video fixtures preserve
  sample values/order, essence, rational clocks, nonzero start/timecode and source offsets.
  Distinct tones/impulses must catch channel swaps and phase shifts; copied video essence must
  compare equal with framing differences accounted for. Whole-file hashes need not match.
- Missing/ambiguous streams, unsupported codecs/rates, unequal spans, output collisions,
  disk exhaustion, cancellation and a corrupt/stale resume receipt refuse safely and report
  the affected source. No original media or valid project is overwritten.
- The new file's OP1a structure and actual stream map must validate independently of file
  extension. User checks in Premiere must confirm import, source-channel interpretation,
  sync, sequence edits, save/reopen and export; parser acceptance alone is insufficient.
- Option/review include **Creates new media.**, destination and disk estimate. The import
  report identifies each created/reused/skipped/failed output and all losses. Disk estimates
  use qualified essence durations/sizes, not the AVB-0023 misleading descriptor shortcut.

Application tests should use small disposable fixtures and read existing footage
in place. Actual rewrapping follows the selected construction and confirmed media
destination.

The 2026-11-07 playback/acceptance follow-up
separates exact media/routing preservation from the reproducible59.94 loop-drop
symptom. A successful render or test suite does not clear that performance gate.
