# Any NLE

Any NLE is an alpha-stage converter for native video-editing projects. Source
readers capture edits and media relationships in a shared graph (graph/2).
Editor-specific writers use those facts to rebuild editable controls where
verified and report changes or omissions in Review.

| Route | Status |
| --- | --- |
| **Avid → Premiere 27** | Available in the published wizard and CLI. Convert selected Media Composer bins (`.avb`) to an editable Premiere project (`.prproj`). [Workflow and coverage](#convert-an-avid-project) |
| **Avid → Resolve** | macOS preview for qualified 1080p23.976 projects. Link supported managed and original media, or rewrap managed media to OP1a. [Avid → Resolve qualification](docs/ROADMAP.md#avid--resolve) |
| **Premiere ↔ Resolve** | Internal previews in both directions. Verified cases include editable multicam, mixed-rate timing, and independent audio trims. [Coverage by direction](docs/ROADMAP.md#premiere--resolve-conversion) |
| **Premiere → Avid and Resolve → Avid** | Planned product routes with early native output prototypes. [Route roadmap](docs/ROADMAP.md#linked-media-conversion-readiness) |

The roadmap also tracks the [experimental FCPXML export](docs/ROADMAP.md#further-fcpxml-work).

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

In current source builds, linked modes also let you prefer verified camera
originals for picture and attach matching Avid media as proxies. Check the
[wizard guide](docs/GUI-WIZARD.md) and release notes for
the supported options in your build.

Standalone conversion works without Premiere installed for supported media.
On macOS, verified originals that need a Premiere media record can use an
optional installed helper with Premiere 27 running at Home.
[Premiere-assisted linking](tools/premiere-import-uxp/README.md#avid-to-premiere-wizard)

Supported transfers include bin organization and logging metadata, trims and
sequence timecode, markers and clip colors, and selected editable picture,
audio, and text controls. Support depends on the saved controls, media
profile, and clip mode. See the [linked AAF comparisons](docs/ROADMAP.md#avid--premiere)
for what Any NLE preserves and where equivalent transfer still needs work.

| To find | Read |
| --- | --- |
| Wizard setup, saved jobs, and Review | [Wizard guide](docs/GUI-WIZARD.md) |
| Direct links and rewrap profiles | [Direct linking](docs/SPLIT-EDIT-AUDIO-2026-11-06.md#premiere-27-direct-linking) and [media rewrap](docs/MEDIA-REWRAP.md) |
| Editable groups and text | [Avid Groups](docs/EDITABLE-GROUPS-2026-11-08.md) and [caption and text translation](docs/CAPTION-TEXT-TRANSLATION-2026-11-08.md) |
| Reusable AutoSync, track names, and disabled edits | [AutoSync source sequences](docs/SPLIT-EDIT-AUDIO-2026-11-06.md#reusable-autosync-source-sequences), [timeline track names](docs/SPLIT-EDIT-AUDIO-2026-11-06.md#timeline-track-names), and [disabled edits](docs/SPLIT-EDIT-AUDIO-2026-11-06.md#disabled-clip-occurrences) |
| Moved files and relinking | [Verified relink](docs/VERIFIED-RELINK.md) |
| Conversion behavior and coverage by direction | [Decisions explained](docs/DECISIONS-EXPLAINED.md) and [roadmap](docs/ROADMAP.md#linked-media-conversion-readiness) |
| Current implementation and qualification priorities | [Next work](docs/ROADMAP.md#next-work) |

## Command line

Save settings from Review and replay them with the same job engine as the
wizard.

```sh
any-nle review --job SETTINGS.json
any-nle run --job SETTINGS.json
```

Review prints a machine-readable preflight report without creating the project
or media. See the [report and exit codes](docs/GUI-WIZARD.md#automation).

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
- **pr2resolve** by Ersiter. Its native format notes helped identify Resolve's
  speed maps, timeline In/Out marks, and linked audio fields. Any NLE checked
  those fields against native Resolve saves. [Repository](https://github.com/Ersiter/pr2resolve) · [Pinned format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md)

Original bootstrap contributions: @whatever-industries.

## License

First-party Any NLE code is [MIT licensed](LICENSE). Third-party code keeps its own licenses; see [Third-party notices](THIRD_PARTY_NOTICES.md). The packaged FFmpeg tools use a pinned LGPL-only build with source and license materials included.
