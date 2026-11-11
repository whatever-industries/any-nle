# Internal Resolve project writer

The experimental Rust boundary is `crates/resolve-adapter`. It borrows a
graph/2 document, selects a sequence by exact ID, validates a verified media
binding and prepares typed record/source ranges. The archive writer consumes
those records offline. The app and job dispatch do not expose a Resolve route.

## Qualification profile

- One selected contiguous straight-cut sequence, exact 24000/1001, a known
  non-drop direct Avid TC1 or a qualified saved Resolve sequence extent,
  explicit 1920×1080 square-pixel destination, V1 and two independently
  placed mono lanes.
- One verified progressive DNxHR LB Rec.709 OP1a file with two PCM24/48 kHz
  mono streams. The binding's SHA-256 pins media already qualified for essence
  preservation. The harness rechecks bytes, container metadata, clock and
  physical selectors. It does not rewrap media. Logical Avid labels are
  validated separately from physical OP1a stream indices. For the Resolve
  source profile, the saved path, video cache clock, and both native
  selector-to-cache-track mappings must agree with the verified binding.
- The writer preserves bounded, sequence-owned range markers on the exact
  record grid. Blue and Red are qualified for the Avid and Resolve palettes.
  Lavender is qualified for the Resolve palette. Point, clip-owned, authored,
  off-grid, unsupported-color and colliding markers produce occurrence-specific
  loss lines. Unknown offsets, unhandled source losses, text, retiming,
  transitions, wider routing and Groups are refused in this profile.
- For a qualified Resolve source, the writer reconstructs an isolated
  static clip volume from −6 through +3 dB, default linear-amplitude fades,
  or two linear-dB volume points with endpoint hold on a direct mono edit.
  Static volume uses the placement's dB value. Fade lengths are measured
  from the clip edges.
  Volume key positions use exact item-source frames and must lie in the
  used source range. Fades and volume keys require the matching Resolve
  source receipt. Preparation resolves only the corresponding loss on
  that placement. Combined controls, separate gain layers, unknown curves,
  and other effects still refuse. Project and mixer settings come from
  the pinned destination skeleton.
- The qualified two-mono Resolve mixer accepts static track faders and
  one main stereo bus fader from −6 through 0 dB in exact tenths.
  Track level comes from the graph track, clip volume stays on the edit,
  and main bus level comes from the Resolve sequence receipt.
  The writer patches the separate fader words in the pinned mixer.
- For static track mute the writer updates the timeline track flag and
  both mute rows in the pinned mixer from the qualified Resolve receipt.
  Muted tracks keep their placements and all gain settings. Lock, solo,
  bus mute and other routing are outside this profile.
- The caller supplies the private, hash-pinned Resolve Studio 21.2.0.0006
  synthetic skeleton. Its opaque project/Fairlight settings, native identities
  for the fixed track layout, and neutral item defaults are retained.
  Timeline registration and media/placement references are validated. The
  writer generates the current media path and cache, file-package identity,
  clock, frame count, mono mappings, edit IDs and ranges. No media donor
  archive or running Resolve is needed for the bound file.
- Archive input and output are bounded. Output uses exclusive same-directory
  hard-link publication and refuses an incumbent or dangling symlink. Filesystems
  without hard links fail safely.

```text
resolve-core-probe GRAPH SEQUENCE-ID 1920x1080 BINDING SKELETON FFPROBE OUTPUT.drp
```

The internal binding JSON has `item_id`, absolute MXF `path`, `sha256`
and exactly three `streams` records. Each record has a graph `media_id`,
physical `index` (0, 1, 2 in order), and `channel` (null, 0, 0).
This is a qualification harness input, separate from public job schemas.

## Direct Avid OP-Atom

The internal automatic route takes a saved AVB and native media roots.

```text
resolve-avid-probe BIN.avb SEQUENCE SKELETON.drp FFPROBE OUTPUT.drp MEDIA_ROOT...
```

