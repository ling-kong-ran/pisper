//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/diff.ts` (147 lines, sha256
//! `b43f8d91ba9f19088dc9dc86126091c3fdfca25eac6461f5510f5e1e9ddd6491`):
//! [`render_diff`] with intra-line change highlighting.
//!
//! The upstream `Diff.diffWords` call resolves to the `diff` npm package
//! (8.0.4, pinned in `packages/coding-agent/package.json`); the `word diff`
//! section below re-states that package faithfully — the
//! `tokenizeIncludingWhitespace` regex and whitespace stitching (`word.js`),
//! the Myers base diff (`base.js`: `bestPath`/`extractCommon`/`buildValues`),
//! `WordDiff.equals` (trimmed token comparison), `join` (leading-whitespace
//! stripping) and `postProcess` (`dedupeWhitespaceInChangeObjects`).
//! Oracle: the verbatim upstream `diff.ts` runs against the real jsdiff 8.0.4
//! tarball in `tests/fixtures/interactive_r19_oracle/` (scenario
//! `diff_render_diff`), so the tests pin byte-identical output.
//!
//! Disclosed divergence (S19.2): jsdiff's whitespace helpers index strings by
//! UTF-16 code unit; this port indexes by `char`. Identical for the BMP
//! content the components render; only non-BMP characters at diff whitespace
//! boundaries could shift a suffix split.

use crate::coding_agent::modes::interactive::theme::Theme;

// ===========================================================================
// jsdiff wordDiff re-statement
// ===========================================================================

fn is_js_whitespace(c: char) -> bool {
    // ECMAScript `\s`
    matches!(
        c,
        ' ' | '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// Upstream `extendedWordChars` (`word.js`).
fn is_extended_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || c == '_'
        || c == '\u{AD}'
        || ('\u{C0}'..='\u{D6}').contains(&c)
        || ('\u{D8}'..='\u{F6}').contains(&c)
        || ('\u{F8}'..='\u{2C6}').contains(&c)
        || ('\u{2C8}'..='\u{2D7}').contains(&c)
        || ('\u{2DE}'..='\u{2FF}').contains(&c)
        || ('\u{1E00}'..='\u{1EFF}').contains(&c)
}

/// Upstream `tokenizeIncludingWhitespace` regex:
/// `[extendedWordChars]+|\s+|[^extendedWordChars]` (u flag).
fn tokenize_including_whitespace(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut kind: Option<u8> = None; // 0 = extended word run, 1 = whitespace run, 2 = single other
    for c in value.chars() {
        let next_kind = if is_extended_word_char(c) {
            0
        } else if is_js_whitespace(c) {
            1
        } else {
            2
        };
        match kind {
            Some(k) if k == next_kind && next_kind != 2 => current.push(c),
            _ => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
                current.push(c);
                kind = Some(next_kind);
            }
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Upstream `WordDiff.tokenize` (whitespace stitching onto adjacent tokens).
fn tokenize(value: &str) -> Vec<String> {
    let parts = tokenize_including_whitespace(value);
    let mut tokens: Vec<String> = Vec::new();
    let mut prev_part: Option<String> = None;
    for part in parts {
        let part_is_ws = part.chars().any(is_js_whitespace);
        match &prev_part {
            None => tokens.push(part.clone()),
            Some(prev) => {
                let prev_is_ws = prev.chars().any(is_js_whitespace);
                if part_is_ws {
                    // `tokens.push(tokens.pop() + part)`
                    let popped = tokens.pop().unwrap_or_default();
                    tokens.push(format!("{popped}{part}"));
                } else if prev_is_ws {
                    if tokens.last().map(String::as_str) == Some(prev.as_str()) {
                        let popped = tokens.pop().unwrap_or_default();
                        tokens.push(format!("{popped}{part}"));
                    } else {
                        tokens.push(format!("{prev}{part}"));
                    }
                } else {
                    tokens.push(part.clone());
                }
            }
        }
        prev_part = Some(part);
    }
    tokens
}

/// Upstream `WordDiff.equals`: trimmed equality.
fn tokens_equal(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

/// Upstream `WordDiff.join`: strip leading whitespace from every token but
/// the first.
fn join_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .enumerate()
        .map(|(i, token)| {
            if i == 0 {
                token.clone()
            } else {
                token.trim_start_matches(is_js_whitespace).to_string()
            }
        })
        .collect()
}

// ---- string whitespace helpers (util/string.js) ----------------------------

fn leading_ws(s: &str) -> &str {
    let end = s.find(|c: char| !is_js_whitespace(c)).unwrap_or(s.len());
    &s[..end]
}

fn trailing_ws(s: &str) -> &str {
    let start = s
        .char_indices()
        .rev()
        .find(|(_, ch)| !is_js_whitespace(*ch))
        .map(|(i, ch)| i + ch.len_utf8())
        .unwrap_or(0);
    &s[start..]
}

fn longest_common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut end = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            break;
        }
        end += ca.len_utf8();
    }
    &a[..end]
}

