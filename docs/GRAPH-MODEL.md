# Edit graph contracts

Current source contract: **`anynle.graph/2`**, a qualified source interpretation.
The direct Avid adapter and explicit `PreparedPremiere` boundary are described in
[Source graph and destination preparation](SOURCE-GRAPH-MIGRATION.md).
Graph/1 remains a separate legacy contract; its interpretation and APIs are retained.
Do not infer preparation provenance or apply graph/2 transition normalization to it.

## Historical graph/1 profile

Initial version, 2026-10-26 (Claude). Built to the plan the User approved on 2026-10-26
("structure for adding more editing programs"). This is also the Premiere-side review of Codex's
proposal in AVB-PORT-COORDINATION.md ("For additional NLE pairs").
Nothing here changes `avid2prproj.model/1`, the Avid reader, or the Python oracle.

## Why

Each editing program gets **one reader into the graph and one writer out of it**. Five programs
then need ten adapters, not twenty pairwise converters. Choices that depend on the *pair* (bin
mode, which Premiere label an Avid colour becomes, marker conflicts, Avid's default pan) are not
graph content: they belong to a mapping stage between graph and writer.

## Where it lives

```
converter/crates/            separate Cargo workspace (own lock and target/)
  graph/   anynle-graph      types, strict JSON, validation, exact rational time (std only)
  bridge/  anynle-bridge     avid2prproj.model/1 <-> graph (depends on ../prproj)
  mapping/ anynle-mapping    pair policies: Avid -> Premiere (options, colour preview, losses)
  fcpxml/  anynle-fcpxml     graph -> FCPXML 1.9 (Resolve, Final Cut Pro); losses listed
  app/     anynle-app        `any-nle` command line: reader -> graph -> policy -> writer
  tools/compare_pipelines.zsh  any-nle vs avid-reader | prproj-write on every fixture bin
```

- **Policy.** `any-nle policy` prints the default Avid → Premiere policy
  (`anynle.policy.avid-premiere/1`), which is model/1's default options.
  `any-nle convert --policy FILE` applies an edited copy.
- **Colour preview.** `any-nle colours GRAPH.json` shows the colour mapper's table, computed
  with the writer's own functions.
- **Pipeline check.** `compare_pipelines.zsh` covers 24 fixture bins × 3 bin modes, each bin
  searching only its own snapshot's media:
  - 60 runs byte-identical;
  - 12 runs refused identically by both: 4 bins whose snapshots hold no MXF (ROADMAP);
  - 0 differences.

Why a separate workspace:
- A workspace over `avid-reader/` and `prproj/` would move their build output into one shared
  `target/`, breaking the documented `avid-reader/target/release/...` and
  `prproj/target/release/...` commands, `golden_sweep.py` and Codex's worktree builds.
- It would also silently drop each crate's `overflow-checks` release profile.
- Instead the existing crates stay untouched and are path dependencies. `crates/Cargo.lock` was
  seeded from `prproj/Cargo.lock`, and flate2/libz-sys resolve to the same versions.

In that initial implementation, the only change outside `crates/` was `impl FromIterator<Clip> for ClipMap` in
`prproj/src/convert/model.rs`: an output-neutral constructor the bridge needs.

## The contract's concepts, and how graph/1 holds them

| Contract concept | graph/1 |
|---|---|
| Identity | Every media stream, item, bin, sequence and placement has an `id`, unique across the graph; references must resolve (`Graph::validate`). Bridge IDs are namespaced (`clip:<model id>`, `clip:<id>:audio:0`, `bin:0`, `sequence:0`, `sequence:0:3`) so model/1 names cannot collide. |
| Time | `Rational` seconds, reduced, written `"num/den"`; rates are rationals too. No program's clock is built in. Model/1 ticks convert exactly; going back, a time off the 254016000000-tick clock is refused, not rounded. |
| Hierarchy | `bins[]` with optional `parent` (an earlier bin, so there are no cycles); an item may sit in several bins. |
| Media | `Media` keeps separate fields for identity (`id`), locations (a list), technical facts (codec, size, rates, bits), source stream number, and `online` (`null` = not checked). |
| Edits | `Placement`: item, record start, source in, duration, picture track and/or audio tracks, per-track stream routing, multichannel groups, gain/volume/pan, speed, frame map, transform. |
| Compound items | **Not yet.** Model/1 has none: merged/grouped builds are a Premiere writer option. Subclips, groups, multicam and nests are the next additions, typed and not flattened, when a reader produces them. |
| Properties | Typed where neutral: transform in frame fractions, percent and degrees; audio in dB and percent pan. Effects with no neutral form yet are `effects[]`, named `<program>.<effect>` (`avid.color_correction`, `avid.animatte`), with their parameters. |
| Losses | `losses[]`: object ID, aspect (e.g. `transform`) and reason. A reader's "not converted" notes become losses. `bridge::to_model` returns what model/1 could not hold. |
| Native data | `sidecars[]`: adapter, object ID and data. They are never portable meaning. Today: `avid` (Mob ID, bin columns, start/aux timecodes, Source File/Path) and `premiere` (`time_display`). |
| Pair policy | Not in the graph. `bridge::to_graph` returns model/1's `Options` beside the graph. |

In graph/2 an Avid picture chain is a single `avid.picture_chain` effect.
Its `params.stages` array runs from the picture input to the output.
Each stage carries its `kind`, `params`, and a `transform` when the stage
has one. The transform uses the same units as a placement transform.
For Flop inside 3D Warp, the first stage reflects the picture and the
second stage carries Warp's transform and saved curves. The placement
has no separate transform to apply to the chain.

The graph/2 `picture.motion_linear` effect carries two ordered keys on a
standalone picture placement. Its clock is `item_source` and its
interpolation is `linear`. Each key stores an exact rational second
in `at`, X and Y frame fractions from center in `position` (right and
down are positive), and uniform `scale_percent`. Destination preparation
checks whether the key times and geometry fit its qualified edit profile.

## Acceptance (the contract's first step): passed 2026-10-26

`crates/bridge/tests/round_trip.rs` runs every model in `examples/` and the golden corpus
`local/golden/models/`: 37 models, 36 golden plus 1 example. Each goes model/1 → graph → graph
JSON → graph → model/1 and must meet five conditions:
- the graph JSON round trip is equal, and its text is stable;
- the model comes back **equal**;
- graph losses are exactly model/1's own `not_converted` entries;
- `to_model` reports **no new losses**;
- the Rust writer, with seed `any-nle-golden` and the same output path, writes **byte-identical**
  projects (LUT files included) from the original and the round-tripped model.

Result: 37/37 equal, 37/37 byte-identical.

Run: `cd crates && cargo test --release`.

Codex's fixture request ("a nested bin, a source used by two placements, rational timing, and
one deliberately unsupported effect"):
- `graph-bridge-min` in `scripts/make_golden.py` (2026-10-20) covers it except for the nested
  bin. It has one source in two bins and two placements, 30000/1001 timing, and a
  not-converted 3D Warp rotation.
- Model/1 cannot express nesting, so a nested-bin case can only come from a reader that writes
  the graph directly (the Avid project-folder import on the ROADMAP).

## Projection validation (2026-10-29)

`bridge::to_model` rechecks model/1's cross-object placement rules before returning a writer
input. The JSON loader and bridge call the same `validate_sequence_links` helper: a picture
placement needs picture, audio channel/group references must exist, record start and duration
must fall on the sequence frame grid, and source In plus source span must fit the clip's
media-duration bound. Speed changes use their source duration. Source In retains model/1's
existing subframe allowance; it is not newly snapped to a picture frame.

The helper checks the sequence rate before tick arithmetic. Projection also refuses two
otherwise distinct graph item IDs that would collide after removing the bridge's `clip:`
prefix. The check lives in the bridge so the app and other library callers both receive it.
These are shared model/1 rules, not a change to graph/1 or to Avid interpretation. A new
adapter still validates its own scalar fields, codecs, effect semantics and supported shapes;
these cross-object checks are not a complete substitute for adapter validation.

## Known gaps (to do as they are needed)

- Avid colour correction and AniMatte are `avid.*` effects. Making them neutral (colour
  operations; masks in frame fractions, with feather in neutral units) needs the Avid render
  calibrations (AVB-0015, AVID colour findings) and is per-effect work.
- `time_display` (a Premiere TimeDisplay code the Avid reader chooses) sits in a `premiere`
  sidecar. Its neutral form is the item's display rate.
- Tracks are listed only where model/1 has a level. A reader that knows all tracks (names,
  mute/lock) will fill `tracks[]`.
- Model/1's parser accepts an `event` index on a *clip* marker, which means nothing there. The
  bridge refuses it rather than dropping it silently. The Avid reader puts `event` only on
  sequence markers, so only hand-written models are affected.
- Marker and clip colours keep their program palette (`{palette: "avid", name}` and 16-bit
  RGB). Mapping them is the mapping stage's job (the colour-mapper wizard on the ROADMAP).

## Coordination

- Codex owns the AVB port and its proposal. The User relays to Codex only Avid knowledge, so
  this review is **not** relayed automatically. Whether to share this converter design with
  Codex is the User's decision.
- The original direct-reader coordination gate is superseded by the User-approved
  2026-11-05 source-graph migration; see the current guide above. This dated graph/1
  record does not describe the new default Avid→Premiere path.
- Folder renames (`avid-reader/`, `prproj/` → `crates/`) are deferred while Codex ports AVB.
