# Premiere 27 Color Mode

Premiere 27 places Adjust, Style, and LUT operations in Color Mode. The legacy
Lumetri Color effect remains editable. An operation can belong to a clip, a
group, or the sequence. [Adobe Color Mode guide](https://helpx.adobe.com/premiere/desktop/correct-color/color-mode-fundamentals/color-mode-basics.html) · [Adobe Lumetri changes](https://helpx.adobe.com/premiere/desktop/correct-color/color-mode-fundamentals/lumetri-color.html)

## Translation target

New translations to Premiere 27 write native Color Mode operations.
For translations from Premiere 27, capture the saved Color Mode operation
on its original owner and retain the color pipeline through graph/2.
Qualify the native destination mapping with reopened chart renders and
report unmapped source operations in Review.

The affine clip LUT and isolated extended Rec.709 SDR controls below
already use Color Mode. The modeled Avid correction and retimed source
LUT legacy writer profiles remain migration work. Compare their native
Color Mode candidates against the source render before switching each
profile.

## Clip Adjust

Premiere 27.0.0 build 88 saved a clip-owned Adjust 1 on a 1080p24
Rec.709 synthetic grayscale chart. The saved video clip component chain
references a compound component with type `65537`. Its child chain
contains type `65541`, which contains Adjust 1 at type `65538`. The
Adjust chain contains Color And Contrast, Color Shift, and a type `65539`
compound with Color Texture and Unsharp Mask. Its order and default objects
survived Save As, close and reopen.

The Color And Contrast filter's match name is `AE.ADBE Color And Contrast`.
The static global Exposure field is parameter ID `2`, at index `1` in
this filter's parameter list. Its scalar occupies the second value in
`StartKeyframe`, with a default of zero and saved bounds of −5 and +5.
Other groups have fields named Exposure, so the filter, owner, and
parameter ID all matter.

Two private one-field variants of the native Save As project changed
global Exposure from zero to +1 or −1. Premiere opened both and exported
the same chart frame. Premiere saved the −1 variant, and the decoded frame
hash matched on a second open. The neutral operation matched the eight
patches of the project without Adjust. Exported 8-bit RGB patch values
were achromatic throughout.

| Exposure | Patch values from left to right |
| --- | --- |
| 0 | 17, 46, 75, 105, 134, 163, 193, 222 |
| +1 | 24, 65, 107, 148, 189, 230, 255, 255 |
| −1 | 12, 33, 53, 74, 94, 115, 136, 157 |

This establishes response for the tested Rec.709 pipeline. Before mapping
Avid grades to the Adjust controls, compare the source and destination
response on a chart with neutral, saturated, and near-clipping patches.

### Clip Adjust Exposure to Resolve

The internal Premiere → Resolve route captures the clip-owned global
Exposure value and its saved color context on the edit in graph/2. With
the other Adjust controls at their observed defaults, it supports static
values across the saved −5 through +5 range on plain 1080p24 ProRes HQ
picture in the measured Rec.709 pipeline. Exposure stays visible in
graph/2 when changed controls or color context require a different mapping.

Resolve receives one editable CDL node with equal RGB slopes of
`2^(Exposure/2)`, zero offset, and unit power and saturation. Zero leaves
the edit neutral. Master Gain matched the gray ramp at +1 but missed
saturated patches by up to 75.31 RGB codes on an 8-bit scale.

Renders from Premiere 27.0.0 build 88 and Resolve Studio 21.2.0 build
00006 covered −5, −2, −1, +0.5, +1, +2 and +5 after Save As and reopen.
Serialized graph/2 reproduced the seven independent slopes on one V1
timeline linked to the original chart. All 336 reopened frames matched
independently authored Resolve controls exactly. Across the 24 neutral,
saturated and clipping patches, the largest difference from Premiere was
1.29 RGB codes on an 8-bit scale. The reader verifies the complete
Save As node body apart from the three equal slope values. It also
recognizes equal-slope native Resolve saves with the same neutral
offset, power and saturation. Only the slope changes across the
qualified range. See the [native Resolve round trip](#native-resolve-cdl-round-trip).

The trial of a clip LUT saved a Resolve library reference. An export
requested with LUT assets did not include the table in that control.
An install-independent LUT transfer remains separate work.

### Global Contrast and Pivot

The same clip-owned Color And Contrast filter saves global Contrast as
parameter ID `4` and Pivot as ID `5`. Their neutral values are 1 and 0.
The reader retains non-neutral static values as `contrast` and `pivot`
on the existing `premiere.color_mode_exposure` graph/2 receipt. Each
known scalar survives independently when another control is unresolved.
The isolated Exposure mapping still requires the complete neutral
witness for the other Adjust controls. For Contrast and Pivot, the
reader resolves Premiere's shared binary payloads before comparing
the full control tree with the static Adjust witness.

Premiere 27 Save As and reopen covered Contrast values 0.5, 0.75,
1.1, 1.25, 1.5 and 2, Pivot at −1, 0, +0.5 and +1, and combined
Exposure at −1.5 and +1. On the measured Rec.709 chart, the response
fits a shared multiplier for the three RGB channels. Apply Exposure
first, square the resulting RGB values before computing Rec.709
luminance, and multiply the exposed RGB by
`(luminance/pivot) ^ ((power - 1)/2)`. Power equals the Contrast
value below unity and rises three times as fast above unity. Each
Pivot stop doubles the luminance reference. With the effective
calibration value 0.1786 for the decoded chart at Pivot 0, the
formula predicts the measured patches within 0.42 RGB codes on an
8-bit scale, including the clipped patches. The calibration is
scoped to this chart and pipeline.

Two private archive prototypes embedded a Fusion Custom Tool with
editable Exposure, Contrast and Pivot inputs. Each kept its
values and original media link after import, save and reopen in
Resolve Studio 21.2. Across 96 frames, the largest measured
chart difference from Premiere was 1.20 codes.

The internal writer now maps this static Adjust to one editable Fusion
Custom Tool per qualified V1 edit. It uses the edit's Exposure, Contrast
and Pivot from graph/2 and binds MediaIn to the linked Media Pool item.
The profile is picture-only 1080p24 with explicitly tagged Rec.709
ProRes HQ MOV and the verified Premiere Rec.709 pipeline. It admits
Exposure from −1.5 to +1, Contrast from 0.5 to 2, and Pivot from −1 to
+1. Review reports an appearance approximation and requires approval
for the run. Other controls retain the edit's existing omission notice.

The numbered chart repeats the same original with trims at source
frames [5,25) and [14,42), then checks the file boundaries in a second
project. After save, close and reopen in Resolve, the generated projects
preserved all 96 source-frame selections, with distinct editable
settings on each occurrence. The maximum patch difference from
Premiere was 1.06 codes on an 8-bit RGB scale. With the native author's
saved Fusion start offset, a fresh DRP import selected both trims one
frame early despite matching edit In points. The writer adjusts that
field for the archive import. Prepublication read-back checks the
composition and offset fields against the prepared edit. The reader
retains other clip Fusion compositions as bounded observations.

### Native Resolve CDL round trip

Resolve Studio 21.2.0 build 00006 saves a one-node CDL differently
after editing the grade through its native API. Graph/2 now gives
both saved layouts the same edit-owned range or slope receipt. The
reader validates the complete witnessed control payload, including
neutral controls and the relationships among the changing integer
fields. Changing one channel or breaking the integer relationship
leaves a grade omission on that edit.

Reopened linked DRPs preserved both range shifts at 24, 23.976, 25
and 29.97 DF. The picture-only controls use a ProRes HQ MOV at 24
fps and H.264 MOV at the other rates. Each keeps the nonzero picture
trim, linked original, and native empty mono track declarations.
All 288 decoded frames matched the respective native Resolve
controls under the same Deliver settings. Isolated equal-RGB slope
controls at −1.25 and +1.75 exposure stops also passed the 24 fps
native reopen and frame comparison. These checks cover Resolve
reconstruction in the qualified picture profile.

## Clip LUT

The qualified Avid affine LUT route now writes a clip-owned Color Mode LUT
in Premiere 27. Graph/2 and project-local cube qualification use the
existing source facts. The writer uses the eight red-fastest corners of
the verified unit RGB grid. It supports the existing plain V1, Rec.709
1080p24 Separate files profile. Lumetri remains the general fallback for
retimed source LUTs and modeled Avid Color Correction.

In a native save, the clip's video chain owns a compound of type `65537`.
Its child chain contains the clip operations compound of type `65541`.
The saved `AE.ADBE LUT Transform` filter appears in that compound.
Parameter ID 3 stores the raw `ITX3` table with its dimensions, RGB
domain and float samples. Parameters 2 and 9 carry the LUT ID and
display name, respectively. The tested Rec.709 (Scene), Full Range
profile uses menu values 28 and 1, with Intensity at 100. A LUT-only
chain renders without the native UI's initial neutral Adjust operation.
Premiere renders the embedded table independently of the browsed cube.

A comparison in Premiere 27.0.0 build 88 used the same retained
red/blue swap cube and the same two 87-frame edits sharing one source
and trim. The generated Color Mode output matched the native Color
Mode ProRes export on all 174 decoded frames after Save As and reopen.
All 87 neutral frames also matched Lumetri. The sampled Premiere PNG
matched Lumetri exactly. In uncompressed full-range 10-bit DPX, the
graded-frame difference was at most five codes per RGB channel. The
Color Mode ProRes render measured 48.85 dB RGB PSNR against the Avid
graded reference. The earlier Lumetri render measured 48.34 dB.
These measurements support the bounded affine mapping. The nonlinear
profile below covers one ordered Avid Color Adapter.

### Ordered Avid Color Adapters

For a plain V1 edit using a verified 1080p24 Rec.709 picture-only Avid
ProRes LT rewrap, the converter can compose the saved 10-bit
legal-to-full step and a project-local 33³ cube in either order. Graph/2
retains both stages, the occurrence, and its media identity. Where the
master saved the opposite order, the converter requires the saved
reordered-source receipt and an exact match to the same cube and range.
Missing assets and rejected bindings retain the edit's omission report.

The writer embeds one 65³ Color Mode LUT per qualified stack. It
evaluates the cube with tetrahedral interpolation over unit RGB and
clamps the cube input. Expansion after the cube can retain excursions.
Both orders use editable LUT controls in Premiere. Their individual
stages remain available in graph/2. The rewrap copies the coded picture
without applying the color transform to media.

Premiere 27.0.0 build 88 saved and reopened both orders. All 72 decoded
frames matched the retained Premiere prototype exactly. Against Avid,
the 36 frames of range then cube measure 59.30 dB YUV PSNR. The 36
frames of cube then range measure 58.29 dB. Swapping their Avid
references lowers both comparisons to about 30.2 dB. This qualifies the
observed chart and working space. Wider source profiles, other
transform stacks, and writing the stack to Resolve remain open.

### Modeled Avid HSL and curves

Premiere 27.0.0 build 88 saved and reopened paired 1080p24 Rec.709
timelines with identical edits and the same seven production 33³ LUT
tables. The timelines used clip-owned Color Mode LUT operations and
Lumetri Creative Looks, respectively. Read-back confirmed byte-identical
ITX3 tables on corresponding edits and a shared trim, path, and
01:00:00:00 start. Both timelines rendered 192 frames. Each carrier
reproduced all 192 decoded frames after a second close and reopen.

The retained Avid chart's DNxUncompressed still does not link through
the normal converter. The test used two private Rec.709 ProRes HQ
sources, one made from the Avid no-op RGB TIFF and one from the decoded
10-bit 4:2:2 picture in the original MXF. The latter checked whether
the misses arose from using a source already clipped by the TIFF export.
Its Color Mode LUT used explicit Rec.709 (Scene), Full Range, and 100%
intensity. The comparison used the original Avid 16-bit RGB TIFFs.

Values below are the largest absolute differences across 12 flat-patch
interiors in 8-bit-equivalent RGB codes, using the decoded MXF source.

| Control | Color Mode vs Lumetri | Color Mode vs Avid | Lumetri vs Avid |
| --- | ---: | ---: | ---: |
| Neutral | 0.00 | 1.60 | 1.60 |
| Hue +30 | 0.53 | 2.37 | 2.37 |
| Hue −60 | 0.45 | 2.97 | 2.97 |
| Saturation 50 | 0.53 | 1.72 | 1.72 |
| Contrast +30 | 0.00 | 18.11 | 18.11 |
| Gain 150 | 0.33 | 48.83 | 48.75 |
| Gain 50 | 0.45 | 1.65 | 1.70 |
| Five-point Master curve | 1.63 | 2.63 | 3.97 |

All 24 neutral frames matched between the carriers. Across the graded
frames the carriers differed by up to 9.32 RGB codes at individual
pixels, with a maximum whole-frame mean absolute difference of 0.133
codes. The no-op TIFF source reproduced the large contrast and gain
misses against Avid. The modeled LUT continues to use Lumetri. The
shared residual on saturated patches is consistent with the earlier
Avid component-limiting evidence. The rail-crossing comparison below
tests that explanation with a second source.

### Gain and Contrast on a rail-crossing chart

Media Composer 25.12 exported five 72-frame edits at 1080p23.976
Rec.709 as legal-range, 10-bit ProRes 422 HQ. The chart varied luma and
each chroma component independently through their nominal and physical
limits, and included saturated and moderate colors inside Rec.709 RGB.
The dry and neutral HSL controls reproduced the decoded source at all
432 patch centers. Gain 150 scaled Y′ about code 64 and Cb and Cr about
code 512 by 1.5. Contrast +30 scaled Y′ about code 508.132 and both
chroma components about 512 by tan(58.5°). A clamp of 4–1019 on each
component after those operations predicts the Avid Y′CbCr render within
0.50 code for Gain 150 and 0.71 for Contrast +30 before rounding.
This establishes the limits of the measured Avid render path. The codec
also reserves endpoint codes, so the result does not isolate the stage
that applies the limits.

The candidate LUT converts from Rec.709 RGB to Y′CbCr, applies the
measured operation and component limits, then converts back to RGB.
Premiere 27.0.0 build 88 preserved a 65³ table with input domain
−1.125 to 2.125 and extended output values after Save As and reopen.
The ordinary 0–1 LUT improved colors inside Rec.709 but still missed
colors outside the unit RGB cube. With the extended LUT, the worst
flat-patch errors against the Avid render were:

| Source patch | Control | Existing Color Mode LUT | Extended LUT |
| --- | --- | ---: | ---: |
| Inside the unit RGB cube | Gain 150 | 39.32 | 4.41 |
| Outside the unit RGB cube | Gain 150 | 140.91 | 4.03 |
| Inside the unit RGB cube | Contrast +30 | 23.18 | 2.55 |
| Outside the unit RGB cube | Contrast +30 | 96.09 | 1.21 |

These values are 8-bit-equivalent RGB codes after both renders are
decoded with the same matrix and clipped to the SDR display range.
The extended LUT also reduced the original 24 fps chart's errors from
48.83 to 1.91 for Gain 150 and 18.11 to 2.88 for Contrast +30. All 360
decoded frames of the 23.976 extended-LUT render reproduced after a
second reopen. The Premiere ProRes output still changed extreme Y′CbCr
triples that produce similar clipped RGB, so the candidate qualifies
as a visible SDR approximation on these charts.

Premiere’s Wide Gamut (No Tone Mapping) preset saved an ACEScct working
space and a 100-nit Rec.709 output. With the same extended LUT it changed
the ungraded out-of-gamut patches by up to 1.48 visible RGB codes and
did not recover the Avid Y′CbCr values. The extended cube improved the
original Rec.709 comparison without that preset change. Adobe describes
wide gamut as a sequence color-management choice and notes that changing
the sequence setup after grading can change appearance.
[Adobe Color Mode guide](https://helpx.adobe.com/premiere/desktop/correct-color/color-mode-fundamentals/color-mode-basics.html) · [Adobe color-management guide](https://helpx.adobe.com/premiere/desktop/correct-color/set-up-color-management/how-color-management-works.html)

#### Converter mapping for isolated Gain 150 and Contrast +30

Separate files uses the extended Color Mode LUT on a plain same-rate
V1 cut from linked, picture-only 1080p24 ProRes 422 HQ MOV. The source
probe must confirm Rec.709 primaries, transfer and matrix, plus video
range. The writer admits exactly one of the two measured controls with
neutral saturation and no other tone controls, curves, unresolved color
controls, source color override or effect stack on the cut. The Avid
reader verifies the active native parameter and rejects animation from
this mapping. The
converter reports the approximation by occurrence and writes one
editable embedded table and one extended `.cube` per distinct grade.
The defaulted color profile on an untagged original uses the existing
Lumetri model.

The CLI and saved job produced the same five edits, source and trims.
Their tables matched the native trial's bytes and survived Premiere
27.0.0 build 88 Save As, close and reopen. The qualified 24 fps linked
fixture reuses the 23.976 native Avid chart's source pixels. All 72
decoded source pictures match across those rates. The generated project's
360-frame render reproduced exactly on a second reopen with the
`.cube` files hidden. Across the 432 patches, the visible RGB mean and
maximum errors were 0.177 and 4.407 codes for Gain 150 and 0.066 and
2.550 for Contrast +30 on an 8-bit scale. The dry control matched Avid
at all patch centers. The raw extreme Y′CbCr difference still applies.
