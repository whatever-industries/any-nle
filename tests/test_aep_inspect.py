import struct
import unittest

from scripts.aep_inspect import Inspector, inspect_project


def chunk(tag, data):
    return tag + struct.pack(">I", len(data)) + data + bytes(len(data) % 2)


def aep(*items):
    content = b"Egg!" + chunk(b"LIST", b"Fold" + b"".join(items))
    return b"RIFX" + struct.pack(">I", len(content)) + content


def item(kind, item_id, *fields):
    data = bytearray(20)
    struct.pack_into(">H", data, 0, kind)
    struct.pack_into(">I", data, 16, item_id)
    return chunk(b"LIST", b"Item" + chunk(b"idta", data) + b"".join(fields))


def mask_project(*, truncate=False, key_times=None):
    # The third point in each triplet belongs to the following vertex.
    # Use distinct controls so rotating the wrong handle would change the test.
    bounds = struct.pack(">4s4fI", b"\xb3\xde\x02\x09", 0.25, 0, 0.75, 0.5, 0x01000000)
    counts = struct.pack(">13I", 0x00D00BEE, 0, 6, 2, 8, 4, 1, 8, *([0] * 5))
    data = struct.pack(">12f", 0, 0, 0.25, 0, 0.5, 1, 1, 1, 1, 0.5, 0, 0.25)
    if truncate:
        data = data[:-8]
    header = bytearray(b"\xdb\x99\x00\x01" + bytes(120))
    comp = bytearray(204)
    struct.pack_into(">II", comp, 4, 1024, 25600)
    layer = bytes(164)

    def node(tag, value):
        return chunk(tag.encode(), value), f'<{tag} bdata="{value.hex()}"/>'

    def group(tag, *children):
        binary, xml = zip(*children)
        return chunk(b"LIST", tag.encode() + b"".join(binary)), f"<{tag}>{''.join(xml)}</{tag}>"

    def match(name):
        return node("tdmn", f"ADBE {name}".encode())

    points = group("list", node("lhd3", counts), node("ldat", data))
    shapes = [group("shap", node("shph", bounds), points)]
    property_fields = []
    if key_times is not None:
        header[68] = 1
        struct.pack_into(">I", header, 12, 25600)
        key_counts = struct.pack(">13I", 0x00D00BEE, 0, 2, 1, 64, 4, 1, 4, *([0] * 5))
        key_data = (struct.pack(">iBBH16x4d8x", key_times[0], 2, 3, 1, 0, 0.33, 0, 0.2)
                    + struct.pack(">iBBH16x4d8x", key_times[1], 3, 1, 1, 0, 0.5, 0, 0.33))
        property_fields.append(group("list", node("lhd3", key_counts), node("ldat", key_data)))
        closed = struct.pack(">4s4fI", b"\xb3\xde\x02\x01", 0.5, 0, 1, 0.5, 0x01000000)
        shapes.append(group("shap", node("shph", closed), points))
    shape = group("om-s", group("tdbs", node("tdb4", header), *property_fields), group("omks", *shapes))
    name = chunk(b"tdsn", chunk(b"Utf8", b"Same name")), "<tdsn><string>Same name</string></tdsn>"
    parade = group("tdgp", match("Mask Parade"), group(
        "tdgp", match("Mask Atom"), node("mkif", struct.pack(">B3xI", 1, 2)),
        group("tdgp", name, match("Mask Shape"), shape),
        match("Mask Atom"), node("mkif", struct.pack(">B3xI", 0, 0)), group("tdgp", name),
    ))
    layer_binary, layer_xml = group("Layr", node("ldta", layer), parade)
    binary = aep(item(4, 1, chunk(b"cdta", comp), layer_binary))
    _, xml_item = group("Item", node("idta", struct.pack(">H14xI", 4, 1)), node("cdta", comp),
                       (layer_binary, layer_xml))
    xml = (f'<AfterEffectsProject xmlns="http://www.adobe.com/products/aftereffects"'
           f' majorVersion="1" minorVersion="0"><Fold>{xml_item}</Fold></AfterEffectsProject>').encode()
    return binary, xml


