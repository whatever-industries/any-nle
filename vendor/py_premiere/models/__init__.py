"""Typed model classes mirroring Premiere Pro's ExtendScript object model."""

from __future__ import annotations

from .application import Application
from .caption import Caption, CaptionTrack
from .color import Color
from .component import Component, ComponentParam
from .marker import Marker
from .named_list import NamedList
from .preferences import Preferences
from .project import Project
from .project_item import ProjectItem
from .sequence import Sequence, SequenceSettings
from .time import TICKS_PER_SECOND, Time
from .track import Track
from .track_item import TrackItem
from .transition import Transition

__all__ = [
    "TICKS_PER_SECOND",
    "Application",
    "Caption",
    "CaptionTrack",
    "Color",
    "Component",
    "ComponentParam",
    "Marker",
    "NamedList",
    "Preferences",
    "Project",
    "ProjectItem",
    "Sequence",
    "SequenceSettings",
    "Time",
    "Track",
    "TrackItem",
    "Transition",
]
