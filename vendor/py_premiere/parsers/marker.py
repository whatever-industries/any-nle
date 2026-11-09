"""Parse marker collections."""

from __future__ import annotations

from typing import TYPE_CHECKING

from ..models import Marker

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..xml import PremiereDocument


def parse_markers(document: PremiereDocument, owner: ET.Element) -> list[Marker]:
    """Parse the markers of an owner element (`MarkerOwner/Markers` link)."""
    ref = owner.find("MarkerOwner/Markers")
    if ref is None:
        return []
    container = document.resolve(ref)
    markers: list[Marker] = []
    for pair in container.findall("Markers/Marker"):
        second = pair.find("Second")
        if second is not None:
            markers.append(Marker._from_xml(document.resolve(second)))
    return markers
