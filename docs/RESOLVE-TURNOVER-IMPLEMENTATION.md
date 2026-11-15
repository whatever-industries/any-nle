# Internal Resolve turnovers

`resolve-turnover-probe JOB.json` prepares and publishes a linked DRP from saved
Avid bins or a Premiere project. The two sources feed the same graph/2
destination preparation and archive writer. The probe uses original files in
place. Resolve Studio 21.2.0 build 00006, project version 17, remains the native
qualification target.

## Inputs and review

The probe accepts an internal `anynle.job/2` document. Set `schema` to
`anynle.job/2` and put source settings under `source` with a `route` of
`avid_to_resolve` or `premiere_to_resolve`. The earlier flat probe input is
still accepted. The production CLI and wizard continue to use job/1.

The Avid job selects one or more `bins`, ordered `sequences`, and
`media_roots` for managed media. Select a unique saved name or an exact
graph sequence ID. `avid_project_root` can preserve nested Avid folders
from the selected files' paths. Each bin must be inside that root.
`mappings` optionally supplies saved and current path prefixes for UME.

A Premiere job supplies `premiere` and one `sequence` selector (a
one-element `sequences` list in the earlier flat probe input).
Use `resolve-turnover-probe --list-premiere-sequences INPUT.prproj`
to read sequence ObjectUIDs, names, rates, canvas, saved start ticks,
and track, clip and transition counts. This reads only the project.
Use a listed ObjectUID when several sequences share a name.

The prototype admits straight cuts on 1080p24 or 1080p23.976, a saved
whole-frame origin, picture-only ProRes HQ MOV and 48 kHz PCM16/24 WAV.
It follows the saved audio selector through its source reference, checks
cached timing against the file, and admits independent mono selections on
two tracks feeding one stereo Mix bus. The static mixer profile below
defines which mixer controls transfer. Source and record ranges must agree.
Premiere source bins retain panel ownership and stable IDs. Captions keep
their content, lane and exact record span in graph/2. The reader captures
sequence Comment markers with exact ticks and preserves bounded native
receipts by occurrence. Review reports unsupported markers, captions and
components by occurrence.

Both jobs supply `ffprobe`, `templates`, and `output`. The templates are
the existing hash-pinned layout and rate profile, plus the applicable Atom
and Group donors. Use `--review` to check preparation, template assembly
and the output path without staging an archive, or `--graph` to inspect
the source document. Reviewable omissions require
`--accept-losses` on the publication run. Timing, media, routing and topology
failures still block publication. Each input project and media binding is
checked again before exclusive publication.

### Camera picture with independent editorial audio

An internal Avid job can prefer camera originals from verified Avid source
columns. With a verified 23.976 stereo proxy rewrap, Resolve links the camera
picture and attaches the rewrapped MOV as its proxy. The edited mono audio
uses the verified Avid MXF siblings. Review reports **Creates new media.**,
the planned proxy paths, and the estimated bytes when it plans a rewrap.
Picture-only proxy attachment uses the existing MXF in place.

A derived 23.976 bin combines two camera picture cuts, both managed PCM24
siblings, and a linked stereo WAV on the second A2 edit. Archive read-back
retains the exact `01:00:00:00` timeline and `02:00:00:00` camera starts and
all six edit ranges. A1 uses its own MXF for both edits. A2 uses its own MXF
for the first edit and the WAV's right channel for the second. The proxy
contains all 96 verified compressed picture frames. Each audio channel matches
its original mono MXF sample for sample. An occupied proxy path blocks review.

Resolve Studio 21.2.0.6 imported, saved, closed, and reopened the combined
project with the originals online. A fresh DRP export kept all six file
identities, trims, clocks, audio channel selectors, and the proxy attachment.
Resolve matched all 48 decoded picture frames and 96,096 PCM24 samples per
mono lane against a timeline assembled directly in Resolve using the same
source files and ranges. The audio also matched the selected source samples
within one PCM24 step after accounting for the stereo bus pan and the
72-sample clip-edge ramps.

### Require camera originals

