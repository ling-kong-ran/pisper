//! Port of `src/tools/edit-diff.ts`: shared diff computation utilities for
//! the edit and similar tools, including the jsdiff 8.0.4 semantics upstream
//! reaches through `Diff.diffLines` / `Diff.createTwoFilesPatch` (Myers path
//! selection and tie-breaking preserved; see docs/migration/reference/
//! jsdiff-LICENSE).
//!
//! Internal match/patch offsets are UTF-8 byte offsets (never exposed in tool
//! results); line numbers and emitted diff text are identical to upstream.

use std::collections::HashMap;
use std::sync::Arc;

use icu_normalizer::ComposingNormalizerBorrowed;

use crate::durable::errors::PlainError;

// ─── Line ending and normalization helpers ──────────────────────────────────

/// `detectLineEnding(content)` (`tools/edit-diff.ts`).
pub fn detect_line_ending(content: &str) -> &'static str {
    match (content.find("\r\n"), content.find('\n')) {
        (Some(crlf), Some(lf)) if crlf < lf => "\r\n",
        _ => "\n",
    }
}

/// `normalizeToLF(text)` (`tools/edit-diff.ts`).
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `restoreLineEndings(text, ending)` (`tools/edit-diff.ts`).
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

fn js_whitespace(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}

/// `normalizeForFuzzyMatch(text)` (`tools/edit-diff.ts`): NFKC, per-line
/// trailing-whitespace strip, smart quotes to ASCII, dashes to `-`, special
/// spaces to plain spaces.
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    ComposingNormalizerBorrowed::new_nfkc()
        .normalize(text)
        .split('\n')
        .map(|line| line.trim_end_matches(js_whitespace))
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(|c| match c {
            '\u{2018}'..='\u{201b}' => '\'',
            '\u{201c}'..='\u{201f}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            _ => c,
        })
        .collect()
}

/// `stripBom(content)` (`tools/edit-diff.ts`): `(bom, text)`.
pub fn strip_bom(content: &str) -> (&str, &str) {
    if let Some(text) = content.strip_prefix('\u{feff}') {
        ("\u{feff}", text)
    } else {
        ("", content)
    }
}

