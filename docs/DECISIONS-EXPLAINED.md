# Any NLE: why it works the way it does

Plain-language reasoning behind each import option and conversion decision, written so it can
be shown in the app (tooltips, the options dialog, the import report) and in the help. Each
entry: what the user sees, what Any NLE does, and why. Technical evidence lives in the
research repo (finding IDs in brackets) and `research-dependencies.json` (CONV IDs).

Status: started 2026-10-14. Entries marked *(planned)* describe agreed behaviour not yet built.

---

## How Avid and Premiere treat media differently

Most of the choices below follow from a few basic differences:

| | Avid | Premiere | What it means for the conversion |
|---|---|---|---|
| **One clip, many files** | A master clip ties together separate media files: one video file and one file per audio channel (MXF OP-Atom: `V01`, `A01`, `A02`…). | Imports each file as its own clip. | Any NLE rebuilds Avid's single clip, as a group, a merged clip, or separate linked clips (your choice). |
| **Finding media** | Media is managed: clips find their files by ID through Avid's media database, wherever the files sit. | Clips point at a file path. | Any NLE matches each Avid clip to its files by the IDs inside the MXF headers, then writes plain file paths. |
| **Clips vs. bins** | The same master clip can live in several bins. | Each clip lives in exactly one bin. | A copy is made in every bin it appears in. |
| **Audio channels** | A clip's audio files can be grouped as stereo; sequences have dedicated mono, stereo and 5.1 tracks. | Tracks accept mono and stereo clips side by side; a stereo clip must come from one source carrying both channels. | Avid stereo pairs become one stereo clip, built so the two mono files play left and right. |
| **Clip settings** | Settings such as gain live on the master clip and apply to every use. | Settings live on each clip and each edit; a master clip's settings are copied into new edits. | Any NLE writes the setting on the master clip *and* on each edit. |
| **Timecode and frame rate** | Every clip, including audio-only ones, is counted at the project frame rate. | Audio-only clips have no frame rate of their own and are counted at a preference rate (23.976 by default). | Audio-only clips are given Avid's frame rate so their timecode reads the same as in Avid. |
| **Metadata** | Bin columns, including custom ones, are stored in the bin. | A fixed set of logging fields plus a few custom ones. | Avid columns are mapped to the closest Premiere fields; the rest go into the Log Note. |
| **Original media** | Usually works from transcoded MXF copies of the camera files, remembering the original Source File. | Can attach a proxy and a full-resolution file to the same clip. | *(planned)* The Avid MXF can come in as the proxy, with the original source file as full resolution. |

---

## One translation hub for every program *(built for Avid -> Premiere; more programs planned)*

Any NLE does not translate Avid straight into Premiere. It first describes your project in its
own neutral terms (clips, bins, edits, audio levels, markers, effects), then writes that
description out for the target program. Each new program then only needs to be taught once, in
each direction, instead of once for every other program it might talk to. Choices that only make
sense for a particular pair, such as which Premiere label an Avid clip colour becomes, are kept
separate, in the settings you see in the import wizard. Anything that cannot be carried across
is listed in the import report with the clip or edit it belongs to. [ADR-0004; GRAPH-MODEL.md]

---

## Import options

### How clips are built: OP1a rewrap, Grouped, Merged, or Separate

**What you choose.** In Avid, one master clip can hold a video file and several audio files
(OP-Atom: `V01`, `A01`, `A02`…). Premiere imports each of those files as its own clip. Any NLE
can present those sources in four ways. The native Avid → Premiere wizard uses this
order, with **OP1a rewrap selected by default** for new or reset settings (User,
2026-11-05). An explicitly remembered choice or preset remains selected.

| Option | You get in the bin | Good for | Trade-off |
|---|---|---|---|
| **1. OP1a rewrap — default** | one native file-backed AV master per Avid clip, using a new OP1a MXF | keeps supported bin audio groups and channel assignments together in ordinary clips | **Creates new media.** Requires an output folder, additional disk space and supported media |
| **2. Grouped clips** | one multicamera clip per Avid clip, referencing original files | keeps video and all audio together; audio channels stay switchable; can carry stereo | it is a multicam clip: some clip-level tools behave differently |
| **3. Merged clips** | one merged clip per Avid clip, referencing original files | the familiar "merged clip" workflow | Premiere drops most metadata on merged clips (User report) and **cannot carry stereo** (Adobe: merging breaks audio into mono channels) |
| **4. Separate files** | the original video and each audio file as their own clips | simplest; nothing hidden | on the timeline, picture and sound are separate (linked) clips |

The native wizard exposes all four choices. Its OP1a choice selects both media rewrapping
and native AV-master construction; it is not rewrapping under Grouped/Merged construction.
Current CLI commands still require explicit rewrap flags. Selecting the wizard default
must still lead through the folder choice and media review before writing anything.

Notes shown with the choice:
- The native rewrap route preserves [qualified two-camera Avid Groups](EDITABLE-GROUPS-2026-11-08.md)
  as editable multicam bin items and timeline edits with fixed mono audio. Other
  Group layouts need separate qualification and refuse with a reason.
- Qualified linked-original clips use ordinary file-backed masters. Descriptor,
  codec and path limits still apply; general AMA compatibility is not established.

