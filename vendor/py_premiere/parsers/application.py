"""Parse a whole document into the `Application` model."""

from __future__ import annotations

from typing import TYPE_CHECKING

from ..models import Application
from .project import parse_project
from .sequence import parse_sequence

if TYPE_CHECKING:
    from pathlib import Path

    from ..xml import PremiereDocument


_OPEN_LIST_PREFIX = "MZ.PrefixKey.OpenSequenceGuidList."


def _active_sequence_id(document: PremiereDocument) -> str | None:
    # No dedicated key exists; the frontmost sequence is the highest-numbered
    # entry of the open-sequence GUID list in Project's property bag.
    stub = document.root.find("Project")
    if stub is None:
        return None
    project_element = document.resolve(stub)
    best_index = -1
    best: str | None = None
    for element in project_element.findall("Node/Properties/*"):
        if element.tag.startswith(_OPEN_LIST_PREFIX) and element.text:
            try:
                index = int(element.tag[len(_OPEN_LIST_PREFIX) :])
            except ValueError:
                continue
            if index > best_index:
                best_index = index
                best = element.text
    return best


def parse_application(document: PremiereDocument, path: Path) -> Application:
    project = parse_project(document, path)
    for element in document.root:
        if element.tag == "Sequence":
            project._sequences.append(parse_sequence(document, project, element))
    active_id = _active_sequence_id(document)
    if active_id is not None:
        for sequence in project._sequences:
            if sequence.sequence_id == active_id:
                project._active_sequence = sequence
                break
    return Application(project)
