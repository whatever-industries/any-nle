# ProRes LT duration and geometry evidence

## Native Avid picture-only ProRes LT

A square-pixel 1080p24 managed ProRes 422 LT control had 36 declared
pictures and one terminal packet containing only zero padding. The
qualified MOV route verifies the package, index, complete picture
boundaries, and exact clock before excluding the terminal nonframe.
All 36 compressed pictures stay byte-identical. A changed terminal
byte prevents publication.
[Supported managed media](MEDIA-REWRAP.md#managed-picture-and-mono-streams)

### Marked Color Adapter inputs

The converter verifies the saved frame position and byte span before
writing the editable range transform. In the retained control, the
transform and both clip markers survived Premiere 27 Save As and reopen.

### Matching color-transform metadata on the input clip

An exact duplicate saved transform on the input clip can use the
qualified mapping. A disagreement is reported on the affected edit.
The editable correction, point marker, and span marker survived native
read-back on the matching control.

### Layered tracks and a separate dissolve

The managed picture profile verifies geometry and indexed positions per
edit, including V2. Three corrections and a separate centered dissolve
survived Save As and reopen. Effects on transition endpoints still need
separate order evidence.

### Managed Color Adapter verification at reel scale

A synthetic project with 209 packages verified independent position
checks on 211 edits. An incorrect saved byte range left that cut in place
with a reported color omission. Verified physical indexes can be reused
for other edits of the same unchanged package.

### Retimed subclip cadence

The retained same-rate positive constant-speed and Color Adapter
controls matched the saved frame selection in Premiere 27. The route
checks the backing file range, saved speed and position values, and
adapter order. [Timewarp and Gain controls](SOURCE-GRAPH-MIGRATION.md#avid-gain-and-timewarp-fallbacks)

## Source evidence and index interpretation

A separate retained 24 fps anamorphic control declares 446 pictures.
Its index exposes one further 21,952-byte all-zero span outside the
coded pictures. An independent payload and decode comparison verified
all 446 pictures in the corrected MOV and frame-wrapped OP1a. The saved
bin and media identities disagree for the original control. Automatic
rewrap also needs verified source and destination geometry.
[SMPTE ProRes bitstream and MXF mapping](https://pub.smpte.org/pub/rdd44/rdd44-2022.pdf)


## Premiere 27 read-back

Premiere 27.0.0 build 88 imported the corrected MOV and the BER5 OP1a
in separate projects. Each stayed online after Save As and reopen with
all 446 frames at 24 fps and source timecode `00:00:05:14`. Both files
carry `16:15` sample aspect, but Premiere assigned `768:702` to the
source and its new sequence at this 1920×1080 raster. A Match Source
export used `35:32` sample aspect. Adobe documents the corresponding
default PAL pixel aspect as approximately 1.094.
[Adobe pixel aspect guidance](https://helpx.adobe.com/my_ms/premiere/desktop/edit-projects/intro-to-editing/aspect-ratios.html)

An exact `16:15` source override stored `OverriddenPAR` and
`IsPAROverridden` on the video stream. In each wrapper, a fresh
source-matched sequence inherited `16:15` on the track group and placed
clip and kept it through Save As and reopen. Both exports carried
`16:15` and matched each other across all 446 decoded frames.
The Premiere writer now copies the saved effective ratio when it
places an interpreted video stream. An added placement on the reopened
MOV project kept the ratio and bounds after another Save As and reopen.
The resulting 892-frame export matched two copies of the native
446-frame render.

Automatic Avid rewrap admission still requires a matching saved package,
verified extent, and source geometry carried into destination preparation.
The retained AVB and the media package disagree. This qualification
covers the two standalone picture candidates on macOS.

A separate 1440×1080 ProRes control uses a generated AVB whose file package
matches the source MXF. It reaches Review's geometry check and reports
the verified `4:3` pixel aspect specifically. The square-pixel companion
passes Review.
This gives the automatic route a matching-package control for the next
geometry implementation step.
[Automatic rewrap task](ROADMAP.md#media-linking-user-2026-10-14)