### Rewrap Avid OP-Atom as OP1a MXF *(CLI and macOS folder prompt implemented; wider qualification pending)*

**What you choose.** The first, default choice in the native Avid → Premiere clip
menu. The option and conversion review must display **“Creates new media.”** The CLI
shows it before preparation and remains explicitly opt-in.

**Folder choice.** In the CLI, `--rewrap-op1a-prompt` opens a native macOS folder picker.
Choose a parent location; the review shows the new subfolder, **“Creates new media.”**,
estimated additional space and available space. Cancel creates no outputs. Explicit
`--rewrap-op1a NEW_DIR` remains available for scripts. The native wizard offers its own
folder selection and pre-flight review.

**What Any NLE does.** Writes new OP1a MXF files in your chosen output folder from the
qualified Avid media set, then links the Premiere project to those files. The original
Avid media stays in place. Rewrapping is intended to preserve the existing media without
re-encoding; unsupported inputs must be reported, with no silent transcode. Multiple
OP-Atom files must be joined from verified identities and source tracks, not filenames.

**Why.** It offers a file-based alternative to rebuilding picture and sound from separate
OP-Atom sources. Creating the new files takes additional disk space and processing time;
the review shows the destination, estimated size and unsupported items before writing.
It is separate from Grouped/Merged/Separate clip construction. The explicit experimental
`--rewrap-native-master` flag instead creates ordinary file-backed AV masters preserving
supported bin groups/routing; it replaces bin mode for that run. Rewrapping does not promise that
changing the wrapper fixes every Premiere decoding problem. Channel mapping, sync,
metadata and wider native Premiere profiles still require qualification. Bounded audio
routing, saved bin groups and fresh bin insertion have agent-operated checks; see
[NATIVE-OP1A-MASTERS.md](NATIVE-OP1A-MASTERS.md). The CLI supports 20 exact MXF rates, including 23.976, for
DNxHD/DNxHR 8-bit 4:2:2 picture with up to 32 separate mono 48 kHz PCM 16/24-bit
sound files. Extended rates use a reported frame-count display. Other audio types
are refused rather than converted implicitly. See
[current use and limits](MEDIA-REWRAP.md).

### Where things go in the bin: builds and parts

**What you see.** The top level of each converted bin holds the clips you edit with ("builds"):
grouped or merged clips, stereo audio clips, and clips that needed no assembly (video-only,
mono audio-only). The files they were built from ("parts": the separate video and audio files)
go into a **Processed Clips** subfolder inside the same bin.

**Why.** Premiere itself moves the sources of a multicam clip into "Processed Clips". Keeping
the parts next to the clips built from them, per bin, keeps large projects navigable when the
Avid folder structure is recreated, and keeps the top level matching what an editor used in
Avid: one item per Avid clip.

### Stereo (built in all three clip modes)

**What you see.** A clip Avid marks as a stereo pair plays as one stereo clip in Premiere, on
the same track number it had in Avid. Clips Avid treats as mono stay mono.

