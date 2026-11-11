# Premiere and Resolve metadata for relink and online

Use this inventory when expanding Premiere → graph/2 → Resolve and
Resolve → graph/2 → Premiere. It records the column or control the editor
exposes, the owner that supplies its value, and what can travel. The
Premiere inventory comes from a saved Premiere 26.5.2 column schema and
Adobe's documentation. The Resolve inventory comes from the 21.1 reference
manual and selected 21.2 Studio archives. Both editors support custom
fields, and an installed-build census of every selectable field remains
open. The Avid knowledgebase already has a per-heading bin column matrix.
Keep its ownership and lifecycle findings there.

## Premiere Project panel

The retained 26.5.2 schema in PRPROJ-0015 includes these 58 definitions.
A schema entry confirms the name and native column ID. Storage and
editability require separate evidence.

| Area | Saved headings |
| --- | --- |
| Clip and media identity | Name, Media Type, Media File Name, Media File Path, Tape Name, Original Video File Name, Original Audio File Name |
| Media timing and selected range | Frame Rate, Media Start, Media End, Media Duration, Video In Point, Video Out Point, Video Duration, Audio In Point, Audio Out Point, Audio Duration, Subclip Start, Subclip End, Subclip Duration |
| Source tracking and sync | Sound Timecode, Linear Timecode, Aux Timecode, Sound Roll, Camera Roll, Daily Roll, Lab Roll, Keycode, Sync Offset |
| Technical properties | Video Info, Audio Info, Video Codec, Field Order |
| Proxy and online state | Proxy, Proxy Media File Name, Proxy Media File Path, Status, Offline Properties |
| Logging | Scene, Shot, Description, Comment, Log Note, Client, Good |
| Color and viewing | Label, ASC_SOP, ASC_SAT, Lut, Lut1, Lut2 |
| Project state | Selected, Hide, Video Usage, Audio Usage, Project Locked, Transcription Status, Content Credentials |

- **Project logging.** PRPROJ-0015 observed Tape Name, Shot, Scene,
  Description, and Log Note on the master's referenced `ClipLoggingInfo`.
  Comment and Client are on the project item's own property bag. The Shot
  field is serialized as `ShotTake`. Keep its value as a single string
  unless a separate saved Take field has evidence. The read-back found an
  empty displayed Log Note for one multiline case. A general multiline
  write remains open.
- **Physical media and time.** `Media/FilePath` and
  `Media/ActualMediaFilePath` are saved media locators. `RelativePath`
  can occur more than once on the same Media record. `FileKey` is a saved
  Premiere identifier. For the tested MXF case PRPROJ-0017 observed
  `Media/Start` in Premiere ticks, separate alternate-start state and
  absolute logging in/out. A subclip's edit origin and marks are separate
  quantities.
