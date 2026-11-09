"""Serializer reproducing Premiere's exact XML output style.

Premiere's writer is uniform: LF line endings, tab indentation, decimal
character references, self-closing empty elements, and a `\\n\\n` epilogue
after the root element. Data newlines exist only inside leaf-element text,
stored as `&#10;`/`&#13;`; every raw LF byte in the file is formatting.
ElementTree collapses both onto `\\n` at parse time, so escaping decisions
below are made from element shape: leaf text is data (entity-escaped),
branch text and tails are formatting (passed through raw, and refused when
non-whitespace - mixed content would make the distinction ambiguous).

These rules are validated two ways: the byte-identity round-trip suite over
the sample corpus, and a parse-time self-check in
`PremiereDocument.from_bytes` that refuses any file these rules cannot
reproduce byte-for-byte.
"""

from __future__ import annotations

import re
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

PROLOG = b'<?xml version="1.0" encoding="UTF-8" ?>\n'
EPILOGUE = b"\n\n"

# The close-tag indent of a block-form leaf is `\n` plus at least one tab;
# a bare trailing `\n` is data (e.g. `WorkspaceDefinition` ends `&#10;</...>`).
_TRAILING_FORMATTING = re.compile(r"\n\t+\Z")

_TEXT_ENTITIES = {"\r": "&#13;", "\n": "&#10;"}
_ATTR_ENTITIES = {'"': "&quot;", "\r": "&#13;", "\n": "&#10;", "\t": "&#9;"}


def _is_formatting_only(text: str) -> bool:
    # A data value that is pure whitespace would have had its newlines stored
    # as `&#10;`; raw newlines in childless-element text only occur in empty
    # containers serialized in expanded form. The from_bytes self-check turns
    # any file violating this into a loud parse error.
    return "\n" in text and text.isspace()


def _write_leaf_text(parts: list[str], text: str) -> None:
    if _is_formatting_only(text):
        # Body of an empty container written in expanded form (`<Tag>\n\t</Tag>`).
        parts.append(text)
        return
    # Blob leaves are written in block form (`<Tag>BASE64==\n\t</Tag>`): a
    # final raw newline + indent precedes the close tag. Split that trailing
    # run off and escape only the data head.
    match = _TRAILING_FORMATTING.search(text)
    if match:
        parts.append(escape(text[: match.start()], _TEXT_ENTITIES))
        parts.append(match.group())
    else:
        parts.append(escape(text, _TEXT_ENTITIES))


def _write_element(parts: list[str], element: ET.Element) -> None:
    parts.append("<" + element.tag)
    for name, value in element.attrib.items():
        parts.append(" " + name + '="' + escape(value, _ATTR_ENTITIES) + '"')
    if not len(element) and not element.text:
        parts.append("/>")
        return
    parts.append(">")
    if element.text:
        if len(element):
            if not element.text.isspace():
                raise ValueError(
                    f"mixed content in <{element.tag}>: text alongside child elements"
                )
            parts.append(element.text)
        else:
            _write_leaf_text(parts, element.text)
    for child in element:
        _write_element(parts, child)
        if child.tail:
            if not child.tail.isspace():
                raise ValueError(
                    f"mixed content in <{element.tag}>: text after <{child.tag}>"
                )
            parts.append(child.tail)
    parts.append("</" + element.tag + ">")


def serialize_document(root: ET.Element) -> bytes:
    """Serialize a document tree to Premiere-style XML payload bytes."""
    parts: list[str] = []
    _write_element(parts, root)
    return PROLOG + "".join(parts).encode("utf-8") + EPILOGUE


def serialize_element(element: ET.Element) -> bytes:
    """Serialize a single element subtree (no prolog/epilogue), for display."""
    parts: list[str] = []
    _write_element(parts, element)
    return "".join(parts).encode("utf-8")
