"""Parse the project-panel item tree."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from ..enums import ProjectItemType
from ..models import Project, ProjectItem
from ..models.project_item import clip_core, item_container
from .marker import parse_markers

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..xml import PremiereDocument

_ITEM_TYPES = {
    "RootProjectItem": ProjectItemType.ROOT,
    "BinProjectItem": ProjectItemType.BIN,
    # A search bin wraps a BinProjectItem; ExtendScript reports it as a bin.
    "SmartBinProjectItem": ProjectItemType.BIN,
    "ClipProjectItem": ProjectItemType.CLIP,
}


def _media_path(document: PremiereDocument, master: ET.Element) -> Path | None:
    # MasterClip -> Clips/Clip -> Video/AudioClip -> Clip/Source ->
    # *MediaSource -> MediaSource/Media -> Media/FilePath.
    for clip_ref in master.findall("Clips/Clip"):
        core = clip_core(document.resolve(clip_ref))
        source_ref = None if core is None else core.find("Source")
        if source_ref is None:
            continue
        source = document.resolve(source_ref)
        media_ref = source.find("MediaSource/Media")
        if media_ref is None:
            continue
        media = document.resolve(media_ref)
        file_path = media.findtext("FilePath")
        # Internal generators (Black Video, Adjustment Layer, ...) store a
        # numeric token; ExtendScript reports no media path for them.
        if file_path and not file_path.isdigit():
            return Path(file_path)
    return None


def _parse_item(
    document: PremiereDocument, project: Project, element: ET.Element
) -> ProjectItem:
    item_type = _ITEM_TYPES.get(element.tag, ProjectItemType.CLIP)
    item = ProjectItem(element, project, item_type)
    container = item_container(element)
    for child_ref in [] if container is None else container.findall("Items/Item"):
        child = _parse_item(document, project, document.resolve(child_ref))
        child._parent = item
        item._children.append(child)
    master_ref = element.find("MasterClip")
    if master_ref is not None:
        master = document.resolve(master_ref)
        item._master_element = master
        item._media_path = _media_path(document, master)
        item._clip_elements = [
            document.resolve(ref) for ref in master.findall("Clips/Clip")
        ]
        if item._clip_elements:
            core = clip_core(item._clip_elements[0])
            if core is not None:
                item._markers = parse_markers(document, core)
            source_ref = None if core is None else core.find("Source")
            if source_ref is not None:
                source = document.resolve(source_ref)
                duration = source.findtext("OriginalDuration")
                if duration:
                    item._default_out_ticks = int(duration)
                sequence_ref = source.find("SequenceSource/Sequence")
                if sequence_ref is not None:
                    item._sequence_uid = sequence_ref.get("ObjectURef")
    return item


def _assign_node_ids(root_item: ProjectItem) -> None:
    # Premiere numbers items from 1000000 in depth-first order at load;
    # stored IDs win, unstored items take the next counter value (validated
    # against ExtendScript nodeId across the corpus).
    counter = 1000000

    def visit(item: ProjectItem) -> None:
        nonlocal counter
        stored = item._element.findtext("ProjectItem/Node/ID")
        if stored:
            item._node_id_int = int(stored)
            counter = max(counter, int(stored) + 1)
        else:
            item._node_id_int = counter
            counter += 1
        for child in item.children:
            visit(child)

    visit(root_item)


def parse_project(document: PremiereDocument, path: Path) -> Project:
    project = Project(document, path)
    stub = document.root.find("Project")
    if stub is not None:
        project_element = document.resolve(stub)
        root_ref = project_element.find("RootProjectItem")
        if root_ref is not None:
            project._root_item = _parse_item(
                document, project, document.resolve(root_ref)
            )
            _assign_node_ids(project._root_item)
    return project
