"""Marker color palette (decoded 2026-07-20, Premiere 26.3).

The blob stores no index: `setColorByIndex` writes the palette color as a
packed `0xAABBGGRR` uint32 into a `mCuePointList` entry whose `mValue` is
the JSON `{"color": N}`. Index 5 is white - present in the palette but
missing from UXP's `Constants.MarkerColor` enum. Uncolored markers get a
type-based default index (UXP `getColorIndex` on virgin markers).
"""

from __future__ import annotations

#: index -> packed 0xAABBGGRR color, from `make_marker_colors_fixture.js`
#: sweeps over all eight indices.
MARKER_COLOR_PACKED = {
    0: 4281828977,  # green
    1: 4281740498,  # red
    2: 4289825711,  # magenta
    3: 4280578025,  # orange
    4: 4281049552,  # yellow
    5: 4294967295,  # white
    6: 4294741314,  # blue
    7: 4292277273,  # cyan
}

PACKED_TO_INDEX = {packed: index for index, packed in MARKER_COLOR_PACKED.items()}

#: colorIndex reported for markers with no stored color, by marker type.
DEFAULT_INDEX_BY_TYPE = {
    "Comment": 0,
    "Chapter": 1,
    "Segmentation": 2,
    "WebLink": 3,
}
