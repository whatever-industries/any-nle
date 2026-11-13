# Native release process

Release version: `0.1.0-alpha.3`. First-party code is MIT; dependency notices remain
applicable. The GUI is native Rust, and both GUI and CLI ship with media tools.

## Runtime layout

`any-nle`, `any-nle-gui`, `ffmpeg`, `ffprobe` and `backend-manifest.json` share one
executable directory. Windows names end in `.exe`. On macOS it is
`Any NLE.app/Contents/MacOS`; on Linux it is `AppDir/usr/bin`; on Windows it is the
per-user installation directory. The manifest schema is `1`, with version, exact
Rust target, LGPL license, pinned upstream source URL/hash, configure flags and
per-tool basename/SHA256. The runtime verifies bundled bytes before use. Explicit
CLI backend overrides remain available. PATH is a development fallback only when
an unbundled CLI has no adjacent manifest; packaged GUI conversions require the
verified bundle.

## Reproduce a candidate

Use Rust 1.95.0 and the committed lockfile. If several Rust installations are on
your PATH, put the rustup shims first and set `RUSTUP_TOOLCHAIN=1.95.0` before
running the scripts. Check `rustc --version` and `cargo --version` in that shell.
`packaging/ffmpeg.json` pins the source
archive and configuration; `scripts/release/build_ffmpeg.py` checks the archive
hash before building. No installed FFmpeg is copied. GPL, nonfree and version3
components, external autodetection and networking are disabled. The only input
device is synthetic lavfi for repeatable generated-media tests. Compiler runtimes
are static for Windows tools. Linux packages redistribute no distro shared
libraries: the AppImage requires Ubuntu 22.04 or a compatible glibc 2.35 desktop
with the X11/Wayland, OpenGL/EGL and portal prerequisites listed in
[LINUX_RUNTIME_REQUIREMENTS.md](../packaging/LINUX_RUNTIME_REQUIREMENTS.md).
This alpha is not fully self-contained across arbitrary Linux distributions.
The package records its observed host SONAMEs in `linux-runtime.json`.

1. Build the backend using `build_ffmpeg.py --target TARGET --output backend
   --work backend-work`; run `backend_smoke.py` with that directory and target.
2. Run `scripts/public_acceptance.py --output public-checks`, packaging unit tests,
   and build the two Rust binaries with `cargo build --locked --release
   --manifest-path crates/Cargo.toml --target TARGET --bin any-nle --bin any-nle-gui`.
3. Collect notices using `scripts/release/licenses.py notices`, then invoke
   `scripts/release/package.py` with target, binaries, backend, notices and output.
   Run `verify_candidate.py` against the actual package, and audit the staged files
   plus compressed provenance before publication.

Scripts use Python 3.13 in CI. Native packaging uses macOS ditto/codesign, NSIS 3.11
on Windows, using the zlib compressor and retained NSIS notices, and
checksum-pinned AppImage appimagetool 1.9.1 on Linux. AppImage composition
always supplies the separately pinned dated type2-runtime 20251108 via
`--runtime-file`; it never downloads an implicit latest runtime. The package
retains exact runtime/FUSE sources, the upstream FUSE patch, all original grants,
rebuild/relink scripts and versioned permissive dependency source archives.
Historical Alpine recipe URL/hash context is recorded separately, without claiming
object-level package identity or byte-reproducible upstream builds. The six runners
are macos-15 (ARM), macos-15-intel, windows-latest (x64), windows-11-arm,
ubuntu-22.04 (x64) and ubuntu-22.04-arm. Dependencies and actions are pinned where
an immutable published identifier exists. Compiler/tool versions and generated
configuration travel with the evidence. macOS backend hashes are renewed after ad
hoc signing, before the containing app is signed and verified. macOS ZIP packages
use ditto with sequestered resource attributes; extract with Finder or `ditto -x
-k` to preserve those attributes. The candidate verifier uses native ditto and
checks the extracted app signature as well as byte equality.

