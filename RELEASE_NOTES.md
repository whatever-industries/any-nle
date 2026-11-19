# Any NLE v0.1.0-alpha.5

## Premiere Pro and Resolve development

- Expand the internal conversion probes for selected editable multicam,
  mixed-rate picture, and transitions. The
  [qualification guide](docs/RESOLVE-TURNOVER-IMPLEMENTATION.md) records the
  tested media profiles and each editor's read-back and render evidence.
- Add an internal macOS Review screen for original-media linking. The
  converter tries direct linking first and can use a running Resolve Studio
  worker for qualified MOV and MP4 originals that need assisted linking.
- Preserve fit and Center Crop framing for qualified wide-raster picture
  edits. The archive comparison identifies both clip overrides and inherited
  framing changes.

The packaged wizard and CLI continue to ship Avid → Premiere Pro as their
public conversion route. Resolve development remains available through the
internal tools described in the guide.

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/Any.NLE_Windows_x64_v0.1.0-alpha.5.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/Any.NLE_Windows_ARM_v0.1.0-alpha.5.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/Any.NLE_macOS_ARM_v0.1.0-alpha.5.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/Any.NLE_macOS_x64_v0.1.0-alpha.5.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/Any.NLE_Linux_x64_v0.1.0-alpha.5.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/Any.NLE_Linux_ARM_v0.1.0-alpha.5.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.5/SHA256SUMS) accompany the downloads.

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
