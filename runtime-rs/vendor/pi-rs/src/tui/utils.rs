//! Port of upstream `packages/tui/src/utils.ts`: terminal text measurement and
//! ANSI-preserving text operations.
//!
//! Upstream file: `packages/tui/src/utils.ts` (behavior authority), using the
//! `get-east-asian-width` package and JS `Intl.Segmenter` / v-flag regexes.
//! Generated tables: `east_asian_width` / `rgi_emoji` (see
//! `docs/migration/reference/generate-tui-width-tables.mjs`).

mod east_asian_width;
mod rgi_emoji;
mod rgi_emoji_vs16;
mod spacing_mark;
mod utf16;

pub(crate) use utf16::strip_terminal_sequences_utf16;
pub(crate) use utf16::{visible_width_utf16, wrap_text_with_ansi as wrap_text_with_ansi_utf16};

use std::sync::OnceLock;

use icu_properties::props::{DefaultIgnorableCodePoint, GeneralCategory, Script};
use icu_properties::script::{ScriptWithExtensions, ScriptWithExtensionsBorrowed};
use icu_properties::{
    CodePointMapData, CodePointMapDataBorrowed, CodePointSetData, CodePointSetDataBorrowed,
};
use unicode_segmentation::UnicodeSegmentation;

use self::east_asian_width::east_asian_width;
use self::rgi_emoji_vs16::is_rgi_emoji;
use self::spacing_mark::is_spacing_mark;

/// Test-only probe for the generated RGI table.
/// JS `\s` unicode categories (Zs/Zl/Zp beyond the ASCII set).
pub fn is_js_space_unicode(c: char) -> bool {
    use GeneralCategory::*;
    matches!(
        general_category_map().get(c),
        SpaceSeparator | LineSeparator | ParagraphSeparator
    )
}

/// JS `\p{L}|\p{N}` (marked emStrong `_`-between-alphanumerics guard).
pub fn is_letter_or_number_unicode(c: char) -> bool {
    use GeneralCategory::*;
    matches!(
        general_category_map().get(c),
        LowercaseLetter
            | UppercaseLetter
            | TitlecaseLetter
            | ModifierLetter
            | OtherLetter
            | DecimalNumber
            | LetterNumber
            | OtherNumber
    )
}

/// JS `\p{P}|\p{S}` (used by the marked lexer punctuation classes).
pub fn is_punct_or_symbol_unicode(c: char) -> bool {
    use GeneralCategory::*;
    matches!(
        general_category_map().get(c),
        DashPunctuation
            | OpenPunctuation
            | ClosePunctuation
            | InitialPunctuation
            | FinalPunctuation
            | ConnectorPunctuation
            | OtherPunctuation
            | MathSymbol
            | CurrencySymbol
            | ModifierSymbol
            | OtherSymbol
    )
}

#[cfg(test)]
pub(crate) fn is_rgi_emoji_probe(segment: &str) -> bool {
    is_rgi_emoji(segment)
}

fn general_category_map() -> CodePointMapDataBorrowed<'static, GeneralCategory> {
    static MAP: OnceLock<CodePointMapDataBorrowed<'static, GeneralCategory>> = OnceLock::new();
    *MAP.get_or_init(CodePointMapData::<GeneralCategory>::new)
}

fn default_ignorable() -> CodePointSetDataBorrowed<'static> {
    static SET: OnceLock<CodePointSetDataBorrowed<'static>> = OnceLock::new();
    *SET.get_or_init(CodePointSetData::new::<DefaultIgnorableCodePoint>)
}

fn script_extensions() -> ScriptWithExtensionsBorrowed<'static> {
    static SWE: OnceLock<ScriptWithExtensionsBorrowed<'static>> = OnceLock::new();
    *SWE.get_or_init(ScriptWithExtensions::new)
}

