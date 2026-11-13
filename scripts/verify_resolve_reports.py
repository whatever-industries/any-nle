#!/usr/bin/env python3
"""Replay hash-pinned private Resolve timeline and conform report fixtures."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def require_sha256(value):
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
        raise ValueError("expected a lowercase SHA-256 digest")


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate manifest key: {key}")
        result[key] = value
    return result


def case_args(case, pins, ffmpeg, ffprobe):
    mode = case["mode"]
    allowed = {"name", "mode", "left", "left_id", "right", "right_id", "report_sha256"}
    if mode == "conform":
        allowed |= {"all_frames", "renders"}
    elif mode != "diff":
        raise ValueError(f"unsupported report mode: {mode}")
    if case.keys() - allowed:
        raise ValueError(f"{case['name']}: unknown case fields")
    require_sha256(case["report_sha256"])
    sides = []
    for side in ("left", "right"):
        selector = case[f"{side}_id"]
        if not isinstance(selector, str) or not selector.strip():
            raise ValueError(f"{case['name']}: empty timeline selector")
        sides.extend((pins[case[side]], selector))
    flags = []
    if "all_frames" in case:
        if not isinstance(case["all_frames"], bool):
            raise ValueError(f"{case['name']}: all_frames must be boolean")
        if case["all_frames"]:
            flags.append("--all-frames")
    if "renders" in case:
        renders = case["renders"]
        if not isinstance(renders, list) or len(renders) != 2 or not (ffmpeg and ffprobe):
            raise ValueError(f"{case['name']}: supply two pinned renders and both backend paths")
        flags.extend(("--renders", *(pins[path] for path in renders), ffmpeg, ffprobe))
    return [f"--{mode}", *sides], flags


def verify(manifest_path, probe, output, ffmpeg, ffprobe):
    manifest_bytes = manifest_path.read_bytes()
    manifest = json.loads(manifest_bytes, object_pairs_hook=unique_object)
    if (not isinstance(manifest, dict) or set(manifest) != {"schema", "files", "cases"}
            or manifest["schema"] != "anynle.resolve-report-fixtures/1"
            or not isinstance(manifest["files"], dict)
            or not isinstance(manifest["cases"], list) or not manifest["cases"]):
        raise ValueError("expected a nonempty anynle.resolve-report-fixtures/1 manifest")
    pins = {name: manifest_path.parent / name for name in manifest["files"]}

    def check_pins():
        for name, expected in manifest["files"].items():
            require_sha256(expected)
            if sha256(pins[name]) != expected:
                raise ValueError(f"fixture hash mismatch: {name}")

    commands = {}
    for case in manifest["cases"]:
        name = case["name"]
        if (not re.fullmatch(r"[a-z0-9][a-z0-9_-]*", name)
                or name == "verified" or name in commands):
            raise ValueError(f"invalid or duplicate case name: {name}")
        commands[name] = case_args(case, pins, ffmpeg, ffprobe)
    check_pins()
    output.mkdir()
    results = {}
    for case in manifest["cases"]:
        name = case["name"]
        report = output / f"{name}.json"
        args, flags = commands[name]
        with (output / f"{name}.log").open("xb") as log:
            run = subprocess.run([probe, *args, report, *flags], stdout=log,
                                 stderr=subprocess.STDOUT, timeout=180)
        if run.returncode:
            raise ValueError(f"{name}: probe failed; see {log.name}")
        actual = sha256(report)
        if actual != case["report_sha256"]:
            raise ValueError(f"{name}: report hash mismatch ({actual}); see {report.name}")
        results[name] = actual
    check_pins()
    if manifest_path.read_bytes() != manifest_bytes:
        raise ValueError("fixture manifest changed during replay")
    receipt = {"manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(), "reports": results}
    with (output / "verified.json").open("x") as file:
        json.dump(receipt, file, indent=2)
        file.write("\n")
    print(f"Verified {len(results)} Resolve reports with {len(pins)} pinned inputs")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("manifest", "probe", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--ffmpeg", type=Path)
    parser.add_argument("--ffprobe", type=Path)
    args = parser.parse_args()
    try:
        verify(args.manifest.absolute(), args.probe.resolve(), args.output.absolute(),
               args.ffmpeg and args.ffmpeg.resolve(), args.ffprobe and args.ffprobe.resolve())
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        print(f"Resolve report verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
