"""
Premiere project file inspection tool.

Inspect `.prproj` structure:
- Object summary (top-level object counts by tag)
- Top-level object listing with IDs/UIDs/names
- Element tree visualization
- Raw XML dump of a specific object or of the whole payload

Modes:
    Default:  pr-inspect file.prproj                (object summary)
    List:     pr-inspect file.prproj --list         (top-level objects)
    Tree:     pr-inspect file.prproj --tree         (element tree)
    Dump:     pr-inspect file.prproj --dump 42      (object by ObjectID/UID/tag)
    XML:      pr-inspect file.prproj --xml --out payload.xml
"""

from __future__ import annotations

import argparse
import sys
from collections import Counter
from pathlib import Path
from typing import TYPE_CHECKING

from ..xml import parse_prproj
from ..xml.serializer import serialize_element

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..xml import PremiereDocument


def _text_preview(text: str, limit: int = 60) -> str:
    preview = repr(text)
    if len(preview) > limit:
        preview = preview[: limit - 3] + "..."
    return preview


def _element_label(element: ET.Element) -> str:
    attrs = "".join(f' {k}="{v}"' for k, v in element.attrib.items())
    return f"<{element.tag}{attrs}>"


def _summary(path: Path, document: PremiereDocument) -> int:
    payload = document.to_xml_bytes()
    print(f"{path.name}: {path.stat().st_size} bytes on disk, {len(payload)} bytes XML")
    print(
        f"PremiereData Version={document.root.get('Version')}, "
        f"{len(document.root)} top-level objects "
        f"({len(document.by_object_id)} with ObjectID, "
        f"{len(document.by_object_uid)} with ObjectUID)"
    )
    counts = Counter(element.tag for element in document.root)
    for tag, count in counts.most_common():
        print(f"  {count:5d}  {tag}")
    return 0


def _list_objects(document: PremiereDocument) -> int:
    for index, element in enumerate(document.root):
        object_id = element.get("ObjectID", "")
        object_uid = element.get("ObjectUID", "")
        name = element.findtext("Name") or ""
        ids = " ".join(part for part in (object_id, object_uid) if part)
        detail = f"  {name!r}" if name else ""
        print(f"  [{index:4d}] {element.tag} {ids}{detail}")
    return 0


def _print_tree(element: ET.Element, depth: int, max_depth: int) -> None:
    if 0 <= max_depth < depth:
        return
    detail = ""
    if not len(element) and element.text and element.text.strip():
        detail = f" = {_text_preview(element.text)}"
    print(f"{'  ' * depth}{_element_label(element)}{detail}")
    for child in element:
        _print_tree(child, depth + 1, max_depth)


def _dump(document: PremiereDocument, selector: str) -> int:
    # No truthiness on Elements: a childless Element is falsy.
    element = document.by_object_id.get(selector)
    if element is None:
        element = document.by_object_uid.get(selector)
    if element is not None:
        sys.stdout.buffer.write(serialize_element(element) + b"\n")
        return 0
    matches = [el for el in document.root if el.tag == selector]
    if not matches:
        print(f"No top-level object matches {selector!r} ", file=sys.stderr)
        print(
            "(try an ObjectID, an ObjectUID, or a top-level tag name)", file=sys.stderr
        )
        return 1
    if len(matches) > 1:
        print(f"# {len(matches)} <{selector}> objects", file=sys.stderr)
    for match in matches:
        sys.stdout.buffer.write(serialize_element(match) + b"\n")
    return 0


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for the `pr-inspect` command."""
    parser = argparse.ArgumentParser(
        prog="pr-inspect",
        description="Inspect Premiere Pro project file (.prproj) structure.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    %(prog)s file.prproj                 (object summary)
    %(prog)s file.prproj --list          (top-level objects with IDs and names)
    %(prog)s file.prproj --tree --depth 3
    %(prog)s file.prproj --dump 42      (object with ObjectID 42)
    %(prog)s file.prproj --dump Sequence
    %(prog)s file.prproj --xml --out payload.xml

Use --out for byte-exact payload extraction; PowerShell's `>` redirection
re-encodes output and corrupts it.
        """,
    )
    parser.add_argument("file", type=Path, help="prproj file to inspect")
    parser.add_argument("--list", action="store_true", help="List top-level objects")
    parser.add_argument("--tree", action="store_true", help="Print the element tree")
    parser.add_argument(
        "--depth",
        type=int,
        default=-1,
        metavar="N",
        help="Max tree depth (-1 for unlimited, use with --tree)",
    )
    parser.add_argument(
        "--dump",
        type=str,
        default=None,
        metavar="SELECTOR",
        help="Print one object's XML by ObjectID, ObjectUID, or top-level tag",
    )
    parser.add_argument(
        "--xml",
        action="store_true",
        help="Write the decompressed XML payload (to --out or stdout)",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        metavar="FILE",
        help="Write --xml payload bytes to FILE instead of stdout",
    )

    args = parser.parse_args(argv)
    # Windows consoles/pipes default to the ANSI codepage; project content is
    # UTF-8 and must not crash the report.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    path: Path = args.file
    if not path.exists():
        print(f"Error: File not found: {path}", file=sys.stderr)
        return 1

    document = parse_prproj(path)
    if args.xml:
        payload = document.to_xml_bytes()
        if args.out is not None:
            args.out.write_bytes(payload)
        else:
            sys.stdout.buffer.write(payload)
        return 0
    if args.tree:
        _print_tree(document.root, 0, args.depth)
        return 0
    if args.dump is not None:
        return _dump(document, args.dump)
    if args.list:
        return _list_objects(document)
    return _summary(path, document)


if __name__ == "__main__":
    sys.exit(main())
