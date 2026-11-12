//! Source receipts for the bounded Fairlight records described by drpkit.
use super::encoding::{pb_bytes, Blobs};
use crate::Result;
use flate2::bufread::ZlibDecoder;
use graph::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

const PROVENANCE: &str = "drpkit:af3cedc0 documented record layout";
const FADER: &[u8] = &[1, 0, 0, 0, 1, 0, 0, 0];
const BUS_KIND: &[u8] = &[0, 0, 0, 0, 7, 0, 0, 0];
const BUS_TABLE: &[u8] = &[1, 0, 0, 0, 0x1d, 0, 0, 0];
const MAX_COLUMNS: usize = 64;
const MAX_TABLE_BYTES: usize = 32 * 1024;

struct Table<'a> {
    rows: BTreeMap<u32, &'a [u8]>,
    secondary_solo: Option<&'a [u8]>,
    columns: usize,
    names: Option<Vec<&'a str>>,
}

impl Table<'_> {
    fn value(&self, row: u32, column: usize) -> Option<i32> {
        let at = 4 * column;
        self.rows.get(&row)?.get(at..at + 4).map(signed)
    }
}

fn signed(word: &[u8]) -> i32 {
    i32::from_le_bytes(word.try_into().expect("four bytes"))
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("four bytes"))
}

fn table(mut bytes: &[u8], mixed_width: bool) -> Option<Table<'_>> {
    let columns = word(bytes.get(8..12)?) as usize;
    if !(1..=MAX_COLUMNS).contains(&columns) {
        return None;
    }
    bytes = &bytes[..bytes.len().min(MAX_TABLE_BYTES)];
    let mut out = Table {
        rows: BTreeMap::new(),
        secondary_solo: None,
        columns,
        names: None,
    };
    while let Some(header) = bytes.get(..12) {
        let id = word(&header[4..8]);
        let width = word(&header[8..12]) as usize;
        if word(&header[..4]) != 1 || id > 0xffff || !(1..=4096).contains(&width) {
            break;
        }
        if !mixed_width && width != columns {
            break;
        }
        bytes = &bytes[12..];
        if id == 0xbc {
            if out.names.is_some() || width != columns {
                break;
            }
            let mut names = Vec::with_capacity(width);
            for _ in 0..width {
                let length = word(bytes.get(..4)?) as usize;
                if length > 1000 {
                    return None;
                }
                names.push(std::str::from_utf8(bytes.get(4..4 + length)?).ok()?);
                bytes = &bytes[4 + length..];
            }
            out.names = Some(names);
            continue;
        }
        let Some(data) = bytes.get(..4 * width) else {
            break;
        };
        if width == columns {
            if id == 0x458 && out.rows.get(&id).is_some_and(|old| *old != data) {
                return None;
            }
            if id == 0x11 && out.rows.contains_key(&id) && out.secondary_solo.is_none() {
                out.secondary_solo = Some(data);
            }
            out.rows.entry(id).or_insert(data);
        }
        bytes = &bytes[data.len()..];
    }
    Some(out)
}

fn hits<'a>(data: &'a [u8], signature: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    data.windows(signature.len())
        .enumerate()
        .filter_map(move |(at, bytes)| (bytes == signature).then_some(at))
}

fn track_columns(table: &Table<'_>, tracks: &[i64]) -> Option<Vec<(i64, usize)>> {
    if table.names.is_none() || !table.rows.contains_key(&0xdf) {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut owner = None;
    let mut columns = Vec::new();
    for col in 0..table.columns {
        let id = table.value(0x458, col)?;
        if id == -1 {
            let (track, channel) = owner?;
            owner = Some((track, channel + 1));
        } else {
            let track = i64::from(id) + 1;
            if id < 0 || !tracks.contains(&track) || !seen.insert(track) {
                return None;
            }
            owner = Some((track, 0));
        }
        columns.push(owner?);
    }
    (seen.len() == tracks.len()).then_some(columns)
}

fn track_table<'a>(data: &'a [u8], tracks: &[i64]) -> Option<(Table<'a>, Vec<(i64, usize)>)> {
    let mut found = None;
    let candidates = hits(data, FADER).filter(|&at| {
        let Some(width) = data.get(at + 8..at + 12).map(word) else {
            return false;
        };
        if !(tracks.len()..=MAX_COLUMNS).contains(&(width as usize)) {
            return false;
        }
        let next = at + 12 + width as usize * 4;
        data.get(next..next + 12).is_some_and(|header| {
            header.starts_with(&[1, 0, 0, 0, 2, 0, 0, 0]) && word(&header[8..]) == width
        })
    });
    for (attempt, at) in candidates.enumerate() {
        if attempt == 128 {
            return None;
        }
        let Some(table) = table(&data[at..], true) else {
            continue;
        };
        let Some(columns) = track_columns(&table, tracks) else {
            continue;
        };
        if found.replace((table, columns)).is_some() {
            return None;
        }
    }
    found
}

