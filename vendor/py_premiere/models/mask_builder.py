"""Synthesize Premiere's default `AEMask2` mask component.

The default-state parameter table and binary payloads come from
`data/mask_template.py`, read off the 26_effect_mask fixture (regenerate
with `scripts/dev/gen_mask_template.py`). A mask appears in two roles with
the same parameters: attached to an EFFECT (a `SubComponents` entry on the
filter element) or applied to the CLIP (its own chain, referenced from the
track item as `SelectionComponents`).
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING

from ..data.mask_template import (
    MASK_CLASS_ID,
    MASK_PARAMS,
    MASK_VERSION,
    PRIVATE_DATA,
    SELECTION_CHAIN,
)
from ..xml.mutations import insert_before
from .media_import import _leaf, _top

if TYPE_CHECKING:
    from ..xml import PremiereDocument

MASK_MATCH_NAME = "AE.ADBE AEMask2"


def build_mask_params(document: PremiereDocument) -> list[str]:
    """Register the 27 default mask parameters; returns their ObjectIDs.

    Arbitrary-data payloads are written in full only when the document does
    not already store their hash - Premiere keeps ONE copy per `BinaryHash`
    and hash-references the rest (the fixture's clip mask carries the
    payloads its effect mask references).
    """
    ids = []
    for tag, class_id, version, fields in MASK_PARAMS:
        element = _top(tag, class_id, version)
        children = []
        for child_tag, text, attrs in fields:
            child = ET.SubElement(element, child_tag, dict(attrs or {}))
            binary_hash = (attrs or {}).get("BinaryHash")
            if text is not None and not (
                binary_hash and document.payload_stored(binary_hash)
            ):
                child.text = text
            child.tail = "\n\t\t"
            children.append(child)
        children[-1].tail = "\n\t"
        ids.append(document.add_object(element))
    # New payload carriers may have joined the store.
    document._by_binary_hash = None
    return ids


def build_mask_component(
    document: PremiereDocument,
    param_ids: list[str],
    clip_role: bool,
    instance_name: str = "01",
) -> tuple[str, ET.Element]:
    """Register the mask's `VideoFilterComponent`; returns (ObjectID, element).

    The two roles share the parameter set but not the shell: the clip role
    carries the `NextComponentNumber` bag, a chain-local `ID` and
    `Intrinsic`; the effect role an `InstanceName` numbered by position -
    `01`, `02`, ... (26_effect_mask, 76_two_masks).
    """
    element = _top("VideoFilterComponent", MASK_CLASS_ID, MASK_VERSION)
    inner = ET.SubElement(element, "Component", {"Version": "7"})
    inner.text = "\n\t\t\t"
    inner.tail = "\n\t\t"
    if clip_role:
        node = ET.SubElement(inner, "Node", {"Version": "1"})
        node.text = "\n\t\t\t\t"
        node.tail = "\n\t\t\t"
        properties = ET.SubElement(node, "Properties", {"Version": "1"})
        properties.text = "\n\t\t\t\t\t"
        properties.tail = "\n\t\t\t"
        key = ET.SubElement(properties, "BE.VideoFilterComponent.NextComponentNumber")
        key.text = "2"
        key.tail = "\n\t\t\t\t"
    params = ET.SubElement(inner, "Params", {"Version": "1"})
    params.text = "\n\t\t\t\t"
    params.tail = "\n\t\t\t"
    for index, param_id in enumerate(param_ids):
        entry = ET.SubElement(
            params, "Param", {"Index": str(index), "ObjectRef": param_id}
        )
        entry.tail = "\n\t\t\t\t" if index < len(param_ids) - 1 else "\n\t\t\t"
    if clip_role:
        _leaf(inner, "ID", "1", "\n\t\t\t")
        _leaf(inner, "DisplayName", "Mask2", "\n\t\t\t")
        _leaf(inner, "Intrinsic", "true", "\n\t\t")
    else:
        _leaf(inner, "DisplayName", "Mask2", "\n\t\t\t")
        _leaf(inner, "InstanceName", instance_name, "\n\t\t")
    private = ET.SubElement(
        element,
        "PremiereFilterPrivateData",
        {"Encoding": "base64", "BinaryHash": PRIVATE_DATA[0]},
    )
    if not document.payload_stored(PRIVATE_DATA[0]):
        private.text = PRIVATE_DATA[1] + "\n\t\t"
    private.tail = "\n\t\t"
    _leaf(element, "MatchName", MASK_MATCH_NAME, "\n\t\t")
    _leaf(element, "VideoFilterType", "2", "\n\t")
    object_id = document.add_object(element)
    document._by_binary_hash = None
    return object_id, element


def attach_sub_mask(holder: ET.Element, mask_id: str) -> None:
    """Wire a registered mask into a holder's `SubComponents` list.

    Creates the list ahead of `MatchName` on the first attach and appends
    with the running index after (26_effect_mask, 76_two_masks).
    """
    subs = holder.find("SubComponents")
    if subs is None:
        subs = ET.Element("SubComponents", {"Version": "1"})
        subs.text = "\n\t\t\t"
        insert_before(holder, "MatchName", subs)
        index = 0
    else:
        entries = subs.findall("SubComponent")
        entries[-1].tail = "\n\t\t\t"
        index = len(entries)
    entry = ET.SubElement(
        subs, "SubComponent", {"Index": str(index), "ObjectRef": mask_id}
    )
    entry.tail = "\n\t\t"


def build_selection_chain(document: PremiereDocument, mask_id: str) -> str:
    """Register the clip-role `VideoComponentChain`; returns its ObjectID."""
    element = _top(SELECTION_CHAIN[0], SELECTION_CHAIN[1], SELECTION_CHAIN[2])
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t\t"
    chain.tail = "\n\t"
    components = ET.SubElement(chain, "Components", {"Version": "1"})
    components.text = "\n\t\t\t\t"
    components.tail = "\n\t\t"
    entry = ET.SubElement(components, "Component", {"Index": "0", "ObjectRef": mask_id})
    entry.tail = "\n\t\t\t"
    return document.add_object(element)
