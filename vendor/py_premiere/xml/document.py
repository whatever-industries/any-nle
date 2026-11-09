"""Parse and save `.prproj` documents with byte fidelity."""

from __future__ import annotations

import base64
import re
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import TYPE_CHECKING

from .gzip_io import compress_prproj, decompress_prproj
from .mutations import append_uniform_child, remove_child
from .serializer import serialize_document

if TYPE_CHECKING:
    from .gzip_io import GzipFraming

#: A stored payload keeps the line breaks the serializer wraps it in.
_WHITESPACE = re.compile(r"\s+")


class ReferenceIndex:
    """Which top-level objects point at which, from one pass over the tree.

    A snapshot, deliberately not cached on the document: every edit that adds
    or drops a reference invalidates it. A caller removing several objects
    builds one and hands it to each step instead of paying the pass again.
    """

    def __init__(self, document: PremiereDocument) -> None:
        self.referrers: dict[int, set[int]] = {}
        self.targets_of: dict[int, list[ET.Element]] = {}
        for top in document.root:
            for node in top.iter():
                if id(node) in document._scoped:
                    # Inline-definition subtrees (project view state, column
                    # classes) reuse ids from their own scope; their refs do
                    # not point at top-level objects.
                    continue
                for attribute, index in (
                    ("ObjectRef", document.by_object_id),
                    ("ObjectURef", document.by_object_uid),
                ):
                    ref = node.get(attribute)
                    if ref is None:
                        continue
                    target = index.get(ref)
                    if target is None or target is top:
                        continue
                    self.referrers.setdefault(id(target), set()).add(id(top))
                    self.targets_of.setdefault(id(top), []).append(target)

    def referrers_outside(
        self, target: ET.Element, ignoring: list[ET.Element]
    ) -> set[int]:
        """Ids of the top-level objects still pointing at `target`."""
        return self.referrers.get(id(target), set()) - {
            id(element) for element in ignoring
        }


def _divergence_context(produced: bytes, expected: bytes) -> str:
    limit = min(len(produced), len(expected))
    offset = limit
    for i in range(limit):
        if produced[i] != expected[i]:
            offset = i
            break
    lo = max(0, offset - 32)
    return (
        f"first divergence at payload offset {offset}: "
        f"reserialized {produced[lo : offset + 32]!r} vs "
        f"original {expected[lo : offset + 32]!r}"
    )


