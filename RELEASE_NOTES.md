# Any NLE v0.1.0-alpha.7

## Avid to Premiere Pro 27

- Review can estimate rewrap size from the source files before creating media.
- Selective rewrap keeps qualified direct links and can prepare supported managed
  DNx MXFs and ProRes MOVs. Layouts wider than 32 mono channels use separate audio
  files. [Supported profiles and verification](docs/MEDIA-REWRAP.md)
- On macOS, an optional Premiere 27 UXP helper can supply media records for
  verified originals that need assisted linking. Review identifies those
  files. [Helper setup and scope](tools/premiere-import-uxp/README.md#automatic-linking-on-macos)
- Run `any-nle review --job SETTINGS.json` for JSON preflight without creating
  project or media outputs. Route and mapping arrows now render from SVG.
  [Automation guide](docs/GUI-WIZARD.md#automation)

## Internal Resolve development

Local macOS qualification adds a wizard flow for Avid to Resolve with two
media choices, direct linking and managed OP1a rewrap. Public availability
depends on reviewed synthetic project templates and native release checks.
[Resolve conversion guide](docs/RESOLVE-TURNOVER-IMPLEMENTATION.md#avid-to-resolve-in-the-macos-wizard)

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/Any.NLE_Windows_x64_v0.1.0-alpha.7.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/Any.NLE_Windows_ARM_v0.1.0-alpha.7.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/Any.NLE_macOS_ARM_v0.1.0-alpha.7.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/Any.NLE_macOS_x64_v0.1.0-alpha.7.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/Any.NLE_Linux_x64_v0.1.0-alpha.7.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/Any.NLE_Linux_ARM_v0.1.0-alpha.7.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.7/SHA256SUMS) accompany the downloads.

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
