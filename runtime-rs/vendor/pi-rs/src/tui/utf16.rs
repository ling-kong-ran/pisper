//! Lossless UTF-16 text for stages that must match JavaScript strings.
//!
//! Code-unit positions and lone surrogates are intentional. There is no Display
//! implementation: callers must explicitly choose the final UTF-8 boundary.

use std::slice::SliceIndex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utf16Text(pub(crate) Vec<u16>);

/// Inputs that can be appended without converting existing units to UTF-8.
pub trait Utf16Append {
    fn append_to(self, output: &mut Vec<u16>);
}
impl Utf16Append for &str {
    fn append_to(self, output: &mut Vec<u16>) {
        output.extend(self.encode_utf16());
    }
}
impl Utf16Append for String {
    fn append_to(self, output: &mut Vec<u16>) {
        self.as_str().append_to(output);
    }
}
impl Utf16Append for &String {
    fn append_to(self, output: &mut Vec<u16>) {
        self.as_str().append_to(output);
    }
}
impl Utf16Append for char {
    fn append_to(self, output: &mut Vec<u16>) {
        output.extend_from_slice(self.encode_utf16(&mut [0; 2]));
    }
}
impl Utf16Append for &Utf16Text {
    fn append_to(self, output: &mut Vec<u16>) {
        output.extend_from_slice(&self.0);
    }
}
impl Utf16Append for Utf16Text {
    fn append_to(self, output: &mut Vec<u16>) {
        output.extend(self.0);
    }
}
impl From<&str> for Utf16Text {
    fn from(text: &str) -> Self {
        Self(text.encode_utf16().collect())
    }
}
impl From<String> for Utf16Text {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}
impl From<&String> for Utf16Text {
    fn from(text: &String) -> Self {
        Self::from(text.as_str())
    }
}
impl From<&Utf16Text> for Utf16Text {
    fn from(text: &Utf16Text) -> Self {
        text.clone()
    }
}
impl From<char> for Utf16Text {
    fn from(c: char) -> Self {
        let mut text = Self::new();
        text.push(c);
        text
    }
}
impl AsRef<[u16]> for Utf16Text {
    fn as_ref(&self) -> &[u16] {
        &self.0
    }
}
impl PartialEq<&str> for Utf16Text {
    fn eq(&self, other: &&str) -> bool {
        self.0.iter().copied().eq(other.encode_utf16())
    }
}
impl Utf16Text {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }
    pub fn into_units(self) -> Vec<u16> {
        self.0
    }
    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
    pub fn to_string_checked(&self) -> Result<String, std::string::FromUtf16Error> {
        String::from_utf16(&self.0)
    }
    /// Number of UTF-16 code units, not Unicode scalars or terminal cells.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn truncate(&mut self, units: usize) {
        self.0.truncate(units);
    }
    pub fn push(&mut self, value: impl Utf16Append) {
        value.append_to(&mut self.0);
    }
    pub fn push_str(&mut self, value: impl Utf16Append) {
        self.push(value);
    }
    pub fn slice(&self, range: impl SliceIndex<[u16], Output = [u16]>) -> Self {
        Self(self.0[range].to_vec())
    }
    pub fn find(&self, needle: impl Into<Self>) -> Option<usize> {
        let needle = needle.into();
        if needle.is_empty() {
            return Some(0);
        }
        self.0.windows(needle.len()).position(|w| w == needle.0)
    }
    pub fn contains(&self, needle: impl Into<Self>) -> bool {
        self.find(needle).is_some()
    }
    pub fn starts_with(&self, needle: impl Into<Self>) -> bool {
        self.0.starts_with(&needle.into().0)
    }
    pub fn ends_with(&self, needle: impl Into<Self>) -> bool {
        self.0.ends_with(&needle.into().0)
    }
    /// Split on a nonempty sequence of code units, retaining empty fields.
    ///
    /// # Panics
    /// Panics for an empty separator. Use `as_units()` to iterate individual units.
    pub fn split(&self, needle: impl Into<Self>) -> std::vec::IntoIter<Self> {
        let needle = needle.into();
        assert!(
            !needle.is_empty(),
            "use unit iteration for empty separators"
        );
        let mut result = Vec::new();
        let mut at = 0;
        while let Some(offset) = self.0[at..]
            .windows(needle.len())
            .position(|w| w == needle.0)
        {
            let end = at + offset;
            result.push(self.slice(at..end));
            at = end + needle.len();
        }
        result.push(self.slice(at..));
        result.into_iter()
    }
    pub fn join(parts: impl IntoIterator<Item = Self>, separator: impl Into<Self>) -> Self {
        let separator = separator.into();
        let mut output = Self::new();
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                output.push(&separator);
            }
            output.push(part);
        }
        output
    }
    /// Replace all occurrences of a nonempty sequence of code units.
    ///
    /// # Panics
    /// Panics for an empty search sequence, as does [`Self::split`].
    pub fn replace(&self, needle: impl Into<Self>, replacement: impl Into<Self>) -> Self {
        Self::join(self.split(needle), replacement)
    }
}

macro_rules! raw_text {
    ($($part:expr),* $(,)?) => {{
        let mut text = $crate::tui::utf16::Utf16Text::new();
        $(text.push($part);)*
        text
    }};
}
pub(crate) use raw_text;

#[cfg(test)]
mod tests {
    use super::Utf16Text;

    #[test]
    fn concatenation_can_rejoin_a_surrogate_pair_without_a_sentinel() {
        let high = Utf16Text::from_units(vec![0xd83d]);
        let low = Utf16Text::from_units(vec![0xde00]);
        assert!(high.to_string_checked().is_err());
        let joined = raw_text!(&high, &low);
        assert_eq!(joined.to_string_checked().unwrap(), "😀");
        assert_eq!(joined.len(), 2);
        assert_eq!(joined.slice(1..), low);
    }

    #[test]
    fn operations_keep_lone_units_and_private_use_literals_separate() {
        let mut text = Utf16Text::from_units(vec![0xd800, 10, 0xdc00, 0xe000]);
        text.push('\u{f0000}');
        let replaced = text.replace('\n', "|");
        assert_eq!(
            replaced.as_units(),
            &[0xd800, 124, 0xdc00, 0xe000, 0xdb80, 0xdc00]
        );
        assert_eq!(Utf16Text::join(replaced.split('|'), "\n"), text);
    }
}
