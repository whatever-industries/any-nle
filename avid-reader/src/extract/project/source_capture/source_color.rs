use super::*;
use crate::color::file_order_hex;

pub(super) const MANAGED_INPUT_TRANSFORM_CONFLICT: &str =
    "the picture clip's saved color transform differs from the Color Adapter";
const CLIP_RGB: [&str; 3] = ["_COLOR_R", "_COLOR_G", "_COLOR_B"];
const VERIFIED_GRAY_CLIP_RGB: [u16; 3] = [48_896; 3];

pub(super) fn capture_adapter(
    ctx: &Ctx<'_>,
    effect: &Node,
    length: i64,
    ambiguous: &HashSet<MobId>,
    budget: &SharedBudget,
    hd_24_still_canvas: bool,
) -> Option<SourceColorAdapter> {
    if effect.class != *b"TKFX"
        || int(effect, "length") != Some(length)
        || float(effect, "edit_rate") != Some(24.0)
    {
        return None;
    }
    let transform = saved_payload(ctx, effect)?;
    let Value::List(tracks) = effect.get("tracks")? else { return None };
    let [Value::Node(track)] = tracks.as_slice() else { return None };
    let program = matte_track(ctx, effect, 1)?;
    let neutral = neutral_wrapper(ctx, effect);
    let fitted_still = (hd_24_still_canvas && neutral && spatial_track(ctx, track))
        .then(|| centered_still_input(ctx, program, length)).flatten();
    let input = fitted_still.or_else(|| sole_picture_input(ctx, program))?;
    if int(input, "length") != Some(length) || float(input, "edit_rate") != Some(24.0)
    {
        return None;
    }
    let master_id = mob_id(input)?;
    let master_track = int(input, "track_id").filter(|track| *track > 0)?;
    let source_in = int(input, "start_time")?;
    let wrapper_attrs = borrowed_attrs(ctx, effect);
    let wrapper_clip_color = match CLIP_RGB.map(|key| {
        unique_attribute(wrapper_attrs, key).and_then(Value::as_int)
            .and_then(|value| u16::try_from(value).ok())
    }) {
        [Some(r), Some(g), Some(b)] => Some([r, g, b]),
        _ => None,
    };
    let subclip = subclip_picture_file(ctx, master_id, master_track, source_in, length, ambiguous);
    let subclip_motion = if subclip.is_none() {
        let allow_other_speeds = neutral && master_track == 1
            && SourceColorRange::from_saved_xml(transform) == Some(SourceColorRange::VideoToFull);
        subclip_motion(ctx, master_id, master_track, source_in, length, ambiguous, Rate::new(24, 1)?, allow_other_speeds)
            .map(|(speed_effect, mut motion)| {
                let other_speed = motion.sampled_input_frames.is_some_and(|samples| samples.ratio != (2, 1));
                if !neutral_managed_input(ctx, input, Some(transform))
                    || (other_speed && (!motion_clip_is_plain(ctx, input, 0, length - 1)
                        || validate_direct_motion_positions(ctx, input, speed_effect, &motion).is_none()))
                {
                    motion.sampled_picture_file = None;
                }
                motion
            })
            .filter(|_| charge(budget, size_of::<SourceSubclipMotion>()))
            .map(Box::new)
    } else {
        None
    };
    let picture = subclip.as_ref().map(|source| (source.file_id, source.file))
        .or_else(|| direct_picture_file(ctx, master_id, master_track, ambiguous));
    let centered_still_fit = fitted_still.is_some() && subclip.is_none()
        && picture.is_some_and(|(_, file)| still_raster(ctx, file));
    let managed_cache = master_track == 1
        && ctx.deref(effect.get("left_bob")).is_some_and(|cache| cache.class == *b"DIDP");
    let managed_range = managed_cache
        && SourceColorRange::from_saved_xml(transform) == Some(SourceColorRange::VideoToFull);
    let input_attrs = borrowed_attrs(ctx, input);
    let conflicting_input_transform = managed_range
        && input_attrs.iter().any(|(name, _)| name == "_COLOR_INPUT_TRANSFORMATION")
        && unique_attribute(input_attrs, "_COLOR_INPUT_TRANSFORMATION").and_then(Value::as_str)
            != Some(transform);
    let managed_picture_span = picture.and_then(|(id, file)| {
        if !managed_range || !managed_wrapper_attributes(wrapper_attrs, false)
            || !neutral_managed_input(ctx, input, Some(transform))
        {
            return None;
        }
        let backing = if let Some(source) = &subclip {
            if source.master.track != 1 || !neutral_managed_input(ctx, source.input, None)
                || ["effect_id", "param_list", "left_bob", "right_bob", "precomputed", "session_attrs"]
                    .iter().any(|key| !matches!(source.input.get(key), None | Some(Value::Null | Value::Ref(0))))
            {
                return None;
            }
            source.master
        } else {
            ColorAdapterMaster { mob: master_id, track: master_track, source_in }
        };
        // DIDP addresses the backing file, while the Color Adapter input addresses
        // its subclip. The zero-origin master check below makes master_in file-relative.
        let master = ctx.mobs.get(&backing.mob)?;
        managed_picture_span(ctx, effect, master, id, file, backing.source_in, length)
    }).or_else(|| {
        // On the retimed route DIDP describes the inclusive backing span,
        // from the first sampled source frame through the last sampled frame.
        let retimed_cube = managed_cache && is_canonical_external_cube(transform);
        if !(managed_range || retimed_cube) || !managed_wrapper_attributes(
            wrapper_attrs, wrapper_clip_color == Some(VERIFIED_GRAY_CLIP_RGB),
        ) {
            return None;
        }
        let motion = subclip_motion.as_ref()?;
        let SourceSubclipSampling { first, last, ratio: (2, 1), .. } =
            motion.sampled_input_frames? else { return None };
        let id = motion.sampled_picture_file?;
        let backing = motion.sampled_picture_master.unwrap_or(ColorAdapterMaster {
            mob: motion.input_mob, track: motion.input_track, source_in: first,
        });
        let master = ctx.mobs.get(&backing.mob)?;
        let file = ctx.mobs.get(&id)?;
        let sampled_span = last.checked_sub(first)?.checked_add(1)?;
        if backing.track != 1 {
            return None;
        }
        managed_picture_span(ctx, effect, master, id, file, backing.source_in, sampled_span)
    });
    let picture_file = match picture {
        Some(_) if conflicting_input_transform => Observed::Unresolved(copy_text(
            budget, MANAGED_INPUT_TRANSFORM_CONFLICT,
        )),
        Some((id, file)) if input_transform_text(ctx, file) == Some(Ok(transform)) => Observed::Known(id),
        Some((id, file)) if managed_picture_span.is_some() && input_transform_text(ctx, file).is_none() =>
            Observed::Known(id),
        Some(_) => Observed::Unresolved(copy_text(
            budget, "edit-side color transformation does not match the saved picture source",
        )),
        None if subclip_motion.as_ref().is_some_and(|motion| motion.sampled_picture_master.is_some()) =>
            Observed::Unresolved(copy_text(
                budget, "the retimed subclip's input cadence is qualified; its picture-file color binding remains unresolved",
            )),
        None if subclip_motion.as_ref().is_some_and(|motion| motion.sampled_input_frames.is_some()) =>
            Observed::Unresolved(copy_text(
                budget, "the retimed subclip's master-frame cadence is qualified; its picture-file color binding remains unresolved",
            )),
        None if subclip_motion.is_some() => Observed::Unresolved(copy_text(
            budget, "the Color Adapter input is a retimed subclip; its rendered source mapping is unqualified",
        )),
        None => Observed::Unresolved(copy_text(
            budget, "color transformation has no unambiguous direct picture file",
        )),
    };
    Some(SourceColorAdapter {
        master: master_id,
        master_track,
        source_in,
        subclip_master: subclip
            .filter(|_| charge(budget, size_of::<ColorAdapterMaster>()))
            .map(|source| Box::new(source.master)),
        subclip_motion,
        picture_file,
        transform: copy_text(budget, transform),
        neutral_wrapper: neutral,
        wrapper_clip_color,
        managed_picture_span,
        centered_still_fit,
    })
}

