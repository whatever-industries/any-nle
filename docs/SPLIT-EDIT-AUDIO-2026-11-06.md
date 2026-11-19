# Split-edit native audio qualification and MXF route review

2026-11-06. The retained Avid-authored split-edit fixture now has an automated
Premiere audio-render comparison, supplementing the
structural and picture trial.
Converter runtime is unchanged from `6d441a22cf02589fdf221ad1f855c72f44d94008`;
Premiere is 26.5.2 build 5. This is agent-operated native evidence with a separate
same-author measurement check, not User listening or independent review.

## Audio result

Six native exports cover control/fade with both lanes audible and each mono lane
isolated. All are two-channel 48 kHz PCM24 with 144144 samples per channel, exactly
72 picture units at 24000/1001. An independent reference assembles the original
OP-Atom PCM payloads at the saved Avid edit ranges. Separate FFmpeg decoding agrees
with every original source sample used by the direct KLV reference reader.

With the existing centered-pan policy, each isolated mono lane contributes at
sqrt(1/2) to both master channels. Every sample of each control export matches its
intended source lane and placements within one PCM24 integer step. The same bound
holds outside the fade window in the fade exports. Native left/right output equality
and the complete mixed-output sum are checked separately. This is measured timing
and routing after the existing pan policy, not bit-identical original PCM through
an arithmetic mixer or Avid odd/even pan parity.

Seven negative controls reject wrong source channels, one-sample shifts, missing
lanes and moving the audio cut to the picture cut. The intentionally unequal source
levels are retained; the quiet second lane is still independently distinguishable.
No global comparison tolerance or parity rule was relaxed.

Significant fade-versus-control differences occur only inside the requested
asymmetric window, record samples [36036,48048), corresponding to sequence-relative
00:00:00:18–00:00:01:00. The audio cut is at 00:00:00:20. Outside that window the
source ranges remain intact. Simple cosine/sine, linear and square-root envelope
hypotheses are retained as comparisons; none establishes the exact native curve.
Avid/Premiere envelope equivalence and subjective playback acceptance remain open.
The existing Constant Power approximation is not promoted to Avid envelope fidelity.

The current project stayed open. Temporary mutes on the two test sequences were
restored and checked. All eleven pre-existing sequences were preserved. A later
native-import probe added one research sequence; none was deleted. Native returns
and new research saves are separate files, and original media/bin hashes match.
No mouse/keyboard control, editor restart, cache deletion or installation was used.

## What the earlier MXF problems establish

Keep these evidence classes distinct:

| Case | Established result | Current implication |
|---|---|---|
| Audio Media duration shorter than its MXF file package (PRPROJ-0018) | The corrected full file-package duration played in the retained User trial. | Existing construction correction; not evidence every OP-Atom file is unusable. |
| Audio-only zero-video multicam (PRPROJ-0022) | It poisoned one media pair's playback for the session; the replacement nested stereo sequence played. | Existing construction correction. Do not reintroduce that known trigger for a new baseline. |
| Selected ProRes LT OP-Atom picture | Premiere's own project also reported frame-substitution recursion; an essence-preserving MOV rewrap played/exported in the retained trial. | Still unresolved for direct OP-Atom on that case. Grouped/Merged/Separate item structure does not establish a container-decoder remedy. |
| Selected DNxHR OP-Atom terminal zero span | The strict OP1a route excludes only the qualified non-picture terminal span and preserves all coded pictures/PCM. | Current guarded rewrap result remains bounded to its admission profile. |

PRPROJ-0018/0022 are read-only Premiere knowledgebase sources; this task renews no
finding confidence or decision binding. The prior ProRes result is recorded in the
converter roadmap. OP1a is also MXF, so one successful rewrap profile cannot establish
that every Premiere MXF session/decoder issue is solved.

## Fresh direct-import observation

The actual converter's Separate, Merged and Grouped modes all refuse this retained
23.976 fixture before writing a project:
`no Premiere-observed media timecode format for 24000/1001 fps OP-Atom video yet`.
These are preparation refusals, not three Premiere playback failures.

A fresh native import of the same original video succeeds. The initial import tool
lost its bridge response; after the User reopened the bridge, read-only inspection
confirmed the imported item existed. No import retry was made. This transport failure
is not classified as a Premiere decoder failure or crash.

The native-saved import supplies these scoped facts:

- Exact rate 24000/1001; 10594584000 ticks per picture unit.
- `ClipLoggingInfo/TimecodeFormat` = **110**.
- DNxHR `VideoStream/CodecType` = **1096180840** (`AVdh`).
- Native stream/logging duration = 36 picture units, matching the retained coded
  pictures despite the separate FFmpeg/index terminal-packet count.

A native footage-based sequence exports first, middle and final pictures with pixels
identical to the retained OP1a native exports. The last image is shared by two source
pictures, so unique final-frame attribution remains limited. No continuous playback
or converter Grouped/Merged/Separate construction is qualified by these three frames.
No universal padding rule is derived from the importer's duration choice.

The next implementation assessment can use the new native rate/codec witnesses to
address the direct-route refusal. It must inspect the reader/graph/writer codec and
clock boundaries, retain legacy compatibility, match the Python reference, and qualify
the resulting item constructions natively. Merely assigning code110 does not qualify
all direct routes or solve the independent ProRes case. This research changes no
runtime rate table, codec interpretation or menu behavior.

## Premiere 27 direct linking

The 2026-11-12 follow-up qualified **Separate files** for linked square-pixel
1080p DNxHR LB at exact `24000/1001` with distinct mono PCM24/48 kHz A1 and A2
siblings. Use `--bin-mode separate` in the direct CLI or `"mode": "separate"`
in a saved job. The graph must establish the DNxHR LB coding label, exact matching
file clocks and durations, and discrete audio routing. The converter points to the
three original MXFs and creates no source media.

The linked AAF import supplies two distinct clock values for this profile. The
physical timecode track starts at count 1,387,746. The file source clip adds two
picture frames or 4,004 audio samples, producing effective count 1,387,748. At
the exact rate, Premiere stores the former in `Media/Start` and the latter in
clip logging. The picture stream uses `AVdh` and display code 110. The
timeline in-points stay file-relative. CONV-0071

Premiere 27.0.0 build 88 reopened the Save As project with zero offline media.
Both 72-frame timelines played to the end. Their decoded frames and full PCM
mixes matched the retained OP1a renders. The isolated A1 and A2 renders each
matched all 144,144 PCM24 sample frames and had distinct content. The straight
cut's source ranges and paths also matched the linked AAF read-back. The fade
comparison at that revision used the converter's Constant Power approximation.

### Ordinary picture dissolves

A derived Avid bin combines a saved ordinary Dissolve with the online
23.976 DNxHR LB picture from the split-edit control. Both timelines use
V1 and cut at sequence-relative frame 16. Each dissolve spans eight
frames. The centered window is `[12,20)` and the asymmetric window is
`[14,22)`. Both leave the visible source trims at `[4,20)` and `[12,28)`.

The direct CLI and saved job write the same cut, trim and transition
facts. Premiere 27.0.0 build 88 retains the editable Cross Dissolve
(Legacy) and both endpoint references after Save As, close and reopen.
All three Premiere exports contain 32 frames. Decoded picture matches
the straight-cut control outside each window and at its first frame.
The remaining seven frames differ in each case. A source overrun in the
outgoing handle refuses before publication.

Media Composer exported both sequences with AAF Edit Protocol, linked
media, all tracks, marks off, and rendered effects off. The AAF picture
transitions are eight-frame `VideoDissolve_2` records with cut points 4
and 2, matching the AVB. Premiere 27 retained the intervals and edit
endpoints after Save As, close and reopen. It used the newer Cross
Dissolve (`AE.AE_Impact_Dissolve`) and saved a four-frame alignment for
both. On the incoming edit it saved the AAF's handle-inclusive In at
frame 8 or 10. The visible In in Avid and the direct conversion is frame
12. The AAF import renders the incoming picture four or two frames early
after the dissolve. The decoded frames in that region match the earlier
frames of the straight-cut control. Both projects reference the same
original picture file. The bridge reported that it could not locate a
single imported item. Both compositions were present, reopened, and
rendered. No separate Premiere Events report was captured.