class AepInspectionTests(unittest.TestCase):
    def test_effect_input_resolves_in_owning_composition(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        effect = b"ADBE Set Matte3"

        def group(tag, *fields):
            return chunk(b"LIST", tag + b"".join(fields))

        def layer(layer_id, *fields):
            return group(b"Layr", chunk(b"ldta", struct.pack(">I160x", layer_id)), *fields)

        def report(target):
            owner = chunk(b"tdmn", effect + b"-0000") + group(b"tdbs", chunk(b"tdpi", struct.pack(">I", 10)))
            header = b"\xdb\x99\x00\x01\x00\x01\x00\x00" + bytes(116)
            control = chunk(b"tdmn", effect + b"-0001") + group(
                b"tdbs", chunk(b"tdsb", struct.pack(">I", 1)), chunk(b"tdb4", header),
                chunk(b"cdat", bytes(40)), chunk(b"tdpi", target),
            )
            parade = group(b"tdgp", chunk(b"tdmn", effect), group(b"sspc", group(b"tdgp", owner, control)))
            fields = group(b"tdgp", chunk(b"tdmn", b"ADBE Effect Parade"), parade)
            return inspect_project(aep(
                item(4, 1, chunk(b"cdta", comp), layer(10, fields), layer(11)),
                item(4, 2, chunk(b"cdta", comp), layer(12)),
            ))

        for target, binding in [(0, "none"), (10, "self"), (11, "other_layer"), (12, "unresolved")]:
            result = report(struct.pack(">I", target))
            control = result["items"][0]["layers"][0]["effects"][0]["layer_inputs"][0]
            self.assertEqual((control["target_layer_id"], control["binding"]), (target, binding))
            self.assertEqual(len(result["unresolved_references"]), int(target == 12))
        malformed = report(b"\0")
        self.assertEqual(
            malformed["items"][0]["layers"][0]["effects"][0]["layer_inputs"][0]["state"], "unqualified"
        )

    def test_ramp_shared_definition_never_lends_another_instances_value(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        ramp = b"ADBE Ramp"

        def group(tag, *fields):
            return chunk(b"LIST", tag + b"".join(fields))

        def named(name, data):
            return chunk(b"tdmn", name) + data

        point = ramp + b"-0001"
        definition = named(point, chunk(b"pard", struct.pack(">12xI40x2i84x", 6, 32768, 0)))
        header = b"\xdb\x99\x00\x02\x00\x0f\x00\x03" + bytes(116)
        explicit = named(point, group(b"tdbs", chunk(b"tdsb", struct.pack(">I", 1)),
                                      chunk(b"tdb4", header),
                                      chunk(b"cdat", struct.pack(">2d32x", 1.25, -0.5))))

        def instance(layer_id, definition=b"", point=b""):
            owner = named(ramp + b"-0000", group(b"tdbs", chunk(b"tdpi", struct.pack(">I", layer_id))))
            return named(ramp, group(b"sspc", group(b"parT", definition), group(b"tdgp", owner, point)))

        def layer(layer_id, instance):
            parade = named(b"ADBE Effect Parade", group(b"tdgp", instance))
            return group(b"Layr", chunk(b"ldta", struct.pack(">I160x", layer_id)), group(b"tdgp", parade))

        project = aep(item(4, 1, chunk(b"cdta", comp),
                           layer(17, instance(17, definition, explicit)), layer(18, instance(18))))
        shared = group(b"EfDf", instance(17, definition, explicit))

        def controls(*definitions):
            content = project[8:] + group(b"EfdG", *definitions)
            data = b"RIFX" + struct.pack(">I", len(content)) + content
            layers = inspect_project(data)["items"][0]["layers"]
            return [row["effects"][0]["controls"][0] for row in layers]

        first, second = controls(shared)
        self.assertEqual((first["state"], first["values"]), ("saved", [1.25, -0.5]))
        self.assertEqual((second["state"], second["values"]), ("shared_default", [0.5, 0]))
        for definitions in ((), (shared, shared)):
            first, second = controls(*definitions)
            self.assertEqual(first["values"], [1.25, -0.5])
            self.assertEqual(second["state"], "unqualified")
            self.assertNotIn("values", second)

    def test_effect_order_uses_instance_records_and_distinguishes_explicit_zero(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        layer = struct.pack(">I160x", 27)
        name = b"ADBE Gaussian Blur 2"

        def node(tag, value):
            return chunk(tag.encode(), value), f'<{tag} bdata="{value.hex()}"/>'

        def group(tag, *children):
            return (chunk(b"LIST", tag.encode() + b"".join(b for b, _ in children)),
                    f"<{tag}>{''.join(x for _, x in children)}</{tag}>")

        def label(tag, value):
            return chunk(tag.encode(), chunk(b"Utf8", value.encode())), f"<{tag}><string>{value}</string></{tag}>"

        def blur(*, owner=27, definitions=True, explicit=False, duplicate=False):
            value = group("tdbs", node("tdsb", struct.pack(">I", 1)),
                          node("tdb4", b"\xdb\x99\x00\x01\x00\x01" + bytes(118)),
                          node("cdat", struct.pack(">d32x", 0)))
            fields = (node("tdmn", name + b"-0003"), value)
            values = group("tdgp", label("tdsn", "Same"), node("tdmn", name + b"-0000"),
                           group("tdbs", node("tdpi", struct.pack(">I", owner))),
                           *(fields * (2 if duplicate else int(explicit))))
            definition = struct.pack(">12xI40xI88x", 4, 1)
            return group("sspc", label("fnam", "Gaussian Blur"),
                         group("parT", *((node("tdmn", name + b"-0003"), node("pard", definition))
                                         if definitions else ())),
                         values)

        def project(second):
            effects = group("tdgp", node("tdmn", name), blur(), node("tdmn", name), second)
            layers = group("Layr", node("ldta", layer),
                           group("tdgp", node("tdmn", b"ADBE Effect Parade"), effects))
            _, xml_item = group("Item", node("idta", struct.pack(">H14xI", 4, 1)),
                                node("cdta", comp), layers)
            xml = (f'<AfterEffectsProject xmlns="http://www.adobe.com/products/aftereffects"'
                   f' majorVersion="1" minorVersion="0"><Fold>{xml_item}</Fold></AfterEffectsProject>')
            return aep(item(4, 1, chunk(b"cdta", comp), layers[0])), xml.encode()

        for data in project(blur(definitions=False, explicit=True)):
            effects = inspect_project(data)["items"][0]["layers"][0]["effects"]
            self.assertEqual([e["user_name"] for e in effects], ["Same", "Same"])
            repeat_edge = [effect["controls"][1] for effect in effects]
            self.assertEqual([(c["state"], c["value"]) for c in repeat_edge],
                             [("local_default", True), ("saved", False)])
        effects = inspect_project(project(blur(definitions=False))[0])["items"][0]["layers"][0]["effects"]
        self.assertEqual(effects[1]["controls"][1]["state"], "unqualified")
        self.assertNotIn("value", effects[1]["controls"][1])
        effects = inspect_project(project(blur(owner=3))[0])["items"][0]["layers"][0]["effects"]
        self.assertEqual(effects[1]["controls_state"], "unqualified")
        for data in project(blur(explicit=True, duplicate=True)):
            with self.assertRaisesRegex(ValueError, "ambiguous After Effects effect parameter"):
                inspect_project(data)

    def test_orientation_and_axis_rotation_belong_to_the_3d_layer(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        header = bytearray(b"\xdb\x99\x00\x01\x00\x01" + bytes(118))
        layer = bytearray(164)
        struct.pack_into(">I", layer, 40, 2)
        rotation = (
            chunk(b"tdmn", b"ADBE Rotate X")
            + chunk(b"LIST", b"tdbs" + chunk(b"tdb4", header) + chunk(b"cdat", struct.pack(">d", 30)))
        )

        def saved_layer(orientation, is_3d):
            layer[38] = 4 if is_3d else 0
            transform = chunk(
                b"LIST",
                b"tdgp" + chunk(b"tdmn", b"ADBE Transform Group") + chunk(
                    b"LIST", b"tdgp" + rotation + chunk(b"tdmn", b"ADBE Orientation") + chunk(
                        b"LIST", b"otst" + chunk(b"LIST", b"tdbs" + chunk(b"tdb4", header))
                        + chunk(b"LIST", b"otky" + chunk(b"otda", orientation))
                    )
                ),
            )
            project = aep(item(4, 1, chunk(b"cdta", comp),
                               chunk(b"LIST", b"Layr" + chunk(b"ldta", layer) + transform)), item(7, 2))
            return inspect_project(project)["items"][0]["layers"][0]

        values = struct.pack(">3d", 10, 20, 30)
        static = saved_layer(values, True)
        self.assertTrue(static["three_dimensional"])
        self.assertEqual(static["saved_transform"]["rotation_x"]["values"], [30])
        self.assertEqual(static["saved_transform"]["orientation"]["values"], [10, 20, 30])
        self.assertNotIn("saved_transform", saved_layer(values, False))
        with self.assertRaisesRegex(ValueError, "Orientation value extent"):
            saved_layer(values[:-8], True)

    def test_parent_positions_remain_local_and_parent_cycles_are_reported(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        prop = bytearray(124)
        prop[:6] = b"\xdb\x99\x00\x03\x00\x01"
        position = chunk(b"tdmn", b"ADBE Position") + chunk(
            b"LIST", b"tdbs" + chunk(b"tdb4", prop) + chunk(b"cdat", struct.pack(">3d", 12, 34, 0)),
        )
        transform = chunk(b"LIST", b"tdgp" + chunk(b"tdmn", b"ADBE Transform Group")
                          + chunk(b"LIST", b"tdgp" + position))

        def layer(layer_id, parent_id):
            header = bytearray(164)
            struct.pack_into(">I", header, 0, layer_id)
            struct.pack_into(">I", header, 40, 99)
            struct.pack_into(">I", header, 132, parent_id)
            return chunk(b"LIST", b"Layr" + chunk(b"ldta", header)
                         + chunk(b"Utf8", b"Same name") + transform)

        report = inspect_project(aep(
            item(4, 1, chunk(b"cdta", comp), layer(4, 0), layer(5, 4), layer(6, 7),
                 layer(7, 6), layer(8, 7), layer(9, 9), layer(10, 11)),
            item(4, 2, chunk(b"cdta", comp), layer(11, 0)),
            item(7, 99),
        ))
        root, child, *_ = report["items"][0]["layers"]
        self.assertEqual(root["saved_transform"]["position"]["unit"], "composition_pixels")
        self.assertEqual(child["saved_transform"]["position"],
                         {"state": "static", "unit": "parent_layer_pixels", "values": [12, 34, 0]})
        self.assertEqual(
            [(row["layer_id"], row["target_id"], row.get("reason")) for row in report["unresolved_references"]],
            [(6, 7, "cycle"), (7, 6, "cycle"), (9, 9, "self_reference"), (10, 11, None)],
        )

    def test_time_remap_keeps_layer_ticks_and_source_seconds_separate(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        layer = bytearray(164)
        struct.pack_into(">i", layer, 8, -1)
        struct.pack_into(">I", layer, 108, 2)
        header = bytearray(124)
        header[:4] = b"\xdb\x99\x00\x01"
        header[68] = 1
        struct.pack_into(">I", header, 12, 25600)
        counts = struct.pack(">13I", 0x00D00BEE, 0, 2, 1, 48, 4, 1, 4, *([0] * 5))
        keys = (struct.pack(">iBBH5d", -12800, 1, 3, 0, 1.5, 0, 0, 0, 0)
                + struct.pack(">iBBH5d", 25600, 3, 1, 0, 0.1, 0, 0, 0, 0))

        def project(key_bytes, *, disabled=False, expression=False):
            prop = bytearray(header)
            prop[120] = int(expression)
            if disabled:
                prop[68] = 0
                prop[5] = 1
                saved_value = chunk(b"cdat", bytes(40))
            else:
                saved_value = chunk(b"LIST", b"list" + chunk(b"lhd3", counts) + chunk(b"ldat", key_bytes))
            fields = chunk(b"tdsb", struct.pack(">I", 3 if disabled else 1)) + chunk(b"tdb4", prop)
            remap = chunk(b"tdmn", b"ADBE Time Remapping") + chunk(b"LIST", b"tdbs" + fields + saved_value)
            return aep(item(4, 1, chunk(b"cdta", comp), chunk(
                b"LIST", b"Layr" + chunk(b"ldta", layer) + chunk(b"LIST", b"tdgp" + remap),
            )))

        def saved_layer(data):
            return inspect_project(data)["items"][0]["layers"][0]

        result = saved_layer(project(keys))
        self.assertEqual(result["stretch"], {"numerator": -1, "denominator": 2})
        remap = result["time_remap"]
        self.assertEqual((remap["layer_timescale"], remap["unit"]), (25600, "source_seconds"))
        self.assertEqual(
            [(key["ticks"], key["values"], key["interpolation_in"], key["interpolation_out"]) for key in remap["keys"]],
            [(-12800, [1.5], "linear", "hold"), (25600, [0.1], "hold", "linear")],
        )
        self.assertNotIn("time_remap", saved_layer(project(b"", disabled=True)))
        self.assertEqual(saved_layer(project(keys, expression=True))["time_remap"],
                         {"state": "expression_or_computed"})
        with self.assertRaisesRegex(ValueError, "invalid After Effects key list extent"):
            saved_layer(project(keys[:-1]))

    def test_mask_order_path_handles_and_missing_path_in_binary_and_xml(self):
        binary, xml = mask_project()
        masks = inspect_project(binary)["items"][0]["layers"][0]["masks"]
        self.assertEqual(masks, inspect_project(xml)["items"][0]["layers"][0]["masks"])
        self.assertEqual([(m["saved_name"], m["mode"], m["inverted"]) for m in masks],
                         [("Same name", "subtract", True), ("Same name", "none", False)])
        self.assertEqual(masks[0]["path"], {
            "state": "static", "unit": "source_fraction", "closed": False,
            "bounds_ltrb": [0.25, 0, 0.75, 0.5],
            "vertices": [[0.25, 0], [0.75, 0.5]],
            "out_handles": [[0.375, 0], [0.75, 0.25]],
            "in_handles": [[0.25, 0.125], [0.5, 0.5]],
        })
        self.assertEqual(masks[1]["path"], {"state": "not_saved"})

    def test_mask_rejects_inconsistent_point_extent(self):
        for project in mask_project(truncate=True):
            with self.assertRaisesRegex(ValueError, "invalid After Effects mask point list extent"):
                inspect_project(project)

    def test_keyed_mask_pairs_geometry_with_saved_times_and_ease(self):
        binary, xml = mask_project(key_times=(-12800, 51200))

        def path(project):
            return inspect_project(project)["items"][0]["layers"][0]["masks"][0]["path"]

        report = path(binary)
        self.assertEqual(report, path(xml))
        self.assertEqual((report["state"], report["layer_timescale"], report["key_count"]), ("keyed", 25600, 2))
        first, second = report["keys"]
        self.assertEqual((first["ticks"], first["interpolation_out"], first["saved_ease"]["out_influence"]),
                         (-12800, "hold", 0.2))
        self.assertEqual((second["ticks"], second["interpolation_in"], second["saved_ease"]["in_influence"]),
                         (51200, "hold", 0.5))
        self.assertFalse(first["geometry"]["closed"])
        self.assertTrue(second["geometry"]["closed"])
        self.assertEqual(second["geometry"]["vertices"][0], [0.5, 0])
        with self.assertRaisesRegex(ValueError, "not in increasing time order"):
            path(mask_project(key_times=(0, 0))[0])

    def test_marker_owners_keep_independent_time_and_duration_clocks(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        layer = bytearray(164)
        struct.pack_into(">I", layer, 0, 7)
        header = bytearray(124)
        header[:4] = b"\xdb\x99\x00\x01"
        header[68] = 1
        struct.pack_into(">I", header, 12, 25600)
        list_header = struct.pack(">13I", 0x00D00BEE, 0, 1, 1, 16, 4, 1, 4, *([0] * 5))

        def marker_group(ticks, comment, include_value=True):
            keys = chunk(b"LIST", b"list" + chunk(b"lhd3", list_header)
                         + chunk(b"ldat", struct.pack(">i12x", ticks)))
            prop = chunk(b"LIST", b"tdbs" + chunk(b"tdb4", header) + keys)
            value = chunk(b"NmHd", struct.pack(">4IB3x", 5, 1, 180, 600, 3))
            value += b"".join(chunk(b"Utf8", s) for s in (comment, b"", b"", b"", b"cue", b"k", b"v"))
            payload = chunk(b"LIST", b"mrky" + (chunk(b"LIST", b"Nmrd" + value) if include_value else b""))
            return chunk(b"LIST", b"tdgp" + chunk(b"tdmn", b"ADBE Marker")
                         + chunk(b"LIST", b"mrst" + prop + payload))

        def project(include_value):
            return aep(item(
                4, 1, chunk(b"cdta", comp),
                chunk(b"LIST", b"SecL" + chunk(b"Utf8", b"Markers") + marker_group(64000, b"Composition")),
                chunk(b"LIST", b"Layr" + chunk(b"ldta", layer)
                      + marker_group(13312, "Layer café".encode(), include_value)),
            ))

        sequence, = inspect_project(project(True))["items"]
        comp_marker, = sequence["markers"]
        layer_marker, = sequence["layers"][0]["markers"]
        self.assertEqual(comp_marker["time"], {"numerator": 64000, "denominator": 25600})
        self.assertEqual(layer_marker["time"], {"numerator": 13312, "denominator": 25600})
        self.assertEqual(layer_marker["duration"], {"numerator": 180, "denominator": 600})
        self.assertEqual(layer_marker["comment"], "Layer café")
        self.assertEqual(layer_marker["cue_parameters"], [["k", "v"]])
        with self.assertRaisesRegex(ValueError, "marker times and records disagree"):
            inspect_project(project(False))

    def test_track_matte_binds_below_by_composition_local_id(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1024, 25600)
        struct.pack_into(">HH", comp, 140, 1920, 1080)

        def layer_header(layer_id, enabled, mode=0, matte_id=0):
            header = bytearray(164)
            struct.pack_into(">I", header, 0, layer_id)
            header[39] = 7 if enabled else 6
            header[107] = mode
            struct.pack_into(">I", header, 160, matte_id)
            return header

        headers = [layer_header(3, True, 4, 5), layer_header(5, False), layer_header(7, True, 1, 9)]
        headers[0][99] = 5
        headers[1][99] = 3

        def layer(header):
            return chunk(b"LIST", b"Layr" + chunk(b"ldta", header) + chunk(b"Utf8", b"Same name"))

        binary = aep(item(4, 1, chunk(b"cdta", comp), *(layer(h) for h in headers)),
                     item(4, 2, chunk(b"cdta", comp), layer(layer_header(9, True))))
        report = inspect_project(binary)
        xml_items = []
        for item_id, group in ((1, headers), (2, [layer_header(9, True)])):
            idta = struct.pack(">H14xI", 4, item_id).hex()
            layers = "".join(f'<Layr><ldta bdata="{h.hex()}"/><string>Same name</string></Layr>' for h in group)
            xml_items.append(f'<Item><idta bdata="{idta}"/><cdta bdata="{comp.hex()}"/>{layers}</Item>')
        xml = (f'<AfterEffectsProject xmlns="http://www.adobe.com/products/aftereffects"'
               f' majorVersion="1" minorVersion="0"><Fold>{"".join(xml_items)}</Fold></AfterEffectsProject>')
        xml_report = inspect_project(xml.encode())
        self.assertEqual(xml_report["items"], report["items"])
        self.assertEqual(xml_report["unresolved_references"], report["unresolved_references"])
        target, matte, dangling = report["items"][0]["layers"]
        self.assertEqual(target["blend_mode"], {"mode_code": 5, "mode": "multiply"})
        self.assertEqual(matte["blend_mode"], {"mode_code": 3, "mode": None})
        self.assertEqual(target["track_matte"], {"mode_code": 4, "mode": "luma_inverted", "layer_id": 5})
        self.assertFalse(matte["video_enabled"])
        self.assertNotIn("track_matte", matte)
        self.assertEqual(dangling["track_matte"]["layer_id"], 9)
        self.assertEqual(report["unresolved_references"], [
            {"composition_id": 1, "layer_id": 7, "reference": "track_matte",
             "target_id": 9, "reason": "not_in_composition"}
        ])

    def test_opacity_keys_keep_signed_layer_ticks_and_both_interpolation_sides(self):
        header = bytearray(124)
        header[:4] = b"\xdb\x99\x00\x01"
        header[68] = 1
        struct.pack_into(">I", header, 12, 25600)
        list_header = struct.pack(">13I", 0x00D00BEE, 0, 2, 1, 48, 4, 1, 4, *([0] * 5))
        records = (
            struct.pack(">iBBH5d", -12800, 1, 3, 0, 0.25, 0, 0, 0, 0)
            + struct.pack(">iBBH5d", 25600, 3, 1, 0, 1, 0, 0, 0, 0)
        )
        cached = chunk(b"cdat", struct.pack(">d", 0.75))
        prefix = chunk(b"tdb4", header) + cached

        def property_body(key_data):
            return prefix + chunk(b"LIST", b"list" + chunk(b"lhd3", list_header) + chunk(b"ldat", key_data))

        result = Inspector(b"").transform_value(property_body(records), 1, "fraction", name="opacity")
        self.assertEqual(result["layer_timescale"], 25600)
        self.assertEqual(
            [(k["ticks"], k["interpolation_in"], k["interpolation_out"], k["values"]) for k in result["keys"]],
            [(-12800, "linear", "hold", [0.25]), (25600, "hold", "linear", [1.0])],
        )
        with self.assertRaisesRegex(ValueError, "invalid After Effects key list extent"):
            Inspector(b"").transform_value(property_body(records[:-1]), 1, "fraction", name="opacity")

    def test_vector_keys_keep_ease_with_the_correct_component(self):
        header = bytearray(124)
        header[:4] = b"\xdb\x99\x00\x02"
        header[68] = 1
        struct.pack_into(">I", header, 12, 25600)
        list_header = struct.pack(">13I", 0x00D00BEE, 0, 1, 2, 88, 4, 1, 4, *([0] * 5))
        values = [40, 20, 1, 2, 0.3, 0.4, 3, 4, 0.6, 0.7]

        def property_body(values):
            records = struct.pack(">iBBH10d", 25600, 2, 1, 0, *values)
            return chunk(b"tdb4", header) + chunk(
                b"LIST", b"list" + chunk(b"lhd3", list_header) + chunk(b"ldat", records)
            )

        result = Inspector(b"").transform_value(property_body(values), 2, "source_pixels", name="feather")
        key, = result["keys"]
        self.assertEqual(key["values"], [40, 20])
        self.assertEqual(key["saved_ease_by_component"], [
            {"in_speed": 1, "in_influence": 0.3, "out_speed": 3, "out_influence": 0.6},
            {"in_speed": 2, "in_influence": 0.4, "out_speed": 4, "out_influence": 0.7},
        ])
        values[-1] = float("nan")
        with self.assertRaisesRegex(ValueError, "non-finite After Effects key ease"):
            Inspector(b"").transform_value(property_body(values), 2, "source_pixels", name="feather")

    def test_expression_does_not_expose_cached_value_as_static(self):
        header = bytearray(124)
        header[:6] = b"\xdb\x99\x00\x01\x00\x01"
        value = chunk(b"cdat", struct.pack(">d", 0.25))
        inspector = Inspector(b"")
        plain = inspector.transform_value(chunk(b"tdb4", header) + value, 1, "fraction")
        self.assertEqual(plain, {"state": "static", "unit": "fraction", "values": [0.25]})
        header[120] = 1
        expression = inspector.transform_value(chunk(b"tdb4", header) + value, 1, "fraction")
        self.assertEqual(expression, {"state": "expression_or_computed"})

    def test_unresolved_source_keeps_layer_and_exact_record(self):
        comp = bytearray(204)
        struct.pack_into(">II", comp, 4, 1000, 23976)
        struct.pack_into(">HH", comp, 140, 1920, 1080)
        layer = bytearray(164)
        struct.pack_into(">I", layer, 0, 7)
        struct.pack_into(">iI", layer, 20, 48, 48000)
        struct.pack_into(">I", layer, 40, 45)
        body = aep(item(4, 1, chunk(b"cdta", comp), chunk(b"LIST", b"Layr" + chunk(b"ldta", layer))))
        result = Inspector(body + b"XMP tail").inspect()
        sequence, = result["items"]
        self.assertEqual(sequence["frame_rate"], [2997, 125])
        self.assertEqual(sequence["layers"][0]["relative_in"], {"numerator": 48, "denominator": 48000})
        self.assertEqual(result["trailer_bytes"], 8)
        self.assertEqual(result["unresolved_references"], [
            {"composition_id": 1, "layer_id": 7, "reference": "source_item", "target_id": 45}
        ])

    def test_rejects_duplicate_item_ids(self):
        data = aep(item(7, 1), item(7, 1))
        with self.assertRaisesRegex(ValueError, "duplicate project item ID"):
            Inspector(data).inspect()

    def test_rejects_chunks_that_overrun_their_parent(self):
        # The extra bytes lie inside RIFX and outside the Fold list.
        broken = aep(b"LIST\x00\x00\x00\x40Item")
        extra = chunk(b"head", bytes(64))
        expanded = broken[:4] + struct.pack(">I", len(broken) + len(extra) - 8) + broken[8:] + extra
        with self.assertRaisesRegex(ValueError, "crosses its parent boundary"):
            Inspector(expanded).inspect()

    def test_xml_footage_path_preserves_unicode_and_xml_escaping(self):
        locator = "C:\\Media\\café & tone.wav"
        binary = aep(item(
            7, 22, chunk(b"Utf8", b"tone"),
            chunk(b"LIST", b"Pin " + chunk(b"LIST", b"Als2" + chunk(
                b"alas", b'{"fullpath":"C:\\\\Media\\\\caf\\u00e9 & tone.wav"}',
            ))),
        ))
        idta = struct.pack(">H14xI", 7, 22).hex()
        xml = (f'<AfterEffectsProject xmlns="http://www.adobe.com/products/aftereffects"'
               f' majorVersion="1" minorVersion="0"><Fold><Item><idta bdata="{idta}"/>'
               f'<string>tone</string><Pin><Als2><fileReference fullpath="{locator.replace("&", "&amp;")}"/>'
               '</Als2></Pin></Item></Fold></AfterEffectsProject>').encode()
        report = inspect_project(xml)
        self.assertEqual(report["items"], inspect_project(binary)["items"])
        self.assertEqual(report["items"][0]["saved_locator"], locator)

    def test_movie_conform_clock_does_not_replace_saved_native_clock(self):
        profile = bytearray(222)
        profile[22:26] = b"XCEX"
        struct.pack_into(">IIII", profile, 30, 64, 48, 25, 25)
        struct.pack_into(">I", profile, 56, 25)

        def timing(conform, effective):
            struct.pack_into(">II", profile, 146, conform, effective)
            footage = item(7, 1, chunk(b"LIST", b"Pin " + chunk(b"sspc", profile)))
            return inspect_project(aep(footage))["items"][0]["saved_movie_timing"]

        self.assertEqual(timing(30, 30), {
            "state": "saved", "frame_count": 25,
            "native_rate": {"numerator": 25, "denominator": 1},
            "conform_rate": {"numerator": 30, "denominator": 1},
            "effective_rate": {"numerator": 30, "denominator": 1},
        })
        self.assertIsNone(timing(0, 25)["conform_rate"])
        self.assertEqual(timing(30, 25)["state"], "unqualified")

    def test_xml_rejects_document_type_and_bad_field_bytes(self):
        root = b'<AfterEffectsProject xmlns="http://www.adobe.com/products/aftereffects" majorVersion="1" minorVersion="0">'
        with self.assertRaisesRegex(ValueError, "document type"):
            inspect_project(b'<!DOCTYPE AfterEffectsProject [<!ENTITY custom "secret">]>' + root
                            + b'<Fold>&custom;</Fold></AfterEffectsProject>')
        with self.assertRaisesRegex(ValueError, "invalid AEPX idta"):
            inspect_project(root + b'<Fold><Item><idta bdata="000x"/></Item></Fold></AfterEffectsProject>')
