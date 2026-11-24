# Managed media for Avid destinations

The shared destination preview is available through `any-nle-gui --avid-destination premiere`
or `any-nle-gui --avid-destination resolve`. The Project page picks the source
project and Avid project folder. Media offers **Link originals**, **Create
Avid media for unsupported originals**, and **Create Avid media for all
selected originals**. For managed media, choose a media drive separately
from the Avid project folder. Review shows the proposed numbered folder,
available space, presets, and indexing step. The internal project conversion
probe is available below. The wizard is still a preview. A saved extension
alone cannot establish whether Avid can link an
original. Classify UME availability using actual provider and source evidence.

## Media format choices

The shared encoder uses the bundled FFmpeg DNxHD encoder's DNxHR profiles.
It stages one MXF OP-Atom per picture or selected mono channel. Start with
DNxHR SQ for routine 8-bit work, LB for smaller editorial media, and HQX
for 10-bit 4:2:2. PCM24 at 48 kHz is the audio default, with PCM16 an
explicit choice. Ten-bit sources require HQX in the first profile.
Media Composer uses unified Avid DNx labels when choosing its own
transcode targets. The stage names the actual DNxHR bitstream it produces.
[Avid 2025.12 codec naming](https://resources.avid.com/SupportFiles/attach/Media_Composer/2025.12/Media_Composer_v2025.12_ReadMe.pdf)
· [FFmpeg 8.1 MXF muxers](https://ffmpeg.org/doxygen/8.1/mxfenc_8c.html)

The first admissible inputs are square-pixel progressive HD or UHD SDR
picture at 23.976, 24, 25 or 29.97 fps with an exact saved frame clock, and
independently selected 48 kHz PCM16/24 channels. Review for native Avid
qualification should cover same-raster links before introducing resize,
interlace, HDR, RGB, alpha, mixed or variable rates, or audio resampling.
Formats that UME cannot open can still be candidates if the bundled
backend can decode them and establish their clocks. AAC, MP3, FLAC,
44.1 kHz audio, phone VFR, and additional native codecs need source and
output policies before admission. Avoid choosing a fallback by extension.

## Drive, packages, and database handoff

On a standalone media drive, plan a fresh `Avid MediaFiles/MXF/<number>`
under the selected volume root. The preview is read-only. The media probe
builds the same relative layout under its own new staging directory. Its
manifest is outside the numbered scan folder. The probe never edits an
existing folder or database.

The MXF muxer assigns a material-package UMID and a file-package UMID to
each staged file. The stage cross-checks the file package from the MXF
header against the essence stream and records the material package, source
item, stream or channel, input clock, duration, and output filename in
`anynle-avid-media.json`. Separate picture and sound files have distinct
package IDs. The manifest preserves which files belong to the inputs of
one master clip. The internal AVB writer uses those IDs to build and
verify the master and source relationships.

The index plan names `msmMMOB.mdb` and `msmFMID.pmr` for Media Composer
to create. On a standalone drive, Media Composer can scan the new numbered
folder at launch or via **File → Media → Refresh Media Directories**.
The stage creates no placeholder database files. Shared-storage folder
naming and indexes require their own evidence.
[Avid database refresh](https://kb.avid.com/pkb/articles/en_US/Knowledge/Refreshing-Media-Databases)
· [Avid indexing of externally transcoded media](https://kb.avid.com/pkb/articles/en_US/Knowledge/import-dailies-into-MC)

### Native indexing pilot

Media Composer 25.12.0 build 58863 indexed a fresh macOS test volume
containing 1080p24 DNxHR SQ picture and two 48 kHz PCM24 mono OP-Atom MXFs.
Both PMR sections matched the three filenames and their file and material
package UMIDs. The MDB's active object list contained three independent
material-to-file-to-source chains, each with the expected picture or mono
track, 12-frame duration, and `01:23:45:06` source start. The two MXFs
each contained 24,000 audio samples. Indexing preserved the MXF bytes.

The saved database represents three separate media items. Media
Composer's Media Tool listed all three and loaded the indexed picture
at the expected source start.

### Reusable managed master pilot

A private AVB prototype uses the stage manifest to bind one master
to all three files. V1, A1, and A2 each reference track 1 of their own
file package. A2 remains track 2 on the master. The bin also embeds the
file and upstream source mobs and the file descriptors and locators
observed in the native index. A package-reference-only control opened
offline on its first load. Media Composer subsequently saved that bin
with the referenced source mobs materialized from its index.

The prototype with embedded source records saved and reopened in
Media Composer 25.12.0 build 58863. It played from `01:23:45:06`
through `01:23:45:17`. A 24 fps ProRes export had all 12 source
frames in order and two independent 48 kHz PCM24 mono tracks. Each
track matched all 24,000 samples from its selected MXF byte for byte.
The source package links and both PCM descriptors also survived the
native bin save.

### Converter-written managed master

`avid-media-probe --master-bin STAGE.json INDEXED_MXF_FOLDER NEW.avb [FFMPEG FFPROBE]`
builds one AVB master for the verified 1080p24 DNxHR SQ profile with two
equal-length PCM24 mono MXFs. Supply the stage manifest and the numbered
folder on the mounted media drive. Media Composer indexes that folder.
The writer verifies the files' package IDs, clocks, raster and essence
extents against the stage facts and uses the manifest's lane order.
It builds the source records from verified MXF facts and the qualified
Avid profile. The generated bin carries the shared source relationships
and does not consume the MDB or PMR.

On fresh MXFs, the converter-written AVB opened with picture online at
`01:23:45:06`. After a native save, close, and reopen, a 24 fps export
matched the order of all 12 picture frames and all 24,000 samples on
each PCM24 lane. The saved AVB retained both distinct mono package links
and their common source clock. The writer places referenced objects
before their owners and the bin root last. A structurally readable
trial with the root before its clips displayed an empty bin in Avid.

The internal command is available independently of full Premiere or
Resolve project conversion.

### Native timeline insertion

Media Composer inserted the generated master into a new 24 fps sequence
with separate V1, A1 and A2 edits. Each edit initially used the full
12-frame source. After independent trims, the saved V1 range was
`[2, 8)`, A1 was `[1, 7)`, and A2 was `[4, 10)` in source and record
frames. The native bin kept the same master and lane references, the
`01:23:45:06` source start, and the `01:00:00:00` sequence start.

After closing and reopening the project and bin, the export matched six
source picture frames in order and four black filler frames. Each
48 kHz Direct Out PCM24 mono track matched all 20,000 expected samples
byte for byte, including 12,000 from its own MXF and 8,000 of silence.
Wider media profiles and standalone database generation remain separate
checks.

### Native original-link pilot

A private test assembled one sequence over two Avid-authored linked
masters. Media Composer 25.12.0 build 58863 saved and reopened the bin,
then exported the sequence. Each master references a file package and
that package references an upstream physical source. The 1080p24 ProRes HQ file
package stores a `CDCI` descriptor. The 48 kHz stereo PCM24 WAV stores
one `PCMA` descriptor with 96,000 samples and two channels. Both file
packages have an `MSML` locator with their own package ID. The full
original path appears on the nested physical descriptor and on the
upstream source descriptor. Both copies agree. The reopened WAV
master's `TapeID` retains the WAV filename.

The two WAV master tracks share the file package. Their saved channel
IDs and source track IDs are 1 and 2. In graph/2 these correspond to
stream 0, channels 0 and 1, with independent edit ranges. The picture
source starts at `03:04:05:06`, the WAV source at `00:00:00:00`, and
the sequence at `01:00:00:00`.

| Track | Record frames | File-relative source frames |
|---|---|---|
| V1 | `[2, 8)` | `[9, 15)` |
| A1 | `[1, 7)` | `[7, 13)` on WAV left |
| A2 | `[4, 10)` | `[13, 19)` on WAV right |

All six decoded 10-bit picture frames matched the original. The other
four frames were black filler. Both Direct Out PCM24 lanes matched all
20,000 expected samples, including silence. The saved-bin comparison
also detected a substituted same-name locator, a one-frame change to
the source clock, and a changed audio selector in test variants.

This pilot established the source records used by the graph/2 writer.

### Graph/2 replay with a native link donor

`avid-media-probe --linked-bin GRAPH.json SEQUENCE_ID DONOR.avb NEW.avb [FFMPEG FFPROBE]`
is an internal writer for one neutral 1080p24 timeline. It expects one
flat bin, one ProRes HQ MOV, and two independently selected channels
from a 48 kHz stereo PCM WAV. The donor supplies the native masters
and their file and physical source records. File paths, provider IDs,
source clocks, stream extents, and any supplied reel must agree.
Graph/2 supplies the record start, edit ranges, source offsets, and
physical channel selections. The writer creates a fresh sequence in
the output bin and reads it back before publishing. The original
media and the input bin are read in place.

The PCM24 control wrote a sequence starting at `01:00:00:07`, seven
frames later than the donor. Its graph supplied an eight-frame V1
source in and an item-to-file offset of one frame. The saved edit
selected source frames `[9, 15)`. Avid rewrote the bin during a
disposable rename and reopened it with the same source clocks, paths,
and three independent trims. The export matched all ten decoded
picture frames and all 20,000 samples on each mono lane against the
native control.

### Generate native links from verified originals

`avid-media-probe --linked-originals GRAPH.json SEQUENCE_ID NEW.avb [FFMPEG FFPROBE]`
generates the source records for the same neutral 1080p24 graph profile
using originals on mounted macOS media volumes. The picture original
must be a ProRes 422 HQ MOV at Rec.709 video levels with an exact
embedded timecode track. The separate stereo WAV must use 48 kHz
PCM16 or PCM24 with a zero source start. The writer verifies the
physical streams before creating fresh native masters, file packages,
and physical sources. It preserves independent channel selections
and file-relative edits. The probe reads the staged AVB back and
publishes it to a new path.

Media Composer 25.12.0 build 58863 saved and reopened the generated
PCM24 bin. Both original paths, the source clocks, the WAV reel, and
all three trims survived. The export at `01:00:00:07` matched all 10
decoded frames and 20,000 PCM24 samples on each discrete mono lane
against the donor-backed control.

### Replay saved Premiere and Resolve projects

`avid-media-probe --linked-project INPUT SEQUENCE_ID NEW.avb [FFMPEG FFPROBE]`
accepts a saved Premiere `.prproj` or exported Resolve `.drp` on macOS.
Use the exact saved sequence ID. The readers pass cuts, clocks, linked
paths and physical selectors through graph/2 to the same native AVB writer.
This profile writes one bin with V1 and two independently selected mono
edits from a linked stereo WAV. It checks for neutral source mixers before
publication. It admits one saved Resolve timeline and a selected Premiere
sequence, including Premiere's default empty track declarations. Resolve
originals use file-state and stream verification without full-file hashes.

The Premiere 27 Save As and reopened project and the independently
saved Resolve export each produced a native Avid bin. Media Composer
25.12.0 build 58863 saved and reopened both with their originals online.
At `01:00:00:00`, V1 selects source frames `[4, 28)`. Both audio
lanes use source frames `[12, 36)`, one from each WAV channel.
All 24 decoded frames and all 48,000 samples on each Direct Out PCM24
lane matched their original references in each route. The wizard
publication and wider editing profiles remain open.

### Replay AVB controls from graph/2

The internal `avid-media-probe --roundtrip-avb REQUEST.json [--review]`
accepts an object with `graph`, `donor`, `output`, and `owners`. Paths are
resolved by the caller's working directory. The owner list uses exact
graph IDs: the Group ID for a reusable Group marker, the marker's
`owner_occurrence` for a Group edit copy, the clip item ID for auxiliary
clocks, or a saved Timewarp occurrence's `owner` in `avid.timewarp`.

- `avid.group` preserves markers on the original output selector, with
  separate edit-owned copies in the sequence. The writer restores one
  Green V1 point per selected selector with an existing matching marker
  slot. It verifies branch identities, lane, clock, interval and trim.
- `avid.aux_clocks` preserves secondary `TCCP` and populated film
  `ECCP` records on their import MobID, with track, span, exact rate,
  and the saved import offset. The writer applies the raw counts and
  flags to matching donor segments. This command qualifies the written
  AVB structure. Treat edited clock values as pending native verification.
- `avid.timewarp` retains native controls for qualified saved maps.
  The writer replays two positive Linear Speed keys and the saved
  half-frame Position data in a matching exact-24 progressive donor.
  It re-extracts the staged effect and checks the frame map as well as
  the controls. A different key layout or a saved precompute refuses.

Review validates the same staged bin in memory and reports selected
owners. Publication requires a new path, unambiguous donor references,
and verified linked Avid-managed MXF packages. The donor supplies native
structure and the graph supplies the selected control values. Graph-only
construction is a separate qualification.

In Media Composer 25.12.0 build 58863, the reusable Group point at
frame 37 and the separate point on a trimmed Group edit at record
frame 17 retained their changed names after save and reopen. The saved
Group bindings and all 16 picture and audio placements agreed with the
graph. The changed 24 fps Timewarp reopened with editable 200% and 50%
Linear keys. The two reopened effects retained all 96 saved frame
selections. The native export produced an incomplete MOV, so rendered
frame comparison remains open.

A native save established that, on the qualified 24 fps picture
master, secondary tracks 3 and 4 held the two Auxiliary TC columns.
Staged read-back confirmed changed counts for both, while a same-session
Avid save restored the original values. Whether the altered values
survive a fresh session remains open. Synthetic film edgecode survived
staged read-back, and Avid removed it when saving the separate synthetic
clock fixture. Sound TC needs a native audio donor.

### Write editable point markers from graph/2

The internal `--linked-originals` probe writes owner-level point markers
on the new picture master and the new sequence. It accepts whole-frame
positions on the 24 fps owner clock and explicit Green, Red, and Blue
palette entries. Picture markers use the saved item-to-file offset once.
Sequence markers are relative to the sequence start. The writer refuses
unsupported ownership, durations, colors, and timing before publication.
It allocates each marker's attributes, `TMBC` record, and `TMCS` list
before the owning composition and checks that order in the staged bin.
[Pinned pyavb marker reader](https://github.com/markreidvfx/pyavb/blob/ff64e06b60077ecabe5db3fef239898b08363898/src/avb/misc.py#L817)

Media Composer 25.12.0 build 58863 showed the green picture marker at
`03:04:05:17` and the blue sequence marker at `01:00:00:12`. Both names
were editable in the Markers tool and persisted through save, close, and
reopen. The independent pyavb read recovered both owners, positions,
colors, comments, and authors from the generated bin and the Avid save.
Avid displayed `?1` in the Track column for both owner-level markers
and assigned its own IDs after editing and saving them. Native project
capture of markers into this profile and ranged and track-specific
Avid output remain open.

## Internal media-only probe

`avid-media-probe --plan-drive VOLUME` inspects the proposed target.

`avid-media-probe --stage REQUEST.json NEW_STAGING_FOLDER [FFMPEG FFPROBE]`
encodes the explicitly selected source streams with a fresh output path.
A request includes the intended Avid edit rate and source timecode. These
become the new files' timecode. The input PTS facts travel separately in
the receipt. Stream and channel ordinals are zero-based. The file package
ID and new codec describe the staged media, while the original path and
optional source item ID remain available for linking and verification.

```json
{
  "source": "/path/to/original.avi",
  "source_item_id": "source-item-id",
  "edit_rate": "24",
  "timecode": "01:23:45:06",
  "video_stream": 0,
  "audio_channels": [{"stream": 1, "channel": 0}, {"stream": 1, "channel": 1}],
  "picture_profile": "sq",
  "audio_profile": "pcm24"
}
```

The stage checks source clock continuity, checks the staged essence and
timecode, and rechecks the original before publishing the receipt. If a
check fails after encoding begins, the private staging files can remain
for inspection without a completed manifest.
