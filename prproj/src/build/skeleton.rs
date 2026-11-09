//! Port of `models/skeleton.py`: the object graph of an empty project (9 objects).

use crate::ids::UuidSource;
use crate::xml::mutations::{build_leaf as leaf, indent_tree};
use crate::xml::{NodeId, Tree};

const PROJECT: &str = "62ad66dd-0dcd-42da-a660-6d8fbde94876";
const ROOT_ITEM: &str = "1c307a89-9318-47d7-a583-bf2553736543";
const PROJECT_SETTINGS: &str = "50c16708-a1a1-4d2f-98d5-4e283ae28353";
const SCRATCH_DISK: &str = "4c6ed82b-a81c-4df1-8bd0-750504c4b560";
const INGEST: &str = "2db8f76b-2c37-48ee-925d-9a4f7278152d";
const WORKSPACE: &str = "c4372273-e1aa-4683-98aa-a2ceadf3066c";
const DUMMY_CAPTURE: &str = "328c2aa2-47f9-4211-805b-b6a6dbd4ca29";
const DEFAULT_SEQUENCE: &str = "567bdf53-d6d9-4d61-b2f1-f4834bebea9b";

const ID_PROJECT: &str = "1";
const ID_PROJECT_SETTINGS: &str = "3";
const ID_SCRATCH: &str = "9";
const ID_INGEST: &str = "10";
const ID_WORKSPACE: &str = "11";
const ID_CAPTURE: &str = "16";
const ID_DEFAULT_SEQUENCE: &str = "17";

const ROOT_NODE_ID: &str = "1000000";
const NEXT_ID: &str = "1000001";
const ZERO_GUID: &str = "00000000-0000-0000-0000-000000000000";

const SCRATCH_KEYS: [&str; 8] = [
    "AudioPreviewLocation0",
    "VideoPreviewLocation0",
    "AutoSaveLocation0",
    "DVDEncodingLocation0",
    "TransferMediaLocation0",
    "CCLibrariesLocation0",
    "CapturedVideoLocation0",
    "CapsuleMediaLocation0",
];

const PROJECT_SETTINGS_VALUES: [(&str, &str); 13] = [
    (
        "ColorManagementSettings",
        "{\"enableLogColorManagement\":2,\"graphicsWhiteLuminance\":203,\"lutInterpolationMethod\":1}",
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
    ("EditingModeID", ZERO_GUID),
    ("PreviewFileFormatID", ZERO_GUID),
    ("UsePreviewCache", "false"),
];

const DEFAULT_SEQUENCE_VALUES: [(&str, &str); 7] = [
    ("TotalVideoTracks", "1"),
    ("DefaultAudioStandardMonoTracks", "0"),
    ("DefaultAudioStandardStereoTracks", "1"),
    ("DefaultAudioStandard51Tracks", "0"),
    ("DefaultAudioSubmixMonoTracks", "0"),
    ("DefaultAudioSubmixStereoTracks", "0"),
    ("DefaultAudioSubmix51Tracks", "0"),
];

const PROJECT_SETTINGS_REFS: [(&str, &str); 6] = [
    ("VideoSettings", "12"),
    ("AudioSettings", "13"),
    ("VideoCompileSettings", "14"),
    ("AudioCompileSettings", "15"),
    ("CaptureSettings", ID_CAPTURE),
    ("DefaultSequenceSettings", ID_DEFAULT_SEQUENCE),
];

/// Python `build_empty_project` default for `workspace`.
pub const DEFAULT_WORKSPACE: &str = "Learning";
/// Python `build_empty_project` default for `build_version`.
pub const DEFAULT_BUILD_VERSION: &str = "26.3.0x93 - 20/07/2026 10:07:43";

/// Python `_object` (identifier first, then ClassID and Version).
fn object(
    tree: &mut Tree,
    root: NodeId,
    tag: &str,
    class_id: &str,
    version: &str,
    object_id: Option<&str>,
    object_uid: Option<&str>,
) -> NodeId {
    let mut attrs: Vec<(&str, &str)> = Vec::new();
    if let Some(id) = object_id {
        attrs.push(("ObjectID", id));
    }
    if let Some(uid) = object_uid {
        attrs.push(("ObjectUID", uid));
    }
    attrs.push(("ClassID", class_id));
    attrs.push(("Version", version));
    tree.sub_element(root, tag, &attrs)
}

/// Arguments of [`build_empty_project`]. A `None` (or empty, like Python's `or`) identifier is
/// minted from the UUID source in the order document id, project GUID, view-state id.
#[derive(Default, Clone, Debug)]
pub struct EmptyProjectOptions<'a> {
    pub document_id: Option<&'a str>,
    pub project_guid: Option<&'a str>,
    pub view_state_id: Option<&'a str>,
    /// `None` = [`DEFAULT_WORKSPACE`].
    pub workspace: Option<&'a str>,
    /// `None` = [`DEFAULT_BUILD_VERSION`].
    pub build_version: Option<&'a str>,
}

