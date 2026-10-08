//! Port of the marked 18.0.11 lexer (`Lexer.ts`, `Tokenizer.ts`, `rules.ts`,
//! `helpers.ts` extracted from the reference dependency's sourcemap; the
//! upstream pin 18.0.11 tarball is restored from the npm content cache and its
//! SHA-512 matches `package-lock.json`). Upgraded from the previous 18.0.5
//! port; version deltas are disclosed in
//! `docs/migration/TUI_COMPATIBILITY.md`.
//!
//! Most JS rules need lookarounds/backreferences unavailable in the offline
//! `regex` crate and are implemented as scanners; the two title rules use
//! equivalent anchored regexes without those features. The source API accepts
//! UTF-8. Inline-link JS substring boundaries may split a surrogate pair;
//! lossless token overrides carry those units into Markdown rendering while
//! the legacy String fields remain display views. This is not arbitrary raw
//! UTF-16 source input, extension-hook or complete marked parity.
//!
//! Inline attachment: marked queues `{src, tokens}` during the block pass and
//! fills the arrays after; here each block token that needs inline lexing
//! stores its source in `inline_src` and [`Lexer::lex`] walks the finished
//! tree in DFS pre-order — which is exactly marked's queue creation order —
//! and fills `tokens` in place.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::tui::utf16::Utf16Text;

/// A marked token (subset of fields exercised by the markdown component).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Token {
    pub kind: String,
    pub raw: String,
    pub text: String,
    /// Lossless overrides when a JS source substring bisects a surrogate pair.
    /// The legacy String fields are UTF-8 display views, not source offsets.
    pub raw_utf16: Option<Utf16Text>,
    pub text_utf16: Option<Utf16Text>,
    pub href_utf16: Option<Utf16Text>,
    pub depth: usize,
    pub lang: Option<String>,
    pub ordered: bool,
    pub start: usize,
    pub loose: bool,
    pub task: bool,
    pub checked: Option<bool>,
    pub href: String,
    pub items: Vec<Token>,
    pub tokens: Vec<Token>,
    pub header: Vec<Token>,
    pub rows: Vec<Vec<Token>>,
    pub align: Vec<Option<char>>,
    pub pending: bool,
    /// Block tokens whose inline tokens are filled by [`Lexer::lex`].
    pub inline_src: Option<String>,
}

impl Token {
    pub(crate) fn new(kind: &str, raw: impl Into<String>) -> Self {
        Token {
            kind: kind.to_string(),
            raw: raw.into(),
            ..Default::default()
        }
    }

    pub(crate) fn raw_units(&self) -> Utf16Text {
        self.raw_utf16
            .clone()
            .unwrap_or_else(|| Utf16Text::from(&self.raw))
    }
    pub(crate) fn text_units(&self) -> Utf16Text {
        self.text_utf16
            .clone()
            .unwrap_or_else(|| Utf16Text::from(&self.text))
    }
    pub(crate) fn set_raw_units(&mut self, units: Utf16Text) {
        self.raw = units.to_string_lossy();
        self.raw_utf16 = units.to_string_checked().is_err().then_some(units);
    }
    pub(crate) fn set_text_units(&mut self, units: Utf16Text) {
        self.text = units.to_string_lossy();
        self.text_utf16 = units.to_string_checked().is_err().then_some(units);
    }
    fn append_text(&mut self, other: &Token) {
        if self.raw_utf16.is_some() || other.raw_utf16.is_some() {
            let units = self
                .raw_utf16
                .get_or_insert_with(|| Utf16Text::from(&self.raw));
            units.push_str(
                other
                    .raw_utf16
                    .clone()
                    .unwrap_or_else(|| Utf16Text::from(&other.raw)),
            );
        }
        if self.text_utf16.is_some() || other.text_utf16.is_some() {
            let units = self
                .text_utf16
                .get_or_insert_with(|| Utf16Text::from(&self.text));
            units.push_str(
                other
                    .text_utf16
                    .clone()
                    .unwrap_or_else(|| Utf16Text::from(&other.text)),
            );
        }
        self.raw.push_str(&other.raw);
        self.text.push_str(&other.text);
    }
}

/// Advance over exactly JS UTF-16 units. If the boundary divides a scalar,
/// return its remaining low surrogate separately; never round or replace it.
fn consume_utf16_prefix(src: &mut String, units: usize) -> Option<u16> {
    let mut consumed = 0;
    let mut byte_end = src.len();
    let mut remainder = None;
    for (index, c) in src.char_indices() {
        if consumed == units {
            byte_end = index;
            break;
        }
        consumed += c.len_utf16();
        if consumed > units {
            let mut pair = [0; 2];
            c.encode_utf16(&mut pair);
            byte_end = index + c.len_utf8();
            remainder = Some(pair[1]);
            break;
        }
    }
    src.drain(..byte_end);
    remainder
}

/// A source substring can retain one trailing high surrogate after a JS slice.
/// It remains a real unit, never a replacement sentinel.
fn split_inline_tail(text: Utf16Text) -> (String, Option<u16>) {
    let mut units = text.into_units();
    let tail = units
        .last()
        .is_some_and(|u| (0xd800..=0xdbff).contains(u))
        .then(|| units.pop().unwrap());
    let prefix = String::from_utf16(&units)
        .expect("inline source substring has only a possible trailing high surrogate");
    (prefix, tail)
}

/// Consume exact JS units and retain a split pair's low unit in the following
/// text token. The optional trailing high unit belongs to the actual source.
fn consume_inline_prefix(
    src: &mut String,
    tail: &mut Option<u16>,
    units: usize,
    ext: &dyn LexerExtensions,
) -> Option<Token> {
    if units > src.encode_utf16().count() {
        *tail = None;
    }
    let low = consume_utf16_prefix(src, units)?;
    let cut = ext.inline_start(src).unwrap_or(src.len()).min(src.len());
    let len = inline_text_continuation(&src[..cut], 0);
    let mut text = Utf16Text::from_units(vec![low]);
    text.push_str(&src[..len]);
    src.drain(..len);
    if src.is_empty() {
        if let Some(high) = tail.take() {
            text.push(Utf16Text::from_units(vec![high]));
        }
    }
    let mut token = Token::new("text", "");
    token.set_raw_units(text.clone());
    token.set_text_units(text);
    Some(token)
}

/// Extension hooks the markdown component installs (latex block/inline
/// tokenizers, strict strikethrough `del`).
pub trait LexerExtensions {
    fn block_tokenizer(&self, _lexer: &mut Lexer, _src: &str) -> Option<Token> {
        None
    }
    /// Index (relative to `src`) where a block extension could start.
    fn block_start(&self, _src: &str) -> Option<usize> {
        None
    }
    fn inline_tokenizer(&self, _lexer: &mut Lexer, _src: &str) -> Option<Token> {
        None
    }
    /// Internal source slices can retain one trailing high surrogate. Legacy
    /// extensions keep their UTF-8 hook; raw-aware extensions may override this
    /// adapter without a lossy sentinel. This is not arbitrary raw-source input.
    fn inline_tokenizer_with_tail(
        &self,
        lexer: &mut Lexer,
        src: &str,
        _tail: Option<u16>,
    ) -> Option<Token> {
        self.inline_tokenizer(lexer, src)
    }
    /// Index (relative to `src`) where an inline extension could start.
    fn inline_start(&self, _src: &str) -> Option<usize> {
        None
    }
    /// Upstream `Tokenizer.del` override (StrictStrikethroughTokenizer).
    fn del(&self, _lexer: &mut Lexer, _src: &str) -> Option<Token> {
        None
    }
}

/// No-op extensions (plain marked defaults).
pub struct NoExtensions;
impl LexerExtensions for NoExtensions {}

// ---------------------------------------------------------------------------
// Character classes (JS \p{P}\p{S} / \s via icu_properties)
// ---------------------------------------------------------------------------

pub(crate) fn is_js_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{feff}')
        || crate::tui::utils::is_js_space_unicode(c)
}

fn skip_js_space(src: &str, start: usize) -> usize {
    start
        + src[start..]
            .chars()
            .take_while(|&c| is_js_space(c))
            .map(char::len_utf8)
            .sum::<usize>()
}

fn is_js_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

pub(crate) fn is_punct_or_symbol(c: char) -> bool {
    crate::tui::utils::is_punct_or_symbol_unicode(c)
}

fn is_email_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '.' | '!'
                | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '/'
                | '='
                | '?'
                | '_'
                | '`'
                | '{'
                | '|'
                | '}'
                | '~'
                | '-'
        )
}

fn is_protocol_at(src: &str, i: usize) -> bool {
    let rest = &src[i..];
    for p in ["https://", "http://", "ftp://"] {
        // [hH][tT]... case-insensitive protocols
        if rest
            .as_bytes()
            .get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p.as_bytes()))
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// helpers.ts
// ---------------------------------------------------------------------------

fn rtrim(s: &str, c: char) -> String {
    s.trim_end_matches(c).to_string()
}

fn trim_trailing_blank_lines(s: &str) -> String {
    let lines: Vec<&str> = s.split('\n').collect();
    let blank = |l: &str| l.chars().all(|c| c == ' ' || c == '\t');
    let mut end = lines.len() as i64 - 1;
    while end >= 0 && blank(lines[end as usize]) {
        end -= 1;
    }
    if lines.len() as i64 - end <= 2 {
        return s.to_string();
    }
    lines[..(end + 1) as usize].join("\n")
}

fn find_closing_bracket(s: &str, open: char, close: char) -> i64 {
    if !s.contains(close) {
        return -1;
    }
    let mut level = 0i64;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == open {
            level += 1;
        } else if c == close {
            level -= 1;
            if level < 0 {
                return i as i64;
            }
        }
    }
    if level > 0 {
        return -2;
    }
    -1
}

fn expand_tabs(line: &str, indent: usize) -> String {
    let mut col = indent;
    let mut out = String::new();
    for c in line.chars() {
        if c == '\t' {
            let added = 4 - (col % 4);
            for _ in 0..added {
                out.push(' ');
            }
            col += added;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// `splitCells(tableRow, count?)` (helpers.ts): unescaped `|` → `" |"`, then
/// split on `" |"`.
fn split_cells(table_row: &str, count: Option<usize>) -> Vec<String> {
    let chars: Vec<char> = table_row.chars().collect();
    let mut row = String::new();
    for (offset, c) in chars.iter().copied().enumerate() {
        if c != '|' {
            row.push(c);
            continue;
        }
        let mut escaped = false;
        let mut curr = offset;
        while curr > 0 {
            curr -= 1;
            if chars[curr] == '\\' {
                escaped = !escaped;
            } else {
                break;
            }
        }
        if escaped {
            row.push('|');
        } else {
            row.push_str(" |");
        }
    }
    let mut cells: Vec<String> = row.split(" |").map(str::to_string).collect();
    if cells
        .first()
        .is_some_and(|s| s.trim_matches(is_js_space).is_empty())
    {
        cells.remove(0);
    }
    if cells
        .last()
        .is_some_and(|s| s.trim_matches(is_js_space).is_empty())
    {
        cells.pop();
    }
    if let Some(count) = count {
        if cells.len() > count {
            cells.truncate(count);
        } else {
            while cells.len() < count {
                cells.push(String::new());
            }
        }
    }
    cells
        .iter()
        .map(|c| c.trim_matches(is_js_space).replace("\\|", "|"))
        .collect()
}

// ---------------------------------------------------------------------------
// Block rules (rules.ts, gfm set)
// ---------------------------------------------------------------------------

/// `block.newline`: /^(?:[ \t]*(?:\n|$))+/
fn rx_newline(src: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut n = 0usize;
    loop {
        let mut j = n;
        while j < len && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if j < len && b[j] == b'\n' {
            n = j + 1;
        } else if j == len {
            if j == n {
                break;
            }
            n = j;
            break;
        } else {
            break;
        }
    }
    (n > 0).then_some(n)
}

/// One indented-code line group: `(?: {4}| {0,3}\t)[^\n]+(?:\n(?:[ \t]*(?:\n|$))*)?`
fn block_code_line_group(src: &str, start: usize) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut j = start;
    if len >= j + 4 && b[j..j + 4] == *b"    " {
        j += 4;
    } else {
        let mut k = j;
        while k < len && k < j + 3 && b[k] == b' ' {
            k += 1;
        }
        if k < len && b[k] == b'\t' {
            j = k + 1;
        } else {
            return None;
        }
    }
    let s = j;
    while j < len && b[j] != b'\n' {
        j += 1;
    }
    if j == s {
        return None;
    }
    if j < len && b[j] == b'\n' {
        let mut k = j + 1;
        loop {
            let mut m = k;
            while m < len && (b[m] == b' ' || b[m] == b'\t') {
                m += 1;
            }
            if m < len && b[m] == b'\n' {
                k = m + 1;
            } else if m == len {
                k = m;
                break;
            } else {
                break;
            }
        }
        j = k;
    }
    Some(j)
}

/// `block.code`: returns raw end.
fn rx_block_code(src: &str) -> Option<usize> {
    let mut n = block_code_line_group(src, 0)?;
    while let Some(next) = block_code_line_group(src, n) {
        n = next;
    }
    Some(n)
}

/// `block.fences`: returns (total_len, info, body).
fn rx_fences(src: &str) -> Option<(usize, String, String)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    let fence_char = *b.get(i)?;
    if fence_char != b'`' && fence_char != b'~' {
        return None;
    }
    let mut run_end = i;
    while run_end < len && b[run_end] == fence_char {
        run_end += 1;
    }
    if run_end - i < 3 {
        return None;
    }
    if fence_char == b'`' {
        let mut k = run_end;
        while k < len && b[k] != b'`' && b[k] != b'\n' {
            k += 1;
        }
        if k < len && b[k] == b'`' {
            return None;
        }
    }
    let marker: String = src[i..run_end].to_string();
    let info_start = run_end;
    let mut j = run_end;
    while j < len && b[j] != b'\n' {
        j += 1;
    }
    let info: String = src[info_start..j].to_string();
    if j >= len {
        // (?:\n|$) matched $; body empty; closing via `|$`
        return Some((j, info, String::new()));
    }
    let body_start = j + 1;
    let mut p = body_start;
    loop {
        let at_end = p >= len;
        if at_end || b[p] == b'\n' {
            let body = src[body_start..p].to_string();
            let close_from = if at_end { p } else { p + 1 };
            if let Some(close_end) = try_fence_close(src, close_from, &marker) {
                return Some((close_end, info, body));
            }
        }
        if at_end {
            return None;
        }
        p += 1;
    }
}

fn try_fence_close(src: &str, from: usize, marker: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    if from == len {
        return Some(from);
    }
    let mut j = from;
    while j < len && j < from + 3 && b[j] == b' ' {
        j += 1;
    }
    if !src[j..].starts_with(marker) {
        return None;
    }
    j += marker.len();
    while j < len && (b[j] == b'`' || b[j] == b'~') {
        j += 1;
    }
    while j < len && b[j] == b' ' {
        j += 1;
    }
    if j >= len || b[j] == b'\n' {
        return Some(j);
    }
    None
}

/// `block.hr`.
fn rx_hr(src: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    let hc = *b.get(i)?;
    if hc != b'-' && hc != b'_' && hc != b'*' {
        return None;
    }
    let mut count = 0usize;
    let mut j = i;
    while j < len && b[j] == hc {
        count += 1;
        j += 1;
        while j < len && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
    }
    if count < 3 {
        return None;
    }
    if j >= len {
        return Some(j);
    }
    if b[j] == b'\n' {
        while j < len && b[j] == b'\n' {
            j += 1;
        }
        return Some(j);
    }
    None
}

/// `block.heading`: returns (end, depth, text).
fn rx_heading(src: &str) -> Option<(usize, usize, String)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    let hash_start = i;
    while i < len && b[i] == b'#' && i - hash_start < 6 {
        i += 1;
    }
    let depth = i - hash_start;
    if depth == 0 {
        return None;
    }
    if src[i..].chars().next().is_some_and(|c| !is_js_space(c)) {
        return None;
    }
    let text_start = i;
    i += src[i..].find(is_js_line_terminator).unwrap_or(len - i);
    if i < len && b[i] != b'\n' {
        return None;
    }
    let text = src[text_start..i].to_string();
    let end = if i < len {
        let mut j = i;
        while j < len && b[j] == b'\n' {
            j += 1;
        }
        j
    } else {
        i
    };
    Some((end, depth, text))
}

/// `block.list` head: ^( {0,3}(?:[*+-]|\d{1,9}[.)]))([ \t][^\n]*?)?(?:\n|$)
fn rx_list_head(src: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    let ordered = b.get(i).is_some_and(|c| c.is_ascii_digit());
    if ordered {
        let digits_start = i;
        while i < len && b[i].is_ascii_digit() && i - digits_start < 9 {
            i += 1;
        }
        if i == digits_start || !matches!(b.get(i), Some(b'.') | Some(b')')) {
            return None;
        }
        i += 1;
    } else {
        match b.get(i) {
            Some(b'*') | Some(b'+') | Some(b'-') => i += 1,
            _ => return None,
        }
    }
    if i < len && (b[i] == b' ' || b[i] == b'\t') {
        while i < len && b[i] != b'\n' {
            i += 1;
        }
    }
    if i < len && b[i] != b'\n' {
        return None;
    }
    Some(i + if i < len { 1 } else { 0 })
}

