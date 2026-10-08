//! Port of upstream `packages/tui/src/components/markdown.ts` — the Markdown
//! component: token rendering to ANSI terminal lines with heading/list/
//! blockquote/table/code/latex support, wrap + padding + background stages,
//! render caching and the streamed-partial-fence trim. The parser is the
//! marked 18.0.5 lexer port in [`crate::tui::markdown_lexer`] extended with
//! the upstream latex tokenizers and the strict strikethrough override.

use std::sync::Arc;

use crate::tui::component::Component;
use crate::tui::latex::{render_latex_utf16, RenderLatexOptions};
use crate::tui::markdown_lexer::{is_js_space, Lexer, LexerExtensions, Token};
use crate::tui::terminal_image::get_capabilities;
use crate::tui::utf16::{raw_text, Utf16Text};
use crate::tui::utils::{visible_width_utf16, wrap_text_with_ansi_utf16 as wrap_text_with_ansi};

/// Styling callbacks receive real UTF-16 units, including lone surrogates from
/// LaTeX. Convert to UTF-8 only intentionally; no surrogate sentinel is used.
pub type StyleFn = Arc<dyn Fn(&Utf16Text) -> Utf16Text + Send + Sync>;
pub type HighlightCodeFn = Arc<dyn Fn(&str, Option<&str>) -> Vec<String> + Send + Sync>;
pub type TransformFn = Arc<dyn Fn(&str, usize) -> String + Send + Sync>;

/// Upstream `MarkdownTheme`.
#[derive(Clone)]
pub struct MarkdownTheme {
    pub heading: StyleFn,
    pub link: StyleFn,
    pub link_url: StyleFn,
    pub code: StyleFn,
    pub code_block: StyleFn,
    pub code_block_border: StyleFn,
    pub quote: StyleFn,
    pub quote_border: StyleFn,
    pub hr: StyleFn,
    pub list_bullet: StyleFn,
    pub bold: StyleFn,
    pub italic: StyleFn,
    pub strikethrough: StyleFn,
    pub underline: StyleFn,
    /// Upstream `highlightCode` (optional syntax highlighting hook).
    pub highlight_code: Option<HighlightCodeFn>,
    /// Upstream `codeBlockIndent` (default `"  "`).
    pub code_block_indent: Option<String>,
}

impl std::fmt::Debug for MarkdownTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MarkdownTheme")
    }
}

/// Upstream `DefaultTextStyle`.
#[derive(Clone, Default)]
pub struct DefaultTextStyle {
    pub color: Option<StyleFn>,
    pub bg_color: Option<StyleFn>,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}

/// Upstream `MarkdownOptions`.
#[derive(Clone, Default)]
pub struct MarkdownOptions {
    pub preserve_ordered_list_markers: bool,
    pub preserve_backslash_escapes: bool,
    pub transform: Option<TransformFn>,
    pub render_latex: bool,
}

/// Upstream `InlineStyleContext`.
#[derive(Clone)]
struct InlineStyleContext {
    apply_text: StyleFn,
    style_prefix: Utf16Text,
}

fn identity_style(t: &Utf16Text) -> Utf16Text {
    t.clone()
}

// ---------------------------------------------------------------------------
// Lexer extensions (latex tokenizers + strict strikethrough)
// ---------------------------------------------------------------------------

/// Upstream `looksLikePendingDollarMath`.
fn looks_like_pending_dollar_math(source: &str) -> bool {
    // /\\[A-Za-z]+|[_^=+*/<>()[\]|±≤≥≠≈∈→⇒∞∫∑√-]/
    if source
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'\\' && pair[1].is_ascii_alphabetic())
    {
        return true;
    }
    source.chars().any(|c| {
        matches!(
            c,
            '_' | '^'
                | '='
                | '+'
                | '*'
                | '/'
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '|'
                | '±'
                | '≤'
                | '≥'
                | '≠'
                | '≈'
                | '∈'
                | '→'
                | '⇒'
                | '∞'
                | '∫'
                | '∑'
                | '√'
                | '-'
        )
    })
}

fn is_escaped(source: &str, index: usize) -> bool {
    let bytes = source.as_bytes();
    let mut backslashes = 0;
    let mut position = index;
    while position > 0 && bytes[position - 1] == b'\\' {
        backslashes += 1;
        position -= 1;
    }
    backslashes % 2 == 1
}

fn find_closing_delimiter(source: &str, closing: &str, start: usize) -> i64 {
    if start > source.len() {
        return -1;
    }
    let mut index = source[start..].find(closing).map(|p| p + start);
    while let Some(idx) = index {
        if is_escaped(source, idx) {
            let next = idx + closing.len();
            if next > source.len() {
                return -1;
            }
            index = source[next..].find(closing).map(|p| p + next);
        } else {
            return idx as i64;
        }
    }
    -1
}

fn regexp_const_pattern(inner: &str) -> bool {
    // /^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9_\s])?$/
    let mut chars = inner.chars();
    let c0 = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !(c0.is_ascii_uppercase() || c0 == '_') {
        return false;
    }
    let mut rest = chars.skip_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_');
    match rest.next() {
        None => true,
        Some(c) => {
            // The upstream regex has no /u flag: its optional final atom
            // matches one UTF-16 code unit, not an astral scalar.
            c.len_utf16() == 1
                && !(c.is_ascii_alphanumeric() || c == '_' || is_js_space(c))
                && rest.next().is_none()
        }
    }
}