Set `source.require_camera_originals` to `true` in an internal Avid job/2
to require a single verified original for every selected picture edit and
every camera in a selected editable Group. The option defaults to off.
With it enabled, preparation also looks for originals using the saved Avid
source columns and any supplied path mappings. Verified existing UME
picture links qualify. Managed editorial audio, linked WAV audio, and
explicit proxy attachment keep their respective media policies.

If picture would fall back to managed media, preparation reports the
affected timelines, tracks, and record positions or camera names. The
report counts all failed bindings and shows up to 16. It fails before
writing a DRP or proxy. The internal probe has exercised the strict gate
on the mixed editorial audio control and a picture-only camera original.
The wizard and saved-path navigation remain pending.

## Premiere sequence markers

On qualified Premiere → Resolve timelines, a Comment marker on the
sequence retains its name, comment, and record-relative time in graph/2.
Resolve receives a whole-frame marker at the same position. A point
uses Resolve's one-frame marker duration. Whole-frame ranges retain
their saved length. Green, Red, Blue, Cyan and Yellow map by name.
The installed Premiere 27 exposes eight marker color indices and Resolve
accepts sixteen named colors. When Premiere's color has no counterpart,
Resolve uses Lavender for Magenta, Sand for Orange, and Cream for White.
Review reports the substitution and graph/2 keeps the Premiere color.
A marker whose position or duration cannot be represented, whose start
is shared with another marker, or whose type or payload is unsupported
is reported by occurrence and requires loss approval.
Saved values remain in graph/2 or the bounded source receipt. Clip
markers and richer Premiere marker types remain outside this profile.

A Premiere 27.0.0 build 88 Save As and reopen supplied a 24 fps
sequence beginning at `01:00:00:01`. Resolve Studio 21.2.0 build 00006
imported, saved, closed and reopened the generated DRP with all five
sequence markers, including frame zero, a cut boundary and two
four-frame ranges. Names, multiline accented comments, Green, Red,
Blue and Yellow survived. The six linked edit ranges and both WAV
channel selectors were unchanged. The same Premiere project's linked
AAF contained all five marker positions and comments, with no embedded
essence. Its marker entries lacked names, colors and range lengths.
Resolve imported the AAF at the same timeline origin and reopened it
with zero timeline markers. That import did not reconnect the originals
in the tested setup, so it does not qualify AAF media or audio behavior.

## Premiere timeline In/Out

The Premiere 27 reader retains saved `MZ.InPoint` and `MZ.OutPoint` ticks in a
`premiere.timeline_marks` sidecar on the sequence. Whole-frame values also
carry their saved frame number. Subframe values retain their exact ticks, and
the unset sentinel produces no mark. Resolve turnover reports each mark as an
omission and requires `--accept-losses` before writing. Premiere export tests
must establish the saved Out boundary before the route writes a destination
range. Timeline In/Out is separate from work area, clip trims and timecode origin.

## Premiere timeline track state

The Premiere 27 reader records custom track names, edit locks and the
V1 output toggle in `premiere.track_state`. Each record is attached
to the selected sequence and identifies a lane by kind and number.
On the internal V1/A1/A2 linked profile, the Resolve writer uses the
same track preparation as Resolve round trips. It writes the timeline
fields and the Fairlight mirrors for audio names and edit locks.
Disabling V1 leaves all picture edits and both mono routes in the project.

Sync lock is distinct from edit lock. A disabled Premiere sync lock
remains in `premiere.track_sync_lock` and requires approval to omit.
Invalid or duplicate track flags block output. Audio track mute is
handled through the saved fader control described below.

Offline variants of a retained Premiere 27 save kept accented text,
duplicate audio track names, edit locks and a disabled V1. The archive
read-back retained the original sequence origin, six edit ranges,
physical WAV selectors and five sequence markers.

Both variants were imported as separate Resolve Studio 21.2.0 build
00006 projects, saved, closed, reopened, and re-exported. Resolve
kept both identically named mono tracks and their separate edit locks.
The archive comparison found only the V1 output toggle. V1 off rendered
all 48 frames black with both picture edits still in place. Each isolated
audio lane had 96,000 identical PCM24 sample frames with picture on
and off. The saved selectors still identify the two distinct channels.