// ─── jsdiff line diff (`Diff.diffLines`) ────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Equal,
    Add,
    Remove,
}
#[derive(Debug, Clone)]
struct Part {
    kind: Change,
    value: String,
}
struct Component {
    kind: Change,
    count: usize,
    previous: Option<Arc<Component>>,
}
#[derive(Clone)]
struct Path {
    old: i64,
    last: Option<Arc<Component>>,
}
fn add(path: &Path, kind: Change) -> Path {
    let (count, previous) = match &path.last {
        Some(last) if last.kind == kind => (last.count + 1, last.previous.clone()),
        _ => (1, path.last.clone()),
    };
    Path {
        old: path.old + i64::from(kind == Change::Remove),
        last: Some(Arc::new(Component {
            kind,
            count,
            previous,
        })),
    }
}
fn common(path: &mut Path, old: &[&str], new: &[&str], diagonal: i64) -> i64 {
    let mut new_pos = path.old - diagonal;
    let mut count = 0;
    while (new_pos + 1) < new.len() as i64
        && (path.old + 1) < old.len() as i64
        && old[(path.old + 1) as usize] == new[(new_pos + 1) as usize]
    {
        path.old += 1;
        new_pos += 1;
        count += 1;
    }
    if count > 0 {
        path.last = Some(Arc::new(Component {
            kind: Change::Equal,
            count,
            previous: path.last.clone(),
        }));
    }
    new_pos
}
fn values(path: Path, old: &[&str], new: &[&str]) -> Vec<Part> {
    let mut components = Vec::new();
    let mut last = path.last;
    while let Some(item) = last {
        components.push((item.kind, item.count));
        last = item.previous.clone();
    }
    components.reverse();
    let (mut old_pos, mut new_pos) = (0, 0);
    components
        .into_iter()
        .map(|(kind, count)| {
            let value = if kind == Change::Remove {
                let value = old[old_pos..old_pos + count].concat();
                old_pos += count;
                value
            } else {
                let value = new[new_pos..new_pos + count].concat();
                new_pos += count;
                if kind == Change::Equal {
                    old_pos += count;
                }
                value
            };
            Part { kind, value }
        })
        .collect()
}
/// `Diff.diffLines(oldContent, newContent)` (`tools/edit-diff.ts`): line
/// tokens keep their trailing newline, like jsdiff's default line tokenizer.
fn diff_lines(old: &str, new: &str) -> Vec<Part> {
    let old: Vec<_> = old.split_inclusive('\n').collect();
    let new: Vec<_> = new.split_inclusive('\n').collect();
    let mut seed = Path {
        old: -1,
        last: None,
    };
    let np = common(&mut seed, &old, &new, 0);
    if seed.old + 1 >= old.len() as i64 && np + 1 >= new.len() as i64 {
        return values(seed, &old, &new);
    }
    let mut best = HashMap::from([(0i64, seed)]);
    let (mut min_diagonal, mut max_diagonal) = (i64::MIN, i64::MAX);
    for distance in 1..=(old.len() + new.len()) as i64 {
        let mut diagonal = min_diagonal.max(-distance);
        while diagonal <= max_diagonal.min(distance) {
            let remove = best.remove(&(diagonal - 1));
            let add_path = best.get(&(diagonal + 1)).cloned();
            let can_add = add_path.as_ref().is_some_and(|path| {
                let np = path.old - diagonal;
                0 <= np && np < new.len() as i64
            });
            let can_remove = remove
                .as_ref()
                .is_some_and(|path| path.old + 1 < old.len() as i64);
            if !can_add && !can_remove {
                best.remove(&diagonal);
                diagonal += 2;
                continue;
            }
            let mut path = if !can_remove
                || (can_add && remove.as_ref().unwrap().old < add_path.as_ref().unwrap().old)
            {
                add(add_path.as_ref().unwrap(), Change::Add)
            } else {
                add(remove.as_ref().unwrap(), Change::Remove)
            };
            let np = common(&mut path, &old, &new, diagonal);
            if path.old + 1 >= old.len() as i64 && np + 1 >= new.len() as i64 {
                return values(path, &old, &new);
            }
            if path.old + 1 >= old.len() as i64 {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if np + 1 >= new.len() as i64 {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            best.insert(diagonal, path);
            diagonal += 2;
        }
    }
    unreachable!("unbounded edit graph always has a path")
}

/// `generateUnifiedPatch(path, oldContent, newContent, contextLines)`
/// (`tools/edit-diff.ts`): jsdiff `createTwoFilesPatch` with
/// `headerOptions: FILE_HEADERS_ONLY`.
pub fn generate_unified_patch(
    path: &str,
    old_content: &str,
    new_content: &str,
    context: usize,
) -> String {
    let mut parts = diff_lines(old_content, new_content);
    parts.push(Part {
        kind: Change::Equal,
        value: String::new(),
    });
    let lines: Vec<Vec<&str>> = parts
        .iter()
        .map(|part| part.value.split_inclusive('\n').collect())
        .collect();
    let (mut old_start, mut new_start, mut old_line, mut new_line) =
        (0usize, 0usize, 1usize, 1usize);
    let mut range = Vec::<String>::new();
    let mut output = vec![format!("--- {path}"), format!("+++ {path}")];
    for (i, part) in parts.iter().enumerate() {
        let current = &lines[i];
        if part.kind != Change::Equal {
            if old_start == 0 {
                old_start = old_line;
                new_start = new_line;
                if i > 0 {
                    let previous = &lines[i - 1];
                    let start = previous.len().saturating_sub(context);
                    range = previous[start..]
                        .iter()
                        .map(|line| format!(" {line}"))
                        .collect();
                    old_start -= range.len();
                    new_start -= range.len();
                }
            }
            let prefix = if part.kind == Change::Add { '+' } else { '-' };
            range.extend(current.iter().map(|line| format!("{prefix}{line}")));
            if part.kind == Change::Add {
                new_line += current.len();
            } else {
                old_line += current.len();
            }
        } else {
            if old_start != 0 {
                if current.len() <= context.saturating_mul(2) && i < parts.len().saturating_sub(2) {
                    range.extend(current.iter().map(|line| format!(" {line}")));
                } else {
                    let count = current.len().min(context);
                    range.extend(current[..count].iter().map(|line| format!(" {line}")));
                    let old_count = old_line - old_start + count;
                    let new_count = new_line - new_start + count;
                    output.push(format!(
                        "@@ -{},{} +{},{} @@",
                        old_start - usize::from(old_count == 0),
                        old_count,
                        new_start - usize::from(new_count == 0),
                        new_count
                    ));
                    for line in range.drain(..) {
                        if let Some(line) = line.strip_suffix('\n') {
                            output.push(line.to_string());
                        } else {
                            output.push(line);
                            output.push("\\ No newline at end of file".to_string());
                        }
                    }
                    old_start = 0;
                    new_start = 0;
                }
            }
            old_line += current.len();
            new_line += current.len();
        }
    }
    output.join("\n") + "\n"
}

/// `generateDiffString(oldContent, newContent, contextLines)`
/// (`tools/edit-diff.ts`): `firstChangedLine` is a line number in the new
/// file.
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> DisplayDiff {
    let parts = diff_lines(old_content, new_content);
    let width = old_content
        .split('\n')
        .count()
        .max(new_content.split('\n').count())
        .to_string()
        .len();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    let mut last_was_change = false;
    let mut first_changed_line = None;
    let mut output = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let mut raw: Vec<_> = part.value.split('\n').collect();
        if raw.last() == Some(&"") {
            raw.pop();
        }
        if part.kind != Change::Equal {
            first_changed_line.get_or_insert(new_line);
            for line in raw {
                if part.kind == Change::Add {
                    output.push(format!("+{new_line:>width$} {line}"));
                    new_line += 1;
                } else {
                    output.push(format!("-{old_line:>width$} {line}"));
                    old_line += 1;
                }
            }
            last_was_change = true;
        } else {
            let next_part_is_change = parts.get(i + 1).is_some_and(|p| p.kind != Change::Equal);
            let len = raw.len();
            let (head, tail) = match (last_was_change, next_part_is_change) {
                (true, true) if len <= context_lines.saturating_mul(2) => (len, 0),
                (true, true) => (context_lines, context_lines),
                (true, false) => (len.min(context_lines), 0),
                (false, true) => (0, len.min(context_lines)),
                (false, false) => {
                    old_line += len;
                    new_line += len;
                    last_was_change = false;
                    continue;
                }
            };
            for line in &raw[..head] {
                output.push(format!(" {old_line:>width$} {line}"));
                old_line += 1;
                new_line += 1;
            }
            let skipped = len - head - tail;
            if skipped > 0 {
                output.push(format!(" {:>width$} ...", ""));
                old_line += skipped;
                new_line += skipped;
            }
            for line in &raw[len - tail..] {
                output.push(format!(" {old_line:>width$} {line}"));
                old_line += 1;
                new_line += 1;
            }
            last_was_change = false;
        }
    }
    DisplayDiff {
        diff: output.join("\n"),
        first_changed_line,
    }
}

/// `generateDiffString` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayDiff {
    pub diff: String,
    pub first_changed_line: Option<usize>,
}