The Avid reference movies and Premiere renders are full 32-frame,
1920×1080, 23.976, legal-range Rec.709 ProRes 422 HQ exports. Mean
squared error is measured in decoded 10-bit Y′CbCr code values against
each Avid reference, over all frames in the given interval.

| Dissolve | Inside, Any NLE / linked AAF | After, Any NLE / linked AAF |
|---|---:|---:|
| Centered | 16.35 / 262.90 | 1.06 / 3532.61 |
| Asymmetric | 22.57 / 592.72 | 1.05 / 2771.82 |

Fitting decoded luma to the two intended source frames at each record
position gives incoming weights near `1/9, 2/9, …, 8/9` in both Avid
exports. The editable Premiere legacy dissolve gives weights near
`0/8, 1/8, …, 7/8`. The AAFs store a Linear foreground-opacity ramp
from 0 to 100 at normalized times `-1/7` and `8/7`. The measured
Avid weights agree with evaluating those points at eight evenly spaced
samples from 0 through 1. This is a measured eight-frame case on the
managed picture profile. The direct output meets the linked AAF parity
criterion for both fixtures and retains the blend sampling difference.
CONV-0040

### Source logging and linked AAF

A derived copy of the linked split-edit bin added distinct Comments, Camroll,
Scene, Take, and custom text to the two source masters, one managed DNxHR and
one UME. The Comments included an accented character and XML punctuation.
Graph/2 retained the five populated columns on each master. Direct CLI and
saved-job conversion agreed on the Premiere values. In Premiere 27.0.0 build
88, Save As, close and reopen retained Comments in the item property bag,
Camroll, Scene and Take in clip logging, and the custom field in Log Note
across 16 file-backed items, including generated edit copies. The managed
Source File value appeared in Description. The saved UME link still resolved
through its media locator. All four original paths, media start values, and
managed media reel names matched the unmodified control. CONV-0014

Media Composer 25.12.0 build 58863 exported a linked AAF from a disposable
copy of the populated mixed timeline. Both master mobs retain all five
values as AAF `UserComments`. Premiere 27 kept the following after Save
As, close and reopen.

| Avid field | Premiere from linked AAF | Premiere from Any NLE |
|---|---|---|
| Camroll | Camera Roll on the linked UME item and the managed file items | Camera Roll on both media types. Managed Tape Name keeps its separate reel value. |
| Comments | UME text survives in an AAF import property. Its Comment column shows the preceding Video tag. The managed Comment column is empty. | Comment column on both media types, with the original text and punctuation. |
| Scene and Take | Scene and Shot | Scene and Shot |
| Custom text | Value in Log Note | Field name and value in Log Note |

The new Camroll mapping uses Premiere's `ClipLoggingInfo/FilmCameraRoll`.
It replaces the earlier Tape Name mapping. Direct CLI and saved-job output
agree, and the updated output retains all four file paths and their saved
physical timecodes after reopen. This comparison covers saved fields and
links. Source LUT metadata remains a separate check. CONV-0129

Two retained gain-and-pan linked AAFs carry `Soundroll TC Rate` with
value `25` and no `Soundroll` comment. Premiere 27 reopened both
imports with `25` in `ClipLoggingInfo/SoundRoll`. Any NLE now writes
that field only from a populated exact `Soundroll` column. It keeps
`Soundroll TC Rate` as labelled text in Log Note. Premiere 27 Save As
and reopen of a derived linked WAV project retained the exact accented
roll on its own master. The second master kept an empty Sound Roll, and
both masters kept the rate in Log Note.
CONV-0165 and CONV-0176

### External source LUT and linked AAF

Media Composer 25.12.0 build 58863 applied a project-scoped external 3D
LUT to the source master and refreshed the 24 fps sequence's color adapter.
The picture source mob saved `_COLOR_INPUT_TRANSFORMATION` with an
`ExternalLut` entry containing the LUT name, relative `.cube` filename, and
dimension. The linked AAF preserved the same XML on the picture source
mob and in the 87-frame ColorAdapter operation for V1. The native project
held the actual `.cube` as a separate file.

Premiere 27.0.0 build 88 retained two frame-zero Unknown Effect markers
for ColorAdapter after Save As, close, and reopen. All 87 decoded picture
frames matched the reopened Any NLE project. The two projects pointed to
different filenames for byte-identical picture MXFs. The initial
comparison covered linked picture and saved metadata.

Graph/2 retained the exact LUT XML and owner. Both direct conversion
and saved jobs reported the clip's source transform and the affected
V1 span. With the same seed and project name, the prepared Premiere
project matched the neutral control after normalizing the bin name.

The follow-up measured the LUT and neutral outputs directly in Avid.
Separate files now uses the selected bin's Avid project `LUTs` folder.
The converter checks the sibling XML against the saved edit-side
ExternalLut, verifies the linked Rec.709 DNxHD picture package, and
embeds the qualified affine transform in an editable Lumetri Look on the
bound V1 edit. Avid's saved reference identifies the LUT by name and
settings. The verification pins the current cube and XML until publication.
A missing cube, conflicting descriptor, or unsupported stack leaves an
edit-specific report.

After Save As and reopen in Premiere 27, direct conversion and saved
jobs kept one original picture reference, exact trims, and both mono
media assignments. In the adjacent-use control, each project rendered
87 neutral frames matching linked AAF and 87 processed frames matching
the retained placed-LUT control. The processed frames measure 48.34 dB
RGB PSNR against Avid's LUT render. Audio was checked structurally for
this picture-only comparison.

The source cube can have 2–65 samples per axis. Each saved value must
agree with a single affine transform to within one millionth per RGB
component, including interior samples. The writer embeds the eight
corners. The derived 33- and 65-point versions of the tested channel
swap produced the same seeded project and graph as the size-two cube.
Premiere 27 reopened the derived 33-point case with the same original,
trims, and effect placement, and rendered all 174 frames identically.
A change to one interior value, with all corners unchanged, still
produces an occurrence-specific loss, as do truncated and oversized grids.
CONV-0175

### Timeline track names

The retained Phase 23 Avid bin records its two custom audio track names
as `_COMMENT` attributes on the direct sequence track slots. The reader
preserves the owner by sequence, picture or sound kind, and one-based
track number. Clip comments and marker text have separate owners.

A derived online control names V1 and V2 identically and gives A1 and A2
distinct names with accented text and punctuation. Both the direct CLI
and saved job write the four names in Separate files, Grouped, and
Merged. Premiere 27.0.0 build 88 saved and reopened the Separate project
with all four names on their intended tracks. Media paths and edit ranges
match the unnamed control. A duplicate saved name attribute on A2 is
reported for that lane. The other three names still write.

A second derived bin adds a direct filler on V5, a filler-only standard
mono A6, and a filler-only stereo A7. The direct CLI and saved jobs write
V5 and A6 in Separate files, Grouped, and Merged. Premiere 27 Save As,
close, and reopen of the Separate project retained both names at the
saved track numbers with zero clips. The 24 populated edit placements
retain the same media paths and trims as the earlier reopened control.
A7 remains source metadata with a lane-specific report.

For media omitted during linked conversion, the same writer retains
named picture lanes and named audio lanes with an explicitly saved
standard mono layout. The lane is empty in Premiere, and omitted edit
spans retain their gap markers and diagnostics. Unverified or stereo
audio layouts still receive a lane-specific track-name loss.

The 35 retained Avid linked AAFs had no names on their 98 picture and
sound composition slots. Four sound slots carried only filler. A fresh
AAF from a named sequence would be needed to compare name preservation.
CONV-0163 and CONV-0164

## 29.97 drop-frame direct DNxHD