fn fresh(given: Option<&str>, ids: &mut dyn UuidSource) -> String {
    match given {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => ids.uuid4().to_string(),
    }
}

/// Python `build_empty_project`: the `PremiereData` root of a new, empty project, created
/// detached in `tree`.
pub fn build_empty_project(tree: &mut Tree, opts: &EmptyProjectOptions, ids: &mut dyn UuidSource) -> NodeId {
    let document_id = fresh(opts.document_id, ids);
    let project_guid = fresh(opts.project_guid, ids);
    let view_state_id = fresh(opts.view_state_id, ids);
    let workspace = opts.workspace.unwrap_or(DEFAULT_WORKSPACE);
    let build_version = opts.build_version.unwrap_or(DEFAULT_BUILD_VERSION);

    let root = tree.new_element_with("PremiereData", &[("Version", "3")]);
    tree.sub_element(root, "Project", &[("ObjectRef", ID_PROJECT)]);

    let project = object(tree, root, "Project", PROJECT, "45", Some(ID_PROJECT), None);
    let node = tree.sub_element(project, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    leaf(tree, properties, "MZ.Project.WorkspaceName", workspace);
    leaf(tree, properties, "MZ.BuildVersion.Created", build_version);
    leaf(tree, properties, "MZ.BuildVersion.Modified", build_version);
    leaf(tree, properties, "MZ.Project.ApplicationID", "Pro");
    leaf(tree, properties, "MZ.Project.GUID", &project_guid);
    leaf(tree, properties, "TL.PJSnappingState", "1");
    tree.sub_element(project, "RootProjectItem", &[("ObjectURef", &document_id)]);
    tree.sub_element(project, "ProjectSettings", &[("ObjectRef", ID_PROJECT_SETTINGS)]);
    tree.sub_element(project, "ScratchDiskSettings", &[("ObjectRef", ID_SCRATCH)]);
    tree.sub_element(project, "IngestSettings", &[("ObjectRef", ID_INGEST)]);
    tree.sub_element(project, "ProjectWorkspace", &[("ObjectRef", ID_WORKSPACE)]);
    leaf(tree, project, "NextID", NEXT_ID);

    let root_item = object(tree, root, "RootProjectItem", ROOT_ITEM, "1", None, Some(&document_id));
    let item = tree.sub_element(root_item, "ProjectItem", &[("Version", "1")]);
    let item_node = tree.sub_element(item, "Node", &[("Version", "1")]);
    let item_properties = tree.sub_element(item_node, "Properties", &[("Version", "1")]);
    leaf(tree, item_properties, "ProjectViewState.ID", &view_state_id);
    leaf(tree, item_node, "ID", ROOT_NODE_ID);
    leaf(tree, item, "Name", "Root Bin");
    tree.sub_element(root_item, "ProjectItemContainer", &[("Version", "1")]);

    let settings = object(tree, root, "ProjectSettings", PROJECT_SETTINGS, "21", Some(ID_PROJECT_SETTINGS), None);
    for (tag, target) in PROJECT_SETTINGS_REFS {
        tree.sub_element(settings, tag, &[("ObjectRef", target)]);
    }
    for (tag, value) in PROJECT_SETTINGS_VALUES {
        leaf(tree, settings, tag, value);
    }

    let scratch = object(tree, root, "ScratchDiskSettings", SCRATCH_DISK, "4", Some(ID_SCRATCH), None);
    for key in SCRATCH_KEYS {
        leaf(tree, scratch, key, "SameAsProject");
    }

    let ingest = object(tree, root, "IngestSettings", INGEST, "2", Some(ID_INGEST), None);
    leaf(tree, ingest, "Action", "copy");
    leaf(tree, ingest, "Enabled", "false");

    object(tree, root, "WorkspaceSettings", WORKSPACE, "1", Some(ID_WORKSPACE), None);
    object(tree, root, "DummyCaptureSettings", DUMMY_CAPTURE, "1", Some(ID_CAPTURE), None);

    let default_sequence = object(
        tree,
        root,
        "DefaultSequenceSettings",
        DEFAULT_SEQUENCE,
        "2",
        Some(ID_DEFAULT_SEQUENCE),
        None,
    );
    for (tag, value) in DEFAULT_SEQUENCE_VALUES {
        leaf(tree, default_sequence, tag, value);
    }

    indent_tree(tree, root, 0);
    root
}