/// `\p{Mark}`: General_Category in {Mn, Mc, Me}.
fn is_mark_char(c: char) -> bool {
    matches!(
        general_category_map().get(c),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

/// `\p{Control}` (Cc).
fn is_control_char(c: char) -> bool {
    general_category_map().get(c) == GeneralCategory::Control
}

/// `\p{Format}` (Cf).
fn is_format_char(c: char) -> bool {
    general_category_map().get(c) == GeneralCategory::Format
}

/// `\p{Surrogate}` (Cs) — unreachable for `char`, kept for parity with the
/// upstream union sets.
fn is_surrogate_char(c: char) -> bool {
    general_category_map().get(c) == GeneralCategory::Surrogate
}

/// `\p{Default_Ignorable_Code_Point}`.
fn is_default_ignorable(c: char) -> bool {
    default_ignorable().contains(c)
}

/// `terminalSpacingMarkRegex` (utils.ts:46): the generated `\p{Spacing_Mark}`
/// set minus the three regex exceptions, plus the explicit legacy-wcwidth
/// additions.
fn is_terminal_spacing_mark_char(c: char) -> bool {
    let cp = c as u32;
    if matches!(cp, 0x1734 | 0x302E | 0x302F) {
        return false;
    }
    if is_spacing_mark(c) {
        return true;
    }
    matches!(
        cp,
        0x065F | 0x0F7F
            | 0x102B
            | 0x102C
            | 0x1031
            | 0x1033..=0x1035
            | 0x1038
            | 0x103A..=0x103E
    )
}

/// `couldBeEmoji` (utils.ts:27): fast pre-filter before the RGI emoji test.
fn could_be_emoji(segment: &str) -> bool {
    let Some(first) = segment.chars().next() else {
        return false;
    };
    let c = first as u32;
    (0x1f000..=0x1fbff).contains(&c)
        || (0x2300..=0x23ff).contains(&c)
        || (0x2600..=0x27bf).contains(&c)
        || (0x2b50..=0x2b55).contains(&c)
        || segment.contains('\u{fe0f}')
        || utf16_len(segment) > 2
}

/// JS `[...str].length`: UTF-16 code unit count.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// JS `isPrintableAscii`: every UTF-16 unit in 0x20..=0x7e.
fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// `zeroWidthRegex`: the whole segment is default-ignorable/control/mark/surrogate.
fn is_zero_width_segment(segment: &str) -> bool {
    segment.chars().all(|c| {
        is_default_ignorable(c) || is_control_char(c) || is_mark_char(c) || is_surrogate_char(c)
    })
}

/// `nonPrintingCharRegex` for a single char (adds `\p{Format}` to the
/// zero-width set).
fn is_non_printing_char(c: char) -> bool {
    is_default_ignorable(c)
        || is_control_char(c)
        || is_format_char(c)
        || is_mark_char(c)
        || is_surrogate_char(c)
}

/// `graphemeWidth` (utils.ts:180).
fn grapheme_width(segment: &str) -> usize {
    // Fast path: JS `segment.length === 1` — exactly one UTF-16 unit, i.e. a
    // single BMP code point. Printable ASCII occupies one cell, a tab three.
    let mut chars = segment.chars();
    if let (Some(single), None) = (chars.next(), chars.next()) {
        if single.len_utf16() == 1 {
            let code = u32::from(single);
            if (0x20..=0x7e).contains(&code) {
                return 1;
            }
            if code == 0x09 {
                return 3;
            }
        }
    }

    // Some marks occupy cells even without a base character. Upstream returns
    // `[...segment].length`, which counts CODE POINTS (a supplementary-plane
    // spacing mark costs 1 even though it is two UTF-16 units).
    if segment.chars().all(is_terminal_spacing_mark_char) {
        return segment.chars().count();
    }

    // Zero-width clusters.
    if is_zero_width_segment(segment) {
        return 0;
    }

    // Emoji check with pre-filter.
    if could_be_emoji(segment) && is_rgi_emoji(segment) {
        return 2;
    }

    // Base visible codepoint after stripping leading non-printing chars.
    let base = segment.trim_start_matches(|c| {
        is_default_ignorable(c)
            || is_control_char(c)
            || is_format_char(c)
            || is_mark_char(c)
            || is_surrogate_char(c)
    });
    let Some(base_first) = base.chars().next() else {
        return 0;
    };
    let base_cp = base_first as u32;

    // Regional indicators stay conservative (2) during streaming.
    if (0x1f1e6..=0x1f1ff).contains(&base_cp) {
        return 2;
    }

    let mut width = east_asian_width(base_first);

    // Count trailing visible code points terminals may allocate cells for.
    let mut follows_mark = false;
    for c in base.chars().skip(1) {
        if is_terminal_spacing_mark_char(c) {
            width += 1;
            follows_mark = false;
        } else if is_mark_char(c) {
            follows_mark = true;
        } else if !is_non_printing_char(c) {
            let c_num = c as u32;
            if follows_mark || (0xff00..=0xffef).contains(&c_num) {
                width += east_asian_width(c);
            } else if c_num == 0x0e33 || c_num == 0x0eb3 {
                width += 1;
            }
            follows_mark = false;
        }
    }

    width
}

/// `cjkBreakRegex` (unanchored): any char whose Script_Extensions contain
/// Han/Hiragana/Katakana/Hangul/Bopomofo.
pub(crate) fn is_cjk_break_char(c: char) -> bool {
    let swe = script_extensions();
    swe.has_script(c, Script::Han)
        || swe.has_script(c, Script::Hiragana)
        || swe.has_script(c, Script::Katakana)
        || swe.has_script(c, Script::Hangul)
        || swe.has_script(c, Script::Bopomofo)
}

pub(crate) fn is_cjk_break_segment(segment: &str) -> bool {
    segment.chars().any(is_cjk_break_char)
}

/// JS `\p{Punctuation}` (General_Category=P; distinct from the ASCII
/// `isPunctuationChar` helper near the word-navigation code).
fn is_unicode_punctuation_char(c: char) -> bool {
    use GeneralCategory::*;
    matches!(
        general_category_map().get(c),
        DashPunctuation
            | OpenPunctuation
            | ClosePunctuation
            | InitialPunctuation
            | FinalPunctuation
            | ConnectorPunctuation
            | OtherPunctuation
    )
}

/// `cjkPunctuationRegex` against a single code point: a CJK-script char that
/// is punctuation (`(?=\p{Punctuation})` + the CJK class), or one of the
/// explicit CJK punctuation literals.
pub(crate) fn is_cjk_punctuation_char(c: char) -> bool {
    (is_unicode_punctuation_char(c) && is_cjk_break_char(c))
        || matches!(
            c,
            '，' | '．'
                | '：'
                | '；'
                | '！'
                | '？'
                | '（'
                | '）'
                | '［'
                | '］'
                | '｛'
                | '｝'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '…'
                | '—'
        )
}

/// `autocompleteSeparatorRegex` (`(?:\s|cjkPunctuation)`) against a single
/// code point: JS `\s` or CJK punctuation.
pub(crate) fn is_autocomplete_separator_char(c: char) -> bool {
    crate::tui::markdown_lexer::is_js_space(c) || is_cjk_punctuation_char(c)
}

/// `autocompleteSeparatorRegex.test(value)` (unanchored search): some code
/// point matches.
pub(crate) fn has_autocomplete_separator(value: &str) -> bool {
    value.chars().any(is_autocomplete_separator_char)
}

/// `tokenStartRegex` = `autocompleteBoundaryRegex` anchored at the end
/// (`(?:^|(?:\s|cjkPunctuation))$`): the empty string, or the last code point
/// is a separator.
pub(crate) fn is_token_start_boundary(text: &str) -> bool {
    match text.chars().next_back() {
        None => true,
        Some(last) => is_autocomplete_separator_char(last),
    }
}

/// `visibleWidth` (utils.ts:251). The upstream width cache is an optimization,
/// not observable behavior, and is not ported.
pub fn visible_width(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }

    // Fast path: printable ASCII, tabs, and ANSI escape sequences. Styled lines
    // take this path, so re-rendering after a theme change does not run
    // grapheme segmentation on every line.
    if let Some(ascii_width) = ascii_visible_width(s) {
        return ascii_width;
    }

    // Normalize: tabs to 3 spaces, strip ANSI escape codes.
    let mut clean = s.to_string();
    if s.contains('\t') {
        clean = clean.replace('\t', "   ");
    }
    if clean.contains('\x1b') {
        clean = strip_terminal_sequences(&clean);
    }

    clean.graphemes(true).map(grapheme_width).sum()
}

/// `asciiVisibleWidth` (utils.ts:429): width of a string made of printable
/// ASCII, tabs, and ANSI escape sequences, or `None` (JS -1) if it contains
/// anything else. Matches `visibleWidth` for those strings.
fn ascii_visible_width(s: &str) -> Option<usize> {
    let mut width = 0;
    let mut i = 0;
    while i < s.len() {
        let code = s.as_bytes()[i];
        if (0x20..=0x7e).contains(&code) {
            width += 1;
            i += 1;
        } else if code == 0x09 {
            width += 3;
            i += 1;
        } else if code == 0x1b {
            let length = ansi_code_length(s, i);
            if length == 0 {
                return None;
            }
            i += length;
        } else {
            return None;
        }
    }
    Some(width)
}

/// `stripTerminalSequences` (utils.ts:298).
pub fn strip_terminal_sequences(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some(code) = extract_ansi_code(s, i) {
            i += code.len();
            continue;
        }
        let ch_len = s[i..].chars().next().map_or(1, char::len_utf8);
        result.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    result
}

/// `extractAnsiCode` (utils.ts:419). `pos` must be a char boundary; returns
/// the matched escape-sequence slice.
pub fn extract_ansi_code(s: &str, pos: usize) -> Option<&str> {
    let length = ansi_code_length(s, pos);
    if length > 0 {
        Some(&s[pos..pos + length])
    } else {
        None
    }
}

/// `ansiCodeLength` (utils.ts:436): length of the ANSI/OSC/APC escape sequence
/// starting at `pos` (a char boundary), or 0 if there is none. Lengths are in
/// UTF-16 units upstream and bytes here; every matched sequence is pure ASCII,
/// so the counts coincide.
fn ansi_code_length(s: &str, pos: usize) -> usize {
    let bytes = s.as_bytes();
    if pos >= s.len() || bytes[pos] != 0x1b {
        return 0;
    }

    let next = bytes.get(pos + 1).copied();

    // CSI sequence: ESC [ ... m/G/K/H/J
    if next == Some(b'[') {
        let terminator = bytes[pos + 2..]
            .iter()
            .position(|byte| matches!(byte, 0x6d | 0x47 | 0x4b | 0x48 | 0x4a));
        return match terminator {
            Some(offset) => pos + 2 + offset + 1 - pos,
            None => 0,
        };
    }

    // OSC / APC sequence: BEL- or ST-terminated. Used for hyperlinks (OSC 8),
    // window titles, cursor markers and application-specific commands.
    if next == Some(b']') || next == Some(b'_') {
        for j in pos + 2..s.len() {
            if bytes[j] == 0x07 {
                return j + 1 - pos;
            }
            if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                return j + 2 - pos;
            }
        }
        return 0;
    }

    0
}

