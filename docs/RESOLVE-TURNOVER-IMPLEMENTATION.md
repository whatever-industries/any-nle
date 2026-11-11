# Internal Resolve turnovers

`resolve-turnover-probe JOB.json` prepares and publishes a linked DRP from saved
Avid bins or a Premiere project. The two sources feed the same graph/2
destination preparation and archive writer. The probe uses original files in
place. Resolve Studio 21.2.0 build 00006, project version 17, remains the native
qualification target.

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

The prototype admits straight cuts on 1080p24 or 1080p23.976, a saved
whole-frame origin, picture-only ProRes HQ MOV and 48 kHz PCM16/24 WAV.
It follows the saved audio selector through its source reference, checks
cached timing against the file, and admits independent mono selections on
two tracks with a neutral stereo mix. Source and record ranges must agree.
Premiere source bins retain panel ownership and stable IDs. Captions keep
their content, lane and exact record span in graph/2. Native sequence marker
payloads retain their owning sequence and occurrence in the source receipt.
Review reports caption, marker and component omissions by occurrence.

Both jobs supply `ffprobe`, `templates`, and `output`. The templates are
the existing hash-pinned layout and rate profile, plus the applicable Atom
and Group donors. Use `--review` to check preparation, template assembly
and the output path without staging an archive, or `--graph` to inspect
the source document. Reviewable omissions require
`--accept-losses` on the publication run. Timing, media, routing and topology
failures still block publication. Each input project and media binding is
checked again before exclusive publication.

## Premiere clip components

For Premiere 27 project version 46, `--graph` includes a
`premiere.component_parameters` sidecar on each edit with supported, saved
clip controls. It carries Motion Position, Scale, Scale Width and Uniform
Scale and the Level parameter of the Internal Volume Mono clip component.
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
The Resolve writer still reports omitted appearance, and publishing with
those losses requires `--accept-losses`.

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
source rate and tested controls. Automatic application of Premiere
component receipts remains pending in the turnover writer.

## Capture in graph/2

Resolve archive reading now retains the following scoped facts for future
destination mappings.

| Graph location | Saved facts | Destination status |
|---|---|---|
| `resolve.transitions`, sequence | Occurrence, track identity, saved start and duration, and separate alignment and position codes | Endpoint and handle mapping pending |
| `resolve.timing`, sequence | Timing evidence, compact extents, and recognized time-map points, explicit zero flags and saved handles | Cadence and audio treatment pending |
| `resolve.fairlight`, sequence | Track columns identified by strip ID, pan, bypass, solo, pre-gain, output assignment and raw EQ/dynamics controls | Baseline fader and mute mappings remain scoped. Additional controls are source facts |
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

The automatic Avid probe reopened a nested, two-timeline split-MXF and
editable Group project, and separately reopened an H.264 UME project. A
picture-only Group angle switch changed the picture while both isolated
PCM lanes remained byte-identical to the Save As copy. A retained combined
turnover also passed Save As and reopen. Product rollout still needs an
automatically produced same-rate mixed UME and managed-media cohort, a native
Premiere-authored source cohort, a distributable template review, and the
packaged CLI, saved job and wizard gate. The current probe is internal.
See the [Resolve project qualification](RESOLVE-PROJECT-CORE.md) and
[roadmap](ROADMAP.md#davinci-resolve-user-2026-10-26-bookmark-and-research).
