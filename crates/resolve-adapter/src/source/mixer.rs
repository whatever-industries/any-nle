//! Static faders and track mute in the qualified two-mono Fairlight mixer.
use super::encoding::qt;
use crate::{
    linked::mixer::{ProcessingCells, StripProcessing, PROCESSING_RECEIPT, PROCESSING_ROWS},
    track_state::{valid_name, Track},
    Result,
};
use flate2::{bufread::ZlibDecoder, write::ZlibEncoder, Compression};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    ops::Range,
};

const FIELD_TAG: &[u8] = b"\0\0\0\x1e\0F\0L\0S\0t\0u\0d\0i\0o\0M\0o\0d\0e\0l\0B\0A\0\0\0\x0c\0";
const MAX_MODEL_BYTES: usize = 1024 * 1024;
const TRACK_FADERS: Range<usize> = 203_093..203_101;
const TRACK_MUTES: [Range<usize>; 2] = [203_113..203_121, 203_133..203_141];
const TRACK_MUTE_HEADER: [u8; 12] = [1, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0];
const MAIN_BUS_FADER: Range<usize> = 235_340..235_344;
const SKELETON: &str = "5bfebb8fa74dacf3d8185bfe459d719b4824cf3ba623d11c9d873a2cace5adb8";
// Fingerprints with only the qualified control cells neutralized.
const DIRECT_ONE: &[&str] = &[
    SKELETON,
    "a3b176b7e9dc4ba936b2efe909b975775004cfc2e9508f6681addeb2d638bdcf",
    "46ac7ad463f934d2456732b1a8f086eb79d129db1176b43e2551753466fbbf07",
];
const DIRECT_TWO: &str = "00765117a479b3e6469ce7a35f66c7befb8e5981d3baa4969f08e47815abed11";
// Native track-state saves after neutralizing the identified track controls
// and the existing direct mixer cells.
const DIRECT_TRACK_STATE_SAVE: &[&str] = &[
    "d1e00dc8c0439a16a20c75672487a7ca867ba4b7d4334d97464c5ab46a269bea",
    "e87824e22afb81d176b45211d4aceed3c879d4505f86e3829be6f447f1887391",
    "cd119bc87d33ae0e298aee89d2ba814f6c97d1e05705a4b6f8ac3f75a26e15bd",
    "16cbeed9bc0e951b469785040842bf9410cedbe88698ebedfbf66f6f312e098b",
    "20d2bdf778983a562f20a2f2f181a0b509d810f3e3340b2f631d88ec63408dca",
];
const DIRECT_TWO_FAIRLIGHT_SAVE: &str =
    "83d76c19c40bbb9d75afb2bea828387fb364303b8da9885fd4eaf5185582650c";
const DIRECT_PAN: usize = 203_173;
const DIRECT_PAN_BYPASS: usize = 203_393;
const DIRECT_SOLO: usize = 203_473;
const DIRECT_ROUTE: usize = 211_685;
const TWO_BUS_SHIFT: usize = 8;
const BUS_2_FADER: usize = 235_352;
const BUS_2_MUTE: usize = 235_372;
const BUS_2_MUTE_MIRROR: usize = 235_392;

fn bus_processing_row(ordinal: usize, buses: i64, shift: usize) -> usize {
    if buses == 1 {
        return 302_641 + ordinal * 16;
    }
    302_838 + (shift - TWO_BUS_SHIFT) + ordinal * 20
}

fn bus_pre_gain_row(buses: i64, shift: usize) -> usize {
    if buses == 1 {
        235_632
    } else {
        235_716 + (shift - TWO_BUS_SHIFT)
    }
}

#[derive(PartialEq, Eq)]
struct DirectProfile {
    buses: i64,
    tracks: [ProcessingCells; 2],
    bus_processing: [ProcessingCells; 2],
    bus_pre_gain: [i32; 2],
}

impl DirectProfile {
    const fn neutral(buses: i64) -> Self {
        Self {
            buses,
            tracks: [StripProcessing::DEFAULT.cells(); 2],
            bus_processing: [StripProcessing::DEFAULT.cells(); 2],
            bus_pre_gain: [0; 2],
        }
    }
}