**What Any NLE does.**
- Reads the pairing from the Avid bin itself (the clip's channel grouping), never from file names.
- Grouped: the multicam clip is built so its two audio files play left and right. [PRPROJ-0019]
- Separate (and any clip without picture): the pair becomes a small nested stereo sequence (left
  file panned fully left, right file fully right) and edits use it; the picture stays its own
  clip, linked on the timeline. [PRPROJ-0022: a multicam without picture made Premiere go silent]
- Merged: Premiere cannot merge into stereo, so a stereo pair is built as a group instead, and
  the import report says so (User decision 2026-10-14).

**Why.** Avid has dedicated stereo tracks; Premiere tracks accept mono and stereo clips side by
side, so a stereo edit needs one stereo clip, not two tracks. Premiere cannot make a stereo
clip by merging two mono files (Adobe-confirmed), but a multicam-style clip can, and the same
media can be stereo in one Avid clip and mono in another; each keeps its own setting.

### Include Trash and Avid Attic

A checkbox, off by default. Avid keeps deleted bins in `Trash/` and
automatic backups in the Attic; most imports do not want them.

### Video as proxies; link to source files *(planned)*

Avid video media (MXF) is usually a transcode of the original camera file. By default Any NLE
imports the Avid video as the **proxy** and links the original **Source File** as the
full-resolution media when it can be found. A separate option, **Link Audio to source file (if
found)**, does the same for audio.

---

## Conversion decisions (automatic)

### Names and metadata [PRPROJ-0015; CONV-0014]

Clip names come from the Avid bin's Name, not file names. Avid columns land in the matching
Premiere fields: Camroll -> Tape Name, Take -> Shot, Comments -> Comment, Source File ->
Description, Scene -> Scene; everything else goes into Log Note as one line ("Column: value |
…"), because Premiere does not show multi-line Log Notes.

### Timecode [PRPROJ-0016, PRPROJ-0017; CONV-0015, CONV-0016]

Sequences keep Avid's start timecode. Each media file keeps the start timecode stored in it.
Audio-only clips are shown at Avid's frame rate (e.g. 24 fps): otherwise Premiere counts them
at 23.976 and a 01:00:00:00 start would display as 00:59:56:09.

### Audio length taken from the media file [PRPROJ-0018; CONV-0017]

Avid's bin can record an audio file one frame shorter than the file actually is. Any NLE uses
the length stored in the file, as Premiere's own import does; using Avid's shorter value made
Premiere play the clip silent.

### Speed [PRPROJ-0024; CONV-0023; CONV-0051]

Constant speed changes and reverse from Avid (Motion Effect) become Premiere's clip speed and
Reverse Speed. The legacy constant-speed route uses the stored span ratio. A clip Avid
labels 200% can display as 198% in Premiere because the saved ratio describes the
whole source span, including its final sampled frame. Matching that cadence remains open.

For the qualified 1080p24 Linear and Spline ramps, Any NLE uses Avid's saved picture
positions to build Premiere Time Remapping with one editable key per frame. The
report identifies the change from Avid's original Speed keys and handles. A recognized
sampled Timewarp with an unsupported or inconsistent map blocks conversion because
using a constant speed would change the picture cadence.

### Clip color [PRPROJ-0025; CONV-0024]

Avid stores any RGB color on a clip; Premiere offers 16 label colors whose exact shades are
set in each user's Preferences. Any NLE picks the nearest Premiere label for each Avid color,
comparing hue first and lightness less (Avid's swatches are pastel, Premiere's labels dark).
All 32 Avid palette swatches have a default; custom colors are approximated and listed in the
report. The **color mapper** step of the import lets you change any mapping (User 2026-10-15).

### Marker color, name and comment [PRPROJ-0025; CONV-0024]

Avid saves each marker's color by name (16 names); Premiere has 8 marker colors. Defaults:
Red, Green, Blue, Cyan, Magenta, Yellow, White, Orange -> same name; Pink -> Magenta; Forest ->
Green; Denim -> Blue; Gold -> Yellow; Violet -> Magenta; Purple -> Blue; Black and Grey ->
White (Premiere has no dark marker color). All changeable in the color mapper (User
2026-10-15). Premiere's "Magenta" marker is a light lavender (User read-back), the nearest it
has to Avid's magenta and pink; the mapper shows swatches, not names. Marker name and comment carry over. An option adds the Avid user and original
color name to the comment (e.g. `Avid: BENJI · Grey`), since Premiere markers have no such
fields (User 2026-10-15).

### Where markers go [PRPROJ-0025; CONV-0024] (User 2026-10-15)

In Avid, a master clip's markers are copied into a sequence when the clip is edited in, and
markers can then be added or changed on the timeline for that edit only. In Premiere a clip's
markers belong to its source: every use of the clip shows the same ones.

| Avid marker | In Premiere |
|---|---|
| on a master clip or subclip | a marker on the bin clip, so every use shows it |
| copied into a sequence from the source | nothing extra: the source clip already shows it |
| added on a timeline clip | a marker on that edit: the edit uses its own subclip of the same media, carrying the source's markers plus its own |
| on filler in a video track | a one-frame adjustment layer carrying the marker (an empty adjustment layer changes nothing) |
| on filler in an audio track | a sequence marker (adjustment layers are video-only) |

When an edit's own marker and a source marker sit on the same frame, the edit's marker wins by
default, as in Avid's timeline. Options: **Source markers win instead**, and **Keep the other
marker as a sequence marker** rather than dropping it.

### Color correction [PRPROJ-0026, PRPROJ-0027; CONV-0026]

Avid's Color Correction effect becomes a **Lumetri Color** effect on the same clip carrying one
**LUT** (a colour lookup table) that reproduces the whole Avid correction: hue, saturation,
brightness, contrast, gain, gamma, setup and the four curves. The LUT is stored inside the
Premiere project, so it works even if the file is moved; a copy is also saved as a `.cube` file
in a `<project> LUTs` folder next to the project, for reuse in other apps.

Why a LUT and not Lumetri's own sliders: Lumetri's Saturation, hue and master curve use different
maths from Avid (a red at saturation 50 came out 207,100,100 in Lumetri, 155,27,27 in Avid). A LUT
computed from measurements of Avid's output matched Avid within about one level on a test chart.
Trade-off: the correction is not editable as Lumetri sliders; adjust on top of it instead.
Accuracy limits are listed in the report: the model comes from one Avid version and test chart;
Avid's slight colour bleed on fully saturated colours under strong gain/contrast is not
reproduced; other Avid tabs (Channels, Levels, Secondary, Hue Offsets) and keyframes are not
converted yet. Premiere 27 keeps the Lumetri Color effect; its new Color mode is a later option.

### Pan [PRPROJ-0023; CONV-0022]

Pans set in Avid are converted exactly. Avid also pans mono tracks by default (odd tracks
left, even tracks right) as a listening convenience while cutting; those clips are centred in
Premiere, Premiere's normal behaviour, and the import report says how many. The option **Match
Avid's default panning** reproduces Avid's sound instead. A stereo clip whose two channels Avid
panned separately (for example 50L / 25R) is listed in the report for now; a Premiere stereo clip
has a single Balance control.

### Clip gain [PRPROJ-0020; CONV-0019]

Gain set on an Avid master clip becomes Premiere's Audio Gain on the master clip, and every edit
of it carries the same gain, as in Avid. Gain changed on an edit in an Avid sequence replaces the
master's for that edit (Avid copies the master's value when the clip is cut in). For stereo and grouped clips the gain sits on the clips
*inside* the build (open the build in a timeline to see it), so edits of the build itself show
0 dB while playing at the Avid level; putting it on both would double it.

### A clip that is in several bins [CONV decision, User 2026-10-14]

Avid lets the same master clip live in several bins; Premiere does not. Any NLE puts a copy in
each bin, each with the attributes saved in that bin. A sequence uses the copy in its own bin,
otherwise the first; the report lists copies that differ.

### Audio that plays silent

Premiere sometimes plays MXF audio silent (the waveform may still show) until it is quit and
reopened; clearing the media cache alone may not help. This is a known Premiere issue, not
specific to Avid or to converted projects. Any NLE avoids the two project-side causes it found
(an audio length shorter than the file, several media records for one file) and its import
report tells you to restart Premiere if converted audio is silent. If a file keeps failing, the
planned "Link Audio to source file" option uses the original audio file instead.
[PRPROJ-0018; research note docs/mxf-audio-silence.md]

### Resize and 3D Warp become Motion [CONV-0027; provisional]

Avid's Resize and the flat parts of 3D Warp (position, scale, rotation around the screen, the
axis it turns about, and crop) become the clip's Motion settings in Premiere, so they stay
editable there: Position, Scale (Uniform Scale off when width and height differ), Rotation,
Anchor Point and Crop. A Resize and the 3D Warp Avid promotes it to give the same Motion, as they
give the same picture in Avid. True 3D parts (depth, tilting around X or Y) have no Motion
equivalent; they are listed in the import report and left out. Keyframed moves are a later step.