Rust license collection requires an applicable crate grant in addition to
attribution. Embedded font notices do not replace the font crate's own MIT/Apache
grant. Crates declaring MPL 2.0 include their exact Cargo.lock-hashed registry
source archives and `SOURCE_AVAILABILITY.md` in every package and provenance
archive. Full-grant AUTHORS files, such as r-efi's, are retained as grants; a list
of author names alone cannot pass the grant check.

Windows binaries use a static Rust CRT and are inspected for both ordinary and
delayed PE imports. The package refuses non-system DLL dependencies in the GUI,
CLI, FFmpeg, ffprobe and installer stub; runner-installed toolchains cannot satisfy
a missing redistribution dependency silently.

## Separate gates

The public profile uses synthetic tests only. It covers the graph, Avid adapter,
mapping, GUI model, inventory, job engine, backend resolution, media preparation,
writer and reader cancellation. `scripts/public_acceptance.py` defines the
required test counts and platform exclusions. It rejects empty, failed, ignored
or skipped selected suites. Each architecture also runs the bundled backend
against synthetic picture and PCM. Full converter qualification uses the
separate private corpus.

The full private acceptance manifest and native application evidence remain local.
The filtered GitHub export excludes private regression dumps from every revision.
The pre-push guard binds the reviewed export to its source revision.
The release owner must separately review every reachable remote ref/history, logs,
compressed fixtures, package assets and provenance against a private token
inventory, then bind the reviewed full acceptance/privacy receipt by SHA256.
If pre-existing private project references are discovered on GitHub, stop public
publication for discussion; do not scrub or force-push and then continue.

`audit.py` accepts a private external token file and scans UTF-8/UTF-16, ZIP, TAR,
gzip and embedded zlib streams with expansion limits. Tokens and raw matching
content are never emitted. Public scanner tests use generic synthetic sentinels.
Malformed archives fail the audit with a structured receipt. An explicit
`--known-upstream-fixtures` manifest classifies only exact-byte reviewed public
decoder regression fixtures. The receipt retains each accepted exception and
`expansion_complete=false`; altered or unknown fixtures fail, and raw private-token
matches always fail. This does not claim that malformed gzip was expanded.
This scanner is a component of the full privacy review, not proof that unknown
private material is absent.

## Build, stage and publish

Use `build_only=true` and `publish=false` in the manual `Native release candidate`
workflow to verify a new commit against the current version. This uploads six
workflow artifacts and leaves existing releases and tags in place. With
`build_only=false`, the workflow stages a draft by default. Both modes require
the retained private-gate receipt SHA256 and the reviewed filtered export
commit SHA. The latter must equal both the checkout HEAD and workflow event SHA,
so an attestation for a different source cannot be dispatched accidentally. The
private receipt remains an owner attestation; its contents are retained privately.
One job downloads and verifies the pinned FFmpeg source archive. All six jobs
build from that verified archive. The workflow
verifies package extraction against the audited stage, checks executable target
headers and runs the packaged CLI/backend. Its aggregate gate checks all six
receipts bind identical source and private receipt, then checks every asset hash.
Native checks also verify the packaged launcher icon on each platform.
If the artwork changes, regenerate the committed resources with
`scripts/release/make_icons.py` before building.
The candidate artifacts retain evidence for seven days; release assets retain
provenance and checksums.

Only the release job has `contents: write`. `publish.py` refuses any published version or draft from a different source. It
stages a prerelease, downloads all uploaded assets, verifies exact
SHA256 equality and publishes only when explicitly selected. An existing draft can be resumed only with identical source and downloaded asset
bytes; it is never overwritten. A published version is immutable: use a new version for any change. The script never changes repository
visibility or pushes Git history. The release owner performs those actions only
after the full private and remote privacy gates have passed. Publication fails if
the repository is still private when publishing is selected. There is no updater,
signing secret, file association or extra approval environment.
