"""
Premiere project comparison tool.

Compares two `.prproj` files structurally, aware of resave churn: top-level
objects are matched by `ObjectUID` first, then by a normalized signature that
ignores per-save integers (`ObjectID`/`ObjectRef`), then pairwise by tag in
document order. `ObjectRef` values are compared through their resolved target
(tag + `ObjectUID`), so pure renumbering does not show up as a difference.

Children are aligned by longest common subsequence (keyed on tag plus any
stable `ObjectUID`/`ObjectURef`), so an element inserted mid-list is reported
once instead of misaligning every sibling after it.

Signatures are REF-AWARE: an `ObjectRef` contributes the identity of what it
points at (the target's `ObjectUID`, or the target's own signature two hops
deep), so two UID-less objects differing only in which target they reference
no longer collide.

Modes:
    Compare:  pr-compare file1.prproj file2.prproj
    Filter:   pr-compare a.prproj b.prproj --filter Sequence
    Churn:    pr-compare a.prproj b.prproj --show-churn
"""

from __future__ import annotations

import argparse
import sys
from collections import Counter
from difflib import SequenceMatcher
from pathlib import Path
from typing import TYPE_CHECKING

from ..xml import parse_prproj

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..xml import PremiereDocument

#: Attributes renumbered on every Premiere save; excluded from signatures.
VOLATILE_ATTRS = frozenset({"ObjectID", "ObjectRef"})


class _Stats:
    def __init__(self) -> None:
        self.diff_lines = 0
        self.churn_suppressed = 0
        self.formatting_suppressed = 0


#: How far a ref-aware signature follows a chain of UID-less targets before
#: settling for the target's tag. Two hops tells apart the objects that
#: actually collide - a parameter or chain pointing at one of several
#: identical-looking neighbours - without walking the graph, and bounds the
#: recursion on reference cycles.
_REF_DEPTH = 2


def _ref_identity(document: PremiereDocument, ref: str, depth: int) -> str:
    # What a reference POINTS AT, in terms that survive renumbering: the
    # target's UID where it has one, otherwise the target's own signature,
    # to `depth` further hops.
    target = document.by_object_id.get(ref)
    if target is None:
        # The raw id is per-save; naming it would make signatures differ
        # between two documents that are dangling in the same way.
        return "<dangling>"
    uid = target.get("ObjectUID")
    if uid:
        return f"{target.tag}[{uid}]"
    if depth <= 0:
        return target.tag
    parts: list[str] = []
    _signature_parts(target, parts, document, depth - 1)
    return "\x00".join(parts)


def _signature_parts(
    element: ET.Element,
    out: list[str],
    document: PremiereDocument | None = None,
    depth: int = _REF_DEPTH,
) -> None:
    out.append(element.tag)
    for key in sorted(element.attrib):
        if key == "ObjectRef" and document is not None:
            out.append(
                "ObjectRef->" + _ref_identity(document, element.attrib[key], depth)
            )
        elif key not in VOLATILE_ATTRS:
            out.append(f"{key}={element.attrib[key]}")
    if element.text is not None and element.text.strip():
        out.append(element.text.strip())
    out.append("(")
    for child in element:
        _signature_parts(child, out, document, depth)
    out.append(")")


def _signature(
    element: ET.Element,
    document: PremiereDocument | None = None,
    depth: int = _REF_DEPTH,
) -> str:
    """A renumbering-proof identity for an object.

    Without `document` the references are simply dropped, which cannot
    tell apart two UID-less objects differing only in what they point at.
    With it, each `ObjectRef` contributes its TARGET's identity instead.
    """
    parts: list[str] = []
    _signature_parts(element, parts, document, depth)
    return "\x00".join(parts)


def _object_label(element: ET.Element) -> str:
    uid = element.get("ObjectUID")
    if uid is not None:
        return f"{element.tag}[{uid}]"
    name = element.findtext("Name")
    if name:
        return f"{element.tag}[{name!r}]"
    return f"{element.tag}[ObjectID={element.get('ObjectID')}]"


def _ref_target(document: PremiereDocument, ref: str) -> str:
    target = document.by_object_id.get(ref)
    if target is None:
        return f"<dangling:{ref}>"
    uid = target.get("ObjectUID")
    return f"{target.tag}[{uid}]" if uid else _signature(target, document)


def _truncate(value: str, limit: int = 80) -> str:
    return value if len(value) <= limit else value[: limit - 3] + "..."


