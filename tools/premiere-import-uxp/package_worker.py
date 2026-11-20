"""Build a local CCX worker package for Adobe's Unified Plugin Installer Agent."""
import argparse
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("output", type=Path)
args = parser.parse_args()
if args.output.suffix.lower() != ".ccx":
    parser.error("output must have a .ccx extension")
source = Path(__file__).resolve().parent
with ZipFile(args.output, "x", ZIP_DEFLATED) as package:
    package.write(source / "worker-manifest.json", "manifest.json")
    for name in ("capture.js", "worker.js"):
        package.write(source / name, name)