pub(crate) const FAIRLIGHT_ADAPTER: &str = "resolve.fairlight";
pub(crate) const DIRECT_PROFILE: &str = "direct_mono_stereo_profile";
pub(crate) const FAIRLIGHT_LOSS: (&str, &str) = (
    "fairlight_controls",
    "Resolve Fairlight state retained in graph; destination mapping is unqualified",
);

const NATIVE_CONTROL: &str = "07df7b43d05ef9564fd6cf69e1b911eb9b3884c65d5c1f86aa7bde797aad5f35";
const NATIVE_BUS_CONTROL: &str = "dcf061b1178e516baf8157cba488fb260a37e77b653b914e649fa7df1b06c060";

pub(crate) const MAIN_BUS_LEVEL: &str = "main_bus_level_db";
pub(crate) const MAIN_BUS_LOSS: (&str, &str) = (
    "bus_volume",
    "Resolve main bus level is retained in source metadata",
);
pub(crate) const MUTED_AUDIO_TRACKS: &str = "muted_audio_tracks";
pub(crate) const TRACK_MUTE_LOSS: (&str, &str) = (
    "track_mute",
    "Resolve track mute is retained in source metadata",
);

pub(crate) struct Mixer {
    fields: Vec<u8>,
    field: Range<usize>,
    model: Vec<u8>,
}

struct TrackCells {
    names: Range<usize>,
    locks: Range<usize>,
}

impl TrackCells {
    fn locate(model: &[u8], name_row: usize, next_id: u32, lock_delta: usize) -> Option<Self> {
        let header = |offset, id: u32| {
            model.get(offset..offset + 12).is_some_and(|row| {
                row[..4] == 1u32.to_le_bytes()
                    && row[4..8] == id.to_le_bytes()
                    && row[8..] == 2u32.to_le_bytes()
            })
        };
        if !header(name_row, 0xbc) {
            return None;
        }
        let start = name_row + 12;
        let mut end = start;
        for _ in 0..2 {
            let len = u32::from_le_bytes(model.get(end..end + 4)?.try_into().ok()?) as usize;
            if len > 255 {
                return None;
            }
            end += 4 + len;
        }
        let lock = end + lock_delta;
        if !header(end, next_id) || !header(lock, 0xdb) {
            return None;
        }
        model.get(lock + 12..lock + 20)?;
        Some(Self {
            names: start..end,
            locks: lock + 12..lock + 20,
        })
    }

    fn read(&self, model: &[u8]) -> Option<[Track; 2]> {
        let mut names = model.get(self.names.clone())?;
        let mut out: [Track; 2] = Default::default();
        for (track, lock) in out
            .iter_mut()
            .zip(model.get(self.locks.clone())?.chunks_exact(4))
        {
            let len = u32::from_le_bytes(names.get(..4)?.try_into().ok()?) as usize;
            let name = std::str::from_utf8(names.get(4..4 + len)?).ok()?;
            if !valid_name(name) || !matches!(lock, [0 | 1, 0, 0, 0]) {
                return None;
            }
            track.name = name.into();
            track.locked = lock[0] == 1;
            names = &names[4 + len..];
        }
        Some(out)
    }
}

fn audio_track_cells(model: &[u8]) -> Option<TrackCells> {
    let mut rows = [206_869, 206_877, 206_917]
        .into_iter()
        .filter_map(|offset| TrackCells::locate(model, offset, 0xbd, 140));
    let found = rows.next()?;
    rows.next().is_none().then_some(found)
}

fn video_track_cells(model: &[u8]) -> Option<TrackCells> {
    let mut rows = (340_000..model.len().min(360_000))
        .filter_map(|offset| TrackCells::locate(model, offset, 0xd6, 20));
    let found = rows.next()?;
    rows.next().is_none().then_some(found)
}

