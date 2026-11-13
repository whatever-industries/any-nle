# Any NLE v0.1.0-alpha.3

Any NLE can link supported Avid DNxHR LB OP-Atom media directly in
Premiere Pro 27. Separate files, Grouped, and Merged keep the original
1080p23.976 picture MXF and both mono PCM24/48 kHz audio MXFs in place.

The internal Resolve conversion tools now preserve more editable controls.
Resolve to Premiere supports scoped audio gain, pan, fades, and transitions,
plus static crop and Linear position and scale on trimmed clips. Premiere
to Resolve transfers sequence Comment markers, including names, notes, and
duration. The converter reports marker color substitutions when the palettes
differ. The Resolve routes are available through the internal tools.

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/Any.NLE_Windows_x64_v0.1.0-alpha.3.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/Any.NLE_Windows_ARM_v0.1.0-alpha.3.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/Any.NLE_macOS_ARM_v0.1.0-alpha.3.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/Any.NLE_macOS_x64_v0.1.0-alpha.3.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/Any.NLE_Linux_x64_v0.1.0-alpha.3.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/Any.NLE_Linux_ARM_v0.1.0-alpha.3.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.3/SHA256SUMS) accompany the downloads.

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
