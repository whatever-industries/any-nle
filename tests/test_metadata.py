import gzip
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

import avid2prproj  # noqa: F401  (adds vendor/ to sys.path)
from avid2prproj import metadata
from avid2prproj.model import AuxTimecode, AvidInfo, Model, load_model
from avid2prproj.writer import write_project

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "opatom_clip.model.json"
AVID = AvidInfo(
    mob_id="urn:smpte:umid:test",
    columns=(("Camroll", "A001"), ("Take", "3"), ("Comments", "nice"), ("Soundroll", "SR1"), ("UNC Path", "/x/y.mov")),
    start_timecode="01:00:00:00",
    aux_timecodes=(AuxTimecode(5, "00:00:00:09", 9, 24),),
    source_file="y.mov",
    source_path="/x/",
)


class MetadataTest(unittest.TestCase):
    def test_mapping(self):
        logging, bag = metadata.premiere_fields(AVID)
        self.assertEqual([k for k, _ in logging], ["ShotTake", "FilmCameraRoll", "Description", "LogNote"])
        fields = dict(logging)
        self.assertEqual(fields["FilmCameraRoll"], "A001")
        self.assertEqual(fields["ShotTake"], "3")
        self.assertEqual(fields["Description"], "y.mov")
        self.assertEqual(fields["LogNote"], "Soundroll: SR1 | Source Path: /x/ | Avid Start: 01:00:00:00 | Aux TC3: 00:00:00:09")
        self.assertEqual(bag, [("Column.PropertyText.Comment", "nice")])

    def test_every_item_gets_the_fields_including_childless_video_items(self):
        model: Model = load_model(EXAMPLE)
        clips = {k: replace(c, avid=AVID) for k, c in model.clips.items()}
        for mode in ("separate", "merged", "grouped"):
            with tempfile.TemporaryDirectory() as d:
                out = Path(d) / "m.prproj"
                write_project(replace(model, clips=clips, options=replace(model.options, bin_mode=mode)), out)
                xml = gzip.open(out).read().decode()
                expected = 3 if mode == "separate" else 4
                self.assertEqual(xml.count("<FilmCameraRoll>A001</FilmCameraRoll>"), expected, mode)
                self.assertNotIn("<TapeName>A001</TapeName>", xml, mode)
                self.assertEqual(xml.count("<Column.PropertyText.Comment>nice<"), expected, mode)
                self.assertEqual(xml.count("<ShotTake>3</ShotTake>"), expected, mode)


if __name__ == "__main__":
    unittest.main()
