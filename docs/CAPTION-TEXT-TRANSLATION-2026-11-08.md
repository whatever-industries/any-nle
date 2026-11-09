# Avid captions and current Titler+ text translation

2026-11-08. Source baseline `db7aebb822bdc656fdf9a8fd5482e37f50639fea`.
Avid authority: AVB-0039, AVB-0013 and AVB-0038. Premiere: PRPROJ-0033 and the existing
caption/graphic primitives. CONV-0037 pins exact revisions/hashes after local KB integration.
Media, private native metadata, returns and screenshots remain outside Git.

## Implemented scope

The direct reader captures text occurrence identity, owning lane/rate, content and
qualified record span while the decoded bin is alive. It retains owned bounded facts,
not decoded nodes, native addresses or source buffers. Unsupported timing stays unresolved.
The exact current effect/parameter/type identities select SubCap and current Titler+;
Classic Title and Titler+ Legacy are not guessed from labels or suffixes.

Graph/2 has optional `texts` with caption/graphic kind, content and timing/reason. It omits
empty lists; older graph/2 remains accepted. Graph/1 and strict model/1 are unchanged.
Native title property scalars are retained separately in `avid.text`: role/list/name/value,
including transform, bounds, anchor/enable settings, font and selected material/shadow
fields. Repeated character roles are not a per-glyph identity map. Values remain native
strings with provenance, not neutral geometry, flattened inheritance or resolved fonts.
Unknown/compound metadata yields a scoped reason rather than invented scalar semantics.

Premiere preparation borrows the graph. It accepts exactly representable frame-aligned
spans, reports unresolved timing, same-lane title-over-picture and overlapping subtitle
cues, and merges only disjoint subtitle lanes into one active track. It never infers
missing media offsets from text. Multiple graphic overlaps remain safe validation errors.

Prepared text is separate from the legacy model. The writer embeds captions directly
from exact ticks with no temporary SRT or millisecond round-trip. Graphics are editable
text. Source LF becomes destination CR paragraphs in the tested payloads. All converter
caption/graphic style references use ArialMT/48pt; graphics use normalized(0.1,0.35).
These are explicit destination defaults, not source appearance. The older generic writer
primitives and no-text Rust/Python oracle outputs are preserved.

## What the Avid knowledge already establishes

AVB-0013 documents scoped position/size/kerning and property-list inheritance evidence.
It also distinguishes applied character/flow lists from unused defaults and stale cached
text. That knowledge is used to resolve list identities and retain native properties.
It does not establish source-to-Premiere fixed-box glyph alignment. The new User-authored
title is fixed width/height, unlike the older point-text native render baseline. Mapping
its position directly to a Premiere point-text baseline would conceal that unresolved
anchor/wrapping/metrics problem. Native values survive; geometry is honestly reported lost.

Next useful slice: resolve one fixed-box origin/anchor and wrapping control with the
actual selected font/list, then map it to a matching Premiere box representation. Keep
font metrics/substitution, inherited per-character styles, cross-raster normalization,
rotation/scale and animation separate. Do not repeat completed point-text delta tests.

## Native verification and its limits

User authored two native examples, then removed one duplicated Titler+ text box and moved
the remaining box. SubCap stores accented UTF8 plus LF; current Titler+ uses Flow/Text
paragraph escapes with no Character records. Both actual effects span record0..48 at the
qualified24000/1001 playback family; the originally requested+1s was not the saved span.
Avid raster capture failed, so native Avid glyph/style equality is not claimed.

Full source/prepared A/V routing was checked against media read in place. A separately
derived visual control clears media after that preparation and retains only text. In
Premiere26.5.2 Mac, both two-line accented examples display at sampled frames0/24/47.
SaveAs, actual close and reopen reproduce frame24 RGB exactly; PNG byte hashes differ.
This verifies tested native text rendering/persistence, not full-media playback, visual
source equivalence, User acceptance, Windows or every Unicode script.

A separate24fps cue test has the intended half-open boundary samples. Earlier tiny
LucidaConsole/template and font-fallback trials remain retained; final payloads have no
LucidaConsole reference. The User's earlier missing-font report is not discarded. No
claim of universal font availability or absence of future dialogs follows.

The full-media separate OP-Atom path still refuses its existing fractional-rate timecode
case. The native OP1a route completes read-only dry-run with text and reports appearance
losses. No media was generated/copied to qualify this text slice; no broader media route
was enabled. Editors were not restarted and the original User project stayed open.

## Verification and preserved audit

Private task root: `local/outputs/subcap-text-translation-2026-11-08/` outside Git.
It contains baseline/protected hashes, source revisions, failed trials, native artifacts,
reopen pixel checks, property-list reconciliation and acceptance logs. A live external
Avid bin changed during the authoring session; cause is unestablished. Historical fixture
manifests remain intact. The successor keeps1669 controls, explicitly requalifies that
bin as the same pre-change/current nonempty PCMA-extension refusal, and records the old
unavailable digest. Empty outputs were never counted as golden parity. New dependency
pins receive a separately recorded successor hash after review.

Run the required-fixture acceptance driver, KB metadata/coverage/codex/provenance checks
and research suites. Source snapshots, preparation side effects, malformed content,
duplicate fields, native identity/timing uncertainty, overlap, bounds and no-text parity
have controls. Acceptance checks are same-author verification, not independent review or
native compatibility beyond the recorded cohort. Final canonical results belong in the
private completion receipt. Held features, original snapshots and older findings stay
unchanged; public projection/export bindings remain stale. Nothing is pushed here.
