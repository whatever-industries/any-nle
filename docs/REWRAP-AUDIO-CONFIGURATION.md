# Rewrapped Avid audio: Premiere channel configuration

Use the [media rewrap guide](MEDIA-REWRAP.md) for current writer profiles
and the [media roadmap](ROADMAP.md#media-linking-user-2026-10-14) for
remaining verification. The assessment below records the November 4 scope.

2026-11-04. Converter implementation assessment from existing Avid/Premiere evidence,
current code and current Adobe UI documentation. Bounded native OP1a read-back is now recorded in
[NATIVE-OP1A-MASTERS.md](NATIVE-OP1A-MASTERS.md); wider qualification remains pending. The rate/audio extension was integrated locally at `512c852`. The subsequent explicit
[native AV-master route](NATIVE-OP1A-MASTERS.md) is implemented experimentally; it does
not establish native OP1a acceptance.

For the supported native mono OP-Atom cohort, file separation describes essence storage.
It does not determine whether Avid treats those tracks as independent mono or a stereo
pair. Preserve the saved bin's logical clip configuration and sequence routing when
putting those same sample streams in one OP1a file. No mixing or panning is baked in.

## Authority and the existing code path

| Question | Evidence/input | Current handling and boundary |
|---|---|---|
| Which picture/sound files belong to this master? | AVB master/file-mob references, full file PackageUIDs in MXF headers | `extract::MasterClip.tracks` → MediaIndex file-package join. No basename pairing. |
| Which Avid audio label is each file? | Bin's track label/index | `AudioFile.avid_track`; physical output sorted by that explicit number. |
| Mono or stereo, and which side? | Saved `_CHANNEL_GROUP_LIST`, parsed ordered `ST` members | `Clip.stereo_pairs` preserves left/right order. Unpaired supported tracks remain independent mono; two files do not imply stereo. |
| Which source channels go to which timeline tracks? | Sequence component channels/master-track references | `audio_channels`, `stereo.channels` and destination track numbers; do not substitute source file order. |
| Can the same media have another interpretation? | Separate AVB masters/bin copies, grouping and gain | Keep logical masters distinct even when the physical OP1a recipe is identical and deduplicated. |
| What do media databases establish? | PMR file/material identity joins; MDB retained graph research | Useful location/identity corroboration. Cache freshness/full MDB interpretation are unqualified; do not override a supplied bin's grouping. Current converter resolves headers directly and does not require database parsing. |

Avid authority: AVB-0005 (scoped grouping/order and routing evidence), AVB-0021/0023
(identity/linked versus native controls, MCA limitations), PMR-0003 (identity joins),
and the maintained `docs/bin-columns.md` BCOL-095 / `docs/avb-attribute-backlog.md`.
These remain in `knowledge/avid`; no second Avid database is created here.

## Intended translated configurations

The examples use the converter's ordered-pair model (first=left, second=right) to
specify intended logical mapping. Wider Avid routing keeps its recorded qualification
limits; these are not newly qualified native PRPROJ shapes.

| Saved Avid configuration | Physical OP1a sound | Required Premiere interpretation |
|---|---|---|
| A1/A2 unpaired | Two independent mono streams | Two mono clip channels, source A1→first and A2→second; not one stereo pair. |
| `ST:A1A2` | Same two mono streams | One stereo group: left=A1, right=A2. |
| `ST:A2A1` | Same two mono streams | One stereo group: left=A2, right=A1. Preserve the reversal. |
| A1/A2/A3; `ST:A3A1` | Streams ordered A1, A2, A3 | Stereo group left=A3/right=A1 plus independent mono A2. The experimental native route represents both groups explicitly; bounded three-channel PCM24 native checks pass; wider acceptance remains pending. |
| Four unpaired tracks | Four mono streams | Four discrete mono channels; no quad/surround inference. |
| Unsupported/ambiguous grouping | Sample streams may still be readable | Refuse rewrap before model projection/media creation rather than discard the declaration. |

The explicit `--rewrap-native-master` route builds a video+audio master backed by the
OP1a, with source-channel groups derived from the AVB. Each Avid master stays distinct,
including different interpretations of shared media. The native project retains full-source templates and physical-channel vectors.
Each logical group has its own gain-bearing template; both converted edits and fresh
bin insertion have bounded native checks. Wider qualification remains pending.

The existing Grouped/Merged/Separate choice applies without the new flag. Choosing
`--rewrap-native-master` explicitly replaces it; simultaneous `--bin-mode` is refused.
Source offsets, effective gains and sequence routing remain separate from packing.
The native route refuses unsupported routing/gain/pan/stereo-volume combinations before media creation;
see [its precise boundaries](NATIVE-OP1A-MASTERS.md#refusals-and-limits).

## Gaps found and bounded safeguards

The reader retains unsupported channel-group strings as `MasterClip.other_groups`,
but model/1 has no representation for them. The rewrap entry point now checks the bin
before that projection. Unknown declarations, invalid/duplicate source labels and
missing/repeated/overlapping stereo members refuse; ordinary conversion is unchanged.
Recognized reversed/non-adjacent stereo pairs pass without reordering. This guard covers
recognized `_CHANNEL_GROUP_LIST` data, not all possible Avid audio metadata.

Receipts now record each logical master's Avid track→source file→output audio ordinal
and absolute container stream index, bit depth, rate and master gain, plus its ordered
stereo pairs. Ordinals start at zero within audio; the first audio's container index is
one because picture is index zero. Physical order and logical pair order stay separate.

The former `StreamNumber` hypothesis failed even on uniform-depth media. Correcting
narrow views with `SingleAudioChannel`/`SingleChannel` fixed prebuilt timeline playback,
but fresh bin insertion exposed collapsing source templates. Native AV masters now keep
full-source templates and select physical channels in their logical vectors. Timeline
copies select the corresponding secondary channels. Both native save/reopen and fresh
insertion are checked; the [native route report](NATIVE-OP1A-MASTERS.md) gives exact scope.
Mixed PCM depths in a single OP1a file are refused before media creation because the tested
importer omits mismatched tracks; no normalization or sample rewriting is introduced.

Other unresolved configuration data: `_ASOUND_FIELD_GROUP_LIST`/MCA authority, speaker
roles/5.1/7.1, empty MCA lists, per-track display names not carried by AudioFile, and
master-pan policy beyond the existing per-edit/pan choices. Absence of decoded grouping
is not universal proof of absence. None is inferred from filenames or track count.

## Completed slice and remaining qualification

The native AV-master route has matched Rust/Python structural coverage and bounded
Premiere 26.5.2 read-back for mono, normal/reversed stereo and a non-adjacent pair plus mono.
The check includes fresh bin insertion, source assignments, offsets, gains, zero overrides,
distinct masters sharing media, save/reopen and sample-level exports.
See [native route scope and provenance](NATIVE-OP1A-MASTERS.md).

User acceptance, production AVB extraction end-to-end, wider channel counts/profiles,
other versions and legacy clip-construction modes remain qualification tasks. Sparse
A7/A3, 32-source and exact-rate checks are structural/backend coverage; the smaller fixture
cohort does not promote them to native qualification.

## Documentary provenance

Reviewed Avid authority revision `a166c881e78af7ee515d7e0940a992b2858e4c9b` and
Premiere authority revision `2bb68e520826bedbaa05c30dbd9ebe0527303d72`. Exact current
finding hashes are pinned by CONV-0030 in `research-dependencies.json`; prior pins
were retained. Source tracing starts from converter implementation `512c852`.

Adobe documents source-to-clip mapping through Modify > Audio Channels and a matrix
of source and clip channels. Import defaults can depend on Timeline preferences, so
the converter must set the intended mapping rather than rely on local import defaults.
This is UI capability evidence, not a PRPROJ serialization specification:

- [Audio channel mapping](https://helpx.adobe.com/premiere/desktop/add-audio-effects/advanced-audio-techniques/audio-channel-mapping.html), page dated 2026-01-07.
- [Change source audio channel mapping](https://helpx.adobe.com/premiere/desktop/add-audio-effects/advanced-audio-techniques/change-source-audio-channel-mapping.html), page dated 2026-04-15.
- [Map channels on import](https://helpx.adobe.com/premiere/desktop/add-audio-effects/advanced-audio-techniques/map-source-audio-channels-on-import.html), page dated 2026-01-07.

Fetched HTML hashes/access receipt and code/finding pins are retained privately under
`local/outputs/converter-op1a-rates-audio-merge-2026-11-04/` in the Any NLE workspace.
Premiere evidence PRPROJ-0019/0022 keeps its one-pair/native-version limitations.
