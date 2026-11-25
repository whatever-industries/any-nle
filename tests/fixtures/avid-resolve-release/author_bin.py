"""Author the fixture bin against generated MXF files. Requires pyavb."""
import json
from pathlib import Path
import subprocess
import sys

import avb
from avb.mobid import MobID

RATE = 23.976
EXTENT = 24
SOURCE_START = (1 * 3600 + 12 * 60) * 24 + 3
SEQUENCE_START = 3600 * 24 + 7


def probe(ffprobe, path):
    result = subprocess.check_output(
        [ffprobe, "-v", "error", "-show_format", "-show_streams", "-of", "json", path]
    )
    return json.loads(result)


def mob_id(tag):
    value = tag.removeprefix("0x")
    return MobID("urn:smpte:umid:" + ".".join(value[i:i+8] for i in range(0, 64, 8)))


def author(root, ffprobe):
    names = ["Picture.mxf", "Left.mxf", "Right.mxf"]
    facts = [probe(ffprobe, root / name) for name in names]
    materials = {entry["format"]["tags"]["material_package_umid"] for entry in facts}
    material, = materials

    with avb.open() as document:
        def composition(name, kind, length):
            obj = document.create.Composition(mob_type=kind)
            obj.name = name
            obj.edit_rate = RATE
            obj.length = length
            return obj

        def source(kind, length, mob, lane, start=0):
            obj = document.create.SourceClip(edit_rate=RATE, media_kind=kind)
            obj.length = length
            obj.mob_id = mob
            obj.track_id = lane
            obj.start_time = start
            return obj

        def track(index, component):
            obj = document.create.Track()
            obj.index = index
            obj.component = component
            return obj

        def clock(length, start):
            obj = document.create.Timecode(edit_rate=RATE, media_kind="timecode")
            obj.fps = 24
            obj.length = length
            obj.start = start
            return obj

        master = composition("Synthetic Release Media", "MasterMob", EXTENT)
        master.mob_id = mob_id(material)
        for i, entry in enumerate(facts):
            kind, lane = ("picture", 1) if i == 0 else ("sound", i)
            package = mob_id(entry["streams"][i]["tags"]["file_package_umid"])
            file = composition("", "SourceMob", EXTENT)
            file.mob_id = package
            file.tracks.extend((track(1, source(kind, EXTENT, MobID(), 0)),
                                track(1, clock(EXTENT, SOURCE_START))))
            if i == 0:
                desc = document.create.CDCIDescriptor()
                desc.edit_rate = RATE
                desc.length = EXTENT
                desc.stored_width = desc.sampled_width = desc.display_width = 1920
                desc.stored_height = 1080
                desc.sampled_height = desc.display_height = 1080
                desc.aspect_ratio = [16, 9]
                desc.component_width = 8
                desc.horizontal_subsampling = 2
                desc.vertical_subsampling = 1
                desc.compress_method = b"RI00"
            else:
                desc = document.create.PCMADescriptor()
                desc.edit_rate = desc.sample_rate = 48000
                desc.length = EXTENT * 2002
                desc.channels = 1
                desc.quantization_bits = 24
                desc.block_align = 3
                desc.average_bps = 144000
                desc.locked = True
                desc.audio_ref_level = -20
                desc.electro_spatial_formulation = 2
                desc.dial_norm = 27
                desc.coding_format = 1
                desc.has_peak_envelope_data = False
                desc.peak_of_peaks_offset = (1 << 64) - 1
                for field in ("sequence_offset", "peak_envelope_version", "peak_envelope_format",
                              "points_per_peak_value", "peak_envelope_block_size", "peak_channel_count",
                              "peak_frame_count", "peak_envelope_timestamp"):
                    setattr(desc, field, 0)
            desc.mob_kind = 1
            locator = document.create.MSMLocator()
            locator.last_known_volume = locator.last_known_volume_utf8 = "Synthetic"
            locator.domain_type = 4
            locator.mob_id = package
            desc.locator = locator
            file.descriptor = desc
            document.content.add_mob(file)
            master.tracks.append(track(lane, source(kind, EXTENT, package, 1)))
        master.tracks.append(track(1, clock(EXTENT, SOURCE_START)))
        document.content.add_mob(master)

        timeline = composition("Synthetic Split Edit", "CompositionMob", 16)
        timeline.attributes["SEQUERNCE_FORMAT_STRING"] = "HD 1080p/23.976"
        timeline.attributes["SEQUERNCE_FORMAT_TYPE"] = 10
        timeline.tracks.append(track(1, clock(16, SEQUENCE_START)))
        for kind, lane, edits in [
            ("picture", 1, [(3, 8), (13, 8)]),
            ("sound", 1, [(2, 6), (11, 10)]),
            ("sound", 2, [(5, 10), (17, 6)]),
        ]:
            seq = document.create.Sequence(edit_rate=RATE, media_kind=kind)
            seq.components.extend(source(kind, length, master.mob_id, lane, trim)
                                  for trim, length in edits)
            timeline.tracks.append(track(lane, seq))
        document.content.add_mob(timeline)
        document.write(str(root / "Synthetic Split Edit.avb"))


if __name__ == "__main__":
    author(Path(sys.argv[1]), sys.argv[2])