### Timewarp becomes Time Remapping [provisional]

Avid stores, for a Timewarp, the source position it will show at every frame. Any NLE uses
exactly those positions: the Premiere clip gets Time Remapping with one keyframe per frame, so
each frame shows the same source frame as in Avid, whatever the speed curve (Linear, Spline,
ramps). Each key sits a quarter frame into its source frame so Premiere cannot round to the
frame before. Speed graph handles are therefore not editable as a smooth curve in Premiere;
the picture timing is what is kept.

### AniMatte becomes masks on Opacity [provisional]

An Avid AniMatte (Key In) shape becomes a mask on the clip's Opacity in Premiere, which shows
the inside of the shape like Avid. A shape moved by Avid's tracker becomes a mask path with a
keyframe on every frame. Several shapes become several masks. Key Out, feathering and shapes
whose number of points changes are reported, not converted yet.

### Keeping stereo edits together through a transition (reliability fix, 2026-10-29)

**What you see.** A stereo edit keeps both channels when an audio transition is converted.
The clips meet at the same cut on both channels. Identical extracted records for one track
transition produce one crossfade, including when a middle edit has fades at both ends.

**What Any NLE does.** Before assembling stereo edits, it applies the existing transition trim
to every matching channel in each edit span. Both writers use the generated stereo source's
actual content duration when checking transition handles. They keep the current Constant Power
audio-fade policy; video transitions still become reported cuts. Mono edits and the available-channel
fallback for missing stereo media keep their existing behavior.

**Why.** Trimming only one channel makes a valid stereo pair look inconsistent and can drop
both edits. Invalid transition timing, a self-referencing boundary, or a trim that would
consume an edit is reported and left out without applying that trim. These are model assembly
checks on extracted records, not new Avid storage rules or new transition shapes. Playback
compatibility still needs application read-back.

### Preserving files beside a project during save (reliability fix, 2026-10-29)

**What you see.** A file, folder or symbolic link already named like `output.prproj.tmp` is
kept, including a graph snapshot saved there. This applies to new project saves and explicit
in-place saves. A successful save leaves no new staging file behind.

**What Any NLE does.** Both writers create the temporary sibling exclusively. If its name is
occupied, they try `.tmp.2`, `.tmp.3` and so on without replacing or following the existing
entry. They finish writing and close that file before renaming it to the project destination.
If writing or renaming fails, they try to remove only their newly created temporary file and
report the original failure. The project's reported path changes only after a successful save.

**Why.** A predictable temporary filename must not overwrite an unrelated file or consume a
saved graph. This changes staging-file handling, not project bytes or media paths. It keeps
the existing final-rename behavior; it does not add a lock for multiple writers targeting the
same project or a transaction across project, LUT and graph outputs. Cleanup is best effort
if the filesystem refuses removal or the process is killed.

### Keeping colour-correction LUT exports distinct (reliability fix, 2026-10-29)

**What you see.** Existing `.cube` files are kept. If a generated LUT filename is already
in use, the new file gets ` (2)`, ` (3)` and so on before `.cube`, and the report names the
chosen export. Two corrected edits at the same timecode, or sequence names that become the
same after filename cleanup, keep separate LUT exports.

**What Any NLE does.** Both writers create LUT files exclusively and use the actual chosen
path in the project's Lumetri effect. Files, folders and symbolic links already occupying a
candidate name are skipped without changing them or following the link. The LUT output folder
itself must be a real directory; a symbolic link there is refused so exports are not redirected
elsewhere. A fresh, unused filename and its LUT values keep their existing behavior.

**Why.** The exported table must stay matched to its edit, and a new conversion must not
replace someone else's table. This changes file allocation, not colour math or editable
Lumetri controls. It is not a multi-file transaction: a later failure can still leave new
LUT files or a partial new file. Existing files are not removed to retry.