/// Per-item regex: ^( {0,3}<bull>)((?:[\t ][^\n]*)?(?:\n|$)).
/// Returns (total_end, bullet_str, group2).
fn rx_list_item(src: &str, ordered: bool, bullet_char: char) -> Option<(usize, String, String)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    let bullet_start = i;
    if ordered {
        let digits_start = i;
        while i < len && b[i].is_ascii_digit() && i - digits_start < 9 {
            i += 1;
        }
        if i == digits_start || b.get(i) != Some(&(bullet_char as u8)) {
            return None;
        }
        i += 1;
    } else {
        if b.get(i) != Some(&(bullet_char as u8)) {
            return None;
        }
        i += 1;
    }
    let bullet = src[bullet_start..i].to_string();
    if i < len && (b[i] == b' ' || b[i] == b'\t') {
        while i < len && b[i] != b'\n' {
            i += 1;
        }
    }
    let group2_start = bullet_start + bullet.len();
    let group2_end = if i < len && b[i] == b'\n' {
        i += 1;
        i
    } else if i == len {
        i
    } else {
        return None;
    };
    Some((
        group2_end,
        bullet,
        src[group2_start..group2_end].to_string(),
    ))
}

// -- shared negative-lookahead sets (lheading) --------------------------------

fn lheading_block_start_at(src: &str, pos: usize, gfm: bool) -> bool {
    let s = &src[pos..];
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && i < 3 && b[i] == b' ' {
        i += 1;
    }
    // bull = / {0,3}(?:[*+-]|\d{1,9}[.)]) / (trailing space from "bull ")
    let bulletish = match b.get(i) {
        Some(b'*') | Some(b'+') | Some(b'-') => {
            i += 1;
            true
        }
        Some(c) if c.is_ascii_digit() => {
            let ds = i;
            while i < b.len() && b[i].is_ascii_digit() && i - ds < 9 {
                i += 1;
            }
            if matches!(b.get(i), Some(b'.') | Some(b')')) {
                i += 1;
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if bulletish && b.get(i) == Some(&b' ') {
        return true;
    }
    // blockCode = /(?: {4}| {0,3}\t)/
    if s.starts_with("    ") {
        return true;
    }
    let spaces = s.chars().take_while(|&c| c == ' ').count().min(3);
    let rest = &s[spaces..];
    if rest.starts_with('\t') {
        return true;
    }
    // fences = / {0,3}(?:`{3,}|~{3,})/
    if rest.starts_with("```") || rest.starts_with("~~~") {
        return true;
    }
    // blockquote = / {0,3}>/
    if rest.starts_with('>') {
        return true;
    }
    // heading = / {0,3}#{1,6}(?:\s|$)/
    {
        let hashes = rest.chars().take_while(|&c| c == '#').count();
        if hashes >= 1 {
            let n = hashes.min(6);
            let after = rest[n..].chars().next();
            if after.is_none_or(is_js_space) {
                return true;
            }
        }
    }
    // html = / {0,3}<[^\n>]+>\n/
    if let Some(stripped) = rest.strip_prefix('<') {
        if let Some(gt) = stripped.find(['>', '\n']) {
            if rest.as_bytes()[1 + gt] == b'>' && rest[gt + 2..].starts_with('\n') {
                return true;
            }
        }
    }
    // table (gfm) = / {0,3}\|?(?:[:\- ]*\|)+[:\- ]*\n/
    if gfm {
        let rest2 = rest.strip_prefix('|').unwrap_or(rest);
        let bb = rest2.as_bytes();
        let mut k = 0usize;
        let mut pipes = 0usize;
        while k < bb.len() {
            match bb[k] {
                b':' | b'-' | b' ' => k += 1,
                b'|' => {
                    pipes += 1;
                    k += 1;
                }
                _ => break,
            }
        }
        if pipes >= 1 && bb.get(k) == Some(&b'\n') {
            return true;
        }
    }
    false
}

fn blank_line_ahead(src: &str, pos: usize) -> bool {
    // \s*?\n — whitespace run reaching a newline
    let b = src.as_bytes();
    let mut j = pos;
    while j < b.len() {
        if b[j] == b'\n' {
            return true;
        }
        if !is_js_space(b[j] as char) {
            return false;
        }
        j += 1;
    }
    false
}

/// `block.lheading` (gfm). Returns (end, text, level_char).
fn rx_lheading(src: &str, gfm: bool) -> Option<(usize, String, char)> {
    let b = src.as_bytes();
    let len = b.len();
    if lheading_block_start_at(src, 0, gfm) {
        return None;
    }
    let mut pos = 0usize;
    loop {
        if pos >= len {
            return None;
        }
        if b[pos] == b'\n' {
            if blank_line_ahead(src, pos + 1) || lheading_block_start_at(src, pos + 1, gfm) {
                return None;
            }
            pos += 1;
        } else {
            let c = src[pos..].chars().next()?;
            if is_js_line_terminator(c) {
                return None;
            }
            pos += c.len_utf8();
        }
        // lazy: attempt tail `\n {0,3}(=+|-+) *(?:\n+|$)` after each unit
        if b.get(pos) == Some(&b'\n') {
            let mut j = pos + 1;
            while j < len && j < pos + 4 && b[j] == b' ' {
                j += 1;
            }
            if matches!(b.get(j), Some(b'=') | Some(b'-')) {
                let level_char = b[j] as char;
                while j < len && (b[j] == b'=' || b[j] == b'-') {
                    j += 1;
                }
                while j < len && b[j] == b' ' {
                    j += 1;
                }
                if j >= len {
                    return Some((j, src[..pos].to_string(), level_char));
                }
                if b[j] == b'\n' {
                    while j < len && b[j] == b'\n' {
                        j += 1;
                    }
                    return Some((j, src[..pos].to_string(), level_char));
                }
            }
        }
    }
}

// -- paragraph ---------------------------------------------------------------

/// gfm paragraph continuation-line negative lookahead.
fn paragraph_line_interrupted(src: &str, gfm: bool) -> bool {
    let s = src;
    if rx_hr(s).is_some() {
        return true;
    }
    // heading: ' {0,3}#{1,6}(?:\s|$)'
    let spaces = s.chars().take_while(|&c| c == ' ').count().min(3);
    let rest = &s[spaces..];
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        let after = rest[hashes..].chars().next();
        if after.is_none_or(is_js_space) {
            return true;
        }
    }
    // blockquote: ' {0,3}>'
    if rest.starts_with('>') {
        return true;
    }
    // fences: ' {0,3}(`{3,}(?=[^`\n]*(?:\n|$))|~~~)[^\n]*(?:\n|$)'
    {
        let bb = rest.as_bytes();
        if bb.first() == Some(&b'`') || bb.first() == Some(&b'~') {
            let fc = bb[0];
            let mut k = 0usize;
            while k < bb.len() && bb[k] == fc {
                k += 1;
            }
            let ok = if fc == b'`' {
                if k >= 3 {
                    // (?=[^`\n]*(?:\n|$)): no backtick may appear in the info
                    // string before the line end (or EOF).
                    let mut m = k;
                    while m < bb.len() && bb[m] != b'`' && bb[m] != b'\n' {
                        m += 1;
                    }
                    m >= bb.len() || bb[m] == b'\n'
                } else {
                    false
                }
            } else {
                k >= 3
            };
            if ok {
                return true;
            }
        }
    }
    // list: ' {0,3}(?:[*+-]|1[.)])[ \t]+[^ \t\n]'
    {
        let bb = rest.as_bytes();
        let is_bullet = match bb.first() {
            Some(b'*') | Some(b'+') | Some(b'-') => true,
            Some(b'1') => matches!(bb.get(1), Some(b'.') | Some(b')')),
            _ => false,
        };
        if is_bullet {
            let mut k = if bb[0] == b'1' { 2 } else { 1 };
            let sp0 = k;
            while k < bb.len() && (bb[k] == b' ' || bb[k] == b'\t') {
                k += 1;
            }
            if k > sp0 && k < bb.len() && !matches!(bb[k], b' ' | b'\t' | b'\n') {
                return true;
            }
        }
    }
    if gfm && rx_table(s).is_some() {
        return true;
    }
    // html: '</?(?:tag)(?: +|\n|/?>)|<(?:script|pre|style|textarea|!--)'
    if html_interrupt_at(s) {
        return true;
    }
    // '[ \t]+\n' (18.0.11: tab-only lines also fail to continue a paragraph)
    {
        let run = s.chars().take_while(|&c| c == ' ' || c == '\t').count();
        if run >= 1 && s[run..].starts_with('\n') {
            return true;
        }
    }
    false
}

fn html_interrupt_at(s: &str) -> bool {
    let lower = s.to_lowercase();
    for tag in ["<script", "<pre", "<style", "<textarea", "<!--"] {
        if lower.starts_with(tag) {
            return true;
        }
    }
    let name_at = |rest: &str| -> bool {
        let name_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        if !TYPE6_TAGS.contains(&&rest[..name_end]) {
            return false;
        }
        let after = &rest[name_end..];
        after.starts_with(' ')
            || after.starts_with('\n')
            || after.starts_with("/>")
            || after.starts_with('>')
    };
    if let Some(rest) = s.strip_prefix('<') {
        if name_at(rest) {
            return true;
        }
    }
    if let Some(rest) = s.strip_prefix("</") {
        if name_at(rest) {
            return true;
        }
    }
    false
}

const TYPE6_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "meta",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// `block.paragraph` (gfm): returns raw length.
fn rx_paragraph(src: &str, gfm: bool) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && b[i] != b'\n' {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    loop {
        if i + 1 >= len || b[i] != b'\n' || b[i + 1] == b'\n' {
            break;
        }
        if paragraph_line_interrupted(&src[i + 1..], gfm) {
            break;
        }
        let mut j = i + 1;
        while j < len && b[j] != b'\n' {
            j += 1;
        }
        i = j;
    }
    Some(i)
}

/// `block.blockquote` (gfm): ^( {0,3}> ?(paragraph|[^\n]*)(?:\n|$))+ — end
/// offset. helpers.edit strips the caret from the embedded paragraph regex, so
/// the `paragraph` branch is live: each `>` line also consumes lazy
/// continuation lines until a line starts with an interrupting construct
/// (heading/hr/blockquote/fences/list/html or a `[ \t]+` blank line).
fn rx_blockquote_end(src: &str, _gfm: bool) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut n = 0usize;
    loop {
        let mut j = n;
        let mut spaces = 0;
        while j < len && spaces < 3 && b[j] == b' ' {
            j += 1;
            spaces += 1;
        }
        if b.get(j) != Some(&b'>') {
            break;
        }
        j += 1;
        if b.get(j) == Some(&b' ') {
            j += 1;
        }
        let mut line_end = j;
        while line_end < len && b[line_end] != b'\n' {
            line_end += 1;
        }
        if line_end > j {
            // paragraph branch: [^\n]+ then (\n(?!interrupt)[^\n]+)*
            let mut k = line_end;
            while k < len && b[k] == b'\n' && !quote_paragraph_interrupted(src, k + 1) {
                let mut m = k + 1;
                while m < len && b[m] != b'\n' {
                    m += 1;
                }
                if m == k + 1 {
                    break; // [^\n]+ requires content
                }
                k = m;
            }
            j = k;
        }
        if j < len && b[j] == b'\n' {
            j += 1;
        }
        n = j;
    }
    (n > 0).then_some(n)
}

/// Negative lookahead set for blockquote paragraph continuation lines
/// (18.0.11 `blockquoteParagraph`: hr | heading | `>` | fences | bare list |
/// html | `[ \t]+\n`). `pos` is the start of a line.
fn quote_paragraph_interrupted(src: &str, pos: usize) -> bool {
    let s = &src[pos..];
    if rx_hr(s).is_some() {
        return true;
    }
    let spaces = s.chars().take_while(|&c| c == ' ').count().min(3);
    let rest = &s[spaces..];
    // heading: ' {0,3}#{1,6}(?:\s|$)'
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if hashes >= 1 {
        let n = hashes.min(6);
        if rest[n..].chars().next().is_none_or(is_js_space) {
            return true;
        }
    }
    // blockquote: ' {0,3}>'
    if rest.starts_with('>') {
        return true;
    }
    // fences: ' {0,3}(`{3,}(?=[^`\n]*(?:\n|$))|~~~)[^\n]*(?:\n|$)'
    {
        let bb = rest.as_bytes();
        if bb.first() == Some(&b'`') || bb.first() == Some(&b'~') {
            let fc = bb[0];
            let mut k = 0usize;
            while k < bb.len() && bb[k] == fc {
                k += 1;
            }
            let ok = if fc == b'`' {
                if k >= 3 {
                    let mut m = k;
                    while m < bb.len() && bb[m] != b'`' && bb[m] != b'\n' {
                        m += 1;
                    }
                    m >= bb.len() || bb[m] == b'\n'
                } else {
                    false
                }
            } else {
                k >= 3
            };
            if ok {
                return true;
            }
        }
    }
    // list (blockquote variant): ' {0,3}([*+-]|\d{1,9}[.)])(?:[ \t]|\n|$)'
    {
        let bb = rest.as_bytes();
        let marker_len = match bb.first() {
            Some(b'*') | Some(b'+') | Some(b'-') => Some(1usize),
            Some(c) if c.is_ascii_digit() => {
                let mut k = 1usize;
                while k < bb.len() && k <= 9 && bb[k].is_ascii_digit() {
                    k += 1;
                }
                matches!(bb.get(k), Some(b'.') | Some(b')')).then_some(k + 1)
            }
            _ => None,
        };
        if let Some(mlen) = marker_len {
            match bb.get(mlen) {
                None => return true,
                Some(&c) if c == b' ' || c == b'\t' || c == b'\n' => return true,
                _ => {}
            }
        }
    }
    if html_interrupt_at(s) {
        return true;
    }
    // '[ \t]+\n'
    {
        let run = s.chars().take_while(|&c| c == ' ' || c == '\t').count();
        if run >= 1 && s[run..].starts_with('\n') {
            return true;
        }
    }
    false
}

// -- html block ---------------------------------------------------------------

fn comment_at(src: &str) -> Option<usize> {
    let rest = src.strip_prefix("<!--")?;
    if let Some(rest2) = rest.strip_prefix('>') {
        return Some(4 + (rest.len() - rest2.len()));
    }
    if let Some(rest2) = rest.strip_prefix("->") {
        return Some(4 + (rest.len() - rest2.len()));
    }
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'-' && rest[i..].starts_with("-->") {
            return Some(4 + i + 3);
        }
        i += 1;
    }
    Some(src.len())
}

/// One html attribute (block variant: ` +name(?: *=...)?`).
fn attribute_at(src: &str, block_variant: bool) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    if block_variant {
        while i < len && b[i] == b' ' {
            i += 1;
        }
    } else {
        i = skip_js_space(src, i);
    }
    if i == 0 {
        return None;
    }
    let c0 = *b.get(i)?;
    if !(c0.is_ascii_alphabetic() || c0 == b':' || c0 == b'_') {
        return None;
    }
    i += 1;
    while i < len && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'.' | b':' | b'-' | b'_')) {
        i += 1;
    }
    // optional ` *= value`
    let save = i;
    let mut j = i;
    if block_variant {
        while j < len && b[j] == b' ' {
            j += 1;
        }
    } else {
        j = skip_js_space(src, j);
    }
    if b.get(j) == Some(&b'=') {
        j += 1;
        if block_variant {
            while j < len && b[j] == b' ' {
                j += 1;
            }
        } else {
            j = skip_js_space(src, j);
        }
        match b.get(j) {
            Some(b'"') => {
                j += 1;
                let mut k = j;
                while k < len && b[k] != b'"' && (!block_variant || b[k] != b'\n') {
                    k += 1;
                }
                if b.get(k) == Some(&b'"') {
                    return Some(k + 1);
                }
                return None;
            }
            Some(b'\'') => {
                j += 1;
                let mut k = j;
                while k < len && b[k] != b'\'' && (!block_variant || b[k] != b'\n') {
                    k += 1;
                }
                if b.get(k) == Some(&b'\'') {
                    return Some(k + 1);
                }
                return None;
            }
            _ => {
                let k = j + src[j..]
                    .find(|c| is_js_space(c) || matches!(c, '"' | '\'' | '=' | '<' | '>' | '`'))
                    .unwrap_or(len - j);
                if k > j {
                    return Some(k);
                }
                return None;
            }
        }
    }
    Some(save)
}

