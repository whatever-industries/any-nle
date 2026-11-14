# Any NLE

Any NLE translates editing projects between native project formats. Its current
desktop wizard and CLI convert selected Avid Media Composer bins (`.avb`) to
Adobe Premiere Pro projects (`.prproj`). Conversion can run without Premiere
installed. You need Premiere to review the result in the editor.

DaVinci Resolve conversion is in development. Premiere → Avid and Resolve → Avid
are planned, with Pro Tools and After Effects as longer-term targets. The
[roadmap](docs/ROADMAP.md#linked-media-conversion-parity) tracks each direction
and the [experimental FCPXML export](docs/ROADMAP.md#further-fcpxml-work).

## Download

[Download Any NLE](https://github.com/whatever-industries/any-nle/releases)
for macOS, Windows, or Linux. Packages include the wizard, CLI, and media tools.
See the [release notes](RELEASE_NOTES.md) for installation and checksums.

## Convert an Avid project

1. Open the wizard and select a project folder or individual Avid bins.
2. Select the bins to convert and point Any NLE to the source media.
3. Choose a clip mode. **Rewrap to native clips** is the wizard default
   and shows **Creates new media.** **Separate files**, **Grouped**, and
   **Merged** can link qualified media in place.
4. In Review, run preflight and check the media paths, blockers, and reported
   losses. Convert, then open the new project in Premiere.

Supported transfers include bin organization and logging metadata, trims and
sequence timecode, markers and clip colors, and selected editable picture,
audio, and text controls. Support depends on the saved controls, media
profile, and clip mode.

| To find | Read |
| --- | --- |
| Wizard setup, saved jobs, and Review | [Wizard guide](docs/GUI-WIZARD.md) |
| Direct links and rewrap profiles | [Direct linking](docs/SPLIT-EDIT-AUDIO-2026-11-06.md#premiere-27-direct-linking) and [media rewrap](docs/MEDIA-REWRAP.md) |
| Editable groups and text | [Avid Groups](docs/EDITABLE-GROUPS-2026-11-08.md) and [caption and text translation](docs/CAPTION-TEXT-TRANSLATION-2026-11-08.md) |
| Moved files and relinking | [Verified relink](docs/VERIFIED-RELINK.md) |
| Conversion behavior and coverage by direction | [Decisions explained](docs/DECISIONS-EXPLAINED.md) and [roadmap](docs/ROADMAP.md#linked-media-conversion-parity) |

## Command line

Save settings from Review and replay them with the same job engine as the
wizard.

```sh
any-nle run --job SETTINGS.json
```

Or convert directly. Replace the example paths with your bin, media folder,
and a new project filename.

```sh
any-nle convert --from avid "Selected bin.avb" --media "Avid MediaFiles" \
  --to premiere "Converted.prproj" --bin-mode separate
```

Run `any-nle --help` for graph export and
[media preparation options](docs/MEDIA-REWRAP.md#current-cli). Existing
outputs are protected from replacement during conversion.

## Contributing

Start with [CONTRIBUTING.md](CONTRIBUTING.md) for the build, public test suite,
and clean-room evidence requirements. AI coding agents should also read
[AGENTS.md](AGENTS.md). The
[current architecture](docs/SOURCE-GRAPH-MIGRATION.md#boundaries) explains
how source adapters, the shared graph, and destination writers fit together.

## Thanks

Any NLE stands on the work of others:

- **[py-premiere](https://github.com/Pouf/py-premiere)** by Aurore Delaunay (MIT). Its
  reverse-engineered Premiere project model, verified against Premiere itself, is what lets
  Any NLE write native `.prproj` files without Premiere. A pinned fork is vendored in
  [`vendor/`](vendor/FORK.md) with its license.
- **[pyavb](https://github.com/markreidvfx/pyavb)** by Mark Reid (MIT). `avid-reader`'s bin
  decoding is a partial Rust port of it; see [`avid-reader/NOTICE`](avid-reader/NOTICE) and
  [`avid-reader/LICENSE-pyavb`](avid-reader/LICENSE-pyavb).

Original bootstrap contributions: @whatever-industries.

## License

First-party Any NLE code is [MIT licensed](LICENSE). Third-party code keeps its own licenses; see [Third-party notices](THIRD_PARTY_NOTICES.md). The packaged FFmpeg tools use a pinned LGPL-only build with source and license materials included.
