# After Effects format reference

The EffectCraft AEP work was proposed in PR #1869, which closed without a
merge. The intake uses the notes and synthetic project fixtures from
`b3900c70cce9b40a3e10f0e94c773427dddd1f5f`. The reference code and fixtures
stay outside this repository. [Pull request](https://github.com/storytold/effectcraft/pull/1869) · [Format notes](https://github.com/storytold/effectcraft/blob/b3900c70cce9b40a3e10f0e94c773427dddd1f5f/docs/aep-format.md) · [Import notes](https://github.com/storytold/effectcraft/blob/b3900c70cce9b40a3e10f0e94c773427dddd1f5f/docs/aep-import.md) · [Export notes](https://github.com/storytold/effectcraft/blob/b3900c70cce9b40a3e10f0e94c773427dddd1f5f/docs/aep-export.md)

Run the internal, read-only probe with `python3 scripts/aep_inspect.py INPUT`.
It accepts binary AEP and AEPX 1.0 and prints JSON to stdout. The report includes saved names and locators, so keep
reports from real projects private. It bounds input and traversed records,
checks parent chunk boundaries, and separates the trailer from the declared
container. The XML reader limits element count and nesting, rejects document
type declarations and decodes the same owner records as the binary reader.
The probe reads saved static transforms and keyed Position and Opacity on
ordinary 2D AV layers. It also reads layer visibility, blend modes,
saved track mattes, layer masks, Time Remap keys, composition and layer markers,
the saved 3D switch and static rotations on ordinary AV layers, and the ordered
effect instances with a bounded set of scalar, point and layer input controls.
Other property payloads and XMP contents remain opaque to this inspection.

| Area | Checked offline | Next qualification |
|---|---|---|
| Project and composition | Native item IDs, folder membership, layer order, canvas, pixel aspect, exact frame rate, duration and saved display start. The binary base and its XML sibling agree on IDs, canvas, clock and layer records. | Native linked-footage project with two named folders, a nonzero display start and one straight cut. |
| Layer timing | Preserves the saved start, relative in/out and signed stretch separately. One trim is one second, reverse stores −1/1, and the saved 23.976 clock is 23976/1000. A WAV out-point uses 48000/48000 while the composition uses 25600 ticks per second. | Confirm the record and source ranges with nonzero start and trim. Qualify reverse and mixed rates separately. |
| Time Remap | The enabled precomp and image-sequence controls retain layer ticks, source seconds, both interpolation codes and saved ease. Inert controls are omitted and expressions have their own state. | Compare a native numbered-source render with a nonzero layer start and trim. Verify a remapped layer with stretch separately before composing either control into graph/2. |
| Source identity | A precomp resolves by saved item ID and the WAV item carries a saved alias path. A missing source remains explicit in the report. | Test two same-named originals. Verify the source path and physical media before binding it through graph/2. |
| Movie interpretation | A pinned movie save keeps 25 source frames at a cached 25 fps and stores a separate 30 fps Interpret Footage override. Its layer still stores an out point of 25/25 seconds. | Check frame selection across that out point with the original media online before mapping interpreted movie timing. |
| Properties | One-change saves verified position, anchor, scale, rotation, and opacity. Position, Anchor Point, Scale, and Opacity keys retain signed layer ticks, their property clock, values, and both interpolation sides. Position and Anchor Point retain spatial tangents and saved ease. Scale retains ease for each component. Expression records, including disabled expressions, carry a separate state. | Validate the source-dimension anchor and composition position together against a native linked-footage render. Qualify rendered interpolation and the relationship between layer and composition time before mapping keys to a destination. |
| 3D layer transforms | A saved switch identifies the 3D AV layer. Four pinned saves distinguish the switch alone, 30° X Rotation, 45° Y Rotation, and Orientation of (10°, 20°, 30°). Z Rotation stays in the existing rotation field. | Compare a combined Orientation and axis rotation on an asymmetric chart before mapping the rotation order or projection. |
| Layer parenting | A saved child links to its null parent by composition-local layer ID and stores Position in parent-layer pixels. Cycles, self-links and missing parents appear as unresolved references. | Compare a moved and rotated parent with an independently offset child. Verify their composed transforms before converting them. |
| Track mattes | Six saved projects cover alpha, inverted alpha, luma, inverted luma, a matte below its target, and a matte with video still enabled. The target layer owns the mode and the composition-local matte ID. | Compare visible alpha and luma output in the source and destination editors before adding a conversion. |
| Layer blend modes | One-change saves and a multi-layer comparison project identify 13 saved modes, including Normal, Add, Multiply, Screen, Overlay and Difference. Unmapped codes remain visible with their original numeric value. | Compare a pair of overlapping clips with matching color and alpha settings before adding a destination mapping. |
| Markers | Eight saved projects cover composition and layer ownership, fractional times and durations, comments, labels and cue data. The probe pairs each saved timestamp with its own marker record and preserves both clocks. | Check the timing of a layer marker through a nonzero start, trim or stretch before expressing it as a destination record-time marker. |
| Layer masks | Fifteen pinned AEP saves cover ordered mask atoms, all seven mode codes, inversion, closed and open paths, saved static feather, opacity and expansion, an absent path, and Linear and Bézier path keys. Saved vertices and handles are in source-raster fractions. | Compare composited pixels on a source smaller than its composition, then qualify animated shape interpolation and destination mask order. |
| Effect instances | The layer owns the saved stack order, stable match names, effect names and any user rename. The selected scalar profiles cover Slider, Checkbox, Angle, and Gaussian Blur's Blurriness and Repeat Edge Pixels. | Check the enabled switch, shared definitions, and additional parameter types before mapping the effect stack into graph/2. |
| Effect points | Point Control and Gradient Ramp endpoints use fractions of the owning layer's raster. Static values can lie outside 0–1. The Ramp control retains its signed key ticks, interpolation and saved tangents. | Compare rendered placement and animation on a layer smaller than its composition before mapping to a destination coordinate system. |
| Effect layer inputs | Layer Control, Set Matte, Displacement Map and Compound Blur retain their saved composition-local target IDs. The report distinguishes None, self, another layer and an unresolved ID. It also retains the saved selector code. | Verify which version of the target layer each selector mode samples and compare a native render before mapping the effect. |

The offline comparison used nine binary fixtures and five matched AEP/AEPX
pairs. Within each pair the inspected item, layer and unresolved reference
records matched. The pairs include a precomp, text, keyed opacity and an
expression, with the latter three cases providing layer ownership controls.
The no-edit save retained the same inspected items even though the file
hash changed. Tests also reject truncated owner chunks and duplicate item IDs.
The WAV fixture contains a sanitized locator, so it
establishes saved reference layout without establishing relink behavior.

In the effect controls, a missing value with a qualified local definition is
reported as `local_default`. Gaussian Blur saves Blurriness 25 and Repeat
Edge Pixels on in its definition, an explicit 40 in a changed Blurriness
record, and an explicit zero when Repeat Edge Pixels is turned off. Values
never come from the value records in the project-wide effect definition
copy, which can hold another instance's edits. For the two qualified point
profiles, a unique shared point definition can provide a fixed-point
default when the instance lacks definitions. The two-layer Gradient Ramp
control keeps the edited Start of Ramp at (0.1, 0.2) on its 100×100 layer;
the other instance gets the shared default (0.5, 0). Missing and duplicate
shared definitions leave the default unresolved. The retained Slider Control stores Linear keys
10 and 90 at 0 and 51,200 ticks on a 25,600 Hz clock; an expression control
is reported separately and is not evaluated. Other effects still appear by
occurrence and in order. The probe does not establish render equivalence or
offer an After Effects conversion route.

For the observed integer-rate movie profile, the report keeps the cached
native rate and frame count alongside the saved override and effective
rate. Layer in and out points stay on their own saved clocks. Movie
interpretation still needs physical media verification.
[Adobe's Interpret Footage guide](https://helpx.adobe.com/after-effects/desktop/work-with-footage-items/import-and-interpret-footage-items/importing-interpreting-footage-items.html)

Static position uses composition pixels on an unparented layer and
parent-layer pixels on a parented layer. The probe reports saved local
values without composing the parent transform. [Adobe's parenting guide](https://helpx.adobe.com/after-effects/desktop/work-with-layers/layer-properties/layer-properties.html)

The two anchor components use
fractions of the source width and height. Scale and opacity retain their
stored fractions, and rotation retains degrees. Absent properties are
omitted because their defaults depend on layer and composition context.
The report includes the saved footage raster and resolves an explicit anchor
to pixels for solids and precompositions by source item ID. A 100×100 solid
resolved to (10, 20) and a 400×300 solid to (100, 50), both in a 1920×1080
composition. Linked footage retains its cached raster and saved anchor fraction
pending native checks for interpreted media.

The saved Position control has three keys at 0, 25,600 and 64,000 ticks,
with values (0, 0), (100, 0) and (200, 100) in composition pixels. The
matched binary and XML Opacity controls both retain 0 and 1 at 0 and
51,200 ticks. Both use a property clock of 25,600 ticks per second. Ease
and tangents remain saved values, without evaluation by the probe.

In the retained Linear Scale control, the value changes from (1, 1, 1)
to (2, 1.5, 1) over 25,600 ticks. A separate Bézier save retains its
speed and influence for each of the three scale components. Anchor Point
keeps the two saved source fractions and the Z component at each key.
The 400×800 solid moves from (0.5, 0.5) to (0.75, 0.625) in those
fractions, equivalent to (200, 400) and (300, 500) source pixels.

The 3D layer report keeps static Orientation and the separate axis
rotations on the saved layer. An absent control is omitted. Rendering
still needs the camera, coordinate system and composition order.
[Adobe's 3D layers guide](https://helpx.adobe.com/after-effects/desktop/work-with-layers/3d-layers/3d-layers.html)

The precomp remap holds keys at 0 and 128,000 layer ticks with source
values of 0 and 5 seconds. The image-sequence remap holds keys at 0
and 2,560 ticks with source values of 0 and 0.1 seconds. Both use a
25,600 Hz property clock and save Linear interpolation on both sides.
The report keeps the signed layer stretch as a separate fact.
[Adobe's time-remapping guide](https://helpx.adobe.com/after-effects/desktop/animate-in-after-effects/time-stretching-and-time-remapping/time-stretching-time-remapping.html)

The matte report retains the saved mode code and layer ID. It flags missing
IDs, self references, IDs outside the composition, and inactive modes
that carry an ID. Unknown modes retain their numeric code. The matte layer's
video switch is reported independently of its use by another layer.
[Adobe's track matte guide](https://helpx.adobe.com/after-effects/using/track-mattes-and-traveling-mattes.html)

The layer blend mode report covers a saved mode and its numeric code.
Ambiguous codes retain their number with an unresolved mode name, including
the shared Dissolve and Dancing Dissolve code. Rendered blend behavior also
depends on the composition's color settings and the participating layers.
[Adobe's blend mode guide](https://helpx.adobe.com/after-effects/desktop/work-with-layers/work-with-layer-blending-modes/blending-modes-layer-styles.html)

The marker at 0.52 seconds is stored at 13,312 ticks on a 25,600 Hz
property clock and lasts 180 ticks on a separate 600 Hz clock. A
composition control has distinct markers at 1 and 2.5 seconds. Marker
records remain on their saved owner. Cue parameters retain their order,
and marker times with a mismatched record count are rejected.
[Adobe's marker guide](https://helpx.adobe.com/ph_fil/after-effects/desktop/work-with-layers/work-with-layer-and-composition-markers/layer-markers-composition-markers.html)

The mask report pairs each layer's header with its own mask atom in saved
order. It retains absolute in- and out-handle positions in the same source
coordinates as the vertices. On the 100×100 solid, the saved bounds give
a 60×40 rectangle inside a 1920×1080 composition. The probe reports a
missing path as `not_saved`. For an animated path, it pairs each saved
geometry with its layer tick, incoming and outgoing interpolation, and
saved ease. The Bézier controls distinguish outgoing/incoming influence
0.75/0.75 and 0.20/0.50.
Feather and static expansion use source pixels. Linear and Bézier
Feather keys retain both axis values and both sides of each component's
ease. The Bézier save changes from (0, 0) to (50, 30) at 51,200 ticks.
Opacity uses a fraction. An open path retains its saved mode code even though Adobe
documents that open paths do not mask layer opacity.
[Adobe's mask guide](https://helpx.adobe.com/after-effects/desktop/work-with-transparency-and-compositing/work-with-alpha-channels-and-masks/alpha-channels-masks-mattes.html)

The first conversion profile should start with a same-rate composition and
an online, square-pixel footage layer with a nonzero trim. A graph adapter
needs verified media identity, the exact display clock, and an explicit
relationship between the composition and the source. Native Save As, reopen
and frame comparison remain required for the writer. Expression evaluation,
text appearance, track mattes, masks, effects, and wider nested compositions
each need their own source and destination evidence.
