# prproj

Rust port of py_premiere's `.prproj` document core (Rust port plan, Phase 1). The Python writer
(`src/avid2prproj` on `vendor/py_premiere`) is the oracle: this crate must reproduce its bytes
exactly. Std-only apart from `flate2` (zlib backend).

## Modules

| Module | Python source | What it does |
|---|---|---|
| `xml::tree` | `xml.etree.ElementTree` | Arena DOM: `Tree` + `NodeId` handles; tag, ordered attributes, `text`, `tail` kept verbatim (`None` vs `""` distinct). Detached nodes for fragments, `deep_copy`, `iter`, `find`/`findall`/`findtext` (ElementPath subset). |
| `xml::parser` | `ET.fromstring` (expat) | Builds the same tree ElementTree builds (EOL and attribute normalization, entity/char refs, comments/PIs dropped, CDATA as text). Namespaces and DOCTYPE are refused. |
| `xml::serializer` | `xml/serializer.py` | Premiere's output style: fixed prolog/epilogue, tabs and newlines kept, `&#10;`/`&#13;` in leaf text, `&quot; &#9; &#10; &#13;` in attributes, `<Tag/>` for empty elements, mixed content refused. |
| `xml::gzip_io` | `xml/gzip_io.py` | Header kept verbatim (optional fields too); raw deflate level 6 via system zlib; CRC/size trailer. |
| `xml::mutations` | `xml/mutations.py` | `indent_tree`, `build_leaf`, `append_uniform_child`, `append_pair`, `append_child`, `append_leaf`, `set_elided_flag`, `remove_child`, `insert_before`, `insert_leaf_before`. |
| `xml::document` | `xml/document.py` | `PremiereDocument`: `from_bytes` (with the byte-identity self-check), `by_object_id`/`by_object_uid`, `resolve`, scoped inline IDs, `next_object_id`, `add_object`, `attach_object`, `remove_object`, `payload`, `payload_stored`, `index_payloads`, `ReferenceIndex`, `owned_objects`, `to_xml_bytes`, `to_bytes`, `save`. |
| `splice` | `models/project.py` `_splice_fragment` | Renumber a fragment's ObjectIDs from the document counter, remint ObjectUIDs, rewrite in-fragment refs, skip scoped subtrees, attach. |
| `payload` | `avid2prproj/lumetri.py` | `binary_hash` (Premiere-shaped hash) and `set_payload` (store-once-safe overwrite). |
| `probe` | `models/media_import.py` readers, `models/project.py` `_probe_movie` | Media-file probes: still sizes, AVI/MP4/MOV/MXF movie headers, WAV/AIFF/MP3/AAC/WMA/M4A audio headers, with Python's results and errors (see Media import). |
| `ids` | `avid2prproj/determinism.py` | `UuidSource` trait; `RandomUuids` (v4) and `SeededUuids` (the exact `seeded_ids(seed)` sequence). |
| `base64`, `hash`, `pystr` | stdlib | Python-exact base64 (non-strict decode), MD5/SHA-256, Python `isspace`/`strip`. |

Usage sketch:

```rust
use prproj::{xml::parse_prproj, splice::splice_fragment, SeededUuids};
let mut doc = parse_prproj("in.prproj")?;              // refuses what it cannot round-trip
let mut ids = SeededUuids::new("any-nle-golden");      // or RandomUuids::new()
let frag = doc.tree.parse_fragment(template_text)?;    // ET.fromstring
let spliced = splice_fragment(&mut doc, frag, &mut ids)?;
let item = spliced.get("ClipProjectItem").unwrap();
doc.save("out.prproj")?;
```

Thread ONE `UuidSource` through a whole write so the sequence advances in the same order as
Python's global `uuid.uuid4`. Like Python, the object table is updated only by
`add_object`/`attach_object`/`remove_object`, and the payload cache must be dropped with
`invalidate_payloads()` after direct edits of payload text or hashes.

### Differences from Python's object model (read before porting graph code)

- **One parent per node.** ElementTree lets the same element sit under two parents (an append
  does not detach). Rust nodes have exactly one parent; `Tree::append`/`insert` panic on a node
  that still has one, so `detach` first. Python's `_splice_fragment` leaves every spliced element
  in the fragment as well as under `<PremiereData>`; `splice_fragment` moves them, leaving the
  fragment empty. Python code that appends one element to two places must deep-copy here.
- **Snapshots.** The scoped-ID set (inline definitions under nested ObjectID/ObjectUID carriers)
  is computed once in `PremiereDocument::new`, like Python's `_scoped`: elements attached later
  are never scoped. A `ReferenceIndex` is a snapshot of the refs at construction; rebuild it after
  edits. `by_object_id`/`by_object_uid` follow only `add_object`/`attach_object`/`remove_object`.
- **ObjectID counter.** Python's ints are unbounded; here the counter is `u64`. A top-level
  ObjectID of 2^64-1 or more still parses and saves, but `next_object_id`, `add_object` and
  `splice_fragment` (when the fragment has ObjectIDs) return an error instead of minting.
- **Nesting depth.** Python's serializer recurses once per level, so `from_bytes` raises
  `RecursionError` on deep files. Rust refuses the same files: an element at depth d needs d
  frames plus 3 (leaf data text), 2 (formatting-only leaf text, or attributes) out of a budget of
  996 (`PY_SERIALIZER_FRAME_BUDGET`: recursion limit 1000 minus the frames of a module-level
  `from_bytes` call), measured on Python 3.14.4. A Python caller deeper in the stack refuses
  slightly shallower files. Everything else in the crate (parser, serializer, deep copy, indent,
  splice, paths) is iterative and has no depth limit.
