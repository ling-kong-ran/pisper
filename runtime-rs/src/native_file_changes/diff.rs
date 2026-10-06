//! 使用 Pi 与 release 相同的 jsdiff 行差异语义，保留文件头与末尾换行标记。
use pi_rust::coding_agent::core::tools::edit::generate_unified_patch;

pub(crate) const MAX_DIFF_CHARS: usize = 200_000;

pub(crate) fn normalized(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}
pub(crate) fn patch(path: &str, before: &str, after: &str) -> String {
    generate_unified_patch(path, before, after, 4)
}
pub(crate) fn stats(path: &str, before: &str, after: &str) -> (usize, usize) {
    let patch = patch(path, before, after);
    let added = patch
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let removed = patch
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    (added, removed)
}
fn quote(path: &str) -> String {
    if path.chars().any(|value| {
        matches!(
            value,
            '\t' | '\n'
                | '\u{000b}'
                | '\u{000c}'
                | '\r'
                | ' '
                | '\u{00a0}'
                | '\u{1680}'
                | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
                    | '"'
                    | '\\'
        )
    }) {
        serde_json::to_string(path).expect("字符串可序列化")
    } else {
        path.to_owned()
    }
}
pub(crate) fn preview(path: &str, before: &str, after: &str, is_new: bool) -> (String, bool) {
    if before == after {
        return (String::new(), false);
    }
    let old = quote(&format!("a/{path}"));
    let new = quote(&format!("b/{path}"));
    let patch = patch(path, before, after);
    let mut lines = patch.splitn(3, '\n');
    lines.next();
    lines.next();
    let remainder = lines.next().unwrap_or("");
    let output = format!(
        "diff --git {old} {new}\n{}--- {}\n+++ {new}\n{remainder}",
        if is_new { "new file mode 100644\n" } else { "" },
        if is_new { "/dev/null" } else { &old }
    );
    if output.encode_utf16().count() <= MAX_DIFF_CHARS {
        return (output, false);
    }
    // Rust 线协议不输出孤立 UTF-16 代理项；边界最多少一个代理对字符。
    let mut units = 0;
    let end = output
        .char_indices()
        .find_map(|(index, ch)| {
            units += ch.len_utf16();
            (units > MAX_DIFF_CHARS).then_some(index)
        })
        .unwrap_or(output.len());
    (output[..end].to_owned(), true)
}
