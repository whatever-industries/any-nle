//! Premiere 27's clip-owned LUT operation for the qualified SDR source LUT.
use crate::build::{leaf, top};
use crate::hash::sha256;
use crate::ids::Uuid;
use crate::model::{Project, TrackItemId};
use crate::payload::{binary_hash, set_payload};
use crate::xml::NodeId;
use crate::{Error, Result};

use super::lumetri::itx3_lut;
use super::motion::append_component;
use super::prepared::PreparedSourceLut;

const STATIC_TIME: &str = "-91445760000000000";
const REC709_SCENE_ID: &str = "3491c4cd-1e21-2af0-9af0-a4e119768173";

fn compound(project: &mut Project, kind: &str, id: &str, child: NodeId) -> Result<NodeId> {
    let tree = &mut project.doc.tree;
    let chain = top(
        tree,
        "VideoComponentChain",
        "0970e08a-f58f-4108-b29a-1a717b8e12e2",
        "3",
        None,
    );
    tree.sub_element(chain, "ComponentChain", &[("Version", "3")]);
    let chain_id = project.register(chain)?;
    append_component(project, chain, child)?;

    let tree = &mut project.doc.tree;
    let node = top(
        tree,
        "CompoundComponent",
        "250b3605-8b7a-4687-bf33-f54e312a510e",
        "1",
        None,
    );
    let owner = tree.sub_element(node, "ComponentOwner", &[("Version", "1")]);
    tree.sub_element(owner, "Components", &[("ObjectRef", &chain_id)]);
    let component = tree.sub_element(node, "Component", &[("Version", "7")]);
    leaf(tree, component, "Unique", "true", "\n\t\t\t");
    leaf(tree, component, "ID", id, "\n\t\t");
    leaf(tree, node, "CompoundComponentType", kind, "\n\t");
    project.register(node)?;
    Ok(node)
}

fn utf16_z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