fn still_raster(ctx: &Ctx<'_>, file: &Node) -> bool {
    ctx.deref(file.get("descriptor")).is_some_and(|descriptor| {
        let centered_rect = |field| {
            matches!(descriptor.get(field), Some(Value::List(points)) if points.len() == 4
                && points.iter().zip([(-640, 1), (-1280, 3), (1280, 1), (2560, 3)])
                    .all(|(point, (num, den))| matches!(point, Value::List(value)
                        if value.as_slice() == [Value::Int(num), Value::Int(den)])))
        };
        descriptor.class == *b"RGBA"
            && [
                ("stored_width", 1536), ("stored_height", 1024),
                ("sampled_width", 1536), ("sampled_height", 1024),
                ("display_width", 1536), ("display_height", 1024),
                ("sampled_x_offset", 0), ("sampled_y_offset", 0),
                ("display_x_offset", 0), ("display_y_offset", 0), ("frame_layout", 0),
                ("alpha_transparency", 0),
            ].iter().all(|(field, value)| int(descriptor, field) == Some(*value))
            && matches!(descriptor.get("aspect_ratio"), Some(Value::List(ratio))
                if ratio.as_slice() == [Value::Int(3), Value::Int(2)])
            && ["valid_box", "essence_box", "source_box"].into_iter().all(centered_rect)
            && descriptor.get("framing_box").is_none()
            && descriptor.get("reformatting_option").is_none()
    })
}

pub(super) fn hd_24_canvas(ctx: &Ctx<'_>, mob: &Node) -> bool {
    let attrs = borrowed_attrs(ctx, mob);
    unique_attribute(attrs, "SEQUERNCE_FORMAT_TYPE").and_then(Value::as_int) == Some(10)
        && unique_attribute(attrs, "SEQUERNCE_FORMAT_STRING").and_then(Value::as_str) == Some("HD 1080p/24")
}

