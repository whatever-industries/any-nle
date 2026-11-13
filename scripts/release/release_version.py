#!/usr/bin/env python3
"""Read the release version and check that CI runs on the reviewed export."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]
VERSION_PATTERN = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?")


def release_version(requested=None, *, root=None):
    if root is None:
        root = ROOT
    with (root / "crates/gui/Cargo.toml").open("rb") as file:
        version = tomllib.load(file)["package"]["version"]
    if not isinstance(version, str) or not VERSION_PATTERN.fullmatch(version):
        raise ValueError("invalid release version in GUI Cargo manifest")
    if requested is not None and requested != version:
        raise ValueError(f"requested release version must match GUI Cargo version {version}")
    return version


def verify_notes(version, *, root=None):
    if root is None:
        root = ROOT
    text = (root / "RELEASE_NOTES.md").read_text(encoding="utf-8")
    if not text.startswith(f"# Any NLE v{version}\n"):
        raise ValueError("release notes heading does not match GUI Cargo version")
    tags = re.findall(r"/releases/download/v([^/\s)]+)/", text)
    if not tags or any(tag != version for tag in tags):
        raise ValueError("release notes download links do not match GUI Cargo version")


def gate_binding(source, receipt):
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("invalid reviewed export SHA")
    if not re.fullmatch(r"[0-9a-f]{64}", receipt):
        raise ValueError("invalid private gate receipt digest")
    return f"private-receipt-sha256 {receipt}\nsource {source}\n"


def validate_workflow(version, source, receipt, event, *, root=None):
    if root is None:
        root = ROOT
    release_version(version, root=root)
    verify_notes(version, root=root)
    binding = gate_binding(source, receipt)
    head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    if head != source or event != source:
        raise ValueError("qualified source differs from run checkout")
    paths = subprocess.check_output(["git", "-C", str(root), "ls-tree", "-rz", "--name-only", head])
    if any(path.startswith(b"prproj/tests/data/convert_model/golden/") for path in paths.split(b"\0")):
        raise ValueError("private fixture path in publication tree")
    return binding


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validate-workflow", action="store_true")
    args = parser.parse_args()
    if args.validate_workflow:
        validate_workflow(os.environ["VERSION"], os.environ["QUALIFIED_SOURCE"],
                          os.environ["PRIVATE_GATE"], os.environ["GITHUB_SHA"])
    else:
        print(release_version())


if __name__ == "__main__":
    main()