fn html_body_to_blank(src: &str, from: usize) -> Option<usize> {
    // [\s\S]*?(?:(?:\n[ \t]*)+\n|$): stop at the first blank-line run,
    // consuming all its newlines but not indentation after the final one.
    let b = src.as_bytes();
    let mut cursor = from;
    while let Some(offset) = src[cursor..].find('\n') {
        let first = cursor + offset;
        let mut next = first + 1;
        while matches!(b.get(next), Some(b' ' | b'\t')) {
            next += 1;
        }
        if b.get(next) == Some(&b'\n') {
            let mut end = next + 1;
            loop {
                next = end;
                while matches!(b.get(next), Some(b' ' | b'\t')) {
                    next += 1;
                }
                if b.get(next) != Some(&b'\n') {
                    return Some(end);
                }
                end = next + 1;
            }
        }
        cursor = next;
    }
    Some(src.len())
}

/// `block.html` (gfm). Returns raw length.
fn rx_html_block(src: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    let s = &src[i..];
    let lower = s.to_ascii_lowercase();
    for tag in ["script", "pre", "style", "textarea"] {
        if lower.starts_with(&format!("<{tag}")) {
            let after = s[1 + tag.len()..].chars().next();
            if after.is_some_and(|c| c == '>' || is_js_space(c)) {
                let body_start = 1 + tag.len() + after.unwrap().len_utf8();
                let close = format!("</{tag}>");
                match lower[body_start..].find(&close) {
                    Some(p) => {
                        let abs = body_start + p + close.len();
                        let mut k = abs;
                        while k < s.len() && s.as_bytes()[k] != b'\n' {
                            k += 1;
                        }
                        if k < s.len() {
                            let mut e = k;
                            while e < s.len() && s.as_bytes()[e] == b'\n' {
                                e += 1;
                            }
                            return Some(i + e);
                        }
                        return Some(i + k);
                    }
                    None => return Some(src.len()),
                }
            }
        }
    }
    if let Some(clen) = comment_at(s) {
        let mut k = i + clen;
        while k < len && b[k] != b'\n' {
            k += 1;
        }
        if k < len {
            let mut e = k;
            while e < len && b[e] == b'\n' {
                e += 1;
            }
            return Some(e);
        }
        return Some(k);
    }
    // (3)/(4)/(5) close markers swallow the rest of their line (18.0.11:
    // `(?:\?>[^\n]*\n*|$)` and friends).
    let close_to_line_end = |from: usize| -> usize {
        let mut k = from;
        while k < len && b[k] != b'\n' {
            k += 1;
        }
        let mut e = k;
        while e < len && b[e] == b'\n' {
            e += 1;
        }
        e
    };
    if s.starts_with("<?") {
        if let Some(p) = s.find("?>") {
            return Some(i + close_to_line_end(p + 2));
        }
        return Some(len);
    }
    if s.starts_with("<!") && s.as_bytes().get(2).is_some_and(|c| c.is_ascii_alphabetic()) {
        if let Some(p) = s.find('>') {
            return Some(i + close_to_line_end(p + 1));
        }
        return Some(len);
    }
    if lower.starts_with("<![cdata[") {
        if let Some(p) = s.find("]]>") {
            return Some(i + close_to_line_end(p + 3));
        }
        return Some(len);
    }
    // (6) </?tag(?: +|\n|/?>)...
    {
        let name_start = if s.starts_with("</") {
            2
        } else if s.starts_with('<') {
            1
        } else {
            0
        };
        if name_start > 0 {
            let rest = &s[name_start..];
            let name_end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(rest.len());
            if TYPE6_TAGS.contains(&rest[..name_end].to_ascii_lowercase().as_str()) {
                let after = &rest[name_end..];
                let sep_len = if after.starts_with([' ', '\n']) {
                    1
                } else if after.starts_with("/>") {
                    2
                } else if after.starts_with('>') {
                    1
                } else {
                    0
                };
                if sep_len > 0 {
                    return html_body_to_blank(src, i + name_start + name_end + sep_len);
                }
            }
        }
    }
    // (7) open/closing tag line + body to blank line
    {
        let rest = s.strip_prefix('<')?;
        let is_close = rest.starts_with('/');
        let rest2 = if is_close { &rest[1..] } else { rest };
        let lower2 = rest2.to_ascii_lowercase();
        if ["script", "pre", "style", "textarea"]
            .iter()
            .any(|t| lower2.starts_with(t))
        {
            return None;
        }
        let name_end = rest2
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest2.len());
        if name_end == 0 || !rest2.as_bytes()[0].is_ascii_alphabetic() {
            return None;
        }
        let after = &rest2[name_end..];
        let mut p;
        if is_close {
            let mut k = 0usize;
            while k < after.len() && is_js_space(after.as_bytes()[k] as char) {
                k += 1;
            }
            if !after[k..].starts_with('>') {
                return None;
            }
            p = i + 1 + 1 + name_end + k + 1;
        } else {
            let mut k = 0usize;
            loop {
                let at = &after[k..];
                let mut m = 0usize;
                while m < at.len() && at.as_bytes()[m] == b' ' {
                    m += 1;
                }
                if at[m..].starts_with("/>") {
                    k += m + 2;
                    break;
                }
                if at[m..].starts_with('>') {
                    k += m + 1;
                    break;
                }
                match attribute_at(at, true) {
                    Some(alen) if alen > 0 => k += alen,
                    _ => return None,
                }
            }
            p = i + 1 + name_end + k;
        }
        while p < len && (b[p] == b' ' || b[p] == b'\t') {
            p += 1;
        }
        if p < len && b[p] != b'\n' {
            return None;
        }
        html_body_to_blank(src, p)
    }
}

// -- def ----------------------------------------------------------------------

/// `block.def`. Returns (end, tag, href, title).
fn rx_def(src: &str) -> Option<(usize, String, String, String)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && i < 3 && b[i] == b' ' {
        i += 1;
    }
    if b.get(i) != Some(&b'[') {
        return None;
    }
    i += 1;
    {
        let j = skip_js_space(src, i);
        if b.get(j) == Some(&b']') {
            return None;
        }
    }
    let label_start = i;
    let mut escaped = false;
    while i < len {
        let c = b[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if c == b'[' {
            return None;
        }
        if c == b']' {
            break;
        }
        i += 1;
    }
    if b.get(i) != Some(&b']') {
        return None;
    }
    let label = src[label_start..i].to_string();
    i += 1;
    if b.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    while i < len && b[i] == b' ' {
        i += 1;
    }
    let mut j = i;
    if j < len && b[j] == b'\n' {
        let mut k = j + 1;
        while k < len && (b[k] == b' ' || b[k] == b'\t') {
            k += 1;
        }
        j = k;
    }
    // href: ([^<\s][^\s]*|<.*?>)
    let href_start = j;
    if b.get(j) == Some(&b'<') {
        let mut k = j + 1;
        while k < len && b[k] != b'\n' && b[k] != b'>' {
            k += 1;
        }
        if b.get(k) != Some(&b'>') {
            return None;
        }
        j = k + 1;
    } else {
        if j >= len || src[j..].chars().next().is_some_and(is_js_space) {
            return None;
        }
        j += src[j..].find(is_js_space).unwrap_or(len - j);
    }
    let href_raw = src[href_start..j].to_string();
    // optional title
    let mut title: Option<String> = None;
    let save = j;
    let sep = {
        let mut k = j;
        let mut found = None;
        if k < len && b[k] == b' ' {
            while k < len && b[k] == b' ' {
                k += 1;
            }
            if k < len && b[k] == b'\n' {
                let mut m = k + 1;
                while m < len && (b[m] == b' ' || b[m] == b'\t') {
                    m += 1;
                }
                found = Some(m);
            } else {
                found = Some(k);
            }
        } else {
            while k < len && b[k] == b' ' {
                k += 1;
            }
            if k < len && b[k] == b'\n' {
                let mut m = k + 1;
                while m < len && (b[m] == b' ' || b[m] == b'\t') {
                    m += 1;
                }
                found = Some(m);
            }
        }
        found
    };
    if let Some(after_sep) = sep {
        if let Some(tlen) = definition_title_at(&src[after_sep..]) {
            title = Some(src[after_sep..after_sep + tlen].to_string());
            j = after_sep + tlen;
        } else {
            j = save;
        }
    }
    while j < len && b[j] == b' ' {
        j += 1;
    }
    if j < len && b[j] == b'\n' {
        while j < len && b[j] == b'\n' {
            j += 1;
        }
    } else if j != len {
        return None;
    }
    let title_owned = title
        .map(|t| t[1..t.len() - 1].to_string())
        .unwrap_or_default();
    let href = href_raw
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(&href_raw);
    Some((
        j,
        replace_multi_space(&label.to_lowercase()),
        strip_any_punctuation(href),
        strip_any_punctuation(&title_owned),
    ))
}

fn definition_title_at(src: &str) -> Option<usize> {
    // rules.ts block.def: single quotes and parentheses intentionally do
    // not share the inline-title escape grammar.
    static TITLE: OnceLock<regex::Regex> = OnceLock::new();
    TITLE
        .get_or_init(|| {
            regex::Regex::new(r#"^(?:"(?:\\"?|[^"\\])*"|'[^'\n]*(?:\n[^'\n]+)*\n?'|\([^()]*\))"#)
                .expect("valid marked definition title regex")
        })
        .find(src)
        .map(|m| m.end())
}

fn title_at(src: &str) -> Option<usize> {
    static TITLE: OnceLock<regex::Regex> = OnceLock::new();
    TITLE
        .get_or_init(|| {
            regex::Regex::new(r#"^(?:"(?:\\"?|[^"\\])*"|'(?:\\'?|[^'\\])*'|\((?:\\\)?|[^)\\])*\))"#)
                .expect("valid marked inline title regex")
        })
        .find(src)
        .map(|m| m.end())
}

// -- gfm table ----------------------------------------------------------------

/// `block.table` (gfm). Returns (end, header_line, align_line, body_text).
fn rx_table(src: &str) -> Option<(usize, String, String, String)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    while i < len && b[i] == b' ' {
        i += 1;
    }
    match b.get(i) {
        None | Some(b'\n') | Some(b' ') => return None,
        _ => {}
    }
    let header_start = i;
    // [^\n ] consumes the first character; the remaining .* uses JS dot.
    i += src[i..].chars().next()?.len_utf8();
    i += src[i..].find(is_js_line_terminator).unwrap_or(len - i);
    if i < len && b[i] != b'\n' {
        return None;
    }
    let header = src[header_start..i].to_string();
    if i >= len {
        return None;
    }
    i += 1;
    let mut sp = 0usize;
    while i + sp < len && sp < 3 && b[i + sp] == b' ' {
        sp += 1;
    }
    let align_start = i + sp;
    let mut j = align_start;
    if b.get(j) == Some(&b'|') {
        j += 1;
        while j < len && b[j] == b' ' {
            j += 1;
        }
    }
    if b.get(j) == Some(&b':') {
        j += 1;
    }
    let ds = j;
    while j < len && b[j] == b'-' {
        j += 1;
    }
    if j == ds {
        return None;
    }
    if b.get(j) == Some(&b':') {
        j += 1;
    }
    while j < len && b[j] == b' ' {
        j += 1;
    }
    loop {
        let save = j;
        if b.get(j) == Some(&b'|') {
            j += 1;
            while j < len && b[j] == b' ' {
                j += 1;
            }
            if b.get(j) == Some(&b':') {
                j += 1;
            }
            let ds2 = j;
            while j < len && b[j] == b'-' {
                j += 1;
            }
            if j == ds2 {
                j = save;
                break;
            }
            if b.get(j) == Some(&b':') {
                j += 1;
            }
            while j < len && b[j] == b' ' {
                j += 1;
            }
        } else {
            break;
        }
    }
    if b.get(j) == Some(&b'|') {
        j += 1;
        while j < len && b[j] == b' ' {
            j += 1;
        }
    }
    let align = src[align_start..j].to_string();
    let body_start = if j < len && b[j] == b'\n' {
        j + 1
    } else if j == len {
        len
    } else {
        return None;
    };
    let mut body = String::new();
    let end = if body_start < len {
        let mut k = body_start;
        loop {
            if k >= len {
                break;
            }
            let line_end = src[k..].find('\n').map(|p| k + p).unwrap_or(len);
            let line = &src[k..line_end];
            if line.chars().all(|c| c == ' ') && line_end < len {
                break; // ' *\n'
            }
            if table_line_interrupted(line) {
                break;
            }
            body.push_str(line);
            body.push('\n');
            if line_end >= len {
                k = len;
                break;
            }
            k = line_end + 1;
        }
        let mut e = k;
        while e < len && b[e] == b'\n' {
            e += 1;
        }
        e
    } else {
        len
    };
    Some((end, header, align, body))
}

/// Table body line interruption: hr | heading | blockquote | code | fences |
/// list | html.
fn table_line_interrupted(line: &str) -> bool {
    if rx_hr(line).is_some() {
        return true;
    }
    let spaces = line.chars().take_while(|&c| c == ' ').count().min(3);
    let rest = &line[spaces..];
    // heading
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].chars().next().is_none_or(is_js_space) {
        return true;
    }
    // blockquote
    if rest.starts_with('>') {
        return true;
    }
    // code: (?: {4}| {0,3}\t)[^\n]
    if line.starts_with("    ") && line.chars().nth(4).is_some() {
        return true;
    }
    if rest.starts_with('\t') && rest.len() > 1 {
        return true;
    }
    // fences: ' {0,3}(?:`{3,}(?=[^`\n]*\n)|~{3,})[^\n]*\n'
    {
        let bb = rest.as_bytes();
        if bb.first() == Some(&b'`') || bb.first() == Some(&b'~') {
            let fc = bb[0];
            let mut k = 0usize;
            while k < bb.len() && bb[k] == fc {
                k += 1;
            }
            let ok = if fc == b'`' {
                if k >= 3 {
                    let mut m = k;
                    while m < bb.len() && bb[m] != b'`' && bb[m] != b'\n' {
                        m += 1;
                    }
                    // (?=[^`\n]*\n) against the line: need a \n after — the
                    // line has none, so the lookahead must look past the line
                    // end which counts as \n in the stream
                    m >= bb.len() || (m < bb.len() && bb[m] == b'\n')
                } else {
                    false
                }
            } else {
                k >= 3
            };
            if ok {
                return true;
            }
        }
    }
    // list: ' {0,3}(?:[*+-]|1[.)])[ \t]'
    {
        let bb = rest.as_bytes();
        let is_bullet = match bb.first() {
            Some(b'*') | Some(b'+') | Some(b'-') => true,
            Some(b'1') => matches!(bb.get(1), Some(b'.') | Some(b')')),
            _ => false,
        };
        if is_bullet {
            let k = if bb[0] == b'1' { 2 } else { 1 };
            if matches!(bb.get(k), Some(b' ') | Some(b'\t')) {
                return true;
            }
        }
    }
    if html_interrupt_at(line) {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Inline rules
// ---------------------------------------------------------------------------

fn rx_escape(src: &str) -> Option<(usize, char)> {
    let mut chars = src.chars();
    if chars.next() != Some('\\') {
        return None;
    }
    let c = chars.next()?;
    if "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".contains(c) {
        Some((2, c))
    } else {
        None
    }
}

/// `inline.tag`. Returns raw length.
fn rx_inline_tag(src: &str) -> Option<usize> {
    if let Some(l) = comment_at(src) {
        if src[..l].ends_with("-->") {
            return Some(l);
        }
    }
    let b = src.as_bytes();
    let len = b.len();
    if b.first() != Some(&b'<') {
        return None;
    }
    if b.get(1) == Some(&b'/') {
        let c = *b.get(2)?;
        if c.is_ascii_alphabetic() {
            let mut i = 3;
            while i < len && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b':' | b'-')) {
                i += 1;
            }
            i = skip_js_space(src, i);
            if b.get(i) == Some(&b'>') {
                return Some(i + 1);
            }
        }
        return None;
    }
    if b.get(1) == Some(&b'?') {
        let mut i = 2;
        while i + 1 < len && !(b[i] == b'?' && b[i + 1] == b'>') {
            i += 1;
        }
        if i + 1 < len {
            return Some(i + 2);
        }
        return None;
    }
    if src[1..].starts_with("![CDATA[") {
        if let Some(p) = src.find("]]>") {
            return Some(p + 3);
        }
    }
    if b.get(1) == Some(&b'!') && b.get(2).is_some_and(|c| c.is_ascii_alphabetic()) {
        let mut i = 3;
        while i < len && b[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i > 2 {
            let ws = i;
            i = skip_js_space(src, i);
            if i > ws {
                while i < len && b[i] != b'>' {
                    i += 1;
                }
                if b.get(i) == Some(&b'>') {
                    return Some(i + 1);
                }
            }
        }
        return None;
    }
    // open tag
    let c = *b.get(1)?;
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let mut i = 2;
    while i < len && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'-')) {
        i += 1;
    }
    loop {
        let at = &src[i..];
        let mut m = 0usize;
        m = skip_js_space(at, m);
        if at[m..].starts_with("/>") {
            return Some(i + m + 2);
        }
        if at[m..].starts_with('>') {
            return Some(i + m + 1);
        }
        match attribute_at(at, false) {
            Some(alen) if alen > 0 => i += alen,
            _ => return None,
        }
    }
}

