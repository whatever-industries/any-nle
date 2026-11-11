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
