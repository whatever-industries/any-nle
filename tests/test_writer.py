"""Writer tests. Structural only: they do not establish Premiere compatibility."""
import copy
import gzip
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from avid2prproj.model import parse_model  # noqa: E402
from avid2prproj.opatom import UnverifiedShape  # noqa: E402
from avid2prproj.writer import write_project  # noqa: E402
import py_premiere  # noqa: E402  (vendored, path set by avid2prproj)

EXAMPLE = json.loads((ROOT / "examples/opatom_clip.model.json").read_text())
sys.path.insert(0, str(ROOT / "prproj" / "tools"))
from corpus_base import corpus_base  # noqa: E402  (Any NLE local/corpus-base, or the old sibling layout)
PROBE = corpus_base() / "premiere-format-research/scripts/prproj_probe.py"

#: Media field sets Premiere 26.5.2 wrote for Avid OP-Atom files (PRPROJ-0009), minus the
#: machine-local caches it regenerates (none live on Media itself).
# Premiere 26.5.2's own OP-Atom audio import also writes Start (the MXF start timecode) and
# AlternateStart 0 (User Auto-Save 2026-10-14, PRPROJ-0017).
VIDEO_MEDIA_FIELDS = {"VideoStream", "ModificationState", "RelativePath", "Start", "ImplementationID", "FileKey",
                      "AlternateTapeName", "ContentAndMetadataState", "TapeName", "ActualMediaFilePath", "FilePath",
                      "AlternateStart", "Title"}
AUDIO_MEDIA_FIELDS = (VIDEO_MEDIA_FIELDS - {"VideoStream"}) | {"AudioStream", "ConformedAudioRate"}
LOGGING_FIELDS = {"ClipID", "CaptureMode", "ClipName", "MediaInPoint", "MediaOutPoint", "MediaFrameRate", "TapeName", "TimecodeFormat"}


def write(data, tmp):
    out = Path(tmp) / "out.prproj"
    report = write_project(parse_model(data), out)
    return out, report


class WriterTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "local").mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / "local")
        self.addCleanup(self.tmp.cleanup)

    def test_separate_items_linked_placement_and_marker(self):
        out, report = write(copy.deepcopy(EXAMPLE), self.tmp.name)
        self.assertEqual(report.unverified, [])
        self.assertTrue(any("avid2prproj-1080p24" in n for n in report.notes))
        project = py_premiere.parse(str(out)).project
        bin_ = project.root_item.children[0]
        self.assertEqual(bin_.name, "Example Avid bin")
        self.assertEqual([c.name for c in bin_.children], ["Example clip_V", "Example clip_A01", "Example clip_A02"])
        seq = project.sequences[0]
        self.assertEqual(seq.timebase, 10584000000)
        placed = [c for t in list(seq.video_tracks) + list(seq.audio_tracks) for c in t.clips]
        self.assertEqual(len(placed), 3)
        for c in placed:
            self.assertEqual((c.start.ticks, c.end.ticks, c.in_point.ticks, c.out_point.ticks),
                             (0, 1270080000000, 254016000000, 1524096000000))
        self.assertEqual([(m.name, m.start.ticks, m.comments) for m in seq.markers], [("Locator", 508032000000, "from Avid locator")])
        root = ET.fromstring(gzip.decompress(out.read_bytes()))
        ids = {e.get("ObjectID"): e for e in root if e.get("ObjectID")}
        links = root.findall("Link")
        self.assertEqual(len(links), 1)
        members = [ids[r.get("ObjectRef")].tag for r in links[0].findall("TrackItemGroup/TrackItems/TrackItem")]
        self.assertEqual(sorted(members), ["AudioClipTrackItem", "AudioClipTrackItem", "VideoClipTrackItem"])

    def test_media_shapes_match_premiere_opatom_import(self):
        out, _ = write(copy.deepcopy(EXAMPLE), self.tmp.name)
        root = ET.fromstring(gzip.decompress(out.read_bytes()))
        media = root.findall("Media")
        self.assertEqual(len(media), 3)
        for m in media:
            fields = {c.tag for c in m}
            self.assertEqual(fields, VIDEO_MEDIA_FIELDS if m.find("VideoStream") is not None else AUDIO_MEDIA_FIELDS)
            self.assertEqual(m.findtext("Title"), "Example clip")  # media metadata keeps the Avid Name
            self.assertEqual(len(m.findtext("TapeName")), len("Example clip"[:40]))
        for li in root.findall("ClipLoggingInfo"):
            if li.findtext("ClipName") == "Example clip":
                self.assertEqual({c.tag for c in li}, LOGGING_FIELDS)
                self.assertEqual(li.findtext("ClipID"), EXAMPLE["clips"][0]["material_umid"])
        stream = root.find("VideoStream")
        self.assertEqual(stream.findtext("CodecType"), "1096180846")
        self.assertEqual(stream.findtext("FrameRate"), "10584000000")

    def test_graph_integrity_with_research_probe(self):
        spec = importlib.util.spec_from_file_location("probe", PROBE)
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        out, _ = write(copy.deepcopy(EXAMPLE), self.tmp.name)
        g = probe.probe_bytes(out.read_bytes())["graph"]
        self.assertEqual(g["top_level_duplicate_ids"], 0)
        self.assertEqual(g["cycles_top_level"], 0)
        # Exactly the 4 skeleton settings references Premiere regenerates (PRPROJ-0008).
        self.assertEqual(g["unresolved_H1_nested_scopes"], 4)

    def test_merged_clip_shape(self):
        data = copy.deepcopy(EXAMPLE)
        data["options"]["bin_mode"] = "merged"
        data["clips"][0]["audio"].reverse()  # model order A02, A01: merge must still be A01 first
        out, _ = write(data, self.tmp.name)
        root = ET.fromstring(gzip.decompress(out.read_bytes()))
        ids = {e.get("ObjectID"): e for e in root if e.get("ObjectID")}
        uids = {e.get("ObjectUID"): e for e in root if e.get("ObjectUID")}
        names = [i.findtext("ProjectItem/Name") for i in root.findall("ClipProjectItem")]
        self.assertEqual(sorted(names), sorted(["Example clip_V", "Example clip_A02", "Example clip_A01", "Example clip", "Example sequence"]))
        merged_seq = [s for s in root.findall("Sequence") if s.find("Node/Properties/BE.Sequence.IsMergedClip") is not None]
        self.assertEqual(len(merged_seq), 1)
        groups = [ids[g.get("ObjectRef")] for g in merged_seq[0].findall("TrackGroups/TrackGroup/Second")]
        counts = {g.tag: len(g.findall("TrackGroup/Tracks/Track")) for g in groups}
        self.assertEqual((counts["VideoTrackGroup"], counts["AudioTrackGroup"]), (1, 2))
        bags = {}
        for mc in root.findall("MasterClip"):
            props = {e.tag: e.text for e in mc.findall("Node/Properties/*")}
            if "MZ.MergeClipUtils.AudioTrackNumberFromOriginalMergedClip" in props:
                bags[props["MZ.MergeClipUtils.AudioTrackNumberFromOriginalMergedClip"]] = props["MZ.MergeClipUtils.ComponentMasterClipOriginalName"]
        self.assertEqual(bags, {"0": "Example clip_A01", "1": "Example clip_A02"})
        spec = importlib.util.spec_from_file_location("probe", PROBE)
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        g = probe.probe_bytes(out.read_bytes())["graph"]
        self.assertEqual((g["top_level_duplicate_ids"], g["cycles_top_level"], g["unresolved_H1_nested_scopes"]), (0, 0, 4))
        parsed = py_premiere.parse(str(out)).project
        top = {c.name: c for c in parsed.root_item.children}
        # Builds at the bin's top level, parts in the bin's own "Processed Clips" (User 2026-10-14).
        children = {c.name: c for c in top["Example Avid bin"].children}
        self.assertEqual(sorted((n, c.is_sequence) for n, c in children.items()), [("Example clip", True), ("Processed Clips", False)])
        self.assertEqual(sorted(c.name for c in children["Processed Clips"].children), ["Example clip_A01", "Example clip_A02", "Example clip_V"])
        self.assertNotIn("Processed Clips", top)

    def test_grouped_clip_shape(self):
        data = copy.deepcopy(EXAMPLE)
        data["options"]["bin_mode"] = "grouped"
        data["clips"][0]["audio"].reverse()  # model order A02, A01: group must be A01 first
        out, _ = write(data, self.tmp.name)
        parsed = py_premiere.parse(str(out)).project
        top = {c.name: c for c in parsed.root_item.children}
        self.assertNotIn("Processed Clips", top)
        avid_bin = top["Example Avid bin"]
        children = {c.name: c for c in avid_bin.children}
        self.assertEqual(sorted((n, c.is_sequence) for n, c in children.items()), [("Example clip", True), ("Processed Clips", False)])
        self.assertEqual(sorted(c.name for c in children["Processed Clips"].children),
                         ["Example clip_A01", "Example clip_A02", "Example clip_V"])
        root = ET.fromstring(gzip.decompress(out.read_bytes()))
        ids = {e.get("ObjectID"): e for e in root if e.get("ObjectID")}
        uids = {e.get("ObjectUID"): e for e in root if e.get("ObjectUID")}
        deref = lambda e: ids.get(e.get("ObjectRef")) if e.get("ObjectRef") else uids.get(e.get("ObjectURef"))
        master = [m for m in root.findall("MasterClip") if m.find("Node/Properties/Source.Monitor.Multicam.Enabled") is not None]
        self.assertEqual(len(master), 1)
        self.assertEqual(len(master[0].findall("AudioComponentChains/AudioComponentChain")), 2)
        inner = [s for s in root.findall("Sequence") if s.findtext("Name") == "Example clip"][0]
        order = []
        for g in inner.findall("TrackGroups/TrackGroup/Second"):
            g = deref(g)
            if g.tag == "AudioTrackGroup":
                for t in g.findall("TrackGroup/Tracks/Track"):
                    item = deref(deref(t).find("ClipTrack/ClipItems/TrackItems/TrackItem"))
                    sub = deref(item.find("ClipTrackItem/SubClip"))
                    media = deref(deref(deref(sub.find("Clip")).find("Clip/Source")).find("MediaSource/Media"))
                    order.append(media.findtext("FilePath").rsplit("/", 1)[-1][:3])
        self.assertEqual(order, ["A01", "A02"])
        self.assertEqual(inner.findall("PersistentGroupContainer/LinkContainer/Links/Link"), [])
        spec = importlib.util.spec_from_file_location("probe", PROBE)
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        g = probe.probe_bytes(out.read_bytes())["graph"]
        self.assertEqual((g["top_level_duplicate_ids"], g["cycles_top_level"], g["unresolved_H1_nested_scopes"]), (0, 0, 4))

    def _placements(self, out):
        root = ET.fromstring(gzip.decompress(out.read_bytes()))
        ids = {e.get("ObjectID"): e for e in root if e.get("ObjectID")}
        uids = {e.get("ObjectUID"): e for e in root if e.get("ObjectUID")}
        deref = lambda e: ids.get(e.get("ObjectRef")) if e.get("ObjectRef") else uids.get(e.get("ObjectURef"))
        seq = [s for s in root.findall("Sequence") if s.findtext("Name") == "Example sequence"][0]
        rows = []
        for g in seq.findall("TrackGroups/TrackGroup/Second"):
            g = deref(g)
            for n, t in enumerate(g.findall("TrackGroup/Tracks/Track")):
                for x in deref(t).findall("ClipTrack/ClipItems/TrackItems/TrackItem"):
                    item = deref(x)
                    clip = deref(deref(item.find("ClipTrackItem/SubClip")).find("Clip"))
                    src = deref(clip.find("Clip/Source"))
                    media = deref(src.find("MediaSource/Media")) if src.find("MediaSource/Media") is not None else None
                    what = media.findtext("FilePath").rsplit("/", 1)[-1][:3] if media is not None else src.tag
                    channel = [deref(c).findtext("ChannelIndex") for c in clip.findall("SecondaryContents/SecondaryContentItem")]
                    rows.append((g.tag[:5], n, what, channel, clip.findtext("Clip/InPoint"), clip.findtext("Clip/OutPoint")))
        links = [len(deref(l).findall("TrackItemGroup/TrackItems/TrackItem")) for l in seq.findall("PersistentGroupContainer/LinkContainer/Links/Link")]
        return rows, links

    def test_timeline_uses_combined_clips(self):
        inn, out_ = "254016000000", "1524096000000"
        for mode, expected in {
            "merged": [("Video", 0, "V01", [], inn, out_), ("Audio", 0, "A01", ["0"], inn, out_), ("Audio", 1, "A02", ["0"], inn, out_)],
            "grouped": [("Video", 0, "VideoSequenceSource", [], inn, out_), ("Audio", 0, "AudioSequenceSource", ["0"], inn, out_),
                        ("Audio", 1, "AudioSequenceSource", ["1"], inn, out_)],
        }.items():
            data = copy.deepcopy(EXAMPLE)
            data["options"]["bin_mode"] = mode
            with self.subTest(mode):
                sub = Path(self.tmp.name) / mode
                sub.mkdir()
                out, _ = write(data, sub)
                rows, links = self._placements(out)
                self.assertEqual(rows, expected)
                self.assertEqual(links, [3])
                root = ET.fromstring(gzip.decompress(out.read_bytes()))
                for core in root.iter("Clip"):
                    tags = [c.tag for c in core]
                    if "InPoint" in tags and "ClipID" in tags:  # Premiere's order
                        self.assertLess(tags.index("ClipID"), tags.index("InPoint"))
                        self.assertLess(tags.index("InPoint"), tags.index("OutPoint"))
                flags = [c.findtext("Clip/IsMulticam") for c in root.findall("VideoClip") if c.find("Clip/IsMulticam") is not None]
                self.assertEqual(flags, ["true"] if mode == "grouped" else [])

    def test_audio_gain_volume_track_level_and_crossfade(self):
        import math
        F = 10584000000
        data = copy.deepcopy(EXAMPLE)
        seq = data["sequences"][0]
        seq["events"] = [
            {"clip": "clip-1", "record_start_ticks": 0, "source_in_ticks": 24 * F, "duration_ticks": 200 * F,
             "video_track": 1, "audio_tracks": [1, 2],
             "audio": {"volume_keys": [{"source_ticks": 72 * F, "db": 0}, {"source_ticks": 108 * F, "db": 12},
                                       {"source_ticks": 168 * F, "db": -17.4}]}},
            {"clip": "clip-1", "record_start_ticks": 0, "source_in_ticks": 0, "duration_ticks": 120 * F, "audio_tracks": [3, 4],
             "audio": {"gain_db": -3}},
            {"clip": "clip-1", "record_start_ticks": 120 * F, "source_in_ticks": 150 * F, "duration_ticks": 90 * F, "audio_tracks": [3, 4]},
        ]
        seq["track_levels"] = [{"audio_track": 1, "db": 2}]
        seq["audio_crossfades"] = [{"audio_track": n, "cut_ticks": 120 * F, "duration_ticks": 10 * F, "cut_offset_ticks": 3 * F} for n in (3, 4)]
        out, report = write(data, self.tmp.name)
        self.assertTrue(any("6.000 dB moved into audio gain" in n for n in report.notes))
        root = ET.fromstring(gzip.decompress(out.read_bytes()))
        ids = {e.get("ObjectID"): e for e in root if e.get("ObjectID")}
        db = lambda v: 20 * math.log10(v) + 15
        filters = [e for e in root.findall("AudioFilterComponent") if e.findtext("FilterMatchName") == "Internal Volume Mono"]
        self.assertEqual(len(filters), 2)  # both audio placements of event 1
        level = [ids[p.get("ObjectRef")] for p in filters[0].iter("Param") if ids[p.get("ObjectRef")].findtext("Name") == "Level"][0]
        keys = [k.split(",") for k in level.findtext("Keyframes").strip(";").split(";")]
        self.assertEqual([int(k[0]) for k in keys], [72 * F, 108 * F, 168 * F])
        self.assertEqual([round(db(float(k[1])), 3) for k in keys], [-6.0, 6.0, -23.4])  # +12 peak: 6 dB moved to gain
        self.assertTrue(all(len(k) == 8 for k in keys))
        gains = sorted(round(20 * math.log10(float(c.findtext("Gain"))), 3) for c in root.findall("AudioClip") if c.find("Gain") is not None)
        self.assertEqual(gains, [-3.0, -3.0, 6.0, 6.0])
        fader_volumes = [round(20 * math.log10(float(p.findtext("CurrentValue"))), 3) for p in root.findall("AudioComponentParam")
                         if p.findtext("Name") == "Volume" and p.findtext("CurrentValue") not in (None, "1")]
        self.assertEqual(fader_volumes, [2.0])
        fades = root.findall("AudioTransitionTrackItem")
        self.assertEqual(len(fades), 2)
        for f in fades:
            self.assertEqual((int(f.findtext("TransitionTrackItem/TrackItem/Start")), int(f.findtext("TransitionTrackItem/TrackItem/End")),
                              int(f.findtext("TransitionTrackItem/Alignment"))), (117 * F, 127 * F, 3 * F))
        g = self._probe(out)
        self.assertEqual((g["top_level_duplicate_ids"], g["cycles_top_level"], g["unresolved_H1_nested_scopes"]), (0, 0, 4))

        seq["audio_crossfades"][0]["style"] = "constant_gain"
        with tempfile.TemporaryDirectory() as folder:
            linear, _ = write(data, folder)
            root = ET.fromstring(gzip.decompress(linear.read_bytes()))
            fade, = [f for f in root.findall("AudioTransitionTrackItem")
                     if f.findtext("TransitionTrackItem/MatchName") == "Constant Gain"]
            self.assertEqual(fade.findtext("TransitionTrackItem/Alignment"), str(3 * F))
            self.assertEqual(fade.findtext("FadeShapeType"), "0")
            self.assertEqual(fade.findtext("FadeShapeValue"), "0")

    def _probe(self, out):
        spec = importlib.util.spec_from_file_location("probe", PROBE)
        probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(probe)
        return probe.probe_bytes(out.read_bytes())["graph"]

    def test_refusals(self):
        data = copy.deepcopy(EXAMPLE)
        data["options"]["link_media_in_timeline"] = False  # removed option is refused, not ignored
        with self.assertRaises(Exception):
            write(data, self.tmp.name)
        data = copy.deepcopy(EXAMPLE)
        data["clips"][0]["video"]["rate"] = "25/1"
        data["clips"][0]["video"]["duration_ticks"] = 2540160000000
        with self.assertRaises(UnverifiedShape):
            write(data, self.tmp.name)
        out, _ = write(copy.deepcopy(EXAMPLE), self.tmp.name)
        with self.assertRaises(FileExistsError):
            write_project(parse_model(copy.deepcopy(EXAMPLE)), out)


