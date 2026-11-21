# avid-reader

Reads Avid Media Composer bins (`.avb`) for the avid2prproj converter.

    cargo build --release
    avid-reader dump BIN.avb                       # pyavb-shaped mob dump, for differential tests
    avid-reader mxf FILE.mxf                       # package UMIDs from an MXF header
    avid-reader sequences BIN.avb...               # cross-bin sequence events and problems
    avid-reader model BIN.avb... --media DIR [--media DIR...] [--project NAME] [--bin-mode separate|merged|grouped]

`model` writes the converter's `avid2prproj.model/1` JSON (stdout; problems on stderr). Master
clips are relinked to OP-Atom MXF files by identity: each bin file source mob's UMID must equal
exactly one MXF file package UID found under `--media` (header partitions only are read).

Scope today: master clips, cross-bin sequence traversal, subclip/group references **inside
sequences**, selected metadata and marker fields, selected audio/effect translations, and
MXF identity relinking. The current converter model is `avid2prproj.model/1`; it is not a
general multi-NLE project graph. Subclips and groups as independent bin items, many
effects and keyframe evaluations, MDB/PMR database linking, complete AVB writing, and
whole-project streaming import remain open. The model builder reports losses; successful
object decoding does not imply faithful conversion or Avid playback equivalence.

The pyavb-port inventory is `pyavb-parity.json` (source pinned in NOTICE). Check its
class-ID and source-hash status with `python3 tools/check_pyavb_parity.py --pyavb
/path/to/pinned/pyavb`; omit `--pyavb` to check the Rust declarations alone. Statuses
mark decoder presence, not verified semantics. The cross-format coordination draft is
`../docs/AVB-PORT-COORDINATION.md`.
The separate `pyavb-feature-parity.json` audits the public/core API beyond class IDs,
including writing, object construction, copying, MobIDs and curve evaluation. Run
`python3 tools/check_pyavb_features.py --pyavb /path/to/pinned/pyavb`; see
`../docs/AVB-FULL-PYAVB-PORT-ROADMAP.md` for scope, uncertainty and work order.

`AvbIndex` is an additive, lazy container index for lossless preservation work.
It owns one copy of the original bin bytes and records each object's exact chunk and
payload offsets without decoding the object. It can return unknown chunks and trailing
bytes byte-for-byte. It is **not** a writer and cannot make reference-changing edits safe.
The existing `AvbFile` importer remains unchanged. `AvbDocument` provides the
stable-ID whole-file writer described below. Set `PYAVB_TEST_FILES` to the pinned
pyavb `tests/test_files` directory to compare the index with the eager decoder on all
14 upstream full-bin fixtures.
The writer safety boundary and an ID-stable candidate strategy are documented in
`../docs/AVB-WRITER-BOUNDARY.md`.
`writer_io` now provides bounded primitive serialization and fixed byte-vector
parity with pinned pyavb `ioctx.py`; see `../docs/AVB-WRITER-PRIMITIVES.md` for
the big-endian f64 conflict and other unresolved write semantics. On its own,
it does not serialize classes or establish Avid write compatibility.
`bin_writer::serialize` now writes bounded typed payloads for ABIN, BINF,
ASET, BVst and ATTR using existing object-reference IDs. Pinned pyavb samples
byte-roundtrip for 14 ABIN, 3 BINF, 17 BVst and 1,556 ATTR payloads; ASET
has a synthetic inherited-write oracle because pyavb does not register its
`Setting` class and no real ASET fixture is available. These are payload
proofs only. The reader normalizes strings and discards BVst's descriptor
prefix, so noncanonical source bytes may not roundtrip. The document layer
now allocates objects and writes whole files, but Avid read-back remains open.
The synthetic ASET/ATTR oracle bytes can be regenerated with
`python3 tools/generate_bin_writer_oracles.py /path/to/pinned/pyavb`.
The first typed payload writers cover `COMP`, `CLIP`, `SCLP`, `SEQU`, `FILL`,
and `TCCP` with original-only fields supplied explicitly. See
`../docs/AVB-COMPONENT-WRITERS.md` for exact fixture results and gaps.
`misc_writer::serialize` writes bounded APOS, ABOB, DIDP, MCBR, MCMR and
TMBC payloads with existing MobIDs/references. Exact pinned chunk checks
cover DIDP, MCBR, MCMR and TMBC; 78 additional MCBR objects from 14 full
bins roundtrip byte-for-byte. APOS/ABOB have deterministic pyavb write
oracles (`tools/generate_misc_writer_oracles.py`) but no real pinned chunks.
MPGP now has a bounded writer, but its reader discards a leading-discard count,
so edits may need an original payload and can be rejected. These payload
checks do not establish full-file or Avid write compatibility.
`curve_writer` adds bounded `PRCL`, `CTRL`, and `ECCP` payload serialization;
see `../docs/AVB-CURVE-WRITERS.md` for source-only fields and round-trip limits.
The `mobid` helpers add pinned pyavb URN parsing, half-swapped material
handling, byte-array interchange and deterministic ID construction; see
`../docs/AVB-MOBID-HELPERS.md` for source parity and limits.
`experimental_writer::patch_cmpo_name_same_length` now makes an in-memory candidate
copy with only one parsed, fixed-length CMPO name span changed. The separate example
`cargo run --example patch_cmpo_name -- INPUT.avb OBJECT_ID NEW_NAME OUTPUT.avb`
creates a new file without overwriting an existing path. This is a parser-level
proof only; it has not been opened in Avid.
`patch_cmpo_name_variable_length` also rebuilds just one known CMPO chunk while
preserving every other chunk and object ID. It is a separate parser-level proof,
not Avid-validated or a general writer. The Rust crate now includes bounded
`writer_io` primitives, `AvbIndex::decode_object` for uncached one-object reads,
cycle-safe decoded-reference traversal, pyavb-style read-only bin navigation,
MobID URN/byte helpers, all pinned numerical interpolation functions, and
numeric PRCL value/offset-map evaluation. These APIs run without Python; Python
and pyavb are used only to generate/check research fixtures and attribution.
`parameter_registry` exposes the 735 UUID-to-name hints in pinned py-avb's
`parameter_uuids.py` through typed UUID lookup and its unknown-UUID fallback.
Names are not unique or independently verified Avid parameter meanings. Check
the generated Rust table against the pinned source with
`python3 tools/generate_parameter_registry.py /path/to/pinned/pyavb --check`.
`id_stable_writer::assemble_existing` can rebuild a copy from complete replacement
chunks at unchanged object IDs, with class/size checks and parser validation; it
does not create a new bin or prove Avid compatibility.
`AvbDocument` adds an editable existing-bin layer, deterministic empty-bin
construction, typed component/mob constructors, and same-byte-order graph copying.
It preserves unchanged chunks, uses bounded class writers for modified objects,
checks known references, and rejects unsupported edits. See
`../docs/AVB-DOCUMENT-API.md` for its exact source-parity proof and limits.
`document_extra` adds explicit reference lookup and transactional reference-list
edits, plus verified no-argument defaults for 16 factory classes. `sequence_nav`,
`semantic_helpers`, and `semantic_extra` expose bounded sequence, track, curve,
timecode and locator queries; their pinned py-avb oracles do not imply Avid
render or media-link behavior. `primitives_extra` keeps stored timestamps raw,
with explicit UTC conversion, and adds typed MobID dictionary defaults and
OS UUIDv4 generation. Python's host-local datetime conversion is deliberately
not reproduced.
The unified payload dispatcher covers all 66 registered py-avb classes and
passes exact round trips for all 79 pinned chunks and all decoded objects in
14 pinned full bins. The field ledger and optional hash-pinned local-corpus
gate expose missing variants and representation differences; these should not
be read as general Avid write compatibility.
`pyavb-feature-parity.json` tracks these capabilities separately from the 70
class-ID reader inventory, including uncertain and missing behavior.