// ─── Matching and replacement ───────────────────────────────────────────────

/// `splitLinesWithEndings(content)` (`tools/edit-diff.ts`): lines keep their
/// `\n`; an empty content yields no lines.
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    content.split_inclusive('\n').collect()
}

struct LineSpan {
    start: usize,
    end: usize,
}

/// A replacement matched against base content; byte offsets (`MatchedEdit`
/// minus `editIndex` / `newText`).
#[derive(Clone)]
struct TextReplacement {
    match_index: usize,
    match_length: usize,
    new_text: String,
}

fn get_line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = LineSpan {
                start: offset,
                end: offset + line.len(),
            };
            offset = span.end;
            span
        })
        .collect()
}

fn get_replacement_line_range(
    lines: &[LineSpan],
    replacement: &TextReplacement,
) -> Result<(usize, usize), PlainError> {
    let replacement_start = replacement.match_index;
    let replacement_end = replacement.match_index + replacement.match_length;

    let mut start_line = None;
    for (i, line) in lines.iter().enumerate() {
        if replacement_start >= line.start && replacement_start < line.end {
            start_line = Some(i);
            break;
        }
    }
    let Some(mut end_line) = start_line else {
        return Err(PlainError::new(
            "Replacement range is outside the base content.",
        ));
    };

    while end_line < lines.len() && lines[end_line].end < replacement_end {
        end_line += 1;
    }
    if end_line >= lines.len() {
        return Err(PlainError::new(
            "Replacement range is outside the base content.",
        ));
    }

    Ok((start_line.unwrap(), end_line + 1))
}

