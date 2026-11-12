# drp-format reference closeout

The review covers the MIT drp-format package pinned at
`1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048`. The converter uses its own Rust
archive reader and writer. The upstream source and tests serve as format
references. [Pinned package](https://github.com/samuelgursky/davinci-resolve-mcp/tree/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format)

| Reference area | Converter status and next native check |
|---|---|
| Archive packaging, registrations, keyed blobs and media caches | Existing version 17 DRP reader and writer retain the scoped timeline, bin, media and channel assignments after reopen. The [DRT follow-up](RESOLVE-DRT-CLOSEOUT.md) qualified native inspection and a bounded graph/2 reader by matching the archive, visible frames and both PCM lanes after reopen. DRT authoring remains open. |
| Linked media, relink and cache changes | Existing verified MOV relink and linked turnover profiles have native controls. Upstream new-media authoring uses an H.264 template. To qualify repointing to a different media profile, verify changed file identity and cache fields against actual codec, rate, frame count, timecode and channel metadata, then reopen and render. |
| Edits, markers and archive surgery | The converter writes straight cuts and bounded timeline markers. Standalone move, trim, split, ripple, archive diff and repair are research tools. An expanded marker test should cover nonzero origin, ranges and clip ownership after save and reopen. |
| Video transitions | The left-aligned eight-frame V1 Cross Dissolve below reads and writes in the internal DRP path. Qualify centered or right-aligned placement and Avid or Premiere transition translation with independent native cuts and handle controls. Audio fades and crossfades have their own PCM checks. |
| Compound and nested timeline ownership | Existing two-camera Group support has separate native evidence. For compounds and nested timelines, test two similarly named instances with different origins. Verify each inner sequence reference and source frame after reopen. |
| Time maps | Graph/2 records bounded saved timing facts. Qualify a nonidentity map with numbered picture, exact nonzero trims, and an independent sample-clock check where audio is involved. |
| Generators, Text+, and subtitles | Start with an editable Solid Color generator with a distinctive color. Verify rendered pixels and track placement. For Text+, check text, duration and rendered glyphs after reopen because the upstream project observed an Inspector and render failure despite structural read-back. Caption writing has a separate visible-text and cue-timing gate. |
| DRX, grades and LUT references | Compare one applied node and its rendered chart after re-export, including a missing LUT control. Cross-editor grade fidelity requires a matched source-editor render. |

## Qualified left-aligned Cross Dissolve

The retained twelve-frame request saved eight frames with exactly eight
incoming handle frames. A second Resolve Studio 21.2.0 build 00006 control
saved the same effect payload, with AlignmentType 3 and Position 2. The
qualified reader checks the payload and surrounding state, then binds the
two actual adjacent edit IDs. On reopen the opaque FieldsBlob varied in
four expanded bytes while the rest of its structure and the effect payload
stayed the same. The converter treats those four bytes as opaque.

The internal linked writer uses the sequence-owned `resolve.transitions`
receipt for one eight-frame, left-aligned V1 Cross Dissolve on the same-rate
1920×1080, `24000/1001` profile. The outgoing clip must cover the blend.
The incoming clip needs eight preceding source frames. Graph/2's generic
transition representation remains available for overlaps. The staged DRP
is read back to verify the transition, both adjacent edits and existing
media routing before publication.

In the disposable control, the timeline started at 01:00:00:00, the
visible cut was at relative frame 24, and the dissolve covered frames
16–23. Save Project As, close, reopen and re-export preserved both edit
ranges and their online media. The Inspector showed eight frames and
allowed the curve to change from 0.500 to 0.400 and back. All 48 decoded
picture frames matched the native dissolve. Exactly the eight blend
frames differed from the straight cut. Each isolated stereo PCM24 export
matched its straight-cut control over all 96,096 sample frames and the
two lane signals were distinct.

Refusal controls cover insufficient preroll, a mismatched edit identity,
an unsupported alignment and multiple transitions. The native and generated
re-export SHA-256 pins are
`a7bacbb3afc106019ef7f02fec0b172bbdcae81e0dc4c679010b65de986390db` and
`e57098b522d078ba41048b604e59cfebc65d63100ade292591e9ba8eebaeccd0`.