Select a sequence by exact graph ID or unique saved name. The Avid adapter
resolves file packages from the supplied roots. The Resolve adapter selects
picture, A1 and A2 by their saved track labels, verifies the same physical
MXF profile used by the core probe, then rechecks the bin and media digests
before publishing. A missing or ambiguous selected sibling prevents output.
This route supports one 1920×1080 square-pixel timeline and up to 64
referenced DNxHR LB items. Each item uses its own picture and two mono
MXF caches. Each edit uses the package, source clock and playable range of
its selected item, including across split audio and picture cuts.
Its first direct AVB conversion preserved all nine split edits and both
original mono paths after Resolve Save Project As and reopen. All 72 picture
frames and 144,144 PCM24 sample frames per isolated lane matched the
qualified OP1a control.

A two-item test alternated DNxHR LB sources with independent split cuts on
V1, A1 and A2. Resolve Save Project As and reopen preserved nine edit ranges
and all six original MXF locators and package IDs. The original audio was
silent. A separate save and reopen used disposable MXF copies with four
distinct PCM signals. All six isolated edit spans matched their assigned
source within one PCM24 code value after accounting for the measured
stereo-bus gain and excluding the short edit-edge ramps.

The core probe also accepts an internal binding with `item_id` and three
`siblings` records in picture, A1, A2 order. Each has `path`, `sha256`,
`media_id`, physical `index` and `channel`. The tested Avid MXFs retain
placeholder stream references to the other siblings. The physical stream
indices are 0, 1 and 2 in their respective files. Each mono stream uses
channel 0.

The writer requires progressive square-pixel 1080p DNxHR LB Rec.709 and
two PCM24/48 kHz siblings on the exact 24000/1001 clock. It checks the
master and all three file package identities, graph track receipts, paths,
source clocks, sample counts and packet spans. A single 16 or 24 KiB DNx terminal packet may follow the playable
pictures if all its bytes are zero. The writer leaves
all original bytes in place and rechecks their digests before publication.

Resolve stores one picture cache and two separately addressed audio caches
on the same media pool item. Timeline selectors 0 and 1 each address track
0 of their respective mono cache. The timeline uses the picture locator
and media pool ID for all three lanes. This graph-driven writer is internal
and takes one straight-cut timeline.

In Resolve Studio 21.2.0.0006, the generated split edit survived Save
Project As, close and reopen with its three original paths and both saved
mono selectors. An independent archive check verified all nine source and
record ranges. Two reopened renders each matched a freshly rendered OP1a
control on all 72 decoded pictures and 144,144 PCM24 sample frames per
isolated lane. The two lane renders were distinct. Source hashes were
unchanged. The tests used the single straight-cut profile above.

The native layout evidence is RESOLVE-0014 at Resolve knowledgebase commit
`24d0bd6dec7ea62e1c406711ce877a354ba42705`, finding SHA-256
`d9a4130d43cc51c0c59f9e760827443f6231709439de07a293fced65c46c68a3`.

## Native results and evidence

The retained split edit opened and reopened with nine placements, the exact
01:00:00:00 start and both independent mono lanes. A one-frame origin variant
preserved all picture and audio. A one-frame first-video source trim matched
a separately authored native trim control. All variants preserved exact source
ranges, media and routing on re-export and re-import. All 72 picture frames and
each 144,144-sample PCM bus isolation matched their native controls byte for
byte. The stereo bus's approximately −3 dB gain is destination mix behavior.

The Resolve authority contains RESOLVE-0001 (archive and origin),
RESOLVE-0002 (linked media and audio) and RESOLVE-0003 (native Group switch).
The corresponding private fixtures are pinned in its native fixture catalog.
Read these findings alongside the [shared effects audit](SHARED-EFFECTS-AUDIT.md).
The separate [editable Group profile](RESOLVE-EDITABLE-GROUPS.md) builds on
this writer. Other builds/platforms, arbitrary media and product exposure
remain open.

