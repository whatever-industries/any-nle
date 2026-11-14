"""Clip label and marker tests. Structural only: they do not establish Premiere compatibility."""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from avid2prproj import colors  # noqa: E402
from avid2prproj.model import ModelError, parse_model  # noqa: E402
from avid2prproj.writer import write_project  # noqa: E402
import py_premiere  # noqa: E402

EXAMPLE = json.loads((ROOT / "examples/opatom_clip.model.json").read_text())
FRAME = 10584000000  # ticks per frame at 24 fps


def marker(comment, color, frame, **kw):
    m = {"name": "", "comment": comment, "user": "ED", "color": color, "start_ticks": frame * FRAME}
    m.update(kw)
    return m


class ColorTests(unittest.TestCase):
    def test_marker_defaults_are_the_user_decisions(self):
        # User 2026-10-15: Black and Grey -> White, Violet -> Magenta, Purple -> Blue.
        expect = {"Black": "White", "Grey": "White", "Violet": "Magenta", "Purple": "Blue", "Pink": "Magenta",
                  "Forest": "Green", "Denim": "Blue", "Gold": "Yellow", "Red": "Red", "Orange": "Orange"}
        for avid, premiere in expect.items():
            self.assertEqual(colors.MARKER_NAMES[colors.marker_color(avid)], premiere, avid)
        self.assertIsNone(colors.marker_color("Chartreuse"))
        self.assertEqual(colors.marker_color("Black", {"Black": 6}), 6)

    def test_nearest_label_for_avid_swatches(self):
        cases = {(56832, 25600, 29696): "Rose", (36608, 0, 45824): "Purple", (14592, 11776, 37888): "Blue",
                 (62464, 33024, 12544): "Mango", (16896, 32768, 13824): "Caribbean"}
        for rgb16, name in cases.items():
            self.assertEqual(colors.LABEL_NAMES[colors.nearest_label(rgb16)], name, colors.rgb16_hex(rgb16))
        self.assertEqual(colors.nearest_label((56832, 25600, 29696), {"#DD6474": 15}), 15)


def marked_model(**options):
    data = copy.deepcopy(EXAMPLE)
    data["options"].update(options)
    clip = data["clips"][0]
    clip["avid"] = {"mob_id": "urn:x", "color_rgb16": [56832, 25600, 29696]}
    clip["markers"] = [marker("clip V", "Red", 20, track="V1"), marker("clip V shared frame", "Yellow", 30, track="V1"),
                       marker("clip A1", "Green", 25, track="A1")]
    seq = data["sequences"][0]
    # The event plays source frames 24..143 from record 0.
    seq["markers"] = [
        marker("seq", "Blue", 2),
        marker("edit own", "Denim", 6, owner="edit", track="V1", event=0, source_ticks=30 * FRAME),
        marker("inherited", "Red", 0, owner="edit", track="V1", event=0, source_ticks=24 * FRAME, inherited=True),
        marker("filler V2", "Cyan", 200, owner="filler", track="V2"),
        marker("filler A3", "Magenta", 210, owner="filler", track="A3"),
    ]
    return data


class MarkerWriterTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "local").mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / "local")
        self.addCleanup(self.tmp.cleanup)

    def write(self, data):
        out = Path(self.tmp.name) / "out.prproj"
        report = write_project(parse_model(data), out)
        return py_premiere.parse(str(out)).project, report

    def items(self, project):
        out = {}

        def walk(item):
            for c in item.children:
                out[c.name] = c
                walk(c)
        walk(project.root_item)
        return out

    def test_labels_clip_markers_edit_copy_and_filler(self):
        project, report = self.write(marked_model())
        items = self.items(project)
        video = items["Example clip_V"]
        self.assertEqual(video.color_label, colors.LABEL_NAMES.index("Rose"))
        self.assertEqual([(m.comments, m.color_index) for m in video.markers], [("clip V", 1), ("clip V shared frame", 4)])
        self.assertEqual([m.comments for m in items["Example clip_A01"].markers], ["clip A1"])
        copy_name = next(n for n in items if "[edit" in n and n.endswith("_V"))
        bin_ = project.root_item.children[0]
        parts = next(c for c in bin_.children if c.name == "Processed Clips")
        self.assertIn(copy_name, [c.name for c in parts.children])
        # Edit wins by default: the clip marker on the same frame is dropped.
        self.assertEqual(sorted(m.comments for m in items[copy_name].markers), ["clip V", "edit own"])
        seq = project.sequences[0]
        self.assertEqual(sorted(m.comments for m in seq.markers), ["filler A3\nAvid: on empty A3", "seq"])
        layer = next(i for n, i in items.items() if n.endswith("V2 markers"))
        self.assertTrue(layer.is_adjustment_layer)
        self.assertEqual([m.start.ticks for m in layer.markers], [200 * FRAME])
        v2 = list(seq.video_tracks[1].clips)
        self.assertEqual([(c.start.ticks, c.end.ticks, c.in_point.ticks) for c in v2], [(200 * FRAME, 201 * FRAME, 200 * FRAME)])
        v1 = list(seq.video_tracks[0].clips)
        self.assertEqual(v1[0].project_item.name, copy_name)
        # Premiere needs each track's clips in time order (a V1 clip listed after later ones
        # played but was not drawn, User 2026-10-15).
        for track in list(seq.video_tracks) + list(seq.audio_tracks):
            starts = [c.start.ticks for c in track.clips]
            self.assertEqual(starts, sorted(starts))

    def test_source_wins_and_losers_go_to_the_sequence(self):
        project, _ = self.write(marked_model(marker_conflict_winner="source", marker_conflict_loser_to_sequence=True,
                                             marker_avid_info=True))
        items = self.items(project)
        copy_name = next(n for n in items if "[edit" in n and n.endswith("_V"))
        self.assertEqual(sorted(m.comments.split("\n")[0] for m in items[copy_name].markers), ["clip V", "clip V shared frame"])
        comments = [m.comments for m in project.sequences[0].markers]
        self.assertIn("edit own\nAvid: ED · Denim · edit marker under a clip marker", comments)

    def test_edit_marker_needs_its_placement(self):
        data = marked_model()
        del data["sequences"][0]["markers"][1]["event"]
        with self.assertRaises(ModelError):
            parse_model(data)

    def test_overlapping_filler_ranges_and_picture_boundary(self):
        data = copy.deepcopy(EXAMPLE)
        seq = data["sequences"][0]
        seq["events"][0].update(record_start_ticks=20 * FRAME, duration_ticks=8 * FRAME)
        seq["markers"] = [
            marker(name, "Red", start, track="V1", owner="filler", duration_ticks=duration * FRAME)
            for name, start, duration in [
                ("first", 2, 4), ("overlap", 4, 4), ("chain", 7, 3),
                ("touching", 10, 0), ("before picture", 18, 2), ("crosses picture", 19, 3)
            ]
        ]
        project, report = self.write(data)
        out = project.sequences[0]
        self.assertEqual([(c.start.ticks // FRAME, c.end.ticks // FRAME) for c in out.video_tracks[0].clips],
                         [(2, 10), (10, 11), (18, 20), (20, 28)])
        layer = next(c for c in self.items(project).values() if c.is_adjustment_layer)
        self.assertEqual([m.comments for m in layer.markers], ["first", "overlap", "chain", "touching", "before picture"])
        self.assertEqual([(m.start.ticks // FRAME, m.end.ticks // FRAME, m.color_index) for m in out.markers],
                         [(19, 22, 1)])
        self.assertIn("crosses picture", out.markers[0].comments)
        self.assertTrue(any("kept on sequence" in note for note in report.notes))


if __name__ == "__main__":
    unittest.main()
