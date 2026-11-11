# Architecture decisions

Current source/target boundary: [ADR-0005](#adr-0005-qualified-source-graph-and-explicit-destination-preparation-2026-11-05).
Earlier decisions below are dated history; the standalone legacy interfaces remain available.

## ADR-0001: The JSON edit model is the boundary; Rust is the long-term language (2026-10-14)

Decided with the User.

- The neutral edit model (`avid2prproj.model/1`, see `src/avid2prproj/model.py`) is the
  contract between the **Avid reader** and the **Premiere writer**. Each side may be written
  in any language as long as it produces or consumes this JSON, with the same strict validation.
- **Long-term target: everything in Rust.**
- **Avid reader:** written in Rust from the start, from the pinned Avid knowledgebase findings
  only. No Filmcraft code.
- **Premiere writer:** stays in Python for now on our fork of py_premiere (ADR-0002). It is
  ported to Rust later as **one coherent rewrite, not piecemeal**, when there is a concrete
  reason. The User Premiere read-backs and fixture comparisons accumulated by then are the
  acceptance suite: the Rust writer must produce semantically identical projects for the same
  models before it replaces the Python one.
- **Do not mix languages inside one component** (no FFI between the Python writer and Rust).

## ADR-0002: py_premiere is our fork (2026-10-14)

`vendor/py_premiere` started as an unmodified copy of upstream `adb9e57` (MIT). It is now our
fork: change it directly where that is cleaner than working around it. Every change is
recorded in `vendor/FORK.md` (what, why, upstream base), so upstream improvements can still
be merged and the later Rust port knows exactly what we depend on. The MIT copyright notice
stays with the code.

## ADR-0003: The Rust writer (`prproj`) replaces the Python writer (2026-10-20)

Decided with the User after the port (plan: phases 0-5).

- `prproj/` is a standalone Rust crate: the py_premiere port (XML core, builders, graph
  operations) plus the converter's Premiere logic (`prproj::convert`). Std-only except `flate2`
  against the system zlib (byte-identical gzip with Python's on macOS; elsewhere only the XML is
  guaranteed). Command line: `prproj-write MODEL.json OUTPUT.prproj [--bin-mode …]
  [--match-avid-default-pan] [--seed SEED]`.
- Acceptance: byte-identical output to the Python writer on the golden corpus
  (`scripts/make_golden.py`, 23 models, seeded IDs), 222 option variants and 90 refusals
  (`prproj/tools/golden_sweep.py`), ~28k fuzzed models in the Phase 4 review; User Premiere 26.5
  read-back of Rust-written projects (stereo builds, markers/per-edit copies/adjustment layers,
  pan and gain, speed) on 2026-10-20.
- The standalone Avid reader remains separate. The JSON boundary in ADR-0001 was
  retained for that API; ADR-0005 supersedes it for normal Avid→Premiere input.
- The Python writer (`src/avid2prproj` + `vendor/py_premiere`) is kept as the reference oracle:
  any change to Premiere output is made in both, and the golden corpus must stay identical, until
  the User decides to retire it. Known shared bugs are listed in docs/ROADMAP.md.

ADR-0003 addendum (2026-10-20): the port now covers all of py_premiere, not only the converter's
part: read/object-model API (`prproj::model`, 185 projects and 21,035 edit operations identical
to Python), masks/text graphics/MOGRT (`build::mask/graphic/mgt`), captions (`prproj::caption`),
media import (`prproj::probe`, `import_files`, proxies, subclips, merged/multicam creation;
~208,000 probe calls identical), FCPXML export and the inspect/compare/validate/visualize tools
(`prproj-*` binaries; 2,690 cases identical). Known differences are listed per section in
`prproj/README.md` (Python-only TypeErrors, unbounded ints, help-text wording). Python stays the
oracle; `cargo test --release` (161 tests) checks every part against it.

## ADR-0004: More programs go through one neutral graph (2026-10-26)

Decided with the User (plan "structure for adding more editing programs", approved 2026-10-26),
adopting Codex's proposal in AVB-PORT-COORDINATION.md. Details: [GRAPH-MODEL.md](GRAPH-MODEL.md).

- **Hub and spoke.** Each program gets one reader into `anynle.graph/1` and one writer out of
  it. Choices that depend on the program pair live in a mapping policy (`crates/mapping`), never
  in the graph. Program-native data without portable meaning goes in adapter-owned sidecars.
  What is not converted is a loss with its object and reason.
- **`avid2prproj.model/1` stays frozen** as the working Avid → Premiere boundary. It is not
  renamed or upgraded, and the Python oracle stays on it. `crates/bridge` projects it to the
  graph and back; acceptance: 37/37 golden models equal and byte-identical.
- **Rational time.** The graph uses exact fractions of a second, not Premiere ticks. Going back
  to model/1, a time off the tick clock is refused, not rounded.
- **Layout.**
  - The new crates are a separate Cargo workspace in `crates/` (graph, bridge, mapping, app).
    `avid-reader/` and `prproj/` stay standalone, keeping their own targets and release profiles
    and leaving Codex's worktrees undisturbed.
  - No folder renames while Codex ports AVB.
  - `any-nle convert` (crates/app) is the single entry point; its output is byte-identical to
    `avid-reader model | prproj-write` (`crates/tools/compare_pipelines.zsh`).
- **Next programs**, in order:
  1. FCPXML: a graph writer (built 2026-10-27, `crates/fcpxml`). It is an optional export only.
  2. Resolve, **native `.drp`** (User 2026-10-27: Avid -> Resolve goes straight to Resolve's own
     project, never through FCPXML or another interchange file; ROADMAP).
  3. Premiere → Avid, which needs a complete AVB writer (Codex milestones 3-4) and Avid
     read-back.
- **Every pair is direct.** Interchange formats (FCPXML, AAF, OTIO, EDL) are offered as extra
  exports, never as the intermediate between two programs; the graph is the only intermediate.
- **Open:** retiring the Python oracle; routing the Avid reader straight into the graph (needs
  Codex's agreement).

### Selected-bin extraction foundation (2026-10-31)

`avid_reader::extract::extract_project_paths(&[PathBuf])` reads the ordered selection one bin
at a time. `extract/project.rs` compiles owned conversion summaries, explicitly drops each
`AvbFile`, then resolves cross-bin source routes. Summaries contain typed source ranges,
master facts, markers/transitions and a deferred sequence operation list; no decoded `Node`,
object table or source-file buffer survives compilation. Supported effect parameter/curve
data is retained where conversion needs it. Bin-local shared route/effect facts are memoized.

The deferred operations preserve modifier scope when a source resolves to several events,
motion/matte cardinality checks, and the order of diagnostics. Local master extraction keeps
its previous last-item lookup; cross-bin resolution keeps the first full-MobID occurrence.
Master copies remain separate and sequence order follows the selected bins and bin items.
The eager `extract_project(&[&AvbFile])` API remains available through the same compiler.

Both CLIs use the path API after input-path deduplication. A `ProjectReadError` carries the
failing path and underlying decoder error, and extraction completes before media scanning
or output creation. This does not add folder traversal, new Avid routing/effect semantics,
whole-project constant memory, or a different graph/model schema. Whole-file decoding,
retained summaries, final output allocation and the app's 16 MiB model limit remain relevant.
Independent comparisons use the frozen pre-change reader, not only the shared eager API.
See validation and capped runs.

### Adding a program (checklist)

1. **Research authority.** Create `knowledge/<program>/` with the same finding, provenance and
   hash rules; record third-party code licences first (no GPL/AGPL in the converter).
2. **Fixtures.** Get one small User file per feature in `converter/local/to-test/`
   (generate-then-confirm).
3. **Reader → graph** (or **graph → writer**) as its own crate in `crates/`. Unconverted parts
   are losses; native-only data goes in its sidecar; nothing pair-specific.
4. **Policy.** If the new pair needs choices, add a policy in `crates/mapping` with defaults and
   a preview.
5. **Read-back.** The User opens the result in the real program, a few checks at a time. Parser
   round trips are not read-back.
6. **Record it.**
   - A row and column in [FEATURE-MATRIX.md](FEATURE-MATRIX.md).
   - CONV pins in `research-dependencies.json`.
   - DECISIONS-EXPLAINED entries.
   - A ROADMAP section.


## ADR-0005: Qualified source graph and explicit destination preparation (2026-11-05)

User-approved central translation hardening and source-graph migration.

- Normal Avid→Premiere input goes through `avid-reader` owned source witnesses,
  `avid-adapter`, `anynle.graph/2`, then `premiere-adapter::PreparedPremiere`.
  Source extraction depends on no destination model/writer. Unknown facts remain
  explicit; source interpretation is not complete native reconstructability.
- Preparation borrows source facts, resolves transitions before trimming lanes,
  assembles compatible picture/audio spans, and keeps marker/diagnostic ownership.
  Model/1 remains the typed writer API. The normal source path constructs no legacy
  model JSON or graph/1 intermediate.
- Legacy graph/1, standalone reader/model JSON and Python oracle remain differential
  interfaces. FCPXML stays on its explicit held legacy profile. Color preview reads
  either source version without timeline preparation.
- Ordinary media clocks retain the old fallback rules from retained raw tuples.
  A qualified, direct 23.976 non-drop sequence TC1 sets Premiere's sequence zero
  point from its frame count and exact edit rate. Other sequence origins retain
  the compatibility fallback. Edit positions and media timecodes use their own
  clocks. The opt-in rewrap media clock profile keeps its separately scoped rules.
- Build and validate one encoded Premiere document before creating media. Plan
  asset names before writes; never rebuild the document or consume another UUID
  stream after media execution. Publish new projects without replacement and report
  partial/published/cleanup/receipt outcomes accurately.
- Acceptance requires frozen references, required fixture denominators, explicit
  correction classification, memory measurements, review and a final canonical
  double-check. Automated checks do not qualify newly changed native output.

Details, limits and provenance: [source-graph guide](SOURCE-GRAPH-MIGRATION.md) and
correction ledger.