/// End of the next run of text that starts at `start` and contains no escape
/// sequence (upstream's inner `textEnd` scans, one JS unit / one char at a
/// time — equivalent because both stop only at char boundaries).
fn next_text_run_end(s: &str, start: usize) -> usize {
    let mut end = start;
    while end < s.len() && extract_ansi_code(s, end).is_none() {
        end += s[end..].chars().next().map_or(1, char::len_utf8);
    }
    end
}

/// `getGraphemeCellRange` (utils.ts:320): the terminal-cell range occupied by
/// the grapheme at a visible column.
pub fn get_grapheme_cell_range(line: &str, column: usize) -> Option<(usize, usize)> {
    let mut current_col = 0;
    let mut i = 0;
    while i < line.len() {
        if let Some(code) = extract_ansi_code(line, i) {
            i += code.len();
            continue;
        }
        let text_end = next_text_run_end(line, i);
        for segment in line[i..text_end].graphemes(true) {
            let width = grapheme_width(segment);
            if width > 0 && column >= current_col && column < current_col + width {
                return Some((current_col, current_col + width));
            }
            current_col += width;
        }
        i = text_end;
    }
    None
}

/// `getOsc8LinkAtColumn` (utils.ts:344).
pub fn get_osc8_link_at_column(line: &str, column: usize) -> Option<&str> {
    let mut active_url: Option<&str> = None;
    let mut current_col = 0;
    let mut i = 0;
    while i < line.len() {
        if let Some(code) = extract_ansi_code(line, i) {
            if let Some(hyperlink) = osc8_url(code) {
                active_url = hyperlink;
            }
            i += code.len();
            continue;
        }
        let text_end = next_text_run_end(line, i);
        for segment in line[i..text_end].graphemes(true) {
            let width = if segment == "\t" {
                3
            } else {
                grapheme_width(segment)
            };
            if column >= current_col && column < current_col + width {
                return active_url;
            }
            current_col += width;
        }
        i = text_end;
    }
    None
}

