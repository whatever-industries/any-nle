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

Sequence markers preserve relative position, duration, title and note.
Blue, Red and Lavender have native color witnesses. Other saved color bits
and custom data remain in a Resolve receipt. Clip-owned marker clocks are
unqualified. Bounded static position, independent scale, crop, and
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
current graph and destination interfaces stay unchanged. Combined
controls, custom fade shapes, keys outside the edit, wider gain ranges,
Group audio effects, other mixer controls, mixed rates, and discontinuous
MultiGroups need separate evidence.

Static track faders on the qualified two-mono Fairlight mixer map to
the graph track's level. Static clip volume stays on the placement.
The static main stereo bus level is retained in the Resolve sequence
receipt as `main_bus_level_db`, with a scoped portability loss that the
internal Resolve writer resolves.
Static track mute on that mixer is retained in the Resolve sequence
receipt as `muted_audio_tracks`, with a scoped portability loss. The
timeline flags and both mixer rows must agree. The track and its edits
remain present, with clip volume, track level and bus level independent.
Recognized but unqualified mixer settings or conflicting mute fields
receive scoped losses that block an internal Resolve write.

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

Use exact sequence IDs from the DRP reader or qualified DRT graph output.
The timeline report compares clocks, canvas, track layout, edit ranges,
saved media timing and locators, physical audio selectors, timeline
markers, picture controls, clip volume and the qualified Cross Dissolve.
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