Media Composer 25.12.0 build 58863 saved a managed 1920×1080 DNxHD LB
picture and two independent mono PCM24/48 kHz siblings. All three original
MXFs use exact `30000/1001` drop-frame timecode from `02:00:00;00`,
with 150 picture frames and 240,240 audio samples per sibling. The picture
descriptor is `AVHD`, resolution 1253, with the matching DNxHD coding UL.
The sequence starts at `00:00:59;25` and cuts at `00:01:00;02`, selecting
file frames 10–14 and 45–54.

Both the linked AAF and the generated Separate-mode project opened,
saved and reopened in Premiere 27.0.0 build 88. Both renders selected
the 15 numbered Avid frames, and the two Premiere renders had identical
decoded picture. With **Match Avid default pan** enabled, Any NLE's
24,024 samples per output channel matched Avid Direct Out exactly. The
default AAF import retained both source tracks but centered their mix
into both output channels. Fresh insertion from the reopened Any NLE
bin put the full 150-frame original picture and both original mono
siblings on V1, A1, and A2.

Separate mode keeps the exact source file origin and clip logging clock
in the written project, with Premiere's observed `AVdn` codec identifier
and display code 102. After reopen, Premiere retains the picture file origin
and all three logging starts but rewrites each audio `Media/Start` from
1,828,913,371,084,800 to 1,830,744,115,200,000 ticks. This rewrite did not
change the saved edit ranges or PCM in the test.
The saved Avid master TC1 also has count 215,784 with the drop-frame flag.
Graph/2 retains the formatted `02:00:00;00` label and the Premiere Log Note
shows that Avid Start. The linked AAF import keeps the numeric picture
source clock and format 102, with no Avid Start note. The note is separate
from the media's numeric start and the editorial clock.
This is distinct from the qualified 23.976 DNxHR `AVdh`/110 profile.
The result covers same-rate drop-frame edits with a qualified TC1, zero
source offset, and matching discrete PCM24 siblings.

Grouped and Merged now use those same verified sources. Direct CLI and
saved jobs retain the linked AAF's record and source ranges. Premiere 27
build 88 kept the two mono routes and the qualified drop-frame sequence
origin after Save As, close, and reopen. Frame stepping showed
`00:00:59;29` followed by `00:01:00;02` in both projects. The media and
component logging values follow the same pattern as the Separate control.
The combined source sequence owns the reusable bin item's displayed source
clock. With its start set to the verified picture origin, Premiere 27 shows
`02:00:00;00` through `02:00:04;29` for both modes. Changing only the Merged
wrapper's logging range did not fix the display. The production outputs
retain the corrected source clock after Save As and reopen. The editorial
clock and both file-relative trims are unchanged.

Fresh insertion from the corrected bin items saved V1 and two independent
mono lanes over the full 150-frame range. Grouped referenced the source
sequence with channels 0 and 1; Merged used the original picture and each
mono MXF directly. Premiere 27 exported all 15 frames from each reopened
editorial timeline as PNG. Grouped and Merged have identical decoded
RGB pixels. Frame matching against the Separate and linked AAF renders
identifies the same numbered frames (10–14 and 45–54). The comparison
across PNG and ProRes checks frame selection.

Premiere 27's **Render Audio** produces a 150-frame Grouped source
preview. The saved `AudioSequenceSource` links the preview to the combined
source sequence. Both preview channels match all 240,240 decoded samples of
their respective PCM24 mono MXFs. On the editorial timeline, Grouped selects
channels 0 and 1 of that source, and Merged selects channel 0 of each mono
MXF. Both saved projects select file sample spans `[16016, 24024)` and
`[72072, 88088)` on each lane. Reconstructing those slices matches all
24,024 samples per lane in Avid Direct Out and the retained Separate render.
Swapping the lane or slipping the file ranges by one sample fails the
comparison. After Save As and reopen in Premiere 27, both modes exported
A1 and A2 independently as 48 kHz stereo PCM24 WAV. Each intended channel
matched all 24,024 samples of Avid Direct Out, with digital silence on the
opposite channel. 29.97 DNxHR and non-drop require separate qualification.
CONV-0130

## 23.976 direct DNxHD with PCM16

The Media Composer **DNx LB** preset wrote 1920×1080 DNxHD with descriptor
`AVHD` and resolution 1253. The original OP-Atom set contained one picture
MXF and two mono PCM16/48 kHz MXFs. Their exact `24000/1001` physical clocks
start at `02:00:00:00`. The `01:00:00:00` sequence trims select file frames
5–28 and 59–82 inclusive on V1, A1, and A2.

Premiere 27.0.0 build 88 kept both linked AAF audio selections under a
picture-path reference. Any NLE Separate kept each audio MXF's own path.
After Save As and reopen, both projects were online and all 48 decoded
picture frames matched. Both AAF lanes selected the expected source samples,
centered into both outputs at about −3 dB per side. With **Match Avid default
pan** enabled, Any NLE matched all 96,096 expected samples on each discrete
output, with the other output silent for each isolated render. Fresh insertion
preserved the three original MXFs and the full 96-frame extent after reopen.

This profile requires online originals, the observed non-drop DNxHD coding,
matching 16-bit mono siblings, matching clocks and extents, and zero source
offsets. DNxHR with PCM16, offline media, and editable Avid Groups require
separate qualification. CONV-0136

The same native fixture now writes in Grouped and Merged as well. Grouped
sends the two mono siblings to their own channels in the source sequence.
Merged shares each physical media record between its original and component
masters. Both direct CLI and saved jobs retain the same three MXF references,
source and sequence clocks, and two trims on each lane. Premiere 27.0.0
build 88 accepted both generated projects through Save As, close and reopen.
The saved Grouped project's outer A1 and A2 select channels 0 and 1 of the
source sequence, whose own lanes bind the original A1 and A2 MXFs. The saved
Merged project binds each timeline lane to its original mono MXF directly.
The retained linked AAF establishes the same file-relative trims. A wrong
mono sibling was refused without a project. Direct CLI and saved jobs both
identify the duplicate candidate by the selected MXF package ID. Both
reusable bin items initially displayed zero. The writer now sets the
hidden source sequence to the verified `02:00:00:00` source start and
keeps the editorial origin at `01:00:00:00`. After Save As and reopen,
fresh insertion from those clock-corrected items retained all 96 frames
on V1, A1, and A2. Grouped used channels 0 and 1 of the source sequence;
Merged retained the assigned mono MXFs directly. After reopen, Premiere
27 rendered two 24-frame ProRes LT picture previews for each mode. The
saved sequence identifies their record intervals as [0,24) and [24,48).
The 48 decoded frames match exactly between Grouped and Merged. The
numbered chart selects the same source frames as the retained Separate
and linked AAF ProRes HQ exports. The Grouped audio preview also
contains both decoded source lanes, each matching 192,192 source samples.
Following the saved Grouped and Merged edit references and mono selectors
reconstructs 96,096 samples per lane, identical to the corresponding
discrete channel of the reopened Separate export. The saved clip pans
are fully left and right. A one-sample slip and swapped channels both
fail the comparison. After Save As and reopen in Premiere 27, both modes
exported A1 and A2 independently as 48 kHz stereo PCM24 WAV. Each intended
channel matched all 96,096 samples of the corresponding Separate render
after converting the PCM16 reference to PCM24. The opposite channel was
digitally silent.

### Linear audio dissolve against linked AAF

The retained Avid Edit Protocol AAF and the original converter both wrote a
six-frame Constant Power fade in Premiere 27.0.0 build 88. The AAF import
saved a three-frame transition alignment, while the saved Avid cut is two
frames into the fade and the converter writes that two-frame alignment.
The fade spans record frames 18–24 (48 kHz samples `[36036,48048)`).
Reopened exports use the full 72-frame sequence and isolated A1/A2. The
centered mono policy contributes `sqrt(1/2)` to each output channel.

