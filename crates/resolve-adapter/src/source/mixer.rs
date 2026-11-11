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
const MODEL_SIZES: [usize; 2] = [364_028, 364_088];
const TRACK_FADERS: Range<usize> = 203_093..203_101;
const TRACK_MUTES: [Range<usize>; 2] = [203_113..203_121, 203_133..203_141];
const TRACK_MUTE_HEADER: [u8; 12] = [1, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0];
const MAIN_BUS_FADER: Range<usize> = 235_340..235_344;
const SKELETON: &str = "5bfebb8fa74dacf3d8185bfe459d719b4824cf3ba623d11c9d873a2cace5adb8";
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
        if !MODEL_SIZES.contains(&size) {
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
        TRACK_MUTES
            .iter()
            .any(|range| self.model[range.clone()] != [0; 8])
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

    fn neutral_digest(&self) -> String {
        let mut hash = Sha256::new();
        let mut end = 0;
        for field in [
            TRACK_FADERS,
            TRACK_MUTES[0].clone(),
            TRACK_MUTES[1].clone(),
            MAIN_BUS_FADER,
        ] {
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

pub(crate) fn fader_ticks(db: f64) -> Option<i32> {
    let ticks = (db * 10.0).round() as i32;
    (valid_ticks(ticks) && f64::from(ticks) / 10.0 == db).then_some(ticks)
}

pub(crate) fn valid_ticks(ticks: i32) -> bool {
    (-60..=0).contains(&ticks)
}
