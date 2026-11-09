# Feature matrix: what converts, per program

The planning tool for new directions (docs/GRAPH-MODEL.md). One row per feature: its form in
`anynle.graph/1`, how each program reads it into the graph and writes it out, and the evidence.
**A direction's to-do list is the empty cells in its two columns.**

Evidence is cited, not restated:
- CONV IDs: `research-dependencies.json`, with finding hashes;
- PRPROJ IDs: `knowledge/premiere`;
- AVB and IDENTITY IDs: `knowledge/avid`.

`knowledge/shared/` points here: it is not under version control, and this table must change in
step with the code.

Status words:
- **verified**: checked by the User in the target program against the source;
- **built**: implemented with pinned findings, not yet read back;
- **approx.**: built, but knowingly approximate;
- **loss**: reported as not converted;
- **—**: not started.

The FCPXML column is DTD-valid (Apple FCPXMLv1_9.dtd, all 36 golden models) but not yet read back in Resolve or Final Cut, so nothing in it is "verified".

| Feature | graph/1 | Avid → graph | graph → Premiere | graph → FCPXML 1.9 (Resolve, FCP) | Resolve native, OTIO, AAF, Premiere → Avid |
|---|---|---|---|---|---|
| Source media (OP-Atom MXF) | `media[]` (locations, codec, size, rates, stream) | built: UMID relink, MXF durations [CONV-0013, CONV-0017; IDENTITY-0006] | verified, DNx and ProRes LT [CONV-0001, CONV-0016; PRPROJ-0009, PRPROJ-0017] | built: one asset per stream | — |
| Master clips | `items[]` | built [CONV-0012; AVB-0006] | verified, separate / merged / grouped [CONV-0001, CONV-0007, CONV-0008; PRPROJ-0011] | built: browser clip, picture with audio lanes | — |
| Clip in several bins | `bins[].items` (an item in many bins) | built | verified: a copy per bin (DECISIONS-EXPLAINED) | built: a clip in each bin's event | — |
| Nested bins | `bins[].parent` | — (project-folder import, ROADMAP) | — (`bridge::to_model` flattens, with a loss) | loss: top-level events | — |
| Subclips, groups, multicam, nests as items | not yet in graph/1 | loss: skipped with a reason | — | — | — |
| Metadata (Name, Tape, columns) | `items[]` name, tape, umid; `avid` sidecar (columns) | built | verified [CONV-0014; PRPROJ-0015] | name only | — |
| Timecode (clip, sequence, aux) | `media[].start`, `sequences[].start`; aux in `avid` sidecar | built; aux TC is a hypothesis [CONV-0012] | verified [CONV-0015, CONV-0016; PRPROJ-0016, PRPROJ-0017] | built: asset start, tcStart (NDF only) | — |
| Edits (record/source ranges, tracks) | `placements[]` | built [AVB-0002] | verified [CONV-0003, CONV-0004, CONV-0015; PRPROJ-0003, PRPROJ-0008, PRPROJ-0016] | built: spine + connected clips, lanes for tracks | — |
| Stereo pairs | `items[].stream_groups`, `placements[].grouped_audio` | built | verified [CONV-0018, CONV-0020, CONV-0021; PRPROJ-0019, PRPROJ-0021, PRPROJ-0022] | approx.: separate mono lanes (loss) | — |
| Clip / edit gain, volume, keys | `items[].media[].gain_db`, `placements[]` gain/volume | built [AVB-0005] | verified [CONV-0009, CONV-0010, CONV-0019; PRPROJ-0013, PRPROJ-0014, PRPROJ-0020] | approx.: constant adjust-volume; keys loss | — |
| Pan | `placements[].track_pans`, `grouped_audio[].pans` | built (explicit pans only) [AVB-0005] | verified [CONV-0022; PRPROJ-0023] | loss | — |
| Track levels | `sequences[].tracks[].level_db` | built | verified [CONV-0010; PRPROJ-0014] | loss | — |
| Audio crossfades | `sequences[].transitions[]` (audio) | built | built: Constant Power, Custom Fade [CONV-0011; PRPROJ-0013] | loss | — |
| Video transitions | `transitions[]` (video) | — | — | — | — |
| Constant speed | `placements[].speed` | built [AVB-0010] | approx.: picture only [CONV-0023; PRPROJ-0024] | loss | — |
| Timewarp / time remap | `placements[].frame_map` | editable ramps are not decoded; current reader warns and assumes constant speed [AVB-0010; ROADMAP] | supplied frame-map writer checked over retained 192 frames; this does not qualify Avid ramp extraction [CONV-0028; PRPROJ-0028] | loss | — |
| Resize, static 3D Warp | `placements[].transform` | built; animated/3D controls: loss [AVB-0009] | verified [CONV-0027] | loss | — |
| Colour correction | `effects[]` `avid.color_correction` (not neutral yet) | built [AVB-0012] | approx.: Lumetri + 33³ Look LUT, provisional model [CONV-0025, CONV-0026; PRPROJ-0026, PRPROJ-0027] | loss | — |
| AniMatte | `effects[]` `avid.animatte` (not neutral yet) | built: Key In/Out, one tracked point [AVB-0015, AVB-0016] | verified, 46/46 frames, square pixels [CONV-0029; PRPROJ-0029] | loss | — |
| Markers (clip, sequence, edit, filler) | `markers[]` with owner, placement, source, track | built [AVB-0002] | verified [CONV-0024; PRPROJ-0025] | built: on clips and spine | — |
| Marker and clip colours | `markers[].color` (palette label), `items[].color_rgb16` | built | verified; tables in the mapping policy [CONV-0024] | loss (no colour in 1.9) | — |
| SubCap / current static single-flow Titler+ | graph/2 `texts[]`; native title property sidecar | bounded content/timing [AVB-0039, AVB-0013] | embedded captions / editable text; destination style defaults [CONV-0037, PRPROJ-0033]; scoped native read-back, full-media/style qualification pending | — | — |
| Opacity, other title/effect forms | — | — | — | — | — |

Pair policies (mapping stage, not graph content; `crates/mapping`), Avid → Premiere:
- bin mode;
- Avid default pan;
- marker conflict winner and loser;
- Avid info in marker comments;
- marker colour and clip label tables.

A new pair defines its own policy.
