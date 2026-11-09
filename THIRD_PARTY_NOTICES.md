# Third-party notices

First-party Any NLE code is MIT licensed; see LICENSE. Third-party notices and
copyrights remain applicable to their respective code and assets.

- The Avid reader includes MIT code derived from Mark Reid's pyavb and the bounded
  AniMatte reader from FilmCraft. Preserve `avid-reader/NOTICE`,
  `avid-reader/LICENSE-pyavb` and `avid-reader/LICENSE-filmcraft`.
- The Python reference writer vendors Aurore Delaunay's MIT py-premiere fork.
  Preserve `vendor/py_premiere/LICENSE` and `vendor/FORK.md` when redistributing it.
- Binary releases include FFmpeg and ffprobe 8.1 built from pinned upstream source
  with GPL, nonfree and version3 components disabled. They are separate executables
  under LGPL 2.1 or later. `licenses/ffmpeg/` provides the applicable license,
  configuration recipe and source hash; the complete corresponding source archive
  accompanies the binaries. Users may replace these executables, and may reverse
  engineer them for debugging modifications as permitted by the LGPL.
- Rust dependencies, GUI libraries and embedded fonts retain their respective
  licenses. Each release includes a dependency SBOM and the available upstream
  license/notice files in `licenses/rust/`. These include the eframe/egui font
  notices distributed by epaint_default_fonts. The SBOM states exact versions and
  registry archive hashes; local first-party crates use the root MIT license.
  MPL 2.0 crates, including option-ext, include exact hash-verified registry source
  archives under `sources/rust/`; see `SOURCE_AVAILABILITY.md` for source access.
- Windows installers use NSIS 3.11 with its zlib compressor. The installer stub,
  MUI and standard plugin notices are preserved in `licenses/nsis/COPYING`;
  LZMA/CPL and bzip2 compressor modules are not selected.
- Linux AppImages include pinned type2-runtime 20251108 (MIT), static libfuse3
  (LGPL 2.1), squashfuse, musl, Zstandard, zlib and mimalloc. Original notices, full
  runtime/FUSE/dependency source archives, the upstream FUSE patch and rebuild
  instructions are retained in `licenses/appimage-runtime/`,
  `sources/appimage-runtime/` and `appimage-runtime-manifest.json`.
- Linux distribution shared libraries are host prerequisites and are not copied
  into release packages. The AppImage carries `LINUX_RUNTIME_REQUIREMENTS.md` and
  observed runtime SONAMEs; this alpha requires a compatible installed desktop.

The build scripts never redistribute a system or Homebrew FFmpeg executable.