The static volume encoding uses RESOLVE-0008 at Resolve knowledgebase
commit `2961617634f305567603f77bf37533dfb173c9ca`, finding SHA-256
`76129cdc40e1c16d65125153669a996adc5c9f0119bbd9aef08b0d8a18351238`.
The fade and automation encoding uses the same-build byte and PCM witnesses
in RESOLVE-0009. Its [reader evidence pin](RESOLVE-SOURCE-READER.md#bounds-and-validation)
also applies to writer admission. Four generated archives tested both
envelope shapes at source In 2 and 6. Resolve reopened all nine placements
in each archive and re-exported the same decoded clip controls. Each affected
lane, the two untouched A2 controls, and the shifted neutral control matched
their retained PCM24 references over all 144,144 sample frames per render.
The static-volume project was saved as a new Resolve project, closed,
and reopened. The −6 dB A1 and +3 dB A2 clips retained their saved controls,
and each lane and its neutral counterpart matched its PCM24 reference
over all 144,144 sample frames. The neighboring spans remained identical
to the neutral render.
Track faders at −3 dB on A1 and −6 dB on A2 were verified alongside the
original clip volumes. The generated Save As copy matched the native
reference on both lanes over all 144,144 PCM24 sample frames per lane.
The track fader evidence is pinned in [the reader profile](RESOLVE-SOURCE-READER.md#bounds-and-validation).
The static main bus comparison kept the track and clip levels and added
a −3.7 dB bus fader. Both generated isolated lanes matched the native
reference over all 144,144 PCM24 sample frames after Save As and reopen.
The track mute test preserved all those levels. After Save As and reopen,
A1 produced exactly zero PCM24 across 144,144 sample frames with A2
disabled for isolation. A2 matched its unmuted reference sample for
sample with A1 still muted.
General project and Fairlight mixer round trips remain outside this profile.

## Linked project profile

The internal `resolve-linked-probe` prepares an ordered set of exact timeline
IDs, explicit timeline bin assignments, and verified original MOV/WAV bindings.
The original one-sequence DNx and Group harnesses keep their existing contracts.
The new internal manifest has `root_bin_id`, `timelines` and `media`.
Each timeline supplies `id`, `bin_id` and a `canvas` pair. Each media binding
supplies `item_id`, `bin_id`, absolute `path`, `sha256` and a `streams` list
of graph media ID, physical stream index and optional physical channel.

The initial profile admits V1 and two neutral, independent mono lanes,
contiguous same-rate cuts, and 1920×1080 square-pixel canvas. One project
uses exact 24 or 24000/1001 with independently qualified non-drop starts.
Native Resolve receipts retain each timeline's owning bin. For Avid inputs
the caller supplies the destination bin explicitly. A complete Avid project
capture and product route remain qualification tasks.

Original media are read in place. ProRes HQ is progressive 1080p, 10-bit
4:2:2 with physical picture stream zero, an exact constant packet clock,
and either the witnessed unspecified primaries/transfer or tagged Rec.709.
The initial auxiliary-track case is the retained 24 fps zero-origin
QuickTime timecode track, which has the same duration and time base as picture.
Other timecode origins and auxiliary tracks require their own evidence.
PCM WAV is 48 kHz, 16- or 24-bit, mono or stereo. A stereo source retains
its stereo pool identity while independent mono edits select left and right.
The saved WAV import frame basis is separate from its physical sample count.

Archive generation builds timeline registration, nested bin relationships,
per-timeline containers, media references and marker ownership. A folder's
child list in its compressed FieldsBlob must agree with the XML parentage
and archive paths. The reader now checks those relationships. An initial
candidate lacking the child lists passed the older reader but lost nested
media on native import. It is retained privately as a failed control.

The writer reads its staged output through the source reader and compares
the prepared identities, ownership, ranges and channel selections before
exclusive publication. Original hashes are rechecked immediately before
publication. Limits include 64 selected timelines, 256 media bindings,
256 source bins, 512 edits per lane, 16 bin levels, 2 GiB per physical file,
8 GiB per selected media set, and 8 MiB generated XML. The source graph
and archive reader retain their separate bounds.

Same-build scripting checks saved, closed, reopened and re-exported the
multi-timeline, duplicate-name, two-rate and stereo-source candidates.
The two 23.976 timelines matched all 48 decoded frames per timeline and
both isolated 96,096-sample PCM24 lane renders against native controls.
The 24 fps output matched its 48-frame native picture control. Stereo
PCM16 and PCM24 outputs preserved independently selected channels.
The 24 fps tagged ProRes case also matched its direct-import picture control.
These ordinary save/reopen checks leave Save Project As pending.

The sample comparisons observe a 72-sample linear edge ramp on these native
WAV edits. The PCM16/PCM24 measurements match the expected source sample
positions and destination mix level to within one PCM24 code value when
that native envelope is included. The converter adds no corrective ramp.

## Combined grading turnover

`resolve-linked-probe` also accepts the pinned direct MXF skeleton as an
optional seventh argument and the pinned Group skeleton as an eighth
argument. The internal manifest's optional `atom_media` array holds
`{bin_id, binding}` records. Each binding uses the direct route's
`{item_id, siblings}` shape with explicit picture, A1, and A2 file
identities. A selected Group timeline adds a `group` object with its
owning `bin_id` and the two `cameras` item IDs in saved angle order.
Every Group camera needs a verified split MXF binding. The project can
register one Group timeline at the qualified 23.976 rate. The same pool
items can supply straight cuts and Group members. The staged reader
checks original locators and package identities, plus both mono selectors,
edit ranges, and each saved angle. Publication rechecks all sibling bytes.

The same-build research project reopened at 01:00:00:01 and 01:00:00:00
with six bins, five media items, and thirteen placements across the two
timelines. One split MXF master is shared across both timelines. Its
other footage is a linked ProRes HQ MOV, and the straight-cut audio
uses linked PCM16 and PCM24 WAV. The Group keeps its editable A/B/A
picture sequence with both mono lanes on camera A. A picture-only
selector change survived native reopen and changed precisely the first
48 frames out of 145. Each isolated audio lane had 290,290 PCM24 sample
frames. Samples 10,000 onward were identical across the switch and
within one PCM24 code of the expected sibling samples after the measured
native stereo-bus gain. The startup envelope differed on A2.

The retained linked MOV/WAV project was saved and reopened through
Resolve scripting. Fresh PCM16 and PCM24 stereo insertions with explicit
left/right selectors and nonzero trims also reopened and matched the
retained isolated references over all 96,096 PCM24 sample frames per lane.
File > Save Project As is a separate pending UI check.

The Resolve authority revision `24d0bd6dec7ea62e1c406711ce877a354ba42705`
pins RESOLVE-0004 at
`11564bb16ace6f62f2c36e2d3fb7e6d4a6068df4314f4e07509391bca80d6fae`,
RESOLVE-0013 at
`fefd639ebaa57569a76c5f3b05623008eb29d16d51f8820439028a74e8213508`,
and RESOLVE-0014 at
`d9a4130d43cc51c0c59f9e760827443f6231709439de07a293fced65c46c68a3`.
The private same-build split MXF reference Group archive has SHA-256
`d3670d1c665f200fd36182090707bc0f504d0e9c1a66069fa517a8d00a3adeb2`.

### Static picture controls

The linked project writer accepts graph/2 static position, independent X/Y
scale, and four crop edges on same-rate 1920×1080 V1 cuts. Resolve's saved
vertical position is positive up. Position and crop use frame fractions,
and scale uses a multiplier in the saved parameter. The graph continues
to use Y down and scale percent. The bounded profile admits position
within five percent per axis, scale from 87.5 through 112.5 percent, and
crop from zero through five percent per edge. A clip that combines
nonneutral transform and crop requires a separate order control.

The eight single-property controls each matched their reopened native
reference on all 48 decoded frames. The effect applied to exactly the
first 24 frames in each case. A separate native left-aligned cross
dissolve requested 12 frames and saved eight, matching the incoming
clip's available handle. Avid overlap-to-cut timing and dissolve writing
remain open.

### Flip, Flop, and Flip-Flop

The linked project writer translates isolated Avid Flip, Flop, and
Flip-Flop to Resolve's editable Inspector controls. Flip reflects the
picture vertically, Flop horizontally, and Flip-Flop along both axes.
The writer reads the exact axis flags from graph/2 and accepts a Resolve
reader's equivalent receipt. It refuses reflection combined with
other picture controls or in an unqualified placement.

Same-build Resolve Studio 21.2.0.0006 exports identified two independent
static boolean slots in the motion section of `EffectFiltersBA`. A
picture-only, 1080p24 Avid UME control referenced the same MOV and
36-frame source range in four edits, starting three frames into the
source. After Save Project As, close and reopen, the four edits retained
their source link, trims, timing, and Inspector flags. All 144 decoded
frames matched a reopened native Resolve reference. A separate nested
Avid effect control refused before publication. The standalone
straight-cut and Group probes retain their separate admission profiles.

### Linear animated Resize and 3D Warp

The linked project writer converts the qualified two-key Avid Resize and
promoted 3D Warp pair to editable Inspector Position X/Y and independent
Zoom X/Y curves. Each axis has two Linear keys at the first and last
frame of a same-rate, progressive, square-pixel 1920×1080 edit.
Preparation verifies the first-key transform, neutral crop and other
geometry, and the physical source span. It adds the source In once to
each edit-relative key and converts graph Y-down to Resolve Tilt-up.
The three-key profile and its bounded crop case are covered below.
Mixed effect stacks need separate qualification.

A native Resolve Studio 21.2.0.0006 save with a three-frame source trim
put its edit-frame keys 0 and 35 at source frames 3 and 38. The same
encoding was verified on the retained AVB-0033 Resize and Warp sequences.
The private test projected their graph onto a compatible ProRes HQ
derivative of the synthetic chart. Save Project As, close, reopen and
re-export preserved source frames 24 and 71, all four axis values, and
the 48-frame edit in both timelines at exact `24000/1001`. Their 48
decoded frame pairs matched exactly. Compared with the retained Avid
exports, the sampled chart edges and grid lines across every frame
agreed within two pixels horizontally and one vertically.

### Three-key Linear Resize and 3D Warp after trimming

The linked writer preserves the saved Position X/Y and Zoom Y clock
and the separate Zoom X clock on qualified Fixed and Elastic trims.
It adds the edit's source In to each signed Avid key time and checks
the result against the verified physical frame count. Keys before the
visible In and after the visible Out stay editable if the source
contains them. This preserves the saved motion. Later trims in Resolve
follow Resolve's editing behavior.

A native Resolve Studio 21.2.0.0006 control kept source frames 3, 20,
and 38 after trimming to the visible range 6–35. The converted
23.976 project used the retained nine Resize timelines and five 3D
Warp timelines with the verified ProRes HQ derivative of the
synthetic chart. Save Project As, close, and reopen retained the exact
source ranges and every key, including the two Zoom X keys before the
second Fixed head Resize. All 468 decoded frames matched the renders
made before Save As. Resize edges agreed within one pixel of the
retained native Avid measurements across 288 frames. Warp edges agreed
within two pixels of the saved Linear curves across 180 frames. The
retained Warp trim tests have no Avid renders.

### Three-key Linear Resize and 3D Warp with animated crop

The linked writer admits four editable crop edges on the same Elastic
grid as Position and independent Zoom. The three keys span the first
through last visible frame. Crop is stored as source raster fractions
with zero Softness and Retain Image Position off. In that mode Resolve
applies crop in source space before Position and Zoom. The edit's source
In is added once to all eight curves. The retained head trims keep their
12-frame leading gap on the picture-only timeline.

Native Resolve Studio 21.2.0.0006 saves established the crop slots, source
key clock, and the crop-before-motion order using an asymmetric control.
The conversion replay used the retained Avid Resize and 3D Warp baselines,
12-frame head trims and 12-frame tail trims with the verified ProRes HQ
derivative of the chart. Save Project As, close and reopen preserved all
six edits and key curves. All 264 decoded frames matched the pre-save
renders, including both 12-frame black leaders. On the 240 active frames,
each measured edge agreed with the retained Avid renders within one pixel
at the primary grayscale threshold and within two across all three
thresholds. The source-space crop prediction was within two pixels; the
alternative of applying crop after Zoom missed by up to 66 pixels.

This profile is same-rate square-pixel 1080p with neutral background and
other controls. Independent crop clocks, Fixed Width with crop, other
curve modes, and stacks of separate effects remain unqualified.

### Avid UME originals

The internal Avid UME probe reads a saved bin through the same Avid source
adapter used by the Premiere route. Select the sequence by exact ID or
unique name. The probe builds Resolve bindings from graph/2 stream and
channel identities, the saved locator, and the verified original bytes.
It supports explicit saved-root to current-root mappings for moved files.
The writer links those files in place.

```text
resolve-avid-ume-probe BIN SEQUENCE LAYOUT.drp RATE.drp FFPROBE OUTPUT.drp [SAVED_ROOT CURRENT_ROOT ...]
```

The initial AVC profile is progressive square-pixel 1080p24, H.264 High
level 4.0 in MOV or MP4, with untagged color or limited-range Rec.709.
The same-rate edit can be picture-only or use the embedded stereo
48 kHz AAC-LC as two independent mono lanes. Picture presentation
timestamps must cover a complete constant grid, including when B frames
reorder packets. The AAC profile checks the priming packet and declared
playable sample interval. The existing picture-only ProRes HQ and
PCM16/PCM24 WAV profiles also use the shared preparation. The source
sequence requires an exact 24-count non-drop origin and qualified
source offsets. A picture-only timeline keeps the template's two empty
mono tracks. Mixed rates, active pan/gain controls, other audio layouts
and ambiguous source locators need separate qualification.

Resolve Studio 21.2.0.0006 reopened controlled H.264 MOV and MP4 sequences
from saved synthetic Avid bins with nonzero trims. Both picture-only
files and files with AAC were exercised. Audio measurements used direct
Resolve imports with the same mixer settings. The embedded cache uses
the native AAC container settings and saves the left and right mono
selectors. The source and destination keep their separate identities.
The native cache witnesses are pinned by SHA-256
`f7c438a08f14bc8dea1759b25b5a4499c7dfd239c446c59ec2a47d2b097eba2f`
and `5e9344f0d9c27ecc4ab60a133833405bfed7530fe13cc0c6662054e6860cb6cc`
for the untagged and Rec.709 cohorts, respectively.

## Pending editor checks

When an authorized Resolve UI session is available, use the private
linked-project return packet and perform these checks.

1. Open `Final-linked-23976.drp` and Save Project As a new disposable project.
   Close and reopen it, switch between both timelines and export a new DRP.
   Verify the one-frame start difference, nested media bin, shared picture,
   exact trims, and both discrete audio lanes. Repeat with
   `Final-linked-24.drp`, `Final-stereo.drp` and `Final-tagged-24.drp`.
2. In the stereo candidate, insert the existing PCM16 and PCM24 pool items
   afresh and explicitly assign left and right to separate mono tracks.
   Save and reopen. Export isolated PCM24 lane references to the packet's
   `native-return` folder. Compare with the original physical channels,
   including nonzero trims and both clip boundaries.
3. Author one isolated, editable mixed-rate timing control per direction.
   The retained default Resolve controls differ from Avid's phase-aware
   floor sampling. Export the saved control and full numbered-frame picture,
   then isolated audio. Verify every selected source frame, first sample and
   playable tail before admitting an editable mapping.

The private packet identifies the exact local projects, hashes, filenames
and output directory. The roadmap retains these checks under their feature
tasks so they can resume after keyboard and mouse authorization.

Resolve authority revision 6eaa0e747ee98f679d9677d24a102fe033447eb9. Finding content pins:

- RESOLVE-0001 · 8e59632a1bbfee92684a52422bdd7298ac0b0736f983210044ba39a315773784
- RESOLVE-0002 · b759f34eb27fdda2d4a54adb629cc07cc1077d013971df8a70aa40fe6d929753
- RESOLVE-0003 · c6cc5cb45aa4dc90603c2ff85391a15ac0659449d5da235387f09c1c03472176
