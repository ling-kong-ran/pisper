//! Lossless JavaScript-string operations used inside the LaTeX renderer.
//!
//! Positions are UTF-16 code-unit offsets. Scalar iteration is explicit and
//! retains lone surrogates as their original u32 values. No sentinel encoding
//! or early U+FFFD substitution is used for stored/rendered text.

use std::slice::SliceIndex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Text(pub(super) Vec<u16>);

pub(super) trait Append {
    fn append_to(self, target: &mut Vec<u16>);
}

impl Append for &str {
    fn append_to(self, target: &mut Vec<u16>) {
        target.extend(self.encode_utf16());
    }
}

impl Append for String {
    fn append_to(self, target: &mut Vec<u16>) {
        self.as_str().append_to(target);
    }
}

impl Append for char {
    fn append_to(self, target: &mut Vec<u16>) {
        target.extend_from_slice(self.encode_utf16(&mut [0; 2]));
    }
}

impl Append for &Text {
    fn append_to(self, target: &mut Vec<u16>) {
        target.extend_from_slice(&self.0);
    }
}

impl Append for Text {
    fn append_to(self, target: &mut Vec<u16>) {
        target.extend(self.0);
    }
}

impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self(value.encode_utf16().collect())
    }
}

impl From<char> for Text {
    fn from(value: char) -> Self {
        let mut text = Self::default();
        text.push(value);
        text
    }
}

impl PartialEq<&str> for Text {
    fn eq(&self, other: &&str) -> bool {
        self.0.iter().copied().eq(other.encode_utf16())
    }
}

impl AsRef<[u16]> for Text {
    fn as_ref(&self) -> &[u16] {
        &self.0
    }
}

impl Text {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn slice(&self, range: impl SliceIndex<[u16], Output = [u16]>) -> Self {
        Self(self.0[range].to_vec())
    }

    pub(super) fn push(&mut self, value: impl Append) {
        value.append_to(&mut self.0);
    }

    pub(super) fn push_unit(&mut self, unit: u16) {
        self.0.push(unit);
    }

    pub(super) fn push_point(&mut self, point: u32) {
        if let Some(scalar) = char::from_u32(point) {
            self.push(scalar);
        } else {
            debug_assert!((0xd800..=0xdfff).contains(&point));
            self.push_unit(point as u16);
        }
    }

    pub(super) fn truncate(&mut self, units: usize) {
        self.0.truncate(units);
    }

    /// JavaScript for-of / Array.from: combine valid pairs, keep lone units.
    pub(super) fn points(&self) -> impl Iterator<Item = u32> + '_ {
        char::decode_utf16(self.0.iter().copied()).map(|point| match point {
            Ok(scalar) => scalar as u32,
            Err(error) => u32::from(error.unpaired_surrogate()),
        })
    }

    /// Syntax discrimination only. The caller still copies the original unit.
    /// Surrogates cannot match any of the parser's BMP syntax/whitespace tests.
    pub(super) fn syntax_at(&self, position: usize) -> Option<char> {
        self.0
            .get(position)
            .map(|&unit| char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    pub(super) fn to_utf8(&self) -> Option<String> {
        String::from_utf16(&self.0).ok()
    }

    pub(super) fn find(&self, pattern: &str) -> Option<usize> {
        let pattern: Vec<_> = pattern.encode_utf16().collect();
        self.0
            .windows(pattern.len())
            .position(|window| window == pattern)
    }

    pub(super) fn starts_with(&self, pattern: &str) -> bool {
        self.0
            .starts_with(&pattern.encode_utf16().collect::<Vec<_>>())
    }

    pub(super) fn ends_with(&self, pattern: char) -> bool {
        self.0.ends_with(pattern.encode_utf16(&mut [0; 2]))
    }

    pub(super) fn split(&self, pattern: &str) -> Vec<Self> {
        let needle: Vec<_> = pattern.encode_utf16().collect();
        assert!(!needle.is_empty());
        let mut parts = Vec::new();
        let mut at = 0;
        while let Some(relative) = self.0[at..].windows(needle.len()).position(|w| w == needle) {
            let end = at + relative;
            parts.push(self.slice(at..end));
            at = end + needle.len();
        }
        parts.push(self.slice(at..));
        parts
    }

    pub(super) fn join<T: AsRef<[u16]>>(
        parts: impl IntoIterator<Item = T>,
        separator: &str,
    ) -> Self {
        let mut result = Self::new();
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                result.push(separator);
            }
            result.0.extend_from_slice(part.as_ref());
        }
        result
    }

    pub(super) fn replace(&self, pattern: char, replacement: &str) -> Self {
        Self::join(self.split(pattern.encode_utf8(&mut [0; 4])), replacement)
    }
}
