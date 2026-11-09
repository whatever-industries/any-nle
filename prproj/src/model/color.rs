//! The `Color` value type (port of `py_premiere/models/color.py`).

/// An RGBA colour with 8-bit channels (Python `Color(red, green, blue, alpha=255)`). Channels
/// are `i64` so out-of-range values can reach `validate_color`, as they can in Python.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub red: i64,
    /// Green.
    pub green: i64,
    /// Blue.
    pub blue: i64,
    /// Alpha.
    pub alpha: i64,
}

impl Color {
    /// `Color(red, green, blue)` (alpha 255).
    pub fn rgb(red: i64, green: i64, blue: i64) -> Color {
        Color { red, green, blue, alpha: 255 }
    }

    /// `Color(red, green, blue, alpha)`.
    pub fn rgba(red: i64, green: i64, blue: i64, alpha: i64) -> Color {
        Color { red, green, blue, alpha }
    }

    /// The channels in tuple order (red, green, blue, alpha).
    pub fn channels(self) -> [i64; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }
}
