use crate::build::{leaf, top};
use crate::model::{Project, TrackItemId};
use crate::{Error, Result};

pub(super) fn apply_range_scale(project: &mut Project, item: TrackItemId, video_to_full: bool) -> Result<()> {
    let item = project.track_item(item).element;
    let reference = project.doc.tree.find1(item, "ClipTrackItem/ComponentOwner/Components")
        .ok_or_else(|| Error::Value("range scaling has no picture component chain".into()))?;
    let chain = project.doc.resolve(reference)?;
    let tree = &mut project.doc.tree;
    let effect = top(tree, "VideoFilterComponent", "d10da199-beea-4dd1-b941-ed3a78766d50", "9", None);
    let inner = tree.sub_element(effect, "Component", &[("Version", "7")]);
    let params = tree.sub_element(inner, "Params", &[("Version", "1")]);
    tree.set_text(inner, Some("\n\t\t\t"));
    tree.set_tail(inner, Some("\n\t\t"));
    tree.set_text(params, Some("\n\t\t\t\t"));
    tree.set_tail(params, Some("\n\t\t\t"));
    leaf(tree, inner, "ID", "3", "\n\t\t\t");
    leaf(tree, inner, "DisplayName", "ASC CDL", "\n\t\t");
    leaf(tree, effect, "VideoFilterType", "2", "\n\t\t");
    leaf(tree, effect, "MatchName", "AE.ADBE AEASCCDL", "\n\t");
    project.register(effect)?;

    // Premiere supplies RGB normalized to video levels. The inverse maps
    // full-range codes 0 and 1023 back to the nominal 10-bit video endpoints.
    let (slope, offset) = if video_to_full {
        (1023.0 / (940.0 - 64.0), -64.0 / (940.0 - 64.0))
    } else {
        ((940.0 - 64.0) / 1023.0, 64.0 / 1023.0)
    };
    let controls = [
        ("Red Slope", slope, "0"), ("Red Offset", offset, "-10"), ("Red Power", 1.0, "0"),
        ("Green Slope", slope, "0"), ("Green Offset", offset, "-10"), ("Green Power", 1.0, "0"),
        ("Blue Slope", slope, "0"), ("Blue Offset", offset, "-10"), ("Blue Power", 1.0, "0"),
        ("Saturation", 1.0, "0"),
    ];
    for (index, (name, value, minimum)) in controls.into_iter().enumerate() {
        let tree = &mut project.doc.tree;
        let node = top(tree, "VideoComponentParam", "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542", "10", None);
        leaf(tree, node, "Name", name, "\n\t\t");
        leaf(tree, node, "ParameterID", &(index + 1).to_string(), "\n\t\t");
        leaf(tree, node, "StartKeyframe",
            &format!("-91445760000000000,{value:.20},0,0,0,0,0,0"), "\n\t\t");
        leaf(tree, node, "LowerBound", minimum, "\n\t\t");
        leaf(tree, node, "UpperBound", "10", "\n\t");
        let id = project.register(node)?;
        let entry = project.doc.tree.sub_element(params, "Param",
            &[("Index", &index.to_string()), ("ObjectRef", &id)]);
        project.doc.tree.set_tail(entry, Some(if index == 9 { "\n\t\t\t" } else { "\n\t\t\t\t" }));
    }
    super::motion::append_component(project, chain, effect)
}