/// Upstream `tokenizeInlineLatex`.
fn tokenize_inline_latex(source: &str) -> Option<Token> {
    let (opening, closing): (&str, &str) = if source.starts_with("$$") {
        ("$$", "$$")
    } else if source.starts_with("\\(") {
        ("\\(", "\\)")
    } else if source.starts_with("\\[") {
        ("\\[", "\\]")
    } else if source.starts_with('$') && source[1..].chars().next().is_none_or(|c| !is_js_space(c))
    {
        ("$", "$")
    } else {
        return None;
    };
    let opening_len = opening.len();
    let closing_index = find_closing_delimiter(source, closing, opening_len);
    if closing_index >= 0 && opening == "$" {
        let ci = closing_index as usize;
        let inner = &source[opening_len..ci];
        let after = &source[ci + closing.len()..];
        let inner_ends_space = inner.chars().last().is_some_and(is_js_space);
        let after_starts_digit = after.chars().next().is_some_and(|c| c.is_ascii_digit());
        let inner_is_const = regexp_const_pattern(inner);
        let after_is_ident = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        let inner_has_backtick = inner.contains('`');
        if inner_ends_space
            || after_starts_digit
            || (inner_is_const && after_is_ident)
            || inner_has_backtick
        {
            return None;
        }
    }
    if closing_index < 0 {
        let pending_source = &source[opening_len..];
        if opening.starts_with('\\') || looks_like_pending_dollar_math(pending_source) {
            let mut t = Token::new("latex", source.to_string());
            t.text = pending_source.to_string();
            t.pending = true;
            return Some(t);
        }
        return None;
    }
    let ci = closing_index as usize;
    let text = &source[opening_len..ci];
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let raw = source[..ci + closing.len()].to_string();
    let mut t = Token::new("latex", raw);
    t.text = text.to_string();
    Some(t)
}

/// Tail after a block delimiter: [ \t]*(?:\n)? — returns (ws_len, consumed).
fn block_lead_ws(after: &str) -> (usize, bool) {
    let ws = after.chars().take_while(|&c| c == ' ' || c == '\t').count();
    if after[ws..].starts_with('\n') {
        (ws, true)
    } else {
        (ws, false)
    }
}

/// Upstream `tokenizeBlockLatex`.
fn tokenize_block_latex(source: &str) -> Option<Token> {
    let lead = source.chars().take_while(|&c| c == ' ').count().min(3);
    let rest = &source[lead..];
    if let Some(after_dollars) = rest.strip_prefix("$$") {
        let (ws, nl) = block_lead_ws(after_dollars);
        let text_start = lead + 2 + ws + if nl { 1 } else { 0 };
        let closing = find_closing_delimiter(source, "$$", text_start);
        if closing >= 0 {
            let ci = closing as usize;
            let text = &source[text_start..ci];
            // tail: [ \t]*(?:\n|$)
            let tail = &source[ci + 2..];
            let tws = tail.chars().take_while(|&c| c == ' ' || c == '\t').count();
            let tail_rest = &tail[tws..];
            if !text.is_empty() && (tail_rest.is_empty() || tail_rest.starts_with('\n')) {
                let raw_len = if tail_rest.starts_with('\n') {
                    ci + 2 + tws + 1
                } else {
                    ci + 2 + tws
                };
                let mut t = Token::new(
                    "latexBlock",
                    source[..raw_len.min(source.len())].to_string(),
                );
                t.text = text.trim_matches(is_js_space).to_string();
                return Some(t);
            }
        }
        // pending: /^ {0,3}\$\$[ \t]*(?:\n)?([\s\S]*)$/ + looksLike
        if looks_like_pending_dollar_math(&source[text_start..]) {
            let mut t = Token::new("latexBlock", source.to_string());
            t.text = source[text_start..].to_string();
            t.pending = true;
            return Some(t);
        }
        return None;
    }
    if let Some(after_bracket) = rest.strip_prefix("\\[") {
        let (ws, nl) = block_lead_ws(after_bracket);
        let text_start = lead + 2 + ws + if nl { 1 } else { 0 };
        let closing2 = find_closing_delimiter(source, "\\]", text_start);
        if closing2 >= 0 {
            let ci = closing2 as usize;
            let text = &source[text_start..ci];
            if !text.is_empty() {
                let tail = &source[ci + 2..];
                let tws = tail.chars().take_while(|&c| c == ' ' || c == '\t').count();
                let tail_rest = &tail[tws..];
                if tail_rest.is_empty() || tail_rest.starts_with('\n') {
                    let raw_len = if tail_rest.starts_with('\n') {
                        ci + 2 + tws + 1
                    } else {
                        ci + 2 + tws
                    };
                    let mut t = Token::new(
                        "latexBlock",
                        source[..raw_len.min(source.len())].to_string(),
                    );
                    t.text = text.trim_matches(is_js_space).to_string();
                    return Some(t);
                }
            }
        }
        // pending bracket: always pending
        let mut t = Token::new("latexBlock", source.to_string());
        t.text = source[text_start..].to_string();
        t.pending = true;
        return Some(t);
    }
    None
}

fn latex_block_start(src: &str) -> Option<usize> {
    // /(?:^|\n) {0,3}(?:\$\$|\\\[)/ — returns the line-start index
    let mut offset = 0usize;
    loop {
        let line_end = src[offset..].find('\n').map(|p| offset + p);
        let line = match line_end {
            Some(e) => &src[offset..e],
            None => &src[offset..],
        };
        let lead = line.chars().take_while(|&c| c == ' ').count().min(3);
        if line[lead..].starts_with("$$") || line[lead..].starts_with("\\[") {
            return Some(offset);
        }
        offset = line_end? + 1;
    }
}

fn latex_inline_start(src: &str) -> Option<usize> {
    let a = src.find('$');
    let b = src.find("\\(");
    let c = src.find("\\[");
    [a, b, c].into_iter().flatten().min()
}

/// Upstream `StrictStrikethroughTokenizer.del`:
/// /^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)/
fn strict_strikethrough_del(
    lexer: &mut Lexer,
    src: &str,
    ext: &dyn LexerExtensions,
) -> Option<Token> {
    if !src.starts_with("~~") {
        return None;
    }
    let after = &src[2..];
    match after.chars().next() {
        Some(c) if !is_js_space(c) && c != '~' => {}
        _ => return None,
    }
    let bytes = after.as_bytes();
    let mut end = 0usize;
    loop {
        if end > bytes.len() {
            return None;
        }
        if end > 0 {
            // final unit: \\. (pair) or [^\s~\\]
            let final_ok = if end >= 2 && bytes[end - 2] == b'\\' {
                true // escape pair — any char
            } else {
                let c = after[..end].chars().next_back().unwrap();
                c != '~' && !is_js_space(c) && c != '\\'
            };
            if final_ok && after[end..].starts_with("~~") {
                let tail = &after[end + 2..];
                let tail_ok = tail.chars().next().is_none_or(|c| c != '~');
                if tail_ok {
                    let raw = src[..2 + end + 2].to_string();
                    let mut t = Token::new("del", raw);
                    t.text = after[..end].to_string();
                    let mut inner = Vec::new();
                    lexer.inline_tokens(&t.text, &mut inner, ext);
                    t.tokens = inner;
                    return Some(t);
                }
            }
        }
        if end >= bytes.len() {
            return None;
        }
        if bytes[end] == b'\\' && end + 1 < bytes.len() {
            end += 2;
        } else {
            end += 1;
        }
    }
}