| Premiere fade | Peak PCM24 error outside, A1 / A2 | Peak PCM24 error inside, A1 / A2 |
|---|---:|---:|
| Linked AAF, centered pan | 101,134 / 609 | 48,070 / 491 |
| Prior Any NLE Constant Power, centered pan | 1 / 1 | 10,747 / 97 |
| Constant Gain control, centered pan | 1 / 1 | 4 / 1 |
| Generated Constant Gain, Avid pan policy | 0 / 0 | 5 / 1 |

The two straight-cut controls match the centered Avid reference within one
code on both lanes. The Constant Gain control changes only the fade shape.
The generated project preserves the Avid cut offset, both source trims and
the mono source layout, and retains the editable Constant Gain after Save
As and reopen. With the Avid pan policy, the opposite output on each
isolated lane is digital silence.

Graph/2 retains the two curve values on the saved transition receipt.
Both must establish Linear before preparation selects Constant Gain for
the qualified direct 23.976 DNxHR LB Separate route with two independent
mono PCM24 siblings. The writer checks that the exact span fits the clips
and source handles. Other shapes and constructions keep their existing
approximation. CONV-0094

### Grouped and Merged direct linking

Grouped and Merged use the same qualified three original MXFs and exact
timecode as Separate files. A grouped source sends each inner mono clip to its
own channel to avoid Premiere's center-pan attenuation. Pan on the resulting
timeline edits stays independent. A merged build retains the shared media state
payload for each file and reads the fade handles from its hidden components'
saved source durations. CONV-0072

In Premiere 27.0.0 build 88, both modes survived Save As, close and reopen with
zero offline items. The two timelines played to their ends and matched the
Separate files renders across all 72 frames and 144,144 PCM24 sample frames per
isolated lane per timeline. Fresh insertion of the reopened bin clips produced
the full 36-frame source and matched 72,072 PCM sample frames on each of A1 and
A2. Qualification covers square-pixel 1080p DNxHR LB at exact `24000/1001`
with two mono PCM24/48 kHz sibling files.

### Combine managed and original links

The direct CLI accepts `--link-originals` for the same qualified file-backed
masters that saved jobs use. It uses saved paths or an explicit
`--map-original-root SAVED_ROOT CURRENT_ROOT` mapping. The reader verifies
the original's saved descriptor against its streams and checks the file
revision again before publication. Include `--media` to locate managed MXFs.
Use saved jobs for projects that link originals and also create rewrapped media.

An offline two-bin control contained both 23.976 split-edit sequences and a
separate 24 fps H.264/AAC UME sequence. The Separate CLI and saved-job outputs
agree on all four file paths, track placements, record starts and trims. The
UME edit lanes select physical channels 0 and 1 of the same MP4. Grouped and
Merged saved-job controls retain both source types and the UME channel
selections. The CLI refuses a missing original mapping before publishing
either a project or a requested graph snapshot. The earlier CLI invocation
without original linking now refuses bins with unresolved original links,
with a diagnostic that explains how to convert them.

On Premiere 27.0.0 build 88, the first bridge Open timed out. Opening from
the UI after restart showed the Convert Project prompt for the writer's
version 45 document. Premiere wrote a version 46 copy. Save As, close and
reopen preserved all three timelines, exact starts and trims, and all four
original paths. Premiere reported zero offline items and playback reached
the end of each timeline.

- Both 23.976 timelines matched the retained DNxHR-only control across all
  72 decoded picture frames and 144,144 PCM24 sample frames per isolated
  lane. Fresh full-mix exports of the DNxHR-only control also matched the
  combined project's full mixes sample for sample.
- The UME timeline matched the Premiere 27 UME-only control across all
  48 decoded picture frames. The stereo mix and both isolated mono lane
  exports had 96,000 sample frames each, with the expected silent intervals
  on both lanes. The largest PCM24 difference from the UME-only control
  across two export passes was 41,216 code values. Two passes of the combined
  project also differed by up to 37,376 code values. Sample-exact AAC
  rendering remains open.

Native results cover Separate mode and separate timelines in one project.
Grouped and Merged and a single timeline mixing both source types still
need their own native checks.

### Open Avid conversions directly in Premiere 27

The Avid CLI, saved jobs, and wizard use Project version 46 when writing
Premiere projects. Premiere 27 build 88 opened the generated Separate
and combined linked projects directly and saved and reopened them on
the original media. The linked managed OP1a and the picture-only
rewrap controls also opened directly. Seeded linked outputs differ
from their earlier XML only at the Project version field.

The locally packaged macOS arm64 wizard passed the Separate-files
control. Review found all three original MXFs. **Open in Premiere**
opened the exact output path in build 88 without creating an upgrade
copy. After Save As and reopen, both sequences retained their clocks,
cuts, trims, original paths, and independent mono selectors. In the
rendered split edit, all 48 picture frames and all 96,096 PCM samples
per lane matched the retained Premiere control.

## Ranged filler marker read-back

A graph-authored control over the qualified 29.97 DNxHD originals put six
markers on V1 filler. Three overlapping ranges at `[2,6)`, `[4,8)` and
`[7,10)` share a carrier through frame 10. A point marker at frame 10 uses a
separate one-frame carrier. The marker at `[18,20)` ends exactly where
picture starts. The marker at `[19,22)` keeps its full range on the sequence,
with a report explaining why V1 cannot carry it. All six markers retained
their text, exact positions and durations, and explicit palette values
after Save As and reopen in Premiere 27.0.0 build 88.

The 15 decoded source-picture frames match the marker-free render. Entire
Sequence export begins at the first clip, so the marker carriers add 18
leading black frames to this comparison. This is a destination check using
authored graph facts. A native Avid/linked AAF comparison for coincident
source and edit markers remains open.
CONV-0131

## Coincident source and edit markers

An inheritance match uses a saved creation timestamp along with the exact
source lane, position, duration, and marker content. If both timestamps
are missing, the edit marker reaches the ordinary conflict preference.
With **Keep the other marker as a sequence marker** enabled, both
occurrences can survive. A graph/2 save-and-reload control covers the
undated range. The native Avid marker-copy lifecycle remains open.

The match also needs exactly one source marker and exactly one matching
edit marker on the prepared edit lane. Multiple matching occurrences
reach the conflict preference. A synthetic graph/2 round trip verifies
that both preferences retain all occurrences in the written project
when sequence fallback is enabled. Native duplicate-copy behavior is
still open.

For linked file-backed masters, equal source time triggers the marker
preference only when the source and edit lanes overlap or the lane is
unresolved. Picture and verified independent audio channels can keep
separate markers at the same file position on the edit copy. Audio
uses the source channel the edit selects, including when it is routed
to a different timeline track.

Premiere 27 build 88 retained both ranges after Save As and reopen on
the linked H.264/AAC control. The source A1 marker started at file
frame 3 and lasted two frames. The V1 edit marker fell at the same
file frame and lasted three frames. Both preferences produced the
same project with both markers on the picture edit copy. Premiere's
linked clip collection also carries the original source marker on
the audio components.

Moving the two-frame source marker to V1 made the ranges conflict.
With sequence fallback enabled, each preference kept its chosen
range on the edit and moved the other to record frame 1 with its
duration, color and comment. The CLI, saved job and reopened project
agree on all nine edits and two originals. All 36 decoded pictures
matched. Both same-lane PCM exports matched the repeat cross-lane
export across 72,000 samples per channel. The first cross-lane export
differed by up to 256 PCM24 counts in the last cut. The repeat from
the same saved project reproduced the other two renders. (CONV-0183)

In the verified same-rate direct DNxHD Grouped profile, a picture-only
marker collection can be shared with the picture clip. When a V1 edit
has both picture and audio markers in the collection, the writer applies
the picture conflict preference separately and keeps surviving V1 edit
ranges on the sequence at their exact record positions. The audio
markers stay in the Grouped clip collection. The CLI report, saved-job
receipt, and Review identify the V1 ownership change at the marker's
record timecode or its exact frame from sequence start.