def _match_objects(
    doc1: PremiereDocument, doc2: PremiereDocument
) -> tuple[list[tuple[ET.Element, ET.Element]], list[ET.Element], list[ET.Element]]:
    """Pair top-level objects across the two documents (UID > signature > tag order)."""
    left = list(doc1.root)
    right = list(doc2.root)
    pairs: list[tuple[ET.Element, ET.Element]] = []
    matched_right: set[int] = set()

    right_by_uid = {el.get("ObjectUID"): el for el in right if el.get("ObjectUID")}
    left_rest: list[ET.Element] = []
    for element in left:
        uid = element.get("ObjectUID")
        partner = right_by_uid.get(uid) if uid else None
        if partner is not None:
            pairs.append((element, partner))
            matched_right.add(id(partner))
        else:
            left_rest.append(element)

    right_by_sig: dict[str, list[ET.Element]] = {}
    for element in right:
        if id(element) not in matched_right:
            right_by_sig.setdefault(_signature(element, doc2), []).append(element)
    left_rest2: list[ET.Element] = []
    for element in left_rest:
        bucket = right_by_sig.get(_signature(element, doc1))
        if bucket:
            partner = bucket.pop(0)
            pairs.append((element, partner))
            matched_right.add(id(partner))
        else:
            left_rest2.append(element)

    right_rest = [el for el in right if id(el) not in matched_right]
    right_by_tag: dict[str, list[ET.Element]] = {}
    for element in right_rest:
        right_by_tag.setdefault(element.tag, []).append(element)
    only_left: list[ET.Element] = []
    for element in left_rest2:
        bucket = right_by_tag.get(element.tag)
        if bucket:
            pairs.append((element, bucket.pop(0)))
        else:
            only_left.append(element)
    only_right = [el for bucket in right_by_tag.values() for el in bucket]
    return pairs, only_left, only_right


def _diff_attrs(
    doc1: PremiereDocument,
    doc2: PremiereDocument,
    el1: ET.Element,
    el2: ET.Element,
    path: str,
    lines: list[str],
    stats: _Stats,
    show_churn: bool,
) -> None:
    for key in sorted(set(el1.attrib) | set(el2.attrib)):
        v1 = el1.get(key)
        v2 = el2.get(key)
        if v1 == v2:
            continue
        if key in VOLATILE_ATTRS and not show_churn:
            if v1 is None or v2 is None:
                # A reference added or removed is structural, not churn.
                lines.append(f"  {path} @{key}: {v1!r} vs {v2!r}")
                continue
            if key == "ObjectRef":
                t1 = _ref_target(doc1, v1)
                t2 = _ref_target(doc2, v2)
                if t1 != t2:
                    lines.append(
                        f"  {path} @{key}: -> {_truncate(t1)} vs -> {_truncate(t2)}"
                    )
                    continue
            stats.churn_suppressed += 1
            continue
        lines.append(f"  {path} @{key}: {v1!r} vs {v2!r}")


def _child_key(element: ET.Element) -> str:
    # Alignment key: the tag, plus a STABLE identifier when the element
    # carries one. ObjectID/ObjectRef are deliberately excluded - they
    # renumber freely between saves, and keying on them would turn ordinary
    # churn into a wall of insert/delete pairs.
    uid = element.get("ObjectUID") or element.get("ObjectURef")
    return f"{element.tag}\x00{uid}" if uid else element.tag


def _align_children(
    el1: ET.Element, el2: ET.Element
) -> list[tuple[int | None, int | None]]:
    """Pair up children by longest common subsequence, not by position.

    Positional pairing turns a single inserted element into a cascade of
    bogus `tag <A> vs <B>` lines for every later sibling plus a phantom
    unpaired tail. Aligning first reports the insertion once and diffs the
    genuinely corresponding children against each other.
    """
    keys1 = [_child_key(child) for child in el1]
    keys2 = [_child_key(child) for child in el2]
    aligned: list[tuple[int | None, int | None]] = []
    matcher = SequenceMatcher(a=keys1, b=keys2, autojunk=False)
    for opcode, start1, end1, start2, end2 in matcher.get_opcodes():
        if opcode == "equal":
            aligned.extend(zip(range(start1, end1), range(start2, end2)))
            continue
        # A replaced run: pair positionally within it (same-tag siblings that
        # simply changed), then report whatever is left over on one side.
        overlap = min(end1 - start1, end2 - start2)
        aligned.extend(
            zip(range(start1, start1 + overlap), range(start2, start2 + overlap))
        )
        aligned.extend((index, None) for index in range(start1 + overlap, end1))
        aligned.extend((None, index) for index in range(start2 + overlap, end2))
    return aligned


