# Any NLE v0.1.0-alpha.11

## Avid to Resolve preview on macOS

- The package includes reviewed project templates for 1920×1080 at
  23.976 fps. The wizard can link qualified managed DNxHR LB OP-Atom
  picture and two independent PCM24 mono lanes in place, or optionally
  rewrap managed media to OP1a. Review can estimate the size of that rewrap.
- The Apple Silicon reopen and render checks covered both media choices,
  linked originals, visible managed-media reel names, and a two-camera
  Group. Direct links can be prepared with Resolve closed. Assisted media
  linking requires Resolve Studio 21.2.0 build 00006 with local external
  scripting available. [Tested profiles](docs/RESOLVE-TURNOVER-IMPLEMENTATION.md#local-avid-release-template-checks)

## Conversion and source capture

- Avid Review reports affected edit locations in more cases and preserves
  verified direct links when only selected media needs rewrap.
  [Roadmap](docs/ROADMAP.md#avid--premiere)
- The internal Resolve to Premiere preview uses saved physical WAV channel
  assignments and shares source capture across timelines in project Review.
  [Resolve reader guide](docs/RESOLVE-SOURCE-READER.md#review-a-project-for-premiere)
- Graph/2 retains additional Avid marker, clock, and Timewarp facts.
  Destination preparation applies media and picture-effect qualification.
  [Source graph guide](docs/SOURCE-GRAPH-MIGRATION.md)

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/Any.NLE_Windows_x64_v0.1.0-alpha.11.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/Any.NLE_Windows_ARM_v0.1.0-alpha.11.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/Any.NLE_macOS_ARM_v0.1.0-alpha.11.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/Any.NLE_macOS_x64_v0.1.0-alpha.11.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/Any.NLE_Linux_x64_v0.1.0-alpha.11.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/Any.NLE_Linux_ARM_v0.1.0-alpha.11.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. macOS
packages also include the reviewed Resolve templates. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.11/SHA256SUMS) accompany the downloads.

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