enum Control<'a> {
    Menu(&'a str, &'a str),
    Percent,
    Bytes(&'a [u8]),
    StoredBytes(&'a str),
}

fn add_control(
    project: &mut Project,
    params: NodeId,
    index: usize,
    name: &str,
    control: Control<'_>,
) -> Result<()> {
    let tree = &mut project.doc.tree;
    let (tag, class, version) = match control {
        Control::Menu(..) => (
            "VideoComponentParam",
            "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8",
            "10",
        ),
        Control::Percent => (
            "VideoComponentParam",
            "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542",
            "10",
        ),
        Control::Bytes(..) | Control::StoredBytes(..) => (
            "ArbVideoComponentParam",
            "313e54d4-6903-49ad-b0bf-8262cdd10f4e",
            "3",
        ),
    };
    let node = top(tree, tag, class, version, None);
    if matches!(control, Control::Menu(..)) {
        leaf(tree, node, "DiscontinuousInterpolate", "true", "\n\t\t");
    }
    if index == 0 {
        leaf(tree, node, "UpperUIBound", "61", "\n\t\t");
    }
    leaf(tree, node, "Name", name, "\n\t\t");
    leaf(
        tree,
        node,
        "ParameterID",
        &(index + 1).to_string(),
        "\n\t\t",
    );
    match control {
        Control::Bytes(_) | Control::StoredBytes(_) => {
            leaf(tree, node, "StartKeyframePosition", STATIC_TIME, "\n\t\t");
            let value = tree.sub_element(node, "StartKeyframeValue", &[("Encoding", "base64")]);
            tree.set_tail(value, Some("\n\t"));
            match control {
                Control::Bytes(bytes) => set_payload(&mut project.doc, value, bytes),
                Control::StoredBytes(hash) => tree.set(value, "BinaryHash", hash),
                _ => unreachable!(),
            }
        }
        scalar => {
            let (value, upper) = match scalar {
                Control::Menu(value, upper) => (value, upper),
                _ => ("100.", "200"),
            };
            leaf(
                tree,
                node,
                "StartKeyframe",
                &format!("{STATIC_TIME},{value},0,0,0,0,0,0"),
                "\n\t\t",
            );
            leaf(tree, node, "LowerBound", "0", "\n\t\t");
            leaf(tree, node, "UpperBound", upper, "\n\t");
        }
    }
    let id = project.register(node)?;
    let tree = &mut project.doc.tree;
    let reference = tree.sub_element(
        params,
        "Param",
        &[("Index", &index.to_string()), ("ObjectRef", &id)],
    );
    tree.set_tail(reference, Some("\n\t\t\t\t"));
    Ok(())
}

pub(super) fn apply_lut(
    project: &mut Project,
    placed: TrackItemId,
    lut: &PreparedSourceLut,
) -> Result<()> {
    let name = lut
        .filename
        .strip_suffix(".cube")
        .ok_or_else(|| Error::Value("Color Mode LUT has no .cube filename".into()))?;
    ClipLut::new(name, itx3_lut(&lut.points, lut.edge)).apply(project, placed)
}

/// Each shared table belongs to one project. Later occurrences reference its first embedded copy.
pub(super) struct ClipLut {
    name: String,
    cube: Vec<u8>,
    lut_id: Vec<u8>,
    stored_hash: Option<String>,
}

impl ClipLut {
    pub(super) fn new(name: &str, cube: Vec<u8>) -> Self {
        // Equal names with different tables must remain separate LUT choices.
        let mut identity = name.as_bytes().to_vec();
        identity.push(0);
        identity.extend_from_slice(&cube);
        let digest = sha256(&identity);
        let mut uuid = [0; 16];
        uuid.copy_from_slice(&digest[..16]);
        let lut_id = utf16_z(&Uuid::from_bytes_v4(uuid).to_string());
        Self {
            name: name.into(),
            cube,
            lut_id,
            stored_hash: None,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project, placed: TrackItemId) -> Result<()> {
        let item = project.track_item(placed).element;
        let chain_ref = project
            .doc
            .tree
            .find1(item, "ClipTrackItem/ComponentOwner/Components")
            .ok_or_else(|| {
                Error::Value("Color Mode LUT placement has no component chain".into())
            })?;
        let chain = project.doc.resolve(chain_ref)?;
        let name = &self.name;
        let tree = &mut project.doc.tree;
        let effect = top(
            tree,
            "VideoFilterComponent",
            "d10da199-beea-4dd1-b941-ed3a78766d50",
            "9",
            None,
        );
        let component = tree.sub_element(effect, "Component", &[("Version", "7")]);
        let params = tree.sub_element(component, "Params", &[("Version", "1")]);
        leaf(tree, component, "ID", "2", "\n\t\t\t");
        leaf(tree, component, "DisplayName", "LUT Transform", "\n\t\t\t");
        leaf(tree, component, "InstanceName", name, "\n\t\t");
        let private = tree.sub_element(
            effect,
            "PremiereFilterPrivateData",
            &[("Encoding", "base64")],
        );
        leaf(tree, effect, "VideoFilterType", "2", "\n\t\t");
        leaf(tree, effect, "MatchName", "AE.ADBE LUT Transform", "\n\t");
        // Static SDR LUT private record from Premiere 27's clip LUT save.
        let mut private_bytes = [0; 72];
        private_bytes[64] = 0x3a;
        set_payload(&mut project.doc, private, &private_bytes);
        project.register(effect)?;

        let color_space_id = utf16_z(REC709_SCENE_ID);
        let mut display_name = name.as_bytes().to_vec();
        display_name.push(0);
        use Control::*;
        let table = self
            .stored_hash
            .as_deref()
            .map_or(Bytes(&self.cube), StoredBytes);
        let controls = [
            ("LUT", Menu("3", "61")),
            ("LUT ID", Bytes(&self.lut_id)),
            ("LUT Blob", table),
            ("Color Space", Menu("28", "34")),
            ("Color Space ID", Bytes(&color_space_id)),
            ("Data Range", Menu("1", "1")),
            ("Intensity", Percent),
            ("HDR Highlights", Percent),
            ("LUT Display Name", Bytes(&display_name)),
        ];
        for (index, (name, control)) in controls.into_iter().enumerate() {
            add_control(project, params, index, name, control)?;
        }
        let clip_operations = compound(project, "65541", "1", effect)?;
        let color_mode = compound(project, "65537", "3", clip_operations)?;
        append_component(project, chain, color_mode)?;
        if self.stored_hash.is_none() {
            self.stored_hash = Some(binary_hash(&self.cube));
        }
        Ok(())
    }
}