fn centered_still_input<'a>(ctx: &Ctx<'a>, program: &'a Node, length: i64) -> Option<&'a Node> {
    let adapter = sole_picture_segment(ctx, program)?;
    if program.class != *b"SEQU" || float(program, "edit_rate") != Some(24.0)
        || adapter.class != *b"TKFX" || int(adapter, "length") != Some(length)
        || float(adapter, "edit_rate") != Some(24.0)
        || adapter.get("effect_id").and_then(Value::as_str) != Some("INT_EFF_SPATIAL_ADAPTER")
        || !neutral_wrapper(ctx, adapter) || !centered_still_controls(ctx, adapter)
    {
        return None;
    }
    let Value::List(tracks) = adapter.get("tracks")? else { return None };
    let [Value::Node(track)] = tracks.as_slice() else { return None };
    if !spatial_track(ctx, track) {
        return None;
    }
    let input = sole_picture_input(ctx, matte_track(ctx, adapter, 1)?)?;
    let attrs = borrowed_attrs(ctx, input);
    const BOUNDS: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?><Bounds>",
        "  <Framing>-640/1 -1280/3 1280/1 2560/3</Framing>",
        "  <Valid>-640/1 -1280/3 1280/1 2560/3</Valid>",
        "  <Essence>-640/1 -1280/3 1280/1 2560/3</Essence>",
        "  <Source>-640/1 -1280/3 1280/1 2560/3</Source></Bounds>",
    );
    (["effect_id", "param_list", "precomputed", "session_attrs", "left_bob", "right_bob"]
        .iter().all(|field| input.get(field) == Some(&Value::Null)) && attrs.len() == 2
        && unique_attribute(attrs, "_BOUNDS_REFORMATTING_OPTION").and_then(Value::as_int) == Some(1)
        && unique_attribute(attrs, "_IMAGE_BOUNDS_OVERRIDE").and_then(Value::as_str) == Some(BOUNDS))
        .then_some(input)
}

fn spatial_track(ctx: &Ctx<'_>, track: &Node) -> bool {
    ordinary_picture_track(track) && ctx.deref(track.get("filler_proxy")).is_some_and(|proxy| {
        proxy.class == *b"TRKR" && int(proxy, "media_kind_id") == Some(KIND_PICTURE)
            && float(proxy, "edit_rate") == Some(24.0) && int(proxy, "length") == Some(i64::from(i32::MAX))
            && int(proxy, "relative_scope") == Some(1) && int(proxy, "relative_track") == Some(-1)
    })
}

fn centered_still_controls(ctx: &Ctx<'_>, effect: &Node) -> bool {
    let Some(Value::RefList(items)) = ctx.deref(effect.get("param_list"))
        .filter(|n| n.class == *b"PRLS").and_then(|n| n.get("items")) else { return false };
    if items.len() != 35 {
        return false;
    }
    let mut seen = HashSet::with_capacity(items.len());
    items.iter().all(|reference| {
        let Some(p) = ctx.deref(Some(reference)) else { return false };
        let Some(Value::Uuid(id)) = p.get("uuid") else { return false };
        if p.class != *b"PRIT" || p.get("enable") != Some(&Value::Bool(true))
            || p.get("control_track") != Some(&Value::Null) || !seen.insert(*id)
        {
            return false;
        }
        if file_order_hex(id) == "dca5f78a7b361949afc75ca00a5bd558" {
            return int(p, "value_type") == Some(1) && int(p, "value") == Some(1)
                && p.get("contribs_to_sig") == Some(&Value::Bool(false));
        }
        let Some(field) = crate::parameter_registry::parameter_name(id)
            .and_then(|name| name.strip_prefix("AFX_SPATIAL_")) else { return false };
        if p.get("contribs_to_sig") != Some(&Value::Bool(true)) {
            return false;
        }
        if field == "DVE_ROT_DEFAULT" {
            return int(p, "value_type") == Some(2) && float(p, "value") == Some(0.0);
        }
        let value = if field == "REFORMAT" {
            1
        } else {
            let Some((box_name, field)) = field.split_once('_') else { return false };
            if !matches!(box_name, "FRAMING" | "VALID" | "ESSENCE" | "SOURCE") {
                return false;
            }
            match field {
                "POSX_NUM" => -640,
                "POSY_NUM" => -1280,
                "WID_NUM" => 1280,
                "HEI_NUM" => 2560,
                "POSX_DEN" | "WID_DEN" => 1,
                "POSY_DEN" | "HEI_DEN" => 3,
                _ => return false,
            }
        };
        int(p, "value_type") == Some(1) && int(p, "value") == Some(value)
    })
}

fn neutral_wrapper(ctx: &Ctx<'_>, effect: &Node) -> bool {
    let attrs = borrowed_attrs(ctx, effect);
    let border = unique_attribute(attrs, "_BORDER_FIXED").and_then(Value::as_int);
    attrs.len() == 1 && border == Some(0)
        && effect.get("left_bob") == Some(&Value::Null)
        && neutral_controls(ctx, effect)
}