fn bus_table(data: &[u8], track_count: usize) -> Option<(Table<'_>, Table<'_>)> {
    let mut found = None;
    for (attempt, at) in hits(data, BUS_TABLE).enumerate() {
        if attempt == 128 {
            return None;
        }
        let Some(named) = table(&data[at..], true) else {
            continue;
        };
        if named.names.is_none()
            || !(0..named.columns).all(|col| {
                named
                    .value(0x1b4, col)
                    .is_some_and(|n| (1..=64).contains(&n))
                    && named.value(0x458, col) == Some((track_count + col + 1) as i32)
            })
        {
            continue;
        }
        let mut candidates =
            hits(&data[..at], BUS_KIND).filter(|i| data[i + BUS_KIND.len()..at].starts_with(FADER));
        let prefix = candidates
            .by_ref()
            .take(128)
            .filter_map(|i| {
                let start = i + BUS_KIND.len();
                table(&data[start..at], false)
            })
            .filter(|t| t.columns == named.columns && t.rows.len() >= 10)
            .last();
        if candidates.next().is_some() {
            return None;
        }
        if let Some(prefix) = prefix {
            if found.replace((named, prefix)).is_some() {
                return None;
            }
        }
    }
    found
}

type Fields = Vec<(String, Value)>;

fn number(out: &mut Fields, name: &str, value: Option<i32>) {
    if let Some(value) = value {
        out.push((name.into(), Value::Int(i64::from(value))));
    }
}

fn digest(data: &[u8]) -> Value {
    Value::Str(hex::encode(Sha256::digest(data)))
}

fn processing(table: &Table<'_>, col: usize, out: &mut Fields) -> bool {
    number(out, "processing_order_code", table.value(0xde, col));
    number(out, "eq_enabled_code", table.value(0xdf, col));
    let mut eq_active = false;
    let bands = (1..=6)
        .filter_map(|band| {
            let mut values = vec![("band".into(), Value::Int(i64::from(band)))];
            for (row, label) in [
                (0x100 + band, "enabled_code"),
                (0xf8 + band, "filter_type_code"),
                (0xf0 + band, "frequency_hz"),
                (0xe0 + band, "gain_tenths_db"),
                (0xe8 + band, "raw_q"),
                (0x333 + band, "raw_frequency_knob"),
            ] {
                number(&mut values, label, table.value(row, col));
            }
            match band {
                1 => number(&mut values, "slope_code", table.value(0x4be, col)),
                6 => number(&mut values, "slope_code", table.value(0x4c3, col)),
                _ => number(&mut values, "range_code", table.value(0x287 + band, col)),
            }
            eq_active |= table.value(0x100 + band, col) == Some(1)
                && (table.value(0xe0 + band, col).is_some_and(|n| n != 0)
                    || matches!(table.value(0xf8 + band, col), Some(4..=6)));
            (values.len() > 1).then_some(Value::Obj(values))
        })
        .collect();
    out.push(("eq_bands".into(), Value::Arr(bands)));

    let dynamics = [
        (0x109, "makeup_tenths_db"),
        (0x10c, "compressor_enabled_code"),
        (0x10d, "compressor_ratio_raw"),
        (0x10e, "compressor_threshold_tenths_db"),
        (0x10f, "compressor_knee_raw"),
        (0x110, "compressor_attack_raw"),
        (0x111, "compressor_hold_raw"),
        (0x112, "compressor_release_raw"),
        (0x114, "expander_enabled_code"),
        (0x115, "expander_range_raw"),
        (0x116, "expander_threshold_raw"),
        (0x117, "compressor_mix_raw"),
        (0x118, "expander_attack_raw"),
        (0x119, "expander_hold_raw"),
        (0x11a, "expander_release_raw"),
        (0x11b, "expander_mode_code"),
        (0x11d, "limiter_enabled_code"),
        (0x11e, "limiter_threshold_tenths_db"),
        (0x11f, "limiter_attack_raw"),
        (0x120, "limiter_hold_raw"),
        (0x121, "limiter_release_raw"),
        (0x253, "expander_ratio_raw"),
    ]
    .into_iter()
    .filter_map(|(row, label)| {
        table
            .value(row, col)
            .map(|n| (label.into(), Value::Int(i64::from(n))))
    })
    .collect();
    out.push(("dynamics".into(), Value::Obj(dynamics)));
    let raw = table
        .rows
        .keys()
        .filter_map(|&id| {
            if !matches!(id, 0xde..=0x121 | 0x253 | 0x289..=0x28c | 0x334..=0x339 | 0x4be | 0x4c3) {
                return None;
            }
            Some((
                format!("0x{id:03x}"),
                Value::Int(i64::from(table.value(id, col)?)),
            ))
        })
        .collect();
    out.push(("eq_dynamics_fields".into(), Value::Obj(raw)));
    (eq_active && table.value(0xdf, col) == Some(1))
        || [0x10c, 0x114, 0x11d]
            .into_iter()
            .any(|id| table.value(id, col).is_some_and(|n| n != 0))
}