### Saving a graph snapshot safely (reliability fix, 2026-10-29)

**What you see.** The optional `--graph` snapshot needs a new file path, just like the converted
project. If either output already exists, is a symbolic link, has a missing parent folder,
or both names refer to the same destination, conversion stops with a named error before
creating either output. Choose a different file name to keep the existing file.

**What Any NLE does.** It checks both output paths before reading the bins and creates the
snapshot exclusively, so a file that appears later cannot be overwritten either. It resolves
parent folders when comparing paths, including relative paths and symbolic-link parents.
Names in the same folder that differ only by letter case are refused on every host to avoid
case-insensitive filesystem collisions. The project and snapshot bytes for valid, distinct
new paths stay the same.

**Why.** A debug snapshot must not replace an original bin, an existing project or another
saved snapshot. These checks protect the selected outputs; they do not make a multi-file
transaction. A later conversion failure can still leave a newly created snapshot for diagnosis,
and a snapshot write failure can leave its newly created partial file. Existing files are not
removed to retry the conversion.

### Selecting the same bin more than once (reliability fix, 2026-10-29)

**What you see.** Repeating a bin path, or selecting the same bin through a symbolic link,
imports it once and reports the repeated selection. Its name and position follow the first
selected path. Separate bin files still import separately, even if their contents match.

**What Any NLE does.** Both CLIs resolve existing input paths for comparison before decoding
bins. They keep the first spelling and selection order, and use that filtered list for bin
names and extraction together. Inputs that cannot be resolved stay in the list so the normal
decoder reports their errors. Sequence inspection applies the same selection rule.

**Why.** Repeated arguments should not duplicate a bin, its clips or its timelines. This does
not merge bins by name, contents or Avid IDs, and it preserves the existing copies-per-bin
policy. It is input selection, not folder discovery, project streaming or a storage rule.

### Reading selected bins with less memory (2026-10-31)

**What you see.** The existing commands accept the same ordered list of bin files. A sequence
can still use a clip from a later bin, and each bin keeps its own copy of a master clip. If any
selected bin cannot be read, the command stops before writing model text, a graph, a project
or its supporting files.

**What Any NLE does.** It reads one bin, keeps the clip facts and supported sequence data needed
for conversion, releases the decoded bin, then reads the next. After reading all selected bins,
it resolves references between them using their full Avid IDs. Supported effects, edits,
diagnostics and their order stay the same.

**Why.** Keeping every decoded bin open used memory for data the conversion no longer needed.
Only one decoded bin is now retained at a time. Memory still grows with the retained project
summary and final output, and one very large bin can still need substantial memory to decode.
This is the foundation for project import; folder discovery, nested folder layout and the
import dialog remain planned. Existing size limits still apply.

### Enforcing the bin input size limit (reliability fix, 2026-10-31)

**What you see.** A bin larger than the existing 1 GiB input limit is refused before its
contents are loaded. A bin that grows beyond that limit while being read is also refused.
Normal bins keep their existing conversion results, and a read failure produces no partial
conversion output.

**What Any NLE does.** The decoded reader, indexed reader and editable-document reader check
and read through the same open file handle. Reads enforce the byte limit independently of
the reported file size, including when that size is zero or becomes outdated. At most one
extra byte is read to detect an overrun; it is never appended to the input buffer.

**Why.** Checking a file's size before opening it did not limit a later read to the end of a
growing file. The editable-document reader also loaded oversized files before rejecting them.
The shared guard closes those gaps. This is an input byte limit; decoding one large bin and
retaining a large project's summaries can still need more memory.

### Loading an editable bin with fewer temporary copies (2026-10-31)

**What you see.** Tools that open an AVB as an editable document need less temporary memory.
Unchanged documents keep their exact original bytes, and edits keep the same validation.

**What Any NLE does.** It keeps one owned original input buffer and moves decoded objects
into the document. It still retains separate original and editable node states, preserved
chunks for unknown data, object IDs and trailing bytes.

**Why.** Loading previously made additional copies of the entire input and its decoded
objects while building the editable document. Those temporary copies were unnecessary.
These savings apply to editable-document loading; ordinary converter extraction uses a
different reader. Large decoded bins, retained document state and project output still
need further memory work.

### Saving an edited bin with fewer temporary copies (2026-10-31)

**What you see.** Tools saving editable AVB documents can need less temporary memory,
especially when large chunks are carried across unchanged. Saved bytes and edit validation
stay the same.

**What Any NLE does.** It copies unchanged chunks directly from their preserved bytes into
the output, then checks the container structure and decodes the result as before. Validation
uses the completed output buffer instead of making another whole-file copy. Preserved source
chunks remain available for later edits and saves.

**Why.** Extra chunk and output copies could add substantial memory during a save. Their
removal preserves the existing checks and refusal rules. Savings depend on the data: decoding
large supported payloads still needs memory. This applies to editable AVB serialization;
ordinary conversion and whole-project memory remain separate work.

### Keeping one original byte image per editable bin (2026-10-31)

**What you see.** Editable AVB documents retain less memory after opening. Original data,
unchanged chunks and edit checks stay available throughout the document's lifetime.

