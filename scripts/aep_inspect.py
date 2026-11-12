#!/usr/bin/env python3
"""Inspect saved AEP composition and source identities for format research."""

import argparse
from fractions import Fraction
import hashlib
import json
import math
from pathlib import Path
import struct
from xml.etree import ElementTree


MAX_BYTES = 32 * 1024 * 1024
MAX_DEPTH = 32
MAX_CHUNKS = 100_000
MAX_TEXT = 4096
MAX_KEYS = 16_384
MAX_MASK_VERTICES = 16_384
ITEM_TYPES = {1: "folder", 4: "composition", 7: "footage"}
XML_NAMESPACE = "{http://www.adobe.com/products/aftereffects}"
TRANSFORM_FIELDS = {
    b"ADBE Position": ("position", 3, "composition_pixels"),
    b"ADBE Anchor Point": ("anchor", 3, "source_fraction_xy_pixels_z"),
    b"ADBE Scale": ("scale", 3, "fraction"),
    b"ADBE Rotate Z": ("rotation", 1, "degrees"),
    b"ADBE Opacity": ("opacity", 1, "fraction"),
}
ROTATION_3D_FIELDS = {
    b"ADBE Rotate X": ("rotation_x", 1, "degrees"),
    b"ADBE Rotate Y": ("rotation_y", 1, "degrees"),
}
KEY_INTERPOLATION = {1: "linear", 2: "bezier", 3: "hold"}
# Map only the saved property families with verified key layouts.
KEY_LAYOUTS = {
    ("position", 3): "spatial",
    ("anchor", 3): "spatial",
    ("opacity", 0): "scalar",
    ("scale", 0): "vector",
    ("feather", 0): "vector",
}
MATTE_MODES = {0: "none", 1: "alpha", 2: "alpha_inverted", 3: "luma", 4: "luma_inverted"}
BLEND_MODES = {
    2: "normal",
    4: "add",
    5: "multiply",
    6: "screen",
    7: "overlay",
    8: "soft_light",
    9: "hard_light",
    10: "darken",
    11: "lighten",
    25: "exclusion",
    26: "difference",
    27: "color_dodge",
    28: "color_burn",
}
MARKER_CUE_TYPES = {4: "event", 5: "navigation"}
MARKER_TEXT_FIELDS = ("comment", "chapter", "url", "frame_target", "cue_name")
MASK_MODES = dict(enumerate(("none", "add", "subtract", "intersect", "lighten", "darken", "difference")))
MASK_FIELDS = {
    b"ADBE Mask Feather": ("feather", 2, "source_pixels"),
    b"ADBE Mask Opacity": ("opacity", 1, "fraction"),
    b"ADBE Mask Offset": ("expansion", 1, "source_pixels"),
}
# Match names identify the saved control independently of localized or user
# names. Other effects remain visible in stack order with unresolved controls.
EFFECT_SCALARS = {
    b"ADBE Slider Control": ((1, 10, "slider_value"),),
    b"ADBE Checkbox Control": ((1, 4, "boolean"),),
    b"ADBE Angle Control": ((1, 3, "degrees"),),
    b"ADBE Gaussian Blur 2": ((1, 10, "effect_native"), (3, 4, "boolean")),
}
EFFECT_POINTS = {
    b"ADBE Point Control": (1,),
    b"ADBE Ramp": (1, 3),
}
EFFECT_LAYER_INPUTS = {
    b"ADBE Layer Control": 1,
    b"ADBE Set Matte3": 1,
    b"ADBE Displacement Map": 1,
    b"ADBE Compound Blur": 1,
}


def u32(body, offset):
    return struct.unpack_from(">I", body, offset)[0]


def exact_time(body, offset, denominator_offset):
    numerator = struct.unpack_from(">i", body, offset)[0]
    denominator = u32(body, denominator_offset)
    return {"numerator": numerator, "denominator": denominator} if denominator else None


def cyclic_parent_ids(layers):
    parents = {layer["id"]: layer["parent_layer_id"] for layer in layers}
    visited = set()
    cycles = set()
    for layer_id in parents:
        path = {}
        current = layer_id
        while current and current in parents and current not in visited and current not in path:
            path[current] = len(path)
            current = parents[current]
        if current in path:
            start = path[current]
            cycles.update(node for node, index in path.items() if index >= start)
        visited.update(path)
    return cycles


