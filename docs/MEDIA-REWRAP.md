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

## Calculate rewrap size

Use **Calculate rewrap size** in Review to see the total size of the
source files for the planned rewrap outputs. A shared output counts once
across edits. A source used in multiple distinct outputs contributes to
each. The button reads file metadata without starting an editor or
running FFmpeg. The converter checks actual available space before writing.

## Current CLI

Use `--rewrap-needed NEW_DIR` in a linked Avid → Premiere conversion to keep
qualified originals in place and rewrap the managed clips that require it.
Wizard Review lists the affected clips and asks for the new media location.
Without a folder, supported clips still convert. The writer marks saved edit
spans it can safely leave vacant and retains independently routed mono audio
in the qualified Separate files case. The saved job carries the selected
folder in optional `rewrap_needed`. Existing full-rewrap jobs keep their
media choice.

### Managed picture and mono streams

Graph/2 recognizes the registered DNxHD/DNxHR codec labels and the six
ProRes 422 and 4444 labels. It retains the raw MXF display, field, pixel
layout, and color descriptor observations alongside the saved Avid clock
and package references. Rewrap checks the coded profile, exact raster,
rate, and extent. Progressive square-pixel media has offline controls
across the new codec paths. The selective route reports non-square or
non-progressive candidates as pending when the saved descriptor establishes
those properties. Native interpretation remains separate from wrapping.

### Anamorphic ProRes in the full rewrap route

The explicit native-media rewrap uses the saved graph/2 picture descriptor
and file package to verify the MXF and probed pixel aspect. The MOV retains
that ratio. Premiere gets the same exact interpretation on the source
stream and new timeline placements. The graph and saved-job schemas are
unchanged. A conflicting descriptor or file package blocks publication.

Offline checks on picture-only 1440×1080p24 ProRes 422 Standard and LT
preserved 4:3 pixels, all 36 compressed packets and decoded frames, and
two 24-frame edits with six-frame source trims. Standard passed through
the direct CLI and saved jobs. The LT saved job also preserved a nonzero
source timecode.

Premiere 27.0.0 build 88 preserved the 4:3 source interpretation, media
paths, and exact trims after Save As, close, and reopen. For each profile,
six exported frames across both edits matched pixel for pixel before and
after reopen and against fresh Premiere insertions on the same canvas.
Premiere saved square-pixel values on the timeline items for that canvas,
with the 4:3 interpretation still on the source stream.

The controls have no verified Avid sequence canvas. The resulting
1440×1080 square-pixel fallback centers the picture and crops the sides
of the 16:9 source, as does Premiere on the same canvas. A native sequence
created from the clip used 1440×1080 with 4:3 pixels. Full-width framing
on an Avid-authored anamorphic sequence remains a separate qualification.
Selective rewrap still reports anamorphic managed ProRes as pending.

### PCM siblings

The MXF route carries one mono PCM stream per sibling. The selective MOV
route also uses a distinct mono stream per sibling. Existing full MOV
rewrap keeps its qualified packed audio layouts through eight channels.
Offline container checks cover up to 64 audio siblings. Premiere 27.0.0
build 88 imported and saved the 32-channel MXF and MOV through UXP. It
saved one 32-channel audio view for the MXF and 32 one-channel stream
views for the MOV. Both generated timelines preserved all 32 mono lanes,
two source trims per lane, and their selectors after Save As and reopen.
Fresh insertion matched the native importer on all 32 lanes. All 48
picture frames and isolated audio on A1, A16, A31, and A32 matched the
controls, with the established stereo pan law applied to PCM.