/// Upstream anchored regex `/^\x1b\]8;[^;]*;([^\x07\x1b]*)(?:\x07|\x1b\\)$/`:
/// returns the URL; `None` (JS `undefined`) for an empty URL; no match →
/// `Some(None)`-free double option collapsed here to `Option<Option<&str>>`
/// where the outer `None` means "not an OSC 8 code".
fn osc8_url(ansi_code: &str) -> Option<Option<&str>> {
    let body = ansi_code.strip_prefix("\x1b]8;")?;
    let url = if let Some(rest) = body.strip_suffix('\x07') {
        rest
    } else {
        body.strip_suffix("\x1b\\")?
    };
    let (params, url) = url.split_once(';')?;
    if url.is_empty() || url.contains(['\x07', '\x1b']) {
        return Some(None);
    }
    if !params.bytes().all(|b| b != b';') {
        return None;
    }
    Some(Some(url))
}

const RESET: &str = "\x1b[0m";

/// `truncateFragmentToWidth` (utils.ts:67).
fn truncate_fragment_to_width(text: &str, max_width: usize) -> (String, usize) {
    if max_width == 0 || text.is_empty() {
        return (String::new(), 0);
    }

    if is_printable_ascii(text) {
        let end = text.len().min(max_width);
        return (text[..end].to_string(), end);
    }

    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');
    if !has_ansi && !has_tabs {
        let mut result = String::new();
        let mut width = 0;
        for segment in text.graphemes(true) {
            let w = grapheme_width(segment);
            if width + w > max_width {
                break;
            }
            result.push_str(segment);
            width += w;
        }
        return (result, width);
    }

    let mut result = String::new();
    let mut width = 0;
    let mut i = 0;
    let mut pending_ansi = String::new();

    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            pending_ansi.push_str(ansi);
            i += ansi.len();
            continue;
        }

        if text[i..].starts_with('\t') {
            if width + 3 > max_width {
                break;
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push('\t');
            width += 3;
            i += 1;
            continue;
        }

        let mut end = i;
        while end < text.len() && !text[end..].starts_with('\t') {
            if extract_ansi_code(text, end).is_some() {
                break;
            }
            end += text[end..].chars().next().map_or(1, char::len_utf8);
        }

        for segment in text[i..end].graphemes(true) {
            let w = grapheme_width(segment);
            if width + w > max_width {
                return (result, width);
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push_str(segment);
            width += w;
        }
        i = end;
    }

    (result, width)
}

/// `finalizeTruncatedResult` (utils.ts:147).
fn finalize_truncated_result(
    prefix: &str,
    prefix_width: usize,
    ellipsis: &str,
    ellipsis_width: usize,
    max_width: usize,
    pad: bool,
) -> String {
    let hyperlink_close = get_active_osc8_close(prefix);
    let visible = prefix_width + ellipsis_width;
    let mut result = String::new();
    result.push_str(prefix);
    result.push_str(&hyperlink_close);
    result.push_str(RESET);
    if !ellipsis.is_empty() {
        result.push_str(ellipsis);
        result.push_str(RESET);
    }

    if pad {
        let padding = max_width.saturating_sub(visible);
        result.extend(std::iter::repeat_n(' ', padding));
    }
    result
}

/// JS `String.prototype.trim` / `trimEnd` whitespace set: no U+0085, includes
/// U+FEFF.
fn is_js_trim_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}'
            | '\u{a}'
            | '\u{b}'
            | '\u{c}'
            | '\u{d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_trim_whitespace)
}

pub(crate) fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_trim_whitespace)
}

