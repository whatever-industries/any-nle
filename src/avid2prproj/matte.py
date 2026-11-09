"""Avid AniMatte as Premiere masks on the clip's Opacity.

The clip's Opacity intrinsic is materialised as Premiere's GUI does (py-premiere
`_materialize_opacity`, 81_blend_modes) and each Avid shape becomes one default `AEMask2`
sub-component (py-premiere mask builder, 26_effect_mask) whose Path is the shape, Type 0 (pen),
Feather 0, not inverted (Avid Key In shows the inside).

Path value, Premiere 26.5 form (tesseract-converter's Premiere-saved fixtures
feature_keyed_mask_path_26_5_saved / feature_mask_tracker_26_5, third-party): u32 2, u32 1
(one outline), byte 0x56, u32 (payload length - 13), u32 2, u32 n, then per vertex u32 flag
(1 smooth, 0 corner) and f32 x, y, in_x, in_y, out_x, out_y as fractions of the clip's source
frame with ABSOLUTE handle positions, then a final byte 1. A moving shape stores
`ticks,base64;` per key in `Keyframes` with IsTimeVarying true. Key times here are the clip's
source time (source in + frame), the "owner's source clock"; not yet confirmed in Premiere.
"""
from __future__ import annotations

import base64
import struct
import xml.etree.ElementTree as ET

from py_premiere.enums import BlendMode
from py_premiere.models.mask_builder import attach_sub_mask, build_mask_component, build_mask_params

from .lumetri import set_payload

PATH_PARAM, TYPE_PARAM = 9, 25
MASK_OPACITY_ID, INVERTED_ID = "15", "17"  # ParameterIDs (Inverted: tesseract-converter, 26.5 form)


def _set_value(param: ET.Element, text: str) -> None:
    leaf = param.find("StartKeyframe")
    fields = leaf.text.split(",")
    fields[1] = text
    leaf.text = ",".join(fields)


def path_payload(points) -> bytes:
    body = struct.pack("<II", 2, len(points))
    for x, y, ix, iy, ox, oy in points:
        corner = ix == iy == ox == oy == 0.0
        body += struct.pack("<I6f", 0 if corner else 1, x, y, x + ix, y + iy, x + ox, y + oy)
    body += b"\x01"
    # The length word counts what follows it (whole value - 13 bytes): 121 for 4 vertices.
    return struct.pack("<II", 2, 1) + b"\x56" + struct.pack("<I", len(body)) + body


def _opacity(project, placed) -> ET.Element:
    document = project._document
    chain = document.resolve(placed._element.find("ClipTrackItem/ComponentOwner/Components"))
    if chain.find("DefaultOpacity") is not None:
        placed._materialize_opacity(BlendMode.NORMAL)
    for ref in chain.findall("ComponentChain/Components/Component"):
        component = document.resolve(ref)
        if component.findtext("MatchName") == "AE.ADBE Opacity":
            return component
    raise ValueError("clip has no Opacity component")


def apply_matte(project, placed, matte, source_in: int, unit: int) -> list[str]:
    """Add one Opacity mask per matte shape to a placed video clip. Returns report notes."""
    document = project._document
    opacity = _opacity(project, placed)
    notes = []
    for index, keys in enumerate(matte.shapes):
        param_ids = build_mask_params(document)
        mask_id, _ = build_mask_component(document, param_ids, clip_role=False, instance_name=f"{index + 1:02d}")
        attach_sub_mask(opacity, mask_id)
        params = [document.by_object_id[i] for i in param_ids]
        path = params[PATH_PARAM]
        set_payload(document, path.find("StartKeyframeValue"), path_payload(keys[0][1]))
        by_id = {p.findtext("ParameterID"): p for p in params}
        shape_opacity = matte.opacities[index] if matte.opacities else 100.0
        if shape_opacity != 100.0:
            _set_value(by_id[MASK_OPACITY_ID], f"{int(shape_opacity)}." if shape_opacity.is_integer() else repr(shape_opacity))
        if matte.inverted:
            _set_value(by_id[INVERTED_ID], "true")
        if len(keys) > 1:
            control = path.find("ParameterControlType")
            varying = ET.Element("IsTimeVarying")
            varying.text = "true"
            varying.tail = control.tail
            path.insert(list(path).index(control) + 1, varying)
            position = path.find("StartKeyframePosition")
            frames = ET.Element("Keyframes")
            frames.text = "".join(f"{source_in + frame * unit},{base64.b64encode(path_payload(points)).decode('ascii')};"
                                  for frame, points in keys)
            frames.tail = position.tail
            path.insert(list(path).index(position) + 1, frames)
    if len(matte.shapes) > 1:
        notes.append(f"{len(matte.shapes)} AniMatte shapes as {len(matte.shapes)} Opacity masks (Premiere adds masks together)")
    return notes