class PremiereDocument:
    """A parsed `.prproj`: the element tree plus its gzip framing.

    `by_object_id` / `by_object_uid` index the top-level object table (direct
    children of `PremiereData`), built once at parse; mutating the tree does
    not update them. Inline object definitions nested deeper (the project
    view state and its column classes) reuse IDs from their own scope; they
    are not indexed and `resolve` refuses refs located inside them.
    """

    def __init__(self, root: ET.Element, framing: GzipFraming | None) -> None:
        self.root = root
        self.framing = framing
        self.by_object_id: dict[str, ET.Element] = {}
        self.by_object_uid: dict[str, ET.Element] = {}
        # Built on the first `payload()` that needs it: most documents never
        # ask, and the walk touches every element.
        self._by_binary_hash: dict[str, bytes] | None = None
        #: Highest integer ObjectID handed out so far; `next_object_id` reads
        #: it instead of rescanning the table.
        self._highest_object_id = 0
        for element in root:
            object_id = element.get("ObjectID")
            if object_id is not None:
                if object_id in self.by_object_id:
                    raise ValueError(f"duplicate ObjectID {object_id!r}")
                self.by_object_id[object_id] = element
                if object_id.isdigit():
                    self._highest_object_id = max(
                        self._highest_object_id, int(object_id)
                    )
            object_uid = element.get("ObjectUID")
            if object_uid is not None:
                if object_uid in self.by_object_uid:
                    raise ValueError(f"duplicate ObjectUID {object_uid!r}")
                self.by_object_uid[object_uid] = element
        # Everything under a nested (non-top-level) ObjectID/ObjectUID carrier
        # lives in that object's own ID scope.
        self._scoped: set[int] = set()
        for top in root:
            for nested in top.iter():
                if nested is top:
                    continue
                if (
                    nested.get("ObjectID") is not None
                    or nested.get("ObjectUID") is not None
                ):
                    for descendant in nested.iter():
                        self._scoped.add(id(descendant))

    @classmethod
    def from_bytes(cls, data: bytes) -> PremiereDocument:
        xml_bytes, framing = decompress_prproj(data)
        try:
            root = ET.fromstring(xml_bytes)
        except ET.ParseError as error:
            # ParseError is a SyntaxError; callers of a file reader expect a
            # ValueError for "this is not a project file".
            raise ValueError(f"not a Premiere project file: {error}") from None
        document = cls(root, framing)
        # Self-check: ElementTree normalizes away constructs the serializer
        # cannot reproduce (comments, PIs, CDATA, entity forms, writer idioms
        # outside the known set). Any such file would silently corrupt on
        # save, so refuse it loudly at parse time instead.
        reserialized = document.to_xml_bytes()
        if reserialized != xml_bytes:
            raise ValueError(
                "cannot faithfully round-trip this file; "
                + _divergence_context(reserialized, xml_bytes)
            )
        return document

    def resolve(self, element: ET.Element) -> ET.Element:
        """Follow an element's `ObjectRef`/`ObjectURef` to its target element."""
        if id(element) in self._scoped:
            raise ValueError(
                f"<{element.tag}> is inside a scoped inline-definition subtree; "
                "its refs do not resolve through the top-level object table"
            )
        ref = element.get("ObjectRef")
        if ref is not None:
            try:
                return self.by_object_id[ref]
            except KeyError:
                raise ValueError(f"unknown ObjectID {ref!r}") from None
        uref = element.get("ObjectURef")
        if uref is not None:
            try:
                return self.by_object_uid[uref]
            except KeyError:
                raise ValueError(f"unknown ObjectUID {uref!r}") from None
        raise ValueError(f"<{element.tag}> carries no ObjectRef/ObjectURef")

    def payload(self, element: ET.Element) -> bytes | None:
        """Decode a base64 element's payload, following the hash index.

        Premiere stores a given binary payload ONCE and writes every further
        copy of it as an empty element carrying the same `BinaryHash` - a
        timeline caption's styled text and a Motion Graphics template's
        parameter values are both stored that way. An empty copy therefore
        has to be resolved through the hash to the populated one.

        `None` when the element holds no payload and no copy of its hash
        does either.
        """
        text = (element.text or "").strip()
        if not text:
            binary_hash = element.get("BinaryHash")
            if binary_hash is None:
                return None
            if self._by_binary_hash is None:
                self._by_binary_hash = self._index_payloads()
            return self._by_binary_hash.get(binary_hash)
        return base64.b64decode(_WHITESPACE.sub("", text))

    def payload_stored(self, binary_hash: str) -> bool:
        """Whether some element already carries this payload's text.

        Writers use this to follow Premiere's store-once rule: the first
        occurrence of a payload carries the base64 text, every further copy
        is an empty element with the same `BinaryHash`.
        """
        if self._by_binary_hash is None:
            self._by_binary_hash = self._index_payloads()
        return binary_hash in self._by_binary_hash

    def _index_payloads(self) -> dict[str, bytes]:
        payloads: dict[str, bytes] = {}
        for element in self.root.iter():
            binary_hash = element.get("BinaryHash")
            text = (element.text or "").strip()
            if binary_hash is None or not text or binary_hash in payloads:
                continue
            payloads[binary_hash] = base64.b64decode(_WHITESPACE.sub("", text))
        return payloads

    def next_object_id(self) -> str:
        """The next free integer ObjectID in the top-level object table.

        Answered from a high-water mark kept in step by `attach_object`, so
        creating N objects costs N rather than N times the table size.
        """
        return str(self._highest_object_id + 1)

    def add_object(self, element: ET.Element) -> str:
        """Append a new top-level object under a freshly minted ObjectID."""
        object_id = self.next_object_id()
        if "ObjectID" in element.attrib:
            element.set("ObjectID", object_id)
        else:
            # Premiere writes the identifier ahead of ClassID/Version on every
            # one of the 11k top-level objects in the corpus, and the
            # serializer emits attributes in insertion order - so setting it
            # on an element that has none has to put it in front, not append.
            rest = dict(element.attrib)
            element.attrib.clear()
            element.attrib["ObjectID"] = object_id
            element.attrib.update(rest)
        self.attach_object(element)
        return object_id

    def attach_object(self, element: ET.Element) -> ET.Element:
        """Append a top-level object that already carries its identifier.

        For objects Premiere keys by `ObjectUID`, and for graphs whose IDs
        were allocated up front so their internal refs could be wired before
        anything was spliced in.
        """
        append_uniform_child(self.root, element)
        self._index_object(element)
        # The new object may carry a binary payload other elements will
        # hash-reference.
        self._by_binary_hash = None
        return element

    def remove_object(self, element: ET.Element) -> None:
        """Detach a top-level object and drop it from the indexes."""
        remove_child(self.root, element)
        object_id = element.get("ObjectID")
        if object_id is not None:
            self.by_object_id.pop(object_id, None)
        object_uid = element.get("ObjectUID")
        if object_uid is not None:
            self.by_object_uid.pop(object_uid, None)
        # It may have been the carrier of a hash-shared payload.
        self._by_binary_hash = None

    def owned_objects(
        self, seeds: list[ET.Element], index: ReferenceIndex | None = None
    ) -> list[ET.Element]:
        """Top-level objects reachable from `seeds` that nothing else needs.

        The graph Premiere deletes along with a panel item (master clip,
        template clips, source, media, streams...). A shared object - Media
        another item still references, say - survives because its external
        referrer keeps it out of the set. Pass `index` to spend one pass over
        the tree across several removals instead of one per call.
        """
        if index is None:
            index = ReferenceIndex(self)
        deleted: dict[int, ET.Element] = {id(seed): seed for seed in seeds}
        changed = True
        while changed:
            changed = False
            for element_id in list(deleted):
                for target in index.targets_of.get(element_id, []):
                    if id(target) in deleted:
                        continue
                    external = index.referrers.get(id(target))
                    if external is None or external <= deleted.keys():
                        deleted[id(target)] = target
                        changed = True
        return list(deleted.values())

    def _index_object(self, element: ET.Element) -> None:
        object_id = element.get("ObjectID")
        if object_id is not None:
            self.by_object_id[object_id] = element
            if object_id.isdigit():
                self._highest_object_id = max(self._highest_object_id, int(object_id))
        object_uid = element.get("ObjectUID")
        if object_uid is not None:
            self.by_object_uid[object_uid] = element

    def to_xml_bytes(self) -> bytes:
        """The decompressed XML payload (the byte-fidelity contract)."""
        return serialize_document(self.root)

    def to_bytes(self) -> bytes:
        """The full file as written to disk (gzip framing applied)."""
        return compress_prproj(self.to_xml_bytes(), self.framing)

    def save(self, path: str | Path) -> None:
        """Write the document to `path`, overwriting it if it exists.

        The document layer carries no policy: `Project.save` is the one that
        refuses to overwrite and keeps the project's own path in step.
        """
        Path(path).write_bytes(self.to_bytes())


def parse_prproj(path: str | Path) -> PremiereDocument:
    """Parse a `.prproj` file from disk."""
    return PremiereDocument.from_bytes(Path(path).read_bytes())
