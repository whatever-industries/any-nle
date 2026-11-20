# Any NLE v0.1.0-alpha.8

## Wizard and Review

- Project, Media, and Review now use a guided sidebar. The Source bins
  tree is scrollable and can browse folders before reading bin contents.
- Review places a recovery action with each blocker. It can group
  findings by timeline or source clip. Activity keeps detailed errors
  available from the sidebar. Cancelling an operation gets a short notice.
- Avid to Premiere can omit an unqualified bin clip when the full saved
  dependency check proves it unused. Review identifies the affected clip.

## Avid to Premiere Pro 27

- Wide DNx rewraps preserve 33 to 64 independent 48 kHz PCM16 or PCM24
  mono lanes with picture-only OP1a and one WAV per source. Broadcast
  Wave metadata carries a shared nonzero source start when it lands on
  an exact sample. [Media rewrap coverage](docs/MEDIA-REWRAP.md)
- Qualified ProRes alpha and DNxHR 444 rewraps retain their verified
  picture interpretation. New Premiere edits carry the source's
  effective pixel aspect ratio. Review identifies non-square ProRes
  inputs that need a verified rewrap mapping.
- The roadmap links to retained evidence for AAF comparisons, media,
  and native reopen checks. [Roadmap](docs/ROADMAP.md#next-work)

## Internal Resolve development

The internal converter preserves more ordered picture effects and carries
qualified source color adjustments into Resolve. The macOS Avid to Resolve
wizard still uses externally supplied, pinned project templates for local
qualification. [Resolve conversion guide](docs/RESOLVE-TURNOVER-IMPLEMENTATION.md)

## Download

| Platform |
| --- |
| [**Windows (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/Any.NLE_Windows_x64_v0.1.0-alpha.8.exe) |
| [**Windows (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/Any.NLE_Windows_ARM_v0.1.0-alpha.8.exe) |
| [**macOS (Apple Silicon)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/Any.NLE_macOS_ARM_v0.1.0-alpha.8.zip) |
| [**macOS (Intel)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/Any.NLE_macOS_x64_v0.1.0-alpha.8.zip) |
| [**Linux (x64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/Any.NLE_Linux_x64_v0.1.0-alpha.8.AppImage) |
| [**Linux (ARM64)**](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/Any.NLE_Linux_ARM_v0.1.0-alpha.8.AppImage) |

Each package includes the `any-nle` CLI, `any-nle-gui`, verified FFmpeg/ffprobe
8.1, dependency notices, an SBOM and corresponding FFmpeg source. Separate
provenance archives and [SHA256 checksums](https://github.com/whatever-industries/any-nle/releases/download/v0.1.0-alpha.8/SHA256SUMS) accompany the downloads.

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
