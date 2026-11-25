"""Verify the packaged Avid-to-Resolve media choices with a synthetic split edit."""

from fractions import Fraction
import json
import pathlib
import subprocess
import zipfile

from build_ffmpeg import sha


ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/fixtures/avid-resolve-release/split-edit.zip"
VIDEO = "Picture.mxf"
AUDIO = ("Left.mxf", "Right.mxf")
BIN = "Synthetic Split Edit.avb"
SEQUENCE = "Synthetic Split Edit"
RATE = Fraction(24000, 1001)
PICTURE_FRAMES = 24
MONO_SAMPLES = PICTURE_FRAMES * 2002
EDITS = {
    ("video", 1, 0): (3, 8),
    ("video", 1, 8): (13, 8),
    ("audio", 1, 0): (2, 6),
    ("audio", 1, 6): (11, 10),
    ("audio", 2, 0): (5, 10),
    ("audio", 2, 10): (17, 6),
}


def expected_receipt():
    return {
        "fixture_sha256": sha(FIXTURE),
        "modes": ["direct", "op1a"],
        "edits_per_mode": len(EDITS),
        "decoded_picture_frames": PICTURE_FRAMES,
        "pcm_samples_per_channel": MONO_SAMPLES,
        "pcm_channels": len(AUDIO),
    }


def require(condition, message):
    if not condition:
        raise ValueError(message)


def run(*args):
    return subprocess.check_output(args, timeout=90)


def unpack(folder):
    folder.mkdir()
    names = {BIN, VIDEO, *AUDIO}
    with zipfile.ZipFile(FIXTURE) as archive:
        members = archive.infolist()
        require(len(members) == len(names) and {m.filename for m in members} == names,
                "unexpected Avid-to-Resolve fixture inventory")
        for member in members:
            require(0 < member.file_size <= 6 << 20, "invalid smoke fixture member size")
            (folder / member.filename).write_bytes(archive.read(member))


def check_graph(graph, paths, rewrapped):
    sequence, = graph["sequences"]
    require(sequence["name"] == SEQUENCE and sequence["frame_size"] == [1920, 1080]
            and Fraction(sequence["rate"]) == RATE and Fraction(sequence["start"]) * RATE == 86407,
            "synthetic sequence name, raster or clock changed")
    item, = graph["items"]
    media = {row["id"]: row for row in graph["media"]}
    require(len(media) == len(item["media"]) == 3, "expected one picture and two mono bindings")
    for binding in item["media"]:
        row = media[binding["media"]]
        kind, lane = row["kind"], row["stream"]
        require(row["locations"] == [str(paths[kind, lane])]
                and Fraction(row["start"]) * RATE == 103683
                and Fraction(row["duration"]) * RATE == PICTURE_FRAMES
                and Fraction(binding["source_offset"]) == 0,
                "saved synthetic path, clock, source extent or offset changed")
        if kind == "audio":
            receipt, = (s["data"] for s in graph["sidecars"]
                        if s["adapter"] == "resolve.source" and s["object"] == row["id"])
            require(receipt["saved_selector"] == lane - 1
                    and receipt["saved_cache_track"] == (lane - 1 if rewrapped else 0),
                    "saved synthetic physical audio selector changed")

    remaining = dict(EDITS)
    for row in sequence["placements"]:
        require(row["item"] == item["id"], "synthetic edit references the wrong item")
        if row["video_track"] is not None:
            kind, lane = "video", row["video_track"]
            require(not row["audio_tracks"] and not row["audio_streams"],
                    "picture-only edit acquired audio")
        else:
            kind = "audio"
            lane, = row["audio_tracks"]
            require(row["audio_streams"] == [lane], "synthetic edit changed audio lane")
        start = Fraction(row["record_start"]) * RATE
        bounds = (Fraction(row["source_in"]) * RATE, Fraction(row["duration"]) * RATE)
        require(remaining.pop((kind, lane, start), None) == bounds, "synthetic split edit range changed")
    require(not remaining, "synthetic split edit is missing placements")


def decode(ffmpeg, path, stream, output):
    return run(ffmpeg, "-nostdin", "-v", "error", "-i", path, "-map", stream, "-f", output, "-")


