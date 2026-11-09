"""Parse component chains (effects) of a track item."""

from __future__ import annotations

from typing import TYPE_CHECKING

from ..models import Component, ComponentParam

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..models import TrackItem
    from ..xml import PremiereDocument


def _inner_component(element: ET.Element) -> ET.Element | None:
    inner = element.find("Component")
    if inner is None:
        inner = element.find("AudioComponent/Component")
    return inner


def _build_component(
    document: PremiereDocument, element: ET.Element, track_item: TrackItem
) -> Component:
    # A component owns its parameters and, where an effect carries masks, a
    # nested chain of sub-components (a mask is itself a component).
    component = Component(element, track_item)
    # Sub-components sit on the OUTER element, beside MatchName - unlike the
    # parameters, which live inside the nested <Component>.
    for sub_ref in element.findall("SubComponents/SubComponent"):
        component._sub_components.append(
            _build_component(document, document.resolve(sub_ref), track_item)
        )
    inner = _inner_component(element)
    if inner is None:
        return component
    for param_ref in inner.findall("Params/Param"):
        param_element = document.resolve(param_ref)
        component._properties.append(ComponentParam(param_element, document, component))
    return component


def _parse_chain(
    document: PremiereDocument, track_item: TrackItem, chain_ref: ET.Element | None
) -> list[Component]:
    if chain_ref is None:
        return []
    chain = document.resolve(chain_ref)
    return [
        _build_component(document, document.resolve(component_ref), track_item)
        for component_ref in chain.findall("ComponentChain/Components/Component")
    ]


def parse_components(
    document: PremiereDocument,
    track_item: TrackItem,
    clip_track_item: ET.Element,
) -> list[Component]:
    """Parse the materialized components of a track item.

    Untouched intrinsics (Motion, Opacity, ...) are synthesized by Premiere
    at runtime and leave no elements; only modified components appear.
    """
    return _parse_chain(
        document, track_item, clip_track_item.find("ComponentOwner/Components")
    )


def parse_selection_components(
    document: PremiereDocument,
    track_item: TrackItem,
    element: ET.Element,
) -> list[Component]:
    """Parse a track item's selection (mask) components.

    A mask applied to the CLIP rather than to a single effect lives in its
    own chain, referenced from the track item as `SelectionComponents`
    instead of the `ComponentOwner/Components` chain the effects use.
    """
    return _parse_chain(document, track_item, element.find("SelectionComponents"))
