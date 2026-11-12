# Resolve timeline archive inspection

The internal `resolve-source-probe` supports a native Resolve Studio 21.2.0
build 00006, project version 17, single-timeline `.drt` export:

```sh
resolve-source-probe --inspect-drt INPUT.drt OUTPUT.json
resolve-source-probe --graph-drt INPUT.drt OUTPUT.graph.json
```

Inspection reports the saved timeline clock, edit occurrences, transition,
timeline markers, locators, and qualified source ranges and channel selectors.
It distinguishes saved timeline paths from Media Pool cache locators.
For the qualified standalone WAV profile, the resolved audio stream numbers
are one-based physical channels, independent of the Resolve virtual track
index. The probe never opens, searches for, or relinks media. Online status
is unknown. Inspecting an incomplete archive keeps the raw occurrence and
reports missing witnesses; malformed or contradictory references still fail.
Reports use the shared bounded ZIP/XML reader and a 64 MiB output ceiling.
Both modes publish exclusively. The existing DRP invocation is unchanged.

Graph/2 export is admitted only for the native-tested non-drop 1080p
`24000/1001` slice: two same-rate plain ProRes HQ picture edits on V1, an
eight-frame left Cross Dissolve, and two independent mono edits selecting
distinct channels from the same PCM16/48 kHz WAV. The reader checks exact
sequence timing, adjacency, the incoming handle, saved media references,
source start and extent, and channel routing from the DRT itself. A zero
file-relative picture start is witnessed by `MediaStartTime` and the saved
cache timing; it does not assert an embedded timecode label. The transition
stays in its native sidecar, with a loss for destinations that cannot map it.
Graph/2 and public CLI/job schemas are unchanged. DRT authoring, synthetic
archive layouts, additional builds, mixed rates, compound timelines, and
product exposure are separate work.

## Pinned source review

- [CiprianSpiridon DRT reader, `9f2537e5` (MIT)](https://github.com/CiprianSpiridon/davinci-resolve-mcp/blob/9f2537e5b8734c87ef05d484b34733982b94087b/src/davinci_resolve_mcp/formats/drt.py)
  identifies the UUID-named sequence container and clip fields. Its validator
  forbids `project.xml` and expects timeline clock fields in that container.
  Both native exports in this trial instead contain `project.xml`,
  `MediaPool/Master/MpFolder.xml`, and one UUID-named container. The timeline
  handle and exact clock live in the pool's `Sm2Sequence`; marker state
  lives in the project lockable map. The validator assumptions therefore do
  not fit these exports. The tool-authored numbered layout is unqualified.
- [drp-format, `1e2bf034` (MIT)](https://raw.githubusercontent.com/samuelgursky/davinci-resolve-mcp/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format/README.md)
  points to the Media Pool cache for saved binding and to visible render
  checks for qualification. We used the native cache and existing WAV
  route decoder. No upstream code is included or needed at runtime.

## Native comparison

The reference timeline starts at `01:00:00:00` (86400) and has four
placements over 48 frames. V1 cuts at relative frame 24 with source trims
of 5 and 8 frames. The blend covers frames 16 through 23. A1 and A2
start at source frames 3 and 12 and use physical channels 1 and 2.
The blue timeline marker is at relative frame 12 with duration 5.

Resolve rewrote object IDs when exporting DRT; they are archive-local.
The dissolve's effect payload, alignment, position and interval matched
the paired DRP. The DRT transition envelope added an outer field and
cleared four bytes of the 44-byte inner state that vary in DRP saves.
The reader matches this observed DRT envelope separately; it does not
relax the DRP profile or assign meaning to the added field.

Imported the DRT into a clean same-rate project, used Save Project As,
closed and reopened, and exported DRT and DRP again. Media stayed online
and the reopened start, trims, marker ownership, dissolve and selectors
matched. All 48 decoded video frames matched in each comparison.
The two isolated stereo bus renders matched exactly at 96,096 PCM24
sample frames per lane. The reference also had a separate mono output;
comparison used the stereo bus on both sides by stream properties.
This qualifies native DRT reading and native import/reopen/render for
this profile. Any NLE did not write a DRT.

| Evidence | SHA-256 |
|---|---|
| Same-timeline native DRT | `2dcc149943c1abce57a87e82d374838dbbcffd53865167d3a1da7f7fb10f9576` |
| Same-timeline native DRP | `46358e5c08eb9ed5d2d29e1705e82161954ba215987d416a5088c4858fd1f1a6` |
| Reopened import, DRT | `24980120dcb8d29e6806d291aebe77da11bea897dd50f7f72517308e306b26e8` |
| Reopened import, DRP | `f2947d90517b75fc427f096a2baeffe36132ebf1302dd83a077362b26b625de2` |

Private evidence includes the saved archives, native read-back, per-frame
and PCM comparisons, and the replay script. Negative controls cover
missing media records, selectors, timeline clock and project metadata,
malformed XML, duplicate ZIP members, and existing output. No missing
fact was recovered from the companion DRP.
