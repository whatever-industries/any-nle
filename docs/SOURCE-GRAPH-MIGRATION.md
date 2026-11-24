# Source graph and destination preparation

The 2026-11-05 migration separates a qualified source interpretation from the
preparation required by a destination. Final acceptance receipts and the correction
ledger accompany integration; implementation or parser acceptance alone does not
qualify a newly changed project in an editor.

## Boundaries

```text
Avid bin files, read in place
  avid-reader: one decoded bin at a time; compact owned source witnesses
  avid-adapter: referenced media, exact clocks, independent lanes, native facts
  anynle.graph/2: qualified source interpretation, including unresolved facts
  premiere-adapter: PreparedPremiere, borrowed graph + ownership/trim tables
  prproj: one typed model -> one encoded PreparedProject + small LUT recipes
  media plan -> verified media -> assets -> source snapshot -> project -> receipt
```

The source adapter has no production dependency on Premiere, the legacy bridge or
the model builder. Premiere preparation borrows the source graph. It neither clones
the entire graph nor constructs a graph/1 or model JSON document as an intermediate.
`avid2prproj.model/1` remains the typed writer input and the standalone JSON API.
There is no new model serializer or model-size counting pass.

Graph/2 describes qualified interpretation, **not complete native reconstruction**.
An unknown offset or relationship remains unknown. Unsupported native controls,
owner clocks and omitted portable objects retain bounded native receipts and losses.
Malformed references and contradictions refuse. Occurrence IDs are scoped to the
input revision; they are not native identity claims or object addresses.

For an original stream, graph/2 checks the exact duration against its tick clock,
the physical channel index against the saved channel count, and the bit depth of
known sample representations. A single still has a separate editorial hold.
Signed packet origins, larger channel layouts, still rasters and additional
representation names can round-trip as source facts. Linked destination
preparation invokes the shared original-stream profile before it uses those
facts. Each route also checks its own supported edits, file identity and routing.
Graph capture alone therefore establishes no new playback or export support.

For ordered Avid picture effects, graph/2 preserves the saved stage list.
The shared picture-chain policy in `anynle-mapping` checks which stages can
share one destination Motion control and how a Corner Pin adjoins a reflection.
Premiere preparation reports an unsupported stack on its edit. Resolve
preparation refuses an unsupported stack. Each writer applies its own geometry
and stage ordering after the shared check.

The separate [Resolve source reader](RESOLVE-SOURCE-READER.md) feeds graph/2
from a bounded native archive profile. Its saved Resolve clocks and channel
selectors retain their own provenance. Product route qualification remains
separate from the source adapter.

## Skip unused bin clips that can’t link

The linked Avid → Premiere job uses this fallback by default. The source
adapter checks saved Mob references in the same decoded bins used for
capture. The check covers alternate and hidden source branches and
disallows omission when a reachable editorial component is opaque or
its reference is incomplete. It also protects reusable clips that refer
to a master when no timeline selects them. Failed links outside the
omission profile retain their normal diagnostic. Qualified offline
placeholders still take precedence.

A regular managed picture master can also be omitted when its saved picture
rate agrees with the master but has no supported exact clock. The route checks
the full-span picture and up to two ordinary mono PCM siblings. The same
dependency proof applies even if the MXFs are present. A referenced clip or a
contradictory saved master and picture rate still blocks.

The reader also recognizes a saved drive-like locator spelling on a bounded
23.976 MXF bundle with one picture file and eight mono PCM24 siblings. A
complete bundle has distinct package IDs and file names, one saved directory,
matching descriptor extents and explicit file-track selectors. The saved
spelling establishes no local filesystem root. Review searches the selected
Avid media roots by package ID and verifies each single-essence MXF's clock,
descriptor span, and physical stream before using the ordinary managed-media
route. Graph/2 retains all saved locators and file selectors. Missing files
may be omitted as a whole bundle when dependency checks prove the master
unused. References, conflicting locators, or incorrect physical identities
keep otherwise unresolved media visible as blockers.

Each accepted omission retains a bin-owned `omitted_unused_masters`
receipt and occurrence-specific `unused_bin_item` loss. The source list,
input revisions, and media checks are revalidated by saved jobs.

## Continue with omissions

With complete saved dependency evidence, linked Avid → Premiere reports
duplicate MXF packages outside the selected bins' saved references as
diagnostic warnings. Packages referenced by a selected clip or saved
dependency retain their blocking diagnostic. An incomplete dependency walk
retains the scan refusal. The same check lets qualified missing-media
omissions proceed in the presence of unrelated duplicates. The media index
excludes ambiguous packages from physical binding in every mode.

Linked Avid → Premiere conversions use best effort by default. Qualified
media bindings and requested offline items take precedence. The linked
reader can omit ordinary saved originals that are unavailable, a missing
qualified legacy MXF bundle, wholly unavailable managed clips within the
qualified source profile, and 4K AVC/PCM files with corroborated saved
layout and extents that the writer does not support. It retains the saved
bin facts under `omitted_media_masters` and reports the omitted bin
occurrence without claiming the clip was unused.

An omitted media edit gets a sequence range marker when its saved bounds,
rate, and lane are exact and the range does not overlap another edit or
transition on that lane. Its source selection and range remain in the
source receipt. Other unsupported timing stays in the loss report, and
the rest of the sequence can continue. Contradictory bindings, corrupt
inputs, graph limits, and invalid project outputs still fail. The same
default applies to older jobs.

When an ordinary video or audio dissolve starts at the cut from saved
filler or ends at the cut into saved filler, a linked conversion can keep
the adjacent edit and report the dropped fade by occurrence. This requires
an isolated span on the affected lane, a matching saved transition clock,
a verified filler receipt, and an edit that already starts or ends at the
cut. This includes Avid Film Dissolve and Film Fade when the saved effect
identity and the filler receipt agree. Other video layers retain their
edits. Qualified editable stereo fades take precedence.

Linked Entire Project preparation can also omit a wholly offline
managed master that cannot become a valid Premiere placeholder. It
keeps the original graph facts and produces a range marker only on a
verified vacant lane interval. An ordinary dissolve with a surviving
endpoint can preserve the saved cut when both endpoint ranges prove
its edges. Qualified offline items keep their relink entries.

## Avid linked MP3 and stills

The linked MP3 reader verifies the physical 44.1 or 48 kHz sample clock, decoded
sample extent, and explicit channel selectors against the saved Avid descriptor.
Its graph stream starts at file-relative sample zero. The native descriptor
extent remains on its source receipt. For the qualified fresh 44.1 and 48 kHz
MPGA stereo profiles, the reader verifies the leading MPEG `Info` frame and
gapless decoder delay. Straight mono placements at 25 fps use gapless file
time and retain the saved trim and physical sample clock in
`avid.mp3_presentation`. The reader checks that the entire corrected edit is
playable. Premiere preparation uses the gapless extent for the linked master.
Premiere preparation reports the observed 44.1 kHz resampling difference by edit.