- **Names.** The parser accepts the non-ASCII name characters expat accepts, from tables measured
  from expat itself (`tools/make_name_tables.py` -> `src/xml/name_tables.rs`; expat uses the
  older XML 1.0 Appendix B classes, so e.g. U+2070 and astral characters are not name
  characters). The XML declaration is validated like expat's `doParseXmlDecl`.

## Byte checks

`tools/make_expected.py` runs the real Python code and writes `tests/data/`:

```sh
cd converter   # or a worktree
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/make_expected.py   # --no-corpus to skip the verdict scan
```

The oracle interpreter is **Python 3.14** (3.14.4, zlib 1.2.12, expat 2.7.4; recorded in
`tests/data/oracle_python.txt`). The tool exits if run under another major.minor: base64's
non-strict decode changed (3.14 no longer stops at a complete pad) and the RecursionError depth
is interpreter-specific.

- `py_verdicts.tsv`: for every corpus file (py_premiere fixtures, golden outputs,
  `local/tested`, `local/to-test`) Python's `from_bytes` verdict, whether `to_bytes()` equals the
  file, `next_object_id`, sha256 of the XML and of Python's `to_bytes()`. `tests/corpus.rs`
  requires Rust to agree on all of it (gzip bytes for every file, including those Premiere
  compressed differently).
- `refusal_cases.tsv`: small crafted documents (CRLF, quotes, CDATA, entities, namespaces,
  mixed content, nesting depth around the RecursionError threshold, run in a fresh interpreter)
  with Python's accept/refuse verdict. `parse_cases.tsv`: `ET.fromstring` verdicts for name
  characters and XML declarations.
- `mutation_cases.tsv`: every `xml/mutations.py` helper run by Python; `tests/oracle_extra.rs`
  replays the same calls. `base64_cases.tsv`, `deflate_vectors.tsv`: Python 3.14 decode results
  and `zlib.compressobj(6, DEFLATED, -15)` output.
- `splice_expected.prproj`: the adjustment-layer fragment (`markers.py`) spliced into
  `01_empty.prproj` with seeded UUIDs; compared as full gzip bytes.
- `set_payload_expected.xml`, `payloads.tsv`, `binary_hashes.tsv`, `owned_objects.tsv` (46 seed sets over 5 documents),
  `seeded_uuids.txt`: the other oracles.

Tests that need a corpus file skip when it is absent (set `PRPROJ_CORPUS_BASE` to relocate).
`cargo test --release --test corpus -- --nocapture` prints the round-trip summary.