fn neutral_managed_input(ctx: &Ctx<'_>, input: &Node, adapter_transform: Option<&str>) -> bool {
    let attrs = borrowed_attrs(ctx, input);
    attrs.len() <= 3 && attrs.iter().all(|(name, _)| {
        unique_attribute(attrs, name).is_some_and(|value| match name.as_str() {
            "_IMAGE_BOUNDS_OVERRIDE" => value.as_str().is_some_and(matte::equal_1600_picture_bounds),
            // Avid renders this exact copy as part of the existing adapter.
            "_COLOR_INPUT_TRANSFORMATION" => adapter_transform.is_some_and(|xml| value.as_str() == Some(xml)),
            // The normal component walk already records the markers in the input clock.
            "_TMP_CRM" => ctx.deref(Some(value)).is_some_and(|list| {
                list.class == *b"TMCS"
                    && matches!(list.get("items"), Some(Value::RefList(items))
                        if items.iter().all(|item| ctx.deref(Some(item))
                            .is_some_and(|marker| marker.class == *b"TMBC")))
            }),
            _ => false,
        })
    })
}

fn managed_wrapper_attributes(attrs: &[(String, Value)], verified_gray_clip_color: bool) -> bool {
    let zero = |key| unique_attribute(attrs, key).and_then(Value::as_int) == Some(0);
    zero("_BORDER_FIXED") && match attrs.len() {
        1 => true,
        2 => zero("_IGNORE_TRACKING"),
        // In the retimed Avid control this exact triplet had no effect on the
        // decoded picture. All other wrapper controls still need to match.
        5 => verified_gray_clip_color && zero("_IGNORE_TRACKING"),
        _ => false,
    }
}

fn managed_picture_span(
    ctx: &Ctx<'_>, effect: &Node, master: &Node,
    file_id: MobId, file: &Node, source_in: i64, length: i64,
) -> Option<(i64, i64)> {
    if !neutral_controls(ctx, effect) || source_in < 0 {
        return None;
    }
    let end = source_in.checked_add(length)?;
    let (track, component) = picture_track(ctx, master, 1)?;
    let backing = sole_picture_input(ctx, component)?;
    let descriptor = ctx.deref(file.get("descriptor"))?;
    if !ordinary_picture_track(track) || !same_rate_picture_span(backing, end)
        || int(backing, "start_time") != Some(0) || int(backing, "track_id") != Some(1)
        || descriptor.class != *b"CDCI" || !same_rate_picture_span(descriptor, end)
        || int(descriptor, "component_width") != Some(10)
        || int(descriptor, "black_ref_level") != Some(64)
        || int(descriptor, "white_ref_level") != Some(940)
    {
        return None;
    }
    let cache = ctx.deref(effect.get("left_bob"))?;
    if cache.class != *b"DIDP" || mob_id(cache) != Some(file_id)
        || int(cache, "sample_num") != Some(source_in) || int(cache, "length") != Some(length)
        || int(cache, "track_type") != Some(KIND_PICTURE) || int(cache, "track_index") != Some(1)
        || int(cache, "strip") != Some(0) || cache.get("spos_invalid") != Some(&Value::Bool(false))
    {
        return None;
    }
    let position = |key| match cache.get(key) {
        Some(Value::UInt(n)) if *n > 0 => i64::try_from(*n).ok(),
        _ => None,
    };
    Some((position("offset")?, position("byte_length")?))
}

fn neutral_controls(ctx: &Ctx<'_>, effect: &Node) -> bool {
    ["right_bob", "precomputed", "session_attrs", "trackman"]
            .iter().all(|k| effect.get(k) == Some(&Value::Null))
        && [
            ("mc_mode", 0), ("num_scalars", 0), ("info_version", 2),
            ("info_current", 0), ("info_smooth", 0), ("info_color_item", 1),
            ("info_quality", 1), ("info_is_reversed", 0),
            ("left_length", 15), ("right_length", 15),
        ].iter().all(|(key, expected)| int(effect, key) == Some(*expected))
        && [
            ("info_aspect_on", true), ("info_force_software", false),
            ("info_never_hardware", false),
        ].iter().all(|(key, expected)| effect.get(key) == Some(&Value::Bool(*expected)))
        && ctx.deref(effect.get("keyframes"))
            .is_some_and(|fx| matte::neutral_keys(fx, 1 << 29))
}

fn sole_picture_input<'a>(ctx: &Ctx<'a>, component: &'a Node) -> Option<&'a Node> {
    let clip = if component.class == *b"SEQU" {
        sole_picture_segment(ctx, component)?
    } else {
        component
    };
    (clip.class == *b"SCLP"
        && int(clip, "media_kind_id") == Some(KIND_PICTURE)
        && int(clip, "length").is_some_and(|length| length > 0))
        .then_some(clip)
}