For the qualified linked 1080p24 JPEG and PNG descriptors, the original stream
describes one physical image. The media duration describes the saved Avid hold
in whole frames. Premiere preparation writes a native still and preserves the
saved hold and edit trims. The same split between physical image and editorial
hold also applies to the bounded 1536×1024 RGB PNG profile. For a V1 edit with
the saved centered, full-image spatial adapter, graph/2 carries the verified
1920×1080 canvas and a uniform Motion scale of 105.46875%. The edit retains its
native source color receipt. The imported PNG Matte Key path handles a
different saved Avid relationship. [Linked format checks](ROADMAP.md#high-priority-linked-audio-and-stills)

## Avid Gain and Timewarp fallbacks

If Gain, an automatic Motion Adapter, or a generated Freeze Frame has
no qualified translation, the reader can preserve its saved record
interval on the occurrence receipt. The converter writes an exact
sequence range marker when the effect is on a verified lane and clock
and the interval is clear of competing edits. A Gain gap can share a
boundary with a directly identified Avid audio dissolve. Graph/2
retains the saved overlap, cut, and the unavailable endpoint's owner.
Premiere places the gap edge at the cut, omits the dissolve, and
trims the verified neighboring audio to that cut. An adjacent saved
filler or second Gain gap needs no audio placement. The project report
identifies the gap and the omitted dissolve. Unresolved adjacency,
contradictory timing, and unsupported neighboring trim behavior
remain blockers.

Direct linked conversion can bypass one recognized AudioSuite Gain when the
saved 24 fps mono route resolves through a direct master or subclip and its
zero-offset managed PCM16 or PCM24 sibling covers the entire source range on
the 48 kHz clock.
The same placement carries independent edit gain, pan, automation and its
transition relationships. The saved Gain binding and its dry input remain
in graph/2 source receipts. The existing prepared-media route can still use
the qualified processed Gain cache.

The saved legacy Avid EQ (`EFF_AUDIO_EQMB`) can use the same managed
PCM verification for an unrendered, full-span 24 fps mono input.
Graph/2 retains the effect enable, filter name, and each band's raw
type, frequency, gain, Q and enable on the source event. These fields
remain in Avid's saved value space. Enabled EQ reports an edit-specific
processing loss. Independent timeline gain and pan still apply.

For a saved render of that effect on a same-rate mono edit, the linked
Premiere path can use the saved PCM24 cache in place. The
precompute's EQ values and dry file-relative input must agree with the
sequence's EQ and source trim. The cache's master, file and origin
packages and exact 48 kHz playable sample extent must all verify.
When the edit uses a portion of a longer cache, graph/2 retains the
cache's full length and the edit's in-point. Applying that in-point to
the cache's saved dry input must yield the same file, channel and
position as the sequence edit. A verified
render retains the processed audio and independent timeline mix.
Conversion reports the loss of an editable EQ. If the saved cache package
is absent from the scanned media roots, direct linking can use the dry
master lane and exact file range from the same verified source receipt.
That lane must resolve to the saved file package with a zero offset and
continuous, playable mono 48 kHz PCM. Conversion reports the missing cache
and omitted EQ on the edit and preserves independent timeline mix controls.
Ambiguous or mismatched caches remain explicit omissions.

The dry input can also use one same-rate audio subclip. The verifier
follows the selected sound track through the subclip and its master,
accounts for leading filler, and permits qualified neutral Pan Volume
wrappers. The resulting file package, saved channel label, and exact
dry start must match the EQ precompute's copy. Graph/2 keeps those
facts with the cache reference, including the dry master and its lane.
A mismatched channel, one-frame slip, shortened input or nonneutral
gain inside the subclip omits the EQ edit.

A usage-7 master can supply the same dry binding when it has one sound
track and a full-span neutral Pan Volume wrapper over a zero-offset
PCM24 file. The saved precompute must independently identify the same
neutral input and exact file range. Graph/2 records the master usage
on that dry-input receipt.

One same-rate mono PCM24 master with explicit static gain and centered
pan also qualifies when the precompute copy has the same saved gain,
channel and file-relative range. Both inputs must wrap one full-span
sound clip with zero-length boundary fillers. The saved AAF slot must
agree with the clip's track label. In the tested Avid profile, the EQ
cache and rendered timeline both bypassed this master gain. Premiere
links the cache and retains independent timeline gain and pan. Graph/2
records the inner gain on the cache's source receipt. If that cache is
missing, the edit remains an explicit omission. Other layered effects
remain open.

For the saved two-second mono Pitch II inside EQ case, the source event
receipt records the exact edit range, file and channel, EQ controls,
and both cache bindings. The live Pitch II settings must agree with
the two saved copies. The receipt records the plug-in payload length
and SHA-256. A derived Avid control with the same neutral subclip route
verified a 96,000-sample outer EQ cache and a 96,002-sample inner Pitch
II cache. The Avid export played the outer cache, including at both
edit boundaries. Premiere direct linking verifies the outer file
package and extent, links it once, and reports the loss of editable
Pitch II and EQ controls. Premiere 27 rendered the complete control
sample for sample after Save As and reopen with just the outer cache
and the dry source online. If the outer cache is missing or fails
verification, the affected span stays a gap.

An unsupported sampled Timewarp has separate handling when its saved
picture span and clock agree with the sequence and the span is clear of
same-lane edits and transitions. A direct precompute must match the saved
master, file and upstream origin package, with exact trim coverage in a
qualified progressive 1080p picture stream. A saved precompute can name the
clip directly or through a same-clock picture sequence containing only that
clip and zero-length fillers. The saved clip in-point still determines the
cache trim. If the cache is unavailable or
fails verification, a sequence range marker names the affected lane and
identifies the exact gap. The saved witness and the per-edit decision stay
in the graph report. The editable sampled profiles retain their usual
checks and contradictory or unresolved timing still blocks conversion.

For an unresolved Timewarp, the sequence's `avid.source.unresolved` receipt
also retains the saved span ratio, phase and offset, render override, graph
selector, available input and precompute references, and bounded curve data.
Half-frame samples retain their first saved sample index and values. Sparse
keys retain their exact saved times, curve modes, and known key properties.
Explicit FluidMotion and FluidMotion Draft overrides retain both their numeric
value and the name confirmed in Avid's Motion Effect Editor.
These observations are in the effect's own clock. A destination needs a
qualified source route and sampling rule to use them as file-relative frames.

For one full-span Resize over one sampled Timewarp, an exact saved render
owned by the outer Resize can supply the composite. The decoded cache must
trace to a Resize over Timewarp on the matching picture clock. The same
master, file, origin, packet coverage, and overlap checks apply. The
source receipt identifies the Resize separately from the inner Timewarp.

The traditional 1080p24 Freeze Frame's 120-frame generated render can also
supply a shorter edit from its start. Two distinct picture subclips qualify
when their single picture routes reach the same master and source frame.
The converter can keep this picture beneath the retained video-levels
Color Adapter profile. It reports the omitted Color Adapter on the edit.

### Standalone AudioSuite renders

A same-rate 24 fps mono edit can use an AudioSuite PCM24 cache or
a trimmed window within it. The timeline effect and the cache origin
must have matching saved AAX plug-in identity and settings. The
effect's input must agree with the copy's file-relative in-point plus
the cache trim. Both select the same lane of one mono 48 kHz PCM file.
Graph/2 retains the cache in-point and full length, input range, package
bindings, plug-in IDs, and opaque chunk length and digest in the
native event receipt.

The timeline input can use one same-rate subclip. Its selected lane
must resolve to the same file and channel, including the subclip
offset. Graph/2 records the subclip identity, track and local in-point
alongside the resolved input. The saved copy can address the file
directly or through one full-span Pan Volume wrapper. For subclip
inputs it can also use two such wrappers. Each wrapper must have
verified unity gain and center pan. The verified render supplies playback.

A direct master and its render copy can also carry a sequence inside
the neutral Pan Volume wrapper. The sequence must contain one
full-span sound clip between two zero-length fillers. The wrapper and
sequence must agree on the saved AAF slot or on all three saved routing
fields. Slot and channel must select the sound clip's saved track, and
the qualified mono source slot is 1. The ordinary file, channel, and
range checks still apply to both paths.

Direct linking checks the MXF package chain, the complete playable
PCM span, and a separate terminal packet containing exactly two zero
samples. The playable range ends before that packet. Premiere 27 Save
As and reopen matched the full-span Avid D-Verb and Pitch II Direct
Out controls and two trimmed D-Verb controls sample for sample. The
full-span and trimmed subclip D-Verb controls also matched exactly.
One-wrapper D-Verb controls matched for both direct and subclip inputs.
Two additional direct-master D-Verb controls with a saved sequence
inside Pan Volume also matched exactly.
The report names the baked plug-in by occurrence. An absent or
invalid cache leaves a reported gap. Nested AudioSuite effects, stereo
processing, and prepared-media rewrap remain separate qualification
cases for this route.

## Avid source ColorAdapter ownership

The physical picture source's saved input transform remains in the item's
`avid.source.input_color_transforms` receipt. A top-level
`EFF2_LUTSFX` edit with one full-span picture input on the sequence's
saved clock can carry a separate
`color_adapter` record in the owning sequence's `avid.source.unresolved`
receipt. It retains the exact edit-side text, input master and track, and
saved source in-point. When that text matches the directly referenced
picture source and the wrapper resolves to one placement over the same
range, the record names the picture file package and that placement.
A missing or contradictory source match retains its reason and a null
placement. A plain edit of the same master has no ColorAdapter record.

For direct same-rate picture, capture uses the saved rational edit rate
instead of requiring 24 fps. The master, selected picture track, and
file must agree on that clock, and the requested trim must fit the
saved source extent. The 24 fps subclip, retime, still-image and
managed-cache profiles retain their scoped checks. Destination writers
qualify coverage separately.

The exact standalone 10-bit LinearLut lists also retain a `linear_range`
record on each saved file or edit owner. It gives the direction, bit
depth, and 64/940 video endpoints alongside the original XML. The
full-to-video list requires Avid's saved `Inverted` flag. A stacked or
unrecognized list retains its raw XML. For an exact manual video-to-full
adjustment paired with an external 3D cube, `ordered_steps` records both
stages in saved order. If the direct picture file saves the same two
stages in reverse order, `reordered_picture_source` records the package
and the uniquely owned placement when it resolves. The ordinary source
match stays unresolved. Avid re-exported the earlier edit identically
after the file's saved order changed, so the earlier edit keeps its own
order. The range receipt describes the saved instruction. The physical
file's own color tags and whether the
destination applies the instruction remain independent. Premiere
preparation uses both receipts for the qualified managed ProRes LT
rewrap. It checks the selected package, edit trim, and project-local
cube, then composes the edit's two steps into one clip Color Mode LUT
without changing their order in graph/2.
[Composition and render checks](PREMIERE-27-COLOR-MODE.md#ordered-avid-color-adapters)

The `affine_cdl` and `clamped_cdl` receipts describe a standalone saved
10-bit `ColorDecision` with clamping enabled and unit saturation.
They carry per-channel slope, offset and power. `affine_cdl` has unit
power and `clamped_cdl` has an explicit non-unity power on at least
one channel. A saved scalar Power applies to all three channels.
The receipt accompanies the original XML on the saved file-package
record and the timeline occurrence. Avid's
`ASC_SOP` and `ASC_SAT` bin columns are logging metadata. Color Adapter
application requires the saved transform. Avid Processing can use the
same source interpretation to build the opt-in editable RGB Curves
trial. [Avid Processing scope](ROADMAP.md#timeline-and-effects)

Premiere's Separate files preparation can translate video-to-full scaling on a
plain 1080p24 picture edit of the qualified picture-only ProRes HQ original
with an editable ASC CDL. It uses per-channel slope (1023/876) and
offset (-64/876) in Premiere's normalized video-level space. The
master clip retains the linked original. A plain timeline occurrence
of that master does not get an ASC CDL. The qualified edit can be on a
higher picture lane alongside a separate video dissolve.
An edit that is a transition endpoint still requires its own qualification.

The standalone 10-bit full-to-video list also writes an editable ASC CDL
on the same plain linked ProRes HQ profile. In Premiere's video-normalized
RGB space, the slope is 876/1023 and the offset is 64/1023 on all three
channels, with unity power and saturation. Preparation requires the saved
`Inverted` flag and matching source and edit text. The effect belongs to
one timeline edit. Nested retimes and managed rewraps remain outside this
direction's verified profile.

Both directions use the same graph/2 interpretation at 24, 23.976,
25, and 29.97 fps. For plain same-rate 1080p picture-only AVC MOV, the
writers verify the source clock, trim, and physical color tags separately
from the adapter. Premiere applies an editable ASC CDL on the timeline
clip. Resolve applies one editable clip Color node and records the saved
direction as `resolve.cdl_range` on read-back. The Resolve mapping also
covers qualified 24 fps picture-only ProRes HQ MOV and requires an
unretimed V1 edit outside a transition. Neither writer changes the linked
original.

For legal-to-full grades in Resolve, set Deliver data levels to **Video**
and enable **Retain sub-black and super-white data** to preserve
excursions in the exported picture. The default export clipped them in
the chart test. [Resolve reference manual](https://documents.blackmagicdesign.com/UserManuals/DaVinci_Resolve_21.1_Reference_Manual.pdf)

The source receipt leaves color-space and cube asset qualification to
destination preparation. Unapplied transforms keep an occurrence loss.
[Premiere comparison](ROADMAP.md#edit-fidelity-and-metadata)

## Avid timeline track declarations and names

Graph/2 stores qualified Avid track names on `sequences[].tracks[]`, keyed
by media kind and one-based index. A saved name can declare a track with
no portable edits. The optional `empty: true` flag requires a verified
plain picture or mono filler lane. The graph rejects a placement on a
lane declared empty. An omitted flag leaves the lane's emptiness
unverified. Unnamed track inference follows the existing placement,
transition, and marker rules.

The Avid name record retains ambiguous or unrepresentable text. Premiere
preparation still accepts previously saved graphs with names in the
`avid.track_names` sidecar. It reports conflicting rows for the same lane.
Destination preparation decides which names and layouts it can write.
The sequence's `avid.plain_track_layouts` receipt identifies named,
direct picture lanes and explicitly saved mono audio lanes whose source
contains edits. Premiere can preserve their names on empty output lanes
when the edits are omitted. The source lane remains distinct from a
filler-only declaration and the original omissions stay in the report.
Graph/1 tracks and saved job formats keep their existing contracts.
[Timeline track checks](SPLIT-EDIT-AUDIO-2026-11-06.md#timeline-track-names)

## Avid Linear clip pan

Qualified Avid pan curves on direct 24 fps mono edits use the
`audio.pan_linear` placement effect. Each key stores exact seconds from
the start of the edit and a rational pan from −1 at left to +1 at right.
The graph checks key order, edit extent, and mono ownership. The original
PVOL points remain in the source record.

Premiere preparation converts the edit-local times to the prepared
clip's source clock and rounds fractional ticks at that boundary. It
also reads the original pan records in older snapshots. The same
media and timeline requirements apply, and unknown pan layouts and
rendering differences continue to receive scoped reports.
[Managed mono pan checks](SPLIT-EDIT-AUDIO-2026-11-06.md#editable-managed-mono-clip-pan)

## Disabled Avid edits

Graph/2 carries a verified inactive backing cut in
`sequences[].disabled_placements[]`. It retains the occurrence ID, exact
record and source spans, item identity, and ordered audio channel
selection. The selected filler stays a gap in the active timeline.
The Avid record and scoped loss retain the native selector evidence.

Premiere preparation still checks the destination route, physical media,
and overlap with other edits before writing a disabled clip. It also
reads older snapshots containing just the original Avid record.
[Disabled clip qualification](SPLIT-EDIT-AUDIO-2026-11-06.md#disabled-clip-occurrences)

## Saved Avid subclip ranges

The bin-owned `avid.subclips` sidecar retains one entry per saved usage-2
occurrence, including its native mob ID, occurrence ID, name, owner edit
rate, backing length, and separate raw `_START` and `_END` observations.
Track rows retain the label, media kind, component class, clock and length.
For an immediate source clip they also retain the target mob and track
and the stored target offset. Missing or ambiguous marks are unresolved.
Duplicate display names remain distinct. Wrapped tracks retain their
immediate component facts without resolving a new source range.

In the retained 24 fps case, the bin range is 76–163 (87 frames) and the
backing composition is 242 frames. The edit already starts at frame 76.
In the 29.97 case, the stored range is 116–229 (113 frames) over a
334-frame composition. The saved timeline route remains the authority
for the edit's source trim. Nested, mixed-rate and overrun playback
remain separate cases.
CONV-0097

Premiere preparation uses the saved receipts to make reusable
picture-only bin items in Separate files mode (CONV-0098). The profile
requires one direct same-rate V1 reference to a uniquely identified,
qualified 23.976 DNxHR LB parent, zero backing offset, and a range within
the backing composition and verified file. Each item has its own bounds
and logging range and shares the parent media. Premiere's native subclip
logging time starts at the physical file clock plus the subclip In.
Existing timeline edits
retain their source ranges. Other subclips receive a scoped loss.

Premiere 27.0.0 build 88 kept two same-named items after Save As and
reopen. Fresh insertion used file frames 5–12 and 17–28. All 20 decoded
frames matched Premiere's own subclip construction. A derived AVB
exercises the production entry points.

A later native Avid test saved two V1 subclips with the same name and
distinct identities at file ranges [274, 275) and [586, 598). The direct
CLI and saved job retained both from an isolated copy of the bin.
After Save As and reopen in Premiere 27 build 88, fresh insertion from
the CLI project matched all 13 rendered frames from equivalent parent
trims and from the subclip bin items imported through linked AAF.
The standalone AAF exports used Avid's `.Exported.NN` names. Premiere's
AAF import also made two full-source sequences; those sequences did not
preserve the subclip ranges. The imported subclip bin items did.

A separate 23.976 Avid control places a marked range from one V1/A1/A2
subclip over managed DNx/PCM16 media. The bin receipt records the subclip's
full [0, 96) backing range. Each edit's source chain starts at the subclip
with a 59-frame offset, then reaches the master with a zero offset. The
direct CLI, saved job and reopened Premiere project all use file range
[59, 83) on all three lanes at sequence start 01:00:00:00. The linked AAF
from that exact sequence keeps the original subclip identity and the same
range. Premiere 27 retained a separate reusable subclip after Save As and
reopen. All three edits refer to it, and fresh insertion from that item
produced 96 frames with V1 and both mono lanes. The 24 decoded frames of
the original edit match Any NLE. Both isolated 48 kHz lanes use the same
samples; Premiere's AAF import centers the mono audio, while the selected
Any NLE policy follows Avid's default left/right pan.

Separate files now builds one reusable Premiere master for a full-range
23.976 V1/A1/A2 subclip over verified DNxHD LB and two mono PCM16 MXFs.
It reuses the original file objects. The saved occurrence route binds
each edit lane to that subclip when the full native identities, clocks
and ranges agree. Direct CLI and saved jobs retained that master reference
on all three lanes after Save As and reopen in Premiere 27. Fresh insertion
also reopened with 96 frames and two mono lanes. Both picture renders
matched the linked AAF, and audio matched under each sequence's pan policy.

Shortened subclips in the same profile retain the saved bin bounds on
the picture and both audio sources. In the native control the subclip
stores [59, 83) and the AVB edit already starts at file frame 59.
The linked AAF expresses the same range with a subclip-relative edit
start of zero and a subclip-to-master start of 59. The Premiere writer
keeps file-relative edit ranges and adds the boundaries to the reusable
item. Both entry points matched the 24-frame AAF picture after reopen
and fresh insertion. Both mono lanes matched the same-pan references
sample for sample. Nonzero backing offsets and edits requiring a
separate marker copy remain outside this profile.

The verified 23.976 DNxHR LB route with two original mono PCM24 siblings
also preserves a shortened A/V subclip. The native AVB stores file range
[8, 32) on a 36-frame backing master with zero offsets. Premiere 27 build
88 reopened the CLI and saved-job conversions with a shared subclip item
on V1/A1/A2. Fresh insertion retained that range and all three lanes. The
imported linked AAF timeline used the same trim and original files, with
separate recovered picture and mono items. Fresh insertion from its
picture item produced only V1. The converter timeline and fresh insertion
matched all 24 AAF picture frames. Under default center pan, A2 matched all
48,048 samples and A1 differed by at most one PCM24 count per output
channel. Separate files mode requires two siblings of the same bit depth.

The direct CLI also uses the reusable subclip writer for saved subclips
in Separate files mode when the media comes from `--media`. The retained
unmarked and source-marked A/V controls produce the same projects as the
previous explicit `--link-originals` command and the saved job.

A backing marker wholly outside a reusable A/V subclip stays on the
backing master. A range that ends at In or starts at Out is outside. A
point at In is inside, and Out is exclusive. The writer can still copy
one contained V1 marker, and a crossing range still triggers the reported
fallback. An independently owned edit marker can use the sequence
fallback while all three timeline lanes keep their shared subclip
reference. This applies when the subclip has no source marker or all
resolved edit marker starts differ from the copied source marker start.
The two boundary-range AVBs are
derived from the retained native control. Both CLI and saved-job
projects preserve the trims, selectors, and marker ownership. Premiere
27 converted both CLI projects to its native format and reopened them.
The native saves retain all three references to the same reusable
subclip and the exact range on the backing picture.
CONV-0187

The distinct-start control keeps the source marker at file frames
[10, 13) and the independent edit marker at sequence frames [8, 11).
Premiere 27 reopened the native copy with both ranges and one subclip
referenced by V1, A1, and A2. The report identifies the marker moved to
the sequence. The seeded control with matching source starts still
uses the existing edit marker preference.
CONV-0188

When multiple selected bins contain the same native backing master,
subclip preparation uses the unique matching occurrence in the
subclip's own bin. This keeps that bin's marker and logging data.
Without a local match, the backing identity must resolve uniquely
across the selection. Two different matches within the same bin
still produce a scoped loss. The five-bin macOS replay retained
the marked subclip and all 13 other sequences after Premiere 27
reopen. CONV-0189

## Trimmed Avid Submaster nests

Avid Collapse saves the picture inputs inside the parent’s Submaster.
In the 23.976 test, extracting seven frames from the parent head changes
the inner source range from [2, 35) to [7, 35). The nonzero parent TC1
survives, and A1 and A2 remain independent parent edits. The linked AAF
retains a Submaster operation. Premiere 27 flattens it to an ordinary
picture edit on import.

The direct Separate files route recognizes the saved neutral single-input
Submaster and writes one editable picture child for each qualified
occurrence. The child’s display start is the parent’s sequence origin plus
the placement’s record offset. The parent reads the child from local zero.
The child reads its saved range from the original file. This keeps the
child’s display timecode separate from both trims. The native control
timeline remains separate because the AVB does not link it to the inline
Submaster.

For managed Submasters, the direct CLI can use `--media` by itself. It
preserves the same editable children as the explicit `--link-originals`
command and saved jobs.

After Save As and reopen in Premiere 27, the head-trimmed CLI and saved-job
projects match all 29 linked AAF picture frames. The two isolated mono
lanes cover 58,058 samples each. A2 matches exactly, and A1 differs by
at most one PCM24 count per output channel under center pan. A parent with
the nest two frames into the sequence also retains its later child clock.
The qualified profile covers same-rate online 1080p23.976 direct managed
picture. Other Submaster shapes keep the existing conversion behavior and
report a flattened boundary where the picture edit survives.

A resolved V1 review marker can stay on the parent sequence while the
Submaster stays editable. This also covers a saved edit marker that matches
an inherited source marker. In the derived head-trimmed test, Premiere 27
Save As and reopen retained the parent range at record frames [6, 10) and
the source range at file frames [13, 17) on the picture inside the child.
Both mono edits still link directly to their original MXFs. A graph
round trip also preserves the editable Submaster when a marker with a
confirmed picture owner moves to the parent because its source
position falls between Premiere ticks. Its full record range must
fit inside the prepared edit. A marker with unresolved ownership uses
the flattening fallback.

The parent can also place a plain Submaster above V1. In the derived
V5 control, the parent uses one editable child over record frames [0, 28).
The child reads file frames [7, 35) on its own V1. The lower picture and
two mono edits remain on the parent for all 29 frames. The V5 review
range stays on the parent at [6, 10), and the report identifies V5.
The CLI and saved job agree, and the V1 control is byte-identical to
the previous converter output. Premiere 27 reopened the V5 edit in
the combined wizard project. All 29 picture frames match the flat
control, including the last frame, where V5 has ended and V1 shows
through. This profile uses one neutral picture input. See
`CONV-0167` and `CONV-0182` in the evidence ledger.

Separate files also prepares a plain Submaster at exact 24 fps from
verified 1080p DNxHD LB picture with a zero file origin and two PCM16
mono siblings. The saved parent TC1 must agree with the prepared
sequence start. In the derived control, the parent places the child
at [56, 84), the child starts at 01:00:02:08, and its picture uses
file frames [21, 49). Only the parent picture reads from child frame
zero. The coincident mono edits keep distinct source trims, [22, 50)
on A1 and [21, 49) on A2. Both entry points and Premiere 27 build 88
Save As and reopen agree on the parent, child, and audio edits. A
render against the reopened flat Premiere control matches all 196
picture frames. The full mix and isolated A1, A2, and A3 each match
392,000 PCM24 samples per output channel under the same pan settings.
See `CONV-0169` and `CONV-0184` in the evidence ledger.

### Layered Submaster inputs

Graph/2 retains the ordered input tracks of a layered Submaster.
Its `avid.source` receipt records the
timeline occurrence, qualified record span, input count, and up to
32 input tracks. Each input keeps its saved track index, component
class, and the class and effect ID of its direct or sole nonempty
segment when available. A same-rate picture input with one saved source
clip and optional filler also carries the referenced MOB and track,
the saved source in-point, and the clip's offset and duration on the
input's own clock. The capture verifies the full input length against
the effect, including leading and trailing filler. For plain layers,
the adapter also records the selected graph item. A unique copy in the
sequence's bin takes precedence. If the bin has no copy, the adapter
can use a unique copy from another selected bin. The Premiere writer
checks the full MOB ID and qualifies the item's media before writing.
An unresolved item or unsupported composite stays a reported omission.
Older graph receipts without an item binding still require a unique
master across the selected bins.

The retained two-layer Collapse comparison saves three inputs in
order. The first is filler, the second is a plain source, and the
third is a Picture-in-Picture effect. Two of the saved occurrences
retain a qualified 48-frame interval at 24 fps. In all three copies the
plain layer reads saved source frames [48, 96) at local frames [0, 48).
The variant wrapped inside another effect still has unresolved parent
record timing. The original and collapsed Avid TIFFs match for the
returned frame. The Picture-in-Picture composite remains unsupported.
CONV-0186

Separate files writes an editable Premiere nest for the bounded
23.976 and exact 24 fps profiles with two plain 1080p picture layers, a full lower
layer, a verified upper-layer range, and an exact parent clock. Each
source must resolve to a verified managed DNx picture with the matching
original media. The writer retains the child V1 and V2 ranges and
keeps the parent's mono edits independent. Direct CLI and saved jobs
produced the same structure from the derived 28-frame AVB. The lower
picture occupies [0, 28) and the upper [4, 23). Other controls and
uncertain media or picture overlaps stay outside the profile.

The exact 24 fps profile uses the verified zero-origin DNxHD LB and
PCM16 package. The saved sequence TC1 must match the converted parent
origin. A derived 24 fps control recovered the V1 gap at record frames
[56, 84) as an editable nest. Inside it, V1 covers [0, 28) from source
[21, 49) and V2 covers [4, 23) from source [29, 48). The direct CLI and
saved job agree on the parent and child display clocks and all 12
parent audio placements, including the stereo combiner on A3. The
saved source selection matches the flat control for all 196 frames.
A different framing record stays a reported omission. Native
playback and the linked AAF comparison remain pending.

The two-bin control selects the same saved picture master under two
different names. Both bin orders bind the layered edit to the item in
its own bin. The direct CLI and saved job produce matching child tracks
and source ranges. At 23.976, copies of a layered DNxHD LB/PCM24 source
can share the direct link when the full master ID, complete picture
and audio package, and verified file clocks agree.

The writer evaluates each occurrence in the sequence independently.
In the derived three-occurrence control, both CLI and saved jobs wrote
the first two nests with distinct upper source trims and preserved the
parent clocks and mono edits. The third occurrence stays a reported
loss because its upper trim extends past the linked file. An overlap
or an unresolved picture span on the same track blocks that placement.

Known record ranges let a layered Submaster share a track with
separate marked edits, graphics and dissolves. In a derived AVB, the
28-frame nest stayed editable and the next cut kept its one-frame
range as a clip marker in both the CLI and job outputs. The structural
control also preserves a separate dissolve and title. When a title
overlaps a nest on the same track, the converter reports the affected
nest as an omission.

An exact marker on the outer Submaster can travel with the nest when
its saved local and record positions place its start inside the effect.
Premiere keeps the full marker range, text and color on the parent
sequence and reports the ownership change. In the derived 24 fps
control, both entry points preserve the nest at frames [56,84) and the
marker at [81,87), three frames into the next cut. A marker on an inner
component or one that starts outside its saved owner still blocks the
nest.

The same ownership check covers a range continuing into a second
qualified Submaster. A derived 23.976 AVB keeps nests over [0,28) and
[29,57), the one-frame cut between them, and the full marker [25,33) on
the parent. Neither the marker range nor the recovered first nest
clears the separate source-overrun report for the third occurrence.

Premiere 27.0.0 build 88 saved and reopened the derived 23.976 control.
The parent kept the full blue range, both nests and the intervening cut.
The saved child ranges, channel assignments and nine media paths match
the generated project, and Premiere reported zero offline items.
All 57 picture frames exported from the reopened timeline match the
unmarked companion pixel for pixel, including both sides of the cut.
This isolates the marker change; the flat picture reference, a native
Avid Collapse and its linked AAF remain separate comparisons.

For the native layered control, use two linked picture clips
with different trims on V1 and V2. Leave a gap before and after V2 so
both inputs are visible over the preserved span. Keep parent audio
separate and use no inner effect. Verify both child tracks and render
through the V2 coverage changes and after trimming the parent. This
test isolates the child track layout before introducing PIP controls.

## Static Avid Corner Pin

The Avid sequence sidecar retains `controls.picture.corner_pin` on each
matching native event. It includes the measured 1920×1080 reference
canvas, four target points in top-left, top-right, bottom-right,
bottom-left order, and four source crop margins. The points use
full-image edge fractions, with X right and Y down. Apply source crop
before mapping the surviving picture through the full-image pin.
The geometry receipt survives when the graph cannot place the media.

This capture uses the compact static 24 fps control set from the
retained Avid renders, with one direct picture input, a filler
background and a convex target. It checks saved parameter identities
and the absence of curve references. Other controls remain in the
native parameter receipt. An isolated explicit pin with verified
square-pixel picture becomes a `picture.corner_pin` effect on a same-rate
24, 24000/1001, 25 or 30000/1001 placement. It carries optional
`source_crop_ltrb` fractions. An absent
field means zero crop. Premiere 27 writes editable Corner Pin and, for
nonzero margins, Rounded Crop. The Rounded Crop offset places the matte
around the intended source rectangle. The two effects render in the
measured order, with the saved component list placing Corner Pin first.
Resolve writes an isolated static pin as an editable Fusion Corner
Positioner on the linked 1080p24 Rec.709 ProRes HQ picture-only profile.
The writer uses the same graph coordinates and converts Y to Fusion's
upward axis. For nonzero crop margins, a Rectangle mask and Matte Control
remove the outside in the original source raster before the pin. Both
controls remain editable. In Resolve Studio 21.2 build 00006, 48 frames
across two trimmed edits matched the native control after save and reopen.
The linked Avid job retained the crop and pin, and all 16 sampled crop
edges matched the retained Avid TIFF. The writer also supports one static
Flop before or after the pin in the same profile. It adds a Fusion
Transform at the saved position in the graph/2 chain. A preceding Flop
feeds the source matte when present. A following Flop reflects the
projected picture and its alpha together. The composition and MediaIn
share one-based frame timing so Resolve keeps the source trims after
archive reopen and inspection in Fusion. The combined native Avid render,
animation and tracking remain open.
[Blackmagic's Fusion matte guide](https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-20-Fusion-Visual-Effects.pdf)
· [Fusion Transform controls](https://documents.blackmagicdesign.com/UserManuals/FusionManual.pdf)

CONV-0099 through CONV-0101

Premiere uses matched 1920×1080 geometry or verified square-pixel linked
picture at the same source and destination raster. This covers explicit
targets with optional source crop and one horizontal reflection before
or after the pin. In twelve derived cases at 1600×900 and 1440×1080,
Premiere 27 kept the four editable target points after Save As and
reopen. All 84 chart points matched the homography within 0.30 pixels,
and sampled crop or picture edges were within one pixel. These compare
Premiere frames to the geometric projection from saved targets.

Premiere preparation and writer validation use the same rule for the
four exact clocks, matching picture and canvas, and qualified linked
rasters. In one Premiere 27 project, 28 derived timelines used the same
compressed ProRes HQ chart pictures at all four clocks. After Save As
and reopen, all 112 decoded frames matched their respective 24 fps
controls pixel for pixel. Editable points, asymmetric crop and both
Flop orders survived. A repeated reopen and render matched all 32
additional cropped Flop frames.

Static 3D Warp pitch and yaw with neutral position, scale, pivot and roll
can also produce `picture.corner_pin`. This path allows up to one
nondefault Perspective axis in the measured range. It records the computed
corners under `controls.picture.projected_corners` and retains the original
parameter values. Premiere exposes editable target points and reports the
loss of the original Rotation and Perspective controls. The full source
plane must have positive projective depth. Camera-derived points retain
the 1920×1080, 24 fps qualification. Both wider-raster camera controls kept their
cuts with an explicit effect loss. CONV-0102

The default camera profile also accepts bounded static Z/X/Y rotation
combined with independent X/Y scaling, an XYZ Axis and XYZ Position.
Avid scales the Axis pivot with the image, rotates around that pivot
in Z/X/Y order, and adds Position before projection. The same
`projected_corners` receipt retains the result alongside the original
controls. CONV-0103

### Saved Avid 3D Warp compositing controls

The owning Warp receipt in `avid.source` includes `saved_compositing`
when saved numeric controls can be identified by their full parameter IDs.
This records the foreground group gate, numeric key mode, foreground
opacity in percent, background color selector, saved hue, saturation
and luminance, and the separate background video-level control. The
receipt retains the underlying parameter values and types alongside the
named fields. Missing, duplicate, nonnumeric and nonfinite values stay
unresolved. The saved scalars can be cached values on animated controls.
The receipt leaves effect-time evaluation, the actual background input
and the resulting alpha open.

Four retained media-offline bins verified the 39% and 14% opacity values,
the background settings, and distinct key codes 1, 3 and 5. The destination
mapping still needs native controls with contrasting inputs and verified
renders. CONV-0193 ·
[Avid foreground controls](https://resources.avid.com/SupportFiles/attach/FXReference.pdf)

The sequence's `avid.source` sidecar also stores `warp_inputs` by Warp
occurrence. Each receipt keeps the track, exact record bounds when known,
effect clock and length, and saved Swap Sources setting. `slots` follows
the native track indices 1 and 2. Each slot stores filler, an exact
same-rate picture reference with its Mob ID and trim, or an unresolved
binding. The retained checkbox pair changes the saved flag while both
slots stay the same. An offline two-source control preserves both trims
when the track records are reordered. Duplicate indices and mixed-rate
branches remain unresolved where the saved bindings are ambiguous or
outside the verified scope. Effective foreground and background,
including any picture coming from the track below, still need a native
render comparison. CONV-0194 ·
[Avid Effects and Color Correction Guide](https://resources.avid.com/SupportFiles/attach/Media_Composer_Effects_and_CC_Guide_2024.x.pdf)

Each Warp occurrence also retains its saved compositing `controls` under
`warp_inputs`. These observations survive when the input branches cannot
produce a picture edit. They keep the full parameter ID, typed scalar,
saved enable flag and whether the curve reference is absent, empty,
populated or unresolved. The record covers foreground and background
settings, Invert Key, Swap Sources, Show Alpha, Reverse Animation and
the Luma Key range, threshold and softness controls. The checkbox
replays preserve the exact single-control changes. A two-source control
keeps the saved key and background settings without a portable Warp
edit. Duplicate controls remain separate observations. The receipt
keeps the scalar and enable flag separate from the curve. CONV-0195

A populated curve adds `curve_data` with its saved value type,
interpolation and extrapolation codes, field setting, and point count.
For up to 64 readable numeric keys, graph/2 retains the original offset pair,
timescale, value, handles, anchor and flags. The two-source opacity
control preserves the pair `[-7, 2]` before the edit and `[11, 2]`
after its visible span. The keys stay in the effect's saved coordinates.
A duplicate property, invalid offset or oversized curve leaves the
key list unresolved while retaining the curve summary. Runtime curve
evaluation and destination opacity remain open. CONV-0197

A Warp with two resolved picture slots and an exact record interval
uses the existing `effect_gap` fallback. The sequence marker identifies
the lane and omitted occurrence. Lower tracks and adjacent edits keep
their own ranges. A 24 fps linked control writes a five-frame gap on V2
starting at record frame 3 while retaining one V1 edit and two adjacent
V2 edits. Reordered input records give the same gap. Duplicate indices,
a mixed-rate branch and nested timing without a verified record range
keep their original omission behavior. The gap check also rejects an
overlap with another edit or transition. CONV-0196

The same Warp receipt keeps Border width, softness, color and style
code, Shadow position and opacity, and the Defocus mode and amounts.
Retained saves show that selecting FG Only with amounts 87 and 174
leaves the Internal amounts 2 and 6 intact. Both sets remain in the
graph even when Defocus is turned off. The explicit Plain border code
is distinct from a save that omits the style field. Background Color's
level-limit checkbox has its own ID. The corresponding Border checkbox
and the rendering measurements remain open. CONV-0198

The reader also uses the saved slot numbers when routing a single-picture
Warp with filler in slot 1. Reversing the two track records preserves
the same picture source, trims, Corner Pin and reflection order, and
Linear, Shelf, Bezier and Spline roll keys. The six reordered fixtures
write byte-identical seeded Premiere projects to their ordinary-order
controls. A repeated slot number or missing reference omits the affected
picture and reports its occurrence in Review. Neighboring edits remain
in the project. CONV-0199

#### Native Full Key calibration

Use two 48-frame 1920×1080/24 ProRes HQ sources with video-range Rec.709
and a six-bit frame stamp. Each has three neutral luma bands. The lower
source uses 10-bit codes 128, 320 and 576. The upper uses 768, 512 and
256. Use distinct nonzero source timecodes and trim the lower source
at file frame 4. Begin the upper source at file frame 10 on record frame
4. This gives the upper source a file-to-record offset of six frames.

Put the lower source on V1 for 36 frames. Use V2 for the nine four-frame
intervals below, starting with an empty V2 interval. Keep other picture
controls neutral. Use a fixed rather than animated opacity for each
Warp and verify the saved mode and toggle values before export.

| Record frames | V2 control |
| --- | --- |
| `[0, 4)` | Lower source visible alone |
| `[4, 8)` | Neutral upper source |
| `[8, 12)` | Full Key at 0% |
| `[12, 16)` | Full Key at 25% |
| `[16, 20)` | Full Key at 50% |
| `[20, 24)` | Full Key at 100% |
| `[24, 28)` | Full Key at 25% with Swap Sources enabled |
| `[28, 32)` | Full Key at 100% with centered 50% scale and Background Color off |
| `[32, 36)` | Same as the preceding edit with Background Color on and Hue, Saturation and Lum at zero |

Save the bin, reopen it, and export the full sequence as legal-range
10-bit ProRes HQ. Read both unwarped frame stamps before comparing
opacity. Sample the three band centers at x=320, 960 and 1600 on
row 540. Compare code-value blending and a blend after undoing the
BT.709 transfer characteristic. Test the swapped source choice at 25%,
where the two assignments give different values. Compare the outer
samples of the two scaled edits to isolate generated background color.
The offline harness discriminates these cases on synthetic output.

Avid Media Composer 25.12.0.58863 controls used the same source trims
and a continuous 32-frame V2 overlap. Each 24p export had 36 frames.
At 0% and 100%, the decoded frame stamps matched the lower and upper
source ranges respectively. At 25%, all 32 frames matched the Rec.709
code-value blend at the three band centers. The unswapped codes were
288, 368 and 496. Enabling Swap Sources yielded 608, 464 and 336.
The saved foreground opacity and swap control are already retained
in `avid.source.warp_inputs`. The native effect saves filler in slot 1
with a `TRKR` proxy at relative scope 1, track −1. Slot 2 holds the
trimmed upper source and a scope 0, track −1 proxy. Graph/2 keeps each
proxy on its own numbered slot in `avid.source.warp_inputs`. An absent
proxy has no field, a dangling or unsupported proxy is null, and a
saved picture TrackRef retains its signed relative scope and track.
The reference still needs to be resolved across lower-track cuts,
gaps, and nested effects before writing a destination composite.
The scaled Background Color test and non-neutral key modes remain open.
[ITU-R BT.709](https://www.itu.int/rec/R-REC-BT.709-6-201506-I/en)

#### Native Superimpose and Picture-in-Picture calibration

Superimpose (`EFF2_BLEND_SUPER`) saves static Opacity as a floating-point
percentage in parameter `8d56813d-847e-11d5-935a-50f857c10000`. The
0% and 100% controls select the same respective source ranges as Full
Key. At 50%, all 32 overlap frames gave luma codes 448, 416 and 416,
exactly matching the code-value blend at the three band centers.
Superimpose and Picture-in-Picture save the same slot and proxy pattern
as the tested Full Key occurrence.

Picture-in-Picture (`EFF2_BLEND_PIP`) uses the same parameter for
Foreground Level. The tested edit used 46% level, 50% scale on both
axes and horizontal position 157. The measured foreground edge
agreed with a 301.44-pixel horizontal offset on a 1920-pixel canvas
within one pixel. Changing only Left Crop from 0 to 302 shifted the
measured left edge from x=781 to x=1071 and kept the right edge fixed.
The interior swatches matched the 46% code-value blend. The uncropped
control reopened with editable values and re-exported with identical
decoded picture across all 36 frames. This comparison covers static
controls and opaque Rec.709 sources. Graph/2 preserves each effect's
identity, controls, trim and proxy references on its occurrence under
`avid.source.picture_blend_inputs`. The two scale parameters retain
their original IDs and raw values pending independent axis checks.
Both effects still report unsupported two-input routing. Editable
destination mappings require input ownership and blend qualification.
The local research ledger records the calibration as `CONV-0204`.

### Animated 3D Warp Z Rotation

Compact three-key Linear Warp can carry animated X Position,
Scale Width and Z Rotation together. Graph/2 records the three exact
edit-relative frame positions, normalized X and the saved width and
rotation values in `avid.warp_roll_linear`. Premiere adds the verified
source in-point once to place the editable Motion keys. The profile uses a
centered anchor, neutral crop, full height and an isolated same-rate edit
with no retiming or effect stack.
Preparation and writer validation share the rotation frame clock rule at
24, 24000/1001, 25 and 30000/1001. The first and last keys must still
coincide with the first and last visible frames.
CONV-0109

Graph/2 also carries Avid Shelf as
`avid.warp_roll_shelf`, with the identical edit-relative key layout.
All three source curves must use Shelf. Premiere writes Hold to the
editable Position, Width and Rotation keys, with no spatial tangents on
the held Position values. The value changes at the next saved key, as
confirmed after reopen. CONV-0110

For the same three-control layout, graph/2 distinguishes
`avid.warp_roll_bezier` and `avid.warp_roll_spline`. Both require
zero saved value tangents and temporal handles of 40% of the adjacent
key span. Premiere writes editable Bezier timing with zero slopes and
40% influence, and keeps the Position path straight.
CONV-0111

Premiere admits these roll controls on matched 1920×1080 picture and on
verified square-pixel linked AVC/PCM at a matching raster. After Save As
and reopen, the 1600×900 and 1440×1080 synthetic media controls retained
all nine editable keys at file frames 6, 11, and 18 in each mode. All 728
points across 104 rendered frames were within 0.21 pixels of the expected
pixel-space projection. The new rasters use derived AVBs. Native Avid
render comparisons at those rasters remain pending.

One Premiere 27.0.0 build 88 project tested Linear rotation at all four
clocks with matching 1600×900 linked ProRes and retained Avid curves in
derived bins. The two source in-points put the editable keys at file
frames 6, 11, 18 and 17, 22, 29. After Save As, close, and reopen, all
156 decoded comparison frames matched their 24000/1001 controls. A
second reopen reproduced 104 animated frames. All 728 measured chart
points landed within 0.15 pixels of the saved curves' projection.
A mismatched curve clock omitted the effect and preserved the edit.

A second Premiere 27 project tested Shelf, Bezier and Spline at all four
clocks using the same linked ProRes chart and derived AVBs. The source
in-point put all three editable keys at file frames 17, 22 and 29.
After Save As, close, and reopen, all 260 comparison frames, including
the plain and Linear controls, matched their 24000/1001 counterparts.
A second reopen reproduced 156 animated frames. All 1,092 measured
chart points were within 0.21 pixels of the projected motion. Mixed
interpolation and a changed handle each omitted the effect and preserved
the edit. Native Avid renders at the added rates remain pending.

### Additional static 3D Warp controls

Static Skew uses simultaneous X and Y shear on the source coordinates.
In the tested X-only profile with Z Rotation, rotation follows the
shear. The four derived corners use the same graph/2 receipt and
editable Premiere effect. CONV-0104

Isolated static Position Z in the default-camera profile changes
magnification about the picture center even with zero rotation. The
retained 1080p24 cases at +100 and −100 use the same projected-corners
receipt and editable Premiere output. Other model controls must be
neutral for this depth-only profile. CONV-0105

The retained compact static crop, Scale and Z Rotation profile stays
in the ordinary editable Motion representation. Its exact neutral
controls remain in the Avid receipt without spurious loss entries.
Controls outside the verified envelope retain their previous
diagnostics. CONV-0106

## Nested Avid picture effects

Graph/2 preserves simple chains of Flip, Flop, Resize, and 3D Warp
(CONV-0065). The reader follows the saved picture input, including a
sequence wrapper with one full-span child and zero-length filler. A Warp
background must be filler across the edit. Each stage shares one
same-rate picture range. The graph records stages in input-to-output
order with the existing interpretations of their axes, transforms and
qualified Linear curves.

The retained Avid control saves Flop as the input to an animated Warp.
Graph/2 retains both stages and all of Warp's keys. Branches, split
inputs, and mixed-rate or unsupported wrappers remain outside this
profile. See the [graph contract](GRAPH-MODEL.md).

A bounded static Corner Pin can be a stage rather than a standalone
effect. The source reader follows same-rate full-span reflection inputs
to the media edit, and the graph retains the pin and reflection in saved
order. The native event receipt lists each qualified projective stage
by index, with its original controls and source-space crop, including
when the media does not resolve.
CONV-0107

Premiere 27 writes one static Corner Pin and one horizontal reflection,
with optional source crop, on a same-rate 1080p24 picture edit. For this
combination the standard components execute in reverse saved-list order.
Both orders survived reopening, with the crop adjacent to the pin.
Combined native Avid rendering and Resolve writing remain unqualified.
CONV-0108

### Writing ordered picture chains

Premiere and the internal Resolve writer consume the same ordered stages.
Both support reflections and up to one qualified static or Linear
Resize/3D Warp stage on a same-rate, matched 1920×1080 picture edit.
Premiere also uses verified square-pixel linked AVC MOV or MP4 and ProRes
422 HQ MOV for static and uncropped two-key Linear motion at a matching
source and destination raster. This applies to a single motion effect or
a motion stage in an ordered chain. The saved key span and source rate
checks apply at every raster. The raster decision uses the bound picture
stream. Any audio layout must pass its own checks. Picture-only originals
use the same rule when the source reader establishes an exact fixed-rate
stream. Reopened 24 fps custom-raster ProRes controls verified static
Corner Pin with source crop and both Pin/Flop orders.
It uses the selected destination raster when the Avid sequence canvas is
unknown. Motion uses a centered anchor and neutral rotation. Repeated
reflections combine per axis. Unsupported stages or multiple Motion
stages receive an explicit loss in Premiere or refusal in Resolve.

Premiere applies standard Flip before fixed Motion. When the source puts
a reflection after Motion, preparation mirrors the position and crop in
the native Motion controls. Resolve's Inspector crops before Flip and
then applies Position and Zoom. Preparation mirrors the incoming crop
edges for a reflection before Motion and mirrors position for a
reflection after Motion.
[Adobe effect processing order](https://helpx.adobe.com/premiere/desktop/add-video-effects/types-of-effects/types-of-effects.html)

The retained Avid Flop-before-Warp comparison passed on Premiere 26.5.2
and Resolve Studio 21.2.0.0006 after Save As, close, and reopen. In
Premiere all 48 stacked frames held the same measured edges as the
standalone Warp. The retained Avid comparison frame matched within
one pixel, and saved Flip and all 21 Warp keys survived. Resolve kept
all 144 decoded frames identical before and after reopening across the
stack, standalone Warp, and synthetic reverse-order control. The latter
matched horizontally reflected Warp edges within one pixel. The two
orders differed by up to 480 pixels at a visible edge.

Premiere 27.0.0 build 88 reopened the generated 1600×900 at 24 fps and
1440×1080 at 24000/1001 controls after Save As and close. Derived AVBs
combined retained effect controls with linked synthetic media. At three
frames per timeline, Flop before Warp kept the asymmetric crop bounds,
and Flop after Warp mirrored those bounds. The left edge fell 48 and
44 pixels apart between the two orders, respectively. Both mono edits
survived structural read-back. Native Avid renders at these rasters and
rendered audio comparisons remain pending.

Derived Avid two-key controls on the same rasters preserved the first and
last Linear keys at file frames 6 and 41 after Premiere 27 Save As and
reopen. At five sampled frames per timeline, visible edges were within
one pixel of the bounds projected from the saved position and scale.
Flop before motion kept those bounds. Flop after motion mirrored them.
The paired 1440×1080 Resize and promoted Warp produced identical pixels
at all five samples.

Two-key Linear crop at 24000/1001 also uses the matched linked AVC/PCM
profile. Derived AVBs at 1440×1080 and 3840×2160 retained editable
position, scale and all four crop curves at the same file frames 6 and 41
after Premiere 27 Save As and reopen. Resize and Warp produced identical
pixels at five sampled positions per raster. Flop before Warp kept the
visible bounds, and Flop after Warp mirrored them. The measured low-luma
edges were within three pixels at 1440×1080 and four pixels at 3840×2160
of the geometric projection. Native Avid render comparisons at those
rasters remain pending.

The qualified three-key 24000/1001 trim profiles also use this matched
linked geometry. In the derived 1440×1080 and 3840×2160 head and tail
controls, Premiere 27 preserved each saved position, width, height and
crop key after Save As and reopen, including keys outside the visible
edit. The UHD head trim also retained both Flop orders. Fifty sampled
frames put the visible edges within four pixels of the piecewise Linear
projection. Both insufficient-handle controls retained the picture and
two mono edits with a loss for the omitted effect. Native Avid renders
for these linked rasters remain pending.

Uncropped Linear Resize and 3D Warp use the same trim clock at 24,
24000/1001, 25 and 30000/1001. Preparation and writer validation use the
exact same-rate sequence frame duration. A key's file-relative time is
the edit's source in-point plus its saved frame offset. Fixed Width
retains its own keys, and Elastic retains the saved times for each
qualified track. Keys outside the visible edit require verified source
handles.

One Premiere 27.0.0 build 88 project used derived AVBs and identical chart
packets on all four clocks at 1600×900. Two-key Motion, Fixed and Elastic
Resize head trims, Fixed Width 3D Warp head and tail trims, and an
Elastic 3D Warp tail kept the exact ranges and editable Linear keys
after Save As, close, and reopen. All 1,056 decoded comparison frames
matched their corresponding 24000/1001 controls. A missing handle
omitted only the effect. These are Premiere render comparisons against
the established clock using retained Avid curves. Native Avid renders
at the added rates remain untested.

Two-key and trimmed Linear crop use the same four clocks. The crop edges
retain their own shared key times alongside Position, Width and Height.
Uncropped two-key Motion retains its existing admission.

The Premiere 27.0.0 build 88 crop project used matching 1600×900 linked
ProRes and derived AVBs with the retained Avid crop curves. Resize and
3D Warp head and tail trims retained off-cut keys. Both Flop orders
preserved the expected crop geometry. After Save As, close, and reopen,
all 1,200 decoded comparison frames matched their 24000/1001 controls.
Measured edges on all 1,056 animated frames were within one pixel of
the projection from the saved curves. Missing source handles and a
mismatched crop clock each omitted the effect and retained the edit.
Native Avid renders for the combined trim cases at the added rates
remain untested.

## Compatibility profiles

| Interface | Interpretation |
|---|---|
| `anynle.graph/1`, `bridge`, standalone `avid-reader model`, Python oracle | Retained legacy APIs and interpretation. Graph/1 does not imply known preparation provenance. |
| Normal `any-nle graph` and Avid→Premiere source snapshots | Explicit graph/2, with independent video/audio lanes and owner-local marker witnesses. |
| `premiere-adapter::PreparedPremiere` | Destination preparation and ownership map; not another source document. |
| Held FCPXML conversion | Explicit legacy reader/graph/1 profile. Graph/2 is not implicitly accepted or normalized into this route. |
| Color preview | Reads palette/item/marker facts directly from either graph version; successful timeline preparation is unnecessary. |

Ordinary Premiere media preparation uses `ClockProfile::LegacyImport`. It applies
the exact old fallback rules to the retained **raw** legacy tuple: integer
display-base conversion, zero for absent/drop/off-tick/overflowing legacy starts.
It does not round graph/2's exact media start to imitate old output. Audio never
acquires a clock from adjacent picture. For the sequence zero point only,
preparation can use qualified 23.976 or 25 fps non-drop TC1, 29.97 drop-frame
TC1, or the qualified 24 fps Resolve archive clock. Other sequence origins use
the compatibility fallback. Diagnostics identify the clock interpretation.

Graph/2's optional typed `SourceClock` carries effective count, exact rate,
display base, drop flag and typed resolution provenance. Validation checks exact
origin/offset arithmetic and its agreement with the portable start. Native counts
remain full u64 decimal strings on the wire; missing witnesses stay absent.
Raw compatibility tuples remain separate from this witness.

Sequence canvas is also optional. The centered still-fit profile verifies it from
the saved Avid sequence format and the qualified adapter. For other Avid sequences
the first-picture/default raster remains a `compatibility_frame_size` receipt.
Premiere uses that receipt for existing compatibility behavior.

`VerifiedRewrap` preserves the existing opt-in file-clock normalization at the
picture rate, including base/drop/one-day/shared-source guards. The retained native
36-frame cohort has a separate model/document differential against the frozen
Python oracle. Broader corrected clocks and other changed native output require
their own evidence and application check before becoming default support.

## Destination preparation

The direct CLI and saved jobs share typed Premiere preparation, including the
clock profile, PNG Matte Key recovery and its loss filtering, and selection of
the online, offline or folder-layout writer. The direct CLI retains its
compatibility writer for inputs that use the legacy project construction.

All transition endpoints resolve against immutable original spans before any trim.
Lane-specific trim bounds are calculated together, then compatible picture/audio
placements are assembled. A genuine multichannel combiner stays atomic; expected
native membership is retained independently of surviving decoded channels.

Original transition IDs accompany the Constant Power audio and video approximations.
Typed Premiere preparation retains ordinary two-sided same-rate Dissolves as
editable Cross Dissolve (Legacy) windows. Other qualified video identities use
cuts. Retimed endpoints, ambiguous or unresolved topology and unavailable
handles have named preparation failures. A raw native transition cannot disappear
behind a generic loss and leave unnormalized overlaps in the output.

Unknown/nonzero item-to-file origins cannot become implicit-zero bin masters,
including bin-only clips. Absolute frame maps retain their existing absolute
meaning; offsets are never added a second time. The current writer has no general
origin-correction interface; a future slice needs its own qualification.

Markers retain owner-local position/rate and occurrence bindings in the source.
Only a qualified transform yields portable record time. Unsupported marker timing
is a scoped loss. A known record marker whose edit ownership is removed by trimming
moves to the sequence with an ownership-loss report. The same fallback keeps the
exact record range when the edit marker's source position cannot be expressed in
Premiere ticks. A graph round trip verifies the written sequence range and loss
report using a derived source time. Master clip markers carry
their saved track. Older graph/2 files can recover the track from their native
marker receipt. Inheritance requires matching duration, content and creation
stamp on the same source track at the exact source time. Unqualified retimed
marker transforms remain unresolved.

Typed writer models are validated before native routing or document construction.
This closes paths where malformed public Rust values could panic before validation.
Destination tick, frame-grid, scalar, count and layout limits remain separate from
the graph's ability to represent source facts.

## Limits and memory

| Limit | What it bounds |
|---|---|
| Existing AVB read/scan/nesting limits | Per-file decoding and traversal, independently of project summaries. |
| 8 GiB accounted Avid source-facts budget on 64-bit builds | Project-wide retained records, collection/reference slots and owned payload capacity during source construction. Shared payload is charged once and every reference slot separately. The 32-bit reader keeps the 256 MiB bound. |
| 768 MiB Avid adapter construction budget | Native receipts, typed records, and owned graph payload. Conservative reservations can overcount temporary work. The serialized graph has its own limit. |
| 2 GiB linked Entire Project working graph budget | Shared graph and one timeline's construction scratch. The adapter drops consumed source timelines and reaccounts the retained view after each spill. |
| 16 GiB private receipt spool | Complete native timeline receipts and detailed source omissions for linked Entire Project conversion. Each record retains the graph/2 per-record bound. |
| 512 MiB linked whole-project prepared input allowance | Bounds the typed destination payload, separately from the Premiere document's allocations. Standalone typed preparation retains its 64 MiB allowance. |
| 256 MiB encoded graph/2 ceiling | Bounded streaming serialization, counted before caller output; no additional complete JSON value tree. The legacy graph/1 reader retains its 64 MiB ceiling. |
| Existing 16 MiB pretty-model JSON input boundary | Standalone/legacy model-file interfaces. It is not imposed by a new serializer on the direct source path. |
| Existing writer field/count/track limits | What the current Premiere writer can represent. |

These thresholds are not equivalent and do not promise peak RSS. One decoded bin
at a time still permits decoder/compiler temporaries. Source facts, graph, prepared
model, document, assets and media-scan overhead must be measured together. The
initial native comparison's high-water memory follows the existing MXF scan;
referenced-only retained headers do not imply that header-reading temporaries vanish.

Linked Entire Project jobs stage native timeline receipts and high-volume
omissions in a temporary file. Only the receipt fields needed for native
preparation stay in the working graph. The job still validates the shared
media, bin, placement, clock, and ownership references. The 256 MiB graph/2
limit remains on graph serialization; native preparation from the staged
view does not serialize that view as a graph/2 export. Successful jobs
copy detailed receipts to an exclusively created `.anynle-debug.jsonl`
file beside the Premiere project. Each record names its owner and includes
its native facts or loss. The temporary file is released on failure or
cancellation. Source capture and Premiere's document builder have separate
allocations beyond the graph working budget.

While creating a new sequence, the Premiere writer accumulates the maximum
source extent it would have published after each clip insertion and trim.
It publishes the extent once per sequence, including any larger temporary
extent inherited from a clip's default trim. This avoids repeatedly scanning
all earlier edits and preserves the saved duration of the backing sequence
sources.

After preparation, the document tree is released and its encoded payload remains
opaque. The app releases the typed writer model before media execution. LUTs retain
validated recipes and expected hashes rather than duplicate full external payloads;
publication regenerates and verifies them without drawing UUIDs or rebuilding the
document.

### Saved graph loading and Resolve preparation

Resolve linked preparation builds a borrowed sidecar index after graph
validation. Lookups by adapter and object ID reuse that index in the edit
loop. The text decoder moves the parsed sidecar payloads into the graph.
Both decoder entry points share the same shape checks and validation.
Source types, validation, reading, and bounded writing have separate
modules within graph/2.

On macOS arm64, Rust 1.100.0 and FFmpeg 8.1, five paired release runs
measured the following medians. Parentheses show the observed ranges.

| Measurement | Baseline | Final |
| --- | ---: | ---: |
| Resolve preparation, 32 timelines × 512 edits | 3.47 s (3.44–3.54) | 0.62 s (0.62–0.64) |
| Peak RSS loading the 64 MiB sidecar-heavy graph | 197.9 MiB (197.9–198.0) | 133.6 MiB (133.5–133.7) |
| DRP writing and staged read-back, four timelines × 128 edits | 342 ms (337–346) | 346 ms (344–376) |

Both optimizations passed their performance gates in isolation. The
final build repeated the gains and stayed within the small fixture's
runtime and memory tolerances. At the time of this benchmark, the
32-timeline stress case hit the 8 MiB archive XML budget after
preparation, so the write measurement uses the smaller fixture.
The full case later passed [linked archive qualification](RESOLVE-PROJECT-CORE.md#archive-validation-and-limits).

Graph re-encoding matched byte for byte through both decoder entry
points. The retained selection, editable multicam, and fractional
clip-edge fade replays produced matching DRP XML and decoded fields
after mapping generated IDs. The replay also kept the same refusal
messages for invalid graph inputs and fade ownership or length.

## Publication and failures

Predictable model/document/LUT-name failures precede media creation. Planned LUT
names are checked against existing paths, other allocated names and the requested
snapshot path. A late collision refuses publication; an embedded path is not renamed.

New projects use native exclusive rename where supported, otherwise same-directory
hard-link publication. Neither path replaces an incumbent file, directory or dangling
symlink. Unsupported filesystems refuse safely. Intentional `save_in_place` replacement
is a separate API.

Successful publication remains successful if stage cleanup fails; its warning names
the retained staging path. Failed cleanup after a failed write/publication preserves
the original failure and also names its retained stage. A published project with a
failed completion receipt is reported as **published**, with retained project/media
paths. Failure receipts are best effort; stderr still identifies completed and
retained outputs, including snapshots and assets.

The ordering is not an atomic transaction across directories. Exclusive publication
provides visibility safety, not a power-loss durability guarantee. Verified outputs
are never deleted to simulate rollback.

## Acceptance and corrections

Use `scripts/translation_acceptance.py` with the private required-fixture manifest.
It checks the actual checkout and fixture aliases, nonempty golden/corpus denominators,
required backend tests, and source stability during each suite. Missing required
fixtures, ignored backends and vacuous golden runs fail acceptance.

```sh
python3 scripts/translation_acceptance.py run --manifest PRIVATE-MANIFEST.json \
  --converter-root . --log-dir NEW-RECEIPT-DIRECTORY --target-dir PRIVATE-BUILD-DIRECTORY
```

Freeze the manifest from independent pre-change references. Never recapture it just
to fit a failing result. The current task's augmented manifest also pins the retained
source-bin/native cohorts; their differential receipts supplement the test-runner gate.

The private task audit freezes the pre-change executable, Python source, settings,
fixture hashes, expected successes/refusals, diagnostics and output hashes. Differential
checks require byte-identical seeded projects and LUTs for unchanged cases. Source
snapshot schema changes and each diagnostic/refusal difference receive individual
review; parity is not weakened globally. See
the correction ledger.

Final integration requires a separate pass against the canonical checkout, protected
hash verification, retained native measurements without editors, required fixture
coverage, memory comparisons and clean Git/worktree inspection. Review establishes
neither new native compatibility nor User acceptance.

## Master output-mix source observation (2026-11-07)

The `avid.source` item sidecar's optional `master_output_mix` is a typed raw observation
of `_AUDIO_OUTPUT_FORMAT`: `{type, raw}`. Current extraction always records it, including
`{type: "absent", raw: null}`. Older graph/2 documents may lack the observation; this is
unknown provenance, not a default mix. Integer, text, hex bytes, explicit null and opaque
bin-local object-reference indices retain their types. An unresolved master carries the
same observation in its bin receipt. Source and encoded graph budgets still apply.

Preparation reports every present setting as an unapplied Avid mixing control; it does
not select the destination bus from that setting. Graph/1 and model/1 have no new fields.
See implementation and regression scope.

## Bounded text extension (2026-11-08)

Source sequences may carry optional `texts`: caption/graphic occurrence identity,
content, lane and qualified start/duration or explicit uncertainty. Empty lists remain
omitted; older graph/2 and graph/1 interpretation is preserved. Selected native title
property roles/list indices remain in `avid.text`, outside neutral rendering semantics.
Prepared text is a separate typed writer extension; model/1 JSON is unchanged.
See [scope, native checks and appearance gaps](CAPTION-TEXT-TRANSLATION-2026-11-08.md).

## Source controls and text from additional adapters

The Avid sequence receipt at `avid.source.native_events` retains saved effect
parameters by native occurrence. `placement_id` names the graph placement
when one was constructed. A null link leaves the original event and its
controls inspectable, including when a media route or timing could not be
qualified. Floating parameters retain the exact 64-bit representation in
`float_bits`. AVUP payloads have a byte count and SHA-256, with inline
`bytes_hex` through 4 KiB. The existing interpreted effect values remain
separate from these observations.

The internal Premiere source reader emits sequence captions as `texts` with
exact rational record spans and source occurrence IDs. Saved caption text
and ticks remain in `premiere.captions`. Premiere sequence Comment
markers with qualified fields keep exact ticks in the sequence's
`markers` array. Native payload receipts in `premiere.markers` identify
each occurrence, including markers the reader cannot interpret. Resolve
preparation decides which marker positions, spans and colors it can
write. Unsupported occurrences remain visible at Review and require
approval for that run.

## Picture reflection

Graph/2 represents a reflection as `picture.reflection` with boolean
`horizontal` and `vertical` axes. At least one axis must be active. In a
picture of width W and height H, the active axes reverse sample coordinates
as x → W − 1 − x or y → H − 1 − y. A reflection leaves the edit span and
source timing intact. A chain retains the reflection's position relative to
other picture stages.

Avid's unpromoted single-input Flop, Flip and Flip-Flop select those axes
with `EFF2_BLEND_FLIP_HORZ`, `EFF2_BLEND_FLIP_VERT` and
`EFF2_BLEND_FLIP_BOTH`, respectively. The inspected saves share three PRIT
values and the same decoded FXPS defaults. The saved effect identity remains
in the placement's `avid.source.native_reflections` receipt. Avid documents
Grid as a preview guide. Promoted 3D and multi-input records require their
own interpretation. [Avid Effects Guide](https://resources.avid.com/SupportFiles/attach/Media_Composer/Media_Composer_v2025.x_FX_Guide.pdf).

The Avid reader and Resolve read-back emit the shared effect. Both destination
adapters accept the prior `avid.flip` and `resolve.flip` spellings. Premiere
reports a loss for a standalone reflection paired with a separate transform
when the input does not specify a qualified order. Existing ordered chains
still preserve the retained Avid Flop inside Warp case. The new encoding
produced the same saved Premiere filter parameters and Resolve effect bytes
as the earlier controls.

## Group source facts follow-up (2026-11-08)

The optional graph/2 root collections `multicams` and `multicam_uses` retain supported
source layouts, independent lane alternatives and structural selector occurrences.
They are omitted from group-free encodings. Known filler intervals are distinct from
unresolved layouts; ambiguous full-ID/offset matches are retained without selecting a
candidate. Native marks, selector flags and endpoint receipts remain sidecars.

For a resolved occurrence, each bound leaf edit must occupy the occurrence's
destination track and media kind. The source lane and selected alternative may
have different indices from the destination track. A member piece with one
matching physical stream and a known item-to-file offset must fit that stream
after applying the offset. An exact end at the stream boundary is valid.

The typed native rewrap path prepares a bounded editable Premiere multicam from these
facts. Model/1 projection and non-native Premiere routes refuse Groups explicitly;
unqualified group layouts or selections refuse before output execution. The standalone
compatibility reader and held FCPXML profile retain their existing behavior.
See the source-stage boundary
and [current implementation scope](EDITABLE-GROUPS-2026-11-08.md).
