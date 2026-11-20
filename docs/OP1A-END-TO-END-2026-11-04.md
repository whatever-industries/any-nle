# OP1a synthetic AVB end-to-end validation

See the [media roadmap](ROADMAP.md#media-linking-user-2026-10-14) for
current qualification priorities. This report records the November 4 controls.

2026-11-04. Tested converter revision
`ef4eb18aeffdab6b05b6a354dd8e55106d90f688`, Premiere 26.5.2 build 5 and
FFmpeg/ffprobe 8.1. Both cases passed, followed by a same-author double-check.
This is agent-operated application evidence, not User acceptance, independent review
or qualification of Avid-created production bins. No production code changed.

## Inputs and path exercised

Two serialized synthetic AVBs reference generated 1920×1080 DNxHR LB picture at
24000/1001 fps and three discrete mono 48 kHz audio sources: uniformly PCM16 in one
case and uniformly PCM24 in the other. No User footage was copied or edited. The Rust
AVB writer created the bins using existing fixture chunks; their file-mob identities
match the actual source MXF package identities. No extracted model or prebuilt media
binding was supplied to the converter.

Each bin contains three distinct logical masters sharing the physical media:
independent mono, ordered stereo A2/A1 plus mono A3, and stereo A3/A1 plus mono A2.
Source labels are deliberately stored out of order. The sequence has five edits,
nonzero source offsets and gaps, with mono and stereo destination tracks.

The actual CLI exercised AVB extraction, MXF identity matching, graph projection,
rewrap preflight, strict full-span OP-Atom copy/mux, and native AV-master project
writing. It used `--rewrap-op1a`, `--rewrap-native-master` and explicit
`--match-avid-default-pan`. Premiere then opened, saved, closed and reopened each
project; native audio/picture exports and a fresh mixed-group source-bin insertion
were inspected. No folder-picker UI, mouse or keyboard control was used.

## Results

- Each case produces one OP1a file shared by three distinct logical masters.
  All four encoded essence hashes match the originals and backend receipts;
  the original per-case 16/24-bit depth is preserved.
- The full 108-frame timeline render, 216216 samples per output channel, exactly
  matches the expected source PCM at the PCM16 export resolution, including clip
  boundaries and gaps. Audio before and after native save/reopen is identical.
- Routing passes for source A1 to destination A1, A2 to A3, stereo A2/A1 to A2,
  stereo A3/A1 to A2, and mono A2 to A4. Native source vectors retain grouping;
  fresh mixed-group insertion renders A3 left/A1 right plus independent A2.
  Direct tone projection and Goertzel measurements agree.
- Picture at sequence `00:00:00:03` corresponds to source `00:00:00:09`, checked
  against adjacent source frames. The gap at `00:00:00:18` is black. This is an
  offset/gap check, not general color or pixel-fidelity qualification.
- Dry run displays **Creates new media.**, size/free-space estimates, and creates
  no graph, project or media. Four refusal controls pass without outputs or
  overwrites: missing source mobs, missing media, and repeat conversion into
  existing outputs for each bit depth. Original input hashes remain unchanged.

Initial incomplete fixture bins remain as negative controls. Builder mistakes were
corrected privately, without modifying production code. Older padded diagnostic
holders were not used as backend inputs. No dependency installation was needed.

## Evidence pins and reproduction

Raw media, projects, native returns, builder source and logs remain outside Git.
Paths below are relative to the canonical workspace or converter, respectively;
they are retained local evidence, not portable repository fixtures.

- Converter packet: `local/to-test/2026-11-04 OP1a end to end/`.
- Workspace audit/builder: `local/outputs/converter-op1a-e2e-2026-11-04/`.
- Packet `evidence.json` SHA-256:
  `072c78c6eeb68a5d7a9f2d118e69b19ed4b8efa75b4f6d1156e285f15756872e`.
- Packet `RESULTS.txt` SHA-256:
  `73c739ed6f548fb79eae01606973b3d17184a42e86ab01dea6ad25ba8ff656f1`.

Run `python3 verify_e2e.py` in the retained packet to reproduce the measurements
without controlling an application. It requires the retained inputs/returns, tested
converter binary, FFmpeg/ffprobe and the existing Pillow environment. It verifies
input hashes, essence, exact PCM, rational graph positions, native vectors, tone
assignments and picture offset/gap. It does not regenerate native application returns.
The audit's `double-check.log` records the second pass; all 75 manifest entries were
also checked against their retained files before this documentation commit.

Knowledgebase findings, decision bindings and their confidence remain unchanged.

## Remaining qualification

Real Avid-created bins and production footage, User acceptance, full application
restart, wider rates/profiles/channel counts and other versions remain unqualified.
The native timeline render is PCM16; original-depth preservation is established
separately by encoded essence hashes and source/output PCM checks. Gain behavior
is covered by the earlier [routing fixtures](NATIVE-OP1A-MASTERS.md), not extended
by these unit-gain end-to-end cases.

A cosmetic follow-up was identified at the tested revision: the CLI summary prints
the underlying bin-mode policy
(`separate`) although writer notes and `conversion.json` correctly identify the
explicit native AV-master route. A subsequent CLI summary fix reports that route
explicitly; ordinary conversions retain their selected bin-mode summary. The pinned
test revision and private historical receipts above remain unchanged.