/// The markdown component's lexer extension set.
pub struct MarkdownExtensions;
impl LexerExtensions for MarkdownExtensions {
    fn block_tokenizer(&self, _lexer: &mut Lexer, src: &str) -> Option<Token> {
        tokenize_block_latex(src)
    }
    fn block_start(&self, src: &str) -> Option<usize> {
        latex_block_start(src)
    }
    fn inline_tokenizer(&self, _lexer: &mut Lexer, src: &str) -> Option<Token> {
        tokenize_inline_latex(src)
    }
    fn inline_tokenizer_with_tail(
        &self,
        _lexer: &mut Lexer,
        src: &str,
        tail: Option<u16>,
    ) -> Option<Token> {
        let mut token = tokenize_inline_latex(src)?;
        // The only malformed internal suffix is a lone high surrogate. It is
        // neither whitespace nor a delimiter; pending math includes it verbatim.
        if token.pending {
            if let Some(high) = tail {
                let unit = Utf16Text::from_units(vec![high]);
                let mut raw = token.raw_units();
                raw.push(&unit);
                token.set_raw_units(raw);
                let mut text = token.text_units();
                text.push(unit);
                token.set_text_units(text);
            }
        }
        Some(token)
    }
    fn inline_start(&self, src: &str) -> Option<usize> {
        latex_inline_start(src)
    }
    fn del(&self, lexer: &mut Lexer, src: &str) -> Option<Token> {
        let me: &dyn LexerExtensions = self;
        strict_strikethrough_del(lexer, src, me)
    }
}

/// Upstream `trimPartialClosingFences`.
fn trim_partial_closing_fences(tokens: &mut [Token]) {
    let Some(token) = tokens.last_mut() else {
        return;
    };
    match token.kind.as_str() {
        "list" => {
            if let Some(item) = token.items.last_mut() {
                trim_partial_closing_fences(&mut item.tokens);
            }
            return;
        }
        "blockquote" => {
            trim_partial_closing_fences(&mut token.tokens);
            return;
        }
        "code" => {}
        _ => return,
    }
    let raw = token.raw.clone();
    let fence_char = raw.chars().next();
    let marker_len = if matches!(fence_char, Some('`') | Some('~')) {
        raw.chars()
            .take_while(|&c| c == fence_char.unwrap())
            .count()
    } else {
        0
    };
    let (marker, last_line) = if marker_len >= 3 {
        (
            Some((fence_char.unwrap(), marker_len)),
            raw.split('\n').next_back().unwrap_or("").to_string(),
        )
    } else {
        (None, String::new())
    };
    if let Some((fence_char, marker_len)) = marker {
        if last_line.is_empty()
            || last_line.chars().count() >= marker_len
            || last_line.chars().any(|c| c != fence_char)
        {
            return;
        }
        let last_len = last_line.chars().count();
        let text_chars = token.text.chars().count();
        let mut text: String = token.text.chars().take(text_chars - last_len).collect();
        if text.ends_with('\n') {
            text.pop();
        }
        token.text = text;
    }
}

/// Composite default-style applier (upstream `applyDefaultStyle`).
#[derive(Clone)]
struct StyleEngine {
    theme: Arc<MarkdownTheme>,
    style: Option<DefaultTextStyle>,
}

impl StyleEngine {
    fn apply(&self, text: &Utf16Text) -> Utf16Text {
        let Some(style) = &self.style else {
            return text.clone();
        };
        let mut styled = text.clone();
        if let Some(color) = &style.color {
            styled = color(&styled);
        }
        if style.bold {
            styled = (self.theme.bold)(&styled);
        }
        if style.italic {
            styled = (self.theme.italic)(&styled);
        }
        if style.strikethrough {
            styled = (self.theme.strikethrough)(&styled);
        }
        if style.underline {
            styled = (self.theme.underline)(&styled);
        }
        styled
    }
}

/// Upstream `Markdown`.
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<DefaultTextStyle>,
    theme: Arc<MarkdownTheme>,
    options: MarkdownOptions,

    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<Utf16Text>>,
    /// Parsed tokens depend only on the source, so they survive theme and
    /// width invalidation.
    cached_tokens: Option<(String, Vec<Token>)>,
}

