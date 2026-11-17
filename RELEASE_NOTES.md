# Any NLE v0.1.0-alpha.4

## Avid to Premiere Pro 27

- Link qualified ProRes HQ and H.264 originals at matching source and sequence
  rasters. Streaming picture checks verify long clips without collecting the
  whole packet report in memory.
- Transfer editable static Corner Pin, Linear Resize and 3D Warp motion and
  crop, and the qualified three-key 3D Warp Z Rotation controls at 24, 23.976,
  25, and 29.97 fps. Shelf maps to Hold. The qualified Bezier and Spline
  curves retain editable zero-slope handles.
- Preserve the saved cut when an unsupported fade or dissolve safely resolves
  to an isolated edit against filler. The conversion report identifies the
  omitted transition.

## Review

- Save a preflight report as JSON from a ready or blocked Review.
- Search affected clips and packages in longer blocker lists. Review shows
  conflicting media paths and opens the relevant media settings.

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/Any.NLE_Windows_x64_v0.1.0-alpha.4.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/Any.NLE_Windows_ARM_v0.1.0-alpha.4.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/Any.NLE_macOS_ARM_v0.1.0-alpha.4.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/Any.NLE_macOS_x64_v0.1.0-alpha.4.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/Any.NLE_Linux_x64_v0.1.0-alpha.4.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/Any.NLE_Linux_ARM_v0.1.0-alpha.4.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.4/SHA256SUMS) accompany the downloads.

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
