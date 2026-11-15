# Editable Avid Groups over native OP1a media

The first bounded route is implemented in the typed Premiere writer. It converts
two same-rate, continuous 1920×1080 DNx cameras at 24000/1001 with two independent
PCM24/48 kHz mono lanes per camera. Picture angles remain editable; audio stays on
the first camera as established by the saved selectors. Leading and trailing gaps
within each camera's extent are preserved. The native test covers DNxHR LB in
Premiere 26.5.2 build 5 on macOS. Wider compatibility and User acceptance remain open.

## Where the 3 dB went

The initial native comparison measured
−3.0103 dB for native Camera 1 / Mono multicam compared with direct mono placement.
The source recording was silent, so the audio experiment used separately generated
sources with distinguishable tones. The source sequence's fixed stereo-to-adaptive
track routing selects L for the first lane and R for the second. Default mono clip
pan had already attenuated the signal reaching those channels. The outer edited
sequence applied the same mono-to-stereo placement as its direct reference.

| Controlled change in a clone of the native source | Premiere PCM24 result |
| --- | --- |
| XML round trip only | Identical to the attenuated native mix |
| Explicit center pan on the first inner mono clip | Identical to the attenuated native mix |
| Hard left on the first inner mono clip only | First isolated lane matches direct PCM; second lane stays byte-identical to its attenuated reference |
| Hard left/right on the two inner mono clips | Mix and both isolated lanes match direct PCM byte for byte |

The writer uses the existing mono clip-pan encoding for **transport inside the
multicam source**. It preserves the outer edit's ordinary gain, pan and mix policy.
No fitted gain, latency offset, or universal claim about Premiere's pan law follows.
The unaffected ordinary stereo nest remains a useful negative control.

## What the writer constructs

Graph/2 remains a source document. Typed preparation resolves the complete Group
and each bound edit before writing, using the full Group identity and the existing
source placement bindings. Audio alternatives resolve by leaf identity, channel
label and span; an audio alternative index does not imply a video camera index.
Each camera must have one continuous qualified media piece with exact frame edges;
observed filler may surround it. Both default audio lanes must match the first
camera's saved extent. Missing, ambiguous, mixed-clock or discontinuous alternatives
refuse with a named reason. The rewrap route still requires fixed audio selectors.

The group item is a native sequence-backed multicam with two video tracks, both
cameras' audio, two exposed mono groups, and the alternate camera audio muted in
the source. The picture selector and group-local In/Out are written per edited use.
Separate audio placements retain their channel groups across picture changes. Both
leaf masters are planned even when an angle is unused, and each verified file is
rewrapped once on the native rewrap route. The CLI and saved jobs share construction.
The Separate files route can also link the qualified original camera MXFs.

The first profile caps prepared groups at 256 and uses the existing event limits
and accounted input bounds. Model/1, graph/1, the Python model/1 oracle, ordinary
master output and held FCPXML behavior are unchanged. Native Group marks and markers
remain in source facts with a scoped loss. An unavailable exact sequence start keeps
the existing compatibility origin: the test bin displays `00:59:56:09` at sequence
start under that policy, while relative edits and the `00:00:06:01` span are exact.
This is separate from Group routing; no new record clock was inferred.

## Checks on generated output

- The actual native Avid bin went through the reader, source graph, typed preparation,
  verified two-file OP1a rewrap, and Premiere writer. Two saved Group identities, one
  from each authoring command, became distinct native source items. This covers their
  equal bounded layouts in this fixture, not general MultiGroup equivalence.
- Six frame samples from the generated A/B/A edit match the native reference pixels.
  Two match again after native save, close and reopen. Both generated source items
  insert afresh with two mono lanes. Switching to camera B in the leading gap shows
  the expected camera B pixels, including after reopening. One normal-speed pass of
  the reopened edit reached its endpoint.
- The actual source audio is silent. The same typed writer with independently
  generated tone media rendered the mix and both isolated lanes at 48 kHz, 24-bit,
  290,290 sample frames. Every PCM sample equals the direct reference, before and
  after native save, close and reopen. This covers fixed audio, not audio following
  video or independently switching audio angles.