fn apply_replacements(content: &str, replacements: &[TextReplacement], offset: usize) -> String {
    let mut result = content.to_string();
    for replacement in replacements.iter().rev() {
        let match_index = replacement.match_index - offset;
        result.replace_range(
            match_index..match_index + replacement.match_length,
            &replacement.new_text,
        );
    }
    result
}

/// `TextReplacement` view of matched edits (`Pick<MatchedEdit, ...>`).
fn text_replacements(edits: &[MatchedEdit]) -> Vec<TextReplacement> {
    edits
        .iter()
        .map(|edit| TextReplacement {
            match_index: edit.match_index,
            match_length: edit.match_length,
            new_text: edit.new_text.clone(),
        })
        .collect()
}

/// `applyReplacementsPreservingUnchangedLines(originalContent, baseContent,
/// replacements)` (`tools/edit-diff.ts`).
pub fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[MatchedEdit],
) -> Result<String, PlainError> {
    let mut sorted_replacements = text_replacements(replacements);
    let original_lines = split_lines_with_endings(original_content);
    let base_lines = get_line_spans(base_content);
    if original_lines.len() != base_lines.len() {
        return Err(PlainError::new(
            "Cannot preserve unchanged lines because the base content has a different line count.",
        ));
    }

    let mut groups: Vec<(usize, usize, Vec<TextReplacement>)> = Vec::new();
    sorted_replacements.sort_by_key(|replacement| replacement.match_index);
    for replacement in &sorted_replacements {
        let (start_line, end_line) = get_replacement_line_range(&base_lines, replacement)?;
        if let Some(current) = groups.last_mut().filter(|current| start_line < current.1) {
            current.1 = current.1.max(end_line);
            current.2.push(replacement.clone());
            continue;
        }
        groups.push((start_line, end_line, vec![replacement.clone()]));
    }

    let mut original_line_index = 0;
    let mut result = String::new();
    for (start_line, end_line, group) in &groups {
        result.push_str(&original_lines[original_line_index..*start_line].concat());

        let group_start_offset = base_lines[*start_line].start;
        let group_end_offset = base_lines[end_line - 1].end;
        result.push_str(&apply_replacements(
            &base_content[group_start_offset..group_end_offset],
            group,
            group_start_offset,
        ));
        original_line_index = *end_line;
    }
    result.push_str(&original_lines[original_line_index..].concat());

    Ok(result)
}

/// `FuzzyMatchResult` (`tools/edit-diff.ts`). `index` mirrors the JS
/// `indexOf` shape (`-1` when not found).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatchResult {
    /// Whether a match was found.
    pub found: bool,
    /// The index where the match starts (in the content that should be used
    /// for replacement); `-1` when not found.
    pub index: i64,
    /// Length of the matched text.
    pub match_length: usize,
    /// Whether fuzzy matching was used (false = exact match).
    pub used_fuzzy_match: bool,
    /// The content to use for replacement operations. When exact match:
    /// original content. When fuzzy match: normalized content.
    pub content_for_replacement: String,
}

/// One matched edit (`MatchedEdit` in `tools/edit-diff.ts`).
#[derive(Debug, Clone)]
pub struct MatchedEdit {
    pub edit_index: usize,
    pub match_index: usize,
    pub match_length: usize,
    pub new_text: String,
}

/// `Edit` (`tools/edit-diff.ts`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// `AppliedEditsResult` (`tools/edit-diff.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEditsResult {
    pub base_content: String,
    pub new_content: String,
}