pub(super) fn mixer(data: &[u8], tracks: &[i64], typed_gain_mute: bool) -> Option<(Value, bool)> {
    if tracks.is_empty() || tracks.len() > MAX_COLUMNS {
        return None;
    }
    let (table, owners) = track_table(data, tracks)?;
    let mut active = false;
    let tracks_facts = owners
        .iter()
        .enumerate()
        .map(|(col, &(track, channel))| {
            let mut out = vec![
                ("track".into(), Value::Int(track)),
                ("strip_id".into(), Value::Int(track - 1)),
                ("channel_index".into(), Value::Int(channel as i64)),
            ];
            if let Some(name) = table
                .names
                .as_ref()
                .map(|names| names[col])
                .filter(|name| !name.is_empty())
            {
                out.push(("saved_channel_name".into(), Value::Str(name.into())));
            }
            for (id, name, neutral) in [
                (1, "fader_tenths_db", 0),
                (2, "mute_code", 0),
                (4, "pan_tenths_percent", 0),
                (0x0d, "pan_bypass_code", 0),
                (0x11, "solo_code", 0),
                (0x19, "pre_dynamics_gain_tenths_db", 0),
                (0x1d, "effects_in_code", 0),
                (0x400, "output_connected_code", 1),
                (0x40b, "output_bus_index", 0),
            ] {
                let value = table.value(id, col);
                number(&mut out, name, value);
                // Faders and mute use the separate existing typed qualification gate.
                active |= (id > 2 || !typed_gain_mute) && value.is_some_and(|n| n != neutral);
            }
            let mut observations = Vec::new();
            for id in [0x008, 0x009, 0x0b9, 0x13b] {
                number(
                    &mut observations,
                    &format!("0x{id:03x}"),
                    table.value(id, col),
                );
            }
            if let Some(raw) = table
                .secondary_solo
                .map(|row| signed(&row[4 * col..4 * col + 4]))
            {
                observations.push(("0x011_second".into(), Value::Int(i64::from(raw))));
                active |= raw != 0;
            }
            out.push(("unresolved_channel_fields".into(), Value::Obj(observations)));
            active |= processing(&table, col, &mut out);
            Value::Obj(out)
        })
        .collect();
    let mut out = vec![
        ("provenance".into(), Value::Str(PROVENANCE.into())),
        ("tracks".into(), Value::Arr(tracks_facts)),
        ("mixer_sha256".into(), digest(data)),
    ];
    let buses = bus_table(data, tracks.len());
    if let Some((named, prefix)) = &buses {
        let entries: Vec<_> = (0..named.columns)
            .map(|col| {
                let mut values = vec![("index".into(), Value::Int(col as i64))];
                if let Some(name) = named
                    .names
                    .as_ref()
                    .map(|names| names[col])
                    .filter(|name| !name.is_empty())
                {
                    values.push(("name".into(), Value::Str(name.into())));
                }
                for (id, name) in [
                    (1, "fader_tenths_db"),
                    (2, "mute_code"),
                    (4, "raw_pan"),
                    (0x0d, "raw_pan_bypass"),
                    (0x11, "raw_solo"),
                    (0x19, "pre_dynamics_gain_tenths_db"),
                ] {
                    let value = prefix.value(id, col);
                    number(&mut values, name, value);
                    active |= (id != 1 || !typed_gain_mute) && value.is_some_and(|n| n != 0);
                }
                for (id, name) in [
                    (0x1b4, "channels"),
                    (0x458, "strip_id"),
                    (0x400, "output_connected_code"),
                    (0x40b, "output_bus_index"),
                    (0x1d, "effects_in_code"),
                ] {
                    let value = named.value(id, col);
                    number(&mut values, name, value);
                    active |= matches!(id, 0x400 | 0x1d) && value.is_some_and(|n| n != 0);
                }
                active |= processing(named, col, &mut values);
                Value::Obj(values)
            })
            .collect();
        active |= entries.len() > 1;
        out.push(("main_bus".into(), entries[0].clone()));
        out.push(("buses".into(), Value::Arr(entries)));
    } else {
        active = true;
    }
    let bus_count = buses.as_ref().map_or(0, |(named, _)| named.columns);
    let mut mixer_plugins = Vec::new();
    for (at, record) in plugin_records(data) {
        let Some(header) = at.checked_sub(12).and_then(|start| data.get(start..at)) else {
            continue;
        };
        let [kind, col, zero, slot] = header[..4].try_into().expect("four bytes");
        let owner_code = word(&header[8..]);
        let col = usize::from(col);
        if zero != 0 {
            continue;
        }
        let mut owner = match (kind, slot, owner_code) {
            (0, 0..=5, 2) | (0, 6..=10, 0..=2) => {
                let Some(&(track, channel)) = owners.get(col) else {
                    continue;
                };
                vec![
                    ("track".into(), Value::Int(track)),
                    ("channel_index".into(), Value::Int(channel as i64)),
                ]
            }
            (7, 0..=5, 2) if col < bus_count => vec![("bus_index".into(), Value::Int(col as i64))],
            _ => continue,
        };
        owner.push(("slot".into(), Value::Int(i64::from(slot))));
        owner.push(("owner_code".into(), Value::Int(i64::from(owner_code))));
        if kind == 0 && slot == 6 && record.name == "Voice Isolation" {
            let enabled = record.state & 0x10 == 0;
            owner.push(("enabled".into(), Value::Bool(enabled)));
            active |= enabled;
        } else {
            active |= slot <= 5 || !record.params.is_empty();
        }
        owner.extend(record.facts());
        mixer_plugins.push(Value::Obj(owner));
    }
    if !mixer_plugins.is_empty() {
        out.push(("plugins".into(), Value::Arr(mixer_plugins)));
    }
    Some((Value::Obj(out), active))
}