fn video_mute_cells(model: &[u8], row: &TrackCells) -> Option<[Range<usize>; 2]> {
    // The optional video registry stores the same mute flags in two rows
    // before the names. Its columns are distinct from the audio mixer.
    let name_header = row.names.start.checked_sub(12)?;
    let cells = [name_header.checked_sub(60)?, name_header.checked_sub(40)?];
    cells
        .iter()
        .all(|&header| model.get(header..header + 12) == Some(&TRACK_MUTE_HEADER))
        .then(|| cells.map(|header| header + 12..header + 20))
}

fn update_model_length(model: &mut [u8]) -> Option<()> {
    if model.get(..8)? != b"\x66\x88\x66\x77\x01\x00\x00\x00" {
        return None;
    }
    let len = u32::try_from(model.len().checked_sub(12)?).ok()?;
    model.get_mut(8..12)?.copy_from_slice(&len.to_le_bytes());
    Some(())
}

impl Mixer {
    pub(super) fn has_track_state(&self) -> bool {
        let active =
            |row: &TrackCells| !self.cells_are_neutral([row.names.clone(), row.locks.clone()]);
        audio_track_cells(&self.model).as_ref().is_some_and(active)
            || video_track_cells(&self.model).as_ref().is_some_and(|row| {
                active(row)
                    || video_mute_cells(&self.model, row)
                        .is_some_and(|cells| !self.cells_are_neutral(cells))
            })
    }

    pub(super) fn track_state(&self) -> Option<([Track; 2], Option<[Track; 2]>, Self)> {
        let audio = audio_track_cells(&self.model)?;
        let a = audio.read(&self.model)?;
        let video = video_track_cells(&self.model);
        let mut model = self.model.clone();
        let v = if let Some(row) = &video {
            let mut tracks = row.read(&self.model)?;
            let mut mutes = [None; 2];
            for (cell, copy) in video_mute_cells(&self.model, row)?
                .into_iter()
                .zip(&mut mutes)
            {
                let values = model.get_mut(cell)?;
                let [first @ (0 | 1), 0, 0, 0, second @ (0 | 1), 0, 0, 0] = values else {
                    return None;
                };
                *copy = Some([*first == 1, *second == 1]);
                values.fill(0);
            }
            if mutes[0] != mutes[1] {
                return None;
            }
            for (track, disabled) in tracks.iter_mut().zip(mutes[0]?) {
                track.disabled = disabled;
            }
            Some(tracks)
        } else {
            None
        };
        for row in video.into_iter().chain([audio]) {
            model.get_mut(row.locks)?.fill(0);
            model.splice(row.names, [0; 8]);
        }
        update_model_length(&mut model)?;
        Some((
            a,
            v,
            Self {
                fields: vec![],
                field: 0..0,
                model,
            },
        ))
    }

    pub(crate) fn write_track_state(mut self, tracks: &[Track]) -> Result<String> {
        let row =
            audio_track_cells(&self.model).ok_or("missing Fairlight track-name and lock rows")?;
        if tracks.len() != 2 || row.read(&self.model) != Some(Default::default()) {
            return Err("expected neutral Fairlight track names and locks in the template".into());
        }
        let mut names = Vec::new();
        for (slot, track) in self.model[row.locks].chunks_exact_mut(4).zip(tracks) {
            slot.copy_from_slice(&i32::from(track.locked).to_le_bytes());
            names.extend_from_slice(&u32::try_from(track.name.len())?.to_le_bytes());
            names.extend_from_slice(track.name.as_bytes());
        }
        self.model.splice(row.names, names);
        update_model_length(&mut self.model).ok_or("invalid Fairlight model header")?;
        self.encode()
    }
    pub(crate) fn decoded_bytes(&self) -> usize {
        self.fields.len() + self.model.len()
    }

