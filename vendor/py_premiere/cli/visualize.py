"""
Premiere project tree visualization.

Prints the parsed object model: the project-panel item tree and every
sequence with its tracks and clips.

Modes:
    Default:  pr-visualize file.prproj
    Items:    pr-visualize file.prproj --items
    Timeline: pr-visualize file.prproj --sequences
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import TYPE_CHECKING

import py_premiere

from ..models import TICKS_PER_SECOND

if TYPE_CHECKING:
    from ..models import ProjectItem, Sequence


def _print_item(item: ProjectItem, depth: int) -> None:
    media = item.media_path
    detail = f"  ({media})" if media else ""
    print(f"{'  ' * depth}{item.name or '(unnamed)'} [{item.type.name}]{detail}")
    for child in item.children:
        _print_item(child, depth + 1)


def _print_sequence(sequence: Sequence) -> None:
    details = []
    if sequence.frame_size is not None:
        details.append(f"{sequence.frame_size[0]}x{sequence.frame_size[1]}")
    if sequence.timebase:
        details.append(f"{TICKS_PER_SECOND / sequence.timebase:.3f} fps")
    if sequence.audio_frame_rate:
        details.append(f"{round(TICKS_PER_SECOND / sequence.audio_frame_rate)} Hz")
    suffix = f" ({', '.join(details)})" if details else ""
    print(f"\n{sequence.name or '(unnamed sequence)'}{suffix}")
    for prefix, tracks in (("V", sequence.video_tracks), ("A", sequence.audio_tracks)):
        for track in tracks:
            print(f"  {prefix}{track.index + 1} [{len(track.clips)} clip(s)]")
            for clip in track.clips:
                print(
                    f"    {clip.start.seconds:8.3f}s - {clip.end.seconds:8.3f}s  "
                    f"{clip.name or '(unnamed)'}  "
                    f"(in {clip.in_point.seconds:.3f}s, out {clip.out_point.seconds:.3f}s)"
                )
    for caption_track in sequence.caption_tracks:
        captions = caption_track.captions
        print(f"  C{caption_track.index + 1} [{len(captions)} caption(s)]")
        for caption in captions:
            print(
                f"    {caption.start.seconds:8.3f}s - {caption.end.seconds:8.3f}s  "
                f"{caption.text or '(no text)'}"
            )


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for the `pr-visualize` command."""
    parser = argparse.ArgumentParser(
        prog="pr-visualize",
        description="Visualize the parsed object model of a .prproj file.",
    )
    parser.add_argument("file", type=Path, help="prproj file to visualize")
    parser.add_argument("--items", action="store_true", help="Item tree only")
    parser.add_argument("--sequences", action="store_true", help="Sequences only")
    args = parser.parse_args(argv)
    # Windows consoles/pipes default to the ANSI codepage; project content is
    # UTF-8 and must not crash the report.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if not args.file.exists():
        print(f"Error: File not found: {args.file}", file=sys.stderr)
        return 1

    application = py_premiere.parse(args.file)
    project = application.project
    print(f"{project.name}  ({len(project.sequences)} sequence(s))")
    show_items = args.items or not args.sequences
    show_sequences = args.sequences or not args.items
    if show_items and project.root_item is not None:
        print()
        _print_item(project.root_item, 0)
    if show_sequences:
        for sequence in project.sequences:
            _print_sequence(sequence)
    return 0


if __name__ == "__main__":
    sys.exit(main())