**What Any NLE does.** It reads original chunks from its already-owned file buffer instead
of keeping a second copy of each chunk. A graph copied from another document gets its own
source bytes, so closing that source does not break later edits, saves or further copies.
The original and editable node states remain separate.

**Why.** The retained file already contained every original chunk; storing those bytes again
used extra memory. This removes that duplication without depending on the input file staying
open or unchanged on disk. Decoded node data still consumes memory, and ordinary converter
extraction and whole-project memory remain separate work.

### Sharing decoded node data until mutable access (2026-10-31)

**What you see.** Editable AVB documents need less memory when much of their decoded data
stays unchanged. Original bytes, edit validation and saved results stay the same.

**What Any NLE does.** The original and editable snapshots initially share read-only decoded
data. Requesting mutable access gives that object its own working copy, keeping the original
snapshot intact for validation and rollback. Objects already detached can be edited in place.

**Why.** Loading used to copy every decoded object before any edit was requested. Sharing
read-only data avoids those unused working copies. Editing a large object can still require
a large copy, and saving still validates decoded output. This is the editable-document API;
ordinary converter extraction and whole-project memory remain separate work.

### Validating a saved bin one object at a time (2026-10-31)

**What you see.** Saving an edited AVB document can use less temporary memory when it has
many decoded objects. The saved bytes and validation errors stay the same.

**What Any NLE does.** After checking the output container, it runs the same full-file
object decoder used by normal reading. Each decoded object is released after its checks,
before the next object is decoded. Ordinary reading still keeps its decoded objects and
raw effect data for conversion.

**Why.** Save validation previously built and immediately discarded a second complete bin.
Validation only needs the result of those checks. It still decodes every declared object,
including objects outside the bin's reachable graph, and checks unread payload bytes and
understood reference bounds. One large object can still need substantial memory; the editable document,
output buffer and ordinary project extraction remain separate memory costs.

### Decoding directly into editable bin records (2026-10-31)

**What you see.** Opening an editable AVB can use less temporary memory, particularly when
it contains large raw effect payloads. Original bytes, editable values and saved results
stay the same.

**What Any NLE does.** It decodes objects directly into the document's records, sharing their
original and editable snapshots as before. Original chunks remain in the owned file index.
It avoids building the ordinary extraction reader's temporary object table and raw-effect
copies. Normal reading still retains the decoded and raw data needed for conversion.

**Why.** Editable documents already own the original file bytes and do not use the separate
raw-effect lookup kept by ordinary extraction. Copying that data while opening the document
was unnecessary. The document still retains all decoded nodes and its original byte image;
this reduces loading peaks rather than solving large-bin or whole-project memory use.

### Growing the bin object table after validation (2026-10-31)

**What you see.** A malformed bin's advertised object count no longer causes a full table
allocation before its objects are checked. Valid bins keep their decoded contents and
existing conversion results.

**What Any NLE does.** The ordinary reader starts with its null-reference slot, then grows
the table as decoded or unsupported objects pass their existing checks. Requested capacity
never exceeds the declared final slot count. The decoder, object order and reported errors
stay the same.

**Why.** A file can contain enough bytes to make a large object count plausible while its
first chunk is already corrupt. Reserving the entire table at that point wastes resources
and can fail before the useful parse error. Valid large bins still need storage for their
actual objects, and growing their table can temporarily require extra allocation. This
removes speculative allocation without adding a new size limit.

### Repeated media identities inside one file (reliability fix, 2026-10-29)

**What you see.** A media file stays linkable when its header repeats the same package identity.
If two distinct files carry that identity, it is still reported as ambiguous and not selected.

**What Any NLE does.** The media index counts each complete identity once per scanned file.
It keeps all distinct identities found in the file. Raw header inspection still shows the
extracted reference list; timecode, duration and codec interpretation are unchanged.

**Why.** Repeating a reference inside one file does not make another file. This prevents a
false missing-media result without choosing between competing files or comparing their
contents. It is index bookkeeping, not a new rule for interpreting Avid/MXF metadata.

### Selecting overlapping media folders (reliability fix, 2026-10-29)

**What you see.** Selecting the same media folder twice, or selecting both a folder and a
subfolder inside it, keeps its media linked. The scanned-file count and unreadable-file
report count each file once.

**What Any NLE does.** It resolves selected folder paths and scans an identical folder
selection once. Across overlapping folders, it reads each discovered absolute file
path once. An explicitly selected subfolder still gets its own search depth, so selecting it
can find media beyond the parent folder's depth limit.

**Why.** Finding the same file again through another selected folder is not an ambiguous
media identity. Two distinct file paths with the same identity remain ambiguous, even if
their contents match; Any NLE does not choose one copy or remove files. This fixes selection
handling without changing MXF interpretation or the existing policy for genuinely duplicated
media.

### Missing media (reliability fix, 2026-10-29)

**What you see.** Available clips still convert when another clip has no usable media.
The report names each omitted clip and its unconverted timeline edits.

**What Any NLE does.** It leaves out a clip with no media before handing the model to a
writer. The existing policy for missing audio still keeps the picture and remaining audio;
a missing picture still leaves out the clip. A clip with no media cannot be represented by
the current model/1 writer. Surviving edits keep their record positions. No offline
placeholders are created; an omitted final edit may shorten the resulting sequence.

