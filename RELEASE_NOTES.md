# Any NLE v0.1.0-alpha.12

## Avid to Premiere 27

- Preserve more linked edits and reusable bin items, including qualified
  timecoded AVC/AAC, discrete managed PCM24, ProRes A/V subclips, AutoSync
  with independent mono WAVs, and editable two-camera ProRes Groups.
- Keep qualified still holds and editable JPEG and PNG alpha matte masks.
  A verified managed MXF can supply a JPEG matte fill when the original
  JPEG is offline. Supported sRGB originals can keep an editable Rec.709
  source interpretation. [Conversion evidence](docs/AVID-PREMIERE-EVIDENCE.md)
- Review and Activity can preview and export redacted diagnostics. An
  optional local MCP server explains findings and can run read-only
  checks for supplied settings or relink proposals.
  [Diagnostics setup](docs/GUI-WIZARD.md#share-diagnostics)

## Internal Avid writer and packaged media tools

- The internal macOS Premiere to Avid prototype writes a mixed linked
  MP4 and managed DNxHD timeline into one Avid bin. Offline read-back
  verifies independent mono trims and centered transitions.
  [Tested profiles](docs/AVID-MANAGED-MEDIA-STAGING.md#mixed-managed-and-ume-edits)
- Packaged FFmpeg enables PNG decoding with alpha. The release checks
  exercise the extracted media tools on all six targets and compare
  both Avid to Resolve media choices on Intel macOS.
  [Release checks](docs/RELEASE.md#build-stage-and-publish)

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/Any.NLE_Windows_x64_v0.1.0-alpha.12.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/Any.NLE_Windows_ARM_v0.1.0-alpha.12.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/Any.NLE_macOS_ARM_v0.1.0-alpha.12.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/Any.NLE_macOS_x64_v0.1.0-alpha.12.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/Any.NLE_Linux_x64_v0.1.0-alpha.12.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/Any.NLE_Linux_ARM_v0.1.0-alpha.12.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. macOS
packages also include the reviewed Resolve templates. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.12/SHA256SUMS) accompany the downloads.

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
