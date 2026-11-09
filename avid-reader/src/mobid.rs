//! Public MobID conversion helpers based on pinned pyavb `mobid.py`.
//! Source: ff64e06b60077ecabe5db3fef239898b08363898 (MIT; see NOTICE).
//! These helpers do not validate a UMID against the SMPTE standard or prove
//! that an arbitrary ID is accepted by Avid.

use crate::io::{Error, MobId, Result, Uuid};
use std::str::FromStr;

/// Typed equivalent of py-avb's `MobID.to_dict()` / `from_dict()` shape.
/// The nested `material` fields are the UUID's Data1..Data4 components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobIdParts {
    pub smpte_label: [u8; 12],
    pub length: u8,
    pub instance_high: u8,
    pub instance_mid: u8,
    pub instance_low: u8,
    pub material: Uuid,
}

impl MobId {
    pub fn to_parts(&self) -> MobIdParts {
        MobIdParts {
            smpte_label: self.label,
            length: self.length,
            instance_high: self.instance[0],
            instance_mid: self.instance[1],
            instance_low: self.instance[2],
            material: self.material,
        }
    }

    pub fn from_parts(parts: MobIdParts) -> Self {
        Self {
            label: parts.smpte_label,
            length: parts.length,
            instance: [parts.instance_high, parts.instance_mid, parts.instance_low],
            material: parts.material,
        }
    }
    /// Construct pyavb's `UniqueMobID` layout with a caller-supplied UUID.
    /// Random UUID generation is deliberately left to the caller.
    pub fn unique_with_material(material: Uuid) -> Self {
        Self {
            label: [
                0x06, 0x0a, 0x2b, 0x34, 0x01, 0x01, 0x01, 0x05, 0x01, 0x01, 0x0f, 0x20,
            ],
            length: 0x13,
            instance: [0, 0, 0],
            material,
        }
    }

    /// The pyavb `bytes_le` representation. Its final 16 bytes store the UUID
    /// fields in little-endian order regardless of AVB file byte order.
    pub fn to_pyavb_bytes_le(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..12].copy_from_slice(&self.label);
        out[12] = self.length;
        out[13..16].copy_from_slice(&self.instance);
        out[16..20].copy_from_slice(&self.material.data1.to_le_bytes());
        out[20..22].copy_from_slice(&self.material.data2.to_le_bytes());
        out[22..24].copy_from_slice(&self.material.data3.to_le_bytes());
        out[24..32].copy_from_slice(&self.material.data4);
        out
    }

    pub fn from_pyavb_bytes_le(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(Error::new(
                format!("MobID bytes_le requires 32 bytes, got {}", bytes.len()),
                0,
            ));
        }
        let mut label = [0u8; 12];
        label.copy_from_slice(&bytes[..12]);
        let mut instance = [0u8; 3];
        instance.copy_from_slice(&bytes[13..16]);
        let mut data4 = [0u8; 8];
        data4.copy_from_slice(&bytes[24..32]);
        Ok(Self {
            label,
            length: bytes[12],
            instance,
            material: Uuid {
                data1: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
                data2: u16::from_le_bytes(bytes[20..22].try_into().unwrap()),
                data3: u16::from_le_bytes(bytes[22..24].try_into().unwrap()),
                data4,
            },
        })
    }

    /// Parse pyavb's URN form, including the legacy half-swapped material layout.
    /// Like pyavb, `urn:smpte:umid:`, dots, hyphens, and `0x` decorations are
    /// removed before 64 hex digits are interpreted. Malformed input is rejected.
    pub fn from_urn(value: &str) -> Result<Self> {
        let normalized = value
            .to_ascii_lowercase()
            .replace("urn:smpte:umid:", "")
            .replace('.', "")
            .replace('-', "")
            .replace("0x", "");
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::new("MobID URN requires exactly 64 hex digits", 0));
        }
        let mut raw = [0u8; 32];
        for (index, byte) in raw.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&normalized[index * 2..index * 2 + 2], 16)
                .map_err(|_| Error::new("invalid MobID hex digit", index * 2))?;
        }
        let mut label = [0u8; 12];
        label.copy_from_slice(&raw[..12]);
        let mut instance = [0u8; 3];
        instance.copy_from_slice(&raw[13..16]);
        let material = &raw[16..];
        let swapped = label[11] == 0 && material[..6] == [0x06, 0x0e, 0x2b, 0x34, 0x7f, 0x7f];
        let uuid = if swapped {
            let mut data4 = [0u8; 8];
            data4.copy_from_slice(&material[..8]);
            Uuid {
                data1: u32::from_be_bytes(material[8..12].try_into().unwrap()),
                data2: u16::from_be_bytes(material[12..14].try_into().unwrap()),
                data3: u16::from_be_bytes(material[14..16].try_into().unwrap()),
                data4,
            }
        } else {
            let mut data4 = [0u8; 8];
            data4.copy_from_slice(&material[8..16]);
            Uuid {
                data1: u32::from_be_bytes(material[..4].try_into().unwrap()),
                data2: u16::from_be_bytes(material[4..6].try_into().unwrap()),
                data3: u16::from_be_bytes(material[6..8].try_into().unwrap()),
                data4,
            }
        };
        Ok(Self {
            label,
            length: raw[12],
            instance,
            material: uuid,
        })
    }
}

