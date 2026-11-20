# Relink moved MOV originals

In the macOS desktop app, open **Relink media in an existing project** on the
Project page. Choose a saved Premiere 27 or Resolve Studio 21.2 project. Use
**Choose moved file** beside each original you moved, then save a new project.
The row shows the full path from the saved project. Keep the original filename
when choosing its new location. Unselected files must still be available at
their saved paths. The original project and media stay in place.

The default checks the exact saved path and filename. If multiple saved paths
share a filename, their saved reel and source timecode must distinguish them.
Any NLE reads the replacement MOV's embedded timecode-track tags for that
comparison. Missing or ambiguous tags stop the operation. This only reads the
container metadata. The default trusts that a uniquely named file at the
explicit destination is the intended original.

**Verify file contents with SHA-256** is off by default. Leave it off for routine
moves. It reads every byte of each selected file and can take considerable time.
Use it with checksums captured while the known-good originals are online. In the
dialog, enable verification, check the originals to capture, and save a checksum
manifest before moving them. When relinking, choose that manifest. A previously
captured manifest also resolves duplicate names with insufficient reel or
timecode metadata. The same-name Premiere cache refusal applies in both modes.

## Local command

Build the standalone tool with
`cargo build --manifest-path crates/Cargo.toml -p anynle-resolve-adapter --bin verified-relink`.
Save a JSON array of moves. Use absolute paths from the project and specify the
exact file at each destination.

```json
[
  {
    "original": "/Volumes/Camera/Card A/CLIP.mov",
    "destination": "/Volumes/Online/Card A/CLIP.mov"
  }
]
```

```text
verified-relink apply Input.prproj moves.json Relinked.prproj
verified-relink capture Input.prproj Original-checksums.json
verified-relink apply Input.prproj moves.json Relinked.prproj Original-checksums.json
```

The CLI captures all known-good originals when `capture` is explicitly requested.
The macOS dialog can capture just a selected subset. The `apply` command verifies
file contents only when given a manifest, and hashes each selected destination
once. The manifest is tied to the exact saved input project. The tool uses
`ffprobe` on PATH for duplicate-name metadata checks. The macOS app can also use
its bundled tool or a configured override. Successful writes use exclusive
publication in the destination directory. Existing outputs are refused.

## Verified scope

The supported project formats are Premiere project version 46 and Resolve
21.2.0.0006 archive version 17. The native comparison used two same-filename
1080p24 ProRes HQ MOV originals with embedded PCM24, different reels and nonzero
non-drop source starts. The exact paths, reels, edit ranges, 48 decoded picture
frames and 96,000 stereo PCM sample frames agreed after native save and reopen.
Resolve retained both reels in the archive's Media Pool metadata. The default
Reel Name display was blank for this fixture.

Premiere refused the retained equal-size, equal-timestamp cohort with a shared
content cache ID. In the native control, that cohort could display both correct
paths and play correct audio while switching picture at the wrong frame. The
exact cache-key inputs are unresolved. Any NLE also refuses equal file size and
modification second for same-name Premiere candidates as a conservative guard.

The current profile uses linked MOVs and the inspected native locator
structures. Proxies, separately linked Resolve audio, other Resolve pool object
classes, missing paths, and contradictory media references require separate
support. This is a relink of an existing native project. Cross-editor conform
and other media or project profiles have their own qualification.

### Repeat a move after Premiere Save As

Premiere 27.0.0 build 88 saved two relative path records for each of the
same-named MOVs after the first directed move. The two spellings differed
only by a leading `./`. Verified relink accepts that pair and still refuses
different relative paths. The existing checks on the full saved paths,
duplicate names, reels, and source timecodes apply to every repeat move.
An offline replay from the native Save As project wrote both second
destinations with the original reels, source starts, and edit ranges.
The reel/timecode default and the pre-move checksum option agreed.
Premiere 27 saved and reopened the second-move output with both new paths,
the distinct reels and source starts, and the original edit ranges. Both
24-frame picture previews matched their assigned camera. The solid-color
fixture distinguishes cameras, and the saved project establishes the exact
source trims.

On macOS and other Unix platforms, the verifier also refuses two distinct
destination paths that are hard links to the same file. This uses filesystem
identity and needs no content hashing. Avid original-media root mapping uses
the same identity check when resolving competing saved locators. The direct
CLI and saved jobs refuse a mapped UME collision. The opt-in camera-original
lookup reports the conflict and follows the managed-media policy.
When a saved locator and its mapped path resolve to different files,
the diagnostic lists the competing paths so the user can correct the mapping.