Premiere 27 build 88 reopened the retained same-rate 23.976 mixed-lane
control. It kept the green picture marker on the sequence at record
frames `[2,5)`, and the red audio marker at source frames `[7,11)` with
both comments intact. Premiere moved the red marker into the shared
Grouped sequence collection during its save. The reopened picture and
both audio components reference that one collection, so the red marker
is no longer exclusive to audio. All edit ranges, media paths and
channel assignments stayed the same. The marked and unmarked sequences
match across all 48 decoded pictures and 96,096 stereo PCM16 sample
frames. This verifies range retention for the existing fallback;
Grouped does not guarantee lane-exclusive markers. (CONV-0180)

For the same direct DNxHD Grouped profile, the conflict check compares
the edit's selected audio source track. In the mixed-marker control, the
A1 edit conflicts with the source A1 marker at the same frame. The
source A2 range stays in the Grouped collection under both preferences.
When sequence fallback is enabled, the other A1 marker retains its range
at the corresponding record position.

The Avid reader already captured the source track in the master marker
witness. The graph now carries that track through to the corresponding
Premiere bin item. Premiere preparation can also read the track from an
older graph/2 file's native receipt. An edit marker with a different
duration or a different source track stays independent even when its
start, text and creation stamp match a source marker.

A derived copy of the 29.97 DF test placed a two-frame source marker at
source frame 12 and a three-frame edit marker at record frame 2. With
**Keep the other marker as a sequence marker** enabled and the edit
preferred, the Premiere 27 reopened project retains the edit marker
on its V1 occurrence and the source marker on the sequence. The bin
master also retains the original source marker. All three retain their
own duration, text and palette. The Avid export comparison remains
open after a derived test sequence failed to load in Media Composer.
CONV-0132

The same direct DNxHD control exposed a Grouped writer gap. The shared
marker collection was on the audio template, so the picture edit lacked
the winning range. The writer now also links that collection to the
picture template for verified online direct DNxHD groups with a nonzero
source clock and V1 markers. Direct CLI and saved jobs keep the
winning range on V1 and the other on the sequence under either
preference. Premiere 27 Save As and reopen confirm the edit-preferred
case at the original trims and with the two existing mono selectors.
Merged keeps source V1 markers on the shared picture component. For
same-rate, unretimed V1 edits on the verified direct DNxHD profile with
a nonzero source clock, a marked edit uses its own picture component.
That component follows the same preference and sequence fallback as
Separate and Grouped. Both preferences passed CLI and saved-job checks.
The 29.97 DF edit-preferred Premiere 27 Save As and reopen kept both
markers at the same positions, with the two mono edits and original
media links intact. Other Merged edit-marker cases retain the sequence
fallback. Opening the reusable Merged bin item after reopen also showed
the original two-frame source marker with its saved comment in the
Source Monitor at `02:00:00;12`. This verifies display for picture-only
marker collections in the direct 29.97 DF DNxHD profile with a nonzero
source origin. Other Merged source-marker collections retain the
unverified-timebase diagnostic.

For the direct 23.976 DNxHD LB profile with two PCM16 mono siblings and
a nonzero source origin, each uniquely selected mono file can also carry
its own Merged edit markers. On the derived mixed-lane control, either
A1 preference keeps the chosen marker at source frame 7 and puts the
other on the sequence at record frame 2. The source range lasts two
frames and the edit range lasts four. The independent V1 and A2 ranges
keep their original times, comments, colors and component ownership.
Both preferences passed CLI and saved-job read-back and Premiere 27
Save As and reopen, with the six edits and three original links intact.
Both renders match the linked AAF on all 48 picture frames. Their
96,096 stereo PCM16 sample frames match the unmarked Merged control.
The 29.97 DF and unmarked 23.976 seeded projects are byte-identical to
the previous writer. This test used a derived marked AVB and the
retained unmarked picture AAF.

The Premiere 27 Source Monitor shows the reusable Merged item's origin
at `02:00:00:00` and both two-frame audio ranges from `02:00:00:07`
to `02:00:00:09`. It lists each range twice. A test project with the
markers on only the individual mono components acquired both additional
entries when Premiere saved it. The converter reports this display
behavior for the verified audio-only range profile.

A fresh native edit from the reusable item placed V1, A1 and A2 with
source and record frames `[5,29)`. After Save As and reopen, each mono
component kept one source range and its original MXF. Premiere's export
covered the 24 occupied frames, which match the first cut in the linked
AAF. The isolated 48,048-sample renders match their respective mono
files under Premiere's center pan within one PCM16 count.
CONV-0132

### Native coincident point markers and linked AAF

The 23.976 DNxHD LB/PCM16 control has a red source marker at file frame
16 and a blue edit marker at record frame 11. The first edit starts at
file frame 5, so the markers coincide there. An independent green
edit marker is at record frame 14. The native linked AAF contains the
master event and both timeline events with their names, tracks, comments
and Avid colors. Premiere 27 build 88 reopens the AAF with two sequence
markers at record frames 11 and 14. It keeps the edit comments and
assigns generic names and default colors. The source marker is omitted
from the reopened import.

Any NLE keeps the reusable source marker, both edit comments and all
three names and colors after Save As and reopen. With **Keep the other
marker as a sequence marker** enabled, either preference puts the chosen
marker on the first V1 edit and the other at sequence frame 11. The
nonconflicting edit marker remains on V1. CLI and saved jobs agree on
the markers and six audio and picture edit ranges. All 48 decoded
frames match the AAF import and the retained unmarked control. Both
96,096-sample stereo renders from Any NLE match the unmarked control
under the same pan policy. The linked AAF uses centered audio.

### Retained native sequence span and linked AAF

The saved 24 fps sequence has a green sequence marker from `01:00:01:00`
to `01:00:01:22` (exclusive), 22 frames on the sequence clock. Graph/2
retains the start and duration exactly. The marker has no explicit
saved track binding. The linked AAF Edit Protocol export from the
24 fps project completed after Avid warned that media was offline.
The AAF event slots contain a grey source point and a grey timeline
point at record frame 16. They contain no event for the green span.
After Save As and reopen, Premiere 27 build 88 shows the grey event
as a default-green point at frame 16, plus an Unknown Effect point
for SpatialAdapter at frame 0.

A marker-only project written from the saved graph fact retains the
green span at the exact start and duration after Premiere Save As
and reopen. The Premiere bridge and the saved project independently
show a start of 24 frames and a duration of 22 frames on the
24 fps sequence with its original 01-hour start. Normal conversion
of the retained timeline still refuses its unqualified mixed-rate
Motion Adapter.

An online control copies the saved span into an isolated same-rate
87-frame sequence with verified DNxHD LB and two PCM16 siblings.
Media Composer's linked Edit Protocol AAF carries the span in a
24 fps TimelineMobSlot named `Segmentation`. A 24-frame filler precedes
the 22-frame DescriptiveMarker, whose green color and length are
explicit. The AAF's three media locators resolve to matching MXFs.
Premiere 27 build 88 imports the sequence with zero markers and
reopens it with zero. Both Any NLE entry points write the exact green
range, and it survives Premiere Save As and reopen. The exports match
all 87 decoded frames and differ by at most one PCM16 count across
174,000 stereo sample frames under center pan. (CONV-0172)

The online control changes the containing sequence and media state
together. It establishes that Premiere dropped this saved span despite
its presence in a linked, playable AAF. Native source ranges and copied
range ownership remain separate checks.

### Derived source span and edit copy

Two derived 24 fps controls place a red marker at file frames `[100,122)`
and save a V1 marker at record frames `[24,46)`. The edit begins at
file frame 76. Names, comments, colors, and lengths agree. In one
sequence the creation timestamps match, and in the other they differ
by one second.

Media Composer's linked AAFs contain a MasterMob marker and a
CompositionMob marker, each with Length 22 and Position zero in its
event slot. Premiere 27 build 88 reopens each AAF with one generic green
point at record frame zero. The text of the comment survives.

With **Keep the other marker as a sequence marker** enabled, the
reopened Any NLE project retains one red V1 marker for the matching copy.
For the independent pair it retains the V1 marker and a red sequence
marker at `[24,46)`. The CLI and saved job agree. The independent-range
render matches all 87 picture frames and remains within one PCM16
count across 174,000 stereo sample frames compared with its AAF.
The following native edit tests the copy operation. (CONV-0173)