fn direct_picture_file<'a>(
    ctx: &Ctx<'a>, master_id: MobId, master_track: i64, ambiguous: &HashSet<MobId>,
) -> Option<(MobId, &'a Node)> {
    let master = ctx.mobs.get(&master_id)?;
    if ambiguous.contains(&master_id) || int(master, "mob_type_id") != Some(MOB_MASTER) {
        return None;
    }
    let (_, component) = picture_track(ctx, master, master_track)?;
    let backing = sole_picture_input(ctx, component)?;
    let file_id = mob_id(backing)?;
    let file = ctx.mobs.get(&file_id)?;
    if ambiguous.contains(&file_id) || int(file, "mob_type_id") != Some(MOB_SOURCE) {
        return None;
    }
    Some((file_id, file))
}

fn picture_track<'a>(ctx: &Ctx<'a>, owner: &'a Node, label: i64) -> Option<(&'a Node, &'a Node)> {
    let master_tracks = ctx.tracks(owner);
    let mut matches = master_tracks.iter().filter(|track| {
        int(track, "index") == Some(label)
            && ctx.deref(track.get("component")).and_then(|node| int(node, "media_kind_id"))
                == Some(KIND_PICTURE)
    });
    let track = *matches.next()?;
    matches.next().is_none().then_some((track, ctx.deref(track.get("component"))?))
}

fn same_rate_picture_span(node: &Node, end: i64) -> bool {
    float(node, "edit_rate") == Some(24.0)
        && int(node, "length").is_some_and(|length| end <= length)
}

fn ordinary_picture_track(track: &Node) -> bool {
    matches!(int(track, "start_pos"), None | Some(0))
        && track.get("control_code").is_none()
        && track.get("control_sub_code").is_none()
}

fn subclip_motion<'a>(
    ctx: &Ctx<'a>, id: MobId, label: i64, start: i64, length: i64,
    ambiguous: &HashSet<MobId>, rate: Rate, allow_other_speeds: bool,
) -> Option<(&'a Node, SourceSubclipMotion)> {
    let at_rate = |node: &Node| float(node, "edit_rate").and_then(Rate::from_avid) == Some(rate);
    let covers = |node: &Node, end| at_rate(node) && int(node, "length").is_some_and(|len| len >= end);
    let subclip = ctx.mobs.get(&id)?;
    let end = start.checked_add(length)?;
    if start < 0 || length <= 0 || ambiguous.contains(&id)
        || int(subclip, "mob_type_id") != Some(MOB_COMPOSITION)
        || int(subclip, "usage_code") != Some(2)
        || !covers(subclip, end)
    {
        return None;
    }
    let (track, component) = picture_track(ctx, subclip, label)?;
    let neutral_filler_proxy = match track.get("filler_proxy") {
        None | Some(Value::Null | Value::Ref(0)) => true,
        reference => ctx.deref(reference).is_some_and(|proxy| {
            proxy.class == *b"TRKR" && int(proxy, "media_kind_id") == Some(KIND_PICTURE)
                && at_rate(proxy)
                && int(proxy, "length") == Some(i64::from(i32::MAX))
                && int(proxy, "relative_scope") == Some(0) && int(proxy, "relative_track") == Some(-1)
        }),
    };
    if !ordinary_picture_track(track) || !at_rate(component)
        || !neutral_filler_proxy
    {
        return None;
    }
    let effect = if component.class == *b"SEQU" {
        sole_picture_segment(ctx, component)?
    } else {
        component
    };
    let mut motion = timewarp::observe_subclip_motion(ctx, effect, start, length, ambiguous, rate, allow_other_speeds)?;
    if int(subclip, "length") != Some(motion.length) {
        return None;
    }
    if let Some(samples) = motion.sampled_input_frames {
        let SourceSubclipSampling { first, last, .. } = samples;
        let input = timewarp::direct_input(ctx, effect)?;
        let mut master = ColorAdapterMaster {
            mob: motion.input_mob, track: motion.input_track, source_in: first,
        };
        let sampled_span = last.checked_sub(first)?.checked_add(1)?;
        if !timewarp::master_picture_matches(ctx, input) {
            let route = subclip_picture_file(
                ctx, motion.input_mob, motion.input_track, first, sampled_span, ambiguous,
            ).filter(|source| {
                samples.ratio == (2, 1) && source.master.track == 1
                    // The native nonzero-offset control did not render the
                    // frame positions implied by a straight addition.
                    && source.master.source_in == first
                    && motion_hop_is_plain(ctx, source.input, first, last)
                    && motion_hop_is_plain(ctx, input, first - motion.input_start, last - motion.input_start)
            });
            let Some(route) = route else {
                motion.sampled_input_frames = None;
                return Some((effect, motion));
            };
            master = route.master;
            motion.sampled_picture_master = Some(master);
        }
        motion.sampled_picture_file = (|| {
            let end = samples.source_end(length)?;
            let owner = ctx.mobs.get(&master.mob)?;
            let (track, component) = picture_track(ctx, owner, master.track)?;
            let backing = sole_picture_input(ctx, component)?;
            let (id, file) = direct_picture_file(ctx, master.mob, master.track, ambiguous)?;
            let desc = ctx.deref(file.get("descriptor"))?;
            if !ordinary_picture_track(track) || !at_rate(component) || !covers(backing, end)
                || int(backing, "start_time") != Some(0) || int(backing, "track_id") != Some(1)
                || !covers(desc, end)
                || int(desc, "frame_layout") != Some(0)
            {
                return None;
            }
            Some(id)
        })();
    }
    Some((effect, motion))
}