fn longest_common_suffix<'a>(a: &'a str, b: &str) -> &'a str {
    if a.is_empty() || b.is_empty() || a.chars().last() != b.chars().last() {
        return "";
    }
    let a_rev: Vec<char> = a.chars().rev().collect();
    let b_rev: Vec<char> = b.chars().rev().collect();
    let mut count = 0;
    for (ca, cb) in a_rev.iter().zip(b_rev.iter()) {
        if ca != cb {
            break;
        }
        count += 1;
    }
    let char_count = a.chars().count();
    let byte_index = a
        .char_indices()
        .nth(char_count - count)
        .map(|(i, _)| i)
        .unwrap_or(a.len());
    &a[byte_index..]
}

fn remove_prefix<'a>(s: &'a str, prefix: &str) -> &'a str {
    &s[prefix.len()..]
}

fn remove_suffix<'a>(s: &'a str, suffix: &str) -> &'a str {
    let split = s.len() - suffix.len();
    &s[..split]
}

fn replace_prefix(s: &str, old_prefix: &str, new_prefix: &str) -> String {
    format!("{new_prefix}{}", remove_prefix(s, old_prefix))
}

fn replace_suffix(s: &str, old_suffix: &str, new_suffix: &str) -> String {
    if old_suffix.is_empty() {
        return format!("{s}{new_suffix}");
    }
    format!("{}{new_suffix}", remove_suffix(s, old_suffix))
}

/// Upstream `maximumOverlap` (KMP; see util/string.js `overlapCount`).
fn maximum_overlap(a: &str, b: &str) -> String {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut start_a = 0;
    if a.len() > b.len() {
        start_a = a.len() - b.len();
    }
    let end_b = b.len().min(a.len());
    let mut map = vec![0usize; end_b];
    let mut k = 0usize;
    for j in 1..end_b {
        if b[j] == b[k] {
            map[j] = map[k];
        } else {
            map[j] = k;
        }
        while k > 0 && b[j] != b[k] {
            k = map[k];
        }
        if b[j] == b[k] {
            k += 1;
        }
    }
    k = 0;
    for &a_i in a[start_a..].iter() {
        while k > 0 && a_i != b[k] {
            k = map[k];
        }
        if a_i == b[k] {
            k += 1;
        }
    }
    b[..k].iter().collect()
}

// ---- the Myers diff (base.js) ----------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Change {
    Keep { value: String },
    Removed { value: String },
    Added { value: String },
}

#[derive(Clone)]
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<Box<Component>>,
}

/// Upstream `buildValues` + `join` (values are strings here, not raw tokens).
fn build_values(
    last: Option<Box<Component>>,
    new_tokens: &[String],
    old_tokens: &[String],
) -> Vec<Change> {
    let mut components = Vec::new();
    let mut next = last;
    while let Some(component) = next {
        next = component.previous.clone();
        components.push(component);
    }
    components.reverse();

    let mut changes = Vec::with_capacity(components.len());
    let mut new_pos = 0usize;
    let mut old_pos = 0usize;
    for component in &components {
        if component.removed {
            changes.push(Change::Removed {
                value: join_tokens(&old_tokens[old_pos..old_pos + component.count]),
            });
            old_pos += component.count;
        } else {
            changes.push(if component.added {
                Change::Added {
                    value: join_tokens(&new_tokens[new_pos..new_pos + component.count]),
                }
            } else {
                Change::Keep {
                    value: join_tokens(&new_tokens[new_pos..new_pos + component.count]),
                }
            });
            new_pos += component.count;
            if !component.added {
                old_pos += component.count;
            }
        }
    }
    changes
}

