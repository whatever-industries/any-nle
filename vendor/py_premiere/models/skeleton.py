"""Build an empty project from scratch, without a bundled skeleton file.

Premiere writes 40 objects and 44 KB for an empty project, but only 9 of them
carry state it cannot work out again: opening the 9-object skeleton this
module emits and saving makes Premiere restore the whole compile-settings
tree and the 26.7 KB of project-panel view state from its own defaults.
Measured, not assumed - see `scripts/dev/strip_empty_project.py`, which
produced the stripped variants Premiere was asked to judge.

Emitting it rather than replaying a captured file also fixes two things the
captured one carried: every project got the SAME `documentID`, and every
project embedded the capturing machine's home directory in
`project.settings.lastknowngoodprojectpath`.
"""

from __future__ import annotations

import uuid
import xml.etree.ElementTree as ET

from ..xml.mutations import build_leaf as _leaf
from ..xml.mutations import indent_tree

#: ClassIDs of the objects an empty project needs, in the order Premiere
#: writes them.
_PROJECT = "62ad66dd-0dcd-42da-a660-6d8fbde94876"
_ROOT_ITEM = "1c307a89-9318-47d7-a583-bf2553736543"
_PROJECT_SETTINGS = "50c16708-a1a1-4d2f-98d5-4e283ae28353"
_SCRATCH_DISK = "4c6ed82b-a81c-4df1-8bd0-750504c4b560"
_INGEST = "2db8f76b-2c37-48ee-925d-9a4f7278152d"
_WORKSPACE = "c4372273-e1aa-4683-98aa-a2ceadf3066c"
_DUMMY_CAPTURE = "328c2aa2-47f9-4211-805b-b6a6dbd4ca29"
_DEFAULT_SEQUENCE = "567bdf53-d6d9-4d61-b2f1-f4834bebea9b"

#: ObjectIDs Premiere uses for these objects. They only have to be internally
#: consistent, but keeping Premiere's numbering makes a diff against one of
#: its own projects readable.
_ID_PROJECT = "1"
_ID_PROJECT_SETTINGS = "3"
_ID_SCRATCH = "9"
_ID_INGEST = "10"
_ID_WORKSPACE = "11"
_ID_CAPTURE = "16"
_ID_DEFAULT_SEQUENCE = "17"

#: The root bin's node ID; `NextID` is the next one the project will hand out.
_ROOT_NODE_ID = "1000000"
_NEXT_ID = "1000001"

_ZERO_GUID = "00000000-0000-0000-0000-000000000000"

#: Scratch locations all default to "beside the project file".
_SCRATCH_KEYS = (
    "AudioPreviewLocation0",
    "VideoPreviewLocation0",
    "AutoSaveLocation0",
    "DVDEncodingLocation0",
    "TransferMediaLocation0",
    "CCLibrariesLocation0",
    "CapturedVideoLocation0",
    "CapsuleMediaLocation0",
)

#: `ProjectSettings` values, in Premiere's stored order.
_PROJECT_SETTINGS_VALUES = (
    (
        "ColorManagementSettings",
        '{"enableLogColorManagement":2,"graphicsWhiteLuminance":203,'
        '"lutInterpolationMethod":1}',
    ),
    ("VideoTimeDisplay", "102"),
    ("AudioTimeDisplay", "200"),
    ("ColorAwareEffectsEnabled", "0"),
    ("VideoTimeDisplayInitial", "102"),
    ("ActionSafeWidth", "10"),
    ("ActionSafeHeight", "10"),
    ("TitleSafeWidth", "20"),
    ("TitleSafeHeight", "20"),
    ("ShouldScaleMedia", "false"),
    ("EditingModeID", _ZERO_GUID),
    ("PreviewFileFormatID", _ZERO_GUID),
    ("UsePreviewCache", "false"),
)

#: What a new sequence gets by default: one video track and one stereo audio
#: track.
_DEFAULT_SEQUENCE_VALUES = (
    ("TotalVideoTracks", "1"),
    ("DefaultAudioStandardMonoTracks", "0"),
    ("DefaultAudioStandardStereoTracks", "1"),
    ("DefaultAudioStandard51Tracks", "0"),
    ("DefaultAudioSubmixMonoTracks", "0"),
    ("DefaultAudioSubmixStereoTracks", "0"),
    ("DefaultAudioSubmix51Tracks", "0"),
)

#: Objects `ProjectSettings` points at. `CaptureSettings` resolves to the
#: dummy object; the video/audio ones are part of the tree Premiere rebuilds,
#: so they are referenced but not emitted - it restores both on first save.
_PROJECT_SETTINGS_REFS = (
    ("VideoSettings", "12"),
    ("AudioSettings", "13"),
    ("VideoCompileSettings", "14"),
    ("AudioCompileSettings", "15"),
    ("CaptureSettings", _ID_CAPTURE),
    ("DefaultSequenceSettings", _ID_DEFAULT_SEQUENCE),
)


