"""Lumetri Color on a placed clip, for converted Avid Color Correction.

The effect is copied from a Premiere-saved template (data/lumetri_template.xml, from
py-premiere's sample 67): Lumetri is ~130 ordinary component params, and py-premiere verified
that Premiere keeps a written scalar through open and resave. RGB Curves are the 2080-byte
arbitrary-data param after "HDR Range": four 520-byte records (master, red, green, blue), each
u32 0, u32 point count, 32 (f64 x, f64 y) pairs in 0..1 (PRPROJ-0026).

Premiere 27 retires the Lumetri panel but keeps the Lumetri Color effect, and old projects open
unchanged (Adobe helpx "Color mode basics"); Color mode effects are a later target.
"""
from __future__ import annotations

import base64
import struct
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path

TEMPLATE = Path(__file__).with_name("data") / "lumetri_template.xml"
CURVE_ORDER = ("master", "red", "green", "blue")
CURVES_PARAM_INDEX = 53  # position in the template's Params list (after "HDR Range")
RECORD_SIZE = 520
MAX_POINTS = 32


def binary_hash(payload: bytes) -> str:
    """A BinaryHash shaped like Premiere's own: 96 bits of content hash, then the payload length
    + 12 as the last 32 bits (Premiere 26.5 saves: 15156-byte Blob -> ...00003b40, 59016-byte
    embedded LUT -> ...0000e694). A random UUID there made Premiere call a project damaged when it
    carried large payloads (User, 2026-10-15, LUT write test)."""
    import hashlib
    h = hashlib.md5(payload).hexdigest()[:20] + f"{len(payload) + 12:012x}"[-12:]
    h = h[:20] + "0000" + f"{len(payload) + 12:08x}" if len(payload) + 12 < 2**32 else h
    return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}"


def set_payload(document, value: ET.Element, payload: bytes) -> None:
    """Give an arbitrary-data element a new payload without breaking Premiere's store-once rule.
    Premiere keeps each BinaryHash's bytes in ONE element; other uses are empty elements with the
    same hash. If this element is that stored copy, its text first moves to another element still
    using the old hash; overwriting it in place left those copies pointing at nothing, and
    Premiere 26.5 then called the project damaged (User, 2026-10-15/19, LUT tests)."""
    old_hash, old_text = value.get("BinaryHash"), (value.text or "").strip()
    if old_hash and old_text:
        for other in document.root.iter(value.tag):
            if other is not value and other.get("BinaryHash") == old_hash and not (other.text or "").strip():
                other.text = value.text
                break
    value.set("BinaryHash", binary_hash(payload))
    value.text = base64.b64encode(payload).decode("ascii") + "\n\t\t"
    document._by_binary_hash = None


def _param(document, component: ET.Element, index: int) -> ET.Element:
    ref = component.find(f"Component/Params/Param[@Index='{index}']")
    return document.resolve(ref)


def _named(document, component: ET.Element, name: str) -> ET.Element:
    """The first param with this display name (Lumetri repeats some names in later sections;
    the first is the Basic Correction one)."""
    for ref in component.findall("Component/Params/Param"):
        param = document.resolve(ref)
        if param.findtext("Name") == name:
            return param
    raise ValueError(f"Lumetri template has no {name!r} param")


def _number(value: float) -> str:
    # Premiere writes whole values with a trailing dot ("100.").
    return f"{int(value)}." if float(value).is_integer() else repr(float(value))


def _set_scalar(param: ET.Element, value: float) -> None:
    leaf = param.find("StartKeyframe")
    fields = leaf.text.split(",")
    fields[1] = _number(value)
    leaf.text = ",".join(fields)


def curve_payload(curves: dict[str, list[tuple[float, float]]]) -> tuple[bytes, list[str]]:
    """The 2080-byte RGB Curves value. Points are clamped to 0..1 (Lumetri's range); untouched
    curves are the identity (0,0)-(1,1)."""
    notes = []
    out = b""
    for name in CURVE_ORDER:
        points = list(curves.get(name) or [(0.0, 0.0), (1.0, 1.0)])
        if len(points) > MAX_POINTS:
            raise ValueError(f"{name} curve has {len(points)} points; Lumetri stores at most {MAX_POINTS}")
        clamped = [(min(max(x, 0.0), 1.0), min(max(y, 0.0), 1.0)) for x, y in points]
        if clamped != points:
            notes.append(f"{name} curve points outside Premiere's range were clamped")
        out += struct.pack("<II", 0, len(clamped))
        flat = [v for p in clamped for v in p] + [0.0] * (2 * (MAX_POINTS - len(clamped)))
        out += struct.pack(f"<{2 * MAX_POINTS}d", *flat)
    return out, notes