    pub(crate) fn decode(encoded: &str) -> Result<Option<Self>> {
        if encoded.len() > 2 * 65536 {
            return Ok(None);
        }
        let fields = hex::decode(encoded)?;
        let mut hits = fields
            .windows(FIELD_TAG.len())
            .enumerate()
            .filter(|(_, bytes)| *bytes == FIELD_TAG);
        let Some((at, _)) = hits.next() else {
            return Ok(None);
        };
        if hits.next().is_some() {
            return Err("duplicate Fairlight mixer fields".into());
        }
        let start = at + FIELD_TAG.len();
        let length = u32::from_be_bytes(
            fields
                .get(start..start + 4)
                .ok_or("truncated Fairlight mixer length")?
                .try_into()?,
        ) as usize;
        if length == u32::MAX as usize {
            return Ok(None);
        }
        let end = (start + 4)
            .checked_add(length)
            .ok_or("Fairlight mixer extent overflows")?;
        let data = fields
            .get(start + 4..end)
            .ok_or("truncated Fairlight mixer")?;
        let size = data
            .get(..4)
            .ok_or("missing Fairlight mixer expanded length")?;
        let size = u32::from_be_bytes(size.try_into()?) as usize;
        if !(MAIN_BUS_FADER.end..=MAX_MODEL_BYTES).contains(&size) {
            return Ok(None);
        }
        if qt(&fields)?.bytes("FLStudioModelBA")? != data {
            return Err("Fairlight mixer field disagrees with its Qt map".into());
        }
        let mut decoder = ZlibDecoder::new(&data[4..]);
        let mut model = Vec::new();
        (&mut decoder)
            .take((size + 1) as u64)
            .read_to_end(&mut model)?;
        if model.len() != size || decoder.total_in() != (data.len() - 4) as u64 {
            return Err("Fairlight mixer expanded length disagrees".into());
        }
        Ok(Some(Self {
            fields,
            field: start..end,
            model,
        }))
    }

    pub(crate) fn track_faders(&self) -> Option<[i32; 2]> {
        // Detect the fader row separately so an unknown strip layout with
        // active faders is reported as a loss by the reader.
        if self.model[TRACK_FADERS.start - 12..TRACK_FADERS.start]
            != [1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]
        {
            return None;
        }
        Some([
            i32::from_le_bytes(
                self.model[TRACK_FADERS.start..TRACK_FADERS.start + 4]
                    .try_into()
                    .ok()?,
            ),
            i32::from_le_bytes(
                self.model[TRACK_FADERS.start + 4..TRACK_FADERS.end]
                    .try_into()
                    .ok()?,
            ),
        ])
    }

    pub(crate) fn main_bus_fader(&self) -> Option<i32> {
        // Bus kind 7, then the single-column fader record.
        if self.model[MAIN_BUS_FADER.start - 20..MAIN_BUS_FADER.start]
            != [0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]
        {
            return None;
        }
        Some(i32::from_le_bytes(
            self.model[MAIN_BUS_FADER].try_into().ok()?,
        ))
    }

    pub(crate) fn has_mute_state(&self) -> bool {
        TRACK_MUTES.iter().any(|range| {
            self.model[range.start - TRACK_MUTE_HEADER.len()..range.start] == TRACK_MUTE_HEADER
                && self.model[range.clone()] != [0; 8]
        })
    }

    pub(crate) fn track_mutes(&self) -> Option<[bool; 2]> {
        let [primary, mirror] = TRACK_MUTES;
        let bytes = &self.model[primary.clone()];
        if TRACK_MUTES.iter().any(|range| {
            self.model[range.start - TRACK_MUTE_HEADER.len()..range.start] != TRACK_MUTE_HEADER
        }) || bytes != &self.model[mirror]
            || !bytes
                .chunks_exact(4)
                .all(|slot| matches!(slot, [0 | 1, 0, 0, 0]))
        {
            return None;
        }
        Some([bytes[0] == 1, bytes[4] == 1])
    }

