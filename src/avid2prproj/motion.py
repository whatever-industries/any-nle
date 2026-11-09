"""Motion (Position, Scale, Rotation, Anchor Point, Crop) on a placed clip, for converted Avid
Resize and 3D Warp.

Premiere stores nothing for an untouched Motion: the clip's chain only says DefaultMotion. Once
changed, the chain loses DefaultMotion/DefaultMotionComponentID and holds the intrinsic
component (ID 1, AE.ADBE Motion) with its 11 params, as py-premiere's sample 05 shows for a 26.5
save (data/motion_template.xml; effects ledger in premiere-format-research/docs/effects-ledger.md).

Units: Position is where the source's Anchor Point lands, normalised to the sequence frame
(0.5:0.5 = centre, y down); Anchor Point is normalised to the source frame; Scale percent
("Scale" is the height when Uniform Scale is off; Scale Width the width); Rotation degrees
clockwise; Crop percent of the source per edge. Conventions from tesseract-converter's
Premiere-saved fixtures and AME renders; Crop is not render-measured yet.
"""
from __future__ import annotations

import xml.etree.ElementTree as ET
from pathlib import Path

from py_premiere.xml.mutations import remove_child

TEMPLATE = Path(__file__).with_name("data") / "motion_template.xml"
POSITION, SCALE, SCALE_WIDTH, UNIFORM, ROTATION, ANCHOR, CROP_LEFT, CROP_TOP, CROP_RIGHT, CROP_BOTTOM = 0, 1, 2, 3, 4, 5, 7, 8, 9, 10


def _number(value: float) -> str:
    # Premiere writes whole values with a trailing dot ("100."), points as plain decimals.
    return f"{int(value)}." if float(value).is_integer() else repr(float(value))


def _coord(value: float) -> str:
    return repr(float(value)) if not float(value).is_integer() else f"{int(value)}"


def _set(param: ET.Element, text: str) -> None:
    leaf = param.find("StartKeyframe")
    fields = leaf.text.split(",")
    fields[1] = text
    leaf.text = ",".join(fields)


def apply_motion(project, placed, transform) -> list[str]:
    """Materialise Motion on a placed video clip and set it from a model Transform. Returns notes."""
    document = project._document
    already = set(document._index_payloads())
    fragment = ET.fromstring(TEMPLATE.read_text(encoding="utf-8"))
    spliced = project._splice_fragment(fragment)
    component = spliced["VideoFilterComponent"]
    private = component.find("PremiereFilterPrivateData")
    if private.get("BinaryHash") in already:
        private.text = None  # stored once per BinaryHash; this copy is a reference
    document._by_binary_hash = None
    params = [document.resolve(r) for r in component.findall("Component/Params/Param")]

    notes = []
    x, y = transform.position
    _set(params[POSITION], f"{_coord(0.5 + x)}:{_coord(0.5 + y)}")
    ax, ay = transform.anchor
    _set(params[ANCHOR], f"{_coord(0.5 + ax)}:{_coord(0.5 + ay)}")
    sx, sy = transform.scale
    uniform = sx == sy
    _set(params[SCALE], _number(sy))
    _set(params[SCALE_WIDTH], _number(100.0 if uniform else sx))
    _set(params[UNIFORM], "true" if uniform else "false")
    _set(params[ROTATION], _number(transform.rotation))
    for index, value in zip((CROP_LEFT, CROP_TOP, CROP_RIGHT, CROP_BOTTOM), transform.crop):
        percent = min(max(value * 100.0, 0.0), 100.0)
        if percent != value * 100.0:
            notes.append("a crop outside Premiere's 0..100% was clamped")
        _set(params[index], _number(percent))

    chain = document.resolve(placed._element.find("ClipTrackItem/ComponentOwner/Components"))
    for tag in ("DefaultMotion", "DefaultMotionComponentID"):
        flag = chain.find(tag)
        if flag is not None:
            remove_child(chain, flag)
    inner = chain.find("ComponentChain")
    components = inner.find("Components")
    if components is None:
        components = ET.Element("Components", {"Version": "1"})
        components.text = "\n\t\t\t\t"
        components.tail = "\n\t\t"
        if len(inner):
            list(inner)[-1].tail = "\n\t\t\t"
        else:
            inner.text = "\n\t\t\t"
        inner.append(components)
    entries = components.findall("Component")
    if entries:
        entries[-1].tail = "\n\t\t\t\t"
    entry = ET.SubElement(components, "Component", {"Index": str(len(entries)), "ObjectRef": component.get("ObjectID")})
    entry.tail = "\n\t\t\t"
    return notes