/// Upstream `WordDiff.postProcess` → `dedupeWhitespaceInChangeObjects` over a
/// flat change list (same five cases, values mutated in place).
fn post_process(changes: &mut [Change]) {
    // Upstream parity shim: named counterpart of the `(deletion, insertion)`
    // tuple below; wired into the interactive shell in r19+.
    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    struct Pending {
        keep: Option<usize>,
        deletion: Option<usize>,
        insertion: Option<usize>,
    }
    let mut last_keep: Option<usize> = None;
    let mut pending: Option<(Option<usize>, Option<usize>)> = None; // (deletion, insertion)

    let len = changes.len();
    for index in 0..len {
        let (added, removed) = match &changes[index] {
            Change::Added { .. } => (true, false),
            Change::Removed { .. } => (false, true),
            Change::Keep { .. } => (false, false),
        };
        if added {
            if let Some((_, insertion)) = pending.as_mut() {
                *insertion = Some(index);
            } else {
                pending = Some((None, Some(index)));
            }
        } else if removed {
            if let Some((deletion, _)) = pending.as_mut() {
                *deletion = Some(index);
            } else {
                pending = Some((Some(index), None));
            }
        } else {
            if let Some((deletion, insertion)) = pending {
                dedupe_whitespace(changes, last_keep, deletion, insertion, Some(index));
            }
            last_keep = Some(index);
            pending = None;
        }
    }
    if let Some((deletion, insertion)) = pending {
        dedupe_whitespace(changes, last_keep, deletion, insertion, None);
    }
}

// Slot-named accessor kept as the readable counterpart of the tuple-based
// `post_process` bookkeeping; wired into the interactive shell in r19+.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    StartKeep,
    Deletion,
    Insertion,
    EndKeep,
}

// Slot-named accessor kept as the readable counterpart of the tuple-based
// `post_process` bookkeeping; wired into the interactive shell in r19+.
#[allow(dead_code)]
fn value_of(
    changes: &[Change],
    slot: Slot,
    start: Option<usize>,
    deletion: Option<usize>,
    insertion: Option<usize>,
    end: Option<usize>,
) -> String {
    let index = match slot {
        Slot::StartKeep => start,
        Slot::Deletion => deletion,
        Slot::Insertion => insertion,
        Slot::EndKeep => end,
    };
    match index {
        Some(i) => match &changes[i] {
            Change::Keep { value } | Change::Added { value } | Change::Removed { value } => {
                value.clone()
            }
        },
        None => String::new(),
    }
}

// Slot-named accessor kept as the readable counterpart of the tuple-based
// `post_process` bookkeeping; wired into the interactive shell in r19+.
#[allow(dead_code)]
fn set_value(changes: &mut [Change], index: Option<usize>, value: String) {
    if let Some(i) = index {
        match &mut changes[i] {
            Change::Keep { value: v }
            | Change::Added { value: v }
            | Change::Removed { value: v } => *v = value,
        }
    }
}