### Premiere static audio mixer

For V1 and two independent mono WAV lanes, the turnover keeps static
Audio Track Mixer volume and mute on each track. The Premiere Mix fader
becomes Resolve's main stereo bus fader. Track and bus gain use tenths
of a decibel from −6 through 0 dB. Mute is independent of track gain,
clip volume, and the original channel selector. Mixer automation,
contradictory saved values, and gains outside this profile block output.

The 23.976 control used a retained Premiere 27 save with its clip-edge
transitions removed from a disposable copy. Resolve Studio 21.2.0 build
00006 reopened it with A1 muted, track levels of −3 and −6 dB, and a
−3.7 dB main bus. The saved mix matched isolated A2 exactly across all
96,096 PCM24 sample frames. Re-enabling A1 reproduced the unmuted
control, and each lane matched its expected gain within one PCM24 code
value relative to the unity control.

The internal route also preserves editable static track pan from Premiere's
mono-to-stereo panner on A1 and A2. The saved hard-right default and explicit
values map to Resolve's signed tenths of a percent. A retained Premiere 27
save reopened in Resolve with hard left and right on the correct lanes, A1
muted, and the saved clip, track, and Mix levels intact. The centered,
swapped, and ±50% controls varied only the saved track pan on disposable
copies of that project. Across 96,096 sample frames per lane, swapping hard
pan swapped the output channels exactly. The interior PCM matched the trimmed
source and saved gain within one PCM24 code value. Resolve applied its
previously measured 72-sample ramps at the edit edges.

At ±50%, the quieter channel measured about 27% of the louder channel in
Resolve and 41% in the retained Premiere control. Review reports the use of
Resolve's pan law for intermediate settings. Animated pan, other mixer
routing, and linked pan combined with custom track state remain outside
this profile.

### Premiere clip-edge Constant Gain

On the internal two-mono WAV route, a Constant Gain transition
with one saved endpoint becomes an editable Resolve fade on that clip.
Graph/2 keeps the original transition ID and length on the owning edit.
The reader checks the lane list, the clip's head or tail reference, the
alignment, and the exact record interval. The visible trims stay the same.
The profile covers 24 and 23.976 fps, including both edges of one clip and
static clip volume. Other fade curves and a crossfade on the same lane
require separate mappings. Clip pan and unqualified clip processing on a
faded edit block the conversion.

The Premiere 27.0.0 build 88 control reopened in Resolve Studio 21.2.0
with a six-frame tail on A1 and an eight-frame head on A2 at 23.976 fps.
Both 96,096-sample PCM24 lanes match a fade-free copy sample for sample
outside their fade intervals. The interiors follow the linear-amplitude
curve within one PCM24 code value. A separate source variant put a
12-frame head and an 18-frame tail on A1 with static clip, track and Mix
gain, track pan, and A2 mute. Both fades and the mixer controls survived
reopen, with all 48 picture frames matching the unfaded control.

For fractional fades, graph/2 keeps the original Premiere tick lengths.
Resolve stores them as fractional frame counts. The internal writer requires
the conversion to recover the exact source tick and admit at least one audio
sample per fade. On a disposable copy of a Premiere 27 reopened save with
clip panners removed, the 23.976 head and tail lengths of 64,714,012,088 and
86,625,055,551 ticks survived Resolve reopen and conversion back to saved
Premiere XML. The independent WAV channels and static clip levels survived.
Across 96,096 PCM24 sample frames per lane, the audio outside each fade
matched a fade-free copy exactly. The interior linear-curve residual was
at most 10 code values for the head and one for the tail. Fractional fades
with custom curve points or volume keys remain unqualified.

## Premiere timeline clip labels

For Premiere 27 projects, graph/2 records the timeline edit's saved label
slot and packed color in `premiere.clip_label`. When both fields agree with
the source templates, the edit uses the destination default. Resolve receives
an explicit clip color for a distinct edit label, even when two edits share
one master. The route uses the saved Premiere slot to choose a standard
Resolve color and reports substitutions or a nonstandard saved swatch by
edit. Unrecognized label fields require approval to omit. Duplicate label
fields block preparation. Media Pool labels are outside this mapping.