pub(super) fn standalone_subclip_motion(
    ctx: &Ctx<'_>, clip: &Node, rate: Rate, ambiguous: &HashSet<MobId>,
) -> Option<SourceSubclipMotion> {
    let id = mob_id(clip)?;
    if ctx.mobs.get(&id).and_then(|mob| int(mob, "usage_code")) != Some(2) {
        return None;
    }
    let length = int(clip, "length").filter(|&length| length > 0)?;
    if !motion_clip_is_plain(ctx, clip, 0, length - 1) {
        return None;
    }
    let (effect, motion) = subclip_motion(ctx, id, int(clip, "track_id")?,
        int(clip, "start_time")?, length, ambiguous, rate, true)?;
    validate_direct_motion_positions(ctx, clip, effect, &motion)?;
    Some(motion)
}

fn validate_direct_motion_positions(
    ctx: &Ctx<'_>, clip: &Node, effect: &Node, motion: &SourceSubclipMotion,
) -> Option<()> {
    let SourceSubclipSampling { first, last, .. } = motion.sampled_input_frames?;
    let file = motion.sampled_picture_file?;
    let input = timewarp::direct_input(ctx, effect)?;
    if motion.sampled_picture_master.is_some()
        || !motion_clip_is_plain(ctx, input, first - motion.input_start, last - motion.input_start)
    {
        return None;
    }
    // The saved source clips can carry an inherited position for the motion
    // owner's untrimmed span. Check its backing package before using the
    // independent Speed and Position controls to resolve the selected trim.
    let position_matches = |node: &Node, span, trimmed: Option<(i64, i64)>| {
        matches!(node.get("right_bob"), None | Some(Value::Null | Value::Ref(0)))
            && (matches!(node.get("left_bob"), None | Some(Value::Null | Value::Ref(0)))
                || ctx.deref(node.get("left_bob")).is_some_and(|saved| {
                    let saved_range = int(saved, "sample_num").zip(int(saved, "length"));
                    saved.class == *b"DIDP" && mob_id(saved) == Some(file)
                        && (saved_range == Some((motion.input_start, span))
                            || trimmed.is_some_and(|range| saved_range == Some(range)))
                        && int(saved, "track_type") == Some(KIND_PICTURE)
                        && int(saved, "track_index") == Some(1) && int(saved, "strip") == Some(0)
                        && saved.get("spos_invalid") == Some(&Value::Bool(false))
                }))
    };
    let sampled_span = last.checked_sub(first)?.checked_add(1)?;
    (position_matches(clip, motion.length, Some((first, sampled_span)))
        && position_matches(input, motion.length, None)
        && position_matches(effect, motion.input_length, None)).then_some(())
}

fn motion_hop_is_plain(ctx: &Ctx<'_>, clip: &Node, first: i64, last: i64) -> bool {
    matches!(clip.get("left_bob"), None | Some(Value::Null | Value::Ref(0)))
        && motion_clip_is_plain(ctx, clip, first, last)
}

fn motion_clip_is_plain(ctx: &Ctx<'_>, clip: &Node, first: i64, last: i64) -> bool {
    if !["effect_id", "param_list", "right_bob", "precomputed", "session_attrs"]
        .iter().all(|key| matches!(clip.get(key), None | Some(Value::Null | Value::Ref(0))))
        || !neutral_managed_input(ctx, clip, None)
    {
        return false;
    }
    let mut clear = true;
    each_marker(ctx, clip, |marker| {
        let fields = borrowed_attrs(ctx, marker);
        let length = if fields.iter().any(|(key, _)| key == "_ATN_CRM_LENGTH") {
            unique_attribute(fields, "_ATN_CRM_LENGTH")
                .and_then(Value::as_int).filter(|length| *length >= 0)
        } else {
            Some(1)
        };
        clear &= int(marker, "comp_offset").is_some_and(|position| {
            length.is_some_and(|length| {
                position > last || position.checked_add(length.max(1)).is_some_and(|end| end <= first)
            })
        });
    });
    clear
}

fn subclip_picture_input<'a>(ctx: &Ctx<'a>, subclip: &Node, component: &'a Node) -> Option<&'a Node> {
    if component.class == *b"SCLP" {
        return sole_picture_input(ctx, component);
    }
    if component.class != *b"SEQU" || float(component, "edit_rate") != Some(24.0) {
        return None;
    }
    let Value::RefList(children) = component.get("components")? else { return None };
    let [head, clip, tail] = children.as_slice() else { return None };
    let filler_length = |node: &Node| {
        if node.class != *b"FILL" || int(node, "media_kind_id") != Some(KIND_PICTURE)
            || float(node, "edit_rate") != Some(24.0)
        {
            return None;
        }
        int(node, "length").filter(|length| *length >= 0)
    };
    if filler_length(ctx.deref(Some(head))?)? != 0 {
        return None;
    }
    let clip = ctx.deref(Some(clip))?;
    if clip.class != *b"SCLP" {
        return None;
    }
    let clip = sole_picture_input(ctx, clip)?;
    let span = int(clip, "length")?.checked_add(filler_length(ctx.deref(Some(tail))?)?)?;
    (int(subclip, "length") == Some(span)).then_some(clip)
}

