"""Standalone Avid-to-Premiere converter (spike). Writer side only; Avid reading comes later."""
import sys
from pathlib import Path

_VENDOR = Path(__file__).resolve().parents[2] / "vendor"
if str(_VENDOR) not in sys.path:
    sys.path.insert(0, str(_VENDOR))