### Interleaved linked A/V originals

A derived 24 fps bin references two same-named H.264/AAC MOVs in A–B–A
order through three distinct masters. The direct CLI and saved job write
all nine edits with the expected original paths, file-relative trims,
and independent left and right channel selections. A second variant
keeps distinct Avid Start values in Premiere's Log Note while the files
retain their zero-based media clocks. Mapping both saved paths to one
file blocks publication.

Preparation caches two recent original probes. This fixture calls the
probe twice instead of three times, with identical seeded project and
graph bytes. The first probe still checks content before and after
inspection, and the final check catches changes even on a cache hit.
These are structural checks with retained original media. The test with
two native links and distinct signals is below.

The tagged version of the same control establishes a separate embedded
timecode receipt for the linked 24 fps H.264/AAC MOV profile. The picture
and single timecode track must declare the same non-drop label and agree
on rate, time base, and extent. Conflicting declarations stay unresolved.
Graph/2 retains the observation separately from Avid Start and from
file-relative edit positions. Premiere preparation reports the receipt
without setting a source start override from it.

Premiere 27.0.0 build 88 showed the same Media Start and End in the
direct imports and in the converted project after Save As, close, and
reopen: `10:02:03:04`–`10:02:05:03` for A and
`11:03:04:05`–`11:03:06:04` for B. Premiere populated the same
saved alternate-start ticks in both projects. The converted masters
kept their separate Avid Start values in Log Note, and all nine edits
kept their paths, trims, and physical channel selections. All 36
rendered picture frames matched the expected file-relative frame
numbers. The two MOVs share picture and audio payloads, so file identity
was checked by saved path and embedded clock. The stereo render is not
an isolated PCM comparison. No writer timecode override is needed for
this fixture. See `CONV-0174` for the hash pins.

### Native same-name A/V control

Media Composer 25.12.0 build 58863 linked two 24 fps H.264/AAC MOVs both
named `COUNTER.mov`. They have distinct picture and stereo signals and
the embedded starts `10:02:03:04` and `11:03:04:05`. A derived A–B–A sequence
uses those native master and file IDs with separate trims on V1, A1 and
A2. The linked AAF contains both paths and all nine edits. Premiere 27
build 88 kept the three picture edits, correct media paths and embedded
starts after Save As and reopen. The reopened timeline has no audio
edits, and its exported PCM is silent.

Avid's 48 kHz Direct Out selects all six expected file and channel
combinations. Each begins 2,000 samples into the physical stream beyond
the saved trim. The two native MPGA descriptors each report 97,024 samples,
and both audio track headers have start position 1. The MOVs expose
96,000 playable samples and a skipped 1,024-sample AAC priming packet.
Keep the one-frame route offset separate from decoder priming. Each
saved original TC1 count is one frame before the MOV's embedded start.

A disposable dual-mono control established the offset with a manually
adjusted copy. The converter now reads the untouched native link facts
from the test bin. In Separate files mode, graph/2 carries the physical
position plus a receipt with the original trim and file-track start.
Premiere retains each stereo source in its original bin; a separate
**Avid timeline audio** bin contains mono views of the same originals
for the independently trimmed A1 and A2 edits. The converter creates
no media for either view.

The direct CLI and saved job retain both paths and all nine placements
with the expected starts, trims, and audio selectors. After Save As,
close, and reopen in Premiere 27 build 88, all 36 decoded pictures match
the linked AAF and all 72,000 PCM24 samples per channel match Avid
Direct Out with **Match Avid default pan**. The original clips remain
stereo in the reopened project. This is a straight-edit profile; new
insertions use the MOV audio origin and do not recreate the saved Avid
offset. Both entry points refuse the controls with an unproven origin
or a trim extending beyond playable audio. See `CONV-0178` for the
original comparison and `CONV-0179` for conversion evidence.

### Combined macOS package replay

A local Apple Silicon package converted the native same-name bin
together with three retained bins covering mixed managed/UME media,
marked dissolves, and trimmed Submasters. Review found all six original
files and showed the AAC fresh-insertion limitation before conversion.
The wizard, direct CLI, and saved job agree with the reopened Premiere 27
project on 13 sequences including the three children, 63 placements,
19 marker uses, and four transitions. The 12 existing sequences retain
the same saved structure as the earlier combined control.

