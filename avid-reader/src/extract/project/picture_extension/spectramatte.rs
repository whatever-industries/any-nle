use super::*;

pub(in crate::extract::project) const ID: &str = "B654B779-8100-3B98-BFBD-864F89C96166";
const RED: &str = "26d2ceccef0e73378ee3d91acdc9b200";
const GREEN: &str = "c28aa9154c78b13db047e0eb2ed20185";
const BLUE: &str = "1bf4c60e7d1ad333959ca3c94a62ecdf";
const PROGRESS: &str = "6a91a08b0b7ce441b2dac817048cbf5d";
const STOCK: &str = "dca5f78a7b361949afc75ca00a5bd558";

fn parameters<'a>(ctx: &Ctx<'a>, effect: &Node, count: usize) -> Option<HashMap<String, &'a Node>> {
    let list = ctx
        .deref(effect.get("param_list"))
        .filter(|list| list.class == *b"PRLS")?;
    let Value::RefList(items) = list.get("items")? else {
        return None;
    };
    if items.len() != count {
        return None;
    }
    let mut values = HashMap::with_capacity(count);
    for item in items {
        let item = ctx.deref(Some(item))?;
        let Value::Uuid(id) = item.get("uuid")? else {
            return None;
        };
        if item.class != *b"PRIT"
            || item.get("enable") != Some(&Value::Bool(true))
            || values.insert(file_order_hex(id), item).is_some()
        {
            return None;
        }
    }
    Some(values)
}

fn overlay_envelope(ctx: &Ctx<'_>, effect: &Node, level: i64) -> bool {
    let attrs = ctx.attrs(effect);
    effect.class == *b"TKFX"
        && int(effect, "media_kind_id") == Some(KIND_PICTURE)
        && ["precomputed", "trackman"]
            .iter()
            .all(|name| matches!(effect.get(name), None | Some(Value::Null | Value::Ref(0))))
        && int(effect, "info_is_reversed") == Some(0)
        && int(effect, "info_smooth") == Some(0)
        && ["_GRID_ENABLED", "_IGNORE_TRACKING"].iter().all(|name| {
            attrs
                .iter()
                .filter(|(key, _)| key == name)
                .map(|(_, v)| v.as_int())
                .eq([Some(0)])
        })
        && ctx
            .deref(effect.get("keyframes"))
            .is_some_and(|keys| super::super::source_capture::neutral_effect_keys(keys, level))
}

fn qualified_warp(ctx: &Ctx<'_>, warp: &Node) -> Option<()> {
    if !effect_envelope(ctx, warp, "3DWarp") || !overlay_envelope(ctx, warp, 1 << 30) {
        return None;
    }
    let mut items = parameters(ctx, warp, 14)?;
    let mut take = |prefix, kind| {
        let key = format!("{prefix}568d7e84d511935a50f857c10000");
        let item = items.remove(&key)?;
        if has_curve(item) || int(item, "value_type") != Some(kind) {
            return None;
        }
        if kind == 1 {
            int(item, "value").map(|n| n as f64)
        } else {
            float(item, "value")
        }
    };
    for (prefix, value) in crate::transform::COMPACT_WARP_DEFAULTS {
        let (kind, value) = match prefix {
            "2b81" => (1, 1.0),
            "8e82" => (1, value),
            _ => (2, value),
        };
        if take(prefix, kind) != Some(value) {
            return None;
        }
    }
    if take("a981", 2) != Some(1.0) || take("b281", 2) != Some(1.0) {
        return None;
    }
    let x = take("aa81", 2)?;
    let scale = take("b381", 2)?;
    ((-500.0..=500.0).contains(&x)
        && (50.0..=200.0).contains(&scale)
        && take("b481", 2) == Some(scale)
        && items.is_empty())
    .then_some(())
}

fn key_stage(ctx: &Ctx<'_>, keyer: &Node, length: i64, rate: Rate) -> Option<PictureStage> {
    let attrs = ctx.attrs(keyer);
    let one = |name, value| {
        attrs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
            .eq([Some(value)])
    };
    if !one("_EFFECT_PLUGIN_TYPE", "AVX2")
        || !one("_EFFECT_PLUGIN_NAME", "SpectraMatte")
        || !one("_EFFECT_PLUGIN_CLASS", "Key")
        || !one(
            "_EFFECT_PLUGIN_PRODUCT_ID",
            "409578e0-2ee2-4f08-ac91-19df6b1cc737",
        )
        || !overlay_envelope(ctx, keyer, 1 << 29)
    {
        return None;
    }
    let items = parameters(ctx, keyer, 5)?;
    let color = |id| {
        let item = items.get(id)?;
        let value = float(item, "value")?;
        (int(item, "value_type") == Some(2)
            && !has_curve(item)
            && (0.0..=255.0).contains(&value)
            && value.fract() == 0.0)
            .then_some(value as u8)
    };
    let rgb = [color(RED)?, color(GREEN)?, color(BLUE)?];
    let stock = items.get(STOCK)?;
    if int(stock, "value_type") != Some(1) || int(stock, "value") != Some(1) || has_curve(stock) {
        return None;
    }
    let progress = items.get(PROGRESS)?;
    let curve = ctx.deref(progress.get("control_track"))?;
    if float(progress, "value") != Some(0.0)
        || int(progress, "value_type") != Some(2)
        || !curve_header(progress, curve, length, rate, 3)
        || int(curve, "media_kind_id") != Some(10)
    {
        return None;
    }
    let Value::List(points) = curve.get("control_points")? else {
        return None;
    };
    let [first, last] = points.as_slice() else {
        return None;
    };
    let (progress_end, end_value) = point(last)?;
    if point(first) != Some((0, 0.0))
        || end_value != 1.0
        || !(length.saturating_sub(1)..=length.saturating_add(2)).contains(&progress_end)
        || points.iter().any(|p| {
            !matches!(p, Value::Node(node)
            if matches!(node.get("pp"), Some(Value::List(props)) if props.is_empty()))
        })
    {
        return None;
    }
    Some(PictureStage::SpectraMatte { rgb, progress_end })
}

/// Match the edited stack, with both lower-track inputs represented by full-span filler.
pub(in crate::extract::project) fn spectramatte_warp<'a>(
    ctx: &Ctx<'a>,
    warp: &'a Node,
    length: i64,
) -> Option<(&'a Node, Rate, PictureStage)> {
    let rate = Rate::new(24_000, 1001)?;
    let picture = |node: &Node| {
        int(node, "media_kind_id") == Some(KIND_PICTURE)
            && float(node, "edit_rate").and_then(Rate::from_avid) == Some(rate)
    };
    if length <= 1 || !picture(warp) {
        return None;
    }
    let keyer =
        single_picture_segment(ctx, ordered_picture_input(ctx, warp, length, rate)?, length)?;
    if !picture(keyer) || keyer.get("effect_id").and_then(Value::as_str) != Some(ID) {
        return None;
    }
    let leaf = single_picture_segment(
        ctx,
        ordered_picture_input(ctx, keyer, length, rate)?,
        length,
    )?;
    if leaf.class != *b"SCLP" || !picture(leaf) {
        return None;
    }
    qualified_warp(ctx, warp)?;
    let stage = key_stage(ctx, keyer, length, rate)?;
    Some((leaf, rate, stage))
}
