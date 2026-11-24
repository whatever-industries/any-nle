#!/usr/bin/env python3
"""Generate linked-media controls and check saved graph/2 against their edits."""

import argparse
import json
import math
import subprocess
import wave
from fractions import Fraction
from pathlib import Path


RATE = Fraction(24000, 1001)
ORIGIN = 86407
FRAME_SIZE = (1920, 1080)
VIDEO_FRAMES = 64
SAMPLE_RATE = 48000
SAMPLES = 144000
SAMPLES_PER_FRAME = SAMPLE_RATE / RATE


def expectation(bits):
    audio_length = int(48 * SAMPLES_PER_FRAME)
    return {
        "rate": str(RATE),
        "start_timecode": "01:00:00:07",
        "origin_frames": ORIGIN,
        "frame_size": list(FRAME_SIZE),
        "picture_frame_numbers": list(range(5, 29)) + list(range(264, 288)),
        "edits": [
            {"kind": "video", "lane": 1, "record_frames": [0, 24],
             "path": "picture/a/Counter.mov", "stream": 0, "channel": None,
             "source_frames": [5, 29]},
            {"kind": "video", "lane": 1, "record_frames": [24, 48],
             "path": "picture/b/Counter.mov", "stream": 0, "channel": None,
             "source_frames": [8, 32]},
            *({"kind": "audio", "lane": lane, "record_frames": [0, 48],
               "path": f"audio/Signals-{bits}.wav", "stream": 0, "channel": lane - 1,
               "source_samples": [start, start + audio_length]}
              for lane, start in [(1, 6006), (2, 24024)]),
        ],
    }


