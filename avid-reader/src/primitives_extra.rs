//! Additional py-avb primitive behavior, pinned to
//! `ff64e06b60077ecabe5db3fef239898b08363898` (MIT; see NOTICE).
//!
//! AVB timestamps are unsigned Unix seconds. py-avb converts them through the
//! process's local timezone. This module keeps the stored instant explicit and
//! offers deterministic UTC civil conversion instead. Callers that need a
//! local display should choose their own timezone outside the serialization
//! layer.

use crate::io::{Error, MobId, Reader, Result, Uuid};
use crate::writer_io::Writer;

/// Typed input equivalent to py-avb's partially populated `MobID.from_dict`
/// material mapping. Missing keys become zero, as in the pinned source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MobIdMaterialInput {
    pub data1: Option<u32>,
    pub data2: Option<u16>,
    pub data3: Option<u16>,
    pub data4: Option<[u8; 8]>,
}

/// Typed input for py-avb's `MobID.from_dict` defaulting behavior. For fully
/// populated dictionaries, `MobId::to_parts` / `from_parts` already provide a
/// lossless fixed-field representation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MobIdDictionaryInput {
    pub smpte_label: Option<[u8; 12]>,
    pub length: Option<u8>,
    pub instance_high: Option<u8>,
    pub instance_mid: Option<u8>,
    pub instance_low: Option<u8>,
    pub material: Option<MobIdMaterialInput>,
}

impl MobIdDictionaryInput {
    pub fn into_mob_id(self) -> MobId {
        let material = self.material.unwrap_or_default();
        MobId {
            label: self.smpte_label.unwrap_or([0; 12]),
            length: self.length.unwrap_or(0),
            instance: [
                self.instance_high.unwrap_or(0),
                self.instance_mid.unwrap_or(0),
                self.instance_low.unwrap_or(0),
            ],
            material: Uuid {
                data1: material.data1.unwrap_or(0),
                data2: material.data2.unwrap_or(0),
                data3: material.data3.unwrap_or(0),
                data4: material.data4.unwrap_or([0; 8]),
            },
        }
    }
}

/// An AVB timestamp as stored: whole seconds since 1970-01-01T00:00:00Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvbTimestamp(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl AvbTimestamp {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self> {
        reader.u32().map(Self)
    }

    pub fn write(self, writer: &mut Writer) -> Result<()> {
        writer.u32(self.0)
    }

    pub fn to_utc(self) -> UtcDateTime {
        let days = (self.0 / 86_400) as i64;
        let seconds = self.0 % 86_400;
        let (year, month, day) = civil_from_days(days);
        UtcDateTime {
            year,
            month,
            day,
            hour: (seconds / 3_600) as u8,
            minute: ((seconds % 3_600) / 60) as u8,
            second: (seconds % 60) as u8,
        }
    }

    pub fn from_utc(value: UtcDateTime) -> Result<Self> {
        if !(1..=12).contains(&value.month)
            || value.day == 0
            || value.day > days_in_month(value.year, value.month)
            || value.hour > 23
            || value.minute > 59
            || value.second > 59
        {
            return Err(Error::new("invalid UTC date/time", 0));
        }
        let days = days_from_civil(value.year, value.month, value.day);
        let seconds = days * 86_400
            + value.hour as i64 * 3_600
            + value.minute as i64 * 60
            + value.second as i64;
        let seconds = u32::try_from(seconds)
            .map_err(|_| Error::new("UTC date/time outside AVB u32 timestamp range", 0))?;
        Ok(Self(seconds))
    }
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

// Gregorian civil-date algorithms with the epoch offset for 1970-01-01.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = year as i64 - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// py-avb's `UniqueMobID` uses UUIDv4 for its material number. This method
/// obtains all 16 random bytes from the operating system on supported targets,
/// then sets RFC 4122 version and variant bits. It deliberately makes no
/// Avid-identity guarantee.
pub fn new_unique_mob_id() -> Result<MobId> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| Error::new(format!("OS randomness unavailable: {error}"), 0))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut data4 = [0u8; 8];
    data4.copy_from_slice(&bytes[8..16]);
    Ok(MobId::unique_with_material(Uuid {
        data1: u32::from_be_bytes(bytes[0..4].try_into().unwrap()),
        data2: u16::from_be_bytes(bytes[4..6].try_into().unwrap()),
        data3: u16::from_be_bytes(bytes[6..8].try_into().unwrap()),
        data4,
    }))
}

/// Write one extension marker and its one-byte tag atomically with respect to
/// the writer's size limit. `Reader::next_ext` remains the bounded reader.
pub fn write_extension_tag(writer: &mut Writer, tag: u8) -> Result<()> {
    writer.bytes(&[0x01, tag])
}

/// Write an assertion/version tag; `Reader::assert_tag` checks it on read.
pub fn write_assert_tag(writer: &mut Writer, tag: u8) -> Result<()> {
    writer.u8(tag)
}

/// Write a bounded sequence of extension markers in a single append. The
/// caller writes the following non-0x01 field or terminator separately.
pub fn write_extension_tags(writer: &mut Writer, tags: &[u8], max_tags: usize) -> Result<()> {
    if tags.len() > max_tags {
        return Err(Error::new(
            "extension tag count exceeds limit",
            writer.len(),
        ));
    }
    let size = tags
        .len()
        .checked_mul(2)
        .ok_or_else(|| Error::new("extension tag size overflow", writer.len()))?;
    let mut bytes = Vec::with_capacity(size);
    for tag in tags {
        bytes.extend_from_slice(&[0x01, *tag]);
    }
    writer.bytes(&bytes)
}