impl Markdown {
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<DefaultTextStyle>,
        options: Option<MarkdownOptions>,
    ) -> Self {
        Markdown {
            text: text.into(),
            padding_x,
            padding_y,
            default_text_style,
            theme: Arc::new(theme),
            options: options.unwrap_or(MarkdownOptions {
                preserve_ordered_list_markers: false,
                preserve_backslash_escapes: false,
                transform: None,
                render_latex: true,
            }),
            cached_text: None,
            cached_width: None,
            cached_lines: None,
            cached_tokens: None,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }

    pub fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    fn default_context(&self) -> InlineStyleContext {
        let engine = StyleEngine {
            theme: Arc::clone(&self.theme),
            style: self.default_text_style.clone(),
        };
        let prefix = {
            let sentinel = '\u{0}';
            let styled = engine.apply(&Utf16Text::from(sentinel));
            match styled.find(sentinel) {
                Some(idx) => styled.slice(..idx),
                None => Utf16Text::new(),
            }
        };
        InlineStyleContext {
            apply_text: Arc::new(move |t: &Utf16Text| engine.apply(t)),
            style_prefix: prefix,
        }
    }

    fn get_style_prefix(&self, f: &dyn Fn(&Utf16Text) -> Utf16Text) -> Utf16Text {
        let sentinel = '\u{0}';
        let styled = f(&Utf16Text::from(sentinel));
        match styled.find(sentinel) {
            Some(idx) => styled.slice(..idx),
            None => Utf16Text::new(),
        }
    }

    /// Encode only after Markdown wrapping, margins, tables and backgrounds.
    pub fn render(&mut self, width: usize) -> Vec<String> {
        self.render_utf16(width)
            .iter()
            .map(Utf16Text::to_string_lossy)
            .collect()
    }

    /// Lossless rendered lines, suitable for another raw-unit layout stage.
    pub fn render_utf16(&mut self, width: usize) -> Vec<Utf16Text> {
        if let (Some(lines), Some(text), Some(w)) =
            (&self.cached_lines, &self.cached_text, self.cached_width)
        {
            if *text == self.text && w == width {
                return lines.clone();
            }
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let text = match &self.options.transform {
            Some(f) => f(&self.text, content_width),
            None => self.text.clone(),
        };

        if text.trim_matches(is_js_space).is_empty() {
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Some(Vec::new());
            return Vec::new();
        }

        let normalized_text = text.replace('\t', "   ");
        // Parsed tokens depend only on the source, so they survive theme and
        // width invalidation.
        let cached_tokens = self
            .cached_tokens
            .as_ref()
            .filter(|(source, _)| source == &normalized_text)
            .map(|(_, tokens)| tokens.clone());
        let mut tokens = match cached_tokens {
            Some(tokens) => tokens,
            None => {
                let mut lexer = Lexer::new();
                let mut tokens = lexer.lex(&normalized_text, &MarkdownExtensions);
                trim_partial_closing_fences(&mut tokens);
                self.cached_tokens = Some((normalized_text.clone(), tokens.clone()));
                tokens
            }
        };

        let mut rendered_lines: Vec<Utf16Text> = Vec::new();
        for i in 0..tokens.len() {
            let next_type = tokens.get(i + 1).map(|t| t.kind.clone());
            let token_lines =
                self.render_token(&mut tokens[i], content_width, next_type.as_deref(), None);
            rendered_lines.extend(token_lines);
        }

        let mut wrapped_lines: Vec<Utf16Text> = Vec::new();
        for line in &rendered_lines {
            if is_image_line(line) {
                wrapped_lines.push(line.clone());
            } else {
                wrapped_lines.extend(wrap_text_with_ansi(line, content_width));
            }
        }

        let left_margin = " ".repeat(self.padding_x);
        let right_margin = " ".repeat(self.padding_x);
        let bg_fn = self
            .default_text_style
            .as_ref()
            .and_then(|s| s.bg_color.clone());
        let mut content_lines: Vec<Utf16Text> = Vec::new();
        for line in &wrapped_lines {
            if is_image_line(line) {
                content_lines.push(line.clone());
                continue;
            }
            let line_with_margins = raw_text!(&left_margin, line, &right_margin);
            if let Some(bg) = &bg_fn {
                content_lines.push(apply_background_to_line(&line_with_margins, width, |t| {
                    bg(t)
                }));
            } else {
                let visible_len = visible_width(&line_with_margins);
                let padding_needed = width.saturating_sub(visible_len);
                content_lines.push(raw_text!(line_with_margins, " ".repeat(padding_needed)));
            }
        }

        let empty_line = Utf16Text::from(" ".repeat(width));
        let mut empty_lines: Vec<Utf16Text> = Vec::new();
        for _ in 0..self.padding_y {
            let line = match &bg_fn {
                Some(bg) => apply_background_to_line(&empty_line, width, |t| bg(t)),
                None => empty_line.clone(),
            };
            empty_lines.push(line);
        }

        let mut result = empty_lines.clone();
        result.extend(content_lines);
        result.extend(empty_lines);

        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = Some(result.clone());

        if result.is_empty() {
            vec![Utf16Text::new()]
        } else {
            result
        }
    }

    fn render_token(
        &mut self,
        token: &mut Token,
        width: usize,
        next_token_type: Option<&str>,
        style_context: Option<InlineStyleContext>,
    ) -> Vec<Utf16Text> {
        let mut lines: Vec<Utf16Text> = Vec::new();
        match token.kind.as_str() {
            "heading" => {
                let heading_level = token.depth;
                let heading_prefix = format!("{} ", "#".repeat(heading_level));
                let heading_fn: StyleFn = if heading_level == 1 {
                    let bold = Arc::clone(&self.theme.bold);
                    let underline = Arc::clone(&self.theme.underline);
                    let heading = Arc::clone(&self.theme.heading);
                    Arc::new(move |text: &Utf16Text| heading(&bold(&underline(text))))
                } else {
                    let bold = Arc::clone(&self.theme.bold);
                    let heading = Arc::clone(&self.theme.heading);
                    Arc::new(move |text: &Utf16Text| heading(&bold(text)))
                };
                let style_prefix = self.get_style_prefix(heading_fn.as_ref());
                let heading_style_context = InlineStyleContext {
                    apply_text: heading_fn.clone(),
                    style_prefix,
                };
                let heading_text =
                    self.render_inline_tokens(&mut token.tokens, &heading_style_context);
                let styled_heading = if heading_level >= 3 {
                    raw_text!(heading_fn(&Utf16Text::from(heading_prefix)), heading_text)
                } else {
                    heading_text
                };
                lines.push(styled_heading);
                if let Some(nt) = next_token_type {
                    if nt != "space" {
                        lines.push(Utf16Text::new());
                    }
                }
            }
            "paragraph" => {
                let fallback_ctx;
                let ctx_ref = match &style_context {
                    Some(c) => c,
                    None => {
                        fallback_ctx = self.default_context();
                        &fallback_ctx
                    }
                };
                let paragraph_text = self.render_inline_tokens(&mut token.tokens, ctx_ref);
                lines.push(paragraph_text);
                if let Some(nt) = next_token_type {
                    if nt != "list" && nt != "space" {
                        lines.push(Utf16Text::new());
                    }
                }
            }
            "text" => {
                let fallback_ctx;
                let ctx_ref = match &style_context {
                    Some(c) => c,
                    None => {
                        fallback_ctx = self.default_context();
                        &fallback_ctx
                    }
                };
                let mut single = std::mem::take(&mut token.tokens);
                let text_line = self.render_inline_tokens(&mut single, ctx_ref);
                token.tokens = single;
                lines.push(text_line);
            }
            "latexBlock" => {
                let rendered = if !token.pending && self.options.render_latex {
                    render_latex_utf16(
                        &token.text.encode_utf16().collect::<Vec<_>>(),
                        RenderLatexOptions { display: true },
                    )
                    .map(Utf16Text::from_units)
                    .unwrap_or_else(|| Utf16Text::from(token.raw.trim_matches(is_js_space)))
                } else {
                    Utf16Text::from(token.raw.trim_matches(is_js_space))
                };
                for line in rendered.split('\n') {
                    lines.push(self.apply_default_style(&line));
                }
                if let Some(nt) = next_token_type {
                    if nt != "space" {
                        lines.push(Utf16Text::new());
                    }
                }
            }
            "code" => {
                let indent = self
                    .theme
                    .code_block_indent
                    .clone()
                    .unwrap_or_else(|| "  ".to_string());
                lines.push((self.theme.code_block_border)(&Utf16Text::from(format!(
                    "```{}",
                    token.lang.clone().unwrap_or_default()
                ))));
                if let Some(hl) = &self.theme.highlight_code {
                    for hl_line in hl(&token.text, token.lang.as_deref()) {
                        lines.push(Utf16Text::from(format!("{indent}{hl_line}")));
                    }
                } else {
                    for code_line in token.text.split('\n') {
                        lines.push(raw_text!(
                            &indent,
                            (self.theme.code_block)(&Utf16Text::from(code_line))
                        ));
                    }
                }
                lines.push((self.theme.code_block_border)(&Utf16Text::from("```")));
                if let Some(nt) = next_token_type {
                    if nt != "space" {
                        lines.push(Utf16Text::new());
                    }
                }
            }
            "list" => {
                let list_lines = self.render_list(token, 0, width, style_context);
                lines.extend(list_lines);
            }
            "table" => {
                let table_lines =
                    self.render_table(token, width, next_token_type, style_context.as_ref());
                lines.extend(table_lines);
            }
            "blockquote" => {
                let quote = Arc::clone(&self.theme.quote);
                let italic = Arc::clone(&self.theme.italic);
                let quote_fn: StyleFn = Arc::new(move |text: &Utf16Text| quote(&italic(text)));
                let quote_style_prefix = self.get_style_prefix(quote_fn.as_ref());
                let quote_inline_ctx = InlineStyleContext {
                    apply_text: Arc::new(identity_style),
                    style_prefix: quote_style_prefix.clone(),
                };
                let quote_content_width = width.saturating_sub(2).max(1);
                let mut sub_tokens = std::mem::take(&mut token.tokens);
                let mut rendered_quote_lines: Vec<Utf16Text> = Vec::new();
                for i in 0..sub_tokens.len() {
                    let next_type = sub_tokens.get(i + 1).map(|t| t.kind.clone());
                    rendered_quote_lines.extend(self.render_token(
                        &mut sub_tokens[i],
                        quote_content_width,
                        next_type.as_deref(),
                        Some(quote_inline_ctx.clone()),
                    ));
                }
                token.tokens = sub_tokens;
                while rendered_quote_lines
                    .last()
                    .map(|l| l.is_empty())
                    .unwrap_or(false)
                {
                    rendered_quote_lines.pop();
                }
                for quote_line in &rendered_quote_lines {
                    let styled_line = if quote_style_prefix.is_empty() {
                        quote_fn(quote_line)
                    } else {
                        let line_with = quote_line
                            .replace("\x1b[0m", raw_text!("\x1b[0m", &quote_style_prefix));
                        quote_fn(&line_with)
                    };
                    for wrapped in wrap_text_with_ansi(&styled_line, quote_content_width) {
                        lines.push(raw_text!(
                            (self.theme.quote_border)(&Utf16Text::from("│ ")),
                            wrapped
                        ));
                    }
                }
                if let Some(nt) = next_token_type {
                    if nt != "space" {
                        lines.push(Utf16Text::new());
                    }
                }
            }
            "hr" => {
                let w = width.min(80);
                lines.push((self.theme.hr)(&Utf16Text::from("─".repeat(w))));
                if let Some(nt) = next_token_type {
                    if nt != "space" {
                        lines.push(Utf16Text::new());
                    }
                }
            }
            "html" => {
                lines.push(self.apply_default_style(token.raw.trim_matches(is_js_space)));
            }
            "space" => {
                lines.push(Utf16Text::new());
            }
            other => {
                // JS default case checks "text" in token: image carries text;
                // def/checkbox do not.
                if other == "image" {
                    lines.push(Utf16Text::from(&token.text));
                }
            }
        }
        lines
    }

    fn apply_default_style(&self, text: impl Into<Utf16Text>) -> Utf16Text {
        let text = text.into();
        let Some(style) = &self.default_text_style else {
            return text.clone();
        };
        let mut styled = text.clone();
        if let Some(color) = &style.color {
            styled = color(&styled);
        }
        if style.bold {
            styled = (self.theme.bold)(&styled);
        }
        if style.italic {
            styled = (self.theme.italic)(&styled);
        }
        if style.strikethrough {
            styled = (self.theme.strikethrough)(&styled);
        }
        if style.underline {
            styled = (self.theme.underline)(&styled);
        }
        styled
    }

    fn render_inline_tokens(
        &mut self,
        tokens: &mut [Token],
        ctx: &InlineStyleContext,
    ) -> Utf16Text {
        let apply_text: &dyn Fn(&Utf16Text) -> Utf16Text = ctx.apply_text.as_ref();
        let style_prefix = &ctx.style_prefix;
        let mut result = Utf16Text::new();
        for token in tokens.iter_mut() {
            match token.kind.as_str() {
                "latex" => {
                    let rendered = if !token.pending && self.options.render_latex {
                        render_latex_utf16(
                            token.text_units().as_units(),
                            RenderLatexOptions { display: false },
                        )
                        .map(Utf16Text::from_units)
                        .unwrap_or_else(|| token.raw_units())
                    } else {
                        token.raw_units()
                    };
                    result.push_str(apply_with_newlines(apply_text, &rendered));
                }
                "escape" => {
                    if self.options.preserve_backslash_escapes {
                        result.push_str(apply_with_newlines(apply_text, &token.raw));
                    } else {
                        result.push_str(apply_with_newlines(apply_text, &token.text));
                    }
                }
                "text" => {
                    if !token.tokens.is_empty() {
                        let mut nested = std::mem::take(&mut token.tokens);
                        let s = self.render_inline_tokens(&mut nested, ctx);
                        token.tokens = nested;
                        result.push_str(&s);
                    } else {
                        let text = token
                            .text_utf16
                            .clone()
                            .unwrap_or_else(|| Utf16Text::from(&token.text));
                        result.push_str(apply_with_newlines(apply_text, &text));
                    }
                }
                "paragraph" => {
                    let mut nested = std::mem::take(&mut token.tokens);
                    let s = self.render_inline_tokens(&mut nested, ctx);
                    token.tokens = nested;
                    result.push_str(&s);
                }
                "strong" => {
                    let mut nested = std::mem::take(&mut token.tokens);
                    let bold_content = self.render_inline_tokens(&mut nested, ctx);
                    token.tokens = nested;
                    result.push_str((self.theme.bold)(&bold_content));
                    result.push_str(style_prefix);
                }
                "em" => {
                    let mut nested = std::mem::take(&mut token.tokens);
                    let italic_content = self.render_inline_tokens(&mut nested, ctx);
                    token.tokens = nested;
                    result.push_str((self.theme.italic)(&italic_content));
                    result.push_str(style_prefix);
                }
                "codespan" => {
                    result.push_str((self.theme.code)(&Utf16Text::from(&token.text)));
                    result.push_str(style_prefix);
                }
                "link" => {
                    let mut nested = std::mem::take(&mut token.tokens);
                    let link_text = self.render_inline_tokens(&mut nested, ctx);
                    token.tokens = nested;
                    let styled_link = (self.theme.link)(&(self.theme.underline)(&link_text));
                    let href = token
                        .href_utf16
                        .clone()
                        .unwrap_or_else(|| Utf16Text::from(&token.href));
                    if get_capabilities().hyperlinks {
                        result.push_str(&raw_text!(
                            "\x1b]8;;",
                            &href,
                            "\x1b\\",
                            &styled_link,
                            "\x1b]8;;\x1b\\"
                        ));
                        result.push_str(style_prefix);
                    } else {
                        let href_for_comparison = if href.starts_with("mailto:") {
                            href.slice(7..)
                        } else {
                            href.clone()
                        };
                        let text = token.text_units();
                        if text == href || text == href_for_comparison {
                            result.push_str(&styled_link);
                            result.push_str(style_prefix);
                        } else {
                            result.push_str(&styled_link);
                            result.push_str((self.theme.link_url)(&raw_text!(" (", &href, ")")));
                            result.push_str(style_prefix);
                        }
                    }
                }
                "br" => {
                    result.push('\n');
                }
                "del" => {
                    let mut nested = std::mem::take(&mut token.tokens);
                    let del_content = self.render_inline_tokens(&mut nested, ctx);
                    token.tokens = nested;
                    result.push_str((self.theme.strikethrough)(&del_content));
                    result.push_str(style_prefix);
                }
                "html" => {
                    result.push_str(apply_with_newlines(apply_text, &token.raw));
                }
                "image" => {
                    result.push_str(apply_with_newlines(apply_text, &token.text));
                }
                _ => {}
            }
        }
        while !style_prefix.is_empty() && result.ends_with(style_prefix) {
            result.truncate(result.len() - style_prefix.len());
        }
        result
    }

    fn ordered_list_marker(&self, item: &Token) -> Option<String> {
        // /^(?: {0,3})(\d{1,9}[.)])[ \t]+/
        let raw = &item.raw;
        let spaces = raw.chars().take_while(|&c| c == ' ').count();
        if spaces > 3 {
            return None;
        }
        let rest = &raw[spaces..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.chars().count() > 9 {
            return None;
        }
        let after = &rest[digits.len()..];
        let mut chars = after.chars();
        match chars.next() {
            Some(sep @ ('.' | ')')) => {
                if chars.as_str().starts_with(' ') || chars.as_str().starts_with('\t') {
                    Some(format!("{digits}{sep} "))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn unordered_list_marker(&self, item: &Token) -> Option<String> {
        // /^(?: {0,3})([-+*])(?:[ \t]+|(?=\r?\n|$))/
        let raw = &item.raw;
        let spaces = raw.chars().take_while(|&c| c == ' ').count();
        if spaces > 3 {
            return None;
        }
        let rest = &raw[spaces..];
        match rest.chars().next() {
            Some(c @ ('-' | '+' | '*')) => Some(format!("{c} ")),
            _ => None,
        }
    }

    fn render_list(
        &mut self,
        token: &mut Token,
        depth: usize,
        width: usize,
        style_context: Option<InlineStyleContext>,
    ) -> Vec<Utf16Text> {
        let mut lines: Vec<Utf16Text> = Vec::new();
        let indent = "    ".repeat(depth);
        let start_number = if token.ordered { token.start.max(1) } else { 1 };
        let item_count = token.items.len();
        for i in 0..item_count {
            let is_last_item = i == item_count - 1;
            let (bullet, task_marker) = {
                let item = &token.items[i];
                let bullet = if token.ordered {
                    if self.options.preserve_ordered_list_markers {
                        self.ordered_list_marker(item)
                            .unwrap_or_else(|| format!("{}. ", start_number + i))
                    } else {
                        format!("{}. ", start_number + i)
                    }
                } else if self.options.preserve_ordered_list_markers {
                    self.unordered_list_marker(item)
                        .unwrap_or_else(|| "- ".to_string())
                } else {
                    "- ".to_string()
                };
                let task_marker = if item.task {
                    format!(
                        "[{}] ",
                        if item.checked.unwrap_or(false) {
                            "x"
                        } else {
                            " "
                        }
                    )
                } else {
                    String::new()
                };
                (bullet, task_marker)
            };
            let marker = format!("{bullet}{task_marker}");
            let first_prefix =
                raw_text!(&indent, (self.theme.list_bullet)(&Utf16Text::from(&marker)));
            let continuation_prefix = Utf16Text::from(format!(
                "{}{}",
                indent,
                " ".repeat(visible_width(&Utf16Text::from(&marker)))
            ));
            let item_width = width.saturating_sub(visible_width(&first_prefix)).max(1);
            let mut rendered_any_line = false;

            let mut item_tokens = std::mem::take(&mut token.items[i].tokens);
            let mut j = 0usize;
            while j < item_tokens.len() {
                if item_tokens[j].kind == "list" {
                    let sub = self.render_list(
                        &mut item_tokens[j],
                        depth + 1,
                        width,
                        style_context.clone(),
                    );
                    lines.extend(sub);
                    rendered_any_line = true;
                    j += 1;
                    continue;
                }
                let item_lines =
                    self.render_token(&mut item_tokens[j], item_width, None, style_context.clone());
                for line in &item_lines {
                    for wrapped in wrap_text_with_ansi(line, item_width) {
                        let line_prefix = if rendered_any_line {
                            continuation_prefix.clone()
                        } else {
                            first_prefix.clone()
                        };
                        lines.push(raw_text!(line_prefix, wrapped));
                        rendered_any_line = true;
                    }
                }
                j += 1;
            }
            token.items[i].tokens = item_tokens;

            if !rendered_any_line {
                lines.push(first_prefix);
            }
            if token.loose && !is_last_item {
                lines.push(Utf16Text::new());
            }
        }
        lines
    }

    fn get_longest_word_width(&self, text: &Utf16Text, max_width: Option<usize>) -> usize {
        let mut longest = 0usize;
        for word in text.as_units().split(|&u| {
            char::from_u32(u32::from(u)).is_some_and(crate::tui::utils::is_js_space_unicode)
        }) {
            if word.is_empty() {
                continue;
            }
            longest = longest.max(visible_width_utf16(word));
        }
        match max_width {
            None => longest,
            Some(mw) => longest.min(mw),
        }
    }

    fn wrap_cell_text(
        &self,
        text: &Utf16Text,
        max_width: usize,
        style_prefix: &Utf16Text,
    ) -> Vec<Utf16Text> {
        let lines = wrap_text_with_ansi(text, max_width.max(1));
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let style_reset = if index < lines.len() - 1 {
                    "\x1b[22;23;24;25;27;28;29;39m"
                } else {
                    ""
                };
                raw_text!(line, style_reset, style_prefix)
            })
            .collect()
    }

    fn render_table(
        &mut self,
        token: &mut Token,
        available_width: usize,
        next_token_type: Option<&str>,
        style_context: Option<&InlineStyleContext>,
    ) -> Vec<Utf16Text> {
        let mut lines: Vec<Utf16Text> = Vec::new();
        let num_cols = token.header.len();
        if num_cols == 0 {
            return lines;
        }
        // Upstream only restores an explicitly supplied enclosing context here,
        // not the implicit default text style used by renderInlineTokens.
        let cell_prefix = style_context
            .map(|c| c.style_prefix.clone())
            .unwrap_or_default();
        let fallback_ctx = style_context
            .cloned()
            .unwrap_or_else(|| self.default_context());
        let style_context = &fallback_ctx;
        let border_overhead = 3 * num_cols + 1;
        if (available_width as i64 - border_overhead as i64) < num_cols as i64 {
            // Too narrow to render a stable table. Fall back to raw markdown.
            let mut fallback_lines =
                wrap_text_with_ansi(&Utf16Text::from(&token.raw), available_width);
            if let Some(nt) = next_token_type {
                if nt != "space" {
                    fallback_lines.push(Utf16Text::new());
                }
            }
            return fallback_lines;
        }
        let available_for_cells = available_width - border_overhead;
        let max_unbroken_word_width = 30usize;

        let mut natural_widths = vec![0usize; num_cols];
        let mut min_word_widths = vec![1usize; num_cols];
        for i in 0..num_cols {
            let mut cell_tokens = std::mem::take(&mut token.header[i].tokens);
            let header_text = self.render_inline_tokens(&mut cell_tokens, style_context);
            token.header[i].tokens = cell_tokens;
            natural_widths[i] = visible_width(&header_text);
            min_word_widths[i] = self
                .get_longest_word_width(&header_text, Some(max_unbroken_word_width))
                .max(1);
        }
        for row in &mut token.rows {
            for i in 0..row.len().min(num_cols) {
                let mut cell_tokens = std::mem::take(&mut row[i].tokens);
                let cell_text = self.render_inline_tokens(&mut cell_tokens, style_context);
                row[i].tokens = cell_tokens;
                natural_widths[i] = natural_widths[i].max(visible_width(&cell_text));
                min_word_widths[i] = min_word_widths[i].max(
                    self.get_longest_word_width(&cell_text, Some(max_unbroken_word_width))
                        .max(1),
                );
            }
        }

        let mut min_column_widths = min_word_widths.clone();
        let mut min_cells_width: usize = min_column_widths.iter().sum();
        if min_cells_width > available_for_cells {
            min_column_widths = vec![1usize; num_cols];
            let remaining = available_for_cells.saturating_sub(num_cols);
            if remaining > 0 {
                let total_weight: usize =
                    min_word_widths.iter().map(|&w| w.saturating_sub(1)).sum();
                let growth: Vec<usize> = min_word_widths
                    .iter()
                    .map(|&w| {
                        let weight = w.saturating_sub(1);
                        weight
                            .checked_mul(remaining)
                            .and_then(|weighted| weighted.checked_div(total_weight))
                            .unwrap_or(0)
                    })
                    .collect();
                for (i, width) in min_column_widths.iter_mut().enumerate() {
                    *width += growth.get(i).copied().unwrap_or(0);
                }
                let allocated: usize = growth.iter().sum();
                let mut leftover = remaining.saturating_sub(allocated);
                let mut i = 0usize;
                while leftover > 0 && i < num_cols {
                    min_column_widths[i] += 1;
                    leftover -= 1;
                    i += 1;
                }
            }
            min_cells_width = min_column_widths.iter().sum();
        }

        let total_natural_width: usize = natural_widths.iter().sum::<usize>() + border_overhead;
        let column_widths: Vec<usize> = if total_natural_width <= available_width {
            (0..num_cols)
                .map(|i| natural_widths[i].max(min_column_widths[i]))
                .collect()
        } else {
            let total_grow_potential: usize = natural_widths
                .iter()
                .zip(min_column_widths.iter())
                .map(|(&n, &m)| n.saturating_sub(m))
                .sum();
            let extra_width = available_for_cells.saturating_sub(min_cells_width);
            let mut widths: Vec<usize> = min_column_widths
                .iter()
                .zip(natural_widths.iter())
                .map(|(&min_width, &natural_width)| {
                    let min_width_delta = natural_width.saturating_sub(min_width);
                    let grow = min_width_delta
                        .checked_mul(extra_width)
                        .and_then(|weighted| weighted.checked_div(total_grow_potential))
                        .unwrap_or(0);
                    min_width + grow
                })
                .collect();
            let allocated: usize = widths.iter().sum();
            let mut remaining = available_for_cells.saturating_sub(allocated);
            while remaining > 0 {
                let mut grew = false;
                for i in 0..num_cols {
                    if remaining > 0 && widths[i] < natural_widths[i] {
                        widths[i] += 1;
                        remaining -= 1;
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            widths
        };

        let top_cells: Vec<String> = column_widths.iter().map(|&w| "─".repeat(w)).collect();
        lines.push(Utf16Text::from(format!("┌─{}─┐", top_cells.join("─┬─"))));

        let mut header_cell_lines: Vec<Vec<Utf16Text>> = Vec::new();
        for (i, &column_width) in column_widths.iter().enumerate() {
            let mut cell_tokens = std::mem::take(&mut token.header[i].tokens);
            let text = self.render_inline_tokens(&mut cell_tokens, style_context);
            token.header[i].tokens = cell_tokens;
            header_cell_lines.push(self.wrap_cell_text(&text, column_width, &cell_prefix));
        }
        let header_line_count = header_cell_lines.iter().map(|c| c.len()).max().unwrap_or(0);
        for line_idx in 0..header_line_count {
            let row_parts: Vec<Utf16Text> = header_cell_lines
                .iter()
                .enumerate()
                .map(|(col_idx, cell_lines)| {
                    let text = cell_lines.get(line_idx).cloned().unwrap_or_default();
                    let padded = raw_text!(
                        &text,
                        " ".repeat(column_widths[col_idx].saturating_sub(visible_width(&text)))
                    );
                    (self.theme.bold)(&padded)
                })
                .collect();
            lines.push(raw_text!("│ ", Utf16Text::join(row_parts, " │ "), " │"));
        }

        let sep_cells: Vec<String> = column_widths.iter().map(|&w| "─".repeat(w)).collect();
        let separator_line = Utf16Text::from(format!("├─{}─┤", sep_cells.join("─┼─")));
        lines.push(separator_line.clone());

        for row_index in 0..token.rows.len() {
            let mut row_cell_lines: Vec<Vec<Utf16Text>> = Vec::new();
            let row_len = token.rows[row_index].len();
            for (i, &column_width) in column_widths.iter().enumerate().take(row_len) {
                let mut cell_tokens = std::mem::take(&mut token.rows[row_index][i].tokens);
                let text = self.render_inline_tokens(&mut cell_tokens, style_context);
                token.rows[row_index][i].tokens = cell_tokens;
                row_cell_lines.push(self.wrap_cell_text(&text, column_width, &cell_prefix));
            }
            let row_line_count = row_cell_lines.iter().map(|c| c.len()).max().unwrap_or(0);
            for line_idx in 0..row_line_count {
                let row_parts: Vec<Utf16Text> = row_cell_lines
                    .iter()
                    .enumerate()
                    .map(|(col_idx, cell_lines)| {
                        let text = cell_lines.get(line_idx).cloned().unwrap_or_default();
                        raw_text!(
                            &text,
                            " ".repeat(column_widths[col_idx].saturating_sub(visible_width(&text)))
                        )
                    })
                    .collect();
                lines.push(raw_text!("│ ", Utf16Text::join(row_parts, " │ "), " │"));
            }
            if row_index < token.rows.len() - 1 {
                lines.push(separator_line.clone());
            }
        }

        let bottom_cells: Vec<String> = column_widths.iter().map(|&w| "─".repeat(w)).collect();
        lines.push(Utf16Text::from(format!("└─{}─┘", bottom_cells.join("─┴─"))));

        if let Some(nt) = next_token_type {
            if nt != "space" {
                lines.push(Utf16Text::new());
            }
        }
        lines
    }
}

fn apply_with_newlines(
    apply: &dyn Fn(&Utf16Text) -> Utf16Text,
    text: impl Into<Utf16Text>,
) -> Utf16Text {
    Utf16Text::join(text.into().split('\n').map(|line| apply(&line)), "\n")
}

fn visible_width(text: &Utf16Text) -> usize {
    visible_width_utf16(text.as_units())
}

fn apply_background_to_line(
    line: &Utf16Text,
    width: usize,
    bg: impl FnOnce(&Utf16Text) -> Utf16Text,
) -> Utf16Text {
    bg(&raw_text!(
        line,
        " ".repeat(width.saturating_sub(visible_width(line)))
    ))
}

fn is_image_line(line: &Utf16Text) -> bool {
    line.contains("\x1b_G") || line.contains("\x1b]1337;File=")
}

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<String> {
        Markdown::render(self, width)
    }
}