def write_picture(path, first_code, ffmpeg):
    width, height = FRAME_SIZE
    path.parent.mkdir(parents=True)
    # 16 cells encode an unsigned frame number and survive native ProRes renders.
    background = bytes([42 if first_code == 0 else 94]) * (width * height)
    chroma = bytes([128]) * (width * height // 2)
    cmd = [ffmpeg, "-nostdin", "-v", "error", "-n", "-f", "rawvideo",
           "-pixel_format", "yuv420p", "-video_size", f"{width}x{height}",
           "-framerate", str(RATE), "-i", "-", "-vf", "setsar=1", "-c:v", "prores_ks", "-profile:v", "3",
           "-pix_fmt", "yuv422p10le", "-color_range", "tv", "-colorspace", "bt709",
           "-color_trc", "bt709", "-color_primaries", "bt709", "-threads", "2",
           "-video_track_timescale", "24000", path]
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as enc:
        for code in range(first_code, first_code + VIDEO_FRAMES):
            row = b"".join(bytes([218 if code & (1 << bit) else 25]) * 80
                           for bit in range(16))
            row += background[:width - len(row)]
            enc.stdin.write(background[:80 * width] + row * 72 + background[152 * width:] + chroma)
        enc.stdin.close()
        status = enc.wait(timeout=120)
        if status:
            raise subprocess.CalledProcessError(status, cmd)


def write_wave(path, bits):
    width = bits // 8
    audio = bytearray()
    for sample in range(SAMPLES):
        for frequency, amplitude in [(431, 0.31), (683, 0.56)]:
            value = math.sin(math.tau * frequency * sample / SAMPLE_RATE)
            audio.extend(round(value * amplitude * (1 << (bits - 1))).to_bytes(width, "little", signed=True))
    with wave.open(str(path), "wb") as writer:
        writer.setparams((2, width, SAMPLE_RATE, SAMPLES, "NONE", "not compressed"))
        writer.writeframes(audio)


def generate(root, ffmpeg):
    root.mkdir(parents=True)
    for folder, code in [("a", 0), ("b", 256)]:
        write_picture(root / f"picture/{folder}/Counter.mov", code, ffmpeg)
    (root / "audio").mkdir()
    for bits in (16, 24):
        write_wave(root / f"audio/Signals-{bits}.wav", bits)
    manifest = {str(bits): expectation(bits) for bits in (16, 24)}
    (root / "expected.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def check_graph(root, graph_path, sequence_id, bits, resolve_cache):
    expected = json.loads((root / "expected.json").read_text(encoding="utf-8"))[str(bits)]
    graph = json.loads(graph_path.read_text(encoding="utf-8"))
    [seq] = [seq for seq in graph["sequences"] if seq["id"] == sequence_id]
    def require(actual, wanted, subject):
        if actual != wanted:
            raise ValueError(f"{subject}: expected {wanted!r}, found {actual!r}")

    require(graph["schema"], "anynle.graph/2", "schema")
    require(Fraction(seq["rate"]), RATE, "sequence rate")
    require(Fraction(seq["start"]) * RATE, ORIGIN, "sequence start")
    require(seq["frame_size"], expected["frame_size"], "sequence raster")
    require(sorted((track["kind"], track["index"]) for track in seq["tracks"]
                   if not track.get("empty")),
            [("audio", 1), ("audio", 2), ("video", 1)], "populated sequence lanes")
    items = {item["id"]: item for item in graph["items"]}
    media = {row["id"]: row for row in graph["media"]}
    edits = {(row["kind"], row["lane"], row["record_frames"][0]): row for row in expected["edits"]}
    for edit in seq["placements"]:
        picture = edit["video_track"] is not None
        kind = "video" if picture else "audio"
        lane = edit["video_track"] if picture else edit["audio_tracks"][0]
        require(edit["audio_tracks"], [] if picture else [lane], edit["id"])
        require(edit["grouped_audio"], [], edit["id"])
        start = Fraction(edit["record_start"])
        length = Fraction(edit["duration"])
        row = edits.pop((kind, lane, start * RATE), None)
        if row is None:
            raise ValueError(f"Unexpected edit {edit['id']}")
        require((start + length) * RATE, row["record_frames"][1], edit["id"])
        bindings = []
        for link in items[edit["item"]]["media"]:
            source = media[link["media"]]
            if source["kind"] == kind and (picture or [source["stream"]] == edit["audio_streams"]):
                bindings.append((link, source))
        [(link, source)] = bindings
        require(link["source_offset"], "0", f"{edit['id']} item-to-file offset")
        require(source["locations"], [str(root / row["path"])], f"{edit['id']} linked path")
        trim = Fraction(edit["source_in"])
        units = RATE if picture else SAMPLE_RATE
        require([trim * units, (trim + length) * units],
                row["source_frames" if picture else "source_samples"], edit["id"])
        original = source.get("original")
        if picture:
            require(source["rate"], str(RATE), source["id"])
            require(Fraction(source["duration"]) * RATE, VIDEO_FRAMES, source["id"])
            if original is not None:
                require(original["stream_index"], 0, source["id"])
            continue
        require(source["sample_rate"], SAMPLE_RATE, source["id"])
        require(source["bits"], bits, source["id"])
        require(Fraction(source["duration"]) * SAMPLE_RATE, SAMPLES, source["id"])
        require(original["saved_locators"], source["locations"], source["id"])
        require(original["container"], "wav", source["id"])
        require(original["stream_index"], row["stream"], source["id"])
        require(original["channel_index"], row["channel"], source["id"])
        require(original["channels"], 2, source["id"])
        require(Fraction(original["time_base"]), Fraction(1, SAMPLE_RATE), source["id"])
        require(original["start_pts"], 0, source["id"])
        require(original["duration_ts"], SAMPLES, source["id"])
        require(original["sample_representation"], f"pcm{bits}", source["id"])
        if resolve_cache:
            require(source["online"], None, source["id"])
    require(edits, {}, "missing edits")


def picture_numbers(path, ffmpeg):
    data = subprocess.check_output([
        ffmpeg, "-nostdin", "-v", "error", "-i", path, "-an",
        "-vf", "crop=1280:24:0:96,scale=16:1:flags=neighbor,format=gray",
        "-f", "rawvideo", "-"], timeout=60)
    if len(data) % 16:
        raise ValueError("Incomplete picture counter")
    return [sum((value > 128) << bit for bit, value in enumerate(data[i:i + 16]))
            for i in range(0, len(data), 16)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    media = commands.add_parser("generate")
    media.add_argument("directory", type=Path)
    media.add_argument("--ffmpeg", default="ffmpeg")
    graph = commands.add_parser("check-graph")
    graph.add_argument("directory", type=Path)
    graph.add_argument("graph", type=Path)
    graph.add_argument("sequence_id")
    graph.add_argument("bits", type=int, choices=[16, 24])
    graph.add_argument("--resolve-cache", action="store_true")
    picture = commands.add_parser("check-picture")
    picture.add_argument("render", type=Path)
    picture.add_argument("--ffmpeg", default="ffmpeg")
    args = parser.parse_args()
    if args.command == "generate":
        generate(args.directory.resolve(), args.ffmpeg)
    elif args.command == "check-graph":
        check_graph(args.directory.resolve(), args.graph, args.sequence_id, args.bits, args.resolve_cache)
    else:
        actual = picture_numbers(args.render, args.ffmpeg)
        if actual != expectation(16)["picture_frame_numbers"]:
            raise ValueError(f"Unexpected picture frames: {actual}")
    print(f"{args.command} passed")


if __name__ == "__main__":
    main()