impl FromStr for MobId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::from_urn(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixed outputs from pinned pyavb MobID(...), `.urn`, `.bytes_le`, `.to_dict`.
    const ORDINARY: &str =
        "urn:smpte:umid:060a2b34.01010101.01010f20.13010203.12345678.9abcdef0.11223344.55667788";
    const ORDINARY_LE: &str = "060a2b340101010101010f201301020378563412bc9af0de1122334455667788";
    const SWAPPED: &str =
        "urn:smpte:umid:060a2b34.01010101.01010f00.13000000.060e2b34.7f7f2a80.4fa5c20f.4e301e50";
    const SWAPPED_LE: &str = "060a2b340101010101010f00130000000fc2a54f304e501e060e2b347f7f2a80";

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn pyavb_ordinary_and_half_swapped_vectors() {
        for (urn, bytes) in [(ORDINARY, ORDINARY_LE), (SWAPPED, SWAPPED_LE)] {
            let parsed = MobId::from_urn(urn).unwrap();
            assert_eq!(parsed.urn(), urn);
            assert_eq!(parsed.to_pyavb_bytes_le().as_slice(), hex(bytes));
            assert_eq!(MobId::from_pyavb_bytes_le(&hex(bytes)).unwrap(), parsed);
            assert_eq!(urn.parse::<MobId>().unwrap(), parsed);
        }
        let normal = MobId::from_urn(ORDINARY).unwrap();
        assert_eq!(normal.material.data1, 305419896);
        assert_eq!(normal.material.data2, 39612);
        assert_eq!(normal.material.data3, 57072);
        assert_eq!(normal.material.data4, [17, 34, 51, 68, 85, 102, 119, 136]);
        let swapped = MobId::from_urn(SWAPPED).unwrap();
        assert_eq!(swapped.material.data1, 1336263183);
        assert_eq!(swapped.material.data2, 20016);
        assert_eq!(swapped.material.data3, 7760);
        assert_eq!(swapped.material.data4, [6, 14, 43, 52, 127, 127, 42, 128]);
    }

    #[test]
    fn unique_layout_is_deterministic_with_supplied_uuid() {
        let material = Uuid {
            data1: 0x12345678,
            data2: 0x9abc,
            data3: 0xdef0,
            data4: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
        };
        let id = MobId::unique_with_material(material);
        assert_eq!(id.label, [0x06, 0x0a, 0x2b, 0x34, 1, 1, 1, 5, 1, 1, 15, 32]);
        assert_eq!(id.length, 19);
        assert_eq!(id.instance, [0, 0, 0]);
        assert_eq!(id.material, material);
        assert_eq!(MobId::from_urn(&id.urn()).unwrap(), id);
        let parts = id.to_parts();
        assert_eq!(parts.instance_high, 0);
        assert_eq!(parts.instance_mid, 0);
        assert_eq!(parts.instance_low, 0);
        assert_eq!(parts.material, material);
        assert_eq!(MobId::from_parts(parts), id);
    }

    #[test]
    fn malformed_inputs_fail_without_panicking() {
        for bad in [
            "",
            "urn:smpte:umid:1234",
            "urn:smpte:umid:zz",
            "💥",
            "0".repeat(65).as_str(),
        ] {
            assert!(MobId::from_urn(bad).is_err());
        }
        for len in [0, 1, 31, 33, 100] {
            assert!(MobId::from_pyavb_bytes_le(&vec![0; len]).is_err());
        }
    }
}