### A stereo channel has no media (reliability fix, 2026-10-29)

**What you see.** When one side of a stereo edit has no media, the available side still
appears on its original timeline audio track. The report identifies the missing source
channel and the surviving channel placed as mono. The bin clip keeps its available media.

**What Any NLE does.** It removes incomplete stereo pairs from the bin clip and places a
surviving stereo edit channel as mono, keeping its source channel, timing, gain and explicit
pan. Without an explicit pan, the left channel stays fully left and the right stays fully
right. Complete stereo edits keep the existing behavior. When both channels are missing,
the picture still converts if available. Larger incomplete channel groups are reported and
left out rather than treated as stereo; this does not add surround support.

**Why.** A stereo pair cannot reference missing media. Keeping the available side avoids
refusing the entire project and preserves its channel position. Missing-media decisions
follow the specific bin copy used by the sequence, so a missing file in another copy does
not remove available sound. These are converter fallback rules, verified structurally;
application playback still needs read-back.

### Names, markers and malformed curves (reliability fixes, 2026-10-29)

**What you see.** Unicode sequence names work with colour correction. In the optional ASCII
`.cube` file's display title, non-ASCII characters, quotes and control characters become `?`;
the project's sequence name and the LUT values stay intact. An audio edit marker whose source
has no Avid track label uses the source file's position in the clip, just like its bin item.

**What Any NLE does.** Both writers validate curve points with repeated x positions and marker
comments that cannot survive XML saving before conversion starts. They report an input error
rather than guess a curve or silently change a comment. Ordinary multiline comments and tabs
are allowed. These are input-validation checks, not new Avid interpretation rules.

### Windows media paths (reliability fix, 2026-10-29)

**What you see.** A conversion on a Mac keeps Windows drive and network-share media locations
as absolute paths. It does not prepend the converter's current folder to them.

**What Any NLE does.** Both writers keep an absolute Windows path as the media path fallback,
including when refreshing imported media during save. POSIX paths keep their normal relative
form. This preserves the location supplied in the input; it does not translate drive letters
or network shares into mounted Mac locations, or establish application relinking compatibility.

### Checking the translated timeline before writing (2026-10-29)

**What you see.** A translated timeline is refused with its sequence/edit and reason when its
record timing falls between sequence frames, a required picture/audio stream is missing, or
its source range runs beyond the clip's media bound. The converter does not silently move or
trim those edits. Speed changes are checked against the amount of source they use.

**What Any NLE does.** The graph-to-Premiere bridge runs the same placement checks as direct
model input before returning anything to the project writer. It also refuses invalid sequence
rates and item identifiers that would collapse onto the same clip during translation. Valid
existing conversions keep the same timing and project bytes. A later reader still needs its
own format-specific checks; these checks do not establish application read-back compatibility.

### Linked original files

**What you see:** linked MP4/MOV and PCM WAV sources keep their original file paths,
with explicit stream and channel assignments. Native application qualification is required
before treating generated projects as compatible.

**What Any NLE does:** the typed original-file route follows saved FILE locators or an
explicit old-root to current-root mapping. It refuses missing or conflicting paths, unknown
stream selections, unsupported sample representations and unresolved origins. Shared stereo
AAC remains compressed audio with two explicit physical channels; it does not acquire a
PCM bit depth. Stored descriptor cadence, stored picture raster and physical stream clocks
and display raster remain separate observations. Original masters are ordinary file-backed
masters and do not create new media or hidden grouped/merged source sequences. Native media
and linked originals retain separate clip identities, including copies in different bins.

**Why:** names and common provenance do not prove file identity or interchangeable streams.
A qualified source graph is distinct from destination construction and native read-back.
The original-file writer input is a typed union; legacy model/1 remains strict and unchanged.

**Initial movie admission scope.** MP4/MOV picture must be H.264 High level 4.0
(`avc1`, 8-bit `yuv420p`), square-pixel progressive 1920×1080 at exactly 24 fps.
Video-only sources remain admissible; any present movie audio must be AAC LC (`mp4a`),
48,000 Hz stereo. The whole physical movie is checked, including streams not selected by
an individual Avid track. Higher bit depth, other profiles/levels, raster/rate variants,
HE-AAC and other movie-audio configurations refuse. PCM16/24 WAV admission is unchanged.

Explicit picture color tags must be BT.709, with limited (`tv`) range if tagged.
Absent, `unknown` or `unspecified` color/range tags produce a report note: the writer's
BT.709 destination profile is a default, **not observed source color or range**. Tagged
HDR, other color profiles and full-range sources refuse. Nonempty picture stream side data
(including HDR metadata and display matrices) also refuses. A legacy `tags.rotate` value, when present,
must be explicitly zero; nonzero or unresolved rotation refuses. These checks avoid silently
using the writer's identity-orientation profile for media requiring another interpretation.
They narrow admission to the initial slice; they do not establish that every matching file
will open in Premiere, or widen native qualification beyond the retained synthetic cohort.
### Project-folder import and source selection (native wizard, 2026-11-08)

**What you see.** Choose Avid bins or a project folder, then a new Premiere destination.
The folder tree shows stored master-clip and top-level sequence counts, checkboxes and
search. **Import entire project** selects every cataloged bin and disables individual
checkboxes. Trash and Attic are excluded unless explicitly included. The source safety
line is **Your Avid project is only read, never changed.**

