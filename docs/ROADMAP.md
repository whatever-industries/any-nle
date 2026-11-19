# Any NLE roadmap

Start with the [README](../README.md) for the current workflow and the
[release notes](../RELEASE_NOTES.md) for what ships. Checked items apply to
the profile and verification described in each entry. Pending editor or
render checks remain open even when the implementation exists. The
[contributor guide](../CONTRIBUTING.md) covers how to pick up and verify work.

## Where work is tracked

| Work | Start here |
| --- | --- |
| **Media, relinking, and proxies** | [Media linking](#media-linking-user-2026-10-14) and [verified relink](VERIFIED-RELINK.md) for tested path, reel, and timecode checks. |
| **Linked conversion readiness** | [Native acceptance and AAF benchmarking](#linked-media-conversion-readiness), including [Avid → Premiere](#avid--premiere) and [Premiere ↔ Resolve](#premiere--resolve-conversion). The [Resolve turnover boundary](RESOLVE-TURNOVER-IMPLEMENTATION.md#qualification-boundary) and [pending editor checks](RESOLVE-PROJECT-CORE.md#pending-editor-checks) give fixture-level steps. |
| **Effects, audio, and source interpretation** | [Timeline and effects](#timeline-and-effects), [audio](#audio), and [declared audio layout gaps](#cataloged-audio-configuration-gaps-user-2026-11-07). The [shared effects audit](SHARED-EFFECTS-AUDIT.md) records graph and destination contracts. |
| **Resolve source and destination coverage** | [Native Resolve research](#davinci-resolve-user-2026-10-26-bookmark-and-research), [completed reference reviews](#completed-reference-reviews-and-their-scope), and [implementation priorities](#general-conversion-priorities-from-the-pinned-references). The [reader guide](RESOLVE-SOURCE-READER.md) has probe commands and tested profiles. |
| **Graph and planned destinations** | [Neutral graph and After Effects](#more-programs-the-neutral-graph-adr-0004-user-2026-10-26) and the [source graph guide](SOURCE-GRAPH-MIGRATION.md). |
| **App, packaging, and release** | [Wizard and platform work](#app) and the [release guide](RELEASE.md). |
| **Holds and prior results** | [Work on hold](#on-hold-user-2026-10-28) and [completed capabilities](#completed-capabilities). The [graph/1 feature matrix](FEATURE-MATRIX.md) is a compatibility reference. |

The [Premiere/Resolve metadata crosswalk](PREMIERE-RESOLVE-METADATA.md)
tracks where logging and media identity fields belong. The maintainer
workspace keeps the format research queues and evidence pins.

Keep each open converter task under one route or shared section here. Link
other entries to that task. Keep test steps and results in the feature guides,
library parity in its ledgers, and dated reports in the maintainer handoff notes.
When a newer result supersedes a dated task, update its status here and link
to the newer evidence from the older document.

Detailed checklists cover [media choices by route](#media-relinking-and-proxies-by-route),
[offline output](#offline-output-and-relink), [high-priority Avid → Premiere fallbacks](#high-priority-conversion-fallbacks),
[linked audio and stills](#high-priority-linked-audio-and-stills),
[the next Avid → Premiere tests](#remaining-native-qualification),
[Avid reader and writer gates](#avid-reader-and-writer-gates), and
[release maintenance](#release-maintenance-and-packaging).

## Next work

1. **Build and test the combined Avid → Resolve project.** Assemble one mixed
   managed/UME project from Avid input, then verify picture, both mono lanes,
   and editable Group selection after Save Project As and reopen.
   [Avid → Resolve](#avid--resolve)
2. **Improve Avid → Premiere Review.** Group findings by timeline so a user
   can see what will transfer, what will be omitted, and what blocks publication.
   [Batch diagnostics](#project-import-user-2026-10-14)
3. **Extend Premiere ↔ Resolve conversion in both directions.** Reuse
   fixtures and graph/2 facts across both routes. Test each writer against
   the receiving editor's saved project and render.
   [Premiere ↔ Resolve](#premiere--resolve-conversion)

The [Windows x64 smoke test](#app) can run independently. Deferred work is
listed under the relevant route or [holds](#on-hold-user-2026-10-28).

## Editor-version compatibility

Target current editor versions for new readers, writers, and native qualification.
Premiere 27 is the current Premiere target. Older files and format notes can
serve as research controls. Add compatibility for a legacy feature only as a
rare, feature-specific exception when it was widely used and a current editor
can still read, render, edit, or carry it forward. Record the current-version
behavior, the practical value, and the tested limits before expanding scope.

Titler+ Legacy and Lumetri are candidates for such exceptions where current
editor behavior supports them. AMA links may also qualify when a current Avid
build still resolves or carries the saved locator. Each exception covers its
tested feature and format. Broader old project-version support needs its own
product decision.

## Media, relinking, and proxies by route

Linking existing media is the shared workflow. Keep **Rewrap to native clips**
as the Avid → Premiere wizard default. For routes into Resolve or Avid, the
proposed default is **Link to originals**, using the source project's primary
files in place. For Avid input, a primary file can be a managed MXF sibling or
a linked UME original. Camera-original lookup is a separate choice that can
change the picture source while keeping the edited audio bindings.

| Direction and status | Primary media and available choices | Trade-off and next work |
| --- | --- | --- |
| **Avid → Premiere** · available | **Rewrap to native clips** makes file-backed clips. Qualified DNx uses OP1a and the bounded ProRes LT profile uses MOV. **Separate files**, **Grouped**, and **Merged** can link qualified OP-Atom picture and mono siblings directly. Supported UME originals stay linked. The local wizard offers a separate primary-media choice and optional Avid proxy attachment in linked modes. | Rewrap simplifies Premiere's media layout and can address qualified playback or audio-layout problems. It takes time and disk space. Direct linking avoids new media. Separate, Grouped, and Merged change the clip construction, while decoder admission remains profile-specific. [Media rewrap](MEDIA-REWRAP.md) · [Direct linking](SPLIT-EDIT-AUDIO-2026-11-06.md#premiere-27-direct-linking) · [Proxy profiles](VERIFIED-RELINK.md#automatic-avid-proxy-attachment) |
| **Avid → Resolve** · internal | Link supported managed DNxHR siblings and H.264 UME originals in place. For grading, offer **Prefer camera originals for picture**, with optional Avid proxy attachment. Preserve separately edited audio on its verified sources, including independent Avid mono MXFs. | Direct linking is fast and creates no media. Camera-original linking gives the colorist the verified source picture. In the offline-original control, proxy picture played while audio from the camera item was silent. Independent mono MXFs supplied both audio lanes, including in one 48-frame Deliver export with two discrete PCM24 mono tracks. Qualify the combined managed/UME turnover and a real managed-bin conform before rollout. [Turnover gates](RESOLVE-TURNOVER-IMPLEMENTATION.md#qualification-boundary) · [Proxy and Deliver evidence](VERIFIED-RELINK.md#automatic-avid-proxy-attachment) |
| **Premiere → Resolve** · internal | Link the files Premiere identifies as primary. Preserve saved paths, source timecode, trims, and physical channel selectors. The current linked profile is bounded ProRes HQ MOV and PCM16/PCM24 WAV. | Keeps source media available for grading without a new copy. Qualify existing Premiere proxy attachments separately. If only a proxy is available, require an explicit choice before using it as primary in Resolve. [Current profile](RESOLVE-TURNOVER-IMPLEMENTATION.md#inputs-and-review) |
| **Resolve → Premiere** · internal | Link Resolve's primary files in place. The qualified profile includes ProRes HQ picture and two discrete mono lanes from PCM16/PCM24 WAV. | Preserves the saved sample trims and physical channels. Offer existing Resolve proxy attachments after qualifying the pairing in Premiere. Channel-count and layout mismatches can prevent attachment, so retain the saved audio selection. [Native read-back](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere) · [Adobe proxy requirements](https://helpx.adobe.com/content/dam/help/en/pdf/premiere_pro_reference.pdf) |
| **Premiere → Avid** · planned | Start with native linked sources through Avid's current link providers. Qualify ProRes MOV picture and PCM WAV audio first. Reuse verified Avid-managed OP-Atom packages through a separate binding. | Linking in place avoids new media. Verify link-provider identity, reel, source clock, and discrete audio in Media Composer. Qualify any managed proxy paired with a linked original separately. [Premiere → Avid](#premiere--avid) |
| **Resolve → Avid** · planned | Use the same Avid destination writer. Start with Resolve's primary ProRes MOV and PCM WAV sources and preserve the saved mono selectors. Qualify reuse of existing managed OP-Atom packages by package and sibling identity. | Reuses the native Avid linking path. A Resolve proxy needs its own Avid pairing test before it can travel as a proxy. [Resolve → Avid](#resolve--avid) |

### Media choices and Review

- [x] **Use Avid editorial media as primary by default in Premiere.** Rewrap
      is the wizard default. Separate, Grouped, and Merged can link supported
      Avid media in place. Optional proxy attachment requires a detected
      camera original with matching timing and stream layout. Stereo also
      requires matching PCM in both channels. A failed match keeps the chosen
      managed-media policy. Premiere 27 passed with a picture-only MXF proxy
      and matching rewrapped PCM16 and PCM24 MOV proxies.
      [Verification rules](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [x] **Choose primary media separately from clip construction.** The local
      Avid → Premiere wizard offers **Use the media in the Avid edit** and
      **Prefer camera originals for picture** in linked modes. A saved UME
      locator stays primary. Review shows matched camera paths, the physical
      channels used from those files, attached proxies, and reasons for
      rejected camera matches.
- [x] **Build a mixed editorial audio control.** The internal Avid → Resolve
      job reads back a 23.976 DRP with camera picture, an attached proxy,
      independent managed mono MXFs, and an A2 cut to the right channel of a
      linked WAV. Preflight lists the proxy path and estimated bytes.
      [Archive verification](RESOLVE-TURNOVER-IMPLEMENTATION.md#camera-picture-with-independent-editorial-audio)
- [x] **Qualify the mixed audio control in Resolve with originals online.**
      Save, close, reopen, and export retain the camera cuts, proxy, source
      clocks, and both mono lanes. All 48 picture frames and 96,096 PCM24
      samples per lane match a timeline assembled natively from the same
      media. [Native verification](RESOLVE-TURNOVER-IMPLEMENTATION.md#camera-picture-with-independent-editorial-audio)
- [x] **Require camera originals in internal Resolve jobs.** Strict Review
      blocks unverified picture occurrences and editable Group cameras
      before writing a project or proxy. Managed editorial audio remains
      eligible. [Job settings](RESOLVE-TURNOVER-IMPLEMENTATION.md#require-camera-originals)
- [ ] **Expose strict camera-original Review in the Resolve wizard.**
      Give the user a way to open saved-path mappings from a blocker.
      See the [combined managed/UME test](#avid--resolve) for that media gate.
- [x] **Expose verified Avid proxies in the local Premiere wizard.** When
      camera originals are preferred, the user can request matching Avid
      proxies and opt into **Rewrap proxy for compatibility** by choosing a
      folder. Review shows **Creates new media.** and estimates space when a
      stereo proxy is planned. A folder or filename conflict opens the proxy
      folder picker. [Current bounds and editor behavior](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [ ] **Transfer existing proxy attachments between other routes.** First
      qualify the saved pair, picture timing, and audio selectors in the
      destination. Keep use of a proxy as primary an explicit choice.
- [ ] **Share relink identity checks across routes.** Use the full saved path
      or an explicit old-root-to-new-root mapping. Refuse conflicting matches
      when both paths resolve to different files. Keep same-named
      originals distinct, use saved reel and source timecode where qualified,
      and verify managed Avid MXFs by package and sibling track. Keep
      **Verify file contents with SHA-256** off by default and use a trusted
      pre-move manifest when selected. Keep saved and resolved paths, primary
      and proxy roles, exact source clocks, extents, and audio selectors in
      graph/2 where their meanings are shared. Keep the destination's native
      relink behavior in its adapter. [Metadata crosswalk](PREMIERE-RESOLVE-METADATA.md#cross-editor-transfer)
      The Avid → Premiere wizard already offers the current-folder picker for
      saved-path mappings and saves each mapping in the job.
- [x] **Read back Premiere Save As output for a second directed move.**
      Equivalent relative path records with and without `./` are accepted.
      Reapply both reel/timecode and checksum verification to the saved project.
      Premiere 27 saved and reopened the second move with both paths, reels,
      clocks, and trims intact. All 48 preview frames select the expected camera.
      Truly different relative paths, swapped same-name candidates, and
      distinct paths to one hard-linked file on Unix fail before publication.
      [Repeat-move check](VERIFIED-RELINK.md#repeat-a-move-after-premiere-save-as)
- [x] **Offer offline output as an explicit choice.** Separate files, Grouped,
      and Merged offer **Allow offline media** with the option off by default.
      Rewrap requires the source online. Review offers locating the media or
      switching to Separate files with offline output enabled. Track profile
      coverage in the [offline relink checks](#offline-output-and-relink).
      The macOS [verified relink tool](VERIFIED-RELINK.md#verified-scope)
      updates moved MOV links in an existing Premiere or Resolve project.

Prioritize **Avid → Resolve camera originals for picture with editorial audio
preserved**. Keep the tested proxy-only Deliver case as a regression, and
verify the grading setup with the originals online before rollout.

<a id="linked-media-conversion-parity"></a>

## Linked-media conversion readiness

Qualify fidelity against the saved source edit. Check the destination
after save, close and reopen, using source renders and native destination
controls for picture, PCM and editable parameters where relevant. Keep
the closest verified result to the source and report any losses. Ship
the supported profile after its native and packaged checks pass.

Use linked AAF parity as one measure of ship readiness for common
workflows. Reuse retained comparisons. Run a new linked AAF comparison
when it answers a specific release claim, helps diagnose a practical
transfer gap, or could change an implementation decision. A matching
AAF result still needs native evidence for fidelity. Native qualification
can close a case while its AAF comparison remains unmeasured.

This checklist covers links to existing media. Embedding, consolidation,
rendered round trips and rewrap have separate scope. The
[existing holds](#on-hold-user-2026-10-28) still apply.
[AAF Edit Protocol](https://static.amwa.tv/as-01-aaf-edit-protocol-spec.pdf)
· [AAF Effects Protocol](https://static.amwa.tv/as-05-aaf-effects-protocol-spec.pdf)

### Shared graph/2 work and acceptance

- [ ] **Qualify linked edit and media identity across routes.** Reuse graph/2
      source and record ranges, exact timeline and file clocks, known or
      unresolved source offsets, saved paths, source filename, reel or
      package identity, and physical audio selectors. Extend the shared
      checks for duplicate basenames, moved originals, and wrong sibling
      channels. Keep each editor's relink rules in destination preparation.
      [Metadata crosswalk](PREMIERE-RESOLVE-METADATA.md)
      · [Verified relink](VERIFIED-RELINK.md)
- [ ] **Extend portable edit relationships where two routes can use them.**
      Reuse exact transition endpoints and handles, marker ownership,
      ordered picture effects, Group angles and audio selectors, and
      timed text. Add subclip ranges, child sequence references, and
      effect or envelope semantics where both consumers can use the same
      meaning. Retain saved parameters, interpolation, and unresolved
      values on their source owners. Keep native mixer, color, and effect
      encodings with their adapters. [Source graph design](SOURCE-GRAPH-MIGRATION.md)
- [ ] **Give each route a reproducible native acceptance fixture.** Start
      with cuts, a nonzero start and trim, linked picture and two distinct
      mono signals. Reuse retained fixtures where they answer the same
      question. For each added feature compare saved edit ranges and
      controls after reopen, decoded picture frames, and isolated PCM
      where relevant. Record source capture, destination writing, reopen,
      and render separately. Report safe omissions by occurrence and
      block ambiguous media, timing, or routing.
- [ ] **Advance the native routes through the existing product gates.**
      Reuse route-aware review, saved jobs, and exclusive publication.
      Expose a route after its native and packaged checks pass. Retain
      AAF as an interchange reference and use graph/2 as the intermediate
      for native conversion. [Architecture decision](ARCHITECTURE.md#adr-0004-more-programs-go-through-one-neutral-graph-2026-10-26)

### Avid → Premiere

The production route has selected-bin import, scoped editable Avid Groups,
SubCap and Titler+ text, and direct linking for verified OP-Atom and UME
profiles. Separate, Grouped, and Merged pass the tested 23.976 DNxHR LB/PCM24,
23.976 DNxHD LB/PCM16, and 29.97 DF DNxHD LB/PCM24 cases. Editable Groups
use Separate files. [Direct-link read-back](SPLIT-EDIT-AUDIO-2026-11-06.md#grouped-and-merged-direct-linking)
· [Adobe AAF migration baseline](https://helpx.adobe.com/il_en/premiere/desktop/render-and-export/export-files/export-from-avid-media-composer-to-premiere.html)

Qualify the saved Media Composer 25.12.0.58863 edit against the Any NLE
project after Save As and reopen in Premiere 27.0.0 build 88. Check saved
editability and use Avid picture and Direct Out where timing or rendering
matters. Record read, write, reopen, and render coverage per fixture.
Track the [offline output and relink workflow](#offline-output-and-relink)
alongside it.

For a selected [AAF readiness benchmark](#linked-media-conversion-readiness),
export AAF Edit Protocol with all tracks linked, marks off, and rendered
effects off. Retain the import report and check the reopened import
against the same source references. Record where Any NLE matches,
improves on, or falls short of the linked AAF workflow.

#### High-priority conversion fallbacks

Linked Avid → Premiere uses best effort by default through the wizard, direct
CLI, and saved jobs. Use the verified editable translation where available.
Use a qualified saved Avid render, a verified base edit, or an exact-span gap
where the converter can establish the required identity, timing, and routing.
Keep the saved source facts in graph/2 and report the fallback by occurrence.
Contradictory source bindings, corrupt inputs, graph limits, and invalid output
paths retain their publication checks. [Current fallback rules](SOURCE-GRAPH-MIGRATION.md#continue-with-omissions)

Linked Avid → Premiere modes automatically skip verified-unused clips that
can’t link. They use saved package references across the selected
bins, including hidden source branches, to verify that an item has no
editorial reference. Review lists each omitted bin item and reason.
An available original or eligible offline item takes precedence.
An opaque dependency prevents claiming a clip is unused. In the separate
media-gap path, an omitted edit gets a marker when its range and lane can
be verified. A contradictory locator still blocks.

| Priority | Work | Acceptance |
| --- | --- | --- |
| 1 | **AudioSuite Gain and Timewarp.** They accounted for 62 of the 70 saved timing blockers in the ten latest reels of the retained large-project audit. Direct linking now uses a verified dry input for the supported same-rate mono Gain route and retains the edit's independent mix controls. Unsupported Timewarp with proven record bounds uses a verified saved cache when available or a sequence range marker over the gap. | The CLI and saved-job controls preserve exact edit boundaries and select the verified file, dry input, or marked gap. Expand native cache profiles, resolve overlapping transition cases and qualify the whole online project separately. [Fallback boundaries](SOURCE-GRAPH-MIGRATION.md#avid-gain-and-timewarp-fallbacks) |
| 2 | **Extend fallbacks across effects and edit structures.** Preserve a verified base edit when an appearance effect is unsupported. For an unsupported transition, retain the underlying cut when its endpoints and ranges are known. For layered effects, keys, titles, or Groups, use a verified existing render or resolved source selection when it represents the saved edit. Preserve the exact span as a marked gap when no safe input can be placed. Apply the same approach to unsupported Motion Adapter, Freeze Frame, and component-rate cases. | Exercise one appearance omission, a transition at an exact cut, a composite with no proved input, and a retained editable effect in the same sequence. Verify ranges, channel selectors, reporting, and reopen. Unresolved timing on an occurrence stays visible even when the rest of the sequence can convert. |
| 3 | **Contain source-route and media failures.** Extend the [batch diagnostics](#project-import-user-2026-10-14) to group results by timeline and distinguish existing safe omissions and marked gaps from failures that still block publication. Prioritize composition branches, unresolved master/track references, and online media profiles found in selected edits. | Exercise an unrelated bin-only clip, a used clip, and a contradictory source through all three entry points. Confirm that the unaffected timelines retain the same cuts and media assignments. |
| 4 | **Qualify large-project capacity past source capture.** Measure in-memory graph construction, encoding, preparation, and project writing separately. Use a large linked synthetic control for scale and a representative online feature cut for editorial fidelity. Offer explicit bin or sequence selection with required dependencies when the whole project exceeds a measured limit. | Reopen the requested scope in Premiere 27 and check exact starts, edit ranges, and lane counts. Keep the whole-project limit and the fallback selection visible in Review. |

Use the [read-only batch diagnostic task under Project import](#project-import-user-2026-10-14)
to gather blockers and candidate actions in one Review pass. Run the seeded
unchanged controls and exercise these fallbacks through direct CLI, saved jobs,
and the packaged wizard. Record downgraded edits separately from fidelity
passes. Keep [offline-only descriptor and clock coverage](#offline-output-and-relink)
below failures that persist with media online.

#### High-priority linked audio and stills

Qualify these formats for ordinary online Avid → Premiere conversion in
parallel with the fallback work. Link verified originals in place. The
large-project audit surfaced saved MP3 and PNG occurrences. JPEG is a gap
in ordinary still-image linking. The low-level Premiere import builder
already has MP3, `.jpg`, and `.png` support that destination preparation can
reuse once it has qualified source facts.

- [x] **Write linked 48 and 44.1 kHz MP3 from matching MPGA descriptors.** The reader
      verifies the full decoded sample extent and saved physical selectors.
      Derived stereo controls retained two independent mono lanes and a
      one-second source trim after Premiere 27 Save As and reopen. At 48 kHz,
      the 96,000-sample lanes matched a separately decoded source with zero
      sample shift. At 44.1 kHz, both 88,200-sample lanes matched a separate
      native Premiere import with the same trim and output settings bit for
      bit. Fresh insertion retained the four-second playable extent at both
      rates. The saved Avid sample rate must agree with the original file.
- [x] **Preserve a qualified 25 fps sequence origin.** A unique same-rate
      non-drop TC1 supplies the exact frame count through graph/2. Both
      entry points wrote `01:02:03:04` for the test sequence, and Premiere 27
      preserved it after Save As and reopen. Edit ranges stayed relative to
      the sequence start.
- [x] **Write linked `.jpg`, `.jpeg`, and PNG stills in the 1080p24 RGBA profile.**
      The reader requires a single decoded image and retains the separate Avid
      hold duration. Derived controls retained 36-frame trimmed placements
      after Premiere 27 Save As and reopen, with matching first and last
      frames. Two fresh inserts retained the saved 30-second hold. A PNG
      over a JPEG retained transparency at both ends of the composite.
- [x] **Link the 1920×64 RGB16 TIFF test-pattern profile.** One saved
      24 fps image supplies a separate editorial hold. Avid displays it as a
      thin centered strip in the HD source monitor, so its physical raster
      stays on the media and does not set the sequence canvas. The route
      verifies a single top-left RGB TIFF with no embedded ICC profile.
      Saved Avid source color transforms remain visible in graph/2 and the
      existing loss report.
- [x] **Translate interior trims from the fresh 48 kHz MP3 link.** The saved
      Avid 25.12 MPGA descriptor counts a leading MPEG `Info` frame. For the
      qualified stereo profile on a 25 fps sequence, graph/2 retains the
      saved trim and subtracts 2,257 samples from the linked-file in-point.
      Both entry points write the unchanged edit duration and independent
      mono selectors. Two isolated 96,000-sample renders after Premiere 27
      Save As and reopen matched the decoded source with zero sample shift
      and a silent opposite channel. A center-pan sum matched the same span
      of the full Avid export within 0.000004 FS RMS. The edit fixture adds
      a synthetic trim to the native Avid master. Conversion reports that
      fresh insertion uses the gapless file extent.
- [x] **Translate interior trims from the fresh 44.1 kHz MP3 link.** The
      qualified 25 fps stereo profile uses the same 2,257-sample correction
      on its own 44.1 kHz clock. A synthetic trim on a native master retained
      the original link, both mono channels, and the exact source trim after
      Premiere 27 Save As and reopen. Both lanes matched a Premiere direct
      import sample for sample. The Avid comparison at 48 kHz has a slight
      phase difference, reported by edit in Premiere preparation.
- [ ] **Extend the fresh MP3 timing coverage.** Qualify native edits with a
      different sequence rate and edit boundaries that include Avid's
      priming or padding. Edits outside the qualified gapless range still
      produce a diagnostic. Keep Avid-managed PCM from an imported MP3 on
      its verified managed route.
- [x] **Translate the centered 1536×1024 RGB PNG fit on V1 at 1080p24.**
      The saved static `INT_EFF_SPATIAL_ADAPTER` becomes editable Premiere
      Motion at 105.46875%. After Save As and reopen, the first and last
      frames match the equivalent native Premiere control bit for bit.
      Both editors show the same 1,620 picture columns and 150-pixel side
      borders. The source color and border levels differ and are reported.
      Fresh insertion retains the 30-second hold. Other saved framing modes
      and color mappings still need qualification.

Exercise both conversion entry points and fresh insertion. Reject a mismatched
file or unresolved source range and report its occurrence in Review.

<a id="remaining-parity-work"></a>

#### Remaining native qualification

Avid-authored DNxHD LB at 23.976 with PCM16 and 29.97 DF with PCM24 has
passed in retained Separate files, Grouped, and Merged controls. The
independent A1 Group switch, simple trimmed Submaster, and reusable
A/V subclips also have reopened controls. Keep those as regressions, alongside the mixed UME
edits and the editable dissolves. The detailed results below retain the
native evidence and the AAF benchmarks already measured.

| Work remaining | What is ready | What closes it |
| --- | --- | --- |
| Source range markers from native edits | The reopened Premiere 27 linked AAF retained the copied point as a sequence marker and carried no ranges. Any NLE retained both 22-frame ranges and both points on the picture edits, with their names, comments, and colors. Both projects kept the sequence start and all six edit ranges. | **Passes for the retained source copy.** Direct Source Monitor authoring remains a separate evidence case. [Native source copy and linked AAF results](SPLIT-EDIT-AUDIO-2026-11-06.md#native-source-range-and-point-copy) |
| Source identity and audio for same-named originals | Premiere 27 reopened the linked AAF with the correct picture and both clocks, but dropped all six audio edits. The native Avid bin now converts in Separate files mode for the verified 24 fps H.264/AAC MOV profile. After Save As and reopen, all 36 pictures match the AAF and both 72,000-sample PCM24 channels match Avid Direct Out exactly. The two original bin items stay stereo. | Already placed mono edits use the verified one-frame Avid route offset. Fresh insertion uses the MOV clock and does not reproduce that offset. Edits beyond playable audio and unresolved offsets are refused. Other AAC profiles and clip constructions are not qualified by this test. [Linked A/V control](VERIFIED-RELINK.md#native-same-name-av-control) |
| Avid volume envelope fidelity beyond linked AAF | At 24 fps, the shifted-key Any NLE and linked AAF renders agree within one PCM24 count after reopen. At 23.976, Premiere 27 retained four of five AAF points and all five Any NLE points at their exact sample times. Against Avid Direct Out, the Any NLE envelope measured 0.432 dB RMS error over the edit, and the linked AAF measured 4.156 dB. | The editable comparison passes on the tested 23.976 mono edit. Exact Avid render matching remains beyond AAF. A measured slope change near sequence sample 32,768 does not explain the short ramp at 65,536. Keep the interpolation report until the short-ramp behavior is understood. [PVOL measurements](SPLIT-EDIT-AUDIO-2026-11-06.md#mirrored-pvol-volume-ramps) |
| Wider Group and nest structures | The independent A1 switch and simple trimmed nest pass their targeted comparisons. Separate files writes a scoped 23.976 three-mono Group. The native Avid control preserves all three mono routes, including the independent A3 switch. A fresh Premiere 27 Source Monitor edit defaults to camera B and retains V1 and all three independent mono lanes after Save As and reopen. The direct CLI and saved job write the same two editable Submasters from a derived AVB with staggered picture layers. They report the third occurrence separately because its source trim exceeds the file. Eight-mic AutoSync has its own independent-lane qualification. | **Deferred: layered Submasters.** When resumed, compare a native Avid Collapse with the staged flat and nested renders. Test child audio and inner effects separately. A linked AAF can benchmark what remains editable. [Three-mono Group test](EDITABLE-GROUPS-2026-11-08.md#three-independent-mono-group-outputs) · [Layered Submaster evidence](SOURCE-GRAPH-MIGRATION.md#layered-submaster-inputs) |
| Final packaged replay | The updated four-bin macOS package passed Review and Premiere 27 reopen with the same-name AAC case, mixed managed/UME, marked dissolves, and trimmed Submasters. The AAC and mixed managed/UME renders match their picture and PCM references. [Package replay](VERIFIED-RELINK.md#combined-macos-package-replay) | Repeat the affected controls after conversion changes. The Sound Roll, moved picture reels, upper-track Submaster, and independent Group audio combination also passed wizard conversion and Premiere 27 reopen. [Group audio and fresh insertion](EDITABLE-GROUPS-2026-11-08.md#audio-layouts-and-groups) |

The offline exact 24 fps layered Submaster control now writes the
two child tracks and matches the flat source selection at every
frame. Selecting a second bin with a copy of the same master now
preserves the nest and uses the copy in the sequence's bin. A range
marker on the outer Submaster also survives on the parent sequence,
even when it ends in the following cut or in another qualified nest.
Premiere 27 saved and reopened the derived 23.976 marker spanning two
nests. Its full range, edit ranges and media bindings survived. All 57
rendered picture frames match the unmarked companion. The flat-render
comparison and individual reopen checks for the other controls remain
pending. Linked AAF coverage is unmeasured.
[Layered Submaster evidence](SOURCE-GRAPH-MIGRATION.md#layered-submaster-inputs)

The native three-mono Group uses B/A/B picture, fixed camera B on A1
and A2, and B/A/B on A3. Only the middle A3 edit switches audio.
The linked AAF retains all twelve source selections and trims and
imports as picture cuts and independent mono clips in Premiere 27.
After Save As and reopen, the initial Any NLE project matched all 72
decoded frames and retained editable picture angles. The middle A3 edit
played from its linked MXF, and the two outer A3 segments from the
multicam source rendered silent. An isolated Premiere control restored
both segments by declaring eight adaptive channels on the inner Group
bus. The fixed converter project passed Save As and reopen. All 72
picture frames match the AAF, and each isolated 144,144-sample PCM24
render agrees within one count per channel with the same center pan.
Premiere's native Source Monitor edit defaults to camera B on a fresh
multicam placement and reproduces all three default camera B audio
lanes after Save As and reopen. Camera B's 96 frames and 192,192 audio
sample frames per lane are followed by six frames of black and silence
at the end of the full Group span.
[Three-mono Group test](EDITABLE-GROUPS-2026-11-08.md#three-independent-mono-group-outputs)

DNxHR LB with PCM16 and wider 29.97 managed profiles remain media
qualification tasks. Add a representative Avid specimen when a codec,
clock, or channel layout changes the conversion path. The existing DNxHD
checks already cover all three linked clip constructions, so another
round across equivalent variants adds little evidence.

An archived editorial cut exposed Resolve-authored Avid OP-Atom DNxHR LB
with the `0A` picture coding label and Avid picture essence element key.
The source reader recognizes the exact label and rewrap recognizes the
observed element. Package identity and clock checks still apply. The
rebuilt macOS package's saved-job replay opened a derived 669-frame excerpt
in Premiere 27. After Save As and reopen, all three picture ranges and ten
mono audio ranges matched. Both 1,339,338-sample lanes matched the selected
source PCM, including silence through the shared gap. A repeat export matched
all decoded picture and PCM. The excerpt keeps the original V1 and A1 cuts
and adds the corresponding second source channel as an independent mono lane.
It qualifies OP1a rewrap for the tested media profile.

The three 24 → 23.976 audio-only Motion Adapters now retain their exact sample
origins from managed mono PCM16/48 kHz. The isolated Premiere 27 replay kept
both editable six-frame Constant Gain dissolves, −12 dB gain, and center pan.
After Save As and reopen, the 4,034,030 sample frames matched the archived
Avid SFX stem within three PCM24 counts per channel. The one-mono editable
Group replay used two DNxHR LB cameras, each rewrapped with eight PCM24 lanes.
The existing two picture edits and the single selected audio lane survived
Save As and reopen. All 120 picture frames matched the selected sources and
the saved A1 audio matched its original samples exactly under the tested
hard-left policy. With native Source Monitor insertion the saved default
camera is first in Premiere's angle order and the Group exposes one mono
audio lane. The diagnostic reports the angle order change.

The original 2,015-frame cut now converts in Separate files mode with the
original MXFs linked in place. A7 keeps its 12-frame stereo Constant Gain fade
into filler and its saved music level. Premiere 27 Save As and reopen kept the
fade editable. The isolated lane from the reopened full cut matches the
archived music stem within eight PCM24 counts per channel across the fade.
The cut also retains three editable one-mono Groups, all nine V1 Group edits
under 3D Warp, and the 11 independently selected A1 audio edits. This covers
static Motion on a same-rate, full-span Group input with an empty Warp
background.

The 14 V2 edits in the archived cut use SpectraMatte inside a static
3D Warp. The source graph retains the RGB key and effect order. For the
verified 1080p23.976 green-screen profile, Premiere writes editable Ultra
Key before Motion and keeps the foreground linked to its original MXF.
All 14 edits survived Save As and reopen. The converter reports the
matte approximation on each occurrence and omits unsupported overlays
so their opaque foregrounds do not cover the lower track. The V1
insert's remaining 3D projection, color, and caption appearance losses
remain open.

#### Multi-reel feature cut audit

An offline scan covered ten current-cut bins and their saved sequence versions.
All ten original bins now pass source graph export with their saved sequence
versions, including the four that previously exceeded the source budget. The
largest graph is 69 MiB. A separate linked, two-hour control writes 64,800
picture and audio edits on nine independent lanes through both the direct
CLI and a saved job. Premiere writer inspection covers both outputs.
See [large project limits](SOURCE-GRAPH-MIGRATION.md#limits-and-memory).

- Online 2048×858 AVC MOV at exact 24 fps can link in place when the
  stereo PCM24/48 kHz file matches the saved Avid descriptors and
  channel selectors. Separate files retains the stereo bin source and
  two independent mono timeline edits. In a synthetic Premiere 27
  Save As and reopen, all 36 trimmed frames matched a Premiere direct
  import and both 72,000-sample mono lanes matched the source under the
  Avid pan policy. Entire Project Review still stops at the required
  offline MOV after reading all 1,903 bins.
- A full-bin source audit identified a managed bin clip whose saved picture
  rate is outside the supported exact clock family. Its master and picture
  rates agree. Linked conversion can omit that
  occurrence when dependency checks prove it unused, even if the media
  is available. A cross-bin editorial reference and a contradictory
  master rate each still block Review. Separate checks remain for the
  saved mixed-rate components, source routes without captured tracks, and
  whole-project construction and writer capacity.
- Review recognizes the nine saved drive-like MXF locators as one picture
  and eight mono packages. It can recover matching files by package ID in
  selected Avid media roots and retains the saved locators and selectors.
  A verified-unused omission still requires a complete dependency
  proof. The archive contains a 15-frame audio reference inside an effect
  stack, so this bundle still needs its media. Native playback is pending.
- Whole-project diagnostic Review read all 1,903 bins and passed the
  installed ColorBars TIFF and the 1920×1080 PNG slate with temporary MOV,
  MP3, and PNG controls. The next stop was an offline 3840×2160 MP4 at
  23.976 fps with two saved PCM16/48 kHz channel selections. The bin
  contains 21 saved MP4 paths with the same descriptor families and
  clocks. Linked Separate files now qualifies progressive square-pixel
  3840×2160 AVC High Level 5.1 MP4 at exact 24000/1001 with stereo
  48 kHz PCM16 or PCM24. Both synthetic Avid-linked clips retained
  picture and two mono timeline lanes after Premiere 27 Save As and
  reopen. All 36 trimmed picture frames and all 72,072 samples per
  lane matched their references for each depth. The archived originals
  still need onlining to confirm which meet these checks.
- Online 23.976 UHD AVC High Level 5.1 MP4 with stereo AAC-LC at 48 kHz
  now links in Separate files mode. The saved Avid file-track start
  advances existing audio edits by one source frame, independently of
  the MP4's skipped 1,024-sample priming packet. Premiere 27 Save As
  and reopen preserved all picture frames, both mono trims, and audio
  through the 96,096-sample playable endpoint. An edit through the last
  picture frame keeps its picture and reports the remaining silent
  span on each audio lane. Avid Direct Out decoded 160 more samples
  from the final AAC packet in this fixture.
  Native Premiere 27 trims in two disposable projects also produced
  different A2 waveforms over the same 512-sample source interval,
  with unchanged picture, A1, trim, and playable end. Both observed
  converter waveforms matched native trim or full-clip output exactly.
- **Best-effort conversion** published the full 1,903-bin archive as a
  Premiere project with 1,217 saved sequences. The destination omitted
  159 unqualified offline managed masters and marked 92 additional
  exact gaps. Fifteen qualified offline locators remained in the relink
  report. Structural read-back verified the output and the occurrence
  report. Premiere playback of the archived project remains untested.
  [Fallback scope](SOURCE-GRAPH-MIGRATION.md#continue-with-omissions)
- [ ] **Bound working memory for whole-project conversion.** Measure each
      stage separately. Use a compact project-wide identity index and
      spill completed bin and timeline records and diagnostics to disk.
      Preserve cross-bin reference, clock, and lane validation. Measure
      destination preparation and writing independently on the same
      full project with a peak-memory target. Linked Entire Project
      jobs now stage native timeline receipts and high-volume omissions
      under a separate working graph budget. The recorded dissolve
      can be omitted with an occurrence-level loss when neither
      endpoint survives and the lane is empty over its entire range.
      An earlier full offline replay peaked at approximately 4.02 GiB
      resident memory and published about 2.4 GB of staged diagnostics.
      A repeated offline baseline peaked at 4.05 to 4.16 GiB RSS.
      The instrumented run reached 3.63 GiB during source capture,
      4.05 GiB during graph construction, and 4.16 GiB while
      assembling Review after encoding.
      Sampled RSS peaked at 2.64 GiB during typed destination
      preparation, 3.14 GiB during document construction, and
      2.90 GiB during XML and gzip encoding.
      Trials shortened the lifetime of the typed input and of consumed
      source graph fields, and eliminated a full XML buffer copy.
      Whole-process RSS failed the acceptance threshold for all three.
      Releasing consumed graph fields moved the measured peak from
      Review to working graph construction. The Avid reader now
      allocates optional picture extensions, matte inputs, and color
      adapter controls only when present. Source events fell from
      1,080 to 800 bytes and source issues from 600 to 280 bytes on
      arm64. This lowered the accounted source peak by 290 MiB.
      Two compact whole-project replays peaked at 4.26 and 4.33 GiB
      RSS, within the 4.23 to 4.75 GiB baseline span.
      Measure the retained source routes before designing a disk-backed
      source index.
      [Working graph and remaining allocations](SOURCE-GRAPH-MIGRATION.md#limits-and-memory)
- [ ] **Batch-qualify Avid camera links across container and raster.**
      Save one Avid bin of numbered clips with distinct mono signals.
      Use H.264/AAC and H.264/PCM MP4 and MOV, ProRes/PCM MOV, and
      qualified DNxHR/PCM MXF as controls. Cover SD, HD, UHD, and one
      nonstandard raster at representative exact and fractional rates;
      test PCM16, PCM24, stereo splits, and independent MXF siblings.
      Avid linked all 30 synthetic masters. Premiere 27 direct import
      kept all 30 online after Save As and reopen. Any NLE retained eight
      masters, omitted five and refused 17 in isolated replays. The
      translated 1080p AVC/AAC MOV, 2048×858 AVC/PCM24 MOV and six
      mono or stereo PCM16/24 WAVs at 44.1/48 kHz stayed linked after
      reopen. Fresh insertion rendered all numbered frames and the
      expected channel signals. Avid-authored trims on this matrix and
      the remaining container and channel profiles are pending.
      Verify media raster and pixel aspect independently of the sequence
      canvas. Compare nonzero trims, picture, and isolated audio after
      Premiere Save As and reopen. Generalize raster admission where the
      codec, media clock, audio mapping, and destination geometry agree.
      The follow-on 25 and exact 30000/1001 matrix qualifies 30 more
      Avid-linked sources in Separate files mode. It covers 720p, HD,
      2048×858, and UHD AVC MP4/MOV with AAC-LC or PCM16/24, HD Main 10
      HEVC/AAC MP4, HD ProRes HQ MOV with or without PCM24, and HD DNxHR
      LB OP1a MXF with PCM16/24. The converter checks both saved MXF
      package IDs. When a file stores two mono streams, graph/2 keeps
      both physical stream selectors and Premiere uses one media object
      per stream. All 1,080 trimmed picture frames matched direct
      Premiere 27 imports after reopen. Both mono lanes retained the
      measured trim and channel identity. 25 fps PCM was sample-exact.
      The 29.97 PCM exports ended with one silent rounding sample.
      The ProRes drop-frame control preserved its saved source start
      and crossed from 01:00:59;20 to 01:01:00;02 ten frames later.
      Other AAC layouts and fractional-frame AAC edits beyond the
      playable extent remain separate cases.
      Stereo PCM16/24 at 48 kHz in progressive, square-pixel H.264 High
      level 4.0 or 5.1 MOV/MP4 now uses the same saved-raster, frame-rate
      and channel checks at 24, 23.976, 25 and 29.97. The PCM extent is
      rounded once from the complete source duration. For a sequence
      without a named preset, the writer can use an explicit raster
      when every picture edit verifies the same raster and rate in
      this linked profile. Derived AVBs with 1600×900, 1440×1080 and
      1536×864 sources kept their settings, trims, links and channel
      selectors after Save As and reopen in Premiere 27. All 12 sampled
      pictures selected the expected source frames. The 29.97 test file
      retained 76,876 PCM sample frames for 48 picture frames. Full
      exports of the new rasters remain pending.
- [x] **Decouple linked AVC picture geometry from audio layout.**
      The explicit raster and matching-raster picture effects use the
      verified picture stream. Audio keeps its own source and writer
      checks. Picture-only, progressive square-pixel H.264 High MOV/MP4
      at 24, 23.976, 25 and 29.97 now links at the verified source raster.
      The new source profile checks the saved picture descriptor, zero
      origin and complete fixed-rate packet clock. CLI and saved jobs
      produced matching cuts and trims on 12 derived timelines.
      After Premiere 27 Save As and reopen, 20 sampled frames matched
      their PCM-bearing controls pixel for pixel, including Corner Pin,
      source crop and both Pin/Flop orders. Unit checks also cover
      applying the same geometry with AAC already admitted.
- [x] **Use the matched-raster path for linked ProRes 422 HQ MOV.**
      The Avid reader verifies progressive square-pixel picture at 24,
      23.976, 25 and 29.97, the complete packet clock and the saved
      descriptor. It checks the embedded timecode track when present
      and verifies any stereo PCM16 or PCM24 bindings independently.
      Eleven derived timelines at 1600×900, 1440×1080 and 1536×864
      agreed across direct CLI, saved jobs and Premiere 27 reopen.
      All 109 plain-cut picture frames matched Premiere direct imports.
      Six Corner Pin and Flop controls matched the projected chart
      points within 0.27 pixels. Both 23.976 PCM16 lanes matched exactly.
      The 29.97 drop-frame PCM24 case preserved the source timecode and
      reproduced the retained 1080p audio, including its one silent
      rounding sample.
- [x] **Stream the linked ProRes HQ picture clock verification.**
      Verify every packet through EOF with one bounded row at a time.
      Exact packet timing and the declared count still have to agree.
      The verifier handles reports over 1 MiB and sources over 16,384
      frames. The 50,000-frame controls include trims beyond the former frame
      limit and a late two-packet discrepancy that preserves the
      total duration and declared rate.
- [x] **Stream the picture-only H.264 clock verification.**
      Read one packet row at a time and track the presentation slots
      in a bitmap of at most 128 KiB. This allows decode-order B-frames
      and detects missing or repeated slots through EOF. The
      1,048,576-frame count bound still applies. The
      81,600-frame controls exercise a report over 1 MiB and a late
      duplicate presentation timestamp with the same declared rate
      and duration.
- [x] **Keep linked picture packet scans running while they make progress.**
      The ProRes HQ and picture-only H.264 verifiers refresh a 30-second
      idle deadline after validating packet rows. The cancellable source
      reader checks for cancellation during those scans and reaps its
      FFprobe child on a stall or cancellation. The converter still
      requires the complete packet proof and successful process exit.
      Short metadata probes retain their total deadline and output bound.
- A rejected Gain, automatic Motion Adapter, or generated Freeze Frame
  can become a marked gap when the reader proves its exact record range
  and the lane has no conflicting placement. When a Gain gap borders a
  saved audio dissolve, Premiere can omit that dissolve at its saved cut.
  The saved adjacency must resolve to an ordinary linked trim, known
  filler, or a second Gain gap. The latest-cut audit replaced seven
  additional timing blockers with markers and retained nine related
  dissolve records. A linked mono control preserved both trims and the
  exact silent span after Premiere 27 Save As and reopen. The reel
  neighbors still need online playback verification. Whole-project output
  and the unused-media blocker remain open.

- The saved audio track-insert container now keeps its underlying edits and
  transitions when the first slot is the full-span program and the other slots
  contain only verified plug-in filler. Review reports each unmapped insert.
  The offline graph comparison recovered 2,139 audio edit facts and 675 native
  transition facts across the ten latest cuts.
- Standalone legacy Titler+ retains plain text and cue timing when both saved
  text carriers agree and both picture inputs are empty. Premiere uses the
  existing editable graphic path and reports appearance loss. Conflicting or
  missing text and unsupported owner timing remain explicit.
- Audio Pan Volume inside a subclip accounted for 3,345 unresolved branches.
  The same-rate, single-source profile with unset unity gain and saved
  center pan now recovers 3,184 audio edits across the ten cuts. Graph/2
  retains the inner settings alongside the existing timeline gain and pan.
  Following additional subclip hops with the same controls recovers 60
  more audio edit routes, 53 through two saved wrappers and seven through
  three. Each additional hop must cover the entire requested range in
  one qualified audio component. Four native dissolve endpoints and one
  Group use now bind to the recovered source events.
  A single unlabeled wrapper input with a saved zero lock recovers another
  56 routes. The source clip's track label selects the master lane. Any
  saved AAF track number on the wrapper must agree with that label.
  Another 45 edits carry explicit subclip gain. Graph/2 retains the
  saved level, pan state, exact trims, and selected master lane.
  In the derived 24 fps mono control, Avid Direct Out and the stereo mix
  both bypass the subclip gain and apply the independent timeline gain.
  For verified same-rate mono PCM24, the converter now restores the edit
  and reports that it used the original sound. Premiere 27 Save As and
  reopen kept all edits, including a higher-numbered managed channel.
  The original reel media stayed offline. Other pan settings, reusable
  audio-only subclips and broader subclip mixes remain open.
  Broader subclip mixes and timing, generalized AudioSuite render linking,
  and upper-track composites remain open.
- Unrendered legacy Avid clip EQ with a full-span 24 fps mono input
  now retains the linked edit when the original managed 48 kHz PCM
  verifies. Graph/2 keeps the saved EQ enable and raw band controls.
  Review identifies each enabled EQ that was bypassed. Premiere 27
  Save As and reopen preserved both the source trim and independent
  timeline gain and pan. The PCM24 render matched the dry Premiere
  control sample for sample. The linked route can also use the saved
  render for a full-span same-rate mono EQ with matching source and
  effect facts. Premiere 27 reopened and rendered the tested PCM24
  cache, including a separate timeline gain and pan. A same-rate mono
  subclip now qualifies when both saved routes resolve to the same dry
  file, channel and range. An online subclip control with a nonzero
  trim likewise matched the native cached audio in Premiere 27.
  When that cache package is missing, direct linking keeps the dry edit
  if the saved master lane, package and PCM range verify. The loss report
  names the missing cache and bypassed EQ at the affected edit.
  A reopened Premiere 27 control matched all three seconds of dry PCM
  on the selected channel.
  Trimmed edits can use a matching portion of a longer verified cache.
  Graph/2 retains the full cache length and the saved in-point. The
  offline audit verified six such source bindings, including two with
  nonzero in-points. One saved usage-7 mono PCM24 master also qualifies
  when its neutral Pan Volume input matches the precompute's copy.
  A matching static-gain wrapper on a mono PCM24 master can also use
  the cache. Avid's test export confirmed that the EQ playback bypasses
  the inner gain. Independent timeline gain and pan remain editable.
  The final saved layered occurrence nests Pitch II inside EQ. A derived
  24 fps mono control confirmed that Avid plays the outer 96,000-sample
  EQ cache. Direct linking uses that verified PCM24 cache and preserves
  both saved bindings in graph/2. Premiere 27 Save As and reopen matched
  all 144,000 samples of the three-second Avid control, including the
  two dry neighbors. A missing outer cache leaves a reported gap.
  Playback of the archived occurrence remains pending while its media
  is offline. Other nested plug-in profiles, prepared-media rewrap and
  editable EQ remain open.
  [Legacy EQ fallback](SOURCE-GRAPH-MIGRATION.md#avid-gain-and-timewarp-fallbacks).
- A standalone mono AudioSuite effect can now link its saved PCM24
  render on a 24 fps timeline. The saved copy must match the plug-in
  settings and the full source edit. Avid D-Verb and Pitch II controls
  each played 96,000 samples from a cache with two trailing zero
  samples. After Premiere 27 Save As and reopen, both three-second
  exports matched Avid Direct Out exactly. The offline audit identified
  14 structurally matching render references, including 13 FabFilter
  Pro-R 2 edits and one Timeless 3 edit. Their MXFs remain offline.
  A shorter edit can also link an exact window into the saved render.
  The head-and-tail and tail-only D-Verb controls matched Avid Direct
  Out after Premiere 27 reopen. Three more archived edits have matching
  saved bindings. Missing or invalid caches leave a reported gap.
  One same-rate subclip hop now qualifies when its selected lane and
  exact offset agree with the saved cache input. The cached copy can
  contain two neutral Pan Volume wrappers. Full-span and trimmed
  D-Verb subclip controls matched Avid Direct Out after Premiere 27
  reopen. Twenty-one more archived mono edits have matching bindings.
  A single neutral Pan Volume wrapper on the cached copy also
  qualifies for both direct masters and subclips. The two D-Verb
  controls matched Avid after Premiere 27 reopen. Nine more archived
  edits have matching saved bindings.
  A neutral Pan Volume wrapper can also contain a single sound segment
  in a sequence with matching saved AAF routing. Both direct-master
  D-Verb controls matched Avid Direct Out after Premiere 27 reopen.
  The ten archived reels retained their existing placements and
  accepted bindings on replay.
  Nested plug-ins and stereo processing require separate qualification.
  [AudioSuite cache scope](SOURCE-GRAPH-MIGRATION.md#standalone-audiosuite-renders).
- Audio-only automatic 23.976 → 24 Motion Adapters on managed mono PCM24
  retain the saved source range and use the absolute 48 kHz sample position
  even when the saved phase wraps. The derived online control preserved
  three edits across two mono lanes after Premiere 27 Save As and reopen.
  The waveform matched the verified source starts with zero sample shift.
  The saved Pan Volume key stays editable, with an explicit notice that
  Avid envelope playback is unqualified. The reel originals remain offline.
- A saved 24 fps stereo master can link two verified PCM16 MXF siblings
  when both lanes end with a separate one-frame MPEG audio piece. The
  source graph records the cutoff and the saved MPEG package. The
  converter refuses edits or transition ranges that cross the cutoff.
  One retained reel has 13 stereo edits fully inside the PCM section.
  A derived online control in Premiere 27 preserved three trimmed edits
  and the full reusable PCM range after Save As and reopen. Both output
  channels matched the source samples exactly. The original reel media
  remains offline.
- Plain nested 50%, 150%, and 200% picture cuts transfer as editable
  speed on the qualified 1080p24 managed DNxHD LB profile in Separate
  files mode. Premiere 27 retained the odd and even trims after Save As
  and reopen, including half-frame source In values. All 120 rendered
  frames selected the same source frames as Avid. The same six trims under
  a neutral video-to-full-range Color Adapter also retained editable speed,
  the exact trims, and an editable ASC CDL. The reopened renders select
  the same 120 source frames as Avid. The 50%, 150%, and 200% cases have
  the same 95th-percentile level differences from the Avid reference.
  Linked picture now uses a general positive constant-speed rule when
  the saved Speed and Position agree and Premiere can represent the trim
  exactly. Premiere 27 Save As and reopen verified 25%, 80%, 125%, and
  250%, plain and with the same Color Adapter, across 320 frames.
  Plain nested edits now share this rule across verified same-rate
  managed and original links. Reopen testing covers two speeds and two
  trims each on 23.976 DNxHD LB MXF and 25 fps ProRes HQ MOV. The 23.976
  render agrees with Avid, and the 25 fps render agrees with the saved
  cadence and the original picture.
- Color Adapter source binding now follows one direct 24 fps picture
  subclip to the master and file, checking the entire trimmed range at
  each hop. The offline reels gained 3,140 matching file receipts. A
  derived linked control writes the same two editable external LUT
  placements through direct CLI and saved-job conversion. Conflicting
  transforms and a shortened subclip leave the occurrence losses visible.
  A same-rate subclip sequence with trailing filler contributes 40 more
  matching receipts when the selected range ends inside the picture.
  A range that crosses into filler leaves its warning. The remaining
  82 retimed uses now retain the input range, saved Speed and Position
  controls, span ratio, and phase and render settings in graph/2. Those
  edits still require verified picture-file bindings before placement.
  In a 24 fps native Avid control, the same 20-frame trim at 200%
  selected source frames 56 through 94 in steps of two, both by itself
  and under a range Color Adapter. Graph/2 now records those selected
  master frames for the matching direct-master control family. Separate
  files now converts the verified 1080p24 managed DNxHD LB and PCM16
  profile with an editable 200% retime. An editable ASC CDL can reproduce
  the range expansion when the original picture has verified Rec.709
  video levels. Native rewrap now also covers the verified picture-only
  1080p24 ProRes LT profile. Premiere 27 saved and reopened the editable
  retime and range correction. One additional zero-offset picture
  subclip hop now preserves the same sampled frames after reopening.
  The offline audit recovered 13 file routes with that hop. Two match
  the saved wrapper and position checks, including one within the
  current file verification bound. A verified gray clip-color triplet
  on the retimed Color Adapter lets ten more saved caches qualify for
  media verification, including eight with the extra subclip hop.
  The 20-frame Premiere reopen matches the prior render exactly.
  A project-local, unit-domain 33×33×33 external LUT also survives
  rewrap on this retime as an editable Lumetri Look. Avid and Premiere
  each rendered explicit and implicit unit-domain controls identically.
  A 10,021-frame Avid-native synthetic MXF now verifies across two
  consecutive ProRes LT index segments. Premiere reopens the 200% edit
  crossing that boundary with its editable LUT and exact source trim.
  Bounded streaming packet verification raises the source limit to 32,768
  frames. A synthetic 18,704-frame Avid import exercises all three joins
  across four index segments.
  The two offline reel uses have a compatible LUT table and the same
  declared source length. Their actual MXF still needs verification.
  Nonzero inner offsets, nonzero phase, and the real reel media still
  require their own checks.
- Across the latest cuts, 3,114 standalone video-to-full-range
  Color Adapters have a matching picture-source receipt. Graph/2
  retains the exact 10-bit endpoints and direction on both the source
  file and the sequence occurrence where present. In the online Rec.709
  ProRes HQ chart, Avid expanded neutral patches from codes 64–940 to
  roughly 4–1019 in a tagged legal-range export. Premiere 27's direct
  link kept the original levels. The qualified Separate files path puts
  an editable ASC CDL on a plain 1080p24 edit of the verified picture-only
  ProRes HQ original. The source and edit payload must match, and the
  file must have explicit Rec.709 video levels. Plain edits of the same
  original stay neutral. A derived online linked control retains the
  adjustment on V1 and V2 while preserving a separate centered dissolve.
  The direct CLI and saved job were checked by structural read-back.
  In the single-lane control after Save As and reopen, the 24 expanded
  frames match the editable native Premiere control exactly. The
  sampled chart values are within two code values of Avid's 10-bit
  processing export, with identical black and white endpoints.
  ProcAmp and a Lumetri Input LUT missed the
  Avid render. Conversion keeps the cut and reports an unapplied
  adjustment for other profiles. The 1080p24 picture-only managed ProRes
  422 LT control now also qualifies through **Rewrap to native clips**.
  Avid saved the effect on the edit and retained a picture-position
  reference into the MXF. The converter checks that reference against the
  file package and indexed bytes before adding ASC CDL. After Premiere 27
  Save As and reopen, all 24 neutral frames matched the neutral Premiere
  render, and the 24 adjusted frames matched Avid's sampled 10-bit levels
  within two codes. Both edits retained source frames 6–29. The offline
  production reels still need their media to verify each occurrence's
  binding. Wider clocks, effects and track layouts remain open.
  Empty and populated marker lists on the managed adapter's picture
  input now travel with the effect. A point and a seven-frame range
  marker stayed on the correct edit after Premiere 27 reopen. One
  same-rate picture subclip hop now qualifies when its combined trim
  agrees with the backing MXF index. The tested native subclip
  preserved the same picture, editable effect and markers as the
  direct-master control. An exactly matching range transform saved
  on the adapter input now reuses the same edit effect, with one
  editable ASC CDL verified after Premiere 27 reopen. A conflicting
  source-clip transform reports a loss on the affected edit. The
  offline reels now yield 2,687 saved managed picture position receipts.
  Managed LT adjustments now qualify on verified edits on multiple tracks
  with a dissolve elsewhere on the timeline. Premiere 27 reopen preserved
  the editable corrections on V1 and V2 and the independent V1 dissolve.
  Verify real reel media, full Avid layer compositing, and effect order
  on transition endpoints separately.
  Managed picture verification now scales by indexed frame count and
  distinct inputs. A synthetic 209-package reel passes the former
  32-picture boundary while retaining per-edit range checks.
  [Avid levels reference](https://resources.avid.com/SupportFiles/attach/Media_Composer/Media_Composer_v2025.x_Editing_Guide.pdf)
  · [Adobe effect controls](https://helpx.adobe.com/premiere/desktop/add-video-effects/types-of-effects/effects.html)
  · [SMPTE ProRes coding labels](https://pub.smpte.org/pub/rdd44/rdd44-2022.pdf)

The native source-copy comparison used the retained range payload on
a disposable master, then two new Avid edits with **Copy Source Markers**
enabled. Avid copied the point control as edit-owned and kept the span
on the master in both cases. Use direct native source authoring as a
separate check if there is evidence of a different saved range structure.
[Marker test](SPLIT-EDIT-AUDIO-2026-11-06.md#native-source-range-and-point-copy)

Native named-track AAF export is optional comparison evidence for the
name mapping already verified in Premiere 27.

The offline four-bin regression combines the exact-24 Submaster, the
matching and independent source spans, the independent Group audio
defaults, and the linked-original marker conflict. Both entry points
match the retained Premiere saves across nine sequences and 11 file
links, including sequence and nest clocks, edit trims, channels, and marker
ranges. Two seeded CLI outputs and graph snapshots are byte-identical.
Premiere reopen of this combined regression is pending. The individual
reopened results remain recorded under each test.

A fresh local Apple Silicon package also replayed seven bins, adding
the two layered Submaster rates and the duplicate-master control. Its
CLI and saved job matched the retained sequence structure, source
links, markers, and Group selections across 16 sequences and 14
linked files. The package separately matched the two reopened
picture dissolve controls and refused the overrun on both entry
points. After Premiere 27 Save As and reopen, all 16 sequence
structures and 14 original links matched the packaged output, with
zero offline items. The independent A1 Group sequence matched the
linked AAF on all 120 picture frames. This replay checked audio
assignments in the saved project.
[Independent A1 switch](EDITABLE-GROUPS-2026-11-08.md#native-independent-a1-switch)

<a id="recent-parity-checks"></a>

#### Recent native checks and AAF benchmarks

- [x] **Coincident ranges on linked original clips.** Premiere 27
      retained both when the source and edit use separate lanes. When
      they use the same lane, each preference kept the selected range
      on the edit and preserved the other on the sequence.
      [Marker verification](SPLIT-EDIT-AUDIO-2026-11-06.md#coincident-source-and-edit-markers)

- [x] **Compare a shifted Avid clip volume ramp with linked AAF.**
      Premiere 27 Save As and reopen preserved all four editable keys
      and the source trim in both projects. Dry and processed stereo
      PCM24 renders agree within one count per sample. Avid Direct Out
      differs by about 0.506 dB RMS over the processed edit after
      normalizing each render against its dry reference. (CONV-0177)
- [x] **Explicit external source LUT.** The linked AAF carries the
      source LUT reference and ColorAdapter operation. Premiere 27 reopened
      the AAF with Unknown Effect markers and neutral picture. Any NLE
      embeds the qualified affine project-local cube as an editable
      Premiere 27 Color Mode LUT on the bound edit in Separate files mode.
      The generated LUT matched the native Color Mode render on all 174
      frames after Save As and reopen. The neutral use of the same picture
      source matches the linked AAF on every frame. The
      [Color Mode comparison](PREMIERE-27-COLOR-MODE.md#clip-lut) records
      the measured difference from the earlier Lumetri output.
      [Source LUT comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#external-source-lut-and-linked-aaf)
- [x] **One native Avid Group with an independent A1 switch.** Premiere 27
      reopened the linked AAF and Any NLE projects with identical picture,
      exact ranges, and the same mono source selections. Any NLE retained
      switchable picture and matched Avid Direct Out across both lanes.
      The switched A1 edit is a direct mono clip.
      [Scope and results](EDITABLE-GROUPS-2026-11-08.md#native-independent-a1-switch)
- [x] **Native coincident source and edit point markers.** Both ownership
      preferences passed Premiere 27 Save As and reopen.
      [Native point-marker comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#native-coincident-point-markers-and-linked-aaf)
- [x] **Grouped picture and audio marker ranges.** Premiere 27 Save As
      and reopen kept both ranges. Premiere shares the audio marker
      with the picture through the Grouped collection. Edit ranges and
      selectors were unchanged, and the marked and unmarked renders
      matched in picture and audio. This confirms range retention with
      a limitation on lane ownership.
      [Marker comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#coincident-source-and-edit-markers)
- [x] **Retained 24 fps sequence span.** The linked AAF exported with
      media offline and omitted the 22-frame sequence range. Premiere 27
      reopened the exact span from an isolated Any NLE project.
      [Sequence span comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#retained-native-sequence-span-and-linked-aaf)
- [x] **Online 24 fps sequence span.** Avid exported the same saved
      22-frame marker in an online control. Premiere 27 reopened the
      imported AAF with no marker. The Any NLE version retained the
      range, matched all 87 picture frames, and stayed within one PCM16
      count per sample in the stereo mix. (CONV-0172)
- [x] **Derived source span and edit copy at 24 fps.** For equivalent
      offsets, durations, source track, and creation timestamp, the
      reopened Any NLE project shows one range. Changing the edit
      timestamp by one second exercises the conflict preference and
      keeps two ranges when the sequence fallback is enabled. Both
      imported AAFs collapse to a point at frame zero. (CONV-0173)


##### Qualification workflow

- Start with the pinned references and retained native saves where available.
  Record the field, owning object, and observable result you expect.
  Implement documented mappings and exercise the
  affected paths offline before batching editor checks.
- Before opening an editor, write down the one result that could change
  implementation or the support claim. Run one native case per materially
  different path. Reuse existing saves and renders for equivalent variants.
  A change in codec handling, clock, channel assignment, or clip construction
  earns its own test when it could change the result. Reuse linked AAF
  evidence for equivalent cases and reserve new exports for a specific
  [readiness question](#linked-media-conversion-readiness).
- Before starting a runtime batch, run
  `python3 scripts/translation_acceptance.py preflight --converter-root . --manifest MANIFEST.json`
  with the retained local manifest. During development, run the tests for
  the touched paths and replay relevant success and refusal fixtures.
  Settle the batch before running the full pinned suite and keep each
  completed change in a scoped local commit with its verification record.

#### Links, clocks, and trims

- [x] **Keep managed and UME links together in one Premiere project.**
      A two-bin offline control retained both 23.976 DNxHR LB timelines
      and a separate 24 fps H.264/AAC UME timeline. Direct CLI and saved
      jobs agree on linked paths, trim bounds and UME channel selection
      in Separate mode. Premiere 27 Save As and reopen kept all media
      online. All three timelines played to their ends, with matching
      picture and exact DNxHR PCM against separate controls. The UME
      comparison confirms channel placement and sample counts, with
      export-to-export PCM variation. Grouped and Merged writing was
      checked offline. An unresolved original path blocks publication.
      [Offline and native checks](SPLIT-EDIT-AUDIO-2026-11-06.md#combine-managed-and-original-links)
- [x] **23.976 DNxHD LB with two managed PCM16 mono siblings.** Avid’s
      DNx LB preset produced DNxHD. The Premiere 27 linked AAF and Any NLE
      Separate projects reopened online with the same 48 decoded picture
      frames. Both AAF lanes selected the correct trimmed samples but were
      center panned. With Avid pan matching, Any NLE preserved all 96,096
      samples on each discrete output channel. Fresh insertion preserved
      V1/A1/A2. [Native comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#23976-direct-dnxhd-with-pcm16)
- [x] **Grouped and Merged for 23.976 DNxHD LB and PCM16.** Both modes write
      the qualified DNxHD LB media and reopen in Premiere 27 with the same
      starts, cuts, and two mono assignments. The reusable bin item now
      displays the verified source start; fresh insertion retained V1,
      A1 and A2 over all 96 frames. All 48 native preview frames match
      between Grouped and Merged and select the same frames as Separate
      and the linked AAF. The native Grouped source preview decodes both
      mono siblings exactly. The saved trims and channel selectors in both
      modes reconstruct all 96,096 samples per lane of the Separate
      reference. After Save As and reopen in Premiere 27, isolated exports
      matched all 96,096 samples on each intended channel with the opposite
      channel silent.
      DNxHR LB with PCM16, offline output, and editable Avid Groups
      remain separate profiles.
      [Grouped and Merged read-back](SPLIT-EDIT-AUDIO-2026-11-06.md#23976-direct-dnxhd-with-pcm16)
- [x] **Refuse duplicate candidates for a selected MXF package.** Direct CLI
      now reports the same ambiguous identity as saved-job Review. With
      two files claiming the selected A1 package and no verified A2,
      neither path publishes a project. An unrelated duplicate remains a
      diagnostic in direct CLI conversion.
- [x] **29.97 DF DNxHD LB with two managed PCM24 mono siblings.** The native
      Avid transcode produced DNxHD. Separate mode preserves the DF
      sequence and logging clocks, trims, and all three original MXFs. After
      Premiere 27 Save As and reopen, both outputs selected the same 15
      source frames; with Avid default pan enabled, Any NLE also matched
      Avid Direct Out sample for sample on both lanes. Fresh insertion
      kept all three originals on separate tracks.
      The Premiere Log Note now also preserves the Avid Start label
      `02:00:00;00` from the saved drop-frame master TC1.
      [Native comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#2997-drop-frame-direct-dnxhd)
- [x] **Grouped and Merged for 29.97 DF DNxHD LB and PCM24.** Both
      modes now write through direct CLI and saved jobs. Premiere 27 Save As
      and reopen retain the exact start, minute-boundary jump, file-relative
      trims, and both mono routes. The reusable bin item now displays the
      verified source start. Fresh insertion saves picture and two separate
      mono tracks in both modes. All 15 exported PNG frames agree between
      Grouped and Merged and select the same numbered source frames as
      Separate and the linked AAF. Premiere's Grouped source preview matches
      both mono originals across all 240,240 samples per lane. Rebuilding the
      timeline from the saved Grouped and Merged trims and channel selectors
      matches Avid Direct Out for 24,024 samples per lane. After Save As and
      reopen in Premiere 27, isolated exports matched Avid Direct Out on each
      intended channel, with the opposite channel silent.
      [Drop-frame read-back](SPLIT-EDIT-AUDIO-2026-11-06.md#2997-drop-frame-direct-dnxhd)
- [ ] **29.97 DNxHR and other unqualified managed profiles.** DNxHR at
      this rate, PCM16 and non-drop need their own native specimens. The
      earlier synthetic DNxHR picture produced a codec error in Premiere 27.
- [x] **One mixed managed and UME Motion Adapter timeline.** The 23.976
      DNxHR/PCM24 and 24 fps H.264/AAC fixture now links all four originals
      in Premiere 27. Save As and reopen retain the source ranges and
      channel placement. The adapted section matches 1,102 Avid numbered
      frames and 2,206,204 PCM samples per lane. Separate CLI and saved
      jobs write it. Grouped and Merged saved jobs also reopen with the
      four original paths and all twelve placements.
      The linked AAF import matches the picture cadence and leaves out
      both UME audio placements. Any NLE preserves them. Fresh UME
      insertion retains both original channels at their full extent.
      [Mixed timeline read-back](MOTION-ADAPTER-24-TO-23976.md#premiere-27-direct-link-read-back)
- [x] **Link an existing Avid-managed OP1a Direct Out.** Separate files mode
      links the verified 1080p24 DNxHD LB picture and two 48 kHz PCM24 mono
      streams in place. The direct CLI and saved job retain the physical
      selectors, and both refuse a missing selector. The CLI project
      reopened with all 100 intended picture frames and both 200,000-sample
      mono lanes. This is the [verified Direct Out profile](#cataloged-audio-configuration-gaps-user-2026-11-07),
      with no media creation. Other managed OP1a layouts remain open.
- [x] **Refuse colliding root mappings.** Both Premiere entry points keep
      two same-named linked WAV originals distinct by their full saved paths.
      Mapping both saved paths to one current file stops conversion before
      publication, including when two Unix paths are hard links to the same
      file. Separate bin items can still reuse the same saved original, and
      distinct files may contain equal bytes. The derived control has matching
      audio descriptors and distinct signals (CONV-0134, offline).
- [x] **Relink same-named picture originals with distinct reels and clocks.**
      The Avid camera-original path and repeat Premiere move are qualified
      for the retained picture-only ProRes MOV pair. After Premiere 27
      Save As and reopen, the four-bin wizard output kept both moved
      paths, reels on the media and clip Tape Name fields, source clocks,
      and file-relative edits. Default and checksum verification accepted
      the intended moves and the swapped candidates were refused.
      [Verified moves](VERIFIED-RELINK.md#repeat-a-move-after-premiere-save-as)
- [x] **Compare a linked AAF with two same-named A/V originals.**
      The reopened native 24 fps H.264/AAC control preserved the selected
      picture paths and clocks in both projects. The linked AAF lost both
      audio lanes on import. Any NLE preserved all six audio placements
      and matched the Avid Direct Out on both channels.
      [Native A/V comparison](VERIFIED-RELINK.md#native-same-name-av-control)
- [ ] **Identity extensions and regression replay.** Replay the linked
      PCM16/PCM24 WAV and managed Separate, Grouped, and Merged controls
      after changes that affect their links. The moved picture-only
      ProRes pair has native checks and remains available for a targeted
      AAF relink benchmark.

#### Edit fidelity and metadata

A read-only audit of 35 retained Avid linked AAFs found nine operation
families, all associated with cases below. The saved Motion Control
operations are all mixed-rate Motion Adapters. The source color samples
cover a levels transform and wrappers without transform XML. The 45
descriptive markers are top-level point events. Later exports below cover
an explicit external LUT, ranged markers, and same-rate Motion.
[Adobe's migration guidance](https://helpx.adobe.com/il_en/premiere/desktop/render-and-export/export-files/export-from-avid-media-composer-to-premiere.html)

- [x] **Compare two-key Linear Resize and promoted 3D Warp with linked AAF.**
      Both AAFs carry independent scale keys, but Premiere 27 reopens them
      as editable Position and uniform Scale, with source width applied to
      both axes. The promoted Warp's final Motion key is on the exclusive
      out-point in the AAF import. Any NLE retains both scale axes and
      places the final key on the last visible frame of each 23.976 edit.
      Across all 48 frames per effect after Save As and reopen, the visible
      chart edges stay within one pixel of the Avid exports with Any NLE;
      the AAF imports deviate by up to 324 pixels. The AAF links the original
      chart; Any NLE uses the retained synthetic DNx/OP1a counterpart, so
      this qualifies geometry, not exact decoded-pixel equality or direct
      media linking. Wider static Motion variants remain open.

- [x] **Compare the retained asymmetric audio dissolve.** On the qualified
      direct Separate route, verified Avid Linear dissolves write editable
      Constant Gain at the saved alignment. The reopened isolated output
      matches Avid Direct Out exactly outside the fade and within five PCM24
      codes on A1 and one on A2 inside. Retain the Constant Power approximation
      outside this profile. [AAF and Avid comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#linear-audio-dissolve-against-linked-aaf)
- [x] **Compare centered and asymmetric picture dissolves with linked AAF.**
      On 23.976 DNxHR LB, both Any NLE outputs retained the editable eight-frame
      dissolve, exact cut offset, and visible source trims after Premiere 27
      reopen. Premiere imported both linked AAFs as the newer Cross Dissolve
      and saved the asymmetric cut offset as four frames instead of two.
      Both AAF renders select earlier picture after the blend window.
      Any NLE retains the intended frames there and its blend renders
      are measurably closer to the Avid exports.
      [Dissolve qualification](SPLIT-EDIT-AUDIO-2026-11-06.md#ordinary-picture-dissolves)
- [ ] **Picture and editable audio controls.** The 24 fps linked mono WAV
      control played in Avid. Its +3 dB source-only edit rendered flat;
      combining that source with a −4 dB edit yielded the same 96,000
      Direct Out samples as the −4 dB edit alone. Avid's linked AAF
      carries both controls on their respective owners. Premiere 27
      imported the ten AAFs individually and, after Save As, close and
      reopen, rendered the same gain behavior as Avid. The reopened
      Any NLE project matches the four static gain controls and static
      pan within one PCM24 count per channel of the AAF renders. A
      separately authored Avid control used the same master for two cuts:
      the one made before a +3 dB bin gain adjustment stayed dry, and
      the one made afterward saved and rendered +3 dB. For verified
      online 48 kHz mono PCM24 WAV at 1080p24 in Separate files mode,
      Any NLE keeps bin gain separate and uses the saved gain of each
      qualified cut. Both CLI and saved-job renders preserve the 3 dB
      difference after Premiere 27 reopen. Other source profiles retain
      their existing behavior.
      Premiere's AAF import kept static pan and four editable clip pan
      keys; the AAF had already rounded the subframe pan key to a whole
      frame. It also kept four editable volume keys, but rounded the
      subframe volume key even though the AAF carried the fractional
      time. The reopened Any NLE project keeps the subframe volume time
      and matches the inline AAF volume render within one PCM24 count.
      In Separate files mode, Any NLE writes editable Linear pan keys
      for verified 48 kHz mono PCM24 WAV edits on the 1080p24 profile.
      Direct CLI and saved-job renders match on both channels after
      Premiere 27 Save As and reopen. The whole-frame render matches
      the AAF import within one PCM24 count, and the fractional key
      survives rounded to the nearest Premiere tick. Premiere and Avid
      produce different PCM envelopes. Separate files, Grouped and Merged
      preserve Linear pan on the verified 1080p24 managed DNxHD LB and mono
      PCM16 profile.
      [Managed mono pan](SPLIT-EDIT-AUDIO-2026-11-06.md#editable-managed-mono-clip-pan)
      Premiere 27 reopened all nine retained three-key Linear Resize
      controls on the verified DNx OP1a route. Both Fixed and Elastic
      trim rounds retained the exact source ranges and key times, including
      keys outside the edit. The 288 rendered frames match Avid's measured
      edges within one pixel (CONV-0049). The 24 to 23.976 automatic
      Motion Adapter also has a [Premiere 27 direct-link read-back](MOTION-ADAPTER-24-TO-23976.md#premiere-27-direct-link-read-back).
      The [reciprocal adapter](MOTION-ADAPTER-24-TO-23976.md#premiere-27-replay-of-23976-to-24)
      retained all 1,104 source-frame selections and matched the earlier
      Premiere PCM on both channels.
      The 24 fps managed DNxHD LB/PCM16 control now keeps editable 50%,
      200%, and full-speed reverse on 50-frame picture edits in Separate
      files mode. The saved Speed and Position controls establish the
      exact rate. Premiere gets raw picture trims and an end-relative
      trim for reverse. After Save As and reopen in Premiere 27, both
      entry points matched all 150 Avid frame selections and all 300,000
      samples per mono lane reconstructed from the saved Avid audio
      edits and original MXFs. A graph-authored three-marker control now
      keeps all three editable retimes and preserves each known edit marker
      range on the Premiere sequence with an ownership loss report. Save As
      and reopen retained the three ranges, and all 150 native preview frames
      selected the original source frames. Native Avid marker authoring and
      clip-owned retime marker timing remain open. (CONV-0023)
- [x] **Replay the compact static Motion profile in Premiere 27.**
      The retained four 1080p24 3D Warp edits cover source crop alone,
      Z Rotation, independent Scale, and their combination. Both entry
      points preserve the saved Motion values and precise edit ranges.
      Independent remeasurement of the retained Premiere 27 reopen
      renders matched all 28 Avid chart points within 0.10 px. Use native
      chart renders when qualifying wider static variants. (CONV-0106)
- [x] **Account for the five retained split-edit markers.** The AAF moves the five
      split-edit markers at record frames 6, 20, 24, 28 and 50 to the
      sequence. Any NLE retains one sequence marker and four edit-owned
      markers on their edit copies. Projecting the reopened clip marker
      offsets through their source trims gives the same five positions
      on both timelines. Any NLE writes an explicit palette entry for each.
- [x] **Compare marker names and colors on both linked dissolve controls.**
      Premiere 27 kept all four linked AAF point positions and comments,
      renamed the markers `Marker 1` and `Marker 2` on each sequence,
      and discarded the explicit red palette tags. The reopened Any NLE
      projects kept the original names and red entries on the V1 edits
      at the same record positions.
- [x] **Preserve named timeline tracks for populated lanes and verified
      empty layouts.** Premiere 27 reopened the Separate files control
      with its original named V1, V2, A1, and A2, plus empty V5 and
      standard mono A6. Both entry points write the names in all three
      linked modes. An empty stereo track remains a scoped loss.
      [Timeline track names](SPLIT-EDIT-AUDIO-2026-11-06.md#timeline-track-names)
- [x] **Compare populated source logging against linked AAF.** The AAF
      import retains Scene, Take, Camera Roll and a custom Log Note on
      the tested source items. The direct CLI and saved job preserve the
      same values. Any NLE puts the saved Avid Comments in Premiere's
      Comment column on both media types. Camera Roll and Tape Name
      remain distinct after Save As and reopen.
      [Logging comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#source-logging-and-linked-aaf)
- [x] **Reopen an explicit Sound Roll.** Two retained linked AAF imports
      put the value of `Soundroll TC Rate` into Sound Roll. Any NLE writes
      the exact `Soundroll` column instead and retains the rate as labelled
      text. Premiere 27 Save As and reopen kept the roll on its own master
      and the labelled rate on both masters in the derived linked WAV fixture.
      [Saved-field evidence](SPLIT-EDIT-AUDIO-2026-11-06.md#source-logging-and-linked-aaf)
- [x] **Preserve overlapping filler marker ranges.** The carrier now spans
      all overlapping markers. If a carrier would cover an existing picture
      edit on its own or a lower track, the sequence retains the marker's
      full range and the report names the fallback. Premiere 27 read-back
      and renders cover both same-track and upper-track examples.
      [Marker range read-back](SPLIT-EDIT-AUDIO-2026-11-06.md#ranged-filler-marker-read-back)
- [ ] **Remaining source markers, source LUTs, and matte handling.**
      The native 23.976 point-marker comparison verifies both source/edit
      preferences. Linked AAF holds all three events. Premiere 27 reopens
      two sequence markers with the edit comments and drops the source
      event. Any NLE retains all three names and colors, the source bin
      marker and the selected edit/sequence ownership. The retained native
      24 fps sequence span survived Premiere 27 reopen in a marker-only
      project. Its linked AAF, exported with media offline, omitted the
      span. In the paired 24 fps source-copy control, the reopened AAF
      carried the copied point onto the sequence. Any NLE kept both source
      ranges and both point occurrences on the edits. Direct native
      authoring of coincident source/edit ranges remains open.
      [Native point-marker comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#native-coincident-point-markers-and-linked-aaf)
      [Sequence span comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#retained-native-sequence-span-and-linked-aaf)
      [Source copy comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#native-source-range-and-point-copy)
      A derived AVB now preserves both coincident ranges after Premiere 27
      reopen when **Keep the other marker as a sequence marker** is enabled.
      [Marker ownership check](SPLIT-EDIT-AUDIO-2026-11-06.md#coincident-source-and-edit-markers)
      Grouped now retains the winning V1 range for the verified direct
      DNxHD profile. The 29.97 DF edit-preferred case survived Premiere
      27 Save As and reopen. Both preferences passed direct CLI and
      saved-job read-back. Merged keeps the winning range on a dedicated
      V1 component for the tested same-rate direct DNxHD edit. The 29.97
      DF edit-preferred project also survived Save As and reopen with its
      other range on the sequence and both original mono siblings linked.
      The reusable Merged item displays its source range at
      `02:00:00;12` with a two-frame duration. In the verified 29.97 DF
      profile, a mono edit with a unique sibling binding now keeps the
      preferred marker on its dedicated Merged audio component. Premiere
      27 Save As and reopen kept the winning A1 range and the other range
      on the sequence. Both preferences passed writing checks. Direct
      Media Composer authoring of coincident audio ranges remains open.
      Merged also preserves lane-specific clip markers for the verified
      23.976 DNxHD LB/PCM16 profile with a nonzero source origin. The
      preferred A1 range stays on its own mono component and the other
      retains its full range on the sequence. The independent A2 and V1
      ranges stay on their own components. Both preferences passed the
      direct CLI, saved job, and Premiere 27 Save As and reopen. All 48
      picture frames match the retained linked AAF, and all 96,096 stereo
      PCM16 sample frames match the unmarked Merged control.
      On the reusable Merged item, Premiere 27 shows both two-frame
      source ranges at `02:00:00:07` and lists each twice. Premiere
      recreates the extra entries on save even with only the component
      marks in the input project. The fresh edit keeps one range per
      mono lane. Its 24 picture frames match the linked AAF, and each
      isolated 48,048-sample lane matches its original under the
      center-pan mix policy to within one PCM16 count. (CONV-0132)
      Any NLE retains the saved source color transformation text and source
      owner in graph/2. Premiere preparation flags the affected clip for
      manual review. For an Avid-authored full-range to video-level transform,
      linked AAF preserved the source record and exported a ColorAdapter
      operation on the sequence. Premiere 27 reopened with an Unknown Effect
      marker at the edit, and all 72 decoded frames matched a direct import.
      Any NLE reports the affected sequence track and record position.
      Applying the transform in Premiere remains open. (CONV-0144)
      The external .cube control preserves its filename in the source and
      sequence AAF records. On reopen, Premiere's AAF import renders the
      neutral look and reports the unknown ColorAdapter. Both Any NLE
      entry points now name the saved cube in the clip's review message and
      retain the affected edit report. Avid's neutral and LUT-applied
      exports differ on all 87 frames. A generated Premiere source Input
      LUT asset survived Save As and reopen with no matching installed
      filename. All 87 decoded frames match Premiere's manually assigned
      Input LUT. The prototype measures 48.85 dB RGB PSNR against Avid's
      LUT-applied render, compared with 16.78 dB for the linked AAF import.
      Both saved Avid bins can carry the same picture source LUT
      metadata while their sequence renders differ. Only the LUT-applied
      sequence has the matching timeline ColorAdapter. Graph/2 now
      binds the saved 24 fps ColorAdapter payload to the exact picture
      placement when its source package, saved transform and edit
      ranges agree. A contradictory payload stays available for review
      with the placement unresolved.
      Premiere 27 coalesced duplicate same-file source references on
      Save As and dropped their distinct LUT state. The initial writer
      embedded a clip-level Lumetri Look when the saved adapter was neutral
      apart from the LUT, the Avid project descriptor matches, and the
      direct managed picture verifies as the tested 1080p24 Rec.709 DNxHD
      profile. The current mapping checks the entire affine LUT grid at
      edge sizes 2–65 in an explicit unit RGB domain and embeds its
      eight corner samples on a plain V1 edit in Separate files.
      A derived 33-point grid rendered all 174 frames identically to
      the original size-two control after Premiere 27 reopen. The
      65-point grid produced the same seeded Premiere project.
      The adjacent neutral and processed uses share the original file and
      source trim after Save As and reopen. All 87 neutral frames match
      the linked AAF. The processed render measures 48.34 dB RGB PSNR
      against Avid, compared with 16.78 dB for that AAF import.
      The qualified affine route now writes a
      [Premiere 27 clip LUT](PREMIERE-27-COLOR-MODE.md#clip-lut).
      Other color transforms, cube profiles and effect stacks remain
      occurrence-specific omissions.
      [Premiere Input LUT controls](https://helpx.adobe.com/premiere/desktop/correct-color/set-up-color-management/color-management-using-clip-modify.html)
      (CONV-0175)
      The retained Avid alpha bin produces four Matte Key losses on V2 at
      relative frames 0, 1, 4 and 5. Graph/2 retains each range, its saved
      fill and matte references, and the shared master's original image
      locator and alpha import flags. The linked AAF carries all
      four composites with separate inputs. Premiere 27 rejected that AAF.
      With known-good linked picture and the saved Avid Matte Key operation,
      Premiere accepted both flat and nested controls, saved the fill and
      matte on separate tracks inside a nest, and added an unsupported-effect
      marker. After Save As and reopen, all 24 frames in each control
      matched the raw matte input. Linking the original straight-alpha PNG in
      a Premiere control reproduced both Avid polarities within 0.94 mean
      absolute 8-bit RGB codes after reopen with **Composite in Linear
      Color** disabled. A child sequence can keep that blend local when it
      contains both the keyed foreground and the matching background. A
      foreground-only child uses the parent's blend at its boundary.
      The retained background edits use an unsupported picture compression.
      Graph/2 also retains four imported PNG masters independently of
      managed media admission, with exact saved locator, master identity,
      track, source range and import settings. The eight background edits
      resolve to the two master identities. Graph/2 now also keeps the four
      markers on the matte input, with qualified record times and distinct
      owners. A local graph/2 prototype generated all four editable nests
      and linked the originals. After Premiere 27 reopened the project, all
      eight frames were within one mean absolute 8-bit RGB code of the
      Avid TIFFs, and all eight comments and marker positions survived.
      The reader now identifies the tested saved-control profile on
      each of the four Matte Key occurrences and distinguishes twelve
      empty Color Adapters from changed payloads. A separate bounded
      verifier resolves the original PNG by exact path, decodes it, and
      pins the verified bytes. Separate files conversion now uses this
      path for the verified 1080p24 still-only sequence with one-frame
      Matte Keys and exact V1 background coverage. Direct CLI and saved
      jobs build four editable children and keep both alpha polarities
      and all eight marker times, comments and colors after Premiere 27
      Save As and reopen. The markers use sequence ownership. The
      standalone Avid Matte Key bin compositions remain unsupported.
      (CONV-0143)

#### Editable structure and final acceptance

- [ ] **Subclips, nesting, and Group selection.** Compare two same-named
      subclips with different ranges, a trimmed nest with a nonzero child
      start, and a two-camera A/B/A Group with independently changed audio
      selection. Validate source, range, channel, render and editability
      against the saved Avid edit. Record reusable or switchable output
      when the retained AAF flattens the structure. The fixed-audio Group
      AAF reopened with three flat A/B/A picture edits and three camera-A edits on each
      mono lane. Any NLE now keeps the same cuts in an editable Group
      with all six original MXFs linked. All 145 decoded frames matched
      the AAF import. A separate non-silent control verified both mono
      lanes. The converted selector variant also passed a fresh Premiere 27
      Save As and reopen in the four-bin project. Picture remained
      editable A/B/A and matched all 145 reference frame hashes. Only the
      middle A1 edit linked directly to camera B, with its file trim
      intact. A2 stayed on camera A. Both 290,290-sample lane exports
      matched the silent originals. Two graph-authored controls with
      distinct signals in complementary cameras matched their flat
      references over all 48,048 samples per lane after reopen.
      [Independent audio check](EDITABLE-GROUPS-2026-11-08.md#independent-mono-audio-edits)
      A Group edit with its own marker now converts without losing the
      editable picture or selected audio. Premiere preparation moves the
      marker to the sequence with its range, color and comment and reports
      the ownership change. A graph-authored control verified the fallback
      on V1 and an independently selected A1. In a separate marked variant
      of the retained fixed-audio Group, Premiere 27 Save As and reopen
      preserved both three-frame ranges, comments, colors and all edit
      references. The inherited source marker control also reopened with
      its picture marker on the sequence and its alternate A1 marker on
      the linked mono clip, with no duplicate. The native three-mono
      Group's middle V1 marker survived both Premiere 27 reopens at record
      frame 30. Any NLE retained the name, comment and red color on the
      sequence and reported the ownership change. The linked AAF import
      retained the comment with a generic name and default green. All 72
      converted picture frames and all three isolated mono renders matched
      the unmarked conversion. [Native marked Group](EDITABLE-GROUPS-2026-11-08.md#native-marker-on-a-three-mono-group-edit)
      Graph/2 now records the saved subclip identities, range
      attributes and direct backing references. Picture-only subclips
      over qualified 23.976 direct media now create reusable Premiere
      bin items. Two Avid-authored, same-named V1 subclips reopened with
      distinct identities and exact bounds; all 13 fresh-insert picture
      frames match the linked AAF subclip items. A 23.976 managed A/V
      subclip edit reopened in Premiere 27 at the correct trim on V1,
      A1, and A2. The linked AAF from that exact sequence matched all 24
      frames and both mono sample ranges, with the documented pan-law
      difference. Premiere 27 kept the distinct A/V subclip after Save As
      and reopen, and fresh insertion preserved all three lanes. Any NLE
      now preserves the same reusable A/V item and its timeline references
      for the verified 23.976 DNxHD LB/PCM16 case. Both entry
      points matched the AAF picture and source audio after reopen.
      A shortened subclip also reopened and freshly inserted with its
      exact 24-frame range and both mono lanes.
      The DNxHR LB/PCM24 control also retains one reusable A/V item.
      Its linked AAF imports the edited picture and audio correctly,
      with separate recovered media items. Fresh insertion from the AAF
      picture item brings in V1 alone, while the converted item inserts all
      three lanes. [Subclip evidence](SOURCE-GRAPH-MIGRATION.md#saved-avid-subclip-ranges)
      When every lane in a marked edit resolves to the same reusable A/V
      subclip, conversion keeps that reference and reports the move of
      its edit marker to the sequence. Premiere 27 Save As and reopen
      retained the V1 and A2 ranges, comments, colors, original trims,
      and shared subclip in a control with markers added to graph/2.
      One V1 source marker fully inside the reusable subclip now stays on
      the backing picture and appears once on the subclip at its original
      file position. Premiere 27 Save As and reopen kept both marker records
      and the shared V1/A1/A2 reference in a derived AVB control. A range
      crossing the subclip boundary keeps the linked backing edits and
      reports the unsupported reusable item. A source marker and an
      independent edit marker at different source frames can coexist
      while all three lanes keep the reusable item. Premiere 27 reopen
      preserved the backing and subclip marker plus the independent
      review range on the sequence. A shared source frame still uses
      the configured source/edit conflict policy.
      A backing marker wholly outside the subclip stays on the backing
      master and all three lanes keep the reusable item. Derived controls
      at both boundaries pass CLI and saved-job project checks. Premiere
      27 conversion and reopen keep the shared subclip, media clocks,
      trims, channel selectors, and marker ownership in both CLI cases.
      A head-trimmed, picture-only Submaster now keeps an editable child
      sequence with its own nonzero display start, plus independent parent
      audio lanes. The reopened linked AAF flattens the Submaster and
      matches the converted picture and audio ranges.
      A V1 edit marker with a resolved owner now stays at its exact
      time on the parent sequence while the editable Submaster survives.
      Premiere 27 Save As and reopen kept the four-frame range, comment,
      color, child start, and both parent mono edits in a control with a
      marker added to graph/2. The report identifies the ownership change.
      A derived AVB with matching source and edit markers also reopened
      with the editable child and both ranges on their respective clocks.
      Direct CLI and saved jobs agree on the parent review range, backing
      source marker, trims and mono assignments.
      The derived upper-track control keeps a plain Submaster editable on
      V5, with its review range on the parent and the V1 and mono edits
      intact. Its reopened picture matches the flat control across all
      29 frames, including the last frame when V1 shows through. The
      exact 24 fps DNxHD LB control also retains its child origin and
      distinct coincident mono trims. After Premiere 27 reopen, all 196
      frames and the full mix and isolated audio match its flat control.
      [Trimmed nest evidence](SOURCE-GRAPH-MIGRATION.md#trimmed-avid-submaster-nests)
      The verified Separate route links an independently switched mono edit
      directly to the selected camera's MXF. The Group still supplies its
      default audio and editable picture. The same direct route now uses
      each saved lane default independently. The corrected split-default
      control passes Premiere 27 reopen with both mono outputs. A fresh
      insertion uses the selected camera on each lane, and the existing
      edits match the retained PCM.
      [Default lane selections](EDITABLE-GROUPS-2026-11-08.md#audio-layouts-and-groups)
      The native 23.976 DNxHD LB/PCM24 case switched only the middle A1
      edit. The reopened linked AAF and Any NLE project agree on all eight
      edit ranges and all 120 picture frames. Any NLE kept picture angles
      editable and reported the direct mono A1 link. Both lanes match all
      PCM24 samples in Avid's Direct Out. The AAF import center-pans
      both lanes. Wider Group audio and reusable sequence-to-sequence
      references remain open. A derived two-layer Submaster project
      has Premiere 27 reopen and picture comparison with its unmarked
      companion. The Avid-authored layered comparison remains open.
      [Native independent A1 comparison](EDITABLE-GROUPS-2026-11-08.md#native-independent-a1-switch)
      A native fixed-audio DNxHD control now matches the linked AAF
      on all 120 picture frames and exact edit ranges. Any NLE keeps
      editable picture angles and matches both original PCM24 lanes
      with Avid pan matching. The AAF import center-pans those lanes.
      [Linked Group checks](EDITABLE-GROUPS-2026-11-08.md#link-the-original-op-atom-camera-files)
      [Subclip source facts](SOURCE-GRAPH-MIGRATION.md#saved-avid-subclip-ranges)
- [x] **Reopen and render the four-bin macOS wizard output.** A local
      Apple Silicon package with an audible derived Group kept all ten
      original links, 68 placements, 14 sequences, and four transitions
      after Premiere 27 Save As and reopen. Direct CLI and saved jobs
      agree on the saved edits, trims, clocks, markers, and audio selectors.
      All 1,495 decoded frames across seven timelines match the retained
      renders. Both isolated Group, split-fade, and mixed managed/UME
      lanes match the earlier PCM. The trimmed nests match within one
      PCM24 count after accounting for the older controls' center pan.
      The package used the derived A1 switch. The native A1 switch was
      verified separately through direct CLI and saved jobs.
      Separate SubCap and Titler+ controls retained visible text after
      Premiere 27 reopen. A fresh DNx OP1a rewrap also reopened and
      matched the 32-frame direct-link picture control. The pinned
      acceptance suite passed all 1,669 required fixture hashes.
- [x] **Combined and packaged acceptance for the tested linked profile.**
      The final four-bin Apple Silicon replay includes the native source
      copy, mixed managed and UME Motion Adapter, marked picture dissolves
      and native three-mono Group. Review found all 15 originals. After
      Save As and reopen in Premiere 27, eight sequences, 60 placements
      and four transitions match the wizard, CLI and saved job. All 1,419
      compared picture frames match their references. Three isolated
      Group lanes each match 144,144 AAF sample frames within one PCM24
      count under center pan. Two seeded CLI projects and graphs match
      byte for byte. Both packaged entry points refuse an occupied
      output path.
      A previous Apple Silicon package passed Review and conversion on
      the mixed managed/UME Motion Adapter, marked dissolves, and trimmed
      Submaster bins together in Separate files mode. Premiere 27 Save As
      and reopen retained the same saved structure as the direct CLI and
      saved job, including four original media paths, 54 timeline
      placements, four transitions, 19 marker uses, and three editable
      Submaster references. The mixed timeline, asymmetric dissolve, and
      head-trimmed Submaster each played to the end. A four-bin replay
      added the derived A/B/A Group selector control to both CLI and saved
      jobs. Offline project read-back agreed on all 75 placements, ten
      original paths, four transitions, and the alternate A1 edit with
      A2 still supplied by the Group. The four-bin CLI project now passed
      native reopen for the Group, with matching picture and isolated
      audio renders. The audible four-bin package replay above also passed.
      The four-bin AAC package replay retains six original links and all
      63 placements. Its AAC and mixed managed/UME sequences match the
      full picture and audio references after Premiere 27 reopen.
      [Package replay](VERIFIED-RELINK.md#combined-macos-package-replay)
      A five-bin replay adds the marked A/V subclip to that selection.
      The wizard found all six original paths. Its output reopened
      with 14 sequences, 66 placements and four transitions. The three
      entry points preserve the marked subclip when its bin provides
      a unique local backing master, including when other bins carry
      the same master identity. Read-back of the other 13 sequences
      matches the prior native project. (CONV-0189)
      The earlier local Apple Silicon build reproduces the prior reopened
      project through its packaged CLI and saved-job path. A second run
      replaces the derived Group with the native A1 switch and matches the
      retained reopened edit ranges, angle choices, and linked lanes. The
      latest local Apple Silicon package also converts Sound Roll, moved
      picture reels, the upper-track Submaster, and independent Group audio
      defaults in one project. Its bundled CLI and saved job match the
      individual controls across four bins, 21 saved sequences, and 12
      existing originals. The wizard and Premiere 27 Save As and reopen
      agree with the direct CLI and saved job on those saved facts. The
      upper-track Submaster matches all 29 frames of the flat picture
      control. The corrected Group matches the AAF picture and retained
      mono renders and passes independent audio defaults on fresh insertion.
      [Group verification](EDITABLE-GROUPS-2026-11-08.md#audio-layouts-and-groups)
      Compare decoded
      frame hashes for unchanged picture and exact PCM under the same mix
      policy. For AAC check first sample and playable extent and measure
      repeat-export variation. Rerun the pinned acceptance and research
      gates and replay editable Groups, SubCap, Titler+, and rewrap.
- [x] **Locate translation warnings in Review.** When a warning identifies
      a saved edit or transition, Review shows its sequence, lane and
      edit start or transition cut. It uses the qualified record timecode
      when available and an exact relative frame otherwise.

- [x] **Keep plain disabled mono Avid edits editable.** Separate files
      preserves four 24 fps managed PCM16 occurrences after Premiere 27 Save
      As and reopen. The linked AAF carried the selectors, but Premiere's
      import dropped the editable clips. Native re-enable restored the
      selected 56,000-sample range without altering the rest of A1.
- [x] **Keep an editable disabled stereo combiner.** Separate files
      preserves the inactive 24 fps PCM16 stereo edit on A3 with both
      managed mono siblings in the saved channel order. Premiere 27
      reopened the edit disabled. Re-enabling and reopening it restored
      56,000 exact samples per channel, with no change elsewhere on A3.
- [x] **Keep the verified disabled picture clip editable.** Separate files
      preserves the 24 fps DNxHD picture edit on V1. Its four saved
      framing rectangles are equal and centered. Avid's native render
      confirms that this pattern uses the full 1920×1080 raster. Premiere
      27 reopened the edit disabled and restored all 28 pictures when
      re-enabled. Changed framing remains a reported gap.
      [Disabled clip comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#disabled-clip-occurrences)

- [x] **Keep a timeline stereo combiner over dual-mono masters.** In
      Separate files, the verified 24 fps PCM16 combiner plays on one
      linked stereo lane while independent mono edits retain their trims.
      Premiere 27 reopened the muted and re-enabled controls and rendered
      exact PCM on both channels.
      [Stereo edit evidence](SPLIT-EDIT-AUDIO-2026-11-06.md#stereo-combiner-on-a-dual-mono-master)
- [x] **Keep AutoSync mic isos in the edited timeline.** A 24 fps AutoSync
      edit with a two-frame sound offset reopened in Premiere 27 with eight
      independent mono lanes linked to the correct managed MXFs. All 24
      decoded picture frames matched the linked AAF import, and each isolated
      PCM24 lane matched within one low-order bit.
      [AutoSync comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#eight-lane-autosync-from-imported-wav)
- [x] **Keep the reusable AutoSync item.** Separate files rebuilds the saved
      1080p24 picture and linked PCM16 mono siblings as an editable source
      sequence in the originating bin. Fresh insertion as individual clips
      preserved all 48 picture frames, the two-frame sound offset and eight
      independent mono lanes after Premiere 27 Save As and reopen. The
      verified reusable source also retains its saved picture TC1 origin.
      [Reusable AutoSync check](SPLIT-EDIT-AUDIO-2026-11-06.md#reusable-autosync-source-sequences)
- [x] **Qualify one original eight-channel WAV in Premiere.** Native
      import and fresh insertion put the Avid trim onto eight independent
      mono lanes. A private AutoSync prototype preserved the WAV's BWF
      origin, per-lane selectors, and the original timeline and reusable
      source ranges after Premiere 27 Save As and reopen. Every isolated
      timeline lane matched linked AAF within one PCM24 step.
- [x] **Connect a verified eight-channel original WAV in Separate files.**
      At 24 fps, the linked-original path admits a 48 kHz PCM16 BWF
      when its clock, extent and every channel match the eight saved
      managed packages. Both entry points keep the managed MXFs when
      the WAV is absent or differs. Premiere 27 Save As and reopen
      preserved the V1 trim and all eight independent mono lanes,
      including fresh insertion from the reusable AutoSync item.
      [Original WAV qualification](SPLIT-EDIT-AUDIO-2026-11-06.md#original-eight-channel-wav-in-premiere)

### Avid → Resolve

The internal native DRP route has linked DNxHR siblings and selected H.264
originals, multiple timelines and nested bins, a fixed-audio Group, SubCap,
and scoped Resize/Warp and crop. Use Avid source renders and reopened
Resolve controls to qualify picture effects. Resolve's AAF conform
coverage helps identify common workflows for readiness benchmarking.
[Any NLE qualification](RESOLVE-PROJECT-CORE.md)
· [Resolve AAF conform features](https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf?_v=1783666810000#page=468)

- [ ] **Complete the combined linked turnover.** Generate one 23.976
      project from saved Avid input with managed DNxHR OP-Atom, UME picture,
      two timelines in nested bins, shared media, and an editable Group.
      After Save Project As and reopen, verify cuts, original paths, reel and
      source timecode, and compare picture and both isolated mono lanes.
      Switch only the Group picture and verify both audio selections stay
      fixed. Measure the known startup envelope separately. The explicit
      binding control and automatic single-profile tests provide the
      [native references](RESOLVE-TURNOVER-IMPLEMENTATION.md#qualification-boundary).
      Packaged exposure follows the [Resolve product gate](#resolve-product-exposure).
- [ ] **Qualify common Avid picture effects.** Compare editable
      constant and variable Timewarp, freeze, and both 24 ↔ 23.976
      Motion Adapter directions. Check frame cadence and sound timing
      separately. Extend 3D Warp to the qualified pitch/yaw and opacity
      cases once the destination geometry and blend order match the source.
      [Resolve qualification queue](#davinci-resolve-user-2026-10-26-bookmark-and-research)
- [ ] **Widen transitions and timeline structure.** Finish native
      cross-editor dissolve and mono fade checks, then qualify Dip to Color,
      Edge Wipe, and the other documented wipe and iris families with
      native controls. Check subclips, simple nests, and alternate Group
      audio against saved source IDs. Use linked AAF for a scoped readiness
      comparison when the retained evidence leaves an open question,
      including plug-in-specific transfer where relevant.
      [Resolve conversion priorities](#general-conversion-priorities-from-the-pinned-references)

### Premiere ↔ Resolve conversion

Use a shared graph/2 fixture for each feature where the source facts agree.
Track reading, writing, reopen, and render results per direction. Qualify
each writer with the receiving editor's own controls and renders.

| Shared priority | Premiere → Resolve | Resolve → Premiere |
| --- | --- | --- |
| Editable multicam | Two 23.976 OP1a cameras, A/B/A picture, and two fixed camera-A mono lanes verified after reopen and render. A fresh insertion kept both mono lanes and used camera A for picture. | The same Group returns as an editable Premiere multicam. Reopen and render verified all nine edits. A fresh insertion kept both fixed mono lanes and used Premiere's native picture default. Graph/2 also retains the actual reference audio source when Resolve saves a different angle label. |
| Mixed-rate picture and independent audio | The internal writer preserves plain 1080p ProRes HQ Frame Sampling in both directions between 24 and 23.976 fps. Reopened Resolve controls matched every Premiere frame selection, with separate WAV trims and verified mono lanes. | The internal Premiere writer reproduces both Premiere-origin maps and Resolve-authored default Nearest conform between 24 and 23.976 fps in both directions. It also preserves native 25 and 29.97 fps picture on a 23.976 timeline. Reopened projects matched every source selection and both mono WAV trims. A longer native 24-to-23.976 case used editable Time Remapping to preserve varying skip positions. |
| Editable nests, styled captions, and color | Qualify each receiving editor's controls and renders using the same source facts where possible. | Track unsupported controls by occurrence and preserve their facts through graph/2. |

#### Premiere → Resolve

The internal route qualifies selected linked ProRes/WAV edits, static and
two-key Linear Motion, and static mono clip volume. Plain captions and
bounded picture speed have separate native controls. Qualify with the
native Premiere save and reopened Resolve output. Use a retained linked
AAF import to answer a specific readiness question for a common workflow.
[Turnover evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md)
· [Adobe AAF export scope](https://helpx.adobe.com/premiere/desktop/render-and-export/export-files/export-aaf-files.html)

- **Resolve-assisted originals.** Internal Review shows which saved
  originals can link directly and which need a running Studio worker.
  Graph/2 supplies the timeline facts under both import strategies. The
  native assisted control covers 23.976 UHD AAC-LC, 25 fps wide-raster
  PCM24 and 29.97 drop-frame ProRes PCM24, alongside the 24 fps
  picture-only and embedded PCM controls. The original raster and source
  timecode travel separately from the 1080p timeline canvas. The reader
  also carries the saved Premiere sizing choice for raster mismatches.
  Reopened control renders agreed for picture and each isolated mono lane.
  Both import orders preserved the same edits. The warm Studio benchmark
  favored direct linking for an already supported original and API-first
  for the mixed and repeated-original batches. Standalone-first is the
  default and Review offers the Studio launch only for affected originals.
  [Worker requirements and qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#automatic-native-media-import-fallback)

##### Next Premiere → Resolve milestones

1. **Ordinary transitions.** Reopened controls qualify variable whole-frame
   left and evenly centered V1 Cross Dissolves and centered A1 Constant Gain
   crossfades in the existing linked profile. Plain mono Constant Power
   maps to editable +3 dB with explicit approval. It measured closer to
   Premiere than the linked AAF's 0 dB fade. Queue an exact Power envelope,
   custom fades and wider alignments separately.
   [Transition checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#variable-lengths-and-constant-power)
2. **Editable multicam.** The internal linked OP1a profile preserves two
   Premiere cameras, the A/B/A picture cuts and two separate mono outputs
   fixed to camera A through graph/2. Reopened Resolve renders confirm the
   picture and audio assignments, including fresh insertion in both
   editors. A reference-audio switch retained camera A on the changed
   edit and added short envelopes at the two cut edges. Capture preserves
   the saved angle and physical source separately. A genuine per-angle
   Source Audio switch is pending a saved control that renders camera B
   on just one lane. A targeted AAF benchmark can measure editability
   where the linked transfer retains the same structure.
   [Forward multicam checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-editable-multicam) ·
   [Return multicam checks](RESOLVE-SOURCE-READER.md#resolve-to-premiere-editable-multicam)
3. **Mixed-rate conform and media profiles.** The Resolve writer uses
   the exact Premiere source-time phase carried in graph/2 for both
   24-to-23.976 and 23.976-to-24 Frame Sampling. The latter supports
   zero and positive trims. Both reopened profiles matched the Premiere
   frame selections and preserved the independent WAV trims. The
   Premiere return path matched both Premiere-origin cadences and native
   PCM after reopen. The qualified cases can stand on their native
   evidence. [Resolve-authored default conform](RESOLVE-SOURCE-READER.md#resolve-authored-default-conform)
   is qualified for 23.976 picture on a 24 fps timeline.
   [Resolve write checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#mixed-rate-native-comparison) ·
   [Premiere write checks](RESOLVE-SOURCE-READER.md#resolve-to-premiere-mixed-rate-frame-sampling)
   Expand the linked writer across verified profiles already captured by
   the Premiere reader. The internal 1080p24 route can also use an already
   active, dedicated Resolve worker to import a verified original when
   standalone preparation cannot. The first profile covers picture-only
   and embedded stereo PCM split to two mono lanes.
   [Native import fallback](RESOLVE-TURNOVER-IMPLEMENTATION.md#automatic-native-media-import-fallback)
   Qualify moved originals and duplicate file names
   using verified paths, source timecode and explicit Tape Name → Reel
   Name ownership. [Metadata crosswalk](PREMIERE-RESOLVE-METADATA.md#cross-editor-transfer)
4. **Audio automation and layouts.** Verify clip volume and pan against
   the saved Premiere edit and native audio. Keep the Review loss until
   an editable Resolve envelope reproduces the measured ramp and unity
   region. Qualify wider channel layouts and their physical selectors.
   Extend Motion, crop and speed where the saved controls and reopened
   renders agree.
5. **Native project and package replay.** Complete a fully Premiere-authored
   ProRes/WAV project, then combine the qualified cases in a packaged trial
   covering Review, CLI, wizard and saved jobs. Keep the public route behind
   the [Resolve product gate](#resolve-product-exposure).
6. **Editable nests.** Qualify a child sequence with a nonzero start and
   a trimmed parent edit. Bind the child by identity and compare the
   visible source ranges and audio after reopen.
7. **Styled captions.** Build on plain editable subtitles. Verify font,
   size, position and line breaks on two adjacent cues, and report
   appearance losses where Resolve cannot reproduce the Premiere style.
8. **Native Color Mode.** Move the editable Contrast/Pivot prototype into
   the writer once trim timing, repeated media ownership and bounded
   composition read-back qualify. Establish an install-independent
   Resolve LUT carrier for embedded Premiere clip LUTs.
   [Color Mode evidence](PREMIERE-27-COLOR-MODE.md)

Editable multicam, nests, caption styling and Color Mode each have native
conversion acceptance cases. A linked AAF can measure release readiness
where Premiere exports the feature and Resolve imports it with the
original media.

- [x] **Capture the Premiere 27 linked media matrix in graph/2.**
      Reopened 23.976, 24, 25, and 29.97 controls retain verified original
      locators, frame and sample clocks, exact trims, and stereo or
      independent mono selectors. Standalone capture also retains exact
      saved timeline In/Out ticks, including an explicit frame-zero In.
      Media capture includes the staged AVC, HEVC, ProRes and DNxHR LB OP1a
      profiles. Qualify wider Resolve writer profiles against the captured graph.
      [Premiere linked-media source capture](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-linked-media-source-capture)
- [x] **Link Premiere 27 AVC MOV and MP4 into Resolve.** The internal
      1080p24 route keeps trimmed picture and independently sourced
      AAC-LC mono edits linked in place. Resolve reopen matched all
      36 picture frames and exact audio placement and routing on both
      lanes. A2 changed within one 256-sample window on native MP4
      reimport. [Media profile and native checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-avc-mov-and-mp4)
- [x] **Qualify the same Premiere AVC layout at exact 25 fps.** Reopened
      MOV and MP4 projects retain the 25-frame display clock, nonzero
      sequence start, trimmed picture and both independent AAC channels.
      All 72 picture frames matched. Three isolated 69,120-sample lanes
      matched exactly and a fresh native import reproduced the MP4 A2
      variation. [25 fps qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#exact-25-fps)
- [x] **Qualify 1080p29.97 DF Premiere AVC MOV and MP4.** Both linked
      projects retain the drop-frame minute rollover, trimmed picture and
      two independent AAC channels after Resolve reopen. All 80 picture
      frames and both 64,064-sample-frame lane renders per format matched
      native controls. Audio edits require whole-sample boundaries.
      [29.97 DF qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#2997-drop-frame)
- [x] **Qualify Premiere UHD AVC/AAC MP4 at 25 and 29.97 DF.** The shared
      UHD media cache and matching native project profiles retain 3840×2160
      straight cuts, exact clocks and both mono lanes after reopen.
      All 80 decoded picture frames and the four isolated PCM renders
      matched the native Resolve controls.
      [UHD qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#uhd-avc-at-25-and-2997-df)
- [x] **Link embedded PCM16 and PCM24 in Premiere AVC MOV and MP4.**
      HD MOV and UHD MP4 at 25 fps retain both independently selected
      mono channels, the matching canvas and nonzero source trims.
      All 160 picture frames and eight isolated PCM renders match native
      Resolve after reopen.
      [Embedded PCM qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#embedded-pcm16-and-pcm24-at-25-fps)
- [x] **Qualify embedded PCM at 29.97 DF.** HD MOV and UHD MP4 with
      PCM16 or PCM24 retain the drop-frame minute rollover, exact
      picture and audio trims, and independent left and right selectors.
      All 160 picture frames and eight isolated 64,064-sample-frame
      renders match the native Resolve controls after reopen.
      [29.97 DF PCM qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#embedded-pcm16-and-pcm24-at-2997-df)
- [x] **Preserve one native stereo A1 track at 25 fps.** Linked HD AVC
      MOV with embedded PCM16 or PCM24 keeps one editable stereo track and
      ordered left/right selectors. Both channels and all 80 picture frames
      matched native Resolve controls after reopen. Fresh insertion also
      retained stereo. [Stereo A1 qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#native-stereo-a1-at-25-fps)
- [x] **Preserve a separate stereo PCM16 or PCM24 WAV at 25 fps.** Premiere
      capture and graph/2 replay retain the original WAV, left/right order
      and an audio trim independent of picture. Resolve read-back, reopen
      and fresh insertion retain one stereo A1. All 80 picture frames and
      both PCM channels match the native controls.
      [Separate WAV qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#separate-stereo-wav-at-25-fps)
- [ ] **Qualify a fully native Premiere 27 source project.** Author one
      same-rate timeline with linked ProRes HQ MOV and PCM WAV, a nonzero
      start, nonzero trims, and independent mono channel selections. Use the
      [sequence inventory](RESOLVE-TURNOVER-IMPLEMENTATION.md#inputs-and-review)
      to select its saved ObjectUID. After Resolve Save Project As and reopen,
      compare clocks, trims, media selectors, picture, and isolated PCM.
      Packaged exposure follows the [Resolve product gate](#resolve-product-exposure).
- [x] **Capture and write an ordered Premiere project selection offline.**
      The internal 1080p24 manifest preserves two same-named timelines in
      distinct nested bins with their own starts and shared originals.
      The combined V1/V2 and three-mono control also preserves the narrower
      timeline and separate master identities for one file. Staged read-back
      and graph/2 replay agree. [Project selection checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-project-selections)
- [x] **Reopen and render the combined Premiere project.** Both retained
      ProRes/WAV selections passed Resolve save, reopen and re-export.
      Timeline order, bin ownership, shared masters, trims and channels
      survived. On each of four timelines all 48 picture frames matched
      native Resolve controls, and all nine isolated PCM renders matched.
      [Reopen and render evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-project-selections)
- [x] **Qualify combined stereo and mono tracks.** A native Premiere 27
      1080p25 source with independent trims on stereo A1 and mono A2
      round-tripped through graph/2. Both reopened Resolve outputs
      matched every frame and sample of the full mix and isolated track
      renders. [Mixed layout qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#stereo-a1-with-mono-a2-at-25-fps)
- [x] **Link 23.976 AVC/AAC at HD and UHD.** The three native Premiere 27
      source timelines kept exact clocks, independent trims and both
      channels after Resolve reopen. All decoded frames and both isolated
      PCM lanes matched native Resolve, including the UHD playable tail
      replayed from graph/2. The retained same-name conform control still
      matched. [23.976 qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#exact-240001001-fps)
- [x] **Link 1080p24 HEVC Main 10 with AAC-LC.** A Premiere 27 source
      with split-mono audio preserved its clock, trims and channels
      through the saved job and graph/2 replay. Both reopened Resolve
      projects matched the native HEVC cache, all 36 decoded frames
      and both isolated PCM24 renders.
      [HEVC qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-hevc-main-10-mp4)
- [x] **Capture Premiere Linear clip-volume keys and measure the curve.**
      Graph/2 keeps the qualified item-source times, dB endpoints, and
      native interpolation. Native Premiere parameter and PCM controls
      establish the difference from Resolve's dB-linear ramp.
      [Volume curve evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-linear-clip-volume-curve)
- [x] **Test editable Resolve keys for the Premiere clip-volume curve.**
      Rising and falling frame-key approximations survived Save As and
      reopen and measured within 0.046 dB of the Premiere renders. Neither
      reproduced Premiere's roughly 7 ms unity plateau. Forcing adjacent
      unity keys produced roughly 21 ms at shifted times.
      [Reopen and PCM results](RESOLVE-TURNOVER-IMPLEMENTATION.md#editable-resolve-ramp-trial)
- [x] **Test editable subframe keys for the same clip-volume ramps.**
      Resolve 21.2 saved two native keys within one frame and preserved
      generated subframe keys after Save As and reopen. The isolated PCM
      still missed the unity plateau in both directions.
      [Subframe results](RESOLVE-TURNOVER-IMPLEMENTATION.md#subframe-clip-volume-keys)
- [x] **Carry editable sequence Comment markers.** Premiere 27's
      linked AAF carried five marker positions and comments but Resolve
      imported none in the tested workflow. The native DRP retained all
      five with names, comments, colors and range lengths after reopen.
      [Sequence marker comparison](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-sequence-markers)
- [x] **Capture Premiere source clip Comment markers.** Both readers keep
      markers on the shared source item with exact local times. The
      Premiere 27 reopen control retained a point and a subframe range
      across two uses with different trims.
      [Source clip marker evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-source-clip-markers)
- [x] **Write editable Resolve Media Pool markers for the first picture profile.**
      Resolve 21.2 reopened frame-aligned Premiere source markers on the
      shared 24 fps ProRes HQ item. The two trimmed uses and native renders
      matched. Subframe markers remain in graph/2 and are reported in Review.
      [Linked source marker evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-source-clip-markers)
- [x] **Restore edit markers from verified Premiere provenance.** The
      1080p24 ProRes HQ round trip returned the markers to their cut and
      both cuts to the same source item after Resolve reopen.
      [Round-trip evidence](RESOLVE-SOURCE-READER.md#resolve-to-premiere-timeline-edit-markers)
- [x] **Preserve bounded Resolve edit-marker custom data through Premiere.**
      Two distinct values survived Premiere 27 and Resolve 21.2 reopen
      with their marker GUIDs and edit ownership resolved. Version 1
      records still restore ownership.
      [Custom-data evidence](RESOLVE-SOURCE-READER.md#resolve-to-premiere-timeline-edit-markers)
- [x] **Round-trip the eight Resolve-only edit marker colors.**
      The reopened 1080p24 control returned them to the trimmed V1 cut and
      retained a deliberate Premiere recolor. Three coincident timeline
      markers survived on their own owner. Picture and both mono lanes
      matched the native control.
      [Edit palette evidence](RESOLVE-SOURCE-READER.md#resolve-only-edit-marker-colors)
- [x] **Restore edit markers when the original is only in the Premiere bin.**
      Both marked cuts returned to one verified Resolve Media Pool item.
      The reopened project kept their separate marker owners, trims, and
      exact picture and mono audio. Review still lists diverged copies.
      [Original item recovery](RESOLVE-SOURCE-READER.md#restore-when-the-original-is-only-in-the-premiere-bin)
- [x] **Round-trip Resolve-only colors across source, edit, and timeline markers.**
      The reopened two-cut control kept all three marker owners. A saved
      recolor of a copied source marker flagged just that cut in Review.
      [Combined owner evidence](RESOLVE-SOURCE-READER.md#compare-copied-source-markers-on-their-saved-premiere-palette)
- [x] **Round-trip edit markers on the exact 23.976 clock.**
      Two trimmed cuts kept their eight edit markers after Premiere and
      Resolve reopen, alongside the timeline and shared source markers.
      Both isolated mono renders matched.
      [Fractional-rate evidence](RESOLVE-SOURCE-READER.md#edit-markers-on-the-exact-23976-clock)
- [x] **Round-trip edit markers on a tagged 24 fps original.**
      Both reopened ProRes HQ cuts kept file-relative edit markers and
      the verified `10:00:01:00` source timecode. Picture and both mono
      lanes matched the native control.
      [Tagged edit evidence](RESOLVE-SOURCE-READER.md#edit-markers-on-a-tagged-24-fps-original)
- [x] **Round-trip source and edit markers on a tagged 23.976 original.**
      Both reopened cuts kept the verified display origin, file-relative
      ranges, and marker ownership on the exact `24000/1001` clock.
      Picture and both mono lanes matched.
      [Tagged fractional-rate evidence](RESOLVE-SOURCE-READER.md#edit-markers-on-a-tagged-23976-original)
- [x] **Keep edit markers attached when a Premiere cut moves.**
      Two swapped cuts kept their eight edit markers, source trims, and
      shared source item. Timeline markers stayed at their record positions.
      The reopened picture and independent mono lanes matched the native edit.
      [Moved-cut evidence](RESOLVE-SOURCE-READER.md#edit-markers-follow-a-moved-cut)
- [x] **Keep edit markers attached when a Premiere cut is trimmed.**
      Six edit markers survived both editor reopens. Two markers outside
      the new bounds stayed in graph/2 and appeared by occurrence in Review.
      All 39 frames and both isolated mono lanes matched the native control.
      [Trimmed-cut evidence](RESOLVE-SOURCE-READER.md#edit-markers-on-a-trimmed-premiere-cut)
- [x] **Assign edit markers to adjacent pieces of a Premiere cut.**
      Seven edit markers kept their exact cut ownership after reopen.
      Both the two- and three-piece cases kept the crossing range in graph/2.
      All 48 frames and both mono lanes matched
      the native controls. Source and record continuity is checked at every join.
      [Split-cut evidence](RESOLVE-SOURCE-READER.md#edit-markers-on-adjacent-pieces)
- [x] **Keep the full timing of edit markers that cross a split.**
      The linked writer can write an editable timeline range after approval.
      The Resolve reopen preserved its timing, color, name, note, and custom
      data. A timeline marker at the same frame takes priority.
      [Timeline fallback evidence](RESOLVE-SOURCE-READER.md#timeline-fallback-for-markers-crossing-a-split)
- [x] **Preserve bounded Resolve sequence-marker custom data through Premiere.**
      Two distinct values returned to their timeline markers by GUID after
      Premiere 27 Save As and Resolve 21.2 reopen. All five timeline markers,
      the two edit markers, and the native picture and PCM comparison passed.
      [Round-trip evidence](RESOLVE-SOURCE-READER.md#resolve-to-premiere-sequence-marker-custom-data)
- [x] **Round-trip the eight Resolve-only timeline marker colors.**
      All 13 timeline markers kept their owner after Premiere 27 and
      Resolve 21.2 reopen. An explicit color change took precedence
      and a text-only edit kept the original color. Picture and both
      mono lanes matched the native control.
      [Timeline palette evidence](RESOLVE-SOURCE-READER.md#resolve-only-timeline-marker-colors)
- [x] **Preserve bounded Resolve Media Pool marker custom data through Premiere.**
      Both values returned to one shared 1080p24 ProRes HQ source item.
      The Premiere 27 and Resolve 21.2 reopen checks preserved separate
      timeline and edit marker data, trims, picture, and both mono lanes.
      [Round-trip evidence](RESOLVE-SOURCE-READER.md#media-pool-marker-custom-data)
- [x] **Carry Media Pool markers on the exact 23.976 source clock.**
      Source positions, the ranged duration, and both custom values
      survived Premiere 27 and Resolve 21.2 reopen on the shared ProRes HQ
      MOV. The 48 picture frames and 96,096 samples per isolated mono
      lane matched. An off-grid duration was reported by occurrence.
      [Fractional-rate evidence](RESOLVE-SOURCE-READER.md#exact-23976-source-marker-clock)
- [x] **Keep Media Pool markers file-relative on tagged 24 fps originals.**
      The shared ProRes HQ MOV retained `10:00:01:00`, both source markers
      and their custom values, exact trims, and matching picture and mono
      renders after Premiere 27 and Resolve 21.2 reopen. Both saved and
      graph timecode contradictions blocked publication.
      [Embedded timecode evidence](RESOLVE-SOURCE-READER.md#embedded-source-timecode)
- [x] **Round-trip source markers on standalone mono WAV.**
      PCM16 and PCM24 controls at 24 and `24000/1001` kept both markers
      per item, exact custom data, and matching isolated mono renders.
      A one-sample offset remained exact in graph/2 and required
      approval to omit the affected marker in Resolve.
      [WAV marker evidence](RESOLVE-SOURCE-READER.md#standalone-wav-source-markers)
- [x] **Keep stereo WAV source markers shared across independent lanes.**
      The `24000/1001` PCM16 and PCM24 controls kept one marker collection
      per WAV, both physical channel selectors, distinct trims, and
      matching isolated mono renders. Review listed an off-grid marker
      once by source occurrence.
      [Stereo WAV marker evidence](RESOLVE-SOURCE-READER.md#standalone-wav-source-markers)
- [x] **Carry all eight mapped source marker colors.**
      Premiere Magenta, Orange, and White map to Resolve Lavender, Sand,
      and Cream with occurrence-specific approval. The reopened ProRes
      picture and stereo WAV controls retained all 16 source markers.
      [Palette results](RESOLVE-SOURCE-READER.md#source-marker-palettes)
- [x] **Round-trip the eight Resolve-only source marker colors.**
      Premiere displays its default green and preserves the original
      color on the owning source item. A different saved Premiere color
      takes precedence. Native reopen and render checks passed for the
      ProRes and stereo WAV control.
      [Resolve-only colors](RESOLVE-SOURCE-READER.md#resolve-only-source-marker-colors)
- [ ] **Carry more annotations.** Extend clip marker clocks, additional
      marker types and colors on other owners, logging and labels.
      [Metadata crosswalk](PREMIERE-RESOLVE-METADATA.md#cross-editor-transfer)
- [x] **Verify Premiere timeline clip labels after reopening in Resolve.** The
      internal route wrote five edit colors and kept one neighbor uncolored.
      Standalone graph capture retains overrides and unrecognized labels
      by edit occurrence, including two edits that share a master.
      Follow up with a Premiere 27 authoring control for changed labels.
      [Timeline clip labels](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-timeline-clip-labels)
- [x] **Preserve Premiere timeline In/Out in Resolve.** The writer maps the
      exclusive Premiere Out to the last included Resolve frame and keeps
      the original ticks in graph/2. Reopened ranges at 23.976, 24, 25 and
      29.97 DF matched the native Resolve renders. Unsupported ranges
      remain explicit Review omissions.
      [Premiere In/Out qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-timeline-inout)
- [x] **Carry one-sided Premiere marks and frame-zero In.** Resolve 21.2
      retains these marks on Edit-page import and reopen. Deliver-page
      import can clear an explicit frame-zero In.
      [One-sided and zero-mark checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#one-sided-and-frame-zero-marks)
- [x] **Qualify editable 200% forward picture speed.** Three trimmed
      Premiere 27 controls use linked ProRes HQ at 24 fps. Saved-job and
      graph/2 replay preserve the exact file trim, including odd starts.
      All 60 source-frame selections match after reopening in Resolve.
      [200% qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-200-forward-picture-speed)
- [x] **Reopen Premiere track state in Resolve.** Saved Premiere 27
      names, edit locks, and V1 output survived on the V1/A1/A2 controls.
      V1 off rendered black while both picture edits and both audio lanes
      survived. Isolated PCM matched the enabled control on each lane.
      Sync lock remains a reported omission.
      [Track state](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-timeline-track-state)
- [x] **Carry static Premiere track gain, mute, and main bus gain.** The
      V1/A1/A2 WAV control keeps separate levels, original channel
      selectors, and the muted lane after Resolve reopen.
      [Static mixer qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-static-audio-mixer)
- [x] **Carry static Premiere mono track pan.** Both lanes retain editable
      pan, mute and independent levels after Resolve reopen. Hard pan has
      no crossfeed. Review reports the different pan law for intermediate
      settings. [Static mixer qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-static-audio-mixer)
- [x] **Carry whole-frame Premiere Constant Gain clip fades.** Both
      mono WAV lanes retain editable clip-edge fades and their original
      trims. Reopened PCM agrees outside the fade windows, and the
      two-edge control retains independent mixer settings.
      [Clip-edge fades](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-clip-edge-constant-gain)
- [x] **Preserve fractional Constant Gain durations.** A 23.976 mono
      WAV control preserves both exact tick lengths through Resolve
      reopen and Premiere project generation.
      [Clip-edge fades](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-clip-edge-constant-gain)
- [x] **Qualify eight-frame Premiere Cross Dissolve in Resolve.** The
      left-aligned and centered V1 cases preserve saved endpoints,
      cut, handles, and visible trims. Both renders match native
      Resolve controls after reopen. Review reports the change in blend
      sampling. [Transition qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-cross-dissolve)
- [x] **Qualify Premiere Constant Gain in Resolve.** The 24 fps A1 case
      preserves the centered eight-frame interval, the visible trims,
      and both mono assignments. Each isolated PCM lane matches the
      native Resolve control after reopen.
      [Crossfade qualification](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-constant-gain-crossfade)
- [x] **Combine the qualified picture dissolve and audio crossfade.** The
      1080p24 turnover preserves a V1 and an A1 transition on separate cuts.
      Reopened renders match the separate picture and audio controls.
      [Combined transitions](RESOLVE-TURNOVER-IMPLEMENTATION.md#combined-picture-and-audio-transitions)
- [x] **Compare the combined transitions against linked Premiere AAF.**
      Automatic source import reopens the untouched AAF online. Its two
      transitions, all 48 pictures and both isolated PCM lanes match the
      native DRP. The DRP keeps the original A1/A2 track numbering.
      The online marker control separately tests annotations and physical
      selectors on split stereo WAVs.
      [Linked AAF benchmark](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-linked-aaf-benchmark)
- [x] **Carry repeated editable transitions on V1 and A1.** Two
      centered eight-frame Cross Dissolves and two Constant Gain crossfades
      preserved all four windows and seven edits. Both middle clips kept
      their head and tail transitions. Resolve reopened the direct project,
      the graph/2 replay and the linked AAF with identical picture and
      both isolated PCM renders. Any NLE kept A1 and A2.
      [Repeated transition evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md#repeated-transitions-on-one-lane)
- [x] **Preserve whole-frame transition lengths and offer an editable
      Constant Power approximation.** A six-frame centered dissolve, a
      twelve-frame left-aligned dissolve and a ten-frame Constant Gain
      crossfade survived Resolve reopen. Review identifies the +3 dB
      mapping for the eight-frame Constant Power crossfade and requires
      approval. Both output paths match the native Resolve controls.
      [Variable-length transition evidence](RESOLVE-TURNOVER-IMPLEMENTATION.md#variable-lengths-and-constant-power)
- [x] **Map Premiere 27 clip Adjust Exposure to Resolve.**
      Graph/2 retains the saved clip value and color context. The verified
      1080p24 Rec.709 profile writes an editable CDL for isolated static
      Exposure across the saved −5 through +5 range. The seven reopened
      renders match native Resolve controls across 336 frames and stay
      within 1.29 RGB codes of Premiere at the measured patches.
      [Exposure qualification](PREMIERE-27-COLOR-MODE.md#clip-adjust-exposure-to-resolve)
- [x] **Capture Premiere 27 clip Adjust Contrast and Pivot.** Graph/2
      retains independently known values on the edit. Reopened Rec.709
      chart measurements cover the Contrast curve on both sides of unity,
      Pivot stops and Exposure before Contrast.
      [Contrast qualification](PREMIERE-27-COLOR-MODE.md#global-contrast-and-pivot)

#### Resolve → Premiere

The internal Premiere destination writes a selected linked 1080p ProRes
HQ timeline with two discrete WAV mono lanes at 24 or 23.976 fps.
The 24 fps picture-only profile also covers scoped forward speed, reverse
and plain captions.
[Current profiles](RESOLVE-SOURCE-READER.md#editable-conversion-profiles)
· [Resolve AAF export](https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf?_v=1783666810000#page=4162)

- [x] **Preserve Resolve-authored 23.976 picture on a 24 fps timeline.**
      Graph/2 retains the native Nearest phase. The editable Premiere
      output matched all 1,075 frame selections on three reopened
      trims and kept both independent mono WAV trims. The PCM matched
      native Premiere. The Resolve export confirmed the sync and sample
      extent. [Native default-conform results](RESOLVE-SOURCE-READER.md#resolve-authored-default-conform)
- [x] **Preserve Resolve-authored 24 fps picture on a 23.976 timeline.**
      Reopened Premiere projects selected every frame across three
      1,074-frame controls and one 3,113-frame control. The long edit
      used editable Time Remapping to preserve varying skip positions.
      Both mono lanes kept their independent WAV trims and matched
      Premiere PCM while active. Native Resolve confirmed the extent,
      sample placement and silent tail.
      [Native default-conform results](RESOLVE-SOURCE-READER.md#resolve-authored-default-conform)
- [x] **Preserve Resolve-authored 25 fps picture on a 23.976 timeline.**
      Default Nearest conform matched all 6,185 reopened Premiere 27
      frame selections across four controls. Editable Frame Sampling
      preserved the cadence, and both independent WAV lanes preserved
      their exact trims, PCM and silent tail under the measured mix
      policies. [Native default-conform results](RESOLVE-SOURCE-READER.md#resolve-authored-default-conform)
- [x] **Preserve Resolve-authored 29.97 fps picture on a 23.976 timeline.**
      Default Nearest's five-to-four cadence matched all 5,735 frame
      selections across four reopened Premiere 27 controls. The
      editable Frame Sampling clips kept their original picture In.
      Both mono WAVs preserved the independent trims, sample placement
      and silent tail. [Native default-conform results](RESOLVE-SOURCE-READER.md#resolve-authored-default-conform)
- [x] **Link timecoded 24 fps camera AVC with embedded PCM on a 23.976 timeline.**
      Reopened Premiere 27 controls preserved the source clock and
      default Nearest cadence for 4096×2160 8-bit 4:2:0 and 10-bit 4:2:2
      QuickTime originals. Two mono edits select the original stereo
      stream independently. Verified inherited input fit and both PCM
      channels. [Native default-conform results](RESOLVE-SOURCE-READER.md#resolve-authored-default-conform)
- [x] **Return an editable two-camera Group to Premiere 27.** The linked
      1080p23.976 OP1a profile preserves the nine cuts, both picture
      angles, and two mono lanes fixed to camera A through graph/2, Save
      As and reopen. All 145 picture frames and each isolated 290,290-sample
      render match the original Premiere control. Fresh insertion kept both
      mono lanes independent. The original Group picture default remains
      unknown in Resolve's save, so conversion requires approval for
      Premiere's fresh-insert default. Independently switched audio remains
      open. [Return multicam checks](RESOLVE-SOURCE-READER.md#resolve-to-premiere-editable-multicam)
- [x] **Write editable 200% forward picture speed.** Three same-rate
      1080p24 ProRes HQ MOV trims retained their exact file and record
      ranges after Premiere 27 Save As and reopen. All 60 rendered frame
      selections and the recaptured graph/2 mapping matched Resolve.
      [200% picture qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-200-forward-picture-speed)
- [x] **Capture and write two linked PCM16/PCM24 mono lanes.** Saved
      Resolve channel selectors and independent trims survive graph/2
      reload and internal Premiere project writing. [Linked audio profile](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Preserve Resolve Media Pool markers on a shared Premiere item.**
      The linked 1080p24 picture control kept a point and a range on one
      project item used at two trims after Premiere 27 Save As and reopen.
      Resolve custom data stays in graph/2 with an occurrence-specific
      export notice. [Source clip markers](RESOLVE-SOURCE-READER.md#resolve-to-premiere-source-clip-markers)
- [x] **Keep Resolve timeline edit markers on the intended Premiere cut.**
      Premiere 27 reopened a point and a range on the first of two cuts
      using the same original file. The marked edit's label, both trims,
      and the picture and audio renders survived.
      [Timeline edit markers](RESOLVE-SOURCE-READER.md#resolve-to-premiere-timeline-edit-markers)
- [x] **Save the original and edit-marker IDs in Premiere.** The internal
      linked route writes the relationship on the dedicated item.
      Premiere 27 reopen and graph/2 recapture verified both marker IDs,
      the original item, and the exact cut. The linked Resolve writer
      consumes this relationship on the qualified return route.
      [Round-trip evidence](RESOLVE-SOURCE-READER.md#resolve-to-premiere-timeline-edit-markers)
- [x] **Reopen and render the linked PCM cohort in Premiere 27.** All
      three controls kept their links, clocks, trims and independent
      channels after Save As and reopen. Isolated renders and fresh
      insertions match the selected source PCM within one 24-bit code
      value under Premiere's center-pan law. The 23.976 PCM16 stereo
      linked AAF matches the same cuts, source ranges, channels and
      Premiere-rendered picture. Its isolated audio differs by at most
      one 24-bit code value. [Native comparison](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Write editable clip gain, pan, and default fades.** Premiere 27
      retained the static controls and clip-owned Constant Gain fade
      intervals after Save As and reopen. Both isolated lanes were
      rendered and checked against the selected source samples and
      the Resolve fade curves. Nonzero pan retains its value and
      reports the destination pan-law difference. [Clip-control qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Write editable two-key clip volume.** Both trimmed mono lanes
      kept their key times and levels in Premiere 27 after Save As
      and reopen, including ramps overlapping default whole-frame
      fades. The conversion reports the measured difference in curve
      interpolation. [Volume-key qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Write separate static track faders.** Premiere 27 kept the
      A1 and A2 fader settings after Save As and reopen. The isolated
      renders show the expected fader scaling with both static and
      keyframed clip volume. [Track-fader qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Write static main stereo bus gain.** Resolve Bus 1 maps to
      the Premiere sequence Mix fader. Premiere 27 kept the Mix,
      track, and clip settings after Save As and reopen, and both
      isolated lanes matched the expected bus scaling. [Bus-fader qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Preserve static audio track mute.** Premiere 27 retained each
      lane's saved mute after Save As and reopen. Each muted lane
      rendered silence, the other lane matched its reference, and
      Mix, track, and clip levels survived. [Track mute qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Preserve static mono track pan.** Premiere 27 kept editable
      track pan alongside mute and independent gains. Both hard-panned
      lanes reached their expected output without crossfeed. The
      converter reports the intermediate pan-law difference.
      [Track pan qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Combine static track pan with clip pan and default fades.**
      Premiere retains the effective pan as an editable clip value on
      affected lanes and keeps track pan on lanes without clip pan.
      Both lanes retained mute, gains and editable fades after reopen.
      A separate linked AAF from the same opposing-pan fixture kept the
      original links, trims, channels and clip levels. Resolve omitted
      clip and track pan, track and bus gain, and track mute from the
      export. Premiere imported the saved fades as Constant Power, with
      a seven-frame tail where the Resolve project and native conversion
      kept eight. The isolated renders confirmed centered audio through
      AAF and the expected stereo bias through the native conversion.
      [Combined pan qualification](RESOLVE-SOURCE-READER.md#write-linked-audio-to-premiere)
- [x] **Write an editable mono audio crossfade.** Preserve the eight-frame
      centered Resolve Cross Fade 0DB at 24 fps as Premiere Constant Gain
      with exact endpoints and verified original WAV handles. After Save As
      and reopen, the blend follows Resolve's linear amplitude curve.
      The linked AAF imported as Constant Power and shifted the incoming
      source by four frames. A2 agreed within one PCM24 code value.
      [Crossfade qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-audio-crossfades)
- [x] **Write editable V1 Cross Dissolve.** Preserve an eight-frame
      left-aligned or centered Resolve dissolve at 23.976 with verified
      original picture handles. Both alignments reopened in Premiere 27
      with their visible trims, cut and interval. Premiere samples the
      blend differently, and the conversion reports that difference.
      Both isolated WAV lanes match the Premiere straight-cut control.
      [Video dissolve qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-video-dissolves)
- [x] **Reopen and render combined Resolve transitions in Premiere 27.**
      The internal 1080p24 writer preserves the centered eight-frame V1
      dissolve and A1 crossfade at separate cuts. After Save As and
      reopen, all 48 picture frames and both isolated 96,000-sample PCM24
      renders matched their respective Premiere single-transition controls.
      Both transitions, five edit ranges, and original channel selections
      survived. [Combined transitions](RESOLVE-SOURCE-READER.md#resolve-to-premiere-combined-transitions-at-24-fps)
- [x] **Write editable static position, scale, and crop.** The internal
      23.976 Resolve → Premiere path preserves signed X/Y position,
      independent width and height, and four source-space crop edges.
      Combined controls survived Premiere 27 Save As and reopen, with
      measured picture edges within one pixel of Resolve.
      [Static picture qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-static-picture-controls)
- [x] **Qualify Resolve → Premiere static 3D Warp geometry in Premiere 27.**
      The internal 1080p24 writer preserves centered rotation, centered
      crop with independent scale and rotation, and isolated displaced
      pivots. After Save As and reopen, all ten linked edits retained
      their exact ranges and Motion values. Measured chart points agreed
      with the Resolve renders within 0.06 pixels.
      [Scope and checks](RESOLVE-SOURCE-READER.md#static-3d-warp-geometry-at-24-fps)
- [x] **Write editable two-key Linear position and scale.** Preserve a
      shared source-frame grid with independent X/Y scale at 23.976.
      Premiere 27 retained both keys after Save As and reopen on the
      trimmed 48-frame fixture.
      [Linear picture qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-linear-position-and-scale)
- [x] **Write editable three-key Linear position and scale on trimmed
      clips.** At 23.976, Premiere 27 preserved the separate width grid
      and keys outside the visible edit after Save As and reopen. Six
      cases covered head and tail trims. All 192 rendered frames agreed
      with the Resolve controls within one pixel at the outer edges.
      [Trimmed motion qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-linear-position-and-scale)
- [x] **Qualify editable Resolve → Premiere Linear Z Rotation in Premiere 27.**
      All nine position, width and rotation keys at 1080p24 survived Save
      As and reopen. All 13 rendered frames agreed with Resolve within
      0.15 pixels at the measured chart points. Editing the middle rotation
      key survived a second reopen and changed the 11 interior frames.
      [Scope and checks](RESOLVE-SOURCE-READER.md#resolve-to-premiere-linear-z-rotation)
- [ ] **Map more captured controls to Premiere.** Qualify wider
      volume curves, custom fade curves, and track automation. Map
      wider animated picture controls and transition profiles through
      the shared placements and endpoints. Report unmapped Fairlight
      bus routing, processing and Resolve grade controls by occurrence.
- [x] **Qualify sequence marker colors in Premiere 27.** All 19 markers
      across two projects kept their exact times, ranges, text and mapped
      colors after Save As and reopen. The controls covered all eight
      Premiere slots and three reported substitutions. Editing one marker
      survived a second reopen.
      [Sequence marker colors](RESOLVE-SOURCE-READER.md#resolve-to-premiere-sequence-marker-colors)
- [ ] **Widen native project transfer.** Bring over more linked profiles
      and timelines, bin ownership, markers, and logging. Qualify one
      editable nest and a multicam with explicit angle and audio selection.
      Check moved media and duplicate filenames using the shared identity work.
- [ ] **Check Resolve edit-label display and editing in Premiere 27.**
      Save As and reopen retained all 16 mapped labels on individual picture
      and mono audio edits and eight default-label neighbors. Check their
      display and change one label in the timeline without changing the
      shared source master. [Timeline clip labels](RESOLVE-SOURCE-READER.md#resolve-to-premiere-timeline-clip-labels)
- [x] **Qualify Resolve track names, locks and picture output in Premiere 27.**
      Names, locks, both mono lanes and all edit ranges survived Save As
      and reopen. Disabling V1 rendered 48 black frames. Re-enabling the
      locked V1 restored all 48 picture frames and kept the lock. Both
      96,000-sample PCM24 lanes matched the selected WAV samples exactly.
      [Track state](RESOLVE-SOURCE-READER.md#resolve-to-premiere-track-state)
- [x] **Map Resolve timeline In/Out marks.** Matching audio and video
      marks stay editable in Premiere. Paired, one-sided, frame-zero and
      one-frame selections retained exact ranges after reopen at 24 fps.
      A separate 23.976 control matched picture and PCM across the cut.
      Independent audio and video marks still require approval to omit.
      [Timeline In/Out qualification](RESOLVE-SOURCE-READER.md#resolve-to-premiere-timeline-inout-marks)

### Premiere → Avid

Adobe documents the linked AAF workflow, including unmerging Premiere
merged clips and a separate relink step in Media Composer. Any NLE has no
end-to-end Avid destination route yet. [Adobe AAF export](https://helpx.adobe.com/premiere/desktop/render-and-export/export-files/export-aaf-files.html)

- [ ] **Establish the shared native Avid destination.** Use the existing
      AVB writer research for both incoming graph/2 routes, with **Link to
      originals** as the default. Start with one same-rate sequence using
      linked ProRes MOV picture and separate left and right selections from
      a stereo PCM WAV at independent, nonzero trims. Use Avid-authored
      links to establish the saved UME picture identity and the actual WAV
      link provider. Compare file paths, reel, source and sequence timecode,
      trims, and both channel assignments after Media Composer saves and
      reopens the bin. Qualify existing Avid-managed OP-Atom packages
      separately by verified package and sibling identity.
      [Destination architecture](ARCHITECTURE.md#adr-0004-more-programs-go-through-one-neutral-graph-2026-10-26)
- [ ] **Qualify a Premiere-authored cut and fade.** Reuse the Premiere
      reader for exact source ranges, saved locators, and audio selectors.
      Compare a current Premiere 27 fixture with the reopened AVB in
      Media Composer. Verify original-file linking and make relink
      requirements visible in Review. Use the linked AAF as a benchmark
      if a release or relink question needs the comparison.
- [ ] **Extend the verified Avid output.** Add markers, source logging
      fields, audio levels and pan, and common dissolves. Then qualify
      editable Motion, speed and text and investigate preserving the
      components of merged, nested and multicam clips.

### Resolve → Avid

Resolve can export standalone AAF. For an unedited imported Avid timeline,
its round-trip workflow can also carry unsupported material through from
the original AAF. Any NLE has no end-to-end route here. Use a Resolve-authored
linked timeline as the core comparison. [Resolve AAF export](https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf?_v=1783666810000#page=4162)

- [ ] **Connect the Resolve reader to the shared Avid destination.** Start
      with a linked same-rate MOV/WAV sequence, known reel and source
      timecode, and distinct mono selectors. Compare the reopened native
      AVB with the saved Resolve edit and render. Resolve's standalone AAF
      is available for a targeted readiness benchmark. Reuse the direct
      DNxHR package observations for a separate managed media test.
      [Resolve source profiles](RESOLVE-SOURCE-READER.md)
- [ ] **Carry source edits and annotations.** Add sequence markers,
      nonzero source trims and a dissolve, then clip gain, pan and fades.
      Qualify same-rate speed, reverse and still frames before mixed-rate
      conversions. Keep Fairlight buses and grades in the source receipt
      when the Avid destination has no established mapping.
- [ ] **Qualify editorial structures and round-trip behavior.** Add bins,
      multiple timelines, and one simple nest or Group. Test a re-edited
      Resolve timeline and separately check an unedited Avid-origin
      timeline to distinguish new conversion from AAF carry-through.

## Declared multichannel media (User, 2026-11-07)

The 7.1 review retains eight-channel Adaptive plus explicit routing; it is not a
dedicated 7.1 speaker bus. Surround timelines now preserve the selected preset’s
DF/NDF display policy. Review and correction.

- [x] Add explicit 5.1 and eight-channel Adaptive group preparation for DNx OP1a and
      bounded ProRes LT MOV, preserving reordered Avid labels and PCM16/PCM24 values.
      Converted timelines, fresh bin insertion and actual native save/reopen pass
      the bounded Mac cohort. [Scope, CLI and verification](MULTICHANNEL-REWRAP-2026-11-07.md).
- [x] Reconcile current acceptance inputs separately from the immutable historical
      manifest: reviewed CONV-0034, explicit 60-model selection and a fresh native
      reference reproducing retained PCM/pictures. The historical return's provenance
      remains unresolved. [Qualification and acceptance record](PLAYBACK-ACCEPTANCE-2026-11-07.md).
- [ ] Resolve **59.94 short-loop playback performance**: selected DNx Adaptive8
      runs report 69 and 47 dropped frames, including a repeat without offline work.
      Audio and sampled pictures remain exact. No converter/container cause is established;
      retain this gate rather than making a runtime change from counters alone.
- [ ] Finish User listening/acceptance. Broader ProRes geometry/rates, Windows and
      long-source streaming remain pending. Three other selected ten-minute loops
      report zero drops; these short synthetic cases do not qualify long footage.
- [ ] Qualify mixed-width surround timeline output before adding routing/downmix policy.
      Explicit refusals remain. The native wizard/default menu is implemented; its wider
      application and platform qualification remains separate.

## Original links, native wizard and release preparation (2026-11-08)

Implementation, evidence and remaining gates.

- [x] Typed linked-original source/writer route, explicit stream/channel selection,
      locator/root-map resolution, content pins and bounded physical probing.
- [x] Shared prepare-once GUI/CLI job engine, project inventory/dependencies, destination
      folder layout, explicit losses, settings replay and safe publication reporting.
- [x] Five-page native Rust wizard, presets, color/effect review, cancellable work and
      verified adjacent LGPL-only backend resolution.
- [x] Six native target definitions, MIT/third-party notices/source collection, private
      candidate gates and non-overwriting draft-release tooling.
- [x] A generated H.264/AAC master matches sampled picture and PCM from Premiere's
      native import in a two-second macOS fixture.
- [x] **Link picture-only 1080p24 ProRes HQ MOV** (CONV-0054). Use the
      saved Avid locator with the observed zero-origin timecode track and
      a verified picture packet grid. The original and generated freeze
      stayed online after Save As and reopen in Premiere 26.5.2. The
      linked original matched all 240 direct-import pictures. The
      generated freeze matched all 120 Avid pictures.
- [x] Preserve stereo left/right on fresh inserts for the scoped linked AAC bin
      route. The mapped two-second fixture stayed linked and played after
      Premiere Save As and reopen. Existing sequence audio keeps its source grouping.
- [x] Block Premiere conversion before writing when unsupported source rate
      routing or a recognized automatic Avid Motion Adapter would omit timeline edits.
      Review names the affected sequence and tracks. A same-rate UME/OP1a control
      stays online after Premiere Save As and reopen.
- [x] Translate the qualified automatic 24 fps to 23.976 Motion Adapter on
      linked H.264/AAC. Premiere Save As and reopen matched the Avid picture cadence
      and both Direct Out PCM lanes on one trimmed mixed-media sequence.
      [Scope and refusals](MOTION-ADAPTER-24-TO-23976.md).
- [x] Translate the qualified automatic 23.976 to 24 Motion Adapter on linked
      H.264/AAC. Premiere Save As and reopen matched the picture repeat and
      both PCM lanes within the MP4's declared extent. The conversion reports
      silence beyond that boundary where Avid can render decoded AAC padding.
      [Scope and refusals](MOTION-ADAPTER-24-TO-23976.md#automatic-23976-to-24).
- [ ] Broaden native AVB-to-UME playback qualification with representative saved
      bins. The supplied long-form MP4 also failed Premiere's direct import.
- [x] **Read back linked PCM16 and PCM24 WAV in Premiere.** Synthetic stereo
      writer controls retained exact source trims, left/right samples and fresh
      insertions after Save As and reopen. In this audio-only control, the
      encoder began the export at the first occupied edit. The Premiere
      sequence retained the leading silence.
- [x] **Saved Avid-linked PCM16 and PCM24 WAV via the job engine.** Two
      WaveAiff links at 48 kHz retained A1/A2 as separate mono placements. With
      Avid default pan selected, Premiere matched both 48,000-sample trimmed
      Direct Out channels after Save As and reopen. Fresh inserts retained
      both source selectors. Premiere centers new mono clips by default.
      Explicit clip hard pans matched all 96,000 samples per source channel.
- [x] **Align original-file conversion across entry points.** Direct
      `convert --link-originals` and saved jobs both write the saved Avid
      PCM16 and PCM24 WAV fixtures. An ambiguous exact path remains a
      blocker. Standalone `graph` continues to use native media lookup.
- [x] Remove the identified private diagnostics from reachable GitHub export history.
      The reviewed private export and cleanup evidence are retained.
- [x] Publish [Any NLE v0.1.0-alpha.1](https://github.com/whatever-industries/any-nle/releases/tag/v0.1.0-alpha.1)
      from the reviewed public source. All six platform builds, extracted-package audits
      and download checksum checks passed. [Release gates](RELEASE.md).

## Cataloged audio configuration gaps (User, 2026-11-07)

Complete audit and evidence boundaries.
Grouping/order, master output mix, timeline track capacity and native qualification
are separate. No held mixing/speaker-policy work is resumed by this list.

- [x] **Priority 1 — Preserve and diagnose Master Clip Output Mix Format.**
      Raw absence/type/value now survives extraction and graph/2, including unresolved
      masters and bounded opaque values. Preparation reports present settings as unapplied;
      legacy model/1 warns without a schema change. Missing never implies Stereo.
- [x] **Priority 1 — Plan destination audio-track capacity before writing.**
      Both writers allocate to the highest event/group/level/fade lane within 1..99,
      preserving the selected clock and output bus. Mono-discrete bus expansion refuses.
      Eight-mono, complete Phase25-like six-track and sparse A99 controls pass structurally.
      Fresh native qualification of expanded outputs remains pending below.
- [x] **Priority 2 — Complete permanent named-layout regression coverage.**
      5.1 AAC and 7.1 SMPTE DS now have reader, graph, preparation and MXF/MOV writer
      cases; all six named wide layouts have literal saved-selector checks. Mono/stereo,
      custom sparse/reversed order and composite controls remain covered.
- [ ] **Priority 2 — Qualify each cataloged layout end to end.**
      Native cohorts cover selected 5.1 SMPTE and 7.1 EXT declarations, not all named
      variants. Use admitted media, distinct channel signals, complete PCM, fresh insertion
      and save/reopen; keep source codec, grouping, output mix, speaker monitoring and
      audio-only admission separate. No long native test is scheduled by this audit.
- [x] **Priority 2 — Reconcile CONV-0034's grouping provenance.**
      Added the reviewed exact AVB-0005 revision/hash for Phase24/25; prior pins and
      confidence unchanged. CONV-0035 records preservation/capacity boundaries.
- [ ] **Priority 2 — Native-check expanded destination track counts.**
      The Premiere writers also allocate picture tracks through the highest
      occupied lane or video filler marker. After Premiere 27 Save As and
      reopen, two V5 dissolves matched all 64 rendered frames from the V1
      controls. A V7 marker carrier originally covered two frames of V1
      picture. With the sequence fallback for that marker, five markers
      remain on V7 and the sixth keeps its range on the sequence.
      The corrected export matched all 33 decoded frames and 52,853 stereo
      PCM sample frames from a fresh lower-track control. A derived
      23.976 DNxHD LB and PCM16 sequence with overlapping V1 and V5
      edits also retained both lanes, empty V2–V4, and two mono audio
      lanes after Premiere 27 Save As and reopen. With each picture
      track isolated in turn, all 48 frames and 96,096 PCM sample
      frames per channel matched the single-track control.
      A second derived bin used different source ranges on V1 and V5,
      with a gap on V5 after the cut. The reopened Premiere 27 render
      used V5 for all 24 overlap frames and V1 for all 24 frames in the
      upper-track gap. Both mono lanes matched the single-track control
      sample for sample.
      A synthetic 24 fps DNxHR OP1a project with eight independent PCM24
      mono tracks retained all eight selectors and the source trim after
      Premiere 27.0.0 build 88 Save As, close and reopen. In eight isolated
      stereo exports, each track matched all 16,000 source samples on its
      assigned side, with digital silence on the opposite side. This
      qualifies saved timeline placement and routing for the writer fixture.
      Fresh insertion from the same bin clip placed V1 and eight independent
      mono edits on A1–A8 in source order. After Save As, close and reopen,
      all eight isolated 24,000-sample exports matched the corresponding
      source channels within one PCM24 count of Premiere's center-pan law.
      The 24 fps two-pair/four-mono DNxHR OP1a writer fixture also preserved
      all six placements and trims after Save As, close and reopen. The
      two stereo pairs retained physical channels 1/2 and 7/8. The four
      mono tracks retained channels 3–6. Each of the eight active outputs
      matched all 16,000 expected PCM24 samples in isolated exports.
      Fresh insertion of the same bin clip placed V1 and six audio groups
      in saved source order, with stereo channels 1/2 on A1, mono channels
      3–6 on A2–A5, and stereo channels 7/8 on A6. After save and reopen,
      isolated 24,000-sample exports matched both stereo pairs exactly and
      each mono channel within one PCM24 count of Premiere's center-pan law.
      A sparse 24 fps DNxHR OP1a writer fixture kept its A1 and A99 edits
      and empty A2–A98 after Save As, close and reopen. Both isolated tracks
      matched all 16,000 expected PCM24 samples with their original trims
      and assigned output sides. This covers sparse mono placement.
      Avid-authored eight-mono ingest remains open. The saved Phase 26
      all-mono master references eight separate audio file packages. None
      resolved in the checked managed media roots. The online [managed OP1a
      mixdown cohort](../../knowledge/avid/findings/AVB-0023.json) exposes a
      separate source profile. The retained 24 fps DNxHD Direct Out control has one picture
      and two independent PCM24 streams. The AVB routes by media kind and
      file track label. Each file track's saved physical track number
      matches the MXF essence key, which identifies the decoder stream by
      packet position and hash. The picture spans 120 frames and each
      mono stream contains 240,000 samples. A writer-only project linked
      this file in place and retained both channels after Premiere 27 Save
      As, close and reopen. Each of the 100 rendered frames matched the
      intended source position and both isolated mono exports matched
      all 200,000 trimmed samples exactly. Graph/2 now retains the shared
      file package, saved track numbers and child descriptor clocks for
      all eight masters in the unresolved master receipts. A standalone
      verifier now confirms the Direct Out profile against the MXF package
      paths, exact physical clock and complete decoder packet extents. It
      resolves the two mono PCM stream ordinals by essence key and checks
      the file revision before and after probing. Missing, conflicting or
      swapped selectors, a different package and damaged essence refuse.
      Separate files mode now binds the verified Direct Out profile through
      graph/2 and the Premiere native master writer. Direct CLI and saved
      jobs link the existing MXF in place and recheck it before publishing.
      The converted CLI project reopened in Premiere 27.0.0 build 88 with
      the 100-frame trim and both mono lanes. All decoded picture frames
      matched the reopened writer control, and each isolated lane matched
      its original PCM within one PCM24 count under Premiere's center pan.
      The control uses a derived sequence over the retained Avid master.
      Grouped, Merged, Rewrap, and other managed OP1a layouts remain open.
      The two retained eight-channel mixdowns declare 7.1 groups. Expanded
      stereo, surround, and crossfade cases on high audio tracks remain open.

Implemented fixes, tests and remaining native gates.

## Project import (User, 2026-10-14)

- [x] **Central source-graph migration implementation** (2026-11-05). Reliability,
      graph/2, direct Avid source extraction, explicit Premiere preparation and
      prepare-once publication are integrated locally. Required fixtures, full
      suites, project/LUT differentials, bounded memory comparisons and a separate
      final canonical verification pass are complete; private receipts retain
      the frozen baseline, exact corrections and qualification limits.
      Corrected native timing and wider application qualification remain separate.
      [Design and gates](SOURCE-GRAPH-MIGRATION.md),
      [correction ledger](SOURCE-GRAPH-CORRECTIONS.md).

- [ ] **Finish split-edit listening and User checks**
      (2026-11-05). Native Avid control/fade fixtures, graph/2 preparation and
      Premiere save/reopen preserve split ranges, mono routes and asymmetric
      audio transitions. The omitted TC1 marker correction (`07fe580`) is integrated
      locally. The [2026-11-06 native audio renders](SPLIT-EDIT-AUDIO-2026-11-06.md)
      pass sample timing/routing and fade-window checks. The
      [2026-11-07 follow-up](MEDIA-QUALIFICATION-2026-11-07.md) adds three normal
      passes and over ten minutes of native looping per sequence, unchanged post-loop
      PCM and a native Avid reference. The qualified direct Separate route now
      uses [Constant Gain for verified Linear curves](SPLIT-EDIT-AUDIO-2026-11-06.md#linear-audio-dissolve-against-linked-aaf).
      Wider fade profiles and User listening remain open. See the release checks below for absolute TC1.
      [Evidence and limits](SPLIT-EDIT-READBACK-2026-11-05.md).

- [x] **Point the app at a whole Avid project folder.** Walk it recursively, select
      `.avb` (case-insensitive: real projects contain `.AVB`), and skip everything else
      (`.avp`, `.log`, `.lck`, `.awf` waveform caches, `.xml`, `Statistics/`, `WaveformCache/`,
      `.DS_Store`, …). Entire Project mode skips the inventory search for
      extra dependency bins because the selection already includes every
      eligible bin. Review still resolves and validates each source route.
      Inventory reads master facts without compiling sequence edits and
      counts media by saved package ID. The retained 1,903-bin offline
      project completes the catalog and reaches source preflight.
- [x] **Keep selected bin folders** as nested Premiere bins under one UTC
      `AVID_IMPORT_{YYYY-MM-DD_hh:mm:ss}` root.
- [x] **Show import progress** by stage, with the current bin and count during bin
      processing. The wizard and saved-job CLI also show the current file and
      count while rechecking linked media before publication. During
      preflight, they show when the converter checks saved links, builds
      timelines, and validates the graph. Publication shows the percentage
      of diagnostics written. Preflight losses and results appear in the report.
- [ ] Show a running problem count during bin processing.
- [x] **Read one decoded AVB at a time**, including folder imports through the
      shared job engine. Compact per-bin facts support cross-bin references.
      The maintainer handoff records the streaming measurements and limits.

- [ ] **Reduce large-bin decoding and retained project/output memory.** The whole-file decoder
      still has per-bin temporaries, and summaries/output grow with project size. The
      64-bit Avid source reader has an 8 GiB project-wide accounted budget.
      Encoded graph/2 retains its 256 MiB bound and the standalone legacy model API
      retains its 16 MiB input limit. Premiere project writing batches sequence
      extent updates to avoid rescanning all earlier edits. Historical private selections reached the 4 GiB benchmark
      guard. Reference scale: one real project had 2,986 bins in 246 folders, 11 GB, largest
      bin 136 MB; reading one decoded bin at a time is not a constant-memory project importer.
      An offline Entire Project replay across 1,903 bins now completes through
      the saved-job path. It stages source diagnostics on disk and publishes a
      Premiere project with exact-span gap markers where the saved timing
      permits. Structural read-back checked all 1,217 timelines. Memory use
      for the same scope with online media and full-project Premiere reopen
      and playback remain unmeasured.
- [ ] **Add read-only batch diagnostics for Avid → Premiere Review.** A local
      census continued past the first missing-original refusal and identified
      offline file-clock and descriptor gaps, saved locator errors, unsupported
      sequence timing, and unresolved edit sources. Group findings by timeline
      and retain the bin, timecode, track, occurrence and reason. Distinguish
      intact edits, safe effect omissions, offline items, marked gaps,
      omitted bin items and failures that block publication. Let reviewers
      filter by timeline or referenced source. Continue preparing unaffected
      edits and sequences where safe. When timing, routing, or affected scope
      cannot be established, offer exclusion of the affected sequence once
      its dependencies are known. Keep checks that need online media distinct
      and preserve the existing publication guards.
      Rewrap Review checks the remaining saved original paths when it encounters
      a missing, malformed, or ambiguous locator. The bounded report identifies
      affected bins and occurrences. The additional checks resolve paths using
      the current mappings and stop short of probing the media.
      A legacy MXF package refusal also triggers a report of other absent or
      ambiguous package IDs on the affected clip and later clips, using the
      existing media index. Duplicate package details include up to eight
      conflicting paths and the indexed count. Review also lists the paths
      for duplicate managed packages found by the linked clip modes.
      With a complete dependency check, linked modes report duplicates
      outside the selected bins' saved references as warnings. The same
      check keeps qualified omissions and gaps available for missing media.
      Package failures open Avid media locations in Review. Failures resolving
      linked originals open saved-path mappings.
      When source verification reaches the media scan and channel routing
      checks, Review lists their findings separately, including when a missing
      or unresolved master is the primary blocker.
- [x] **Browse large preflight and completion reports.** Show 25 notes at a
      time, with a page-number field, search across all notes, and filters
      for translation changes and warnings. Keep every occurrence in the
      saved completion report. Review borrows the prepared report, which
      moves into completion. Saved jobs stream the receipt and CLI JSON.
- [x] **Save preflight findings before conversion.** Review can export all
      findings from a successful preflight to a new JSON file while keeping
      the current job ready to convert. A blocked preflight can save its reason
      and full collected details, including affected clips, Groups, or timing
      findings. Review can search the full blocker list and browse 25 matching
      details per page. Preflight still stops at the first blocking stage.
- [x] **Trash/Attic opt-in** in the project step. The implemented default is off.
- Duplicate bin names in one folder: not possible (Avid forbids two same-named bins in a
  folder, User 2026-10-14); a case-only clash on a case-insensitive disk is the one thing to
  report rather than assume.
- Lock files (`.lck`): ignored; the bin is read as saved (User, 2026-10-14).

## Media linking (User, 2026-10-14)

- [x] **OP-Atom → OP1a CLI preparation** (User, 2026-11-04; CONV-0030).
      `any-nle convert ... --rewrap-op1a NEW_DIR` displays **“Creates new media.”**,
      estimates disk use and uses external FFmpeg/ffprobe 8.1 with no encoding. Optional
      `--rewrap-dry-run` writes nothing. The 2026-11-04 extension covers 20 exact MXF
      rates, including 24000/1001, common broadcast rates and experimental extended
      rates through 120; extended rates use reported frame-count display. Video-only
      or 1–32 discrete mono 48 kHz PCM tracks at one 16/24-bit depth per output,
      require exact matching source clocks, spans and sample counts. Inputs stay in
      place. PCM bytes, stream hashes and exact packet clocks are verified. Default
      conversion behavior and model schema are unchanged. Existing timeline-layout
      limits are checked before media creation. [Rate/audio scope](MEDIA-REWRAP.md).
- [x] **Experimental native OP1a-backed AV-master route** (CONV-0031).
      Explicit `--rewrap-native-master` replaces bin mode with ordinary file-backed
      masters, preserves mono/ordered stereo groups, source routing, exact clocks and
      effective gains, and shares physical media across distinct logical masters.
      Matched Rust/Python and structural/backend checks; bounded native trials are recorded
      below. User acceptance and wider qualification remain pending.
      [Use, refusals and tests](NATIVE-OP1A-MASTERS.md).
- [x] **Correct OP1a channel assignments and native bin grouping (bounded checks).**
      Full-source templates and physical-channel vectors preserve mono, normal/reversed
      stereo and non-adjacent stereo plus mono on fresh bin insertion. Timeline offsets,
      group gains, zero overrides and save/reopen exports pass in retained Premiere 26.5.2
      PCM16x2 / PCM24x2/x3 fixtures at 23.976 fps. Legacy constructions use corrected
      flattened channel views. Mixed-depth output refuses without normalization.
      [Evidence and remaining qualification](NATIVE-OP1A-MASTERS.md).
- [x] **Synthetic AVB end-to-end OP1a check** (2026-11-04). Actual CLI extraction,
      media identity matching and strict rewrap through Premiere save/reopen pass for
      three PCM16 or three PCM24 streams at 23.976 fps. Complete rendered timeline PCM,
      grouping/routing, picture offset/gap and refusal checks pass. Agent-operated,
      same-author double-check; no User acceptance or production-bin claim.
      [Scope and evidence pins](OP1A-END-TO-END-2026-11-04.md).
- [ ] **Widen OP1a native qualification.** User acceptance, additional Avid-created
      production bins, more channels, profiles and Premiere versions remain open.
      Selected declared 5.1 and Adaptive8 cohorts are covered above. Sparse labels
      and 32-source cases have backend coverage. Audio-only preparation, flexible
      source spans, derived mono-from-stereo, arbitrary MCA routing, unequal stereo
      gains and verified resume/reuse remain open. [Current scope](MEDIA-REWRAP.md).
- [x] **Qualify direct linking for 23.976 DNxHR LB in Premiere 27** (2026-11-12).
      Separate files links the original 1080p picture and two mono PCM24/48 kHz
      siblings. Save As and reopen preserved the two split-edit timelines, and
      all decoded frames and both isolated PCM lanes matched the rewrap control.
      [Scope and evidence](SPLIT-EDIT-AUDIO-2026-11-06.md#premiere-27-direct-linking).
- [x] **Qualify Grouped and Merged for the same 23.976 DNxHR LB profile.**
      Both retained the original MXFs and matched picture and both mono lanes
      after Premiere 27 reopen. Fresh insertion of each bin item also matched
      the Separate files control.
      [Combined clip checks](SPLIT-EDIT-AUDIO-2026-11-06.md#grouped-and-merged-direct-linking).
- [ ] **Qualify wider direct OP-Atom profiles.** Direct ProRes OP-Atom playback
      remains unresolved.
      [Evidence and decision gate](SPLIT-EDIT-AUDIO-2026-11-06.md#users-fallback-decision-and-remaining-work).

- [ ] **Resolve the retained anamorphic ProRes fixture and OP1a alternative**
      (2026-11-07). Its 446/447 duration/index conflict, 21952-byte terminal nonframe,
      SAR16:15/model geometry and identity mismatch remain outside automatic admission.
      The separate bounded correction preserves 446 pictures/clocks offline; native
      playback of that original-footage candidate remains pending. The implemented
      square-pixel ProRes LT MOV slice does not reuse a DNx terminal exception or
      silently normalize this source. [Investigation](PRORES-DURATION-2026-11-07.md).
- [x] **Rewrap admitted Avid ProRes LT OP-Atom to MOV** (User, 2026-11-07).
      Premiere originally raised “Frame substitution recursion” on a ProRes LT
      OP-Atom source. MOV stream copy played and exported in the retained trial.
      The converter now prepares admitted progressive square-pixel 1080p24 ProRes LT
      with PCM16/PCM24. Selected 5.1 and Adaptive8 macOS controls passed native render,
      reopen and ten-minute loops. [Implementation](MULTICHANNEL-REWRAP-2026-11-07.md).
      [Playback checks](PLAYBACK-ACCEPTANCE-2026-11-07.md).
- [x] **Rewrap Avid-transcoded picture-only ProRes LT with a verified zero tail.**
      A native 1080p24 square-pixel control uses a separate OP-Atom label and
      exposes a 37th demuxed packet after 36 coded pictures. The MOV keeps all
      36 compressed frames, source timecode and color tags. A nonzero tail
      blocks both CLI and saved-job publication. Premiere 27 Save As and
      reopen preserved both trimmed picture edits.
      [Native control](PRORES-DURATION-2026-11-07.md#native-avid-picture-only-prores-lt).
- [ ] **Expand ProRes MOV qualification.** Wider rates and profiles, long production
      footage, User acceptance and Windows remain open. The retained anamorphic
      fixture has its own duration, identity and geometry task above.

- [x] **Implement bounded MOV audio bindings and compliant PCM descriptions**
      (2026-11-07). Discrete interleaved PCM has version-2 `lpcm` descriptions;
      wide MOVs use per-channel mono source views instead of OP1a's flattened views.
      AVB grouping controls vectors, including multiple pairs, 5.1 and eight-channel
      Adaptive. Separate mono MOV tracks and generic layouts remain unqualified.
      [Implementation and read-back](MULTICHANNEL-REWRAP-2026-11-07.md).
      [Earlier failed controls](MOV-AUDIO-LAYOUTS-2026-11-07.md) remain retained.
- [x] **Native MXF preflight corrections** (2026-11-04; CONV-0032).
      Exact RI00/DNxHR LB label, verified sibling-package placeholders, explicit
      demuxer indexes, bounded private-header framing and source-offset timecode
      calculation. [Scope, evidence and remaining blocker](NATIVE-MXF-PREFLIGHT-2026-11-04.md).
- [x] **Investigate the selected native DNxHR terminal packet.** A physical/frame-hash
      check and generated counterexamples distinguish the zero tail from a real extra
      picture and from index-only changes. A bounded copy prototype preserves coded
      video and complete PCM. [Evidence and implementation requirements](NATIVE-DNX-TERMINAL-PACKET-2026-11-04.md).
- [x] **Implement narrowly validated native DNxHR zero-tail preparation** (2026-11-05;
      CONV-0033). Scoped LB 1080p23.976 coded/index/packet checks, singleton zero-tail
      exclusion and separate original/copied/excluded hash receipts. The retained
      Avid-saved36-frame fixture preserves every coded picture and both complete PCM24
      channels through the converter. [Guards, evidence and limits](NATIVE-ZERO-TAIL-2026-11-05.md).
- [x] **Finish the bounded native Premiere read-back of the guarded native-source
      conversion** (2026-11-05). Actual Avid-saved fixture through CLI, native
      open/save/close/reopen, exact complete PCM24 channels, stable picture positions
      and playback endpoint pass in Premiere26.5.2 build5. [Evidence and limits](NATIVE-OP1A-READBACK-2026-11-05.md).
      User/wider acceptance remains pending. The full mixed native/UME bin's separate
      PCMA02 decoder gap remains open.
- [x] **Clarify the CLI construction summary** (2026-11-04). With
      `--rewrap-native-master`, the final summary identifies the experimental native
      OP1a AV-master route, consistent with writer notes and `conversion.json`.
      Ordinary conversions retain their selected bin-mode summary.
- [x] **Resolve the effective Premiere construction once** (2026-11-04).
      One internal route now drives preflight, writer selection, receipts and CLI
      reporting after policy overrides. Public interfaces, model/policy schemas,
      refusal ordering and project construction remain unchanged.
- [x] **Separate media-preparation responsibilities** (2026-11-04).
      The existing `Plan` interface now fronts focused modules for backend inspection,
      source fingerprints/space checks, clock/group validation, read-only planning,
      execution and receipt writes. Existing tests moved alongside them. Validation
      order, exclusive output handling, retained failures and receipt schemas stay
      unchanged. Entry point: [rewrap module](../crates/app/src/rewrap.rs).
- [x] **Media output folder prompt (macOS CLI app).** `--rewrap-op1a-prompt` opens a native
      parent-folder picker, then reviews the new child folder, “Creates new media.” notice,
      estimated additional bytes and available volume space before any outputs. Cancellation
      writes nothing. The CLI also supports explicit destination and dry-run.
      [CLI and wizard review](MEDIA-REWRAP.md#import-option-and-review).

- [ ] **General media repair for Premiere decode failures** (User, 2026-10-20).
      Scope this as an opt-in repair for media outside the current managed OP-Atom
      routes. Candidate inputs include XAVC or other long-GOP MXF, m2ts and H.264.
      Establish each failure in a native import and verify that a container-only
      change fixes it. Check packet timing, timecode, reel, color and pixel aspect.
      Record original identity, path and hash in a repair receipt for later relink.
      Keep the original files in place and report supported recovery steps.

- [ ] **Pixel aspect from Avid** (AniMatte read-back 2026-10-20): Avid stored 256:135 on a
      1920x1080 raster and showed it square; Premiere honours the file's 16:15 and stretches the
      picture (and its masks) 1.09x. Model/1 has no pixel-aspect field (Codex reports it as a loss).
      Confirmed for this one clip: with PAR 1:1 the AniMatte matches Avid (IoU 0.993 vs 0.907).
      Codex (2026-10-21): no KB rule says what makes Avid display it square (spatial adapter,
      project format, source interpretation or descriptor precedence); BCOL-039/070 are
      counterexamples to a direct descriptor rule. So: keep the descriptor value, report the
      display as unresolved, no automatic override. Needs a controlled Avid adapter/project-format
      comparison before a rule.

- The 2026-10-20 Premiere Replace Footage return recorded the QuickTime FourCC,
  color-space label and CCFileModTime. The typed linked-original route now covers
  qualified saved locators and explicit path mappings. The bounded automatic proxy
  option is tracked below. Linked-original scope.

- [x] Opt-in **Prefer source media** for direct Avid → Premiere and the internal
      Avid → Resolve route. It tries the saved **Source File / Source Path** for
      picture-only 1080p24 or 23.976 ProRes HQ MOV, with the same exact source
      timecode and frame extent. If unavailable or outside the profile, report
      the reason and use the normal managed-media policy.
      [Configuration and scope](VERIFIED-RELINK.md#prefer-camera-originals-from-avid)
- [ ] Extend native camera-original validation beyond the synthetic picture-only
      fixtures. Premiere 27 passed at 23.976 and Resolve passed at 24 and 23.976.
      Exercise a real managed bin, additional codecs, and split audio replacement
      before broadening the profile.
- [x] Attach qualified picture-only DNxHD LB OP-Atom proxies and opt in to
      stereo PCM16 or PCM24 MOV rewrap for both destinations. Native reopen
      retained the attachments and trims. Both stereo timeline lanes rendered
      against the corresponding source samples with originals online.
      [Profile, native results and rewrap criteria](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [ ] Extend the attachment profile to wider channel layouts and other picture
      profiles. Evaluate 44.1 kHz separately from the current 48 kHz profile.
- [x] Verify independent Resolve audio links with the camera original offline.
      The attached 48 kHz PCM24 proxy supplied timeline picture and the two
      Avid OP-Atom MXFs supplied their respective mono lanes after Save Project
      As and reopen. The isolated lanes matched the online reference.
      [Offline control and sample-rate scope](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [x] Automatically bind the verified Avid mono siblings as independent
      Resolve sources when proxy rewrap is enabled. The converter-generated
      PCM24 project passed offline Save Project As, reopen and archive
      reimport for all 48 picture frames and both isolated lanes.
      [Automatic audio binding](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [x] Verify Resolve Deliver with the camera original offline. **Use proxy
      media** exported all 48 expected frames despite the queue warning.
      Both isolated 48 kHz PCM24 lanes matched their references.
      [Deliver verification](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [x] Verify one offline Resolve Deliver QuickTime with proxy picture and
      two discrete PCM24 mono streams. All 48 picture frames matched the
      reference. Both audio streams preserved the source trims and channels,
      with Resolve's short edge ramps accounted for.
      [Deliver verification](VERIFIED-RELINK.md#automatic-avid-proxy-attachment)
- [ ] Import option **"Link Audio to source file (if found)"**.
- [ ] MDB/PMR media-database folder input (from the KB's findings, not Filmcraft code).

### Offline output and relink

- [x] **Allow offline media** defaults off in Separate files, Grouped, and
      Merged. Saved jobs and the direct CLI carry the same choice. Review
      identifies offline items by bin and source locator or file package.
      Rewrap requires online sources. Review offers **Locate media** and
      **Switch to Separate files** when a required source is missing.
- [x] **Write offline projects and verify proposed files.** The initial
      managed profile uses saved 1080p23.976 DNx/PCM24 package IDs and TC1
      with two independent mono siblings. The UME profiles use a saved
      locator and matching 1080p24 H.264/AAC MP4 or 48 kHz PCM16/24 WAV
      descriptors. Available media passes the usual online checks. The
      sidecar and read-only command verify the proposed file before
      relinking. [Offline workflow](VERIFIED-RELINK.md#avid-offline-output)
- [x] **Offline reopen in Premiere 27.** The managed split edit retained
      both sequence starts, all 18 placements, and the three offline
      sources after Save As and reopen in Separate, Grouped, and Merged.
      The packaged macOS wizard converted and reopened a mixed Separate
      project with those three MXFs online and one UME master offline.
      A separate UME master also retained its offline locator after reopen.
- [ ] **Qualify Premiere 27 Link Media and playback.** In Separate mode,
      native Link Media left 8 of 11 bin items offline after Save As and
      reopen. Only 8 of 18 timeline placements pointed to the verified
      originals even when their filenames matched the offline placeholders.
      A diagnostic copy bound the three verified paths before opening and
      reopened with all items online. Both timelines matched the online
      controls at all 72 picture frames and all 144,144 PCM sample frames
      per mono lane. Keep manual relinking when the files cannot be resolved;
      qualify applying each verified path to all its references at once.
      Include the UME sequence, moved roots, and fresh insertion
      in all three modes. Qualify a same-name picture profile with saved
      physical reel and source timecode. The current offline verifier
      requires trusted checksums for duplicate original basenames.

## Explaining decisions (User, 2026-10-14)

- [x] Show in-app “Why?” guidance for conversion choices and report entries.
      [Decision reference](DECISIONS-EXPLAINED.md).
- [ ] Complete the side-by-side clip-construction comparison and add more specific
      explanations for report entries that still use general guidance.

## Audio

- [ ] Stereo edits with custom per-channel pans (e.g. 50L/25R): plain stereo + report today;
      decide on per-edit nested sequences after the User listens in Avid.
- [x] **Editable clip volume keys for direct same-rate mono edits** (CONV-0040).
      Read the PVOL volume control lane from inline CTRL points or the qualified
      subframe payload. Preserve the media-relative clock and write editable Premiere
      keys. Premiere read back both four-point controls using synthetic media.
- [ ] **Match PVOL rendered volume envelopes.** A reopened Premiere
      control on the verified dry WAV matched the flat Avid export exactly.
      Inline and subframe keys retained their times. Both keyed renders
      differed from Avid, including a −6.0 dB versus −6.1 dB plateau and
      different ramp shapes. Premiere 27 reproduced all three earlier
      Premiere PCM renders after Save As and reopen. Applying the measured
      plateau offset to the retained Premiere render improved the falling span but increased
      the rising-span error from 0.078 to 0.152 dB RMS. The mirrored −12 dB
      control retains its saved endpoints in both editors, with a measured
      change of slope in the final Avid ramp. Moving the last two keys
      leaves the break near sample 65,533. Moving the edit eight frames
      later changes the ramp, with breaks near three multiples of 32,768
      on the sequence sample clock. The prediction in CONV-0185 uses
      amplitude interpolation at those boundaries, downward 0.1 dB
      rounding, and dB interpolation between boundaries. It fits all six
      retained controls within 0.001 dB RMS. The 23.976 fixture stages
      five editable keys including one near the suspected boundary.
      Render both dry and processed controls before changing interpolation.
      [Mirrored ramp measurements](SPLIT-EDIT-AUDIO-2026-11-06.md#mirrored-pvol-volume-ramps)
      The historical managed-media
      conversion, additional layouts and nested owners remain open.
- [x] **Capture Avid mono pan automation** (CONV-0133). The source receipt
      retains control code 2000, saved interpolation kind, exact edit-local
      frame positions, and raw pan values. Both Premiere entry points report
      omitted automation on the affected placement. Resolve's internal
      linked writer treats the omission as a blocking source loss.
- [x] **Translate editable clip pan keys for linked mono WAV.** Separate
      files mode writes Linear keys for verified 48 kHz PCM24 mono WAV at
      1080p24. Premiere 27 preserved the keys and nonzero source trim after
      Save As and reopen. The whole-frame render agreed with the linked AAF
      within one PCM24 count per channel. Any NLE also kept the subframe key
      to the nearest Premiere tick. [Pan qualification](#edit-fidelity-and-metadata)
- [x] **Translate editable clip pan keys for verified managed mono MXF.**
      Separate files and Grouped keep the Linear pan keys on the verified
      1080p24 DNxHD LB and PCM16 profile. Premiere 27 reopened all four keys
      and the nonzero trim. The later direct-routing change aligns Grouped
      with Separate and Merged for this managed profile.
      [Managed mono pan](SPLIT-EDIT-AUDIO-2026-11-06.md#editable-managed-mono-clip-pan)
- [x] **Repair Merged direct linking and keep editable mono pan.** The
      verified 1080p24 DNxHD LB/PCM16 Merged case retains the media-state
      records for all three original MXFs. Premiere 27 reopened the dry
      and panned projects with the same exact ranges. All 196 picture frames
      match, and the panned A1 render matches Separate files sample for sample.
      [Merged reopen and render](SPLIT-EDIT-AUDIO-2026-11-06.md#24-fps-merged-media-state-and-clip-pan)
- [x] **Match Grouped audio level and verify fresh insertion.** On the
      verified 24 fps managed profile, Grouped routes the two original
      mono siblings independently, then applies clip pan on the timeline.
      After reopen, the Grouped picture and both audio lanes match Merged.
      Fresh insertion from either bin item keeps all 242 source frames
      on V1 and both mono lanes in sync.
      [Routing and insertion comparison](SPLIT-EDIT-AUDIO-2026-11-06.md#24-fps-grouped-routing-and-fresh-insertion)
- [ ] **Extend editable clip pan coverage.** Other managed profiles,
      stereo combiners in Merged, and the difference between the Avid
      and Premiere pan laws remain open.
- [ ] Master-clip pan (Premiere has no master-clip pan).

## Clip, marker and correction color

- [ ] **Color correction extensions**: keyframes and other Avid tabs (Channels, Levels,
      HSL Hue Offsets, Secondary). The measured HSL Controls (hue, saturation, brightness,
      contrast, gain, gamma, setup) and curves already feed the LUT writer; do not reimplement
      those as missing formulas. Decoding the retained Phase 52 10-bit 4:2:2
      source supports a Y′CbCr component limit near the physical code range
      before RGB clipping on the tested export path. Across the 12 flat
      patches, this reduces the maximum Gain 150 residual from 48.83 to 1.94
      8-bit-equivalent RGB codes. Contrast +30 improves from 19.18 to 2.66
      codes on the same patches. The hue sweep shows the same improvement.
      A later eight-patch export stayed inside the physical Y′CbCr limits
      and matched unclamped 1.5× gain within 1.06 RGB codes. Compare a
      rail-crossing chart in both Y′CbCr and RGB to locate the clipping and
      distinguish exact bounds before changing the LUT. The static 10-bit
      HSL Master matrix retained adjacent code values and measured default
      Clip Low/High endpoints of 64/940, fractional endpoints of 321/763,
      and a half-code case of 323/762. Clipping followed luma inversion
      and followed Gain in separate combined controls. Legal-range luma
      inversion gave 1004 minus the input code; chroma inversion and Hue
      180 reflected Cb and Cr around neutral within one code. Verify the
      remaining out-of-range behavior and an end-to-end destination
      pipeline before changing the LUT. Premiere 27's native Color Mode
      and Lumetri rendered the same production LUTs close to each other
      after Save As and reopen. Both retained the large Gain 150 and
      Contrast +30 errors against Avid. A follow-up 432-patch native
      chart measured the post-adjustment 4–1019 component bounds for
      both controls on the 10-bit Avid export path. Separate files now
      writes the 65³ extended Color Mode LUT for isolated Gain 150
      or Contrast +30 on qualified, explicitly tagged Rec.709 1080p24
      ProRes HQ. Both generated controls improved visible RGB after
      Premiere 27 Save As and reopen. Extreme Y′CbCr values still differ
      in Premiere's export, including with the ACEScct working-space preset.
      [Rail-crossing comparison and mapping](PREMIERE-27-COLOR-MODE.md#gain-and-contrast-on-a-rail-crossing-chart)
      [Source packing reference](https://pub.smpte.org/pub/rdd50/rdd50-2019.pdf#page=15).
- [ ] **Broaden the extended Rec.709 SDR Gain and Contrast mapping.** Test
      several additional Avid Gain and Contrast values in isolation and with
      both controls on the same clip. Use the rail-crossing chart to check
      clipping and processing order. Compare the extended LUT with Avid
      after Save As and reopen in Premiere 27. Broaden the mapping only for
      the cases that match, repeat the comparison, and keep the work local.
      [Current mapping and measurements](PREMIERE-27-COLOR-MODE.md#converter-mapping-for-isolated-gain-150-and-contrast-30).
- [ ] **Migrate remaining Premiere color output to native Color Mode.**
      New Premiere-bound color mappings target Color Mode. Qualify the
      modeled Avid correction and retimed source LUT migrations against
      the source renders after Premiere 27 Save As and reopen. Use the
      retained legacy outputs as comparison controls.
      [Translation target and test evidence](PREMIERE-27-COLOR-MODE.md#translation-target)

## Timeline and effects

The format research worklists are `knowledge/avid/docs/effects-codex.md` and
`knowledge/premiere/docs/effects-codex.md`. Use their property evidence and the
User's priority tiers to scope converter tasks.

- [x] **SubCap → Premiere captions**, bounded content/timing path (2026-11-08).
      Exact native identities, strict UTF8/LF, source occurrence ownership and qualified
      record spans feed embedded subtitle cues. Native synthetic accented/two-line controls
      survive Premiere save/close/reopen. Styling remains an explicit destination-default
      loss; this is not full-media playback or general Unicode qualification.
      [Scope and evidence](CAPTION-TEXT-TRANSLATION-2026-11-08.md).
- [x] **Retain local SubCap appearance evidence** (CONV-0058). Graph/2
      preserves explicit FontSize, FONTFACE and ExtraLeading carriers plus
      opaque synchronization presence. Unqualified appearance receipts report
      a reason while independently qualified caption text survives.
- [ ] **SubCap font-size and layout mapping.** The Avid 20 px case omitted the
      field. Native 40, 50 and 60 px saves stored their displayed integers.
      Premiere cue-local style overrides survived reopen with unchanged
      neighboring cues. The direct shared-font mapping missed the two-pixel
      full two-line block-height gate at 40 px, and line widths differed. Resolve font
      metrics, line spacing and synchronization before applying source styling.
- [x] **Current Titler+ static single-flow text → editable Premiere text** (2026-11-08).
      Character and native Flow/Text paragraph carriers are supported separately. Retain
      native appearance values by property-list role; do not use cached `_TTEXT` as text.
- [ ] **Title appearance.** A native static fixed-box horizontal move
      changed only its local position value and shifted the glyphs 318 px.
      A separate saved-carrier width probe preserved the left edge and wrapped
      two lines into four in Avid. The current Premiere writer renders all
      three as the same point text with reported ArialMT/48pt defaults.
      Qualify a Premiere paragraph box, shared font metrics and anchor
      before mapping fixed-box appearance. Multiple flows/layers, dynamic
      modes, mixed carriers, Classic Title and Titler+ Legacy remain unsupported.

- [ ] **Alpha (keyed graphics)** (User 2026-10-15; Avid side: Codex alpha-test intake, provisional,
      KB 5bba92c, AVB-0006/AVB-0012 context). Avid facts: an image imported with alpha becomes a
      Matte Key clip (composition usage 3, EFF2_2DMATTE): track 2 = fill, track 3 = matte, as
      separate MXFs; "Ignore" gives a plain clip. The saved source import
      settings (_IGNOREALPHA, _INVERTALPHA) predict polarity for these
      original imports, whose Matte Key parameters match. Measured on a straight-alpha
      PNG: "Invert on import (white = opaque)" -> opacity = a; "Do not invert (black = opaque)" ->
      opacity = 1 - a; ordinary straight-alpha blending (MAE ~0.3 codes).
      Graph/2 records omitted Matte Key occurrences by component identity,
      including known record range and track (CONV-0143).
      Remaining work and supported scope:
      1. Reader: resolve Matte Key clips (fill + matte file mobs, Source File, import alpha
         settings) instead of skipping usage-3 compositions; Matte Key edits in sequences.
      2. Preferred Premiere result: the original source file (via Source File, see Media
         linking) with Interpret Footage alpha set from Avid: Invert-on-import -> Premiere
         default; Do-not-invert -> Invert Alpha; Ignore -> Ignore Alpha (py_premiere writes
         these interpretation fields). A Premiere 27 control linked the original
         straight-alpha PNG and retained both polarities after reopen. Relative
         to the retained Avid black and white TIFF composites, mean absolute RGB
         error was 12.70–30.14 equivalent 8-bit codes with **Composite in Linear
         Color** enabled and 0.52–0.94 with it disabled. After Save As and
         reopen, including both foreground and background in a child
         sequence with this setting disabled reproduced all 48 decoded
         reference frames per polarity. A separate 24-frame parent edit
         kept its linear blend in both cases. When the child contained only
         the foreground, all 48 frames used the parent's linear blend over
         its background. Separate files conversion now recreates the
         verified one-frame PNG edits with both inputs in each child.
         An unavailable original or changed effect or framing blocks the
         verified composite from being written.
      3. Fallback when the original is missing: resolve the saved fill and
         matte representations by file-package identity. In the retained
         DNxUncompressed test, each matte has separate `y208` and `rl08`
         representations with opposite stored polarity. The `y208`
         luma spans codes 0–255. Qualify which representation a destination
         can use and how to apply its polarity and the saved effect controls.
      4. Wizard option: "Keyed graphics: link original file / rebuild from Avid matte".
      5. Direct CLI and saved-job renders of the eight-frame fixture match
         the verified Premiere prototype exactly. Relative to Avid's TIFFs,
         the mean absolute difference remains below one equivalent 8-bit RGB
         code per frame, with larger differences around the chart's stripes.
      Open: premultiplied sources, other codecs, alpha in video files (ProRes 4444 etc.).

- [x] **Saved Timewarp ramp cadence** (CONV-0051). The reader carries the
      retained two-key Linear rise/fall and Spline rise profiles as saved
      picture samples. The Premiere writer uses one editable remap key per frame.
      On the rewrapped DNx OP1a chart, Premiere 26.5.2 retained the remap keys,
      curve In/Out points and media links after Save As, close and reopen.
      All 144 individually exported frames in each sequence matched Avid's frame selections.
      The Spline rise retained all 35 differences from the Linear rise.
      Premiere 27.0.0 build 88 reopened the saved OP1a project containing
      both curves. All 288 rendered frame selections matched Avid, and
      all four editable remap records retained their times and ranges.
- [x] **Other durations of the two-key Linear Timewarp profile.** Accept a
      contiguous saved half-frame position map with the same explicit
      progressive render settings, a direct same-rate 24 fps source, neutral
      phase and offset, and verified source coverage. Two 36-frame fixtures
      use the visible samples from the retained native rising and falling
      ramps. Premiere 27 retained both editable remap curves after Save As and
      reopen. All 72 selected frames matched the corresponding Avid frames in
      two Premiere exports. The 36-frame AVBs are derived controls.
- [x] **Default Speed Boost and Speed Bump picture timing.** For the two
      verified 1666-frame 1080p23.976 profiles, evaluate the four saved Position
      keys and handles and write one Premiere Time Remapping key per frame.
      After Save As, close and reopen, Premiere 26.5.2 retained the editable
      keys and DNx OP1a link and played both sequences through to the end.
      All 3,332 frame selections from the Premiere renders matched the Avid
      exports at 24000/1001. This fixture qualifies picture cadence.
- [ ] **Extend Timewarp coverage.** Qualify other durations, modified presets
      and inherited Avid render settings. Resolve other anchors, editable and
      generated map authority, exact constant 200% cadence, trims, nonzero media
      offsets, mixed rates, blending, optical flow and sound separately.
      Constant full-speed reverse already uses Premiere Reverse Speed.
      In the retained large-project audit, 29 of 31 unsupported Timewarps are
      nested. Avid's Motion Effect Editor identified explicit mode 6 as
      FluidMotion (3 edits) and mode 7 as FluidMotion Draft (20 edits).
      Graph/2 retains their controls. One additional direct precompute
      resolves through a single-clip wrapper with a six-frame trim.
      A third precompute chain resolves through the outer Resize, which
      preserves the composite as a rendered clip. A derived control passed
      Premiere 27 reopen and export. Both project caches remain offline.
      For editable output, compare synthesized frames against a native
      control separately from the underlying retime.
- [x] **Traditional Freeze Frame, five-second 1080p24 profile** (CONV-0053). Preserve the
      generated ProRes HQ picture as a regular Premiere clip on the native
      media route. The converter checks the saved clip and sequence binding,
      copies all 120 coded pictures to MOV and excludes the verified zero-only
      packet beyond the picture span. Premiere 26.5.2 played the five-second
      sequence after Save As, close and reopen. All 120 rendered pictures
      matched the held Avid picture. The saved render can also supply a
      shorter edit from its start when the input resolves to the same
      master frame. A large-project witness has a 28-frame cut beneath a
      video-levels Color Adapter. The converter retains the freeze and
      reports the color omission. The witness's original cache is offline.
      Other render durations, field modes, and editable Timewarp holds
      need separate qualification.
- [x] **Saved AudioSuite Gain, one- and two-second mono PCM24 at 24 fps**
      (CONV-0055, CONV-0057).
      Native media preparation follows the saved Gain render binding and copies
      its 48,000 or 96,000 playable samples to WAV after verifying the two extra stored
      samples are zero. The saved render remains separate from PVOL volume and
      pan. Premiere 26.5.2 matched all 144,000 stereo sample frames of each
      three-second Avid Direct Out after Save As, close and reopen using the Avid default-pan
      option. Other render spans, channel layouts and missing caches remain
      outside this profile.
- [x] **Linear animated Resize position and scale** (CONV-0041). Convert two
      aligned whole-frame keys on 1920x1080 same-rate edits to editable Premiere
      Motion, including independent X/Y scale. Premiere rendered start, middle,
      and end geometry in the synthetic source control.
- [x] **Linear animated 3D Warp Position and Scale** (CONV-0046). Exact
      promoted control identities and neutral crop/background feed the existing
      editable Motion writer on an admitted 1080p23.976 picture edit. The
      synthetic DNx comparison matched the Resize output across all 48 frames
      and kept the keys after Premiere Save As and reopen.
- [x] **Extend uncropped two-key Motion to matching linked AVC/PCM rasters.**
      Premiere 27 preserved editable Resize keys at 1600×900/24 and
      1440×1080/23.976 and promoted Warp keys at 1440×1080/23.976.
      Sampled picture matched the projected bounds within one pixel after
      Save As and reopen, including both orders with Flop.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [x] **Linear animated Resize and 3D Warp crop** (CONV-0047). Translate four
      source-space crop curves alongside position and independent scale to
      editable Motion. The paired 1080p23.976 synthetic DNx renders matched
      each other across 48 frames before and after Save As and reopen. Measured
      edges stayed within one pixel of the retained Avid reference.
- [x] **Extend two-key animated crop to matching linked AVC/PCM rasters.**
      At 23.976, Premiere 27 preserved all four editable crop curves and both
      Flop orders at 1440×1080 and 3840×2160 after Save As and reopen.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [x] **Saved Linear 3D Warp keys after head or tail trim** (CONV-0048).
      Preserve the saved three-key profile, including independent Fixed Width
      keys outside one visible edge where the source media has the handle.
      Five Fixed/Elastic cases preserve their editable keys and render after
      Premiere Save As and reopen. Premiere's own editing rules govern later trims.
- [x] **Saved Linear Resize keys after head or tail trim** (CONV-0049).
      Preserve three saved Position and Scale keys, including Fixed and Elastic
      keys beyond one visible edge when the source media contains them. Both
      trim rounds and the baseline retain editable keys after Premiere Save As
      and reopen. All 288 frames preserve visible geometry within one pixel of
      the Avid reference on the synthetic DNx route.
- [x] **Saved Linear animated crop on three-key trims** (CONV-0050, CONV-0056).
      Preserve four editable Motion crop curves on a shared crop clock.
      Require same-rate progressive square-pixel 23.976, a neutral background,
      and source media coverage for every key. The 1080p native Avid head and
      tail trims for Resize and the compact 18-control promoted Warp retained
      exact bounds and three Elastic keys after Premiere Save As and reopen.
      Across all 144 active trim frames, visible edges over black were within
      one pixel of the Avid export. The two untrimmed controls each had two
      two-pixel left-edge differences at the lowest luma threshold.
- [x] **Extend three-key trims to matching linked AVC/PCM rasters.**
      Premiere 27 preserved the independent width and crop clocks on Resize
      and Warp head and tail trims at 1440×1080 and UHD. Missing handles
      produce an occurrence loss while the visible edits survive.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [ ] **Extend animated Resize and 3D Warp.** Qualify other rates, interpolation,
      compact Fixed clocks and additional key shapes separately. Track anchor,
      rotation and perspective in the Avid codex.
- [x] **Translate three-key Linear 3D Warp Z Rotation to Premiere**
      (CONV-0109). The compact 1080p24 profile preserves editable
      horizontal Position, independent Scale Width, and Z Rotation
      on a shared three-key clock. Premiere 27 Save As and reopen
      matched 91 Avid chart points across 13 frames within 0.12 pixels.
      The Linear rule also uses the exact frame clock at 23.976, 25, and
      29.97. One Premiere 27 project kept the three editable keys at two
      nonzero source in-points and matched all 156 comparison frames to
      the 23.976 controls after Save As and reopen. Keys outside the
      visible effect span remain unqualified.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#animated-3d-warp-z-rotation).
- [x] **Translate the compact three-key 3D Warp Shelf mode to Premiere Hold**
      (CONV-0110). Position X, Scale Width and Z Rotation keep three
      editable Hold keys. The reopened 13-frame chart holds each value
      until the next saved key and matches all 91 Avid fiducials within
      0.085 pixels. Mixed interpolation and other layouts remain open.
- [x] **Translate compact three-key 3D Warp Bezier and Spline to Premiere**
      (CONV-0111). The measured curves use zero value tangents and
      temporal handles of 40% of the adjacent key span. Premiere 27
      keeps the nine editable Bezier keys in each project. All 182
      chart points across 26 reopened frames agree with Avid within
      0.17 pixels. Other handle shapes and trimmed rotation remain open.
- [x] **Share the Warp rotation clock for Shelf, Bezier and Spline.**
      One Premiere 27 project tested 24, 23.976, 25 and 29.97 after Save
      As and reopen. All 260 comparison frames matched the corresponding
      23.976 controls. Shelf retained editable Hold keys. The qualified
      Bezier and Spline curves retained zero slopes and 40% handles.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#animated-3d-warp-z-rotation).
- [x] **Extend isolated 24 fps Warp roll to matching linked AVC/PCM rasters.**
      Premiere 27 preserved Linear, Shelf, and the qualified Bezier and
      Spline shapes at 1600×900 and 1440×1080. All reopened chart frames
      matched the projected point positions within 0.21 pixels.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#animated-3d-warp-z-rotation).
- [ ] **Projective 3D Warp geometry.** The installed renderer uses a
      separate perspective projection after the model transform, followed by
      Target translation and scaling. A destination mapping needs a
      source-space crop and projective transform with the same order.
      Qualify combined Target, Perspective and Axis on a numbered chart,
      plus a clipping case, before extending either destination.
- [x] **Translate bounded static 3D Warp pitch, yaw, and Perspective to Premiere**
      (CONV-0102). Four editable target points preserve the rendered geometry
      on the tested 1080p24 profile. All 41 visible chart points across six
      reopened frames agree with Avid within half a pixel. Original Rotation
      and Perspective controls remain in the source receipt.
- [x] **Translate combined static 3D Warp Rotation, Scale, Axis and Position**
      (CONV-0103). The default camera profile preserves the geometry as
      editable Premiere Corner Pin points. Six reopened 1080p24 frames match
      all 42 Avid chart points within 0.69 pixels.
- [x] **Translate bounded static 3D Warp Skew to Premiere** (CONV-0104).
      The tested 1080p24 profile covers X, Y, both axes together and X
      followed by Z Rotation. Four reopened frames match all 28 Avid
      chart points within 0.54 pixels. Skew combined with scale, tilt or
      custom Perspective remains open.
- [x] **Translate isolated static 3D Warp Position Z to Premiere**
      (CONV-0105). Positive and negative depth change the apparent size
      around the picture center with other controls neutral. Premiere
      preserves that geometry as editable Corner Pin points. All 14
      chart points across two reopened 1080p24 frames match Avid within
      0.51 pixels. The original Position Z remains in graph/2.
- [x] **Verify static 3D Warp crop with Z Rotation and nonuniform Scale**
      (CONV-0106). The existing editable Premiere Motion path keeps
      the source crop before geometry in the four retained 1080p24
      cases. Reopened output matched all 28 chart points within 0.10
      pixels. The converter no longer reports the verified neutral
      controls as losses for this compact profile.
- [x] **Capture static 3D Warp Corner Pin in graph/2** (CONV-0099).
      Preserve full-image target corners and independent source crop on
      the retained 1080p24 profile, even when media is unavailable.
      The graph coordinates reproduce 35 measured Avid fiducials within
      0.104 pixels. [Source facts](SOURCE-GRAPH-MIGRATION.md#static-avid-corner-pin)
- [x] **Write editable uncropped Corner Pin in Premiere** (CONV-0100).
      Isolated 1080p24 static pins retain all four editable points after
      Premiere 27 Save As and reopen. All 28 fiducials across four chart
      frames were within 0.2 pixels of the corresponding Avid renders.
- [x] **Write Corner Pin with static source crop in Premiere 27** (CONV-0101).
      The converter combines editable Rounded Crop with Corner Pin, preserving
      the full-image mapping and asymmetric crop. After Save As and reopen,
      all 16 sampled edge positions were within one pixel of Avid and all
      seven interior chart points were within 0.2 pixels.
- [x] **Preserve static Corner Pin in ordered Avid picture chains** (CONV-0107).
      Graph/2 retains the four targets, source crop, native parameters and
      whether Flop precedes or follows the pin.
- [x] **Write static Corner Pin with one Flop to Premiere** (CONV-0108).
      Both orders, with and without asymmetric source crop, keep editable
      controls after Premiere 27 Save As and reopen on the 1080p24 linked
      chart. The destination comparison passes; a native Avid render of
      the combined stack remains pending.
- [x] **Extend explicit Corner Pin targets to matching linked AVC/PCM rasters.**
      Premiere 27 retained the editable pin, source crop and both Flop
      orders at 1600×900 and 1440×1080 after Save As and reopen. The 1080p
      camera-derived projection has a separate geometry check.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#static-avid-corner-pin).
- [x] **Share static Corner Pin geometry across four Premiere clocks.**
      Explicit targets use the same matching-raster rule in preparation
      and writer validation at 24, 23.976, 25 and 29.97. A single Premiere
      27 project with 28 derived timelines retained the exact trims,
      points, crops and both Flop orders after Save As and reopen. All
      112 decoded frames and 32 repeat frames matched their respective
      controls. Camera-derived projection uses the 1080p24 profile.
- [x] **Share the uncropped Linear Motion trim clock in Premiere.**
      Two-key and trimmed Resize and 3D Warp retain their saved Linear
      position and independent scale at 24, 23.976, 25 and 29.97. The
      Premiere 27 project preserved off-cut keys with verified handles
      after Save As and reopen. All 1,056 decoded frames matched the
      corresponding 23.976 controls.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [x] **Use the same Premiere clocks for Linear crop.**
      Premiere 27 preserved editable Resize and 3D Warp crop on
      untrimmed, head-trimmed and tail-trimmed edits at 24, 23.976,
      25 and 29.97 after Save As and reopen. All 1,200 decoded comparison
      frames matched their 23.976 controls. Measured crop edges agreed
      with the saved curves within one pixel.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [ ] **Extend Corner Pin.** Qualify stacked pin and reflection with
      native Avid picture, then investigate Resolve writing, animation
      and tracking separately.
- [ ] **3D Warp compositing.** Qualify Full Key foreground opacity and
      background selection on contrasting picture layers. Then compare a
      plain border, soft crop edge, shadow, and the Internal and FG Only
      Defocus modes. The renderer sends these to different operations.
      Preserve separate key-mode identities and test the saved output
      before admitting an editable mapping.

- [x] **Single-shape AniMatte inversion and opacity in Premiere.** Key In
      and Key Out at full and half opacity survived Save As and reopen.
      The 72-frame synthetic comparison verified inside and outside pixels.
- [x] **Preserve six-shape AniMatte animation and approximate Acceleration 50.**
      The retained graph receipts carry six fixed-topology paths and four
      saved states. Premiere 27 Save As and reopen preserved six editable
      Opacity masks, including three animated paths. A white-source control
      matched the retained Avid masks at 99.43% mean overlap over 201 frames
      at half resolution. The same test with the second key moved earlier
      passed. The bounded Acceleration 50 mapper samples the saved contours
      once per frame and reports the approximation. Its render improved
      mean overlap from 98.19% to 99.43%. Native source-media relinking
      for these specific research bins remains open.
      [Conversion behavior](DECISIONS-EXPLAINED.md#animatte-becomes-editable-masks-on-opacity)
- [ ] **Match partial AniMatte compositing.** The native Avid single-shape
      Key Out 50% control attenuated the inside and preserved the outside.
      Premiere's single inverted mask attenuated the outside. A two-mask
      probe corrected which region is attenuated, and matching the measured
      midpoint also required changing the sequence-wide blend domain.
      Keep the current editable approximation scoped until a local solution
      is verified. Avid's shape renderer applies separate horizontal and
      vertical feather amounts before a Bias-dependent matte threshold.
      Compare an unequal feather pair and Bias 0, 50, and 100 on a native
      matte before mapping to a destination. Topology changes, multiple
      trackers and stabilization remain open.
- [ ] **Qualify full-opacity AniMatte Key Out with overlapping shapes.**
      Premiere 27 saved and reopened two ordered Opacity masks per edit.
      In the static, zero-feather control, an inverted first mask followed
      by a normal second mask with Blend Mode value 1 cut out both contours,
      including their overlap. The render had 144 frames across six controls.
      Compare with an Avid two-shape Key Out render at full opacity and zero
      feather before enabling that mapping. The retained two-shape Avid bin
      uses partial opacity and feather. Conversion reports the omission at
      the affected edit and keeps both contours in graph/2.
- [ ] **Luma Key and Chroma Key.** Avid's Luma Key Gain change moved a sharp
      cutoff on a 16-step grayscale ramp. Premiere reproduced those two
      sampled splits using paired editable Cutoff and Threshold controls.
      A later 10-bit Avid matrix matched the tested static hard, soft,
      colored-center and partial-opacity composites within one code.
      Show Alpha displays foreground rejection, blended over the lower
      layer at partial opacity. Qualify destination blend and layer
      ownership before mapping. The installed Avid palette exposed
      RGBKeyer and SpectraMatte. The exact Chroma Key entry still needs
      a native fixture.
- [ ] **SpectraMatte.** The standalone effect and 3D Warp's SpectraMatte
      mode share the core chroma and spill control IDs. The retained
      promotion saves the key center in different color coordinates.
      Before mapping to a destination keyer, compare a native hue sweep
      with Tolerance changed and a separate spill-only control. Measure
      foreground color and alpha separately.
- [ ] **Opacity** (Avid opacity/levels, fade-to-black etc.) -> Premiere Opacity (and blend modes
      where Avid has an equivalent); keyframes. Superimpose is available under
      Filters → Blend. The installed renderer reads a 0–100 foreground
      level and sends it to the mixer as a 0–1 factor. Verify the saved
      foreground and background roles and compare 0%, 50%, and 100% native
      renders before choosing a destination blend model.
- [ ] **Picture-in-Picture.** The installed Avid renderer shares transform
      and crop evaluators with Resize. Picture-in-Picture registers a second
      picture input and reads a separate Foreground Level. Qualify a saved
      two-layer effect with nonneutral position, crop and opacity before
      mapping its foreground controls and background ownership.
- [ ] **Avid Pan & Zoom.** The effect references an external still and
      calculates a source viewport from its raster, aspect, zoom, and
      position. The placeholder supplies the Video background. Qualify
      the saved path and a static target render first. Position follows
      a separate velocity and path calculation, so qualify animation
      with uneven key spacing before writing editable destination keys.
- [ ] **Shape Blur and Mosaic.** The installed Paint renderer scales the
      shape's horizontal and vertical strengths separately. Blur uses a
      filtered image, while Mosaic samples and repeats a source pixel per
      tile. Test an offset rectangle over an asymmetric chart to verify
      native shape ownership, tile phase, and the visible blur kernel before
      mapping the standalone effects or Paint modes.
- [ ] **Color Effect style.** Posterize and Solarize have separate control
      identities and lookup tables. The traced renderer applies Solarize
      before Posterize when both are active. Qualify saved controls and
      compare an 8-bit ramp and a 10-bit ramp before choosing a mapping.
- [x] **Ordinary two-sided video Dissolve** (CONV-0040). Same-rate cuts with
      qualified source handles become editable Premiere Cross Dissolve (Legacy)
      transitions at the original window and cut offset. A synthetic asymmetric
      control rendered in Premiere and kept both endpoint references on save.
- [ ] **Film Dissolve.** The saved foreground level curve can match an
      ordinary Dissolve. Avid's Film Dissolve renderer remaps the evaluated
      level through an 11-point curve before mixing. Compare a matched native
      render pair to qualify the frame sampling and blend space, then check
      whether the destination can express the curve. Linked conversion keeps
      the exact cut at a verified filler edge and reports the omitted blend.
- [ ] **Film Fade.** Saved Avid occurrences identify `EFF2_BLEND_DISSOLVE_LIN`.
      The installed renderer uses a distinct 11-point opacity curve. At 50%
      Foreground Level it sends 20.24% opacity to the mixer. Qualify fade-in
      and fade-out against matched black and picture references, measuring
      frame sampling and blend space before choosing a destination mapping.
      Linked conversion keeps the exact cut at a verified filler edge and
      reports the omitted blend.
- [ ] **Dip to Color.** The Avid renderer selects one picture input on each
      side of 50% Level and reaches full color blend at 50%. Its Background
      Color path uses the saved luminance as HSV Value. Verify a nonblack
      native save and render, including source order, color and transition
      sampling, before choosing a destination mapping.
- [ ] **Right to Left Push.** Native Avid occurrences label
      `EFF2_BLEND_PUSH_RIGHT` Right to Left. The installed renderer uses the
      saved foreground level to move both pictures left, with the incoming
      picture entering from the right. Verify frame sampling, border behavior
      and a matching destination render before admitting it to conversion.
- [ ] **Horizontal Edge Wipe.** Saved Avid transitions identify
      `EFF2_BLEND_WIPE_HORZ` and retain Foreground Level keys at −1 and 24 in
      a 24-frame window. Both installed renderer architectures dispatch
      SMPTE pattern 1, a left-to-right reveal. Compare a neutral-border
      native render and one width and softness variant to establish exact
      seam sampling and border behavior before mapping it to a destination.
- [ ] **Video transition fidelity and audio fade curves.** Compare ordinary
      Dissolve with a native Avid render and qualify additional video identities
      and control values. Other qualified video transitions retain the cut approximation. The native
      Avid reference now demonstrates a bounded mismatch with the current mapping;
      near-linear evidence is not an exact interpolation rule. Qualify a same-window
      Constant Gain comparison before considering a new default.
      [Measurements and setup limit](MEDIA-QUALIFICATION-2026-11-07.md#native-avid-audio-reference).
- [x] **Flip, Flop, and Flip-Flop** (CONV-0041). Exact Avid IDs map to editable
      Vertical Flip, Horizontal Flip, or both. All three reflected synthetic
      Premiere frames match the expected pixels. A native Avid Warp outside
      Flop control retained both effects and left the asymmetric crop on
      its original side after reflection of the source.
- [x] **Share picture reflection in graph/2.** Avid and Resolve emit one
      axis-based effect, including the Flop stage inside an ordered chain.
      The native Avid effect ID stays with the source receipt.
      [Representation and scope](SOURCE-GRAPH-MIGRATION.md#picture-reflection).
- [x] **Read simple Avid picture chains into graph/2** (CONV-0065).
      Preserve the input-to-output order for qualified full-span
      Flip, Flop, Resize, and 3D Warp stages. The retained Avid
      Flop-inside-Warp test keeps both effects and Warp's animated crop.
      [Source scope](SOURCE-GRAPH-MIGRATION.md#nested-avid-picture-effects).
- [x] **Write bounded ordered picture chains to Premiere and Resolve**
      (CONV-0066). Reflections can occur on either side of one supported
      Resize or 3D Warp stage. Native Save As and reopen verified Flop
      before animated Warp in both editors using the retained Avid control.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [x] **Use matching verified linked AVC/PCM rasters for Premiere static chains.**
      Premiere 27 Save As and reopen preserved both Flop/Warp orders
      at 1600×900 and 1440×1080. Sampled picture distinguished the crop
      order.
- [ ] **Expand destination picture chains.** Qualify a construction that
      can apply multiple Motion stages or other effects in saved order.

- [ ] Avid Start shown on clips whose MXF timecode is 0 (Alternate Timecode, User decision).
- [x] **Editable two-camera Groups over native OP1a** (User, 2026-11-08).
      Graph/2 retains angle layouts, offsets, gaps and
      independent selectors. The writer converts continuous 1080p23.976 DNx camera
      sources with two fixed mono PCM24 lanes. Generated output passed scoped native
      insertion, angle switching, playback and picture/PCM checks. Explicit inner
      channel routing removes the measured 3 dB level loss.
      [Implementation and native checks](EDITABLE-GROUPS-2026-11-08.md).
- [ ] **Widen editable Group qualification.** User acceptance, native Avid
      independent audio comparison, missing or wider audio, mixed rates, discontinuous MultiGroups
      and exact record-start clocks remain open.
      The saved fixed-audio control still matches its pixel and PCM references.
      The Separate files route writes a verified alternate selection as a
      direct mono edit. Two graph-authored A1 switches matched both flat PCM
      lanes after Premiere 27 reopen, with editable picture and A2 unchanged.
      Qualify a saved same-rate Avid selector against native Avid picture
      and PCM, and determine how a Premiere Group can represent alternate
      audio. Use the retained linked AAF as a readiness benchmark.
      [Source contract and acceptance plan](MULTICAM-READINESS-2026-11-08.md).

- [x] **Write reusable picture-only Premiere subclips.** The Separate
      files route admits one direct same-rate V1 reference with a
      qualified 23.976 DNxHR LB parent and validated file-relative bounds.
      Premiere 27 Save As, reopen and fresh insertion preserved two
      same-named items with different ranges. All 20 decoded frames
      matched Premiere's authored control.
      [Subclip source facts](SOURCE-GRAPH-MIGRATION.md#saved-avid-subclip-ranges)
- [x] **Write reusable linked A/V subclips.** Separate files preserves the
      verified full-range 23.976 managed DNxHD LB/PCM16 item with V1 and
      two independent mono lanes. Premiere 27 kept the subclip as the
      timeline source and matched linked AAF after fresh insertion.
- [x] **Preserve shortened linked A/V subclips.** A same-rate V1/A1/A2
      item keeps its separate bin bounds, file-relative timeline trim,
      and reusable source reference. Fresh insertion after Save As and
      reopen matched the 24-frame linked AAF control.
- [x] **Preserve reusable A/V subclips with DNxHR LB and mono PCM24.**
      The verified online 23.976 Separate files route preserves playback,
      the bin range, and fresh insertion after reopen with both original
      mono siblings. The linked AAF timeline matches within one PCM24
      count per output channel under the same pan policy.
- [x] **Reopen exact-24 A/V subclips in Premiere 27.** Both entry points
      write one reusable V1/A1/A2 master for the derived zero-origin DNxHD
      LB and PCM16 control. Offline read-back covers the mixed mono and
      stereo timeline. Premiere 27 Save As and reopen of the isolated
      Avid-authored control keeps its reusable [76,163) file range on V1,
      A1 and A2. Fresh insertion matches all 87 decoded picture frames
      and both 174,000-sample lanes of the editorial render. Nonzero
      backing offsets and other media profiles remain separate cases.
- [x] **Reopen 29.97 DF A/V subclips in Premiere 27.** Separate files writes
      one reusable V1/A1/A2 master with verified direct DNxHD LB and PCM24
      siblings. Premiere 27 Save As and reopen preserves both clocks,
      the shared subclip, and the six saved cuts of the reopened linked
      AAF control. The timeline matches all 15 decoded AAF picture
      frames and both 24,024-sample source ranges within one PCM24 count
      under center pan. Fresh insertion keeps the separate [42,58)
      file bounds and both mono lanes. The isolated WAVs add one
      trailing zero after the 25,625 matching samples on each lane.
      (CONV-0171)

## Avid reader and writer gates

The [Avid document API](../avid-reader/src/document.rs) provides the
bounded AVB construction primitives. Optional property variants,
retiming semantics, and malformed-input controls remain under test.

- [ ] **Qualify one native linked AVB output profile.** Keep construction and
      runtime qualification with the [Premiere → Avid](#premiere--avid) and
      [Resolve → Avid](#resolve--avid) route gates. Prove the current link
      provider, source clocks, mono channel selection and native Save As
      before widening the supported authoring surface.

## More programs: the neutral graph (ADR-0004, User 2026-10-26)

- [x] Direct Avid source extraction into graph/2 and explicit Premiere preparation.
      Folder imports preserve destination bin hierarchy through the shared job path.
      Graph/1 and model/1 remain compatibility interfaces.
      [Current architecture](SOURCE-GRAPH-MIGRATION.md).
- [x] Preserve exact Avid effect parameter receipts and their placement links,
      including controls on events without a usable media route. Preserve
      Premiere caption content and timing as shared text facts, with per-cue
      Resolve omissions reported during destination review.
- [ ] Promote color correction and AniMatte to neutral effects when their
      cross-editor semantics are qualified. Track subclips and nests separately
      from the editable Group scope above.

### Adobe After Effects (User, 2026-11-09)

- [ ] **Translate to and from After Effects.** Plan both a source adapter and a
      destination writer through the neutral graph. Start with a native project
      containing one composition, linked footage and a trimmed layer. Establish
      layer timing, frame rate, transform and media identity before choosing the
      first editor pair to qualify. Add After Effects to the direction menus
      when a route is ready to use.
- [x] **EffectCraft AEP reference intake.** Reviewed the pinned format,
      importer and exporter notes from PR #1869. A bounded binary inspector
      checked exact composition and layer clocks, project item identities
      and saved footage paths in nine retained binary fixtures. Five paired
      AEP and AEPX saves produce the same inspected records. One-change saves
      also verify explicit static 2D AV transforms and their stored units.
      [Scope and next native checks](AFTER-EFFECTS-REFERENCE.md)
- [ ] Qualify the first After Effects graph adapter with a native linked
      footage save. Test source identity and placement independently, then
      add static transform controls before animated properties.

## DaVinci Resolve (User 2026-10-26: bookmark and research)

**Direction decided by the User (2026-10-27): Avid -> Resolve goes straight to Resolve's native
project (`.drp`), read and written by Any NLE, through the graph. FCPXML is a separate, optional
export, never the route to Resolve: Avid can already export interchange files itself, and the
point of Any NLE is to go further than they do.** Route (1) below is the target; route (2) is
only for comparing results; route (3) (scripting API) is for test read-back.

- [x] **Resolve format authority and shared effects audit.** Native archives and
      findings have a versioned catalog under the workspace Resolve knowledgebase.
      The [cross-editor audit](SHARED-EFFECTS-AUDIT.md) traces shared and source-specific
      semantics without changing effect behavior.
- [x] **Qualify the internal straight-cut writer.** A standalone graph/2 writer
      emits the bounded 1080p23.976 split edit with exact TC1, one linked
      DNxHR OP1a item, and two PCM24 mono lanes. Native reopen, re-export and
      render comparisons passed for the baseline, origin change and source trim.
      [Profile, evidence and limits](RESOLVE-PROJECT-CORE.md).
- [x] **Write the bounded editable Group.** The saved A/B/A sequence
      reopened in Resolve Studio 21.2 with exact cuts and both mono lanes
      fixed to camera A. A fresh insertion and distinct-signal tests passed.
      [Scope and audio-envelope comparison](RESOLVE-EDITABLE-GROUPS.md).
- [x] **Read the bounded Resolve project into graph/2.** The internal reader
      covers registered timelines, cached linked media, exact clocks and
      straight cuts, fixed-audio Groups, sequence markers, static X
      position, bounded static mono clip volume, two-point source-time volume
      curves and source receipts for default clip fades. [Source profile and limits](RESOLVE-SOURCE-READER.md).
- [x] **Write the bounded Resolve clip envelopes.** The internal writer
      recreates bounded static clip volume, default fades, and two-point
      linear-dB volume automation on qualified direct mono edits.
      [Writer profile and limits](RESOLVE-PROJECT-CORE.md).
- [x] **Read and write bounded static Resolve track faders.** The qualified
      two-mono mixer preserves track level independently of clip volume.
      Both lanes matched native PCM after Save As and reopen.
- [x] **Read and write the bounded Resolve main bus fader.** The qualified
      stereo bus preserves its level independently of track and clip gain.
      Both mono lanes matched native PCM after Save As and reopen.
- [x] **Read and write bounded Resolve track mute.** The qualified two-mono
      mixer and timeline flags agree after Save As and reopen. The muted
      lane is silent, the enabled lane matches its reference, and all gain
      settings are preserved. [Scope and evidence](RESOLVE-SOURCE-READER.md).
- [ ] **Qualify the linked Resolve project foundation.** The internal project
      writer handles ordered timeline selection, nested media bins and shared
      original MOV/WAV bindings. Fresh stereo PCM16 and PCM24 insertions
      matched both reopened reference lanes. Complete the retained MOV/WAV
      [Save Project As checks](RESOLVE-PROJECT-CORE.md#pending-editor-checks).
      The [mixed project with explicit bindings](RESOLVE-PROJECT-CORE.md#combined-grading-turnover)
      is the comparison for the [automatic Avid turnover](#avid--resolve).
- [ ] **Check Resolve's visible bin order and active timeline on reopen.**
      Use the saved-order and selection controls with duplicate names.
      Check which sibling bins Resolve displays first and which timeline
      opens after reordering or omitting the previously active timeline.
      Compare with the [archive read-back](RESOLVE-PROJECT-CORE.md#linked-project-profile)
      and [saved selection](RESOLVE-PROJECT-CORE.md#saved-active-timeline).
- [x] **Read and write bounded static Resolve picture controls.** Same-rate
      1080p position, independent scale, and four crop edges matched eight
      reopened native controls. See the [static profile](RESOLVE-PROJECT-CORE.md#static-picture-controls).
- [x] **Translate isolated static Avid 3D Warp Z Rotation to Resolve**
      (CONV-0112). The 1080p24 linked control retains editable centered
      angles through save and archive reopen. Three decoded frames
      match native Resolve and 21 chart points match Avid within 0.09 px.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#centered-static-z-rotation).
- [x] **Translate centered static Avid 3D Warp Crop, Rotation and Scale to Resolve**
      (CONV-0113). Four 1080p24 edits retain all four crop edges,
      independent Zoom, and editable Z Rotation after archive reopen.
      All four frames match native Resolve, with at least 99.70% crop
      mask overlap against Avid.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#centered-static-crop-with-z-rotation-and-scale).
- [x] **Translate isolated Avid 3D Warp Axis X or Y with static Z Rotation to Resolve**
      (CONV-0114). The editable Anchor Point preserves the pivot while
      Pan and Tilt remain zero. All three 1080p24 frames match native
      Resolve after archive reopen. All 20 visible chart points match
      Avid within 0.08 px.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#off-center-static-z-rotation).
- [x] **Translate isolated Flip, Flop, and Flip-Flop in the linked Resolve
      writer.** The 1080p24 Avid UME control preserved all three editable
      axis choices after Save Project As and reopen. All 144 decoded frames
      matched a native Resolve control. Picture effect stacks need separate
      qualification. [Scope and evidence](RESOLVE-PROJECT-CORE.md#flip-flop-and-flip-flop).
- [x] **Translate two-key Linear Resize and 3D Warp to Resolve.** The
      retained 48-frame Avid pair keeps editable position and independent
      zoom keys after Save Project As and reopen. All decoded Resize and
      Warp frames match each other. Sampled chart geometry agrees with
      Avid within two pixels horizontally and one vertically.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#linear-animated-resize-and-3d-warp).
- [x] **Translate three-key Fixed and Elastic Resize and 3D Warp to
      Resolve.** Four editable curves retain their saved clocks, including
      keys outside the visible edit with verified source media handles.
      Save Project As and reopen preserved all 468 decoded frames across
      the 14 retained cases. [Scope and evidence](RESOLVE-PROJECT-CORE.md#three-key-linear-resize-and-3d-warp-after-trimming).
- [x] **Translate three-key Elastic Resize and 3D Warp crop to Resolve**
      (CONV-0064). Four editable source-space crop curves run before Position
      and Zoom. Reopened control and trim renders preserve the edit ranges,
      keys, and edges within one pixel of Avid at the primary threshold.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#three-key-linear-resize-and-3d-warp-with-animated-crop).
- [x] **Translate three-key Linear 3D Warp Z Rotation to Resolve**
      (CONV-0115). The compact 1080p24 profile keeps Position X, Scale
      Width and Rotation Angle editable. All nine Linear keys survive
      archive reopen and all 91 chart points across 13 frames agree
      with Avid within 0.22 px.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#three-key-linear-3d-warp-with-z-rotation).
- [ ] **Extend Resolve crop beyond the shared Elastic grid.** Qualify
      Fixed Width or separate crop clocks and mixed effect stacks with
      distinct timing and order controls.
- [x] **Translate the bounded Avid UME profile internally.** The Avid
      adapter resolves and verifies originals through graph/2. Resolve
      links qualified ProRes HQ, PCM WAV and 1080p24 AVC in MOV and MP4.
      The AVC audio profile preserves two independent mono selections
      from embedded stereo AAC. See the [UME profile](RESOLVE-PROJECT-CORE.md#avid-ume-originals).
- [ ] **Widen Avid UME turnover coverage.** Qualify representative production
      bins, other H.264 profiles and rates, and combined automatic binding
      of managed Avid media and UME originals.
- [x] **Qualify an eight-frame left-aligned Resolve Cross Dissolve.**
      The internal Resolve → graph/2 → DRP path preserved the editable
      transition after Save As and reopen. Picture matched the native
      dissolve and both isolated PCM lanes matched the straight cut.
      [Native trial](RESOLVE-DRP-FORMAT-CLOSEOUT.md#qualified-left-aligned-cross-dissolve)
- [ ] Qualify native Avid dissolve endpoint and handle mapping. Use
      saved source controls and native Resolve renders. The two qualified
      Premiere 27 alignments are in the
      [Premiere → Resolve checklist](#premiere--resolve).
- [x] **Implement the internal link-in-place turnovers.** One preparation and
      publication path handles automatic Avid bindings and the bounded Premiere
      ProRes HQ MOV/PCM WAV source. The synthetic Premiere control matched the
      fresh native Resolve edit in picture and isolated PCM.
      [Inputs and scope](RESOLVE-TURNOVER-IMPLEMENTATION.md).
- [x] **Capture additional Resolve source fields in graph/2.** Retain
      transitions, timing maps, Fairlight controls, custom fade points,
      timeline marks, and clip color codes with their saved owners.
      [Capture status](RESOLVE-TURNOVER-IMPLEMENTATION.md#capture-in-graph2).
- [x] Preserve matching audio and video timeline In/Out marks, including
      one-sided selections, in linked Resolve projects. Frame-zero In
      survives Edit-page import. Keep independent
      audio/video selections read-only because fresh import changed them.
- [x] Preserve all 16 Resolve sequence marker colors and marker custom data.
      Verify the marked sequence after reopen.
- [x] Preserve Resolve track names and edit locks on the qualified
      V1/A1/A2 profile. Verify the saved audio mixer mirrors after reopening.
- [x] Preserve disabled V1 separately from the edit lock and verify black
      picture with both audio channels preserved after reopening.
- [ ] **Qualify editable 24 ↔ 23.976 adaptation in Resolve.** Native default
      picture cadence differs from the retained Avid phase controls. Author a
      one-change editable timing control per direction, then verify the full
      numbered-frame sequence and the independent 48 kHz first sample and tail.
      See [pending editor checks](RESOLVE-PROJECT-CORE.md#pending-editor-checks).
- [x] **Compare direct Avid OP-Atom playback in Resolve.** Resolve Studio
      21.2.0.0006 reopened and played the tested DNxHR LB with two mono PCM24
      siblings and the picture-only ProRes LT fixture. The DNx pictures and both
      isolated PCM lanes matched the OP1a control. The ProRes render covered all
      446 pictures with small pixel differences from the MOV control. See
      RESOLVE-0014 in the local Resolve knowledgebase.
- [x] **Qualify the internal direct DNxHR LB OP-Atom writer.** One graph/2
      split edit links the original picture and two verified mono PCM24
      siblings. Resolve Studio 21.2.0.0006 preserved both routes and all nine
      edits after Save Project As and reopen. See the [direct-link profile](RESOLVE-PROJECT-CORE.md#direct-avid-op-atom).
- [x] **Read the qualified direct DNxHR LB OP-Atom Resolve archive.** The
      internal reader preserves both mono cache locators, selectors and
      package identities from the reopened project. See the
      [reader profile](RESOLVE-SOURCE-READER.md).
- [x] **Bind direct DNxHR LB OP-Atom automatically from an Avid bin.** The
      internal Avid probe resolves native media from supplied roots and links
      both mono siblings by saved track and file package. The split edit
      retained both assignments after Save Project As and reopen.
- [x] **Bind multiple direct DNxHR LB OP-Atom clips in one timeline.** The
      internal Avid probe resolves each clip's own picture and mono siblings.
      The two-clip test retained all six paths and all nine edits after Resolve
      Save Project As and reopen. Separate signal renders confirmed each mono
      route. See the [direct-link profile](RESOLVE-PROJECT-CORE.md#direct-avid-op-atom).
- [ ] **Extend direct Avid OP-Atom beyond the core profile.** Qualify more
      media and timeline layouts before product exposure.
- **Older Resolve archive reference.** A private version 15 archive provides a
  hierarchy, identity and locator comparison. Current source and writer work
  targets the version 17 profile. See [editor-version compatibility](#editor-version-compatibility).
- [ ] **Widen Resolve source and destination evidence.** Qualify other media,
      mixed-rate clocks, portable envelope
      rendering, other transforms, and independent Group audio before extending support.

### Resolve product exposure

- [ ] **Qualify each route for the public CLI and wizard.** Complete its
      native acceptance and verify the media, project structure, and editor
      versions in the intended release scope. Review the same-build template
      for redistribution. Exercise Review, CLI conversion, and saved-job
      replay from the packaged build before enabling the route. Publish the
      tested profile and its limitations. Resolve conversion remains internal.

Premiere → Resolve and Avid → Resolve target editable project conversion.
Grading turnovers use **Link to originals** as the preferred media menu
default. Preserve saved camera-original locators, source timecode and reel
or file identity when qualified, and diagnose unresolved relinks. Offer
rewrap as a separate choice for a verified workflow that needs it, with
the **Creates new media.** notice.

- [x] Start the [Premiere and Resolve metadata inventory and crosswalk](PREMIERE-RESOLVE-METADATA.md).
      Capture Premiere logging and physical identity receipts and the Resolve
      Media Pool reel-cache observation in graph/2.
- [x] Add [verified MOV relink for existing Premiere 27 and Resolve projects](VERIFIED-RELINK.md).
      Explicit saved paths and source filenames drive the move. Duplicate names
      use embedded reel and source timecode, with optional pre-move checksums.
- [x] [Verify Resolve conform after a media move](RESOLVE-SOURCE-READER.md#verify-conform-after-a-media-move).
      Both move verification modes matched the reopened same-name DRP
      at the sampled cut. The checks cover relocated paths, saved source
      frames, reel and timecode, and matching supplied renders.
- [ ] Qualify metadata and conform in both directions. Prioritize nonzero source
      timecode, explicit reel ownership, moved originals, duplicate basenames,
      and physical audio selectors before destination logging. Widen the column
      census from native exports and verify each destination field after reopen.
- [x] Preserve saved Premiere 27 Motion position, scale and mono clip volume
      in the turnover graph (CONV-0068). Keep native ticks, interpolation
      codes and auxiliary fields on the owning edit. Verified with a
      synthetic version 46 turnover and retained saved-record evidence.
      [Graph capture](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-clip-components)
- [x] Qualify the static and two-key Linear Motion controls and a static
      −6 dB mono clip-volume control using a Premiere 27 save and a
      reopened Resolve prototype. [Native trial](RESOLVE-TURNOVER-IMPLEMENTATION.md#native-premiere-27-and-resolve-mapping-trial)
- [x] Apply qualified Motion and static mono clip volume during
      Premiere-to-Resolve turnover. Verify both after Save As and reopen.
      [Native trial](RESOLVE-TURNOVER-IMPLEMENTATION.md#native-premiere-27-and-resolve-mapping-trial)
- [x] Translate three aligned Linear Position X, Scale Width, and Rotation
      curves from Premiere to Resolve (CONV-0118). All nine editable keys
      survived archive reopen at 1080p24. All 13 decoded frames matched
      the earlier Avid-to-Resolve control.
      [Scope and checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-linear-rotation-with-independent-width)
- [x] Translate bounded Premiere static crop, scale, and centered
      rotation to Resolve (CONV-0119). All four 1080p24 edits retain
      editable controls after archive reopen and match the earlier
      Avid-to-Resolve render.
      [Scope and checks](RESOLVE-TURNOVER-IMPLEMENTATION.md#premiere-centered-static-crop-scale-and-rotation)

### Completed reference reviews and their scope

- [x] **diop/davinci-resolve-api timeline read-back** (`a75fd3d`). Reviewed
      the community API guide against the installed Resolve 21.2 scripting
      reference. The internal native probe reads the current project's
      timeline ranges and track states. Two saved controls agree with the
      archive reader. [Native timeline read-back](RESOLVE-SOURCE-READER.md#read-back-a-timeline-in-resolve)

- [x] **drp-extractor archive intake** (`b43a358`). The upstream prototype
      locates `project.xml`. The internal archive inspector now reports
      version, timeline registrations and member inventory before graph
      admission. Verified retained version 15 and 17 projects and a
      standalone DRT. [Archive inspection](RESOLVE-SOURCE-READER.md#inspect-archive-and-version-metadata)

- [x] **Samuel Gursky Resolve MCP offline conform verification** (`1e2bf034`).
      The internal probe compares saved V1 source frames around cuts and
      can check the corresponding frames in supplied renders. The retained
      fixtures verified exact trims, dissolve exclusion, and independent
      mapping and picture results. Cut sampling also supports saved
      half speed and reverse at 24 fps and explicit graph/2 frame maps.
      The optional full-frame scan detects cadence changes between cuts
      and covers single-edit timelines. Qualified one-level Resolve
      picture nests contribute child cuts and file-relative frame selections.
      Qualified V1 multicam edits contribute selected camera frames and
      saved angle gaps.
      Database repair, automated relinking, grade transfer, and additional
      retiming profiles are separate research leads.
      [Conform checks](RESOLVE-SOURCE-READER.md#check-conform-at-v1-cuts)

- [x] **davinci-project-repair saved timeline selection** (`cbfcf62`).
      The DRP reader retains the active sequence UUID and the linked writer
      remaps it after timeline reordering. Offline checks covered duplicate
      names, distinct bin assignments, and the first-timeline fallback.
      Resolve UI reopen remains pending. Database repair stays outside the
      converter. [Reader and archive checks](RESOLVE-SOURCE-READER.md#saved-active-timeline)
      · [Pinned upstream notes](https://github.com/LucaL1fe/davinci-project-repair/blob/cbfcf6218d00a3c5a3f8b13bd90361affdb1e7ae/README.md#the-root-cause)
- [x] **sqsolve archive and bin-order review** (`c47316e`). The Resolve
      reader carries the saved sibling UUID order through graph/2. The
      linked writer verifies that order in the written child lists and
      numbered paths. An offline round trip preserved two same-named
      nested bins and their distinct timeline assignments after an
      ordinal swap. [Reader behavior](RESOLVE-SOURCE-READER.md)
      · [Writer and verification](RESOLVE-PROJECT-CORE.md#linked-project-profile)
      · [Pinned sqsolve exporter](https://github.com/donutinit/sqsolve/blob/c47316e87e603c70b41316900c5da497967a97db/internal/drp/export.go#L222)
- [x] **DaVinci Resolve DRP Toolkit** (`aadefac4`). The existing timeline
      diff covers the comparison lead. The internal probe inventories
      saved timeline effects with exact selectors for byte comparison
      and retains payload facts when graph interpretation fails. The
      retained DRT and DRP dissolve selectors produced a matching blob
      comparison. Effect stripping by size and Dolby Vision tag clearing
      remain upstream reference leads. [Inventory and checks](RESOLVE-SOURCE-READER.md#inventory-saved-effects)
      · [Pinned rescue code](https://github.com/koosoli/DaVinci-Resolve-DRP-Toolkit/blob/aadefac4810de59249b8a1980be459b136d5f412/drp_toolkit/plugins/rescue.py)
      · [Pinned downgrade code](https://github.com/koosoli/DaVinci-Resolve-DRP-Toolkit/blob/aadefac4810de59249b8a1980be459b136d5f412/drp_toolkit/plugins/downgrade.py)
- [x] **drpEditor comparison workflow** (`c5bec571`). The internal
      Resolve probe compares selected DRP and qualified DRT timelines and
      reports saved blob byte ranges by exact object ID. The retained
      cross-format pair matched on timeline content despite rewritten IDs
      and a changed envelope. The straight-cut control isolated one added
      dissolve. The comparison also checks track names, locks, V1
      output, and separate audio and video In/Out marks. It distinguishes
      a dropped frame-zero In from unchanged edit bounds.
      It also checks saved speed, plain subtitle cues, and clip colors
      on their paired edit occurrences. For V1 with two mono lanes,
      it checks saved track gain, track mute, and main bus gain
      independently of clip volume. It also compares qualified static
      track pan and bypass, direct bus assignment, and Bus 2 gain and
      mute. Supported static Fairlight EQ and dynamics are compared per
      saved track or bus; unmapped mixer state remains visible as a gap.
      The probe also compares saved V1 nest trims and bindings, with
      unresolved child identity reported when archive IDs are rewritten.
      It pairs multicam uses by occurrence or record position and checks
      saved picture and mono audio selectors and their file bindings.
      It also compares Solid Color and literal Text+ controls by track
      and record position, with gaps for changed Fusion composition bytes.
      Paired picture edits report changes to the qualified clip-local
      primary gain node. Other grade profiles remain inconclusive.
      [Probe and checks](RESOLVE-SOURCE-READER.md#compare-saved-timelines-and-blobs)
      · [Pinned drpEditor workflow](https://github.com/theRealAi/drpEditor/blob/c5bec57146507741f9f70fd16622d4f302ab5865/docs/reverse_engineering.md)
      · [Pinned DRP Toolkit comparison](https://github.com/koosoli/DaVinci-Resolve-DRP-Toolkit/blob/aadefac4810de59249b8a1980be459b136d5f412/drp_toolkit/plugins/diff.py)
- [x] **CiprianSpiridon/davinci-resolve-mcp DRT format review** (Python,
      MIT, `9f2537e5`). Qualified an internal Resolve 21.2 timeline
      inspector and a bounded graph/2 export from a native `.drt`. The
      eight-frame dissolve, ranged marker, four edits and both mono lanes
      survived native import, Save Project As and reopen. Missing witnesses
      remain visible in inspection and block graph export. This closes the
      DRT reference review; DRT authoring and product exposure remain open.
      [Evidence, commands and remaining scope](RESOLVE-DRT-CLOSEOUT.md)
- [x] **drp-format** (JavaScript, MIT, `1e2bf034`). The review covers
      archive structure, media caches, editorial operations, transitions,
      markers and grade authoring. The initial closeout qualified the editable
      V1 Cross Dissolve. The [feature checklist](#general-conversion-priorities-from-the-pinned-references)
      records subsequent results for picture nests, retiming, generators,
      captions and primary gain. DRT authoring remains open.
      [Coverage and trial](RESOLVE-DRP-FORMAT-CLOSEOUT.md)
- [x] **pr2resolve** (Python, MIT, `61c8c9e`). Reviewed its timeline and bin
      registration, source and record clocks, trims, media and audio channel
      bindings, Motion, and marker encoding against the existing turnover
      profiles. Static and two-key Linear Premiere Motion and static mono
      volume pass through the turnover job and match the retained Resolve
      renders after Save As and reopen. See the [Premiere → Resolve
      checklist](#premiere--resolve) for later track-state, labels, caption
      retiming and timeline mark results and the remaining grade work.
      The upstream Lumetri formulas are approximations. Its native exporter
      rejects nested sequences, so the editable multicam profile uses
      saved Premiere references and the retained Resolve Group witness.
      [Pinned native format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md)
- [x] **drpkit** (MIT, `af3cedc0`). The source review covers Fairlight
      channel and bus tables, EQ and dynamics rows, built-in and insert FX
      records, clip controls, and the limits of the audio-only composer.
      The internal turnover writer preserves qualified static mono pan and
      direct routing to one or two persisted stereo buses. A1 hard left
      on Bus 1 and A2 centered on Bus 2 passed Save As, reopen, and both
      PCM comparisons. Bus 2 static gain from −6 to 0 dB and mute are
      supported in the same profile. Separate −6 dB and mute tests
      matched both stereo references after reopen. Later checks qualified
      six-band EQ and scoped dynamics on mono tracks and stereo buses.
      Wider routing and plug-in behavior remain open in the
      [feature checklist](#general-conversion-priorities-from-the-pinned-references).
      [Coverage and trial](RESOLVE-DRPKIT-CLOSEOUT.md)
- [x] **PRPROJ-READER** (SRC-011, CC0-1.0, `a51a2a5`). The converter's
      Premiere reader already follows saved object references for bins, clips,
      media and sequences. Its component reader exposes static values and
      keyframes in ticks. The internal sequence lister and XML inspector were
      checked on a Premiere 27 project containing two picture edits, two
      audio edits, and same-named originals with different paths, reels and
      source starts. The upstream script's rounded timing, name-based
      parameter lookup and component instance labels are historical leads.
      Its Premiere 14.3.2 fixture stays a reference under the
      [editor-version compatibility policy](#editor-version-compatibility).
      [Pinned source](https://github.com/sergeiventurinov/PRPROJ-READER/tree/a51a2a5b1095b8bdead114c812e85132a5eb26e0)

### General conversion priorities from the pinned references

Use the Resolve knowledgebase comparison matrix for the full reference
inventory. Track source capture, destination writing, and native reopen
and render for each conversion direction.

- [x] **Internal editable picture speed and reverse.** At 24 fps, exact
      trimmed 50% forward and 100% reverse travel through graph/2. Native
      Resolve reopen and all 48 rendered frames passed, including outputs
      from Premiere 27 saves. The Premiere 27 saves retain the editable
      controls, and all 48 numbered Premiere frames matched the Resolve
      cadence after Save As and reopen.
      Ramps, mixed-rate timing and pitch-preserving audio are separate.
      [Time-map codec](https://github.com/samuelgursky/davinci-resolve-mcp/blob/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format/media-timemap.js)
- [x] **Write editable odd-length 50% picture edits.** Both internal
      directions preserve the exact fractional graph/2 source span on
      linked 1080p24 ProRes HQ MOV picture. The 23-, 25-, and one-frame
      cuts matched all 49 source-frame selections after reopen. The
      24-frame control matched as well.
      [Odd-length qualification](RESOLVE-SOURCE-READER.md#odd-length-half-speed-picture)
- [x] **Preserve linked picture frame holds.** Premiere and Resolve
      capture the held source picture through graph/2. Both directions
      write the hold at source In on the qualified 1080p24 ProRes HQ MOV
      profile. Resolve matched its 24-frame native control. Premiere 27
      matched all 47 frame selections across the whole-clip and split
      controls after Save As and reopen. The original Resolve archives
      convert through their captured graphs with the qualified empty
      stereo mixer. Premiere → Resolve also writes holds away from source
      In through the turnover job with explicit loss approval. Two 24-frame
      cases matched after Resolve reopen. Resolve writing and read-back also
      verified 24- and 120-frame holds at the last source picture. The
      24-frame last-picture hold returned to Premiere with an explicitly
      approved underlying trim change and matched all frames after reopen.
      [Frame-hold qualification](RESOLVE-SOURCE-READER.md#linked-picture-frame-holds)
- [x] **Plain captions and Solid Color.** Read and write one default
      subtitle track; two adjacent accented cues survived native Resolve
      reopen and rendered visibly. Support one opaque Solid Color at the
      end of the qualified picture-only timeline. Capture literal Text+
      controls. The same captions were visible in the reopened Premiere project.
      A retained Avid SubCap over online OP-Atom also reopened and
      rendered on Resolve subtitle 1. Per-cue style and editable Text+
      remain separate.
      [Caption fields](https://github.com/samuelgursky/davinci-resolve-mcp/blob/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format/subtitle-captions.js)
- [x] **Video dissolves.** Extend the eight-frame left-aligned dissolve to
      centered alignment and both 24 and 24000/1001. Map exact Avid and
      Premiere source transition endpoints for the bounded profile. The
      24 fps native Resolve round trips matched every frame for both
      alignments. The centered A1 0 dB Fairlight crossfade matched the
      reopened comparison PCM; A2 was unchanged. Native cross-editor
      transition tests remain separate.
      [Dissolve baseline](RESOLVE-DRP-FORMAT-CLOSEOUT.md#qualified-left-aligned-cross-dissolve)
      · [Fairlight transition fields](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/FORMAT.md#edit-information---edits)
- [x] **Bounded multilayer profile.** Read and write a 24 fps V1/V2 overlay
      with three independent WAV mono lanes using a matching native
      skeleton. Reopen retained all tracks, with matching picture and
      three distinct isolated PCM renders. Wider layouts remain open.
- [x] **Editable Resolve picture nests.** Read and write one level of
      native compounds and nested timelines at 1080p24. Two identically
      named children with different origins and trims remained distinct
      after reopen and rendered frame for frame. Also verified two
      separately trimmed compounds, including a nonzero compound origin.
      [Scope and checks](RESOLVE-SOURCE-READER.md#write-picture-nests)
- [ ] Qualify nested audio and wider layouts as separate Resolve profiles.
- [x] **Resolve primary gain.** Read and recreate one clip-owned, single
      primary gain node on the qualified Resolve profile. The native
      1.25 control matched all 24 decoded frames after reopen; a 1.5
      variant changed the render. Other node topologies and cross-editor
      grade mappings remain open.
      [DRX calibration](https://github.com/samuelgursky/davinci-resolve-mcp/blob/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drx-parameters/CALIBRATION-STATUS.md)

See the [internal conversion checks](RESOLVE-SOURCE-READER.md#editable-conversion-profiles)
for the exact read, write, reopen and render scope.

- [x] Apply static mono track pan and direct routing to persisted stereo buses
      during Resolve preparation. Verify both outputs after reopen.
- [x] Preserve independent pan bypass on the qualified two-mono, one-stereo-bus
      Resolve profile. The saved pan values and reopened PCM match a one-change
      archive control. [Pan bypass](RESOLVE-DRPKIT-CLOSEOUT.md#mono-track-pan-bypass)
- [x] Preserve Fairlight track solo on the qualified two-mono,
      one-stereo-bus layout. The linked writer preserves either or both
      selected strips and their existing pan. The secondary `0x011` row
      remains a raw observation. [Reopen and PCM checks](RESOLVE-DRPKIT-CLOSEOUT.md#mono-track-solo)
- [ ] Author a native solo control in the Fairlight mixer and check
      combinations with mute and multiple output buses before widening
      the writer profile.
- [x] Qualify Bus 2 static gain and mute with separate native saves and PCM.
- [x] **Editable Fairlight track processing.** Round-trip the default
      band 3 bell gain and compressor enable, threshold and saved ratio
      on the qualified two-mono Resolve mixer. Separate and combined
      reopened controls match the rendered PCM.
      [Fairlight processing checks](RESOLVE-SOURCE-READER.md#write-fairlight-track-processing)
- [x] Preserve band 3 frequency and Q plus compressor attack, hold and
      release through the same two-mono profile. Reopened controls match
      native PCM, with independent checks for frequency, Q and timing.
- [x] Preserve compressor knee and mix, dynamics makeup gain, track EQ
      bypass and processing order. Compare reopened PCM with separate
      one-change controls and with EQ and compression on the same track.
- [x] Preserve editable Fairlight track expander and limiter settings.
      Verify independent and combined playback renders plus disabled
      controls after reopen.
      [Pinned drpkit dynamics map](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/drpkit/fairlight_map.yaml#L87-L103)
- [x] Preserve Fairlight track gate mode 1, its enable switch and saved
      controls. Compare reopened PCM with the same expander settings and
      a separate threshold control. Other mode codes remain read-only.
- [x] Preserve all six Fairlight track EQ bands, including filter type,
      outer-band slope, and mid-band frequency range. Verify reopened
      renders with multitone sources, bypass and individual band switches.
- [x] Preserve the single stereo bus limiter, including disabled settings.
      Compare reopened PCM with bus and track limiter controls.
- [x] Preserve independent limiter and compressor controls on the two
      qualified stereo buses. Compare both reopened PCM outputs and
      confirm their stored settings and bypass behavior.
- [x] Preserve six-band EQ and bypass independently on the qualified stereo
      buses. Verify both reopened outputs and retain the two observed saved
      processing-order codes.
- [x] Preserve independent static bus input gain from −6 to +6 dB on the
      qualified layouts, with fader and dynamics makeup stored separately.
- [x] Write the qualified default clip fades and two-key volume curves on
      direct mono WAV edits in linked 24 fps Resolve projects.
- [x] Preserve editable custom clip fade points, including a separate static
      clip volume, on the qualified Resolve WAV and OP1a profiles.
- [x] Preserve two source-time volume keys alongside clip-edge fades on the
      same qualified Resolve WAV or OP1a edit.
- [x] Preserve the 16 Resolve timeline clip labels by edit occurrence in
      linked project exports, including repeated edits of the same media.
- [ ] Qualify additional Fairlight bus processing and wider bus layouts.
- [ ] Scope plug-in bypass, track and plug-in automation, cross-editor fade
      interpolation, and additional dynamics laws with one-change native controls.

- **Bookmark: storytold/filmcraft PR #670** "Interchange: import DaVinci
  Resolve projects (.drp)" (by nikpottbecker; fork branch `feat/davinci-resolve-drp-import`,
  `crates/interchange/src/drp/` ~1,900 lines Rust). States clean-room: layout read off Resolve
  19/20 exports, no Blackmagic software or other code cited. Facts it records: a `.drp` is a ZIP of
  XML documents mirroring Resolve's project database (`project.xml`, `MediaPool/**/MpFolder.xml`,
  `SeqContainer/<id>.xml`; classes `Sm2MpFolder`, `Sm2Timeline`, `Sm2Sequence`, `Sm2TiTrack`,
  `Sm2TiVideoClip`, ...); binary fields hex (protobuf / Zstandard inside); `Start` = timeline frame
  from the timeline start TC, `In` = source offset in TIMELINE frames (fractional for other-rate
  media, `64|<double>`). Imports bins, clips, timelines, tracks, Text/Text+ titles, Transform,
  Volume, fades, transitions, compound and multicam (flattened); reports grades, Fusion, retiming,
  keyframes, markers. Filmcraft is production code: read it as a lead only, never depend on it.
- **Other open-source readers found (GitHub code search on the class names):**
  | Project | Language / licence | Scope |
  |---|---|---|
  | hfiguiere/drp-extractor | Rust, MPL-2.0 | [Archive intake reviewed](RESOLVE-SOURCE-READER.md#inspect-archive-and-version-metadata) |
  | samuelgursky/davinci-resolve-mcp | Python/JS, MIT | [Offline conform verification reviewed](RESOLVE-SOURCE-READER.md#check-conform-at-v1-cuts) |
  | CiprianSpiridon/davinci-resolve-mcp | Python, MIT | `formats/drt.py` reviewed; [native DRT reader scope](RESOLVE-DRT-CLOSEOUT.md) |
  | koosoli/DaVinci-Resolve-DRP-Toolkit | Python, GPL-3.0 | [Comparison and effect inventory reviewed](RESOLVE-SOURCE-READER.md#inventory-saved-effects) |
  | donutinit/sqsolve | Go, AGPL-3.0 | [Archive and bin order reviewed](#completed-reference-reviews-and-their-scope) |
  | LucaL1fe/davinci-project-repair | MIT | [Saved timeline selection reviewed](#completed-reference-reviews-and-their-scope) |
- **Routes to evaluate:** (1) native `.drp` read/write (most fidelity, undocumented, version
  drift 19 -> 20 -> 21); (2) Resolve's own interchange imports (FCPXML, FCP7 XML, AAF, EDL, OTIO):
  FCPXML is an optional export, and further FCPXML work is on hold; these imports may be used
  for comparisons only; (3) Resolve's scripting API (Studio; the free
  edition via the in-app bridge on 21.0.x) for automated read-back in tests, like the Avid API
  item below; samuelgursky/davinci-resolve-mcp is that route.

## App

- [x] **Avid to Premiere wizard** (User design, 2026-10-15). The five pages cover
      project and dependency selection, clips and media, markers and colors,
      effects, and review and convert. Native rewrap is first and the default,
      followed by Grouped, Merged and Separate. The app remembers choices, supports
      presets and job replay, and shows space estimates and **“Creates new media.”**
      before execution. The completion page offers the output folder, searchable
      report and Open in Premiere when installed. [Current wizard](GUI-WIZARD.md).
- [x] **Run the published Apple Silicon package through Premiere.** The wizard and
      bundled backend converted a small 23.976 Avid split-edit bin. Both sequences
      opened online and played to the end in Premiere 26.5.2. Sampled picture and
      rendered PCM matched the retained reference after save and reopen, including
      separate checks of both mono lanes. This release predates the TC1 correction below.
- [ ] **Next platform check: Windows x64.** Install the published package, run the
      same bin through the wizard and repeat the Premiere checks. Keep the remaining
      platform checks open until they have their own evidence.
- [x] **Correct the qualified 23.976 non-drop sequence start in the converter.**
      For a unique, sequence-spanning primary TC1, write its count at the exact
      record rate. The split-edit bin now writes the `01:00:00:00` origin
      (`915372057600000` ticks); relative edit and transition ranges are unchanged.
      Unknown or unsupported sequence clocks retain the compatibility fallback.
- [x] **Read back the corrected zero point in Premiere 27.** Both direct-linked
      split-edit sequences retained `01:00:00:00` after Save As and reopen.
      [Native read-back](SPLIT-EDIT-AUDIO-2026-11-06.md#premiere-27-direct-linking)
- [ ] **Extend the wizard for qualified new directions.** Reuse Review and
      the direction-specific flow when a route passes its native and packaging
      gates. Use the [current-version compatibility policy](#editor-version-compatibility).
      Proxy and additional original-audio linking remain under [media choices](#media-choices-and-review).

## Release maintenance and packaging

- [x] **Automate preparation of the filtered export.** Use the reviewed path
      manifest and private candidate receipt in the [release guide](RELEASE.md#separate-gates).
- [x] **Use Cargo as the release version authority.** See the version and
      build-only inputs in the [release guide](RELEASE.md#build-stage-and-publish).
- [x] **Recover an interrupted draft upload.** Resume only against the
      original asset set and reviewed commit. The [release guide](RELEASE.md#build-stage-and-publish)
      holds the workflow and recovery procedure.
- [x] **Run the six-target build-only release check.** Verified the six
      platform artifacts and checked that the alpha.3 release and assets
      stayed unchanged. [GitHub Actions run](https://github.com/whatever-industries/any-nle/actions/runs/43362741087)

The [Windows x64 editor smoke test](#app) is the next platform qualification.
Use the [release guide's separate gates](RELEASE.md#separate-gates) for public
build-only verification, private qualification and publication.

## On hold (User, 2026-10-28)

### Further FCPXML work

- FCPXML read-back in Resolve (User, or Resolve's scripting API in a throwaway project), then
      the losses in docs/FEATURE-MATRIX.md one by one: pan (`adjust-panner`), volume keys,
      transform (`adjust-transform`/`adjust-crop`, units to confirm), speed (`timeMap`),
      crossfades, drop-frame, stereo as one 2-channel clip.

### Additional surround work

- The User resumed declared mono/stereo/5.1/7.1 rewrap grouping on 2026-11-07;
  see the active multichannel item above. Further surround mixing, speaker/MCA
  semantics and output formats beyond that bounded slice remain held.

### Avid local API test automation

- **Avid local API as a read-back oracle (test tool only, never a product dependency).**
      Media Composer embeds an undocumented local gRPC service `mcapi.MCAPI` (third-party
      research: github.com/leancoderkavy/avid-media-composer-mcp, MIT, docs/NATIVE_API_RESEARCH.md;
      49 methods on port 9100 in the Windows 2024.12 report). The User's Mac build 2025.12.0.58863
      contains the same `MCAPI.proto` / `MCAPI_Types.proto` descriptors (GetMarkers, ExportFile, ...).
      Plan: confirm the listener with Avid open; read-only probe (reflection, GetAppInfo, GetBins,
      GetMobInfo, GetMobTrackInfo, GetMarkers, GetBinColumnInfo) on a disposable copy of an existing
      test bin; compare with avid-reader's model. Later: ImportFile/ExportFile for the
      Premiere -> Avid direction. Unsupported API; behaviour varies by version.

## Completed capabilities

Retained here with their evidence and limitations; these are no longer active to-dos.

### Explaining decisions (User, 2026-10-14)

- [x] Started `docs/DECISIONS-EXPLAINED.md`: plain-language reasoning for every import option
      and automatic decision, with finding/decision IDs.

### Bin layout: builds and parts (User, 2026-10-14)

- [x] Top level of each converted bin = builds (grouped/merged clips, stereo audio clips,
      unpaired video, mono audio); the parts (source V/A items) go into a per-bin
      **Processed Clips** subfolder.

### Audio

- [x] Stereo pairs (`_CHANNEL_GROUP_LIST`) and stereo edits on stereo tracks, grouped mode
      (PRPROJ-0019, User-confirmed).

- [x] Stereo in Separate mode uses an audio-only stereo nest built from the A01/A02
      pair. It places one stereo item on the Avid track number with picture linked
      when present. The nest superseded the earlier zero-video multicam
      (PRPROJ-0021/0022).

- [x] Stereo in Merged mode: a stereo pair is built as a group instead; reported (User).

- [x] Clip pan storage and value scale (PRPROJ-0021, PRPROJ-0023).

- [x] Master-clip gain from PVOL (PRPROJ-0020, User-confirmed).

- [x] Edit pan and gain from sequence PVOLs; Avid default panning option (PRPROJ-0023, CONV-0022).

### Clip, marker and correction color

- [x] **Color correction** (CONV-0026, User-confirmed render match): Avid HSL Controls + Curves tab ->
      one embedded Lumetri Look LUT computed from the Phase 52 Avid model; .cube beside the project.

- [x] **Markers** (PRPROJ-0025, CONV-0024, User-confirmed except merged mode): Avid marker color *name* (`_ATN_CRM_COLOR_EXTENDED`, 16 names; the stored RGB
      is only the legacy 8-color value) -> Premiere's 8 marker colors, defaults per
      DECISIONS-EXPLAINED; name (`_ATN_CRM_MARKNAME`), comment (`_ATN_CRM_COM`), spanned
      length (`_ATN_CRM_LENGTH`); option to append Avid user + color name to the comment.
      Placement: master/subclip markers -> bin clip markers; sequence copies of them skipped;
      markers on video filler -> one-frame adjustment layer; markers on audio filler -> sequence
      markers. Markers added on a timeline clip in Avid -> markers on that edit in Premiere
      (a per-edit subclip carrying the source markers plus the edit's own), the edit's marker
      replacing a source marker at the same frame. Import options: source marker wins instead;
      put the losing marker on the sequence instead of dropping it (User 2026-10-15).

- [x] Avid clip color (User-confirmed labels) (read: `_COLOR_R/G/B`, KB BCOL-019) -> nearest of Premiere's 16 label
      colors (`Column.PropertyText.Label` = `BE.Prefs.LabelColors.N`, plus the item's
      `asl.clip.label.*`), using Premiere's default label RGBs; custom Avid colors that only
      approximate a label are listed in the report. One generate-then-confirm round.

### Timeline and effects

- [x] Constant speed and reverse (PRPROJ-0024, User-confirmed: 50 %, 198 %, reverse).

- [x] **Motion, static** (CONV-0027): Avid Resize and 3D Warp position / scale / Z rotation /
      axis / crop -> Premiere Motion. Render-compared 2026-10-20: within 1 px in Premiere's H.264
      export; its TIFF export keeps ~3 px more where crop and scale combine (render path).

- [x] **AniMatte baseline**: reader and Premiere Opacity-mask writer built; default Key In,
      1920x1080, fixed topology and one tracked point. 46/46 frames matched Avid after the
      confirmed square-pixel interpretation (PRPROJ-0029 / CONV-0029). Other cases remain above.

- [x] **ProRes LT in Avid OP-Atom MXF**: reader checks both the Avid compression and MXF
      picture-coding identity; both Premiere writers have the observed `apcs` media profile.
      This does not cover all ProRes profiles or solve Premiere's MXF playback issue.

- [x] **Time-remapping writers** (2026-10-20): supplied frame maps reproduced
      192/192 Avid picture selections in the retained Premiere H.264 exports
      (PRPROJ-0028 / CONV-0028). The bounded source path is tracked above.

### More programs: the neutral graph (ADR-0004, User 2026-10-26)

- [x] **Graph-to-model placement validation** (2026-10-29): the bridge shares model/1's
      cross-object checks with the JSON loader (required video/audio streams, sequence record
      frame grid, source span within media). Invalid sequence rates and item-ID collisions
      during projection are refused. Valid existing models retain their output bytes.

- [x] **FCPXML writer from the graph** (2026-10-27, `crates/fcpxml`, `any-nle convert --to fcpxml`):
      DTD-valid on all 36 golden models and 3 real bins. Test files in
      `local/to-test/2026-10-27 fcpxml/`.

### App

- [x] **Port the Premiere writer (py_premiere + converter Premiere logic) to Rust** (done 2026-10-20,
      ADR-0003; Rust `prproj-write` byte-identical to Python, User read-back passed) (ADR-0001;
      started 2026-10-19 at the User's request). Plan: Phase 0 deterministic Python oracle + golden
      corpus (done, `scripts/make_golden.py`); 1 XML core; 2 builders; 3 graph operations;
      4 converter logic; 5 switch-over with a User read-back. New standalone crate `prproj/`; the
      Avid reader is not touched (Codex's Avid research continues there); the JSON model stays the
      boundary. Acceptance: byte-identical output to the Python writer on the golden corpus.

### Reliability (2026-10-29)

- [x] **Grow the ordinary AVB object table after accepted objects** (2026-10-31): avoid
      reserving the advertised full table before malformed chunks are checked. Grow with
      accepted decoded/unsupported objects, capped at the declared final slot count. Keep
      parser values, error order and ordinary conversion results. Actual large-bin decoded
      storage and broader project-memory work remain open. Structural/differential checks only.

- [x] **Decode straight into editable AVB records** (2026-10-31): remove the temporary
      extraction object table and raw-effect copies during document loading. Keep original
      chunks, decoded values, snapshots, errors and edit/copy/save behavior; ordinary
      extraction retains its decoded/raw data. This reduces loading peaks, while retained
      document data and broader project-memory work remain open. Structural/differential
      checks only.

- [x] **Release saved-bin validation objects individually** (2026-10-31): use the normal
      full-file decoder checks without retaining a second complete decoded bin or raw-effect
      payload copies. Original output bytes, refusal rules and ordinary extraction stay the
      same. One large object, the editable document and output buffer still need memory;
      broader project-memory work remains open. Structural/differential checks only.

- [x] **Share unchanged decoded AVB snapshots** (2026-10-31): original/editable states share
      read-only node data until mutable access detaches an object's working copy. Preserve
      value-based change detection, rollback, source-only checks and copy/save behavior.
      Large mutable objects and output validation still need memory; ordinary extraction
      and whole-project memory remain separate.

- [x] **Avoid duplicate preserved AVB chunk buffers** (2026-10-31): loaded documents read
      original chunks from their retained index; copied graphs keep independent owned bytes.
      Preserve source checks, unknown chunks, IDs, repeated saves and copy lifetimes in both
      byte orders. Decoded node snapshots and ordinary conversion/project memory remain.

- [x] **Lower edited AVB save memory** (2026-10-31): borrow preserved chunks while assembling
      output and validate the completed buffer without cloning the whole file. Release
      temporary index spans before decoded validation. Preserve bytes, repeated saves,
      unknown chunks, edit checks and size limits. Savings depend on payload structure;
      ordinary conversion/project memory remain separate.

- [x] **Lower editable AVB loading memory** (2026-10-31): reuse the original input buffer
      and transfer decoded objects into the document, while retaining original/editable
      states and exact unchanged chunks. Existing edit validation and opaque/trailing bytes
      are preserved. This is the editable-document API; ordinary extraction and whole-project
      memory remain separate. The maintainer handoff records the memory measurements.

- [x] **Enforce AVB input byte limits during reads** (2026-10-31): decoded, indexed and editable
      readers share a bounded read of the same file handle. Refuse known oversize inputs
      before loading their bodies, and stop growth/underreported inputs at the existing 1 GiB
      limit. Valid bytes and conversion behavior stay unchanged; decoded/project memory
      remains a separate limit.

- [x] **Repeated bin selections**: both CLIs decode each canonical input path once, report
      skipped repeats, and retain first-selected names/order. Distinct files and normal input
      errors remain distinct; sequence inspection uses the same rule. Whole-project folder
      discovery and bounded-memory streaming remain separate active work.

- [x] **Repeated identities in one MXF**: count each package identity once per file when
      building the media index. Keep all distinct identities and preserve ambiguity between
      different file paths; raw header inspection and codec/timecode/duration rules are unchanged.

- [x] **Stereo transition spans**: trim all matching channels together before assembling
      stereo edits, coalesce identical track-transition records and reject unsafe trim bounds.
      Give generated stereo sources their actual content bound for writer handle checks. Keep
      existing Constant Power audio fades and reported video cuts; mono and partly missing
      stereo controls remain valid. No new transition shape or application read-back.

- [x] **Protect project staging files**: new and in-place saves exclusively allocate an unused
      temporary sibling, preserve occupied files/directories/symlinks, and clean up their own
      stage after write/rename failures when possible. Snapshots at the old `.tmp` name survive;
      Rust/Python project bytes and media paths stay unchanged.

- [x] **Protect and distinguish LUT exports**: keep existing `.cube` files, use numbered
      filenames for collisions (including simultaneous edits and sanitized sequence names),
      and link each edit to its chosen file. Refuse symbolic-link LUT folders. Rust/Python
      writer parity is required; colour math and editable Lumetri controls are unchanged.

- [x] **Protect graph snapshot outputs**: refuse existing entries (including symbolic links),
      missing output folders and colliding project/snapshot names before writing; create new
      snapshots exclusively. Valid conversion bytes stay unchanged. No application read-back.

- [x] **Overlapping media folders**: scan repeated folder selections and discovered file paths
      once, so selecting a parent and child folder cannot make the same MXF falsely ambiguous.
      Separate file copies remain ambiguous; explicit child selections retain their search depth.

- [x] **Partly missing stereo media**: remove incomplete bin pairs; keep the available timeline
      channel as mono with its original gain, pan and record position, reporting the fallback.
      Missing-media status follows the selected bin copy. Larger incomplete channel groups are
      omitted/reported; surround support stays on hold. Structural/differential checks only.

- [x] Skip/report a clip with no usable media instead of emitting an invalid no-media clip
      that refuses the project. Its unconverted edits are reported; other clips still convert.
- [x] Rust/Python writer fixes: Unicode sequence names with colour correction; reject duplicate
      curve x coordinates at validation; positional source-track fallback for audio edit markers;
      reject unsavable marker comments at validation; keep absolute Windows media paths instead
      of making them relative to a POSIX cwd. Structural/differential checks only.