The offline control changes six timeline label pairs on a copy of the
retained Premiere 27 project. Five write as edit-owned Resolve colors and
the unknown slot remains in graph/2 with an omission report. Resolve Studio
21.2 build 00006 kept all five labels and one uncolored neighbor after import,
save, close and reopen. Re-export retained both mono channel selectors, all six
edit ranges and the nonzero timeline start. The Premiere label changes were
made to a disposable project copy; a separate Premiere 27 authoring check
for changed labels remains open.

## Premiere Cross Dissolve

The internal 1080p23.976 turnover maps one eight-frame V1 Cross Dissolve
(Legacy) with left or centered alignment to an editable Resolve Cross
Dissolve. The reader requires the outgoing clip's saved tail reference
and the incoming clip's saved head reference to agree with the transition
and cut. Preparation verifies the source handles and preserves the
visible trims. The turnover reports the blend sampling difference by
transition occurrence.

The two retained Premiere 27 saves cut at record frame 24. After saving,
closing, reopening, and re-exporting in Resolve Studio 21.2.0 build
00006, both retained the cut, original links, audio selectors, and
blend intervals of frames 16–23 and 20–27. All 48 decoded frames in
each project matched its corresponding native Resolve control. Each
isolated audio lane matched the Resolve straight-cut control across
all 96,096 PCM24 sample frames. Premiere starts each blend with the
outgoing image, while the Resolve blend is already visible on that frame.

## Premiere Constant Gain crossfade

The internal 1080p24 turnover maps one centered eight-frame Constant
Gain transition on A1 to an editable Resolve Cross Fade 0DB. It verifies
both saved clip references, the fade settings, and four frames of linked
WAV source on each side of the cut. The visible trims and separate mono
assignments survive. Other fade types and transition endpoints with clip
processing require separate qualification.

The retained Premiere 27 save came from the earlier Resolve-to-Premiere
crossfade trial. After save, close, reopen, and re-export in Resolve
Studio 21.2.0 build 00006, the transition kept its frame-24 cut and
the eight-frame interval starting at frame 20. Each isolated lane
matched the corresponding native Resolve PCM24 render across all
96,000 sample frames. A2 also matched the straight-cut control.

### Combined picture and audio transitions

A 1080p24 timeline can carry one qualified V1 Cross Dissolve and one
qualified A1 Constant Gain crossfade. Each transition keeps its own
cut, clip references, and handles. Two transitions on the same lane
are outside this profile.

An offline Premiere fixture combined a centered picture cut at frame
20 and a centered audio cut at frame 24. After saving and reopening
in Resolve Studio 21.2.0 build 00006, both eight-frame transitions
survived with all five visible edits. All 48 decoded pictures matched
the picture-only transition control. Both isolated PCM24 renders
matched the audio-only transition control across 96,000 sample frames
per lane. The linked Resolve writer also reproduces the combined
project from the saved graph of either editor.

## Premiere clip components

For Premiere 27 project version 46, `--graph` includes a
`premiere.component_parameters` sidecar on each edit with supported, saved
clip controls. It carries Motion Position, Scale, Scale Width, Uniform
Scale, Rotation and four crop edges, plus the Level parameter of the Internal Volume Mono
clip component.
Parameter IDs are scoped to the Motion owner. Audio Level is identified by
its parameter slot and carrier within the mono clip-volume owner.

The sidecar keeps the saved component and parameter object IDs, component
order, value units, CurrentValue, StartKeyframe, and the stored keyframe
list. Each key retains the exact signed tick, value, interpolation fields
2 and 3, and the remaining fields in saved order. Missing flags and
values stay absent from the saved data, represented by null. No default
component is synthesized. Audio Level stays in Premiere's saved numeric
domain, distinct from clip gain, track volume and display dB. Position
stays in Premiere's normalized coordinates.