/// `inline.code`: ^(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)
fn rx_inline_code(src: &str) -> Option<(usize, String)> {
    let b = src.as_bytes();
    if !src.starts_with('`') {
        return None;
    }
    let run = b.iter().take_while(|&&c| c == b'`').count();
    if run == b.len() {
        return None;
    }
    let mut pos = run;
    while pos < b.len() {
        if b[pos] != b'`' {
            pos += 1;
            continue;
        }
        let close_start = pos;
        while pos < b.len() && b[pos] == b'`' {
            pos += 1;
        }
        if pos - close_start == run && close_start > run && b[close_start - 1] != b'`' {
            return Some((pos, src[run..close_start].to_string()));
        }
    }
    None
}

/// `inline.br` (gfm, breaks off): ^( {2,}|\\)\n(?!\s*$)
fn rx_br(src: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut j = 0usize;
    while j < len && b[j] == b' ' {
        j += 1;
    }
    let pos = if j >= 2 {
        j
    } else if b.first() == Some(&b'\\') {
        1
    } else {
        return None;
    };
    if b.get(pos) != Some(&b'\n') {
        return None;
    }
    let mut k = pos + 1;
    k = skip_js_space(src, k);
    if k >= len {
        return None; // \s*$ matched
    }
    Some(pos + 1)
}

/// `inline.autolink`. Returns (len, text, href).
fn rx_autolink(src: &str) -> Option<(usize, String, String)> {
    let s = src.strip_prefix('<')?;
    let gt = s.find('>')?;
    let inner = &s[..gt];
    if inner.contains(|c: char| is_js_space(c) || ('\u{0}'..='\u{1f}').contains(&c)) {
        // scheme branch tolerates only [\s\x00-\x1f<>]; email branch is
        // restricted anyway — check both below per-branch.
    }
    if let Some(colon) = inner.find(':') {
        let scheme = &inner[..colon];
        let valid = (2..=32).contains(&scheme.len())
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .skip(1)
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
        if valid {
            let rest = &inner[colon + 1..];
            if !rest.chars().any(|c| {
                is_js_space(c) || ('\u{0}'..='\u{1f}').contains(&c) || matches!(c, '<' | '>')
            }) {
                let len = gt + 2;
                return Some((len, inner.to_string(), inner.to_string()));
            }
        }
    }
    // email
    if let Some(at) = inner.find('@') {
        let local = &inner[..at];
        let domain = &inner[at + 1..];
        let email_local = |s: &str| {
            !s.is_empty()
                && s.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(
                            c,
                            '.' | '!'
                                | '#'
                                | '$'
                                | '%'
                                | '&'
                                | '\''
                                | '*'
                                | '+'
                                | '/'
                                | '='
                                | '?'
                                | '^'
                                | '_'
                                | '`'
                                | '{'
                                | '|'
                                | '}'
                                | '~'
                                | '-'
                        )
                })
        };
        let label_ok = |s: &str| {
            let parts: Vec<&str> = s.split('.').collect();
            parts.len() >= 2
                && parts.iter().all(|p| {
                    !p.is_empty()
                        && p.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
                        && p.chars().last().is_some_and(|c| c.is_ascii_alphanumeric())
                        && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
        };
        if email_local(local)
            && !domain.is_empty()
            && domain
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            && label_ok(domain)
        {
            let len = gt + 2;
            return Some((len, inner.to_string(), format!("mailto:{inner}")));
        }
    }
    None
}

/// `inline.url` (gfm) + `_backpedal`. Returns (text, href).
fn rx_url(src: &str, tail: Option<u16>) -> Option<(Utf16Text, Utf16Text)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut matched: Option<(usize, bool)> = None;
    let lower_head: String = src.chars().take(8).collect::<String>().to_lowercase();
    let proto = if lower_head.starts_with("https://") {
        8
    } else if lower_head.starts_with("http://") {
        7
    } else if lower_head.starts_with("ftp://") {
        6
    } else {
        0
    };
    if proto > 0 {
        let mut i = proto;
        loop {
            let ds = i;
            while i < len && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
                i += 1;
            }
            if i == ds {
                break;
            }
            if b.get(i) == Some(&b'.') {
                i += 1;
            } else {
                break;
            }
        }
        if i > proto {
            i += src[i..]
                .find(|c| is_js_space(c) || c == '<')
                .unwrap_or(len - i);
            matched = Some((i, false));
        }
    } else if src.starts_with("www.") {
        let mut i = 4;
        loop {
            let ds = i;
            while i < len && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
                i += 1;
            }
            if i == ds {
                break;
            }
            if b.get(i) == Some(&b'.') {
                i += 1;
            } else {
                break;
            }
        }
        if i > 4 {
            i += src[i..]
                .find(|c| is_js_space(c) || c == '<')
                .unwrap_or(len - i);
            matched = Some((i, false));
        }
    }
    if matched.is_none() {
        if let Some(at) = src.find('@') {
            let local = &src[..at];
            if !local.is_empty()
                && local
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
            {
                let mut i = at + 1;
                let ds = i;
                while i < len && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'-' | b'_')) {
                    i += 1;
                }
                if i > ds {
                    let mut dots = 0usize;
                    loop {
                        if b.get(i) == Some(&b'.') {
                            let mut j = i + 1;
                            let gs = j;
                            while j < len
                                && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b'_'))
                            {
                                j += 1;
                            }
                            while j > gs && !b[j - 1].is_ascii_alphanumeric() {
                                j -= 1;
                            }
                            if j > gs {
                                dots += 1;
                                i = j;
                                continue;
                            }
                        }
                        break;
                    }
                    if dots >= 1 && !matches!(b.get(i), Some(b'-') | Some(b'_')) {
                        matched = Some((i, true));
                    }
                }
            }
        }
    }
    let (end, is_email) = matched?;
    let mut text = Utf16Text::from(&src[..end]);
    if is_email {
        let mut href = Utf16Text::from("mailto:");
        href.push(&text);
        return Some((text, href));
    }
    if end == src.len() {
        if let Some(high) = tail {
            text.push(Utf16Text::from_units(vec![high]));
        }
    }
    loop {
        match backpedal_end(text.as_units()) {
            Some(end) if end != text.len() => text.truncate(end),
            _ => break,
        }
    }
    let mut href = Utf16Text::new();
    if text.starts_with("www.") {
        href.push_str("http://");
    }
    href.push(&text);
    Some((text, href))
}

/// Apply the ASCII backpedal rule in JS units, retaining an unpaired tail.
fn backpedal_end(s: &[u16]) -> Option<usize> {
    let trailing = |u| {
        matches!(
            u,
            0x3f | 0x21 | 0x2e | 0x2c | 0x3a | 0x3b | 0x2a | 0x5f | 0x27 | 0x22 | 0x7e | 0x29
        )
    };
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == 0x28 {
            if let Some(close) = s[i..].iter().position(|u| *u == 0x29) {
                i += close + 1;
                continue;
            }
        } else if c == 0x26 {
            let rest = &s[i + 1..];
            if let Some(sc) = rest.iter().position(|u| *u == 0x3b) {
                if sc > 0
                    && sc + 1 == rest.len()
                    && rest[..sc]
                        .iter()
                        .all(|u| u8::try_from(*u).is_ok_and(|u| u.is_ascii_alphanumeric()))
                {
                    return (i > 0).then_some(i);
                }
            }
            i += 1;
            continue;
        } else if trailing(c) {
            let mut j = i;
            while j < s.len() && trailing(s[j]) {
                j += 1;
            }
            if j == s.len() {
                j -= 1;
                if j == i {
                    return (i > 0).then_some(i);
                }
            }
            i = j;
            continue;
        }
        let mut j = i;
        while j < s.len() && !trailing(s[j]) && !matches!(s[j], 0x28 | 0x26) {
            j += 1;
        }
        if j == i {
            return (i > 0).then_some(i);
        }
        i = j;
    }
    (i > 0).then_some(i)
}

/// `inline.punctuation`: ^(?![*_])[\s\p{P}\p{S}]
fn is_punctuation_char(c: char) -> bool {
    c != '*' && c != '_' && (is_js_space(c) || is_punct_or_symbol(c))
}

// -- link / label -------------------------------------------------------------

/// `inline.link`: ^!?\[(label)\]\(\s*(href)(?:sep(title))?\s*\)
fn rx_inline_link(src: &str) -> Option<(usize, String, String, Option<String>)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = 0usize;
    if b.first() == Some(&b'!') {
        i = 1;
    }
    if b.get(i) != Some(&b'[') {
        return None;
    }
    i += 1;
    let (label, mut i) = inline_label_at(src, i, b'(')?;
    if b.get(i) != Some(&b']') {
        return None;
    }
    i += 1;
    if b.get(i) != Some(&b'(') {
        return None;
    }
    i += 1;
    i = skip_js_space(src, i);
    let href_start = i;
    let angled_end = if b.get(i) == Some(&b'<') {
        let mut k = i + 1;
        loop {
            let Some(c) = src[k..].chars().next() else {
                break None;
            };
            if c == '>' {
                break (k > i + 1).then_some(k + 1);
            }
            if c == '\\' {
                k += 1;
                let Some(escaped) = src[k..].chars().next() else {
                    break None;
                };
                if is_js_line_terminator(escaped) {
                    break None;
                }
                k += escaped.len_utf8();
            } else if c == '\n' || c == '<' {
                break None;
            } else {
                k += c.len_utf8();
            }
        }
    } else {
        None
    };
    if let Some(end) = angled_end {
        i = end;
    } else {
        i += src[i..]
            .find(|c| c == ' ' || c <= '\u{1f}')
            .unwrap_or(len - i);
    }
    // Greedy href then minimal backtracking for `(?:sep(title))?` + ws + ')'.
    let greedy_end = i;
    let mut try_i = greedy_end;
    let (href_end, i, title) = loop {
        // optional title separator + title
        let mut title_span: Option<(usize, usize)> = None;
        let mut after_title = try_i;
        let mut sep_opt: Option<usize> = None;
        {
            let mut j = try_i;
            if j < len && (b[j] == b' ' || b[j] == b'\t') {
                while j < len && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                if j < len && b[j] == b'\n' {
                    let mut k = j + 1;
                    while k < len && (b[k] == b' ' || b[k] == b'\t') {
                        k += 1;
                    }
                    sep_opt = Some(k);
                } else {
                    sep_opt = Some(j);
                }
            } else if j < len && b[j] == b'\n' {
                let mut k = j + 1;
                while k < len && (b[k] == b' ' || b[k] == b'\t') {
                    k += 1;
                }
                sep_opt = Some(k);
            }
        }
        if let Some(after) = sep_opt {
            if let Some(tlen) = title_at(&src[after..]) {
                title_span = Some((after, tlen));
                after_title = after + tlen;
            }
        }
        let mut j = after_title;
        j = skip_js_space(src, j);
        if b.get(j) == Some(&b')') {
            let title = title_span.map(|(t0, tlen)| src[t0..t0 + tlen].to_string());
            break (try_i, j + 1, title);
        }
        if try_i == href_start {
            return None;
        }
        try_i = src[..try_i].char_indices().next_back()?.0;
    };
    let href_part = src[href_start..href_end].to_string();
    Some((i, label, href_part, title))
}

