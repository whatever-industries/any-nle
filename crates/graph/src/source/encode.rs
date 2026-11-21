// Bounded graph/2 and private receipt encoding.
use super::*;

// Streaming serialization holds no cloned graph/Value tree. A count pass checks
// the complete encoding before a caller's output writer receives its first byte.
struct JsonOut<'a, W: Write> {
    writer: &'a mut W,
    bytes: usize,
}
macro_rules! member {
    ($o:ident,$first:ident,$key:literal,$write:expr) => {{
        $o.field($key, $first)?;
        $write?;
    }};
}
impl<W: Write> JsonOut<'_, W> {
    fn raw(&mut self, text: &str) -> R<()> {
        self.bytes = self
            .bytes
            .checked_add(text.len())
            .filter(|n| *n <= MAX_SOURCE_GRAPH_BYTES)
            .ok_or_else(|| GraphError("source graph encoding exceeds 256 MiB".into()))?;
        self.writer
            .write_all(text.as_bytes())
            .map_err(|e| GraphError(format!("source graph output: {e}")))
    }
    fn text(&mut self, text: &str) -> R<()> {
        self.raw("\"")?;
        let mut start = 0;
        for (i, c) in text.char_indices() {
            let escaped = match c {
                '"' => Some("\\\""),
                '\\' => Some("\\\\"),
                '\n' => Some("\\n"),
                '\r' => Some("\\r"),
                '\t' => Some("\\t"),
                _ => None,
            };
            if let Some(e) = escaped {
                self.raw(&text[start..i])?;
                self.raw(e)?;
                start = i + c.len_utf8();
            } else if (c as u32) < 0x20 {
                self.raw(&text[start..i])?;
                self.raw(&format!("\\u{:04x}", c as u32))?;
                start = i + c.len_utf8();
            }
        }
        self.raw(&text[start..])?;
        self.raw("\"")
    }
    fn field(&mut self, name: &str, first: &mut bool) -> R<()> {
        if !*first {
            self.raw(",")?;
        }
        *first = false;
        self.text(name)?;
        self.raw(":")
    }
    fn object(&mut self, body: impl FnOnce(&mut Self, &mut bool) -> R<()>) -> R<()> {
        self.raw("{")?;
        body(self, &mut true)?;
        self.raw("}")
    }
    fn array<T>(&mut self, items: &[T], mut item: impl FnMut(&mut Self, &T) -> R<()>) -> R<()> {
        self.raw("[")?;
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                self.raw(",")?;
            }
            item(self, v)?;
        }
        self.raw("]")
    }
    fn value(&mut self, v: &Value) -> R<()> {
        match v {
            Value::Null => self.raw("null"),
            Value::Bool(b) => self.raw(if *b { "true" } else { "false" }),
            Value::Int(n) => self.raw(&n.to_string()),
            Value::Float(n) => self.raw(&format!("{n:?}")),
            Value::Str(s) => self.text(s),
            Value::Arr(a) => self.array(a, Self::value),
            Value::Obj(a) => self.object(|o, f| {
                for (k, v) in a {
                    o.field(k, f)?;
                    o.value(v)?;
                }
                Ok(())
            }),
        }
    }
    fn integer(&mut self, n: i64) -> R<()> {
        self.raw(&n.to_string())
    }
    fn float(&mut self, n: f64) -> R<()> {
        self.raw(&format!("{n:?}"))
    }
    fn boolean(&mut self, n: bool) -> R<()> {
        self.raw(if n { "true" } else { "false" })
    }
    fn time(&mut self, n: Rational) -> R<()> {
        self.text(&n.to_string())
    }
    fn optional<T>(&mut self, n: Option<T>, render: impl FnOnce(&mut Self, T) -> R<()>) -> R<()> {
        match n {
            Some(v) => render(self, v),
            None => self.raw("null"),
        }
    }
    fn times(&mut self, n: Option<Rational>) -> R<()> {
        self.optional(n, Self::time)
    }
    fn integers(&mut self, n: &[i64]) -> R<()> {
        self.array(n, |o, n| o.integer(*n))
    }
    fn label(&mut self, c: &Label) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "palette", o.text(&c.palette));
            member!(o, f, "name", o.text(&c.name));
            Ok(())
        })
    }
    fn track(&mut self, t: TrackRef) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "kind", o.text(t.kind.as_str()));
            member!(o, f, "index", o.integer(t.index));
            Ok(())
        })
    }
    fn binding(&mut self, b: &SourceBinding) -> R<()> {
        self.object(|o, f| {
            member!(
                o,
                f,
                "placements",
                o.array(&b.placements, |o, id| o.text(id))
            );
            member!(o, f, "reason", o.optional(b.reason.as_deref(), Self::text));
            Ok(())
        })
    }
    fn marker(&mut self, m: &SourceMarker) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "name", o.text(&m.name));
            member!(o, f, "comment", o.text(&m.comment));
            member!(o, f, "user", o.text(&m.user));
            member!(o, f, "color", o.optional(m.color.as_ref(), Self::label));
            member!(o, f, "start", o.times(m.start));
            member!(o, f, "duration", o.time(m.duration));
            member!(o, f, "track", o.optional(m.track, Self::track));
            member!(o, f, "owner", o.text(m.owner.as_str()));
            member!(o, f, "owner_occurrence", o.text(&m.owner_occurrence));
            member!(o, f, "owner_local", o.times(m.owner_local));
            member!(o, f, "owner_rate", o.times(m.owner_rate));
            member!(o, f, "binding", o.binding(&m.binding));
            member!(o, f, "source", o.times(m.source));
            member!(o, f, "inherited", o.boolean(m.inherited));
            Ok(())
        })
    }
    fn media(&mut self, m: &SourceMedia) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&m.id));
            member!(o, f, "kind", o.text(m.kind.as_str()));
            member!(o, f, "locations", o.array(&m.locations, |o, p| o.text(p)));
            member!(o, f, "codec", o.optional(m.codec.as_deref(), Self::text));
            member!(
                o,
                f,
                "frame_size",
                o.optional(m.frame_size, |o, (x, y)| o.integers(&[x, y]))
            );
            member!(o, f, "rate", o.times(m.rate));
            member!(
                o,
                f,
                "sample_rate",
                o.optional(m.sample_rate, Self::integer)
            );
            member!(o, f, "bits", o.optional(m.bits, Self::integer));
            member!(o, f, "duration", o.time(m.duration));
            member!(o, f, "start", o.times(m.start));
            member!(o, f, "clock", o.optional(m.clock.as_ref(), Self::clock));
            member!(o, f, "stream", o.optional(m.stream, Self::integer));
            member!(o, f, "online", o.optional(m.online, Self::boolean));
            if let Some(original) = &m.original { member!(o, f, "original", o.original(original)); }
            Ok(())
        })
    }
    fn original(&mut self, v: &OriginalStream) -> R<()> {
        self.object(|o,f| {
            member!(o,f,"saved_locators",o.array(&v.saved_locators,|o,p|o.text(p)));
            member!(o,f,"container",o.text(&v.container));
            member!(o,f,"stream_index",o.integer(v.stream_index));
            member!(o,f,"channel_index",o.optional(v.channel_index,Self::integer));
            member!(o,f,"channels",o.optional(v.channels,Self::integer));
            member!(o,f,"time_base",o.time(v.time_base));
            member!(o,f,"start_pts",o.integer(v.start_pts));
            member!(o,f,"duration_ts",o.integer(v.duration_ts));
            member!(o,f,"sample_representation",o.optional(v.sample_representation.as_deref(),Self::text));
            Ok(())
        })
    }
    fn item(&mut self, i: &SourceItem) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&i.id));
            member!(o, f, "name", o.text(&i.name));
            member!(
                o,
                f,
                "media",
                o.array(&i.media, |o, m| o.object(|o, f| {
                    member!(o, f, "media", o.text(&m.media));
                    member!(o, f, "gain_db", o.float(m.gain_db));
                    member!(o, f, "source_offset", o.times(m.source_offset));
                    Ok(())
                }))
            );
            member!(o, f, "umid", o.optional(i.umid.as_deref(), Self::text));
            member!(o, f, "tape", o.optional(i.tape.as_deref(), Self::text));
            member!(o, f, "rate", o.times(i.rate));
            member!(
                o,
                f,
                "color_rgb16",
                o.optional(i.color_rgb16, |o, c| o.integers(&c))
            );
            member!(
                o,
                f,
                "stream_groups",
                o.array(&i.stream_groups, |o, g| o.integers(g))
            );
            member!(o, f, "markers", o.array(&i.markers, Self::marker));
            Ok(())
        })
    }
    fn placement(&mut self, p: &Placement) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&p.id));
            member!(o, f, "item", o.text(&p.item));
            member!(o, f, "record_start", o.time(p.record_start));
            member!(o, f, "source_in", o.time(p.source_in));
            member!(o, f, "duration", o.time(p.duration));
            member!(
                o,
                f,
                "video_track",
                o.optional(p.video_track, Self::integer)
            );
            member!(o, f, "audio_tracks", o.integers(&p.audio_tracks));
            member!(o, f, "audio_streams", o.integers(&p.audio_streams));
            member!(
                o,
                f,
                "grouped_audio",
                o.array(&p.grouped_audio, |o, g| o.object(|o, f| {
                    member!(o, f, "track", o.integer(g.track));
                    member!(o, f, "streams", o.integers(&g.streams));
                    member!(o, f, "gain_db", o.optional(g.gain_db, Self::float));
                    member!(
                        o,
                        f,
                        "pans",
                        o.array(&g.pans, |o, p| o.optional(*p, Self::float))
                    );
                    Ok(())
                }))
            );
            member!(o, f, "gain_db", o.float(p.gain_db));
            member!(o, f, "volume_db", o.optional(p.volume_db, Self::float));
            member!(
                o,
                f,
                "volume_keys",
                o.array(&p.volume_keys, |o, k| o.object(|o, f| {
                    member!(o, f, "at", o.time(k.at));
                    member!(o, f, "db", o.float(k.db));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "track_gains_db",
                o.array(&p.track_gains_db, |o, n| o.optional(*n, Self::float))
            );
            member!(
                o,
                f,
                "track_pans",
                o.array(&p.track_pans, |o, n| o.optional(*n, Self::float))
            );
            member!(
                o,
                f,
                "speed",
                o.optional(p.speed.as_ref(), |o, s| o.object(|o, f| {
                    member!(o, f, "ratio", o.integers(&[s.ratio.0, s.ratio.1]));
                    member!(o, f, "source_duration", o.time(s.source_duration));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "frame_map",
                o.optional(p.frame_map.as_ref(), |o, m| o.integers(m))
            );
            member!(
                o,
                f,
                "transform",
                o.optional(p.transform.as_ref(), |o, t| o.object(|o, f| {
                    member!(
                        o,
                        f,
                        "position",
                        o.array(&[t.position.0, t.position.1], |o, n| o.float(*n))
                    );
                    member!(
                        o,
                        f,
                        "scale",
                        o.array(&[t.scale.0, t.scale.1], |o, n| o.float(*n))
                    );
                    member!(o, f, "rotation", o.float(t.rotation));
                    member!(
                        o,
                        f,
                        "anchor",
                        o.array(&[t.anchor.0, t.anchor.1], |o, n| o.float(*n))
                    );
                    member!(o, f, "crop", o.array(&t.crop, |o, n| o.float(*n)));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "effects",
                o.array(&p.effects, |o, e| o.object(|o, f| {
                    member!(o, f, "kind", o.text(&e.kind));
                    member!(o, f, "params", o.value(&e.params));
                    Ok(())
                }))
            );
            Ok(())
        })
    }
    fn sequence(&mut self, s: &SourceSequence) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&s.id));
            member!(o, f, "name", o.text(&s.name));
            member!(o, f, "rate", o.time(s.rate));
            member!(
                o,
                f,
                "frame_size",
                o.optional(s.frame_size, |o, (x, y)| o.integers(&[x, y]))
            );
            member!(o, f, "start", o.times(s.start));
            member!(o, f, "clock", o.optional(s.clock.as_ref(), Self::clock));
            member!(
                o,
                f,
                "tracks",
                o.array(&s.tracks, |o, t| o.object(|o, f| {
                    member!(o, f, "kind", o.text(t.kind.as_str()));
                    member!(o, f, "index", o.integer(t.index));
                    member!(o, f, "level_db", o.optional(t.level_db, Self::float));
                    if let Some(name) = &t.name {
                        member!(o, f, "name", o.text(name));
                    }
                    if t.empty {
                        member!(o, f, "empty", o.boolean(true));
                    }
                    Ok(())
                }))
            );
            member!(o, f, "placements", o.array(&s.placements, Self::placement));
            if !s.disabled_placements.is_empty() {
                member!(o, f, "disabled_placements", o.array(&s.disabled_placements, Self::placement));
            }
            member!(
                o,
                f,
                "transitions",
                o.array(&s.transitions, |o, t| o.object(|o, f| {
                    member!(o, f, "id", o.text(&t.id));
                    member!(o, f, "kind", o.text(t.kind.as_str()));
                    member!(o, f, "track", o.integer(t.track));
                    member!(o, f, "cut", o.time(t.cut));
                    member!(o, f, "duration", o.time(t.duration));
                    member!(o, f, "cut_offset", o.time(t.cut_offset));
                    member!(o, f, "style", o.text(&t.style));
                    member!(o, f, "outgoing", o.binding(&t.outgoing));
                    member!(o, f, "incoming", o.binding(&t.incoming));
                    Ok(())
                }))
            );
            member!(o, f, "markers", o.array(&s.markers, Self::marker));
            if !s.texts.is_empty() {
                member!(o, f, "texts", o.array(&s.texts, |o, t| o.object(|o, f| {
                    member!(o, f, "id", o.text(&t.id));
                    member!(o, f, "kind", o.text(t.kind.as_str()));
                    member!(o, f, "track", o.integer(t.track));
                    member!(o, f, "content", o.optional(t.content.as_deref(), Self::text));
                    member!(o, f, "record_start", o.times(t.record_start));
                    member!(o, f, "duration", o.times(t.duration));
                    member!(o, f, "reason", o.optional(t.reason.as_deref(), Self::text));
                    Ok(())
                })));
            }
            Ok(())
        })
    }
    fn graph(&mut self, g: &SourceGraph) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "schema", o.text(SCHEMA));
            member!(
                o,
                f,
                "project",
                o.object(|o, f| {
                    member!(o, f, "name", o.text(&g.project.name));
                    Ok(())
                })
            );
            member!(o, f, "media", o.array(&g.media, Self::media));
            member!(o, f, "items", o.array(&g.items, Self::item));
            member!(
                o,
                f,
                "bins",
                o.array(&g.bins, |o, b| o.object(|o, f| {
                    member!(o, f, "id", o.text(&b.id));
                    member!(o, f, "name", o.text(&b.name));
                    member!(o, f, "parent", o.optional(b.parent.as_deref(), Self::text));
                    member!(o, f, "items", o.array(&b.items, |o, i| o.text(i)));
                    Ok(())
                }))
            );
            member!(o, f, "sequences", o.array(&g.sequences, Self::sequence));
            if !g.multicams.is_empty() { member!(o, f, "multicams", o.array(&g.multicams, Self::multicam)); }
            if !g.multicam_uses.is_empty() { member!(o, f, "multicam_uses", o.array(&g.multicam_uses, Self::multicam_use)); }
            member!(
                o,
                f,
                "losses",
                o.array(&g.losses, |o, l| o.object(|o, f| {
                    member!(o, f, "object", o.text(&l.object));
                    member!(o, f, "aspect", o.text(&l.aspect));
                    member!(o, f, "reason", o.text(&l.reason));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "sidecars",
                o.array(&g.sidecars, |o, s| o.object(|o, f| {
                    member!(o, f, "adapter", o.text(&s.adapter));
                    member!(o, f, "object", o.text(&s.object));
                    member!(o, f, "data", o.value(&s.data));
                    Ok(())
                }))
            );
            Ok(())
        })
    }
    fn multicam(&mut self, m:&Multicam)->R<()> {
        self.object(|o,f|{
            member!(o,f,"id",o.text(&m.id)); member!(o,f,"name",o.text(&m.name));
            member!(o,f,"rate",o.times(m.rate)); member!(o,f,"duration",o.times(m.duration));
            member!(o,f,"reason",o.optional(m.reason.as_deref(),Self::text));
            member!(o,f,"lanes",o.array(&m.lanes,|o,l|o.object(|o,f|{
                member!(o,f,"kind",o.text(l.kind.as_str())); member!(o,f,"index",o.integer(l.index));
                member!(o,f,"selected",o.optional(l.selected,Self::integer));
                member!(o,f,"reason",o.optional(l.reason.as_deref(),Self::text));
                member!(o,f,"alternatives",o.array(&l.alternatives,|o,a|o.object(|o,f|{
                    member!(o,f,"id",o.text(&a.id)); member!(o,f,"reason",o.optional(a.reason.as_deref(),Self::text));
                    member!(o,f,"pieces",o.array(&a.pieces,|o,p|o.object(|o,f|{
                        member!(o,f,"start",o.time(p.start)); member!(o,f,"duration",o.time(p.duration));
                        member!(o,f,"item",o.optional(p.item.as_deref(),Self::text));
                        member!(o,f,"stream",o.optional(p.stream,Self::integer));
                        member!(o,f,"source_in",o.times(p.source_in)); Ok(())
                    }))); Ok(())
                }))); Ok(())
            }))); Ok(())
        })
    }
    fn multicam_use(&mut self,u:&MulticamUse)->R<()> {
        self.object(|o,f|{
            member!(o,f,"id",o.text(&u.id)); member!(o,f,"sequence",o.text(&u.sequence));
            member!(o,f,"candidates",o.array(&u.candidates,|o,id|o.text(id)));
            member!(o,f,"kind",o.text(u.kind.as_str())); member!(o,f,"lane",o.optional(u.lane,Self::integer));
            member!(o,f,"track",o.integer(u.track)); member!(o,f,"selected",o.optional(u.selected,Self::integer));
            member!(o,f,"record_start",o.times(u.record_start)); member!(o,f,"duration",o.times(u.duration));
            member!(o,f,"source_in",o.times(u.source_in)); member!(o,f,"binding",o.binding(&u.binding));
            member!(o,f,"reason",o.optional(u.reason.as_deref(),Self::text)); Ok(())
        })
    }
    fn clock(&mut self, c: &SourceClock) -> R<()> {
        self.object(|o, f| {
            member!(
                o,
                f,
                "effective_count",
                o.text(&c.effective_count.to_string())
            );
            member!(o, f, "rate", o.time(c.rate));
            member!(o, f, "display_base", o.integer(c.display_base as i64));
            member!(o, f, "drop", o.boolean(c.drop));
            member!(
                o,
                f,
                "provenance",
                o.object(|o, f| {
                    member!(
                        o,
                        f,
                        "origin_count",
                        o.text(&c.provenance.origin_count.to_string())
                    );
                    member!(o, f, "source_offset", o.integer(c.provenance.source_offset));
                    member!(o, f, "source_rate", o.time(c.provenance.source_rate));
                    member!(o, f, "source_package", o.text(&c.provenance.source_package));
                    member!(o, f, "timecode_track", o.text(&c.provenance.timecode_track));
                    member!(o, f, "route", o.text(&c.provenance.route));
                    Ok(())
                })
            );
            Ok(())
        })
    }
}

pub(super) fn encoded_len(graph: &SourceGraph) -> R<usize> {
    let mut sink = std::io::sink();
    let mut out = JsonOut { writer: &mut sink, bytes: 0 };
    out.graph(graph)?;
    Ok(out.bytes)
}

pub(super) fn write_graph(graph: &SourceGraph, writer: &mut impl Write) -> R<()> {
    JsonOut { writer, bytes: 0 }.graph(graph)
}

pub(super) fn check_value_size(value: &Value) -> R<()> {
    JsonOut { writer: &mut std::io::sink(), bytes: 0 }.value(value)
}

pub(super) fn write_receipt(writer: &mut impl Write, owner: &str, data: &Value) -> R<usize> {
    let mut out = JsonOut { writer, bytes: 0 };
    out.object(|out, first| {
        out.field("owner", first)?;
        out.text(owner)?;
        out.field("data", first)?;
        out.value(data)
    })?;
    out.raw("\n")?;
    Ok(out.bytes)
}