Deflate: the `zlib` backend links the system libz (macOS `/usr/lib/libz.1.dylib`, 1.2.12, the
same library Python's `zlib` uses); `tests/oracle_extra.rs` asserts the linked `zlibVersion()`
equals the oracle's and checks known deflate vectors. `zlib-rs` was tried and gives different
deflate bytes (XML identical, gzip not), so it is not used. **On non-macOS platforms** `libz-sys`
may link or build a different zlib: only the decompressed XML identity is guaranteed there, and
the gzip byte comparisons in the tests are skipped (`GZIP_BYTES_GUARANTEED`).

## Graph operations (`model`, Phase 3)

`prproj::model` ports the py_premiere object-model operations the converter uses: one
`Project` (document + item/sequence/track tables) with copyable handles (`ItemId`,
`SequenceId`, `TrackId`, `TrackItemId`). Every call that mints a UUID takes the write's single
`&mut dyn UuidSource` and draws in Python's order.

```rust
use prproj::{model::Project, build::time::Time, SeededUuids};
let mut ids = SeededUuids::new("seed");
let mut p = Project::new(&mut ids)?;                  // py_premiere.new()
p.set_path("/out/Show.prproj");                       // project._path = out_path
let bin = p.add_bin(p.root_item().unwrap(), "Clips", &mut ids)?;
// items: build with build::media / opatom, then p.register / p.register_uid,
// p.imported_media.push(media), p.import_target = Some(bin), p.attach_imported_item(...)
let seq = p.add_sequence("Edit", "avid2prproj-1080p24", None, &mut ids)?;
p.set_zero_point(seq, Time::new(3600 * 254016000000))?;
let v = p.add_clip(p.video_tracks(seq)[0], video_item, Time::new(0), &mut ids)?;
p.set_in_point(v, Time::new(..))?; p.set_out_point(v, ..)?; p.set_end(v, ..)?;
p.link(seq, &[v, a])?;
p.add_sequence_marker(seq, &MarkerSpec::comment("Note", Time::new(0)), &mut ids)?;
p.add_transition(a_track, a, "Constant Power", true, Some(duration), None)?;
p.save("/out/Show.prproj")?;                          // refuses to overwrite; atomic
```

Byte checks: `tools/expected_model_ops.py` runs scripted scenarios (empty, bins + labels,
sequences on three presets with zero point, OP-Atom items placed/trimmed/linked, speed changes,
sequence and clip markers with colours, transitions, refiling into "Processed Clips", media at
timecode 0 so cut transitions straddle the cut, transitions between speed-changed clips on
29.97, V2/A3 placements, set_start/set_end edge cases, panel-marked items, 1 and 5 video
tracks, a nested sequence on V5/A4, error parity, and operations on the parsed Premiere fixture
`07_transitions.prproj` when the corpus is present) through the real Python API under
`seeded_ids` and saves to `/tmp/prproj-model-ops/`. Messages of operations Python refuses go to
`<case>.errors`. `tests/model_ops.rs` replays them and compares gzip bytes, XML and error
messages.

Phase 4 helpers beyond the Python model API: `parse_sequence`, `push_item` + `ItemData::new`,
`track_mut`, `item_extra` (`_channel_groups`, `_stereo_nest`), `sort_track_clips_by_start`
(`writer._order_track_items`), `refile_item` (the order `combined._move_to_processed_clips`
uses), `narrow_merged_channel`, `share_file_identity`, `media_element`, `track_name`.

Save re-derives imported
media's RelativePath against the save directory, so the Rust test saves into
`/tmp/prproj-model-ops-rs-<pid>-<case>/` (same depth; on macOS both resolve under
`/private/tmp`, which is where the expected data were made).

```sh
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_model_ops.py
```

## The read / object-model API (`model`)

`prproj::model` also ports everything py_premiere exposes for opening a project and
walking, querying and editing it. Handles: `ItemId`, `SequenceId`, `TrackId`, `TrackItemId`,
`ComponentId`, `CaptionTrackId`, `CaptionId`; markers, transitions and parameters are their
objects (`NodeId`). Getters return `Result` where Python can raise; `Error::py_class()` names
the Python exception (`ValueError`, `TypeError`, `KeyError`, `IndexError`,
`NotImplementedError`, `AttributeError`, `FileExistsError`, `UnicodeDecodeError`, ...) and the
message is Python's.

```rust
use prproj::model::application;
let mut app = application::parse("Show.prproj", None)?;      // py_premiere.parse
let p = app.project_mut();
for item in p.walk(p.root_item().unwrap()) { println!("{}", p.tree_path(item)); }
let seq = p.sequences_named().by_name("Edit")?;               // project.sequences["Edit"]
let clip = p.track_clips(p.video_tracks(seq)[0])[0];
let opacity = p.components(clip).by_name("Opacity")?;
let param = p.component_properties(opacity).by_name("Opacity")?;
model::component::set_param_value(&mut p.doc.tree, param, &ParamInput::Num(Num::Float(50.0)))?;
p.set_blend_mode(clip, BlendMode::Multiply)?;
p.save("Show v2.prproj")?;
```

| Python | Rust |
|---|---|
| `py_premiere.parse(path, preferences_path=)`, `new(...)`, `Application.project` | `model::application::{parse, new}`, `Application::project{,_mut}`, `repr` |
| `parsers/application.py` (active sequence), `parsers/project.py` (`_assign_node_ids`), `parsers/sequence.py`, `parsers/component.py`, `parsers/marker.py`, `parsers/caption.py` | `Project::from_document` (+ `parse_caption_tracks`, `parse_chain`) |
| `Project.name/path/root_item/document_id/active_sequence/sequences/sequence_presets()/save/save_in_place/__repr__` | `name`, `path`, `root_item`, `document_id`, `active_sequence`, `sequences`/`sequences_named`, `Project::sequence_presets`, `save`, `save_in_place`, `repr`; `preferences()` |
| `ProjectItem.name` (get/set), `type`, `is_sequence`, `node_id`, `tree_path`, `in_point`, `out_point`, `children`/`__len__`/`__getitem__`/`__contains__`, `walk()`, `media_path` | `item_name`/`set_item_name`, `item_type`, `is_sequence_item`, `node_id`, `tree_path`, `item_in_point`, `item_out_point`, `item_children` (NamedList), `walk`, `item_media_path` |
| `ProjectItem.color_label`, `scale_to_frame_size`, `start_time` (get/set) | `color_label`/`set_color_label`, `scale_to_frame_size`/`set_scale_to_frame_size`, `item_start_time`/`set_item_start_time` |
| `is_offline`, `generator_id`, `generator_type`, `is_adjustment_layer`, `is_multicam_clip`, `is_mgt`, `is_merged_clip`, `has_proxy`, `proxy_path`, `is_subclip`, `subclip_in_point`, `subclip_out_point`, `has_hard_boundaries` | same names (`model/item_props.rs`) |
| `ProjectItem.add_bin`, `remove_bin`, `remove_item`, `move_to`, `markers`, `add_marker`, `remove_marker`, `__repr__` | `add_bin`, `remove_bin`, `remove_item`, `move_item`, `item_markers`, `add_item_marker`, `remove_item_marker`, `item_repr` |
| `FootageInterpretation` (`frame_rate`, `alpha_usage`, `ignore_alpha`, `invert_alpha`, `field_type`, `pixel_aspect_ratio`, all get/set, repr) | `model::footage::FootageInterpretation` (`ParInput` for the PAR setter; `set_alpha_usage_int`/`set_field_type_int` reproduce Python's behaviour for a raw int, which never clears) |
| `Sequence.name` (get/set), `sequence_id`, `timebase`, `frame_size`, `audio_frame_rate`, `audio_channel_count`, `end`, `project_item`, `clips`, `video_tracks`, `audio_tracks`, `markers`, `caption_tracks`, `__repr__` | `sequence_name`/`set_sequence_name`, `sequence_id`, `timebase`, `frame_size`, `audio_frame_rate`, `audio_channel_count`, `sequence_end`, `sequence_item`, `sequence_clips`/`clips_named`, `video_tracks`/`audio_tracks`/`tracks_named`, `sequence_markers`/`sequence_markers_named`, `caption_tracks`, `sequence_repr` |
| `Sequence.zero_point`, `in_point`, `out_point`, `work_area_in`, `work_area_out`, `playhead`, `video_display_format`, `audio_display_format` (get/set) | `zero_point`/`set_zero_point`, `sequence_in_point`/`set_sequence_in_point`, ..., `video_display_format`/`set_video_display_format`, ... |
| `Sequence.add_marker`, `remove_marker`, `add_linked_clip`, `_link`, `_grow_source_duration` | `add_sequence_marker`, `remove_sequence_marker`, `add_linked_clip`, `link`, `grow_source_duration` |
| `SequenceSettings` (all fields, get/set, repr) | `settings_*` / `set_settings_*`, `settings_repr` |
| `Track.index/id/media_type/name/clips/transitions/__repr__`, `is_muted`, `is_locked`, `is_sync_locked` (get/set) | `track_index`, `track_id`, `track_media_type`, `track_name`, `track_clips`/`track_clips_named`, `track_transitions`/`transitions_named`, `track_repr`, `is_muted`/`set_muted`, `is_locked`/`set_locked`, `is_sync_locked`/`set_sync_locked` |
| `Track.add_clip`, `insert_clip`, `remove_clip`, `move_clip`, `add_transition`, `remove_transition` | same names |
| `TrackItem.name/start/end/in_point/out_point` (get/set), `duration`, `speed`, `is_speed_reversed`, `time_interpolation_type`, `project_item`, `media_type`, `type`, `__repr__` | `track_item_name`/`set_track_item_name`, `start`/`set_start`, ..., `duration`, `speed`, `is_speed_reversed`, `time_interpolation_type`, `track_item_source`, `track_item_media_type`, `track_item_type`, `track_item_repr` |
| `TrackItem.components`, `selection_components`, `blend_mode` (get/set, `_materialize_opacity`), `is_disabled` (get/set), `essential_sound_tag`, `tag_as_dialogue`, `time_remapping`, `set_time_remap`, `clear_time_remap` | `components`, `selection_components`, `blend_mode`/`set_blend_mode`, `is_disabled`/`set_disabled`, `essential_sound_tag`, `tag_as_dialogue`, `time_remapping`, `set_time_remap`, `clear_time_remap` (self-contained in `model/track_item_props.rs`) |
| `Component.display_name/match_name/properties/sub_components/track_item/__repr__` | `component_display_name`, `component_match_name`, `component_properties`, `component_sub_components`, `component_track_item`, `component_repr` |
| `ComponentParam.display_name/class_id/is_time_varying/value/keys/color/text/font_family/path/component`, `get_value_at_key`, `find_nearest_key`, `find_next_key`, `find_previous_key`, `get_interpolation_at_key`, `get_out_interpolation_at_key`, `get_spatial_tangents_at_key`, `set_keyframes`, value/color/text/font_family/path setters | `model::component::param_*` (tree reads), `set_param_value` (`ParamInput`), `set_param_color`, `set_param_keyframes`; `Project::param_text`/`set_param_text`, `param_font_family`/`set_param_font_family`, `param_path`/`set_param_path`, `param_component`; `PathVertex`, `SpatialTangents`, `ParamValue`, `decode_value`, `build_keyframe_string`, `pack_color`/`unpack_color` |
| `Marker.name/comments/start/end` (get/set), `type`, `guid`, `color_index` (get/set), `web_link_url`, `web_link_frame_target`, `set_type_as_*`, `__repr__` | `model::marker::*` |
| `Transition` (all properties, repr) | `model::transition::*` |
| `Caption.text/start/end/source_start/source_end/__repr__`, `CaptionTrack.id/index/captions/format/__repr__` | `caption_text`, `caption_start`, ..., `caption_track_format`, `caption_track_repr` (the captions port, `caption::model`; see Captions below) |
| `models/caption.py` read helpers (`is_payload_text`, `decode_caption_text`, `read_font_family`) and the splices `ComponentParam.text`/`font_family` use (`replace_payload_text`, `write_font_family`, `write_payload_element`) | `caption::flat` (shared with the captions and graphics ports) |
| `Preferences` (`path`, `get`, `get_int`, `get_bool`, `label_names`, `label_colors`, `label_default`, `still_frame_rate`, `still_default_out_ticks`, `load_default`, repr) | `model::preferences::Preferences` |
| `models/validators.py` (all validators), `time.validate_time` | `model::validators` (`NumberRule` constants, `Num`, `validate_string*`, `validate_one_of`, `validate_path` + `PathRule`, `validate_vector2`, `validate_color`, `validate_enum_value`) |
| `NamedList` | `model::named_list::NamedList` (`get`, `by_name` -> `KeyError`, `contains_name`, `index_of`, `keys_of`; derefs to the handle slice) |
| `enums/*` | `model::enums` (`value()`, `name()`, `from_value`, `from_name`) |
| `models/color.py`, `data/param_classes.py` | `model::color::Color`, `component::VERIFIED_VALUE_CLASS_IDS` |

Deliberately not ported in this module:

- Media import (`import_files`, `attach_proxy`, `create_sub_clip`, `create_merged_clip`,
  `create_multicam_clip`) is ported separately: see Media import below.
- Python `TypeError`s for values of the wrong Python type that Rust's types rule out (a string
  given to a number setter, ...). Where Python accepts "int or float or bool or list" the Rust
  setter takes `Num` / `ParamInput`, so those refusals are kept with Python's messages.
- Known edge differences: digits are ASCII-only where Python's `str.isdigit`/`\d` accept other
  Unicode digits; integers are `i64` (`i128` for packed colours) where Python's are unbounded;
  an unpaired surrogate cannot be a Rust string, so `validate_string`'s UTF-8 refusal cannot arise.

Byte checks (`tests/read_api.rs`, data from `tools/expected_read.py`):

- `dumps`: every corpus project (the py_premiere fixtures plus `converter/local/tested` and
  `local/to-test`, 185 when last generated) is described through the whole API (every property
  of every item, sequence, track, clip, component, parameter with keyframe probes, marker,
  transition and caption, NamedList lookups, reprs, raised errors) and must equal Python's dump
  line for line (`tests/data/read/dumps/*.txt.gz`; projects absent here are skipped; a
  mismatch writes both dumps to `$TMPDIR/prproj-read-rs-<pid>/dumps/`).
- `edits`: for each of the 80 fixtures a generated list of ~260 operations (every setter on every
  object, keyframes, colours, texts, paths, blend modes, footage overrides, sequence settings,
  markers, transitions, time remaps, clip add/insert/move/remove, bins, renames, plus refusals)
  is replayed under `seeded_ids("read_edits:<case>")`; each op's result or error, the dump
  afterwards and the saved gzip bytes must equal Python's (`tests/data/read/edits/`).
- `preferences` (this machine's Premiere prefs and crafted files), `validators` (verdicts and
  messages).

```sh
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_read.py
```

## The writer (`convert`, Phase 4) and `prproj-write`

`prproj::convert` ports the converter's Premiere logic (`src/avid2prproj/*.py`, one module per
file; see `convert/mod.rs`). The entry point:

```rust
use prproj::convert::{model::load_model, writer::write_project};
let model = load_model(Path::new("MODEL.json"))?;            // avid2prproj.model/1, Python's rules
let report = write_project(&model, "OUT.prproj", &mut SeededUuids::new("seed"))?;  // or RandomUuids::new()
// report.notes, report.unverified: the Python Report, same texts
```

`write_project` refuses an existing output and a missing folder, writes colour-corrected edits'
LUTs to `"<stem> LUTs/"` next to the project, and draws every UUID from the one source in
Python's order. Errors are `writer::WriteError` by Python exception class (`Model`,
`Unverified`, `Exists`, `NotFound`: the CLI's exit status 2; `Other`: a Python traceback).
`convert::combined` (merged / grouped / stereo-nest builds, `narrow_to_group`, `set_clip_pan`,
`declare_stereo_chain`, Processed Clips filing) and `convert::placement` (Premiere field order
of placements) are public for callers that build their own projects. Python object identity
(`id(a)`) is carried as indices into `clip.audio` (`combined::combined_order`).

CLI (same arguments and JSON report as `python -m avid2prproj`, plus `--seed`):

```sh
cargo build --release --manifest-path prproj/Cargo.toml
prproj/target/release/prproj-write MODEL.json OUTPUT.prproj [--bin-mode separate|merged|grouped] \
    [--match-avid-default-pan] [--seed SEED]   # SEED: Python's seeded_ids(SEED) sequence; default random
```