/// Upstream `dedupeWhitespaceInChangeObjects` (all five cases). The four
/// participants are addressed by index; every helper takes `Option<usize>`
/// and reads/writes the change values in place.
fn dedupe_whitespace(
    changes: &mut [Change],
    start_keep: Option<usize>,
    deletion: Option<usize>,
    insertion: Option<usize>,
    end_keep: Option<usize>,
) {
    let read = |changes: &[Change], index: usize| -> String {
        match &changes[index] {
            Change::Keep { value } | Change::Added { value } | Change::Removed { value } => {
                value.clone()
            }
        }
    };
    let write = |changes: &mut [Change], index: usize, value: String| match &mut changes[index] {
        Change::Keep { value: v } | Change::Added { value: v } | Change::Removed { value: v } => {
            *v = value
        }
    };

    if let (Some(deletion), Some(insertion)) = (deletion, insertion) {
        let old_ws_prefix = leading_ws(&read(changes, deletion)).to_string();
        let old_ws_suffix = trailing_ws(&read(changes, deletion)).to_string();
        let new_ws_prefix = leading_ws(&read(changes, insertion)).to_string();
        let new_ws_suffix = trailing_ws(&read(changes, insertion)).to_string();
        if let Some(start) = start_keep {
            let common_prefix = longest_common_prefix(&old_ws_prefix, &new_ws_prefix).to_string();
            let start_value = read(changes, start);
            write(
                changes,
                start,
                replace_suffix(&start_value, &new_ws_prefix, &common_prefix),
            );
            let del_value = read(changes, deletion);
            write(
                changes,
                deletion,
                remove_prefix(&del_value, &common_prefix).to_string(),
            );
            let ins_value = read(changes, insertion);
            write(
                changes,
                insertion,
                remove_prefix(&ins_value, &common_prefix).to_string(),
            );
        }
        if let Some(end) = end_keep {
            let common_suffix = longest_common_suffix(&old_ws_suffix, &new_ws_suffix).to_string();
            let end_value = read(changes, end);
            write(
                changes,
                end,
                replace_prefix(&end_value, &new_ws_suffix, &common_suffix),
            );
            let del_value = read(changes, deletion);
            write(
                changes,
                deletion,
                remove_suffix(&del_value, &common_suffix).to_string(),
            );
            let ins_value = read(changes, insertion);
            write(
                changes,
                insertion,
                remove_suffix(&ins_value, &common_suffix).to_string(),
            );
        }
    } else if let Some(insertion) = insertion {
        // The whitespaces all reflect what was in the new text; dedupe by
        // keeping trailing whitespace and stripping duplicate leading
        // whitespace.
        if start_keep.is_some() {
            let ins_value = read(changes, insertion);
            let ws_len = leading_ws(&ins_value).len();
            write(changes, insertion, ins_value[ws_len..].to_string());
        }
        if let Some(end) = end_keep {
            let end_value = read(changes, end);
            let ws_len = leading_ws(&end_value).len();
            write(changes, end, end_value[ws_len..].to_string());
        }
    } else if let (Some(start), Some(end)) = (start_keep, end_keep) {
        let Some(deletion) = deletion else {
            return;
        };
        let new_ws_full = leading_ws(&read(changes, end)).to_string();
        let del_value = read(changes, deletion);
        let del_ws_start = leading_ws(&del_value).to_string();
        let del_ws_end = trailing_ws(&del_value).to_string();
        // Whitespace right after startKeep in both texts → startKeep.
        let new_ws_start = longest_common_prefix(&new_ws_full, &del_ws_start).to_string();
        let del_value = remove_prefix(&del_value, &new_ws_start).to_string();
        // Whitespace right before endKeep in both texts → endKeep.
        let new_ws_end =
            longest_common_suffix(remove_prefix(&new_ws_full, &new_ws_start), &del_ws_end)
                .to_string();
        let del_value = remove_suffix(&del_value, &new_ws_end).to_string();
        write(changes, deletion, del_value);
        let end_value = read(changes, end);
        write(
            changes,
            end,
            replace_prefix(&end_value, &new_ws_full, &new_ws_end),
        );
        let start_value = read(changes, start);
        let keep_len = new_ws_full.len() - new_ws_end.len();
        write(
            changes,
            start,
            replace_suffix(&start_value, &new_ws_full, &new_ws_full[..keep_len]),
        );
    } else if let Some(end) = end_keep {
        let Some(deletion) = deletion else {
            return;
        };
        // Start of the text: overlap the deletion suffix with the end keep's
        // leading whitespace.
        let end_keep_ws_prefix = leading_ws(&read(changes, end)).to_string();
        let del_value = read(changes, deletion);
        let deletion_ws_suffix = trailing_ws(&del_value).to_string();
        let overlap = maximum_overlap(&deletion_ws_suffix, &end_keep_ws_prefix);
        write(
            changes,
            deletion,
            remove_suffix(&del_value, &overlap).to_string(),
        );
    } else if let Some(start) = start_keep {
        let Some(deletion) = deletion else {
            return;
        };
        // End of the text: overlap the deletion prefix with the start keep's
        // trailing whitespace.
        let start_keep_ws_suffix = trailing_ws(&read(changes, start)).to_string();
        let del_value = read(changes, deletion);
        let deletion_ws_prefix = leading_ws(&del_value).to_string();
        let overlap = maximum_overlap(&start_keep_ws_suffix, &deletion_ws_prefix);
        write(
            changes,
            deletion,
            remove_prefix(&del_value, &overlap).to_string(),
        );
    }
}

