//! Static faders and track mute in the qualified two-mono Fairlight mixer.
use super::encoding::qt;
use crate::Result;
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
const DIRECT_TWO_FAIRLIGHT_SAVE: &str =
    "83d76c19c40bbb9d75afb2bea828387fb364303b8da9885fd4eaf5185582650c";
const DIRECT_PAN: usize = 203_173;
const DIRECT_ROUTE: usize = 211_685;
const TWO_BUS_SHIFT: usize = 8;
const BUS_2_FADER: usize = 235_352;
const BUS_2_MUTE: usize = 235_372;
const BUS_2_MUTE_MIRROR: usize = 235_392;

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

impl Mixer {
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
    ) -> Option<(graph::Value, bool)> {
        let (mut facts, active) = super::fairlight::mixer(&self.model, tracks, typed_gain_mute)?;
        if active {
            if let (graph::Value::Obj(fields), Some(buses)) = (&mut facts, self.direct_profile()) {
                fields.push((DIRECT_PROFILE.into(), graph::Value::Int(buses)));
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
        let mut hash = Sha256::new();
        let mut end = 0;
        for field in cells {
            hash.update(&self.model[end..field.start]);
            hash.update(&[0; 8][..field.len()]);
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

    fn direct_profile(&self) -> Option<i64> {
        if matches!(self.model.len(), 364_028 | 364_088 | 364_092) {
            let digest = self.digest_with_neutral_cells(direct_track_cells(0));
            return DIRECT_ONE.contains(&digest.as_str()).then_some(1);
        }
        // Saving on the Fairlight page adds 40 bytes before the control tables
        // in the observed project. Both layouts have an independent fingerprint.
        let (offset, expected) = match self.model.len() {
            365_437 => (0, DIRECT_TWO),
            365_477 => (40, DIRECT_TWO_FAIRLIGHT_SAVE),
            _ => return None,
        };
        let [gain, mute, mirror] = bus_2_cells(offset);
        if self.model[mute.clone()] != self.model[mirror.clone()] {
            return None;
        }
        let cells = direct_track_cells(TWO_BUS_SHIFT + offset)
            .into_iter()
            .chain([gain, mute, mirror]);
        (self.digest_with_neutral_cells(cells) == expected).then_some(2)
    }

    pub(crate) fn write_direct(
        mut self,
        two_bus_donor: Option<&Mixer>,
        mixer: crate::linked::mixer::DirectMixer,
    ) -> Result<String> {
        if self.direct_profile() != Some(1) || !self.cells_are_neutral(direct_track_cells(0)) {
            return Err("expected a qualified neutral one-bus Fairlight template".into());
        }
        let shift = if mixer.buses == 2 {
            let donor = two_bus_donor.ok_or("missing pinned two-bus Fairlight template")?;
            let cells = direct_track_cells(TWO_BUS_SHIFT)
                .into_iter()
                .chain(bus_2_cells(0));
            if donor.model.len() != 365_437
                || donor.direct_profile() != Some(2)
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