**What Any NLE does.** Catalogs regular `.avb`/`.AVB` files deterministically, without
following symbolic links, opening one bin at a time. **Also import clips these sequences
use** follows stored source MobID routes through cataloged compositions to master bins;
missing routes or opaque reference barriers are reported. Counts describe stored bin
entries; they do not certify media presence or complete effect translation. The inventory
labels media references as candidates until pre-flight probes and qualifies them.

**Why.** A folder can contain many unrelated bins, and a selected sequence can depend on
clips in an unticked bin. Identity-based selection preserves that distinction without
using filenames as media identity. Catalogs are bounded to 50,000 bins, 500,000 directory
entries, 64 folder levels and 64 MiB of retained source facts; exceeding a limit refuses
rather than silently omitting files.

### Recommended settings, presets and replay (native wizard, 2026-11-08)

**What you see.** Back/Next, **Use recommended settings**, locally remembered choices,
load/save presets, and a command-line equivalent on Review.

**What Any NLE does.** New/reset settings select native rewrap and include sequence
source dependencies. Explicit remembered or preset construction choices remain selected.
Presets contain translation choices and color mappings, while a saved job contains the
specific paths for replay. Output destinations are still required and existing files are
never replaced. Loading settings or changing any choice invalidates the prepared review.

**Why.** Repeatable conversion needs explicit decisions and a review tied to those exact
decisions. A preset can be reused with a different project; replay needs a fresh source
and destination check before execution. Choices are stored only in local configuration;
there is no telemetry, updater or default Avid-bin association.

### Effects choices and pre-flight (native wizard, 2026-11-08)

**What you see.** One row per effect type found, with only the current supported
conversion, reported approximation or reported skip. Alternative choices appear only
when the shared engine actually supports alternatives.

**What Any NLE does.** Uses the same source interpretation and destination preparation
as conversion. Review lists media availability, losses and approximations and shows the
new-media space estimate when applicable. Review creates no output. Convert rechecks and
executes that prepared job; progress names the current bin and cancellation waits for a
safe boundary. Completion lists the actual project, receipt and any retained partial media.

**Why.** Showing a checkbox cannot make an unsupported translation available. Existing
qualified effect handling stays explicit, and failures or cancellation must not be shown
as a successful project. Native application compatibility remains scoped to the qualified
profiles; parser acceptance and UI operation do not extend those bounds.

### Keeping selected bins in their project folders (2026-11-08)

**What you see.** A project-folder import can place selected Avid bins beneath one
`AVID_IMPORT_<UTC timestamp>` bin. Their relative folder names are mirrored, so
`Scene A / Selects` and `Scene B / Selects` stay separate. Shared parent folders appear
once, and converted sequences and otherwise unfiled sequence sources sit inside the
same import root. Only the selected input bins and their parent paths are created;
selected empty bins are retained.

**What Any NLE does.** The app supplies an optional destination layout aligned with the
prepared input bins. The writer validates component names, a maximum of 64 levels
including the import root and actual bin, and a 64 MiB budget that charges UTF-8 text
and component references before allocating project IDs or writing output. Folder
lookup borrows individual components rather than copying every full path prefix.
The Python reference writer independently supports the same optional layout.

**Why.** Folder structure distinguishes same-named bins without renaming source bins.
This is destination organization; model/1 and graph/1 gain no new fields. Existing
writer callers that omit the layout retain their output bytes and UUID order. These
structural checks do not establish native application read-back compatibility.

### Pre-flight stability and replay (2026-11-08)

**What you see.** Pre-flight can spend time verifying source media. Cancel waits for the
current safe boundary. The report identifies actual published and retained outputs.

**What Any NLE does.** Reads media in place, pins content revisions, and rechecks source bins
and media before publication. Original-file facts are pinned around their physical probe;
metadata-only file stamps are not treated as content identity. The physical probe has a
30-second deadline. Native direct links are reinterpreted between content checks. Media
preparation checks cancellation between outputs and verification stages. A failed receipt
never removes a published project or becomes a successful completion report.

**Why.** A successful pre-flight must not authorize stale source facts or replace an output
that appeared later. The copied CLI embeds the current settings; saved jobs can be rerun
with explicit new project/media destinations. These checks do not qualify every file in an
otherwise supported codec profile or establish native application acceptance.

Saved-path mappings understand normalized POSIX paths, Windows drive roots and UNC roots
independently of the current host. A foreign saved prefix is usable only with an explicit
current root. Multiple distinct existing candidates refuse; mapping is never a basename
search. The resolved bytes still require codec, clock, routing and content checks. This
synthetic path-resolution coverage does not establish Windows or Linux editor acceptance.

The copied command encodes settings as one base64 argument to preserve literal filenames
across shells; it is not encryption. Save job settings is an explicit local file operation
and remains available for failed or large jobs. Large inline commands are disabled rather
than offered beyond portable shell limits. Settings reads are capped while streaming.

Bin counts and dependency selection are bound to the exact revisions read by the catalog.
If a selected or required bin changes between inventory and preparation, pre-flight refuses
before any output; a fresh catalog is required. Cancellation also stops original content
hashing between chunks and media-root scans between files.
