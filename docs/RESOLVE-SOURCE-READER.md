# Internal Resolve source reader

`resolve-source-probe INPUT.drp OUTPUT.graph.json` reads an exported project
offline into the existing `anynle.graph/2`. It writes a new graph exclusively
and requires no editor connection or media access. The product CLI, wizard
and jobs have no Resolve source route.

The initial profile targets Resolve 21.2.0.0006 project version 17. It reads
registered timelines and their bin provenance, nested bin relationships,
linked picture and independent mono audio routes, exact canvas and clocks,
and same-rate straight cuts. Native registration IDs identify graph objects
within the input archive. Resolve may regenerate them on re-export. Graph
clocks identify Resolve observations. They carry no Avid compatibility
fallback. The cached media path and saved virtual selector remain separate
from physical stream verification. Offline status stays unknown.

The reader also retains standalone PCM16/PCM24 WAV cache samples, channel
layouts and saved import frame bases. Mono timeline selections of a stereo
WAV become separate logical channels while the pool's stereo group remains
a source fact. The ProRes HQ cache without saved timecode has no invented
timecode-track witness. Each registered timeline receipt includes its
owning `bin_id`. Child-folder registration is cross-checked against the
archive hierarchy. Graph/2 lists parents before their children and preserves
each parent's saved sibling order by UUID. The parent's `resolve.bin_order`
receipt records whether the order came from `fields_blob` or the
`archive_path` fallback. When the numbered paths give a complete order,
`archive_order_matches` shows whether they agree with the saved list.
The saved list controls sibling order when both sources agree on membership
but differ on order. An absent or empty folder blob can retain path order
with an unknown saved order. Malformed or contradictory child lists still
fail. Mixed-rate or nonidentity timing outside the admitted
profile retains occurrence data on a `resolve.timing` owner receipt and
reports the omitted placement.

The direct DNxHR LB MXF profile reads one saved picture cache and two
independent PCM24 mono caches at exact 24000/1001. Saved route references
identify the two cache objects regardless of serialization order. Both routes
use track zero in their respective files. The reader retains all three
locators and saved package identities, and checks that the cache clocks,
sample extents and shared master package agree. These are archive observations.
Physical MXF verification still belongs to media preparation. The generated
and reopened nine-edit split timeline round trips with both mono assignments.

The fixed-reference two-angle Group profile preserves inner spans and gaps,
outer angle cuts, and independent mono audio selectors. A leaf placement
is emitted only where its selected source covers the use. The full Group
use retains the original range and any gap or unresolved binding.

