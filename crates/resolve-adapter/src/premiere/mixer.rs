//! Saved Premiere audio track and Mix bus controls.
use super::{components, controls};
use crate::Result;
use prproj::{model::Project, xml::NodeId};

pub(super) fn read_mix_bus(project: &Project, sequence: NodeId, allow_gain: bool) -> Result<i32> {
    let tree = &project.doc.tree;
    let mut buses = 0;
    let mut gain = 0;
    for reference in tree.findall(sequence, "TrackGroups/TrackGroup/Second")? {
        let group = project.doc.resolve(reference)?;
        if tree.tag(group) != "AudioTrackGroup" {
            continue;
        }
        buses += 1;
        if tree.findtext(group, "NumAdaptiveChannels")?.as_deref() != Some("2") {
            return Err("expected a stereo Premiere mix bus".into());
        }
        let master = tree
            .find1(group, "MasterTrack")
            .ok_or("missing Premiere mix bus")?;
        gain = read_mixer(project, project.doc.resolve(master)?, allow_gain, false)?.tenths_db;
    }
    if buses != 1 {
        return Err("expected one Premiere audio track group".into());
    }
    Ok(gain)
}

#[derive(Default)]
pub(super) struct StaticFader {
    pub(super) tenths_db: i32,
    pub(super) muted: bool,
    pub(super) pan: i32,
}

const AUDIO_SCALAR_CLASS: &str = "a714635e-a628-4b27-9d59-77eba47dbc1a";

fn static_mixer_value(
    tree: &prproj::xml::Tree,
    parameter: NodeId,
    invalid: impl Fn() -> String,
) -> Result<(Option<&str>, Option<&str>)> {
    if !matches!(
        components::field(tree, parameter, "IsTimeVarying")?,
        None | Some("false")
    ) || components::field(tree, parameter, "Keyframes")?.is_some()
    {
        return Err(invalid().into());
    }
    let start = components::field(tree, parameter, "StartKeyframe")?
        .map(|start| {
            let (time, value) = start.split_once(',').ok_or_else(&invalid)?;
            if time.parse() != Ok(controls::DEFAULT_TICKS) {
                return Err(invalid());
            }
            value.strip_suffix(",0,0,0,0,0,0").ok_or_else(&invalid)
        })
        .transpose()?;
    Ok((start, components::field(tree, parameter, "CurrentValue")?))
}

pub(super) fn read_track(
    project: &Project,
    track: NodeId,
    capture_static: bool,
) -> Result<StaticFader> {
    read_mixer(project, track, capture_static, capture_static)
}

fn read_mixer(
    project: &Project,
    track: NodeId,
    allow_gain: bool,
    allow_mute: bool,
) -> Result<StaticFader> {
    let tree = &project.doc.tree;
    let mut fader = StaticFader::default();
    let mut seen = [false; 2];
    let mut check_param = |reference, is_fader| -> Result<()> {
        let p = project.doc.resolve(reference)?;
        if prproj::model::component::param_is_time_varying(tree, p)
            || tree
                .findtext(p, "Keyframes")?
                .is_some_and(|v| !v.is_empty())
        {
            return Err("Premiere mixer automation requires an explicit mapping".into());
        }
        let name = components::field(tree, p, "Name")?.unwrap_or_default();
        if is_fader && ((allow_gain && name == "Volume") || (allow_mute && name == "Mute")) {
            let mute = name == "Mute";
            let invalid = || format!("unqualified Premiere static mixer {name}");
            if tree.get(reference, "Index") != Some(if mute { "1" } else { "0" })
                || std::mem::replace(&mut seen[usize::from(mute)], true)
                || tree.tag(p) != "AudioComponentParam"
                || tree.get(p, "ClassID")
                    != Some(if mute {
                        "32657501-3aa4-445f-a49b-d09ecb9fa1ae"
                    } else {
                        AUDIO_SCALAR_CLASS
                    })
            {
                return Err(invalid().into());
            }
            let (value, current) = static_mixer_value(tree, p, invalid)?;
            if mute {
                let parse = |v| match v {
                    None | Some("false") => Ok(false),
                    Some("true") => Ok(true),
                    _ => Err(invalid()),
                };
                fader.muted = parse(value.or(current))?;
                if current.is_some() && fader.muted != parse(current)? {
                    return Err(invalid().into());
                }
            } else {
                let parse = |v: Option<&str>| -> Result<i32> {
                    let value: f64 = v.unwrap_or("1").parse().map_err(|_| invalid())?;
                    let db = 20.0 * value.log10();
                    let ticks = (db * 10.0).round() as i32;
                    if !crate::source::mixer::valid_ticks(ticks)
                        || !db.is_finite()
                        || (db - f64::from(ticks) / 10.0).abs() > 0.00001
                    {
                        return Err(invalid().into());
                    }
                    Ok(ticks)
                };
                fader.tenths_db = parse(value.or(current))?;
                if current.is_some() && fader.tenths_db != parse(current)? {
                    return Err(invalid().into());
                }
            }
            return Ok(());
        }
        let neutral = |value: &str| match name {
            "Volume" => value.parse::<f64>() == Ok(1.0),
            "Mute" => value == "false",
            "Pan" => value.parse::<f64>() == Ok(0.0),
            "Balance" => value.parse::<f64>() == Ok(0.5),
            _ => false,
        };
        let current = tree.findtext(p, "CurrentValue")?;
        let start = tree.findtext(p, "StartKeyframe")?;
        if !matches!(name, "Volume" | "Mute" | "Pan" | "Balance")
            || current
                .as_deref()
                .is_some_and(|v| !v.is_empty() && !neutral(v))
            || start
                .as_deref()
                .is_some_and(|v| !v.is_empty() && !v.split(',').nth(1).is_some_and(neutral))
        {
            return Err("non-neutral Premiere mixer control requires an explicit mapping".into());
        }
        Ok(())
    };
    let chain = tree
        .find1(track, "AudioTrack/ComponentOwner/Components")
        .ok_or("missing Premiere mixer chain")?;
    let chain = project.doc.resolve(chain)?;
    for reference in tree.findall(chain, "ComponentChain/Components/Component")? {
        let component = project.doc.resolve(reference)?;
        if !matches!(tree.tag(component), "AudioFader" | "AudioMeter") {
            return Err("unsupported Premiere mixer component".into());
        }
        let is_fader = tree.tag(component) == "AudioFader";
        if is_fader
            && allow_gain
            && tree.get(component, "ClassID") != Some("1a38c583-ed5c-11d5-abc4-c1cbf61ec590")
        {
            return Err("unqualified Premiere AudioFader class".into());
        }
        for param in tree.findall(component, "AudioComponent/Component/Params/Param")? {
            check_param(param, is_fader)?;
        }
    }
    let panners = tree.findall(track, "AudioTrack/Panner")?;
    let [panner] = panners.as_slice() else {
        return Err("missing or ambiguous Premiere panner".into());
    };
    let panner = project.doc.resolve(*panner)?;
    if !matches!(
        tree.tag(panner),
        "StereoToStereoPanProcessor" | "MonoToStereoPanProcessor" | "DefaultPanProcessor"
    ) {
        return Err("unsupported Premiere mixer channel routing".into());
    }
    if allow_mute && tree.tag(panner) == "MonoToStereoPanProcessor" {
        fader.pan = read_mono_track_pan(project, track, panner)?;
    } else {
        for param in tree.findall(panner, "PanProcessor/AudioComponent/Component/Params/Param")? {
            check_param(param, false)?;
        }
    }
    for field in ["IsSolo", "IsMuted"] {
        if tree
            .findtext(track, &format!("ClipTrack/Track/{field}"))?
            .is_some_and(|v| v == "true")
        {
            return Err("Premiere mute or solo state requires an explicit mapping".into());
        }
    }
    Ok(fader)
}

