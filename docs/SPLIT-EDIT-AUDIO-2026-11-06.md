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
A7 remains source metadata with a lane-specific report. Other empty audio
layouts and tracks containing unsupported content are outside this mapping.

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
This is distinct from the qualified 23.976 DNxHR `AVdh`/110 profile.
The result covers same-rate drop-frame edits with a qualified TC1, zero
source offset, and matching discrete PCM24 siblings. It does not qualify
29.97 DNxHR, non-drop, or Grouped and Merged output at this rate.
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

This profile is online, non-drop, and Separate only. It requires the observed
DNxHD coding, matching 16-bit mono siblings, matching clocks and extents, and
zero source offsets. It does not establish DNxHR with PCM16, offline links,
Grouped/Merged, or editable Groups. CONV-0136

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
Motion Adapter. This check establishes marker placement and the scope
of this offline AAF export. Keep online AAF behavior, native coincident
source/edit ranges, and inherited-copy behavior open separately.

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
