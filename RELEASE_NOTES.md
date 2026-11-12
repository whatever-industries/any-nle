# Any NLE v0.1.0-alpha.2

This prerelease updates the desktop app and installer icons and includes the
latest converter fixes. The desktop wizard and CLI convert selected Avid bins
to Premiere Pro projects.

- Resolve development tools report saved timeline ranges and track states and
  preserve mono track solo in the supported linked-project cases.
- The After Effects research inspector reads project clocks, transforms,
  masks, markers, and selected effect controls.
- Release builds run the After Effects inspector tests before packaging.

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/Any.NLE_Windows_x64_v0.1.0-alpha.2.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/Any.NLE_Windows_ARM_v0.1.0-alpha.2.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/Any.NLE_macOS_ARM_v0.1.0-alpha.2.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/Any.NLE_macOS_x64_v0.1.0-alpha.2.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/Any.NLE_Linux_x64_v0.1.0-alpha.2.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/Any.NLE_Linux_ARM_v0.1.0-alpha.2.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.2/SHA256SUMS) accompany the downloads.

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
