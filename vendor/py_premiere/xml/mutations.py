"""Tree-mutation helpers that preserve Premiere's formatting.

New elements copy the whitespace of the sibling they displace, so the
serializer reproduces the file's indentation style without any re-formatting
pass.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET


def indent_tree(element: ET.Element, depth: int = 0) -> None:
    """Lay a freshly built tree out the way Premiere writes one.

    Every element in a `.prproj` sits at one tab per nesting level, with no
    exceptions, so a whole tree can be spaced in a single pass. An empty
    element is written self-closing ONLY when it is a pure reference;
    Premiere spells an empty content element out in full.

    For trees built from scratch (`models/skeleton.py`,
    `models/sequence_builder.py`) - the mutation helpers above are the ones
    that edit an already-formatted document in place.
    """
    children = list(element)
    close = "\n" + "\t" * depth
    if not children:
        if element.text is None and not _is_reference(element):
            element.text = close
        return
    pad = close + "\t"
    element.text = pad
    for child in children:
        indent_tree(child, depth + 1)
        child.tail = pad
    children[-1].tail = close


def _is_reference(element: ET.Element) -> bool:
    return "ObjectRef" in element.attrib or "ObjectURef" in element.attrib


def build_leaf(parent: ET.Element, tag: str, text: str) -> ET.Element:
    """Add `<tag>text</tag>` to a tree that `indent_tree` will space later.

    The counterpart to `append_leaf`: this one writes no whitespace, because
    the builders lay the whole tree out in one pass at the end. Use
    `append_leaf` to add a leaf to a document that is already formatted.
    """
    element = ET.SubElement(parent, tag)
    element.text = text
    return element


def append_uniform_child(container: ET.Element, element: ET.Element) -> None:
    """Append `element` to a container whose children share their spacing.

    Works for the top-level object table (`root`) and any list element
    (`Items`, ...): children are separated by a uniform tail, and only the
    last carries the terminal tail before the closing tag. The appended
    element takes over that terminal tail; the previous last takes the
    inter-child tail - so the serializer reproduces the layout with no
    reformatting pass.
    """
    if len(container) < 2:
        raise ValueError("container has too few children to infer spacing")
    last = container[-1]
    element.tail = last.tail
    last.tail = container[-2].tail
    container.append(element)


def append_pair(container: ET.Element, first: str, object_ref: str) -> ET.Element:
    """Append a `<Marker><First>..</First><Second ObjectRef=..></Marker>` pair.

    `container` is the inner list element (`Markers/Markers`); the whitespace
    of the existing pairs is matched, with the new pair becoming the last.
    """
    pairs = list(container)
    indent = "\n\t\t\t"
    pair = ET.Element("Marker", {"Version": "1", "Index": str(len(pairs))})
    pair.text = indent + "\t"
    first_element = ET.SubElement(pair, "First")
    first_element.text = first
    first_element.tail = indent + "\t"
    second = ET.SubElement(pair, "Second", {"ObjectRef": object_ref})
    second.tail = indent
    pair.tail = indent[:-1]
    if pairs:
        pairs[-1].tail = indent
    else:
        container.text = indent
    container.append(pair)
    return pair


def append_child(
    parent: ET.Element, element: ET.Element, empty_indent: str | None = None
) -> ET.Element:
    """Append `element` as the last child, preserving indentation.

    Matches the whitespace of the existing children; for an empty parent the
    child indent is derived by deepening the parent's own closing whitespace
    by one tab, or from `empty_indent` when the parent carries no text to
    deepen. Works at any nesting depth (nothing is hard-coded), so the
    serializer reproduces the layout with no reflow.
    """
    children = list(parent)
    if len(children) >= 2:
        element.tail = children[-1].tail
        children[-1].tail = children[-2].tail
    elif len(children) == 1:
        element.tail = children[0].tail
        children[0].tail = parent.text
    else:
        element.tail = parent.text or empty_indent
        parent.text = (parent.text or empty_indent or "") + "\t"
    parent.append(element)
    return element


def append_leaf(parent: ET.Element, tag: str, text: str) -> ET.Element:
    """Append `<tag>text</tag>` as the last child, preserving indentation."""
    element = ET.Element(tag)
    element.text = text
    return append_child(parent, element)


def set_elided_flag(
    parent: ET.Element, tag: str, present: bool, text: str = "true"
) -> None:
    """Write `<tag>text</tag>`, or remove it - the shape Premiere elides.

    The format stores most booleans only in their non-default state, so
    clearing one means deleting the element rather than writing the opposite
    value. `present` is whether the element should be STORED, which for a
    field whose default is the written state (a sync lock, say) is the
    inverse of the domain value.
    """
    existing = parent.find(tag)
    if present:
        if existing is None:
            append_leaf(parent, tag, text)
        else:
            existing.text = text
    elif existing is not None:
        remove_child(parent, existing)


def remove_child(parent: ET.Element, element: ET.Element) -> None:
    """Remove `element` from `parent`, preserving surrounding whitespace.

    The removed element's tail (the whitespace before whatever followed it)
    is handed to the preceding sibling - or to the parent's text when it was
    the first child - so the layout closes up with no reformatting.
    """
    previous = None
    for child in parent:
        if child is element:
            break
        previous = child
    else:
        raise ValueError(f"<{element.tag}> is not a child of <{parent.tag}>")
    if previous is not None:
        previous.tail = element.tail
    else:
        parent.text = element.tail
    parent.remove(element)


def insert_before(parent: ET.Element, anchor_tag: str, element: ET.Element) -> None:
    """Insert an already-built `element` before the `anchor_tag` child.

    The new element takes over the whitespace that preceded the anchor, so
    the anchor keeps its indentation and nothing reflows.
    """
    children = list(parent)
    for index, child in enumerate(children):
        if child.tag == anchor_tag:
            element.tail = children[index - 1].tail if index else parent.text
            parent.insert(index, element)
            return
    raise ValueError(f"no <{anchor_tag}> child in <{parent.tag}>")


def insert_leaf_before(
    parent: ET.Element, anchor_tag: str, tag: str, text: str
) -> ET.Element:
    """Insert `<tag>text</tag>` before the `anchor_tag` child of `parent`.

    The new element takes over the whitespace that preceded the anchor (the
    anchor keeps identical leading whitespace through the new element's
    tail), so indentation is preserved byte-for-byte around the insertion.
    """
    element = ET.Element(tag)
    element.text = text
    try:
        insert_before(parent, anchor_tag, element)
    except ValueError:
        raise ValueError(
            f"no <{anchor_tag}> child in <{parent.tag}> to anchor <{tag}>"
        ) from None
    return element
