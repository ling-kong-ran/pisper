//! UTF-16 width path for possibly ill-formed LaTeX/Markdown strings.
//! A replacement-character *segmentation view* is indexed back to raw points;
//! lone surrogates remain non-printing (unlike actual U+FFFD). It is never used
//! to store or encode the result. Both code points have GCB=Other; a regression
//! test checks that precondition against the installed Unicode property data.

use super::{
    east_asian_width, grapheme_width, is_mark_char, is_non_printing_char,
    is_terminal_spacing_mark_char, visible_width,
};
use unicode_segmentation::UnicodeSegmentation;

// Shared with raw ANSI wrapping; delimiters are recognized in actual units.
fn ansi_length(units: &[u16], start: usize) -> Option<usize> {
    if units.get(start) != Some(&0x1b) {
        return None;
    }
    match units.get(start + 1) {
        Some(0x5b) => (start + 2..units.len())
            .find(|&i| matches!(units[i], 0x6d | 0x47 | 0x4b | 0x48 | 0x4a))
            .map(|end| end + 1 - start),
        Some(0x5d | 0x5f) => {
            for i in start + 2..units.len() {
                if units[i] == 7 {
                    return Some(i + 1 - start);
                }
                if units[i] == 0x1b && units.get(i + 1) == Some(&0x5c) {
                    return Some(i + 2 - start);
                }
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn visible_width_utf16(units: &[u16]) -> usize {
    if let Ok(text) = String::from_utf16(units) {
        return visible_width(&text);
    }
    // Upstream normalizes the JS string before code-point iteration. Removing
    // ANSI can join a high and low surrogate into a real scalar; a decoded
    // replacement-character view cannot recover that pairing afterwards.
    let mut expanded = Vec::with_capacity(units.len());
    for &unit in units {
        if unit == 9 {
            expanded.extend([0x20; 3]);
        } else {
            expanded.push(unit);
        }
    }
    let mut stripped = Vec::with_capacity(expanded.len());
    let mut offset = 0;
    while offset < expanded.len() {
        if let Some(length) = ansi_length(&expanded, offset) {
            offset += length;
        } else {
            stripped.push(expanded[offset]);
            offset += 1;
        }
    }

    // Segment the normalized units directly. Calling visible_width here would
    // wrongly strip a second time if the first pass formed a new ESC sequence.
    let mut clean = String::new();
    let mut clean_points = Vec::new();
    for point in char::decode_utf16(stripped.iter().copied()) {
        clean_points.push((clean.len(), point.as_ref().ok().copied()));
        clean.push(point.unwrap_or(char::REPLACEMENT_CHARACTER));
    }

    let mut at = 0;
    clean
        .grapheme_indices(true)
        .map(|(start, grapheme)| {
            let end = start + grapheme.len();
            let begin = at;
            while at < clean_points.len() && clean_points[at].0 < end {
                at += 1;
            }
            let points = &clean_points[begin..at];
            if points.iter().all(|(_, point)| point.is_some()) {
                return grapheme_width(grapheme);
            }
            // A cluster containing a surrogate is neither all spacing marks nor an
            // RGI emoji. Leading surrogate/mark/format/control points are ignored.
            let Some(base) = points
                .iter()
                .position(|(_, point)| point.is_some_and(|c| !is_non_printing_char(c)))
            else {
                return 0;
            };
            let first = points[base].1.expect("visible scalar base");
            if (0x1f1e6..=0x1f1ff).contains(&(first as u32)) {
                return 2;
            }
            let mut width = east_asian_width(first);
            let mut follows_mark = false;
            for (_, point) in &points[base + 1..] {
                let Some(c) = *point else {
                    continue;
                };
                if is_terminal_spacing_mark_char(c) {
                    width += 1;
                    follows_mark = false;
                } else if is_mark_char(c) {
                    follows_mark = true;
                } else if !is_non_printing_char(c) {
                    if follows_mark || (0xff00..=0xffef).contains(&(c as u32)) {
                        width += east_asian_width(c);
                    } else if matches!(c, '\u{e33}' | '\u{eb3}') {
                        width += 1;
                    }
                    follows_mark = false;
                }
            }
            width
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::visible_width_utf16;
    use icu_properties::{
        props::{ExtendedPictographic, GraphemeClusterBreak, IndicConjunctBreak},
        CodePointMapData, CodePointSetData,
    };

    #[test]
    fn segmentation_view_preserves_surrogate_break_properties() {
        let map = CodePointMapData::<GraphemeClusterBreak>::new();
        let indic = CodePointMapData::<IndicConjunctBreak>::new();
        let pictographic = CodePointSetData::new::<ExtendedPictographic>();
        for unit in 0xd800..=0xdfff {
            assert_eq!(map.get32(unit), map.get('\u{fffd}'));
            assert_eq!(indic.get32(unit), indic.get('\u{fffd}'));
            assert_eq!(
                pictographic.contains32(unit),
                pictographic.contains('\u{fffd}')
            );
        }
    }

    // Actual upstream visibleWidth: reference/markdown/probe-ansi-rejoin.mjs.
    #[test]
    fn ansi_stripping_rejoins_utf16_pairs_before_width_segmentation() {
        let separators = [
            "",
            "\x1b[0m",
            "\x1b[1G",
            "\x1b[2K",
            "\x1b[3H",
            "\x1b[4J",
            "\x1b]8;;x\x07",
            "\x1b]8;;x\x1b\\",
            "\x1b_marker\x07",
            "\x1b_marker\x1b\\",
            "\x1b[\tm",
        ];
        for (high, low, width) in [(0xd83d, 0xde00, 2), (0xd835, 0xdc9c, 1)] {
            for separator in separators {
                let mut units = vec![high];
                units.extend(separator.encode_utf16());
                units.push(low);
                assert_eq!(visible_width_utf16(&units), width, "{units:x?}");
            }
            assert_eq!(
                visible_width_utf16(&[high, 0x1b, 0x5d, 0xd800, 7, low]),
                width
            );
            assert_eq!(visible_width_utf16(&[high, 9, low]), 3);
            assert_eq!(visible_width_utf16(&[high, 0x20, low]), 1);
        }
    }

    #[test]
    fn raw_width_normalization_strips_ansi_only_once() {
        // Removing OSC joins an earlier ESC and later [0m. The newly formed
        // sequence must stay visible; the original scan does not revisit it.
        assert_eq!(
            visible_width_utf16(&[0xd83d, 0x1b, 0x1b, 0x5d, 0x78, 7, 0x5b, 0x30, 0x6d, 0xde00]),
            3
        );
        assert_eq!(
            visible_width_utf16(&[0xd800, 0x1b, 0x1b, 0x5d, 0x78, 7, 0x5b, 0x30, 0x6d, 0x41]),
            4
        );
    }

    #[test]
    fn raw_surrogate_is_not_a_printed_replacement_character() {
        assert_eq!(visible_width_utf16(&[0xd800]), 0);
        assert_eq!(visible_width_utf16(&[0xfffd]), 1);
        assert_eq!(visible_width_utf16(&[0xd800, 0x93e]), 0);
        assert_eq!(visible_width_utf16(&[0x93e]), 1);
        assert_eq!(visible_width_utf16(&[0xd800, 9, 0xfffd, 0xdc00]), 4);
    }
}

mod wrap;
pub(crate) use wrap::wrap_text_with_ansi;

/// Strip once in original units, allowing ANSI removal to rejoin surrogate pairs.
/// Shares exactly the existing raw ANSI recognizer with width/wrapping.
pub(crate) fn strip_terminal_sequences_utf16(units: &[u16]) -> Vec<u16> {
    let mut result = Vec::with_capacity(units.len());
    let mut offset = 0;
    while offset < units.len() {
        if let Some(length) = ansi_length(units, offset) {
            offset += length;
        } else {
            result.push(units[offset]);
            offset += 1;
        }
    }
    result
}
