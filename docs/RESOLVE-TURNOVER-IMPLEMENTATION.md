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