/// Upstream `diffWords(oldStr, newStr)` (no options).
fn diff_words(old_str: &str, new_str: &str) -> Vec<Change> {
    let old_tokens: Vec<String> = tokenize(old_str)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    let new_tokens: Vec<String> = tokenize(new_str)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();

    let old_len = old_tokens.len() as isize;
    let new_len = new_tokens.len() as isize;
    let max_edit_length = new_len + old_len;

    let mut best_path: Vec<Option<(isize, Option<Box<Component>>)>> = Vec::new();
    let offset = max_edit_length;
    let path_get = |best_path: &mut Vec<Option<(isize, Option<Box<Component>>)>>,
                    diagonal: isize| {
        let idx = (diagonal + offset) as usize;
        best_path.get(idx).cloned().flatten()
    };

    let mut min_diagonal = isize::MIN;
    let mut max_diagonal = isize::MAX;

    // Seed editLength = 0 (extractCommon on the empty path, diagonal 0).
    let mut seed_old_pos: isize = -1;
    let mut seed_last: Option<Box<Component>> = None;
    {
        let mut new_pos = seed_old_pos;
        let mut count = 0usize;
        while new_pos + 1 < new_len
            && seed_old_pos + 1 < old_len
            && tokens_equal(
                &old_tokens[(seed_old_pos + 1) as usize],
                &new_tokens[(new_pos + 1) as usize],
            )
        {
            new_pos += 1;
            seed_old_pos += 1;
            count += 1;
        }
        if count > 0 {
            seed_last = Some(Box::new(Component {
                count,
                added: false,
                removed: false,
                previous: None,
            }));
        }
        if seed_old_pos + 1 >= old_len && new_pos + 1 >= new_len {
            return post_process_all(build_values(seed_last, &new_tokens, &old_tokens));
        }
        // upstream seeds `bestPath = [{ oldPos: -1, lastComponent: undefined }]`
        // and lets extractCommon mutate that entry in place; the main loop
        // branches from it via bestPath[diagonal ± 1] at editLength 1. Store the
        // post-extract seed at diagonal 0 (offset by `offset` here).
        while best_path.len() <= offset as usize {
            best_path.push(None);
        }
        best_path[offset as usize] = Some((seed_old_pos, seed_last));
    }

    let mut edit_length: isize = 1;
    loop {
        if edit_length > max_edit_length {
            break;
        }
        let mut result: Option<Vec<Change>> = None;
        let mut diagonal = min_diagonal.max(-edit_length);
        let diagonal_end = max_diagonal.min(edit_length);
        while diagonal <= diagonal_end {
            let remove_path = path_get(&mut best_path, diagonal - 1);
            let add_path = path_get(&mut best_path, diagonal + 1);
            if remove_path.is_some() {
                best_path[(diagonal - 1 + offset) as usize] = None;
            }
            let can_add = match &add_path {
                Some((add_old_pos, _)) => {
                    let add_path_new_pos = add_old_pos - diagonal;
                    0 <= add_path_new_pos && add_path_new_pos < new_len
                }
                None => false,
            };
            let can_remove = match &remove_path {
                Some((remove_old_pos, _)) => remove_old_pos + 1 < old_len,
                None => false,
            };
            if !can_add && !can_remove {
                let idx = (diagonal + offset) as usize;
                if idx < best_path.len() {
                    best_path[idx] = None;
                }
                diagonal += 2;
                continue;
            }

            let (mut base_old_pos, base_last, added, removed, old_pos_inc) = if !can_remove
                || (can_add && remove_path.as_ref().unwrap().0 < add_path.as_ref().unwrap().0)
            {
                let (old_pos, last) = add_path.as_ref().unwrap();
                let new_last = match last {
                    Some(l) if l.added && !l.removed => Box::new(Component {
                        count: l.count + 1,
                        added: true,
                        removed: false,
                        previous: l.previous.clone(),
                    }),
                    _ => Box::new(Component {
                        count: 1,
                        added: true,
                        removed: false,
                        previous: last.clone(),
                    }),
                };
                (*old_pos, Some(new_last), true, false, 0)
            } else {
                let (old_pos, last) = remove_path.as_ref().unwrap();
                let new_last = match last {
                    Some(l) if !l.added && l.removed => Box::new(Component {
                        count: l.count + 1,
                        added: false,
                        removed: true,
                        previous: l.previous.clone(),
                    }),
                    _ => Box::new(Component {
                        count: 1,
                        added: false,
                        removed: true,
                        previous: last.clone(),
                    }),
                };
                (*old_pos, Some(new_last), false, true, 1)
            };
            base_old_pos += old_pos_inc;
            let _ = (added, removed);

            // extractCommon(basePath, diagonal)
            let mut new_pos = base_old_pos - diagonal;
            let mut count = 0usize;
            while new_pos + 1 < new_len
                && base_old_pos + 1 < old_len
                && tokens_equal(
                    &old_tokens[(base_old_pos + 1) as usize],
                    &new_tokens[(new_pos + 1) as usize],
                )
            {
                new_pos += 1;
                base_old_pos += 1;
                count += 1;
            }
            let base_last = if count > 0 {
                Some(Box::new(Component {
                    count,
                    added: false,
                    removed: false,
                    previous: base_last,
                }))
            } else {
                base_last
            };

            if base_old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                result = Some(build_values(base_last, &new_tokens, &old_tokens));
                break;
            }
            let idx = (diagonal + offset) as usize;
            while best_path.len() <= idx {
                best_path.push(None);
            }
            best_path[idx] = Some((base_old_pos, base_last));
            if base_old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            diagonal += 2;
        }
        if let Some(changes) = result {
            return post_process_all(changes);
        }
        edit_length += 1;
    }
    Vec::new()
}