class Inspector:
    def __init__(self, data):
        self.data = memoryview(data)
        self.chunk_count = 0
        self.key_count = 0
        self.mask_vertex_count = 0
        self.items = {}
        self.point_definitions = {}

    def chunks(self, body):
        offset = 0
        while offset < len(body):
            if len(body) - offset < 8:
                raise ValueError("truncated chunk header")
            tag, length = struct.unpack_from(">4sI", body, offset)
            start = offset + 8
            end = start + length
            offset = end + (length & 1)
            if offset > len(body):
                raise ValueError("chunk or padding crosses its parent boundary")
            self.chunk_count += 1
            if self.chunk_count > MAX_CHUNKS:
                raise ValueError("chunk count exceeds inspection limit")
            if tag == b"LIST":
                if length < 4:
                    raise ValueError("LIST has no type")
                yield (tag, bytes(body[start:start + 4])), body[start + 4:end]
            else:
                yield (tag, None), body[start:end]

    @staticmethod
    def one(chunks, tag, list_type=None, optional=False):
        fields = [value for key, value in chunks if key == (tag, list_type)]
        if len(fields) == 1:
            return fields[0]
        if optional and not fields:
            return None
        raise ValueError(f"expected one {list_type or tag!r} field")

    @staticmethod
    def name(chunks):
        # The display name is the first direct Utf8 slot. Other slots can
        # appear on footage items and contain separate logging fields.
        for key, body in chunks:
            if key == (b"Utf8", None):
                if len(body) > MAX_TEXT:
                    raise ValueError("name exceeds inspection limit")
                return bytes(body).decode("utf-8")
        return None

    def footage(self, chunks):
        pin = self.one(chunks, b"LIST", b"Pin ", optional=True)
        if pin is None:
            return {"saved_locator": None}
        fields = list(self.chunks(pin))
        facts = {"saved_locator": None}
        profile = self.one(fields, b"sspc", optional=True)
        if profile is not None and len(profile) >= 38:
            tag = bytes(profile[22:26])
            if all(32 <= code < 127 for code in tag):
                facts["saved_footage_type"] = tag.decode("ascii")
            raster = list(struct.unpack_from(">II", profile, 30))
            if all(raster):
                facts["saved_raster"] = raster
                if tag == b"XCEX" and len(profile) == 222:
                    timing = self.movie_timing(profile)
                    if timing is not None:
                        facts["saved_movie_timing"] = timing
        reference = self.one(fields, b"LIST", b"Als2", optional=True)
        if reference is None:
            return facts
        alias = self.one(self.chunks(reference), b"alas", optional=True)
        if alias is None:
            return facts
        if len(alias) > 16 * 1024:
            raise ValueError("footage alias exceeds inspection limit")
        data = json.loads(bytes(alias).decode("utf-8"))
        if not isinstance(data, dict) or not isinstance(data.get("fullpath"), str):
            raise ValueError("footage alias has no saved path")
        facts["saved_locator"] = data["fullpath"]
        return facts

    @staticmethod
    def movie_timing(profile):
        # The native rate and duration remain cached separately from
        # the Interpret Footage override and effective rate.
        frames, duration_rate = struct.unpack_from(">II", profile, 38)
        native_rate = u32(profile, 56)
        conform_rate, effective_rate = struct.unpack_from(">II", profile, 146)
        if not frames or not native_rate or native_rate != duration_rate:
            return None
        if effective_rate != (conform_rate or native_rate):
            return {"state": "unqualified", "stored_rate_words": [native_rate, conform_rate, effective_rate]}
        return {
            "state": "saved",
            "frame_count": frames,
            "native_rate": {"numerator": native_rate, "denominator": 1},
            "conform_rate": {"numerator": conform_rate, "denominator": 1} if conform_rate else None,
            "effective_rate": {"numerator": effective_rate, "denominator": 1},
        }

    def layer(self, body):
        chunks = list(self.chunks(body))
        header = self.one(chunks, b"ldta")
        if len(header) < 140:
            raise ValueError("truncated layer header")
        blend_mode = header[99]
        facts = {
            "id": u32(header, 0),
            "saved_name": self.name(chunks),
            "source_item_id": u32(header, 40),
            "parent_layer_id": u32(header, 132),
            "type_code": u32(header, 128),
            "video_enabled": bool(header[39] & 1),
            "blend_mode": {"mode_code": blend_mode, "mode": BLEND_MODES.get(blend_mode)},
            "start": exact_time(header, 12, 16),
            "relative_in": exact_time(header, 20, 24),
            "relative_out": exact_time(header, 28, 32),
            "stretch": exact_time(header, 8, 108),
        }
        matte_mode = header[107]
        matte_id = u32(header, 160) if len(header) >= 164 else None
        if matte_mode or matte_id:
            facts["track_matte"] = {
                "mode_code": matte_mode,
                "mode": MATTE_MODES.get(matte_mode),
                "layer_id": matte_id,
            }
        groups = self.layer_groups(chunks)
        time_remap = self.saved_time_remap(groups)
        if time_remap is not None:
            facts["time_remap"] = time_remap
        markers = self.saved_markers(groups)
        if markers:
            facts["markers"] = markers
        masks = self.saved_masks(groups)
        if masks:
            facts["masks"] = masks
        effects = self.saved_effects(groups, facts["id"])
        if effects:
            facts["effects"] = effects
        three_dimensional = bool(header[38] & 0x04)
        if three_dimensional:
            facts["three_dimensional"] = True
        if (
            facts["type_code"] == 0
            and facts["source_item_id"] not in (0, 0xffff_ffff)
            and not (header[38] & 0x80)
        ):
            transform = self.saved_transform(
                groups, parented=facts["parent_layer_id"] != 0, three_dimensional=three_dimensional
            )
            if transform:
                facts["saved_transform"] = transform
        return facts

    def layer_groups(self, layer):
        root = self.one(layer, b"LIST", b"tdgp", optional=True)
        return list(self.chunks(root)) if root is not None else []

    def named_fields(self, chunks):
        for (key, raw), following in zip(chunks, chunks[1:]):
            if key == (b"tdmn", None):
                if len(raw) > MAX_TEXT:
                    raise ValueError("match name exceeds inspection limit")
                yield bytes(raw).rstrip(b"\0"), following

    def effect_name(self, chunks, tag):
        # RIFX wraps these two text records in ordinary chunks rather than LIST.
        payload = self.one(chunks, tag, optional=True)
        if payload is None:
            payload = self.one(chunks, b"LIST", tag, optional=True)
        return self.name(self.chunks(payload)) if payload is not None else None

    def saved_effects(self, groups, layer_id):
        parades = [field for name, field in self.named_fields(groups) if name == b"ADBE Effect Parade"]
        if not parades:
            return []
        if len(parades) != 1 or parades[0][0] != (b"LIST", b"tdgp"):
            raise ValueError("ambiguous After Effects effect parade")
        parade = list(self.chunks(parades[0][1]))
        instances = [
            (name, data) for name, (key, data) in self.named_fields(parade) if key == (b"LIST", b"sspc")
        ]
        if len(instances) != sum(key == (b"LIST", b"sspc") for key, _ in parade):
            raise ValueError("After Effects effect instance has no match name")
        effects = []
        for name, data in instances:
            fields = list(self.chunks(data))
            group = self.one(fields, b"LIST", b"tdgp", optional=True)
            values = list(self.chunks(group)) if group is not None else []
            user_name = self.effect_name(values, b"tdsn")
            instance = {
                "match_name": name.decode("ascii"),
                "effect_name": self.effect_name(fields, b"fnam"),
                "user_name": user_name if user_name != "-_0_/-" else None,
                "controls_state": "unqualified",
            }
            effects.append(instance)
            profile = EFFECT_SCALARS.get(name)
            points = EFFECT_POINTS.get(name)
            layer_input = EFFECT_LAYER_INPUTS.get(name)
            if not (profile or points or layer_input):
                continue
            # Only read values from this layer's instance. An EfdG definition
            # can carry a different instance's saved values.
            owner = self.effect_parameter(values, name + b"-0000", (b"LIST", b"tdbs"))
            if owner is None:
                continue
            owner_id = self.one(self.chunks(owner), b"tdpi", optional=True)
            if owner_id is None or len(owner_id) != 4 or not layer_id or u32(owner_id, 0) != layer_id:
                continue
            local = self.one(fields, b"LIST", b"parT", optional=True)
            definitions = list(self.chunks(local)) if local is not None else []
            if layer_input:
                control = name + f"-{layer_input:04d}".encode()
                value = self.effect_parameter(values, control, (b"LIST", b"tdbs"))
                definition = self.effect_parameter(definitions, control, (b"pard", None))
                instance["controls_state"] = "selected_layer_inputs"
                instance["layer_inputs"] = [{
                    "match_name": control.decode("ascii"),
                    **self.effect_layer_input(value, definition),
                }]
                continue
            instance["controls_state"] = "selected_scalars" if profile else "selected_points"
            instance["controls"] = []
            for index, kind, unit in profile or ((index, 6, "layer_raster_fraction") for index in points):
                control = name + f"-{index:04d}".encode()
                value = self.effect_parameter(values, control, (b"LIST", b"tdbs"))
                definition = self.effect_parameter(definitions, control, (b"pard", None))
                if points:
                    shared = not definitions and definition is None
                    if shared:
                        definition = self.point_definitions.get((name, control))
                    fact = self.effect_point(value, definition, unit, shared)
                else:
                    fact = self.effect_scalar(value, definition, kind, unit, name == b"ADBE Slider Control")
                instance["controls"].append({
                    "match_name": control.decode("ascii"),
                    **fact,
                })
        return effects

    def effect_layer_input(self, value, definition):
        unresolved = {"state": "unqualified"}
        if value is None or (definition is not None and (len(definition) != 148 or u32(definition, 12) != 0)):
            return unresolved
        fields = list(self.chunks(value))
        state = self.one(fields, b"tdsb", optional=True)
        header = self.one(fields, b"tdb4", optional=True)
        if (state is None or len(state) != 4 or u32(state, 0) != 1 or header is None or len(header) != 124
                or header[:8] != b"\xdb\x99\x00\x01\x00\x01\x00\x00"):
            return unresolved
        if any(header[flag] for flag in (76, 77, 119, 120)) or self.name(fields) is not None:
            return {"state": "expression_or_computed"}
        target = self.one(fields, b"tdpi", optional=True)
        mode = self.one(fields, b"tdps", optional=True)
        value_bytes = self.one(fields, b"cdat", optional=True)
        if (header[68] or self.one(fields, b"LIST", b"list", optional=True) is not None
                or target is None or len(target) != 4 or (mode is not None and len(mode) != 4)
                or value_bytes is None or len(value_bytes) != 40 or any(value_bytes)):
            return unresolved
        return {"state": "saved", "target_layer_id": u32(target, 0), "tdps_code": u32(mode, 0) if mode else None}

    def read_point_definitions(self, chunks):
        table = self.one(chunks, b"LIST", b"EfdG", optional=True)
        if table is None:
            return
        ambiguous = set()
        for key, entry in self.chunks(table):
            if key != (b"LIST", b"EfDf"):
                continue
            fields = list(self.chunks(entry))
            for name, (key, body) in self.named_fields(fields):
                if name not in EFFECT_POINTS or key != (b"LIST", b"sspc"):
                    continue
                params = self.one(self.chunks(body), b"LIST", b"parT", optional=True)
                if params is None:
                    continue
                definitions = list(self.chunks(params))
                for index in EFFECT_POINTS[name]:
                    control = name + f"-{index:04d}".encode()
                    definition = self.effect_parameter(definitions, control, (b"pard", None))
                    ref = (name, control)
                    if definition is None:
                        continue
                    if ref in self.point_definitions or ref in ambiguous:
                        self.point_definitions.pop(ref, None)
                        ambiguous.add(ref)
                    else:
                        self.point_definitions[ref] = definition

    def effect_point(self, value, definition, unit, shared):
        unresolved = {"state": "unqualified", "unit": unit}
        if definition is not None and (len(definition) != 148 or u32(definition, 12) != 6):
            return unresolved
        if value is None:
            if definition is None:
                return unresolved
            # Point defaults are fixed-point layer fractions. Only the type
            # definition may be shared, never its effect instance's values.
            state = "shared_default" if shared else "local_default"
            return {"state": state, "unit": unit,
                    "values": [v / 65536 for v in struct.unpack_from(">2i", definition, 56)]}
        fields = list(self.chunks(value))
        state_word = self.one(fields, b"tdsb", optional=True)
        header = self.one(fields, b"tdb4", optional=True)
        if (state_word is None or len(state_word) != 4 or u32(state_word, 0) != 1
                or header is None or len(header) != 124 or header[:4] != b"\xdb\x99\x00\x02"):
            return unresolved
        if any(header[flag] for flag in (76, 77, 119, 120)) or self.name(fields) is not None:
            return {"state": "expression_or_computed", "unit": unit}
        keys = self.one(fields, b"LIST", b"list", optional=True)
        data = self.one(fields, b"cdat", optional=True)
        if keys is not None:
            if header[4:8] == b"\x00\x0e\x00\x03" and header[68] == 1 and data is None:
                return self.saved_keys(keys, u32(header, 12), unit, "spatial", 2)
            return unresolved
        if header[4:8] != b"\x00\x0f\x00\x03" or header[68] or data is None or len(data) != 48:
            return unresolved
        values = struct.unpack_from(">2d", data, 0)
        return {"state": "saved", "unit": unit, "values": list(values)} if all(map(math.isfinite, values)) else unresolved

    def effect_parameter(self, fields, name, expected):
        matches = [field for match, field in self.named_fields(fields) if match == name]
        if not matches:
            return None
        if len(matches) != 1 or matches[0][0] != expected:
            raise ValueError("ambiguous After Effects effect parameter")
        return matches[0][1]

    def effect_scalar(self, value, definition, kind, unit, allow_keys):
        unresolved = {"state": "unqualified", "unit": unit}
        if definition is not None and (len(definition) != 148 or u32(definition, 12) != kind):
            return unresolved
        if value is None:
            if definition is None:
                return unresolved
            if kind == 10:
                scalar = struct.unpack_from(">d", definition, 56)[0]
            elif kind == 3:
                scalar = struct.unpack_from(">i", definition, 56)[0] / 65536
            else:
                scalar = u32(definition, 56)
            state = "local_default"
        else:
            fields = list(self.chunks(value))
            state_word = self.one(fields, b"tdsb", optional=True)
            header = self.one(fields, b"tdb4", optional=True)
            if (state_word is None or len(state_word) != 4 or u32(state_word, 0) != 1
                    or header is None or len(header) != 124 or header[:4] != b"\xdb\x99\x00\x01"):
                return unresolved
            if any(header[flag] for flag in (76, 77, 119, 120)) or self.name(fields) is not None:
                return {"state": "expression_or_computed", "unit": unit}
            keys = self.one(fields, b"LIST", b"list", optional=True)
            data = self.one(fields, b"cdat", optional=True)
            if keys is not None:
                if allow_keys and header[5] == 0 and not header[7] and header[68] == 1 and data is None:
                    return self.saved_keys(keys, u32(header, 12), unit, "scalar", 1)
                return unresolved
            if header[5] != 1 or header[7] or header[68] or data is None or len(data) != 40:
                return unresolved
            scalar = struct.unpack_from(">d", data, 0)[0]
            state = "saved"
        if not math.isfinite(scalar) or (kind == 4 and scalar not in (0, 1)):
            return unresolved
        return {"state": state, "unit": unit, "value": bool(scalar) if kind == 4 else scalar}

    def saved_time_remap(self, groups):
        stores = [
            following
            for (key, name), following in zip(groups, groups[1:])
            if key == (b"tdmn", None) and bytes(name).rstrip(b"\0") == b"ADBE Time Remapping"
        ]
        if not stores:
            return None
        if len(stores) != 1 or stores[0][0] != (b"LIST", b"tdbs"):
            raise ValueError("ambiguous After Effects time remap property")
        fields = list(self.chunks(stores[0][1]))
        state = self.one(fields, b"tdsb", optional=True)
        header = self.one(fields, b"tdb4", optional=True)
        if (state is None or len(state) != 4 or header is None or len(header) != 124
                or header[:4] != b"\xdb\x99\x00\x01"):
            return {"state": "unqualified"}
        key_list = self.one(fields, b"LIST", b"list", optional=True)
        if any(header[flag] for flag in (76, 77, 120)):
            return {"state": "expression_or_computed"}
        if u32(state, 0) == 3 and header[5] & 1 and not header[68] and key_list is None:
            return None
        if (u32(state, 0) != 1 or header[5] & 1 or header[7] or header[68] != 1
                or key_list is None or self.one(fields, b"cdat", optional=True) is not None):
            return {"state": "unqualified"}
        return self.saved_keys(key_list, u32(header, 12), "source_seconds", "scalar", 1)

    def saved_masks(self, groups):
        parades = [
            body
            for (name_key, name), (key, body) in zip(groups, groups[1:])
            if name_key == (b"tdmn", None)
            and bytes(name).rstrip(b"\0") == b"ADBE Mask Parade"
            and key == (b"LIST", b"tdgp")
        ]
        if len(parades) > 1:
            raise ValueError("ambiguous After Effects mask group")
        if not parades:
            return []
        entries = list(self.chunks(parades[0]))
        masks = []
        for i, (key, name) in enumerate(entries):
            if key != (b"tdmn", None) or bytes(name).rstrip(b"\0") != b"ADBE Mask Atom":
                continue
            if (
                i + 2 >= len(entries)
                or entries[i + 1][0] != (b"mkif", None)
                or entries[i + 2][0] != (b"LIST", b"tdgp")
            ):
                raise ValueError("After Effects mask header and atom disagree")
            header = entries[i + 1][1]
            if len(header) < 8 or header[0] not in (0, 1):
                raise ValueError("invalid After Effects mask header")
            masks.append(self.mask_atom(header, entries[i + 2][1]))
        if len(masks) != sum(key == (b"mkif", None) for key, _ in entries):
            raise ValueError("unpaired After Effects mask header")
        return masks

    def mask_atom(self, header, body):
        fields = list(self.chunks(body))
        names = [value for key, value in fields if key in ((b"tdsn", None), (b"LIST", b"tdsn"))]
        if len(names) > 1:
            raise ValueError("ambiguous After Effects mask name")
        mode = u32(header, 4)
        mask = {
            "saved_name": self.name(self.chunks(names[0])) if names else None,
            "mode_code": mode,
            "mode": MASK_MODES.get(mode),
            "inverted": bool(header[0]),
        }
        for (key, match_name), (record, value) in zip(fields, fields[1:]):
            if key != (b"tdmn", None):
                continue
            match_name = bytes(match_name).rstrip(b"\0")
            if match_name == b"ADBE Mask Shape":
                name = "path"
                if record != (b"LIST", b"om-s"):
                    raise ValueError("invalid After Effects mask path record")
                result = self.mask_path(value)
            elif match_name in MASK_FIELDS:
                name, components, unit = MASK_FIELDS[match_name]
                if record != (b"LIST", b"tdbs"):
                    raise ValueError("invalid After Effects mask property record")
                result = self.transform_value(value, components, unit, name=name)
            else:
                continue
            if name in mask:
                raise ValueError(f"duplicate After Effects mask property {name}")
            mask[name] = result
        mask.setdefault("path", {"state": "not_saved"})
        return mask

    def mask_path(self, body):
        fields = list(self.chunks(body))
        prop = list(self.chunks(self.one(fields, b"LIST", b"tdbs")))
        header = self.one(prop, b"tdb4")
        if len(header) != 124 or header[:4] != b"\xdb\x99\x00\x01":
            return {"state": "unqualified"}
        if any(header[flag] for flag in (76, 77, 120)):
            return {"state": "expression_or_computed"}
        key_list = self.one(prop, b"LIST", b"list", optional=True)
        shape_list = self.one(fields, b"LIST", b"omks")
        shapes = [value for key, value in self.chunks(shape_list) if key == (b"LIST", b"shap")]
        if header[68] or key_list is not None:
            if header[68] != 1 or key_list is None:
                raise ValueError("inconsistent After Effects mask key state")
            timescale = u32(header, 12)
            records = self.key_records(key_list, timescale, 64)
            if len(shapes) != len(records) // 64:
                raise ValueError("After Effects mask keys and shapes disagree")
            keys = []
            for index, shape in enumerate(shapes):
                offset = index * 64
                timing = self.key_timing(records, offset, keys)
                timing["saved_ease"] = self.key_ease(struct.unpack_from(">4d", records, offset + 24))
                timing["geometry"] = self.mask_geometry(shape)
                keys.append(timing)
            return {"state": "keyed", "key_count": len(keys), "layer_timescale": timescale, "keys": keys}
        if len(shapes) != 1:
            raise ValueError("expected one static After Effects mask shape")
        return self.mask_geometry(shapes[0])

    def mask_geometry(self, body):
        fields = list(self.chunks(body))
        header = self.one(fields, b"shph")
        if len(header) != 24 or header[:3] != b"\xb3\xde\x02" or header[3] not in (1, 9) or header[20] != 1:
            return {"state": "unqualified"}
        left, top, right, bottom = struct.unpack_from(">4f", header, 4)
        if not all(map(math.isfinite, (left, top, right, bottom))) or left > right or top > bottom:
            raise ValueError("invalid After Effects mask bounds")
        point_list = self.one(fields, b"LIST", b"list")
        points = list(self.chunks(point_list))
        counts = self.one(points, b"lhd3")
        data = self.one(points, b"ldat")
        if len(counts) != 52 or u32(counts, 0) != 0x00D00BEE or u32(counts, 16) != 8:
            raise ValueError("unsupported After Effects mask point list header")
        count = u32(counts, 8)
        vertices, remainder = divmod(count, 3)
        if remainder or not vertices or vertices != u32(counts, 12) or len(data) != count * 8:
            raise ValueError("invalid After Effects mask point list extent")
        if vertices > MAX_MASK_VERTICES - self.mask_vertex_count:
            raise ValueError("mask vertex count exceeds inspection limit")
        self.mask_vertex_count += vertices
        coordinates = []
        for x, y in struct.iter_unpack(">ff", data):
            if not math.isfinite(x) or not math.isfinite(y):
                raise ValueError("non-finite After Effects mask point")
            coordinates.append([left + (right - left) * x, top + (bottom - top) * y])
        return {
            "state": "static",
            "closed": not bool(header[3] & 8),
            "unit": "source_fraction",
            "bounds_ltrb": [left, top, right, bottom],
            "vertices": coordinates[0::3],
            "out_handles": coordinates[1::3],
            "in_handles": [coordinates[-1], *coordinates[2:-1:3]],
        }

    def saved_transform(self, groups, *, parented=False, three_dimensional=False):
        transform = [
            body
            for ((tag, kind), name), (key, body) in zip(groups, groups[1:])
            if (tag, kind) == (b"tdmn", None)
            and bytes(name).rstrip(b"\0") == b"ADBE Transform Group"
            and key == (b"LIST", b"tdgp")
        ]
        if len(transform) > 1:
            raise ValueError("ambiguous After Effects transform group")
        if not transform:
            return {}
        fields = list(self.chunks(transform[0]))
        saved = {}
        for (key, match_name), (record, body) in zip(fields, fields[1:]):
            if key != (b"tdmn", None):
                continue
            match_name = bytes(match_name).rstrip(b"\0")
            if three_dimensional and match_name == b"ADBE Orientation":
                if "orientation" in saved or record != (b"LIST", b"otst"):
                    raise ValueError("invalid or duplicate After Effects Orientation record")
                saved["orientation"] = self.saved_orientation(body)
                continue
            if record != (b"LIST", b"tdbs"):
                continue
            field = TRANSFORM_FIELDS.get(match_name)
            if field is None and three_dimensional:
                field = ROTATION_3D_FIELDS.get(match_name)
            if field is None:
                continue
            name, components, unit = field
            if name == "position" and parented:
                unit = "parent_layer_pixels"
            if name in saved:
                raise ValueError(f"duplicate After Effects transform property {name}")
            saved[name] = self.transform_value(body, components, unit, name=name)
        return saved

    def saved_orientation(self, body):
        fields = list(self.chunks(body))
        property_fields = list(self.chunks(self.one(fields, b"LIST", b"tdbs")))
        header = self.one(property_fields, b"tdb4")
        if len(header) != 124 or header[:4] != b"\xdb\x99\x00\x01":
            return {"state": "unqualified"}
        if any(header[flag] for flag in (76, 77, 120)):
            return {"state": "expression_or_computed"}
        if header[68] or self.one(property_fields, b"LIST", b"list", optional=True) is not None:
            return {"state": "keyed"}
        container = list(self.chunks(self.one(fields, b"LIST", b"otky")))
        if len(container) != 1 or container[0][0] != (b"otda", None):
            return {"state": "unqualified"}
        data = container[0][1]
        if len(data) != 24:
            raise ValueError("invalid After Effects Orientation value extent")
        values = struct.unpack(">3d", data)
        if not all(map(math.isfinite, values)):
            return {"state": "unqualified"}
        return {"state": "static", "unit": "degrees", "values": list(values)}

    def transform_value(self, body, components, unit, *, name=None):
        fields = list(self.chunks(body))
        header = self.one(fields, b"tdb4")
        if len(header) != 124 or header[:3] != b"\xdb\x99\x00" or header[3] != components:
            return {"state": "unqualified"}
        if any(header[flag] for flag in (76, 77, 120)):
            return {"state": "expression_or_computed"}
        key_list = self.one(fields, b"LIST", b"list", optional=True)
        if header[68] or key_list is not None:
            layout = KEY_LAYOUTS.get((name, header[7]))
            if layout is None:
                return {"state": "keyed"}
            if header[68] != 1 or key_list is None:
                raise ValueError("inconsistent After Effects key list state")
            return self.saved_keys(key_list, u32(header, 12), unit, layout, components)
        value = self.one(fields, b"cdat", optional=True)
        if not (header[5] & 1) or value is None or len(value) < 8 * components:
            return {"state": "unqualified"}
        values = struct.unpack_from(">" + "d" * components, value)
        if not all(math.isfinite(component) for component in values):
            return {"state": "unqualified"}
        return {"state": "static", "unit": unit, "values": list(values)}

    def key_records(self, body, timescale, stride):
        fields = list(self.chunks(body))
        header = self.one(fields, b"lhd3")
        data = self.one(fields, b"ldat")
        if len(header) != 52 or u32(header, 0) != 0x00D00BEE or u32(header, 16) != stride:
            raise ValueError("unsupported After Effects key list header")
        count = u32(header, 8)
        if not timescale or not count or len(data) != count * stride:
            raise ValueError("invalid After Effects key list extent or clock")
        if count > MAX_KEYS - self.key_count:
            raise ValueError("key count exceeds inspection limit")
        self.key_count += count
        return data

    @staticmethod
    def key_timing(data, offset, keys):
        ticks, incoming, outgoing, flags = struct.unpack_from(">iBBH", data, offset)
        if keys and ticks <= keys[-1]["ticks"]:
            raise ValueError("After Effects keys are not in increasing time order")
        return {
            "ticks": ticks,
            "interpolation_in": KEY_INTERPOLATION.get(incoming, incoming),
            "interpolation_out": KEY_INTERPOLATION.get(outgoing, outgoing),
            "saved_flags": flags,
        }

    @staticmethod
    def key_ease(values):
        if not all(map(math.isfinite, values)):
            raise ValueError("non-finite After Effects key ease")
        return dict(zip(("in_speed", "in_influence", "out_speed", "out_influence"), values))

    def saved_keys(self, body, timescale, unit, layout, components):
        stride = 56 + 24 * components if layout == "spatial" else 8 + 40 * components
        data = self.key_records(body, timescale, stride)
        keys = []
        for offset in range(0, len(data), stride):
            key = self.key_timing(data, offset, keys)
            if layout == "spatial":
                ease = struct.unpack_from(">4d", data, offset + 24)
                payload = struct.unpack_from(f">{3 * components}d", data, offset + 56)
                values = payload[:components]
                key["saved_ease"] = self.key_ease(ease)
                key["spatial_in_tangent"] = list(payload[components:2 * components])
                key["spatial_out_tangent"] = list(payload[2 * components:])
            else:
                payload = struct.unpack_from(f">{5 * components}d", data, offset + 8)
                values = payload[:components]
                ease_by_component = [self.key_ease(payload[index + components::components])
                                     for index in range(components)]
                if layout == "scalar":
                    key["saved_ease"] = ease_by_component[0]
                else:
                    key["saved_ease_by_component"] = ease_by_component
            if not all(map(math.isfinite, payload)):
                raise ValueError("non-finite After Effects key value")
            key["values"] = list(values)
            keys.append(key)
        return {"state": "keyed", "unit": unit, "layer_timescale": timescale, "keys": keys}

    def saved_markers(self, groups):
        stores = [
            body
            for (key, name), (record, body) in zip(groups, groups[1:])
            if key == (b"tdmn", None)
            and bytes(name).rstrip(b"\0") == b"ADBE Marker"
            and record == (b"LIST", b"mrst")
        ]
        if not stores:
            return []
        if len(stores) != 1:
            raise ValueError("ambiguous After Effects marker store")
        fields = list(self.chunks(stores[0]))
        prop = list(self.chunks(self.one(fields, b"LIST", b"tdbs")))
        header = self.one(prop, b"tdb4")
        if len(header) != 124 or header[:4] != b"\xdb\x99\x00\x01" or header[68] != 1:
            raise ValueError("unsupported After Effects marker property header")
        timescale = u32(header, 12)
        data = self.key_records(self.one(prop, b"LIST", b"list"), timescale, 16)
        payload = self.one(fields, b"LIST", b"mrky")
        entries = [body for kind, body in self.chunks(payload) if kind == (b"LIST", b"Nmrd")]
        if len(entries) != len(data) // 16:
            raise ValueError("After Effects marker times and records disagree")
        markers = []
        for index, body in enumerate(entries):
            ticks = struct.unpack_from(">i", data, index * 16)[0]
            if markers and ticks <= markers[-1]["time"]["numerator"]:
                raise ValueError("After Effects markers are not in increasing time order")
            marker = self.marker_value(body)
            marker["time"] = {"numerator": ticks, "denominator": timescale}
            markers.append(marker)
        return markers

    def marker_value(self, body):
        fields = list(self.chunks(body))
        header = self.one(fields, b"NmHd")
        if len(header) != 20:
            raise ValueError("unsupported After Effects marker value header")
        cue_code, pair_count, duration, timescale = struct.unpack_from(">4I", header)
        strings = [value for key, value in fields if key == (b"Utf8", None)]
        if not timescale or len(strings) != len(MARKER_TEXT_FIELDS) + pair_count * 2:
            raise ValueError("invalid After Effects marker duration clock or text slots")
        if any(len(value) > MAX_TEXT for value in strings):
            raise ValueError("After Effects marker text exceeds inspection limit")
        text = [bytes(value).decode("utf-8") for value in strings]
        marker = dict(zip(MARKER_TEXT_FIELDS, text[:len(MARKER_TEXT_FIELDS)], strict=True))
        cue_data = iter(text[len(MARKER_TEXT_FIELDS):])
        marker.update({
            "duration": {"numerator": duration, "denominator": timescale},
            "label": header[16],
            "cue_type_code": cue_code,
            "cue_type": MARKER_CUE_TYPES.get(cue_code),
            "cue_parameters": [list(pair) for pair in zip(cue_data, cue_data, strict=True)],
        })
        return marker

    def composition(self, chunks):
        header = self.one(chunks, b"cdta")
        if len(header) < 172:
            raise ValueError("truncated composition header")
        frame_duration, timescale = struct.unpack_from(">II", header, 4)
        if not frame_duration or not timescale:
            raise ValueError("composition clock has a zero rate")
        fps = Fraction(timescale, frame_duration)
        layers = [self.layer(body) for key, body in chunks if key == (b"LIST", b"Layr")]
        if len({layer["id"] for layer in layers}) != len(layers):
            raise ValueError("duplicate layer ID in composition")
        facts = {
            "canvas": list(struct.unpack_from(">HH", header, 140)),
            "pixel_aspect": list(struct.unpack_from(">II", header, 144)),
            "frame_rate": [fps.numerator, fps.denominator],
            "frame_duration_ticks": frame_duration,
            "timescale": timescale,
            "duration_ticks": u32(header, 44),
            "display_start": list(struct.unpack_from(">II", header, 164)),
            "layers": layers,
        }
        marker_owners = []
        for key, body in chunks:
            if key != (b"LIST", b"SecL"):
                continue
            owner = list(self.chunks(body))
            if self.name(owner) == "Markers":
                marker_owners.append(owner)
        if len(marker_owners) > 1:
            raise ValueError("ambiguous After Effects composition marker owner")
        if marker_owners:
            markers = self.saved_markers(self.layer_groups(marker_owners[0]))
            if markers:
                facts["markers"] = markers
        return facts

    def folder(self, body, parent=None, depth=0):
        if depth > MAX_DEPTH:
            raise ValueError("folder nesting exceeds inspection limit")
        for key, body in self.chunks(body):
            if key != (b"LIST", b"Item"):
                continue
            chunks = list(self.chunks(body))
            header = self.one(chunks, b"idta")
            if len(header) < 20:
                raise ValueError("truncated project item header")
            kind = struct.unpack_from(">H", header)[0]
            item_id = u32(header, 16)
            if item_id in self.items:
                raise ValueError(f"duplicate project item ID {item_id}")
            item = {
                "id": item_id, "parent_item_id": parent,
                "kind": ITEM_TYPES.get(kind, "unknown"), "type_code": kind,
                "saved_name": self.name(chunks),
            }
            self.items[item_id] = item
            if kind == 1:
                self.folder(self.one(chunks, b"LIST", b"Sfdr"), item_id, depth + 1)
            elif kind == 4:
                item.update(self.composition(chunks))
            elif kind == 7:
                item.update(self.footage(chunks))

    def resolve_anchor(self, layer):
        anchor = layer.get("saved_transform", {}).get("anchor")
        source = self.items.get(layer["source_item_id"])
        if anchor is None or anchor["state"] != "static" or source is None:
            return
        if source["kind"] == "composition":
            raster = source["canvas"]
        elif source.get("saved_footage_type") == "Soli":
            raster = source.get("saved_raster")
        else:
            return
        if not raster or not all(raster):
            return
        x, y, z = anchor["values"]
        pixels = [x * raster[0], y * raster[1], z]
        if all(math.isfinite(component) for component in pixels):
            layer["anchor_pixels"] = pixels

    def project_chunks(self):
        data = self.data
        if len(data) < 12 or data[:4] != b"RIFX" or data[8:12] != b"Egg!":
            raise ValueError("expected a binary After Effects RIFX/Egg! project")
        end = 8 + u32(data, 4)
        if not 12 <= end <= len(data):
            raise ValueError("declared container length is outside the file")
        return self.chunks(data[12:end]), end, len(data) - end

    def inspect(self):
        if len(self.data) > MAX_BYTES:
            raise ValueError("input exceeds inspection limit")
        chunks, container_bytes, trailer_bytes = self.project_chunks()
        chunks = list(chunks)
        self.read_point_definitions(chunks)
        self.folder(self.one(chunks, b"LIST", b"Fold"))
        unresolved = []
        for item in self.items.values():
            layers = item.get("layers", [])
            layer_ids = {layer["id"] for layer in layers}
            parent_cycles = cyclic_parent_ids(layers)
            for layer in layers:
                self.resolve_anchor(layer)
                for ordinal, effect in enumerate(layer.get("effects", [])):
                    for control in effect.get("layer_inputs", []):
                        if control["state"] != "saved":
                            continue
                        target = control["target_layer_id"]
                        if not target:
                            control["binding"] = "none"
                        elif target == layer["id"]:
                            control["binding"] = "self"
                        elif target in layer_ids:
                            control["binding"] = "other_layer"
                        else:
                            control["binding"] = "unresolved"
                            unresolved.append({
                                "composition_id": item["id"], "layer_id": layer["id"],
                                "reference": "effect_layer_input", "effect_index": ordinal,
                                "match_name": control["match_name"], "target_id": target,
                                "reason": "not_in_composition",
                            })
                source, parent = layer["source_item_id"], layer["parent_layer_id"]
                for kind, value, missing in (
                    ("source_item", source, source not in (0, 0xffff_ffff) and source not in self.items),
                    ("parent_layer", parent, parent != 0 and parent not in layer_ids),
                ):
                    if missing:
                        unresolved.append({"composition_id": item["id"], "layer_id": layer["id"],
                                           "reference": kind, "target_id": value})
                if layer["id"] in parent_cycles:
                    unresolved.append({
                        "composition_id": item["id"], "layer_id": layer["id"],
                        "reference": "parent_layer", "target_id": parent,
                        "reason": "self_reference" if parent == layer["id"] else "cycle",
                    })
                matte = layer.get("track_matte")
                if matte is not None:
                    target = matte["layer_id"]
                    if not matte["mode_code"]:
                        reason = "inactive_mode_with_layer_id"
                    elif not target:
                        reason = "missing_layer_id"
                    elif target == layer["id"]:
                        reason = "self_reference"
                    elif target not in layer_ids:
                        reason = "not_in_composition"
                    else:
                        continue
                    unresolved.append({"composition_id": item["id"], "layer_id": layer["id"],
                                       "reference": "track_matte", "target_id": target, "reason": reason})
        return {
            "schema": "anynle.aep-inspection/1",
            "container_bytes": container_bytes,
            "trailer_bytes": trailer_bytes,
            "input_sha256": hashlib.sha256(self.data).hexdigest(),
            "items": list(self.items.values()),
            "unresolved_references": unresolved,
        }