/// `fuzzyFindText(content, oldText)` (`tools/edit-diff.ts`): exact match
/// first, then fuzzy match in normalized space.
pub fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatchResult {
    // Try exact match first
    if let Some(exact_index) = content.find(old_text) {
        return FuzzyMatchResult {
            found: true,
            index: exact_index as i64,
            match_length: old_text.len(),
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        };
    }

    // Try fuzzy match - work entirely in normalized space
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    let fuzzy_index = fuzzy_content.find(&fuzzy_old_text);

    let Some(fuzzy_index) = fuzzy_index else {
        return FuzzyMatchResult {
            found: false,
            index: -1,
            match_length: 0,
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        };
    };

    // When fuzzy matching, return offsets in normalized space. Callers can
    // use the normalized content to compute replacements, then decide how
    // much of that normalized output should be written back.
    FuzzyMatchResult {
        found: true,
        index: fuzzy_index as i64,
        match_length: fuzzy_old_text.len(),
        used_fuzzy_match: true,
        content_for_replacement: fuzzy_content,
    }
}

/// `countOccurrences(content, oldText)` (`tools/edit-diff.ts`): the JS
/// `split(needle).length - 1` count, where an empty needle splits per UTF-16
/// code unit.
fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    if fuzzy_old_text.is_empty() {
        fuzzy_content.encode_utf16().count().saturating_sub(1)
    } else {
        fuzzy_content.matches(&fuzzy_old_text).count()
    }
}

fn get_not_found_error(path: &str, edit_index: usize, total_edits: usize) -> PlainError {
    if total_edits == 1 {
        PlainError::new(format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        ))
    } else {
        PlainError::new(format!(
            "Could not find edits[{edit_index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        ))
    }
}

fn get_duplicate_error(
    path: &str,
    edit_index: usize,
    total_edits: usize,
    occurrences: usize,
) -> PlainError {
    if total_edits == 1 {
        PlainError::new(format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        ))
    } else {
        PlainError::new(format!(
            "Found {occurrences} occurrences of edits[{edit_index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        ))
    }
}

fn get_empty_old_text_error(path: &str, edit_index: usize, total_edits: usize) -> PlainError {
    if total_edits == 1 {
        PlainError::new(format!("oldText must not be empty in {path}."))
    } else {
        PlainError::new(format!(
            "edits[{edit_index}].oldText must not be empty in {path}."
        ))
    }
}

fn get_no_change_error(path: &str, total_edits: usize) -> PlainError {
    if total_edits == 1 {
        PlainError::new(format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        ))
    } else {
        PlainError::new(format!(
            "No changes made to {path}. The replacements produced identical content."
        ))
    }
}

/// `applyEditsToNormalizedContent(normalizedContent, edits, path)`
/// (`tools/edit-diff.ts`).
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEditsResult, PlainError> {
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();

    for (i, edit) in normalized_edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(get_empty_old_text_error(path, i, normalized_edits.len()));
        }
    }

    let initial_matches: Vec<FuzzyMatchResult> = normalized_edits
        .iter()
        .map(|edit| fuzzy_find_text(normalized_content, &edit.old_text))
        .collect();
    let used_fuzzy_match = initial_matches.iter().any(|m| m.used_fuzzy_match);
    let replacement_base_content = if used_fuzzy_match {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched_edits: Vec<MatchedEdit> = Vec::new();
    for (i, edit) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&replacement_base_content, &edit.old_text);
        if !match_result.found {
            return Err(get_not_found_error(path, i, normalized_edits.len()));
        }

        let occurrences = count_occurrences(&replacement_base_content, &edit.old_text);
        if occurrences > 1 {
            return Err(get_duplicate_error(
                path,
                i,
                normalized_edits.len(),
                occurrences,
            ));
        }

        matched_edits.push(MatchedEdit {
            edit_index: i,
            match_index: match_result.index as usize,
            match_length: match_result.match_length,
            new_text: edit.new_text.clone(),
        });
    }

    matched_edits.sort_by_key(|edit| edit.match_index);
    for pair in matched_edits.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if previous.match_index + previous.match_length > current.match_index {
            return Err(PlainError::new(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index
            )));
        }
    }

    let base_content = normalized_content.to_string();
    let new_content = if used_fuzzy_match {
        apply_replacements_preserving_unchanged_lines(
            normalized_content,
            &replacement_base_content,
            &matched_edits,
        )?
    } else {
        apply_replacements(
            &replacement_base_content,
            &text_replacements(&matched_edits),
            0,
        )
    };

    if base_content == new_content {
        return Err(get_no_change_error(path, normalized_edits.len()));
    }

    Ok(AppliedEditsResult {
        base_content,
        new_content,
    })
}
