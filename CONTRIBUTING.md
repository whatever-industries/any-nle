# Contributing to Any NLE

Use the [roadmap](docs/ROADMAP.md#where-work-is-tracked) to find the current
scope, pending checks, and held work. Use the [release notes](RELEASE_NOTES.md)
to see what is in the download.

## Build and test from a public clone

Use Rust 1.95.0, the release toolchain, and Python 3.11 or later. Check
`rustc --version` and `cargo --version` in the shell that will run the build.
The release workflow lists the [Linux build dependencies](.github/workflows/release.yml).

If Homebrew and Rustup are both installed, invoke Rustup explicitly so
the acceptance runner and its subprocesses use the release toolchain.
With a standard Rustup installation on macOS, run the public suite from
the repository root.

```sh
"$HOME/.cargo/bin/rustup" run 1.95.0 python3 scripts/public_acceptance.py --output crates/target/public-checks
```

Run these commands from the repository root. The public test profile uses
fixtures included in the clone and requires no editor installation.

```sh
cargo build --locked --release --manifest-path crates/Cargo.toml --bin any-nle --bin any-nle-gui
python3 scripts/public_acceptance.py --output crates/target/public-checks
python3 -m unittest discover -s scripts/release -p test_release.py -v
```

Use `public_acceptance.py` for the test profile supported by a public clone.
The full Premiere writer test suite uses local fixtures and helpers from the
private corpus. The public profile includes standalone Premiere preparation,
graph output, media links, bin layout, and headless wizard report checks.

Cargo places the executables in `crates/target/release/` by default. For
development, commands such as `any-nle --help` and `any-nle-gui --version`
work without the media backend. Source builds use external `ffmpeg` and
`ffprobe` for media preparation. See the [release guide](docs/RELEASE.md)
for development and packaged backend requirements.

## Find the code

| Concern | Entry point |
| --- | --- |
| Wizard and shared job workflow | [crates/gui](crates/gui) and [crates/app](crates/app) |
| Avid decoding and source interpretation | [avid-reader](avid-reader) and [Avid adapter](crates/avid-adapter) |
| Portable edit facts and their validation | [graph/2](crates/graph) and [source graph guide](docs/SOURCE-GRAPH-MIGRATION.md) |
| Premiere preparation and native writing | [Premiere adapter](crates/premiere-adapter) and [prproj](prproj) |
| Internal Premiere and Resolve source reading, and Resolve writing | [Resolve adapter](crates/resolve-adapter) and [probe guide](docs/RESOLVE-SOURCE-READER.md) |
| Compatibility and differential references | [graph contracts](docs/GRAPH-MODEL.md) and [Python reference writer](src/avid2prproj) |

Keep portable edit facts in graph/2 and preserve editor-specific controls on
their source owner. Put destination defaults and approximations in
destination preparation. The [architecture decisions](docs/ARCHITECTURE.md)
record why these boundaries exist.

## Submit a change

1. **Use clean-room evidence.** Base format changes on public documentation
   or reproducible observations you make with files and tools you are
   authorized to use. Cite revision-pinned references when available. Keep
   third-party code provenance and license notices with any permitted reuse.
2. **Use shareable fixtures.** Prefer a small synthetic project and
   generated media with generic names. Keep private projects, footage,
   native returns, local paths, and credentials out of the PR and its history.
   Include a recipe when someone needs to reproduce an editor observation.
3. **State the scope.** Name the source and destination, editor builds,
   media profile, and affected control or edit. Describe what is preserved,
   any reported losses, and any input that must be refused.
4. **Report what was checked.** Run the relevant public tests. For changes
   to conversion semantics, exercise a success and a malformed or
   unsupported case. Distinguish structural read-back, editor reopen,
   rendered output, and checks still pending. The private corpus and
   release gates run separately.

Target current editor versions under the
[compatibility policy](docs/ROADMAP.md#editor-version-compatibility). Use the
[roadmap](docs/ROADMAP.md#where-work-is-tracked) for current status and keep
dated evidence with the feature it qualifies.