class MediaStartTest(unittest.TestCase):
    def test_start_timecode_makes_logging_points_absolute(self):
        import gzip, json, tempfile
        from avid2prproj.model import parse_model
        data = copy.deepcopy(EXAMPLE)
        hour = 3600 * 254016000000
        data["clips"][0]["video"]["start_ticks"] = hour
        for a in data["clips"][0]["audio"]:
            a["start_ticks"] = hour
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "s.prproj"
            write_project(parse_model(data), out)
            xml = gzip.open(out).read().decode()
        self.assertEqual(xml.count(f"<Start>{hour}</Start>"), 3)
        self.assertEqual(xml.count(f"<MediaInPoint>{hour}</MediaInPoint>"), 3)
        self.assertEqual(xml.count("<AlternateStart>0</AlternateStart>"), 3)


class TimeDisplayTest(unittest.TestCase):
    def test_audio_master_clips_get_the_avid_time_display(self):
        data = copy.deepcopy(EXAMPLE)
        data["clips"][0]["time_display"] = 100
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "t.prproj"
            write_project(parse_model(data), out)
            root = ET.fromstring(gzip.open(out).read().decode())
        masters = [m for m in root.iter("MasterClip") if m.find("Name") is not None]
        audio = [m for m in masters if m.findtext("Name", "").rsplit("_", 1)[-1].startswith("A0")]
        self.assertEqual(len(audio), 2)
        for m in masters:
            tags = [c.tag for c in m]
            if m in audio:
                self.assertEqual(m.findtext("TimeDisplay"), "100")
                self.assertEqual(tags.index("TimeDisplay") + 1, tags.index("MasterClipChangeVersion"))
            else:
                self.assertIsNone(m.find("TimeDisplay"))


if __name__ == "__main__":
    unittest.main()