def check_essence(tools, source, op1a):
    ffmpeg = tools / "ffmpeg"
    info = json.loads(run(tools / "ffprobe", "-v", "error", "-show_streams",
                          "-show_format", "-of", "json", op1a))
    pattern = info["format"]["tags"]["operational_pattern_ul"].split(".")
    require(pattern[-1].startswith("0101"), "rewrap did not write OP1a media")
    picture, *audio = info["streams"]
    require(picture["codec_type"] == "video" and picture["profile"] == "DNXHR LB",
            "rewrap changed the picture stream format")
    require(len(audio) == len(AUDIO) and all(
        (row["codec_name"], row["channels"], row["sample_rate"], row["duration_ts"])
        == ("pcm_s24le", 1, "48000", MONO_SAMPLES) for row in audio
    ), "rewrap changed the PCM24 track layout or sample extent")

    def frames(path):
        return [line.rsplit(b",", 1)[-1].strip()
                for line in decode(ffmpeg, path, "0:v:0", "framehash").splitlines()
                if not line.startswith(b"#")]

    reference = frames(source / VIDEO)
    require(len(reference) == len(set(reference)) == PICTURE_FRAMES, "invalid numbered picture control")
    require(frames(op1a) == reference, "rewrap changed decoded picture frames")
    channels = [decode(ffmpeg, source / name, "0:a:0", "s24le") for name in AUDIO]
    require(channels[0] != channels[1], "synthetic mono controls must be distinct")
    for i, pcm in enumerate(channels):
        require(len(pcm) == MONO_SAMPLES * 3 and any(pcm), "invalid synthetic PCM control")
        require(decode(ffmpeg, op1a, f"0:a:{i}", "s24le") == pcm, f"rewrap changed PCM lane {i + 1}")


def verify(bundle, probe, work):
    """Use the extracted app's own CLI, FFmpeg and templates."""
    work = work.resolve()
    work.mkdir()
    source = work / "source"
    unpack(source)
    inputs = {p: sha(p) for p in source.iterdir()}
    contents = bundle / "Contents"
    tools = contents / "MacOS"
    cli = tools / "any-nle"
    templates = contents / "Resources" / "resolve-templates"
    output_media = work / "rewrapped"
    direct_paths = {("video", 1): source / VIDEO, ("audio", 1): source / AUDIO[0],
                    ("audio", 2): source / AUDIO[1]}
    for mode in ("direct", "op1a"):
        rewrapped = mode == "op1a"
        output = work / f"{mode}.drp"
        job = {
            "schema": "anynle.job/2",
            "source": {
                "route": "avid_to_resolve",
                "bins": [source / BIN],
                "sequences": [SEQUENCE],
                "media_roots": [source],
                "ffmpeg": tools / "ffmpeg",
                "op1a_rewrap_dir": output_media if rewrapped else None,
            },
            "templates": {role: templates / name for role, name in
                          (("layout", "Layout.drp"), ("profile", "Layout.drp"),
                           ("atom", "Atom.drp"), ("group", "Group.drp"))},
            "ffprobe": tools / "ffprobe",
            "output": output,
        }
        request = work / f"{mode}.json"
        request.write_text(json.dumps(job, default=str))
        review = json.loads(run(cli, "review", "--job", request))
        require(review["status"] == "ready" and not review["losses"]
                and not review["media_linking"]["resolve_files"],
                f"{mode} smoke job did not pass standalone Review")
        require(not output.exists() and not output_media.exists(), "Review published synthetic outputs")
        if rewrapped:
            require(review["rewrap_estimate"]["outputs"] == 1, "expected one OP1a output in Review")
        else:
            require(review["media_linking"]["direct_files"] == 3, "expected three direct MXF links")
        run(cli, "run", "--job", request)
        graph = work / f"{mode}.graph.json"
        run(probe, output, graph)
        paths = direct_paths
        if rewrapped:
            op1a, = output_media.glob("*.mxf")
            paths = dict.fromkeys(direct_paths, op1a)
            check_essence(tools, source, op1a)
        else:
            require(not output_media.exists(), "direct conversion unexpectedly created media")
        check_graph(json.loads(graph.read_bytes()), paths, rewrapped)
    require({p: sha(p) for p in source.iterdir()} == inputs, "synthetic source files changed")
    return expected_receipt()