struct SubclipPicture<'a> {
    master: ColorAdapterMaster,
    file_id: MobId,
    file: &'a Node,
    input: &'a Node,
}

/// Only one subclip hop. The trimmed range must fit the selected picture
/// before any trailing filler, and the backing file at the same clock.
fn subclip_picture_file<'a>(
    ctx: &Ctx<'a>, id: MobId, label: i64, start: i64, length: i64,
    ambiguous: &HashSet<MobId>,
) -> Option<SubclipPicture<'a>> {
    let subclip = ctx.mobs.get(&id)?;
    let end = start.checked_add(length)?;
    if start < 0 || length <= 0 || ambiguous.contains(&id)
        || int(subclip, "mob_type_id") != Some(MOB_COMPOSITION)
        || int(subclip, "usage_code") != Some(2)
        || !same_rate_picture_span(subclip, end)
    {
        return None;
    }
    let (track, component) = picture_track(ctx, subclip, label)?;
    if !ordinary_picture_track(track)
        || !matches!(track.get("filler_proxy"), None | Some(Value::Null | Value::Ref(0)))
    {
        return None;
    }
    let input = subclip_picture_input(ctx, subclip, component)?;
    if !same_rate_picture_span(input, end) {
        return None;
    }
    let master_id = mob_id(input)?;
    let master_label = int(input, "track_id").filter(|label| *label > 0)?;
    let master_in = int(input, "start_time").filter(|offset| *offset >= 0)?.checked_add(start)?;
    let master_end = master_in.checked_add(length)?;
    let master = ctx.mobs.get(&master_id)?;
    if ambiguous.contains(&master_id) || int(master, "mob_type_id") != Some(MOB_MASTER)
        || !same_rate_picture_span(master, master_end)
    {
        return None;
    }
    let (track, component) = picture_track(ctx, master, master_label)?;
    let backing = sole_picture_input(ctx, component)?;
    if !ordinary_picture_track(track) || float(component, "edit_rate") != Some(24.0)
        || !same_rate_picture_span(backing, master_end)
        || !int(backing, "track_id").is_some_and(|label| label > 0)
    {
        return None;
    }
    let file_end = int(backing, "start_time").filter(|offset| *offset >= 0)?.checked_add(master_end)?;
    let file_id = mob_id(backing)?;
    let file = ctx.mobs.get(&file_id)?;
    if ambiguous.contains(&file_id) || int(file, "mob_type_id") != Some(MOB_SOURCE)
        || !same_rate_picture_span(file, file_end)
    {
        return None;
    }
    Some(SubclipPicture {
        master: ColorAdapterMaster { mob: master_id, track: master_label, source_in: master_in },
        file_id,
        file,
        input,
    })
}

