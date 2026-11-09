# Native OP1a-backed AV masters (experimental)

2026-11-07 extension: the OP1a structures described below remain applicable to DNx.
ProRes MOV uses different source bindings, and explicit six/eight-channel groups now
have a bounded native route. Read the multichannel extension
for current scope, optional model fields, tested output buses and remaining refusals.
The earlier exclusions below describe the original slice, not a generic MOV binding.


2026-11-04. CONV-0031. Rust and Python reference writers preserve the supported bin
mono/stereo groups and source assignments. Agent-operated Premiere 26.5.2 checks now
cover bounded 23.976 fps synthetic fixtures, including fresh insertion from the bin,
and the actual Avid-saved native-source trial.
User acceptance and wider profile qualification remain pending.

## Choose the route

```sh
any-nle convert --from avid BIN.avb --media SOURCE_FOLDER --to premiere OUTPUT.prproj \
  --rewrap-op1a-prompt --rewrap-native-master --rewrap-dry-run
# Remove --rewrap-dry-run to create the media/project after the review.
```

The macOS picker can be replaced with `--rewrap-op1a NEW_MEDIA_DIRECTORY` for scripts.
The exact notice is **Creates new media.** `--rewrap-native-master` is explicit, experimental
and off by default in the CLI. It replaces Grouped/Merged/Separate construction for that run;
omit `--bin-mode`. Other policy choices still apply. The complete wizard remains planned.
Its Avid → Premiere clip menu will put **OP1a rewrap first and select it by default**
for new/reset settings (User, 2026-11-05), followed by Grouped, Merged and Separate.
That choice uses this native AV-master route and the output-folder/space review;
explicitly remembered choices and presets remain respected. See the
[menu and review contract](MEDIA-REWRAP.md#import-option-and-review).

## Logical configuration and placement

- Each referenced Avid master remains a distinct file-backed Premiere master, including
  masters with different grouping over the same physical OP1a file.
- Physical sound order follows explicit Avid source labels; the container's picture
  stream is excluded from audio ordinals. Ordered stereo pairs retain their saved sides.
- A native source template describes the **whole physical audio source**, with full
  channel layout and secondary channels in physical order. Separate gain-bearing templates
  let each logical group retain its own master gain.
- Each logical channel vector identifies its source template (`SourceClipIndex`) and
  physical channel (`mSourceChannelIndex`). This preserves independent mono, reversed
  stereo, and non-adjacent stereo plus a mono member. It also preserves fresh bin insertion.
- Timeline placement copies the intended group's template and selects only its physical
  secondary channels, in the saved order. `OrigChGrp` keeps the group identity. Destination
  audio tracks, source in/out, pans and effective gains follow the sequence. An explicit
  zero edit gain replaces a nonzero master gain.
- Native Media views retain full-source AudioStreams without `StreamNumber` or
  `SingleAudioChannel`. Narrowing those views makes Premiere collapse source templates
  to mono; a correct prebuilt timeline alone would fail to expose that defect.
- The older Grouped/Merged/Separate route instead uses explicit `SingleAudioChannel` and
  AudioStream `SingleChannel` selectors plus the full physical layout. Those flattened
  selectors replace the former importer-specific `StreamNumber` guess. That route's
  complete clip constructions still need their own native qualification.

## Refusals and limits

All [media-preparation limits](MEDIA-REWRAP.md) apply: matching picture-led full spans
and clocks, up to 32 discrete mono 48 kHz PCM16 **or** PCM24 sources, qualified DNx
profiles/rates, a new output directory, four audio/three video timeline tracks, and no
implicit transcode. A single OP1a file must use one PCM depth; different clips in a project
may use different depths. Mixed-depth assembly refuses before new media is created.
It does not normalize samples. Unknown saved group declarations refuse before projection.

The native route also refuses missing/duplicate labels, inconsistent clocks/spans,
unequal gains within a stereo pair, mono edits of grouped stereo members, stereo edits
whose order differs from the bin group, non-default per-channel stereo pan, and stereo
volume automation requiring an unsupported component. Speaker/MCA/surround interpretations,
audio-only masters, variable speed and other previously unsupported cases stay out of scope.
No frozen model schema or ordinary no-rewrap writer output changes.

## Native checks and evidence boundaries

Premiere 26.5.2 build 5, 24000/1001 DNxHR LB picture, 48 kHz audio:

- Two uniform PCM16 streams and two uniform PCM24 streams: independent mono routing,
  normal stereo, reversed stereo, nonzero source offsets, native Save As, project close/
  reopen, and repeat export. Tested timeline windows match decoded source PCM exactly;
  pre/post native exports are byte-identical.
- Three uniform PCM24 streams: A3/A1 stereo plus independent A2, unsorted source labels,
  distinct masters sharing media, per-group gains and explicit reset-to-zero gains.
  Non-unity gain checks allow final PCM16 quantization error; unit-gain windows match exactly.
- Fresh insertion from the bin: reversed PCM16 stereo, normal PCM24 stereo, and mixed
  A3/A1 stereo plus mono A2. Three freshly inserted independent PCM24 mono tracks are
  also rendered in isolation to verify each physical assignment. The mixed insertion
  is saved, closed, reopened and re-exported.
- Initial selector-only and aggregate-only probes failed fresh insertion; they are retained
  as refusal/regression evidence. The full-source-template/channel-vector construction is
  the tested correction. No raw project bytes or media are stored in Git.

Private receipts and a reproducible verifier live under the converter's
`local/to-test/2026-11-04 OP1a routing fix/`; build/test/audit receipts live under workspace
`local/outputs/converter-op1a-audio-routing-2026-11-04/`. The final fixture receipt pins
project, native return, media, helper and measurement hashes. Avid/Premiere authority
references remain pinned in `research-dependencies.json`; these private native experiments
are converter evidence and do not change knowledgebase findings or their confidence.

The subsequent synthetic AVB end-to-end test also passes
the actual converter CLI, identity matching and strict rewrap through native save/reopen
for three PCM16 or three PCM24 streams. Its complete timeline PCM, source grouping,
channel assignments, picture offset/gap and refusal checks pass; evidence is hash-pinned.

The Avid-saved native-source trial additionally
passes the selected 36-frame bin through the guarded terminal-zero route and native
Premiere save/reopen. Both complete 72072-sample PCM24 channels match the originals,
and all 36 coded pictures are preserved. Native picture checks and playback pass with
the recorded identical-final-picture and color-fidelity limits.

This is agent-operated validation with a same-author double-check, not User acceptance or
an independent review. Wider rates/profiles, more channels, other versions, Avid-created
production bins beyond that selected fixture, and a full application restart remain unqualified.
Existing backend tests separately verify
strict OP-Atom full-span copy, clocks, sample bytes and refusals; padded diagnostic holders
from older writer audits are not qualified backend inputs.
