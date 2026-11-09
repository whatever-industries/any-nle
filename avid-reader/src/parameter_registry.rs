//! Display names for parameter UUIDs, derived from pinned py-avb source.
//!
//! These names were extracted by py-avb from AAF files. They are useful hints for
//! inspecting parameters, not independently verified AVB or Avid UI semantics.
//! The source and generated table are pinned in `tools/generate_parameter_registry.py`.

use crate::io::Uuid;

include!("parameter_registry_data.rs");

/// Look up a canonical lowercase, hyphenated UUID string. Like py-avb's
/// `PARAMETER_UUIDS.get`, this returns no name for an unknown UUID. Names are
/// not unique and must not be used as stable parameter identities.
pub fn parameter_name_str(uuid: &str) -> Option<&'static str> {
    PARAMETER_UUID_NAMES
        .binary_search_by(|(candidate, _)| candidate.cmp(&uuid))
        .ok()
        .map(|index| PARAMETER_UUID_NAMES[index].1)
}

/// Typed lookup using the same canonical spelling as py-avb's `str(uuid)`.
pub fn parameter_name(uuid: &Uuid) -> Option<&'static str> {
    parameter_name_str(&uuid.to_string())
}

/// Match py-avb's `parameter_name` fallback for an unknown UUID.
pub fn parameter_display_name(uuid: &Uuid) -> String {
    parameter_name(uuid)
        .map(str::to_owned)
        .unwrap_or_else(|| uuid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_complete_sorted_and_keyed_by_uuid() {
        assert_eq!(PARAMETER_UUID_NAMES.len(), 735);
        for pair in PARAMETER_UUID_NAMES.windows(2) {
            assert!(pair[0].0 < pair[1].0);
        }
        assert_eq!(
            parameter_name_str("071f3b3a-4ddd-46a8-a39f-1ab87fcc4437"),
            Some("Red")
        );
        assert_eq!(
            parameter_name_str("0adba400-0bb6-43f3-a435-adb44045bab3"),
            Some("Opacity")
        );
        assert_eq!(
            parameter_name_str("fbd10010-d826-4dc6-972d-9a602d2478d8"),
            Some("Custom Column Name")
        );
    }

    #[test]
    fn duplicate_names_do_not_collapse_distinct_ids() {
        assert_eq!(
            parameter_name_str("0082b859-87f0-426d-85de-ff14e5c9080b"),
            Some("Blue")
        );
        assert_eq!(
            parameter_name_str("0aa39532-3255-4e74-a991-95fe17600e07"),
            Some("Blue")
        );
    }

    #[test]
    fn typed_lookup_and_unknown_fallback() {
        let known = Uuid {
            data1: 0x071f3b3a,
            data2: 0x4ddd,
            data3: 0x46a8,
            data4: [0xa3, 0x9f, 0x1a, 0xb8, 0x7f, 0xcc, 0x44, 0x37],
        };
        assert_eq!(parameter_name(&known), Some("Red"));
        let unknown = Uuid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        };
        assert_eq!(parameter_name(&unknown), None);
        assert_eq!(
            parameter_display_name(&unknown),
            "00000000-0000-0000-0000-000000000000"
        );
    }
}