Sequence markers preserve relative position, duration, title, note, all
16 Resolve marker colors, and saved custom data. The writer verifies
these values and the sequence owner before publication. Clip-owned marker
clocks remain open. [Premiere marker palette](#resolve-to-premiere-sequence-marker-colors)

The linked writer preserves matching audio and video timeline In/Out
marks, including In-only and Out-only selections. These are timeline-relative
frames, and Out is inclusive. The paired 24 fps control selected the same
35-frame native render after reopen. Independent audio and video marks
and frame-zero In marks remain readable and block writing because fresh
Resolve imports changed those selections. [Pinned pr2resolve format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#颜色标记与编辑状态)
· [Pinned drpkit marker notes](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/FORMAT.md#edit-information---edits)

Bounded static position, independent scale, crop, and
isolated horizontal and vertical reflections use the witnessed controls
on equal 1920×1080 source and timeline rasters. A reflection is read as
a `resolve.flip` effect with both axis flags. The witnessed two- and
three-key Linear Position and independent Zoom profiles are read as
`resolve.motion_linear` with source-frame keys, native position
fractions (Tilt positive up), and Zoom multipliers. Three-key curves
retain points outside the visible edit within the saved media extent.
The three-key crop profile adds `crop_left`, `crop_top`, `crop_right`,
and `crop_bottom` source-frame curves as raster fractions. All eight
curves share the visible edit's Elastic grid. Softness and Retain Image
Position must be off. A missing crop scalar base represents zero only
when its first saved key is zero.
Within the ordered picture chain profile, a reflected static control is
read as its native Transform and `resolve.flip` axes. The bounded native
animated shape carries `horizontal_flip` and `vertical_flip` in
`resolve.motion_linear` alongside the saved curves. These describe the
Inspector fields on Resolve's clock and geometry.
Static clip volume on a direct mono edit is read as a per-placement dB
value from −6 through +3 for the witnessed single-control shape. The
bounded two-point volume curve preserves exact item-source times and dB
values. Its interpolation and hold law stay in Resolve metadata. Default
linear-amplitude fades retain whole-frame head and tail lengths as
clip-edge metadata and report the portable representation loss. The
linked-project writer also rebuilds these two envelope types on qualified
24 fps mono WAV edits and checks the reconstructed control on each clip
before publication.

The Resolve reader also retains one custom curve point per clip edge
through graph/2. The writer rebuilds the editable Resolve curve on the
24 fps linked mono WAV profile and the 23.976 standalone OP1a profile.
Each point belongs to a nonzero fade and stores a position from 1 to
99 percent and a level relative to clip volume from −6 to 0 dB. The
clip's qualified static volume stays separate. In the saved envelope,
parameters 99 and 100 store two big-endian doubles at field path
`3 → 1 → 7`. A test with field 7 directly on the parameter lost both
points after Resolve reopen. [Pinned drpkit fade notes](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/FORMAT.md#unresolved)

The same clip can carry both its two Linear volume keys and default or
custom fade controls. Graph/2 keeps the keys on the item source clock and
the fade lengths on the edit edges. Resolve reopened both controls and
matched the isolated audio on the qualified 24 fps WAV and 23.976 OP1a
profiles. Curve interpolation outside Resolve, keys outside the edit,
wider gain ranges, Group audio effects, mixed rates and discontinuous
MultiGroups need separate evidence.

Resolve timeline clip colors live in the project lockable blob map.
The linked writer preserves all 16 known color codes by edit occurrence.
It uses the generated edit UUID as the label owner, including when two
edits reference the same media and share a display name. Read-back checks
each generated owner and its code. Uncolored edits carry no color entry.
An unknown code or duplicate label owner blocks the affected round trip.
Color mappings from other editors need separate evidence. [Pinned drpkit clip color notes](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/FORMAT.md#edit-information---edits)

Linked Resolve exports preserve names and edit locks on the qualified
V1/A1/A2 timeline. Graph/2 records them by sequence and track kind/index,
so repeated names keep separate owners. The writer sets the timeline's
`UserDefinedName` and lock bit and writes the matching audio name and
lock rows in Fairlight. It checks the decoded state before publication.
Native audio XML and mixer values that disagree block the write. This
profile preserves lock separately from audio mute. [Pinned pr2resolve track state notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#颜色标记与编辑状态)

The same profile preserves disabled V1 independently of the edit lock.
The output retains the original edits and can be re-enabled. Resolve
reopened the disabled and disabled-plus-locked controls and rendered black
picture while both audio channels matched the enabled control.

Static track faders on the qualified two-mono Fairlight mixer map to
the graph track's level. Static clip volume stays on the placement.
The reader also recognizes static track and main bus faders in the two
saved linked mono mixer forms with the qualified remaining mixer state.
The static main stereo bus level is retained in the Resolve sequence
receipt as `main_bus_level_db`. The qualified internal writers for
Resolve and Premiere apply that value to their respective output faders.
Static track mute on that mixer is retained in the Resolve sequence
receipt as `muted_audio_tracks`, with a scoped portability loss. The
timeline flags and both mixer rows must agree. The track and its edits
remain present, with clip volume, track level and bus level independent.
The qualified internal Resolve and Premiere writers apply the saved
mute to the owning audio track.
The linked mixer forms also retain static track pan with the saved
track ownership, routing, and processing qualification.
Recognized but unqualified mixer settings or conflicting mute fields
receive scoped losses that block an internal Resolve write.

## Read back a timeline in Resolve

With Resolve already running and its installed Python scripting module on
`PYTHONPATH`, run `python3 scripts/resolve_native_readback.py` for the active
timeline. Use `--timeline-index N` to query a one-based timeline index in the
current project. The command prints the version, record bounds, track states,
and item ranges. It uses the API's subframe-precision record queries and reports
the API source bounds and IDs as returned.

The pinned community guide describes the timeline and item queries. Resolve
21.2's installed reference uses `GetItemListInTrack` for enumeration and
documents the subframe arguments and source frame accessors used here.
[diop/davinci-resolve-api](https://github.com/diop/davinci-resolve-api/blob/a75fd3d22a921cd1a749dbb36bb9e1ae6f5ce781/docs/README.md)

On two retained 24 fps controls, active and indexed queries returned the
same ranges. The API matched graph/2 on the nonzero timeline origin, six
placements per project, trims and durations, plus A2 mute and lock on one
control and V1 disable and lock on the other. Keep API IDs and archived
sequence IDs separate when comparing across imports.

## Inspect archive and version metadata

`resolve-source-probe --inspect-archive INPUT OUTPUT.json` reads the bounded
DRP or DRT directory and, when present, parses `project.xml`. It reports
the saved version header, the separate project version field, whether they
agree, the timeline registration count, and sorted member paths with
declared expanded sizes. It does not parse the remaining member payloads
or make a graph. A DRT may report a missing project record.

The pinned [drp-extractor reference](https://github.com/hfiguiere/drp-extractor/tree/b43a358325f2eb0a3f034c8f431e75de5c96bd89)
locates the Resolve 14 project XML in the archive. Its project, media pool
and sequence reader bodies are placeholders at this revision. The probe
uses the converter's existing bounds, directory and XML checks.

A retained Resolve 20.3 archive reported project version 15, 44 registered
timelines, and 89 sequence container members. The two counts describe
different saved structures. A Resolve 21.2 archive reported version 17
and four of each. The existing graph path still refuses the version 15
archive. Reports can contain private archive member paths.

## Saved active timeline

The DRP reader resolves the zero-based `CurrentTimelineIndex` through
`TimelineHandleVec`. The Media Pool root owns a `resolve.active_timeline`
receipt with `saved_index`, `sequence_id`, and `unavailable_reason`. A
missing, malformed, ambiguous, or out-of-range selection keeps the graph
readable and leaves the sequence ID unset. DRT exports do not supply a
project selection receipt.

The retained `Native-mono-23976.drp` registers Linked B at index 2. An
independent walk of the archive's handle and sequence records confirmed
that UUID. The [linked writer checks](RESOLVE-PROJECT-CORE.md#saved-active-timeline)
verify selection remapping, including duplicate names and subset exports.

## Compare saved timelines and blobs

```text
resolve-source-probe --diff LEFT LEFT_TIMELINE_ID RIGHT RIGHT_TIMELINE_ID OUTPUT.json
resolve-source-probe --diff-blob LEFT LEFT_OBJECT_ID RIGHT RIGHT_OBJECT_ID FIELD OUTPUT.json
```

### Replay retained reports

Build `resolve-source-probe` and run the private offline regression set with
a fresh output directory.

```sh
python3 scripts/verify_resolve_reports.py --probe /path/to/resolve-source-probe \
  --manifest /path/to/private/resolve-report-fixtures.json --output /path/to/new-output \
  --ffmpeg /path/to/ffmpeg --ffprobe /path/to/ffprobe
```

The `anynle.resolve-report-fixtures/1` JSON manifest has a `files` object
mapping paths to SHA-256 digests and a nonempty `cases` array. Each case
has a unique `name`, `mode` (`diff` or `conform`), `left` and `right`
keys from `files`, exact `left_id` and `right_id` timeline selectors,
and the SHA-256 of the reviewed report bytes in `report_sha256`. Conform
cases can also set `all_frames` and supply a `renders` array with two
pinned paths. Relative file paths are relative to the manifest. Backend
paths are required for cases with renders. Keep this manifest and the
fixture data private. A successful run writes `verified.json` after
rechecking the inputs.

### Timeline report scope

Use exact sequence IDs from the DRP reader or qualified DRT graph output.
The timeline report compares clocks, canvas, track layout, edit ranges,
saved media timing and locators, physical audio selectors, timeline
markers, picture controls, clip volume and the qualified Cross Dissolve.
For V1 and two mono lanes, it also compares custom track names, edit
locks and V1 output by lane, independently of saved track UUIDs.
On that layout it compares qualified saved mono track faders, track
mute, and the main bus fader separately from clip volume. Missing
neutral mixer observations and conflicting controls produce gaps.
Retained archive pairs isolated A1 and A2 fader changes of −3 and −6 dB,
a main bus change of −3.7 dB, and an A1-only mute. A conflicting mute
fixture remained inconclusive. The probe also checks qualified static
track pan and bypass, direct output assignments, stereo bus count, and
Bus 2 gain and mute. Track pan is reported as a signed percentage and bus numbers are
one-based. Retained archives isolated a pan change, a change from one
bus to two with A2 assigned to Bus 2, and separate Bus 2 gain and mute
changes. A one-cell bypass control left the pan value unchanged.
A mismatched Bus 2 mute mirror stayed inconclusive. The probe compares
the supported static EQ and dynamics settings by saved track or bus,
including saved values of disabled controls. Retained archives isolate
A1 bell gain, A2 compressor settings, and EQ on each stereo bus.
Missing or inconsistent rows and out-of-profile values are gaps.
Unmapped Fairlight state remains a separate gap; these results describe
saved settings, not automation, plug-ins, or rendered sound.
The probe compares saved video and audio In/Out marks separately, including a
frame-zero In and independent audio/video ranges that may change on
Resolve import. Malformed marks and conflicting track state are reported
as gaps. These comparisons concern saved values, not rendered output.
For paired edits the report also compares decoded speed and source
duration, frame maps when available, and the saved Resolve clip-color
code. A code outside the known palette is a gap. For paired picture
edits, the report also compares the supported single-node primary gain
and identifies an added or removed grade. It validates the value
against the saved node body and treats other grade profiles as
inconclusive. Paired audio edits compare saved Voice Isolation and
Dialogue Leveler records by effect ID, including raw flags and decoded
parameters. Changed clip FX payloads also report a gap for any settings
outside the decoded fields. Plain subtitle cues
are compared by track and exact record start, including text and
duration. A moved cue appears as a removal and addition; an unresolved
subtitle track or cue leaves the caption comparison inconclusive.
Solid Color and Text+ pair by kind, track, and exact record start.
The report compares their durations, the generator's saved 16-bit RGB,
and the title's text and decoded literal controls. A changed Text+
composition hash also produces a gap because the remaining Fusion
settings can affect the picture. The retained controls detected a
red-to-green generator change and a Text+ size change with the same text.
For qualified 24 fps V1 nests, the report compares the saved parent
range, child trim, child origin, and child sequence identity. It pairs
occurrences by ID or by a unique record start. Same-named children remain
distinct. If both archives rewrote the child IDs and lack a shared
identity, the binding is inconclusive. Select the child timelines
directly to compare their contents.

Multicam picture and audio uses pair independently by saved occurrence
ID or by a unique track and record position. The probe checks each
selector against its saved angle and the resulting file edit, including
trim, media path and physical audio channel. Explicit angle gaps remain
part of the comparison. Unpaired or contradictory bindings are
inconclusive. The retained picture switch reports a change from Camera 1
to Camera 2 while both mono lanes retain Camera 1 and their saved channels.
It pairs shared edit IDs of the same kind first, then unique slots by track
and exact record start. An edit that moves and receives a new ID appears
as a removal and an addition. Audio selectors use zero-based audio cache
track and channel indexes. Media locators are the saved paths.

The report records `different` for a confirmed change, `inconclusive`
when unresolved facts prevent a match, and `match` when the compared facts
agree. A report can contain both differences and gaps. For a DRT that
falls outside the graph profile, use `--inspect-drt` to see the saved
observations. Completed comparisons return exit status zero.

For blob comparison, select an exact object UUID from each archive and
choose `FieldsBlob` or `EffectFiltersBA`. The report distinguishes a
missing field from an empty field and gives byte counts, SHA-256, and up
to 64 changed positional ranges with 16-byte previews. A length change
can shift subsequent offsets. Both commands publish a new report
exclusively.

The same-timeline [DRT and DRP](RESOLVE-DRT-CLOSEOUT.md#native-comparison)
matched on timeline content with rewritten IDs. Their dissolve effect
payloads matched, and their saved transition envelopes differed in three
byte ranges. The retained [straight-cut and dissolve controls](RESOLVE-DRP-FORMAT-CLOSEOUT.md)
reported just the added transition. The replay also covered a one-frame
source trim, switched WAV channels, duplicate names, ambiguous slots and
unresolved bindings. Native archives and reports remain in private local
evidence.

## Check conform at V1 cuts

```text
resolve-source-probe --conform REFERENCE REFERENCE_ID CANDIDATE CANDIDATE_ID OUTPUT.json [--all-frames]
resolve-source-probe --conform REFERENCE REFERENCE_ID CANDIDATE CANDIDATE_ID OUTPUT.json --renders REFERENCE.mov CANDIDATE.mov FFMPEG FFPROBE
```

Select exact timeline IDs in DRP, qualified DRT, or saved graph/2 files.
The probe samples the two sides of each internal V1 edit boundary on
either timeline, at matching exact 24 or 23.976 rates and a known common
origin. The report compares the full saved locator and the file-relative
source frame, applying the item-to-file offset once. It also compares
physical video stream selectors when both are known. The profile covers
originals linked in place. Unknown mappings are inconclusive, and a
confirmed mismatch stays visible alongside gaps. Qualified dissolve
windows use half-open frame intervals, and samples inside them are
excluded.

At 24 fps, saved 50% forward and 100% reverse edits also contribute
source-frame samples. Half speed repeats each source picture twice
except the final picture of an odd-length edit, which appears once.
Reverse samples from the last picture in the saved source range first.
An explicit graph/2 frame map provides file-relative frame numbers
directly, including when the source and timeline rates differ. The
probe checks the source rates as well as the selected frame numbers.
Other speed profiles and unresolved saved timing remain inconclusive.

For qualified one-level picture nests at 24 fps, the probe follows the
saved child trim into the child timeline. It reports the parent nest ID,
the child edit ID, and the selected file frame. Cut sampling includes
child cuts and boundaries of unsupported child intervals that fall inside
the parent edit. A saved child V1 gap is a gap in a full scan. Child
dissolves and other unsupported coverage stay inconclusive over their
projected intervals. The retained two-timeline,
two-compound control and reopened writer output matched across all 40
source-frame selections and rendered pictures.

For qualified Resolve V1 multicam edits, the probe checks the saved angle
against its file edit and reports the multicam occurrence ID. Explicit
gaps in the selected angle count as saved V1 gaps in a full scan.
Ambiguous selectors and conflicting bindings stay inconclusive. The
retained 72-frame switch from camera A to camera B detected 24 frames
of newly covered picture and a source change on the other 48 frames.

Use `--all-frames` to compare every timeline position from frame zero
through the longer saved extent. The report identifies the sampling mode
and records an extent difference even when the shared frames agree.
This covers single-edit timelines and catches changes within an edit.
The mode requires known extents, is limited to 16,384 frames, and works
with `--renders` and `--moves`. Dissolve windows stay excluded. Inside
both known extents, two saved V1 gaps match. A saved gap opposite a V1
edit is a coverage difference, even when the edit's source binding is
unresolved. Unknown edit coverage remains inconclusive. The optional
render check compares the actual pictures at known gaps, where other
tracks can contribute to the rendered result.

For the optional picture check, supply same-profile full-timeline MOV
renders beginning at record frame zero. The probe checks their decoded
extent and constant frame grid and compares the SHA-256 of decoded pixels
at the eligible positions. It reports picture and saved mapping results
independently. For example, a missing source offset can leave the saved
mapping inconclusive while the supplied pictures still match. An
unavailable or unsuitable render leaves the saved mapping report usable.
Each media process has a 60-second deadline and an 8 MiB output limit.
Reports are written exclusively.

The retained straight-cut graph and DRP both map record frames 23 and
24 to file frames 28 and 8. The native and reopened dissolve renders
match at record frame 24, with frames 16–23 excluded. Local controls
detected a one-frame slip, a same-name locator change, and a picture
mismatch with matching saved mappings. [Pinned conform runbook](https://github.com/samuelgursky/davinci-resolve-mcp/blob/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/conform-qc/docs/runbook.md)

### Verify conform after a media move

```text
resolve-source-probe --conform BEFORE.drp BEFORE_ID REOPENED.drp REOPENED_ID OUTPUT.json --moves MOVES.json [--checksums MANIFEST.json] [--renders BEFORE.mov REOPENED.mov FFMPEG FFPROBE]
```

Use the [verified relink](VERIFIED-RELINK.md) move list and optional pre-move
checksum manifest. The original paths in the reference DRP may be offline.
Path mode checks the moved files at their explicit destinations and checks
embedded reel and source timecode for duplicate basenames. Checksum mode
hashes the moved files against the reference manifest. The probe uses
`ffprobe` on PATH or the tool supplied with `--renders`. If move evidence
fails or a destination changes during the check, the probe exits with an
error before it publishes a report.

Each cut sample shows the original locator, expected destination, and actual
candidate locator. The probe checks the exact file-relative frame and any
known physical selector, and compares saved reel and timecode where the
reference establishes them. Missing candidate metadata makes that sample
inconclusive unless another fact already differs. Supplied render results
remain independent.

Both verification modes matched the retained same-name DRPs and renders at
record frames 23 and 24. The saved archives select file frames 27 and 5.
The outgoing edit's saved In is 4, while the earlier Resolve scripting
readback reported 3. The outgoing source repeats its picture, so the
render comparison confirms picture equality at the cut without resolving
that source-index discrepancy.

## Inventory saved effects

```text
resolve-source-probe --inventory-effects INPUT OUTPUT.json
resolve-source-probe --diff-blob LEFT LEFT_OBJECT_ID RIGHT RIGHT_OBJECT_ID EffectFiltersBA OUTPUT.json
```

The inventory walks the saved picture and audio tracks in a bounded native
DRP or DRT archive. Each nonempty clip or transition `EffectFiltersBA`
record gives the sequence and container IDs, track kind and one-based
index, object UUID and class, decoded byte count, and SHA-256. The sequence
ID comes from the track and remains visible without project registration.

Recognition comes from the existing source reader. `recognized` means the
reader interpreted that occurrence's saved controls. `unqualified` means
it encountered the payload without qualifying its controls. `not_evaluated`
means the reader could not assess that occurrence. The report preserves
the saved payload facts when graph interpretation fails and includes the
graph error and any available DRT gap details.

Copy a row's `selector.object_id` into the appropriate side of
`--diff-blob` and use its `selector.field`. Select the left and right
objects separately because Resolve can rewrite UUIDs. Payload errors and
duplicate or missing UUIDs leave the row visible without a selector.
For valid hex over the 64 KiB field limit the report gives the saved byte
count. Output uses exclusive publication.

The retained straight cut has zero effects, the native Cross Dissolve has
one recognized effect, and the reflection and gain controls report their
recognized picture and audio effects. The unqualified picture control
remains visible by UUID. The paired [native DRT and DRP](RESOLVE-DRT-CLOSEOUT.md#native-comparison)
give different dissolve UUIDs and identical 66-byte effect payloads.
Using the two selectors produces a matching `--diff-blob` report.
The incomplete DRT controls still list the dissolve when graph export is
refused. The offline replay also covers invalid hex, over-limit fields,
duplicate UUIDs and field tags, malformed archives, and output collisions.

## Bounds and validation

The reader admits single-disk ZIP32 archives with up to 1,024 members and
32 MiB on disk. It checks the raw directory count against the library's
name index to catch exact duplicate members. No member is extracted. The
expanded archive, XML node/depth counts, per-field decoded bytes and
aggregate decoded blobs are bounded. The graph writer enforces the
64 MiB encoded ceiling. Unsupported native objects receive scoped losses
when their owning sequence or bin is known. Broken required references,
malformed encodings and unsupported project versions fail before output.

Qualification compares retained independently authored archives and editor
read-back with writer-generated controls. See the Resolve authority's
RESOLVE-0001 through RESOLVE-0014. The envelope decision is pinned to
RESOLVE-0009 at Resolve knowledgebase commit
`fa0d19dbad3c7194227ad91973d1f933075ba658`, finding SHA-256
`89802b42657d4b537c1badc9fd3ea6dde4d4269e0e81ffc1b81251e431ac8e15`.
The track fader decision is pinned to RESOLVE-0010 at Resolve knowledgebase
commit `01197af63c7cadffa79044ed3105e87a428fb69e`, finding SHA-256
`5ae809dcb9f1242577971b6bb1a8a866cc7610804fd467015d842557e20e551f`.
The bus fader decision is pinned to RESOLVE-0011 at Resolve knowledgebase
commit `2c316bb8653cc1969af80c40e7e6e7821a9bffdb`, finding SHA-256
`c5b214c272c5a14af8d0e32ff5ad3149471ef4a0e621d7f44faa826ae308b9d2`.
The track mute decision is pinned to RESOLVE-0012 at Resolve knowledgebase
commit `260199e27864de88ea2c492acc7e0c0a2e8d251a`, finding SHA-256
`1efe77b10d30ab16e7f1f61c5edf82fccb1a10ac4ef264406fab51b4a18e87db`.
The linked project and WAV cache decisions are pinned to RESOLVE-0013 at
Resolve knowledgebase commit `ca2db99731ac50acb6dcd481cb122826f9bc4368`,
finding SHA-256
`fefd639ebaa57569a76c5f3b05623008eb29d16d51f8820439028a74e8213508`.
The Avid/Premiere dependency checker does not consume Resolve findings.
The new reader does not establish a
Resolve-to-Premiere conversion. Premiere preparation still has explicit
Avid compatibility receipts for clocks, source geometry, effects and
multicam routing. The [internal Resolve writer](RESOLVE-PROJECT-CORE.md)
can recreate bounded static clip volume, fades, two-point volume keys,
static track faders, static main bus fader and static track mute using the
saved Resolve clock and verified media inputs. The envelope and mixer
receipts remain Resolve-specific.

## Editable conversion profiles

The linked DRP probe consumes saved graph/2 through the existing internal
manifest. The following profiles use exact rates and source trims; unsupported
timing, routing and topology remain explicit. The public routes are unchanged.

- **Picture retiming:** A same-rate 24 fps picture-only edit can carry
  50% forward or 100% reverse as graph/2 `Speed`. Resolve's saved map uses
  a stretched In for half speed and an end-relative In for reverse. Qualified
  two-point maps and Premiere 27 saved speed/direction use the same file
  range in graph/2. Other maps remain source observations. Each of two
  24-frame Resolve renders matched the corresponding native control after
  save, close, and reopen. The Premiere 27 saves read back through graph/2
  and produced matching Resolve renders. Premiere 27 uses raw file In/Out
  for the linked forward clip and end-relative In/Out for reverse. The
  corrected Premiere files matched all 48 source frame selections after
  Save As, close, and reopen.
  The reader also preserves an odd record length on this saved 50%
  nearest-frame map. In the retained 23-frame Resolve render, source
  pictures 12–22 each appear twice and picture 23 appears once. Graph/2
  retains the exact 23/48-second source span, and the reader requires all
  12 sampled pictures to be available. Writing an odd-length half-speed
  edit in either destination still requires native qualification.
- **Plain captions:** One Resolve subtitle track with editable default
  styling maps to graph/2 `SourceText`. The saved XML and visible burn-in
  checks passed for two adjacent accented cues, one multiline, on a
  nonzero timeline start. Premiere saved caption text is supported as input;
  preserve destination defaults and report unmapped styling. Both cues
  were visible in the reopened Premiere projects. A retained Avid SubCap
  over linked OP-Atom at 24000/1001 mapped V2 to Resolve subtitle 1, with
  both saved lines and all 48 visible frames after reopen. More than one
  source caption lane requires a separate mapping.
- **Dissolve:** One eight-frame left or center V1 Cross Dissolve between
  plain edits at 24 or 24000/1001. The Resolve sequence-owned transition
  receipt keeps the native alignment, interval, and endpoint IDs. Avid
  Blend Dissolve and Premiere Cross Dissolve use graph/2 transition
  occurrences and verified handles. The two 24 fps native Resolve
  comparisons matched all 48 decoded frames in each timeline. One centered
  eight-frame 0 dB A1 Cross Fade is also writable at 24 fps with verified
  WAV handles. The native reopened comparison matched both isolated
  PCM lanes; the difference from straight cuts stayed inside the A1
  fade window. The comparison control was made from documented saved
  fields, then imported, saved, reopened and rendered in Resolve.
- **Structure:** A matched native skeleton supports 24 fps V1/V2 overlap
  and three independent 48 kHz PCM WAV mono lanes. All 48 picture frames
  and three distinct isolated PCM renders matched after reopen. The
  [picture nesting profile](#write-picture-nests) also writes one level of
  native Resolve compound clips and nested timelines by saved child ID.
- **Generator and titles:** A sequence-owned `resolve.generators` receipt
  can recreate one opaque Solid Color generator at the end of a picture-only
  V1 timeline. The combined native control reopens with that generator,
  speed edits, captions and a separate dissolve. The Text+ source reader
  extracts bounded literal controls without evaluating the Fusion graph.
  Editable Text+ writing and visible text qualification remain open.
- **Color and Fairlight:** `resolve.primary_gain` belongs to the graded edit.
  One exact primary node profile can be recreated in Resolve. The 1.25
  control matches the native render on all 24 frames; a 1.5 control changes
  the output. The [Fairlight track processing profile](#write-fairlight-track-processing)
  also writes one EQ bell and the compressor switch, ratio, threshold
  and timing.

### Write linked audio to Premiere

```text
resolve-source-probe --premiere INPUT.graph.json SEQUENCE_ID FFPROBE OUTPUT.prproj [--accept-losses]
```

The internal reverse route accepts same-rate progressive square-pixel
1080p ProRes HQ MOV on V1 at 24 or 24000/1001 with a qualified non-drop
origin and two independent mono WAV lanes. The WAV can be PCM16 or
PCM24 at 48 kHz. Each placement keeps its saved physical stream and
channel, including independent trims when both lanes reference one
stereo file. Premiere gets discrete mono source views for fresh
insertion. The 24 fps picture-only speed and caption profile remains
available through the same command.

The probe reports timeline In/Out marks and any unrecognized clip label
codes before creating a project. Use `--accept-losses` to proceed with
those omissions. The source graph retains the original values. Timing,
media, routing, and unsupported processing still use the normal refusal checks.

Source capture and project writing are checked with the retained 24 fps
separate-file control and the two 23.976 stereo-file controls. The
offline check follows each written edit to its file and physical
channel, verifies exact clocks and ranges after graph reload, and
decodes the selected PCM ranges from the originals. The retained
Resolve renders include a 72-sample edge ramp and a −3 dB output pan law
in these fixtures. Compare those separately from sample placement and
channel selection.

Premiere 27.0.0 build 88 retained all three timelines' origins, edit
ranges, paths and channel selectors after Save As, close and reopen.
All 48 numbered picture frames per timeline match the Resolve source
frame selections. Decoded pixel hashes differ between the two editors'
renders. The six isolated Premiere renders keep their exact 96,000 or
96,096-sample spans and source offsets. Those renders and fresh mono
insertions match source PCM to within one 24-bit code value at Premiere's
center-pan amplitude of `1/sqrt(2)`. The converter adds no Resolve ramp.

Resolve Studio 21.2.0 build 00006 also exported a standalone linked AAF
for the 23.976 PCM16 stereo timeline. The imported Premiere timeline
needed no relink. Its reopened paths, clock, trims and physical
selectors agree with the Any NLE project. All 48 Premiere-rendered
picture hashes match between those two routes and both isolated lanes
agree within one 24-bit code value.

The linked mono profile also writes static clip gain from −6 through
+3 dB as editable Premiere Volume Level, and saved static clip pan
from −100 through +100 as the mono clip panner. Default linear-amplitude
fades become editable, one-sided Constant Gain transitions owned by
the original clip edge. Whole-frame durations are exact. Fractional
durations are rounded to the nearest Premiere tick.

In Premiere 27.0.0 build 88, both the native fractional-fade case and
the whole-frame control kept their original file, independent channel
selectors, trims, gain, pan, and fade ranges after Save As and reopen.
The tests isolated each mono lane into a stereo PCM24 render of 96,096
sample frames. They checked a −6 dB / −50 pan clip with a head fade and
a +3 dB / +50 pan clip with a tail fade. Both normalized fade midpoints
measured within 0.04 percentage points of half amplitude in each
editor. Premiere applies its own panning law, which changes the stereo
balance at the same pan setting. The conversion reports that difference
on each nonzero pan occurrence. PCM equality across editors is outside
this qualification.

For direct mono clips, the route also writes two in-range, Linear Resolve
volume keys as editable Premiere Volume Level keys. Source times stay on
the item clock and dB values stay within the qualified −6 to +3 dB range.
Keys can coexist with default whole-frame fades on the same clip.

The 23.976 stereo PCM16 source control uses opposite ramps on two
independently trimmed mono lanes. Premiere 27 retained both pairs of
keys, their source times, channels, and trims after Save As and reopen.
A second control overlapped the ramps with a 12-frame head fade and an
18-frame tail fade. The reopened projects retained both fade intervals,
and the isolated PCM renders confirmed their linear-amplitude response.
Across the two 9 dB ramps, Premiere's Linear interpolation differed from
Resolve's linear-dB curve by up to 0.31 dB after normalizing steady
channel levels. The converter reports the interpolation difference on
each keyed clip.

Static Resolve track faders transfer as editable Premiere Audio Track
Mixer faders on A1 and A2. The qualified linked mono mixer accepts exact
tenths of a dB from −6 through 0. Clip Volume Level and its source-time
keys remain on each clip. Premiere 27 kept −3 dB on A1 and −6 dB on A2
after Save As and reopen in two 23.976 controls. One used static clip
volume, pan, and fades. The other used two editable volume keys per
clip. Compared with the corresponding clip-only Premiere renders, all
96,096 sample frames per lane matched the expected fader scaling within
one PCM24 code value after rounding.

The same profile transfers the static main stereo bus fader from Resolve
Bus 1 to Premiere's sequence Mix fader. It accepts exact tenths of a dB
from −6 through 0. Premiere 27 kept a −3.7 dB Mix fader alongside the
separate A1 and A2 faders and each clip's static or keyed volume after
Save As and reopen. The isolated renders confirmed that the Mix fader
applied −3.7 dB to each lane. Against the unity-bus Premiere renders,
all 96,096 frames per lane matched within one PCM24 code value after
rounding. [Adobe Audio Track Mixer overview](https://helpx.adobe.com/uk/premiere/desktop/add-audio-effects/advanced-audio-techniques/about-audio-track-mixer.html)

Static track mute transfers to the owning Premiere audio track. Premiere
27 kept A1 muted in the static clip volume control and A2 muted in the
keyframed clip volume control after Save As and reopen. Each isolated
muted lane rendered digital silence across all 96,096 stereo PCM24
sample frames. The other lane matched its unmuted reference sample for
sample. Re-enabling the muted lane restored its reference signal. The
Mix fader, both track faders, and the clip volume and keyframes survived
on their original owners.

Static track pan is editable on A1 and A2 of the Premiere Audio Track
Mixer. These inputs use mono tracks feeding a stereo Mix. The qualified
Resolve profile keeps the single main bus, direct routing, and static
pan values in tenths of a percent. Premiere 27 kept the pan, mute, Mix
level, both track levels, and each clip's static volume or keyframes
after Save As and reopen. At hard left and hard right, each isolated
lane produced digital zero on the opposite channel. At ±50 the
quieter channel measured about 41% of the louder channel in Premiere
and 27% in Resolve. The converter preserves the pan setting and
reports the difference in pan law for intermediate values.

With both static clip pan and track pan on a lane, Resolve adds the
two values and clamps the result to −100 through +100. Premiere's
mono input did not apply a separate clip panner in the native control.
The converter writes the effective value as one editable Premiere
clip pan on each affected edit, including edits with neutral clip pan.
It reports the loss of independent pan controls. A lane without clip
pan keeps its editable track panner. Premiere's pan law still applies.

Default linear-amplitude clip fades remain editable with both forms.
The fade uses the mono channel layout on a mono input and the stereo
layout on a standard track. Premiere 27 Save As and reopen controls
covered aligned, opposing and intermediate pans, a mixed mono/stereo
timeline, and fades overlapping volume keyframes. Both tracks
retained their source assignments, mute state, Mix and track gains,
and static or keyed clip volume. Isolated PCM confirmed expected
routing, digital silence while muted, and the fade response.

Unknown offsets, out-of-range edits, ambiguous selectors, unsupported
controls and malformed files block the internal write. Other volume
curves, keys outside the edit, keys combined with fractional or custom
fades, mixer automation, additional buses and custom
routing remain outside this Premiere profile. Originals stay in place.

### Resolve to Premiere timeline clip labels

The internal route writes all 16 known Resolve clip colors to individual
Premiere timeline edits. Two edits of one media item can keep different
colors. The shared master and bin labels keep their existing defaults.
Resolve and Premiere use different palettes, and a Premiere installation
can have customized label colors. The writer uses Premiere's standard label
names and reports each named substitution on the source edit ID.

| Resolve | Premiere |
|---|---|
| Green, Teal, Blue, Purple, Tan, Brown, Violet, Yellow | Same label name |
| Pink | Rose |
| Apricot, Orange | Mango |
| Beige | Tan |
| Chocolate | Brown |
| Lime | Green |
| Navy | Blue |
| Olive | Forest |

Offline verification covered four reopened Resolve controls, 16 labeled
edits, eight unlabeled neighbors, and repeated uses of one picture master.
Premiere 27 Save As and reopen is pending. Timeline In/Out and Media Pool
label mapping remain separate.

### Resolve to Premiere track state

The internal linked V1/A1/A2 route uses the selected sequence's
`resolve.track_state` sidecar. It writes each custom name, edit lock,
and V1 output state on the matching Premiere track. Names may repeat
across lanes. Turning off V1 writes the Premiere video track output
toggle and keeps all its clips. It does not write an audio fader mute.

The retained 24 fps Resolve controls cover names and locks independently
and in combination, plus V1 disabled with and without a lock. The
written Premiere files retain the exact edit ranges, source paths, and
WAV selectors. The tested single-bus mixer retains its independent
left and right track pans. Conflicting timeline and mixer state,
unsupported audio mute combined with lock, and the two-bus profile
still block output. Premiere 27 Save As, reopen, and picture and PCM
comparisons remain pending.

### Resolve to Premiere sequence marker colors

The internal linked Premiere writer uses Premiere's named colors for Resolve
sequence markers in Green, Red, Blue, Cyan and Yellow. It maps Lavender to
Magenta, Sand to Orange, and Cream to White, reporting each substitution by
marker occurrence. Other Resolve colors use Premiere's default and are reported.
The graph keeps the original Resolve palette name and any saved custom data.
Premiere receives the marker's title, note, record-relative start, and saved
duration, including Resolve's one-frame duration for a point marker. The writer
reports custom data that only the graph retains.

An offline check used two retained Resolve 21.2 saves at 24 fps with a
nonzero timeline start. The written Premiere files contained all five and
fourteen sequence markers respectively. The five-marker control kept both
four-frame ranges and the multiline accented note. The palette control
exercised all eight Premiere color indices. Premiere 27 Save As and reopen
is the remaining native check for this direction.

### Resolve to Premiere static picture controls

For the internal 1920×1080 square-pixel ProRes HQ profile at exact
24000/1001, the converter writes static position, independent X/Y scale
and all four crop edges as editable Premiere Motion controls. Position
is limited to five percent per axis, scale to 87.5–112.5 percent and
crop to five percent per edge. Rotation, anchor changes, animated
controls and effect stacks are outside this profile.

The Resolve reader accepts native crop with no Motion section and
retains combined static crop and motion on the same graph/2 edit.
In Premiere 27.0.0 build 88, isolated and combined controls kept their
values after Save As and reopen. The rendered crop moved and scaled
with the image. Across 48 frames per case, measured edges matched
Resolve within one pixel. The effect was confined to the intended
24-frame edit. The Resolve writer keeps its separate admission rules
for combined static geometry.

### Resolve to Premiere Linear position and scale

The internal writer maps the saved `resolve.motion_linear` record to
editable Premiere Motion at exact 24000/1001 on the 1920×1080
square-pixel ProRes HQ profile. Resolve's vertical position is positive
up. The writer negates it once and converts Zoom X/Y to Premiere
percentages. It uses the saved file-relative key frames directly.

- Two Linear keys share the source frames at the first and last visible
  frames. Premiere 27.0.0 build 88 kept Position, Scale Width and Scale
  Height on frames 24 and 71 after Save As and reopen. Across 48 frames,
  the measured chart features agreed with Resolve within three pixels
  horizontally and one vertically.
- Three Linear keys on each axis can span a head or tail trim. Position
  and height share a source-frame grid, and width can have its own grid.
  Both grids must span the visible edit and share its first or last
  frame. Every key must fall inside verified original media, including
  keys outside the edit. Premiere 27 retained all three keys and Linear
  interpolation after Save As and reopen in six head- and tail-trim
  cases. Across 192 rendered frames, measured outer edges agreed with
  Resolve within one pixel. Internal chart features agreed within four
  pixels horizontally and one vertically.

Longer curves, nonlinear interpolation, reflection combined with this
motion, and animated crop require separate qualification.

### Resolve to Premiere video dissolves

At exact 24000/1001, the internal writer maps one eight-frame V1 Cross
Dissolve with left or centered alignment to Premiere Cross Dissolve
(Legacy). It retains the two saved edit identities and checks the
same-rate ProRes HQ source handles before writing. The left alignment
needs eight frames of incoming preroll. The centered alignment needs
four frames on each side. Visible clip trims remain intact.

After Save As and reopen in Premiere 27.0.0 build 88, both controls
retained the cut at frame 24, the transition intervals (frames 16–23
and 20–27), and all media links. Picture outside each blend matched
the Premiere straight-cut control. Premiere used the outgoing image
at the first blend frame, where Resolve had already started blending.
The writer reports the difference in blend sampling. Both isolated
audio lanes matched the Premiere straight-cut control sample for
sample across 96,096 frames.

### Resolve to Premiere audio crossfades

The internal writer maps one centered eight-frame Resolve Cross Fade 0DB
on A1 at 24 fps to an editable Premiere Constant Gain transition. The
originals are mono 48 kHz PCM WAV. The writer uses the two saved edit
identities, verifies both four-frame handles against the original files,
and preserves the visible trims. Wider transition profiles and crossfades
on a track with independent pan or clip edge fades remain unqualified.

In Premiere 27.0.0 build 88, the native output and Resolve's linked AAF
both reopened with the same links, physical channels, record spans and
eight-frame transition at frames 20–28. The AAF contained an Avid Audio
Dissolve with a linear Level ramp. Premiere imported it as Constant
Power, saved both handle extensions in the clip trims, and played A1's
incoming source four frames early. The native output preserved the
source trim and the linear blend. Its blend coefficients differed from
Resolve by less than 0.00003 after accounting for the editors' center pan
levels. Across all 96,000 sample frames on A2, the two Premiere renders
agreed within one PCM24 code value.

## Write picture nests

The internal linked probe can round-trip one level of picture-only 1080p24
nests on V1. Select the parent and every referenced child by exact sequence
ID in the project manifest, including unregistered compound children. Use
the saved owning bin for each row. Ordinary project registration order is
preserved independently of where the compound definitions appear in the
manifest.

```text
resolve-source-probe INPUT.drp GRAPH.json
resolve-linked-probe GRAPH.json SPEC.json MONO-LAYOUT.drp RATE-PROFILE.drp FFPROBE OUTPUT.drp --nested NEST-TEMPLATE.drp
```

The same-build synthetic donor is hash-pinned at
`48aa85bd7184d72532b52f993158e7c515b9e820887855f4e1065798b7e5263a`.
Use fresh source receipts so the writer can verify the plain picture
mapping and the compound's saved `SeqRef` container link. The parent
clip's `In` stays relative to its child; `MediaStartTime` retains the
child origin. The identity time map spans the child, including material
outside the parent edit. Before publication, the staged reader checks the
generated pool and sequence IDs, child extent, parent range, trim, and
compound container linkage. Audio nests, deeper nesting, and mixed rates
remain unsupported.

Resolve Studio 21.2.0 build 00006 reopened two identically named children
at 02:00:00:00 and 03:00:00:00 with parent trims of 3 and 7 frames. The
writer intentionally registered them in the opposite order. The 32-frame
parent matched the native control frame for frame, including its trimmed
compound. Editing the 02:00 child advanced only its ten parent frames.
A second control with two compound clips, one starting at 00:00:10:00,
matched all 40 parent frames after reopen. These are Resolve round trips.
[Native structure reference](https://github.com/samuelgursky/davinci-resolve-mcp/blob/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format/compound-nav.js)


## Write Fairlight track processing

The internal linked DRP writer preserves all six track EQ bands on the
qualified two-mono Resolve profile. Each band retains its switch,
filter type, frequency and knob position, gain and Q. Bands 2–5 also
retain the saved frequency range. Bands 1 and 6 retain their slope.
The writer also preserves the EQ switch, processing order, dynamics
makeup gain and the compressor switch, threshold, ratio, attack, hold,
release, knee and mix. The reader verifies the
mixer fingerprint with the supported rows restored to
their defaults. The saved frequency, range and knob position must agree.
Two exact factory frequency/knob pairs have a separate check because
they differ from the edited frequency law.
Q, ratio, timing, knee and mix retain their native encoding. The writer
checks the qualified values against the raw track rows and verifies the
staged archive.
Older receipts can omit later controls at their native defaults.

The native comparison uses two separate channels and a third project
with both controls active. Resolve Studio 21.2.0 build 00006 reopens and
re-exports each generated project. In the 24 fps fixtures, a +6 dB bell on
A1 and an enabled compressor at −30 dB on A2 changed only their own
channel. Each generated render matched its reopened control over all
96,000 stereo PCM24 sample frames and 48 picture frames. The controls
were prepared with targeted changes to documented saved rows and then
saved and reopened in Resolve. Both ratio encodings and a disabled
compressor with changed threshold and ratio survive reopen. A two-bus
control retained its A1/A2 routes and Bus 2 fader while Bus 1 PCM
matched its native reference.

Separate 24 fps controls also verify a 1223 Hz bell in the high-mid
range, a changed Q and changed compressor timing. Reopened writer
outputs match every PCM sample and picture frame in each comparison.
The EQ changes affect A1 only and the timing changes affect A2 only.
The combined control preserves both.

Separate changes to knee, mix and makeup gain changed only the
compressed A2 signal. Bypassing EQ changed only A1 and retained the
saved bell controls. On A2 with EQ and compression active, putting EQ
before Dynamics changed the render relative to the same settings in
Dynamics-before-EQ order. These checks used one stereo bus and no inserts;
the order field does not enable insert plug-in conversion.

The writer accepts band gains from −20 to +20 dB and thresholds from
−60 to 0 dB on this profile. The four middle bands accept low shelf,
bell, notch and high shelf. The outer bands accept both shelves and
notch, plus high-pass on band 1 and low-pass on band 6. It admits
compressor knee and mix codes from 0 to 100 and makeup gain from 0 to
+12 dB.

The same profile preserves the built-in expander's switch, threshold,
range, ratio and timing, plus the limiter's switch, threshold and timing.
Each remains tied to its saved mono strip. The expander threshold, range,
ratio and both processors' timing retain their saved encodings; this
path does not convert their knobs into settings for another editor.
The shared expander/gate slot accepts saved mode 0 for expander and mode 1
for gate. Mode and enable are separate. Other mode codes and malformed
knob codes remain readable source observations and block this write profile.
In the 24 fps control, the expander affected only A1, the limiter only A2,
and disabled processors retained edited settings with unchanged audio.
All four reopened writer outputs matched 96,000 stereo PCM24 sample
frames and 48 picture frames per case.

The gate comparison uses a two-level A1 signal. With the same threshold,
range and timing rows, mode 1 attenuates the quiet section more than mode
0. A lower-threshold gate control passes the quiet section as well. The
writer preserves the selected mode and settings even when the enable
switch is off. The scope is the built-in track gate on the qualified
two-mono profile.

Multitone controls exercised pass-filter slopes, both shelves, bells
and notches, per-band enable and master EQ bypass. The reopened controls
retain independent A1 and A2 settings, and bypass renders match the
neutral reference. The filter choices and frequency checks are scoped
to the qualified track EQ profile. In a repeated outer-shelf test,
two separate imports of the same reference archive rendered different
A2 spectra with identical exported mixer bytes. Each import gave
identical PCM across three consecutive renders.

The writer preserves compressor and limiter controls independently on
each bus in the qualified one- and two-stereo-bus layouts. Preparation
checks the interpreted settings against the raw rows and preserves the
saved values when a processor is off. The compressor retains ratio,
knee, mix, threshold and timing. The limiter retains threshold and
timing. Dynamics makeup gain also round-trips.

The two-bus test exports two stereo PCM streams. Enabling the limiter
on one bus leaves the other stream sample-identical to neutral. A
second test uses distinct compressor settings per bus, then adds a
limiter to Bus 2. Each stream retains its own response. Disabled
settings survive with neutral PCM.

The same stereo bus profiles preserve all six EQ bands independently,
including filter types, frequency and knob values, Q, outer-band slope,
middle-band range, per-band enable and the bus EQ switch. The graph receipt
keeps each control on its bus and is checked against the saved numeric rows.
The reopened multitone controls verify independent processing and bypass
with saved settings intact. In the simultaneous six-band case, Bus 1
matched exactly across imports. Bus 2 initially differed by 0.248 dB RMS
between the reference and writer. A new import of the same reference
archive rendered at the writer's level, and the exported mixer bytes
remained identical. Exact PCM across imports is unqualified for that case.
The two observed
processing-order codes survive a round trip, and rendered PCM was identical
between those codes in the bus EQ and compressor fixture. The qualified bus
tables contain no track expander or gate rows.

The same profiles preserve the saved bus input-gain row (`0x019`)
from −6 to +6 dB in tenths of a decibel. This value has its own bus
receipt and storage location, separate from the bus fader and dynamics
makeup. Resolve rendered the expected gain on each assigned output.
drpkit labels this field pre-dynamics gain. With the tested compressor
settings, applying +6 dB here or at the fader produced identical PCM,
so this comparison leaves the processing position open.
[drpkit Fairlight field map](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/FORMAT.md#channel-items)
· [Pinned frequency ranges](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/drpkit/fairlight_map.yaml#L209)