def _diff_pair(
    doc1: PremiereDocument,
    doc2: PremiereDocument,
    el1: ET.Element,
    el2: ET.Element,
    path: str,
    lines: list[str],
    stats: _Stats,
    show_churn: bool,
) -> None:
    if el1.tag != el2.tag:
        lines.append(f"  {path}: tag <{el1.tag}> vs <{el2.tag}>")
        return
    _diff_attrs(doc1, doc2, el1, el2, path, lines, stats, show_churn)
    text1 = el1.text or ""
    text2 = el2.text or ""
    if text1 != text2:
        is_leaf = not len(el1) and not len(el2)
        if text1.strip() or text2.strip() or is_leaf:
            lines.append(f"  {path}: text {_truncate(text1)!r} vs {_truncate(text2)!r}")
        else:
            stats.formatting_suppressed += 1
    if len(el1) != len(el2):
        tags1 = Counter(child.tag for child in el1)
        tags2 = Counter(child.tag for child in el2)
        delta = {
            tag: (tags1.get(tag, 0), tags2.get(tag, 0))
            for tag in sorted(set(tags1) | set(tags2))
            if tags1.get(tag, 0) != tags2.get(tag, 0)
        }
        lines.append(f"  {path}: child count {len(el1)} vs {len(el2)} {delta}")
    for index1, index2 in _align_children(el1, el2):
        if index1 is not None and index2 is not None:
            _diff_pair(
                doc1,
                doc2,
                el1[index1],
                el2[index2],
                f"{path}/{el1[index1].tag}[{index1}]",
                lines,
                stats,
                show_churn,
            )
        elif index1 is not None:
            lines.append(f"  {path}: only in file1 <{el1[index1].tag}>[{index1}]")
        elif index2 is not None:
            lines.append(f"  {path}: only in file2 <{el2[index2].tag}>[{index2}]")


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for the `pr-compare` command."""
    parser = argparse.ArgumentParser(
        prog="pr-compare",
        description="Compare two Premiere Pro project files (.prproj).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    %(prog)s with_feature.prproj without_feature.prproj
    %(prog)s a.prproj b.prproj --filter Sequence
    %(prog)s a.prproj b.prproj --show-churn      (include ObjectID renumbering)

Objects are matched by ObjectUID, then by an ObjectID-insensitive signature,
then by tag in document order. Use pr-inspect for single-file views.
        """,
    )
    parser.add_argument("file1", type=Path, help="First prproj file")
    parser.add_argument("file2", type=Path, help="Second prproj file")
    parser.add_argument(
        "--filter",
        type=str,
        default=None,
        help="Only report objects whose tag or label contains this substring",
    )
    parser.add_argument(
        "--max-per-object",
        type=int,
        default=25,
        metavar="N",
        help="Cap reported diff lines per object pair (default 25)",
    )
    parser.add_argument(
        "--show-churn",
        action="store_true",
        help="Report raw ObjectID/ObjectRef renumbering instead of resolving it",
    )
    args = parser.parse_args(argv)
    # Windows consoles/pipes default to the ANSI codepage; project content is
    # UTF-8 and must not crash the report.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    for path in (args.file1, args.file2):
        if not path.exists():
            print(f"Error: File not found: {path}", file=sys.stderr)
            return 1

    doc1 = parse_prproj(args.file1)
    doc2 = parse_prproj(args.file2)
    pairs, only_left, only_right = _match_objects(doc1, doc2)

    needle = args.filter.lower() if args.filter else None

    def _wanted(label: str) -> bool:
        return needle is None or needle in label.lower()

    scope = f" (filter: {args.filter})" if needle else ""
    print(f"Comparing {args.file1.name} vs {args.file2.name}{scope}")
    print(
        f"  top-level objects: {len(doc1.root)} vs {len(doc2.root)} "
        f"({len(pairs)} matched)"
    )

    stats = _Stats()
    differing = 0
    for el1, el2 in pairs:
        label = _object_label(el1)
        if not _wanted(label):
            continue
        lines: list[str] = []
        _diff_pair(doc1, doc2, el1, el2, label, lines, stats, args.show_churn)
        if lines:
            differing += 1
            print(f"\n{label}: {len(lines)} difference(s)")
            for line in lines[: args.max_per_object]:
                print(line)
            if len(lines) > args.max_per_object:
                print(f"  ... {len(lines) - args.max_per_object} more")
            stats.diff_lines += len(lines)

    shown_only_left = [el for el in only_left if _wanted(_object_label(el))]
    shown_only_right = [el for el in only_right if _wanted(_object_label(el))]
    if shown_only_left:
        print(f"\nOnly in {args.file1.name} ({len(shown_only_left)}):")
        for element in shown_only_left:
            print(f"  {_object_label(element)}")
    if shown_only_right:
        print(f"\nOnly in {args.file2.name} ({len(shown_only_right)}):")
        for element in shown_only_right:
            print(f"  {_object_label(element)}")

    print(
        f"\nSummary{scope}: {differing} differing object(s), "
        f"{len(shown_only_left)} only in file1, {len(shown_only_right)} only in file2, "
        f"{stats.churn_suppressed} ObjectID/ObjectRef churn suppressed, "
        f"{stats.formatting_suppressed} formatting-only text diffs suppressed"
    )
    if stats.churn_suppressed and not args.show_churn:
        print("(use --show-churn to see raw renumbering)")
    return 1 if differing or shown_only_left or shown_only_right else 0


if __name__ == "__main__":
    sys.exit(main())
