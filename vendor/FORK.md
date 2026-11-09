# Forked third-party code

## py_premiere

- Source: https://github.com/Pouf/py-premiere (research SRC-014)
- Commit: `adb9e5767aeeb833c40147bd84922afd55bc531f` (2026-10-01)
- Files: `src/py_premiere/**` and `LICENSE`, extracted with `git archive`. Upstream base for
  this fork; changes are listed under "Fork changes" below (ADR-0002).
- Tree digest (sha256 of sorted per-file sha256 list, run from `vendor/`):
  `94f00c239b2e24192e263bf3a8a9eab0409f60e2a57bb1d75c3f7a9c31f40e3f`
- License: MIT, Copyright (c) 2026 Aurore Delaunay (`py_premiere/LICENSE`). Attribution
  must be kept with any redistribution. This repository is local/private.
- The converter uses py_premiere's public API plus a few private
  members (`Project._register`, `_register_uid`, `_attach_imported_item`,
  `_imported_media`, `Sequence._link`). These are pinned to this commit, so re-check them
  on any upgrade.

Recompute the upstream digest (on an unmodified tree only):

```sh
cd vendor && find py_premiere -type f -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256
```

## Fork changes

| Date | Files | Change | Why |
|---|---|---|---|
| 2026-11-07 | `models/sequence_builder.py` | Optional six-channel 5.1 / eight-channel Adaptive sequence templates; legacy defaults unchanged | Match Rust and bounded native Premiere reference structures for CONV-0034; local extension, not an upstream capability or generic speaker-bus qualification |
| 2026-11-05 | `models/publication.py`, `models/project.py` | Native exclusive rename or same-directory hard-link publication for new projects; in-place replacement separate; report retained staging paths and cleanup warnings | Preserve late files/directories/symlinks and distinguish published projects from cleanup failures, matching Rust |
| 2026-10-29 | `models/project.py` | Exclusively create an unused temporary sibling for new/in-place saves; clean up own failed stages | Preserve existing `.tmp` files, folders and symlink targets, including graph snapshots; matching Rust behavior |
| 2026-10-29 | `models/media_paths.py`, `models/project.py` | Keep absolute Windows drive/UNC media paths when refreshing paths on save | Prevent POSIX cwd from being prepended to Windows paths; same rule in the Rust writer |
| 2026-10-14 | `models/multicam_builder.py` | Added `build_multicam_mono_audio` (N mono audio angles, one track each) and `import copy`; upstream `build_multicam` unchanged | Avid clips have separate A01/A02 mono files; shape from Premiere 26.5.2 (PRPROJ-0011) |
