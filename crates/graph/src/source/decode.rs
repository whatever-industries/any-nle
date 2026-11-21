// Read graph/2 with the shared strict JSON parser.
use super::*;

fn parse_original(v: &Value) -> R<OriginalStream> {
    let o = Obj::new(v,"original stream", &["saved_locators","container","stream_index","time_base","start_pts","duration_ts"], &["channel_index","channels","sample_representation"])?;
    Ok(OriginalStream {
        saved_locators: o.list("saved_locators")?.iter().map(|v|text(v,"saved locator")).collect::<R<_>>()?,
        container:o.str("container")?, stream_index:o.int("stream_index")?, channel_index:o.opt_int("channel_index")?, channels:o.opt_int("channels")?,
        time_base:o.time("time_base")?,start_pts:o.int("start_pts")?, duration_ts:o.int("duration_ts")?, sample_representation:o.opt_str("sample_representation")?,
    })
}

fn parse_clock(v: &Value, w: &str) -> R<SourceClock> {
    fn count(v: &Value, w: &str) -> R<u64> {
        let Value::Str(s) = v else {
            return fail(format!("{w}: expected canonical decimal u64 string"));
        };
        if s.is_empty()
            || s.len() > 20
            || (s.len() > 1 && s.starts_with('0'))
            || !s.bytes().all(|c| c.is_ascii_digit())
        {
            return fail(format!("{w}: expected canonical decimal u64 string"));
        }
        s.parse()
            .map_err(|_| GraphError(format!("{w}: count exceeds u64")))
    }
    let c = Obj::new(
        v,
        w,
        &[
            "effective_count",
            "rate",
            "display_base",
            "drop",
            "provenance",
        ],
        &[],
    )?;
    let p = Obj::new(
        c.req("provenance"),
        &c.at("provenance"),
        &[
            "origin_count",
            "source_offset",
            "source_rate",
            "source_package",
            "timecode_track",
            "route",
        ],
        &[],
    )?;
    Ok(SourceClock {
        effective_count: count(c.req("effective_count"), &c.at("effective_count"))?,
        rate: c.time("rate")?,
        display_base: u16::try_from(c.int("display_base")?)
            .map_err(|_| GraphError(format!("{w}: display base outside u16")))?,
        drop: c.opt_bool("drop")?.expect("required"),
        provenance: SourceClockProvenance {
            origin_count: count(p.req("origin_count"), &p.at("origin_count"))?,
            source_offset: p.int("source_offset")?,
            source_rate: p.time("source_rate")?,
            source_package: p.str("source_package")?,
            timecode_track: p.str("timecode_track")?,
            route: p.str("route")?,
        },
    })
}

pub(super) fn from_text(text: &str) -> R<SourceGraph> {
    if text.len() > MAX_SOURCE_GRAPH_BYTES {
        return fail("source graph: file too large");
    }
    let mut value = json::parse(text).map_err(GraphError)?;
    check_decode(&value)?;
    // The decoded graph can take ownership of the opaque payloads when
    // the parser owns the input. Keep their field in place for the shared
    // top-level shape checks.
    let rows = match &mut value {
        Value::Obj(fields) => fields
            .iter_mut()
            .find(|(key, _)| key == "sidecars")
            .map(|(_, rows)| std::mem::replace(rows, Value::Null))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    };
    decode(&value, |_| decode_owned_sidecars(rows))
}

pub(super) fn from_value(v: &Value) -> R<SourceGraph> {
    check_decode(v)?;
    decode(v, |object| {
        object.opt_list("sidecars")?
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let (adapter, object, data) = sidecar_fields(index, row)?;
                Ok(Sidecar { adapter, object, data: data.clone() })
            })
            .collect()
    })
}

