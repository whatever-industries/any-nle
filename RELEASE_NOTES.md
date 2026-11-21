# Any NLE v0.1.0-alpha.9

## Avid to Premiere Pro 27

- Full rewrap preserves verified anamorphic ProRes geometry through the
  saved source, prepared media, and Premiere project. The qualified case
  retains framing, source clocks, trims, and both audio lanes after reopen.
  [Media rewrap coverage](docs/MEDIA-REWRAP.md)
- The guided wizard separates the Source bins tree and Review from job
  control. Project inspection and conversion continue through the shared
  job engine. [Wizard guide](docs/GUI-WIZARD.md)

## Internal Resolve and Avid previews

- The Resolve reader captures more saved clocks, standalone audio and
  stills, stereo routing, picture aspect ratios, markers, and multicam
  bindings in graph/2. The internal Premiere writer can rebuild qualified
  Resolve multicam tracks with their saved angle and lane choices.
  [Resolve reader guide](docs/RESOLVE-SOURCE-READER.md)
- The Premiere and Resolve to Avid probes generate editable bins for a
  verified ProRes MOV and stereo WAV profile. A separate managed-media
  preview stages DNxHR OP-Atom picture and PCM audio for native indexing.
  [Avid destination preview](docs/AVID-MANAGED-MEDIA-STAGING.md)
- The internal Avid Processing probe qualifies a bounded set of Color
  Adapter conversions to editable correction. The roadmap records the
  tested ranges and controls. [Roadmap](docs/ROADMAP.md#timeline-and-effects)

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/Any.NLE_Windows_x64_v0.1.0-alpha.9.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/Any.NLE_Windows_ARM_v0.1.0-alpha.9.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/Any.NLE_macOS_ARM_v0.1.0-alpha.9.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/Any.NLE_macOS_x64_v0.1.0-alpha.9.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/Any.NLE_Linux_x64_v0.1.0-alpha.9.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/Any.NLE_Linux_ARM_v0.1.0-alpha.9.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.9/SHA256SUMS) accompany the downloads.

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
