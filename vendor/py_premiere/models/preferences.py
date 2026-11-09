"""Read-only reader for Premiere's on-disk preferences (ported pattern
from py-aep's AE prefs reader).

Premiere persists preferences as an UNCOMPRESSED PremiereData XML file at
`~/Documents/Adobe/Premiere Pro/<version>/Profile-<name>/Adobe Premiere
Pro Prefs`: one `Preferences` object holding a flat `Properties` bag. The
`BE.Prefs.*` keys back the values the .prproj format only references
(label palette/defaults, still-image import defaults). py never writes
this file.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from pathlib import Path

from .time import TICKS_PER_SECOND

#: Size of Premiere's label palette (`BE.Prefs.Label*.0` .. `.15`).
LABEL_COUNT = 16

#: Media-kind keys of `BE.Prefs.LabelDefaults.*`.
LABEL_KINDS = (
    "Bin",
    "Sequence",
    "Video",
    "Audio",
    "AV",
    "Still",
    "DynamicLink",
    "Captions",
)


def _default_prefs_path() -> Path | None:
    base = Path.home() / "Documents" / "Adobe" / "Premiere Pro"
    if not base.is_dir():
        return None
    candidates = []
    for version_dir in base.iterdir():
        for profile in version_dir.glob("Profile-*"):
            prefs = profile / "Adobe Premiere Pro Prefs"
            if prefs.is_file():
                candidates.append(prefs)
    if not candidates:
        return None
    # Most recently used profile.
    return max(candidates, key=lambda p: p.stat().st_mtime)


class Preferences:
    """A parsed `Adobe Premiere Pro Prefs` file. Read-only."""

    def __init__(self, path: str | Path) -> None:
        self._path = Path(path)
        try:
            root = ET.fromstring(self._path.read_bytes())
        except ET.ParseError as error:
            raise ValueError(f"not a Premiere preferences file: {error}") from None
        bag = None
        for element in root:
            if element.tag == "Preferences" and element.get("ObjectID"):
                bag = element.find("Properties")
                break
        if bag is None:
            raise ValueError("not a Premiere preferences file")
        self._bag = bag

    @classmethod
    def load_default(cls) -> Preferences | None:
        """The machine's own prefs, or `None` when Premiere never ran here."""
        path = _default_prefs_path()
        return cls(path) if path is not None else None

    @property
    def path(self) -> Path:
        """The preference file path. Read-only."""
        return self._path

    def get(self, key: str) -> str | None:
        """The raw text of a preference key, or `None`."""
        return self._bag.findtext(key)

    def get_int(self, key: str) -> int | None:
        text = self.get(key)
        return int(text) if text and text.lstrip("-").isdigit() else None

    def get_bool(self, key: str) -> bool | None:
        text = self.get(key)
        if text is None:
            return None
        return text == "true"

    @property
    def label_names(self) -> list[str]:
        """The 16 label names (`BE.Prefs.LabelNames.N`). Read-only."""
        return [self.get(f"BE.Prefs.LabelNames.{i}") or "" for i in range(LABEL_COUNT)]

    @property
    def label_colors(self) -> list[int]:
        """The 16 packed label colors (`BE.Prefs.LabelColors.N`). Read-only."""
        return [
            self.get_int(f"BE.Prefs.LabelColors.{i}") or 0 for i in range(LABEL_COUNT)
        ]

    def label_default(self, kind: str) -> int | None:
        """The default label index for a media kind (see `LABEL_KINDS`).

        `None` when the key is unset or holds an index outside the palette,
        so callers can safely use the result to index `label_colors`.
        """
        if kind not in LABEL_KINDS:
            raise ValueError(f"unknown label kind {kind!r}")
        index = self.get_int(f"BE.Prefs.LabelDefaults.{kind}")
        if index is None or not 0 <= index < LABEL_COUNT:
            return None
        return index

    @property
    def still_frame_rate(self) -> int | None:
        """Ticks per frame for imported stills. Read-only."""
        return self.get_int("BE.Prefs.StillImages.DefaultFramerate")

    @property
    def still_default_out_ticks(self) -> int | None:
        """The default clip length of an imported still, in ticks. Read-only.

        Premiere stores whole seconds and floors to whole frames (5 s at
        29.97 fps -> 149 frames -> 1262874412800 ticks, the corpus value).
        """
        seconds = self.get_int("BE.Prefs.StillImages.DurationInSeconds")
        frame = self.still_frame_rate
        if seconds is None or not frame:
            return None
        return (seconds * TICKS_PER_SECOND // frame) * frame

    def __repr__(self) -> str:
        return f"Preferences(path={str(self._path)!r})"