def apply_lumetri(project, placed, saturation: float | None = None,
                  curves: dict[str, list[tuple[float, float]]] | None = None) -> list[str]:
    """Add a Lumetri Color effect to a placed video clip and set it. Returns report notes."""
    document = project._document
    already = set(document._index_payloads())
    fragment = ET.fromstring(TEMPLATE.read_text(encoding="utf-8"))
    spliced = project._splice_fragment(fragment)
    component = spliced["VideoFilterComponent"]
    params = [_param(document, component, i) for i in range(len(component.findall("Component/Params/Param")))]
    # Premiere stores a binary payload once per BinaryHash; later copies are empty elements.
    for param in params:
        value = param.find("StartKeyframeValue")
        if value is not None and value.get("BinaryHash") in already:
            value.text = None
    document._by_binary_hash = None

    notes = []
    _set_scalar(_named(document, component, "Exposure"), 0.0)  # the template's own +1.0
    if saturation is not None:
        _set_scalar(_named(document, component, "Saturation"), saturation)
    if curves:
        param = params[CURVES_PARAM_INDEX]
        value = param.find("StartKeyframeValue")
        if value is None or len(document.payload(value) or b"") != 4 * RECORD_SIZE:
            raise ValueError("Lumetri template: RGB Curves param not where expected")
        payload, notes = curve_payload(curves)
        set_payload(document, value, payload)

    chain = document.resolve(placed._element.find("ClipTrackItem/ComponentOwner/Components"))
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
    used = [2]  # intrinsic Motion 1, Opacity 2
    for entry in entries:
        existing = document.resolve(entry).findtext("Component/ID")
        if existing and existing.isdigit():
            used.append(int(existing))
    component.find("Component/ID").text = str(max(used) + 1)
    if entries:
        entries[-1].tail = "\n\t\t\t\t"
    entry = ET.SubElement(components, "Component", {"Index": str(len(entries)), "ObjectRef": component.get("ObjectID")})
    entry.tail = "\n\t\t\t"
    return notes


LOOK_PATH_PARAM, EMBEDDED_LUT_PARAM, BLOB_PARAM, COLOR_SPACE_PARAM = 32, 122, 0, 37
BLOB_TEMPLATE = Path(__file__).with_name("data") / "lumetri_look_blob.xml"


def write_cube(path: Path, points, size: int, title: str) -> Path:
    """Write at an unused filename without replacing existing entries; return the chosen path."""
    # .cube titles are ASCII quoted strings; project/sequence names remain untouched.
    title = "".join(c if " " <= c <= "~" and c != '"' else "?" for c in title)
    lines = [f'TITLE "{title}"', f"LUT_3D_SIZE {size}", "DOMAIN_MIN 0.0 0.0 0.0", "DOMAIN_MAX 1.0 1.0 1.0"]
    lines += ["%.6f %.6f %.6f" % p for p in points]
    text = "\n".join(lines) + "\n"
    number = 1
    while True:
        candidate = path if number == 1 else path.with_name(f"{path.stem} ({number}){path.suffix}")
        try:
            file = candidate.open("x", encoding="ascii")
        except FileExistsError:
            number += 1
            continue
        with file:
            file.write(text)
        return candidate


def embedded_lut(lut_id: bytes, points, size: int) -> bytes:
    """Premiere's 'Embedded LUTs' value (PRPROJ-0027): 16-byte ID, u32 length of the rest,
    'ITX3', size, channels 3, 0, f32 domain min x3 and max x3, then size^3 f32 RGB, red fastest."""
    body = struct.pack("<4sIII3f3f", b"ITX3", size, 3, 0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
    body += b"".join(struct.pack("<3f", *p) for p in points)
    return lut_id + struct.pack("<I", len(body)) + body


def apply_lumetri_lut(project, placed, points, size: int, cube_path: Path) -> None:
    """Lumetri Color with a Creative Look LUT embedded in the project, so it renders even if the
    .cube file is missing (User read-back, PRPROJ-0027). The XML Blob is required: without it the
    Look's name shows but the LUT is not applied."""
    import uuid as _uuid
    apply_lumetri(project, placed)
    document = project._document
    chain = document.resolve(placed._element.find("ClipTrackItem/ComponentOwner/Components"))
    component = document.resolve(chain.find("ComponentChain/Components")[-1])
    params = [document.resolve(r) for r in component.findall("Component/Params/Param")]
    lut_id = _uuid.uuid4().bytes
    path = str(cube_path)
    blob = (BLOB_TEMPLATE.read_text(encoding="utf-8").replace("{{LUT_PATH}}", path)
            .replace("{{LUT_ID}}", lut_id.hex().upper()).replace("{{GUID}}", str(_uuid.uuid4())))
    set_payload(document, params[LOOK_PATH_PARAM].find("StartKeyframeValue"), (path + "\x00").encode("utf-16-le"))
    set_payload(document, params[EMBEDDED_LUT_PARAM].find("StartKeyframeValue"), embedded_lut(lut_id, points, size))
    set_payload(document, params[BLOB_PARAM].find("StartKeyframeValue"), blob.encode("utf-8"))
    leaf = params[COLOR_SPACE_PARAM].find("StartKeyframe")
    fields = leaf.text.split(",")
    fields[1] = "28"  # Premiere 26.5's own value (popup, written without a trailing dot)
    leaf.text = ",".join(fields)
