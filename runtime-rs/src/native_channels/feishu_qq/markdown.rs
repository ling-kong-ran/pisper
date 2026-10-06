//! SDK 1.72.0 post markdown preprocessing and UTF-16 chunk boundaries.
/// Rust's regex engine has no backreferences. Match the SDK's greedy opening
/// fence and lazy matching closing fence explicitly, including shorter closing
/// fences (the opening run may backtrack into the language suffix).
pub(super) fn optimize_markdown(value: &str) -> String {
    let mut protected = String::new();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let remaining = &value[cursor..];
        let mut matched = None;
        for (offset, _) in remaining.match_indices("```") {
            let start = cursor + offset;
            if start != 0 && value.as_bytes()[start - 1] != b'\n' {
                continue;
            }
            let run = value.as_bytes()[start..]
                .iter()
                .take_while(|b| **b == b'`')
                .count();
            let Some(newline) = value[start..].find('\n').map(|at| start + at) else {
                continue;
            };
            let mut closing = None;
            for (offset, _) in value[newline + 1..].match_indices('\n') {
                let close_start = newline + 1 + offset + 1;
                let width = value.as_bytes()[close_start..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                let end = close_start + width;
                if (3..=run).contains(&width)
                    && (end == value.len() || value.as_bytes()[end] == b'\n')
                    && closing.is_none_or(|(best, _)| width > best)
                {
                    closing = Some((width, end));
                }
            }
            if let Some((_, end)) = closing {
                matched = Some((start, end));
                break;
            }
        }
        let Some((start, end)) = matched else {
            protected.push_str(&value[cursor..]);
            break;
        };
        protected.push_str(&value[cursor..start]);
        protected.push_str(&format!("___CB_{}___", blocks.len()));
        blocks.push(&value[start..end]);
        cursor = end;
    }
    // JS multiline anchors also treat CR and Unicode line separators as ends.
    // Keeping separators intact avoids normalizing CRLF or trimming a final LF.
    let demote = value
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .any(|line| (1..=3).any(|count| line.starts_with(&format!("{} ", "#".repeat(count)))));
    if demote {
        let mut transformed = String::new();
        for line in protected.split_inclusive(['\n', '\r', '\u{2028}', '\u{2029}']) {
            let body = line.trim_end_matches(['\n', '\r', '\u{2028}', '\u{2029}']);
            let heading = body.bytes().take_while(|byte| *byte == b'#').count();
            if (1..=6).contains(&heading)
                && body.as_bytes().get(heading) == Some(&b' ')
                && body.len() > heading + 1
            {
                transformed.push_str(if heading == 1 { "#### " } else { "##### " });
                transformed.push_str(&line[heading + 1..]);
            } else {
                transformed.push_str(line);
            }
        }
        protected = transformed;
    }
    for (index, block) in blocks.into_iter().enumerate() {
        protected = protected.replacen(&format!("___CB_{index}___"), block, 1);
    }
    regex::Regex::new(r"\n{3,}")
        .unwrap()
        .replace_all(&protected, "\n\n")
        .into_owned()
}

pub(super) fn split_markdown(value: &str, limit: usize) -> Vec<String> {
    if value.encode_utf16().count() <= limit {
        return vec![value.into()];
    }
    let mut out = Vec::new();
    let mut lines = Vec::<String>::new();
    let mut length = 0;
    let mut fence: Option<String> = None;
    let heading = regex::Regex::new(r"^#{1,6}[\t\n\x0B\x0C\r \u{00A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]").unwrap();
    for line in value.split('\n') {
        // The SDK computes this before flushing/reopening a fence.
        let size = line.encode_utf16().count() + usize::from(!lines.is_empty());
        if length + size > limit
            || (heading.is_match(line) && length as f64 > limit as f64 * 0.75 && !lines.is_empty())
        {
            flush(&mut out, &mut lines, &mut length, &fence);
        }
        lines.push(line.into());
        length += size;
        // This SDK regex has no multiline flag: a final CR is not a fence.
        if let Some(language) = line
            .strip_prefix("```")
            .filter(|v| v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            fence = if fence.is_some() {
                None
            } else {
                Some(language.into())
            };
        }
    }
    flush(&mut out, &mut lines, &mut length, &fence);
    out
}
fn flush(
    out: &mut Vec<String>,
    lines: &mut Vec<String>,
    length: &mut usize,
    fence: &Option<String>,
) {
    if lines.is_empty() {
        return;
    }
    let mut chunk = lines.join("\n");
    if fence.is_some() {
        chunk.push_str("\n```");
    }
    out.push(chunk);
    lines.clear();
    *length = 0;
    if let Some(language) = fence {
        lines.push(format!("```{language}"));
        *length = lines[0].encode_utf16().count();
    }
}
