# Avid to Premiere qualification evidence

These dated controls record fixture scope, measured results, and the limits
observed at the time. Current work and holds live in the
[Avid to Premiere roadmap](ROADMAP.md#avid--premiere).

## Native qualification and combined fixtures

Avid-authored DNxHD LB at 23.976 with PCM16 and 29.97 DF with PCM24 has
passed in retained Separate files, Grouped, and Merged controls. The
independent A1 Group switch, simple trimmed Submaster, and reusable
A/V subclips also have reopened controls. Keep those as regressions, alongside the mixed UME
edits and the editable dissolves. The detailed results below retain the
native evidence and the AAF benchmarks already measured.

| Work remaining | What is ready | What closes it |
| --- | --- | --- |
| Source range markers from native edits | The reopened Premiere 27 linked AAF retained the copied point as a sequence marker and carried no ranges. Any NLE retained both 22-frame ranges and both points on the picture edits, with their names, comments, and colors. Both projects kept the sequence start and all six edit ranges. | **Passes for the retained source copy.** Direct Source Monitor authoring remains a separate evidence case. [Native source copy and linked AAF results](SPLIT-EDIT-AUDIO-2026-11-06.md#native-source-range-and-point-copy) |
| Source identity and audio for same-named originals | Premiere 27 reopened the linked AAF with the correct picture and both clocks, but dropped all six audio edits. The native Avid bin now converts in Separate files mode for the verified 24 fps H.264/AAC MOV profile. After Save As and reopen, all 36 pictures match the AAF and both 72,000-sample PCM24 channels match Avid Direct Out exactly. The two original bin items stay stereo. | Already placed mono edits use the verified one-frame Avid route offset. Fresh insertion uses the MOV clock and does not reproduce that offset. Edits beyond playable audio and unresolved offsets are refused. Other AAC profiles and clip constructions are not qualified by this test. [Linked A/V control](VERIFIED-RELINK.md#native-same-name-av-control) |
| Avid volume envelope fidelity beyond linked AAF | At 24 fps, the shifted-key Any NLE and linked AAF renders agree within one PCM24 count after reopen. At 23.976, Premiere 27 retained four of five AAF points and all five Any NLE points at their exact sample times. Against Avid Direct Out, the Any NLE envelope measured 0.432 dB RMS error over the edit, and the linked AAF measured 4.156 dB. | The editable comparison passes on the tested 23.976 mono edit. Exact Avid render matching remains beyond AAF. A measured slope change near sequence sample 32,768 does not explain the short ramp at 65,536. Keep the interpolation report until the short-ramp behavior is understood. [PVOL measurements](SPLIT-EDIT-AUDIO-2026-11-06.md#mirrored-pvol-volume-ramps) |
| Wider Group and nest structures | The independent A1 switch and simple trimmed nest pass their targeted comparisons. Separate files writes a scoped 23.976 three-mono Group. The native Avid control preserves all three mono routes, including the independent A3 switch. A fresh Premiere 27 Source Monitor edit defaults to camera B and retains V1 and all three independent mono lanes after Save As and reopen. The direct CLI and saved job write the same two editable Submasters from a derived AVB with staggered picture layers. They report the third occurrence separately because its source trim exceeds the file. Eight-mic AutoSync has its own independent-lane qualification. | **Deferred: layered Submasters.** When resumed, compare a native Avid Collapse with the staged flat and nested renders. Test child audio and inner effects separately. A linked AAF can benchmark what remains editable. [Three-mono Group test](EDITABLE-GROUPS-2026-11-08.md#three-independent-mono-group-outputs) · [Layered Submaster evidence](SOURCE-GRAPH-MIGRATION.md#layered-submaster-inputs) |
| Final packaged replay | The updated four-bin macOS package passed Review and Premiere 27 reopen with the same-name AAC case, mixed managed/UME, marked dissolves, and trimmed Submasters. The AAC and mixed managed/UME renders match their picture and PCM references. [Package replay](VERIFIED-RELINK.md#combined-macos-package-replay) | Repeat the affected controls after conversion changes. The Sound Roll, moved picture reels, upper-track Submaster, and independent Group audio combination also passed wizard conversion and Premiere 27 reopen. [Group audio and fresh insertion](EDITABLE-GROUPS-2026-11-08.md#audio-layouts-and-groups) |

The offline exact 24 fps layered Submaster control now writes the
two child tracks and matches the flat source selection at every
frame. Selecting a second bin with a copy of the same master now
preserves the nest and uses the copy in the sequence's bin. A range
marker on the outer Submaster also survives on the parent sequence,
even when it ends in the following cut or in another qualified nest.
Premiere 27 saved and reopened the derived 23.976 marker spanning two
nests. Its full range, edit ranges and media bindings survived. All 57
rendered picture frames match the unmarked companion. The flat-render
comparison and individual reopen checks for the other controls remain
pending. Linked AAF coverage is unmeasured.
[Layered Submaster evidence](SOURCE-GRAPH-MIGRATION.md#layered-submaster-inputs)

The native three-mono Group uses B/A/B picture, fixed camera B on A1
and A2, and B/A/B on A3. Only the middle A3 edit switches audio.
The linked AAF retains all twelve source selections and trims and
imports as picture cuts and independent mono clips in Premiere 27.
After Save As and reopen, the initial Any NLE project matched all 72
decoded frames and retained editable picture angles. The middle A3 edit
played from its linked MXF, and the two outer A3 segments from the
multicam source rendered silent. An isolated Premiere control restored
both segments by declaring eight adaptive channels on the inner Group
bus. The fixed converter project passed Save As and reopen. All 72
picture frames match the AAF, and each isolated 144,144-sample PCM24
render agrees within one count per channel with the same center pan.
Premiere's native Source Monitor edit defaults to camera B on a fresh
multicam placement and reproduces all three default camera B audio
lanes after Save As and reopen. Camera B's 96 frames and 192,192 audio
sample frames per lane are followed by six frames of black and silence
at the end of the full Group span.
[Three-mono Group test](EDITABLE-GROUPS-2026-11-08.md#three-independent-mono-group-outputs)

DNxHR LB with PCM16 and wider 29.97 managed profiles remain media
qualification tasks. Add a representative Avid specimen when a codec,
clock, or channel layout changes the conversion path. The existing DNxHD
checks already cover all three linked clip constructions, so another
round across equivalent variants adds little evidence.

An archived editorial cut exposed Resolve-authored Avid OP-Atom DNxHR LB
with the `0A` picture coding label and Avid picture essence element key.
The source reader recognizes the exact label and rewrap recognizes the
observed element. Package identity and clock checks still apply. The
rebuilt macOS package's saved-job replay opened a derived 669-frame excerpt
in Premiere 27. After Save As and reopen, all three picture ranges and ten
mono audio ranges matched. Both 1,339,338-sample lanes matched the selected
source PCM, including silence through the shared gap. A repeat export matched
all decoded picture and PCM. The excerpt keeps the original V1 and A1 cuts
and adds the corresponding second source channel as an independent mono lane.
It qualifies OP1a rewrap for the tested media profile.

The three 24 → 23.976 audio-only Motion Adapters now retain their exact sample
origins from managed mono PCM16/48 kHz. The isolated Premiere 27 replay kept
both editable six-frame Constant Gain dissolves, −12 dB gain, and center pan.
After Save As and reopen, the 4,034,030 sample frames matched the archived
Avid SFX stem within three PCM24 counts per channel. The one-mono editable
Group replay used two DNxHR LB cameras, each rewrapped with eight PCM24 lanes.
The existing two picture edits and the single selected audio lane survived
Save As and reopen. All 120 picture frames matched the selected sources and
the saved A1 audio matched its original samples exactly under the tested
hard-left policy. With native Source Monitor insertion the saved default
camera is first in Premiere's angle order and the Group exposes one mono
audio lane. The diagnostic reports the angle order change.

The original 2,015-frame cut now converts in Separate files mode with the
original MXFs linked in place. A7 keeps its 12-frame stereo Constant Gain fade
into filler and its saved music level. Premiere 27 Save As and reopen kept the
fade editable. The isolated lane from the reopened full cut matches the
archived music stem within eight PCM24 counts per channel across the fade.
The cut also retains three editable one-mono Groups, all nine V1 Group edits
under 3D Warp, and the 11 independently selected A1 audio edits. This covers
static Motion on a same-rate, full-span Group input with an empty Warp
background.

The 14 V2 edits in the archived cut use SpectraMatte inside a static
3D Warp. The source graph retains the RGB key and effect order. For the
verified 1080p23.976 green-screen profile, Premiere writes editable Ultra
Key before Motion and keeps the foreground linked to its original MXF.
All 14 edits survived Save As and reopen. The converter reports the
matte approximation on each occurrence and omits unsupported overlays
so their opaque foregrounds do not cover the lower track. The V1
insert's remaining 3D projection, color, and caption appearance losses
remain open.

## Multi-reel feature cut audit

An offline scan covered ten current-cut bins and their saved sequence versions.
All ten original bins now pass source graph export with their saved sequence
versions, including the four that previously exceeded the source budget. The
largest graph is 69 MiB. A separate linked, two-hour control writes 64,800
picture and audio edits on nine independent lanes through both the direct
CLI and a saved job. Premiere writer inspection covers both outputs.
See [large project limits](SOURCE-GRAPH-MIGRATION.md#limits-and-memory).

- Online 2048×858 AVC MOV at exact 24 fps can link in place when the
  stereo PCM24/48 kHz file matches the saved Avid descriptors and
  channel selectors. Separate files retains the stereo bin source and
  two independent mono timeline edits. In a synthetic Premiere 27
  Save As and reopen, all 36 trimmed frames matched a Premiere direct
  import and both 72,000-sample mono lanes matched the source under the
  Avid pan policy. Entire Project Review still stops at the required
  offline MOV after reading all 1,903 bins.
- A full-bin source audit identified a managed bin clip whose saved picture
  rate is outside the supported exact clock family. Its master and picture
  rates agree. Linked conversion can omit that
  occurrence when dependency checks prove it unused, even if the media
  is available. A cross-bin editorial reference and a contradictory
  master rate each still block Review. Separate checks remain for the
  saved mixed-rate components, source routes without captured tracks, and
  whole-project construction and writer capacity.
- Review recognizes the nine saved drive-like MXF locators as one picture
  and eight mono packages. It can recover matching files by package ID in
  selected Avid media roots and retains the saved locators and selectors.
  A verified-unused omission still requires a complete dependency
  proof. The archive contains a 15-frame audio reference inside an effect
  stack, so this bundle still needs its media. Native playback is pending.
- Whole-project diagnostic Review read all 1,903 bins and passed the
  installed ColorBars TIFF and the 1920×1080 PNG slate with temporary MOV,
  MP3, and PNG controls. The next stop was an offline 3840×2160 MP4 at
  23.976 fps with two saved PCM16/48 kHz channel selections. The bin
  contains 21 saved MP4 paths with the same descriptor families and
  clocks. Linked Separate files now qualifies progressive square-pixel
  3840×2160 AVC High Level 5.1 MP4 at exact 24000/1001 with stereo
  48 kHz PCM16 or PCM24. Both synthetic Avid-linked clips retained
  picture and two mono timeline lanes after Premiere 27 Save As and
  reopen. All 36 trimmed picture frames and all 72,072 samples per
  lane matched their references for each depth. The archived originals
  still need onlining to confirm which meet these checks.
- Online 23.976 UHD AVC High Level 5.1 MP4 with stereo AAC-LC at 48 kHz
  now links in Separate files mode. The saved Avid file-track start
  advances existing audio edits by one source frame, independently of
  the MP4's skipped 1,024-sample priming packet. Premiere 27 Save As
  and reopen preserved all picture frames, both mono trims, and audio
  through the 96,096-sample playable endpoint. An edit through the last
  picture frame keeps its picture and reports the remaining silent
  span on each audio lane. Avid Direct Out decoded 160 more samples
  from the final AAC packet in this fixture.
  Native Premiere 27 trims in two disposable projects also produced
  different A2 waveforms over the same 512-sample source interval,
  with unchanged picture, A1, trim, and playable end. Both observed
  converter waveforms matched native trim or full-clip output exactly.
- **Best-effort conversion** published the full 1,903-bin archive as a
  Premiere project with 1,217 saved sequences. The destination omitted
  159 unqualified offline managed masters and marked 92 additional
  exact gaps. Fifteen qualified offline locators remained in the relink
  report. Structural read-back verified the output and the occurrence
  report. Premiere playback of the archived project remains untested.
  [Fallback scope](SOURCE-GRAPH-MIGRATION.md#continue-with-omissions)
- **Bound working memory for whole-project conversion.** Measure each
      stage separately. Use a compact project-wide identity index and
      spill completed bin and timeline records and diagnostics to disk.
      Preserve cross-bin reference, clock, and lane validation. Measure
      destination preparation and writing independently on the same
      full project with a peak-memory target. Linked Entire Project
      jobs now stage native timeline receipts and high-volume omissions
      under a separate working graph budget. The recorded dissolve
      can be omitted with an occurrence-level loss when neither
      endpoint survives and the lane is empty over its entire range.
      An earlier full offline replay peaked at approximately 4.02 GiB
      resident memory and published about 2.4 GB of staged diagnostics.
      A repeated offline baseline peaked at 4.05 to 4.16 GiB RSS.
      The instrumented run reached 3.63 GiB during source capture,
      4.05 GiB during graph construction, and 4.16 GiB while
      assembling Review after encoding.
      Sampled RSS peaked at 2.64 GiB during typed destination
      preparation, 3.14 GiB during document construction, and
      2.90 GiB during XML and gzip encoding.
      Trials shortened the lifetime of the typed input and of consumed
      source graph fields, and eliminated a full XML buffer copy.
      Whole-process RSS failed the acceptance threshold for all three.
      Releasing consumed graph fields moved the measured peak from
      Review to working graph construction. The Avid reader now
      allocates optional picture extensions, matte inputs, and color
      adapter controls only when present. Source events fell from
      1,080 to 800 bytes and source issues from 600 to 280 bytes on
      arm64. This lowered the accounted source peak by 290 MiB.
      Two compact whole-project replays peaked at 4.26 and 4.33 GiB
      RSS, within the 4.23 to 4.75 GiB baseline span.
      Measure the retained source routes before designing a disk-backed
      source index.
      [Working graph and remaining allocations](SOURCE-GRAPH-MIGRATION.md#limits-and-memory)
- **Batch-qualify Avid camera links across container and raster.**
      Save one Avid bin of numbered clips with distinct mono signals.
      Use H.264/AAC and H.264/PCM MP4 and MOV, ProRes/PCM MOV, and
      qualified DNxHR/PCM MXF as controls. Cover SD, HD, UHD, and one
      nonstandard raster at representative exact and fractional rates;
      test PCM16, PCM24, stereo splits, and independent MXF siblings.
      Avid linked all 30 synthetic masters. Premiere 27 direct import
      kept all 30 online after Save As and reopen. Any NLE retained eight
      masters, omitted five and refused 17 in isolated replays. The
      translated 1080p AVC/AAC MOV, 2048×858 AVC/PCM24 MOV and six
      mono or stereo PCM16/24 WAVs at 44.1/48 kHz stayed linked after
      reopen. Fresh insertion rendered all numbered frames and the
      expected channel signals. Avid-authored trims on this matrix and
      the remaining container and channel profiles are pending.
      Verify media raster and pixel aspect independently of the sequence
      canvas. Compare nonzero trims, picture, and isolated audio after
      Premiere Save As and reopen. Generalize raster admission where the
      codec, media clock, audio mapping, and destination geometry agree.
      The follow-on 25 and exact 30000/1001 matrix qualifies 30 more
      Avid-linked sources in Separate files mode. It covers 720p, HD,
      2048×858, and UHD AVC MP4/MOV with AAC-LC or PCM16/24, HD Main 10
      HEVC/AAC MP4, HD ProRes HQ MOV with or without PCM24, and HD DNxHR
      LB OP1a MXF with PCM16/24. The converter checks both saved MXF
      package IDs. When a file stores two mono streams, graph/2 keeps
      both physical stream selectors and Premiere uses one media object
      per stream. All 1,080 trimmed picture frames matched direct
      Premiere 27 imports after reopen. Both mono lanes retained the
      measured trim and channel identity. 25 fps PCM was sample-exact.
      The 29.97 PCM exports ended with one silent rounding sample.
      The ProRes drop-frame control preserved its saved source start
      and crossed from 01:00:59;20 to 01:01:00;02 ten frames later.
      Other AAC layouts and fractional-frame AAC edits beyond the
      playable extent remain separate cases.
      Stereo PCM16/24 at 48 kHz in progressive, square-pixel H.264 High
      level 4.0 or 5.1 MOV/MP4 now uses the same saved-raster, frame-rate
      and channel checks at 24, 23.976, 25 and 29.97. The PCM extent is
      rounded once from the complete source duration. For a sequence
      without a named preset, the writer can use an explicit raster
      when every picture edit verifies the same raster and rate in
      this linked profile. Derived AVBs with 1600×900, 1440×1080 and
      1536×864 sources kept their settings, trims, links and channel
      selectors after Save As and reopen in Premiere 27. All 12 sampled
      pictures selected the expected source frames. The 29.97 test file
      retained 76,876 PCM sample frames for 48 picture frames. Full
      exports of the new rasters remain pending.
- **Decouple linked AVC picture geometry from audio layout.**
      The explicit raster and matching-raster picture effects use the
      verified picture stream. Audio keeps its own source and writer
      checks. Picture-only, progressive square-pixel H.264 High MOV/MP4
      at 24, 23.976, 25 and 29.97 now links at the verified source raster.
      The new source profile checks the saved picture descriptor, zero
      origin and complete fixed-rate packet clock. CLI and saved jobs
      produced matching cuts and trims on 12 derived timelines.
      After Premiere 27 Save As and reopen, 20 sampled frames matched
      their PCM-bearing controls pixel for pixel, including Corner Pin,
      source crop and both Pin/Flop orders. Unit checks also cover
      applying the same geometry with AAC already admitted.
- **Use the matched-raster path for linked ProRes 422 HQ MOV.**
      The Avid reader verifies progressive square-pixel picture at 24,
      23.976, 25 and 29.97, the complete packet clock and the saved
      descriptor. It checks the embedded timecode track when present
      and verifies any stereo PCM16 or PCM24 bindings independently.
      Eleven derived timelines at 1600×900, 1440×1080 and 1536×864
      agreed across direct CLI, saved jobs and Premiere 27 reopen.
      All 109 plain-cut picture frames matched Premiere direct imports.
      Six Corner Pin and Flop controls matched the projected chart
      points within 0.27 pixels. Both 23.976 PCM16 lanes matched exactly.
      The 29.97 drop-frame PCM24 case preserved the source timecode and
      reproduced the retained 1080p audio, including its one silent
      rounding sample.
- **Stream the linked ProRes HQ picture clock verification.**
      Verify every packet through EOF with one bounded row at a time.
      Exact packet timing and the declared count still have to agree.
      The verifier handles reports over 1 MiB and sources over 16,384
      frames. The 50,000-frame controls include trims beyond the former frame
      limit and a late two-packet discrepancy that preserves the
      total duration and declared rate.
- **Stream the picture-only H.264 clock verification.**
      Read one packet row at a time and track the presentation slots
      in a bitmap of at most 128 KiB. This allows decode-order B-frames
      and detects missing or repeated slots through EOF. The
      1,048,576-frame count bound still applies. The
      81,600-frame controls exercise a report over 1 MiB and a late
      duplicate presentation timestamp with the same declared rate
      and duration.
- **Keep linked picture packet scans running while they make progress.**
      The ProRes HQ and picture-only H.264 verifiers refresh a 30-second
      idle deadline after validating packet rows. The cancellable source
      reader checks for cancellation during those scans and reaps its
      FFprobe child on a stall or cancellation. The converter still
      requires the complete packet proof and successful process exit.
      Short metadata probes retain their total deadline and output bound.
- A rejected Gain, automatic Motion Adapter, or generated Freeze Frame
  can become a marked gap when the reader proves its exact record range
  and the lane has no conflicting placement. When a Gain gap borders a
  saved audio dissolve, Premiere can omit that dissolve at its saved cut.
  The saved adjacency must resolve to an ordinary linked trim, known
  filler, or a second Gain gap. The latest-cut audit replaced seven
  additional timing blockers with markers and retained nine related
  dissolve records. A linked mono control preserved both trims and the
  exact silent span after Premiere 27 Save As and reopen. The reel
  neighbors still need online playback verification. Whole-project output
  and the unused-media blocker remain open.

- The saved audio track-insert container now keeps its underlying edits and
  transitions when the first slot is the full-span program and the other slots
  contain only verified plug-in filler. Review reports each unmapped insert.
  The offline graph comparison recovered 2,139 audio edit facts and 675 native
  transition facts across the ten latest cuts.
- Standalone legacy Titler+ retains plain text and cue timing when both saved
  text carriers agree and both picture inputs are empty. Premiere uses the
  existing editable graphic path and reports appearance loss. Conflicting or
  missing text and unsupported owner timing remain explicit.
- Audio Pan Volume inside a subclip accounted for 3,345 unresolved branches.
  The same-rate, single-source profile with unset unity gain and saved
  center pan now recovers 3,184 audio edits across the ten cuts. Graph/2
  retains the inner settings alongside the existing timeline gain and pan.
  Following additional subclip hops with the same controls recovers 60
  more audio edit routes, 53 through two saved wrappers and seven through
  three. Each additional hop must cover the entire requested range in
  one qualified audio component. Four native dissolve endpoints and one
  Group use now bind to the recovered source events.
  A single unlabeled wrapper input with a saved zero lock recovers another
  56 routes. The source clip's track label selects the master lane. Any
  saved AAF track number on the wrapper must agree with that label.
  Another 45 edits carry explicit subclip gain. Graph/2 retains the
  saved level, pan state, exact trims, and selected master lane.
  In the derived 24 fps mono control, Avid Direct Out and the stereo mix
  both bypass the subclip gain and apply the independent timeline gain.
  For verified same-rate mono PCM24, the converter now restores the edit
  and reports that it used the original sound. Premiere 27 Save As and
  reopen kept all edits, including a higher-numbered managed channel.
  The original reel media stayed offline. Other pan settings, reusable
  audio-only subclips and broader subclip mixes remain open.
  Broader subclip mixes and timing, generalized AudioSuite render linking,
  and upper-track composites remain open.
- Unrendered legacy Avid clip EQ with a full-span 24 fps mono input
  now retains the linked edit when the original managed 48 kHz PCM
  verifies. Graph/2 keeps the saved EQ enable and raw band controls.
  Review identifies each enabled EQ that was bypassed. Premiere 27
  Save As and reopen preserved both the source trim and independent
  timeline gain and pan. The PCM24 render matched the dry Premiere
  control sample for sample. The linked route can also use the saved
  render for a full-span same-rate mono EQ with matching source and
  effect facts. Premiere 27 reopened and rendered the tested PCM24
  cache, including a separate timeline gain and pan. A same-rate mono
  subclip now qualifies when both saved routes resolve to the same dry
  file, channel and range. An online subclip control with a nonzero
  trim likewise matched the native cached audio in Premiere 27.
  When that cache package is missing, direct linking keeps the dry edit
  if the saved master lane, package and PCM range verify. The loss report
  names the missing cache and bypassed EQ at the affected edit.
  A reopened Premiere 27 control matched all three seconds of dry PCM
  on the selected channel.
  Trimmed edits can use a matching portion of a longer verified cache.
  Graph/2 retains the full cache length and the saved in-point. The
  offline audit verified six such source bindings, including two with
  nonzero in-points. One saved usage-7 mono PCM24 master also qualifies
  when its neutral Pan Volume input matches the precompute's copy.
  A matching static-gain wrapper on a mono PCM24 master can also use
  the cache. Avid's test export confirmed that the EQ playback bypasses
  the inner gain. Independent timeline gain and pan remain editable.
  The final saved layered occurrence nests Pitch II inside EQ. A derived
  24 fps mono control confirmed that Avid plays the outer 96,000-sample
  EQ cache. Direct linking uses that verified PCM24 cache and preserves
  both saved bindings in graph/2. Premiere 27 Save As and reopen matched
  all 144,000 samples of the three-second Avid control, including the
  two dry neighbors. A missing outer cache leaves a reported gap.
  Playback of the archived occurrence remains pending while its media
  is offline. Other nested plug-in profiles, prepared-media rewrap and
  editable EQ remain open.
  [Legacy EQ fallback](SOURCE-GRAPH-MIGRATION.md#avid-gain-and-timewarp-fallbacks).
- A standalone mono AudioSuite effect can now link its saved PCM24
  render on a 24 fps timeline. The saved copy must match the plug-in
  settings and the full source edit. Avid D-Verb and Pitch II controls
  each played 96,000 samples from a cache with two trailing zero
  samples. After Premiere 27 Save As and reopen, both three-second
  exports matched Avid Direct Out exactly. The offline audit identified
  14 structurally matching render references, including 13 FabFilter
  Pro-R 2 edits and one Timeless 3 edit. Their MXFs remain offline.
  A shorter edit can also link an exact window into the saved render.
  The head-and-tail and tail-only D-Verb controls matched Avid Direct
  Out after Premiere 27 reopen. Three more archived edits have matching
  saved bindings. Missing or invalid caches leave a reported gap.
  One same-rate subclip hop now qualifies when its selected lane and
  exact offset agree with the saved cache input. The cached copy can
  contain two neutral Pan Volume wrappers. Full-span and trimmed
  D-Verb subclip controls matched Avid Direct Out after Premiere 27
  reopen. Twenty-one more archived mono edits have matching bindings.
  A single neutral Pan Volume wrapper on the cached copy also
  qualifies for both direct masters and subclips. The two D-Verb
  controls matched Avid after Premiere 27 reopen. Nine more archived
  edits have matching saved bindings.
  A neutral Pan Volume wrapper can also contain a single sound segment
  in a sequence with matching saved AAF routing. Both direct-master
  D-Verb controls matched Avid Direct Out after Premiere 27 reopen.
  The ten archived reels retained their existing placements and
  accepted bindings on replay.
  Nested plug-ins and stereo processing require separate qualification.
  [AudioSuite cache scope](SOURCE-GRAPH-MIGRATION.md#standalone-audiosuite-renders).
- Audio-only automatic 23.976 → 24 Motion Adapters on managed mono PCM24
  retain the saved source range and use the absolute 48 kHz sample position
  even when the saved phase wraps. The derived online control preserved
  three edits across two mono lanes after Premiere 27 Save As and reopen.
  The waveform matched the verified source starts with zero sample shift.
  The saved Pan Volume key stays editable, with an explicit notice that
  Avid envelope playback is unqualified. The reel originals remain offline.
- A saved 24 fps stereo master can link two verified PCM16 MXF siblings
  when both lanes end with a separate one-frame MPEG audio piece. The
  source graph records the cutoff and the saved MPEG package. The
  converter refuses edits or transition ranges that cross the cutoff.
  One retained reel has 13 stereo edits fully inside the PCM section.
  A derived online control in Premiere 27 preserved three trimmed edits
  and the full reusable PCM range after Save As and reopen. Both output
  channels matched the source samples exactly. The original reel media
  remains offline.
- Plain nested 50%, 150%, and 200% picture cuts transfer as editable
  speed on the qualified 1080p24 managed DNxHD LB profile in Separate
  files mode. Premiere 27 retained the odd and even trims after Save As
  and reopen, including half-frame source In values. All 120 rendered
  frames selected the same source frames as Avid. The same six trims under
  a neutral video-to-full-range Color Adapter also retained editable speed,
  the exact trims, and an editable ASC CDL. The reopened renders select
  the same 120 source frames as Avid. The 50%, 150%, and 200% cases have
  the same 95th-percentile level differences from the Avid reference.
  Linked picture now uses a general positive constant-speed rule when
  the saved Speed and Position agree and Premiere can represent the trim
  exactly. Premiere 27 Save As and reopen verified 25%, 80%, 125%, and
  250%, plain and with the same Color Adapter, across 320 frames.
  Plain nested edits now share this rule across verified same-rate
  managed and original links. Reopen testing covers two speeds and two
  trims each on 23.976 DNxHD LB MXF and 25 fps ProRes HQ MOV. The 23.976
  render agrees with Avid, and the 25 fps render agrees with the saved
  cadence and the original picture.
  [Trimmed motion control](PRORES-DURATION-2026-11-07.md#retimed-subclip-cadence).
- Color Adapter source binding now follows one direct 24 fps picture
  subclip to the master and file, checking the entire trimmed range at
  each hop. The offline reels gained 3,140 matching file receipts. A
  derived linked control writes the same two editable external LUT
  placements through direct CLI and saved-job conversion. Conflicting
  transforms and a shortened subclip leave the occurrence losses visible.
  A same-rate subclip sequence with trailing filler contributes 40 more
  matching receipts when the selected range ends inside the picture.
  A range that crosses into filler leaves its warning. The remaining
  82 retimed uses now retain the input range, saved Speed and Position
  controls, span ratio, and phase and render settings in graph/2. Those
  edits still require verified picture-file bindings before placement.
  In a 24 fps native Avid control, the same 20-frame trim at 200%
  selected source frames 56 through 94 in steps of two, both by itself
  and under a range Color Adapter. Graph/2 now records those selected
  master frames for the matching direct-master control family. Separate
  files now converts the verified 1080p24 managed DNxHD LB and PCM16
  profile with an editable 200% retime. An editable ASC CDL can reproduce
  the range expansion when the original picture has verified Rec.709
  video levels. Native rewrap now also covers the verified picture-only
  1080p24 ProRes LT profile. Premiere 27 saved and reopened the editable
  retime and range correction. One additional zero-offset picture
  subclip hop now preserves the same sampled frames after reopening.
  The offline audit recovered 13 file routes with that hop. Two match
  the saved wrapper and position checks, including one within the
  current file verification bound. A verified gray clip-color triplet
  on the retimed Color Adapter lets ten more saved caches qualify for
  media verification, including eight with the extra subclip hop.
  The 20-frame Premiere reopen matches the prior render exactly.
  A project-local, unit-domain 33×33×33 external LUT also survives
  rewrap on this retime as an editable Lumetri Look. Avid and Premiere
  each rendered explicit and implicit unit-domain controls identically.
  A 10,021-frame Avid-native synthetic MXF now verifies across two
  consecutive ProRes LT index segments. Premiere reopens the 200% edit
  crossing that boundary with its editable LUT and exact source trim.
  Bounded streaming packet verification raises the source limit to 32,768
  frames. A synthetic 18,704-frame Avid import exercises all three joins
  across four index segments.
  The two offline reel uses have a compatible LUT table and the same
  declared source length. Their actual MXF still needs verification.
  Nonzero inner offsets, nonzero phase, and the real reel media still
  require their own checks.
  [Trimmed motion control](PRORES-DURATION-2026-11-07.md#retimed-subclip-cadence).
- Across the latest cuts, 3,114 standalone video-to-full-range
  Color Adapters have a matching picture-source receipt. Graph/2
  retains the exact 10-bit endpoints and direction on both the source
  file and the sequence occurrence where present. In the online Rec.709
  ProRes HQ chart, Avid expanded neutral patches from codes 64–940 to
  roughly 4–1019 in a tagged legal-range export. Premiere 27's direct
  link kept the original levels. The qualified Separate files path puts
  an editable ASC CDL on a plain 1080p24 edit of the verified picture-only
  ProRes HQ original. The source and edit payload must match, and the
  file must have explicit Rec.709 video levels. Plain edits of the same
  original stay neutral. A derived online linked control retains the
  adjustment on V1 and V2 while preserving a separate centered dissolve.
  The direct CLI and saved job were checked by structural read-back.
  In the single-lane control after Save As and reopen, the 24 expanded
  frames match the editable native Premiere control exactly. The
  sampled chart values are within two code values of Avid's 10-bit
  processing export, with identical black and white endpoints.
  ProcAmp and a Lumetri Input LUT missed the
  Avid render. Conversion keeps the cut and reports an unapplied
  adjustment for other profiles. The 1080p24 picture-only managed ProRes
  422 LT control now also qualifies through **Rewrap to native clips**.
  Avid saved the effect on the edit and retained a picture-position
  reference into the MXF. The converter checks that reference against the
  file package and indexed bytes before adding ASC CDL. After Premiere 27
  Save As and reopen, all 24 neutral frames matched the neutral Premiere
  render, and the 24 adjusted frames matched Avid's sampled 10-bit levels
  within two codes. Both edits retained source frames 6–29. The offline
  production reels still need their media to verify each occurrence's
  binding. Wider clocks, effects and track layouts remain open.
  Empty and populated marker lists on the managed adapter's picture
  input now travel with the effect. A point and a seven-frame range
  marker stayed on the correct edit after Premiere 27 reopen. One
  same-rate picture subclip hop now qualifies when its combined trim
  agrees with the backing MXF index. The tested native subclip
  preserved the same picture, editable effect and markers as the
  direct-master control. An exactly matching range transform saved
  on the adapter input now reuses the same edit effect, with one
  editable ASC CDL verified after Premiere 27 reopen. A conflicting
  source-clip transform reports a loss on the affected edit. The
  offline reels now yield 2,687 saved managed picture position receipts.
  Managed LT adjustments now qualify on verified edits on multiple tracks
  with a dissolve elsewhere on the timeline. Premiere 27 reopen preserved
  the editable corrections on V1 and V2 and the independent V1 dissolve.
  Verify real reel media, full Avid layer compositing, and effect order
  on transition endpoints separately.
  [Layered control](PRORES-DURATION-2026-11-07.md#layered-tracks-and-a-separate-dissolve).
  Managed picture verification now scales by indexed frame count and
  distinct inputs. A synthetic 209-package reel passes the former
  32-picture boundary while retaining per-edit range checks.
  [Reel scale control](PRORES-DURATION-2026-11-07.md#managed-color-adapter-verification-at-reel-scale).
  [Matching input metadata](PRORES-DURATION-2026-11-07.md#matching-color-transform-metadata-on-the-input-clip).
  [Marked control](PRORES-DURATION-2026-11-07.md#marked-color-adapter-inputs).
  [Avid levels reference](https://resources.avid.com/SupportFiles/attach/Media_Composer/Media_Composer_v2025.x_Editing_Guide.pdf)
  · [Adobe effect controls](https://helpx.adobe.com/premiere/desktop/add-video-effects/types-of-effects/effects.html)
  · [SMPTE ProRes coding labels](https://pub.smpte.org/pub/rdd44/rdd44-2022.pdf)

The native source-copy comparison used the retained range payload on
a disposable master, then two new Avid edits with **Copy Source Markers**
enabled. Avid copied the point control as edit-owned and kept the span
on the master in both cases. Use direct native source authoring as a
separate check if there is evidence of a different saved range structure.
[Marker test](SPLIT-EDIT-AUDIO-2026-11-06.md#native-source-range-and-point-copy)

Native named-track AAF export is optional comparison evidence for the
name mapping already verified in Premiere 27.

The offline four-bin regression combines the exact-24 Submaster, the
matching and independent source spans, the independent Group audio
defaults, and the linked-original marker conflict. Both entry points
match the retained Premiere saves across nine sequences and 11 file
links, including sequence and nest clocks, edit trims, channels, and marker
ranges. Two seeded CLI outputs and graph snapshots are byte-identical.
Premiere reopen of this combined regression is pending. The individual
reopened results remain recorded under each test.

A fresh local Apple Silicon package also replayed seven bins, adding
the two layered Submaster rates and the duplicate-master control. Its
CLI and saved job matched the retained sequence structure, source
links, markers, and Group selections across 16 sequences and 14
linked files. The package separately matched the two reopened
picture dissolve controls and refused the overrun on both entry
points. After Premiere 27 Save As and reopen, all 16 sequence
structures and 14 original links matched the packaged output, with
zero offline items. The independent A1 Group sequence matched the
linked AAF on all 120 picture frames. This replay checked audio
assignments in the saved project.
[Independent A1 switch](EDITABLE-GROUPS-2026-11-08.md#native-independent-a1-switch)

## Source markers, LUTs, and Matte Key evidence

These controls record the Avid → Premiere comparisons through the verified
profiles. Current follow-ups are in the [route roadmap](ROADMAP.md#edit-fidelity-and-metadata).

### Source and edit marker range controls

The native 23.976 point-marker comparison verifies both source/edit
preferences. Linked AAF holds all three events. Premiere 27 reopens
two sequence markers with the edit comments and drops the source
event. Any NLE retains all three names and colors, the source bin
marker and the selected edit/sequence ownership. The retained native
24 fps sequence span survived Premiere 27 reopen in a marker-only
project. Its linked AAF, exported with media offline, omitted the
span. In the paired 24 fps source-copy control, the reopened AAF
carried the copied point onto the sequence. Any NLE kept both source
ranges and both point occurrences on the edits. Direct native
authoring of coincident source/edit ranges remains open.
[Native point-marker comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#native-coincident-point-markers-and-linked-aaf)
[Sequence span comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#retained-native-sequence-span-and-linked-aaf)
[Source copy comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#native-source-range-and-point-copy)

When a bound edit marker spans multiple prepared edits on its saved
track, Review names that track and reports that the full range carries
over as one sequence marker. This uses the saved binding and prepared
record ranges, with the existing ownership report for unresolved cases.

A derived AVB now preserves both coincident ranges after Premiere 27
reopen when **Keep the other marker as a sequence marker** is enabled.
[Marker ownership check](SPLIT-EDIT-AUDIO-2026-11-06.md#coincident-source-and-edit-markers)
Grouped now retains the winning V1 range for the verified direct
DNxHD profile. The 29.97 DF edit-preferred case survived Premiere
27 Save As and reopen. Both preferences passed direct CLI and
saved-job read-back. Merged keeps the winning range on a dedicated
V1 component for the tested same-rate direct DNxHD edit. The 29.97
DF edit-preferred project also survived Save As and reopen with its
other range on the sequence and both original mono siblings linked.
The reusable Merged item displays its source range at
`02:00:00;12` with a two-frame duration. In the verified 29.97 DF
profile, a mono edit with a unique sibling binding now keeps the
preferred marker on its dedicated Merged audio component. Premiere
27 Save As and reopen kept the winning A1 range and the other range
on the sequence. Both preferences passed writing checks. Direct
Media Composer authoring of coincident audio ranges remains open.
Merged also preserves lane-specific clip markers for the verified
23.976 DNxHD LB/PCM16 profile with a nonzero source origin. The
preferred A1 range stays on its own mono component and the other
retains its full range on the sequence. The independent A2 and V1
ranges stay on their own components. Both preferences passed the
direct CLI, saved job, and Premiere 27 Save As and reopen. All 48
picture frames match the retained linked AAF, and all 96,096 stereo
PCM16 sample frames match the unmarked Merged control.
On the reusable Merged item, Premiere 27 shows both two-frame
source ranges at `02:00:00:07` and lists each twice. Premiere
recreates the extra entries on save even with only the component
marks in the input project. The fresh edit keeps one range per
mono lane. Its 24 picture frames match the linked AAF, and each
isolated 48,048-sample lane matches its original under the
center-pan mix policy to within one PCM16 count. (CONV-0132)

### Source Color Adapter and LUT controls

Any NLE retains the saved source color transformation text and source
owner in graph/2. Premiere preparation flags the affected clip for
manual review. For an Avid-authored full-range to video-level transform,
linked AAF preserved the source record and exported a ColorAdapter
operation on the sequence. Premiere 27 reopened with an Unknown Effect
marker at the edit, and all 72 decoded frames matched a direct import.
Any NLE reports the affected sequence track and record position.
Any NLE now applies the exact 10-bit full-range to video-level
transform as an editable ASC CDL on a plain same-rate 1080p24
Rec.709 picture-only ProRes HQ MOV edit in Separate files. The saved
source and edit transforms must agree. After Premiere 27.0.0 build 88
Save As, close, and reopen, the 48 neutral chart frames matched Avid
exactly. On each of the 24 adjusted frames, 25 sampled luma and
chroma patch centers agreed within one 10-bit code value. The plain
occurrence of the same file and trim stayed neutral. Both range
directions now share the graph/2 interpretation for 1080p same-rate
picture-only Rec.709 AVC MOV. Premiere 27 and Resolve Studio 21.2.0
build 00006 reopened the synthetic controls at 23.976, 25, and
29.97 DF with editable clip controls. All 24 adjusted frames in
each of the six Resolve outputs matched the separately authored
native Resolve controls with equivalent Deliver settings. In the
24 fps reference test, Resolve's legal-to-full render agreed with
Avid within two 10-bit code values at sampled points with Video
data levels and Retain sub-black and super-white data enabled.
Resolve Review reports this Deliver setting. Native Resolve saves
now have the same graph/2 range representation as writer-produced
saves. A graph replay of both shifts at all four rates matched
288 decoded frames after reopening. Equal-channel CDL slopes
share the native decoder and passed two new 24 fps controls.
Unqualified stacks and media retain their occurrence diagnostics.
[Native Resolve CDL round trip](PREMIERE-27-COLOR-MODE.md#native-resolve-cdl-round-trip)
[Graph and mapping scope](SOURCE-GRAPH-MIGRATION.md#avid-source-coloradapter-ownership)
(CONV-0144)
The external .cube control preserves its filename in the source and
sequence AAF records. On reopen, Premiere's AAF import renders the
neutral look and reports the unknown ColorAdapter. Both Any NLE
entry points now name the saved cube in the clip's review message and
retain the affected edit report. Avid's neutral and LUT-applied
exports differ on all 87 frames. A generated Premiere source Input
LUT asset survived Save As and reopen with no matching installed
filename. All 87 decoded frames match Premiere's manually assigned
Input LUT. The prototype measures 48.85 dB RGB PSNR against Avid's
LUT-applied render, compared with 16.78 dB for the linked AAF import.
Both saved Avid bins can carry the same picture source LUT
metadata while their sequence renders differ. Only the LUT-applied
sequence has the matching timeline ColorAdapter. Graph/2 now
binds the saved 24 fps ColorAdapter payload to the exact picture
placement when its source package, saved transform and edit
ranges agree. A contradictory payload stays available for review
with the placement unresolved.
Premiere 27 coalesced duplicate same-file source references on
Save As and dropped their distinct LUT state. The initial writer
embedded a clip-level Lumetri Look when the saved adapter was neutral
apart from the LUT, the Avid project descriptor matches, and the
direct managed picture verifies as the tested 1080p24 Rec.709 DNxHD
profile. The current mapping checks the entire affine LUT grid at
edge sizes 2–65 in an explicit unit RGB domain and embeds its
eight corner samples on a plain V1 edit in Separate files.
A derived 33-point grid rendered all 174 frames identically to
the original size-two control after Premiere 27 reopen. The
65-point grid produced the same seeded Premiere project.
The adjacent neutral and processed uses share the original file and
source trim after Save As and reopen. All 87 neutral frames match
the linked AAF. The processed render measures 48.34 dB RGB PSNR
against Avid, compared with 16.78 dB for that AAF import.
The qualified affine route now writes a
[Premiere 27 clip LUT](PREMIERE-27-COLOR-MODE.md#clip-lut).
Avid 25.12 also saved video-to-full scaling and the same nonlinear
cube in two orders on two edits of the same picture source. All 36
decoded frames differed between orders. The earlier edit still
rendered identically after the saved source order changed. Graph/2
records both orders and can identify the earlier edit's picture
placement even when the file's saved order differs. Private
Premiere 27 Color Mode prototypes each used a single composed clip
LUT. Across all 36 frames per order after Save As and reopen, the
renders measured 59.30 and 58.29 dB YUV PSNR against their
respective Avid renders. Swapping the references dropped both
comparisons to about 30.2 dB. Normal conversion now composes both
saved orders into editable Premiere 27 clip Color Mode LUTs for
verified 1080p24 Rec.709 picture-only Avid ProRes LT rewraps. The
two production results match all 72 prototype frames after Save
As and reopen. A missing or invalid cube leaves each cut in place
and reports the omitted stack. Other media and Resolve composition
remain open. [Ordered adapter verification](PREMIERE-27-COLOR-MODE.md#ordered-avid-color-adapters)
Other color transforms and unqualified cube profiles remain
occurrence-specific omissions.
[Premiere Input LUT controls](https://helpx.adobe.com/premiere/desktop/correct-color/set-up-color-management/color-management-using-clip-modify.html)
(CONV-0175)

### PNG Matte Key controls

The retained Avid alpha bin produces four Matte Key losses on V2 at
relative frames 0, 1, 4 and 5. Graph/2 retains each range, its saved
fill and matte references, and the shared master's original image
locator and alpha import flags. The linked AAF carries all
four composites with separate inputs. Premiere 27 rejected that AAF.
With known-good linked picture and the saved Avid Matte Key operation,
Premiere accepted both flat and nested controls, saved the fill and
matte on separate tracks inside a nest, and added an unsupported-effect
marker. After Save As and reopen, all 24 frames in each control
matched the raw matte input. Linking the original straight-alpha PNG in
a Premiere control reproduced both Avid polarities within 0.94 mean
absolute 8-bit RGB codes after reopen with **Composite in Linear
Color** disabled. A child sequence can keep that blend local when it
contains both the keyed foreground and the matching background. A
foreground-only child uses the parent's blend at its boundary.
The retained background edits use an unsupported picture compression.
Graph/2 also retains four imported PNG masters independently of
managed media admission, with exact saved locator, master identity,
track, source range and import settings. The eight background edits
resolve to the two master identities. Graph/2 now also keeps the four
markers on the matte input, with qualified record times and distinct
owners. A local graph/2 prototype generated all four editable nests
and linked the originals. After Premiere 27 reopened the project, all
eight frames were within one mean absolute 8-bit RGB code of the
Avid TIFFs, and all eight comments and marker positions survived.
The reader now identifies the tested saved-control profile on
each of the four Matte Key occurrences and distinguishes twelve
empty Color Adapters from changed payloads. A separate bounded
verifier resolves the original PNG by exact path, decodes it, and
pins the verified bytes. Separate files conversion now uses this
path for the verified 1080p24 still-only sequence with one-frame
Matte Keys and exact V1 background coverage. Direct CLI and saved
jobs build four editable children and keep both alpha polarities
and all eight marker times, comments and colors after Premiere 27
Save As and reopen. The markers use sequence ownership. The
standalone Avid Matte Key bin compositions remain unsupported.
(CONV-0143)

The direct CLI, selected-bin job and whole-project job now recover
provisional managed-media gaps for these PNGs. Preparation requires
the exact saved edit, still master and range before it verifies the
original. The Premiere output keeps the eight authored markers and
removes the gap markers for the recovered edits. Structural replay
matched the four editable nests and both alpha polarities. A missing
or damaged original blocks Review with the triggering edit's track,
record timecode, bin and saved PNG locator.

A 1080p23.976 sequence also qualified a full-canvas PNG overlay with neutral
Matte Key controls and only transparent and opaque pixels. Graph/2 records
the exact fill and alpha slots of the same imported master, the lower-track
background reference, the saved range and the import polarity. Separate
files links that PNG as an editable V2 clip. Premiere 27.0.0 build 88 retained
record frames [168,216), source frames [0,48), and the original path with
straight alpha after Save As and reopen. The six captions and three
transitions retained their saved placement. In an 864-frame export, the
overlay changed exactly the expected 48 scaled picture frames compared
with the previous Premiere export. Across those 48 frames, the sampled
opaque interior averaged 1.92 absolute 8-bit RGB codes from the native Avid
render. Both PCM24 channels matched the previous Premiere export over
1,729,728 samples each. The direct overlay is qualified for binary alpha
with these controls. (CONV-0212)

Review names the linked PNG and its exact V2 record range when the overlay
is prepared. A skipped reusable bin composition gets a Matte Key warning
when its saved picture effect identifies it. This accounts for the
successful timeline edit and the separate unsupported bin item.

A valid full-canvas PNG with partially transparent pixels or uniform alpha
leaves the lower track in place. Separate files reports the specific V2
occurrence and its exact span in Review. When a verified direct overlay's
original is missing, Review also reports the saved locator and the
conversion keeps the lower track. Review locates the omission or
file-verification failure by saved track and timecode, using the exact
sequence rate. The skipped overlay adds no linked
item, and its saved fill and matte facts stay in graph/2. Ambiguous
locators and corrupt PNGs still trigger a refusal. The older one-frame
Matte Key composite requires its saved PNG inputs.

Review of a corrupt direct-overlay PNG shared by two same-named
timelines identifies both V2 occurrences at their own timecodes. The
first file error stays the blocker. The additional details come from
verified overlay receipts with exactly the same saved locators.

#### JPEG fill with separate PNG alpha

Separate files conversion also writes the verified 1080p23.976 split
Matte Key. Graph/2 retains both master identities, both source ranges,
the lower-track reference, and the alpha import polarity. Preparation
links the verified JPEG and decodes its separate PNG alpha. When the
selected polarity forms a rectangle, Premiere receives an editable Add
mask on the original timeline track. One strictly enclosed rectangular
hole maps to a second editable mask set to Subtract. Other alpha shapes
leave the lower track visible and report the affected edit.

A reopened Premiere 27.0.0 build 88 control retained four 48-frame
placements from two JPEGs, one linked and one originally imported in
Avid. The imported fill uses the original JPEG saved in Avid's import
settings. Six frames exported after reopen showed all four composites,
the direct PNG overlay, and the independent V1 crop and AniMatte. A
single-line title with matching bounds immediately above a full-width
lower matte strip can sit on the strip with Premiere's default styling.

A second Save As and reopen replay used the same four edits with a hole
in the alpha. All four retained both editable Opacity paths and their
blend modes. In the rendered samples, the hole revealed the lower
picture to within one 8-bit RGB code. The sampled fill outside the hole
matched the solid-matte control exactly.

If the original JPEG cannot be found at its saved or mapped path, the
converter can use the imported still's managed MXF. It matches the Avid
file-package UMID and checks the saved clock and the single coded picture.
The qualified profile is 1080p23.976 Rec.709 DNxHD OP-Atom (CID 1237).
Premiere links the MXF in place and holds its first frame for the saved
edit range. The separate PNG continues to supply the editable mask.

Premiere 27.0.0 build 88 retained both 48-frame MXF-backed edits, their
source clocks, and both mask paths after Save As and reopen. Across all
96 rendered frames, sampled fill pixels matched the decoded MXF frame.
The sampled hole matched the lower-picture control with a mean difference
of 0.17 8-bit RGB codes across separate ProRes exports. The JPEG took
precedence when it was online. If neither fill is available, Review
reports the omitted edit and the lower picture plays through.

### Saved color settings on managed picture

A retained 1080p23.976 managed DNxHD edit saves an automatic sRGB to
Rec.709 source setting on its master. The picture placement has no active
Color Adapter. Across 88 aligned interior frames, the Avid export differed
from the linked MXF by 0.30 mean absolute 8-bit RGB codes. Applying the
nominal sRGB to Rec.709 gamma conversion to those decoded source pixels
increased the error to 13.06. The reopened Premiere output differed from
Avid by 2.49 codes under the same scaled comparison. Review identifies
saved master settings at the master and reports active timeline Color
Adapters at their edits. (CONV-0215)

## Native checks and linked AAF benchmarks

- **Coincident ranges on linked original clips.** Premiere 27
      retained both when the source and edit use separate lanes. When
      they use the same lane, each preference kept the selected range
      on the edit and preserved the other on the sequence.
      [Marker verification](SPLIT-EDIT-AUDIO-2026-11-06.md#coincident-source-and-edit-markers)

- **Compare a shifted Avid clip volume ramp with linked AAF.**
      Premiere 27 Save As and reopen preserved all four editable keys
      and the source trim in both projects. Dry and processed stereo
      PCM24 renders agree within one count per sample. Avid Direct Out
      differs by about 0.506 dB RMS over the processed edit after
      normalizing each render against its dry reference. (CONV-0177)
- **Explicit external source LUT.** The linked AAF carries the
      source LUT reference and ColorAdapter operation. Premiere 27 reopened
      the AAF with Unknown Effect markers and neutral picture. Any NLE
      embeds the qualified affine project-local cube as an editable
      Premiere 27 Color Mode LUT on the bound edit in Separate files mode.
      The generated LUT matched the native Color Mode render on all 174
      frames after Save As and reopen. The neutral use of the same picture
      source matches the linked AAF on every frame. The
      [Color Mode comparison](PREMIERE-27-COLOR-MODE.md#clip-lut) records
      the measured difference from the earlier Lumetri output.
      [Source LUT comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#external-source-lut-and-linked-aaf)
- **One native Avid Group with an independent A1 switch.** Premiere 27
      reopened the linked AAF and Any NLE projects with identical picture,
      exact ranges, and the same mono source selections. Any NLE retained
      switchable picture and matched Avid Direct Out across both lanes.
      The switched A1 edit is a direct mono clip.
      [Scope and results](EDITABLE-GROUPS-2026-11-08.md#native-independent-a1-switch)
- **Native coincident source and edit point markers.** Both ownership
      preferences passed Premiere 27 Save As and reopen.
      [Native point-marker comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#native-coincident-point-markers-and-linked-aaf)
- **Grouped picture and audio marker ranges.** Premiere 27 Save As
      and reopen kept both ranges. Premiere shares the audio marker
      with the picture through the Grouped collection. Edit ranges and
      selectors were unchanged, and the marked and unmarked renders
      matched in picture and audio. This confirms range retention with
      a limitation on lane ownership.
      [Marker comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#coincident-source-and-edit-markers)
- **Retained 24 fps sequence span.** The linked AAF exported with
      media offline and omitted the 22-frame sequence range. Premiere 27
      reopened the exact span from an isolated Any NLE project.
      [Sequence span comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#retained-native-sequence-span-and-linked-aaf)
- **Online 24 fps sequence span.** Avid exported the same saved
      22-frame marker in an online control. Premiere 27 reopened the
      imported AAF with no marker. The Any NLE version retained the
      range, matched all 87 picture frames, and stayed within one PCM16
      count per sample in the stereo mix. (CONV-0172)
- **Derived source span and edit copy at 24 fps.** For equivalent
      offsets, durations, source track, and creation timestamp, the
      reopened Any NLE project shows one range. Changing the edit
      timestamp by one second exercises the conflict preference and
      keeps two ranges when the sequence fallback is enabled. Both
      imported AAFs collapse to a point at frame zero. (CONV-0173)