class BoundedTree(ElementTree.TreeBuilder):
    def __init__(self):
        super().__init__()
        self.depth = 0
        self.nodes = 0

    def start(self, tag, attrs):
        self.depth += 1
        self.nodes += 1
        if self.depth > MAX_DEPTH or self.nodes > MAX_CHUNKS:
            raise ValueError("AEPX XML exceeds depth or element limit")
        return super().start(tag, attrs)

    def end(self, tag):
        self.depth -= 1
        return super().end(tag)

    def doctype(self, name, public_id, system_id):
        raise ValueError("AEPX document type declarations are unsupported")


class XmlInspector(Inspector):
    def project_chunks(self):
        parser = ElementTree.XMLParser(target=BoundedTree(), encoding="utf-8")
        parser.feed(bytes(self.data))
        root = parser.close()
        if (
            root.tag != XML_NAMESPACE + "AfterEffectsProject"
            or root.get("majorVersion") != "1"
            or root.get("minorVersion") != "0"
        ):
            raise ValueError("expected After Effects AEPX version 1.0")
        records = (node for node in root if node.tag in (XML_NAMESPACE + "Fold", XML_NAMESPACE + "EfdG"))
        return self.chunks(records), None, None

    def chunks(self, body):
        for node in body:
            if not node.tag.startswith(XML_NAMESPACE):
                raise ValueError("unexpected namespace in inspected AEPX record")
            name = node.tag.removeprefix(XML_NAMESPACE)
            raw = node.get("bdata")
            if name == "string":
                text = node.text or ""
                if len(node) or raw is not None or len(text) > MAX_TEXT:
                    raise ValueError("invalid AEPX string")
                yield (b"Utf8", None), text.encode("utf-8")
            elif name == "fileReference":
                path = node.get("fullpath")
                if len(node) or raw is not None or path is None or len(path) > MAX_TEXT:
                    raise ValueError("invalid AEPX file reference")
                yield (b"alas", None), json.dumps({"fullpath": path}, ensure_ascii=False).encode("utf-8")
            elif name in ("idta", "cdta", "ldta", "tdmn", "tdsb", "tdb4", "cdat",
                          "lhd3", "ldat", "NmHd", "mkif", "shph", "otda", "tdpi", "tdps", "pard") or (
                name == "sspc" and raw is not None
            ):
                max_hex = MAX_KEYS * 128 * 2 if name == "ldat" else 8192
                if raw is None or len(node) or len(raw) > max_hex or any(
                    char not in "0123456789abcdefABCDEF" for char in raw
                ):
                    raise ValueError(f"invalid AEPX {name} data")
                yield (name.encode("ascii"), None), bytes.fromhex(raw)
            elif name in ("Fold", "Item", "Sfdr", "Layr", "SecL", "Pin", "Als2",
                          "tdgp", "tdbs", "list", "mrst", "mrky", "Nmrd", "tdsn", "om-s", "omks", "shap",
                          "otst", "otky", "sspc", "parT", "fnam", "EfdG", "EfDf"):
                if raw is not None:
                    raise ValueError(f"expected AEPX {name} list")
                yield (b"LIST", name.ljust(4).encode("ascii")), node


def inspect_project(data):
    inspector = Inspector if data.startswith(b"RIFX") else XmlInspector
    return inspector(data).inspect()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path)
    args = parser.parse_args()
    try:
        if not args.project.is_file():
            raise ValueError("expected a regular AEP file")
        with args.project.open("rb") as handle:
            report = inspect_project(handle.read(MAX_BYTES + 1))
        print(json.dumps(report, indent=2))
    except (OSError, ValueError, RecursionError, ElementTree.ParseError) as error:
        parser.exit(1, f"aep inspection: {error}\n")


if __name__ == "__main__":
    main()
