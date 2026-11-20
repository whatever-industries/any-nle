# Internal Resolve turnovers

`resolve-turnover-probe JOB.json` prepares and publishes a linked DRP from saved
Avid bins or a Premiere project. The two sources feed the same graph/2
destination preparation and archive writer. The probe uses original files in
place. Resolve Studio 21.2.0 build 00006, project version 17, remains the native
qualification target.

## Avid to Resolve in the macOS wizard

For local qualification, set `ANYNLE_RESOLVE_TEMPLATES` to a JSON file
containing the pinned `templates` object from an Avid turnover job.
Launch the wizard and choose **Resolve** as the destination. Read the
selected Avid bins or project folder, select and order timelines, and
choose a new `.drp` destination.

**Link to originals** uses qualified OP-Atom siblings and UME originals.
**Rewrap managed media to OP1a** copies qualified 23.976 DNxHR LB picture
and its two independent PCM24 mono siblings into OP1a. UME and editable
Group camera media retain their verified links. Review can calculate
the OP1a size before a media folder is chosen. Optional proxy rewrap
uses its own folder and contributes to the estimate.

Review identifies UME originals that need the Resolve Studio importer.
Prepare the dedicated scratch project as described below. **Save
conversion settings** writes a job/2 file. Reopen it with
`any-nle-gui --job SETTINGS.json` or run it with `any-nle run --job
SETTINGS.json` on macOS. Approval of reported omissions applies to the
current run. Both media choices passed Review and conversion in a local
macOS arm64 package. **Calculate rewrap size** reported the combined
source byte count before a media folder was chosen and cleared its
result when the folder changed.

The current donor archives stay private. Blackmagic documents sharing
exported Resolve projects, and the installed license assigns user
content to its applicable owner. The current archives contain private
locators and saved previews, and generated projects inherit a donor
preview. Public rollout requires a new, self-authored template set with
reviewed contents, fresh pins, and a packaged native replay.
[Blackmagic reference manual](https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf)

## Automatic native media import fallback

The internal linked and turnover writers try standalone preparation first.
A supported project can be written with Resolve closed. Assisted linking
uses a running DaVinci Resolve Studio 21.2.0 build 00006 on macOS with
local external scripting available. Set `ANYNLE_RESOLVE_IMPORT_WORKER` to
the exact name of a disposable project beginning with
`Any NLE Import Worker `. Open that project at 24 fps with the root Media
Pool selected, empty, and free of timelines. The worker exports temporary
Media Pool records and removes the imported items. If an interruption
leaves media in the worker, empty the disposable project before retrying.

The internal `any-nle-gui --resolve-job JOB.json` entry point opens Review
for a Premiere → Resolve job/2. **Media linking** counts distinct
originals and lists affected names and paths. The launch action starts
the selected application only on click. A verified media blocker takes
precedence over Studio readiness. Review shows the affected original and
offers to recheck the source. Status checks read the running
edition, build, and scratch state. If the application bundle cannot be
found, **Locate Resolve Studio…** accepts an installed Resolve bundle.
An application with an unconfirmed edition is checked through the live
scripting connection. **Prepare project** rechecks the worker, verifies
the links, and makes the report available before **Convert** publishes.
Review and conversion check for an available `.drp` destination in an
existing directory before opening the source or calling the media importer.
Graph inspection can still run without a free destination. Publication
checks the destination again and writes it exclusively.
Review distinguishes an import in progress, a scratch render, and a
scratch project that still contains media, bins, or timelines. A stopped
import can leave its clip in the scratch project. Return that dedicated
project to an empty state before checking again. Preparation also checks
the worker when it claims it for an import, so a state change after Review
can still prevent publication. The converter leaves existing scratch
content in place.

The assisted profile covers 1080p straight cuts at exact `24000/1001`,
24, 25, and `30000/1001`. The original may be progressive square-pixel
MOV or MP4 at its verified raster, with picture only or embedded 48 kHz
stereo PCM16, PCM24, or AAC-LC assigned to two independent mono lanes.
Graph/2 owns the edit clocks, trims and selectors. Resolve supplies the
Media Pool cache. The converter verifies file-relative picture and audio
packet grids and checks the saved timecode, including its drop-frame flag.
PCM and AAC use their physical playable sample extents independently of
picture. At 29.97, the linked writer preserves frame-aligned audio
boundaries from a registered Resolve timeline when the imported original is
1080p ProRes HQ with stereo 48 kHz PCM24. This includes cuts that fall
between samples. Two native drop-frame timelines with nonzero trims and
independent split points matched all 33 and 36 picture frames and both
isolated PCM lanes after reopen. For Premiere sources with the same media
profile, fractional source trims and a fractional timeline end are
admitted when the saved timeline origin and every audio edit's record
start fall on whole samples. The native controls matched source sample
placement on both lanes across all five trim phases under that rule.
A record cut between samples shifted Premiere's incoming audio by one
sample relative to Resolve. An off-sample origin also changed the rendered
length by one sample. Those cases stay outside the Premiere profile.
Review identifies the timeline, audio lane, and offending origin or record
start, with its exact sample position. At this rate, whole-sample positions
occur at every fifth frame measured from timecode zero or the timeline
origin, respectively.
The verified playable extent still applies to each edit.
When Premiere saves a different source raster from the timeline canvas,
graph/2 retains the edit's input sizing policy. A native-size Premiere
edit uses Resolve's editable Center Crop control. A saved fit policy
uses the qualified Resolve scaling default.
The linked archive writer also uses the Resolve reader's verified
inherited sizing for this profile. Inherited Center Crop becomes an
editable clip setting on the affected edits. Inherited fit uses the
template's verified fit setting. The writer checks the staged sizing
and identifies the edit when its inherited setting is unresolved.
The writer reads back the staged archive before exclusive publication.
An explicitly supplied media digest has to match.

The internal benchmark uses `ANYNLE_RESOLVE_IMPORT_ORDER=api-first` to
request API-first and `ANYNLE_RESOLVE_IMPORT_TRACE=1` to count imports.
Standalone-first is the default.

The macOS Review trial launched Studio and published an assisted UHD AAC
project after the scratch worker became ready. Resolve Save Project As,
close, reopen, and re-export preserved the compared timeline facts for the
23.976 UHD AAC, 24 fps picture-only, 25 fps wide-raster PCM24, and 29.97 DF
PCM24 controls. Every decoded picture frame and both isolated mono PCM
lanes where present matched independently authored Resolve controls. The
absent, Free, and unsupported-build Review states were exercised with
simulated inputs.

## Premiere linked-media source capture

`resolve-turnover-probe --capture-premiere INPUT.prproj SEQUENCE_ID FFPROBE`
writes a selected Premiere 27 straight-cut timeline to graph/2 on standard
output. The probe checks the saved media references against the original
files and reports contradictory selectors, extents, or clocks before it
writes the graph. It records exact source trims and record clocks, source
raster separately from the timeline canvas, physical streams and channels,
and an embedded source timecode when it agrees with the Premiere save.
The capture inspects the original files and checks for changes without
hashing every media file.

The Premiere 27 Save As and reopen controls cover:

| Rate | Linked source evidence |
| --- | --- |
| 24 | Eleven trimmed native edits covering AVC MP4/MOV with 48 kHz AAC-LC or PCM, independent mono MOV streams, ProRes HQ MOV with or without PCM, and Main 10 HEVC MP4 with AAC-LC. |
| 25 and 30000/1001 | Fifteen sources per rate, including UHD AVC with AAC-LC or PCM16/24, 720p and 2048×858 AVC, ProRes HQ, Main 10 HEVC, and DNxHR LB OP1a MXF with PCM16/24. Both the native imports and the saved outputs of the Avid conversion were checked. |
| 24000/1001 | UHD AVC MP4 with PCM16, PCM24, or AAC-LC, and HD AVC MP4 with AAC-LC. The native UHD AAC saves cover both one stereo edit and two independent mono edits. |

The 29.97 ProRes control retains its `00:00:59;20` file source clock
independently of the sequence origin. When a full-length native audio edit
ends less than one sample beyond the file at the final picture frame, the
graph carries the playable PCM or AAC extent and records the exact saved
end in `premiere.saved_audio_tail`. A whole-sample overrun fails capture.
A retained 24 fps ProRes/48 kHz PCM WAV control also verifies the explicit
left and right selectors used by split-mono edits of a stereo WAV master.