Testing: `cargo test`; differential check against pyavb with `tests/compare_oracle.py`
(see the header of that script). Historical results recorded 2026-10-14: the User's fight
bin matched pyavb field-for-field (57 objects); 2,029 `.avb` files in the Avid KB's
`avid tests` decoded; a 475-bin stratified sample matched pyavb apart from effect classes
then unsupported. These historical counts do not automatically validate later code.
For the 2026-10-20 `PRCL`/`CTRL`/`ECCP` and `BINF`/`BVst` reader batches, tests include
ten pinned pyavb chunks, synthetic malformed inputs, 14 upstream full bins, 126
non-Phase-13 local bins, and a deterministic 50-bin Phase 13 sample below 20 MB.
The sampled path/size/hash manifest is in ignored local build storage. This does not
re-run the older 2,029-bin census or validate Avid's displayed column semantics.
The media descriptor batch adds 11 typed readers, compared with 17 pinned upstream
chunks across ten classes and a synthetic `DATD` oracle record. The same deterministic
190-bin smoke manifest was rechecked after integration: zero hash changes or read
failures. These are read-path results only.
The reader also decodes legacy ASPI, EQMB, REPT and STRB trackgroups. Six pinned
pyavb chunks and bounded malformed synthetic payloads cover their stored fields;
14 upstream full bins still dump successfully. Multi-plugin ASPI payloads are
accepted by count but remain untested, and no Avid read-back or writer validation
has been done for these classes.
The next misc batch decodes CCFX, GRFX, MPGP and SHLP at the pyavb field boundary.
CCFX, GRFX and SHLP still carry opaque effect bytes; this does not interpret color
correction or shape semantics. Three classes have pinned chunks; GRFX has only a
synthetic bounded case. Five tracker classes now have bounded typed readers while
retaining their raw bytes for the existing specialized extractor. At this revision
all 70 pinned pyavb-declared IDs have typed readers (66 are runtime-registered
in pyavb). `FXPS` now decodes its bounded
parameter list while keeping `user_param` opaque; it and six other typed classes
also retain raw bytes for specialized extraction. Reader presence is not full
format or writer parity; a future tracker layout could still fail strict typed
parsing. Two pinned FXPS chunks and the 190-bin smoke sample passed this batch.

`src/interpolation.rs` ports all 13 numerical functions from pinned pyavb
`interpolation.py`. Its 163 synthetic oracle cases cover scalar curves, roots,
handle adjustment, tangents, interpolation and integration. Exceptional upstream
paths and non-finite/oversized inputs return explicit errors. These functions
are not yet connected to `ParamClip`/speed-map evaluation in the converter,
and pyavb parity does not prove Avid render parity. Regenerate the checked-in
oracle values with `python3 tools/generate_interpolation_oracle.py
/path/to/pinned/pyavb > tests/fixtures/pyavb_interpolation.tsv`.

`BinNavigator` adds read-only pyavb-style `mobs`, `toplevel`,
`compositionmobs`, `mastermobs` and `find_by_mob_id` queries over the decoded
bin's own stored items. It preserves item order and duplicate references;
duplicate-ID lookup returns the last item. A null, dangling or malformed item
is an explicit error rather than silently disappearing. Tests compare pyavb's
counts and representative IDs on all 14 pinned upstream bins when
`PYAVB_TEST_FILES` is set. Cross-bin search, hidden/reference-clip UI display,
and `add_mob` mutation are outside these helpers.

Attribution: see NOTICE (pyavb, MIT, Mark Reid).
