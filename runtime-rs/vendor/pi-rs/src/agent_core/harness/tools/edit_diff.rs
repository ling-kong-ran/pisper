//! Upstream tools/edit-diff.ts matching and line-preserving replacements.
//! Internal offsets are UTF-8 byte offsets (never exposed in tool results).
pub use super::line_diff::{generate_diff_string, generate_unified_patch, DisplayDiff};
use icu_normalizer::ComposingNormalizerBorrowed;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEditsResult {
    pub base_content: String,
    pub new_content: String,
}
pub fn detect_line_ending(content: &str) -> &'static str {
    match (content.find("\r\n"), content.find('\n')) {
        (Some(crlf), Some(lf)) if crlf < lf => "\r\n",
        _ => "\n",
    }
}
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}
pub fn strip_bom(content: &str) -> (&str, &str) {
    if let Some(text) = content.strip_prefix('\u{feff}') {
        ("\u{feff}", text)
    } else {
        ("", content)
    }
}
fn js_whitespace(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
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
struct Match {
    index: usize,
    len: usize,
    fuzzy: bool,
}
fn find_text(content: &str, old: &str) -> Option<Match> {
    if let Some(index) = content.find(old) {
        return Some(Match {
            index,
            len: old.len(),
            fuzzy: false,
        });
    }
    let content = normalize_for_fuzzy_match(content);
    let old = normalize_for_fuzzy_match(old);
    content.find(&old).map(|index| Match {
        index,
        len: old.len(),
        fuzzy: true,
    })
}
fn occurrences(content: &str, old: &str) -> usize {
    let content = normalize_for_fuzzy_match(content);
    let old = normalize_for_fuzzy_match(old);
    if old.is_empty() {
        content.encode_utf16().count().saturating_sub(1)
    } else {
        content.matches(&old).count()
    }
}
#[derive(Clone)]
struct Replacement {
    edit_index: usize,
    index: usize,
    len: usize,
    text: String,
}
fn replace(content: &str, replacements: &[Replacement], offset: usize) -> String {
    let mut result = content.to_string();
    for item in replacements.iter().rev() {
        let start = item.index - offset;
        result.replace_range(start..start + item.len, &item.text);
    }
    result
}
fn preserve_lines(
    original: &str,
    base: &str,
    replacements: &[Replacement],
) -> anyhow::Result<String> {
    let original: Vec<_> = original.split_inclusive('\n').collect();
    let mut offset = 0;
    let spans: Vec<_> = base
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, offset)
        })
        .collect();
    anyhow::ensure!(
        original.len() == spans.len(),
        "Cannot preserve unchanged lines because the base content has a different line count."
    );
    let mut groups: Vec<(usize, usize, Vec<Replacement>)> = Vec::new();
    for item in replacements {
        let start = spans
            .iter()
            .position(|(start, end)| item.index >= *start && item.index < *end)
            .ok_or_else(|| anyhow::anyhow!("Replacement range is outside the base content."))?;
        let mut end = start;
        while end < spans.len() && spans[end].1 < item.index + item.len {
            end += 1;
        }
        anyhow::ensure!(
            end < spans.len(),
            "Replacement range is outside the base content."
        );
        let end = end + 1;
        if let Some(group) = groups.last_mut().filter(|group| start < group.1) {
            group.1 = group.1.max(end);
            group.2.push(item.clone());
        } else {
            groups.push((start, end, vec![item.clone()]));
        }
    }
    let mut output = String::new();
    let mut line = 0;
    for (start, end, items) in groups {
        output.push_str(&original[line..start].concat());
        let from = spans[start].0;
        let to = spans[end - 1].1;
        output.push_str(&replace(&base[from..to], &items, from));
        line = end;
    }
    output.push_str(&original[line..].concat());
    Ok(output)
}
pub fn apply_edits_to_normalized_content(
    content: &str,
    edits: &[Edit],
    path: &str,
) -> anyhow::Result<AppliedEditsResult> {
    let edits: Vec<_> = edits
        .iter()
        .map(|e| Edit {
            old_text: normalize_to_lf(&e.old_text),
            new_text: normalize_to_lf(&e.new_text),
        })
        .collect();
    for (i, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            if edits.len() == 1 {
                anyhow::bail!("oldText must not be empty in {path}.");
            }
            anyhow::bail!("edits[{i}].oldText must not be empty in {path}.");
        }
    }
    let fuzzy = edits
        .iter()
        .any(|e| find_text(content, &e.old_text).is_some_and(|m| m.fuzzy));
    let base = if fuzzy {
        normalize_for_fuzzy_match(content)
    } else {
        content.to_string()
    };
    let mut replacements = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let Some(found) = find_text(&base, &edit.old_text) else {
            if edits.len() == 1 {
                anyhow::bail!("Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.");
            }
            anyhow::bail!("Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines.");
        };
        let count = occurrences(&base, &edit.old_text);
        if count > 1 {
            if edits.len() == 1 {
                anyhow::bail!("Found {count} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.");
            }
            anyhow::bail!("Found {count} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique.");
        }
        replacements.push(Replacement {
            edit_index: i,
            index: found.index,
            len: found.len,
            text: edit.new_text.clone(),
        });
    }
    replacements.sort_by_key(|r| r.index);
    for pair in replacements.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        anyhow::ensure!(previous.index+previous.len<=current.index,"edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",previous.edit_index,current.edit_index);
    }
    let new = if fuzzy {
        preserve_lines(content, &base, &replacements)?
    } else {
        replace(&base, &replacements, 0)
    };
    if content == new {
        if edits.len() == 1 {
            anyhow::bail!("No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.");
        }
        anyhow::bail!("No changes made to {path}. The replacements produced identical content.");
    }
    Ok(AppliedEditsResult {
        base_content: content.to_string(),
        new_content: new,
    })
}