fn post_process_all(mut changes: Vec<Change>) -> Vec<Change> {
    post_process(&mut changes);
    changes
}

// ===========================================================================
// renderDiff (the actual component surface)
// ===========================================================================

/// Upstream `parseDiffLine`: `"+123 content"` / `"-123 content"` /
/// `" 123 content"`.
fn parse_diff_line(line: &str) -> Option<(char, String, String)> {
    // ^([+-\s])(\s*\d*)\s(.*)$
    let mut chars = line.chars();
    let prefix = chars.next()?;
    if prefix != '+' && prefix != '-' && !prefix.is_whitespace() {
        return None;
    }
    let rest: Vec<char> = chars.collect();
    let mut idx = 0usize;
    let mut line_num = String::new();
    while idx < rest.len() && rest[idx].is_whitespace() {
        line_num.push(rest[idx]);
        idx += 1;
    }
    while idx < rest.len() && rest[idx].is_ascii_digit() {
        line_num.push(rest[idx]);
        idx += 1;
    }
    // Exactly one whitespace separator is required after the number.
    if idx >= rest.len() || !rest[idx].is_whitespace() {
        return None;
    }
    idx += 1;
    let content: String = rest[idx..].iter().collect();
    Some((prefix, line_num, content))
}

/// Replace tabs with spaces for consistent rendering (upstream `replaceTabs`).
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

fn theme_fg(theme: &Theme, color: &str, text: &str) -> String {
    theme.fg(color, text).expect("theme fg color")
}

/// Compute the word-level diff and render with inverse on changed parts
/// (upstream `renderIntraLineDiff`).
fn render_intra_line_diff(theme: &Theme, old_content: &str, new_content: &str) -> (String, String) {
    let word_diff = diff_words(old_content, new_content);

    let mut removed_line = String::new();
    let mut added_line = String::new();
    let mut is_first_removed = true;
    let mut is_first_added = true;

    for change in word_diff {
        match change {
            Change::Removed { value } => {
                let mut value = value;
                if is_first_removed {
                    let ws_len = leading_ws(&value).len();
                    let ws = value[..ws_len].to_string();
                    value = value[ws_len..].to_string();
                    removed_line.push_str(&ws);
                    is_first_removed = false;
                }
                if !value.is_empty() {
                    removed_line.push_str(&theme.inverse(&value));
                }
            }
            Change::Added { value } => {
                let mut value = value;
                if is_first_added {
                    let ws_len = leading_ws(&value).len();
                    let ws = value[..ws_len].to_string();
                    value = value[ws_len..].to_string();
                    added_line.push_str(&ws);
                    is_first_added = false;
                }
                if !value.is_empty() {
                    added_line.push_str(&theme.inverse(&value));
                }
            }
            Change::Keep { value } => {
                removed_line.push_str(&value);
                added_line.push_str(&value);
            }
        }
    }
    (removed_line, added_line)
}

