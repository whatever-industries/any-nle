# Any NLE

Convert editing projects between NLEs. Today: **selected Avid Media Composer bins to native
Adobe Premiere Pro projects (`.prproj`)**, without Premiere installed. Avid projects are only
read, never changed.

Status: early development. The native wizard supports bin lists and bounded
project-folder discovery, selection and nested destination bins. Implemented Premiere shapes
have scoped application read-backs; automated tests establish structure and parity only.

## What comes across

- Selected bins, with builds and source parts organized per bin; master clips with their Avid
  metadata (Name, Tape/Camroll,
  Scene, Take, Comments, Source File, custom columns).
- Avid OP-Atom MXF media (DNx video, PCM audio), matched by the IDs inside the MXF files.
- Clips built the way you choose: native rewrap (the wizard default), **Grouped**,
  **Merged** or **Separate**, with supported Avid audio groups and explicit routing.
- Bounded linked-original interpretation, with saved paths or explicit root mappings;
  unsupported descriptors and media profiles refuse. This is not general AMA compatibility.
- Sequences: edits, start timecode, constant speed and reverse, clip and edit gain, pan,
  audio crossfades.
- SubCap captions and current static single-flow Titler+ editable text, with bounded
  content/timing support and explicit destination style defaults. Source positioning and
  appearance are not yet reproduced; [qualification limits](docs/CAPTION-TEXT-TRANSLATION-2026-11-08.md).
- Clip colors as Premiere labels; markers with name, comment and color, including markers
  on edits and on empty track space.

Why each choice works the way it does: [docs/DECISIONS-EXPLAINED.md](docs/DECISIONS-EXPLAINED.md).
What is planned: [docs/ROADMAP.md](docs/ROADMAP.md). Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## How it works

The native Rust wizard (`any-nle-gui`) and `any-nle run --job SETTINGS.json` use one
inventory/pre-flight/execution engine. Save job settings from Review for CLI replay.
Changing inputs or choices requires a fresh pre-flight. Conversion never overwrites an
existing project, receipt or media folder. Packaged builds include verified LGPL-only
FFmpeg/ffprobe; unbundled CLI use requires external tools.

Build the GUI and CLI with Rust1.95.0 using `cargo build --release --manifest-path
crates/Cargo.toml --workspace`. The six-target packaging workflow is implemented, but
cross-platform build and application checks remain pending.

The older standalone model APIs remain available:

1. **`avid-reader`** (Rust) reads Avid bins (`.avb`) and MXF headers and
   writes a neutral JSON edit model.
2. **`prproj`** (Rust; standard library plus `flate2`) writes the Premiere project from that
   model. It is a Rust port of the Python writer (`src/avid2prproj` on a fork of py-premiere),
   which is kept as the byte-exact reference (ADR-0003 in docs/ARCHITECTURE.md).

```sh
avid-reader/target/release/avid-reader model BIN.avb... --media MEDIA_FOLDER [--media MORE] > model.json
prproj/target/release/prproj-write model.json OUTPUT.prproj [--bin-mode separate|merged|grouped]
```

Or in one step, through the program-neutral graph (`crates/`, ADR-0004 and
[docs/GRAPH-MODEL.md](docs/GRAPH-MODEL.md)). Unchanged legacy cases retain byte parity;
new source text facts use the graph/2 extension described above:

```sh
(cd crates && cargo build --release)
crates/target/release/any-nle convert --from avid BIN.avb... --media MEDIA_FOLDER --to premiere OUTPUT.prproj \
    [--policy POLICY.json] [--bin-mode separate|merged|grouped] [--graph GRAPH.json]
crates/target/release/any-nle policy              # default Avid -> Premiere choices, to edit
crates/target/release/any-nle colours GRAPH.json  # what each Avid colour becomes in Premiere
```

What converts, per program: [docs/FEATURE-MATRIX.md](docs/FEATURE-MATRIX.md).

The normal Avid→Premiere route reads qualified source facts directly into
`anynle.graph/2`, then prepares the destination once. `any-nle graph` and Premiere
source snapshots explicitly use graph/2. Graph/1 reading and the standalone model
API remain available; held FCPXML conversion keeps its legacy profile.
[Architecture, limits and acceptance](docs/SOURCE-GRAPH-MIGRATION.md).

Optional OP-Atom media preparation: append `--rewrap-op1a NEW_MEDIA_DIRECTORY` to an
Avid→Premiere conversion. It displays **“Creates new media.”** Add `--rewrap-dry-run` for
a preview without writes. On macOS, use `--rewrap-op1a-prompt` instead of a directory
argument for a native folder picker and a review showing estimated additional space and
available disk space before conversion. Requires external FFmpeg/ffprobe8.1; supported inputs and
scoped qualification and remaining limits are in [docs/MEDIA-REWRAP.md](docs/MEDIA-REWRAP.md).
Add `--rewrap-native-master` for the experimental [file-backed AV-master route](docs/NATIVE-OP1A-MASTERS.md),
preserving supported bin audio groups and sequence routing. This explicitly replaces bin
mode. Native read-back is limited to qualified cases.
The same native route supports [bounded editable Avid Groups](docs/EDITABLE-GROUPS-2026-11-08.md):
two same-rate DNx cameras with fixed two-channel mono audio. Other Group layouts
receive a named refusal.


Public synthetic checks (run from the repository root):

```sh
python3 scripts/public_acceptance.py --output public-checks
python3 -m unittest discover -s scripts/release -p test_release.py -v
```

Native corpus qualification requires separate fixtures and is not implied by these checks.

The model format is in [`prproj/src/convert/model.rs`](prproj/src/convert/model.rs) and
[`src/avid2prproj/model.py`](src/avid2prproj/model.py) (example:
[`examples/opatom_clip.model.json`](examples/opatom_clip.model.json)). Times are integer ticks
(254,016,000,000 per second). Loading is strict and bounded; anything unsupported is refused
or reported with a named reason, and output is never overwritten.

## Thanks

Any NLE stands on the work of others:

- **[py-premiere](https://github.com/Pouf/py-premiere)** by Aurore Delaunay (MIT). Its
  reverse-engineered Premiere project model, verified against Premiere itself, is what lets
  Any NLE write native `.prproj` files without Premiere. A pinned fork is vendored in
  [`vendor/`](vendor/FORK.md) with its license.
- **[pyavb](https://github.com/markreidvfx/pyavb)** by Mark Reid (MIT). `avid-reader`'s bin
  decoding is a partial Rust port of it; see [`avid-reader/NOTICE`](avid-reader/NOTICE) and
  [`avid-reader/LICENSE-pyavb`](avid-reader/LICENSE-pyavb).

Original bootstrap contributions: @whatever-industries.

## License

First-party Any NLE code is [MIT licensed](LICENSE). Third-party code keeps its own licenses; see [Third-party notices](THIRD_PARTY_NOTICES.md). The packaged FFmpeg tools use a pinned LGPL-only build with source and license materials included.