Capture follows the observed 14-field point and eight-field scalar
records. Version 46 synthetic turnover checks independently compare the
receipts with the input XML, including subframe ticks, distinct start and
current values, and curves with an omitted IsTimeVarying flag. The
underlying record shapes are pinned to PRPROJ-0030 and PRPROJ-0013.
The source reader puts qualified static Motion in the edit's graph/2
`transform` and static mono Level in `volume_db`. Resolve preparation
reads those fields from the saved graph and verified media bindings.
The position and scale profile requires a matched 1920×1080 square-pixel clip, neutral
anchor, rotation, anti-flicker and crop, and scale between 87.5% and
112.5%. Static mono Level maps to −6 through +3 dB in exact tenths with
neutral clip mute. Resolve writes it on an independent PCM WAV lane.
The original Premiere component values remain in the sidecar.

Qualified two-key Linear Motion travels as `picture.motion_linear` in
graph/2. The reader converts the saved ticks to exact item-source seconds
and retains Linear interpolation. This profile requires uniform scale, a
neutral first key, and both Position and Scale keys on the first and last
visible source frame. Resolve preparation checks the keys on the verified
source frame grid. Other appearance remains a per-edit loss and requires
`--accept-losses`.

### Premiere Linear rotation with independent width

On 1080p24, three aligned Linear keys on Position X, Scale Width, and
Rotation map to editable Pan, Zoom X, and Rotation Angle. Scale Height,
Position Y, pivot, crop, and anti-flicker remain neutral. The first and
last keys must match the visible source bounds. Graph/2 retains both the
original Premiere keys and the interpreted curves. The Resolve writer
uses the interpreted source-frame times, with Premiere's clockwise
rotation converted to Resolve's opposite sign.

The retained Premiere 27.0.0 build 88 save supplied the Motion values.
A disposable copy added an explicit one-hour timeline origin for the
turnover reader. After saving and reopening the exported archive in
Resolve Studio 21.2.0.0006, all nine keys survived. All 13 decoded frames
matched the earlier Avid-to-Resolve render of the same edit.

### Premiere centered static crop, scale, and rotation

At 1080p24, the turnover reads isolated centered rotation and the
centered crop profile with independent scale and clockwise rotation.
The crop edges stay in source space. Graph/2 carries the saved
Premiere percentages in the component receipt and the corresponding
frame fractions on the edit. The crop limit accounts for the rounding
observed in Premiere 27's saved percentages.

A disposable copy of the retained Premiere 27 save added an explicit
one-hour timeline origin. After archive reopen in Resolve Studio
21.2.0.0006, the four edits retained editable crop, zoom, and rotation.
All four decoded frames matched the earlier Avid-to-Resolve control
exactly. All 28 measured chart points agreed with Avid within 0.09 px.
Animated crop, displaced pivots, and other effect stacks require
separate qualification in this direction.

### Static Motion and volume trial

An offline static-only variant of the retained Premiere 27 save matched
the baseline turnover and the replay from saved graph/2 and media
bindings. All six edits matched on archive read-back and had identical
effect payloads. The replay also matched after removing the native
component receipts.

The Linear Motion replay kept the retained Premiere save's two picture
controls and static volume. A disposable copy cleared the unrelated
animated volume and logging omissions for the strict linked probe. Its
saved graph produced matching timeline read-back and effect bytes across
all six edits. The Resolve archive held Linear keys on all four Motion
axes at source frames 5 and 28.

### Native Premiere 27 and Resolve mapping trial

A Premiere 27.0.0 build 88 Save As and reopen preserved six online edits at
24 fps and 1920×1080. A separate Resolve Studio 21.2.0 build 00006
candidate was built from the captured values, then saved, closed, reopened,
re-exported and rendered. The trial covers the following cases
(CONV-0069).