### Native source range and point copy

Media Composer 25.12 saved two online 24 fps overwrites with **Copy
Source Markers** enabled. The disposable source was seeded with the
retained 22-frame range at file frames [100, 122). The first overwrite
uses that exact span. In the second, a 30-frame source selection fully
encloses the range and also contains a point marker at file frame 98.

In both saved AVBs the range remains on the source. In the paired
control, Avid saves the point as an edit-owned copy with its original
creation timestamp and a two-frame offset into the new cut. Any NLE
projects the source range into each picture edit. The paired control's
project has one point per placement, including the placement with the
saved copy, and no duplicate sequence marker with the keep-both
setting. This test seeded the range before loading the bin in Avid.
In a separate direct-authoring attempt, the mapped Composer marker
button opened a one-frame marker with a 12-frame source selection
active. The dialog was cancelled. Direct source range authoring remains
open.

Media Composer exported the exact paired sequence as a linked AAF Edit
Protocol file with all tracks, marks off, and effects rendering off. The
AAF contained zero embedded essence and two point events, one on the
source at file frame 98 and one on the composition at record frame 89.
After Save As, close, and reopen in Premiere 27 build 88, the AAF import
had the copied point at record frame 89 as a green sequence marker with
the original comment and a generic name. It had no saved source range
or source point marker. Premiere also added an Unknown Effect marker
for the Color Adapter at record frame 87.

The direct CLI and saved-job projects agree on the saved edit and
marker facts. After the CLI project reopened in Premiere 27, the
picture edits still showed the red points at record frames 22 and 89
and the red ranges at [24, 46) and [91, 113), with their saved names
and comments. Both reopened projects retained the 01:00:00:00 start,
the two picture edits and four mono audio edits, and their exact source
and record ranges. Any NLE reports the Color Adapter against the edit
that starts at frame 87. This paired comparison checked saved edit and
marker data.

The combined macOS package replay includes the first native overwrite.
After Save As and reopen in Premiere 27 build 88, the two occurrences
retain the 22-frame red range at record frames [24, 46) and [87, 109),
with the same name and comment. All 109 decoded frames match the
expected same-source positions. The report identifies the Color Adapter
saved on the appended picture edit as an appearance loss.

## Disabled clip occurrences

The retained Phase 36 and 37 Avid bins compare the same timeline
before and after re-enabling six clips. The disabled version has an
explicit mute flag, a selected same-span filler and an inactive
backing edit. At 24 fps, the disabled ranges are 28 frames each.
They start at record frame 168 on V1, 84 and 140 on A1, 112 and 140
on A2, and 28 on stereo A3. After re-enable the wrappers disappear
and the saved media selections and trims match the inactive branches.

Graph/2 retains the lane, record span and backing component class.
For an immediate source clip it also records the backing reference. A plain
mono selector with no processing on the selector or its branches can be
recovered in Separate files mode when the original same-rate managed
PCM16 media, channel, exact source range and vacant record span are verified.
The disabled stereo combiner retains its two ordered backing references
when both are direct, unprocessed, equal-length mono clips from one
master with the same trim. The verified disabled V1 case also carries
the backing file package ID from its saved picture reference.

The linked AAF exported from a byte-identical bin copy contained all six
selectors and zero embedded essence, with marks and effect rendering off.
Avid warned that media was offline. Premiere 27 imported the timeline
without any editable disabled clips. This is a structural comparison, with
no AAF playback claim.

The direct CLI and saved job wrote four disabled 28-frame mono clips at
the saved trims and left all enabled edits at the same ranges and paths.
Premiere 27 build 88 reopened their muted state. Re-enabling only the edit
on A1 at record frame 84 survived a second Save As and reopen. The 56,000
samples in that range were silent when disabled and matched the source
within one PCM24 step under Premiere's center pan when re-enabled.
Both PCM lanes of the stereo A1 export were identical outside the range.
CONV-0153

Both entry points also preserve the disabled A3 stereo combiner in
Separate files mode. Its record interval is [28, 56) and both channels
select source frames [21, 49) on the verified 24 fps managed PCM16 pair.
Premiere 27 build 88 retained the disabled state, trims, and media
links after Save As and reopen. The isolated A3 render was identical
to the earlier project with a gap. Re-enabling just that edit and
reopening restored exactly 56,000 samples per channel, matching the
re-enabled Avid-bin conversion and the two original MXFs. Every sample
outside the edit matched the disabled render. The reader rejects
reversed selectors and unequal trims from this recovery path.
CONV-0157

The three original MXFs were brought online in Media Composer
25.12.0 build 58863. A 1920×1080 native picture mixdown used the first
six active V1 edits as controls. Their Framing, Valid, Essence and
Source rectangles are all centered at zero with size 1600×900. The
disabled seventh edit carries the identical override and source trim.
Avid's six active ranges match the full-raster source at frames
[21, 49), with a mean absolute downsampled luma difference of 0.42 on
an 8-bit scale. Center crop and shrink-to-fit interpretations gave
differences of 40.35 and 58.64. The disabled range [168, 196) is black.

Both Separate files entry points now recover that disabled picture
edit when the exact saved framing pattern, direct V1 route, cached
file package, source trim and verified 24 fps DNxHD LB media agree.
Premiere 27 build 88 kept the disable state after Save As and reopen.
After re-enable and reopen, its final 28 frames match its earlier
enabled repetition at every decoded pixel, and its first 168 frame
hashes are unchanged. Across the active Avid and Premiere renders,
downsampled mean absolute luma difference is 0.21 on an 8-bit scale.
A derived bin with a shifted framing rectangle still reports the
missing V1 edit through both entry points. The old linked AAF
comparison remains structural because the export media was offline.
CONV-0158

## Stereo combiner on a dual-mono master

The retained 24 fps bins have a stereo edit on A3 over a master with
two independent mono channels. The timeline combiner gives their
left/right order. Separate files uses this pair to build a linked
audio-only stereo sequence for the verified PCM16/48 kHz profile with
matching zero-origin clocks. The graph preserves the original source
layout. The writer uses the existing Processed Clips bin organization.

Both conversion paths retained the stereo edits and original picture
and mono trims. Premiere 27 build 88 Save As and reopen retained all
three original MXF paths. On isolated A3, both channels matched the
corresponding original file for all 56,000 samples of the first edit.
The muted control had silence afterward. The re-enabled control played
both edits at the saved positions with 112,000 matching samples per
channel and silence afterward. Linked AAF behavior and nondefault
per-channel pan remain separate checks. CONV-0150

## Eight-lane AutoSync from imported WAV

Media Composer AutoSync matched a separate 24 fps camera clip to a
48 kHz PCM16 polyphonic BWF by source timecode. Sound started at
`00:59:59:22`, two frames before the camera at `01:00:00:00`. Avid imported
the eight WAV channels into eight managed mono MXFs. Each decoded sibling
matched all 144,000 samples of its original WAV channel exactly. A 24-frame
edit trimmed four frames from the synced item, selecting camera frames 4–27
and audio samples 12,000–59,999 on each lane.

The linked AAF retained the synced composition and the trimmed edit.
Premiere 27 build 88 imported it as an editable sequence for the synced
item and a separate cut sequence. Any NLE Separate files preserved the cut
as independent linked clips and reported that it omitted the reusable item.
After Save As and reopen, both cut sequences kept the same trims and eight
managed MXF paths. All 24 decoded picture frames matched exactly. The
isolated stereo PCM24 export of each centered mono lane matched the AAF
within one low-order bit. Both routes selected the same samples under
Premiere's center-pan gain.

The converter used an explicitly mapped copy of the synthetic camera MOV
with a square-pixel tag. The saved original lacked that tag and hit the
existing picture-profile refusal. The AAF used the saved picture path.
The original media and managed siblings stayed intact.

### Reusable AutoSync source sequences

