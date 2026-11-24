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

Same-rate 24 fps MOV and MP4 captures recognize a verified embedded 48 kHz
stereo PCM cache by its clock, extent, and channel references. Native split
mono edits, including the ProRes LT control, retain the independent audio
selectors and trims in graph/2.

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

A picture clip with saved effects and an absent `MediaRef` can remain in
graph/2 as a sequence-owned `resolve.unbound_picture` receipt. It retains
the saved track, record range, locator hint, and effect and time-map
evidence. Small blobs retain their saved bytes. Larger bounded blobs
retain their size and digest. The locator hint creates no Media Pool item
or placement, and an occurrence loss records the unresolved binding.

For the saved stereo camera and external mono PCM layout, the
`resolve.virtual_audio` receipt records both routes by cache and pool-item
identity. It retains the ordered physical channels, each cache's 48 kHz
clock and sample extent, the signed sample offset, and the saved virtual
selector. The external reference must resolve to one standalone audio
cache. `resolve.virtual_audio_edits` keeps the timeline track and each
occurrence's selector beside its `resolve.timing` receipt. The reader
reports unresolved playback timing and leaves audio placement pending.

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

An embedded-audio Group can also save one audio slot backed by a native
two-channel transport. The Group receipt preserves that slot and its
default virtual channel. Occurrence receipts keep the saved track index
and selected virtual channel separately when the edit uses the slot.
Slot zero can select either channel of that transport. Physical audio
binding remains unresolved until the saved Group mix is qualified.

