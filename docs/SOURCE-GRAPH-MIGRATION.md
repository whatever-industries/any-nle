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

The separate [Resolve source reader](RESOLVE-SOURCE-READER.md) feeds graph/2
from a bounded native archive profile. Its saved Resolve clocks and channel
selectors retain their own provenance. Product route qualification remains
separate from the source adapter.

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

After Save As and reopen in Premiere 27, the head-trimmed CLI and saved-job
projects match all 29 linked AAF picture frames. The two isolated mono
lanes cover 58,058 samples each. A2 matches exactly, and A1 differs by
at most one PCM24 count per output channel under center pan. A parent with
the nest two frames into the sequence also retains its later child clock.
The qualified profile covers same-rate online 1080p23.976 direct managed
picture. Other Submaster shapes keep the existing conversion behavior and
report a flattened boundary where the picture edit survives.

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
native parameter receipt. An isolated pin with verified square-pixel
1080p24 picture also becomes a `picture.corner_pin` effect on the
placement. It carries optional `source_crop_ltrb` fractions. An absent
field means zero crop. Premiere 27 writes editable Corner Pin and, for
nonzero margins, Rounded Crop. The Rounded Crop offset places the matte
around the intended source rectangle. The two effects render in the
measured order, with the saved component list placing Corner Pin first.
Resolve writing remains open.
CONV-0099 through CONV-0101

Static 3D Warp pitch and yaw with neutral position, scale, pivot and roll
can also produce `picture.corner_pin`. This path allows up to one
nondefault Perspective axis in the measured range. It records the computed
corners under `controls.picture.projected_corners` and retains the original
parameter values. Premiere exposes editable target points and reports the
loss of the original Rotation and Perspective controls. The full source
plane must have positive projective depth. CONV-0102

The default camera profile also accepts bounded static Z/X/Y rotation
combined with independent X/Y scaling, an XYZ Axis and XYZ Position.
Avid scales the Axis pivot with the image, rotates around that pivot
in Z/X/Y order, and adds Position before projection. The same
`projected_corners` receipt retains the result alongside the original
controls. CONV-0103

Compact three-key Linear Warp can also carry animated X Position,
Scale Width and Z Rotation together. Graph/2 records the three exact
edit-relative frame positions, normalized X and the saved width and
rotation values in `avid.warp_roll_linear`. Premiere adds the verified
source in-point once to place the editable Motion keys. The admitted
1080p24 profile uses a centered anchor, neutral crop, full height and no
retiming or stack. CONV-0109

The same bounded profile also carries Avid Shelf as
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
The current construction supports reflections and up to one qualified
static or Linear Resize/3D Warp stage on a same-rate, matched 1920×1080
picture edit. Motion uses a centered anchor and neutral rotation. Repeated
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
preparation uses the qualified direct 23.976 non-drop TC1 when present. Other
sequence origins retain their compatibility fallback. Diagnostics identify the
clock interpretation.

Graph/2's optional typed `SourceClock` carries effective count, exact rate,
display base, drop flag and typed resolution provenance. Validation checks exact
origin/offset arithmetic and its agreement with the portable start. Native counts
remain full u64 decimal strings on the wire; missing witnesses stay absent.
Raw compatibility tuples remain separate from this witness.

Sequence canvas is also optional. A clip's raster does not establish the native
sequence canvas. Current Avid extraction leaves it unknown and preserves the exact
old first-picture/default tuple solely as `compatibility_frame_size`. Both existing
Premiere profiles explicitly consume that receipt; they do not promote the fallback
to a source fact or silently apply a future corrected canvas.

`VerifiedRewrap` preserves the existing opt-in file-clock normalization at the
picture rate, including base/drop/one-day/shared-source guards. The retained native
36-frame cohort has a separate model/document differential against the frozen
Python oracle. Broader corrected clocks and other changed native output require
their own evidence and application check before becoming default support.

## Destination preparation

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
moves to the sequence with an ownership-loss report. Master clip markers carry
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
| 64 MiB accounted source-facts budget | Retained records, collection/reference slots and owned payload capacity during source construction. Shared payload is charged once and every reference slot separately. |
| Additional adapter construction accounting | Native receipts, typed records and owned graph payload; refuses before retained growth. Conservative reservations can overcount released temporary work. |
| 64 MiB encoded graph ceiling | Bounded streaming serialization, counted before caller output; no additional complete JSON value tree. |
| Existing 16 MiB pretty-model JSON input boundary | Standalone/legacy model-file interfaces. It is not imposed by a new serializer on the direct source path. |
| Existing writer field/count/track limits | What the current Premiere writer can represent. |

These thresholds are not equivalent and do not promise peak RSS. One decoded bin
at a time still permits decoder/compiler temporaries. Source facts, graph, prepared
model, document, assets and media-scan overhead must be measured together. The
initial native comparison's high-water memory follows the existing MXF scan;
referenced-only retained headers do not imply that header-reading temporaries vanish.

After preparation, the document tree is released and its encoded payload remains
opaque. The app releases the typed writer model before media execution. LUTs retain
validated recipes and expected hashes rather than duplicate full external payloads;
publication regenerates and verifies them without drawing UUIDs or rebuilding the
document.

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

The typed native rewrap path prepares a bounded editable Premiere multicam from these
facts. Model/1 projection and non-native Premiere routes refuse Groups explicitly;
unqualified group layouts or selections refuse before output execution. The standalone
compatibility reader and held FCPXML profile retain their existing behavior.
See the source-stage boundary
and [current implementation scope](EDITABLE-GROUPS-2026-11-08.md).
