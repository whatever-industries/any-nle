use crate::build::{leaf, top};
use crate::model::{Project, TrackItemId};
use crate::{Error, Result};

enum Control {
    Menu(i32, i32),
    Color(u64),
    Group(i32),
    Slider(i32, i32, i32),
}

pub(super) fn apply(project: &mut Project, placed: TrackItemId, [r, g, b]: [u8; 3]) -> Result<()> {
    let item = project.track_item(placed).element;
    let reference = project
        .doc
        .tree
        .find1(item, "ClipTrackItem/ComponentOwner/Components")
        .ok_or_else(|| Error::Value("Ultra Key placement has no component chain".into()))?;
    let chain = project.doc.resolve(reference)?;
    let tree = &mut project.doc.tree;
    let effect = top(
        tree,
        "VideoFilterComponent",
        "d10da199-beea-4dd1-b941-ed3a78766d50",
        "9",
        None,
    );
    let inner = tree.sub_element(effect, "Component", &[("Version", "7")]);
    let params = tree.sub_element(inner, "Params", &[("Version", "1")]);
    tree.set_text(inner, Some("\n\t\t\t"));
    tree.set_tail(inner, Some("\n\t\t"));
    tree.set_text(params, Some("\n\t\t\t\t"));
    tree.set_tail(params, Some("\n\t\t\t"));
    leaf(tree, inner, "ID", "3", "\n\t\t\t");
    leaf(tree, inner, "DisplayName", "Ultra Key", "\n\t\t");
    leaf(tree, effect, "VideoFilterType", "2", "\n\t\t");
    leaf(tree, effect, "MatchName", "AE.ADBE Ultra Key", "\n\t");
    project.register(effect)?;
    let color = (255u64 << 56) | (u64::from(r) << 40) | (u64::from(g) << 24) | (u64::from(b) << 8);
    use Control::*;
    // Custom matte generation keeps the retained low-saturation green screen
    // transparent while protecting the surrounding picture. Source RGB is exact.
    let controls = [
        ("Output", Menu(0, 2)),
        ("Setting", Menu(3, 3)),
        ("Key Color", Color(color)),
        ("Matte Generation", Group(11)),
        ("Transparency", Slider(60, 0, 100)),
        ("Highlight", Slider(0, 0, 100)),
        ("Shadow", Slider(20, 0, 100)),
        ("Tolerance", Slider(50, 0, 100)),
        ("Pedestal", Slider(25, 0, 100)),
        ("", Group(12)),
        ("Matte Cleanup", Group(11)),
        ("Choke", Slider(0, 0, 100)),
        ("Soften", Slider(0, 0, 100)),
        ("Contrast", Slider(25, 0, 100)),
        ("Mid Point", Slider(50, 0, 100)),
        ("", Group(12)),
        ("Spill Suppression", Group(11)),
        ("Desaturate", Slider(25, 0, 100)),
        ("Range", Slider(50, 0, 100)),
        ("Spill", Slider(50, 0, 100)),
        ("Luma", Slider(50, 0, 100)),
        ("", Group(12)),
        ("Color Correction", Group(11)),
        ("Saturation", Slider(100, 0, 200)),
        ("Hue", Slider(0, -180, 180)),
        ("Luminance", Slider(100, 0, 200)),
        ("", Group(12)),
    ];
    let count = controls.len();
    for (index, (name, control)) in controls.into_iter().enumerate() {
        let (class, value, bounds, discrete, group) = match control {
            Menu(value, max) => (
                "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8",
                value.to_string(),
                Some((0, max)),
                true,
                None,
            ),
            Color(value) => (
                "0fde4e9f-f895-4ba3-b0fe-9a6feafda583",
                value.to_string(),
                None,
                false,
                None,
            ),
            Group(kind) => (
                "cc12343e-f113-4d3b-ae05-b287db77d461",
                "false".into(),
                None,
                false,
                Some(kind),
            ),
            Slider(value, min, max) => (
                "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542",
                format!("{value}."),
                Some((min, max)),
                false,
                None,
            ),
        };
        let tree = &mut project.doc.tree;
        let node = top(tree, "VideoComponentParam", class, "10", None);
        if discrete {
            leaf(tree, node, "DiscontinuousInterpolate", "true", "\n\t\t");
        }
        if let Some(kind) = group {
            leaf(
                tree,
                node,
                "ParameterControlType",
                &kind.to_string(),
                "\n\t\t",
            );
        }
        if !name.is_empty() {
            leaf(tree, node, "Name", name, "\n\t\t");
        }
        leaf(
            tree,
            node,
            "ParameterID",
            &(index + 1).to_string(),
            "\n\t\t",
        );
        leaf(
            tree,
            node,
            "StartKeyframe",
            &format!("-91445760000000000,{value},0,0,0,0,0,0"),
            "\n\t\t",
        );
        if let Some((min, max)) = bounds {
            leaf(tree, node, "LowerBound", &min.to_string(), "\n\t\t");
            leaf(tree, node, "UpperBound", &max.to_string(), "\n\t");
        } else if group.is_some() {
            leaf(tree, node, "UpperBound", "false", "\n\t");
        } else {
            let last = *tree.children(node).last().expect("StartKeyframe was added");
            tree.set_tail(last, Some("\n\t"));
        }
        let id = project.register(node)?;
        let entry = project.doc.tree.sub_element(
            params,
            "Param",
            &[("Index", &index.to_string()), ("ObjectRef", &id)],
        );
        project.doc.tree.set_tail(
            entry,
            Some(if index + 1 == count {
                "\n\t\t\t"
            } else {
                "\n\t\t\t\t"
            }),
        );
    }
    super::motion::append_component(project, chain, effect)
}
