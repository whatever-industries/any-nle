# Automatic Motion Adapters between 24 and 23.976

The shared Any NLE job translates the observed automatic Avid adapter on a
directly linked 24 fps progressive 1080p H.264/AAC source in a 24000/1001
sequence. The MP4 stays linked. Native Avid media elsewhere in the same
sequence can still be rewrapped to OP1a. The 24 fps source master keeps its
own rate. The reciprocal direction has a separate qualified scope below.

The source reader recognizes the saved automatic progressive adapter, its
default single constant control, the direct source range, the 1000/1001
stored span ratio and phase. It refuses unsupported settings, nested ranges,
transition endpoints, and unqualified linked-file origins. The picture
placement is an explicit absolute frame map. Premiere writes it as editable
Time Remapping, sampling the selected source frame on each record frame.

Sound does not use the picture map. Its file-relative In uses the saved
source In, the linked file track's one source-frame origin, and the
fractional adapter phase on the 48 kHz sample clock. This corridor admits
the AAC descriptor's extra priming packet only when the file physically
witnesses the exact skip and playable zero. The physical extent excludes
priming. A verified subframe tail beyond the playable source is left silent
under the corresponding picture; the converter does not read past the file.
Mono source-channel selectors and existing pan policy still apply.

## Native read-back

A user-saved Avid bin linked a deterministic 24 fps source with a nonzero
In and an adjacent native Avid clip. Premiere Pro 26.5.2 on macOS opened the
converted project and a Save As copy with zero offline items. The native
picture and linked source remained online. Before and after close and
reopen, all 1,102 linked output pictures selected the same source frame as
the Avid export, including the phase-dependent skip. Both full linked
PCM24 lanes matched Avid Direct Out sample for sample with separate A1/A2
pans enabled, including the 2,096-sample lead and the 300-sample silent
tail. Reopened playback crossed the skip. The original and test files
remain private and unchanged. This does not qualify broader codec profiles,
other track origins or render modes, or other platforms.

Evidence: CONV-0044 pins AVB-0040 for native Avid measurements and PRPROJ-0034
for Premiere read-back, with AVB-0010/0021 and PRPROJ-0028 as context. The
original returns and native export measurements are held in local evidence.

### Premiere 27 direct-link read-back

The same saved Avid timeline converts in Separate mode with its original
DNxHR LB picture, both mono PCM24 sibling MXFs, and the H.264/AAC MP4 linked
in place. Premiere 27.0.0 build 88 kept all four files online after Save As,
close and reopen. The rendered sequence contains 1,174 frames. In the UME
section all 1,102 numbered frames match the Avid cadence, including the
skip at record frame 1,024. The audio starts at file sample 98,096. Each
UME lane matches 2,206,204 Avid Direct Out samples with the Avid pan
option, including the 300-sample silent tail. The Separate saved job also
writes the timeline.

Avid Media Composer 25.12.0.58863 exported the same edit with AAF Edit
Protocol and linked picture and audio. The Premiere 27 Save As and
reopen kept the three MXFs and the MP4 picture online. Both conversions'
72 managed picture frames decode identically, and both select the same
1,102 numbered UME frames as Avid. The AAF contains an Audio Warp operation
and one input on each sound lane. Premiere's reopened import has no UME
audio edit on either lane and renders silence throughout that section.
Premiere's Events panel showed no warning in the reopened project. Grouped and Merged
projects retained all four paths and twelve placements on reopen. Fresh
insertion from the Separate bin item retained both original channels at
48 kHz, with 2,304,000 samples per channel. The fresh-insertion export
verified channel identity against independently decoded AAC.
The local macOS package also converted the fixture through Review. The
reopened project retained all 12 placements, and the full 2,350,348-sample
PCM24 render per channel matched the direct CLI render.
Source and media references

## Automatic 23.976 to 24

The reciprocal route accepts the observed default progressive adapter on a
direct 1920×1080 H.264/AAC linked original with two proven 48 kHz mono
routes. The saved 1001/1000 span ratio, the single constant control and
the direct source range must agree. Source In and adapter phase are
distinct. Picture positions use the saved phase and the 1000/1001 sampling
ratio while sound uses its independent file-track start and sample clock.
Premiere retains the original MP4 and writes the picture as editable Time
Remapping.

A trimmed native Avid fixture used phase 49 and Source In 48. Premiere
Pro 26.5.2 on macOS matched all 1,104 picture selections, including the
repeat at 01:00:39:17. A1 and A2 began at source sample 100,002, 3,906
samples beyond the nominal In. With the existing Avid pan option enabled,
the 2,206,302 in-range samples per lane matched Avid Direct Out exactly.
The same checks passed after Save As, close and reopen.

The converter limits the audio edit to the MP4's declared playable
range and reports the remaining interval. Avid rendered 768 samples of
final AAC decoder padding before its silent tail. Premiere rendered
silence for that interval even in a control that extended the audio edit.
The project preserves the sequence's full picture duration. This local
read-back covers the saved default adapter and its measured track origins.
Wider source profiles, promoted or nested adapters, other rate pairs and
other platforms require their own qualification. CONV-0045 pins AVB-0041
and PRPROJ-0035 for this reverse-direction result.

### Premiere 27 replay of 23.976 to 24

Premiere 27.0.0 build 88 kept the original MP4, editable picture remap
and both sound edits after Save As, close and reopen. Both picture
counters verify the same 1,104 source-frame selections, including the
repeat. Both channels match the prior Premiere render across all
2,208,000 sample frames, including the silent 1,698-frame tail. The
768 AAC padding samples in the Avid reference remain outside the
MP4's declared playable range.