/// `truncateToWidth` (utils.ts:1064).
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let text_width = visible_width(text);
        if text_width <= max_width {
            return if pad {
                format!("{}{}", text, " ".repeat(max_width - text_width))
            } else {
                text.to_string()
            };
        }

        let (clipped_ellipsis, clipped_width) = truncate_fragment_to_width(ellipsis, max_width);
        if clipped_width == 0 {
            return if pad {
                " ".repeat(max_width)
            } else {
                String::new()
            };
        }
        return finalize_truncated_result("", 0, &clipped_ellipsis, clipped_width, max_width, pad);
    }

    if is_printable_ascii(text) {
        if text.len() <= max_width {
            return if pad {
                format!("{}{}", text, " ".repeat(max_width - text.len()))
            } else {
                text.to_string()
            };
        }
        let target_width = max_width - ellipsis_width;
        return finalize_truncated_result(
            &text[..target_width],
            target_width,
            ellipsis,
            ellipsis_width,
            max_width,
            pad,
        );
    }

    let target_width = max_width - ellipsis_width;
    let mut result = String::new();
    let mut pending_ansi = String::new();
    let mut visible_so_far = 0usize;
    let mut kept_width = 0usize;
    let mut keep_contiguous_prefix = true;
    let mut overflowed = false;
    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');

    if !has_ansi && !has_tabs {
        for segment in text.graphemes(true) {
            let width = grapheme_width(segment);
            if keep_contiguous_prefix && kept_width + width <= target_width {
                result.push_str(segment);
                kept_width += width;
            } else {
                keep_contiguous_prefix = false;
            }
            visible_so_far += width;
            if visible_so_far > max_width {
                overflowed = true;
                break;
            }
        }
        if !overflowed {
            return if pad {
                format!(
                    "{}{}",
                    text,
                    " ".repeat(max_width.saturating_sub(visible_so_far))
                )
            } else {
                text.to_string()
            };
        }
        return finalize_truncated_result(
            &result,
            kept_width,
            ellipsis,
            ellipsis_width,
            max_width,
            pad,
        );
    }

    let mut i = 0;
    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            pending_ansi.push_str(ansi);
            i += ansi.len();
            continue;
        }

        if text[i..].starts_with('\t') {
            if keep_contiguous_prefix && kept_width + 3 <= target_width {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push('\t');
                kept_width += 3;
            } else {
                keep_contiguous_prefix = false;
                pending_ansi.clear();
            }
            visible_so_far += 3;
            if visible_so_far > max_width {
                overflowed = true;
                break;
            }
            i += 1;
            continue;
        }

        let mut end = i;
        while end < text.len() && !text[end..].starts_with('\t') {
            if extract_ansi_code(text, end).is_some() {
                break;
            }
            end += text[end..].chars().next().map_or(1, char::len_utf8);
        }

        for segment in text[i..end].graphemes(true) {
            let width = grapheme_width(segment);
            if keep_contiguous_prefix && kept_width + width <= target_width {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(segment);
                kept_width += width;
            } else {
                keep_contiguous_prefix = false;
                pending_ansi.clear();
            }

            visible_so_far += width;
            if visible_so_far > max_width {
                overflowed = true;
                break;
            }
        }
        if overflowed {
            break;
        }
        i = end;
    }

    if !overflowed {
        return if pad {
            format!(
                "{}{}",
                text,
                " ".repeat(max_width.saturating_sub(visible_so_far))
            )
        } else {
            text.to_string()
        };
    }

    finalize_truncated_result(
        &result,
        kept_width,
        ellipsis,
        ellipsis_width,
        max_width,
        pad,
    )
}

/// `sliceByColumn` (utils.ts:1206).
pub fn slice_by_column(line: &str, start_col: usize, length: usize, strict: bool) -> String {
    slice_with_width(line, start_col, length, strict).0
}

/// `sliceWithWidth`: the sliced text plus its actual visible width.
pub fn slice_with_width(
    line: &str,
    start_col: usize,
    length: usize,
    strict: bool,
) -> (String, usize) {
    if length == 0 {
        return (String::new(), 0);
    }
    let end_col = start_col + length;
    let mut result = String::new();
    let mut result_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0usize;
    let mut pending_ansi = String::new();

    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                // Keep original order (v1.0.0): codes from before the range
                // must precede codes at the boundary.
                result.push_str(&pending_ansi);
                result.push_str(ansi);
                pending_ansi.clear();
            } else if current_col < start_col {
                pending_ansi.push_str(ansi);
            }
            i += ansi.len();
            continue;
        }

        let text_end = next_text_run_end(line, i);

        for segment in line[i..text_end].graphemes(true) {
            let w = grapheme_width(segment);
            let in_range = current_col >= start_col && current_col < end_col;
            let fits = !strict || current_col + w <= end_col;
            if in_range && fits {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(segment);
                result_width += w;
            }
            current_col += w;
            if current_col >= end_col {
                break;
            }
        }
        i = text_end;
        if current_col >= end_col {
            break;
        }
    }
    (result, result_width)
}

