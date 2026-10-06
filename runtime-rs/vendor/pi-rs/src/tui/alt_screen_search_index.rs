//! Transcript search index from `alt-screen-search.ts:1–196`.
//!
//! Matches are literal, Unicode-simple-case-insensitive and non-overlapping.
//! Corpus offsets remain UTF-16 units, while result coordinates are terminal
//! cells. The raw entry points preserve lone surrogates; UTF-8 convenience
//! methods are lossless for well-formed input. No Search UI or host IO lives here.
//!
//! The reference cache returns a mutable array by identity. Consequently result
//! arrays, match objects, segment arrays and segment objects are separate shared
//! handles. Cloning a handle aliases it; a changed query/source creates a new
//! result array without mutating old handles. Release `RefCell` borrows before
//! mutating aliases. This is a same-thread, dense/typed-array API, not arbitrary
//! JavaScript sparse properties, getters, cyclic objects or unrestricted numbers.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use unicode_segmentation::UnicodeSegmentation;

use super::utf16::Utf16Text;
use super::utils::{is_whitespace_char, strip_terminal_sequences_utf16, visible_width_utf16};

mod simple_case_fold;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltScreenSearchSegment {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

pub type SearchSegmentHandle = Rc<RefCell<AltScreenSearchSegment>>;
pub type SearchSegments = Rc<RefCell<Vec<SearchSegmentHandle>>>;
pub type SearchMatchHandle = Rc<RefCell<AltScreenSearchMatch>>;
pub type SearchMatches = Rc<RefCell<Vec<SearchMatchHandle>>>;

#[derive(Clone, Debug, Default)]
pub struct AltScreenSearchMatch {
    pub segments: SearchSegments,
}

impl AltScreenSearchMatch {
    pub fn new(segments: impl IntoIterator<Item = AltScreenSearchSegment>) -> Self {
        Self {
            segments: Rc::new(RefCell::new(
                segments
                    .into_iter()
                    .map(|s| Rc::new(RefCell::new(s)))
                    .collect(),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AltScreenSearchResult {
    pub matches: SearchMatches,
    pub changed: bool,
}

#[derive(Debug)]
pub(super) struct SearchSourceSpan {
    pub(super) text_start: usize,
    pub(super) text_end: usize,
    pub(super) row: usize,
    pub(super) start_col: usize,
    pub(super) end_col: usize,
    pub(super) linear_columns: bool,
}

#[derive(Debug)]
struct FoldedPoint {
    value: u32,
    start: usize,
    end: usize,
}

#[derive(Debug)]
pub(super) struct SearchCorpus {
    pub(super) text: Utf16Text,
    pub(super) spans: Vec<SearchSourceSpan>,
    folded: Vec<FoldedPoint>,
}

/// Cache source string contents and normalized query, not input array identity.
#[derive(Debug, Default)]
pub struct AltScreenSearchIndex {
    source_lines: Option<Vec<Utf16Text>>,
    corpus: Option<SearchCorpus>,
    normalized_query: Option<Utf16Text>,
    matches: SearchMatches,
}

impl AltScreenSearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn search<S: AsRef<str>>(&mut self, lines: &[S], query: &str) -> AltScreenSearchResult {
        let lines: Vec<_> = lines.iter().map(|s| Utf16Text::from(s.as_ref())).collect();
        self.search_utf16(&lines, &Utf16Text::from(query))
    }

    pub fn search_utf16(
        &mut self,
        lines: &[Utf16Text],
        query: &Utf16Text,
    ) -> AltScreenSearchResult {
        let source_changed = self.source_lines.as_deref() != Some(lines);
        if source_changed || self.corpus.is_none() {
            self.source_lines = Some(lines.to_vec());
            self.corpus = Some(build_search_corpus(lines));
        }
        let query = normalize_query(query);
        let changed = source_changed || self.normalized_query.as_ref() != Some(&query);
        if changed {
            self.matches = find_corpus_matches(self.corpus.as_ref().expect("corpus built"), &query);
            self.normalized_query = Some(query);
        }
        AltScreenSearchResult {
            matches: Rc::clone(&self.matches),
            changed,
        }
    }

    // Read-only observation for differential tests; not a second implementation.
    #[cfg(test)]
    pub(super) fn observed_state(
        &self,
    ) -> (
        Option<&[Utf16Text]>,
        Option<&Utf16Text>,
        Option<&SearchCorpus>,
        SearchMatches,
    ) {
        (
            self.source_lines.as_deref(),
            self.normalized_query.as_ref(),
            self.corpus.as_ref(),
            Rc::clone(&self.matches),
        )
    }
}

pub fn find_alt_screen_search_matches<S: AsRef<str>>(lines: &[S], query: &str) -> SearchMatches {
    let lines: Vec<_> = lines.iter().map(|s| Utf16Text::from(s.as_ref())).collect();
    find_alt_screen_search_matches_utf16(&lines, &Utf16Text::from(query))
}

pub fn find_alt_screen_search_matches_utf16(
    lines: &[Utf16Text],
    query: &Utf16Text,
) -> SearchMatches {
    let query = normalize_query(query);
    if query.is_empty() {
        return SearchMatches::default();
    }
    find_corpus_matches(&build_search_corpus(lines), &query)
}

pub fn get_alt_screen_search_match_key(search_match: &AltScreenSearchMatch) -> String {
    let segments = search_match.segments.borrow();
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return String::new();
    };
    let first = first.borrow();
    let last = last.borrow();
    format!(
        "{}:{}:{}:{}",
        first.row, first.start_col, last.row, last.end_col
    )
}

fn whitespace(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(is_whitespace_char)
}

fn normalize_query(query: &Utf16Text) -> Utf16Text {
    let mut result = Vec::with_capacity(query.len());
    let mut pending_space = false;
    for &unit in query.as_units() {
        if whitespace(unit) {
            pending_space = !result.is_empty();
        } else {
            if pending_space {
                result.push(0x20);
                pending_space = false;
            }
            result.push(unit);
        }
    }
    Utf16Text::from_units(result)
}

// Lone surrogates and U+FFFD share the needed grapheme-break properties, but
// not their width or identity. Use a mapped view ONLY for segmentation; slice
// and measure the original units, never the replacement view.
pub(super) fn grapheme_ranges(text: &Utf16Text) -> Vec<Range<usize>> {
    let mut view = String::new();
    let mut offsets = Vec::new();
    let mut unit = 0;
    for point in char::decode_utf16(text.as_units().iter().copied()) {
        offsets.push((view.len(), unit));
        unit += point.as_ref().map_or(1, |c| c.len_utf16());
        view.push(point.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    offsets.push((view.len(), unit));
    view.grapheme_indices(true)
        .map(|(start, s)| {
            let a = offsets
                .binary_search_by_key(&start, |p| p.0)
                .expect("scalar start");
            let b = offsets
                .binary_search_by_key(&(start + s.len()), |p| p.0)
                .expect("scalar end");
            offsets[a].1..offsets[b].1
        })
        .collect()
}

fn build_search_corpus(lines: &[Utf16Text]) -> SearchCorpus {
    let mut text = Vec::new();
    let mut spans = Vec::new();
    let mut pending_separator = false;
    for (row, original) in lines.iter().enumerate() {
        let line = Utf16Text::from_units(strip_terminal_sequences_utf16(original.as_units()));
        let units = line.as_units();
        let mut column = 0;
        let ascii = units.iter().all(|u| (0x20..=0x7e).contains(u));
        let ranges = if ascii {
            let mut ranges = Vec::new();
            let mut start = 0;
            while start < units.len() {
                let mut end = start + 1;
                if units[start] != 0x20 {
                    while end < units.len() && units[end] != 0x20 {
                        end += 1;
                    }
                }
                ranges.push(start..end);
                start = end;
            }
            ranges
        } else {
            grapheme_ranges(&line)
        };
        for range in ranges {
            let part = &units[range];
            let width = if ascii {
                part.len()
            } else {
                visible_width_utf16(part)
            };
            if part.iter().copied().all(whitespace) {
                if !text.is_empty() {
                    pending_separator = true;
                }
                column += width;
                continue;
            }
            if pending_separator {
                text.push(0x20);
                pending_separator = false;
            }
            let text_start = text.len();
            text.extend_from_slice(part);
            spans.push(SearchSourceSpan {
                text_start,
                text_end: text.len(),
                row,
                start_col: column,
                end_col: column + width,
                linear_columns: ascii,
            });
            column += width;
        }
        if !text.is_empty() {
            pending_separator = true;
        }
    }
    let text = Utf16Text::from_units(text);
    let folded = folded_points(&text);
    SearchCorpus {
        text,
        spans,
        folded,
    }
}

fn simple_fold(point: u32) -> u32 {
    let table = simple_case_fold::SIMPLE_FOLD;
    table
        .binary_search_by_key(&point, |&(from, _)| from)
        .map_or(point, |index| table[index].1)
}

fn folded_points(text: &Utf16Text) -> Vec<FoldedPoint> {
    let mut start = 0;
    char::decode_utf16(text.as_units().iter().copied())
        .map(|point| {
            let (value, length) = match point {
                Ok(c) => (u32::from(c), c.len_utf16()),
                Err(e) => (u32::from(e.unpaired_surrogate()), 1),
            };
            let end = start + length;
            let point = FoldedPoint {
                value: simple_fold(value),
                start,
                end,
            };
            start = end;
            point
        })
        .collect()
}

fn find_corpus_matches(corpus: &SearchCorpus, query: &Utf16Text) -> SearchMatches {
    let needle: Vec<_> = folded_points(query).into_iter().map(|p| p.value).collect();
    if needle.is_empty() {
        return SearchMatches::default();
    }
    // KMP on Unicode code points, not bytes or UTF-16 units. This reproduces a
    // literal /giu search: C/S simple folding, no full/locale folding, no match
    // inside a surrogate pair and no overlapping global matches. Keep offsets
    // from the ORIGINAL corpus even if a mapping's encoded length differs.
    let mut prefix = vec![0; needle.len()];
    for i in 1..needle.len() {
        let mut n = prefix[i - 1];
        while n > 0 && needle[i] != needle[n] {
            n = prefix[n - 1];
        }
        if needle[i] == needle[n] {
            n += 1;
        }
        prefix[i] = n;
    }
    let mut matched = 0;
    let mut span_index = 0;
    let mut matches = Vec::new();
    for (i, point) in corpus.folded.iter().enumerate() {
        while matched > 0 && point.value != needle[matched] {
            matched = prefix[matched - 1];
        }
        if point.value == needle[matched] {
            matched += 1;
        }
        if matched != needle.len() {
            continue;
        }
        let start = corpus.folded[i + 1 - matched].start;
        let end = point.end;
        debug_assert!(end <= corpus.text.len());
        matched = 0; // JS matchAll advances past the entire literal match.
        while span_index < corpus.spans.len() && corpus.spans[span_index].text_end <= start {
            span_index += 1;
        }
        let mut segments: Vec<AltScreenSearchSegment> = Vec::new();
        for span in &corpus.spans[span_index..] {
            if span.text_start >= end {
                break;
            }
            if span.text_end <= start {
                continue;
            }
            let start_col = if span.linear_columns {
                span.start_col + start.max(span.text_start) - span.text_start
            } else {
                span.start_col
            };
            let end_col = if span.linear_columns {
                span.start_col + end.min(span.text_end) - span.text_start
            } else {
                span.end_col
            };
            if let Some(previous) = segments
                .last_mut()
                .filter(|p| p.row == span.row && start_col <= p.end_col)
            {
                previous.end_col = previous.end_col.max(end_col);
            } else {
                segments.push(AltScreenSearchSegment {
                    row: span.row,
                    start_col,
                    end_col,
                });
            }
        }
        while span_index < corpus.spans.len() && corpus.spans[span_index].text_end <= end {
            span_index += 1;
        }
        if !segments.is_empty() {
            matches.push(Rc::new(RefCell::new(AltScreenSearchMatch::new(segments))));
        }
    }
    Rc::new(RefCell::new(matches))
}
