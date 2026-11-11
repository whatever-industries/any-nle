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