    pub(super) fn source_facts(
        &self,
        tracks: &[i64],
        typed_gain_mute: bool,
        profile: Option<&Self>,
    ) -> Option<(graph::Value, bool)> {
        let (mut facts, mut active) =
            super::fairlight::mixer(&self.model, tracks, typed_gain_mute)?;
        if let (graph::Value::Obj(fields), Some(profile)) =
            (&mut facts, profile.unwrap_or(self).direct_profile())
        {
            let changed = profile.tracks != [StripProcessing::DEFAULT.cells(); 2];
            active |= changed
                || profile.bus_processing != [StripProcessing::DEFAULT.cells(); 2]
                || profile.bus_pre_gain != [0; 2];
            if let ([Some(a1), Some(a2)], [Some(_), Some(_)]) = (
                profile.tracks.map(StripProcessing::from_cells),
                profile.bus_processing.map(StripProcessing::from_cells),
            ) {
                if changed {
                    fields.push((
                        PROCESSING_RECEIPT.into(),
                        graph::Value::Arr(vec![a1.receipt(), a2.receipt()]),
                    ));
                }
                if active {
                    fields.push((DIRECT_PROFILE.into(), graph::Value::Int(profile.buses)));
                }
            }
        }
        Some((facts, active))
    }

    fn neutral_digest(&self) -> String {
        self.digest_with_neutral_cells([
            TRACK_FADERS,
            TRACK_MUTES[0].clone(),
            TRACK_MUTES[1].clone(),
            MAIN_BUS_FADER,
        ])
    }

    fn digest_with_neutral_cells(&self, cells: impl IntoIterator<Item = Range<usize>>) -> String {
        self.digest_with_cells(cells.into_iter().map(|cell| (cell, [0; 8])))
    }

    fn digest_with_cells(
        &self,
        cells: impl IntoIterator<Item = (Range<usize>, [u8; 8])>,
    ) -> String {
        let mut hash = Sha256::new();
        let mut end = 0;
        for (field, value) in cells {
            hash.update(&self.model[end..field.start]);
            hash.update(&value[..field.len()]);
            end = field.end;
        }
        hash.update(&self.model[end..]);
        hex::encode(hash.finalize())
    }

    pub(crate) fn qualified(&self) -> bool {
        // The fingerprints include strip identity, routing and all other mixer state.
        matches!(
            self.neutral_digest().as_str(),
            SKELETON | NATIVE_CONTROL | NATIVE_BUS_CONTROL
        )
    }