fn decode(v: &Value, read_sidecars: impl FnOnce(&Obj<'_>) -> R<Vec<Sidecar>>) -> R<SourceGraph> {
    let o = Obj::new(
        v,
        "source graph",
        &["schema", "project", "media", "items", "bins", "sequences"],
        &["losses", "sidecars", "multicams", "multicam_uses"],
    )?;
    if o.str("schema")? != SCHEMA {
        return fail(format!("source graph.schema: expected {SCHEMA}"));
    }
    let project = Obj::new(o.req("project"), "source graph.project", &["name"], &[])?;
    let mut media = Vec::new();
    for (i, m) in o.list("media")?.iter().enumerate() {
        let m = Obj::new(
            m,
            &format!("source graph.media[{i}]"),
            &["id", "kind", "locations", "duration"],
            &[
                "codec",
                "frame_size",
                "rate",
                "sample_rate",
                "bits",
                "start",
                "clock",
                "stream",
                "online",
                "original",
            ],
        )?;
        media.push(SourceMedia {
            id: m.str("id")?,
            kind: m.kind("kind")?,
            locations: m
                .list("locations")?
                .iter()
                .map(|p| text(p, &m.at("locations")))
                .collect::<R<_>>()?,
            codec: m.opt_str("codec")?,
            frame_size: m.opt_size("frame_size")?,
            rate: m.opt_time("rate")?,
            sample_rate: m.opt_int("sample_rate")?,
            bits: m.opt_int("bits")?,
            duration: m.time("duration")?,
            start: m.opt_time("start")?,
            clock: m
                .opt("clock")
                .map(|v| parse_clock(v, &m.at("clock")))
                .transpose()?,
            stream: m.opt_int("stream")?,
            online: m.opt_bool("online")?,
            original: m.opt("original").map(parse_original).transpose()?,
        });
    }
    let mut items = Vec::new();
    for (i, item) in o.list("items")?.iter().enumerate() {
        let w = format!("source graph.items[{i}]");
        let item = Obj::new(
            item,
            &w,
            &["id", "name", "media"],
            &[
                "umid",
                "tape",
                "rate",
                "color_rgb16",
                "stream_groups",
                "markers",
            ],
        )?;
        let mut uses = Vec::new();
        for (j, m) in item.list("media")?.iter().enumerate() {
            let m = Obj::new(
                m,
                &format!("{w}.media[{j}]"),
                &["media"],
                &["gain_db", "source_offset"],
            )?;
            uses.push(SourceItemMedia {
                media: m.str("media")?,
                gain_db: m.opt_float("gain_db")?.unwrap_or(0.0),
                source_offset: m.opt_time("source_offset")?,
            });
        }
        let color_rgb16 = if let Some(v) = item.opt("color_rgb16") {
            let ns = ints(v, &item.at("color_rgb16"))?;
            if ns.len() != 3 {
                return fail(format!("{w}.color_rgb16: expected [r,g,b]"));
            }
            Some([ns[0], ns[1], ns[2]])
        } else {
            None
        };
        items.push(SourceItem {
            id: item.str("id")?,
            name: item.str("name")?,
            media: uses,
            umid: item.opt_str("umid")?,
            tape: item.opt_str("tape")?,
            rate: item.opt_time("rate")?,
            color_rgb16,
            stream_groups: item
                .opt_list("stream_groups")?
                .iter()
                .map(|g| ints(g, &item.at("stream_groups")))
                .collect::<R<_>>()?,
            markers: parse_markers(&item)?,
        });
    }
    let mut bins = Vec::new();
    for (i, b) in o.list("bins")?.iter().enumerate() {
        let b = Obj::new(
            b,
            &format!("source graph.bins[{i}]"),
            &["id", "name", "items"],
            &["parent"],
        )?;
        bins.push(Bin {
            id: b.str("id")?,
            name: b.str("name")?,
            parent: b.opt_str("parent")?,
            items: b
                .list("items")?
                .iter()
                .map(|v| text(v, &b.at("items")))
                .collect::<R<_>>()?,
        });
    }
    let mut sequences = Vec::new();
    for (i, s) in o.list("sequences")?.iter().enumerate() {
        let w = format!("source graph.sequences[{i}]");
        let s = Obj::new(
            s,
            &w,
            &["id", "name", "rate", "placements"],
            &[
                "frame_size",
                "start",
                "clock",
                "tracks",
                "disabled_placements",
                "transitions",
                "markers",
                "texts",
            ],
        )?;
        let mut tracks = Vec::new();
        for (j, t) in s.opt_list("tracks")?.iter().enumerate() {
            let t = Obj::new(
                t,
                &format!("{w}.tracks[{j}]"),
                &["kind", "index"],
                &["level_db", "name", "empty"],
            )?;
            tracks.push(SourceTrack {
                kind: t.kind("kind")?,
                index: t.int("index")?,
                level_db: t.opt_float("level_db")?,
                name: t.opt_str("name")?,
                empty: t.opt_bool("empty")?.unwrap_or(false),
            });
        }
        let placements = s
            .list("placements")?
            .iter()
            .enumerate()
            .map(|(j, p)| super::placement(p, &format!("{w}.placements[{j}]")))
            .collect::<R<_>>()?;
        let mut transitions = Vec::new();
        for (j, t) in s.opt_list("transitions")?.iter().enumerate() {
            let tw = format!("{w}.transitions[{j}]");
            let t = Obj::new(
                t,
                &tw,
                &[
                    "id",
                    "kind",
                    "track",
                    "cut",
                    "duration",
                    "cut_offset",
                    "style",
                    "outgoing",
                    "incoming",
                ],
                &[],
            )?;
            transitions.push(SourceTransition {
                id: t.str("id")?,
                kind: t.kind("kind")?,
                track: t.int("track")?,
                cut: t.time("cut")?,
                duration: t.time("duration")?,
                cut_offset: t.time("cut_offset")?,
                style: t.str("style")?,
                outgoing: parse_binding(t.req("outgoing"), &t.at("outgoing"))?,
                incoming: parse_binding(t.req("incoming"), &t.at("incoming"))?,
            });
        }
        sequences.push(SourceSequence {
            id: s.str("id")?,
            name: s.str("name")?,
            rate: s.time("rate")?,
            frame_size: s.opt_size("frame_size")?,
            start: s.opt_time("start")?,
            clock: s
                .opt("clock")
                .map(|v| parse_clock(v, &s.at("clock")))
                .transpose()?,
            tracks,
            placements,
            disabled_placements: s.opt_list("disabled_placements")?.iter().enumerate()
                .map(|(j, p)| super::placement(p, &format!("{w}.disabled_placements[{j}]")))
                .collect::<R<_>>()?,
            transitions,
            markers: parse_markers(&s)?,
            texts: s.opt_list("texts")?.iter().enumerate().map(|(j, v)| {
                let t = Obj::new(v, &format!("{w}.texts[{j}]"), &["id", "kind", "track"], &["content", "record_start", "duration", "reason"])?;
                let kind = match t.str("kind")?.as_str() {
                    "caption" => TextKind::Caption, "graphic" => TextKind::Graphic,
                    _ => return fail(format!("{w}: unknown text kind")),
                };
                Ok(SourceText { id: t.str("id")?, kind, track: t.int("track")?, content: t.opt_str("content")?, record_start: t.opt_time("record_start")?, duration: t.opt_time("duration")?, reason: t.opt_str("reason")? })
            }).collect::<R<_>>()?,
        });
    }
    let mut losses = Vec::new();
    for (i, l) in o.opt_list("losses")?.iter().enumerate() {
        let l = Obj::new(
            l,
            &format!("source graph.losses[{i}]"),
            &["object", "aspect", "reason"],
            &[],
        )?;
        losses.push(Loss {
            object: l.str("object")?,
            aspect: l.str("aspect")?,
            reason: l.str("reason")?,
        });
    }
    let sidecars = read_sidecars(&o)?;
    let source = SourceGraph {
        project: Project {
            name: project.str("name")?,
        },
        media,
        items,
        bins,
        sequences,
        losses,
        sidecars,
        multicams: o.opt_list("multicams")?.iter().enumerate().map(|(i,v)|Multicam::parse(v,&format!("source graph.multicams[{i}]"))).collect::<R<_>>()?,
        multicam_uses: o.opt_list("multicam_uses")?.iter().enumerate().map(|(i,v)|MulticamUse::parse(v,&format!("source graph.multicam_uses[{i}]"))).collect::<R<_>>()?,
    };
    source.validate()?;
    Ok(source)
}

fn check_decode(value: &Value) -> R<()> {
    // Check nested shapes and encoded size before constructing typed records.
    crate::validation::value(value, 0, "source graph")?;
    super::encode::check_value_size(value)
}

fn sidecar_fields(index: usize, row: &Value) -> R<(String, String, &Value)> {
    let row = Obj::new(
        row,
        &format!("source graph.sidecars[{index}]"),
        &["adapter", "object"],
        &["data"],
    )?;
    Ok((row.str("adapter")?, row.str("object")?, row.raw("data")?))
}

fn decode_owned_sidecars(rows: Value) -> R<Vec<Sidecar>> {
    let rows = match rows {
        Value::Null => return Ok(Vec::new()),
        Value::Arr(rows) if rows.len() <= MAX_LIST => rows,
        _ => return fail(format!("source graph.sidecars: expected a list of at most {MAX_LIST}")),
    };
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let (adapter, object, _) = sidecar_fields(index, &row)?;
            let Value::Obj(fields) = row else { unreachable!("validated sidecar row") };
            let data = fields.into_iter()
                .find_map(|(key, data)| (key == "data").then_some(data))
                .expect("validated sidecar data");
            Ok(Sidecar { adapter, object, data })
        })
        .collect()
}