Separate files can now rebuild a same-rate 1080p24 AutoSync bin item from a
separate linked H.264 MOV and up to eight independent 48 kHz PCM16 managed
mono MXFs. The saved source references, offsets and In/Out determine each
lane's range. The Premiere project keeps the named source sequence in the
originating bin. For the verified linked picture route, Avid saves TC1 on
a physical source mob. The reader follows the file source reference and
accepts the observed wrappers with zero-length boundary fillers. The
eight-mic fixture now displays its saved `01:00:00:00` in Premiere's Source
Monitor. Save As, reopen and fresh insertion preserved V1 and A1–A8 with
the exact prior ranges and paths. If the saved picture clock is unresolved,
the source sequence uses a zero display start and reports that fallback.

To use it as source material, open the sequence in Premiere's Source Monitor
and turn off **Insert and Overwrite Sequences as Nests or Individual Clips**
in the destination timeline. Patch V1 and the desired audio tracks. This
inserts the actual source clips on their own lanes.

Premiere 27 build 88 saved and reopened the named source sequence. A fresh
insert at `01:00:01:00` used all 48 picture frames and selected sound samples
4,000–99,999 from each of the eight corresponding MXFs. After a second Save
As, close and reopen, picture cadence matched and the eight isolated PCM24
renders agreed with the source under Premiere's center-pan gain to within
one PCM24 quantization step. The preceding edit retained identical decoded
picture and PCM. Direct CLI and saved jobs agree on the reusable and existing
timeline structure. Other rates, Grouped/Merged/Rewrap, and direct linking of
the original polyphonic WAV are outside this check.
CONV-0151

### Original eight-channel WAV in Premiere

Premiere 27 build 88 natively imported the original 48 kHz PCM16 BWF as
one project item with eight mono source components. The components had
one common file key and media selectors 0–7. A fresh insertion of source
frames [6, 30) put one clip on each of A1–A8. After Save As, close and
reopen, each of the 48,000-sample lane renders matched the retained
linked AAF within one PCM24 step under the same center-pan settings.

A private prototype applied the saved native audio components to the two
AutoSync sequences. It kept the linked picture and exact edit ranges.
Both sequences reopened with one WAV master, the eight correct selectors,
and the BWF origin of 172,796,000 samples (00:59:59:22 at 24 fps). In
the edited sequence, every lane matched the AAF again within one PCM24
step. In the reusable source sequence, every lane selected samples
[4,000, 100,000), matching the two-frame AutoSync offset and the source
PCM to within one PCM24 step. Picture was checked by saved placement.

The prototype checked every WAV channel against its corresponding
verified managed MXF. A same-name WAV with the first two channels swapped
kept the same clock, format, and extent but failed that check.
CONV-0154

Graph/2 now retains the candidate in an `avid.audio_import` record on
the sound master. It requires agreement between the import and physical
source locators, eight distinct managed PCM16 file packages linked
without offsets, and one common 24 fps non-drop physical timecode.
The saved AVB sound labels are 1–8. The linked AAF uses sound slots
1 and 3–9 because slot 2 carries timecode. Conflicting locators suppress
the candidate. CONV-0155

Separate files can use the original WAV through the direct CLI with
`--link-originals` and through saved jobs. Qualification requires the
saved non-drop 24 fps clock, a complete 48 kHz PCM16 eight-channel BWF,
and one online managed PCM16 sibling for each channel. Full package IDs,
essence boundaries, sample extent and every channel's PCM must agree.
The saved locator or its explicit root mapping locates the candidate.
An absent file or a same-named file with swapped channels retains all
eight managed links and the original edit on both entry points.

After Save As, close and reopen in Premiere 27 build 88, the generated
project retained V1, all eight audio selectors, exact edit and source
ranges, one WAV master, and the BWF origin. Each 48,000-sample isolated
lane matched the linked AAF within one PCM24 step. Inserting the reusable
source at 01:00:01:00 as individual clips retained the preceding edit and
saved a further 48 frames on V1 and [4,000, 100,000) on each of A1–A8
after another Save As and reopen. All 72 decoded frames matched the
earlier managed-media insertion, and each of the eight inserted PCM
renders matched its source within one PCM24 step under center pan.
Grouped, Merged, other audio layouts and rate combinations retain
their existing managed-media paths.
CONV-0156

## User's fallback decision and remaining work

The User's direction on 2026-11-06 is to offer OP-Atom→OP1a as the sole Avid→Premiere
method if direct OP-Atom playback cannot be fixed/qualified. OP1a remains the planned
first/default choice. Record the sole-route fallback as a decision gate, not an
already implemented removal of the legacy choices.

Before making that global product change, define handling/refusal for audio-only
masters and codecs outside the current picture-led DNx/PCM rewrap scope. Never fall
back silently to a known-unqualified direct route after a rewrap refusal. Held areas
remain held. User listening, exact fade equivalence, broader native cohorts and
corrected absolute clocks remain separate qualification tasks.

## Mirrored PVOL volume ramps

The retained 24 fps mono control uses the same WAV and a two-second edit
trimmed two seconds into the source. Its four editable volume keys are
at edit-relative frames 0, 12, 24 and 47, at −12, 0, 0 and −12 dB.
Graph/2, the direct CLI, the saved job, and the retained linked AAF
reopened in Premiere 27 agree at those four positions and levels.
The separate subframe control retains its second key at 12.53613 frames
in the converter output, compared with frame 12 in the reopened AAF.

Gain measurements use 48-sample windows against each editor's own
unprocessed reference. On the −12 dB control, the first Avid Direct Out
ramp fits a straight line in dB within 0.001 dB RMS. The final ramp has
two fitted slopes, approximately −0.34 and −0.63 dB per frame, which
intersect about 65,530 samples from the start of the edit. The
opposite-direction −6 dB ramp has a fitted intersection near sample
65,525. The whole-frame and subframe versions of that control differ
at the second key and have identical PCM from the third key onward.
Four window widths from 24 to 192 samples reproduce both intersection
estimates to within one sample. Their proximity to sample 65,536 gives
the next test a fixed sample position to check. The linked AAF render of the
−12 dB control differs from Avid by 0.40 dB RMS over the full edit.
The −12 dB endpoints match, while the saved −6 dB control reaches
−6.10 dB in Avid and −6.00 dB in the AAF render.

An independent search across the final ramp found intersections at about
65,528 and 65,533 samples for the rising and falling Avid controls. The
search used the source-energy centroid of each window and supplied no
candidate break position. Across four window widths, both Avid controls
fit two lines within 0.000002 dB RMS. At 48 samples per window the linked
AAF renders still had residuals of 0.015 and 0.060 dB RMS after the same
two-line fit. The saved keys alone therefore cannot serve as a render
reference for these controls.

The follow-up keeps the same source trim, clip duration, and −12 dB
endpoints, and moves the last two keys from frames 24 and 47 to 17 and
44. Media Composer 25.12.0 build 58863 exported the dry and both ramp
controls as mono PCM24 Direct Out. The measured final-ramp slope change
stayed near sample 65,533 for both key arrangements. Across four window
widths, the shifted control's estimates differ by less than one sample
and are thousands of samples away from the tested key-relative predictions.

A second control moved only the source trim by seven frames. Its dry
render matched the new source samples and the measured gain envelope kept
the same slopes and slope change. Moving the edit eight frames later on
the sequence, with the original trim and the same keys, changed both
ramps. After the 16,000 samples of leading silence, its dry render
matched the original dry PCM. The processed render has slope changes at
approximately 32,766, 65,534, and 98,301 samples from sequence start,
close to successive multiples of 32,768. This establishes a timeline
position dependency for the control. The calculation of the boundaries
and behavior at other rates remain open.

A prediction from the saved keys explains most of the remaining render
difference in the retained controls. At each sequence-relative multiple
of 32,768 samples, interpolate the neighboring saved gains in linear
amplitude. Convert those intermediate gains and the saved gains to dB,
round down to a tenth, then interpolate in dB between those values.
For example, the original and shifted final ramps predict −3.0 and
−5.0 dB at sample 65,536. The moved edit predicts −2.3, −2.2 and
−9.7 dB at its three interior boundaries. These predictions use the
saved edit position and source-relative key times without fitting
parameters to individual renders.

