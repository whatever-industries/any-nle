//! Python `str` semantics the serializer and payload code depend on.
//!
//! Python's `str.isspace()`, `str.strip()` and the regex class `\s` all use
//! the same whitespace set, which is Rust's `char::is_whitespace` plus the
//! four information separators U+001C..U+001F.

/// Python's notion of a whitespace character.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `s.isspace()`: non-empty and every character is whitespace.
pub fn py_isspace(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_py_space)
}

/// Python `s.strip()` (no arguments).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python `re.sub(r"\s+", "", s)`.
pub fn remove_py_space(s: &str) -> String {
    s.chars().filter(|&c| !is_py_space(c)).collect()
}

/// Python `s.isdigit()` restricted to what `int(s)` also accepts for the
/// ObjectID high-water mark: one or more ASCII digits.
///
/// Python's `isdigit` is Unicode-aware (`"²".isdigit()` is true), but no
/// Premiere ObjectID has ever carried a non-ASCII digit; a non-ASCII digit
/// string would make Python's `int()` either succeed (Arabic-Indic digits) or
/// raise. This port treats such IDs as non-numeric.
pub fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_whitespace() {
        assert!(py_isspace("\n\t\t"));
        assert!(py_isspace("\u{1c}"));
        assert!(!py_isspace(""));
        assert!(!py_isspace(" a "));
        assert_eq!(py_strip(" \u{a0}ab\n"), "ab");
        assert_eq!(remove_py_space("a b\nc"), "abc");
    }
}