pub(super) fn parse_binding(value: &Value, w: &str) -> R<SourceBinding> {
    let b = Obj::new(value, w, &["placements"], &["reason"])?;
    Ok(SourceBinding {
        placements: b
            .list("placements")?
            .iter()
            .map(|v| text(v, &b.at("placements")))
            .collect::<R<_>>()?,
        reason: b.opt_str("reason")?,
    })
}

fn parse_markers(o: &Obj) -> R<Vec<SourceMarker>> {
    let mut result = Vec::new();
    for (i, m) in o.opt_list("markers")?.iter().enumerate() {
        let w = format!("{}.markers[{i}]", o.w);
        let m = Obj::new(
            m,
            &w,
            &["name", "duration", "owner", "owner_occurrence", "binding"],
            &[
                "comment",
                "user",
                "color",
                "start",
                "track",
                "owner_local",
                "owner_rate",
                "source",
                "inherited",
            ],
        )?;
        let color = match m.opt("color") {
            None => None,
            Some(v) => {
                let v = Obj::new(v, &m.at("color"), &["palette", "name"], &[])?;
                Some(Label {
                    palette: v.str("palette")?,
                    name: v.str("name")?,
                })
            }
        };
        let track = match m.opt("track") {
            None => None,
            Some(v) => {
                let v = Obj::new(v, &m.at("track"), &["kind", "index"], &[])?;
                Some(TrackRef {
                    kind: v.kind("kind")?,
                    index: v.int("index")?,
                })
            }
        };
        let owner = MarkerOwner::parse(&m.str("owner")?)
            .ok_or_else(|| GraphError(format!("{w}: unknown marker owner")))?;
        result.push(SourceMarker {
            name: m.str("name")?,
            comment: m.opt_str("comment")?.unwrap_or_default(),
            user: m.opt_str("user")?.unwrap_or_default(),
            color,
            start: m.opt_time("start")?,
            duration: m.time("duration")?,
            track,
            owner,
            owner_occurrence: m.str("owner_occurrence")?,
            owner_local: m.opt_time("owner_local")?,
            owner_rate: m.opt_time("owner_rate")?,
            binding: parse_binding(m.req("binding"), &m.at("binding"))?,
            source: m.opt_time("source")?,
            inherited: m.opt_bool("inherited")?.unwrap_or(false),
        });
    }
    Ok(result)
}