Sequence markers preserve relative position, duration, title, note, all
16 Resolve marker colors, and saved custom data. The writer verifies
these values and the sequence owner before publication. The reader also
retains markers and custom data from shared picture-only 1080p ProRes HQ
items at 24 fps and at `24000/1001`, including verified embedded source
timecode. The linked writer
verifies the generated pool item's ownership and marker values.
Standalone mono and stereo PCM WAV items also preserve frame-aligned source
markers and custom data on their saved audio import clock.
The reader also binds markers on qualified straight-cut 1080p V1 edits
at those rates
to their saved edit UUID. Other source profiles remain open.
[Premiere marker palette](#resolve-to-premiere-sequence-marker-colors) ·
[Source marker qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-source-clip-markers) ·
[Edit marker qualification](#resolve-to-premiere-timeline-edit-markers)

Source and sequence lockable metadata accepts up to 256 KiB of stored Qt
bytes and 512 KiB per decoded body within the archive-wide decoded blob budget.
The reader checks at most 32,768 top-level fields and keeps the existing
marker table and row limits. An explicitly empty saved field yields an
owner-bound `resolve.lockable_metadata` receipt. A shared envelope with field 6
byte records leaves a receipt with non-marker field counts and SHA-256 digests
of the stored envelope and decoded body. The saved marker table supplies the
typed markers. Malformed nonempty encodings still fail validation.

The linked writer preserves matching audio and video timeline In/Out
marks, including In-only and Out-only selections. These are timeline-relative
frames, and Out is inclusive. The paired 24 fps control selected the same
35-frame native render after reopen. Independent audio and video marks
remain readable and block writing because fresh Resolve imports changed
those selections. Import the archive on the Edit page to preserve
frame-zero In marks in Resolve 21.2.
[Frame-zero qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#one-sided-and-frame-zero-marks)
· [Pinned pr2resolve format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md#颜色标记与编辑状态)
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
linear-amplitude fades retain whole or fractional head and tail lengths
as clip-edge metadata. The linked writer rebuilds default fades on
qualified 24 and 23.976 fps mono WAV edits and two-point volume curves on
the 24 fps profile. It checks the reconstructed controls on each clip
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
Both internal writers support the qualified static track pan on the linked
mono mixer. The Resolve writer checks the track and bus fader values and
mute against the saved mixer rows before recreating the panners.
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

## Saved embedded reel names

The reader retains the cached file reel in the clip-owned
`resolve.media_metadata` receipt. When the project uses the verified
embedded-reel conform mode and contains a cached reel, the Media Pool
root also owns a `resolve.reel_names` receipt with value `"embedded"`.
The cached file tag and the project setting are separate saved facts.
[Avid → Resolve Reel Name evidence](RESOLVE-PROJECT-CORE.md#visible-media-pool-reel-name)

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
code. Qualified piecewise Linear picture edits also compare retimed In
and every editable curve key under `edit.linear_retime`. A key outside
the visible cut can differ even when all sampled picture frames agree.
Four retained saved and reopened timelines matched. In a disposable
variant, changing the curve before the trim produced one curve difference
and preserved all 48 visible source-frame selections.
A code outside the known palette is a gap. Paired picture edits
also compare qualified input sizing under `edit.input_scaling`. The
native wide-raster control reports one change from `project` to
`center_crop` with the same trim and media. `project` denotes inherited
sizing. For same-rate picture with a different source raster on a 1080p
timeline, the probe also compares the inherited project setting when both
edits use it. The sequence receipt records the qualified fit or center
crop value. A missing default or a custom timeline setup leaves a gap.
The Resolve 21.2 native control changed only the project setting and
reported one inherited sizing change. An explicit clip-level Center Crop
overrode both project settings and matched. Reapplying the Inspector
control changed the opaque clip state while the effect payload continued
to identify the saved choice. The [linked writer](RESOLVE-TURNOVER-IMPLEMENTATION.md#automatic-native-media-import-fallback)
preserves inherited Center Crop as an editable clip setting for qualified
assisted picture edits. For paired picture
edits, the report also compares supported single-node primary gain,
equal RGB CDL slopes, and both recognized range shifts. It identifies
an added or removed grade and treats unknown grade controls as
inconclusive. A clip-owned Fusion composition retains its payload size,
hashes and active index when saved on an ordinary video edit. Unmapped
compositions contribute an occurrence-specific effect omission. The linked
writer can generate an editable Corner Positioner from qualified graph/2
targets. The reader retains the resulting composition receipt, and
staged read-back verifies the saved bytes on the exact edit.
[Static Corner Pin scope](SOURCE-GRAPH-MIGRATION.md#static-avid-corner-pin).
Paired audio edits compare saved Voice Isolation and
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
excluded. A qualified A1 crossfade can coexist with the V1 dissolve.
Only the V1 interval affects picture sampling.

At 24 fps, saved 50% forward, 200% forward and 100% reverse edits also
contribute source-frame samples. Double speed advances by two source
frames beginning at the exact trim. Half speed repeats each source picture twice
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

The file-based DRP graph reader checks single-disk ZIP32 archives up to
512 MiB on disk, 2 GiB expanded, and 4,096 members. It checks the raw
directory against the library's name index and reads in place. It indexes
sequence containers by saved identity, then loads one at a time. Project
and Media Pool XML stay available during interpretation, with a separate
256 MiB and 1.6 million node budget. Each parsed member can contain up to
128 MiB of XML and 1.6 million nodes. The full archive has a 25 million
node ceiling and XML depth remains limited to 96.

Per-field decoded data stays bounded at its existing limit. The DRP graph
reader caps aggregate decoded data charges at 384 MiB. Graph/2 output is
still bounded at 256 MiB encoded. The retained in-memory DRT and saved-blob
comparison paths have their smaller archive budgets. Unsupported native
objects receive scoped losses when the owning sequence or bin is known.
Broken required references, malformed encodings and unsupported project
versions fail before output.

Saved picture retains exact whole frame rates that fit the two-digit
timecode label, along with the witnessed fractional rates 24000/1001,
30000/1001, 48000/1001, and 60000/1001. The reader checks the actual
saved binary encoding and preserves those ratios in graph/2. The
30000/1001 form retains its saved drop-frame flag. Other drop-frame
variants and approximate decimal rates remain unqualified. Source rate
capture leaves destination retiming and media verification separate.

When `BtVideoInfo.Geometry` saves a type-135 `PixelAspectRatio`, the
video stream owns a `resolve.media_geometry` receipt with the exact
reduced fraction and the original binary64 bits. Stored raster dimensions
remain independent of this ratio. Non-square values carry a geometry
loss until a destination can verify the resulting framing.

The reader checks decimal media starts against the exact cached clock.
A rounded spelling must agree at its saved precision and distinguish
adjacent ticks of the owning clock. Edits with a saved fractional
`Start`, `Duration`, or `In` retain the original field, integer frames,
and an exact signed binary significand and exponent in
`resolve.fractional_timing`. Each receipt identifies the occurrence and
its record clock. Existing conform and PCM profiles keep their qualified
interpretation. Other subframe placements remain source evidence with
an omission report until their playback is qualified. A Group with a
fractional duration retains that exact duration and its saved extent
bytes. Its playback mapping remains unresolved.

### Standalone audio and stills

Standalone audio caches retain their saved paths, sample rates, exact sample
extents, and physical channel order for the mono and stereo WAV, AIFF, MP3,
and M4A profiles. The reader checks the stream and the virtual route against
each other. It keeps an unresolved PCM depth code as a saved fact. Same-rate
whole-frame occurrences with an identity time map carry their independent
source trims and saved channel selections. Unsupported edit maps and
selectors receive occurrence-level losses.

Qualified zero-origin 48 kHz PCM16 and PCM24 WAV caches also populate
graph/2's `OriginalStream` with the saved physical stream and channel,
sample clock, and extent. The Resolve receipt retains the virtual
selector. Archive capture leaves file availability unknown. Destination
preparation verifies the actual file before publication.

Embedded 44.1 kHz stereo AAC retains the saved sample origin and sample
count independently of the picture clock. The stereo pool route must
agree with that audio extent and bind each channel to the same saved
audio cache track. The sequence-owned `resolve.embedded_audio_routes`
sidecar retains the selected physical channels, track identity and saved
selector alongside the original `resolve.timing` observation. Current
coverage establishes the source association. These edits still carry a
loss for unresolved playback maps or track mixing. The item-to-file
offset remains unresolved until the original AAC stream is verified.

A literal one-frame JPEG, PNG, TIFF, or HEIC cache can be recognized from its
matching saved filenames and format. The saved cache start remains a native
observation. An edit with the verified constant still map reads as a hold of
the original file's first picture. Image ranges and other maps remain
unqualified. These source facts describe saved records. File availability,
decoding fidelity, and destination support require separate verification.

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
  retains the exact 23/48-second source span. Both destinations now write
  that span on the linked ProRes HQ MOV profile and verify all 12 sampled
  pictures are available. [Odd-length qualification](#odd-length-half-speed-picture)
  Both linked writers also support 200% forward on the qualified 24 fps
  ProRes HQ MOV picture profile. The Resolve reader preserves odd file
  trims from the saved half-frame In. [Premiere to Resolve](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-200-forward-picture-speed) · [Resolve to Premiere](#resolve-to-premiere-200-forward-picture-speed)
- **Plain captions:** One Resolve subtitle track with editable default
  styling maps to graph/2 `SourceText`. The saved XML and visible burn-in
  checks passed for two adjacent accented cues, one multiline, on a
  nonzero timeline start. Premiere saved caption text is supported as input;
  preserve destination defaults and report unmapped styling. Both cues
  were visible in the reopened Premiere projects. A retained Avid SubCap
  over linked OP-Atom at 24000/1001 mapped V2 to Resolve subtitle 1, with
  both saved lines and all 48 visible frames after reopen. More than one
  source caption lane requires a separate mapping.
- **Dissolve:** The linked DRP writer reads and writes whole-frame left
  or evenly centered V1 Cross Dissolves between plain edits at 24 or
  24000/1001. It also supports centered A1 0 dB and +3 dB crossfades at
  24 fps with verified WAV handles. The sequence receipt keeps the
  alignment, interval and endpoint IDs. The 24 fps native comparison
  verified repeated variable-length windows, 48 decoded frames and both
  isolated PCM lanes after reopen. Avid Blend Dissolve retains its
  eight-frame mapping. [Linked transition checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#variable-lengths-and-constant-power)
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
- **Editable multicam:** The [fixed-audio return](#resolve-to-premiere-editable-multicam)
  maps a saved two-camera Resolve Group to an editable Premiere source
  with two independent mono outputs from camera A.

### Resolve to Premiere editable multicam

The `resolve.multicam_tracks` source receipt preserves each saved picture and
audio angle track in order, with its UUID, activity flags, member references,
source coordinates, time maps, and saved audio selectors. Each multicam use
records its matching track UUID when its saved angle ID is unique within the
same track kind. Group output selectors and angle-member source selectors
remain separate. Known member selectors retain the channel numbers verified
against the source cache. Missing or unknown source and routing facts remain
explicit in the receipt.

Graph/2 can project a picture angle with multiple pieces when the track is
enabled and every member has a verified same-rate identity map, in-range trim,
and nonoverlapping frame-aligned bounds. It keeps the intervening gaps and
intersects each timeline use with the selected angle's pieces. A mono Group
with an explicit self-owned mono transport and matching virtual output can
also project same-rate mono WAV members with verified selectors and timing.
The internal Premiere export can rebuild the selected picture angles and one
fixed mono output from these graph bindings. It checks each selected edit
against the exact intersections of its source angle, then builds one editable
multicam source item per saved definition. The source retains its own origin,
canvas, multipart trims, and gaps. A timeline can use separate picture and
mono multicam items. The same-rate 24 and 23.976 route requires verified
linked originals and known sizing when the raster differs. A false binding,
overlap, missing file, or unknown geometry blocks the affected conversion.
Unresolved uses require `--accept-losses` before they may be left out.

#### Multicam alongside unsupported edits

The multipart Premiere exporter can preserve editable Group cuts, a verified
mono lane, and ordinary clips alongside an unsupported Resolve title, nest,
or native transition. With `--accept-losses`, Review and export identify
each omission by its occurrence ID, track, and exact record frame range.
Surviving edits keep their original positions. If the omitted edit defines
the output duration, or its saved track or range cannot be verified,
preparation refuses with the affected occurrence. The saved graph retains
the source records.

A controlled 24 fps graph replay covered all three omissions in one
timeline, with editable Group picture and audio beside an ordinary V1 clip.
The written project and its retained-edit control agreed on the surviving
placements. Premiere reopen and render comparison remain pending for this
combined case.

#### Preserve the verified picture angles of a partial Group

With loss approval, the multipart exporter can retain a Group's saved
picture selections when an unused angle has no verified source binding.
It checks each selected edit's saved angle track ID against the graph
alternative and names every omitted angle in Review. An unresolved or
contradictory selected angle blocks this path. This preserves the linked
edits and keeps the original angle and audio evidence in graph/2.

An unsupported audio transition can be omitted at its exact fractional
record range when the retained edits still cover the range. The omission
report keeps the saved fraction. Picture transitions, titles and nests
still use their whole-frame rules.

#### Custom multicam sequence canvas

The multipart exporter can write a custom canvas from the saved sequence
dimensions and exact record clock. An edit whose original has a
different raster needs a known Resolve input fit setting. The reader
keeps the inherited project setting or the saved version-three timeline
override in graph/2. Same-build controls distinguish Fit from Center
Crop even when the project default differs from the timeline. Unknown
overrides remain unresolved. Preparation uses the Premiere fit policy
on the affected occurrence, including inside a Group. The custom
Premiere canvas currently requires Fit for mismatched originals.

Premiere 27.0.0 build 88 saved and reopened the 3840×1920, 24 fps
control with both the outer and Group canvases intact. The 66-frame render
showed the expected centered picture, 60 active frames at their saved
positions, and the six-frame gap. The two output channels matched the
retained mono control for 96,000 sample frames, followed by the expected
silent tail.

The custom-canvas route also keeps static uniform zoom and translation
editable in Premiere Motion. Premiere's normalized position uses the
file raster. Preparation converts Resolve's fitted-image displacement
using the same fit factor as the clip. The 24 fps ProRes and RED
controls were saved, reopened and rendered in Premiere 27 and compared
with native Resolve frames. Geometry and trims agreed. Rendered
color and resampling can differ between editors.

#### Linked stills on 24 fps timelines

Graph/2 keeps a single physical frame and the saved hold for each still
occurrence. Premiere preparation extends the linked still's editable
range to cover those occurrences. It keeps the original path, bin item,
and record range. The same rule applies to ordinary clips beside
multicam. A raster mismatch uses the qualified timeline or clip Fit
setting. The physical check decodes a bounded opaque, square-pixel
8-bit RGB or RGBA PNG. It accepts untagged images and verified sRGB
metadata. Linked JPEGs require a bounded baseline JFIF image with one
scan, square pixels and no rotation or alternate color profile. The
writer reports a saved clip grade per still occurrence and requires
`--accept-losses` before writing the ungraded hold. The grade receipt
remains on the graph/2 edit. Transparency, animation, other profiles,
and unresolved framing remain open.

Premiere 27.0.0 build 88 saved and reopened a 3840×1920, 24 fps replay
with the 3500×2000 PNG still online. All 111 decoded frames held the
same image. The active picture edges matched a native Resolve render
on the same canvas. The two renders differed slightly in color and
resampling. A separate multipart graph replay retained its Group and
mono routing while writing a linked PNG after the gap.

A separate 2518×1416 JPEG replay kept its original path and all 167
hold frames after Premiere 27 Save As and reopen on the same 3840×1920
canvas. The image edges matched an ungraded Resolve control. The grade
omission was reported, and conversion without approval refused output.
Rotated, truncated and concatenated JPEG controls were also refused.

#### Groups with fractional saved extents

A Group can save a fractional duration separately from its whole-frame
playback endpoint. Graph/2 retains the exact duration and the tail gap.
The reader resolves a whole-frame edit when its saved outer map agrees
with the last complete frame, its source range fits, and the selected
angle has verified members. A known angle can retain its edit bindings
when another angle is unresolved. The incomplete definition stays
unqualified for editable export.

Audio selection still requires a verified physical route and member
clock. A stereo Group output with unresolved source routing and a
member with subframe timing stay explicit in the archive receipt. A
fractional-tail variant of the retained two-mono control kept all six
audio bindings and all three picture edits.

The multipart Premiere writer can use the complete-frame extent with
loss approval when every angle proves the fractional tail is empty.
It refuses a tail containing media or a selected use beyond the
complete-frame extent. The fractional-tail control retained its
editable cuts and mono lane after Premiere 27 Save As and reopen.
All 48 decoded picture frames and 96,000 PCM sample frames matched the
whole-frame control.

The internal `--premiere` probe also accepts one linked 1080p23.976 Group
timeline with neutral V1/A1/A2 and two full unretimed DNxHR LB OP1a camera
sources. Each original has two independently verified 48 kHz PCM24 mono
streams. Preparation checks the saved Group and leaf trims, exact cut
times, media clock, original paths and reference audio selectors. The
Premiere writer uses the same reusable multicam source for all nine
occurrences. The other camera's audio is muted inside that source.

Premiere 27.0.0 build 88 upgraded the generated project and retained its
A/B/A angles and two fixed camera-A mono lanes after Save As, close, and
reopen. All 145 decoded picture frames and both isolated 290,290-sample
48 kHz PCM16 exports matched the original Premiere control exactly.
Graph/2 recapture and regeneration of a Resolve project agreed on all
nine placements, including the timeline origin, file-relative trims and
physical mono source paths and selectors.

A fresh insertion of Group frames 24–168 also kept both mono lanes after
Save As and reopen. Each appended lane matched the corresponding existing
lane sample for sample. Premiere selected camera B on the overlap and
fell back to camera A for the remaining 24 frames. Resolve's save leaves
the Group-wide picture default unknown, so the probe requires
`--accept-losses` for Premiere's default on fresh insertion. Existing
edit selections remain explicit. Independent audio-angle switching and
a linked AAF comparison remain open.
[Forward qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-editable-multicam)

#### Resolve reference-audio switch and fresh insertion

On Resolve Studio 21.2.0 build 00006, the retained Reference Audio Group
accepted a native Angle 2 selection on the middle A1 edit alone. After
export, import as a separate project, save, and reopen, the angle label
survived. All 145 picture frames and the entire isolated A2 render
matched the original. A1 still used camera A, with differences confined
to 143 sample frames around each cut. The reader now binds that edit to
the proven reference source and preserves the selected angle in its
occurrence receipt. Destination export reports the unqualified cut-edge
envelope at that occurrence.

A fresh Resolve insertion appended Group frames `[24, 168)` on V1 and
both mono lanes. After reopen, all 144 appended picture frames selected
camera A. Both 288,288-sample append renders matched their respective
reference signals except for the final 71 sample frames at the new
timeline end. Source Audio mode remains a separate case. The tested
single-field Angle 2 archive variant reverted to Angle 1 on reopen, and
its rendered A1 and A2 matched baseline. A valid per-angle control must
select camera B on A1 while preserving the picture and camera A on A2.
[Blackmagic Reference Audio guide](https://documents.blackmagicdesign.com/SupportNotes/DaVinci_Resolve_20_New_Features_Guide.pdf#page=56)

### Premiere 27 project version

The internal Resolve → Premiere writer emits Project version 46 for
ordinary and multicam output. Premiere 27.0.0 build 88 opened the generated
projects directly. Save As and reopen retained the repeated and variable
transition windows and the three picture and six audio edit occurrences
in the multicam control. All decoded picture frames and both isolated
PCM renders matched the earlier Premiere controls. The unedited files
also round-tripped through the Premiere → Resolve saved-job reader.

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

The ordinary and multipart multicam writers read MOV stream metadata
before verifying the packet clock. The packet verifier checks the count
and cadence in one pass. These paths verify the saved locator, source
clock, extent, and channel layout, then recheck file identity and state
before publication. A linked original can exceed 2 GiB. A synthetic
ProRes HQ MOV with a large QuickTime free-space atom exercised this path
in Premiere 27 after Save As and reopen. A missing packet or a file
changed during verification refused publication.

#### Link 44.1 kHz mono MP3 in Premiere

The same internal route can link a standalone, single-stream mono MP3
at 44.1 kHz with zero decoder delay and padding. It checks the saved
locator, exact sample clock, channel zero and decoded extent. The
qualified saved cache can be up to three samples short of that extent.
Premiere uses the verified playable extent for the master and keeps
each original source trim in exact ticks, including half-sample
positions on the 44.1 kHz clock. No resampling offset is added to the
edit. Truncated files, contradictory selectors or extents, and MP3s
that require decoder delay or padding adjustments remain refused.

Premiere 27.0.0 build 88 saved and reopened a 24 fps control with two
trimmed MP3 edits and an independent WAV lane. Both isolated 94,000-frame
PCM renders matched a separate native Premiere import and exact-trim
control byte for byte. The MP3 waveform led the Resolve reference by two
48 kHz samples in both Premiere renders. The WAV control aligned at
zero. Stereo MP3, attached artwork, and nonzero decoder delay remain
open.

### Link RED2 picture to Premiere

The internal Premiere writer links same-rate 5120×2700 RED2 picture at
24 or 24000/1001. A 1920×1080 timeline can use the saved Resolve input
fit to map the source raster at its correct aspect ratio. The reader
verifies the first R3D part's time-of-day timecode and
checks the part number, clip identity, rate, raster, and accumulated
picture extent through the recording's closing footer. The saved
Resolve clock and extent must agree with the recording. Graph/2 carries
the verified source clock separately from each file-relative edit trim.
Premiere opens the original at part `_001`. Any NLE checks the state of
every part before publishing the project.

Premiere 27.0.0 build 88 preserved online links, both trim endpoints,
and the exact alternate source timecode after Save As, close, and
reopen. All 54 decoded frames matched native Premiere controls across
one single-part and two spanned originals. One edit crosses from the
first part into the second. All 18 frames also matched in the separate
1080p fit control. The comparison uses Premiere's default RED decode.
Embedded RED audio, mixed-rate RED conform, and transferring custom
RAW settings remain separate work.

### Review a project for Premiere

```text
resolve-source-probe --premiere-review INPUT.drp FFPROBE REPORT.json
resolve-source-probe --premiere-review INPUT.graph.json FFPROBE REPORT.json
```

Review uses the internal Premiere writer to prepare one timeline at a
time in saved order. It lists timelines that can be written, timelines
that carry reported losses, and each blocked timeline's first refusal.
Original verification failures include the saved locator and the media
or camera owner. The report includes each attempted original, its
verification result, and the number of times other preparations reused it.

Review and publication use the same preparation. They read the shared
project and copy the selected writer inputs, including media used only
inside multicam angles. For linked PCM WAVs, a saved graph can supply
the physical channel through its typed original or the older Resolve
selector receipt. Present fields must agree with each other and with
the verified file. The destination track number does not select a channel.

The job shares physical evidence for the same original, including
failed inspections. Ordinary files use their resolved paths. A spanned
RED original also depends on the saved recording directory and sibling
names. Each timeline still checks its own saved source clock, range,
and channel claims, and the writer rechecks the file state. Overlapping
requests for incompatible verification profiles are reported as
conflicts. Originals behind an earlier timeline refusal are left
uninspected. Packet inspection of long files can still take time on its
first pass. Review publishes only the JSON report to a new path.

Use `--premiere` with the reported sequence ID to write a new Premiere
project. That separate job rechecks the originals. See the loss approval
rules below.

### Linked Premiere review and omissions

The probe reports unqualified timeline In/Out selections and unrecognized
clip label codes before creating a project. Use `--accept-losses` to
proceed with those omissions. The source graph retains the original values.
Timing, media, routing, and unsupported processing still use the normal
refusal checks. [Timeline In/Out mapping](#resolve-to-premiere-timeline-inout-marks)

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

### Resolve to Premiere timeline In/Out marks

Matching saved Resolve audio and video In/Out marks map to Premiere's
sequence marks. Both use timeline-relative time. Resolve's Out is the
last included frame, so Premiere receives the following frame as its
exclusive Out. The mapped values retain a frame-zero In separately from an
unset In. One-sided selections stay one-sided. Independent audio and video
marks still require approval before omission. Unknown mark fields and
mapped values beyond the destination extent refuse before publication.

On a 24 fps sequence starting at 01:00:00:01, all six marked controls
retained their exact marks after Premiere 27.0.0 build 88 Save As and
reopen. The selection from Resolve frames 2–13 yielded 12 picture frames
and 24,000 stereo PCM24 sample frames. The checks also covered In only,
Out only, a frame-zero In, and a one-frame range. All exports matched
the corresponding decoded frames and PCM in the unmarked Premiere
control. The corresponding Resolve renders selected the same source
frames. Converting the reopened Premiere projects back to Resolve
preserved the mark values on archive read-back.

A separate 23.976 test added matching graph/2 marks to a retained linked
timeline. After Premiere reopen, the marked export matched 12 decoded
picture frames and 24,024 stereo PCM24 sample frames from the unmarked
control, including the cut.

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

All four controls passed Save As and reopen in Premiere 27.0.0 build 88.
Saved read-back kept 16 labels on their individual V1 and A1 edits, eight
neighbors with default labels, and the shared source masters' default
labels. The original links, visible edit ranges and physical channel
selectors agreed with graph/2. An unknown clip label required approval
and used the default for just that occurrence. Visual palette display
and changing a timeline label in the UI remain open. Timeline In/Out and
Media Pool label mapping remain separate.

### Resolve to Premiere track state

The internal linked V1/A1/A2 route uses the selected sequence's
`resolve.track_state` sidecar. It writes each custom name, edit lock,
and V1 output state on the matching Premiere track. Names may repeat
across lanes. Turning off V1 writes the Premiere video track output
toggle and keeps all its clips. It does not write an audio fader mute.

Premiere 27.0.0 build 88 retained the names, including the duplicate
audio track name, and the independent V1 and A2 locks after Save As
and reopen. A neutral project and three state controls kept the same
01:00:00:01 start, six edit ranges, original paths and WAV selectors.
Both disabled V1 projects rendered 48 black frames. Re-enabling the
locked V1 and reopening again restored all 48 Premiere picture frames
exactly, with the lock still set. The enabled picture selected the
same source frames in Resolve and Premiere.

Both isolated Premiere lanes matched all 96,000 selected WAV samples
exactly with digital silence on the opposite output channel. Resolve's
state variants also matched their own neutral render exactly. The neutral
Resolve render had a 0.1 dB lower A1 level and differences within 72
samples of the edit boundaries. These differences also occurred with no
track-state changes.

Conflicting timeline and mixer state, unsupported audio mute combined
with lock, and the two-bus profile still block output.

### Linked picture frame holds

The internal Premiere source capture and Resolve reader preserve a whole-edit
frame hold as a constant graph/2 `frame_map`. This changes which source picture
is selected at every record frame while leaving the record edit range intact.
Premiere Source Timecode holds retain the saved underlying trim separately.
Resolve's witnessed two-point nearest-frame freeze exposes the held picture
as its source In. Both internal writers accept holds at the edit's source In
on plain picture-only V1 at 1080p24 with a verified ProRes HQ MOV and a
zero-relative source clock. Premiere's underlying trim must fit the original.
[Adobe Frame Hold Options](https://helpx.adobe.com/ca/premiere/desktop/edit-projects/change-clip-speed/freeze-a-video-frame-for-the-duration-of-a-clip.html)

Premiere 27.0.0 build 88 saved and reopened 24-frame holds on source pictures
12 and 17 with the same underlying trim starting at 12. The source capture
preserved both distinctions. A saved job and a graph replay of the hold on
picture 12 reopened in Resolve Studio 21.2.0 build 00006 with editable 0%
speed, and all 24 decoded frames matched the native Resolve hold pixel for
pixel. The native Resolve split control also preserved five moving frames
followed by 18 held frames.

When a held Premiere frame differs from source In, the Resolve turnover job
reports that Resolve will use the held frame as source In. Publishing that
job requires the existing `--accept-losses` approval. Graph/2 retains
Premiere's original In separately from the held frame. Two Premiere saves
held frame 17 and kept their original trim at frame 12. Both converted clips
reopened in Resolve at 0% with their original timeline origins and exact
24-frame record spans. Their picture selections matched Premiere, and both
decoded renders matched the native Resolve frame-17 control pixel for pixel.
The job refused publication without approval. The raw linked-graph probe
continues to require source In to match the held frame.

For a Resolve freeze, the held picture must exist and the record span can
extend past the source's playable tail. A Premiere project hold on frame 95
of a 96-frame file reopened in Resolve at 0% with all 24 intended frames.
A graph replay holding the same picture for 120 record frames also reopened
and rendered correctly. Holds selecting frame 96 and ordinary moving trims
past the file end were refused.

Resolve → Premiere writes the native Source Timecode Frame Hold on the placed
clip. After Save As and reopen in Premiere 27.0.0 build 88, all 24 frames of
the whole-clip hold selected source frame 12. The split edit preserved five
moving frames followed by 18 frames held on source frame 17. Both timelines
kept their exact starts and edit bounds, and graph/2 recapture agreed at every
frame. On a disposable copy, changing the hold from source frame 12 to 17
survived another save and reopen with the source In and edit bounds intact.
Both original Resolve archives now convert through their captured graphs
without fixture edits. The converter reports omission of the empty default
stereo track when the saved track layout and complete Fairlight model match
the verified controls. A same-rate clip with an identity time map needs no
extra effect for the saved nearest-frame sampling setting. Changed stereo
faders and a named empty track were refused. Both converted projects matched
all 47 frame selections again after Premiere Save As and reopen.

Premiere needs a playable underlying trim for the full duration of a hold.
A trial 24-frame edit starting at source frame 95 displayed the held picture
for only one frame. The converter can now move the underlying trim to the
latest full-length window inside the original and preserve the held source
timecode separately. The `--premiere` probe reports the new source In and
requires `--accept-losses` for this adjustment. The captured Resolve graph
keeps its original source In.

The 24-frame Resolve hold on frame 95 reopened in Premiere with underlying
source range `[72, 96)`. The timeline start and both record bounds were exact.
All 24 source-frame selections matched Resolve. The decoded render also
matched the retained Premiere control pixel for pixel. Changing the held
picture to frame 94 on a disposable copy survived Save As and reopen with
the same clip trim and edit bounds. A return conversion from the reopened
Premiere project agreed on the sequence origin, record bounds, and all 24
frame selections in Resolve archive read-back. The 120-frame hold was
refused because the original contains 96 frames.

Other Premiere hold modes and one-frame holds remain separate profiles.

### Resolve to Premiere mixed-rate Frame Sampling

For the Premiere-origin profile, the internal `--premiere` writer preserves
an explicit graph/2 map when every entry agrees with Frame Sampling at the
saved source In, source clock and record clock. The editable edit
uses the verified ProRes HQ original at its native rate. Both
independent mono WAV edits keep their own 48 kHz source trims.
A different map, an unverified link, or an out-of-range trim
blocks publication with the affected edit identity.

Premiere 27.0.0 build 88 saved, closed and reopened three projects
from Resolve Studio 21.2.0 build 00006 archives. The 1080p24
timeline had controls with zero and positive trims on 23.976 picture.
The 1080p23.976 timeline had a trimmed 24 fps shot. Each timeline
matched all 1,075 numbered source frames with its separate original
WAV paths, exact edit ranges and non-drop timeline start. Both isolated
PCM24 lanes matched the respective native Premiere controls exactly
over 2,150,000 sample frames per lane at 24 fps and 2,152,150 at
23.976 fps. Reopening and recapturing the projects reproduced the
source-frame maps in graph/2.

The Premiere-origin profile uses plain V1 Frame Sampling and verified
1080p picture-only MOV. Zero source In in the Premiere-origin 24 fps
picture case, Frame Blending, Optical Flow, and the linked AAF comparison
remain separate. [Premiere → Resolve control results](RESOLVE-TURNOVER-IMPLEMENTATION.md#mixed-rate-native-comparison)

#### Resolve-authored default conform

A Resolve Studio 21.2.0 build 00006 project authored with the default
Nearest interpolation repeats 23.976 source frames at a different phase
on a 24 fps timeline. The reader qualifies the compact map against the
original media clock and verifies the project interpolation and the absence
of a timeline override. Graph/2 keeps the original whole-frame file trim
and the resolved frame selections. Five native trims at file frames 0, 1,
24, 77 and 1,000 matched every rendered selection.

The Premiere writer verifies the full map and advances the picture
source In by 0.002 of a source frame in its saved clip. Premiere retains
an editable ordinary Frame Sampling clip with the original linked media,
and the native trim remains exact in graph/2. An unadjusted Premiere
trial using the native trim selected four wrong frames.

Premiere 27.0.0 build 88 Save As and reopen matched all 1,075 source
selections in each of the zero, 24-frame and 1,000-frame trim cases.
Both mono lanes retained separate WAV links and 4,000- and 6,000-sample
trims. All 2,150,000 PCM24 sample frames per lane matched the native
Premiere controls exactly. The Resolve and Premiere exports agreed on
sample extent and impulse positions. Their stereo-bus levels differ by
about 0.0103 dB on this fixture. The reader reported the project's
Frame Blend copy as unqualified and the writer refused it. The writer
also refused a contradictory frame map.

**24 fps picture on a 23.976 timeline.** With the same default Nearest
settings, the reader converts the saved record-frame In plus each edit
offset to seconds, multiplies by 24, adds 0.002 of a source frame and
rounds down. The saved double and order of operations matter at a skip.
Five trimmed controls and a 20-edit, 26,327-frame native montage agreed
with the graph/2 frame selections. Graph/2 also preserves the exact
whole-frame source trim.

For ordinary Frame Sampling, Premiere preparation verifies the whole
map against a bias of either 0.0015 or 0.0025 of a source frame in the
written picture In. The unadjusted frame-23 trim picked two wrong
frames. After Save As and reopen, Premiere 27.0.0 build 88 selected
the correct source frame at all 1,074 positions in each of three
controls with file trims of 0, 23 and 1,000. The two mono WAV lanes kept
independent 4,004- and 6,006-sample trims, and each matched the Premiere
PCM24 control over 2,150,148 sample frames. The Resolve exports agreed
on sample extent and impulse positions. Their stereo-bus levels differed
from Premiere by about 0.0103 dB.

When a single Frame Sampling phase cannot match every skip, Premiere
preparation can use editable Time Remapping on this picture-only 1080p
ProRes HQ profile. It verifies the whole map against the saved Resolve
timing receipt and bounds it to 16,384 record frames per edit. A native
control that skipped at edit frames 998, 1999 and 2999 matched all 3,113
timeline frame selections after Save As and reopen. Both WAV lanes kept
their trims and matched the Premiere PCM control while active. The
remaining 4,082,078 sample frames per lane were silent in both editors.

**25 fps picture on a 23.976 timeline.** The same native Resolve rule
matched the saved zero and whole-frame source trims for linked 1080p
ProRes HQ. The saved compact map has an exact clock ratio of
`1001/960`. Graph/2 preserves the exact file trim and every sampled
frame. Premiere preparation verifies that the whole map can be
reproduced with a 0.0015-source-frame advance in its written picture In,
using ordinary editable Frame Sampling.

After Save As and reopen in Premiere 27.0.0 build 88, all 6,185 frame
selections matched Resolve Studio 21.2.0 build 00006 across three
1,073-frame controls (trims 0, 23 and 1,000) and one 2,966-frame
control. Both linked mono WAVs retained their independent 4,004- and
6,006-sample trims. Each matched the Premiere PCM24 control over
2,148,146 sample frames in the shorter edits or 2,150,148 in the long
edit. The long edit's 3,787,784-sample tail stayed silent in both
editors. The measured stereo-bus level difference was about 0.0103 dB.
A native Frame Blend copy and a contradictory frame map were refused.
This profile qualifies Resolve-authored Nearest conform for Premiere
output.

**29.97 fps picture on a 23.976 timeline.** The default Nearest rule
also matched a source at exact `30000/1001` on a `24000/1001`
timeline. The five-to-four ratio yields source offsets 0, 1, 2, 3, 5
from the saved file trim and repeats that cadence. Graph/2 preserves
the exact trim and sampled positions. The Premiere writer verified the
whole map and kept the original picture In with editable Frame Sampling.

After Save As and reopen in Premiere 27.0.0 build 88, all 5,735 frame
selections matched Resolve Studio 21.2.0 build 00006. This covers
three 1,073-frame controls with file trims 0, 23 and 1,001, and a
2,516-frame control with trim 23. Both original mono WAVs kept their
4,004- and 6,006-sample trims and matched the isolated Premiere PCM24
control over 2,148,146 sample frames in the shorter cuts and
2,150,148 in the longer cut. Its 2,886,884-sample tail was silent
in both editors. Resolve had the same impulse positions and measured
about 0.0103 dB higher at its stereo bus. Frame Blend and a
contradictory saved graph map were refused.

**Timecoded camera AVC with embedded PCM.** The same 24-to-23.976
Nearest rule supports progressive square-pixel 4096×2160 QuickTime
AVC in 8-bit 4:2:0 or 10-bit 4:2:2 with one stereo 48 kHz PCM16
stream. The tested Resolve timelines used two mono edits from that
stream and inherited Scale entire image to fit. The source graph retains
the exact file trim and frame map, the camera clock, the input scaling
witness, and the saved left and right channel selectors by stream.

Premiere preparation checks the picture and sample clocks and packet
extents, matches the saved clock to the MOV timecode track, and
rechecks the original file state before publishing. The original stays
linked in place. Premiere uses editable Frame Sampling and a 46.875%
centered fit on the 1920×1080 timeline. The two source channels retain
their independent mono edits and native big-endian PCM representation.

Premiere 27.0.0 build 88 Save As and reopen preserved the camera
timecodes, 1,048-frame edit ranges and the frame skip at record frame
998 in both original clips. Each mono render covered 2,098,096 samples
from file sample 46,000. The 10-bit original's two active channels
matched their respective PCM slices after accounting for Premiere's
center pan. The original 8-bit excerpt stayed silent on both channels,
and a private 8-bit control with distinct tones verified channel order.
Contradictory timecode, channel selection and changed input scaling
were refused.

### Odd-length half-speed picture

The internal Premiere and Resolve routes preserve 50% speed with an odd
record duration for same-rate 1080p24 ProRes HQ MOV picture-only edits.
Graph/2 keeps `source_duration` at half the record duration. A cut of 23
record frames needs 12 complete source pictures. Premiere Frame Sampling
retains the exact half-frame Out in the saved file coordinates. Resolve
keeps the whole-frame In on its stretched clock and uses the saved record
duration to end the nearest-frame edit.

Resolve Studio 21.2.0 build 00006 and Premiere 27.0.0 build 88 reopened
the 23-frame trim, a 25-frame trim ending on the last file picture, and a
one-frame cut using that picture. All 49 odd-length picture selections
and all 24 even-length control selections matched. The reopened Premiere
projects recaptured the same trims and speed through graph/2. Their
saved-job outputs reopened in Resolve and matched the graph-driven
Resolve renders pixel for pixel. A trim beyond the file, a contradictory
span, and Premiere Frame Blending were refused.

### Resolve to Premiere 200% forward picture speed

The internal linked Premiere writer uses graph/2 `Speed` with ratio `[1, 2]`
for the same-rate 1080p24 ProRes HQ MOV picture-only profile. Premiere
Frame Sampling uses the unscaled file-relative In and Out. The verified
source span must cover exactly twice the record duration.

Premiere 27.0.0 build 88 retained 200% speed, the original file link,
and the `01:00:00:00` timeline start after Save As, close and reopen.
Three numbered controls used file ranges `[12, 60)`, `[13, 61)`, and
`[72, 96)`. All 60 rendered frame selections matched Resolve, and the
decoded frames matched the retained Premiere controls pixel for pixel.
Graph/2 recapture from the reopened projects confirmed the file and frame
mapping for every record frame. A contradictory span, a trim past the
verified file extent, 300% speed, and an occupied output were refused.
The retained 50% and reverse controls kept their saved Premiere ranges
and speed settings.

### Resolve to Premiere sequence marker colors

The internal linked Premiere writer uses Premiere's named colors for Resolve
sequence markers in Green, Red, Blue, Cyan and Yellow. It maps Lavender to
Magenta, Sand to Orange, and Cream to White, reporting each substitution by
marker occurrence. Other Resolve colors use Premiere's default and are reported.
The graph keeps the original Resolve palette name and any saved custom data.
Premiere receives the marker's title, note, record-relative start, and saved
duration, including Resolve's one-frame duration for a point marker.

In Premiere 27.0.0 build 88, both 24 fps controls kept their 01:00:00:01
timeline start and all five and fourteen sequence markers after Save As
and reopen. The saved values and Premiere marker API agreed on all eight
color slots, the exact starts and durations, and the accented titles and
multiline notes. Each marker stayed on the timeline. The picture and both
mono audio lanes stayed online with exact trims and channel selectors.

Editing a marker's title, note, color and range in Premiere survived a
second reopen. Premiere's marker API stored the edited end one tick before
the requested frame boundary. The converted ranges retained exact frame
durations.

#### Resolve-only timeline marker colors

Pink, Purple, Fuchsia, Rose, Sky, Mint, Lemon, and Cocoa display as green
in Premiere. The internal writer preserves up to 128 original colors per
timeline in `AnyNLE.Resolve.SequenceMarkerColors`, tied to the timeline UID
and the saved marker GUID. Both Premiere readers record the original,
fallback, and current colors under `premiere.sequence_marker_color_provenance`.
Graph/2 restores the original color when the saved fallback still matches.
A different saved color takes precedence. The marker's current text and
range come from its saved Premiere fields. Ambiguous or invalid provenance
is refused. A color that cannot be attached is reported by occurrence.

At 24 fps with a `01:00:00:01` start, Premiere 27.0.0 build 88 kept the
property and all 13 timeline markers after Save As and reopen. Resolve
Studio 21.2.0 build 00006 reopened the eight exact Resolve colors. In a
second control, a Premiere recolor to Red took precedence, and a separate
rename and note edit kept their revised text and original Mint color.
The original five timeline markers and distinct source and edit marker
owners survived. Both returned projects matched 48 decoded picture frames
and 96,000 PCM24 samples per isolated mono lane.

### Resolve to Premiere sequence marker custom data

The internal writer stores bounded custom data in
`AnyNLE.Resolve.SequenceMarkerData` on the Premiere sequence. Version 1
identifies the sequence by UID and stores each value as hex-encoded UTF-8
under the saved marker GUID. The limit is 1,024 bytes of custom data per
sequence. Values that cannot be attached or exceed the budget stay in
graph/2 with an occurrence-specific advisory. Both Premiere readers check
the sequence UID and bind each payload to one qualified timeline marker by
GUID. The Resolve writer restores the value from the resulting
`premiere.sequence_marker_custom_data` receipt and verifies it on staged
archive read-back. Malformed or stale records are refused.

Premiere 27.0.0 build 88 kept the property after Save As and reopen. An
internal Resolve → Premiere → Resolve round trip restored two distinct
UTF-8 values to timeline frames 0 and 24 of the 24 fps fixture. All five
timeline markers and the existing edit markers survived. The saved DRP and
the Resolve Studio 21.2.0 build 00006 API agreed after reopen. All 48
decoded picture frames and 96,000 PCM24 samples per channel matched the
native Resolve control.

### Resolve to Premiere source clip markers

For linked 1920×1080 picture-only ProRes HQ MOV, the Premiere writer places
Resolve Media Pool markers on the shared project item. The qualified clocks
are 24 and `24000/1001` fps with verified embedded timecode or zero source
origin. The writer keeps the
item-relative starts, names, notes, colors, and durations, including
Resolve's one-frame duration for a point marker. An unknown saved color
uses Premiere's default and is reported. Unresolved source marker clocks
still block this route.

Premiere 27.0.0 build 88 kept both source markers after Save As and
reopen. A marker at source frame 2 stayed before both trims. A seven-frame
range starting at frame 10 kept its accented title and multiline note.
Both picture edits kept their shared project item and exact trims. The
five sequence markers stayed on the timeline. The reopened marked and
unmarked Premiere controls matched all 48 decoded picture frames and
96,000 PCM24 samples per channel. Graph/2 recapture confirmed the original
media paths and physical audio channel selections. Converting the reopened
Premiere project back to a Resolve archive preserved both source markers,
the five sequence markers, and both trims on read-back.

#### Source marker palettes

Green, Red, Blue, Cyan, and Yellow map by name on both routes.
Resolve Lavender, Sand, and Cream use Premiere Magenta, Orange, and White.
The return mapping uses the corresponding Resolve color. Resolve Review
requires approval for each writable substitution. An off-grid marker is
reported as an omission. Other Resolve colors appear in Premiere's
default green and are reported by occurrence.

The `24000/1001` native control used eight markers on a linked ProRes HQ
picture and eight on a shared stereo PCM16 WAV. Premiere 27.0.0 build 88
reopened both source items with all eight palette slots. Resolve Studio
21.2.0 build 00006 reopened all 16 returned markers with their original
Resolve colors, timing, ranges, comments, and custom data. The separate
mono lanes retained their trims and channel selectors. All 48 picture
frames and 96,096 PCM samples per isolated mono lane matched the native
Resolve control.

#### Resolve-only source marker colors

Pink, Purple, Fuchsia, Rose, Sky, Mint, Lemon, and Cocoa can round-trip
through Premiere's default green. On qualified ProRes HQ and PCM WAV
source items, the writer stores up to 128 colors per item in
`AnyNLE.Resolve.SourceMarkerColors`, keyed by the owning item UID and the
marker GUID. The property stores the fallback displayed in Premiere.
On capture, graph/2 restores the original Resolve color when that
fallback is still the saved color. A different saved Premiere color
takes precedence. The item-owned
`premiere.source_marker_color_provenance` receipt records both values and
whether restoration occurred. Colors that cannot be attached are
reported by occurrence. Malformed provenance or an ambiguous marker
identity stops capture.

The `24000/1001` control carried all eight colors on a linked ProRes HQ
picture and all eight on a stereo PCM16 WAV shared by two mono lanes.
Premiere 27.0.0 build 88 kept both properties through Save As and reopen.
Resolve Studio 21.2.0 build 00006 reopened all 16 exact colors and marker
details. A second native Premiere save changed two markers to Red and
Blue, and both colors took precedence on return. Serialized graph/2
replay kept the same Media Pool marker receipts. Both returned projects
matched all 48 decoded picture frames and all 96,096 PCM samples per
isolated mono lane.

#### Media Pool marker custom data

The writer stores up to 1,024 bytes of UTF-8 custom data per shared source
item in `AnyNLE.Resolve.SourceMarkerData`. Version 1 binds each value to the
saved item UID and marker GUID. Both Premiere readers validate these IDs and
capture the values in a `premiere.source_marker_custom_data` graph/2 receipt
on the owning item. Resolve preparation restores the data to that item's
Media Pool markers and the writer checks it on staged archive read-back.
Payloads outside the supported profile or over the write limit remain in
the source graph with an occurrence-specific advisory.

In the 24 fps trial, Premiere 27.0.0 build 88 kept the property after
Save As and reopen. The Resolve Studio 21.2.0 build 00006 return restored
distinct UTF-8 values
to source frames 2 and 10 on the one shared Media Pool item. It kept the
separate edit and timeline marker data on their own owners. After reopening,
the exact trim and stream assignments survived. All 48 decoded picture
frames and 96,000 PCM24 samples per isolated mono lane matched the native
Resolve control. Invalid hex, unknown or duplicate GUIDs, a property
moved to the edit copy, and excess bytes were refused.

#### Exact 23.976 source marker clock

The same round trip on a `24000/1001` ProRes HQ source kept a one-frame
marker at source frame 2 and a seven-frame range at frame 10. Premiere 27
Save As and Resolve 21.2 reopen preserved both custom values on the shared
source and a third value on the timeline marker at record frame zero.
All four edits kept their exact trims and physical stream assignments.
The 48 decoded picture frames and 96,096 samples per isolated mono lane
matched the native Resolve control. A duration of seven frames at
24 fps is off-grid at `24000/1001`. Review reported the range by
occurrence and required loss approval before writing the point and its
custom data.

#### Embedded source timecode

For the 24 and `24000/1001` picture-only profiles, the converter checks the ProRes
picture and matching QuickTime timecode track before using a nonzero
source start. Premiere capture also checks the saved media origin against
the file. The Resolve writer puts that origin in the Media Pool and timeline
media references. Graph/2 keeps source markers and trims file-relative.

With source timecode `10:00:01:00`, Premiere 27.0.0 build 88 retained
markers at source frames 2 and 10 after Save As and reopen. Resolve
Studio 21.2.0 build 00006 reopened the returned project with both values
on the shared item, the same timecode, and picture trims starting at
frames 5 and 39. The one-frame point and seven-frame range survived.
All 48 decoded frames and 96,000 samples per isolated mono lane matched
the native Resolve control. A contradictory saved Premiere origin and a
contradictory graph source origin each refused publication.

#### Standalone WAV source markers

The Resolve reader uses the saved audio import rate for Media Pool
markers on zero-origin mono and stereo 48 kHz PCM16 or PCM24 WAV. Both editors'
portable markers stay in file-relative time. Resolve writing and archive
read-back use the destination WAV's import clock at 24 or `24000/1001`.
Premiere stores custom data on each WAV's own project item.

Two mono WAV timelines qualified the round trip in Premiere 27.0.0 build
88 and Resolve Studio 21.2.0 build 00006. At each rate, a point and a five-frame
range survived on each of the two WAVs, with distinct names, colors,
comments, and custom data. The original trims and channel selectors
survived. All 48 decoded picture frames matched each native control,
as did 96,000 PCM samples per isolated lane at 24 fps and 96,096 at
`24000/1001`.

Premiere also retained two WAV markers one audio sample apart. An
off-frame marker stayed exact in graph/2, and Resolve Review required
approval to omit that occurrence. The other three WAV markers kept
their owners and values.

The stereo split-channel controls used exact `24000/1001`. The left
and right lanes retained distinct trims and physical selectors on one
shared WAV item after Premiere Save As and Resolve reopen. Each source
kept one point, one range, and their custom data. For both PCM depths,
all 48 picture frames and 96,096 samples per isolated mono lane matched
the native Resolve control. Review listed the off-grid stereo marker
once for the shared source.

### Resolve to Premiere timeline edit markers

For a straight V1 edit of linked picture-only 1080p ProRes HQ MOV
at 24 or `24000/1001` fps, the Resolve reader uses the saved edit UUID
and file-relative source frame. Both rates support verified embedded
timecode.
The record position is the edit start plus
the source marker position minus the source in-point. Graph/2 keeps both
positions, exact range, owner, color, and an occurrence receipt with
Resolve custom data. A marker outside the visible trim or an unqualified
clock retains its frame receipt and needs loss approval for Premiere export.

Premiere places the marked edit on a separate project item referencing the
same original file. After Save As and reopen in Premiere 27.0.0 build 88,
only the intended cut carried the two edit markers. The original source
and sequence markers also survived. The marked cut retained its label
with the reported Orange-to-Mango substitution. Graph/2 records Resolve
custom data independently of the editable Premiere marker. The marked and
neutral controls matched all 48 decoded
picture frames and 96,000 PCM24 samples per channel, with equal trims,
paths, and physical channel selections.

New internal conversions write a versioned property on the dedicated
Premiere item. The property identifies the original project item,
sequence, edit start and trim, and the GUIDs of the saved edit markers.
Premiere 27 kept the property and those IDs through Save As and reopen.
A second reopen preserved the same property and matched the prior
marked render across all 48 picture frames and 96,000 PCM24 samples per
channel.
Graph/2 recapture binds the property to an enabled V1 occurrence or a
verified run of adjacent V1 pieces. It checks the original's saved locator,
source and record frame ranges, and marker IDs. A whole-frame trim within
the saved source range can recover edit markers. Extending beyond that
range, missing or duplicate marker IDs, and mismatched originals are
refused. Older Premiere projects without the property keep their ordinary
source markers.

The reader retains this link in `premiere.edit_marker_provenance`.
On the return to Resolve, matching source markers and media binding let
both cuts share the original Media Pool item again. The linked writer
restores the frame-aligned edit markers to the marked cut and verifies
the native edit UUID, source and record positions, ranges, and colors
before publication. A copied source marker that has diverged from the
original, or a subframe edit marker, requires loss approval.

Resolve Studio 21.2.0 build 00006 reopened and re-exported the returned
project with two markers on the first cut, both markers on the shared
source, and five markers on the timeline. The first cut retained its
Orange label. Both trims and the linked audio lanes agreed with the
original Resolve control across 48 decoded frames and 96,000 PCM24
samples per channel. The tested round trip uses one straight
V1 cut owner and picture-only 1080p24 ProRes HQ MOV with zero source origin.

Version 2 of the dedicated Premiere item property can carry Resolve
edit-marker custom data by marker GUID. The property stores hex-encoded
UTF-8 with a total budget of 1,024 bytes per dedicated item. Version 1
records still restore ownership. Invalid hex, invalid UTF-8, and
over-budget receipts are refused. At export, data beyond the budget
remains in the source graph with an occurrence-specific advisory.

Two different UTF-8 values with punctuation, multiple scripts, and a
line break survived Premiere 27 Save As and reopen. Resolve 21.2 restored
each value to its own edit marker, and the saved archive and Resolve API
agreed after another reopen. The picture and both audio channels matched
the native Resolve control.

#### Resolve-only edit marker colors

Version 3 of `AnyNLE.Resolve.EditMarkers` adds the original Resolve color
and the displayed Premiere fallback to the GUID record for each affected
marker. Versions 1 and 2 remain readable. Both Premiere readers capture
these values on the edit receipt. When the cut and original source pass
the existing ownership checks, graph/2 restores the original color if
Premiere still has the fallback. A different saved color takes precedence.
The current text and span come from the saved marker.

Premiere 27.0.0 build 88 kept the eight green placeholders on the
dedicated copy after Save As and reopen. Resolve Studio 21.2.0 build
00006 restored all eight original colors on the trimmed 1080p24 V1 cut
and shared its original media item with the unmarked neighbor. A second
Premiere save recolored one marker to Red and changed the title and note
of a Mint marker. Both edits survived the Resolve return. All 13 timeline
markers survived, including three that share record frames with
edit-owned markers. Duplicate markers within one native owner still
produce omissions. Both returned projects matched all 48 decoded picture
frames and 96,000 PCM24 samples per isolated mono lane.

#### Restore when the original is only in the Premiere bin

When every selected cut uses a dedicated copy, the internal Resolve
turnover can capture the original from the Premiere bin. The saved edit
record must identify one original item. Preparation checks its locators,
physical media binding and saved clock, then checks the copied source
markers and metadata. Only a cut whose copy agrees can use the recovered
item. Copies that diverge stay separate and are listed in Review.
Missing or ambiguous saved item references still refuse.

The 1080p24 ProRes HQ control used distinct trims on two copies with ten
edit markers. Both copies had markers at the same four file-relative
frames, with different colors and notes. Premiere 27.0.0 build 88 kept
both copies after Save As and reopen. Resolve Studio 21.2.0 build 00006
reopened the returned cuts on one Media Pool item with their own edit
markers, the two shared source markers, and all thirteen timeline
markers. The return matched all 48 picture frames and 96,000 PCM24
samples per isolated mono lane.

#### Compare copied source markers on their saved Premiere palette

When the original's source colors have already been restored in graph/2,
the copy check compares those specific markers using the Premiere colors
recorded in the original's color receipt. The original retains its Resolve
colors for writing. The comparison still checks each marker's saved text,
range and color, along with the existing media and ownership checks.

The two-cut 1080p24 test reopened in Resolve with ten source markers on
the shared item, ten edit markers across the two cuts, and thirteen
timeline markers. All three owners kept their Resolve colors and custom
data. A color changed on only one dedicated copy in Premiere triggered
Review for that cut. The other cut restored its edit markers. The
unmodified return matched 48 decoded frames and 96,000 PCM24 samples per
isolated mono lane.

#### Edit markers on the exact 23.976 clock

The round trip uses the saved timeline rate for original-item recovery
and keeps the matching source clock and trim proof. At `24000/1001`,
the Premiere tick interval is exactly 10,594,584,000 per frame. The same
saved ownership checks apply to the exact record start, source trim,
and duration. An edit marker off that frame grid needs Review approval.

The 1080p ProRes HQ control used two marked cuts with different source
trims, a shared stereo PCM16 WAV feeding two separate mono lanes, and
coincident timeline and cut markers. Both editor reopens kept eight edit
markers, four timeline markers, and sixteen source markers with their
colors, ranges, and custom data. A Premiere recolor and a separate text
edit survived the return to Resolve. Both returned projects matched
48 decoded picture frames and 96,096 PCM samples per isolated mono lane.

#### Edit markers on a tagged 24 fps original

The linked picture profile uses the verified QuickTime timecode as the
clip's display origin. Edit trims and marker positions stay relative to
the file's picture frames. The saved Premiere origin must agree with
the original file before a dedicated copy or its parent can be used.

The native ProRes HQ control starts at source timecode `10:00:01:00`.
Premiere 27.0.0 build 88 and Resolve Studio 21.2.0 build 00006 reopened
both marked cuts at file frames 5 and 39. Resolve recovered one shared
Media Pool item, eight edit markers, two timeline markers, and two
source markers. A Premiere recolor and a separate text-only edit
survived. Both returned projects matched all 48 decoded picture
frames and 96,000 PCM samples per isolated mono lane.

#### Edit markers on a tagged 23.976 original

The exact `24000/1001` profile admits a matching non-drop QuickTime
timecode track at the same frame rate, time base, packet origin, and
duration as the ProRes HQ picture. Source timecode is retained as a
display origin. The cuts and marker positions remain file-relative.

The tagged `10:00:01:00` control has an exact display origin of 864,024
frames on the `24000/1001` clock. Its two cuts start at file frames 5
and 39. Resolve's scripting source-start accessor reported 38 for the
second cut. The saved range, left offset, and decoded frame comparison
confirmed frame 39. Both editor reopens preserved eight edit markers,
two Media Pool markers, two timeline markers, and both mono
assignments. A saved Premiere recolor and a separate text edit survived.
Both returned projects matched 48 decoded picture frames and 96,096
PCM samples per isolated mono lane.

#### Edit markers follow a moved cut

A dedicated Premiere item can move to another whole frame on its original
sequence's V1 track. Recovery requires a single enabled use with the same
source identity and marker IDs. The graph receipt stores the old and new
record starts when they differ. Edit markers use the current record start
and the marker's offset from the current source in-point. Timeline markers
keep their saved record positions.

In Premiere 27.0.0 build 88, swapping two cuts at exact `24000/1001`
kept the source ranges starting at file frames 5 and 39. Resolve Studio
21.2.0 build 00006 reopened the cuts in the new order with their own
four markers each, one shared source at `10:00:01:00`, and unchanged
timeline and source markers. The return matched a separately authored
Resolve swap across 48 decoded picture frames and 96,096 PCM samples per
isolated mono lane.

#### Edit markers on a trimmed Premiere cut

The same profile accepts a shorter whole-frame source range inside the
range recorded on the dedicated item. The graph receipt retains both
file-relative ranges. Markers whose entire span fits keep their edit
ownership and exact duration. Markers outside the new trim or extending
past its out-point keep their original source position, range, and data
in graph/2. They appear individually in Review and are omitted from the
written timeline after loss approval.

Premiere 27.0.0 build 88 saved and reopened the moved cut narrowed from
file frames `[5,29)` to `[9,24)` at record frame 24. Resolve Studio
21.2.0 build 00006 reopened the return with six edit markers, two source
markers, and two timeline markers. Graph/2 retained the point before the
trim and the range crossing the out-point. Against a separately authored
Resolve control, all 39 decoded picture frames and both isolated mono
lanes matched exactly across 78,078 PCM sample frames each. The last 71
samples on each lane also changed in the native trimmed export relative
to the longer control.

#### Edit markers on adjacent pieces

Multiple enabled V1 edits can share the dedicated Premiere item if every
consecutive pair joins exactly in record and file-relative source order,
inside the recorded source range on the original sequence. Each piece gets
its own graph/2 receipt. An intact marker belongs to the piece containing
its whole span. A range crossing any join remains one graph/2 occurrence.
A gap, repeated source frames, or unrelated reuse does not establish a
split. The existing edit limits apply.

The three-piece control added a second split at file frame 19. Premiere
and Resolve reopens preserved the joins at record frames 33 and 38.
The same seven edit markers returned to the correct cuts, including a
range that ends exactly at the second join. An independently authored
Resolve control matched all 48 decoded frames and all 96,096 sample frames
on each isolated mono lane.

Premiere 27.0.0 build 88 saved and reopened consecutive uses of the marked
item over file frames `[5,14)` and `[14,29)`. Resolve Studio 21.2.0 build
00006 reopened the returned three-cut project with seven edit markers
and the original source and timeline markers. The two-frame marker
starting at source frame 13 remained available in graph/2. All 48 picture
frames and all 96,096 samples on each isolated mono lane matched the
independently authored native split and the earlier unsplit render.

#### Timeline fallback for markers crossing a split

When a verified edit marker crosses contiguous V1 pieces, the linked
writer can keep the full range as one editable Resolve timeline marker.
Review reports the change of ownership by occurrence and requires loss
approval before publication. The original edit ownership, source position,
and marker data stay in graph/2. The fallback requires whole-frame timing
and an unoccupied start frame. A collision keeps the existing timeline
marker and reports the edit range as omitted. A range removed by a trim
still follows the trimmed-cut rule.

On the retained three-piece 23.976 control, Resolve Studio 21.2.0 build
00006 reopened the promoted two-frame Purple range at record frame 32 with
its saved name, note and custom data. The seven edit markers, two source
markers, and two existing timeline markers stayed on their original owners.
All 48 decoded frames and both isolated mono lanes matched the independently
authored Resolve control. A separate eight-frame range spanning both joins
produced one timeline marker with the exact duration.

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

#### Static 3D Warp geometry at 24 fps

The internal Resolve to Premiere writer also accepts three 1080p24 V1
profiles from the Resolve reader. It writes isolated centered rotation,
centered source crop with independent scale and clockwise rotation, and
rotation about one displaced X or Y pivot. These profiles reuse the
same graph/2 geometry as the linked Resolve writer. The Resolve reader
has already converted the rotation sign and pivot coordinates to graph
units. Premiere receives those values directly in Motion.

Premiere 27.0.0 build 88 retained the linked ProRes original, exact source
and record bounds, 01:00:00:00 sequence start, and saved Motion values
after Save As and reopen. All ten rendered frames agreed with the
reopened Resolve controls within 0.06 pixels at the 69 measured chart
points. The image bounds agreed within one pixel. Replaying the saved
graphs reproduced the same controls. An isolated pivot combined with
unqualified crop refused before publication.

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

### Resolve to Premiere Linear Z Rotation

The internal writer maps the three aligned curves in
`resolve.warp_roll_linear` to editable Premiere Motion on 1080p24 V1.
The Premiere keys use the saved file-relative frames. Zoom X becomes
Scale Width in percent, and Rotation Angle changes sign. Vertical
position, Scale Height, crop, and pivot use their neutral values.

After Save As and reopen in Premiere 27.0.0 build 88, the generated
project kept the exact 13-frame edit and all nine Linear Motion keys.
Position X, Scale Width, and Rotation each retained keys at source
frames 0, 5, and 12. All 91 chart points across the 13 rendered frames
agreed with the reopened Resolve render within 0.15 pixels. The image
bounds agreed within one pixel.

On a disposable copy, changing the middle Rotation key from 25 to 35
degrees survived a second reopen and changed exactly the 11 interior
frames. The key times, other Motion controls, and edit bounds stayed
intact. A misaligned rotation key refused before publication.

### Resolve to Premiere video dissolves

The internal writer maps V1 Cross Dissolves with left or centered
alignment to Premiere Cross Dissolve (Legacy). It retains the two saved
edit identities and checks the same-rate ProRes HQ handles before writing.
The original eight-frame controls at exact 24000/1001 used eight frames
of incoming preroll for left alignment and four frames on each side for
centered alignment. Visible clip trims remain intact.

After Save As and reopen in Premiere 27.0.0 build 88, both controls
retained the cut at frame 24, the transition intervals (frames 16–23
and 20–27), and all media links. Picture outside each blend matched
the Premiere straight-cut control. Premiere used the outgoing image
at the first blend frame, where Resolve had already started blending.
The writer reports the difference in blend sampling. Both isolated
audio lanes matched the Premiere straight-cut control sample for
sample across 96,096 frames.

### Resolve to Premiere audio crossfades

The internal writer maps centered Resolve Cross Fade 0DB on A1 at 24 fps
to editable Premiere Constant Gain. The originals are mono 48 kHz PCM WAV.
The writer uses the two saved edit identities, verifies both handles
against the original files, and preserves the visible trims. The original
eight-frame control used two four-frame handles. Crossfades with
independent track pan or clip edge fades remain unqualified.

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

### Resolve to Premiere combined transitions at 24 fps

The first combined 1080p24 control used a centered eight-frame V1 Cross
Dissolve and a centered eight-frame A1 Cross Fade 0DB. The writer emits
Premiere Cross Dissolve (Legacy) and Constant Gain at their own cuts and
reports the picture blend sampling difference.

The generated project and three controls (picture transition only, audio
transition only, and straight cuts) went through Save As, close, and reopen
in Premiere 27.0.0 build 88. The combined project kept its 01:00:00:00 start,
all five visible edits, original links, physical audio channels, and the
centered transitions at the V1 cut on frame 20 and the A1 cut on frame 24.

All 48 decoded picture frames matched Premiere's picture-only transition
control. The picture changed from the straight-cut control only on frames
16–23, the same interval as in Resolve. All 40 unblended frames selected
the expected numbered source frames in both editors. Cross-editor picture
hashes differ even on the straight-cut control.

Both isolated 96,000-sample PCM24 stereo renders matched their Premiere
controls sample for sample. A1 followed the audio-only transition control
and A2 matched the straight cut. After accounting for each editor's center
pan, A1's measured blend coefficients differed from Resolve by less than
0.00003. The retained Resolve A2 control also has a short edge envelope.

In a separate check, a disposable copy with its project version marker set
to 46 converted back to Resolve. After reopening in Resolve, all 48 picture
frames and both isolated 96,000-sample PCM24 lanes matched the original
Resolve render.

### Repeated and variable-length transitions

The internal writer preserves each saved blend window for multiple
nonoverlapping transitions on V1 or A1. A middle clip can carry both
its incoming and outgoing transition references. The existing profile
accepts left-aligned and evenly centered V1 Cross Dissolves at 24 and
24000/1001, and evenly centered A1 Cross Fade 0DB at 24. The native
Resolve receipts still verify the edit identities, original handles,
and distinct blend windows. Other curves and alignments require a
separate mapping.

Offline checks preserved two dissolves and two crossfades on one
timeline, including distinct head and tail references on both middle
edits. A separate run preserved a six-frame centered dissolve, a
twelve-frame left-aligned dissolve, and a ten-frame Constant Gain
crossfade. Both retained their saved intervals through archive read-back.
Premiere 27 build 88 saved, closed, and reopened disposable copies with
the current project-version marker. Both retained their links, trims,
and transition windows. The repeated-transition render matched all 48
decoded picture frames and 96,000 PCM24 sample frames per isolated lane
against the Premiere control. The variable-length render matched all
picture frames and A2 samples. The A1 difference was confined to the
Power transition window deliberately omitted from that test copy.

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
