//! Native sequence marker colors. Clip colors have a separate code table.
const COLORS: [(u64, &str); 16] = [
    (2, "Blue"),
    (4, "Cyan"),
    (8, "Green"),
    (16, "Yellow"),
    (32, "Red"),
    (64, "Pink"),
    (128, "Purple"),
    (512, "Fuchsia"),
    (1024, "Rose"),
    (2048, "Lavender"),
    (4096, "Sky"),
    (8192, "Mint"),
    (16384, "Lemon"),
    (32768, "Sand"),
    (65536, "Cocoa"),
    (131072, "Cream"),
];

pub(crate) fn name(bit: u64) -> Option<&'static str> {
    COLORS
        .into_iter()
        .find_map(|(code, name)| (code == bit).then_some(name))
}

pub(crate) fn bit(name: &str) -> Option<u64> {
    COLORS
        .into_iter()
        .find_map(|(bit, saved)| (name == saved).then_some(bit))
}