The reopened wizard project matched all 36 decoded AAC-case picture
frames against the linked AAF and matched both 72,000-sample PCM24
channels against Avid Direct Out. The mixed managed/UME sequence
matched all 1,174 pictures and both 2,350,348-sample PCM24 channels
against the retained Premiere renders under the same pan policy.
These are separate render comparisons within the four-bin project.
No relinking or new media was required. (CONV-0181)

## Avid offline output

Choose **Allow offline media** with Separate files, Grouped, or Merged. Use
`--allow-offline-media` for direct conversion, or set `allow_offline_media`
in a saved job. Review identifies the offline media. The converter writes
`OUTPUT.prproj.anynle-relink.json` next to the project when any source is
offline. Rewrap requires all of its input files before it creates media.

Create a JSON array of `original` and `destination` pairs. Copy `original`
from the report's `path` field, which can be a package-based placeholder.
Use the absolute current file path for `destination`. Each managed MXF
sibling has its own mapping. Both channels of a linked original share
one mapping.

Any NLE keeps the exact saved path for an offline linked original. A managed
MXF may have only a file-package identity in the bin: the source camera file's
logging path cannot locate the separately managed picture and audio files.
If the media is available, adding its Avid MediaFiles folder and rerunning
Review lets Any NLE resolve each MXF by package before conversion. If it cannot
resolve a source, use the report to verify its destination before locating it
in Premiere. In the tested Separate split edit, Premiere 27 left some uses
offline after Link Media and reopen, so check the timeline as well as the bin.

```sh
any-nle verify-offline-relink OUTPUT.prproj.anynle-relink.json MOVES.json
```