Standalone capture also retains saved sequence In/Out in
`premiere.timeline_marks`, using the same reader as conversion jobs. An
explicit In at frame zero stays distinct from an unset In. Subframe
values keep exact ticks for destination preparation. Retained Premiere
27 saves cover 23.976, 24, 25 and 29.97 DF, including a marked and an
unmarked sequence in the same project. Duplicate and invalid mark fields
fail capture.
[Timeline In/Out](#premiere-timeline-inout)

Standalone capture also records saved per-edit label overrides and
unrecognized label fields in `premiere.clip_label`. Edits with valid
labels inherited from their matching source templates need no edit-label
record in this capture. The conversion job continues to record both
inherited labels and overrides. [Timeline clip labels](#premiere-timeline-clip-labels)

This command covers source media and edit timing. Clip processing and mix
controls still need their own destination preparation. Resolve publication
continues to use its existing qualified media and effect profiles.

## Inputs and review

The probe accepts an internal `anynle.job/2` document. Set `schema` to
`anynle.job/2` and put source settings under `source` with a `route` of
`avid_to_resolve` or `premiere_to_resolve`. The earlier flat probe input is
still accepted. The production CLI and wizard continue to use job/1.

The Avid job selects one or more `bins`, ordered `sequences`, and
`media_roots` for managed media. Select a unique saved name or an exact
graph sequence ID. `avid_project_root` can preserve nested Avid folders
from the selected files' paths. Each bin must be inside that root.
`mappings` optionally supplies saved and current path prefixes for UME.

A Premiere job supplies `premiere` and one `sequence` selector (a
one-element `sequences` list in the earlier flat probe input).
Use `resolve-turnover-probe --list-premiere-sequences INPUT.prproj`
to read sequence ObjectUIDs, names, rates, canvas, saved start ticks,
and track, clip and transition counts. This reads only the project.
Use a listed ObjectUID when several sequences share a name.

The prototype admits qualified HD and UHD straight cuts with a saved
whole-frame origin.
Picture-only ProRes HQ MOV and 48 kHz PCM16/24 WAV use 24 or 23.976 fps.
The [AVC profile](#premiere-avc-mov-and-mp4) uses `24000/1001`, exact
24 or 25 fps, or `30000/1001` with a saved drop-frame clock.
The [Main 10 HEVC profile](#premiere-hevc-main-10-mp4) uses exact 24 fps.
It follows the saved audio selector through its source reference, checks
cached timing against the file, and admits independent mono selections on
two tracks feeding one stereo Mix bus. The static mixer profile below
defines which mixer controls transfer. Source and record ranges must agree.

Premiere source bins retain panel ownership and stable IDs. Captions keep
their content, lane and exact record span in graph/2. The reader captures
sequence Comment markers with exact ticks and preserves bounded native
receipts by occurrence. Review reports unsupported markers, captions and
components by occurrence.

Both jobs supply `ffprobe`, `templates`, and `output`. The templates are
the existing hash-pinned layout and rate profile, plus the applicable Atom
and Group donors. Use `--review` to check preparation, template assembly
and the output path without staging an archive, or `--graph` to inspect
the source document. Reviewable omissions require
`--accept-losses` on the publication run. Timing, media, routing and topology
failures still block publication. Each input project and media binding is
checked again before exclusive publication.

### Premiere-authored linked ProRes and WAV

In an empty Premiere 27.0.0 build 88 project, import a picture-only
1080p24 ProRes HQ MOV starting at `02:00:00:00` and two distinct mono
48 kHz WAVs, one PCM16 and one PCM24. Create a 24 fps timeline with a
stereo Mix bus, start it at `01:00:00:07`, and place two edits per track
using these half-open frame ranges.

| Track | Record ranges | Source ranges |
| --- | --- | --- |
| V1 | `[0, 24)`, `[24, 48)` | `[6, 30)`, `[49, 73)` |
| A1 | `[0, 16)`, `[16, 48)` | `[9, 25)`, `[34, 66)` |
| A2 | `[0, 20)`, `[20, 48)` | `[12, 32)`, `[40, 68)` |

Premiere Save As and reopen retained all six ranges. The saved job
selected the timeline by ObjectUID. Review and conversion counted three
direct links, and replay from serialized graph/2 reproduced the saved
timeline. Resolve Studio 21.2.0 build 00006 kept both nonzero clocks, all
six ranges, all three online paths, and the separate mono selectors after
Save Project As, close, reopen, and re-export.

An independently assembled Resolve timeline matched all 48 decoded
picture frames and all 96,000 PCM24 sample frames per isolated lane.
The two lanes carried distinct signals. A Premiere control created from
the picture clip with an eight-channel adaptive Mix bus was refused.

### Packaged Premiere to Resolve replay

A locally signed macOS arm64 package ran three saved jobs through
the internal wizard with the bundled media probe. The jobs covered the
native Premiere ProRes/WAV split edit, repeated V1 dissolves with A1
Constant Gain fades, and the editable Constant Power approximation.
Review and preparation left each destination absent. For Constant
Power, Review named the +3 dB approximation and disabled Convert
until it was approved. An occupied destination was refused and its
bytes stayed intact.

Each wizard output matched the same-revision internal turnover CLI
and its retained Resolve reference. Resolve Studio 21.2.0 build 00006
saved, reopened, and re-exported all three wizard outputs with their
exact clocks, edits, original links, selectors, and transition windows.
For each project all 48 decoded picture frames and both 96,000-sample
PCM24 lane renders matched the corresponding Resolve controls. The
Power comparison uses the native Resolve +3 dB fade as its audio reference.

The trial used private, hash-pinned templates supplied by the saved jobs
and the separate internal CLI probe. The packaged public CLI continues
to refuse Resolve jobs. See the [product gate](ROADMAP.md#resolve-product-exposure)
for the wider native scope and template redistribution review.

### Premiere project selections

Use the internal `resolve-turnover-probe --premiere-project MANIFEST.json`
entry point to write one project for an ordered selection. The manifest
contains `premiere`, `sequences`, `templates`, `ffprobe`, and `output`.
`sequences` is an ordered array of exact saved ObjectUIDs from
`--list-premiere-sequences`. The existing `--review`, `--graph`, and
`--accept-losses` options apply. Multi-sequence export uses the 1080p24
project profile. Existing single-sequence jobs retain their selectors.

The source reader parses the project once, shares physical file checks,
and retains Premiere item identities and the selected timelines' bin
ancestry in graph/2. The writer registers timelines in selection order
and selects the track layout for each timeline. The three-mono profile
uses the existing `templates.fairlight` donor for V1/V2 and A1/A2/A3.

Two offline controls derived from a retained project use same-named
timelines, same-named nested bins, distinct starts, and shared ProRes
HQ and WAV originals. One control pairs a V1/A1/A2 timeline with a
V1/V2/A1/A2/A3 timeline. It verifies overlapping picture, separate audio
source trims and distinct Premiere master identities for the same file.
Staged read-back and unmodified graph/2 replay matched both timelines in
each control. Instrumented publication performed one media stream probe
per physical path. Review required approval for the known logging
omission. Duplicate or missing IDs, invalid media bindings, trims past
the source extent and occupied output paths refused publication.

Both generated projects passed save, close, reopen, and re-export in
Resolve Studio 21.2.0 build 00006. The re-exported archives retained the
ordered timelines, active selection, nested bin ownership, exact starts,
edits, source trims, paths, and physical channel selectors. Read-back also
confirmed that shared masters stayed shared and distinct masters for the
same file stayed distinct.

Native Resolve controls authored with the same originals and ranges
matched all 48 decoded picture frames per timeline and all nine isolated
mono renders, each 96,000 sample frames through the same stereo bus.
The V2 overlay changed picture only over its saved interval. A native
WAV edit saved just below an integer frame boundary now reads at its
whole-frame length. A true one-sample difference still refuses graph
publication.

The combined source fixtures were derived from a retained Premiere
project. A fully native Premiere source for this combined multi-timeline
case remains a separate check.

### Editable Premiere picture nests

The internal 1080p24 route follows a nested edit’s saved sequence
reference and checks it against the child’s project item. Selecting
a parent includes the referenced children. Graph/2 records each parent
occurrence, exact child ID and clock, full child extent, and child-local
trim in `premiere.nested_uses`. The linked writer uses the existing
Resolve nesting template, with one shared media item for repeated
uses of the same original.

The Premiere 27.0.0 build 88 control has two children with the same
name. They start at `02:00:00:03` and `03:00:00:13` and each runs
30 frames. The parent starts at `01:00:00:07`. Its first nest uses
child frames `[3, 13)` at record frame 3. The second uses child frames
`[7, 19)` at record frame 13. A direct edit follows at frame 25.

The saved job and the serialized graph replay both reopened in
Resolve Studio 21.2.0 build 00006 with both child names, their distinct
starts, and both trims intact. Each render matched the independently
authored Resolve control on all 31 frames, including three leading
black frames. Both editors selected the expected 28 original frames.
Advancing the first child’s source In by one frame changed only its
ten parent frames after reopen.

The profile covers one level of picture-only V1 nests. Premiere can
insert a stereo audio companion even when the child has no audio
edits. Review can omit a companion on neutral A1 when its exact child
reference, record range, and source trim agree with the picture.
Graph/2 retains the companion’s occurrence ID and the saved job and
graph replay both require approval for the omission. A native control
confirmed digital silence across all 56,000 stereo sample frames.
After reopen, both converted projects matched all 28 picture frames
of the independently authored Resolve control.

#### Audible nested audio

A native Resolve control nests a child with one mono WAV edit into a
stereo parent at the same start and trim as the saved Premiere 27
control. After save, close and reopen, the audio companion still refers
to the exact child, with child trim `[3, 13)` and record range `[0, 10)`.
The flat Resolve control matches all 28 decoded picture frames. The
native nest selects WAV samples `[10,000, 30,000)` on both output
channels and is silent for the remaining 36,000 sample frames.

Away from the edit edges, Resolve reproduced the WAV at −2.90 dB per
channel in the native nest and at unity in the flat control. Premiere's
reopened nest measured −6.02 dB per channel. The Resolve exports also
had a different response at the edit edge. A matching trim and routing
alone therefore leave an audible difference.

The Resolve reader retains the audio occurrence in `resolve.nested_uses`
even when the parent track layout has no destination mapping. The
record carries the native track ID, child identity, trim, and any bounded
saved selector. The selector describes the child mix. The destination
mix remains unresolved for the mono child.

A separate control changed only the parent audio clip's editable
volume. Resolve reopened the exact values −3.123252, −3.16, −3.24,
and +1.16 dB. The renders applied −3.1, −3.1, −3.2, and +1.1 dB
respectively. These controls show tenths of a dB toward zero for this
profile. Graph/2 retains the exact saved value as
`saved_clip_volume_db` on the audio nest occurrence.

The −3.1 dB adjustment left the interior 0.023 dB above Premiere.
The neutral repeat matched the original Resolve export exactly.
All four adjusted renders kept all 28 picture frames and the same
audio trim and stereo routing. The approximately 72-sample native
ramps still differed from Premiere at the two edit edges with both
Inspector fades at zero.

Two 1080p24 Resolve controls used independent stereo signals from
48 kHz PCM16 and PCM24 WAV files. The child placed WAV frames
`[2, 32)` and the parent used child frames `[3, 13)`. After save,
close and reopen, the parent selected WAV samples `[10,000, 30,000)`
in left/right order. For each bit depth, all 28 picture frames and
all 56,000 stereo PCM frames matched a flat Resolve control byte
for byte. Both Resolve paths had the same short edit-edge ramps.

Graph/2 captures the child's standalone stereo WAV edit as grouped
audio on A1. The parent receipt records `saved_child_mix_channels`
as `[1, 2]` when the native stereo selector matches. This describes
the child mix outputs. The child's WAV placement separately carries
the physical file channels.

The internal linked probe can now write a selected stereo WAV child
as its own editable 1080p24 timeline. It uses a pinned native
24 fps V1 and stereo A1 layout. The profile needs a verified standalone
48 kHz PCM16 or PCM24 stereo WAV, a neutral mixer, and contiguous
picture and audio spans of equal duration. The graph supplies the
picture and audio trims independently.

The reopened outputs retained the child start at `02:00:00:03`,
picture frames `[6, 36)`, and WAV frames `[2, 32)`. Both bit-depth
controls matched on all 30 picture frames and 60,000 stereo sample
frames. The source channel order and native edge ramps matched the
independently authored control and the original child.

The linked probe also round-trips paired native Resolve stereo audio
nests on a neutral 1080p24 parent with one shared child. Graph/2 retains
the reciprocal picture/audio occurrence IDs, exact child reference, shared trim and
ordered stereo mix outputs. The writer uses the pinned stereo layout
and paired-nest templates and rebuilds both reciprocal links on the
generated native IDs. Staged read-back checks each link and its owner.

For standalone PCM16 and PCM24 WAV children, after save, close and
reopen all 28 picture frames and 56,000 stereo sample frames matched
the native parent controls. Advancing the child WAV trim by two frames
shifted both live parent audio channels by 4,000 samples without
changing the picture or the parent's trim.

The audible profile covers one registered Resolve child, matched
V1/A1 pairs and ordinary picture cuts elsewhere on the parent. Broken
links, unqualified routing or mixer state, and mismatched trims refuse
publication. Mono or processed child mixes and deeper chains require
separate qualification.

The internal Premiere → Resolve route also writes paired nests
when Premiere saves an enabled Standard stereo A1 companion beside
each matching V1 edit and the shared child has one standalone stereo WAV on
Standard A1. Both mixers must be neutral. The reader verifies the
actual saved track panners and the audio clip's ordered child refs.
A stereo clip layout alone is insufficient. A control using mono
tracks retained stereo clip labels but mixed the file channels together.
That layout is refused.

Premiere 27.0.0 build 88 saved and reopened both bit depths. The
generated saved-job and graph/2 replay outputs reopened in Resolve
Studio 21.2.0 build 00006 and matched the corresponding native Resolve
controls on all 28 picture frames and all 56,000 stereo sample frames.
Both editors selected the same original frames and samples. Outside
Resolve's 72-sample edge ramps, the PCM16 source matched exactly.
The PCM24 source differed by at most one PCM16 step in the Premiere
reference export.

Review reports the edge approximation on each parent audio occurrence.
Both the saved-job and serialized-graph publication paths require
explicit `--accept-losses` for it. The output keeps the original WAV,
editable child, linked parent picture/audio pairs, and exact edit bounds.

A repeated-use PCM24 fixture starts the parent at `01:00:00:07` and
the child at `02:00:00:03`. Its parent selects child frames `[3, 13)`
and `[15, 27)` on adjacent V1/A1 pairs. The Premiere saved job,
Premiere graph replay and native Resolve graph replay each matched
the reopened native Resolve control on all 28 decoded picture frames
and 56,000 stereo sample frames. The two Premiere routes report
separate edge approximations. The native route needs no edge approval.

After reopen, advancing only the second trim by one frame moved both
of its audio channels by 2,000 samples. The first pair retained its
native links, trim, picture and samples. Resolve added rendered-media
metadata to that first pair during the edit. Re-export of that changed
archive through the converter remains unqualified.

Both occurrences in the unchanged pair carry the same 660-byte
`ImportExportMetadataBA` value. It contains two nonzero rendered-audio
source Mob IDs, a source count of two, a 16-byte zeroed `RenderedExtents`
value, and a nested MXF metadata record with tape, source, and master Mob
IDs. The corresponding occurrences in the retained pre-edit archive have
no payload. The edit and PCM comparison establishes that playback stayed
correct in Resolve. To qualify replay, isolate a metadata-only save and a
rendered-media replacement, then check the child references and source
paths alongside the reopened picture and PCM in each case.

### Premiere AVC MOV and MP4

Current Premiere projects can link progressive, square-pixel 1920×1080
H.264 High level 4.0 MOV and MP4 at 23.976, 24, 25 or 29.97 DF. The tested layout
is V1 with two independent mono edits selecting the left and right
channels of the file's 48 kHz audio track. AAC-LC is qualified at all
four rates. Embedded PCM16 and PCM24 are qualified at
[25 fps](#embedded-pcm16-and-pcm24-at-25-fps) and
[29.97 DF](#embedded-pcm16-and-pcm24-at-2997-df). The turnover reader uses the
same saved master, media cache and channel-group checks as
`--capture-premiere`. Graph/2 retains the physical stream and channel
selectors. The existing Resolve writer checks the original's packet
clocks and extent, then links the file in place. A separate
[stereo A1 profile](#native-stereo-a1-at-25-fps) keeps embedded PCM on one
editable track.

Two native Premiere 27.0.0 build 88 controls used the existing MOV and
MP4 sources at `01:00:00:00` and trimmed each to file frames 6–41.
After Premiere Save As and reopen, both split-mono projects generated
DRPs. After Resolve Studio 21.2.0 build 00006 save, close and reopen,
both retained the paths, trims and selectors. Both matched all 36
decoded picture frames and all 72,000 stereo PCM24 sample frames of
isolated A1 against direct-import Resolve controls. Picture-only
projects from both sources also reopened and matched all 36 frames.

Isolated A2 matched outside samples 36,192–36,447 (zero-based). The two
containers hold byte-identical AAC packets. Reimporting the same MP4
natively changed those same 256 sample frames to the candidate waveform.
The controls establish channel routing, sample placement and playable
extent. Exact AAC PCM repeatability inside that window remains unqualified.
The native AVC imports also saved the default Resolve radiometry cache
when the originals identified the Rec.709 matrix and limited range but
left transfer and primaries unset. The writer uses that same record.

#### Exact 24000/1001 fps

The internal route links 1080p H.264 High level 4.0 and UHD H.264
High level 5.1 with 48 kHz stereo AAC-LC. UHD uses the matching
hash-pinned project and timeline with the established UHD geometry and
radiometry fields. Premiere capture and graph/2 retain the unchanged
locator, exact clock and both physical selectors. The shared verification
checks the original picture and AAC packet grids, including priming and
the last playable audio packet.

Three saved and reopened Premiere 27.0.0 build 88 MP4 controls each start
at `01:00:00:00` and run for 36 frames. The HD control uses source
frames `[6, 42)`. The first UHD control uses picture `[6, 42)` and audio
`[7, 43)`. The second uses picture `[11, 47)` and audio `[12, 48)`,
ending exactly at source sample 96,096. Resolve Studio 21.2.0 build 00006
saved, reopened and re-exported each conversion with the same clocks,
raster, trims and independent A1/A2 assignments. All 36 decoded picture
frames per timeline and both isolated 72,072-sample PCM24 renders
matched corresponding native Resolve controls. All three staged
timelines also matched after replay from serialized graph/2. The
reopened UHD tail replay matched both native renders again. An audio trim
past the tail, duplicate physical selectors, 44.1 kHz AAC, the wrong UHD
profile, and an occupied output path refused publication.

#### Exact 25 fps

Two Premiere 27 Save As and reopen controls use 1080p25 AVC MOV and MP4
with V1 and split-mono A1/A2, a non-drop start of `01:00:00:07` and file
frames 6–41. The source and reader retain an exact 25-frame display clock.
At this rate, a 48 kHz frame is exactly 1,920 samples. The AAC cache
holds 92,160 playable samples per channel for the 48-frame original.
The writer uses the hash-pinned native Resolve 25 fps project and timeline
skeleton supplied as `templates.profile`, with the existing linked media
donor supplied as `templates.layout`.

Both generated projects saved and reopened in Resolve Studio 21.2.0
build 00006 with their original paths, clocks, trims and channel assignments.
All 36 frames per container and the 69,120 sample frames of each of the
two MOV lanes and MP4 A1 matched the direct-import Resolve controls.
MP4 A2 differed in samples 36,672–36,927 (zero-based). A fresh native MP4
import reproduced the candidate's A2 output sample for sample. Review
rejects a contradictory Premiere display clock or an incompatible profile
template before publication.

#### 29.97 drop-frame

The Premiere 27 MOV and MP4 controls save a base-30 drop-frame display
clock at exact `30000/1001`. The linked DRP uses the same-build native 29.97
DF profile supplied as `templates.profile`. The timeline stores the
drop-frame flag separately from the media rate. Save, close and reopen in
Resolve Studio 21.2.0 build 00006 retained the start at `00:00:59;20` and
the minute rollover to `00:01:00;02` ten picture frames later.

The 48-frame original has 76,876 playable AAC samples per channel.
The writer verifies the packet extent, stores the playable sample count,
and preserves the picture extent independently. The tested split-mono
edits use file frames 5–44 and sample interval `[8008, 72072)`. Each
container matched all 40 decoded picture frames and both isolated
64,064-sample-frame PCM24 renders against its direct-import Resolve control.
This profile requires whole 48 kHz sample boundaries for each audio edit.
The non-drop display format, off-sample trims and mismatched template
refuse before publication.

#### UHD AVC at 25 and 29.97 DF

The internal Premiere route links progressive, square-pixel 3840×2160
H.264 High level 5.1 MP4 with 48 kHz AAC-LC when the saved timeline uses
the same raster and exact rate. The checked files declare limited range
and a Rec.709 matrix, with primaries and transfer unset. The writer uses
the same observed UHD media geometry and radiometry fields at these rates
and at [23.976](#exact-240001001-fps).
The project and timeline come from the matching hash-pinned UHD native
profile. Read-back checks the file raster and timeline canvas separately.

Both Premiere 27 controls saved and reopened with picture and two
independent mono edits using source frames 5–44. Resolve Studio 21.2.0
build 00006 saved and reopened the generated projects at 3840×2160.
The 25 fps timeline starts at `01:00:00:07`. The 29.97 DF timeline starts
at `00:00:59;20` and skips to `00:01:00;02` ten frames later. All 40
decoded frames per rate and both isolated PCM24 lanes matched the native
Resolve controls exactly. The exports contain 76,800 sample frames per
lane at 25 fps and 64,064 at 29.97 DF.

An HD timeline referencing a UHD original and an HD template for a UHD
job both refuse before output. Other UHD clocks, editable UHD picture
effects and mixed-raster scaling need separate qualification.

#### Embedded PCM16 and PCM24 at 25 fps

The internal AVC route links 48 kHz stereo PCM16 and PCM24 directly
from matching 25 fps MOV or MP4 originals. It validates the codec tag,
bit depth, channel layout, packet grid and payload sizes. Premiere
graph/2 keeps the physical file stream and both channel selectors. The
Resolve reader verifies the saved track, bit depth and channel routing
in the embedded PCM cache.

Premiere 27 saved and reopened four controls at `01:00:00:07`, with
independent left and right mono edits and a source range of frames 5–44.
They use HD MOV and UHD MP4 at both bit depths. The PCM24 MOV test file
combines the retained HD picture and PCM24 signal for this test.
Resolve Studio 21.2.0 build 00006 saved and reopened all four generated
projects. Each retained the exact trim, paths, bit depth and channel
assignments. All 160 decoded picture frames and all eight isolated
76,800-sample-frame PCM24 exports matched their native Resolve controls.

#### Embedded PCM16 and PCM24 at 29.97 DF

The four same-rate Premiere 27 controls use HD MOV and UHD MP4 with
embedded 48 kHz stereo PCM16 or PCM24. The HD PCM24 MOV combines retained
HD picture with a retained PCM24 signal. Each 48-frame source has 76,876
complete audio samples per channel. The writer validates the physical
PCM packets and keeps the audio extent separate from picture duration.
Audio edit boundaries must fall on whole 48 kHz samples.

After Resolve Studio 21.2.0 build 00006 saved and reopened the generated
projects, the originals stayed online. The timelines retained their
`00:00:59;20` start, `00:01:00;02` rollover, file frames 5–44 and exact
sample interval `[8008, 72072)` on both independently selected mono lanes.
All 160 decoded picture frames and eight isolated 64,064-sample-frame
PCM24 lane renders matched their native Resolve controls exactly.
An off-sample trim, a saved cache one sample too long, and an occupied
output all refused before publication.

#### Native stereo A1 at 25 fps

The internal route can link 1920×1080 25 fps H.264 MOV with one embedded
48 kHz PCM16 or PCM24 stereo stream. It keeps a neutral stereo edit on A1
using graph/2 `grouped_audio` with the ordered left/right selectors. The
writer takes the native stereo layout from the hash-pinned
`templates.profile` export and validates both channels on staged read-back.
The Resolve reader recognizes the saved stereo clip selector and Media Pool
default for the same profile. The timeline comparison checks each channel.

Two Premiere 27 save-and-reopen controls start at `01:00:00:07` and use
file frames 5–44. The HD PCM24 MOV is a synthetic control made from retained
picture and audio. Both generated projects saved and reopened in Resolve
Studio 21.2.0 build 00006 with V1 and one editable stereo A1 track. Fresh
insertion from the Media Pool also used one stereo track in left/right order.
All 80 decoded picture frames and both channels of the two
76,800-sample-frame PCM24 renders matched their direct-import Resolve
controls exactly. Contradictory Premiere channel layouts, a wrong native
template and occupied outputs refused publication. Replaying saved
graph/2 through the linked probe matched the reopened timelines, and
reversing the saved left/right selectors refused publication.

#### Separate stereo WAV at 25 fps

The same stereo track and mixer also carry a separate 48 kHz PCM16 or
PCM24 WAV on a neutral 1080p25 timeline. The Premiere reader validates the
WAV's saved channel group and records both physical selectors in one
graph/2 `grouped_audio` edit. The WAV's own locator, playable sample extent
and source trim remain independent of picture. The Resolve writer shares
the standalone WAV cache and the same stereo timeline selector.

Premiere 27.0.0 build 88 saved and reopened two timelines at
`01:00:00:07`. Both use picture frames `[5, 45)` and WAV frames `[7, 47)`,
which select samples `[13440, 90240)`. The staged and reopened DRP graphs
match the native Resolve controls, including when the separate picture and
audio sources are replayed from serialized graph/2. The native WAV
in-point carries a binary64 residue beside the integer frame. The reader
accepts that representation and refuses a real one-sample offset.

After save, close and reopen in Resolve Studio 21.2.0 build 00006, the WAV
stays online in left/right order on one editable A1 track. Fresh insertion
uses the same mapping. All 80 decoded picture frames and both channels of
each 76,800-sample-frame PCM24 render match the native Resolve controls.
The interior samples match the trimmed source exactly. The native and
generated renders share the same differences within the first and last
72 sample frames. Swapped selectors, a trim past the WAV tail, a missing
file, a mismatched media digest and an occupied output path refuse.

[pr2resolve's audio format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#L27-L29)
and [drpkit's WAV cache writer](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/drpkit/compose.py#L202-L250)
cross-check the cache structure. The saved stereo edit and mixer match
come from the same-build native controls.

#### Stereo A1 with mono A2 at 25 fps

The internal single-timeline route also supports separate 48 kHz stereo
and mono PCM WAV originals on A1 and A2, with neutral mixer controls and
contiguous, equal record coverage on V1, A1 and A2. Graph/2 keeps the
ordered stereo group on A1 and a physical mono selector on A2. The
writer uses a distinct hash-pinned 25 fps layout and mixer donor through
`templates.profile`. Staged read-back checks both lanes against the
prepared edits.

A Premiere 27.0.0 build 88 source saved and reopened at `01:00:00:07`
with picture frames `[5, 45)`, stereo PCM24 frames `[7, 47)` and mono
PCM24 frames `[11, 51)`. The saved job and serialized graph replay
kept those edits and original file paths. In Resolve Studio 21.2.0
build 00006, both outputs saved, reopened and re-exported with exact
trims and channel order. Fresh insertion from the Media Pool kept the
stereo and mono routes.

All 40 decoded picture frames and both channels of each 76,800-sample
render matched the native Resolve control for the full mix, isolated A1
and isolated A2. Outside the edge ramps, stereo matched the two original
channels and the mono source reached both bus channels at Resolve's
measured −3 dB center pan. The archive comparison checks the edits and
physical selectors. The reader separately recognizes the two observed
neutral mixer models from the donor and reopened exports. Unknown mixer
state remains a comparison gap. Wrong selectors, an A2 trim past the
WAV tail, the stereo-only donor and occupied output paths refused
publication.

Other stereo clocks, stereo MP4 edits, mix changes and combinations with
mono timeline tracks need separate qualification. Dual-mono movie
streams, HEVC and MXF also need destination checks. Public route exposure
remains under the [product gate](ROADMAP.md#resolve-product-exposure).

### Premiere HEVC Main 10 MP4

The internal route links progressive, square-pixel 1080p24 Main 10
level 4 MP4 tagged `hvc1`, with limited range, a Rec.709 matrix and
unspecified primaries and transfer. The tested original carries one
48 kHz AAC-LC stereo stream. The writer uses Resolve's native Main 10
media cache and shares the physical packet, trim and channel checks
with AVC. Graph/2 retains the original path, exact clocks and physical
channel selectors.

A Premiere 27.0.0 build 88 control saved and reopened with V1 and two
independent mono edits on A1 and A2. All three edits use source frames
`[6, 42)` at a record start of `01:00:00:00`. The saved job and serialized
graph replay both saved, reopened and re-exported in Resolve Studio
21.2.0 build 00006. Their codec, geometry, radiometry and saved cache
flag match the native Resolve import. All 36 decoded picture frames and
both isolated 72,000-sample PCM24 renders match the native control
exactly. The saved Media Pool assignment also retains both mono channels.

A trim past the original, duplicate physical selectors, HDR signaling,
the wrong rate template and an occupied output path refuse publication.
Other HEVC rasters, rates, encodings and color profiles remain outside
this qualification.

### Camera picture with independent editorial audio

An internal Avid job can prefer camera originals from verified Avid source
columns. With a verified 23.976 stereo proxy rewrap, Resolve links the camera
picture and attaches the rewrapped MOV as its proxy. The edited mono audio
uses the verified Avid MXF siblings. Review reports **Creates new media.**,
the planned proxy paths, and the estimated bytes when it plans a rewrap.
Picture-only proxy attachment uses the existing MXF in place.

A derived 23.976 bin combines two camera picture cuts, both managed PCM24
siblings, and a linked stereo WAV on the second A2 edit. Archive read-back
retains the exact `01:00:00:00` timeline and `02:00:00:00` camera starts and
all six edit ranges. A1 uses its own MXF for both edits. A2 uses its own MXF
for the first edit and the WAV's right channel for the second. The proxy
contains all 96 verified compressed picture frames. Each audio channel matches
its original mono MXF sample for sample. An occupied proxy path blocks review.

Resolve Studio 21.2.0.6 imported, saved, closed, and reopened the combined
project with the originals online. A fresh DRP export kept all six file
identities, trims, clocks, audio channel selectors, and the proxy attachment.
Resolve matched all 48 decoded picture frames and 96,096 PCM24 samples per
mono lane against a timeline assembled directly in Resolve using the same
source files and ranges. The audio also matched the selected source samples
within one PCM24 step after accounting for the stereo bus pan and the
72-sample clip-edge ramps.

### Require camera originals

In the macOS Avid to Resolve wizard, open **Original locations and proxies**.
Enable **Prefer camera originals for picture**, then **Require camera originals**.
This saves `source.require_camera_originals` in job/2 and requires a single
verified original for every selected picture edit and every camera in a
selected editable Group. The option defaults to off.
With it enabled, preparation also looks for originals using the saved Avid
source columns and any supplied path mappings. Verified existing UME
picture links qualify. Managed editorial audio, linked WAV audio, and
explicit proxy attachment keep their respective media policies.

If picture would fall back to managed media or a saved picture edit could
not produce a placement, preparation reports the affected timelines,
tracks, and record positions or camera names. The
report counts all failed bindings and shows up to 16. It fails before
writing a DRP or proxy. The internal probe has exercised the strict gate
on the mixed editorial audio control and a picture-only camera original.
Choose **Open saved-path mappings** from the blocker to return to the
original locations with the timeline order and media choices intact.
Add the saved folder prefix and its current location, then run Review again.

## Premiere sequence markers

On qualified Premiere → Resolve timelines, a Comment marker on the
sequence retains its name, comment, and record-relative time in graph/2.
Resolve receives a whole-frame marker at the same position. A point
uses Resolve's one-frame marker duration. Whole-frame ranges retain
their saved length. Green, Red, Blue, Cyan and Yellow map by name.
The installed Premiere 27 exposes eight marker color indices and Resolve
accepts sixteen named colors. When Premiere's color has no counterpart,
Resolve uses Lavender for Magenta, Sand for Orange, and Cream for White.
Review reports the substitution and graph/2 keeps the Premiere color.
A marker whose position or duration cannot be represented, whose start
is shared with another marker, or whose type or payload is unsupported
is reported by occurrence and requires loss approval.
Saved values remain in graph/2 or the bounded source receipt. Richer
Premiere marker types remain outside this writer profile.

A Premiere 27.0.0 build 88 Save As and reopen supplied a 24 fps
sequence beginning at `01:00:00:01`. Resolve Studio 21.2.0 build 00006
imported, saved, closed and reopened the generated DRP with all five
sequence markers, including frame zero, a cut boundary and two
four-frame ranges. Names, multiline accented comments, Green, Red,
Blue and Yellow survived. The six linked edit ranges and both WAV
channel selectors were unchanged. The same Premiere project's linked
AAF contained all five marker positions and comments, with no embedded
essence. Its marker entries lacked names, colors and range lengths.
Resolve imported the AAF at the same timeline origin and reopened it
with zero timeline markers. The initial import used a preloaded Media Pool
and left the originals offline. The same unmodified AAF reopened online
with automatic source import enabled and still had zero timeline markers.
The AAF import also used the left channel from each stereo WAV on both
audio lanes. The native DRP retained all five markers and kept the
distinct left and right selectors. The reopened 48-frame renders matched
in picture, and the isolated PCM confirmed the AAF's duplicated left
channel across the 96,000-sample-frame span. See the
[linked AAF benchmark](#premiere-linked-aaf-benchmark).

### Premiere linked AAF benchmark

Open and save the 1080p24 combined-transition control in Premiere 27,
then close and reopen it. Export AAF with video mixdown, Breakout to Mono,
and rendered audio effects disabled. Import that unchanged AAF into a
disposable Resolve project with automatic source import enabled. Import
the matching Any NLE DRP into a second project. Save, close and reopen
both. Compare the saved V1 and audio ranges and render each populated audio
lane separately with the same Resolve settings.

On Premiere 27.0.0 build 88 and Resolve Studio 21.2.0 build 00006, the
AAF export log reported three ignored Cross Dissolve properties. The AAF
still contained the eight-frame Video Dissolve and Mono Audio Dissolve,
and Resolve reopened both as editable transitions. The AAF and DRP kept
the same one-hour origin, all five linked edits, both transition windows,
and the same source files and trims. The AAF placed its populated audio
tracks on A2 and A4. The DRP retained Premiere's A1 and A2. After adjusting
for the track numbers when isolating lanes, all 48 decoded picture frames
and both 96,000-sample-frame PCM24 renders matched exactly.

The AAF linked in place with automatic source import. Preloading the same
three files and disabling automatic source import left the AAF events
unbound. A separate locator-only diagnostic copy also linked automatically
and produced identical saved edit facts. The render comparison uses the
unmodified AAF.

## Premiere source clip markers

Both Premiere source readers retain Comment markers on linked source items
in graph/2, with names, comments, palette colors, and exact item-relative
times. The `premiere.clip_markers` receipt records the native object ID and
payload digest. Unsupported payloads retain a bounded receipt and produce
a loss for the affected occurrence.

A Premiere 27.0.0 build 88 Save As and reopen preserved a point before
both trims and a ranged marker on a picture item used twice. Graph/2
kept both markers once on the source item, with the ranged marker's exact
saved subframe length.

The linked Resolve writer maps frame-aligned source markers for verified
picture-only 1920×1080 ProRes HQ MOV at 24 or `24000/1001` fps. Both
profiles accept nonzero embedded source timecode that
agrees with the saved Premiere media origin. The writer uses the verified
picture source rate and assigns the Media Pool marker collection to the
linked item's UUID. Both trimmed edits share the same item. A Premiere
point is a one-frame Resolve marker. Red, Cyan, Blue, Green, and Yellow use
the same palette names. Magenta, Orange, and White use Lavender, Sand, and
Cream with approval in Review. Subframe positions or durations, conflicting
starts, and unsupported colors produce occurrence-specific omissions.
[Source marker palettes](RESOLVE-SOURCE-READER.md#source-marker-palettes)

The frame-aligned variant placed a point at source frame 2 and a
seven-frame range at source frame 10. Resolve Studio 21.2.0 build 00006
saved, closed, reopened, and re-exported both markers on the shared Media
Pool item, with the accents and multiline comment intact. The native
control also distinguished an independent timeline item marker. Editing
the generated marker's custom data survived a separate save and reopen.
The generated and native Resolve renders matched all 48 decoded picture
frames and both PCM24 channels over 96,000 sample frames. The
`24000/1001` round trip also preserved distinct custom values on the
shared source and matched both isolated mono lanes.
[Exact 23.976 source marker clock](RESOLVE-SOURCE-READER.md#exact-23976-source-marker-clock)
[Embedded source timecode](RESOLVE-SOURCE-READER.md#embedded-source-timecode)

The same source marker route supports standalone mono 48 kHz PCM16 and
PCM24 WAV at 24 and `24000/1001` audio import rates. The
`24000/1001` stereo controls preserve one marker collection on the
shared source while the left and right lanes keep their separate
trims and selectors. Sample-level Premiere positions remain exact in
graph/2. Resolve Review reports
positions or durations outside its frame grid by marker occurrence.
[WAV source marker qualification](RESOLVE-SOURCE-READER.md#standalone-wav-source-markers)

## Premiere timeline In/Out

The Premiere 27 reader retains saved `MZ.InPoint` and `MZ.OutPoint` ticks in a
`premiere.timeline_marks` sidecar on the sequence. Whole-frame values also
carry their saved frame number. Subframe values retain their exact ticks, and
the unset sentinel produces no mark. Timeline In/Out is separate from work
area, clip trims and timecode origin.

The writer maps frame-aligned In-only, Out-only and paired selections to
matching timeline-relative audio and video marks. The absent side of a
one-sided selection stays unset. Premiere stores an exclusive Out
boundary. Resolve includes its saved Out frame, so it gets the Premiere
Out minus one frame. Subframe values and ranges outside the edit remain
in graph/2 and are reported for loss approval. Contradictory tick and
frame receipts refuse publication. [Pinned pr2resolve mark encoding](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#L24)

Two Premiere 27.0.0 build 88 saves start at `01:00:00:01`. In frame 2
with saved Out 13 exported 11 frames. Advancing Out to 14 exported 12.
The generated projects reopened in Resolve Studio 21.2.0 build 00006
with Out 12 and 13 respectively. Both applications exported the same
positions within their own full render. Each generated Resolve render
matched its native Resolve control frame for frame and sample for
sample. Three more controls reopened and matched native Resolve at
23.976, 25 and 29.97 DF, including an Out at the end of the edit and
a range spanning the dropped timecode numbers. The 25 fps range also
matched after serialized graph/2 replay and native reopen.

#### One-sided and frame-zero marks

The 24 fps controls use a Premiere 27 save with a 48-frame timeline
starting at `01:00:00:01`. Derived source projects change the saved mark
fields. In-only at frame 2 selected 46 frames through the end. Out-only
at exclusive frame 13 selected 13 frames from the start. Out-only at
exclusive frame 1 selected just the first frame. The mark values survived
Resolve 21.2 save, close and reopen, with both audio selectors intact.

In-only, paired and single-frame selections at zero survived import on
the Edit page and reopen. Import on the Deliver page dropped the explicit
In at zero in this build. A one-field archive comparison confirmed that
changing the saved playhead did not fix Deliver-page import. Use the
Edit page for import when retaining the explicit zero mark matters.

Serialized graph/2 replay matched the saved-job picture and stereo PCM
for In-only, Out-only and paired frame-zero In. For paired single-frame
selections, the render API defaulted to the full timeline when passed
only `SelectAllFrames=false`. Explicit equal render bounds returned the
correct decoded frame and 2,000 exact stereo PCM sample frames on both
tests. The paired-mark and unmarked regressions matched their controls.

## Premiere timeline track state

The Premiere 27 reader records custom track names, edit locks and the
V1 output toggle in `premiere.track_state`. Each record is attached
to the selected sequence and identifies a lane by kind and number.
On the internal V1/A1/A2 linked profile, the Resolve writer uses the
same track preparation as Resolve round trips. It writes the timeline
fields and the Fairlight mirrors for audio names and edit locks.
Disabling V1 leaves all picture edits and both mono routes in the project.

Sync lock is distinct from edit lock. A disabled Premiere sync lock
remains in `premiere.track_sync_lock` and requires approval to omit.
Invalid or duplicate track flags block output. Audio track mute is
handled through the saved fader control described below.

Offline variants of a retained Premiere 27 save kept accented text,
duplicate audio track names, edit locks and a disabled V1. The archive
read-back retained the original sequence origin, six edit ranges,
physical WAV selectors and five sequence markers.

Both variants were imported as separate Resolve Studio 21.2.0 build
00006 projects, saved, closed, reopened, and re-exported. Resolve
kept both identically named mono tracks and their separate edit locks.
The archive comparison found only the V1 output toggle. V1 off rendered
all 48 frames black with both picture edits still in place. Each isolated
audio lane had 96,000 identical PCM24 sample frames with picture on
and off. The saved selectors still identify the two distinct channels.

### Premiere static audio mixer

For V1 and two independent mono WAV lanes, the turnover keeps static
Audio Track Mixer volume and mute on each track. The Premiere Mix fader
becomes Resolve's main stereo bus fader. Track and bus gain use tenths
of a decibel from −6 through 0 dB. Mute is independent of track gain,
clip volume, and the original channel selector. Mixer automation,
contradictory saved values, and gains outside this profile block output.

The reader checks the Mix track and its panner type and channel labels.
The saved channel count of two can describe a stereo Mix or an adaptive
Mix. A Premiere 27 save with two adaptive outputs refuses at the Mix
boundary. A derived stereo project with only the Mix type changed to
adaptive also refuses. The editable multicam backing sequence has its own
adaptive Mix profile and retains its separately verified output matrix.

The 23.976 control used a retained Premiere 27 save with its clip-edge
transitions removed from a disposable copy. Resolve Studio 21.2.0 build
00006 reopened it with A1 muted, track levels of −3 and −6 dB, and a
−3.7 dB main bus. The saved mix matched isolated A2 exactly across all
96,096 PCM24 sample frames. Re-enabling A1 reproduced the unmuted
control, and each lane matched its expected gain within one PCM24 code
value relative to the unity control.

The internal route also preserves editable static track pan from Premiere's
mono-to-stereo panner on A1 and A2. The saved hard-right default and explicit
values map to Resolve's signed tenths of a percent. A retained Premiere 27
save reopened in Resolve with hard left and right on the correct lanes, A1
muted, and the saved clip, track, and Mix levels intact. The centered,
swapped, and ±50% controls varied only the saved track pan on disposable
copies of that project. Across 96,096 sample frames per lane, swapping hard
pan swapped the output channels exactly. The interior PCM matched the trimmed
source and saved gain within one PCM24 code value. Resolve applied its
previously measured 72-sample ramps at the edit edges.

At ±50%, the quieter channel measured about 27% of the louder channel in
Resolve and 41% in the retained Premiere control. Review reports the use of
Resolve's pan law for intermediate settings. Animated pan, other mixer
routing, and linked pan combined with custom track state remain outside
this profile.

### Premiere clip-edge Constant Gain

On the internal two-mono WAV route, a Constant Gain transition
with one saved endpoint becomes an editable Resolve fade on that clip.
Graph/2 keeps the original transition ID and length on the owning edit.
The reader checks the lane list, the clip's head or tail reference, the
alignment, and the exact record interval. The visible trims stay the same.
The profile covers 24 and 23.976 fps, including both edges of one clip and
static clip volume. Other fade curves and a crossfade on the same lane
require separate mappings. Clip pan and unqualified clip processing on a
faded edit block the conversion.

The Premiere 27.0.0 build 88 control reopened in Resolve Studio 21.2.0
with a six-frame tail on A1 and an eight-frame head on A2 at 23.976 fps.
Both 96,096-sample PCM24 lanes match a fade-free copy sample for sample
outside their fade intervals. The interiors follow the linear-amplitude
curve within one PCM24 code value. A separate source variant put a
12-frame head and an 18-frame tail on A1 with static clip, track and Mix
gain, track pan, and A2 mute. Both fades and the mixer controls survived
reopen, with all 48 picture frames matching the unfaded control.

For fractional fades, graph/2 keeps the original Premiere tick lengths.
Resolve stores them as fractional frame counts. The internal writer requires
the conversion to recover the exact source tick and admit at least one audio
sample per fade. On a disposable copy of a Premiere 27 reopened save with
clip panners removed, the 23.976 head and tail lengths of 64,714,012,088 and
86,625,055,551 ticks survived Resolve reopen and conversion back to saved
Premiere XML. The independent WAV channels and static clip levels survived.
Across 96,096 PCM24 sample frames per lane, the audio outside each fade
matched a fade-free copy exactly. The interior linear-curve residual was
at most 10 code values for the head and one for the tail. Fractional fades
with custom curve points or volume keys remain unqualified.

## Premiere timeline clip labels

For Premiere 27 projects, graph/2 records the timeline edit's saved label
slot and packed color in `premiere.clip_label`. When both fields agree with
the source templates, the edit uses the destination default. Resolve receives
an explicit clip color for a distinct edit label, even when two edits share
one master. The route uses the saved Premiere slot to choose a standard
Resolve color and reports substitutions or a nonstandard saved swatch by
edit. Unrecognized label fields require approval to omit. Duplicate label
fields block preparation. Media Pool labels are outside this mapping.

The offline control changes six timeline label pairs on a copy of the
retained Premiere 27 project. Five write as edit-owned Resolve colors and
the unknown slot remains in graph/2 with an omission report. Resolve Studio
21.2 build 00006 kept all five labels and one uncolored neighbor after import,
save, close and reopen. Re-export retained both mono channel selectors, all six
edit ranges and the nonzero timeline start. The Premiere label changes were
made to a disposable project copy; a separate Premiere 27 authoring check
for changed labels remains open.

## Premiere Cross Dissolve

The first 1080p23.976 turnover mapped one eight-frame V1 Cross Dissolve
(Legacy) with left or centered alignment to an editable Resolve Cross
Dissolve. The reader requires the outgoing clip's saved tail reference
and the incoming clip's saved head reference to agree with the transition
and cut. Preparation verifies the source handles and preserves the
visible trims. The turnover reports the blend sampling difference by
transition occurrence.

The two retained Premiere 27 saves cut at record frame 24. After saving,
closing, reopening, and re-exporting in Resolve Studio 21.2.0 build
00006, both retained the cut, original links, audio selectors, and
blend intervals of frames 16–23 and 20–27. All 48 decoded frames in
each project matched its corresponding native Resolve control. Each
isolated audio lane matched the Resolve straight-cut control across
all 96,096 PCM24 sample frames. Premiere starts each blend with the
outgoing image, while the Resolve blend is already visible on that frame.

## Premiere Constant Gain crossfade

The first 1080p24 turnover mapped one centered eight-frame Constant
Gain transition on A1 to an editable Resolve Cross Fade 0DB. It verifies
both saved clip references, the fade settings, and four frames of linked
WAV source on each side of the cut. The visible trims and separate mono
assignments survive. Other fade types and transition endpoints with clip
processing require separate qualification.

The retained Premiere 27 save came from the earlier Resolve-to-Premiere
crossfade trial. After save, close, reopen, and re-export in Resolve
Studio 21.2.0 build 00006, the transition kept its frame-24 cut and
the eight-frame interval starting at frame 20. Each isolated lane
matched the corresponding native Resolve PCM24 render across all
96,000 sample frames. A2 also matched the straight-cut control.

### Combined picture and audio transitions

A 1080p24 timeline can carry qualified V1 Cross Dissolves and A1
Constant Gain crossfades. Each transition keeps its own cut, clip
references and handles. The initial combined control used one on
each lane.

An offline Premiere fixture combined a centered picture cut at frame
20 and a centered audio cut at frame 24. After saving and reopening
in Resolve Studio 21.2.0 build 00006, both eight-frame transitions
survived with all five visible edits. All 48 decoded pictures matched
the picture-only transition control. Both isolated PCM24 renders
matched the audio-only transition control across 96,000 sample frames
per lane. The linked Resolve writer also reproduces the combined
project from the saved graph of either editor.

### Repeated transitions on one lane

Premiere 27 saved and reopened a 1080p24 control with two centered
Cross Dissolves on V1 and two centered Constant Gain crossfades on A1.
Each transition is eight frames. The middle edit on each lane owns a
head and a tail transition. Source capture binds each transition by its
unique saved references and exact cut before adjusting graph/2 ranges
for source handles. The writer restores the visible trims and verifies
each saved interval and endpoint after staging. Duplicate references,
ambiguous endpoints, overlapping windows and insufficient handles
refuse publication.

Resolve Studio 21.2.0 build 00006 saved and reopened the direct DRP,
the serialized graph/2 replay and the untouched linked Premiere AAF.
The three outputs matched all seven edit ranges, original paths,
source trims and four blend windows. All 48 decoded picture frames
and both isolated 96,000-sample PCM24 renders matched exactly. The
DRP retained A1/A2 while the AAF placed the same audio on A2/A4.
Premiere's export reported ignored Border Width, Border Color and
Anti-aliasing Quality for each dissolve. Review still reports the
known blend sampling difference from native Premiere by occurrence.
Archive comparison reports a later transition by its own lane and
cut when more than one candidate shares that lane.

### Variable lengths and Constant Power

Premiere source capture retains the exact whole-frame interval, cut and
endpoint identities in graph/2. The Resolve writer keeps visible trims
and checks the required handles for the saved interval. The linked path
accepts left-aligned V1 Cross Dissolves and even-length centered V1
Cross Dissolves or A1 crossfades within its existing media and rate
profiles. A centered transition with an odd duration remains outside
this profile. Native video saves at 2, 6, 8, 12 and 304 frames establish
the little-endian duration field in the saved effect bytes.

The 1080p24 control saved a six-frame centered and a twelve-frame
left-aligned dissolve, an eight-frame centered Constant Power crossfade,
and a ten-frame centered Constant Gain crossfade. Both internal output
paths kept all seven edits and four windows after save, close, reopen
and re-export in Resolve Studio 21.2.0 build 00006. All 48 decoded
pictures matched both the separately authored Resolve dissolve control
and the linked Premiere AAF import. Both isolated PCM24 lanes matched the
native Resolve control across all 96,000 sample frames per lane.

Resolve imported the AAF's Constant Power as a linear 0 dB fade. The
internal writer maps a qualified plain mono Constant Power to the
editable +3 dB preset. After accounting for the dry level difference,
the relative RMS error against Premiere over the test fade was 21.1%
for the AAF import and 13.7% for the +3 dB mapping. Review reports the
approximation by occurrence and publication requires `--accept-losses`.
Constant Gain remains a 0 dB fade. The fade on A1 leaves the other lane
sample-identical to the AAF import. Custom fade controls and an exact
Constant Power envelope still need separate qualification.

### Premiere editable multicam

The internal linked turnover captures one 1080p23.976 Premiere multicam
source with two continuous DNxHR LB OP1a camera files and two independent
48 kHz PCM24 mono streams per file. It keeps the saved camera offsets
and visible trims in graph/2. The hidden backing sequence must have a
neutral Mix bus, camera A's hard-panned mono transport, and muted camera B
audio. Each outer audio edit identifies its logical channel group and its
physical output independently of the picture's saved SelectedTrackIndex.
Resolve reuses the native two-camera Group and links both camera originals
in place. Other track layouts, creative processing, and an audio angle
switch need separate mappings.

Review follows the multicam backing sequence to its two camera originals.
It checks the saved file and mono references, physical OP1a streams,
and camera duration. It counts each verified camera as a direct link and
identifies its angle and locator in media errors. Prepare and Convert
still use the existing digest checks.

Premiere 27 build 88 saved, closed and reopened a three-cut picture
control with independent fixed mono lanes and a qualified 01:00:00:00
record start. Resolve Studio 21.2.0 build 00006 reopened the direct
conversion and the graph/2 replay with nine matching ranges and trims.
All 145 decoded picture frames agree between the two Resolve paths.
Comparison with the Premiere render identified the same camera at every
frame. The synthetic color patches have a small cross-editor level
difference. Resolve exposed the native multicam switch commands for
the reopened clip.

A second graph selected camera A in place of camera B on the middle
picture edit. After reopen, precisely frames 48–95 changed. All 290,290
sample frames per isolated lane remained identical across the original
and switched Resolve projects. Against the camera-A sources, each lane
kept sample placement and matched within two PCM24 codes after accounting
for Resolve's stereo-bus gain and its 72-sample start and end ramps. Wrong
saved audio selectors, a swapped internal routing matrix, and an
independent audio angle change refused publication.

The pinned pr2resolve notes informed earlier audio channel work. Its
native export guard rejects nested sequences. The Group mapping here
uses the saved Premiere multicam object graph and the retained Resolve
native Group template.
[pr2resolve format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#L27-L29)
· [Native export guard](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/pr2_native.py#L415-L416)

### Mixed-rate native comparison

A separate Premiere 27 control cuts from 23.976 to 24 fps ProRes HQ
on a `24000/1001` timeline, with separate PCM16 mono WAVs trimmed
by 4,004 and 6,006 samples. Premiere Save As and reopen retained
both physical links, a nonzero timeline origin and the two clocks.
On the 24 fps edit, Premiere's saved source In corresponds to 23.023
source frames. Flooring that origin plus the record-relative frame
index times 1001/1000 predicts all 1,059 source selections.

Default Resolve conform begins and ends on the same file frames
when the visible extent is matched. Premiere skips source frame
1,000 at record frame 993, and Resolve skips source frame 1,021 at
record frame 1,014. The 21 intervening picture selections differ.
Both source pulses landed at the same PCM sample positions in the
two 2,152,150-sample renders. Waveform checks near the start and
tail agree within one 16-bit code after fitting each editor's
center mix coefficient.

For plain V1 Frame Sampling from 24 fps ProRes HQ on a 23.976 timeline,
the internal job now captures the saved source In and all selected
picture frames in graph/2. Resolve writes its compact mixed-rate map
with a source In one thousandth of a record frame before the exact
Premiere In. The graph keeps the exact Premiere time. The reader checks
the map and alignment and reconstructs the same frame selections.
Verified media stay linked in place and WAV trims stay on their own
48 kHz clock.

After save, close and reopen in Resolve Studio 21.2.0 build 00006, the
generated project matched all 1,075 numbered Premiere picture frames.
Its mix PCM matched the native Resolve control exactly. The isolated
A1 and A2 renders each retained 2,152,150 sample frames, their distinct
4,004- and 6,006-sample source trims, and their known pulse positions.
Three longer Resolve experiments reproduced all 3,136 expected frame
selections per test with different nonzero trims. This rate pairing
requires a known positive source In and plain picture sampling.

#### 23.976 picture on a 24 fps timeline

Premiere 27.0.0 build 88 saved and reopened controls with a same-rate
lead-in and a picture-only 23.976 ProRes HQ MOV on V1. Both 1080p24
sequences start at non-drop `01:00:00:00`. The mixed-rate edit is 1,059
record frames. Its source In is either zero or one second, which is
exactly `24000/1001` source frames. Premiere Frame Sampling selects source frame
`floor((InOnRecordGrid + recordOffset) × 1000/1001)`. On the trimmed
control, Premiere repeats source frame 1,000 at record frame 994.
Resolve's default conform selected different source frames at 29
positions on that control.

The same graph/2 frame map carries the exact source In and selected
frames for both rate pairings. Resolve writes the compact mixed-rate
map with its source In 0.002 of a record frame earlier for the
23.976-to-24 case. The writer uses a zero whole part and a negative
fraction for the zero-trim case. The reader checks the rate, alignment
and compact map and reconstructs the exact graph trim and frame
selections. [Pinned compact timemap reference](https://github.com/samuelgursky/davinci-resolve-mcp/blob/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format/media-timemap.js)

The offline conform probe checks explicit picture maps against the saved
file clock and marks contradictory timing receipts as inconclusive.

After Save Project As, close and reopen in Resolve Studio 21.2.0 build
00006, both generated timelines matched all 1,075 numbered Premiere
frames. A serialized graph replay reproduced the trimmed timeline.
Re-exported archives preserved the start, edit ranges, linked originals
and both mono selections. The 4,000- and 6,000-sample WAV trims stayed
independent. The mix and both isolated lanes matched the native Resolve
control exactly across 2,150,000 PCM24 sample frames each. Four longer
Resolve controls also matched every expected frame selection, including
a trim that starts at an exact repeat boundary.

The qualified profile uses picture-only, square-pixel 1080p ProRes HQ
MOV at its original clock and plain V1 Frame Sampling. Zero source In
for 24 fps picture on a 23.976 timeline, frame blending, optical flow,
and the linked AAF comparison remain open.
[Resolve → Premiere verification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-mixed-rate-frame-sampling)

## Premiere clip components

For Premiere 27 project version 46, `--graph` includes a
`premiere.component_parameters` sidecar on each edit with supported, saved
clip controls. It carries Motion Position, Scale, Scale Width, Uniform
Scale, Rotation and four crop edges, plus the Level parameter of the Internal Volume Mono
clip component.
Parameter IDs are scoped to the Motion owner. Audio Level is identified by
its parameter slot and carrier within the mono clip-volume owner.

The sidecar keeps the saved component and parameter object IDs, component
order, value units, CurrentValue, StartKeyframe, and the stored keyframe
list. Each key retains the exact signed tick, value, interpolation fields
2 and 3, and the remaining fields in saved order. Missing flags and
values stay absent from the saved data, represented by null. No default
component is synthesized. Audio Level stays in Premiere's saved numeric
domain, distinct from clip gain, track volume and display dB. Position
stays in Premiere's normalized coordinates.

Capture follows the observed 14-field point and eight-field scalar
records. Version 46 synthetic turnover checks independently compare the
receipts with the input XML, including subframe ticks, distinct start and
current values, and curves with an omitted IsTimeVarying flag. The
underlying record shapes are pinned to PRPROJ-0030 and PRPROJ-0013.
The source reader puts qualified static Motion in the edit's graph/2
`transform` and static mono Level in `volume_db`. Resolve preparation
reads those fields from the saved graph and verified media bindings.
The position and scale profile requires a matched 1920×1080 square-pixel clip, neutral
anchor, rotation, anti-flicker and crop, and scale between 87.5% and
112.5%. Static mono Level maps to −6 through +3 dB in exact tenths with
neutral clip mute. Resolve writes it on an independent PCM WAV lane.
The original Premiere component values remain in the sidecar.

Qualified two-key Linear Motion travels as `picture.motion_linear` in
graph/2. The reader converts the saved ticks to exact item-source seconds
and retains Linear interpolation. This profile requires uniform scale, a
neutral first key, and both Position and Scale keys on the first and last
visible source frame. Resolve preparation checks the keys on the verified
source frame grid. Other appearance remains a per-edit loss and requires
`--accept-losses`.

### Premiere Linear rotation with independent width

On 1080p24, three aligned Linear keys on Position X, Scale Width, and
Rotation map to editable Pan, Zoom X, and Rotation Angle. Scale Height,
Position Y, pivot, crop, and anti-flicker remain neutral. The first and
last keys must match the visible source bounds. Graph/2 retains both the
original Premiere keys and the interpreted curves. The Resolve writer
uses the interpreted source-frame times, with Premiere's clockwise
rotation converted to Resolve's opposite sign.

The retained Premiere 27.0.0 build 88 save supplied the Motion values.
A disposable copy added an explicit one-hour timeline origin for the
turnover reader. After saving and reopening the exported archive in
Resolve Studio 21.2.0.0006, all nine keys survived. All 13 decoded frames
matched the earlier Avid-to-Resolve render of the same edit.

### Premiere centered static crop, scale, and rotation

At 1080p24, the turnover reads isolated centered rotation and the
centered crop profile with independent scale and clockwise rotation.
The crop edges stay in source space. Graph/2 carries the saved
Premiere percentages in the component receipt and the corresponding
frame fractions on the edit. The crop limit accounts for the rounding
observed in Premiere 27's saved percentages.

A disposable copy of the retained Premiere 27 save added an explicit
one-hour timeline origin. After archive reopen in Resolve Studio
21.2.0.0006, the four edits retained editable crop, zoom, and rotation.
All four decoded frames matched the earlier Avid-to-Resolve control
exactly. All 28 measured chart points agreed with Avid within 0.09 px.
Animated crop, displaced pivots, and other effect stacks require
separate qualification in this direction.

### Static Motion and volume trial

An offline static-only variant of the retained Premiere 27 save matched
the baseline turnover and the replay from saved graph/2 and media
bindings. All six edits matched on archive read-back and had identical
effect payloads. The replay also matched after removing the native
component receipts.

The Linear Motion replay kept the retained Premiere save's two picture
controls and static volume. A disposable copy cleared the unrelated
animated volume and logging omissions for the strict linked probe. Its
saved graph produced matching timeline read-back and effect bytes across
all six edits. The Resolve archive held Linear keys on all four Motion
axes at source frames 5 and 28.

### Native Premiere 27 and Resolve mapping trial

A Premiere 27.0.0 build 88 Save As and reopen preserved six online edits at
24 fps and 1920×1080. A separate Resolve Studio 21.2.0 build 00006
candidate was built from the captured values, then saved, closed, reopened,
re-exported and rendered. The trial covers the following cases
(CONV-0069).

| Control | Result after Resolve reopen |
|---|---|
| Static Motion | Position `[0.525, 0.475]` and independent 112.5% width and 87.5% height became Pan 48 px, Tilt 27 px and the same independent zooms. |
| Two-key Linear Motion | Position and uniform zoom retained their two keys at source frames 5 and 28 on a trimmed edit. Resolve omitted redundant scalar bases for the neutral starting values. The reader recognizes that saved encoding and requires a neutral initial key. |
| Static mono clip volume | Level mapped to exactly −6 dB using the Premiere Level conversion. Each editor's rendered change was within 0.001 dB of −6 dB relative to its own neutral control. |
| Animated mono clip volume | The −6 dB and +3 dB keys and their times survived. The rendered ramp differed by up to 0.28 dB at the measured points, so matching endpoint values and timing is insufficient to qualify it. |

The picture comparison covered all 48 decoded frames. Mean absolute
grayscale difference was at most 0.41 on a 0–255 scale after downsampling
to 480×270. This is a scoped comparison for the matched canvas, same
source rate and tested controls.

The turnover job subsequently applied the three qualified controls
directly to the saved Premiere 27 project. In Resolve 21.2.0 build 00006,
Save Project As, close and reopen preserved all six online edits, their
source trims and the 01:00:00:01 origin. All 48 decoded frames matched the
earlier Resolve control exactly. The stereo PCM24 render matched the
native −6 dB control for the first edit and the neutral control for the
second, sample for sample across 96,000 frames. Review reported the
animated volume ramp and the existing clip metadata omissions.

### Premiere Linear clip-volume curve

Premiere 27.0.0 build 88 retained the same two Linear keys at item-source
frames 5 and 28 after Save As and reopen. In separate saves, the levels
were changed to test rising, falling, entirely negative, entirely
positive, and wider ranges. The tests sampled 65 parameter values per
range and rendered the trimmed mono clip in isolation. The retained
Resolve PCM follows a dB-linear ramp. Premiere's measured parameter
curve differs from that interpolation by up to 0.31 dB for −6 to +3 dB
and 4.66 dB for −24 to +9 dB.

Away from unity, the sampled Premiere curve fits interpolation in a
coordinate that uses `exp(k * dB)` below 0 dB and
`2 - exp(-k * dB)` above it, with measured `k ≈ 0.05154588`. Inverting
that coordinate predicts the sampled parameter values within 0.00002 dB
outside the unity region. The isolated PCM also shows a unity plateau
near ±0.03 dB. The fitted constant and the plateau are measured evidence
for these controls.

For two ordered whole-frame Linear keys inside a 24 fps mono edit, the
reader now records item-source times, calibrated dB endpoints and
Premiere's native interpolation identity in the occurrence-owned
`premiere.clip_volume` sidecar. Raw levels, ticks and interpolation flags
remain in `premiere.component_parameters`. Review reports the curve
difference for the affected edit. Subframe and other interpolation
receipts continue to retain their raw values.

#### Editable Resolve ramp trial

Resolve Studio 21.2.0 build 00006 retained all 100 Linear keys across the
six ramp controls after Save Project As, close, reopen and re-export. All
controls used the same 24 fps trim and the same physical right channel of
the stereo PCM24 WAV on a mono lane. In the Inspector, a new −4 dB key on a
duplicate survived a second reopen at item-source frame 17. The original
endpoints at frames 5 and 28 and the other test curves were unchanged.

Each isolated render was measured against its editor's own dry reference.
Premiere held unity for 344 samples on the −6 to +3 dB rise and 341 on the
+3 to −6 dB fall at 48 kHz, about 7.1 ms in each direction.

| Resolve keys | Peak error, rise | Peak error, fall | Rendered unity region |
|---|---|---|---|
| Two original endpoints | 0.309 dB | 0.310 dB | No plateau in either direction |
| One key per frame from the measured curve | 0.044 dB | 0.045 dB | One measured unity sample on the rise and none on the fall |
| One key per frame with the two surrounding keys set to unity | 0.318 dB | 0.318 dB | About 21 ms, early on the rise and late on the fall |

The tested frame-based mappings fail to reproduce the unity plateau. The
converter retains the source curve in graph/2 and reports the affected
edit in Review. Resolve clip-volume writing for this Premiere curve
remains pending.

#### Subframe clip-volume keys

The [Resolve 21 New Features Guide](https://documents.blackmagicdesign.com/SupportNotes/DaVinci_Resolve_21_New_Features_Guide.pdf)
documents subframe audio keyframing on the Edit page. In Resolve Studio
21.2.0 build 00006, two keys authored with Option-click at high zoom
survived Save Project As and reopen at item-source frames 19.417334 and
19.799095. In the saved clip-volume parameter 95, a Linear key with a
nonzero fractional time sets flag `0x02000000` and appends a little-endian
`f64` fraction of a frame after the dB value. The frame-aligned keys kept
their original encoding.

The rising and falling candidates each used 187 keys, with keys near the
plateau placed on the 48 kHz sample grid. Their saved key data survived
Save Project As, close, reopen and re-export byte for byte. The right
channel stayed assigned to the trimmed mono edit. Peak render errors
against Premiere were 0.030 dB for the rise and 0.034 dB for the fall.
The rise had no measured unity sample and the fall had one, compared with
the 344- and 341-sample unity spans in Premiere.

Separate steep-change controls placed 347 samples at unity in the edited
curve. Both renders reached unity only at sample 19,072. Around that point,
rendered gain fit linear interpolation in amplitude between points 1,000
samples apart, within 0.000024 dB. This playback result is scoped to the
tested 24 fps, 48 kHz clip path. The editable subframe data did not reproduce
the Premiere plateau, so the graph/2 source receipt and Review loss remain
the conversion behavior for this case.

## Premiere frame holds

Current Premiere projects using Source Timecode Frame Hold can capture the
underlying edit trim and the held picture through graph/2. The internal
linked Resolve writer recreates the hold at source In. When the held picture
differs from Premiere's source In, the turnover job reports the change in
Resolve's exposed source In and uses the existing `--accept-losses` approval
before publishing. The captured graph keeps both values. [Frame-hold profile and native verification](RESOLVE-SOURCE-READER.md#linked-picture-frame-holds)

## Odd-length Premiere 50% picture speed

The internal Premiere → Resolve route reads a saved half-frame Out at 50%
when Premiere uses Frame Sampling and the record and source spans agree
exactly. The linked 1080p24 ProRes HQ MOV writer requires every sampled
source picture to exist, including the final picture shown once. Saved jobs
and graph/2 retain the fractional source span. [Native qualification](RESOLVE-SOURCE-READER.md#odd-length-half-speed-picture)

## Premiere 200% forward picture speed

The internal linked route carries a same-rate 24 fps V1 ProRes HQ
MOV edit with Premiere Frame Sampling as graph/2 `Speed` with ratio
`[1, 2]`. The placement keeps the exact file-relative source In. The
source duration covers twice the record duration. Serialized graph/2
retains the original Premiere trim ticks in `premiere.picture_speed`.
A Premiere Out immediately after the last sampled picture and an Out
including the final skipped frame produced the same numbered cadence.
Both require the full nominal source span to fit the verified original.

Resolve stores In on the retimed clock. Whole source starts that are
odd require a saved half-frame remainder. The two-point native map ends
at source frame `available - 2` and retains `available - 1` as the
`LastValidYOffset` boundary. Native Resolve controls on 96- and 95-frame
media established the saved map fields and the reader's exact decoding.
[Pinned pr2resolve speed reference](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#L10-L12)

Premiere 27.0.0 build 88 saved and reopened three seeded project variants
with their 200% controls and rendered the source-frame controls. Resolve
Studio 21.2.0 build 00006 saved, closed and reopened the output from the
turnover job and from serialized graph/2. The two 24-frame cuts start at
file frames 12 and 13. A 12-frame cut starts at 72 and reaches the
verified file boundary. All 60 Premiere frame selections agree with
both Resolve outputs. The two 24-frame cases also match the corresponding
native Resolve controls pixel for pixel. On a separate duplicate, the
200% speed could be changed to 100%, and both values survived reopen.
An abbreviated span that would
exclude a sampled frame, Optical Flow, a contradictory fractional
Resolve In, and an occupied output were refused. The retained 50% and
reverse turnovers preserved their saved timeline facts.

The same linked picture profile is qualified in the reverse conversion
direction. Ramps and retimed audio remain separate.
[Resolve to Premiere qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-200-forward-picture-speed)

## Premiere piecewise Linear picture retiming

Premiere saves the clip's Time Remapping keys as pairs of retimed ticks
and file-relative source seconds. The saved clip In is on the retimed
axis. The map provides the actual source In. The selected source
frames travel in the existing graph/2 `frame_map`. An edit-owned
`picture.retime_linear` record keeps the whole-frame curve keys and
the retimed In, so the writer can preserve editable speed changes.
The original Premiere key records and trims remain on the edit in
`premiere.time_remapping`. The turnover job and standalone Premiere
source probe share that capture.

The first profile covers picture-only V1 at 1920×1080 and exact 24 fps
with linked, zero-origin ProRes HQ MOV and Frame Sampling. It requires
increasing whole-frame keys on both axes, no curve handles, and
coverage of the full edit. When Premiere ends the map at the exclusive
file boundary, a unity-speed final segment lets the writer move the
terminal key back one frame on both axes for Resolve. Off-grid source
In, reverse segments, holds, effects on the retimed edit, and retimed
audio are outside this profile. The writer verifies each key, the
trim, and the sampled frames in the staged archive.
[Pinned pr2resolve time-map reference](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/pr2_native_time.py)

The numbered picture controls use a nonzero timeline start and a
nonzero retimed In. The speed changes include 100% → 200% → 100%
and fractional-speed segments at 50%, 60%, 150%, and 160%.
The source controls were saved, closed, reopened, and rendered in
Premiere 27.0.0 build 88. Resolve Studio 21.2.0 build 00006 retained
the curves, trims, and links after save and reopen. Retime Controls
exposed both speed points and all three segments on the 100% → 200% →
100% edit.

All four timelines also passed Save Project As, close, and reopen in a
single Resolve project with one shared original. All 192 source-frame
selections agreed with Premiere. Both standalone writer paths and the
combined Save As project matched the separately encoded Resolve
comparison renders on every decoded frame. Bezier handles, nonmonotone
keys, off-grid keys, and a graph curve that contradicted its sampled
frames all failed before publication. The retained half-speed,
double-speed, and reverse controls passed their regression checks.

## Capture in graph/2

Resolve archive reading now retains the following scoped facts for future
destination mappings.

| Graph location | Saved facts | Destination status |
|---|---|---|
| `resolve.transitions`, sequence | Occurrence, track identity, saved start and duration, and separate alignment and position codes | The internal linked DRP path reads and writes [qualified variable-length V1 dissolves and A1 crossfades](#variable-lengths-and-constant-power). The standalone eight-frame profile and the narrower Resolve-to-Premiere profile remain separately qualified. |
| `resolve.timing`, sequence | Timing evidence, compact extents, and recognized time-map points, explicit zero flags and saved handles | Cadence and audio treatment pending |
| `resolve.fairlight`, sequence | Track and bus columns identified by strip ID, pan, bypass, solo, gain, output assignment, EQ/dynamics fields and bounded plug-in records | The [drpkit closeout](RESOLVE-DRPKIT-CLOSEOUT.md) records static mono pan, direct routing, and Bus 2 gain and mute in the linked turnover profile. |
| `resolve.clip_fairlight`, placement | Keyed Voice Isolation and Dialogue Leveler records with saved identity, flags and normalized parameters | Source capture only |
| `resolve.clip_fades`, placement | Clip-edge lengths and custom fade points in percent and tenths of a dB | Custom curve interpolation and writing pending |
| `resolve.source`, sequence | Native video and audio mark fields | Writing pending |
| `resolve.clip_label`, placement | Saved clip color code from the item lockable blob | Destination palette mapping pending |

The retained dissolve saves AlignmentType 3 and Position 2 on the same
occurrence. The reader preserves both. A same-build archive with a retimed
clip supplied a keyed Sm2TimeMap with decoded points. Playback behavior is
tested independently of these saved values.

## Qualification boundary

The Premiere source control combines retained native Premiere picture
records with synthetic edits. The generated project opened, saved under a
new name and reopened in Resolve with its 01:00:00:01 origin, six online
placements, exact trims and separate left and right selectors. A second
Resolve timeline rebuilt the edits with native insertion and native channel
mapping. It matched all 48 decoded picture frames and both isolated
96,000-sample PCM lanes exactly. Both versions exhibited the same short
edge envelopes and stereo mix coefficient.

The [automatic combined Avid turnover](RESOLVE-PROJECT-CORE.md#automatic-combined-avid-turnover)
reopened with nested bins, shared split-MXF media, H.264 UME picture,
and an editable Group. Switching only Group picture preserved both
isolated PCM lanes byte for byte. The combined control with explicit
media bindings also passed File menu Save Project As. The roadmap tracks the
[remaining Avid editor checks](ROADMAP.md#avid--resolve), the
[fully native Premiere source](ROADMAP.md#premiere--resolve), and the
[packaged route gates](ROADMAP.md#resolve-product-exposure).
