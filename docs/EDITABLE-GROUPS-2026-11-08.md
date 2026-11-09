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
observed filler may surround it. Both fixed audio lanes must match the first
camera's saved extent. Missing, ambiguous, mixed-clock or discontinuous alternatives
and switching audio selectors refuse with a named reason.

The group item is a native sequence-backed multicam with two video tracks, both
cameras' audio, two exposed mono groups, and the alternate camera audio muted in
the source. The picture selector and group-local In/Out are written per edited use.
Separate audio placements retain their channel groups across picture changes. Both
leaf masters are planned even when an angle is unused, and each verified file is
rewrapped once. The CLI native rewrap and the typed job writer share construction.
Other Premiere routes refuse Group input rather than falling back to selected leaves.

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

The next meaningful expansion is changing the audio source independently of picture.
Missing-audio cameras, wider or paired layouts, retimed/mixed-rate members, multipart
MultiGroups, sequence clock correction and broader playback remain separate gates.

Private receipts live under `local/outputs/multicam-translation-2026-11-08/`.
`level-isolation/ISOLATION.json` SHA256
`2a7b52b45d259ffa2021fbd5d41bc28d72b01aaf6687303f2fb6bd8185513c3d`;
`editable-output-v1/NATIVE-VERIFICATION.json` SHA256
`802701d6eeba93db41bcfcbf3b0e8c296a8572bf719c2a2330aafff130681b23`.
Same-author native research and recheck; no claim of independent review, continuous
loop coverage, speaker output or User compatibility approval.