/// `getActiveOsc8Close` (utils.ts:482).
fn get_active_osc8_close(prefix: &str) -> String {
    if !prefix.contains("\x1b]8;") {
        return String::new();
    }

    let mut active_hyperlink: Option<ActiveHyperlink> = None;
    let mut i = 0;
    while i < prefix.len() {
        if let Some(ansi) = extract_ansi_code(prefix, i) {
            if let Some(hyperlink) = parse_osc8_hyperlink(ansi) {
                active_hyperlink = hyperlink;
            }
            i += ansi.len();
        } else {
            i += prefix[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    active_hyperlink.map_or_else(String::new, |h| h.close_code())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Osc8Terminator {
    Bel,
    St,
}

impl Osc8Terminator {
    fn as_str(self) -> &'static str {
        match self {
            Osc8Terminator::Bel => "\x07",
            Osc8Terminator::St => "\x1b\\",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveHyperlink {
    params: String,
    url: String,
    terminator: Osc8Terminator,
}

impl ActiveHyperlink {
    fn open_code(&self) -> String {
        format!(
            "\x1b]8;{};{}{}",
            self.params,
            self.url,
            self.terminator.as_str()
        )
    }

    fn close_code(&self) -> String {
        format!("\x1b]8;;{}", self.terminator.as_str())
    }
}

/// `parseOsc8Hyperlink` (utils.ts:454) — manual parse, no ESC/BEL check on the
/// URL (unlike `osc8_url`). `None` when the code is not an OSC 8 sequence;
/// `Some(None)` for a hyperlink with an empty URL (upstream `null`, which
/// clears the active link).
fn parse_osc8_hyperlink(ansi_code: &str) -> Option<Option<ActiveHyperlink>> {
    let body = ansi_code.strip_prefix("\x1b]8;")?;
    let (inner, terminator) = if let Some(rest) = body.strip_suffix('\x07') {
        (rest, Osc8Terminator::Bel)
    } else {
        let rest = body.strip_suffix("\x1b\\")?;
        (rest, Osc8Terminator::St)
    };
    let (params, url) = inner.split_once(';')?;
    if url.is_empty() {
        return Some(None);
    }
    Some(Some(ActiveHyperlink {
        params: params.to_string(),
        url: url.to_string(),
        terminator,
    }))
}

/// `AnsiCodeTracker` (utils.ts:507).
#[derive(Clone, Default, Debug)]
pub struct AnsiCodeTracker {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    fg_color: Option<String>,
    bg_color: Option<String>,
    active_hyperlink: Option<ActiveHyperlink>,
}

/// Upstream `ansiCode.match(/\x1b\[([\d;]*)m/)` — an unanchored search for a
/// CSI SGR body of only digits and semicolons.
fn find_sgr_params(ansi_code: &str) -> Option<&str> {
    let mut search_from = 0;
    while let Some(rel) = ansi_code[search_from..].find("\x1b[") {
        let start = search_from + rel + 2;
        let bytes = ansi_code.as_bytes();
        let mut j = start;
        while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'm' {
            return Some(&ansi_code[start..j]);
        }
        search_from = start;
    }
    None
}

impl AnsiCodeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process(&mut self, ansi_code: &str) {
        // OSC 8 hyperlinks preserve their original terminator (BEL links stay
        // BEL on re-open; see utils.ts:522).
        if let Some(hyperlink) = parse_osc8_hyperlink(ansi_code) {
            self.active_hyperlink = hyperlink;
            return;
        }

        if !ansi_code.ends_with('m') {
            return;
        }

        let Some(params) = find_sgr_params(ansi_code) else {
            return;
        };

        if params.is_empty() || params == "0" {
            // Full reset.
            self.reset();
            return;
        }

        let parts: Vec<&str> = params.split(';').collect();
        let mut i = 0;
        while i < parts.len() {
            // Upstream `Number.parseInt` never yields NaN here: find_sgr_params
            // validated digits/semicolons only, so every part is numeric.
            let Ok(code) = parts[i].parse::<u64>() else {
                i += 1;
                continue;
            };

            // 256-color and RGB codes consume multiple parameters.
            if code == 38 || code == 48 {
                if i + 2 < parts.len() && parts[i + 1] == "5" {
                    let color_code = format!("{};{};{}", parts[i], parts[i + 1], parts[i + 2]);
                    if code == 38 {
                        self.fg_color = Some(color_code);
                    } else {
                        self.bg_color = Some(color_code);
                    }
                    i += 3;
                    continue;
                } else if i + 4 < parts.len() && parts[i + 1] == "2" {
                    let color_code = format!(
                        "{};{};{};{};{}",
                        parts[i],
                        parts[i + 1],
                        parts[i + 2],
                        parts[i + 3],
                        parts[i + 4]
                    );
                    if code == 38 {
                        self.fg_color = Some(color_code);
                    } else {
                        self.bg_color = Some(color_code);
                    }
                    i += 5;
                    continue;
                }
            }

            match code {
                0 => self.reset(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.inverse = true,
                8 => self.hidden = true,
                9 => self.strikethrough = true,
                21 => self.bold = false,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.inverse = false,
                28 => self.hidden = false,
                29 => self.strikethrough = false,
                39 => self.fg_color = None,
                49 => self.bg_color = None,
                // Standard foreground/background colors.
                _ => {
                    if (30..=37).contains(&code) || (90..=97).contains(&code) {
                        self.fg_color = Some(code.to_string());
                    } else if (40..=47).contains(&code) || (100..=107).contains(&code) {
                        self.bg_color = Some(code.to_string());
                    }
                }
            }
            i += 1;
        }
    }

    fn reset(&mut self) {
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.blink = false;
        self.inverse = false;
        self.hidden = false;
        self.strikethrough = false;
        self.fg_color = None;
        self.bg_color = None;
        // SGR reset does not affect OSC 8 hyperlink state.
    }

    /// Clear all state for reuse.
    pub fn clear(&mut self) {
        self.reset();
        self.active_hyperlink = None;
    }

    pub fn get_active_codes(&self) -> String {
        let mut codes: Vec<&str> = Vec::new();
        if self.bold {
            codes.push("1");
        }
        if self.dim {
            codes.push("2");
        }
        if self.italic {
            codes.push("3");
        }
        if self.underline {
            codes.push("4");
        }
        if self.blink {
            codes.push("5");
        }
        if self.inverse {
            codes.push("7");
        }
        if self.hidden {
            codes.push("8");
        }
        if self.strikethrough {
            codes.push("9");
        }
        if let Some(fg) = &self.fg_color {
            codes.push(fg);
        }
        if let Some(bg) = &self.bg_color {
            codes.push(bg);
        }

        let mut result = if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        };
        if let Some(hyperlink) = &self.active_hyperlink {
            result.push_str(&hyperlink.open_code());
        }
        result
    }

    pub fn get_active_background_code(&self) -> String {
        match &self.bg_color {
            Some(bg) => format!("\x1b[{bg}m"),
            None => String::new(),
        }
    }

    pub fn has_active_codes(&self) -> bool {
        self.bold
            || self.dim
            || self.italic
            || self.underline
            || self.blink
            || self.inverse
            || self.hidden
            || self.strikethrough
            || self.fg_color.is_some()
            || self.bg_color.is_some()
            || self.active_hyperlink.is_some()
    }

    /// Underline must close at line end; active hyperlinks close and re-open
    /// on the next line.
    pub fn get_line_end_reset(&self) -> String {
        let mut result = String::new();
        if self.underline {
            result.push_str("\x1b[24m");
        }
        if let Some(hyperlink) = &self.active_hyperlink {
            result.push_str(&hyperlink.close_code());
        }
        result
    }
}

fn update_tracker_from_text(text: &str, tracker: &mut AnsiCodeTracker) {
    let mut i = 0;
    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            tracker.process(ansi);
            i += ansi.len();
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

/// `getActiveBackgroundAnsi` (utils.ts:747).
pub fn get_active_background_ansi(text: &str) -> String {
    let mut tracker = AnsiCodeTracker::new();
    update_tracker_from_text(text, &mut tracker);
    tracker.get_active_background_code()
}

/// `splitIntoTokensWithAnsi` (utils.ts:798).
fn split_into_tokens_with_ansi(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut current_kind: Option<&'static str> = None;
    let mut i = 0;

    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            pending_ansi.push_str(ansi);
            i += ansi.len();
            continue;
        }

        let end = next_text_run_end(text, i);
        let chunk = &text[i..end];
        // Printable ASCII characters are single graphemes, so skip the
        // segmenter for them.
        let ascii = is_printable_ascii(chunk);
        let segments: Vec<&str> = if ascii {
            (0..chunk.len()).map(|k| &chunk[k..k + 1]).collect()
        } else {
            chunk.graphemes(true).collect()
        };

        for segment in segments {
            let segment_is_space = segment == " ";
            if !ascii && !segment_is_space && is_cjk_break_segment(segment) {
                flush_current(&mut tokens, &mut current, &mut current_kind);
                let token = pending_ansi.clone() + segment;
                pending_ansi.clear();
                tokens.push(token);
                continue;
            }

            let segment_kind: &'static str = if segment_is_space { "space" } else { "word" };
            if !current.is_empty() && current_kind != Some(segment_kind) {
                flush_current(&mut tokens, &mut current, &mut current_kind);
            }

            if !pending_ansi.is_empty() {
                current.push_str(&pending_ansi);
                pending_ansi.clear();
            }

            current_kind = Some(segment_kind);
            current.push_str(segment);
        }

        i = end;
    }

    // Remaining pending ANSI codes attach to the last token.
    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn flush_current(
    tokens: &mut Vec<String>,
    current: &mut String,
    current_kind: &mut Option<&'static str>,
) {
    if current.is_empty() {
        return;
    }
    tokens.push(std::mem::take(current));
    *current_kind = None;
}

/// `wrapTextWithAnsi` (utils.ts:843): word wrapping only — no padding, no
/// background colors; active codes are preserved across line breaks.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut result: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();

    for input_line in js_split_lines(text) {
        let prefix = if result.is_empty() {
            String::new()
        } else {
            tracker.get_active_codes()
        };
        let line = prefix + input_line.as_str();
        for wrapped_line in wrap_single_line(&line, width) {
            result.push(wrapped_line);
        }
        update_tracker_from_text(&input_line, &mut tracker);
    }

    if result.is_empty() {
        vec![String::new()]
    } else {
        result
    }
}

/// JS `text.split(/\r\n|\r|\n/)`.
fn js_split_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut current));
            }
            '\n' => {
                lines.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    lines.push(current);
    lines
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let visible_length = visible_width(line);
    if visible_length <= width {
        return vec![line.to_string()];
    }

    let mut wrapped: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();
    let tokens = split_into_tokens_with_ansi(line);

    let mut current_line = String::new();
    let mut current_visible_length = 0usize;

    for token in &tokens {
        let token_visible_length = visible_width(token);
        let is_whitespace = js_trim(token).is_empty();

        // Token itself is too long — break it character by character.
        if token_visible_length > width && !is_whitespace {
            if !current_line.is_empty() {
                // Underline-only reset preserves the background.
                let line_end_reset = tracker.get_line_end_reset();
                if !line_end_reset.is_empty() {
                    current_line.push_str(&line_end_reset);
                }
                wrapped.push(std::mem::take(&mut current_line));
                // Upstream also zeroes currentVisibleLength here; the value is
                // reassigned below before any read, so the dead store is elided.
            }
            // Upstream also zeroes currentVisibleLength here before the
            // long-word break reassigns it below; the dead store is elided.

            let broken = break_long_word(token, width, &mut tracker);
            for piece in &broken[..broken.len() - 1] {
                wrapped.push(piece.clone());
            }
            current_line = broken[broken.len() - 1].clone();
            current_visible_length = visible_width(&current_line);
            continue;
        }

        let total_needed = current_visible_length + token_visible_length;

        if total_needed > width && current_visible_length > 0 {
            let mut line_to_wrap = js_trim_end(&current_line).to_string();
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                line_to_wrap.push_str(&line_end_reset);
            }
            wrapped.push(line_to_wrap);
            if is_whitespace {
                // Don't start a new line with whitespace.
                current_line = tracker.get_active_codes();
                current_visible_length = 0;
            } else {
                current_line = tracker.get_active_codes() + token.as_str();
                current_visible_length = token_visible_length;
            }
        } else {
            current_line.push_str(token);
            current_visible_length += token_visible_length;
        }

        update_tracker_from_text(token, &mut tracker);
    }

    if !current_line.is_empty() {
        wrapped.push(current_line);
    }

    // Trailing whitespace can push lines past the requested width.
    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped
            .into_iter()
            .map(|line| js_trim_end(&line).to_string())
            .collect()
    }
}