    fn direct_profile(&self) -> Option<DirectProfile> {
        // Saving on the Fairlight page adds 40 bytes before the control tables
        // in the observed two-bus project. Both layouts have their own fingerprint.
        let (buses, shift, expected) = match self.model.len() {
            364_028 | 364_088 | 364_092 => (1, 0, DIRECT_ONE),
            364_152 => (1, 0, DIRECT_TRACK_STATE_SAVE),
            365_437 => (2, TWO_BUS_SHIFT, std::slice::from_ref(&DIRECT_TWO)),
            365_477 => (
                2,
                TWO_BUS_SHIFT + 40,
                std::slice::from_ref(&DIRECT_TWO_FAIRLIGHT_SAVE),
            ),
            _ => return None,
        };
        let [pan, route] = direct_track_cells(shift);
        let mut cells = vec![(pan, [0; 8])];
        if buses == 1 {
            for (row, id) in [(DIRECT_PAN_BYPASS, 0x0du32), (DIRECT_SOLO, 0x11)] {
                if self.model.get(row - 12..row)? != [1, id, 2].map(u32::to_le_bytes).concat() {
                    return None;
                }
                cells.push((row..row + 8, [0; 8]));
            }
            if !self.model[DIRECT_SOLO..DIRECT_SOLO + 8]
                .chunks_exact(4)
                .all(|slot| matches!(slot, [0 | 1, 0, 0, 0]))
            {
                return None;
            }
        }
        let mut profile = DirectProfile::neutral(buses);
        for (index, (row, default)) in PROCESSING_ROWS
            .into_iter()
            .zip(StripProcessing::DEFAULT.cells())
            .enumerate()
        {
            let header = row.offset + shift;
            let range = header + 12..header + 20;
            let expected_header = [1, row.id, 2].map(u32::to_le_bytes).concat();
            if self.model.get(header..range.start)? != expected_header {
                return None;
            }
            for (column, slot) in self.model.get(range.clone())?.chunks_exact(4).enumerate() {
                profile.tracks[column][index] = i32::from_le_bytes(slot.try_into().ok()?);
            }
            let mut neutral = [0; 8];
            for column in neutral.chunks_exact_mut(4) {
                column.copy_from_slice(&default.to_le_bytes());
            }
            cells.push((range, neutral));
        }
        cells.push((route, [0; 8]));
        let header = bus_pre_gain_row(buses, shift);
        let start = header + 12;
        let range = start..start + 4 * buses as usize;
        if self.model.get(header..start)? != [1, 0x19, buses as u32].map(u32::to_le_bytes).concat()
        {
            return None;
        }
        for (slot, value) in self
            .model
            .get(range.clone())?
            .chunks_exact(4)
            .zip(&mut profile.bus_pre_gain)
        {
            *value = i32::from_le_bytes(slot.try_into().ok()?);
        }
        cells.push((range, [0; 8]));
        for (index, (row, default)) in PROCESSING_ROWS
            .into_iter()
            .zip(StripProcessing::DEFAULT.cells())
            .enumerate()
        {
            let Some(ordinal) = row.bus_ordinal() else {
                continue;
            };
            let header = bus_processing_row(ordinal, buses, shift);
            let range = header + 12..header + 12 + 4 * buses as usize;
            let expected_header = [1, row.id, buses as u32].map(u32::to_le_bytes).concat();
            if self.model.get(header..range.start)? != expected_header {
                return None;
            }
            let mut neutral = [0; 8];
            for (column, slot) in self.model.get(range.clone())?.chunks_exact(4).enumerate() {
                profile.bus_processing[column][index] = i32::from_le_bytes(slot.try_into().ok()?);
                neutral[4 * column..4 * column + 4].copy_from_slice(&default.to_le_bytes());
            }
            cells.push((range, neutral));
        }
        if buses == 2 {
            let [gain, mute, mirror] = bus_2_cells(shift - TWO_BUS_SHIFT);
            if self.model[mute.clone()] != self.model[mirror.clone()] {
                return None;
            }
            cells.extend([gain, mute, mirror].map(|range| (range, [0; 8])));
        }
        // The outer EQ slopes follow the track routing row in the archive.
        cells.sort_unstable_by_key(|(range, _)| range.start);
        let digest = self.digest_with_cells(cells);
        expected.contains(&digest.as_str()).then_some(profile)
    }