/// `_inlineLabel`: lazy units; returns (label, end_pos_at_closing_bracket).
fn inline_label_at(src: &str, start: usize, following: u8) -> Option<(String, usize)> {
    let b = src.as_bytes();
    let mut units: Vec<(usize, usize)> = Vec::new();
    let mut i = start;
    loop {
        if b.get(i) == Some(&b']') && b.get(i + 1) == Some(&following) {
            let label: String = units
                .iter()
                .map(|&(s, e)| &src[s..e])
                .collect::<Vec<&str>>()
                .join("");
            return Some((label, i));
        }
        if i >= b.len() {
            return None;
        }
        if let Some(end) = bracket_span_at(src, i) {
            units.push((i, end));
            i = end;
            continue;
        }
        if b[i] == b'\\' && i + 1 < b.len() {
            let end = i + 1 + src[i + 1..].chars().next()?.len_utf8();
            units.push((i, end));
            i = end;
            continue;
        }
        if b[i] == b'`' {
            if let Some(end) = backtick_span_at(src, i) {
                units.push((i, end));
                i = end;
                continue;
            }
            let mut run = 1usize;
            while b.get(i + run) == Some(&b'`') {
                run += 1;
            }
            if run >= 2 && b.get(i + run) == Some(&b']') {
                units.push((i, i + run));
                i += run;
                continue;
            }
        }
        let c = b[i];
        if matches!(c, b'[' | b']' | b'\\' | b'`') {
            return None;
        }
        let ch_len = src[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        units.push((i, i + ch_len));
        i += ch_len;
    }
}

fn bracket_span_at(src: &str, start: usize) -> Option<usize> {
    let b = src.as_bytes();
    if b.get(start) != Some(&b'[') {
        return None;
    }
    let mut i = start + 1;
    let mut escaped = false;
    while i < b.len() {
        let c = b[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if c == b'[' {
            return None;
        }
        if c == b']' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn backtick_span_at(src: &str, start: usize) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    let mut run = 0usize;
    while start + run < len && b[start + run] == b'`' {
        run += 1;
    }
    if start + run < len && b[start + run] == b'`' {
        return None;
    }
    let mut i = start + run;
    while i < len {
        if b[i] == b'`' {
            let mut r2 = 0usize;
            while i + r2 < len && b[i + r2] == b'`' {
                r2 += 1;
            }
            if i + r2 >= len || b[i + r2] != b'`' {
                return Some(i + r2);
            }
            i += r2;
            continue;
        }
        i += 1;
    }
    None
}

// -- em / strong --------------------------------------------------------------

/// em/strong left delimiter (gfm): returns (len, g1, g2, g3, g4, delim).
#[allow(clippy::type_complexity)]
fn rx_em_strong_ldelim(
    src: &str,
) -> Option<(
    usize,
    Option<char>,
    Option<char>,
    Option<char>,
    Option<char>,
    char,
)> {
    let c0 = src.chars().next()?;
    if c0 == '*' {
        let mut end = c0.len_utf8();
        while src[end..].starts_with('*') {
            end += 1;
        }
        let rest = &src[end..];
        let mut g1 = None;
        let mut g2 = None;
        if let Some(c) = rest.chars().next() {
            if c != '*' && c != '~' && is_punct_or_symbol(c) {
                g1 = Some(c);
            } else if c != '*' && !is_js_space(c) {
                g2 = Some(c);
            }
        }
        let extra = g1.or(g2).map(|c| c.len_utf8()).unwrap_or(0);
        return Some((end + extra, g1, g2, None, None, '*'));
    }
    if c0 == '_' {
        let mut end = c0.len_utf8();
        while src[end..].starts_with('_') {
            end += 1;
        }
        let rest = &src[end..];
        let mut g3 = None;
        let mut g4 = None;
        if let Some(c) = rest.chars().next() {
            if c != '_' && is_punct_or_symbol(c) {
                g3 = Some(c);
            } else if c != '_' && !is_js_space(c) {
                g4 = Some(c);
            }
        }
        let extra = g3.or(g4).map(|c| c.len_utf8()).unwrap_or(0);
        return Some((end + extra, None, None, g3, g4, '_'));
    }
    None
}

/// One right-delimiter alternative match, with JS UTF-16 positions.
struct RDelimHit {
    at: usize,
    group: u8,
    length: usize,
    resume: usize,
}

/// A Unicode-regex atom consumes a pair together but leaves lone surrogates
/// unclassified. Replacing them with U+FFFD would change punctuation tests.
fn inline_point(src: &[u16], at: usize) -> Option<(Option<char>, usize)> {
    match char::decode_utf16(src.get(at..)?.iter().copied()).next()? {
        Ok(c) => Some((Some(c), c.len_utf16())),
        Err(_) => Some((None, 1)),
    }
}

/// Scan the emStrongRDelimAst/Und Unicode alternation in code-unit coordinates.
fn scan_rdelim(src: &[u16], from: usize, delimiter: char) -> Option<RDelimHit> {
    let delim = delimiter as u16;
    let is_punct = |c: Option<char>| {
        c.is_some_and(|c| is_punct_or_symbol(c) && (delimiter != '*' || c != '~'))
    };
    let is_space = |c: Option<char>| c.is_some_and(is_js_space);
    let is_ps = |c| is_space(c) || is_punct(c);
    let mut pos = from;
    while pos < src.len() {
        if pos == 0 {
            if let Some(end) = orphan_delimiter_end(src, delim) {
                return Some(hit(0, 0, 0, end));
            }
        }
        let (c, point_len) = inline_point(src, pos)?;
        if src[pos] == delim {
            pos += point_len;
            continue;
        }
        // Greedy non-delimiter run, retaining its final Unicode-regex atom.
        let mut last = pos;
        let mut end = pos + point_len;
        while end < src.len() && src[end] != delim {
            last = end;
            end += inline_point(src, end)?.1;
        }
        if last > pos {
            return Some(hit(pos, 0, 0, last));
        }
        let run_start = pos + point_len;
        let mut run_end = run_start;
        while src.get(run_end) == Some(&delim) {
            run_end += 1;
        }
        if run_end == run_start {
            pos = run_start;
            continue;
        }
        let after = inline_point(src, run_end).map(|p| p.0);
        let group = if is_punct(c) && after.is_none_or(is_space) {
            1
        } else if !is_ps(c) && after.is_none_or(is_ps) {
            2
        } else if is_ps(c) && after.is_some_and(|a| !is_ps(a)) {
            3
        } else if is_space(c) && after.is_some_and(is_punct) {
            4
        } else if is_punct(c) && after.is_some_and(is_punct) {
            5
        } else if delimiter == '*' && !is_ps(c) && after.is_some_and(|a| !is_ps(a)) {
            6
        } else {
            pos = run_start;
            continue;
        };
        return Some(hit(pos, group, run_end - run_start, run_end));
    }
    None
}

fn orphan_delimiter_end(src: &[u16], delimiter: u16) -> Option<usize> {
    let other = if delimiter == b'*' as u16 {
        b'_' as u16
    } else {
        b'*' as u16
    };
    let mut pos = 0;
    for expected in [other, other, delimiter] {
        if expected == other && pos > 0 && src.get(pos) == Some(&other) {
            pos += 1;
            continue;
        }
        while src
            .get(pos)
            .is_some_and(|c| *c != b'*' as u16 && *c != b'_' as u16)
        {
            pos += 1;
        }
        if src.get(pos) != Some(&expected) {
            return None;
        }
        pos += 1;
    }
    while src
        .get(pos)
        .is_some_and(|c| *c != b'*' as u16 && *c != b'_' as u16)
    {
        pos += 1;
    }
    src[pos..].starts_with(&[other, other]).then_some(pos)
}

fn hit(at: usize, group: u8, length: usize, resume: usize) -> RDelimHit {
    RDelimHit {
        at,
        group,
        length,
        resume,
    }
}

// ---------------------------------------------------------------------------
// Lexer proper
// ---------------------------------------------------------------------------

pub struct Lexer {
    pub links: HashMap<String, (String, String)>,
    in_link: bool,
    in_raw_block: bool,
    /// Upstream `state.linkEmitted`: a link was produced in the inline run
    /// currently being scanned. An image does not count.
    link_emitted: bool,
    top: bool,
}

impl Default for Lexer {
    fn default() -> Self {
        Self::new()
    }
}

impl Lexer {
    pub fn new() -> Self {
        Lexer {
            links: HashMap::new(),
            in_link: false,
            in_raw_block: false,
            link_emitted: false,
            top: true,
        }
    }

    /// Upstream `_Lexer.lex` with the inline queue resolved via DFS pre-order.
    pub fn lex(&mut self, src: &str, ext: &dyn LexerExtensions) -> Vec<Token> {
        let src = src.replace("\r\n", "\n").replace('\r', "\n");
        let mut tokens: Vec<Token> = Vec::new();
        self.block_tokens(&src, &mut tokens, false, ext);
        let mut queue: Vec<(Vec<(usize, Cont)>, String)> = Vec::new();
        collect_inline_jobs(&mut tokens, &mut Vec::new(), &mut queue);
        for (path, inline_src) in &queue {
            let mut out = Vec::new();
            self.inline_tokens(inline_src, &mut out, ext);
            // upstream inlineTokens pushes into the queue entry's array; the
            // task-checkbox pass may have unshifted a checkbox there already,
            // so extend instead of overwriting.
            let node = resolve_path(&mut tokens, path);
            node.tokens.extend(out);
        }
        tokens
    }

    /// Upstream `blockTokens`.
    pub fn block_tokens(
        &mut self,
        src: &str,
        tokens: &mut Vec<Token>,
        mut last_paragraph_clipped: bool,
        ext: &dyn LexerExtensions,
    ) {
        let mut src = src.to_string();
        let mut src_len = usize::MAX;
        while !src.is_empty() {
            if src.len() < src_len {
                src_len = src.len();
            } else {
                break; // infinite-loop guard (upstream throws)
            }

            // extensions (block)
            if let Some(t) = ext.block_tokenizer(self, &src) {
                let cut = t.raw.len();
                src = src[cut.min(src.len())..].to_string();
                tokens.push(t);
                continue;
            }

            // newline
            if let Some(n) = rx_newline(&src) {
                let raw = src[..n].to_string();
                src = src[n..].to_string();
                if raw.chars().count() == 1 {
                    if let Some(last) = tokens.last_mut() {
                        last.raw.push('\n');
                    }
                } else {
                    tokens.push(Token::new("space", raw));
                }
                continue;
            }

            // code (indented)
            if let Some(n) = rx_block_code(&src) {
                let raw = trim_trailing_blank_lines(&src[..n]);
                let text = remove_code_indent(&raw);
                src = src[raw.len()..].to_string();
                let merged = matches!(
                    tokens.last().map(|t| t.kind.as_str()),
                    Some("paragraph") | Some("text")
                );
                if merged {
                    let last = tokens.last_mut().unwrap();
                    if !last.raw.ends_with('\n') {
                        last.raw.push('\n');
                    }
                    last.raw.push_str(&raw);
                    last.text.push('\n');
                    last.text.push_str(&text);
                    last.inline_src = Some(last.text.clone());
                } else {
                    let mut t = Token::new("code", raw.clone());
                    t.text = text;
                    tokens.push(t);
                }
                continue;
            }

            // fences
            if let Some((n, info, body)) = rx_fences(&src) {
                let raw = src[..n].to_string();
                src = src[n..].to_string();
                let text = indent_code_compensation(&raw, &body);
                let lang = if info.trim_matches(is_js_space).is_empty() {
                    None
                } else {
                    Some(strip_any_punctuation(info.trim_matches(is_js_space)))
                };
                let mut t = Token::new("code", raw);
                t.lang = lang;
                t.text = text;
                tokens.push(t);
                continue;
            }

            // heading
            if let Some((n, depth, text0)) = rx_heading(&src) {
                let raw = rtrim(&src[..n], '\n');
                src = src[raw.len()..].to_string();
                let mut text = text0.trim_matches(is_js_space).to_string();
                if text.ends_with('#') {
                    let trimmed = rtrim(&text, '#');
                    if trimmed.is_empty() || trimmed.ends_with(' ') {
                        text = trimmed.trim_matches(is_js_space).to_string();
                    }
                }
                let mut t = Token::new("heading", raw);
                t.depth = depth;
                t.text = text.clone();
                t.inline_src = Some(text);
                tokens.push(t);
                continue;
            }

            // hr
            if let Some(n) = rx_hr(&src) {
                let raw = rtrim(&src[..n], '\n');
                src = src[raw.len()..].to_string();
                tokens.push(Token::new("hr", raw));
                continue;
            }

            // blockquote
            if let Some(n) = rx_blockquote_end(&src, true) {
                // upstream advances by token.raw, not by the regex cap: the
                // trailing newline of the matched region stays in src and is
                // merged back by the single-newline spacer rule.
                if let Some(t) = self.tokenize_blockquote(&src[..n], ext) {
                    let consumed = t.raw.len();
                    src = src[consumed.min(src.len())..].to_string();
                    tokens.push(t);
                } else {
                    src = src[n..].to_string();
                }
                continue;
            }

            // list
            if rx_list_head(&src).is_some() {
                if let Some(t) = self.tokenize_list(&src, ext) {
                    let cut = t.raw.len();
                    src = src[cut.min(src.len())..].to_string();
                    tokens.push(t);
                    continue;
                }
            }

            // html
            if let Some(n) = rx_html_block(&src) {
                let raw = trim_trailing_blank_lines(&src[..n]);
                src = src[raw.len()..].to_string();
                let mut t = Token::new("html", raw.clone());
                t.text = raw;
                tokens.push(t);
                continue;
            }

            // def
            if let Some((n, tag, href, title)) = rx_def(&src) {
                let raw = rtrim(&src[..n], '\n');
                src = src[raw.len()..].to_string();
                let merged = matches!(
                    tokens.last().map(|t| t.kind.as_str()),
                    Some("paragraph") | Some("text")
                );
                if merged {
                    let last = tokens.last_mut().unwrap();
                    if !last.raw.ends_with('\n') {
                        last.raw.push('\n');
                    }
                    last.raw.push_str(&raw);
                    last.text.push('\n');
                    last.text.push_str(&raw);
                    last.inline_src = Some(last.text.clone());
                } else if let std::collections::hash_map::Entry::Vacant(e) =
                    self.links.entry(tag.clone())
                {
                    e.insert((href, title));
                    tokens.push(Token::new("def", raw));
                }
                continue;
            }

            // table (gfm)
            if let Some((n, header_raw, align_raw, body_raw)) = rx_table(&src) {
                if align_raw.contains('|') || align_raw.contains(':') {
                    if let Some(t) =
                        self.tokenize_table(&src[..n], &header_raw, &align_raw, &body_raw)
                    {
                        src = src[n..].to_string();
                        tokens.push(t);
                        continue;
                    }
                }
            }

            // lheading
            if let Some((n, text0, level_char)) = rx_lheading(&src, true) {
                let raw = rtrim(&src[..n], '\n');
                src = src[raw.len()..].to_string();
                let text = text0.trim_matches(is_js_space).to_string();
                let mut t = Token::new("heading", raw);
                t.depth = if level_char == '=' { 1 } else { 2 };
                t.text = text.clone();
                t.inline_src = Some(text);
                tokens.push(t);
                continue;
            }

            // paragraph (top-level, clipped to extension start)
            let first_len = src.chars().next().unwrap().len_utf8();
            let cut_len = match ext.block_start(&src[first_len..]) {
                Some(idx) if first_len + idx < src.len() => first_len + idx,
                _ => src.len(),
            };
            if self.top {
                if let Some(plen) = rx_paragraph(&src[..cut_len], true) {
                    let raw = src[..plen].to_string();
                    let mut text = raw.clone();
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    let clipped = cut_len != src.len();
                    src = src[plen..].to_string();
                    let merged = last_paragraph_clipped
                        && matches!(tokens.last().map(|t| t.kind.as_str()), Some("paragraph"));
                    if merged {
                        let last = tokens.last_mut().unwrap();
                        if !last.raw.ends_with('\n') {
                            last.raw.push('\n');
                        }
                        last.raw.push_str(&raw);
                        last.text.push('\n');
                        last.text.push_str(&text);
                        last.inline_src = Some(last.text.clone());
                    } else {
                        let mut t = Token::new("paragraph", raw);
                        t.text = text.clone();
                        t.inline_src = Some(text);
                        tokens.push(t);
                    }
                    last_paragraph_clipped = clipped;
                    continue;
                }
            }

            // text
            let raw = src.lines().next().unwrap_or("").to_string();
            let consumed = raw.len();
            src = src[consumed.min(src.len())..].to_string();
            let merged = matches!(tokens.last().map(|t| t.kind.as_str()), Some("text"));
            if merged {
                let last = tokens.last_mut().unwrap();
                if !last.raw.ends_with('\n') {
                    last.raw.push('\n');
                }
                last.raw.push_str(&raw);
                last.text.push('\n');
                last.text.push_str(&raw);
                last.inline_src = Some(last.text.clone());
            } else {
                let mut t = Token::new("text", raw.clone());
                t.text = raw.clone();
                t.inline_src = Some(raw);
                tokens.push(t);
            }
        }
        self.top = true;
    }

    fn tokenize_blockquote(&mut self, cap0: &str, ext: &dyn LexerExtensions) -> Option<Token> {
        let mut lines: Vec<String> = rtrim(cap0, '\n').split('\n').map(str::to_string).collect();
        let mut raw = String::new();
        let mut text = String::new();
        let mut tokens: Vec<Token> = Vec::new();
        while !lines.is_empty() {
            let mut in_blockquote = false;
            let mut current_lines: Vec<String> = Vec::new();
            let mut i = 0usize;
            while i < lines.len() {
                let line = &lines[i];
                let spaces = line.chars().take_while(|&c| c == ' ').count();
                if spaces <= 3 && line[spaces.min(line.len())..].starts_with('>') {
                    current_lines.push(line.clone());
                    in_blockquote = true;
                } else if !in_blockquote {
                    current_lines.push(line.clone());
                } else {
                    break;
                }
                i += 1;
            }
            lines = lines[i..].to_vec();
            let current_raw = current_lines.join("\n");
            let current_text = setext_precede(&current_raw);
            if raw.is_empty() {
                raw = current_raw.clone();
            } else {
                raw = format!("{raw}\n{current_raw}");
            }
            if text.is_empty() {
                text = current_text.clone();
            } else {
                text = format!("{text}\n{current_text}");
            }
            let top = self.top;
            self.top = true;
            self.block_tokens(&current_text, &mut tokens, true, ext);
            self.top = top;
            if lines.is_empty() {
                break;
            }
            match tokens.last().map(|t| t.kind.clone()).unwrap_or_default() {
                kind if kind == "code" => break,
                kind if kind == "blockquote" => {
                    let old = tokens.pop().unwrap();
                    // 18.0.11: the continuation lines belong to the same
                    // nesting frame as the nested blockquote, which already
                    // had one '>' marker stripped, so strip one marker from
                    // them too before re-parsing. Otherwise a restated marker
                    // after a lazy line is re-parsed as a spurious deeper
                    // blockquote.
                    let continuation = lines.join("\n");
                    let stripped = strip_blockquote_markers(&continuation);
                    let new_text = format!("{}\n{}", old.raw, stripped);
                    let new_token = self.tokenize_blockquote(&new_text, ext)?;
                    raw = format!("{raw}\n{continuation}");
                    text = format!("{}{}", &text[..text.len() - old.text.len()], new_token.text);
                    tokens.push(new_token);
                    break;
                }
                kind if kind == "list" => {
                    let old = tokens.pop().unwrap();
                    let new_text = format!("{}\n{}", old.raw, lines.join("\n"));
                    let new_token = self.tokenize_list(&new_text, ext)?;
                    raw = format!("{}{}", &raw[..raw.len() - old.raw.len()], new_token.raw);
                    text = format!("{}{}", &text[..text.len() - old.raw.len()], new_token.raw);
                    let consumed = new_token.raw.len();
                    tokens.push(new_token);
                    let rest = &new_text[consumed.min(new_text.len())..];
                    lines = rest.split('\n').map(str::to_string).collect();
                    continue;
                }
                _ => {}
            }
        }
        let mut t = Token::new("blockquote", raw);
        t.tokens = tokens;
        t.text = text;
        Some(t)
    }

    fn tokenize_list(&mut self, src: &str, ext: &dyn LexerExtensions) -> Option<Token> {
        let (_head_len, bullet_head, _rest) = {
            // reuse head detection for the bullet shape
            let b = src.as_bytes();
            let mut i = 0usize;
            while i < b.len() && i < 3 && b[i] == b' ' {
                i += 1;
            }
            let ordered = b.get(i).is_some_and(|c| c.is_ascii_digit());
            let bullet = if ordered {
                let ds = i;
                while i < b.len() && b[i].is_ascii_digit() && i - ds < 9 {
                    i += 1;
                }
                i += 1;
                src[ds..i].to_string()
            } else {
                i += 1;
                src[..i].to_string()
            };
            (0usize, bullet, ordered)
        };
        let trimmed_bullet = bullet_head.trim_matches(is_js_space).to_string();
        let is_ordered = trimmed_bullet.chars().count() > 1;
        let bullet_char = trimmed_bullet.chars().last().unwrap_or('-');
        let start_num: usize = if is_ordered {
            trimmed_bullet[..trimmed_bullet.len() - 1]
                .parse()
                .unwrap_or(1)
        } else {
            0
        };
        let mut list = Token::new("list", "");
        list.ordered = is_ordered;
        list.start = start_num;
        let mut rest_src = src;
        let mut ends_with_blank_line = false;
        let mut raw_all = String::new();
        while !rest_src.is_empty() {
            let (item_len, bull, group2) = match rx_list_item(rest_src, is_ordered, bullet_char) {
                Some(v) => v,
                None => break,
            };
            if rx_hr(rest_src).is_some() {
                break;
            }
            let mut raw = rest_src[..item_len].to_string();
            let mut rest = &rest_src[item_len..];
            let bullet_chars = bull.chars().count();
            let line0 = expand_tabs(group2.split('\n').next().unwrap_or(""), bullet_chars);
            let next_line0 = rest.split('\n').next().unwrap_or("");
            let mut blank_line = line0.trim_matches(is_js_space).is_empty();
            let mut line = line0.clone();
            let indent;
            let mut item_contents;
            if blank_line {
                indent = bullet_chars + 1;
                item_contents = String::new();
            } else {
                let mut ind = line.find(|c: char| c != ' ').unwrap_or(line.len());
                if ind > 4 {
                    ind = 1;
                }
                item_contents = line[ind..].to_string();
                indent = ind + bullet_chars;
            }
            let mut end_early = false;
            if blank_line
                && !next_line0.is_empty()
                && next_line0.chars().all(|c| c == ' ' || c == '\t')
            {
                raw.push_str(next_line0);
                raw.push('\n');
                rest = &rest[next_line0.len() + 1..];
                end_early = true;
            }
            if !end_early {
                loop {
                    if rest.is_empty() {
                        break;
                    }
                    let raw_line = rest.split('\n').next().unwrap_or("");
                    let next_line_wo_tabs = raw_line.replace('\t', "    ");
                    // marked's cachedIndentRegex builds these with
                    // {0, indent-1} (the cache index), not indent.
                    let begin_indent = indent.saturating_sub(1);
                    if fences_begin_at(&next_line_wo_tabs, begin_indent)
                        || heading_begin_at(&next_line_wo_tabs, begin_indent)
                        || html_begin_at(&next_line_wo_tabs, begin_indent)
                        || blockquote_begin_at(&next_line_wo_tabs, begin_indent)
                        || next_bullet_at(&next_line_wo_tabs, begin_indent)
                        || hr_at(&next_line_wo_tabs, begin_indent)
                    {
                        break;
                    }
                    let non_space = next_line_wo_tabs.find(|c: char| c != ' ');
                    if non_space.is_some_and(|ns| ns >= indent)
                        || next_line_wo_tabs.trim_matches(is_js_space).is_empty()
                    {
                        item_contents.push('\n');
                        item_contents
                            .push_str(&next_line_wo_tabs[indent.min(next_line_wo_tabs.len())..]);
                    } else {
                        if blank_line {
                            break;
                        }
                        let line_ns = line.replace('\t', "    ");
                        if line_ns.find(|c: char| c != ' ').is_some_and(|ns| ns >= 4) {
                            break;
                        }
                        if fences_begin_at(&line, indent)
                            || heading_begin_at(&line, indent)
                            || hr_at(&line, indent)
                        {
                            break;
                        }
                        item_contents.push('\n');
                        item_contents.push_str(raw_line);
                    }
                    blank_line = next_line_wo_tabs.trim_matches(is_js_space).is_empty();
                    raw.push_str(raw_line);
                    raw.push('\n');
                    let adv = raw_line.len() + 1;
                    rest = if adv >= rest.len() { "" } else { &rest[adv..] };
                    let ind = indent.min(next_line_wo_tabs.len());
                    line = next_line_wo_tabs[ind..].to_string();
                }
            }
            if !list.loose {
                if ends_with_blank_line {
                    list.loose = true;
                } else if double_blank_line(&raw) {
                    ends_with_blank_line = true;
                }
            }
            let mut item = Token::new("list_item", raw.clone());
            item.task = is_task(&item_contents);
            item.text = item_contents.clone();
            list.items.push(item);
            raw_all.push_str(&raw);
            rest_src = rest;
        }
        let last_item = list.items.last_mut()?;
        last_item.raw = last_item.raw.trim_end().to_string();
        last_item.text = last_item.text.trim_end().to_string();
        list.raw = raw_all.trim_end().to_string();

        // 18.0.11: first pass tokenizes items and finalizes list.loose from
        // spacers before any checkbox is placed; the second pass places task
        // checkboxes using the final list.loose.
        for idx in 0..list.items.len() {
            self.top = false;
            let item_text = list.items[idx].text.clone();
            let mut child_tokens = Vec::new();
            self.block_tokens(&item_text, &mut child_tokens, false, ext);
            list.items[idx].tokens = child_tokens;
            if !list.loose {
                let has_multi = list.items[idx]
                    .tokens
                    .iter()
                    .filter(|t| t.kind == "space")
                    .any(|t| t.raw.matches('\n').count() >= 2);
                list.loose = has_multi;
            }
        }
        for idx in 0..list.items.len() {
            let item_task = list.items[idx].task;
            let first_kind = list.items[idx].tokens.first().map(|t| t.kind.clone());
            if item_task && matches!(first_kind.as_deref(), Some("text") | Some("paragraph")) {
                let item = &mut list.items[idx];
                item.text = strip_task_marker(&item.text);
                if let Some(first) = item.tokens.first_mut() {
                    first.raw = strip_task_marker(&first.raw);
                    first.text = strip_task_marker(&first.text);
                    if let Some(src) = &mut first.inline_src {
                        *src = strip_task_marker(src);
                    }
                }
                if let Some((cb_raw, checked)) = find_checkbox(&item.raw) {
                    let cb_token_raw = format!("{cb_raw} ");
                    item.checked = Some(checked);
                    let cb = || {
                        let mut c = Token::new("checkbox", cb_token_raw.clone());
                        c.checked = Some(checked);
                        c
                    };
                    if list.loose {
                        let first = item.tokens.first_mut().unwrap();
                        if matches!(first.kind.as_str(), "paragraph" | "text")
                            && !first.tokens.is_empty()
                            || matches!(first.kind.as_str(), "paragraph" | "text")
                        {
                            first.raw = format!("{cb_token_raw}{}", first.raw);
                            first.text = format!("{cb_token_raw}{}", first.text);
                            // inline_src stays stripped: the checkbox is
                            // already at tokens[0] and the queued inline run
                            // lexes only the remaining text.
                            first.tokens.insert(0, cb());
                        }
                    } else {
                        item.tokens.insert(0, cb());
                    }
                }
            } else if item_task {
                list.items[idx].task = false;
            }
        }
        if list.loose {
            for item in &mut list.items {
                item.loose = true;
                for token in &mut item.tokens {
                    if token.kind == "text" {
                        token.kind = "paragraph".to_string();
                    }
                }
            }
        }
        Some(list)
    }

    fn tokenize_table(
        &mut self,
        raw: &str,
        header_raw: &str,
        align_raw: &str,
        body_raw: &str,
    ) -> Option<Token> {
        let headers = split_cells(header_raw, None);
        // aligns: strip ^\| and \| *$, then split on |
        let mut a = align_raw;
        if a.starts_with('|') {
            a = &a[1..];
        }
        let trimmed_end = a.trim_end_matches([' ', '\t']);
        let a = trimmed_end.strip_suffix('|').unwrap_or(trimmed_end);
        let aligns: Vec<String> = a.split('|').map(str::to_string).collect();
        if headers.len() != aligns.len() {
            return None;
        }
        let rows: Vec<&str> = if body_raw.trim_matches(is_js_space).is_empty() {
            Vec::new()
        } else {
            body_raw.trim_end_matches(['\n']).split('\n').collect()
        };
        let mut t = Token::new("table", raw.to_string());
        for align in &aligns {
            let cell = align.trim_matches(is_js_space);
            let av = if is_align_right(cell) {
                Some('r')
            } else if is_align_center(cell) {
                Some('c')
            } else if is_align_left(cell) {
                Some('l')
            } else {
                None
            };
            t.align.push(av);
        }
        for h in &headers {
            let mut cell = Token::new("tablecell", h.clone());
            cell.text = h.clone();
            cell.inline_src = Some(h.clone());
            t.header.push(cell);
        }
        for row in rows {
            let cells = split_cells(row, Some(t.header.len()));
            let mut row_cells = Vec::new();
            for cell in cells {
                let mut ct = Token::new("tablecell", cell.clone());
                ct.text = cell.clone();
                ct.inline_src = Some(cell.clone());
                row_cells.push(ct);
            }
            t.rows.push(row_cells);
        }
        Some(t)
    }

    fn tokenize_link(
        &mut self,
        raw: &str,
        label: &str,
        href_part: &str,
        title_raw: Option<String>,
        ext: &dyn LexerExtensions,
    ) -> Option<Token> {
        let is_image = raw.starts_with('!');
        let mut cap0 = raw.to_string();
        let mut raw_utf16 = None;
        let mut href = href_part.trim_matches(is_js_space).to_string();
        if href.starts_with('<') {
            if !href.ends_with('>') {
                return None;
            }
            let without_last = &href[..href.len() - 1];
            let trailing_slashes = without_last
                .chars()
                .rev()
                .take_while(|&c| c == '\\')
                .count();
            if trailing_slashes % 2 == 1 {
                return None;
            }
        } else {
            let last_paren = find_closing_bracket(href_part, '(', ')');
            if last_paren == -2 {
                return None;
            }
            if last_paren > -1 {
                let lp = last_paren as usize;
                let start = if is_image { 5 } else { 4 };
                // marked uses UTF-16 lengths here and intentionally does not
                // add the whitespace preceding the href capture. A valid
                // input may therefore leave a lone low surrogate in source.
                let link_len =
                    start + label.encode_utf16().count() + href_part[..lp].encode_utf16().count();
                let mut units: Vec<_> = raw.encode_utf16().take(link_len).collect();
                while units
                    .last()
                    .is_some_and(|&u| char::from_u32(u as u32).is_some_and(is_js_space))
                {
                    units.pop();
                }
                cap0 = String::from_utf16_lossy(&units);
                raw_utf16 = Some(Utf16Text::from_units(units));
                href = href_part[..lp.min(href_part.len())].to_string();
            }
        }
        let mut title = String::new();
        if let Some(t) = &title_raw {
            title = t[1..t.len() - 1].to_string();
        }
        href = href.trim_matches(is_js_space).to_string();
        if href.starts_with('<') {
            href = href[1..href.len() - 1].to_string();
        }
        href = strip_any_punctuation(&href);
        let _ = title;
        let text = unescape_brackets(label);
        self.in_link = true;
        let outer_link_emitted = self.link_emitted;
        let outer_in_raw_block = self.in_raw_block;
        self.link_emitted = false;
        let mut inner_tokens = Vec::new();
        self.inline_tokens(&text, &mut inner_tokens, ext);
        let text_has_link = self.link_emitted;
        self.link_emitted = outer_link_emitted;
        self.in_link = false;
        if !is_image {
            // CommonMark: "Links may not contain other links, at any level of
            // nesting." Bail so the caller falls through to text and the inner
            // link is the one kept. Images are exempt: their text is flattened
            // into an alt attribute.
            if text_has_link {
                // these tokens are discarded, so undo the raw-block state they
                // opened; leaving it set would suppress escaping for the text
                // that is re-scanned
                self.in_raw_block = outer_in_raw_block;
                return None;
            }
            self.link_emitted = true;
        }
        let mut t = Token::new(if is_image { "image" } else { "link" }, cap0);
        t.raw_utf16 = raw_utf16;
        t.href = href;
        t.text = text;
        t.tokens = inner_tokens;
        Some(t)
    }

    fn tokenize_reflink(&mut self, src: &str, ext: &dyn LexerExtensions) -> Option<Token> {
        let b = src.as_bytes();
        let is_image = b.first() == Some(&b'!');
        let mut i = if is_image { 1 } else { 0 };
        if b.get(i) != Some(&b'[') {
            return None;
        }
        i += 1;
        // reflink: [label][ref]
        if let Some((label, after_label)) = inline_label_at(src, i, b'[') {
            if b.get(after_label) == Some(&b']') && b.get(after_label + 1) == Some(&b'[') {
                if let Some((ref_label, after_ref)) = bracket_label_at(src, after_label + 1) {
                    if b.get(after_ref) == Some(&b']') {
                        let raw = &src[..after_ref + 1];
                        let link_string = replace_multi_space(&ref_label);
                        return self.make_reflink_token(raw, &link_string, is_image, ext, &label);
                    }
                }
            }
        }
        // nolink: [ref](?:\[\])?
        if let Some((ref_label, after_ref)) = bracket_label_at(src, i - 1) {
            if b.get(after_ref) == Some(&b']') {
                let mut end = after_ref + 1;
                if b.get(end) == Some(&b'[') && b.get(end + 1) == Some(&b']') {
                    end += 2;
                }
                let raw = &src[..end];
                let link_string = replace_multi_space(&ref_label);
                return self.make_reflink_token(raw, &link_string, is_image, ext, &ref_label);
            }
        }
        None
    }

    fn make_reflink_token(
        &mut self,
        raw: &str,
        link_string: &str,
        is_image: bool,
        ext: &dyn LexerExtensions,
        label: &str,
    ) -> Option<Token> {
        let lower = link_string.to_lowercase();
        if let Some((href, _title)) = self.links.get(&lower).cloned() {
            let text = unescape_brackets(label);
            self.in_link = true;
            let outer_link_emitted = self.link_emitted;
            let outer_in_raw_block = self.in_raw_block;
            self.link_emitted = false;
            let mut inner_tokens = Vec::new();
            self.inline_tokens(&text, &mut inner_tokens, ext);
            let text_has_link = self.link_emitted;
            self.link_emitted = outer_link_emitted;
            self.in_link = false;
            if !is_image {
                if text_has_link {
                    self.in_raw_block = outer_in_raw_block;
                    return None;
                }
                self.link_emitted = true;
            }
            let mut t = Token::new(if is_image { "image" } else { "link" }, raw.to_string());
            t.href = href;
            t.text = text;
            t.tokens = inner_tokens;
            Some(t)
        } else {
            let text = raw.chars().next()?.to_string();
            let mut t = Token::new("text", text.clone());
            t.text = text;
            Some(t)
        }
    }

    /// Upstream `inlineTokens`.
    pub fn inline_tokens(&mut self, src: &str, tokens: &mut Vec<Token>, ext: &dyn LexerExtensions) {
        self.inline_tokens_with_tail(src, None, tokens, ext);
    }

    fn inline_tokens_with_tail(
        &mut self,
        src: &str,
        mut tail: Option<u16>,
        tokens: &mut Vec<Token>,
        ext: &dyn LexerExtensions,
    ) {
        // 18.0.11 masks reflinks with `String.replace` and a recursive
        // callback: every match is found on the unmodified source, and a
        // candidate whose text already holds a link keeps that text's emphasis
        // visible by masking only the links it holds instead of the whole
        // span.
        let mut masked_src = String::new();
        if !self.links.is_empty() && src.contains('[') {
            let mut reflink_matches: Vec<(usize, usize)> = Vec::new();
            let mut cursor = 0;
            while let Some((idx, mlen, _label)) = find_reflink_search(&src[cursor..]) {
                let start = cursor + idx;
                cursor = start + mlen;
                reflink_matches.push((start, cursor));
            }
            let mut last = 0;
            for (start, end) in reflink_matches {
                masked_src.push_str(&src[last..start]);
                masked_src.push_str(&self.mask_reflink_match(&src[start..end]));
                last = end;
            }
            masked_src.push_str(&src[last..]);
        } else {
            masked_src.push_str(src);
        }
        // Mask out escaped characters with a length-preserving replacement:
        // emStrong and del align maskedSrc with src by slicing from the end,
        // and `anyPunctuation` matches Unicode punctuation, so an escaped
        // astral character is 3 code units.
        {
            let mut punctuation_masked = String::new();
            let mut last = 0;
            let mut cursor_units = 0;
            while let Some((idx, mlen)) = find_any_punctuation(&masked_src, cursor_units) {
                let units = masked_src[idx..idx + mlen].encode_utf16().count();
                punctuation_masked.push_str(&masked_src[last..idx]);
                punctuation_masked.push_str(&"+".repeat(units));
                cursor_units = masked_src[..idx].encode_utf16().count() + units;
                last = idx + mlen;
            }
            punctuation_masked.push_str(&masked_src[last..]);
            masked_src = punctuation_masked;
        }
        let mut block_cursor = 0;
        while let Some((idx, mlen, group2_len)) = find_block_skip(&masked_src, block_cursor) {
            block_cursor = idx + mlen;
            let start = idx + group2_len;
            let units = masked_src[start..block_cursor].encode_utf16().count();
            let replacement = format!("[{}]", "a".repeat(units.saturating_sub(2)));
            masked_src.replace_range(start..block_cursor, &replacement);
            block_cursor = start + replacement.len();
        }

        let mut masked_units: Vec<_> = masked_src.encode_utf16().collect();
        masked_units.extend(tail);
        let mut src = src.to_string();
        let mut prev_char: Option<u16> = None;
        let mut keep_prev_char = false;
        let mut src_len = usize::MAX;
        while !src.is_empty() || tail.is_some() {
            let remaining = src.len() + usize::from(tail.is_some());
            if remaining < src_len {
                src_len = remaining;
            } else {
                break;
            }
            if !keep_prev_char {
                prev_char = None;
            }
            keep_prev_char = false;

            if src.is_empty() {
                let units = Utf16Text::from_units(vec![tail.take().unwrap()]);
                let mut t = Token::new("text", "");
                t.set_raw_units(units.clone());
                t.set_text_units(units);
                if let Some(last) = tokens.last_mut().filter(|t| t.kind == "text") {
                    last.append_text(&t);
                } else {
                    tokens.push(t);
                }
                continue;
            }

            // extensions (inline)
            if let Some(t) = ext.inline_tokenizer_with_tail(self, &src, tail) {
                let low = consume_inline_prefix(&mut src, &mut tail, t.raw_units().len(), ext);
                tokens.push(t);
                if let Some(text) = low {
                    prev_char = text.text_units().as_units().last().copied();
                    keep_prev_char = true;
                    tokens.push(text);
                }
                continue;
            }

            // escape
            if let Some((len, text)) = rx_escape(&src) {
                let raw = src[..len].to_string();
                src = src[len..].to_string();
                let mut t = Token::new("escape", raw);
                t.text = text.to_string();
                tokens.push(t);
                continue;
            }

            // tag
            if let Some(len) = rx_inline_tag(&src) {
                let raw = src[..len].to_string();
                src = src[len..].to_string();
                let lower = raw.to_lowercase();
                if !self.in_link && lower.starts_with("<a ") {
                    self.in_link = true;
                } else if self.in_link && lower.starts_with("</a>") {
                    self.in_link = false;
                }
                if !self.in_raw_block
                    && ["<pre", "<code", "<kbd", "<script"]
                        .iter()
                        .any(|t| lower.starts_with(t) && lower[t.len()..].starts_with([' ', '>']))
                {
                    self.in_raw_block = true;
                }
                let mut t = Token::new("html", raw.clone());
                t.text = raw;
                tokens.push(t);
                continue;
            }

            // link
            if let Some((len, label, href_part, title_raw)) = rx_inline_link(&src) {
                if let Some(t) = self.tokenize_link(&src[..len], &label, &href_part, title_raw, ext)
                {
                    let low = consume_inline_prefix(&mut src, &mut tail, t.raw_units().len(), ext);
                    tokens.push(t);
                    if let Some(text) = low {
                        prev_char = text.text_units().as_units().last().copied();
                        keep_prev_char = true;
                        tokens.push(text);
                    }
                    continue;
                }
            }

            // reflink, nolink
            if let Some(t) = self.tokenize_reflink(&src, ext) {
                let is_text = t.kind == "text";
                let cut = t.raw.len();
                src = src[cut.min(src.len())..].to_string();
                if is_text && matches!(tokens.last().map(|x| x.kind.as_str()), Some("text")) {
                    let last = tokens.last_mut().unwrap();
                    last.append_text(&t);
                } else {
                    tokens.push(t);
                }
                continue;
            }

            // em & strong
            if let Some(t) = self.tokenize_em_strong(&src, tail, &masked_units, prev_char, ext) {
                let low = consume_inline_prefix(&mut src, &mut tail, t.raw_units().len(), ext);
                tokens.push(t);
                if let Some(text) = low {
                    prev_char = text.text_units().as_units().last().copied();
                    keep_prev_char = true;
                    tokens.push(text);
                }
                continue;
            }

            // code
            if let Some((len, content)) = rx_inline_code(&src) {
                let raw = src[..len].to_string();
                src = src[len..].to_string();
                let mut text = content.replace('\n', " ");
                if text.chars().any(|c| c != ' ')
                    && text.starts_with(' ')
                    && text.ends_with(' ')
                    && text.len() >= 2
                {
                    text = text[1..text.len() - 1].to_string();
                }
                let mut t = Token::new("codespan", raw);
                t.text = text;
                tokens.push(t);
                continue;
            }

            // br
            if let Some(len) = rx_br(&src) {
                let raw = src[..len].to_string();
                src = src[len..].to_string();
                tokens.push(Token::new("br", raw));
                continue;
            }

            // del — markdown.ts replaces the tokenizer with the strict
            // regex-only variant, so there is no base fallback.
            if let Some(t) = ext.del(self, &src) {
                src = src[t.raw.len().min(src.len())..].to_string();
                tokens.push(t);
                continue;
            }

            // autolink
            if let Some((len, text, href)) = rx_autolink(&src) {
                let raw = src[..len].to_string();
                src = src[len..].to_string();
                let mut t = Token::new("link", raw);
                t.text = text.clone();
                t.href = href;
                let mut inner = Token::new("text", text.clone());
                inner.text = text;
                t.tokens = vec![inner];
                tokens.push(t);
                continue;
            }

            // url (gfm)
            if !self.in_link {
                if let Some((text, href)) = rx_url(&src, tail) {
                    let low = consume_inline_prefix(&mut src, &mut tail, text.len(), ext);
                    let mut t = Token::new("link", "");
                    t.set_raw_units(text.clone());
                    t.set_text_units(text.clone());
                    t.href = href.to_string_lossy();
                    t.href_utf16 = href.to_string_checked().is_err().then_some(href);
                    let mut inner = Token::new("text", "");
                    inner.set_raw_units(text.clone());
                    inner.set_text_units(text);
                    t.tokens = vec![inner];
                    tokens.push(t);
                    if let Some(text) = low {
                        prev_char = text.text_units().as_units().last().copied();
                        keep_prev_char = true;
                        tokens.push(text);
                    }
                    continue;
                }
            }

            // text (clipped to extension start)
            let first_len = src.chars().next().unwrap().len_utf8();
            let cut_len = match ext.inline_start(&src[first_len..]) {
                Some(idx) if first_len + idx < src.len() => first_len + idx,
                _ => src.len(),
            };
            if let Some(len) = rx_inline_text(&src[..cut_len]) {
                let raw = src[..len].to_string();
                src = src[len..].to_string();
                let mut text = Utf16Text::from(raw);
                if src.is_empty() {
                    if let Some(high) = tail.take() {
                        text.push(Utf16Text::from_units(vec![high]));
                    }
                }
                if !text.ends_with("_") {
                    prev_char = text.as_units().last().copied();
                }
                keep_prev_char = true;
                let mut t = Token::new("text", "");
                t.set_raw_units(text.clone());
                t.set_text_units(text);
                if matches!(tokens.last().map(|x| x.kind.as_str()), Some("text")) {
                    tokens.last_mut().unwrap().append_text(&t);
                } else {
                    tokens.push(t);
                }
                continue;
            }

            if !src.is_empty() {
                break;
            }
        }
    }

    /// Upstream `Lexer.maskReflink` (18.0.11): mask one reflinkSearch match.
    /// A match is `[`label`][`ref`]` or `[`ref`]`, so the label text is
    /// delimited by single-unit brackets and slicing at unit offsets 1 and
    /// refStart-1 can never divide a surrogate pair.
    fn mask_reflink_match(&self, matched: &str) -> String {
        let units: Vec<u16> = matched.encode_utf16().collect();
        // Lexer masking uses the literal last bracket label, unlike tokenizer
        // lookup, which normalizes whitespace and case.
        let ref_start = units
            .iter()
            .rposition(|&unit| unit == b'[' as u16)
            .expect("reflinkSearch match contains a bracket");
        let label = String::from_utf16(&units[ref_start + 1..units.len() - 1])
            .expect("reflink label boundaries are single-unit brackets");
        if !self.links.contains_key(&label) {
            return matched.to_string();
        }
        // Images are exempt: their text is flattened into an alt attribute.
        if ref_start > 1 && !matched.starts_with('!') {
            let text = String::from_utf16(&units[1..ref_start - 1])
                .expect("reflink label boundaries are single-unit brackets");
            if self.link_in_text(&text) {
                let inner = self.mask_reflink_search_in(&text);
                return format!("[{inner}][{}]", "a".repeat(units.len() - ref_start - 2));
            }
        }
        format!("[{}]", "a".repeat(units.len().saturating_sub(2)))
    }

    /// `text.replace(reflinkSearch, maskReflink)`.
    fn mask_reflink_search_in(&self, text: &str) -> String {
        let mut out = String::new();
        let mut last = 0;
        let mut cursor = 0;
        while let Some((idx, mlen, _label)) = find_reflink_search(&text[cursor..]) {
            let start = cursor + idx;
            cursor = start + mlen;
            out.push_str(&text[last..start]);
            out.push_str(&self.mask_reflink_match(&text[start..cursor]));
            last = cursor;
        }
        out.push_str(&text[last..]);
        out
    }

    /// Upstream `Lexer.linkInText` (18.0.11): does this link text hold a link
    /// already? An image does not count: an image may hold a link, a link may
    /// not.
    fn link_in_text(&self, text: &str) -> bool {
        if !text.contains('[') {
            return false;
        }
        let mut cursor = 0;
        while let Some((idx, mlen, _group2)) = find_block_skip(text, cursor) {
            let start = idx;
            cursor = start + mlen;
            // blockSkip also matches code spans and html, and the `!` of an
            // image is left out of the match, so read the character before it.
            let matched = &text[start..cursor];
            if rx_inline_link(matched).is_some() && !text[..start].ends_with('!') {
                return true;
            }
        }
        let mut cursor = 0;
        while let Some((idx, mlen, _label)) = find_reflink_search(&text[cursor..]) {
            let start = cursor + idx;
            cursor = start + mlen;
            let matched = &text[start..cursor];
            let units: Vec<u16> = matched.encode_utf16().collect();
            let Some(ref_start) = units.iter().rposition(|&unit| unit == b'[' as u16) else {
                continue;
            };
            if matched.starts_with('!') {
                continue;
            }
            let Ok(label) = String::from_utf16(&units[ref_start + 1..units.len() - 1]) else {
                continue;
            };
            if !self.links.contains_key(&label) {
                continue;
            }
            // a candidate holding a link is not a link either, so it does not
            // count
            if ref_start > 1 {
                let Ok(inner) = String::from_utf16(&units[1..ref_start - 1]) else {
                    continue;
                };
                if self.link_in_text(&inner) {
                    continue;
                }
            }
            return true;
        }
        false
    }

    /// Upstream `emStrong` (gfm rules).
    fn tokenize_em_strong(
        &mut self,
        src: &str,
        source_tail: Option<u16>,
        masked_units: &[u16],
        prev_char: Option<u16>,
        ext: &dyn LexerExtensions,
    ) -> Option<Token> {
        let (ldelim_len, g1, g2, g3, g4, delim_char) = rx_em_strong_ldelim(src)?;
        if g1.is_none() && g2.is_none() && g3.is_none() && g4.is_none() {
            return None;
        }
        let prev_scalar = prev_char.and_then(|u| char::from_u32(u as u32));
        if g4.is_some() && prev_scalar.is_some_and(crate::tui::utils::is_letter_or_number_unicode) {
            return None;
        }
        let next_char = g1.or(g3);
        let prev_ok = prev_scalar.is_some_and(is_punctuation_char);
        if next_char.is_none() || prev_char.is_none() || prev_ok {
            let l_length = src[..ldelim_len].chars().count() - 1;
            // A mid-run opener (for example the second star of an unmatched
            // `**`) must only pair with a delimiter that can only close,
            // otherwise it steals the opener of a later span (`**a*b*c` must
            // be `**a<em>b</em>c`).
            let mid_run = prev_char == Some(delim_char as u16);
            let mut delim_total = l_length as i64;
            let mut mid_delim_total = 0i64;
            let mut source_units: Vec<_> = src.encode_utf16().collect();
            source_units.extend(source_tail);
            let clip = (masked_units.len() as i64 - source_units.len() as i64 + l_length as i64)
                .max(0) as usize;
            let clipped = &masked_units[clip..];
            let mut scan_pos = 0usize;
            loop {
                let m = scan_rdelim(clipped, scan_pos, delim_char)?;
                if m.group == 0 {
                    scan_pos = m.resume.max(scan_pos + 1);
                    continue;
                }
                let r_length = m.length as i64;
                if m.group == 3 || m.group == 4 {
                    delim_total += r_length;
                    scan_pos = m.resume;
                    continue;
                } else if matches!(m.group, 5 | 6) {
                    if !l_length.is_multiple_of(3) && (l_length as i64 + r_length) % 3 == 0 {
                        mid_delim_total += r_length;
                        scan_pos = m.resume;
                        continue; // CommonMark Emphasis Rules 9-10
                    }
                    if mid_run {
                        // A mid-run opener cannot close against an ambiguous
                        // delimiter that can also open; that delimiter opens
                        // its own emphasis span instead.
                        break;
                    }
                }
                delim_total -= r_length;
                if delim_total > 0 {
                    scan_pos = m.resume;
                    continue;
                }
                let r_length = (r_length + delim_total + mid_delim_total).min(r_length);
                let last_char_len = inline_point(clipped, m.at)?.1;
                let raw_end =
                    (l_length + m.at + last_char_len + r_length as usize).min(source_units.len());
                let raw = Utf16Text::from_units(source_units[..raw_end].to_vec());
                let delimiter_units = if (l_length as i64).min(r_length) % 2 == 1 {
                    1
                } else {
                    2
                };
                let text_start = delimiter_units.min(raw.len());
                let text_end = raw.len().saturating_sub(delimiter_units).max(text_start);
                let text = raw.slice(text_start..text_end);
                let mut t = Token::new(if delimiter_units == 1 { "em" } else { "strong" }, "");
                t.set_raw_units(raw);
                t.set_text_units(text.clone());
                let (text, tail) = split_inline_tail(text);
                self.inline_tokens_with_tail(&text, tail, &mut t.tokens, ext);
                return Some(t);
            }
        }
        None
    }
}

/// Which container a path segment indexes into.
#[derive(Clone, Debug)]
enum Cont {
    Tokens,
    Items,
    Header,
    Row(usize),
}

fn collect_inline_jobs(
    tokens: &mut [Token],
    path: &mut Vec<(usize, Cont)>,
    queue: &mut Vec<(Vec<(usize, Cont)>, String)>,
) {
    for (i, token) in tokens.iter_mut().enumerate() {
        path.push((i, Cont::Tokens));
        if let Some(src) = token.inline_src.clone() {
            queue.push((path.clone(), src));
        }
        collect_children_of(token, path, queue);
        path.pop();
    }
}

fn collect_children_of(
    t: &mut Token,
    path: &mut Vec<(usize, Cont)>,
    queue: &mut Vec<(Vec<(usize, Cont)>, String)>,
) {
    collect_inline_jobs(&mut t.tokens, path, queue);
    for j in 0..t.items.len() {
        path.push((j, Cont::Items));
        if let Some(src) = t.items[j].inline_src.clone() {
            queue.push((path.clone(), src));
        }
        collect_inline_jobs(&mut t.items[j].tokens, path, queue);
        path.pop();
    }
    for j in 0..t.header.len() {
        path.push((j, Cont::Header));
        if let Some(src) = t.header[j].inline_src.clone() {
            queue.push((path.clone(), src));
        }
        collect_inline_jobs(&mut t.header[j].tokens, path, queue);
        path.pop();
    }
    for r in 0..t.rows.len() {
        for j in 0..t.rows[r].len() {
            path.push((j, Cont::Row(r)));
            if let Some(src) = t.rows[r][j].inline_src.clone() {
                queue.push((path.clone(), src));
            }
            collect_inline_jobs(&mut t.rows[r][j].tokens, path, queue);
            path.pop();
        }
    }
}

fn resolve_path<'a>(tokens: &'a mut [Token], path: &[(usize, Cont)]) -> &'a mut Token {
    let (i0, _) = path[0];
    let mut node: &mut Token = &mut tokens[i0];
    for &(idx, ref cont) in &path[1..] {
        node = match cont {
            Cont::Tokens => &mut node.tokens[idx],
            Cont::Items => &mut node.items[idx],
            Cont::Header => &mut node.header[idx],
            Cont::Row(r) => &mut node.rows[*r][idx],
        };
    }
    node
}

// ---------------------------------------------------------------------------
// Post-processing helpers
// ---------------------------------------------------------------------------

fn setext_precede(current_raw: &str) -> String {
    // .replace(/\n {0,3}((?:=+|-+) *)(?=\n|$)/g, '\n    $1')
    let bytes = current_raw.as_bytes();
    let mut out = String::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let mut j = i + 1;
            let mut sp = 0;
            while j < bytes.len() && sp < 3 && bytes[j] == b' ' {
                j += 1;
                sp += 1;
            }
            let mut k = j;
            while k < bytes.len() && (bytes[k] == b'=' || bytes[k] == b'-') {
                k += 1;
            }
            if k > j {
                let mut m = k;
                while m < bytes.len() && bytes[m] == b' ' {
                    m += 1;
                }
                if m >= bytes.len() || bytes[m] == b'\n' {
                    out.push('\n');
                    out.push_str("    ");
                    out.push_str(&current_raw[j..m]);
                    i = m;
                    continue;
                }
            }
        }
        let ch_len = current_raw[i..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
        out.push_str(&current_raw[i..i + ch_len]);
        i += ch_len;
    }
    // .replace(/^ {0,3}>[ \t]?/gm, '')
    strip_blockquote_markers(&out)
}

