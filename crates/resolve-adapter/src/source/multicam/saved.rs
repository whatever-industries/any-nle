//! Archive links remain useful when the portable multicam profile cannot bind an edit.
use super::*;

const ADAPTER: &str = "resolve.multicam_tracks";
const MAX_SELECTOR_HEX: usize = 512;

#[derive(Default)]
pub(super) struct Tracks {
    picture: HashMap<String, Option<String>>,
    audio: HashMap<String, Option<String>>,
    rows: Vec<Value>,
}

impl Tracks {
    pub(super) fn capture(
        &mut self,
        kind: StreamKind,
        angle: &str,
        track: &Node,
        entries: &[&Node],
        context: &mut Context<'_>,
    ) -> Result<()> {
        let id = track.id()?;
        let index = if kind == StreamKind::Video {
            &mut self.picture
        } else {
            &mut self.audio
        };
        if !angle.is_empty() {
            index
                .entry(angle.to_owned())
                .and_modify(|saved| *saved = None)
                .or_insert_with(|| Some(id.into()));
        }
        let members = entries
            .iter()
            .map(|node| {
                let target = node
                    .maybe("MediaRef")?
                    .filter(|reference| !reference.is_empty());
                let master = target.and_then(|reference| context.masters.get(reference));
                let mut row = vec![
                    ("id".into(), text(node.id()?)),
                    ("class".into(), text(&node.name)),
                    ("media_ref".into(), target.map_or(Value::Null, text)),
                    (
                        "media_pool_ref_exists".into(),
                        Value::Bool(target.is_some_and(|id| context.pool_items.contains(id))),
                    ),
                    (
                        "source_item".into(),
                        match (target, master) {
                            (Some(id), Some(_)) => text(id),
                            _ => Value::Null,
                        },
                    ),
                    (
                        "source_item_rate".into(),
                        master.map_or(Value::Null, |master| text(&master.rate.to_string())),
                    ),
                ];
                for (tag, key) in [
                    ("Flags", "flags"),
                    ("Start", "saved_start"),
                    ("Duration", "saved_duration"),
                    ("In", "saved_in"),
                    ("MediaStartTime", "saved_media_start_time"),
                    ("MediaFrameRate", "saved_media_frame_rate"),
                ] {
                    if let Some(raw) = node.maybe(tag)? {
                        row.push((key.into(), text(raw)));
                    }
                }
                if let Some(saved) = node.maybe("MediaTimemapBA")? {
                    row.push((
                        "saved_time_map".into(),
                        observations::saved_bytes(saved, &mut context.blobs)?,
                    ));
                }
                if kind == StreamKind::Audio {
                    row.push((
                        "audio_selector".into(),
                        selector(node, master, &mut context.blobs)?,
                    ));
                }
                Ok(Value::Obj(row))
            })
            .collect::<Result<Vec<_>>>()?;
        self.rows.push(Value::Obj(vec![
            ("track_id".into(), text(id)),
            ("kind".into(), text(kind.as_str())),
            ("angle_id".into(), text(angle)),
            ("flags".into(), text(track.get("Flags")?)),
            ("subtype".into(), text(track.get("SubType")?)),
            ("members".into(), Value::Arr(members)),
        ]));
        Ok(())
    }

    pub(super) fn publish(
        &mut self,
        id: &str,
        clip: &Node,
        grid: Grid,
        input_scaling: Option<&str>,
        graph: &mut SourceGraph,
        blobs: &mut Blobs,
    ) -> Result<bool> {
        let audio_transport = transport(clip, blobs)?;
        let outputs = clip.maybe("VirtualAudioTracksBA")?;
        let audio_source = clip.maybe("AudioSource")?;
        let mono_transport = match outputs {
            Some(saved)
                if !saved.is_empty()
                    && saved.len() <= MAX_SELECTOR_HEX
                    && audio_source == Some("AUDIO_SOURCE_EMBEDDED")
                    && self_mono_transport(&audio_transport, id) =>
            {
                mono_output(&blobs.qt(saved)?)?
            }
            _ => false,
        };
        let mut facts = vec![
            ("tracks".into(), Value::Arr(std::mem::take(&mut self.rows))),
            ("frame_size".into(), Value::Arr(vec![Value::Int(grid.size.0), Value::Int(grid.size.1)])),
            (
                "record_origin".into(),
                text(&seconds(grid.origin, grid.rate)?.to_string()),
            ),
            (
                "audio_source".into(),
                audio_source.map_or(Value::Null, text),
            ),
            ("audio_transport".into(), audio_transport),
        ];
        if let Some(mode) = input_scaling {
            facts.push((crate::input_scaling::PROJECT_SETTING.into(), text(mode)));
        }
        if let Some(saved) = outputs {
            facts.push((
                "saved_virtual_outputs".into(),
                observations::saved_bytes(saved, blobs)?,
            ));
        }
        graph.sidecars.push(Sidecar {
            adapter: ADAPTER.into(),
            object: id.into(),
            data: Value::Obj(facts),
        });
        Ok(mono_transport)
    }

    pub(super) fn bind_use(
        &self,
        kind: StreamKind,
        angle: Option<&str>,
        node: &Node,
        facts: &mut Vec<(&str, Value)>,
        blobs: &mut Blobs,
    ) -> Result<()> {
        let tracks = if kind == StreamKind::Video {
            &self.picture
        } else {
            &self.audio
        };
        let matched = angle
            .and_then(|name| tracks.get(name))
            .and_then(Option::as_deref);
        facts.push(("saved_angle_track_id", matched.map_or(Value::Null, text)));
        if kind == StreamKind::Audio {
            // This selector addresses the Group output. The angle member has a
            // separate selector addressing the saved source cache.
            facts.push(("saved_group_output_selector", selector(node, None, blobs)?));
        }
        Ok(())
    }
}