    pub(crate) fn write_direct(
        mut self,
        two_bus_donor: Option<&Mixer>,
        mixer: crate::linked::mixer::DirectMixer,
    ) -> Result<String> {
        if self.direct_profile() != Some(DirectProfile::neutral(1))
            || !self.cells_are_neutral(direct_track_cells(0).into_iter().chain([
                DIRECT_PAN_BYPASS..DIRECT_PAN_BYPASS + 8,
                DIRECT_SOLO..DIRECT_SOLO + 8,
            ]))
        {
            return Err("expected a qualified neutral one-bus Fairlight template".into());
        }
        let shift = if mixer.buses == 2 {
            let donor = two_bus_donor.ok_or("missing pinned two-bus Fairlight template")?;
            let cells = direct_track_cells(TWO_BUS_SHIFT)
                .into_iter()
                .chain(bus_2_cells(0));
            if donor.model.len() != 365_437
                || donor.direct_profile() != Some(DirectProfile::neutral(2))
                || !donor.cells_are_neutral(cells)
            {
                return Err("expected a qualified neutral two-bus Fairlight template".into());
            }
            self.model.clone_from(&donor.model);
            TWO_BUS_SHIFT
        } else {
            0
        };
        for (cells, values) in direct_track_cells(shift)
            .into_iter()
            .zip([mixer.pan, mixer.output])
        {
            for (slot, value) in self.model[cells].chunks_exact_mut(4).zip(values) {
                slot.copy_from_slice(&value.to_le_bytes());
            }
        }
        if mixer.buses == 1 {
            for (row, flags) in [
                (DIRECT_PAN_BYPASS, mixer.pan_bypassed),
                (DIRECT_SOLO, mixer.soloed),
            ] {
                for (slot, enabled) in self.model[row..row + 8].chunks_exact_mut(4).zip(flags) {
                    slot.copy_from_slice(&i32::from(enabled).to_le_bytes());
                }
            }
        }
        let [a1, a2] = mixer.processing.map(StripProcessing::cells);
        for (row, (left, right)) in PROCESSING_ROWS.into_iter().zip(a1.into_iter().zip(a2)) {
            let start = row.offset + shift + 12;
            self.model[start..start + 4].copy_from_slice(&left.to_le_bytes());
            self.model[start + 4..start + 8].copy_from_slice(&right.to_le_bytes());
        }
        let start = bus_pre_gain_row(mixer.buses, shift) + 12;
        let end = start + 4 * mixer.buses as usize;
        for (slot, value) in self.model[start..end]
            .chunks_exact_mut(4)
            .zip(mixer.bus_pre_gain)
        {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        for (column, bus) in mixer
            .bus_processing
            .into_iter()
            .take(mixer.buses as usize)
            .enumerate()
        {
            for (row, value) in PROCESSING_ROWS.iter().zip(bus.cells()) {
                let Some(ordinal) = row.bus_ordinal() else {
                    continue;
                };
                let start = bus_processing_row(ordinal, mixer.buses, shift) + 12 + 4 * column;
                self.model[start..start + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        if mixer.buses == 2 {
            let mute = i32::from(mixer.bus_2_muted);
            for (cell, value) in bus_2_cells(0)
                .into_iter()
                .zip([mixer.bus_2_gain, mute, mute])
            {
                self.model[cell].copy_from_slice(&value.to_le_bytes());
            }
        }
        self.encode()
    }

    fn cells_are_neutral(&self, cells: impl IntoIterator<Item = Range<usize>>) -> bool {
        cells
            .into_iter()
            .all(|cell| self.model[cell].iter().all(|&byte| byte == 0))
    }

    pub(crate) fn write(
        mut self,
        tracks: [i32; 2],
        main_bus: i32,
        mutes: [bool; 2],
    ) -> Result<String> {
        if self.track_faders() != Some([0, 0])
            || self.main_bus_fader() != Some(0)
            || self.track_mutes() != Some([false; 2])
            || self.neutral_digest() != SKELETON
        {
            return Err("expected the qualified neutral Fairlight mixer".into());
        }
        for (slot, value) in self.model[TRACK_FADERS].chunks_exact_mut(4).zip(tracks) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        self.model[MAIN_BUS_FADER].copy_from_slice(&main_bus.to_le_bytes());
        for field in TRACK_MUTES {
            for (slot, mute) in self.model[field].chunks_exact_mut(4).zip(mutes) {
                slot.copy_from_slice(&i32::from(mute).to_le_bytes());
            }
        }
        self.encode()
    }

    fn encode(mut self) -> Result<String> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&self.model)?;
        let compressed = encoder.finish()?;
        let mut field = u32::try_from(compressed.len() + 4)?.to_be_bytes().to_vec();
        field.extend_from_slice(&u32::try_from(self.model.len())?.to_be_bytes());
        field.extend_from_slice(&compressed);
        self.fields.splice(self.field, field);
        Ok(hex::encode(self.fields))
    }
}

fn direct_track_cells(shift: usize) -> [Range<usize>; 2] {
    [DIRECT_PAN, DIRECT_ROUTE].map(|start| start + shift..start + shift + 8)
}

fn bus_2_cells(shift: usize) -> [Range<usize>; 3] {
    [BUS_2_FADER, BUS_2_MUTE, BUS_2_MUTE_MIRROR].map(|start| start + shift..start + shift + 4)
}

pub(crate) fn fader_ticks(db: f64) -> Option<i32> {
    let ticks = (db * 10.0).round() as i32;
    (valid_ticks(ticks) && f64::from(ticks) / 10.0 == db).then_some(ticks)
}

pub(crate) fn valid_ticks(ticks: i32) -> bool {
    (-60..=0).contains(&ticks)
}
