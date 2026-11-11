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
moves to the sequence with an ownership-loss report. Inheritance uses the retained
content/creation-stamp rule at an exact source time; marker UUID equality is not
assumed. Unqualified retimed marker transforms are not reconstructed heuristically.

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
and ticks remain in `premiere.captions`. Sequence marker occurrences keep
bounded native payload receipts in `premiere.markers`. Resolve turnover
reports the omission of each caption at destination review and requires
approval for that run. Caption content and timing remain graph facts
regardless of the Resolve destination's support for writing them.

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