`--seed` is a Rust-only extra. Exit codes follow Python's: 0 written; 2 for usage errors and the
errors `avid2prproj` reports (`ModelError`, `UnverifiedShape`, `FileExistsError`,
`FileNotFoundError`, with Python's texts, e.g. `[Errno 2] No such file or directory:
'missing.json'`, `[Errno 17] File exists: 'out/o LUTs'`); 1 where Python crashes with a
traceback. Deliberate differences, where Python crashes (exit 1) and Rust reports a model error
(exit 2): a model path that is a directory, a model file that is not UTF-8, JSON integers of more
than 4300 digits. Tick arithmetic is exact (i128) and refused with a clean error (exit 1) when a
result leaves `i64` (only absurd models: ticks near 2^62, speed ratios near 2^40 or 2^62), where
Python would write the big number; nothing panics.

Byte checks (`tests/convert_writer.rs`):

- `golden_corpus`: every model of `converter/local/golden` (made by `scripts/make_golden.py`) is
  written with `SeededUuids::new("any-nle-golden")` to `local/golden/out-rust/<name>/` (same
  depth as Python's `out/`, so relative media paths match) and must equal Python's gzip bytes,
  XML, `.cube` files and report notes (`manifest.json`). The only allowed difference is the
  output location the writer embeds (the LUT path in the Lumetri Blob payload, UTF-8, and Look
  path payload, UTF-16LE, of `example-color`): it is substituted inside the decoded payloads,
  their BinaryHashes recomputed, and then the bytes must be identical. Skips without the corpus.
- Regression tests: `repr` against Python 3.14 for every non-printable range boundary
  (`tools/make_printable_table.py` generates `convert/printable.rs` from `str.isprintable` and
  `tests/data/convert_writer/py_repr_cases.tsv`), I/O errors, huge ticks, CLI usage/exit codes.
- `in_repo_models`: 11 small models from `examples/` and `tests/test_markers.py` (bin modes,
  stereo pairs, edit-marker copies, gain/volume overflow, pan, track level, crossfade, speed,
  sequence start), with Python's output in `tests/data/convert_writer/`:

```sh
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_convert_writer.py
```

Tools: `tools/golden_diff.py [NAME...]` prints the first differing XML line per golden model
(after `cargo test --test convert_writer`); `tools/golden_sweep.py` runs Python and
`prproj-write` on option variants of every golden model (3 bin modes with and without
`--match-avid-default-pan`, marker-conflict options, no bins, and refusals: 30 fps, tracks
beyond the sequence, existing output, missing folder) into the same path and compares bytes,
LUTs, reports and refusal messages.


## Masks, text graphics and Motion Graphics templates

| Python | Rust |
|---|---|
| `models/mask_builder.py` (`build_mask_params`, `build_mask_component`, `attach_sub_mask`, `build_selection_chain`) | `build::mask` |
| `data/mask_template.py` | `build::mask_template` (generated by `tools/make_mask_template.py`) |
| `models/component.py` `_stamp_next_component_number` | `build::mask::stamp_next_component_number` |
| `TrackItem.add_mask` (clip role, further masks as `01`, `02`, ... sub-components) | `Project::add_clip_mask` (`model::graphics`) |
| `Component.add_mask` (effect role) | `Project::add_component_mask(component_element)` |
| `models/graphic_builder.py` + `data/text_template.py` | `build::graphic` |
| `models/caption.py` `replace_payload_text` and its FlatBuffer walkers | `caption::flat` |
| `Track.add_graphic` | `Project::add_graphic` |
| `models/mgt_builder.py` (`Template`, every `build_*`) | `build::mgt` (small zip reader; JSON via `build::marker::Json`) |
| `Track.add_mgt`, `Sequence.import_mgt`, `Project._add_template_item`, `_tag_as_template`, `Track.remove_clip`/`_clips_within` (overwrite) | `Project::add_mgt`, `import_mgt`, `add_template_item`, `remove_clip` (`model::track_props`), `clips_within` |

Python wraps what the mask/graphic/MOGRT entry points create in `Component`/`ComponentParam`
objects; the Rust results (`MaskAdded`, `GraphicAdded`, `MgtAdded`) name the created objects
(ObjectIDs, elements) instead, for whoever ports the component model. Limits: a template's
numeric JSON fields that Python formats with `str`/arithmetic must be integers; `json.loads`
error texts for a malformed `definition.json` are this crate's.

```sh
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_masks_graphics.py
cargo test --release --test build_masks_graphics
```

17+ cases: builder parts, payload re-texting, and end-to-end documents (graphics, clip and effect
masks, MOGRT import with overwrite, error texts). MOGRT cases copy media under
`/tmp/prproj-mgt-oracle/<case>/` in both languages (absolute media paths are embedded).

## Captions (`caption`)

`prproj::caption` ports `models/caption.py`, `models/caption_builder.py`, `data/caption_template.py`
and `parsers/caption.py`, plus the entry points that use them. Hooks outside the module: a
`captions: CaptionStore` field on `model::Project`, the `DataTrackGroup` branch of
`Project::parse_sequence`, and `pub mod caption` in `lib.rs`.

| Python | Rust |
|---|---|
| `caption.py` module functions (`decode_caption_text`, `replace_payload_text`, `read_/write_style_float/color`, `read_/write_block_float/color`, `read_/write_font_family`, `read_/write_font_size`, `write_payload_element`, `_write_table_flag`, `is_payload_text`) | `caption::flat` (same names; flags as `write_style_flag` / `write_block_flag`) |
| `Caption` (all properties and setters) | `CaptionId` handle; `Project::caption_text`, `set_caption_text`, `caption_start/end`, `caption_source_start/end`, `caption_font_size`, `caption_font_family`, `caption_fill_color`, `caption_stroke_color/width`, `caption_tracking`, `caption_leading`, `caption_shadow_*`, `caption_background_*` (+ `set_caption_*`), `caption_repr` |
| `CaptionTrack` (`id`, `index`, `captions`, `format`, setter, repr) | `CaptionTrackId`; `Project::caption_track_id/index/format`, `captions`, `set_caption_track_format`, `caption_track_repr` |
| `Sequence.caption_tracks`, `Sequence.create_caption_track` | `Project::caption_tracks`, `Project::create_caption_track` |
| `Project._import_caption` (the `.srt` branch of `import_files`) | `Project::import_caption(path, target_bin, label, uuids)` |
| `caption_builder.py` (`parse_srt`, `build_caption_payload`, every `new_*`, `write_track_format`) | `caption::builder` |
| `caption_template.py` | `caption::template` (generated by `tools/make_caption_template.py`) |
| `parsers/caption.py` `parse_caption_tracks` | `Project::parse_caption_tracks` |
| `Color`, `CaptionFormat` | `caption::Color`, `caption::CaptionFormat` |

Python's exception class is part of the behaviour (a corrupt payload raises `ValueError`,
`struct.error`, `IndexError` or `UnicodeDecodeError` depending on where it is cut), so the caption
layer returns `caption::PyError` (class + Python's message), convertible to `prproj::Error`.
Setters take `f64`: Python's `int` vs `float` shows only in a validator message (`got -1` vs
`got -1.0`). `import_caption` takes the Captions label `(index, colour)` explicitly instead of
reading `Preferences` (not part of this port); `None` is the factory Mango label. Not ported:
the `import_files` dispatcher for other media types. SRT `\d` and `int()` follow Unicode decimal
digits (`tools/make_digit_table.py`); cue times beyond `i64` ticks are refused.

Byte checks (`tools/expected_captions.py` -> `tests/data/captions/`, `tests/captions.rs`): 2070
payload-function cases (built, fixture, edited, truncated and corrupt payloads; results and error
messages), every builder branch, `parse_srt` and `build_caption_payload`, two end-to-end projects
(skeleton + sequences + SRT import + caption tracks + styles, saved with seeded IDs) and scripted
edits of each caption fixture (29, 64, 80, 82), and a read dump of every corpus file with
captions. `tools/expected_captions.py` runs under Python 3.14 like the other oracles:

```sh
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_captions.py
```

## FCPXML export and the CLIs (`export`, `cli`)

| Python | Rust | Binary |
|---|---|---|
| `export/fcpxml.py` `export_fcpxml(project, path, sequence=None)` | `export::export_fcpxml(&project, path, Option<SequenceId>)` (writes the file); `export::export_fcpxml_bytes` (the bytes) | |
| `cli/inspect.py` `main(argv)` (`pr-inspect`) | `cli::inspect::main(&argv, &mut out, &mut err) -> i32` | `prproj-inspect` |
| `cli/compare.py` (`pr-compare`; `difflib.SequenceMatcher` ported inside) | `cli::compare::main` | `prproj-compare` |
| `cli/validate.py` (`pr-validate`; `shape_key`, `KNOWN_DIVERGENCES`, `--coverage`, `--report-json`) | `cli::validate::main` | `prproj-validate` |
| `cli/visualize.py` (`pr-visualize`) | `cli::visualize::main` | `prproj-visualize` |
| `argparse` | `cli::args` (unique-prefix abbreviations, `--opt=value`, `--`, negative numbers as values, exit status 2 usage errors) | |
| `int / int`, `Time.seconds` | `pynum::true_div` (correctly rounded; `Time::seconds` now uses it) | |
| `json.loads` | `convert::model::parse_json_python` (NaN/Infinity accepted, repeated keys last-wins, Python's error text) | |

The binaries take the Python CLIs' arguments and exit statuses (0/1, 2 for argparse errors; the
program name is `prproj-*`). An exception Python would not catch (a project py_premiere refuses,
a malformed ground-truth file) prints `Class: message` on stderr (Python prints a traceback
ending in that line) and exits 1, with everything printed before it kept, as in Python. Help
text is hand-written; argparse's usage wording is not reproduced byte for byte.

Known differences: `Path` handling is POSIX only (`py_path`); a ground-truth JSON of an exotic
shape (a list where an object is expected) raises the same exception class as Python in the
usual cases but not necessarily the same message; `json.loads` of lone UTF-16 surrogates is
refused and integers beyond 128 bits keep only their sign; recursion over trees deeper than
Python's recursion limit does not raise `RecursionError` (the parser already refuses those
files); `OSError` subclasses other than `FileNotFoundError` surface as `OSError`.

Byte checks: `tools/expected_export_cli.py` runs the real Python on every fixture project and
every project under `converter/local/tested/` (2690 cases): the FCPXML export of each sequence
(file hash, or the exception), `inspect` (summary, `--list`, `--tree`, `--tree --depth`,
`--dump`, `--xml`, `--xml --out`), `visualize` (all modes), `validate` (own and a neighbour's
ground truth, `--coverage`, `--report-json`, several files), `compare` (self, neighbours,
`--show-churn`, `--filter`, `--max-per-object`) and the error paths (missing files, `--json`
with several files, usage errors). `tests/export_cli.rs` replays them in process (stdout hash
and length, stderr, exit status, written files) and runs a sample through the compiled
binaries. The oracle uses `/tmp/prproj-export-cli-cwd`. Rust replays use isolated temporary
directories and remove them after a passing test. Both use the same relative paths through
`premiere-format-research` and `converter` links into the corpus base. For FCPXML comparisons,
the test restores the recorded oracle directory prefix in relative media URLs before hashing.

```sh
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_export_cli.py
```

## Media import (`probe`, `model::import`, `model::combine`)

The last core of py_premiere: the media-file readers of `models/media_import.py` and the entry
points that synthesize media graphs from files. Std-only; the builders they call were already in
`build::media` / `build::merged` / `build::multicam`.

| Python | Rust |
|---|---|
| `read_bmp_size`, `read_png_size`, `read_tiff_size`, `read_psd_size`, `read_gif_size`, `read_jpeg_size` | `probe::read_*_size` -> `(width, height)` |
| `read_avi_info` | `probe::read_avi_info` -> `AviInfo` |
| `_boxes`, `_box`, `_read_field_order`, `_read_track`, `read_mp4_info` (`MovieInfo`, `MovieAudioInfo`) | `probe::read_mp4_info` (lazy box walk, private helpers) |
| `_mxf_ber`, `_mxf_local_sets`, `read_mxf_info`, `MXF_PICTURE_CODINGS` | `probe::read_mxf_info`, `probe::mxf_picture_coding` |
| `read_wav_info`, `_read_extended` + `read_aiff_info`, `_skip_id3` + `_mp3_trim` + `read_mp3_info`, `read_aac_info` (`_adts_length`), `read_wma_info`, `read_m4a_info`, `read_audio_info` (`WavInfo`) | `probe::read_*_info`, `probe::read_audio_info` -> `WavInfo` |
| `models/project.py` `_probe_movie` (`_MovieProbe`) | `probe::probe_movie(path)` / `probe_movie_data(bytes, suffix)` -> `MovieProbe` |
| `Path.suffix` (3.14 rules) | `probe::py_suffix`, `py_suffix_lower` |
| `Project.import_files(paths, target_bin)` (+ `_import_video`, `_import_audio`, `_import_still`, the `.srt` branch, `_label`) | `Project::import_files(paths, target_bin, uuids)`; `import_files_with(.., Option<&Preferences>, ..)` to pass the preferences explicitly (`None` = factory defaults); `import_preferences()` |
| `Project._make_primary_media`, `_make_proxy_media`, `ProjectItem.attach_proxy(path, is_hi_res)`, `_attach_hi_res`, `_override_frame_rect`, `_check_proxy_aspect` | `Project::attach_proxy(item, path, is_hi_res, uuids)`, `model::import::check_proxy_aspect` |
| `ProjectItem.create_sub_clip(name, start, end, has_hard_boundaries, take_video, take_audio)`, `_drop_subclip_half` | `Project::create_sub_clip(item, name, start, end, hard, take_video, take_audio, uuids)` |
| `Project.create_merged_clip(video_item, audio_item, name)` | `Project::create_merged_clip(video, audio, Option<&str>, uuids)` (`model::combine`) |
| `Project.create_multicam_clip(items, name)` | `Project::create_multicam_clip(&[ItemId], Option<&str>, uuids)` (`model::combine`) |

Behaviour follows Python step for step: UUIDs are drawn and objects registered in Python's
order, and a refusal that Python raises after registering objects (e.g. 3-channel movie audio,
an unverified codec after the content-state GUID was drawn) leaves the same partial state.
Errors carry Python's exception class (`Error::py_class`) and message, including the incidental
ones (`subsection not found`, `index out of range`, `negative shift count`, `division by zero`,
`KeyError` reprs). Readers bound every read like `_unpack_from` and slice like Python; offset
arithmetic is `u128`, so no file value overflows it.

Differences, all at absurd inputs or outside Rust's types:

- Python ints are unbounded. An MXF frame size that does not fit an `i64` (its descriptor value
  can be any length) is refused by `read_mxf_info` with an `OverflowError`; a tick value (frame
  rate, duration, start, media out point) or frame size beyond `i64` is refused by the import
  with an `OverflowError` where Python would write the big number. Probe results themselves are
  `i128` and match Python's.
- `target_bin belongs to another project` cannot arise: an `ItemId` is an index into one project.
- Python `TypeError`s for wrong-typed arguments (`validate_bool`, `validate_time`) are ruled out
  by the signatures.
- File read errors (permissions, ...) carry Rust's `io::Error` text, not Python's `[Errno n]` text.
- `path.suffix.lower()` uses Rust's `to_lowercase` (differs from `str.lower` only for a final
  sigma, which never matches a media suffix).
- `new_movie_video_stream` now raises `NotImplementedError` (it was `ValueError`), as Python does.

Byte checks (`tests/import.rs`, data from `tools/expected_import.py`):

- `probes_match_python`: about 208,000 reader calls: every reader on every file of the media
  corpus, plus, per file family, truncations, single flipped bytes and overwritten 32-bit words
  (concentrated on the headers, `moov`, `strh`, `fmt `, `COMM`, Xing), about 150 crafted byte
  strings (every refusal branch and edge: box sizes 0/1/short/overrun, `fiel` details, `tmcd`
  drop frame, v1/v2 sound descriptions, edit lists, MXF BER lengths, missing tags, WAV
  extensible/float/zero fields, AIFF extended floats, MP3 Xing/Info/CBR/MPEG-2/2.5, ADTS,
  ASF play/send/preroll and rounding), and the user's Avid MXFs when present. Same result or
  same exception and message, all of them.
- `suffix_matches_python`.
- `scenario_*`: ten scripts (`scenarios.txt`, replayed by both languages): stills, audio and
  video of every supported kind and every refusal, imports into bins and batches failing
  midway, factory/empty/custom preferences, proxies (both directions, start timecode, aspect,
  audio, codec and path refusals), subclips (video, audio, A/V with each half, still, captions,
  split multichannel, nested, refusals), merged clips (mono, stereo, 29.97, refusals) and
  multicam clips (linked A/V angle, audio-only angle, `Processed Clips` reuse, refusals). Each
  op's result and the saved XML must equal Python's.

The media corpus (`tests/data/import/media/`) is committed: py-premiere's own sample assets
(MIT) and `syn_*` files from `tools/make_import_media.py` (ffmpeg commands recorded there,
bitexact, reproducible with the same ffmpeg build). Both languages copy it to
`/tmp/prproj-import-media/` (absolute paths are embedded) and save at the same depth
(`/tmp/prproj-import-py/<name>/`, `/tmp/prproj-import-rs-<pid>/<name>/`).

```sh
python3 prproj/tools/make_import_media.py          # only to regenerate the media corpus
PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_import.py
cargo test --release --test import
```