| Control | Result after Resolve reopen |
|---|---|
| Static Motion | Position `[0.525, 0.475]` and independent 112.5% width and 87.5% height became Pan 48 px, Tilt 27 px and the same independent zooms. |
| Two-key Linear Motion | Position and uniform zoom retained their two keys at source frames 5 and 28 on a trimmed edit. Resolve omitted redundant scalar bases for the neutral starting values. The reader recognizes that saved encoding and requires a neutral initial key. |
| Static mono clip volume | Level mapped to exactly −6 dB using the Premiere Level conversion. Each editor's rendered change was within 0.001 dB of −6 dB relative to its own neutral control. |
| Animated mono clip volume | The −6 dB and +3 dB keys and their times survived. The rendered ramp differed by up to 0.28 dB at the measured points, so matching endpoint values and timing is insufficient to qualify it. |

The picture comparison covered all 48 decoded frames. Mean absolute
grayscale difference was at most 0.41 on a 0–255 scale after downsampling
to 480×270. This is a scoped comparison for the matched canvas, same
source rate and tested controls.

The turnover job subsequently applied the three qualified controls
directly to the saved Premiere 27 project. In Resolve 21.2.0 build 00006,
Save Project As, close and reopen preserved all six online edits, their
source trims and the 01:00:00:01 origin. All 48 decoded frames matched the
earlier Resolve control exactly. The stereo PCM24 render matched the
native −6 dB control for the first edit and the neutral control for the
second, sample for sample across 96,000 frames. Review reported the
animated volume ramp and the existing clip metadata omissions.

## Capture in graph/2

Resolve archive reading now retains the following scoped facts for future
destination mappings.

| Graph location | Saved facts | Destination status |
|---|---|---|
| `resolve.transitions`, sequence | Occurrence, track identity, saved start and duration, and separate alignment and position codes | The internal linked DRP path reads and writes the [qualified eight-frame V1 Cross Dissolve](RESOLVE-DRP-FORMAT-CLOSEOUT.md#qualified-left-aligned-cross-dissolve). Cross-editor and other alignment mappings remain open. |
| `resolve.timing`, sequence | Timing evidence, compact extents, and recognized time-map points, explicit zero flags and saved handles | Cadence and audio treatment pending |
| `resolve.fairlight`, sequence | Track and bus columns identified by strip ID, pan, bypass, solo, gain, output assignment, EQ/dynamics fields and bounded plug-in records | The [drpkit closeout](RESOLVE-DRPKIT-CLOSEOUT.md) records static mono pan, direct routing, and Bus 2 gain and mute in the linked turnover profile. |
| `resolve.clip_fairlight`, placement | Keyed Voice Isolation and Dialogue Leveler records with saved identity, flags and normalized parameters | Source capture only |
| `resolve.clip_fades`, placement | Clip-edge lengths and custom fade points in percent and tenths of a dB | Custom curve interpolation and writing pending |
| `resolve.source`, sequence | Native video and audio mark fields | Writing pending |
| `resolve.clip_label`, placement | Saved clip color code from the item lockable blob | Destination palette mapping pending |

The retained dissolve saves AlignmentType 3 and Position 2 on the same
occurrence. The reader preserves both. A same-build archive with a retimed
clip supplied a keyed Sm2TimeMap with decoded points. Playback behavior is
tested independently of these saved values.

## Qualification boundary

The Premiere source control combines retained native Premiere picture
records with synthetic edits. The generated project opened, saved under a
new name and reopened in Resolve with its 01:00:00:01 origin, six online
placements, exact trims and separate left and right selectors. A second
Resolve timeline rebuilt the edits with native insertion and native channel
mapping. It matched all 48 decoded picture frames and both isolated
96,000-sample PCM lanes exactly. Both versions exhibited the same short
edge envelopes and stereo mix coefficient.

The automatic Avid probe reopened a nested, two-timeline split-MXF and
editable Group project, and separately reopened an H.264 UME project. A
picture-only Group angle switch changed the picture while both isolated
PCM lanes remained byte-identical to the Save As copy. A retained combined
turnover also passed Save As and reopen. Product rollout still needs an
automatically produced same-rate mixed UME and managed-media cohort, a native
Premiere-authored source cohort, a distributable template review, and the
packaged CLI, saved job and wizard gate. The current probe is internal.
See the [Resolve project qualification](RESOLVE-PROJECT-CORE.md) and
[roadmap](ROADMAP.md#davinci-resolve-user-2026-10-26-bookmark-and-research).
