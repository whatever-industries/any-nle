# Direct DNxHR OP-Atom linking in Premiere

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
comparison uses the converter's established Constant Power approximation.

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

## Wider direct-link profiles

Direct ProRes OP-Atom and wider DNxHR profiles remain open.