struct Plugin<'a> {
    name: &'a str,
    id: u32,
    state: u32,
    params: Fields,
}

impl Plugin<'_> {
    fn facts(self) -> Fields {
        vec![
            ("name".into(), Value::Str(self.name.into())),
            ("effect_id".into(), Value::Int(i64::from(self.id))),
            ("state_flags".into(), Value::Int(i64::from(self.state))),
            ("raw_parameters".into(), Value::Obj(self.params)),
        ]
    }
}

fn nul_padded(bytes: &[u8]) -> Option<&str> {
    let end = bytes.iter().position(|&b| b == 0)?;
    if bytes[end..].iter().any(|&b| b != 0) {
        return None;
    }
    std::str::from_utf8(&bytes[..end]).ok()
}

fn param_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b':')
}

fn plugin_records(data: &[u8]) -> impl Iterator<Item = (usize, Plugin<'_>)> {
    let mut tags = hits(data, b"bmd:").peekable();
    std::iter::from_fn(move || {
        let at = tags.next()?;
        let limit = tags.peek().copied().unwrap_or(data.len()).min(at + 1024);
        Some((at, plugin(data.get(at..limit)?)))
    })
    .filter_map(|(at, record)| record.map(|record| (at, record)))
    .take(128)
}

fn plugin(record: &[u8]) -> Option<Plugin<'_>> {
    let tag = nul_padded(record.get(..256)?)?.strip_prefix("bmd:")?;
    let (name, id) = tag.rsplit_once(':')?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || matches!(b, b' ' | b'-'))
    {
        return None;
    }
    let id = id.parse::<u32>().ok()?;
    let state = word(record.get(272..276)?);
    let mut params = BTreeMap::new();
    for (at, slot) in record[276..].windows(68).enumerate() {
        if !slot[0].is_ascii_alphabetic() || param_char(record[275 + at]) {
            continue;
        }
        let Some(key) = nul_padded(&slot[..64]) else {
            continue;
        };
        if !key.bytes().all(param_char) {
            continue;
        }
        let value = f64::from(f32::from_bits(word(&slot[64..])));
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            params.entry(key.into()).or_insert(Value::Float(value));
        }
    }
    Some(Plugin {
        name,
        id,
        state,
        params: params.into_iter().collect(),
    })
}