def _object(
    root: ET.Element,
    tag: str,
    class_id: str,
    version: str,
    object_id: str | None = None,
    object_uid: str | None = None,
) -> ET.Element:
    # Premiere writes the identifier first, then ClassID and Version.
    attributes = {}
    if object_id is not None:
        attributes["ObjectID"] = object_id
    if object_uid is not None:
        attributes["ObjectUID"] = object_uid
    attributes["ClassID"] = class_id
    attributes["Version"] = version
    return ET.SubElement(root, tag, attributes)


def build_empty_project(
    document_id: str | None = None,
    project_guid: str | None = None,
    view_state_id: str | None = None,
    workspace: str = "Learning",
    build_version: str = "26.3.0x93 - 20/07/2026 10:07:43",
) -> ET.Element:
    """Return the `PremiereData` root of a new, empty project.

    The three identifiers are minted fresh unless given: the root bin's UID
    (which is what `Project.document_id` reports), the project GUID, and the
    panel view-state ID. Every other GUID in an empty project is a fixed
    class identifier.
    """
    document_id = document_id or str(uuid.uuid4())
    project_guid = project_guid or str(uuid.uuid4())
    view_state_id = view_state_id or str(uuid.uuid4())

    root = ET.Element("PremiereData", {"Version": "3"})
    ET.SubElement(root, "Project", {"ObjectRef": _ID_PROJECT})

    project = _object(root, "Project", _PROJECT, "45", object_id=_ID_PROJECT)
    node = ET.SubElement(project, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "MZ.Project.WorkspaceName", workspace)
    _leaf(properties, "MZ.BuildVersion.Created", build_version)
    _leaf(properties, "MZ.BuildVersion.Modified", build_version)
    _leaf(properties, "MZ.Project.ApplicationID", "Pro")
    _leaf(properties, "MZ.Project.GUID", project_guid)
    _leaf(properties, "TL.PJSnappingState", "1")
    ET.SubElement(project, "RootProjectItem", {"ObjectURef": document_id})
    ET.SubElement(project, "ProjectSettings", {"ObjectRef": _ID_PROJECT_SETTINGS})
    ET.SubElement(project, "ScratchDiskSettings", {"ObjectRef": _ID_SCRATCH})
    ET.SubElement(project, "IngestSettings", {"ObjectRef": _ID_INGEST})
    ET.SubElement(project, "ProjectWorkspace", {"ObjectRef": _ID_WORKSPACE})
    _leaf(project, "NextID", _NEXT_ID)

    root_item = _object(
        root, "RootProjectItem", _ROOT_ITEM, "1", object_uid=document_id
    )
    item = ET.SubElement(root_item, "ProjectItem", {"Version": "1"})
    item_node = ET.SubElement(item, "Node", {"Version": "1"})
    item_properties = ET.SubElement(item_node, "Properties", {"Version": "1"})
    _leaf(item_properties, "ProjectViewState.ID", view_state_id)
    _leaf(item_node, "ID", _ROOT_NODE_ID)
    _leaf(item, "Name", "Root Bin")
    ET.SubElement(root_item, "ProjectItemContainer", {"Version": "1"})

    settings = _object(
        root, "ProjectSettings", _PROJECT_SETTINGS, "21", object_id=_ID_PROJECT_SETTINGS
    )
    for tag, target in _PROJECT_SETTINGS_REFS:
        ET.SubElement(settings, tag, {"ObjectRef": target})
    for tag, value in _PROJECT_SETTINGS_VALUES:
        _leaf(settings, tag, value)

    scratch = _object(
        root, "ScratchDiskSettings", _SCRATCH_DISK, "4", object_id=_ID_SCRATCH
    )
    for key in _SCRATCH_KEYS:
        _leaf(scratch, key, "SameAsProject")

    ingest = _object(root, "IngestSettings", _INGEST, "2", object_id=_ID_INGEST)
    _leaf(ingest, "Action", "copy")
    _leaf(ingest, "Enabled", "false")

    _object(root, "WorkspaceSettings", _WORKSPACE, "1", object_id=_ID_WORKSPACE)
    _object(root, "DummyCaptureSettings", _DUMMY_CAPTURE, "1", object_id=_ID_CAPTURE)

    default_sequence = _object(
        root,
        "DefaultSequenceSettings",
        _DEFAULT_SEQUENCE,
        "2",
        object_id=_ID_DEFAULT_SEQUENCE,
    )
    for tag, value in _DEFAULT_SEQUENCE_VALUES:
        _leaf(default_sequence, tag, value)

    indent_tree(root)
    return root
