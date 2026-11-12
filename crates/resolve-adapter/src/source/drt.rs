//! Internal inspection of the native single-timeline 21.2 export. All facts
//! come from the supplied archive; saved locators are never opened.
use super::*;
use serde_json::{json, Value as Json};
use std::io::Write;

pub struct Inspection {
    report: Json,
    graph: Option<SourceGraph>,
}

impl Inspection {
    pub fn write_json(&self, writer: &mut impl Write) -> Result<()> {
        report::write(writer, &self.report)
    }

    pub fn into_graph(self) -> Result<SourceGraph> {
        self.graph.ok_or_else(|| {
            "Resolve timeline falls outside the qualified graph/2 profile; use --inspect-drt for the saved facts and gaps".into()
        })
    }

    pub(super) fn take_gaps(&mut self) -> Vec<Json> {
        match self.report["gaps"].take() {
            Json::Array(gaps) => gaps,
            _ => Vec::new(),
        }
    }
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    let members = xml::read(path)?;
    inspect_members(&members)
}

pub(super) fn inspect_members(members: &BTreeMap<String, Node>) -> Result<Inspection> {
    let containers = containers(members)?;
    let [(id, container)] = containers
        .into_iter()
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "expected one native Resolve timeline sequence container")?;
    check_reference(id)?;
    if members
        .keys()
        .any(|name| name.starts_with("MediaPool/") && name != ROOT_FOLDER)
    {
        return Err("native Resolve timeline inspection expects a flat export media pool".into());
    }

    let mut gaps = Vec::new();
    let project = members.get("project.xml");
    let root = members.get(ROOT_FOLDER);
    if project.is_none() {
        gaps.push(json!({"fact":"project_metadata", "reason":"missing project.xml; timeline registration and marker data unavailable"}));
    }
    if root.is_none() {
        gaps.push(json!({"fact":"media_pool", "reason":"missing saved media pool; timeline handle, cache locators and physical channels unresolved"}));
    }
    let saved = root
        .map(|root| saved_timeline(root, id))
        .transpose()?
        .flatten();
    if let Some(project) = project {
        expect(project, "SM_Project")?;
        same(project.get("ProjectVersion")?, "17", "project version")?;
        let mut registered = project.one("TimelineHandleVec")?.elements()?;
        let entry = registered
            .next()
            .ok_or("missing Resolve timeline registration")?;
        let handle = entry.scalar()?.trim();
        if registered.next().is_some() {
            return Err("expected one saved Resolve timeline registration".into());
        }
        check_reference(handle)?;
        if let Some((witness, _)) = saved {
            same(handle, witness.id()?, "timeline registration")?;
        }
    }
    let mut timeline = json!({"id":id, "clock":null, "markers":null});
    let mut has_grid = false;
    if let Some((handle, sequence)) = saved {
        let fields = ["FrameRate", "MediaExtents", "Resolution"];
        let mut missing = Vec::new();
        for field in fields {
            if sequence.maybe(field)?.is_none_or(str::is_empty) {
                missing.push(field);
            }
        }
        if !missing.is_empty() {
            gaps.push(json!({"fact":"timeline_grid", "reason":format!("missing saved Resolve fields: {}", missing.join(", "))}));
        }
        let value = |tag| -> Result<Option<Vec<u8>>> {
            sequence
                .maybe(tag)?
                .filter(|s| !s.is_empty())
                .map(hex_bytes)
                .transpose()
        };
        let rate = value("FrameRate")?.as_deref().map(rate).transpose()?;
        let (clock, length) = match (rate, value("MediaExtents")?) {
            (Some(rate), Some(saved)) => {
                let (origin, length) = extent(&saved, rate)?;
                (
                    Some(clock(id, "Sm2Sequence.MediaExtents", origin, rate)?),
                    Some(length),
                )
            }
            _ => (None, None),
        };
        let canvas = value("Resolution")?.as_deref().map(raster).transpose()?;
        has_grid = clock.is_some() && canvas.is_some();
        timeline = json!({
            "id":id, "name":handle.maybe("Name")?, "rate":rate.map(|r| r.to_string()),
            "canvas":canvas, "clock":clock.as_ref().map(clock_json),
            "duration_frames":length, "markers":null,
        });
    } else if root.is_some() {
        gaps.push(
            json!({"fact":"timeline_handle", "reason":"missing saved Resolve timeline handle"}),
        );
    }
    let mut graph = if project.is_some() && has_grid {
        Some(read_members(members, true)?)
    } else {
        None
    };
    let sequence = graph
        .as_ref()
        .map(|g| match g.sequences.as_slice() {
            [sequence] if sequence.id == id => Ok(sequence),
            _ => Err("native Resolve timeline registration disagrees with the container"),
        })
        .transpose()?;
    let placements: HashMap<_, _> = sequence
        .iter()
        .flat_map(|s| &s.placements)
        .map(|p| (p.id.as_str(), p))
        .collect();

    let mut rows = Vec::new();
    let mut identities = HashSet::new();
    let mut counts = [0i64; 2];
    let mut edit_count = 0;
    for (kind, track) in tracks(container)? {
        claim(&mut identities, track.id()?)?;
        let lane = if kind == StreamKind::Video { 0 } else { 1 };
        counts[lane] += 1;
        for entry in track.one("Items")?.elements()? {
            let item = entry.entry()?;
            let occurrence = item.id()?;
            claim(&mut identities, occurrence)?;
            let transition = item.name == "Sm2TiTransition";
            let placed = placements.get(occurrence);
            if !transition {
                edit_count += 1;
                if placed.is_none() {
                    gaps.push(json!({"fact":"resolved_edit", "occurrence":occurrence,
                        "reason":"no qualified placement; see saved fields and occurrence diagnostics"}));
                }
            }
            let saved = [
                ("Start", "start"),
                ("Duration", "duration"),
                ("In", "source_in"),
                ("MediaRef", "media_ref"),
                ("MediaFilePath", "timeline_locator"),
                ("MediaStartTime", "media_start_seconds"),
                ("MediaTrackIdx", "media_track_index"),
                ("PrettyType", "type_label"),
                ("AlignmentType", "alignment_type"),
                ("Position", "position"),
            ]
            .into_iter()
            .map(|(tag, key)| Ok((key, item.maybe(tag)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
            if let Some(media) = saved["media_ref"].filter(|value| !value.is_empty()) {
                check_reference(media)?;
            }
            rows.push(json!({
                "occurrence":occurrence, "class":item.name, "track_id":track.id()?,
                "kind":if lane == 0 {"video"} else {"audio"}, "track":counts[lane], "saved":saved,
                "resolved":placed.map(|p| json!({"record_start":p.record_start.to_string(),
                    "source_in":p.source_in.to_string(), "duration":p.duration.to_string(),
                    "media_item":p.item, "audio_streams":p.audio_streams})),
            }));
        }
    }
    let mut safe = false;
    let mut transitions = Json::Null;
    let mut media = Vec::new();
    if let (Some(g), Some(s)) = (graph.as_ref(), sequence) {
        let profile = crate::dissolve::prepare(g, s).ok().flatten();
        let wav_ids: HashSet<_> = g
            .media
            .iter()
            .filter(|m| is_wav(m))
            .map(|m| m.id.as_str())
            .collect();
        let eligible_edits = s.placements.iter().all(|p| {
            if p.video_track.is_some() {
                p.video_track == Some(1)
                    && p.audio_tracks.is_empty()
                    && p.audio_streams.is_empty()
                    && p.transform.is_none()
                    && p.effects.is_empty()
            } else {
                matches!(p.audio_tracks.as_slice(), [1] | [2])
                    && matches!(p.audio_streams.as_slice(), [1] | [2])
                    && p.audio_streams.iter().all(|selector| {
                        wav_ids.contains(format!("{}/audio/{selector}", p.item).as_str())
                    })
                    && p.gain_db == 0.0
                    && p.volume_db.is_none()
                    && p.volume_keys.is_empty()
            }
        });
        let video_count = s
            .placements
            .iter()
            .filter(|p| p.video_track.is_some())
            .count();
        let audio = s
            .placements
            .iter()
            .filter(|p| p.video_track.is_none())
            .collect::<Vec<_>>();
        let separated = matches!(audio.as_slice(), [a, b]
            if a.audio_tracks == [1] && b.audio_tracks == [2]
                && a.item == b.item && a.audio_streams != b.audio_streams);
        safe = s.rate == crate::prepare::rate()
            && s.frame_size == Some((1920, 1080))
            && s.clock.as_ref().is_some_and(|clock| !clock.drop)
            && counts == [1, 2]
            && video_count == 2
            && separated
            && edit_count == s.placements.len()
            && eligible_edits
            && !g.media.is_empty()
            && g.media.iter().all(|m| {
                !m.locations.is_empty()
                    && m.start.is_some()
                    && match m.kind {
                        StreamKind::Video => {
                            m.codec.as_deref() == Some("apch")
                                && m.rate == Some(s.rate)
                                && m.frame_size == s.frame_size
                        }
                        StreamKind::Audio => is_wav(m),
                    }
            })
            && g.multicams.is_empty()
            && g.losses
                .iter()
                .all(|loss| profile.as_ref().is_some_and(|p| p.recover(loss, &s.id)))
            && profile.is_some();
        for loss in &g.losses {
            if !profile.as_ref().is_some_and(|p| p.recover(loss, &s.id)) {
                gaps.push(json!({"fact":loss.aspect, "owner":loss.object, "reason":loss.reason}));
            }
        }
        for m in &g.media {
            media.push(json!({
                "id":m.id, "kind":if m.kind == StreamKind::Video {"video"} else {"audio"},
                "codec":m.codec, "saved_cache_locators":m.locations, "online":null,
                "rate":m.rate.map(|v| v.to_string()), "sample_rate":m.sample_rate,
                "bits":m.bits, "start":m.start.map(|v| v.to_string()),
                "duration":m.duration.to_string(), "stream":m.stream,
                "saved_timecode_count":m.clock.as_ref().map(|clock| clock.effective_count),
            }));
        }
        transitions = g
            .sidecar(crate::dissolve::ADAPTER, id)
            .map(receipt_json)
            .unwrap_or(Json::Null);
        let markers = s.markers.iter().map(|m| json!({
            "name":m.name,"note":m.comment, "owner":"timeline","owner_occurrence":m.owner_occurrence,
            "start":m.start.map(|v| v.to_string()), "duration":m.duration.to_string(),
            "color":m.color.as_ref().map(|v| v.name.as_str()),
        })).collect::<Vec<_>>();
        timeline["markers"] = json!(markers);
        timeline["native_marker_fields"] = g
            .sidecar(ADAPTER, id)
            .and_then(|v| v.get("markers"))
            .map(receipt_json)
            .unwrap_or(Json::Null);
    }
    if !safe {
        gaps.push(json!({"fact":"graph_profile", "reason":"outside the native-tested 23.976 V1 dissolve and two mono WAV lane profile"}));
        graph = None;
    }
    Ok(Inspection {
        report: json!({
            "schema":"anynle.resolve-drt-inspection/1", "graph_eligible":safe,
            "timeline":timeline, "occurrences":rows, "transitions":transitions, "media":media,
            "gaps":gaps,
        }),
        graph,
    })
}

fn saved_timeline<'a>(root: &'a Node, sequence_id: &str) -> Result<Option<(&'a Node, &'a Node)>> {
    expect(root, "Sm2MpFolder")?;
    let mut found = None;
    for entry in root.one("MediaVec")?.elements()? {
        let clip = entry.entry()?;
        if clip.name != "Sm2MpTimelineClip" {
            continue;
        }
        let handle = clip.one("TimelineSharedHandle")?.entry()?;
        expect(handle, "Sm2Timeline")?;
        same(
            handle.get("MpTimelineClip")?,
            clip.id()?,
            "timeline pool back-reference",
        )?;
        let sequence = handle.one("Sequence")?.entry()?;
        expect(sequence, "Sm2Sequence")?;
        same(
            sequence.get("Parent")?,
            handle.id()?,
            "timeline sequence owner",
        )?;
        same(sequence.id()?, sequence_id, "sequence container reference")?;
        if found.replace((handle, sequence)).is_some() {
            return Err("duplicate Resolve timeline handles in timeline export".into());
        }
    }
    Ok(found)
}

fn clock_json(clock: &SourceClock) -> Json {
    json!({"start_frames":clock.effective_count, "rate":clock.rate.to_string(),
        "display_base":clock.display_base,"drop":clock.drop})
}

fn check_reference(id: &str) -> Result<()> {
    if id.len() != 36 || uuid::Uuid::parse_str(id).is_err() {
        return Err("invalid Resolve timeline object reference".into());
    }
    Ok(())
}

fn is_wav(m: &SourceMedia) -> bool {
    m.kind == StreamKind::Audio
        && m.codec.as_deref() == Some("pcm")
        && m.sample_rate == Some(48000)
        && m.bits == Some(16)
        && m.locations.len() == 1
        && m.locations[0].to_ascii_lowercase().ends_with(".wav")
}

// The inspected native receipts contain integer/string fields only. Do not
// serialize arbitrary graph sidecars or raw blobs through this report.
fn receipt_json(value: &Value) -> Json {
    match value {
        Value::Arr(rows) => Json::Array(rows.iter().map(receipt_json).collect()),
        Value::Obj(fields) => Json::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), receipt_json(v)))
                .collect(),
        ),
        Value::Int(v) => json!(v),
        Value::Str(v) => json!(v),
        _ => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn container_only_keeps_saved_edit_without_guessing_clock_or_channel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-witnesses.drt");
        let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file(
            "SeqContainer/11111111-0000-0000-0000-000000000000.xml",
            SimpleFileOptions::default(),
        )
        .unwrap();
        write!(zip, r#"<?xml version="1.0"?>
<!--DbAppVer="21.2.0.0006" DbPrjVer="17"-->
<Sm2SequenceContainer DbId="11111111-0000-0000-0000-000000000000">
<VideoTrackVec/>
<AudioTrackVec><Element><Sm2TiTrack DbId="22222222-0000-0000-0000-000000000000">
<Sequence>33333333-0000-0000-0000-000000000000</Sequence><Type>1</Type>
<Items><Element><Sm2TiAudioClip DbId="44444444-0000-0000-0000-000000000000">
<Start>86400</Start><Duration>48</Duration><In>3</In><MediaFilePath>/offline/test.wav</MediaFilePath>
<MediaTrackIdx>0</MediaTrackIdx>
</Sm2TiAudioClip></Element></Items></Sm2TiTrack></Element></AudioTrackVec>
</Sm2SequenceContainer>"#).unwrap();
        zip.finish().unwrap();

        let inspection = inspect(&path).unwrap();
        let mut bytes = Vec::new();
        inspection.write_json(&mut bytes).unwrap();
        let json: Json = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["graph_eligible"], false);
        assert!(json["timeline"]["clock"].is_null());
        let edit = &json["occurrences"][0];
        assert_eq!(edit["saved"]["start"], "86400");
        assert_eq!(edit["saved"]["timeline_locator"], "/offline/test.wav");
        assert_eq!(edit["saved"]["media_track_index"], "0");
        assert!(edit["resolved"].is_null());
        assert!(inspection.into_graph().is_err());
    }
}