/// `breakLongWord` (utils.ts:965).
fn break_long_word(word: &str, width: usize, tracker: &mut AnsiCodeTracker) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = tracker.get_active_codes();
    let mut current_width = 0usize;

    let mut i = 0;
    let mut segments: Vec<(bool, &str)> = Vec::new(); // (is_ansi, value)

    while i < word.len() {
        if let Some(ansi) = extract_ansi_code(word, i) {
            segments.push((true, ansi));
            i += ansi.len();
        } else {
            let end = next_text_run_end(word, i);
            for seg in word[i..end].graphemes(true) {
                segments.push((false, seg));
            }
            i = end.max(i + 1);
        }
    }

    for (is_ansi, value) in segments {
        if is_ansi {
            current_line.push_str(value);
            tracker.process(value);
            continue;
        }

        if value.is_empty() {
            continue;
        }

        let grapheme_w = grapheme_width(value);

        if current_width + grapheme_w > width {
            // Underline-only reset preserves the background.
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                current_line.push_str(&line_end_reset);
            }
            lines.push(std::mem::take(&mut current_line));
            current_line = tracker.get_active_codes();
            current_width = 0;
        }

        current_line.push_str(value);
        current_width += grapheme_w;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

/// `applyBackgroundToLine` (utils.ts:1042).
pub fn apply_background_to_line(
    line: &str,
    width: usize,
    bg_fn: impl FnOnce(&str) -> String,
) -> String {
    let visible_len = visible_width(line);
    let padding_needed = width.saturating_sub(visible_len);
    let mut with_padding = String::with_capacity(line.len() + padding_needed);
    with_padding.push_str(line);
    with_padding.extend(std::iter::repeat_n(' ', padding_needed));
    bg_fn(&with_padding)
}

