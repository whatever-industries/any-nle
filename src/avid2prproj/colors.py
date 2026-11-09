"""Avid clip and marker colors -> Premiere label and marker colors.

Premiere side: the 16 label colors are `BE.Prefs.LabelColors.N` in the user's Preferences, packed
0xBBGGRR; the values below are Premiere 26's factory defaults (read from the User's 26.0 prefs,
2026-10-15; names in `BE.Prefs.LabelNames.N`). Marker colors are the 8 packed 0xAABBGGRR values
py_premiere decoded (data/marker_colors.py).

Avid side: clip color = 16-bit RGB (`_COLOR_R/G/B`, KB BCOL-019; 32-swatch palette in KB
snapshots/2026-10-13/phases33-34-review/files/palette.json). Marker color = one of 16 names
(`_ATN_CRM_COLOR_EXTENDED`, Phase 20); its stored RGB is only the legacy 8-color value.

Defaults are User decisions (2026-10-15); the import's color mapper overrides them.
"""
from __future__ import annotations

import math

LABEL_NAMES = ("Violet", "Iris", "Caribbean", "Lavender", "Cerulean", "Forest", "Rose", "Mango",
               "Purple", "Blue", "Teal", "Magenta", "Tan", "Green", "Brown", "Yellow")
LABEL_PACKED = (11405886, 6769408, 480554, 8851829, 5985541, 19005, 3474060, 277129,
                11993185, 10104082, 4541953, 5770628, 4545135, 2579725, 408413, 1664623)

MARKER_NAMES = ("Green", "Red", "Magenta", "Orange", "Yellow", "White", "Blue", "Cyan")

#: Avid marker color name -> Premiere marker color index (User decisions 2026-10-15).
AVID_MARKER_DEFAULTS = {
    "Red": 1, "Green": 0, "Blue": 6, "Cyan": 7, "Magenta": 2, "Yellow": 4, "White": 5, "Orange": 3,
    "Pink": 2, "Forest": 0, "Denim": 6, "Gold": 4,
    "Violet": 2, "Purple": 6,
    "Black": 5, "Grey": 5,  # Premiere has no dark marker color
}


def unpack_bgr(packed: int) -> tuple[int, int, int]:
    return packed & 255, (packed >> 8) & 255, (packed >> 16) & 255


def _lab(rgb):
    def lin(c):
        c /= 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = map(lin, rgb)
    x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047
    y = 0.2126 * r + 0.7152 * g + 0.0722 * b
    z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883
    f = lambda t: t ** (1 / 3) if t > 0.008856 else 7.787 * t + 16 / 116  # noqa: E731
    return 116 * f(y) - 16, 500 * (f(x) - f(y)), 200 * (f(y) - f(z))


def _distance(p, q) -> float:
    """Hue first, lightness weakly: Avid's swatches are light, Premiere's labels dark, so plain
    Lab distance sends most pastels to Tan."""
    (l1, a1, b1), (l2, a2, b2) = _lab(p), _lab(q)
    c1, c2 = math.hypot(a1, b1), math.hypot(a2, b2)
    h1, h2 = math.degrees(math.atan2(b1, a1)) % 360, math.degrees(math.atan2(b2, a2)) % 360
    dh = min(abs(h1 - h2), 360 - abs(h1 - h2))
    d_hue = 2 * math.sqrt(c1 * c2) * math.sin(math.radians(dh) / 2)
    return math.sqrt((0.25 * (l1 - l2)) ** 2 + 0.5 * (c1 - c2) ** 2 + d_hue ** 2)


def rgb16_hex(rgb16) -> str:
    return "#" + "".join(f"{round(v / 257):02X}" for v in rgb16)


def nearest_label(rgb16, overrides: dict[str, int] | None = None) -> int:
    """Premiere label index for an Avid clip color."""
    key = rgb16_hex(rgb16)
    if overrides and key in overrides:
        return overrides[key]
    rgb = tuple(round(v / 257) for v in rgb16)
    return min(range(16), key=lambda i: _distance(rgb, unpack_bgr(LABEL_PACKED[i])))


def marker_color(avid_name: str, overrides: dict[str, int] | None = None) -> int | None:
    """Premiere marker color index for an Avid marker color name; None = unknown name."""
    if overrides and avid_name in overrides:
        return overrides[avid_name]
    return AVID_MARKER_DEFAULTS.get(avid_name)