fn mono_output(routes: &encoding::Map) -> Result<bool> {
    Ok(routes.keys().eq(["0"])
        && embedded_audio::channels(&qt(routes.bytes("0")?)?)? == Some(&[1][..]))
}

fn self_mono_transport(transport: &Value, group: &str) -> bool {
    let Value::Arr(slots) = transport else {
        return false;
    };
    let [slot] = slots.as_slice() else {
        return false;
    };
    let Some(Value::Arr(channels)) = slot.get("channels") else {
        return false;
    };
    let [channel] = channels.as_slice() else {
        return false;
    };
    slot.get("slot") == Some(&text("0"))
        && slot.get("type") == Some(&Value::Int(257))
        && channel.get("media_ref") == Some(&text(group))
        && channel.get("channel_key") == Some(&text("0"))
        && matches!(
            channel.get("saved_channel_index"),
            Some(Value::Null | Value::Int(0))
        )
        && matches!(
            channel.get("saved_track_index"),
            Some(Value::Null | Value::Int(0))
        )
}

fn selector(node: &Node, master: Option<&Master>, blobs: &mut Blobs) -> Result<Value> {
    let slot = node.maybe("MediaTrackIdx")?;
    let mut facts = vec![("saved_slot".into(), slot.map_or(Value::Null, text))];
    let Some(encoded) = node
        .maybe("VirtualAudioTrackBA")?
        .filter(|saved| !saved.is_empty())
    else {
        return Ok(Value::Obj(facts));
    };
    if encoded.len() > MAX_SELECTOR_HEX {
        facts.push((
            "saved_selector".into(),
            observations::saved_bytes(encoded, blobs)?,
        ));
        return Ok(Value::Obj(facts));
    }
    let route = blobs.qt(encoded)?;
    facts.push(("saved_hex".into(), text(encoded)));
    facts.push(("audio_type".into(), Value::Int(route.int("AudioType")?)));
    facts.push((
        "saved_channel_mask".into(),
        text(&hex::encode(route.bytes("ChannelsBA")?)),
    ));
    if let Some(channels) = embedded_audio::channels(&route)? {
        if let Some(master) = master.filter(|master| !master.external_audio && slot == Some("0")) {
            let known_layout =
                master.stereo_pool_route || master.audio.len() == 1 && channels == [1];
            if known_layout
                && channels
                    .iter()
                    .all(|channel| master.audio.get(&(channel - 1)) == Some(&(channel - 1)))
            {
                facts.push((
                    "source_channels".into(),
                    Value::Arr(channels.iter().copied().map(Value::Int).collect()),
                ));
            }
        }
    }
    Ok(Value::Obj(facts))
}

fn transport(clip: &Node, blobs: &mut Blobs) -> Result<Value> {
    let Some(encoded) = clip.maybe("FieldsBlob")?.filter(|value| !value.is_empty()) else {
        return Ok(Value::Null);
    };
    let fields = blobs.envelope(encoded)?;
    let Some(pool) = pb_bytes(&fields, 1)? else {
        return Ok(Value::Null);
    };
    let Some(metadata) = pb_bytes(pool, 1)? else {
        return Ok(Value::Null);
    };
    let Some(bytes) = pb_bytes(metadata, 5)? else {
        return Ok(Value::Null);
    };
    let tracks = qt(bytes)?;
    let mut slots = Vec::new();
    for key in tracks.keys() {
        let track = qt(tracks.bytes(key)?)?;
        let channels = qt(track.bytes("ChannelVecBA")?)?;
        let rows = channels
            .keys()
            .map(|key| {
                let entry = qt(channels.bytes(key)?)?;
                let saved = ["ChannelIdx", "TrackIdx"].map(|field| {
                    if entry.keys().any(|key| key == field) {
                        entry.int(field).map(Value::Int)
                    } else {
                        Ok(Value::Null)
                    }
                });
                let [channel, track] = saved;
                Ok(Value::Obj(vec![
                    ("channel_key".into(), text(key)),
                    (
                        "media_ref".into(),
                        entry.text("MediaRef")?.map_or(Value::Null, text),
                    ),
                    ("saved_channel_index".into(), channel?),
                    ("saved_track_index".into(), track?),
                ]))
            })
            .collect::<Result<Vec<_>>>()?;
        slots.push(Value::Obj(vec![
            ("slot".into(), text(key)),
            ("type".into(), Value::Int(track.int("Type")?)),
            ("channels".into(), Value::Arr(rows)),
        ]));
    }
    Ok(Value::Arr(slots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_transport_requires_the_group_own_channel_zero() {
        let transport = |media_ref, channel, kind| {
            Value::Arr(vec![Value::Obj(vec![
                ("slot".into(), text("0")),
                ("type".into(), Value::Int(kind)),
                (
                    "channels".into(),
                    Value::Arr(vec![Value::Obj(vec![
                        ("media_ref".into(), text(media_ref)),
                        ("channel_key".into(), text("0")),
                        ("saved_channel_index".into(), channel),
                        ("saved_track_index".into(), Value::Null),
                    ])]),
                ),
            ])])
        };
        assert!(self_mono_transport(
            &transport("group", Value::Null, 257),
            "group"
        ));
        assert!(!self_mono_transport(
            &transport("other", Value::Null, 257),
            "group"
        ));
        assert!(!self_mono_transport(
            &transport("group", Value::Int(1), 257),
            "group"
        ));
        assert!(!self_mono_transport(
            &transport("group", Value::Null, 258),
            "group"
        ));
    }
}