pub(super) fn clip_plugins(encoded: &str, blobs: &mut Blobs) -> Result<Option<Value>> {
    // Clip FX are a versioned blob nested under 1/1/2. Require the keyed
    // record, rather than scanning unrelated FieldsBlob payloads for zlib.
    if !encoded.starts_with("00000002") || encoded.len() > 128 * 1024 {
        return Ok(None);
    }
    let data = blobs.envelope(encoded)?;
    if !data.windows(10).any(|w| w == b"FL::ClipFX") {
        return Ok(None);
    }
    let Some(first) = pb_bytes(&data, 1)? else {
        return Ok(None);
    };
    let Some(second) = pb_bytes(first, 1)? else {
        return Ok(None);
    };
    let Some(property) = pb_bytes(second, 2)? else {
        return Ok(None);
    };
    if pb_bytes(property, 1)? != Some(b"FL::ClipFX") {
        return Ok(None);
    }
    let Some(packed) = pb_bytes(property, 2)? else {
        return Ok(None);
    };
    let Some((header, stream)) = packed.split_at_checked(4) else {
        return Ok(None);
    };
    let expected = u32::from_be_bytes(header.try_into()?) as usize;
    if expected > 64 * 1024 {
        return Err("Resolve clip Fairlight state exceeds its byte budget".into());
    }
    blobs.charge(expected)?;
    let mut decoder = ZlibDecoder::new(stream);
    let mut raw = Vec::new();
    (&mut decoder)
        .take((expected + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() != expected || decoder.total_in() != stream.len() as u64 {
        return Err("Resolve clip Fairlight expanded length disagrees".into());
    }
    let plugins: Vec<_> = plugin_records(&raw)
        .filter_map(|(_, plugin)| {
            matches!(plugin.name, "Voice Isolation" | "Dialogue Leveler").then(|| {
                let mut facts = plugin.facts();
                facts.push(("enabled".into(), Value::Bool(true)));
                Value::Obj(facts)
            })
        })
        .collect();
    Ok((!plugins.is_empty()).then(|| {
        Value::Obj(vec![
            ("provenance".into(), Value::Str(PROVENANCE.into())),
            ("clip_fx_sha256".into(), digest(&raw)),
            ("plugins".into(), Value::Arr(plugins)),
        ])
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{pack, pb_bytes, pb_int};
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    fn record(output: &mut Vec<u8>, id: i32, values: &[i32]) {
        for word in [1, id, values.len() as i32].iter().chain(values) {
            output.extend(word.to_le_bytes());
        }
    }

    fn names(output: &mut Vec<u8>, labels: &[&str]) {
        for word in [1i32, 0xbc, labels.len() as i32] {
            output.extend(word.to_le_bytes());
        }
        for label in labels {
            output.extend((label.len() as u32).to_le_bytes());
            output.extend(label.as_bytes());
        }
    }

    fn voice(output: &mut Vec<u8>, col: u8, state: u32, raw: f32) {
        output.extend([0, col, 0, 6, 0x58, 1, 0, 0, 2, 0, 0, 0]);
        let mut record = [0; 552];
        let tag = b"bmd:Voice Isolation:1112364617";
        record[..tag.len()].copy_from_slice(tag);
        record[272..276].copy_from_slice(&state.to_le_bytes());
        let parameter = b"BMDVoiceIsolationControl::DRY_MIX";
        record[484..484 + parameter.len()].copy_from_slice(parameter);
        record[548..].copy_from_slice(&raw.to_le_bytes());
        output.extend(record);
    }

    fn entries<'a>(owner: &'a Value, key: &str) -> &'a [Value] {
        let Some(Value::Arr(entries)) = owner.get(key) else {
            panic!("missing {key}");
        };
        entries
    }

    #[test]
    fn channel_and_plugin_owners_follow_strip_identity_with_repeated_names() {
        let mut data = Vec::new();
        record(&mut data, 1, &[-35, -15, 0]);
        record(&mut data, 2, &[0, 0, 0]);
        record(&mut data, 4, &[0, -1000, 1000]);
        record(&mut data, 0x40b, &[1, 0, 0]);
        record(&mut data, 0xdf, &[1, 1, 1]);
        record(&mut data, 0xf2, &[240, 300, 300]);
        names(&mut data, &["same", "same", "same"]);
        let strip_row = data.len();
        record(&mut data, 0x458, &[1, 0, -1]);
        data.extend([0; 16]);
        data.extend(BUS_KIND);
        for id in 1..=10 {
            record(&mut data, id, &[0, -20]);
        }
        data.extend([0; 16]);
        record(&mut data, 0x1d, &[0, 0]);
        names(&mut data, &["same", "same"]);
        record(&mut data, 0x1b4, &[2, 2]);
        record(&mut data, 0x458, &[3, 4]);
        data.extend([0; 16]);
        voice(&mut data, 0, 0x10, 0.375);
        voice(&mut data, 2, 0, 0.5);

        let (facts, active) = mixer(&data, &[1, 2], false).unwrap();
        assert!(active);
        let tracks = entries(&facts, "tracks");
        assert_eq!(tracks[0].get("track"), Some(&Value::Int(2)));
        assert_eq!(tracks[0].get("output_bus_index"), Some(&Value::Int(1)));
        assert_eq!(tracks[1].get("track"), Some(&Value::Int(1)));
        assert_eq!(tracks[2].get("channel_index"), Some(&Value::Int(1)));
        assert_eq!(
            entries(&tracks[0], "eq_bands")[0].get("frequency_hz"),
            Some(&Value::Int(240))
        );
        assert_eq!(
            entries(&facts, "buses")[1].get("fader_tenths_db"),
            Some(&Value::Int(-20))
        );
        let plugins = entries(&facts, "plugins");
        assert_eq!(plugins[0].get("track"), Some(&Value::Int(2)));
        assert_eq!(plugins[0].get("enabled"), Some(&Value::Bool(false)));
        assert_eq!(plugins[1].get("track"), Some(&Value::Int(1)));
        assert_eq!(plugins[1].get("channel_index"), Some(&Value::Int(1)));
        let serialized = graph::json::write(&facts);
        assert_eq!(graph::json::parse(&serialized).unwrap(), facts);

        data[strip_row + 16..strip_row + 20].copy_from_slice(&1i32.to_le_bytes());
        assert!(mixer(&data, &[1, 2], false).is_none());
        assert!(mixer(&data[..strip_row], &[1, 2], false).is_none());
    }

    #[test]
    fn clip_plugin_uses_keyed_zlib_and_rejects_duplicate_owner_or_wrong_size() {
        let mut raw = Vec::new();
        voice(&mut raw, 0, 0x10, 0.55);
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&raw).unwrap();
        let stream = compressor.finish().unwrap();
        let blob = |size: u32, duplicate: bool| {
            let packed = [size.to_be_bytes().as_slice(), &stream].concat();
            let mut keyed = Vec::new();
            pb_bytes(1, b"FL::ClipFX", &mut keyed);
            pb_bytes(2, &packed, &mut keyed);
            let mut child = Vec::new();
            pb_bytes(2, &keyed, &mut child);
            if duplicate {
                pb_bytes(2, &keyed, &mut child);
            }
            let mut middle = Vec::new();
            pb_bytes(1, &child, &mut middle);
            pb_int(4, 1, &mut middle);
            let mut outer = Vec::new();
            pb_bytes(1, &middle, &mut outer);
            pb_int(15, 4, &mut outer);
            pack(&outer).unwrap()
        };
        let size = raw.len() as u32;
        let data = clip_plugins(&blob(size, false), &mut Blobs::default())
            .unwrap()
            .unwrap();
        let plugins = entries(&data, "plugins");
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].get("effect_id"), Some(&Value::Int(1112364617)));
        assert!(clip_plugins(&blob(size, true), &mut Blobs::default()).is_err());
        assert!(clip_plugins(&blob(size - 1, false), &mut Blobs::default()).is_err());
        assert!(clip_plugins(&blob(65_537, false), &mut Blobs::default()).is_err());
    }
}
