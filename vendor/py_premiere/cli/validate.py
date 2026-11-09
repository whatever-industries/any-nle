"""
Premiere ground-truth validation tool.

Compares py_premiere's parsed output against the ExtendScript JSON exported
by `scripts/jsx/export_project_json.jsx` - the DOM values Premiere itself
reports for the same file.

Modes:
    Validate: pr-validate file.prproj                (expects file.json)
    Explicit: pr-validate file.prproj --json gt.json
    Batch:    pr-validate a.prproj b.prproj c.prproj
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import TYPE_CHECKING

import py_premiere

from ..data.param_classes import VERIFIED_VALUE_CLASS_IDS
from ..models.time import UNSET_TICKS

if TYPE_CHECKING:
    from typing import Any

    from ..models import (
        ComponentParam,
        Marker,
        ProjectItem,
        Sequence,
        Time,
        Track,
        TrackItem,
    )


class Problems(list):  # type: ignore[type-arg]
    """The mismatch list, which also records WHICH ground-truth keys were read.

    Subclassing `list` keeps every existing call site working while giving
    `--coverage` the data it needs: a key present in the JSON but never
    asserted is a silently-ignored field, not a passing check.
    """

    def __init__(self) -> None:
        super().__init__()
        self.asserted: set[str] = set()
        self.divergences: list[str] = []
        self.fixture = ""

    def note(self, path: str, label: str) -> None:
        self.asserted.add(shape_key(path, label))

    def record(self, path: str, label: str, message: str) -> None:
        """File a mismatch, unless it is a documented ES divergence."""
        reason = KNOWN_DIVERGENCES.get((self.fixture, path, label))
        if reason is None:
            self.append(message)
        else:
            self.divergences.append(f"{message}  [{reason}]")


#: Ground-truth values that are NOT derivable from the project file, so no
#: amount of parsing can match them. Each needs positive evidence, named
#: here - never add one to quiet a mismatch that has not been explained.
KNOWN_DIVERGENCES = {
    (
        "34_generators",
        "rootItem/[5]",
        "colorLabel",
    ): "nothing stored; ES falls back to the live BE.Prefs.LabelDefaults.Still (3)",
    (
        "34_generators",
        "rootItem/[6]",
        "colorLabel",
    ): "nothing stored; ES falls back to BE.Prefs.LabelDefaults.AV (1)",
    (
        "34_generators",
        "rootItem/[8]",
        "outPoint",
    ): "IsContinuousTime media; ES asks the importer, the file stores 6 frames",
    (
        "48_smart_bin",
        "rootItem/[4]",
        "colorLabel",
    ): "nothing stored; ES falls back to BE.Prefs.LabelDefaults.Bin (7)",
    (
        "49_seq_settings",
        "sequences[1] ('Seq B')",
        "videoDisplayFormat",
    ): "110 (23.976 TC) is illegal for a 29.97 sequence; Premiere reverts to "
    "102 on load while the file keeps 110",
    (
        "54_display_format",
        "sequences[1] ('Seq B')",
        "videoDisplayFormat",
    ): "same rejection, and here ExtendScript ITSELF wrote the 110 - the "
    "audioDisplayFormat 201 written in the same call does survive",
    (
        "51_ae_comp",
        "rootItem/[4]",
        "isOffline",
    ): "dynamic link reports offline until After Effects has rendered it; the "
    "file stores no OfflineReason",
    (
        "62_start_time",
        "rootItem/[0]",
        "startTime",
    ): "the file keeps the setStartTime AlternateStart (0x2h) but once media "
    "linking settles ES re-derives startTime from the embedded 1h timecode - "
    "an export of the same file BEFORE settle reports the stored value",
}


#: Display paths use human idioms (`sequences[0] ('Seq A')/component 'X'`);
#: the ground-truth JSON uses container keys (`sequences[].components[]`).
#: These rewrite one vocabulary into the other so coverage can be compared.
_SHAPE_RULES = (
    (r"^sequences\[\d+\] \([^)]*\)", "sequences[]"),
    (r"/component '[^']*'", "/components[]"),
    (r"/marker '[^']*'", "/markers[]"),
    (r"/'[^']*'", "/params[]"),
    (r"/clips\[\d+\]", "/clips[]"),
    (r"/(videoTracks|audioTracks)\[\d+\]", r"/\1[]"),
    (r"/\[\d+\]", "/children[]"),
)


def shape_key(path: str, label: str) -> str:
    """Collapse a display path into the ground-truth key shape it asserts."""
    shape = path
    for pattern, replacement in _SHAPE_RULES:
        shape = re.sub(pattern, replacement, shape)
    if shape == "project":
        return label
    return shape.replace("/", ".") + "." + label


def _covered(key: str, asserted: set[str]) -> bool:
    # A key counts as covered when it, or any ancestor of it, was asserted:
    # checking `inPoint` (as ticks) covers `inPoint.ticks`/`inPoint.seconds`.
    if key in asserted:
        return True
    parts = key.split(".")
    return any(".".join(parts[:index]) in asserted for index in range(1, len(parts)))


def _flatten_keys(node: object, prefix: str = "") -> set[str]:
    """Every leaf key shape in the ground-truth JSON."""
    found: set[str] = set()
    if isinstance(node, dict):
        for key, value in node.items():
            child = f"{prefix}.{key}" if prefix else key
            if isinstance(value, (dict, list)):
                found |= _flatten_keys(value, child)
            else:
                found.add(child)
    elif isinstance(node, list):
        for entry in node:
            found |= _flatten_keys(entry, prefix + "[]")
    return found


def _check(
    problems: Problems, path: str, label: str, actual: object, expected: object
) -> None:
    problems.note(path, label)
    if _equal(actual, expected):
        return
    message = f"{path}: {label} {actual!r} != expected {expected!r}"
    problems.record(path, label, message)


def _equal(actual: object, expected: object) -> bool:
    if actual == expected:
        return True
    # ExtendScript's JSON writer rounds a float to 15 significant digits, so
    # an exact ratio (a 40/33 pixel aspect) never matches Python's repr. Only
    # the printed precision may differ - a real divergence is far larger.
    if isinstance(actual, float) and isinstance(expected, (int, float)):
        scale = max(abs(actual), abs(expected), 1.0)
        return abs(actual - float(expected)) <= 1e-12 * scale
    return False


def _normalize_media_path(value: object) -> str:
    if isinstance(value, Path):
        value = str(value)
    if not isinstance(value, str):
        return ""
    return value.replace("\\", "/")


def _ticks_or_unset(value: Time | None) -> int:
    # py maps ExtendScript's -400000 s unset sentinel to None; ground truth
    # stores what ExtendScript reported.
    return UNSET_TICKS if value is None else value.ticks


def _validate_time(
    problems: Problems, path: str, label: str, actual: Time, expected: dict[str, Any]
) -> None:
    _check(problems, path, label, actual.ticks, int(expected["ticks"]))


def _validate_item(
    problems: Problems, path: str, item: ProjectItem, expected: dict[str, Any]
) -> None:
    _check(problems, path, "name", item.name, expected.get("name"))
    _check(problems, path, "type", item.type.value, expected.get("type"))
    # nodeId is assigned per session at load (Premiere ignores stored IDs
    # on reopen), so it is not asserted against ground truth.
    if "treePath" in expected:
        _check(problems, path, "treePath", item.tree_path, expected["treePath"])
    if "isOffline" in expected:
        _check(
            problems, path, "isOffline", item.is_offline, bool(expected["isOffline"])
        )
    if "colorLabel" in expected:
        _check(problems, path, "colorLabel", item.color_label, expected["colorLabel"])
    if "startTime" in expected:
        _check(
            problems,
            path,
            "startTime",
            item.start_time.ticks,
            int(expected["startTime"]["ticks"]),
        )
    for key, actual in (
        ("isAdjustmentLayer", item.is_adjustment_layer),
        ("isMergedClip", item.is_merged_clip),
        ("isMulticamClip", item.is_multicam_clip),
    ):
        if key in expected:
            _check(problems, path, key, actual, bool(expected[key]))
    if "hasProxy" in expected:
        _check(problems, path, "hasProxy", item.has_proxy, bool(expected["hasProxy"]))
    if "proxyPath" in expected:
        # ExtendScript reports no proxy as an empty path, py as None.
        _check(
            problems,
            path,
            "proxyPath",
            _normalize_media_path(item.proxy_path) or "",
            _normalize_media_path(expected["proxyPath"]) or "",
        )
    if "inPoint" in expected:
        _check(
            problems,
            path,
            "inPoint",
            _ticks_or_unset(item.in_point),
            int(expected["inPoint"]["ticks"]),
        )
    if "outPoint" in expected:
        _check(
            problems,
            path,
            "outPoint",
            _ticks_or_unset(item.out_point),
            int(expected["outPoint"]["ticks"]),
        )
    if "mediaPath" in expected:
        _check(
            problems,
            path,
            "mediaPath",
            _normalize_media_path(item.media_path),
            _normalize_media_path(expected.get("mediaPath")),
        )
    interpretation = item.footage_interpretation
    if "interpretation" in expected and interpretation is not None:
        expected_interp = expected["interpretation"]
        _check(
            problems,
            path,
            "interpretation.alphaUsage",
            int(interpretation.alpha_usage),
            int(expected_interp["alphaUsage"]),
        )
        _check(
            problems,
            path,
            "interpretation.ignoreAlpha",
            interpretation.ignore_alpha,
            bool(expected_interp["ignoreAlpha"]),
        )
        _check(
            problems,
            path,
            "interpretation.invertAlpha",
            interpretation.invert_alpha,
            bool(expected_interp["invertAlpha"]),
        )
        _check(
            problems,
            path,
            "interpretation.fieldType",
            int(interpretation.field_type),
            int(expected_interp["fieldType"]),
        )
        if interpretation.frame_rate.ticks:
            _check(
                problems,
                path,
                "interpretation.frameRate",
                round(1.0 / interpretation.frame_rate.seconds, 6),
                round(float(expected_interp["frameRate"]), 6),
            )
        _check(
            problems,
            path,
            "interpretation.pixelAspectRatio",
            interpretation.pixel_aspect_ratio,
            float(expected_interp["pixelAspectRatio"]),
        )
    expected_children = expected.get("children", [])
    _check(problems, path, "child count", len(item.children), len(expected_children))
    for index, (child, expected_child) in enumerate(
        zip(item.children, expected_children)
    ):
        _validate_item(problems, f"{path}/[{index}]", child, expected_child)


def _values_close(actual: object, expected: object) -> bool:
    if isinstance(actual, bool) or isinstance(expected, bool):
        return actual is expected
    if isinstance(actual, (int, float)) and isinstance(expected, (int, float)):
        return abs(actual - expected) <= 1e-6 * max(1.0, abs(actual), abs(expected))
    if isinstance(actual, list) and isinstance(expected, list):
        return len(actual) == len(expected) and all(
            _values_close(a, e) for a, e in zip(actual, expected)
        )
    return actual == expected


def _validate_param_values(
    problems: Problems,
    param_path: str,
    param: ComponentParam,
    expected: dict[str, Any],
) -> None:
    _check(
        problems,
        param_path,
        "isTimeVarying",
        param.is_time_varying,
        expected.get("isTimeVarying"),
    )
    # Value semantics are asserted only for ClassIDs whose decoding is
    # corpus-verified; other encodings (popups, group headers, arb blobs)
    # stay unmapped.
    if param.class_id not in VERIFIED_VALUE_CLASS_IDS:
        return
    # Asserted below without going through _check (the comparison is
    # tolerance-based), so record it explicitly for --coverage.
    problems.note(param_path, "value")
    problems.note(param_path, "keys[]")
    if not param.is_time_varying:
        value = param.value
        expected_value = expected.get("value")
        if value is not None and not _values_close(value, expected_value):
            problems.append(
                f"{param_path}: value {value!r} != expected {expected_value!r}"
            )
        return
    expected_keys = expected.get("keys", [])
    keys = param.keys
    _check(problems, param_path, "key count", len(keys), len(expected_keys))
    for key_time, expected_key in zip(keys, expected_keys):
        expected_ticks = int(expected_key["time"]["ticks"])
        if key_time.ticks != expected_ticks:
            problems.append(
                f"{param_path}: key at {key_time.ticks} != expected {expected_ticks}"
            )
            continue
        value = param.get_value_at_key(key_time)
        expected_value = expected_key.get("value")
        if value is not None and not _values_close(value, expected_value):
            problems.append(
                f"{param_path}: value at key {key_time.ticks} "
                f"{value!r} != expected {expected_value!r}"
            )


def _validate_components(
    problems: Problems, path: str, clip: TrackItem, expected: list[dict[str, Any]]
) -> None:
    # py lists only MATERIALIZED components and their STORED params;
    # ExtendScript also reports synthesized intrinsics and skips stored
    # params it does not script (group headers, hidden UI params). Both
    # sides preserve storage order, so the contract is: py components form
    # an order-preserving subsequence of the DOM list (matched by
    # matchName - duplicates are real, e.g. two Lumetri Color instances;
    # matchName is the reliable key since audio components store no
    # display name), and within a matched component the same holds for
    # params.
    remaining = list(expected)
    for component in clip.components:
        component_path = f"{path}/component {component.match_name!r}"
        # matchName is consumed as the MATCHING key rather than compared
        # field-by-field, but it is very much asserted against.
        problems.note(component_path, "matchName")
        entry = None
        for index, candidate in enumerate(remaining):
            if candidate.get("matchName") == component.match_name:
                entry = candidate
                del remaining[: index + 1]
                break
        if entry is None:
            problems.append(f"{component_path}: not reported by ExtendScript")
            continue
        _check(
            problems,
            component_path,
            "matchName",
            component.match_name,
            entry.get("matchName"),
        )
        if component.display_name:
            # Audio components store no display name; py reports "" and only
            # matchName is comparable there.
            _check(
                problems,
                component_path,
                "displayName",
                component.display_name,
                entry.get("displayName"),
            )
        expected_params = entry.get("params", [])
        pointer = 0
        for param in component.properties:
            expected_param = None
            for k in range(pointer, len(expected_params)):
                if expected_params[k].get("displayName") == param.display_name:
                    expected_param = expected_params[k]
                    pointer = k + 1
                    break
            if expected_param is None:
                # Stored but not scriptable; nothing to compare against.
                continue
            problems.note(f"{component_path}/{param.display_name!r}", "displayName")
            param_path = f"{component_path}/{param.display_name!r}"
            _validate_param_values(problems, param_path, param, expected_param)


def _validate_track(
    problems: Problems, path: str, track: Track, expected: dict[str, Any]
) -> None:
    _check(problems, path, "id", track.id, expected.get("id"))
    _check(problems, path, "mediaType", track.media_type, expected.get("mediaType"))
    if "name" in expected:
        _check(problems, path, "name", track.name, expected["name"])
    if "isMuted" in expected:
        _check(problems, path, "isMuted", track.is_muted, bool(expected["isMuted"]))
    if "isLocked" in expected:
        _check(problems, path, "isLocked", track.is_locked, bool(expected["isLocked"]))
    expected_clips = expected.get("clips", [])
    _check(problems, path, "clip count", len(track.clips), len(expected_clips))
    for index, (clip, expected_clip) in enumerate(zip(track.clips, expected_clips)):
        clip_path = f"{path}/clips[{index}]"
        _check(problems, clip_path, "name", clip.name, expected_clip.get("name"))
        _check(problems, clip_path, "type", clip.type, expected_clip.get("type"))
        if "clipMediaType" in expected_clip:
            _check(
                problems,
                clip_path,
                "clipMediaType",
                clip.media_type,
                expected_clip["clipMediaType"],
            )
        _validate_time(problems, clip_path, "start", clip.start, expected_clip["start"])
        _validate_time(problems, clip_path, "end", clip.end, expected_clip["end"])
        _validate_time(
            problems, clip_path, "inPoint", clip.in_point, expected_clip["inPoint"]
        )
        _validate_time(
            problems, clip_path, "outPoint", clip.out_point, expected_clip["outPoint"]
        )
        _validate_time(
            problems, clip_path, "duration", clip.duration, expected_clip["duration"]
        )
        _validate_components(
            problems, clip_path, clip, expected_clip.get("components", [])
        )


def _validate_markers(
    problems: Problems,
    path: str,
    markers: list[Marker],
    expected: list[dict[str, Any]],
) -> None:
    _check(problems, path, "marker count", len(markers), len(expected))
    expected_by_guid = {m.get("guid"): m for m in expected}
    for marker in markers:
        marker_path = f"{path}/marker {marker.name!r}"
        problems.note(marker_path, "guid")  # the matching key
        entry = expected_by_guid.get(marker.guid)
        if entry is None:
            problems.append(f"{marker_path}: guid {marker.guid!r} not in ground truth")
            continue
        _check(problems, marker_path, "name", marker.name, entry.get("name"))
        _check(
            problems, marker_path, "comments", marker.comments, entry.get("comments")
        )
        _check(problems, marker_path, "type", marker.type, entry.get("type"))
        _check(
            problems,
            marker_path,
            "start",
            marker.start.ticks,
            int(entry["start"]["ticks"]),
        )
        _check(
            problems, marker_path, "end", marker.end.ticks, int(entry["end"]["ticks"])
        )
        if "webLinkURL" in entry:
            _check(
                problems,
                marker_path,
                "webLinkURL",
                marker.web_link_url,
                entry["webLinkURL"],
            )
        if "webLinkFrameTarget" in entry:
            _check(
                problems,
                marker_path,
                "webLinkFrameTarget",
                marker.web_link_frame_target,
                entry["webLinkFrameTarget"],
            )
        if "colorIndex" in entry:
            _check(
                problems,
                marker_path,
                "colorIndex",
                marker.color_index,
                int(entry["colorIndex"]),
            )


def _validate_sequence(
    problems: Problems, path: str, sequence: Sequence, expected: dict[str, Any]
) -> None:
    _check(problems, path, "name", sequence.name, expected.get("name"))
    _check(
        problems, path, "sequenceID", sequence.sequence_id, expected.get("sequenceID")
    )
    timebase = expected.get("timebase")
    if timebase is not None:
        _check(problems, path, "timebase", sequence.timebase, int(timebase))
    _validate_markers(problems, path, sequence.markers, expected.get("markers", []))
    frame_size = sequence.frame_size or (None, None)
    _check(
        problems,
        path,
        "frameSizeHorizontal",
        frame_size[0],
        expected.get("frameSizeHorizontal"),
    )
    _check(
        problems,
        path,
        "frameSizeVertical",
        frame_size[1],
        expected.get("frameSizeVertical"),
    )
    if "end" in expected:
        _check(problems, path, "end", sequence.end.ticks, int(expected["end"]))
    if "zeroPoint" in expected:
        raw = expected["zeroPoint"]
        # Older exports serialized this as an (empty) object; skip those.
        expected_zero = raw.get("ticks") if isinstance(raw, dict) else raw
        if expected_zero is not None:
            _check(
                problems,
                path,
                "zeroPoint",
                sequence.zero_point.ticks,
                int(expected_zero),
            )
    if "workInPoint" in expected:
        _check(
            problems,
            path,
            "workInPoint",
            _ticks_or_unset(sequence.in_point),
            int(expected["workInPoint"]["ticks"]),
        )
    if "workOutPoint" in expected:
        _check(
            problems,
            path,
            "workOutPoint",
            _ticks_or_unset(sequence.out_point),
            int(expected["workOutPoint"]["ticks"]),
        )
    if "videoDisplayFormat" in expected:
        _check(
            problems,
            path,
            "videoDisplayFormat",
            sequence.video_display_format,
            expected["videoDisplayFormat"],
        )
    if "audioDisplayFormat" in expected:
        _check(
            problems,
            path,
            "audioDisplayFormat",
            sequence.audio_display_format,
            expected["audioDisplayFormat"],
        )
    settings = expected.get("settings")
    if settings is not None:
        _check(
            problems,
            path,
            "settings.videoFrameWidth",
            frame_size[0],
            settings.get("videoFrameWidth"),
        )
        _check(
            problems,
            path,
            "settings.videoFrameHeight",
            frame_size[1],
            settings.get("videoFrameHeight"),
        )
        _check(
            problems,
            path,
            "settings.videoFrameRate",
            sequence.timebase,
            int(settings["videoFrameRate"]["ticks"]),
        )
        _check(
            problems,
            path,
            "settings.audioSampleRate",
            sequence.audio_frame_rate,
            int(settings["audioSampleRate"]["ticks"]),
        )
        _check(
            problems,
            path,
            "settings.audioChannelCount",
            sequence.audio_channel_count,
            settings.get("audioChannelCount"),
        )
    for label, tracks, expected_tracks in [
        ("videoTracks", sequence.video_tracks, expected.get("videoTracks", [])),
        ("audioTracks", sequence.audio_tracks, expected.get("audioTracks", [])),
    ]:
        _check(problems, path, f"{label} count", len(tracks), len(expected_tracks))
        for index, (track, expected_track) in enumerate(zip(tracks, expected_tracks)):
            _validate_track(problems, f"{path}/{label}[{index}]", track, expected_track)


def validate_file(prproj: Path, json_path: Path) -> Problems:
    """Return the mismatches between parse output and ground truth.

    The result is a `list` of messages that also carries `asserted`, the set
    of ground-truth key shapes the run actually read (see `--coverage`).
    """
    application = py_premiere.parse(prproj)
    expected = json.loads(json_path.read_text(encoding="utf-8"))
    problems = Problems()
    problems.fixture = prproj.stem

    if "name" in expected:
        _check(problems, "project", "name", application.project.name, expected["name"])
    if "documentID" in expected:
        _check(
            problems,
            "project",
            "documentID",
            application.project.document_id,
            expected["documentID"],
        )
    # activeSequence is session UI state without a dedicated stored key
    # (the open-sequence list can even hold duplicates), so it is not
    # asserted against ground truth.

    root = application.project.root_item
    expected_root = expected.get("rootItem")
    if expected_root is not None:
        if root is None:
            problems.append("rootItem: missing in parsed output")
        else:
            _validate_item(problems, "rootItem", root, expected_root)

    sequences = application.project.sequences
    expected_sequences = expected.get("sequences", [])
    _check(
        problems, "project", "sequence count", len(sequences), len(expected_sequences)
    )
    expected_by_id = {
        s.get("sequenceID"): s for s in expected_sequences if s.get("sequenceID")
    }
    for index, sequence in enumerate(sequences):
        expected_sequence = expected_by_id.get(sequence.sequence_id)
        if expected_sequence is None:
            if index < len(expected_sequences):
                expected_sequence = expected_sequences[index]
            else:
                problems.append(
                    f"sequences[{index}] ({sequence.name!r}): no ground-truth entry"
                )
                continue
        _validate_sequence(
            problems,
            f"sequences[{index}] ({sequence.name!r})",
            sequence,
            expected_sequence,
        )
    return problems


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for the `pr-validate` command."""
    parser = argparse.ArgumentParser(
        prog="pr-validate",
        description="Validate parsed output against ExtendScript ground-truth JSON.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Ground truth is exported by scripts/jsx/export_project_json.jsx (run it via
scripts/run_in_ppro.ps1). By default <file>.json next to each .prproj is used.
        """,
    )
    parser.add_argument(
        "files", type=Path, nargs="+", help="prproj file(s) to validate"
    )
    parser.add_argument(
        "--json",
        type=Path,
        default=None,
        help="Ground-truth JSON (single file mode only; default: sibling .json)",
    )
    parser.add_argument(
        "--report-json",
        type=Path,
        default=None,
        help="Write a machine-readable report (per file: status, mismatches,"
        " asserted/ignored ground-truth keys) to this path",
    )
    parser.add_argument(
        "--coverage",
        action="store_true",
        help="Also print which ground-truth keys were never asserted",
    )
    args = parser.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if args.json is not None and len(args.files) > 1:
        print("Error: --json only applies to a single file", file=sys.stderr)
        return 2

    failures = 0
    report: dict[str, Any] = {}
    for prproj in args.files:
        json_path = args.json if args.json is not None else prproj.with_suffix(".json")
        if not prproj.exists():
            print(f"Error: File not found: {prproj}", file=sys.stderr)
            report[prproj.name] = {"status": "missing"}
            failures += 1
            continue
        if not json_path.exists():
            print(f"{prproj.name}: SKIP (no ground truth at {json_path.name})")
            report[prproj.name] = {"status": "skipped", "reason": "no ground truth"}
            continue
        problems = validate_file(prproj, json_path)
        ignored: list[str] = []
        if args.coverage or args.report_json is not None:
            # Re-reading the ground truth is only worth it when the coverage
            # set is actually reported.
            ignored = sorted(
                key
                for key in _flatten_keys(
                    json.loads(json_path.read_text(encoding="utf-8"))
                )
                if not _covered(key, problems.asserted)
            )
        if problems:
            failures += 1
            print(f"{prproj.name}: {len(problems)} mismatch(es)")
            for problem in problems:
                print(f"  {problem}")
        else:
            print(f"{prproj.name}: OK")
        for divergence in problems.divergences:
            # Never silent: a documented divergence is still reported, just
            # not counted as a failure.
            print(f"  known divergence: {divergence}")
        if args.coverage:
            print(
                f"  asserted {len(problems.asserted)} key shape(s);"
                f" {len(ignored)} never asserted"
            )
            for key in ignored:
                print(f"    not asserted: {key}")
        report[prproj.name] = {
            "status": "fail" if problems else "ok",
            "mismatches": list(problems),
            "divergences": problems.divergences,
            "asserted": sorted(problems.asserted),
            "ignored": ignored,
        }
    if args.report_json is not None:
        args.report_json.write_text(json.dumps(report, indent=1), encoding="utf-8")
        print(f"report written to {args.report_json}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