fn read_mono_track_pan(project: &Project, track: NodeId, panner: NodeId) -> Result<i32> {
    let tree = &project.doc.tree;
    let invalid = || "unqualified Premiere static mono track pan".to_owned();
    let [audio_track] = tree
        .findall(track, "AudioTrack")?
        .try_into()
        .map_err(|_| invalid())?;
    let [processor] = tree
        .findall(panner, "PanProcessor")?
        .try_into()
        .map_err(|_| invalid())?;
    let [audio] = tree
        .findall(processor, "AudioComponent")?
        .try_into()
        .map_err(|_| invalid())?;
    let [owner] = tree
        .findall(audio, "Component")?
        .try_into()
        .map_err(|_| invalid())?;
    let [reference] = tree
        .findall(owner, "Params/Param")?
        .try_into()
        .map_err(|_| invalid())?;
    let param = project.doc.resolve(reference)?;
    if tree.get(panner, "ClassID") != Some("33a94283-ee2c-11d5-abc4-c1cd7f9e3c10")
        || tree.get(panner, "Version") != Some("1")
        || components::field(tree, audio_track, "ChannelType")? != Some("0")
        || components::field(tree, audio, "ChannelType")? != Some("0")
        || components::field(tree, audio, "AudioChannelLayout")? != Some(r#"[{"channellabel":0}]"#)
        || components::field(tree, processor, "OutputAudioChannelLayout")?
            != Some(r#"[{"channellabel":100},{"channellabel":101}]"#)
        || tree.get(reference, "Index") != Some("0")
        || tree.tag(param) != "AudioComponentParam"
        || tree.get(param, "ClassID") != Some(AUDIO_SCALAR_CLASS)
        || components::field(tree, param, "Name")? != Some("Pan")
        || components::field(tree, param, "IsInverted")? != Some("true")
    {
        return Err(invalid().into());
    }
    for element in [panner, processor, audio, owner, param] {
        for tag in ["Bypass", "IsBypassed", "Enabled", "IsEnabled", "Matrix"] {
            if !tree.findall(element, tag)?.is_empty() {
                return Err(invalid().into());
            }
        }
    }
    let (start, current) = static_mixer_value(tree, param, invalid)?;
    if (start.is_none() && !tree.findall(param, "StartKeyframe")?.is_empty())
        || (current.is_none() && !tree.findall(param, "CurrentValue")?.is_empty())
    {
        return Err(invalid().into());
    }
    let parse = |value: &str| -> Result<i32> {
        let scalar: f64 = value.parse().map_err(|_| invalid())?;
        let pan = (scalar * 200.0 - 100.0) * 10.0;
        let ticks = pan.round();
        if !(0.0..=1.0).contains(&scalar) || (pan - ticks).abs() > 0.0001 {
            return Err(invalid().into());
        }
        Ok(ticks as i32)
    };
    match (start, current) {
        // Premiere saves this panner without either value at full right.
        (None, None) => Ok(1000),
        (Some(start), Some(current)) => {
            let pan = parse(start)?;
            if parse(current)? != pan {
                return Err(invalid().into());
            }
            Ok(pan)
        }
        _ => Err(invalid().into()),
    }
}
