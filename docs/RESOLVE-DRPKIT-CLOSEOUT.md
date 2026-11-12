# drpkit reference closeout

The Fairlight review pins [drpkit `af3cedc0`](https://github.com/higuchiki/drpkit/tree/af3cedc0f40ae2192929158399667b4d28863dd6) (MIT). The [field map](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/drpkit/fairlight_map.yaml) SHA-256 is `0b1bfcca6b7a339896c0a093ff3eaa12915ec516e980c0bfe1d4cbd59f055b31`. The repository's records and fixture comparisons were used as format references. Any NLE retains its own bounded archive reader and writer.

## Coverage

| drpkit area | Graph/2 read | Write and qualification |
|---|---|---|
| Mixer faders, mute, pre-gain, pan, pan bypass, solo, Effects In, and output assignment | `resolve.fairlight` holds named integer controls, including fader and pre-gain in tenths of a dB, pan in tenths of a percent, connection code and zero-based output bus index. Rows bind to saved strip IDs, with an explicit channel index for a continuation. Optional channel names are descriptive. | Existing two-mono fader and mute writer profiles remain scoped. The turnover writer now preserves qualified static mono pan and direct stereo bus assignment; the reopened render checks are below. |
| Bus prefix and bus processing | Fader, mute, pre-gain, channel count, strip ID, outgoing routing cells, EQ and dynamics are bus-owned. Bus pan, bypass and solo are retained as raw codes. The first bus keeps the existing `main_bus` receipt. | The turnover profile preserves Bus 2 gain and mute. The main bus fader has a separate qualified profile. |
| Processing order and six EQ bands | Order and master enable codes, per-band enable and type codes, frequency in Hz, gain in tenths of a dB, slope and range codes, raw Q and knob. Original EQ and dynamics row IDs and integer values remain available. | A paired EQ save must vary frequency, gain and Q independently. Confirm the frequency knob and range updates before writing. Keep the partially calibrated Q law in research. |
| Compressor, expander/gate and limiter | Named raw controls and documented units where established. Stored knob encodings, expander mode and partially calibrated thresholds stay raw. | Test one compressor threshold/ratio pair with controlled audio levels, then timing laws and gate modes. A decoded knob law alone does not qualify processing equivalence. |
| Track and bus FX | Bounded plugin records retain track or bus ownership, channel, slot, numeric effect ID, raw flags and finite normalized parameters. Track Voice Isolation's enabled bit is interpreted for slot 6. | A reopened same-build Voice Isolation control confirmed track ownership with repeated display names, including a disabled track with a retained amount. Arbitrary insert bypass, other built-in flags and automation remain unresolved. |
| Clip built-in FX | `resolve.clip_fairlight` retains placement-owned Voice Isolation and Dialogue Leveler records from keyed `FL::ClipFX`. The expanded data is bounded and pinned by hash. | Clip Voice Isolation read-back was checked on 21.2.0 build 00006. Effects writing and audible processing comparisons are pending. |
| Clip gain, fades and source-frame keyframes | Existing gain, default fades and bounded source-frame keys remain available. Custom fade points retain saved position and level. | Existing bounded native controls apply. Combined envelopes and arbitrary curves need separate tests. Test an off-center fade point against isolated PCM before mapping its interpolation. |
| Clip colors, markers, clocks, edits, and crossfades | Existing readers retain scoped clip-color codes, marker owners, clock and placement facts, and saved transition alignment. | Carry broader palettes, clip-marker clocks, fractional audio placement and an asymmetric audio crossfade as separate qualification items. The audio-only composer covers a narrower edit profile. |
| Linked audio caches | The existing linked PCM16/PCM24 WAV profile uses verified media and channel selectors. drpkit also documents float WAV, FLAC, AAC-LC and MP3 at 44.1 and 48 kHz. | Start with one 44.1 kHz float WAV at a nonzero trim. Verify saved sample extent, channel binding, and the first and last rendered samples after reopen. Qualify compressed audio separately, with a gapless MP3 that distinguishes coded duration from playable samples. [drpkit media notes](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/docs/media.md) |
| Composer, project database, raw archive edits | Composer limits, SQLite database inspection, arbitrary raw edits, VCA and folder UI state stay outside the converter route. | Track selection is editor UI state. Unknown channel IDs `0x008`, `0x009`, `0x0b9`, and `0x13b` remain raw observations where present. |

The mixer reader caps expanded models at 1 MiB and requires unique table and
strip ownership. Clip FX expansion is capped at 64 KiB and charged against
the archive's decoded blob budget. The existing exact writer fingerprints
remain in force. drpkit's 21.1.1 archives served as offline comparisons. Full
archive admission remains scoped to the qualified Resolve build.

## Native pan and bus route

Resolve Studio 21.2.0 build 00006 saved separate disposable copies and reopened
them before export. The two-second timeline has two one-second edits. Each
uses distinct mono signals from PCM16 or PCM24 WAV, with nonzero source trims
and unchanged gain at the clip, track and bus levels.

- Static A1 pan used row `0x004 = -1000` and retained that value after reopen.
  In both edit interiors the reopened stereo render measured A1 coefficients
  of 1.0 left and 0.0 right. A2 remained 0.707946 on each channel.
- With two persisted stereo buses, changing only A2 row `0x40b` from 0 to 1
  preserved `0x400 = 1` and the Bus 2 assignment after reopen. Independently
  selected stereo exports put A1 on Bus 1 and A2 on Bus 2. The other track's
  coefficient was below one part per million on each bus.
- The pre-reopen two-bus control had only one serialized bus in the mixer
  even though Resolve displayed two. Using that export normalized the
  candidate to disconnected A2 (`0x400 = 0`, `0x40b = -1`). A bus must
  exist in the persisted mixer before a nonzero output index is usable.

The native results qualify the two specific field mappings on this build.
Across all 96,000 sample frames, the two bus outputs summed to the control
within one PCM24 least significant bit. The pan render's right channel
matched isolated A2 exactly.
Raw projects, renders and their measurement script remain private.

The Resolve knowledgebase records this observation as RESOLVE-0015
(SHA-256 `d8ee32e18487d2ad931877e6161355a2348227944de4a779be83bdce31dcde98`).

## Turnover writer qualification

The internal link-in-place writer now prepares static pan and direct output
from the sequence-owned `resolve.fairlight` graph/2 receipt. It accepts the
pinned two-mono mixer profiles with one or two persisted stereo output buses.
Bus 2 supports static gain from −6 to 0 dB in exact tenths and independent
mute. The tracks and Bus 1 require neutral gain, mute and processing.
The saved pan integer is copied in tenths of a percent. There is one
connected output per track. The writer
uses strip identity and bus index, with no inference from display names.
Other source mixer state retains its loss. This is a same-format mapping;
it does not infer equivalent pan law or routing from Avid or Premiere.

The two-bus writer requires a neutral donor saved on Resolve Studio 21.2.0
build 00006. The internal templates accept an optional `fairlight` path;
the linked project harness takes it as `--fairlight TWO-BUS.drp`. The donor
archive SHA-256 is
`51e9255ae6fed4f82b7a297531a7fab0bff8353153bfe39e7f797bb7296570cf`.
Only its fingerprinted mixer model is carried into the new timeline.
The staged archive is read back before publication to check both channel
owners, saved pan positions and output identities. A missing donor, absent
or ambiguous bus, disconnected track, and unqualified processing block
publication. No public route or graph/job schema was changed.

In Resolve Studio 21.2.0 build 00006, Save Project As, close, and reopen
preserved A1 hard left on Bus 1 and A2 centered on Bus 2. In separate
48 kHz PCM24 exports, each stereo bus held 96,000 sample frames:

- Bus 1's right channel was exactly silent. Its left matched the independent
  hard-left control within one PCM24 least significant bit after removing
  the centered A2 signal from that control.
- Bus 2 matched its previously isolated stereo reference sample for sample.
- All 48 decoded picture frames matched the control in both exports.
- A separate +50% A1 candidate saved and reopened with the same control
  value. Its Bus 1 render measured approximately 0.257 left and 0.955 right
  in both edit interiors, with no detectable A2 at one part per million.
  This is a native response measurement, not a general pan law.

Custom routing, wider layouts, automation, and active processing remain
outside this profile.

Separate native Fairlight saves confirmed that the Bus 2 fader uses the
second column of the bus fader row. Resolve stores the Bus 2 mute value
in two copies of the bus mute row. The writer updates both and rejects a
source with conflicting copies. The Fairlight saves also changed the
surrounding layout. The reader checks a separate fingerprint for that
layout while the writer uses the existing neutral donor.

The generated gain and mute projects were each saved with Save Project As,
closed, and reopened in Resolve Studio 21.2.0 build 00006. Bus 1 matched
the baseline sample for sample in both tests, with A1 hard left. Bus 2
matched each native reference across both channels. It measured −6 dB
with A2 centered in the gain test and produced digital silence in the
mute test. Each bus contained 96,000 PCM24 sample frames and all 48
decoded picture frames matched the baseline. The saved project data
confirmed both track assignments and pan positions.
