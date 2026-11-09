"""The `Color` value type."""

from __future__ import annotations

from typing import NamedTuple


class Color(NamedTuple):
    """An RGBA color with 8-bit (`0`-`255`) channels."""

    red: int
    green: int
    blue: int
    alpha: int = 255