pub(super) fn saved_payload<'a>(ctx: &Ctx<'a>, effect: &Node) -> Option<&'a str> {
    const ENABLE: &str = "dca5f78a7b361949afc75ca00a5bd558";
    const PAYLOAD: &str = "d85c7fbdfd154e42a34d11642fbbb867";
    const TRANSFORM: &str = "cc999a218b2c244286fec05794055e1d";
    let list = ctx.deref(effect.get("param_list")).filter(|n| n.class == *b"PRLS")?;
    let Value::RefList(items) = list.get("items")? else { return None };
    if items.len() != 2 {
        return None;
    }
    let parameter = |id, ty, signature| {
        items.iter().filter_map(|r| ctx.deref(Some(r))).find(|n| {
            n.class == *b"PRIT"
                && matches!(n.get("uuid"), Some(Value::Uuid(u)) if file_order_hex(u) == id)
                && int(n, "value_type") == Some(ty)
                && n.get("enable") == Some(&Value::Bool(true))
                && n.get("control_track") == Some(&Value::Null)
                && n.get("contribs_to_sig") == Some(&Value::Bool(signature))
        })
    };
    if int(parameter(ENABLE, 1, false)?, "value") != Some(1) {
        return None;
    }
    let value = ctx.deref(parameter(PAYLOAD, 4, true)?.get("value"))?;
    if value.class != *b"AVUP" || int(value, "byte_order") != Some(18761)
        || !matches!(value.get("uuid"), Some(Value::Uuid(u)) if file_order_hex(u) == TRANSFORM)
    {
        return None;
    }
    let Value::Bytes(bytes) = value.get("data")? else { return None };
    let text = bytes.strip_suffix(&[0])?;
    if text.is_empty() || text.len() > MAX_INPUT_TRANSFORM_BYTES || text.contains(&0) {
        return None;
    }
    std::str::from_utf8(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{document::AvbDocument, file::Object};

    #[test]
    fn saved_standalone_range_requires_endpoints_and_the_matching_direction_flag() {
        let xml = |direction, inverted| format!(
            "<ColorTransformationList><ColorTransformation><LinearLut><Name>Levels scaling ({direction})</Name><BitDepth>10</BitDepth><Black>64</Black><White>940</White>{inverted}</LinearLut></ColorTransformation></ColorTransformationList>"
        );
        let forward = xml("video levels to full range", "");
        let reverse = xml("full range to video levels", "<Inverted/>");
        assert_eq!(SourceColorRange::from_saved_xml(&forward), Some(SourceColorRange::VideoToFull));
        assert_eq!(SourceColorRange::from_saved_xml(&reverse), Some(SourceColorRange::FullToVideo));
        assert_eq!(SourceColorRange::from_saved_xml(&xml("video levels to full range", "<Inverted/>")), None);
        assert_eq!(SourceColorRange::from_saved_xml(&reverse.replace("<Inverted/>", "")), None);
        assert_eq!(SourceColorRange::from_saved_xml(&forward.replace("940", "939")), None);
        let extra = forward.replace("</ColorTransformationList>",
            "<ColorTransformation><ExternalLut/></ColorTransformation></ColorTransformationList>");
        assert_eq!(SourceColorRange::from_saved_xml(&extra), None);
    }

    #[test]
    fn managed_color_input_accepts_markers_and_a_matching_adapter_copy() {
        let bytes = AvbDocument::new_empty_bin(19, "Managed marker control", 1, false)
            .to_bytes().unwrap();
        let mut file = AvbFile::parse(&bytes).unwrap();
        let push = |file: &mut AvbFile, class, fields| {
            let reference = Value::Ref(file.objects.len() as u32);
            file.objects.push(Some(Object::Decoded(Node { class, type_name: "Control", fields })));
            reference
        };
        let marker = push(&mut file, *b"TMBC", vec![]);
        let empty = push(&mut file, *b"TMCS", vec![("items", Value::RefList(vec![]))]);
        let marked = push(&mut file, *b"TMCS", vec![("items", Value::RefList(vec![marker]))]);
        let bad = push(&mut file, *b"TMCS", vec![("items", Value::RefList(vec![empty.clone()]))]);
        let carrier = |value| ("_TMP_CRM".into(), value);
        let color = |text: &str| ("_COLOR_INPUT_TRANSFORMATION".into(), Value::Str(text.into()));
        for (attrs, adapter_transform, accepted) in [
            (vec![], None, true),
            (vec![carrier(empty)], None, true),
            (vec![carrier(marked.clone())], None, true),
            (vec![carrier(marked.clone()), carrier(marked.clone())], None, false),
            (vec![carrier(bad)], None, false),
            (vec![carrier(Value::Int(1))], None, false),
            (vec![color("match"), carrier(marked)], Some("match"), true),
            (vec![color("match")], None, false),
            (vec![color("different")], Some("match"), false),
            (vec![color("match"), color("match")], Some("match"), false),
        ] {
            let attrs = push(&mut file, *b"ATTR", vec![("entries", Value::Attrs(attrs))]);
            let input = Node {
                class: *b"SCLP",
                type_name: "Picture clip",
                fields: vec![("attributes", attrs)],
            };
            let ctx = Ctx { file: &file, mobs: Default::default() };
            assert_eq!(neutral_managed_input(&ctx, &input, adapter_transform), accepted);
        }
    }

    #[test]
    fn picture_subclip_sequence_admits_only_a_zero_head_and_consistent_tail() {
        let bytes = AvbDocument::new_empty_bin(19, "Source color control", 1, false)
            .to_bytes().unwrap();
        for (head, tail_rate, span, accepted) in [
            (0, 24.0, 25, true),
            (1, 24.0, 26, false),
            (0, 25.0, 25, false),
            (0, 24.0, 24, false),
        ] {
            let mut file = AvbFile::parse(&bytes).unwrap();
            let mut refs = Vec::new();
            for (class, length, rate) in [(*b"FILL", head, 24.0), (*b"SCLP", 20, 24.0), (*b"FILL", 5, tail_rate)] {
                refs.push(Value::Ref(file.objects.len() as u32));
                file.objects.push(Some(Object::Decoded(Node {
                    class,
                    type_name: "Picture input",
                    fields: vec![
                        ("media_kind_id", Value::Int(KIND_PICTURE)),
                        ("edit_rate", Value::Float(rate)),
                        ("length", Value::Int(length)),
                    ],
                })));
            }
            let ctx = Ctx { file: &file, mobs: Default::default() };
            let sequence = Node {
                class: *b"SEQU",
                type_name: "Picture sequence",
                fields: vec![
                    ("media_kind_id", Value::Int(KIND_PICTURE)),
                    ("edit_rate", Value::Float(24.0)),
                    ("components", Value::RefList(refs)),
                ],
            };
            let subclip = Node {
                class: *b"CMPO",
                type_name: "Picture subclip",
                fields: vec![("length", Value::Int(span))],
            };
            let input = subclip_picture_input(&ctx, &subclip, &sequence);
            assert_eq!(input.map(|node| node.class), accepted.then_some(*b"SCLP"));
        }
    }
}