Across the six retained 24 fps mono PCM24 Direct Out controls, this
prediction has 0.00043–0.00088 dB RMS error in 48-sample windows,
compared with 0.169–0.869 dB for a straight line in dB between saved
keys. The same pattern holds with
24- and 96-sample windows. Rounding only the intermediate gains misses
the −6.1 dB plateau in both the whole-frame and subframe controls.
The saved amplitude in those controls is slightly below −6 dB. The
exact sample boundaries, behavior at other rates, and a destination
mapping that preserves editable intent still need separate checks.
CONV-0185

The staged 23.976 control uses a verified 48 kHz managed mono source.
The two sequences share the same 40-frame trim, placed eight frames
into the sequence. One is dry and the other has a short volume ramp
that ends at sequence sample 65,568. On a fixed sample grid, the
65,536-sample boundary falls inside the ramp. On a grid scaled by the
edit rate, the boundary lands near sample 65,601.536, after the last
key of that ramp. With the decoded source PCM, simulated renders
separate those predictions by about 0.59 dB RMS in the boundary window.
Both converter entry points preserve all five editable keys at their
exact sample times and keep the dry edit unchanged.

Media Composer 25.12.0 build 58863 exported both controls as mono PCM24
Direct Out after managed relink. The dry export matches all 96,096
expected samples, including the silent head. The relinked copies kept
the trims and saved volume points. Four measurement windows place the
first rising ramp's slope change at samples 32,766.43–32,766.57, close
to the 32,768-sample boundary predicted from the 24 fps tests. The short
fall crosses the next boundary without the predicted −9.4 dB knot. In
the boundary window the fixed-grid prediction errs by 0.70 dB RMS,
compared with 0.11 dB for a straight dB ramp between the saved points.
The fixed grid describes the first ramp in this test. The second ramp
shows that the 24 fps prediction still needs a rule for short key spans.

The linked AAF contains all five gain points. Premiere 27 build 88 kept
the A1 trim and clock after Save As and reopen, but saved four keys.
It collapsed the two close subframe points at samples 65,296 and
65,568 into a unity key at 64,064. The reopened Any NLE project retained
all five Linear keys and their exact times. The stereo PCM24 exports
contained the 80,080 active samples on each channel. Over that interval,
relative to the dry control, the Any NLE volume envelope differs from Avid
by 0.432 dB RMS per channel and the AAF import by 4.156 dB RMS. Across the
short ramp the corresponding errors are 0.912 and 7.168 dB RMS.
The converter keeps the exact editable keys and continues to report the
interpolation difference. CONV-0185

In the earlier shifted-key 24 fps test, linked AAF and Any NLE retained
the same four editable keys and the original WAV trim after Save As,
close and reopen in Premiere 27 build 88.
Their dry and shifted stereo PCM24 exports agree within one count per
sample on both channels. Normalizing each shifted export against its own
dry reference gives about 0.506 dB RMS error and 1.087 dB peak error
against Avid Direct Out over the two-second edit. The current converter
keeps the editable keys and reports the interpolation difference.
CONV-0177

## Editable managed mono clip pan

The verified 1080p24 direct-link profile now keeps Linear mono clip pan in
Separate files mode. It uses online DNxHD LB picture and two independent
48 kHz PCM16 mono MXF siblings. Key positions follow the selected sound
file's source clock. The scope covers straight, unprocessed mono cuts.

For CONV-0159, the four values from the retained native PVOL control
were applied to a disposable managed edit, with its final key moved to
fit the shorter edit. Direct CLI and saved jobs preserved record frames
`[56,84)`, source frames `[22,50)`, and Linear source key frames 22, 34,
46 and 49. The direct project preserved those facts after Save As and
reopen in Premiere 27.0.0 build 88. Its isolated stereo render selected the
expected 56,000 source samples, reached hard left and hard right at the
first two keys, and kept the adjacent A1 edits centered. The pan-envelope
difference established by the WAV comparison is still reported. This
derived managed control has a Premiere render and uses the retained WAV
AAF comparison for context.

Grouped supports the same managed mono controls (CONV-0160). The
mono-only control uses the same saved edits and original media, with the
unrelated stereo lane removed. Direct CLI, saved job, and Premiere 27
Save As and reopen agree on the three linked originals, both mono lanes,
the exact trim, and all four editable keys on A1. The reopened renders
match the Grouped dry control sample for sample on A2 and on A1 outside
the panned edit. The panned A1 render agrees with the Separate files
reference within one PCM24 count at the Grouped clip's existing
3.01 dB lower playback level in that build. The direct-routing update
below supersedes that level result for the verified profile.

An out-of-range raw pan value still produces an occurrence-specific loss.
The unchanged Separate and Grouped controls preserve their seeded project
and graph. The Merged construction has its own media-state repair.

### 24 fps Merged media state and clip pan

The earlier Merged writer linked the component copies to the same three
`Media` objects, then converted those objects' only `ModificationState`
payloads to empty hash references. Premiere 27 rejected the dry project.
Restoring the three payloads was the entire seeded XML delta for the
repaired dry project, which opened and survived Save As, close and reopen.
The fix uses the existing direct media route for verified online zero-origin
1080p24 DNxHD LB picture and two 48 kHz PCM16 mono siblings. It retains
24 fps display code 100 and links all three originals in place.

Both entry points retain the same edit ranges and original media
paths. After Premiere 27 Save As and reopen, the panned project kept all
four Linear keys on A1 at the same source positions as Separate files.
All 196 picture frames match the repaired dry project. Each isolated
audio lane contains 392,000 sample frames. A1 agrees exactly with Separate
files, and A2 and the unpanned A1 regions match the dry Merged project
exactly. The converter reports the known difference between Avid's and
Premiere's pan envelopes. Other clocks and stereo combiners remain open.
CONV-0161

### 24 fps Grouped routing and fresh insertion

Grouped admits the same verified 24 fps zero-origin DNxHD LB picture and
two PCM16/48 kHz mono siblings as Merged. Each mono file feeds its own
channel in the Grouped source sequence, and the timeline edit still owns
its editable pan. This removes the earlier 3.01 dB attenuation on the
tested mono cuts without a gain adjustment. After Premiere 27 Save As
and reopen, Grouped matches Merged over all 196 picture frames and
392,000 sample frames per isolated lane. Both conversion entry points
agree on the original paths, trims, channel selections, and four A1 keys.

After fresh insertion into disposable copies and another reopen, both
bin constructions place the full 242-frame source at record frame 216
on V1, A1, and A2. The timelines agree on all 458 picture frames and
916,000 sample frames per lane, including the original edits. Each
inserted mono lane selects its own full 484,000-sample sibling, matching
the source within one PCM24 count under the same centered pan policy.
CONV-0162

## Private evidence and reproduction

Workspace audit: `local/outputs/converter-split-edit-audio-2026-11-06/`.
No native projects, source names, WAVs, PNGs or presets are added to Git.

- `reference.json`, `prepare_reference.py`, `source-decode-double-check.json`:
  exact reference geometry, hashes and independent decoder agreement.
- `audio-verification.json`, `audio-double-check.json`, `verify_audio.py`:
  complete sample comparisons and refusal controls. Use a fresh `--output` path.
- `mcp-render-calls.json`: export scripts, native receipts and restored track state.
- `direct-route-preparation.json`: the three CLI refusals and exact arguments.
- `mxf-native-import/`: native saves, frame exports, complete tool receipts, transport
  observations and the code110/AVdh observation.
- `native-import-verification.json`, `verify_native_import.py`: reproduce native
  saved-byte facts, picture equality and preservation of the prior sequences.

The final research project remains open with the original control sequence active.
The earlier native-return file is preserved. The private final receipt records the
current project path, exact revisions and all qualification limitations.