- The final writer reproduced the original generated project byte for byte from the
  same pinned source facts, seed and verified media bindings. The original bin, six
  input media files and four pre-existing Premiere project files match their retained
  hashes. Pre-existing projects remained open.

Missing-audio cameras, wider or paired layouts, retimed/mixed-rate members, multipart
MultiGroups, sequence clock correction and broader playback remain separate gates.

Private receipts live under `local/outputs/multicam-translation-2026-11-08/`.
`level-isolation/ISOLATION.json` SHA256
`2a7b52b45d259ffa2021fbd5d41bc28d72b01aaf6687303f2fb6bd8185513c3d`;
`editable-output-v1/NATIVE-VERIFICATION.json` SHA256
`802701d6eeba93db41bcfcbf3b0e8c296a8572bf719c2a2330aafff130681b23`.
These were same-author native checks. Continuous looping and User acceptance
remain open.

## Link the original OP-Atom camera files

Choose **Separate files** in a saved job or use `convert --bin-mode separate`
with the managed media root supplied by `--media`. If **Grouped** or
**Merged** is selected, Review offers **Switch to Separate files** and
keeps the current offline media setting. Run preflight again after switching.
The writer admits two
1080p23.976 DNxHR LB cameras, each with two discrete PCM24/48 kHz mono MXFs,
qualified file and logging clocks, and the existing continuous Group layout.
The reusable Group links all six original files and keeps the saved picture
angle editable. In the original control, both default mono outputs use the
first camera.
A missing or unqualified sibling or an unresolved Group relationship
remains outside this profile. The three-mono extension has a separate
[qualification record](#three-independent-mono-group-outputs).

Premiere 27.0.0 build 88 retained the six linked paths, 01:00:00:00 start,
A/B/A picture selections, and both mono assignments after Save As and
reopen. All 145 decoded frames matched Premiere's linked AAF import.
Both imports rendered the original silent audio. A separate graph-authored
control used online MXFs with distinguishable PCM. Both reopened lanes
matched the flat direct-link reference over 48,048 samples each. A picture
selector variant kept both lanes sample-identical. A fresh bin insertion
created one picture edit and two mono edits and kept them on reopen.
CONV-0095

### DNxHD LB cameras with PCM24

Separate files also supports online 1920×1080 DNxHD LB cameras at exact
24000/1001 with two independent 48 kHz PCM24 mono siblings. Each camera uses
the Premiere descriptor for its verified picture coding. A saved Avid Group
with one continuous camera on all lanes reopened in Premiere 27 build 88.
The integrated CLI reproduces the input to that native check byte for byte,
and the saved-job output agrees after generated IDs are normalized. The
retained DNxHR Group project and graph are byte-identical to their baseline.

A separate graph-authored switch uses the same originals. After Save As and
reopen it retains A/B/A picture and A1, and A/A/A on A2. All 120 frames match
the expected source colors, and both 240,240-sample lanes match the selected
sources at PCM16 export precision with Avid pan matching. See the native
[independent A1 comparison](#native-independent-a1-switch) below.
CONV-0096

The native DNxHD picture-switch control also passed a linked AAF comparison.
The AAF stores alternate picture and audio choices in Selectors. After
Save As and reopen, Premiere 27 uses ordinary linked picture edits for the
AAF import. Any NLE keeps the picture angles editable. Both retain the
sequence start and exact edit ranges, and all 120 decoded frames match.
With Avid pan matching, Any NLE matches each original mono file sample
for sample over 240,240 PCM24 samples. The AAF import center-pans each
lane to both sides. Both audio lanes use the default camera in this control.

### Independent mono audio edits

An exact saved alternate audio selection on the Separate files route writes
a direct mono timeline clip linked to the selected camera's original MXF.
The clip uses its own file-relative trim, gain and pan. The picture remains
an editable Group, and first-camera audio still uses the Group output. The
converter reports which audio occurrence loses in-Group angle switching.
The conversion report also counts these directly linked timeline edits
separately from the Group's default mono outputs.
Review identifies each affected edit by sequence, track, and record timecode
when the saved sequence clock is qualified. With an unknown sequence start,
it shows the frame offset from the start instead.

Premiere 27 build 88 reopened two graph-authored A/B/A controls. Each changed
only A1 over record frames 8–15. Both isolated 48,048-sample lanes matched a
flat direct-link control byte for byte. In one test the switch silenced only
that span. In the reciprocal test the same span played camera B's signal
while A2 stayed silent. All 24 decoded picture frames in the first test
matched the fixed-audio control. A private AVB variant changed only the
middle A1 selector byte. The AVB reader resolved camera B's mono leaf
and nonzero trim through the normal conversion entry points. Premiere
reopened the result, and all 145 picture frames matched the retained
fixed-audio AAF. Both lanes in that AVB use silent sources. A fresh
four-bin CLI replay in Premiere 27 build 88 kept the Group and all nine
placements after Save As, close and reopen. All 145 decoded frames
matched the retained reference and both isolated 290,290-sample mono
exports remained silent. The matching saved job was checked by saved
references. A fresh render of each audible graph control after reopen
matched both flat control lanes sample for sample. The reciprocal control
confines camera B audio to A1 samples 16,016–32,031.
In a separate graph control, changing only the middle picture angle
changed exactly its eight rendered frames. Both saved audio bindings and
the 48,048-sample mix stayed identical after reopen.

Known edit ranges inherited from a source marker can also survive. When
a placed Group uses its shared source, Premiere preparation keeps the
marker on the sequence at its saved record time and reports the ownership
change. An alternate mono edit linked directly to the original MXF keeps
its inherited source marker there. The converter avoids adding a second
sequence marker for that occurrence. A graph-authored control over the
retained selector variant kept the picture marker at sequence frame 50
and the mono marker at file frame 74 after Premiere 27 Save As and reopen.
Both three-frame ranges, the A/B/A picture choices, and all six original
paths and trims survived.
CONV-0096

A derived control with two audible DNxHD LB cameras changed only the middle
A1 selector in copies of the native fixed-audio AVB and its linked AAF.
Premiere 27 reopened both conversions at the saved start with all eight
edit ranges and all 120 picture frames intact. With Avid pan matching,
Any NLE matched all 240,240 PCM24 sample frames on each isolated lane.
Only A1 switched during record frames 40–79, and A2 was sample-identical to
the fixed control. The AAF import selected the same sources and center-panned
each lane. Any NLE kept editable multicam picture and reported the middle A1
edit's direct mono link. The direct CLI and saved job agreed on saved
timeline facts.

### Native independent A1 switch

A Media Composer 25.12 test changed only the middle A1 segment, over
record frames 40–79 at 23.976. The native bin and linked Edit Protocol
AAF both retained that selection and the continuous A2 binding. Premiere
27 build 88 reopened both paths with all eight edit ranges and identical
decoded picture on all 120 frames. The AAF imported the four picture
edits as ordinary clips. Any NLE kept an editable multicam picture and
linked the switched A1 edit directly to its original mono MXF, with an
occurrence-specific editability notice. Direct CLI and saved jobs agree
on the saved timeline facts.

With Avid pan matching, Any NLE matched all 240,240 PCM24 samples per
lane from the selected original MXFs. It also matched all 240,239
samples per lane in Avid's two-channel Direct Out export. Premiere wrote
one additional final sample per lane, equal to the original media at
that position. The linked AAF chose the same source ranges and
center-panned each lane, with a residual below two PCM24 codes after
accounting for the pan law. A2 matched the earlier fixed-audio conversion
sample for sample. This closes the independent mono switch comparison
for the two-camera DNxHD LB/PCM24 Separate files profile.

### Audio layouts and Groups

Ordinary source clips, timeline tracks and multicam output describe
different parts of audio routing. A mono MXF can remain on a separate
mono edit, and a saved pair or surround channel group describes how its
member channels move together as one clip. The native rewrap route has
separate coverage for ordered stereo pairs, 5.1 and eight-channel
Adaptive. The master output mix is a distinct saved choice, retained
and reported separately from clip routing.
See [native AV-master construction](MEDIA-REWRAP.md#optional-native-av-master-construction).

The tested editable Group has two mono outputs. Default Group audio
comes from the multicam source. An independently switched edit
keeps its selected mono source and range as an ordinary clip, so a later
camera change on that edit requires replacing its audio source.

On the direct Separate files route, each mono output can now use the
saved default camera for its own lane. A derived copy of the native
switch fixture changes only the Group's A1 default to its second camera.
Both converter entry points use that camera for the Group's left output,
keep the first camera on the right, and retain the saved timeline
selections. The first writer put the second camera’s A1 on the third internal track.
After Save As and reopen, Premiere 27 build 88 saved the expected source
and trim but rendered that lane silent. The silence affected all 80,080
samples in the middle A1 edit and the A1 output of a fresh insertion.

The writer now places the selected A1 and A2 sources on the first two
internal audio tracks and keeps the alternates on muted tracks. The
seeded project where both defaults use the first camera is byte-identical.
After reopening the corrected project, all 120 picture frames match the
linked AAF and both 240,240-sample PCM24 lanes match the retained edit
renders exactly. A fresh insertion of the reusable Group retains V1 and
two mono lanes after Save As and reopen. Its isolated A1 and A2 exports
match the second and first cameras respectively within one PCM24 count
with Premiere’s center-pan coefficient applied to the source comparison.
CONV-0182

#### Three independent mono Group outputs

Premiere 27 rendered three independent mono lanes from a derived
multicam source over three distinct linked signals. Its third inner
strip carries the signal on the right side. Full-right clip pan and
the standard second-pair matrix `[[0,[2]],[1,[3]]]` routed it to
physical output 4. The multicam exposed physical outputs 1, 2, and
4 as three consecutive mono clip channels. This uses zero-based
`mSourceChannelIndex` values 0, 1, and 3.

A fresh insertion from the Group bin item placed V1 and A1–A3.
Premiere saved the third audio item's logical `OrigChGrp` and
`SecondaryIndex` as 2 and its resolved `SecondaryContent/ChannelIndex`
as 3. After Save As and reopen, each of the three existing and three
fresh lane renders matched its own 240,240 source sample frames after
center pan and PCM16 quantization. The writer now reads the physical
output from the Group's channel serializer and keeps the logical
index in the placed edit. An offline regression preserves logical
group 2 and physical output 3 and rejects a missing group or output.
CONV-0191

The earlier left-panned version was silent when exported directly
from inner track 3, even through the first output pair. Removing the
explicit pan restored the right-channel signal with center-pan
attenuation. Setting full right restored its full level. A reference
placement made by Premiere on that same strip produced the same
right-channel result without an explicit pan. The initial experiment
had qualified output 3 from inner track 1. CONV-0190

Separate files now writes a scoped three-mono Group. It requires two
continuous 1080p23.976 DNx LB cameras and three separately linked
PCM24/48 kHz mono siblings per camera, with matching clocks and extents.
The default A3 strip sends its right input to the fourth bus output.
An independent A3 switch places the selected camera's mono MXF directly
on the timeline and leaves the Group's reusable default and picture
selection intact. A synthetic graph/2 control verifies these saved
relationships and rejects contradictory selectors or a missing sibling.
The retained bin inventory found no earlier Group with three saved audio
lanes. The test uses two synthetic 23.976 camera originals, each with distinct source timecode, numbered picture and three
independent PCM24/48 kHz mono tracks. Each audio stream was decoded and
compared against its generated signal. The comparison checks 72
picture selections and 144,144 samples per lane. A simulated wrong
camera changes all 48,048 middle A3 samples and leaves A1 and A2
identical.

Check the saved graph before exporting AAF or renders. The private source
check compares the selected Group alternative and its bound edit against
the expected camera, range and physical mono sibling for each lane. It
also checks the defaults separately from the timeline selections, and
refuses a reused package or file. The retained two-lane bin reproduced
the known independent switch. Synthetic three-lane controls caught a
wrong middle A3 camera, a wrong stream, and a missing or reused sibling.
These tests cover source selection and validation.

Media Composer linked both originals and transcoded them to managed DNx
LB and six PCM24/48 kHz mono siblings. Each picture has 96 decoded frames
at the saved 23.976 clock. All six audio files have distinct file package
IDs and each of their 192,192 samples matches the corresponding original
channel. Avid saved the native 72-frame Group sequence after syncing
the two In points. The saved picture choices are B/A/B, and the three
mono lanes initially use B throughout. The direct conversion of a
test copy containing the managed masters retains all 12 placements
and eight original MXF paths in project read-back.

The saved independent switch changes only the middle A3 edit to camera
A's third MXF. The linked AAF retains all twelve selected source
packages and exact trims. The imported AAF and initial Any NLE
project both reopen with all media online in Premiere 27 build 88.
All 72 decoded picture frames match, and Any NLE retains editable
picture angle choices. A1 and A2 select their intended signals across
144,144 sample frames each. The middle A3 edit plays from its direct
MXF. Both outer A3 segments from the multicam source were silent with
either center pan or Avid-style pan.

The saved inner Group declared two adaptive audio channels. Changing
that count to eight, as saved in the earlier working Premiere fixture,
restores its physical fourth output. With the same center pan as the
AAF import, each A3 segment agrees with the AAF to within one PCM24
count in both stereo output channels. Changing the placed edit's
`SecondaryIndex` alone leaves the two outer segments silent. The writer
now declares eight adaptive channels for the three-mono source, keeping
the logical slot 2 and physical output 3 selectors. The rebuilt
converter's project passed Save As and reopen in Premiere 27. The
three editable picture angle choices and 72 decoded picture frames
match the source selections and AAF render. Each isolated stereo
PCM24 export agrees with the corresponding AAF lane within one
sample count per channel across all 144,144 sample frames under
Premiere's center pan. A regression checks the hidden bus declaration
alongside the existing edit and path checks.

A fresh Premiere bridge insertion from the reusable Group placed
V1 and three separate mono items. After Save As and reopen, each
lane matched all 192,192 samples of its own default camera B MXF
within one PCM24 count per channel after center pan, with the
expected six-frame silent tail. The bridge inserts picture as a
regular nest. It plays B for the first six frames, then composites
the upper camera A angle over it.

For a fresh multicam edit, open the Group from the bin in Premiere
27's Source Monitor, enable nested-sequence insertion in the target
timeline, and use the Source Monitor's Overwrite button at the start
of an empty sequence. The edit saves as multicam with angle 1 selected
automatically. After Save As, close, and reopen, all 96 numbered
picture frames come from camera B. The three independent PCM24
signals match their own camera B MXFs within one count per channel
after the same center pan. The last six frames are black and silent
because camera B ends six frames before the Group. Child stereo
pairs and surround remain separate cases.
[Reusable AutoSync](SPLIT-EDIT-AUDIO-2026-11-06.md#reusable-autosync-source-sequences)

#### Native marker on a three-mono Group edit

Media Composer 25.12.0 build 58863 added a red V1 point on the middle
Group edit at `01:00:01:06`. Its saved selector owns the marker six
frames into the edit. Graph/2 resolves that owner to the exact picture
placement and record frame 30 at `24000/1001`. The linked Edit Protocol
AAF saves a red point with the original name and comment at that same
frame.

After Save As, close, and reopen in Premiere 27 build 88, both projects
show the point at frame 30 on the sequence. Premiere's AAF import uses
the generic name `Marker 1` and the default green. Any NLE keeps the
original name, red color and comment, adds the original V1 track to the
comment, and reports the change in ownership. The direct CLI and saved job
agree with the reopened project on the marker, all 12 edit placements and
the eight original links. The three picture angles remain editable and only
the middle A3 edit links directly to the alternate camera.

All 72 decoded frames of the marked conversion match the retained
unmarked Any NLE and linked AAF renders. Each isolated mono lane has
144,144 sample frames and matches the unmarked conversion exactly under
the same center pan. Each lane is within one PCM24 count of the retained
AAF render. Source Group marks and markers have their own source receipts.