- **Other schemas.** Adobe exposes selectable XMP schemas and custom
  project columns. Record an XMP field by schema URI and property name,
  with the owner and source of the value. Writing an XMP property can
  change media or sidecars. Project-only translation leaves originals
  and sidecars untouched. [Adobe metadata guide](https://helpx.adobe.com/premiere/desktop/organize-media/edit-metadata/metadata-in-premiere.html) ·
  [Project panel columns](https://helpx.adobe.com/premiere/desktop/get-started/customize-the-project-panel/list-view-columns.html) ·
  [UXP metadata interface](https://developer.adobe.com/premiere-pro/uxp/ppro-reference/classes/metadata)

## Resolve Media Pool and Metadata Editor

The Resolve 21.1 Reference Manual shows the Media Pool's Customize Columns
dialog on p. 390. Chapter 18 covers editable fields, File Inspector and
metadata CSV/ALE. Chapter 22 covers conform rules.

| Area | Documented columns and metadata controls |
| --- | --- |
| Default list view, identity and status | Clip Name, Cloud Sync, Reel Name, File Path, Type, Flags, Date Modified, Date Added, Synced Audio, Online Status, Transcription Status |
| Default list view, clock and format | Start TC, End TC, Duration, Frames, Resolution, FPS, Audio Ch, Format, Video Codec, Audio Codec |
| Additional Media Pool and metadata fields | File Name in metadata matching, Date Created, Camera #, Description, Comments, Keyword, Shot, Scene, Take, Angle, Circled, Clip Color, Rating, Usage, Start KeyKode, People |
| Source and workflow controls | Reel/Card ID in the File Inspector, Reel Name mode and manual override in Clip Attributes, embedded or frame-count timecode, clip timecode override, proxy and synced-audio relationships |
| Project-defined | Custom metadata and column layouts. CSV exports only fields populated by at least one selected clip. |

The manual's ALE example (p. 4166) has separate `Name`, `Display Name`,
`Tape`, `Reel`, `UNC`, and `Source File Path` columns. It also lists
`Audio SR`, `Audio Bit Depth`, `Auxiliary TC1`, and `KN Start`. Those
are interchange headers. Track them separately from the Media Pool
heading and any native project field.

Resolve's installed scripting reference exposes `GetClipProperty()` and
`GetMetadata()` as different interfaces. The latter returns set values.
The Media Pool columns, clip properties and metadata dictionary are useful
independent census inputs. [Resolve Reference Manual, pp. 389–391, 406–424,
484–486 and 797](https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf)

The archive reader has separately observed a Media Pool record's
`UniqueMediaPoolItemId`, the cached folder and filename on its linked media
record, and `BtVideoInfo.VideoMetadata.ActualReelNumber` on some video
records. The cached reel's provenance and its relationship to the effective
Reel Name and Reel/Card ID controls still require a one-change comparison.
Ordinary video and audio caches use distinct records. Independently linked
audio can belong to a different original. [Pinned DRP archive notes](https://raw.githubusercontent.com/samuelgursky/davinci-resolve-mcp/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format/README.md)

## Cross-editor transfer

| Meaning | Avid reference vocabulary | Premiere | Resolve | Conversion rule |
| --- | --- | --- | --- | --- |
| Editorial display name | Name | Name | Clip Name | Carry the saved item name. Keep the item and physical file identities separate, including when two items have the same name. |
| Original locator and file identity | Source File, Source Path, source or file package IDs | Media File Path, Media File Name, saved Media record, original picture/audio filename fields | File Path, file name, Media Pool cache, pool item ID | Bind the saved physical file, stream and channel. Carry both saved and verified current paths with their provenance. Native record IDs are scoped to their own editor. Original camera, proxy and replacement locators have distinct roles. |
| Primary source clock and extent | Start, End, Duration, source TC track | Media Start, Media End, Media Duration, saved alternate start | Start TC, End TC, Duration, Frames, Clip Attributes override | Carry exact rate, timecode count, drop flag, and origin provenance separately from stream PTS and edit positions. Establish endpoint convention before copying an End value. Preserve original and overridden clocks when both are known. |
| Reel for conform | Tape, with Camroll and other rolls as separate fields | Tape Name in logging, Media tape values when saved | Reel Name, derived by policy or overridden per clip | Candidate mapping only for the same explicit reel role. Preserve source and policy. Resolve's File Inspector Reel edits Reel/Card ID, which has a separate role. A filename-derived Reel Name remains derived. |
| Camera, picture roll, lab and sound identity | Camroll, Labroll, Soundroll and source identifiers | Camera Roll, Lab Roll, Sound Roll, Original Video/Audio File Name | Camera #, Reel/Card ID, picture and synced sound references | Camera designation, picture roll, lab roll and sound roll have separate meanings. Keep unmapped values on their native owner until a corresponding destination field is verified. |
| Additional timecode and sync | Aux TC1–TC5, Sound TC and source offsets | Aux Timecode, Sound Timecode, Linear Timecode, Sync Offset | Additional timecode streams, synced-audio offset or source clock | Preserve exact clock and purpose, including independent audio sample position. Only promote a value with a known source and destination interpretation. |
| Shot logging | Scene, Take, Comments and custom fields | Scene, Shot, Description, Comment, Log Note, Good | Scene, Shot, Take, Description, Comments, Circled | Scene, Description, and Comment/Comments are first write candidates. Preserve independent notes, custom fields and unstructured Shot text until their destination storage is proven. Test Good/Circled separately. |
| Channel layout and interpretation | Track grouping, channel assignments, source descriptors | Audio Info and saved channel selectors | Audio Ch, Media Pool source channel configuration and synced audio | Use physical stream and channel selection. Derive display properties from the verified media. Carry interpretation overrides as explicit facts. |
| Color, usage and status | Color, ASC_SOP, ASC_SAT, Drive, Offline | Label, ASC_SOP, ASC_SAT, LUT and usage or status columns | Clip Color, flags, input color space, grade and online state | Translate display labels using the destination palette. Treat grade metadata and input transforms as separate. Compute actual online state on the destination system. |

The detailed Avid source is the Avid knowledgebase's **Bin column matrix**
(`knowledge/avid/docs/bin-columns.md`, outside this repository).

## Which fields can reconnect media

- **Premiere Link Media.** Adobe documents File Name, File Extension, Media
  Start and Tape Name as match options. The dialog displays the clip name
  separately from the linked filename. A selected option changes what
  Premiere considers a match. Preserve original references and the
  clock fields independently. [Adobe Link Media](https://helpx.adobe.com/premiere/desktop/organize-media/file-organization/locate-and-link-offline-files.html)
- **Resolve relink and conform.** Opening a native DRP uses its saved
  Media Pool cache and locators. For conform from another interchange
  format, Resolve's configured filename/reel and timecode rules apply.
  Reel policy can derive a value from path, file name, bin name or embedded
  source metadata, with per-clip exceptions. Metadata CSV import has its
  own filename and clip-start/end match options, with optional reel or
  source path. A CSV match transfers logging values to an already selected
  clip. [Resolve Reference Manual, pp. 422 and 484–486](https://documents.blackmagicdesign.com/UserManuals/DaVinciResolveReferenceManual.pdf)
- **Any NLE file binding.** Start with the saved source reference and
  verify the physical file, stream, channel, and extent. For relocated
  originals, add explicit root mappings with a single verified result.
  A display name, basename or reel alone is insufficient when originals
  can collide. A conflict in the saved locator, source clock, media
  extent or channel assignment blocks publication until resolved.

## Current graph/2 capture and next destination work

- The Premiere turnover stores saved file path fields, an opaque FileKey
  and the verified original's SHA-256 under `premiere.media_identity`.
  Native logging and item-bag values are available under
  `premiere.logging`. The explicit logging Tape Name also fills the
  source item's `tape`. The internal Resolve turnover reports the
  unmapped fields by source item and requires approval to proceed.
  Different saved primary and actual paths, or a nonzero saved media
  origin outside the current profile, stop this route. A verified
  zero-origin QuickTime timecode track becomes an exact source clock.
- The Resolve reader records the pool item's secondary ID and saved
  video reel cache in `resolve.media_metadata`. It preserves opaque
  optional metadata by hash when decoding is unresolved. A cached reel
  remains a source observation. Resolve → Premiere has no qualified
  destination route yet.
- **Next.** Author a two-original native control with identical basenames
  in separate folders and distinct file IDs, reel values, nonzero
  source timecodes and a trimmed edit. Include one renamed clip and
  distinct Scene, Description and Comment values. Compare a saved
  manual Reel Name with a derived Reel Name and an independently set
  Reel/Card ID. Qualify a moved-original and a wrong-file control, exact
  picture and audio selection, and save/reopen in both editors. Expand
  the installed inventories from saved Premiere schema/UXP output and
  Resolve column, clip-property, and populated metadata exports.

For **Premiere → Resolve**, widen the existing straight-cut turnover by
qualifying the Media Pool cache's filename, source timecode and channel
selectors on that control. Then write the verified manual reel mapping
and the three logging fields. Check fresh insertion as well as the
original sequence.

For **Resolve → Premiere**, start with the same straight-cut MOV/WAV
profile. Resolve archive paths and cache IDs are saved claims until the
current files and physical stream selectors are verified. Feed those
verified facts and the exact source and sequence clocks to Premiere
preparation. Validate the Premiere Media Start and Tape Name read-backs
independently from timeline placement. Keep any unmatched logging
values on graph/2 and report them by item.