/// `PUNCTUATION_REGEX` (utils.ts:949).
const PUNCTUATION_CHARS: &[char] = &[
    '(', ')', '{', '}', '[', ']', '<', '>', '.', ',', ';', ':', '\'', '"', '!', '?', '+', '-', '=',
    '*', '/', '\\', '|', '&', '%', '^', '$', '#', '@', '~', '`',
];

/// `isWhitespaceChar` (utils.ts:954): JS `\s` (includes U+FEFF, no U+0085).
pub fn is_whitespace_char(c: char) -> bool {
    is_js_trim_whitespace(c)
}

/// `isPunctuationChar` (utils.ts:961).
pub fn is_punctuation_char(c: char) -> bool {
    PUNCTUATION_CHARS.contains(&c)
}

/// Upstream `isWhitespaceChar(segment)`: the JS regex `/\s/` is unanchored, so
/// a multi-character segment is whitespace when ANY char matches.
pub fn is_whitespace_segment(segment: &str) -> bool {
    segment.chars().any(is_whitespace_char)
}

/// `normalizeTerminalOutput` (utils.ts:379): Thai/Lao AM vowel compatibility
/// decompositions plus fixed-width tab expansion outside control sequences.
pub fn normalize_terminal_output(s: &str) -> String {
    let mut normalized = String::with_capacity(s.len() + 2);
    if s.contains(['\u{0e33}', '\u{0eb3}']) {
        for c in s.chars() {
            match c {
                '\u{0e33}' => {
                    normalized.push('\u{0e4d}');
                    normalized.push('\u{0e32}');
                }
                '\u{0eb3}' => {
                    normalized.push('\u{0ecd}');
                    normalized.push('\u{0eb2}');
                }
                _ => normalized.push(c),
            }
        }
    } else {
        normalized.push_str(s);
    }
    if !normalized.contains('\t') {
        return normalized;
    }

    let mut result = String::with_capacity(normalized.len());
    let mut i = 0;
    while i < normalized.len() {
        if let Some(ansi) = extract_ansi_code(&normalized, i) {
            result.push_str(ansi);
            i += ansi.len();
            continue;
        }
        let ch = normalized[i..].chars().next();
        let Some(c) = ch else { break };
        if c == '\t' {
            result.push_str("   ");
        } else {
            result.push(c);
        }
        i += c.len_utf8();
    }
    result
}

/// `extractSegments` (utils.ts:1266): "before" and "after" slices for overlay
/// compositing; "after" inherits styling from before the overlay region.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedSegments {
    pub before: String,
    pub before_width: usize,
    pub after: String,
    pub after_width: usize,
}

pub fn extract_segments(
    line: &str,
    before_end: usize,
    after_start: usize,
    after_len: usize,
    strict_after: bool,
) -> ExtractedSegments {
    let mut before = String::new();
    let mut before_width = 0usize;
    let mut after = String::new();
    let mut after_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0usize;
    let mut pending_ansi_before = String::new();
    let mut after_started = false;
    let after_end = after_start + after_len;
    let mut style_tracker = AnsiCodeTracker::new();

    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            style_tracker.process(ansi);
            if current_col < before_end {
                pending_ansi_before.push_str(ansi);
            } else if current_col >= after_start && current_col < after_end && after_started {
                after.push_str(ansi);
            }
            i += ansi.len();
            continue;
        }

        let text_end = next_text_run_end(line, i);

        for segment in line[i..text_end].graphemes(true) {
            let w = grapheme_width(segment);

            if current_col < before_end && current_col + w <= before_end {
                if !pending_ansi_before.is_empty() {
                    before.push_str(&pending_ansi_before);
                    pending_ansi_before.clear();
                }
                before.push_str(segment);
                before_width += w;
            } else if current_col >= after_start && current_col < after_end {
                let fits = !strict_after || current_col + w <= after_end;
                if fits {
                    if !after_started {
                        after.push_str(&style_tracker.get_active_codes());
                        after_started = true;
                    }
                    after.push_str(segment);
                    after_width += w;
                }
            }

            current_col += w;
            let done = if after_len == 0 {
                current_col >= before_end
            } else {
                current_col >= after_end
            };
            if done {
                break;
            }
        }
        i = text_end;
        let done = if after_len == 0 {
            current_col >= before_end
        } else {
            current_col >= after_end
        };
        if done {
            break;
        }
    }

    ExtractedSegments {
        before,
        before_width,
        after,
        after_width,
    }
}