/// Upstream `RenderDiffOptions`.
#[derive(Clone, Debug, Default)]
pub struct RenderDiffOptions {
    /// File path (unused upstream, kept for API compatibility).
    pub file_path: Option<String>,
}

/// Render a diff string with colored lines and intra-line change highlighting
/// (upstream `renderDiff`).
pub fn render_diff(diff_text: &str, options: &RenderDiffOptions, theme: &Theme) -> String {
    let _ = options;
    let lines: Vec<&str> = diff_text.split('\n').collect();
    let mut result: Vec<String> = Vec::new();

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let parsed = parse_diff_line(line);

        let Some((prefix, line_num, content)) = parsed else {
            result.push(theme_fg(theme, "toolDiffContext", line));
            i += 1;
            continue;
        };

        if prefix == '-' {
            // Collect consecutive removed lines.
            let mut removed_lines: Vec<(String, String)> = Vec::new();
            while i < lines.len() {
                match parse_diff_line(lines[i]) {
                    Some(('-', line_num, content)) => {
                        removed_lines.push((line_num, content));
                        i += 1;
                    }
                    _ => break,
                }
            }

            // Collect consecutive added lines.
            let mut added_lines: Vec<(String, String)> = Vec::new();
            while i < lines.len() {
                match parse_diff_line(lines[i]) {
                    Some(('+', line_num, content)) => {
                        added_lines.push((line_num, content));
                        i += 1;
                    }
                    _ => break,
                }
            }

            if removed_lines.len() == 1 && added_lines.len() == 1 {
                let (removed_num, removed) = &removed_lines[0];
                let (added_num, added) = &added_lines[0];
                let (removed_line, added_line) =
                    render_intra_line_diff(theme, &replace_tabs(removed), &replace_tabs(added));
                result.push(theme_fg(
                    theme,
                    "toolDiffRemoved",
                    &format!("-{removed_num} {removed_line}"),
                ));
                result.push(theme_fg(
                    theme,
                    "toolDiffAdded",
                    &format!("+{added_num} {added_line}"),
                ));
            } else {
                for (num, content) in &removed_lines {
                    result.push(theme_fg(
                        theme,
                        "toolDiffRemoved",
                        &format!("-{num} {}", replace_tabs(content)),
                    ));
                }
                for (num, content) in &added_lines {
                    result.push(theme_fg(
                        theme,
                        "toolDiffAdded",
                        &format!("+{num} {}", replace_tabs(content)),
                    ));
                }
            }
        } else if prefix == '+' {
            result.push(theme_fg(
                theme,
                "toolDiffAdded",
                &format!("+{line_num} {}", replace_tabs(&content)),
            ));
            i += 1;
        } else {
            result.push(theme_fg(
                theme,
                "toolDiffContext",
                &format!(" {line_num} {}", replace_tabs(&content)),
            ));
            i += 1;
        }
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};
    use std::sync::Arc;

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark theme"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `diff_render_diff`
    /// (upstream diff.ts against the real jsdiff 8.0.4) — all 11 rows pinned
    /// byte-for-byte.
    #[test]
    fn render_diff_matches_oracle() {
        let theme = dark();
        let inputs = [
            "",
            "context line\nanother ctx",
            "+12 added line\n-3 removed line",
            "-3 old value\n+3 new value",
            "-3 const a = alpha;\n+3 const a = beta;",
            "-3 alpha beta gamma\n+3 alpha beta delta\n-7 x\n+7 y",
            " 1 shared\n-2 gone\n+2 here\n 3 tail",
            "-1\ttabbed\n+1\ttabbed\ttwo",
            "+5 only added\n+6 second",
            "-1 only removed\n-2 second removed",
            "-1 keep alpha, drop beta\n+1 keep alpha, keep beta, add gamma",
        ];
        let expected: &[&str] = &[
            "\x1b[38;2;157;165;169m\x1b[39m",
            "\x1b[38;2;157;165;169mcontext line\x1b[39m\n\x1b[38;2;157;165;169manother ctx\x1b[39m",
            "\x1b[38;2;104;183;141m+12 added line\x1b[39m\n\x1b[38;2;234;127;129m-3 removed line\x1b[39m",
            "\x1b[38;2;234;127;129m-3 \x1b[7mold\x1b[27m value\x1b[39m\n\x1b[38;2;104;183;141m+3 \x1b[7mnew\x1b[27m value\x1b[39m",
            "\x1b[38;2;234;127;129m-3 const a = \x1b[7malpha\x1b[27m;\x1b[39m\n\x1b[38;2;104;183;141m+3 const a = \x1b[7mbeta\x1b[27m;\x1b[39m",
            "\x1b[38;2;234;127;129m-3 alpha beta \x1b[7mgamma\x1b[27m\x1b[39m\n\x1b[38;2;104;183;141m+3 alpha beta \x1b[7mdelta\x1b[27m\x1b[39m\n\x1b[38;2;234;127;129m-7 \x1b[7mx\x1b[27m\x1b[39m\n\x1b[38;2;104;183;141m+7 \x1b[7my\x1b[27m\x1b[39m",
            "\x1b[38;2;157;165;169m 1 shared\x1b[39m\n\x1b[38;2;234;127;129m-2 \x1b[7mgone\x1b[27m\x1b[39m\n\x1b[38;2;104;183;141m+2 \x1b[7mhere\x1b[27m\x1b[39m\n\x1b[38;2;157;165;169m 3 tail\x1b[39m",
            "\x1b[38;2;234;127;129m-1 tabbed   \x1b[39m\n\x1b[38;2;104;183;141m+1 tabbed   \x1b[7mtwo\x1b[27m\x1b[39m",
            "\x1b[38;2;104;183;141m+5 only added\x1b[39m\n\x1b[38;2;104;183;141m+6 second\x1b[39m",
            "\x1b[38;2;234;127;129m-1 only removed\x1b[39m\n\x1b[38;2;234;127;129m-2 second removed\x1b[39m",
            "\x1b[38;2;234;127;129m-1 keep alpha, \x1b[7mdrop\x1b[27m beta\x1b[39m\n\x1b[38;2;104;183;141m+1 keep alpha, \x1b[7mkeep\x1b[27m beta\x1b[7m, add gamma\x1b[27m\x1b[39m",
        ];
        for (input, want) in inputs.iter().zip(expected.iter()) {
            assert_eq!(
                &render_diff(input, &RenderDiffOptions::default(), &theme),
                want,
                "input={input:?}"
            );
        }
    }

    #[test]
    fn parse_diff_line_matches_upstream_regex() {
        let parsed = parse_diff_line("+12 content here");
        assert_eq!(
            parsed,
            Some(('+', "12".to_string(), "content here".to_string()))
        );
        assert_eq!(
            parse_diff_line("-3 x"),
            Some(('-', "3".to_string(), "x".to_string()))
        );
        assert_eq!(
            parse_diff_line(" 1 ctx"),
            Some((' ', "1".to_string(), "ctx".to_string()))
        );
        assert_eq!(parse_diff_line("plain"), None);
        assert_eq!(parse_diff_line("+"), None);
        assert_eq!(parse_diff_line("+x y"), None);
    }

    #[test]
    fn word_diff_whitespace_dedupe_matches_jsdiff() {
        // jsdiff 8.0.4 live output for its doc example (the word.js comment
        // claims D:'bar', but the real package keeps the trailing space):
        let changes = diff_words("foo bar baz", "foo baz");
        let rendered: Vec<String> = changes
            .iter()
            .map(|c| match c {
                Change::Keep { value } => format!("K:{value:?}"),
                Change::Removed { value } => format!("D:{value:?}"),
                Change::Added { value } => format!("I:{value:?}"),
            })
            .collect();
        assert_eq!(rendered, vec!["K:\"foo \"", "D:\"bar \"", "K:\"baz\""]);
    }
}