Premiere rejected both the 33- and 64-channel MXFs during import. Treat
32 as the per-file planning ceiling for DNx rewrap into Premiere. The
rewrap plan uses picture-only OP1a and separate mono WAVs for qualified
33–64 lane DNx masters. [Wide-audio verification](#wide-audio-with-separate-mono-wavs).
Mixed PCM depths still require an independently qualified mapping.

[SMPTE VC-3 labels](https://registry.smpte-ra.org/view/published/labels_view.html)
· [ProRes MXF mapping](https://pub.smpte.org/latest/rdd44/rdd44-2022.pdf)
· [Premiere channel support](https://helpx.adobe.com/ro/premiere-pro/how-to/mix-multichannel-audio.html)
· [EBU MXF channel guidance](https://tech.ebu.ch/docs/r/r134.pdf)

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

The synthetic backend cohort covers DNxHD and DNxHR LB/SQ/HQ/HQX/444,
with video-only or 1–64 separate mono PCM 48 kHz, 16/24-bit inputs. Each output uses uniform 16-bit or uniform 24-bit audio; mixed-depth
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
| 48 kHz signed little-endian PCM, 16/24-bit, one channel per OP-Atom file | Copy 1–64 streams at one depth per output; refuse mixed depths; preserve sample bytes/order |
| 44.1/96 kHz, float/32-bit PCM, compressed audio or AES3 needing conversion | Refuse; no implicit resampling, decoding/re-encoding or repacking |
| Interleaved multi-channel input, audio-only OP1a output | Refuse in this backend route; no splitting, mixdown, dummy picture or guessed layout |

Timeline preparation allocates audio tracks up to lane 99 where the selected bus
allows expansion. Expanded layouts still need native read-back. A source clip may
contain up to 64 discrete sound files. Audio-only masters remain outside this
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

## Wide audio with separate mono WAVs

The explicit and selective Avid → Premiere rewrap paths write a
picture-only OP1a plus one 48 kHz PCM16 or PCM24 WAV for each independent
mono sibling on qualified 33–64 lane DNx masters. The plan checks a shared
source clock, a common bit depth, and full physical sample extents.
The WAVs retain the original PCM bytes. Review counts the new WAVs
and the receipt records each saved lane, WAV path, and sample hash.
Each WAV has its own complete Premiere media-state record. At 32 mono
lanes the DNx rewrap continues to use a single OP1a.

Premiere 27.0.0 build 88 reopened generated DNxHR HQ projects with 32,
33, and 64 independent mono lanes. Both 24-frame trims on every lane
and one reusable bin master per project survived Save As, close,
and reopen. Fresh insertion retained all lane assignments and their
full source spans. All 144 decoded picture frames matched the reference.
Across the 97 trimmed lanes, each 96,000-sample isolated stereo export
matched its source on both outputs within one PCM24 count after the
center pan law. The same comparison passed on seven fresh-insertion
lanes, including A33 in both wide projects and A64 in the wider
project. Four trimmed and two fresh-insertion lanes at 32 passed the
same PCM comparison. Every WAV's decoded PCM matched its source MXF.

The control projects that linked the same synthetic mono MXFs in place
showed the correct paths and selectors after reopen. Every isolated lane
rendered silence. An independent Premiere import of the same A33 MXF
also rendered silence.

Separate 33- and 64-lane PCM24 controls used the same synthetic 1080p24
picture and distinct native-depth mono samples. Premiere 27.0.0 build 88
reopened both trimmed timelines and both fresh insertions. Each of the
97 WAV payloads matched the corresponding source MXF byte for byte.
Isolated PCM on all 97 trimmed lanes and all 97 fresh-insertion lanes
matched the source on both outputs within one PCM24 count after the
center pan law. The first fresh A32 export in the 64-lane project was
silent. A repeat with a pause after changing track mutes matched from
the same saved project. All 96 decoded picture frames matched the
reference. The mixed-depth control was refused before creating a
project or media.

Nonzero starts use a Broadcast Wave `bext` time reference. The writer
converts the shared source clock to an exact 48 kHz sample count and
records it in each WAV receipt. A start between samples or a sibling
clock that disagrees with picture is refused before writing.
Zero-start WAVs retain their original header and file bytes.
[EBU Tech 3285](https://tech.ebu.ch/publications/tech3285/)

A synthetic 1080p24 control with 33 PCM24 siblings at `01:02:03:04`
kept that source start on picture and every audio lane after Premiere
27.0.0 build 88 Save As and reopen, including after fresh insertion.
A plain-WAV control lost its saved audio origins on fresh insertion.
The Broadcast Wave outputs from explicit and selective rewrap were
byte-identical, with every payload matching the original MXF.
All 48 picture frames and all trimmed and fresh-insertion lanes
matched the controls. Premiere briefly reported the fresh edit offline
on the immediate read-back. The saved project read online on a later
reopen and all audio lanes rendered correctly. A sibling offset by one
frame was refused.

Two more 33-lane PCM24 controls passed Premiere 27 reopen and fresh
insertion at `24000/1001` and `30000/1001` drop-frame. Picture and every
WAV retained starts of `01:02:03:04` and `01:00:59;23`, respectively.
The drop-frame trim crossed the minute boundary. All 98 trimmed
picture frames matched the corresponding frames in the fresh
insertions, and all 66 lanes matched the original PCM on both stereo
outputs within one PCM24 count after center pan, in both the trimmed
and fresh-insertion renders. Each WAV payload matched its source MXF
byte for byte. Changing the drop-frame start to `01:00:59;24` put it
between 48 kHz samples and was refused before creating a project or media.

## Managed DNxHR HQ

The Avid graph reader recognizes DNxHR HQ by its registered VC-3 CID 1272
label and the label variant written by FFmpeg 8.1. Any NLE routes these
managed sources to explicit OP1a rewrap and checks that the coded picture
profile agrees. The same rule checks the saved raster against the actual
file. Derived AVBs with synthetic 1080p24 and UHD24 picture-only MXFs passed
conversion and byte comparison of all 36 coded picture packets per source.
[SMPTE Labels Register](https://registry.smpte-ra.org/view/published/labels_view.html)
· [FFmpeg 8.1 MXF codec labels](https://github.com/FFmpeg/FFmpeg/blob/n8.1/libavformat/mxfenc.c#L2196-L2200)

Premiere 27.0.0 build 88 retained the linked OP1a MXFs, HD and UHD rasters
at 24 fps, one-hour sequence start, zero source start, and both source
trims after Save As, close, and reopen. Each timeline uses source frames
`[6,30)` twice. All 96 decoded frames across the two ProRes 422 HQ
exports matched those intervals in fresh Premiere imports of the same
OP1a media. A second reopen and export repeated the result at both
rasters. The retained AVB and OP-Atom input hashes matched.

The linked best-effort jobs on the same fixtures published both edits as
marked gaps. A rewrap job with an HQ descriptor over SQ essence refused
before creating a project or media.

Premiere 27.0.0 build 88 showed an import-failure dialog for each of the
synthetic HQ OP-Atom originals at 1080p24 and UHD24. After each dismissal,
the scripting call returned success and the scratch project had zero items
matching the requested path. The same test imported, reopened,
and rendered the rewrapped HQ OP1a and an Avid-authored DNxHR LB OP-Atom
control. Keep explicit OP1a rewrap for this HQ profile. Direct import
of an Avid-authored HQ source still requires a separate test.

## DNxHR HQX, DNxHR 444, and ProRes alpha

Premiere 27.0.0 build 88 reopened the converted 1080p24 DNxHR HQX OP1a
project. Both 24-frame trims matched all 48 decoded frames of the
corresponding intervals from a fresh Premiere import. The 1080p24 DNxHR
444 OP1a control isolated a rendering difference to Premiere's saved source
color profile. The rewrap plan now selects the native full-range RGB
interpretation for verified progressive, square-pixel DNxHR 444 10-bit
YUV 4:4:4 with a Rec.709 matrix and unspecified range, transfer, and
primaries. The Premiere writer applies it to the bound picture; the
compressed source and rewrapped essence remain identical.

The explicit native-master CLI, the saved job, and selective rewrap
each passed Save As, close, and reopen in Premiere 27. Both source
trims, clocks, and the saved source color profile survived. All 48 frames
per route matched fresh imports in 4:4:4 exports. The DNxHR 444 native
comparison used a synthetic MXF and derived Avid bin.

The 720p24 picture-only ProRes 4444 and 4444 XQ OP-Atom fixtures carried
alpha in every compressed frame. FFmpeg 8.1 wrote a MOV video sample depth
of 24 for these stream copies, and Premiere exported fully opaque picture.
Any NLE now writes depth 32 in the generated MOV sample description when
the verified ProRes pixel format carries alpha. This uses the same bounded
metadata rewrite as the packed PCM path and preserves compressed packets
and media offsets. [FFmpeg 8.1 video sample description](https://github.com/FFmpeg/FFmpeg/blob/n8.1/libavformat/movenc.c#L2625-L2636)

In Premiere 27, both corrected projects passed Save As, close, reopen, and
fresh insertion from the rewrapped file. All 48 exported frames per
profile matched the corresponding fresh-import render. Every decoded
alpha plane matched the source interval, and a second reopen and export
reproduced both results. These tests use derived Avid bins and synthetic
picture with partial transparency.

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

In the linked modes, Review also offers selective rewrap for verified
legacy 23.976 DNxHD LB bundles with eight PCM24 mono siblings and an
unqualified direct file clock. Choosing a folder prepares those packages
for OP1a rewrap. In Separate files, proceeding without a rewrap folder can
leave a marked picture gap with the independent mono links in place.

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
