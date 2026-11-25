# Synthetic Avid to Resolve release fixture

`split-edit.zip` contains one generated AVB and three OP-Atom siblings.
The Intel macOS release job uses them to verify direct linking and OP1a
rewrap from the extracted app. The test needs no editor or external media.

Picture is a 24-frame numbered HD pattern at `24000/1001`. Audio is two
independent 48 kHz PCM24 mono tones at 431 Hz and 683 Hz. The source
start is `01:12:00:03`. The AVB timeline starts at `01:00:00:07` and
has separate picture and audio trims.

| Track | Record frames | Source frames |
| --- | --- | --- |
| V1 | `[0, 8)`, `[8, 16)` | `[3, 11)`, `[13, 21)` |
| A1 | `[0, 6)`, `[6, 16)` | `[2, 8)`, `[11, 21)` |
| A2 | `[0, 10)`, `[10, 16)` | `[5, 15)`, `[17, 23)` |

## Authoring recipe

1. Generate the 24 ProRes picture frames using `write_picture` from
   [the linked media fixture](../../linked_media_fixture.py) with its frame
   count set to 24 and its first code set to 0. Mux those frames and the
   two FFmpeg sine sources as a MOV with two 48,048-sample mono PCM24
   streams. Set the MOV timecode to `01:12:00:03`.
2. In a new 1080p23.976 Resolve Studio project, make a timeline from that
   MOV with one video and two mono tracks. In Resolve 21.2.0 build 00006,
   use the **AVID AAF** render preset with MXF OP-Atom, DNxHR LB, PCM24,
   and 48 kHz. Render the entire 24-frame clip. Rename the three MXFs
   `Picture.mxf`, `Left.mxf`, and `Right.mxf`. Retain the embedded
   package IDs. The AAF stays outside this fixture.
3. In the generated MXF descriptors, change the DNxHR LB coding label
   `060e2b340401010a0401020271280000` to
   `060e2b340401010d0401020271280000`. This selects the existing Avid
   direct-link profile's label. The label occurs twice in picture and
   once per sound MXF. Package IDs, offsets, and essence bytes are
   preserved.
4. Run `python3 author_bin.py DIR FFPROBE` with
   [pyavb at ff64e06](https://github.com/markreidvfx/pyavb/tree/ff64e06b60077ecabe5db3fef239898b08363898)
   on the Python path. The script writes the AVB against the three paired
   MXFs. Zip the AVB and MXFs at the archive root.

The test compares all 24 distinct decoded source frame hashes and every
sample in both lanes with the rewrap. The archived media contains only
the generated counter and tones.
