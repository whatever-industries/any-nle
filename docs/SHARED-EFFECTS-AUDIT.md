# Shared effect contracts for Premiere and Resolve

This audit compares Avid source interpretation and Premiere output with the
Resolve controls each mapping needs. Resolve's qualified static position,
scale, crop, isolated reflections and bounded Linear motion are described in the
[Resolve project core](RESOLVE-PROJECT-CORE.md#static-picture-controls).
Native identities, original parameter lists and unresolved witnesses stay
with the source adapter and format authorities.

| Effect and saved evidence | Graph/2 and current Premiere output | Resolve target and required comparison |
|---|---|---|
| Flip/Flop, including combined reflection (FXE-076/078, CONV-0041, CONV-0061) | The source adapter verifies the Avid identity and emits `avid.flip` with independent horizontal/vertical flags. Premiere prepares editable Horizontal/Vertical Flip. Mixed effect ordering is still refused or reported. | The linked writer maps both flags to editable Inspector controls. Neutral and all three reflections matched the native control after Save Project As and reopen. A later noncommuting crop or move must distinguish pre- and post-transform order. |
| Resize/3D Warp (FXE-081/001, AVB-0033–0037) | Static position, scale, rotation, anchor and crop already have a typed graph representation. Coordinates are frame fractions from center, X right, Y down, scale percent, rotation clockwise. The qualified Linear motion and Fixed/Elastic trim cases use `avid.resize_*` and `avid.warp_*` payloads with frame offsets. Premiere prepares editable Motion with destination ticks and the existing geometry admission. The normalized Warp Y conversion belongs at the source boundary and applies once. | The linked writer converts two- and three-key Linear controls to source-frame Inspector curves. Both trim rounds retain their saved Width clock and keys outside the edit within verified source media. The 14 trimmed and baseline timelines matched their pre-save renders after reopening. Rotation and noncentral anchor need their own controls. |
| Crop (FXE-081/001, AVB-0036/0037, CONV-0056, CONV-0064) | Static crop shares the graph Transform. Animated four-edge crop remains in the source-tagged Linear effect. Premiere writes bounded crop on qualified raster, clock, geometry, and effect span. Native background, unsupported key modes, order, and handle coverage are distinct. | The bounded three-key Elastic profile maps four editable source-space fractions to Inspector Cropping before Position and Zoom. After reopening, six retained Avid controls and trims matched within one pixel at the primary edge threshold. Independent crop clocks and stacks still need native controls. |
| Color Correction (FXE-073, CONV-0026) | `avid.color_correction` carries interpreted HSL and curves plus scoped losses. Premiere's supported path applies a generated LUT. Editable Lumetri remains held. Gain 150 off-channel residual remains open. | Study Color node ordering, channel/transfer/range semantics and LUT placement separately. Compare no-op and correction on legal-range 10-bit charts, including dark and near-clipping patches. A portable LUT operation needs pinned input/output color spaces and order. Editable node controls need separate render evidence. |
| AniMatte (FXE-086) | `avid.animatte` retains calibrated shape/matte payloads. Premiere writes the bounded opacity-mask path with polarity, opacity and supported shape data. Avid partial compositing is still open. | Use paired foreground/background controls. Test full and partial Key In/Out with one closed contour and separately compare feather. Establish coordinate, alpha, winding and operation semantics before treating the destination mask as equivalent. |
| Timewarp (FXE-149) | Graph speed and per-record-frame picture maps carry portable cadence in their qualified slices. Source receipts preserve distinctions between saved editable Speed/Position and generated Position. Premiere may synthesize a key per picture frame and report lost editability. Audio offsets, interpolation, and generated freeze assets have separate semantics. | Compare native time-map edits on a numbered counter at constant, Linear and Spline settings. Check reverse/freeze as separate cases. Require exact picture cadence and treat audio sample placement, pitch and source clocks independently. |
| SubCap and Titler+ (FXE-040, FX-021) | Graph text occurrences carry content, timing and lane. `avid.text` holds appearance witnesses including local FontSize, synchronization or boxed-title geometry. Premiere writes editable captions and bounded graphic text, with explicit appearance losses. | Test native Subtitle and Text/Text+ separately. First prove content and start/duration over a neighboring unchanged cue. Qualify font-size, wrapping, origin and style scope with a shared font and measured glyph bounds. A structurally present Fusion item needs a visible render. |
| Clip volume and AudioSuite Gain (AUDIO-002, FXE-198, AVB-0042) | Placement/item gain in dB and item-source-time volume keys are portable values. Track/clip pan, output mix, and Fairlight/Premiere bus policy are separate. The supported AudioSuite path links saved processed PCM; the plug-in settings are not inferred from labels. | Test neutral mono volume and a nonzero timed ramp with isolated Direct Out or controlled bus reference. Check sample alignment and level after reopen. Compare saved Gain renders as media playback, independently of PVOL automation or plug-in reconstruction. |

## Promotion order

1. **Isolated reflection.** Both preparation adapters now consume the same
   Avid axis flags. Qualify a noncommuting crop or move before admitting a
   reflection in a picture effect stack.
2. **Linear position and independent scale.** Both consumers use the
   same Avid coordinate values and edit-relative keys for the qualified
   two-key pair and three-key trims. Both consume the shared three-key
   Elastic crop grid, with crop applied before motion. Keep other crop
   clocks separate until qualified in both destinations.
3. **Text timing, clip gain, and picture cadence.** Use the existing portable
   values before introducing new types. Destination tests should catch a shifted
   cue or changed neighboring style, a second gain application or mixer policy
   leak, and a different sampled source frame, respectively.
4. **Color and matte.** Promote only the operations whose color/alpha and order
   survive the same controlled comparison in both destinations. Keep richer
   controls and unresolved animation tied to native evidence.

Proposed neutral values require two consuming adapters and a comparison
with actual native renders. Unsupported stacks retain scope and provenance.

Existing authorities and converter references: the workspace Avid effects codex (IDs above),
[source graph contract](SOURCE-GRAPH-MIGRATION.md),
[caption and graphics path](CAPTION-TEXT-TRANSLATION-2026-11-08.md),
[converter roadmap](ROADMAP.md).
