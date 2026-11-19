# Any NLE v0.1.0-alpha.6

## Premiere Pro 27 projects

- Avid conversions now save the native Premiere Pro 27 project version,
  so Premiere 27 can open them directly.
- The packaged wizard and CLI ship Avid → Premiere Pro.

## Internal Premiere Pro and Resolve development

- Add editable forward piecewise Linear retiming on qualified 24 fps
  picture edits and preserve both saved keys and frame selections.
- Expand the qualified profiles for editable nests, multicam, repeated
  transitions, and Resolve-assisted original-media linking.
- Share timing validation between source capture, Resolve preparation,
  archive writing, and read-back.

The [Resolve conversion guide](docs/RESOLVE-TURNOVER-IMPLEMENTATION.md)
records the scope and editor checks for the internal tools.

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/Any.NLE_Windows_x64_v0.1.0-alpha.6.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/Any.NLE_Windows_ARM_v0.1.0-alpha.6.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/Any.NLE_macOS_ARM_v0.1.0-alpha.6.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/Any.NLE_macOS_x64_v0.1.0-alpha.6.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/Any.NLE_Linux_x64_v0.1.0-alpha.6.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/Any.NLE_Linux_ARM_v0.1.0-alpha.6.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.6/SHA256SUMS) accompany the downloads.

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