/// `blockquoteSetextReplace2`: `/^ {0,3}>[ \t]?/gm`.
fn strip_blockquote_markers(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let spaces = line.chars().take_while(|&c| c == ' ').count();
            if spaces <= 3 && line[spaces..].starts_with('>') {
                let after = &line[spaces + 1..];
                if after.starts_with('\t') || after.starts_with(' ') {
                    after[1..].to_string()
                } else {
                    after.to_string()
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn remove_code_indent(raw: &str) -> String {
    raw.split('\n')
        .map(|line| {
            if let Some(stripped) = line.strip_prefix("    ") {
                stripped.to_string()
            } else {
                let spaces = line.chars().take_while(|&c| c == ' ').count().min(3);
                if line[spaces..].starts_with('\t') {
                    line[spaces + 1..].to_string()
                } else {
                    line.to_string()
                }
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn indent_code_compensation(raw: &str, text: &str) -> String {
    let indent_to_code: Option<usize> = {
        let b = raw.as_bytes();
        let mut i = 0usize;
        while i < b.len() && is_js_space(b[i] as char) {
            i += 1;
        }
        if i > 0 && raw[i..].starts_with("```") {
            Some(i)
        } else {
            None
        }
    };
    match indent_to_code {
        None => text.to_string(),
        Some(indent_len) => text
            .split('\n')
            .map(|node| {
                let node_spaces = node.chars().take_while(|&c| c == ' ' || c == '\t').count();
                if node_spaces == 0 {
                    return node.to_string();
                }
                if node_spaces >= indent_len {
                    node.chars().skip(indent_len).collect()
                } else {
                    node.to_string()
                }
            })
            .collect::<Vec<String>>()
            .join("\n"),
    }
}

fn strip_any_punctuation(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                if is_punct_or_symbol(n) {
                    out.push(n);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn unescape_brackets(label: &str) -> String {
    let mut out = String::new();
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                if n == '[' || n == ']' {
                    out.push(n);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn replace_multi_space(s: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for c in s.chars() {
        if is_js_space(c) {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

fn is_task(contents: &str) -> bool {
    let b = contents.as_bytes();
    if b.len() < 5 || b[0] != b'[' || !matches!(b[1], b' ' | b'x' | b'X') || b[2] != b']' {
        return false;
    }
    let mut i = 3;
    let mut spaces = 0;
    while i < b.len() && b[i] == b' ' {
        i += 1;
        spaces += 1;
    }
    spaces >= 1 && i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'\n'
}

fn strip_task_marker(contents: &str) -> String {
    let b = contents.as_bytes();
    if b.len() >= 4 && b[0] == b'[' && matches!(b[1], b' ' | b'x' | b'X') && b[2] == b']' {
        let mut i = 3;
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i > 3 {
            return contents[i..].to_string();
        }
    }
    contents.to_string()
}

fn find_checkbox(raw: &str) -> Option<(String, bool)> {
    let b = raw.as_bytes();
    let mut i = 0usize;
    while i + 2 < b.len() + 1 && i + 2 <= b.len() {
        if i + 2 < b.len()
            && b[i] == b'['
            && matches!(b[i + 1], b' ' | b'x' | b'X')
            && b[i + 2] == b']'
        {
            let s = raw[i..i + 3].to_string();
            let checked = s != "[ ]";
            return Some((s, checked));
        }
        i += 1;
    }
    None
}

fn double_blank_line(raw: &str) -> bool {
    // /\n[ \t]*\n[ \t]*$/
    let t = raw.trim_end_matches([' ', '\t']);
    if !t.ends_with('\n') {
        return false;
    }
    let t2 = &t[..t.len() - 1]; // drop final \n
    let t2 = t2.trim_end_matches([' ', '\t']);
    t2.ends_with('\n')
}

fn fences_begin_at(line: &str, indent: usize) -> bool {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    spaces <= indent && (line[spaces..].starts_with("```") || line[spaces..].starts_with("~~~"))
}

fn heading_begin_at(line: &str, indent: usize) -> bool {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    spaces <= indent && line[spaces..].starts_with('#')
}

fn html_begin_at(line: &str, indent: usize) -> bool {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    if spaces > indent {
        return false;
    }
    let rest = &line[spaces..];
    let lower = rest.to_lowercase();
    if lower.starts_with("<!--") {
        return true;
    }
    if lower.starts_with('<') {
        let c = lower.as_bytes().get(1).copied();
        if c.is_some_and(|c| c.is_ascii_alphabetic()) && rest[1..].contains('>') {
            return true;
        }
    }
    false
}

fn blockquote_begin_at(line: &str, indent: usize) -> bool {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    spaces <= indent && line[spaces..].starts_with('>')
}

fn next_bullet_at(line: &str, indent: usize) -> bool {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    if spaces > indent {
        return false;
    }
    let rest = &line[spaces..];
    let b = rest.as_bytes();
    match b.first() {
        Some(b'*') | Some(b'+') | Some(b'-') => true,
        Some(c) if c.is_ascii_digit() => {
            let mut i = 1;
            while i < b.len() && b[i].is_ascii_digit() && i < 9 {
                i += 1;
            }
            matches!(b.get(i), Some(b'.') | Some(b')'))
        }
        _ => false,
    }
}

fn hr_at(line: &str, indent: usize) -> bool {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    if spaces > indent {
        return false;
    }
    let rest = &line[spaces..];
    let b = rest.as_bytes();
    let hc = match b.first() {
        Some(b'-') => b'-',
        Some(b'_') => b'_',
        Some(b'*') => b'*',
        _ => return false,
    };
    let mut count = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == hc {
            count += 1;
            i += 1;
            while i < b.len() && b[i] == b' ' {
                i += 1;
            }
        } else {
            break;
        }
    }
    count >= 3 && i >= b.len()
}

fn is_align_right(s: &str) -> bool {
    // ^ *-+: *$
    let t = s.trim_matches(' ');
    t.len() >= 2 && t.ends_with(':') && t[..t.len() - 1].chars().all(|c| c == '-')
}

fn is_align_center(s: &str) -> bool {
    // ^ *:-+: *$
    let t = s.trim_matches(' ');
    t.len() >= 3
        && t.starts_with(':')
        && t.ends_with(':')
        && t[1..t.len() - 1].chars().all(|c| c == '-')
}

fn is_align_left(s: &str) -> bool {
    // ^ *:-+ *$
    let t = s.trim_matches(' ');
    t.len() >= 2 && t.starts_with(':') && t[1..].chars().all(|c| c == '-')
}

/// `inline.text` (gfm). Returns raw length.
fn rx_inline_text(src: &str) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    if len == 0 {
        return None;
    }
    let first = b[0];
    if first == b'`' || first == b'~' {
        let mut i = 0usize;
        while i < len && (b[i] == b'`' || b[i] == b'~') {
            i += 1;
        }
        return Some(i);
    }
    let fl = src.chars().next()?;
    Some(inline_text_continuation(src, fl.len_utf8()))
}

/// Continue a text token after its first unit/scalar; zero is used only when
/// that first unit is a separately retained low surrogate.
fn inline_text_continuation(src: &str, mut i: usize) -> usize {
    let len = src.len();
    if lookahead_two_spaces_newline(src, i) || lookahead_email(src, i) {
        return i;
    }
    while i < len {
        let c = src[i..].chars().next().unwrap();
        if matches!(c, '\\' | '<' | '!' | '[' | '`' | '*' | '~' | '_') {
            return i;
        }
        if is_protocol_at(src, i) || src[i..].starts_with("www.") {
            return i;
        }
        if c != ' ' && lookahead_two_spaces_newline(src, i + c.len_utf8()) {
            return i + c.len_utf8();
        }
        if !is_email_char(c) && lookahead_email(src, i + c.len_utf8()) {
            return i + c.len_utf8();
        }
        i += c.len_utf8();
    }
    len
}

fn lookahead_two_spaces_newline(src: &str, pos: usize) -> bool {
    let b = src.as_bytes();
    let mut i = pos;
    let mut spaces = 0;
    while i < b.len() && b[i] == b' ' {
        i += 1;
        spaces += 1;
    }
    spaces >= 2 && i < b.len() && b[i] == b'\n'
}

fn lookahead_email(src: &str, pos: usize) -> bool {
    let b = src.as_bytes();
    let mut i = pos;
    while i < b.len() && is_email_char(b[i] as char) {
        i += 1;
    }
    i > pos && b.get(i) == Some(&b'@')
}

// ---------------------------------------------------------------------------
// Masking helpers (inlineTokens preprocessing)
// ---------------------------------------------------------------------------

/// reflinkSearch: reflink | nolink(?!\() — leftmost match.
fn find_reflink_search(src: &str) -> Option<(usize, usize, String)> {
    let b = src.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'[' || (b[i] == b'!' && b.get(i + 1) == Some(&b'[')) {
            let off = if b[i] == b'!' { 2 } else { 1 };
            // reflink [label][ref]
            if let Some((_label, after)) = inline_label_at(src, i + off, b'[') {
                if b.get(after) == Some(&b']') && b.get(after + 1) == Some(&b'[') {
                    if let Some((ref2, after2)) = bracket_label_at(src, after + 1) {
                        if b.get(after2) == Some(&b']') {
                            return Some((i, after2 + 1 - i, ref2));
                        }
                    }
                }
                // nolink [ref](?!()
                if let Some((ref_label, after_n)) = bracket_label_at(src, i + off - 1) {
                    if b.get(after_n) == Some(&b']') && b.get(after_n + 1) != Some(&b'(') {
                        let mut end = after_n + 1;
                        if b.get(end) == Some(&b'[') && b.get(end + 1) == Some(&b']') {
                            end += 2;
                        }
                        return Some((i, end - i, ref_label));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// anyPunctuation: \\punct — first match (byte idx, len).
fn find_any_punctuation(src: &str, from_units: usize) -> Option<(usize, usize)> {
    let mut units = 0;
    for (i, c) in src.char_indices() {
        let at_or_after_cursor = units >= from_units;
        units += c.len_utf16();
        if at_or_after_cursor && c == '\\' {
            if let Some(c) = src[i + 1..].chars().next() {
                if is_punct_or_symbol(c) {
                    return Some((i, 1 + c.len_utf8()));
                }
            }
        }
    }
    None
}

/// blockSkip: link | (?<!`)`+[^`]+`+(?!`) | <(?! )[^<>]*?>
/// Returns (byte_index, match_len, group2_len).
fn find_block_skip(src: &str, from: usize) -> Option<(usize, usize, usize)> {
    let b = src.as_bytes();
    let len = b.len();
    let mut i = from;
    while i < len {
        if b[i] == b'[' {
            if let Some(l) = blockskip_link_at(src, i) {
                return Some((i, l, 0));
            }
        }
        if b[i] == b'`' && (i == 0 || b[i - 1] != b'`') {
            let mut run = 0usize;
            while i + run < len && b[i + run] == b'`' {
                run += 1;
            }
            let content_start = i + run;
            let mut j = content_start;
            while j < len && b[j] != b'`' {
                j += 1;
            }
            if j > content_start {
                let mut r2 = 0usize;
                while j + r2 < len && b[j + r2] == b'`' {
                    r2 += 1;
                }
                if r2 == run {
                    // Node supports lookbehind: the precode capture is empty.
                    return Some((i, j + r2 - i, 0));
                }
            }
        }
        if b[i] == b'<' && b.get(i + 1) != Some(&b' ') {
            let mut j = i + 1;
            while j < len && b[j] != b'<' && b[j] != b'>' {
                j += 1;
            }
            if b.get(j) == Some(&b'>') {
                return Some((i, j + 1 - i, 0));
            }
        }
        i += 1;
    }
    None
}

/// blockSkip link alternative.
fn blockskip_link_at(src: &str, start: usize) -> Option<usize> {
    let b = src.as_bytes();
    let len = b.len();
    if b.get(start) != Some(&b'[') {
        return None;
    }
    let mut i = start + 1;
    loop {
        if b.get(i) == Some(&b']') && b.get(i + 1) == Some(&b'(') {
            let mut j = i + 2;
            loop {
                if b.get(j) == Some(&b')') {
                    return Some(j + 1 - start);
                }
                if j >= len {
                    break;
                }
                if b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if b[j] == b'(' {
                    let mut k = j + 1;
                    loop {
                        if k >= len {
                            return None;
                        }
                        if b[k] == b'\\' {
                            k += 2;
                            continue;
                        }
                        if b[k] == b')' {
                            break;
                        }
                        k += 1;
                    }
                    if b.get(k) == Some(&b')') {
                        j = k + 1;
                        continue;
                    }
                    break;
                }
                if b[j] == b'(' || b[j] == b')' {
                    break;
                }
                j += 1;
            }
        }
        if i >= len {
            return None;
        }
        if b[i] == b'`' {
            i = backtick_span_at(src, i)?;
        } else if matches!(b[i], b'[' | b']') {
            return None;
        } else if b[i] == b'\\' {
            i += 2;
        } else {
            i += src[i..].chars().next()?.len_utf8();
        }
    }
}

/// `\_blockLabel_`: (?!\s*\])(?:\[\s\S]|[^\[\]\])+ — bracket label used by
/// reflink/nolink/def. Returns (label, index_of_closing_bracket).
fn bracket_label_at(src: &str, start: usize) -> Option<(String, usize)> {
    let b = src.as_bytes();
    if b.get(start) != Some(&b'[') {
        return None;
    }
    let mut i = start + 1;
    // (?!\s*\])
    {
        let j = skip_js_space(src, i);
        if b.get(j) == Some(&b']') {
            return None;
        }
    }
    let label_start = i;
    let mut escaped = false;
    while i < b.len() {
        let c = b[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if c == b'[' || c == b']' {
            break;
        }
        i += 1;
    }
    if b.get(i) != Some(&b']') {
        return None;
    }
    Some((src[label_start..i].to_string(), i))
}

#[cfg(test)]
mod upstream_token_tests {
    use super::*;
    use serde_json::{json, Value};

    /// Mirrors `snap` in tests/fixtures/marked-18.0.11-oracle/oracle/gen-tokens.mjs:
    /// the ACTUAL marked 18.0.11 `Lexer.lex` token tree, with raw/text/href as
    /// JS UTF-16 unit arrays so astral boundaries compare byte-exactly.
    fn snapshot(token: &Token) -> Value {
        json!({
            "kind": token.kind,
            "raw": token.raw_units().into_units(),
            "text": token.text_units().into_units(),
            "href": token.href_utf16.clone().unwrap_or_else(|| Utf16Text::from(&token.href)).into_units(),
            "depth": token.depth,
            "ordered": token.ordered,
            "start": token.start,
            "loose": token.loose,
            "task": token.task,
            "checked": token.checked,
            "lang": token.lang,
            "tokens": token.tokens.iter().map(snapshot).collect::<Vec<_>>(),
            "items": token.items.iter().map(snapshot).collect::<Vec<_>>(),
            "header": token.header.iter().map(cell_snapshot).collect::<Vec<_>>(),
            "rows": token.rows.iter()
                .map(|row| row.iter().map(cell_snapshot).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            "align": token.align.iter().map(|a| a.map(|c| c.to_string())).collect::<Vec<_>>(),
        })
    }

    fn cell_snapshot(token: &Token) -> Value {
        json!({
            "kind": "tablecell",
            "raw": token.raw_units().into_units(),
            "text": token.text_units().into_units(),
            "tokens": token.tokens.iter().map(snapshot).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn markdown_tokens_match_marked_18_0_11() {
        let corpus: Value =
            serde_json::from_str(include_str!("markdown_tokens_fixtures.json")).unwrap();
        let cases = corpus["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 55);
        for case in cases {
            let source = case["source"].as_str().unwrap();
            let mut lexer = Lexer::new();
            let tokens = lexer.lex(source, &NoExtensions);
            let actual: Vec<_> = tokens.iter().map(snapshot).collect();
            assert_eq!(json!(actual), case["expected"], "{}", case["name"]);
        }
    }
}

#[cfg(test)]
mod inline_tail_tests {
    use super::*;
    use serde_json::{json, Value};

    fn snapshot(token: &Token) -> Value {
        json!({
            "kind": token.kind,
            "raw": token.raw_units().into_units(),
            "text": token.text_units().into_units(),
            "href": token.href_utf16.clone().unwrap_or_else(|| Utf16Text::from(&token.href)).into_units(),
            "tokens": token.tokens.iter().map(snapshot).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn markdown_inline_tail_tokens_match_actual_upstream() {
        let corpus: Value =
            serde_json::from_str(include_str!("markdown_inline_tail_fixtures.json")).unwrap();
        let cases = corpus["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 48);
        for case in cases {
            let units: Vec<u16> = serde_json::from_value(case["source"].clone()).unwrap();
            let (prefix, tail) = split_inline_tail(Utf16Text::from_units(units.clone()));
            let mut lexer = Lexer::new();
            let mut tokens = Vec::new();
            lexer.inline_tokens_with_tail(&prefix, tail, &mut tokens, &NoExtensions);
            let actual: Vec<_> = tokens.iter().map(snapshot).collect();
            assert_eq!(json!(actual), case["expected"], "{}", case["name"]);
            let reconstructed: Vec<_> = tokens
                .iter()
                .flat_map(|t| t.raw_units().into_units())
                .collect();
            assert_eq!(reconstructed, units, "raw source {}", case["name"]);
        }
    }
}
