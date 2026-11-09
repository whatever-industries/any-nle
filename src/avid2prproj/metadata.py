"""Avid bin columns -> Premiere clip metadata.

Storage, from the User's Premiere 26.5.2 read-backs (2026-10-14, fixture
"metadata fields.prproj"): Premiere's column list marks each column Class 0 or 1.
- Class 1 columns (Label, Comment, Client) live in the project item's property bag,
  `ProjectItem/Node/Properties/Column.PropertyText.<Name>`.
- Class 0 logging columns live on the master clip's ClipLoggingInfo, as elements
  ShotTake ("Shot"), TapeName, Description, Scene, LogNote (the order Premiere wrote).
  Property-bag leaves with those column IDs are kept on save but never displayed.
Sound Roll, Camera Roll, Aux/Sound Timecode exist as Premiere columns but are not
editable in the Metadata panel and their storage is unknown, so those Avid values go to
Log Note for now.

Mapping (proposal, User to confirm): Camroll -> Tape Name, Scene -> Scene, Take -> Shot,
Comments -> Comment, Source File -> Description, everything else -> Log Note lines.
"""
from __future__ import annotations

import xml.etree.ElementTree as ET

from .model import AvidInfo

LOGGING = (  # (Avid column, ClipLoggingInfo element)
    ("Camroll", "TapeName"),
    ("Scene", "Scene"),
    ("Take", "ShotTake"),
)
BAG = (  # (Avid column, property-bag key)
    ("Comments", "Column.PropertyText.Comment"),
)
LOGGING_ORDER = ("ShotTake", "TapeName", "Description", "Scene", "LogNote")
# Avid-internal or already-represented columns that should not be copied as text.
SKIP = {"UNC Path", "Video"}


def premiere_fields(avid: AvidInfo) -> tuple[list[tuple[str, str]], list[tuple[str, str]]]:
    """(logging-info elements, property-bag leaves) for one clip, in a stable order."""
    logging, bag = [], []
    used = set(SKIP)
    for avid_name, tag in LOGGING:
        value = avid.column(avid_name)
        if value:
            logging.append((tag, value))
            used.add(avid_name)
    for avid_name, key in BAG:
        value = avid.column(avid_name)
        if value:
            bag.append((key, value))
            used.add(avid_name)
    if avid.source_file:
        logging.append(("Description", avid.source_file))
    notes = [f"{k}: {v}" for k, v in avid.columns if k not in used and v]
    if avid.source_path:
        notes.append(f"Source Path: {avid.source_path}")
    if avid.start_timecode:
        notes.append(f"Avid Start: {avid.start_timecode}")
    for aux in avid.aux_timecodes:
        # Aux TC number = track index - 2 is a hypothesis (one fixture, TC3/TC4).
        notes.append(f"Aux TC{aux.track_index - 2}: {aux.timecode}")
    if notes:
        # One line: Premiere showed an empty Log Note for a newline-separated value (2026-10-14).
        logging.append(("LogNote", " | ".join(notes)))
    logging.sort(key=lambda kv: LOGGING_ORDER.index(kv[0]))
    return logging, bag


def _set_leaves(parent: ET.Element, pairs: list[tuple[str, str]], fallback_indent: str) -> None:
    """Replace or append leaf elements, keeping the parent's indentation."""
    if len(parent):
        child_indent, closing = parent.text or fallback_indent, parent[-1].tail
    else:
        child_indent, closing = fallback_indent, fallback_indent[:-1]
        parent.text = child_indent
    for tag, value in pairs:
        existing = parent.find(tag)
        if existing is None:
            if len(parent):
                parent[-1].tail = child_indent
            existing = ET.SubElement(parent, tag)
            existing.tail = closing
        existing.text = value


def apply(project, item, avid: AvidInfo | None) -> None:
    if avid is None:
        return
    logging, bag = premiere_fields(avid)
    properties = item._element.find("ProjectItem/Node/Properties")
    if properties is None:
        raise ValueError("project item has no property bag")
    _set_leaves(properties, bag, "\n\t\t\t\t\t")
    document = project._document
    master = document.resolve(item._element.find("MasterClip"))
    logging_ref = master.find("LoggingInfo")
    if logging_ref is None:
        raise ValueError(f"master clip of {item.name!r} has no LoggingInfo")
    _set_leaves(document.resolve(logging_ref), logging, "\n\t\t")
