#!/usr/bin/env python3
"""Regenerate desktop icon resources from assets/icon_v1.jpg.

Requires Pillow and GNU windres when the artwork changes. Release builds use
the checked-in resources and require neither tool.
"""

import argparse
from pathlib import Path
import subprocess
import tempfile

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]


def generate(windres):
    output = ROOT / "packaging" / "icons"
    output.mkdir(parents=True, exist_ok=True)
    with Image.open(ROOT / "assets" / "icon_v1.jpg") as original:
        if original.width != original.height or original.width < 1024:
            raise ValueError("Desktop icon artwork must be square and at least 1024 pixels")
        # Start with pixels only. Do not copy JPEG metadata into the icons.
        artwork = Image.frombytes("RGB", original.size, original.convert("RGB").tobytes())
    artwork.resize((256, 256), Image.Resampling.LANCZOS).save(output / "any-nle.png")
    artwork.save(output / "any-nle.icns")
    artwork.save(output / "any-nle.ico", sizes=[(n, n) for n in (16, 24, 32, 48, 64, 128, 256)])
    # An MSVC .res is architecture-neutral. The linker wraps it for x64 or ARM64.
    with tempfile.TemporaryDirectory(prefix="anynle-icons-") as temp:
        script = Path(temp) / "icon.rc"
        script.write_text('101 ICON "any-nle.ico"\n')
        (Path(temp) / "any-nle.ico").write_bytes((output / "any-nle.ico").read_bytes())
        subprocess.run(
            [windres, "-J", "rc", "-O", "res", "-i", script.name, "-o", str(output / "any-nle.res")],
            cwd=temp, check=True,
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--windres", default="windres", help="GNU windres executable")
    generate(parser.parse_args().windres)