The verifier checks managed package identity, the effective file clock and
essence extent. For a linked original it checks the filename, stream layout,
and file-relative timing. Saved clip logging timecode remains separate from
the physical file origin. The check reads files in place and prints the
verified mappings. Use them when selecting files in Premiere's **Link Media**
dialog. [Adobe Link Media guide](https://helpx.adobe.com/premiere/desktop/organize-media/ingest-proxy-workflow/relink-offline-clips.html)

For duplicate saved original basenames, supply `--checksums TRUSTED.json`
with `schema` set to `anynle.offline-checksums/1` and an `originals` array
of `path` and `sha256` pairs. The path is the report's offline locator. Use
hashes captured while the known-good originals were available. Without the
option, the verifier does no full-file hashing. The qualified offline
profiles and Premiere read-back status are tracked in the
[roadmap](ROADMAP.md#offline-output-and-relink).

## Prefer camera originals from Avid

Select **Prefer camera originals for picture** under **Primary media** on the
**Clips & media** page for Separate files,
Grouped, or Merged. The Avid → Premiere CLI option is `--prefer-source-media`.
Saved Premiere jobs use `"prefer_source_media": true` at the top level. Internal
Avid → Resolve jobs use the same setting inside the `avid_to_resolve` source
object. The options default off.

This conversion-time lookup uses Avid's saved **Source File** plus **Source
Path**, or the full saved **UNC Path** corroborated by Source File. Use the
existing explicit root mappings when originals have moved. Only exact saved or
mapped paths qualify. UME links continue to use their saved descriptor locators.

Saved Windows drive and UNC directories retain their original spelling in
graph/2 and in Premiere's Log Note. Review also lists the saved locator
when an explicit root mapping resolves the file. The physical link uses
the verified current path. A derived two-camera control passed both the
CLI and saved-job routes with one drive path and one UNC path. It kept
the two same-named files separate and refused swapped roots. This check
reused the retained picture media and verified the generated projects
structurally.

The first profile is a picture-only, full-extent Avid master with a square-pixel
1080p ProRes 422 HQ MOV original at exact 24 or 23.976 fps. The original must
match the saved non-drop timecode and full frame extent and have zero stream and
clip offsets. Timeline edits can still trim that file. Any NLE reports rejected
or missing candidates and follows the selected managed-media policy. Successful
lookup links the original in place, while the Avid media serves as a conversion
fallback. Use the separate attachment option below to make the Avid media a switchable proxy.
Reusable subclips and Group sources keep their managed bindings. The converter
pins selected originals during preparation and rechecks the same files before
publication. The saved path, timecode and extent do not verify the historical
contents of a file that has been replaced in place.

Synthetic AVBs with a seven-frame source trim and a nonzero original timecode
reopened and rendered file-relative frames 7–30 in order at 23.976 in Premiere
27.0.0 build 88 and at 24 and 23.976 in Resolve Studio 21.2.0 build 00006.
An explicit moved-root mapping resolved the correct file. A same-named file one
frame off the saved source timecode was rejected. A managed PCM16 A/V control
with conflicting path metadata kept both managed mono siblings and produced
byte-identical seeded Premiere projects with the option on or off.

An additional 24 fps Separate files test used two picture-only ProRes HQ
MOVs with the same basename. A synthetic Avid bin supplied two different
saved paths, Tape values, non-drop source starts, and trims. Direct CLI
and saved-job conversions selected the moved files using explicit root
mappings. The verified relink tool also resolved both moves from the
native Premiere save with both original paths absent. Premiere 27.0.0
build 88 preserved the new paths, reels, start timecodes and file-relative
trims after Save Project As and reopen, and played through the two-second
edit. Swapping the files failed. A copy with the correct timecode and a
different embedded reel also failed. Camera-original lookup checks the
embedded reel when both the MOV and the saved Avid Tape supply one.
This test used Any NLE's directed mappings and a picture-only fixture.

Any NLE also writes each saved picture reel to the master clip's Tape
Name logging field. Retained Premiere 27 projects for the same MOV pair
store the reel there as well as on the media reference. The converter's
CLI and saved-job outputs agree on both reel fields, source starts, and
file-relative edits. The four-bin wizard project confirmed both reel
fields after Premiere 27 build 88 Save As and reopen. The same-named
originals retained their distinct moved paths, source starts of
`10:00:01:00` and `11:00:02:00`, and file-relative trims. See
`CONV-0166` and `CONV-0183` in the evidence ledger.

### Native proxy attachment and audio layout

A disposable 1080p23.976 picture-only ProRes HQ MOV and its native Avid
DNxHD LB picture OP-Atom MXF kept their primary and proxy paths after Save As
and reopen in Premiere 27.0.0 build 88 and after save, close, reopen, and
DRP export/import in Resolve Studio 21.2.0 build 00006. The explicit Resolve
attachment accepted the existing Avid-generated basename. Both apps kept
source timecode `02:00:00:00` and the tested edit at file frames 7–30.
The first, middle, and last sample frames selected file frames 7, 19, and
30. Premiere's proxy toggle changed the sampled pixels at all three positions
without changing frame selection. Resolve's scripted still export returned the
same pixels in all three proxy preference modes, so it did not establish which
file supplied playback in each mode.

| Case with the original stereo MOV | Premiere 27 | Resolve 21.2 |
| --- | --- | --- |
| Attach the existing picture-only Avid MXF | The attach call returned without an error, but `hasProxy` was false and the stored proxy path was empty. | The attachment and the original stereo mapping survived reopen. The two render comparisons had identical non-silent PCM and video, so those exports did not distinguish original from proxy playback. |
| Attach a rewrap containing the Avid picture and both mono siblings | The two-mono OP1a MXF and two-mono MOV both attached on read-back. A MOV with one stereo stream also attached and its path survived Save As and reopen. | The stereo MOV attachment and original stereo mapping survived reopen. |
| Reconnect the original after cutting with the proxy | Native reconnect remains untested for this pair. | Reconnecting over picture-only MXF, two-mono OP1a, and stereo MOV kept all three 24-frame trims. Reconnecting over the two-mono OP1a changed the Media Pool mapping from two mono assignments to one stereo assignment. Existing timeline audio placements did not acquire a new layout automatically. |

The rewrap controls retained all 96 compressed picture packets and all 192,192
PCM samples per channel in the original order. The stereo MOV interleaved the
two PCM16 siblings as left and right without changing sample values. A direct
experimental Atom-to-MOV copy also carried an extra packet and a 97-frame
declaration, so the successful prototypes use the verified 96-packet boundary.
These are editor-authored controls. The converter-authored results are below.

An opt-in rewrap for proxy attachment should use the camera original's verified
channel layout and preserve the Avid edit's channel selection. Keep independent
Avid audio edits bound to their verified sources when the original audio has a
different layout or different content. Reconnecting a source can change how
fresh insertion maps channels even when existing audio placements are still
present. [Adobe proxy compatibility](https://helpx.adobe.com/content/dam/help/en/pdf/premiere_pro_reference.pdf)
· [Blackmagic proxy guidance](https://documents.blackmagicdesign.com/SupportNotes/DaVinci_Resolve_17_New_Features_Guide.pdf#page=19)


### Automatic Avid proxy attachment

In Premiere's **Separate files**, **Grouped**, or **Merged** mode, enable
**Attach matching Avid media as proxies** under the camera-original choice. On the CLI use
`--attach-avid-proxies`, which implies `--prefer-source-media`. In a saved
Premiere job set `"attach_avid_proxies": true`. The internal Avid → Resolve
turnover job accepts the same setting inside its `avid_to_resolve` source object.

A verified picture-only DNxHD LB OP-Atom file attaches directly. For a matching
stereo original, explicitly choose a folder to enable rewrap. This is
`--rewrap-avid-proxies EXISTING_DIR` on the Premiere CLI or
`"proxy_rewrap_dir": "/existing/folder"` in either saved job. **Creates new media.**
Review checks available space and existing paths before writing. The camera
original remains primary. A stereo MOV proxy copies DNxHD picture and
interleaves managed A1/A2 as left/right PCM. The converter checks each original
channel against its managed sibling before moving the timeline to the original,
and checks the rewrapped packet and sample data again before publishing.
No-match cases report why they keep the managed media. UME links are left as
originals.

The first profile pairs progressive, square-pixel 1080p23.976 DNxHD LB (1253)
and ProRes 422 HQ MOV with matching full extent, a verified non-drop file
origin, and known zero source offsets. Stereo requires 48 kHz, two discrete Avid
mono routes and at most 8,192 picture frames. The camera original and both
siblings must use the same PCM depth, either 16 or 24 bits, and each saved Avid
descriptor must agree with its physical sibling. Wider and independently synced
layouts remain outside this profile. Historical picture identity after an
in-place replacement is unverified. Existing proxy outputs are refused,
including dangling symlinks.

Avid's saved camera path is a lookup hint. By default, Premiere uses the
media that backs the Avid edit as its main media. This includes supported
managed MXFs linked in place and files made by **Rewrap to native clips**.
A saved UME link remains a linked original. The proxy option is an explicit
choice and requires the camera file at conversion time. The converter checks
the saved locator or explicit root mapping, embedded source timecode, exact
rate and extent, and the supported stream layout. For stereo rewrap it also
compares both camera channels with the Avid siblings before attachment. A
failed match keeps the selected Avid media policy and reports the reason.
If the selected mode cannot write the Avid media as primary, Review blocks
conversion and points to the camera match failure.

### Native proxy results

The converter-generated picture-only projects kept the existing MXF attachment
and the 24-frame edit from frame 7 after native reopen in both editors. The
stereo fixture kept two trims beginning at file frames 5 and 59, each 24 frames,
on V1/A1/A2. Premiere 27.0.0 build 88 reopened the Save As project with the
attachment and rendered 96,096 exact PCM samples on each isolated lane with
proxies toggled either way. Resolve Studio 21.2.0 build 00006 retained the
attachment and both mono selectors after save, archive export, close and
reimport. Both isolated renders contained the correct 96,096 samples in order
under Resolve's center mix and short edit-edge ramps, and were byte-identical
to the same project with the proxy detached. Resolve's default export did not
prove which file supplied the audio when the proxy was attached.

A controlled PCM24 variant used the same saved Avid trim and package bindings
with private audio fixtures containing distinct left/right PCM24 signals.
Direct CLI and both saved-job paths preserved 96 compressed picture packets and
all 192,192 samples per source channel, including nonzero low bytes. Premiere
27 Save As/reopen kept the original as primary, both proxy audio selectors and
the V1/A1/A2 trims at frames 5 and 59 for 24 frames each. Both proxy toggle
settings rendered the exact 96,096 expected samples per lane. Resolve kept the
same paths, trims and assignments after Save Project As, close and reopen. With
the original online, both isolated lanes matched the detached-proxy render
byte for byte under Resolve's center mix and edit-edge ramps. The PCM16 control
was replayed through both editors.

With the camera original offline before import and throughout Save Project As,
close and reopen, Resolve displayed proxy picture on both sides of the cut.
The trims and mono assignments survived, but both isolated 96,096-sample
exports were silent. Resolve's own proxy attachment produced the same silent
exports. Restoring the original or using the same proxy MOV as the primary
file restored the expected PCM on both lanes.

When proxy rewrap is enabled, the internal Avid → Resolve writer now keeps
the camera item and attached proxy on V1 and binds each audio edit to its
verified Avid mono OP-Atom MXF. The camera file must be online during
conversion so its clock and channels can be verified. With it offline
after conversion, Resolve Save Project As and reopen kept the two 24-frame
edits starting at source frames 5 and 59 on all three tracks. All 48 sampled
PCM24 timeline frames matched the expected proxy frames. Each isolated lane
matched its online reference across 96,096 sample frames after Save Project
As and again after DRP reimport. The PCM16 regression also passed offline
import, picture samples and isolated audio.

Resolve Studio 21.2.0 build 00006 also completed an offline Deliver export
from the reopened PCM24 project. Under **Video → Advanced Settings**, enable
**Use proxy media**. Resolve still displayed **Add With Offline Clips?** when
queuing the export. The exported QuickTime contained all 48 expected proxy
frames at 23.976 fps, covering source frames 5–28 and 59–82. Each isolated
mono lane matched its 96,096-sample 48 kHz PCM24 reference exactly. With
both lanes enabled, the exported stereo bus matched their summed samples
within one 24-bit code. The test used a stereo bus export and two isolated
passes.
[Blackmagic’s proxy delivery guide](https://documents.blackmagicdesign.com/SupportNotes/DaVinci_Resolve_17_New_Features_Guide.pdf#page=21)

The same reopened project also passed a single QuickTime Deliver job with
**Render as discrete audio tracks** enabled and **Timeline Track** 1 and 2
selected. The file contained one ProRes 422 HQ picture stream and two separate
48 kHz PCM24 mono streams. All 48 decoded picture frames matched the prior
Deliver render exactly. Both mono streams matched the trimmed source MXFs at
unity outside Resolve's 72-sample edit-edge ramps. Within the ramps, the
samples matched the linear envelope within one 24-bit code. Both streams also
matched their respective isolated bus references within one code after applying
the measured −3 dB center-pan attenuation. The camera original remained
offline throughout.

The converter's proxy rewrap requires 48 kHz on the original and both managed
siblings. A 44.1 kHz stereo MOV played as a primary source in the Resolve
control and kept the two trims and channel assignments after reopen. The
converter rejected it as a camera original for the 48 kHz managed pair.
Supporting that pairing requires an explicit sample-rate conversion policy and
verification of edit boundaries and audio phase.

### What triggers rewrap in Premiere

| Retained media | Premiere evidence | Routing decision |
| --- | --- | --- |
| 23.976 DNxHD LB + PCM16; 29.97 DF DNxHD LB + PCM24 | These native OP-Atom controls played and rendered after reopen. The tested 23.976 DNxHD picture also attached directly to a picture-only original. | Keep qualified direct links. Rewrap the verified 23.976 proxy only when the stereo layout requires combining its siblings. |
| 23.976 DNxHR LB + PCM24 | The qualified direct construction plays in Premiere. Some earlier writer constructions had incorrect media descriptors or audio routing. | Do not infer a decoder failure from a bad project construction. Keep the existing qualified direct route. |
| 29.97 synthetic DNx sample | Premiere displayed “codec missing.” The succeeding native DNxHD drop-frame fixture does not qualify this synthetic sample or 29.97 DNxHR. | Leave this unsupported shape unqualified. Drop-frame display is not evidence of a decoder failure. |
| Native 24 fps ProRes LT OP-Atom | Premiere's own import of the retained anamorphic sample hit frame-substitution recursion. A MOV stream copy played in that trial. Separate square-pixel rewrap controls passed Premiere 27 reopen and render. | Review and the Avid CLI direct managed ProRes LT to Any NLE's MOV rewrap. Rewrap still checks raster, pixel aspect, index, and PCM. The retained 16:15 sample remains outside automatic rewrap qualification. |

Premiere-assisted UXP linking is for verified original files. A managed
ProRes LT MXF stops linked preparation with the affected clip and a rewrap
action. **Rewrap to native clips** or `--rewrap-media NEW_DIR
--rewrap-native-master` uses the existing stream-copy path, with a new
media folder selected explicitly.

The 16 KiB all-zero item exposed by FFmpeg after the last DNxHD frame can be
present even when Premiere plays the native MXF normally. The stereo proxy
writer excludes exactly that observed terminal item after verifying its
position, size, and digest. It does not use this check to trigger rewrap.
[Native direct-link results](SPLIT-EDIT-AUDIO-2026-11-06.md#23976-direct-dnxhd-with-pcm16)
