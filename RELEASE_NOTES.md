# Any NLE v0.1.0-alpha.10

## Internal Resolve and Premiere previews

- Resolve project Review checks each timeline and shares original-media
  verification across the project. The internal Premiere writer preserves
  qualified multicam selections and reports unsupported edits by occurrence.
  [Resolve reader guide](docs/RESOLVE-SOURCE-READER.md#review-a-project-for-premiere)
- Resolve to Premiere links qualified RED2 picture, opaque PNG and JPEG
  holds, and 44.1 kHz mono MP3. Native Premiere checks cover the saved
  framing, trims, and audio selection for these profiles.
  [Resolve reader guide](docs/RESOLVE-SOURCE-READER.md)
- Premiere to Resolve covers additional mixed-rate Linear retimes,
  nonzero source timecode, and qualified framing and dissolve combinations.
  [Resolve turnover guide](docs/RESOLVE-TURNOVER-IMPLEMENTATION.md)

## Avid source capture

- Graph/2 retains additional saved input and control evidence for Avid
  picture blends. Destination mapping depends on the qualified effect.
  [Source graph guide](docs/SOURCE-GRAPH-MIGRATION.md)

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/Any.NLE_Windows_x64_v0.1.0-alpha.10.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/Any.NLE_Windows_ARM_v0.1.0-alpha.10.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/Any.NLE_macOS_ARM_v0.1.0-alpha.10.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/Any.NLE_macOS_x64_v0.1.0-alpha.10.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/Any.NLE_Linux_x64_v0.1.0-alpha.10.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/Any.NLE_Linux_ARM_v0.1.0-alpha.10.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.10/SHA256SUMS) accompany the downloads.

macOS apps are ad hoc signed and verified, without Apple notarization. Windows
installers are unsigned and install for the current user. Linux AppImages require
Ubuntu 22.04 or a compatible glibc 2.35 desktop, installed X11/Wayland and
OpenGL/EGL libraries, desktop portals and compatible graphics drivers. Distro
shared libraries are not bundled; see the included Linux requirements. No file association is
registered for Avid bins.

This is an alpha. Synthetic automated checks establish bounded parser, writer,
backend and packaging behavior. Full local private-corpus acceptance remains a
separate gate. Opening and listening in the editing application is a separate
compatibility check; automated structural tests do not establish application
read-back. Wider codecs, clocks and unqualified routing remain subject to explicit
refusal or reported loss. No automatic updater is included.
