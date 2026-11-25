//! Positions in a zone's text. Antidote counts in a [`Unit`]; MailALM's editor in UTF-8 bytes.
//! Every conversion is checked: a position past the end, or inside a character, is refused.

use std::ops::Range;

/// What Antidote's positions count.
///
/// Druide's Python package treats every position as an index into a Python `str`, that is a count
/// of Unicode code points: that is the default. Antidote itself may count UTF-16 code units (as
/// Qt and JavaScript do); the two differ only for characters outside the Basic Multilingual Plane
/// (emoji, some CJK), and the choice is kept open until it is tried with Antidote 12.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    /// One per Unicode scalar value: a Python `str` index, as in Druide's package.
    #[default]
    CodePoints,
    /// One per UTF-16 code unit: a character outside the BMP counts two.
    Utf16,
}

impl Unit {
    fn width(self, c: char) -> usize {
        match self {
            Unit::CodePoints => 1,
            Unit::Utf16 => c.len_utf16(),
        }
    }
}

/// The length of `text` in `unit`s.
pub(crate) fn length(text: &str, unit: Unit) -> usize {
    text.chars().map(|c| unit.width(c)).sum()
}

/// The byte offset of `position` (in `unit`s) in `text`; `None` past the end, or inside a
/// character (half of a UTF-16 surrogate pair).
pub(crate) fn to_byte(text: &str, position: usize, unit: Unit) -> Option<usize> {
    let mut at = 0usize;
    for (byte, c) in text.char_indices() {
        if at == position {
            return Some(byte);
        }
        if at > position {
            return None;
        }
        at = at.saturating_add(unit.width(c));
    }
    (at == position).then_some(text.len())
}

/// The position (in `unit`s) of byte offset `byte` of `text`; `None` past the end or off a
/// character boundary.
pub(crate) fn from_byte(text: &str, byte: usize, unit: Unit) -> Option<usize> {
    text.get(..byte).map(|head| length(head, unit))
}

/// The byte range of Antidote's `start..end`; `None` if either end is invalid or `start > end`.
pub(crate) fn to_byte_range(text: &str, start: usize, end: usize, unit: Unit) -> Option<Range<usize>> {
    if start > end {
        return None;
    }
    Some(to_byte(text, start, unit)?..to_byte(text, end, unit)?)
}

/// Antidote's `(start, end)` for the byte range `range` of `text`; `None` if it is not a valid
/// range of `text` on character boundaries.
pub(crate) fn from_byte_range(text: &str, range: &Range<usize>, unit: Unit) -> Option<(usize, usize)> {
    if range.start > range.end {
        return None;
    }
    Some((from_byte(text, range.start, unit)?, from_byte(text, range.end, unit)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // "Été 🍎 je manges": É and é take 2 bytes, 🍎 takes 4 bytes and 2 UTF-16 units.
    const TEXT: &str = "Été 🍎 je manges";

    #[test]
    fn lengths() {
        assert_eq!(TEXT.len(), 20);
        assert_eq!(length(TEXT, Unit::CodePoints), 15);
        assert_eq!(length(TEXT, Unit::Utf16), 16);
        assert_eq!(length("", Unit::Utf16), 0);
    }

    #[test]
    fn code_points_to_bytes_and_back() {
        // "manges" is 9..15 in code points, 14..20 in bytes.
        assert_eq!(to_byte_range(TEXT, 9, 15, Unit::CodePoints), Some(14..20));
        assert_eq!(TEXT.get(14..20), Some("manges"));
        assert_eq!(from_byte_range(TEXT, &(14..20), Unit::CodePoints), Some((9, 15)));
        // "Été" is 0..3, bytes 0..5; the apple is 4..5, bytes 6..10.
        assert_eq!(to_byte_range(TEXT, 0, 3, Unit::CodePoints), Some(0..5));
        assert_eq!(to_byte_range(TEXT, 4, 5, Unit::CodePoints), Some(6..10));
        assert_eq!(to_byte(TEXT, 15, Unit::CodePoints), Some(20));
        assert_eq!(to_byte(TEXT, 16, Unit::CodePoints), None);
    }

    #[test]
    fn utf16_to_bytes_and_back() {
        // After the apple, UTF-16 positions are one more than code points.
        assert_eq!(to_byte_range(TEXT, 10, 16, Unit::Utf16), Some(14..20));
        assert_eq!(from_byte_range(TEXT, &(14..20), Unit::Utf16), Some((10, 16)));
        assert_eq!(to_byte_range(TEXT, 4, 6, Unit::Utf16), Some(6..10));
        // Position 5 is between the two halves of the apple's surrogate pair.
        assert_eq!(to_byte(TEXT, 5, Unit::Utf16), None);
        assert_eq!(to_byte(TEXT, 17, Unit::Utf16), None);
        assert_eq!(to_byte("🍎", 1, Unit::Utf16), None);
        assert_eq!(to_byte("🍎", 2, Unit::Utf16), Some(4));
    }

    #[test]
    fn invalid_ranges_are_refused() {
        assert_eq!(to_byte_range(TEXT, 5, 4, Unit::CodePoints), None);
        assert_eq!(to_byte_range(TEXT, 0, usize::MAX, Unit::CodePoints), None);
        // Byte 1 is inside "É", byte 7 inside the apple.
        assert_eq!(from_byte(TEXT, 1, Unit::CodePoints), None);
        assert_eq!(from_byte_range(TEXT, &(0..7), Unit::Utf16), None);
        assert_eq!(from_byte_range(TEXT, &(5..2), Unit::Utf16), None);
        assert_eq!(from_byte(TEXT, 21, Unit::CodePoints), None);
        assert_eq!(from_byte(TEXT, 20, Unit::CodePoints), Some(15));
        assert_eq!(to_byte("", 0, Unit::CodePoints), Some(0));
    }
}
