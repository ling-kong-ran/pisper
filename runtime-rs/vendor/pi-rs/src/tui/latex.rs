//! Port of upstream packages/tui/src/latex.ts: LaTeX to Unicode terminal math.
//!
//! Parsing and layout retain upstream UTF-16 code-unit argument semantics.
//! Unknown commands and malformed groups return None for Markdown fallback.
//! render_latex_utf16 preserves lone surrogates through layout; render_latex
//! performs Node-compatible replacement encoding only at the UTF-8 boundary.
//! Tables and upstream fixtures: docs/migration/reference/latex/run.mjs.

mod tables;
#[cfg(test)]
mod tests;
mod utf16;

use super::utils::{is_letter_or_number_unicode, is_whitespace_char, visible_width_utf16};
use icu_properties::{props::GeneralCategory, CodePointMapData};
use utf16::Text;

// Upstream's own marker protocol, not an encoding of surrogates.
const NAMED_OPERATOR_START: char = '\u{f0004}';
const NAMED_OPERATOR_END: char = '\u{f0005}';
const LAYOUT_MARKER_START: char = '\u{f0000}';
const LAYOUT_MARKER_END: char = '\u{f0001}';
const PROTECTED_SPACE: char = '\u{f0002}';
const NEGATIVE_SPACE: &str = "\0";

macro_rules! text {
    ($($part:expr),* $(,)?) => {{
        let mut result = Text::new();
        $(result.push($part);)*
        result
    }};
}

/// Upstream RenderLatexOptions.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderLatexOptions {
    /// Stack fractions and operator limits vertically for display math.
    pub display: bool,
}

fn whitespace(point: u32) -> bool {
    char::from_u32(point).is_some_and(is_whitespace_char)
}

fn letter_or_number(point: u32) -> bool {
    char::from_u32(point).is_some_and(is_letter_or_number_unicode)
}

fn trim_start(value: &Text) -> Text {
    value.slice(
        value
            .0
            .iter()
            .take_while(|&&u| whitespace(u32::from(u)))
            .count()..,
    )
}

fn trim_end(value: &Text) -> Text {
    value.slice(
        ..value
            .0
            .iter()
            .rposition(|&u| !whitespace(u32::from(u)))
            .map_or(0, |i| i + 1),
    )
}

fn js_trim(value: &Text) -> Text {
    trim_end(&trim_start(value))
}
fn visible_width(value: &Text) -> usize {
    visible_width_utf16(&value.0)
}

fn number(point: u32) -> bool {
    matches!(
        CodePointMapData::<GeneralCategory>::new().get32(point),
        GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    )
}

fn simple_value(value: &Text) -> bool {
    !value.is_empty()
        && value
            .points()
            .all(|p| p == u32::from('.') || letter_or_number(p))
}

/// JavaScript object semantics for the upstream plain-object table lookups
/// (`SYMBOLS[command]`, `ACCENTS[command]`, `NEGATED_SYMBOLS[value]`):
/// `Object.prototype` members resolve as inherited values. Commands are
/// `[A-Za-z]+`, so the pure-letter members are reachable as commands, and
/// every member is reachable as a `\not{...}` argument. Each entry is the
/// member's exact V8 stringification (node v25.8.2 oracle); `__proto__` is
/// the accessor's return value, `Object.prototype` itself.
fn object_prototype_member(name: &str) -> Option<&'static str> {
    match name {
        "constructor" => Some("function Object() { [native code] }"),
        "toString" => Some("function toString() { [native code] }"),
        "toLocaleString" => Some("function toLocaleString() { [native code] }"),
        "valueOf" => Some("function valueOf() { [native code] }"),
        "hasOwnProperty" => Some("function hasOwnProperty() { [native code] }"),
        "isPrototypeOf" => Some("function isPrototypeOf() { [native code] }"),
        "propertyIsEnumerable" => Some("function propertyIsEnumerable() { [native code] }"),
        "__defineGetter__" => Some("function __defineGetter__() { [native code] }"),
        "__defineSetter__" => Some("function __defineSetter__() { [native code] }"),
        "__lookupGetter__" => Some("function __lookupGetter__() { [native code] }"),
        "__lookupSetter__" => Some("function __lookupSetter__() { [native code] }"),
        "__proto__" => Some("[object Object]"),
        _ => None,
    }
}

/// Upstream `FONT_SWITCH_COMMANDS` (latex.ts, v0.99.1 delta): consumed with
/// any following whitespace, producing no output. Kept beside the parser (the
/// generated `tables.rs` must stay byte-identical to its checked-in
/// manifest).
fn font_switch_commands(value: &str) -> bool {
    matches!(value, "bf" | "cal" | "it" | "rm" | "sf" | "sl" | "tt")
}

fn normalize_script_value(value: &Text) -> Text {
    // value.trim().replace(/\s*([=+-])\s*/g, "$1")
    let trimmed = js_trim(value);
    let mut compact = Text::new();
    let mut points = trimmed.points().peekable();
    while let Some(point) = points.next() {
        if matches!(point, 0x3d | 0x2b | 0x2d) {
            compact = trim_end(&compact);
            compact.push_point(point);
            while points.peek().is_some_and(|p| whitespace(*p)) {
                points.next();
            }
        } else {
            compact.push_point(point);
        }
    }
    compact
}

fn format_unicode_script(value: &Text, sub: bool) -> Option<Text> {
    let normalized = normalize_script_value(value);
    let replacements = if sub {
        tables::subscripts
    } else {
        tables::superscripts
    };
    let mut unicode = Text::new();
    let mapped = normalized.points().all(|point| {
        char::from_u32(point)
            .and_then(|c| replacements(c.encode_utf8(&mut [0; 4])))
            .is_some_and(|s| {
                unicode.push(s);
                true
            })
    });
    mapped.then_some(unicode)
}

fn format_script(value: &Text, sub: bool) -> Text {
    let value = normalize_script_value(value);
    if let Some(unicode) = format_unicode_script(&value, sub) {
        return unicode;
    }
    let prefix = if sub { '_' } else { '^' };
    if value.points().count() == 1
        || (sub && !value.is_empty() && value.0.iter().all(|u| matches!(u, 65..=90 | 97..=122)))
    {
        text!(prefix, &value)
    } else {
        text!(prefix, '(', &value, ')')
    }
}

fn format_fraction(numerator: &Text, denominator: &Text) -> Text {
    let numerator = js_trim(numerator);
    let denominator = js_trim(denominator);
    let simple_denominator = (!denominator.is_empty()
        && denominator
            .points()
            .all(|p| p == u32::from('.') || number(p)))
        || denominator.points().count() == 1;
    let numerator = if simple_value(&numerator) {
        numerator
    } else {
        text!('(', numerator, ')')
    };
    let denominator = if simple_denominator {
        denominator
    } else {
        text!('(', denominator, ')')
    };
    text!(numerator, '/', denominator)
}

fn format_root(value: &Text, symbol: char) -> Text {
    let value = js_trim(value);
    if simple_value(&value) {
        text!(symbol, value)
    } else {
        text!(symbol, '(', value, ')')
    }
}

fn normalize_output(value: &Text) -> Text {
    // Scalar iteration is intentional: these marker regexes have the u flag.
    let mut left = Text::new();
    let mut previous = None;
    for point in value.points() {
        if point == NAMED_OPERATOR_START as u32 {
            if previous.is_some_and(|p| {
                letter_or_number(p)
                    || matches!(p, 0x29 | 0x5d | 0x7d)
                    || p == LAYOUT_MARKER_END as u32
            }) {
                left.push(' ');
            }
        } else {
            left.push_point(point);
        }
        previous = Some(point);
    }
    let mut right = Text::new();
    let mut points = left.points().peekable();
    while let Some(point) = points.next() {
        if point == NAMED_OPERATOR_END as u32 {
            if points.peek().is_some_and(|p| {
                letter_or_number(*p) || *p == u32::from('√') || *p == LAYOUT_MARKER_START as u32
            }) {
                right.push(' ');
            }
        } else {
            right.push_point(point);
        }
    }
    let lines: Vec<_> = right
        .split("\n")
        .iter()
        .map(|line| {
            let mut collapsed = Text::new();
            let mut space = false;
            for &unit in &line.0 {
                if matches!(unit, 0x20 | 0x09) {
                    if !space {
                        collapsed.push(' ');
                    }
                    space = true;
                } else {
                    collapsed.push_unit(unit);
                    space = false;
                }
            }
            js_trim(&collapsed)
        })
        .collect();
    js_trim(&Text::join(
        lines
            .iter()
            .enumerate()
            .filter(|(index, line)| !line.is_empty() || (*index > 0 && *index < lines.len() - 1))
            .map(|(_, line)| line),
        "\n",
    ))
}

#[derive(Debug)]
enum LayoutNode {
    Fraction {
        numerator: Text,
        denominator: Text,
    },
    Operator {
        operator: Text,
        lower: Option<Text>,
        upper: Option<Text>,
    },
    Script {
        lower: Option<Text>,
        upper: Option<Text>,
    },
    Matrix {
        lines: Vec<Text>,
        baseline: usize,
    },
}
impl LayoutNode {
    fn is_matrix(&self) -> bool {
        matches!(self, Self::Matrix { .. })
    }
}
struct Layout {
    lines: Vec<Text>,
    width: usize,
    baseline: usize,
}
impl Layout {
    fn text(value: Text) -> Self {
        Self {
            width: visible_width(&value),
            lines: vec![value],
            baseline: 0,
        }
    }
}
fn layout_marker(index: usize) -> Text {
    text!(LAYOUT_MARKER_START, index.to_string(), LAYOUT_MARKER_END)
}

/// Match the upstream /marker([0-9]+)marker/gu at UTF-16 offsets.
fn layout_markers(value: &Text) -> Vec<(usize, usize, Option<usize>)> {
    let mut result = Vec::new();
    let mut at = 0;
    while let Some(relative) = value
        .slice(at..)
        .find(LAYOUT_MARKER_START.encode_utf8(&mut [0; 4]))
    {
        let start = at + relative;
        let digits_start = start + LAYOUT_MARKER_START.len_utf16();
        let digits_end = digits_start
            + value.0[digits_start..]
                .iter()
                .take_while(|u| matches!(u, 48..=57))
                .count();
        at = digits_start;
        if digits_end == digits_start
            || !value
                .slice(digits_end..)
                .starts_with(LAYOUT_MARKER_END.encode_utf8(&mut [0; 4]))
        {
            continue;
        }
        at = digits_end + LAYOUT_MARKER_END.len_utf16();
        let index = value
            .slice(digits_start..digits_end)
            .to_utf8()
            .and_then(|s| s.parse().ok());
        result.push((start, at, index));
    }
    result
}
fn trailing_marker(value: &Text) -> Option<usize> {
    layout_markers(value)
        .last()
        .filter(|(_, end, _)| *end == value.len())
        .and_then(|(_, _, index)| *index)
}
fn pad_layout_line(line: &Text, width: usize, centered: bool) -> Text {
    let padding = width.saturating_sub(visible_width(line));
    let left = if centered { padding / 2 } else { 0 };
    text!(" ".repeat(left), line, " ".repeat(padding - left))
}

fn join_layouts(layouts: &[Layout]) -> Layout {
    if layouts.is_empty() {
        return Layout::text(Text::new());
    }
    let baseline = layouts.iter().map(|l| l.baseline).max().unwrap_or(0);
    let below = layouts
        .iter()
        .map(|l| l.lines.len() - l.baseline - 1)
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for row in 0..=baseline + below {
        let mut line = Text::new();
        for layout in layouts {
            let source_row = (row + layout.baseline).checked_sub(baseline);
            if let Some(source) = source_row.and_then(|r| layout.lines.get(r)) {
                line.push(pad_layout_line(source, layout.width, false));
            } else {
                line.push(" ".repeat(layout.width));
            }
        }
        lines.push(trim_end(&line));
    }
    Layout {
        lines,
        width: layouts.iter().map(|l| l.width).sum(),
        baseline,
    }
}

fn render_layout(source: &Text, nodes: &[LayoutNode]) -> Layout {
    let mut rendered_lines = Vec::new();
    let mut first_baseline = 0;
    for source_line in source.split("\n") {
        let mut layouts = Vec::new();
        let mut position = 0;
        let mut previous_node: Option<&LayoutNode> = None;
        for (start, end, index) in layout_markers(&source_line) {
            let Some(node) = index.and_then(|i| nodes.get(i)) else {
                continue;
            };
            if start > position {
                let sliced = source_line.slice(position..start);
                let trimmed = trim_end(&if previous_node.is_some() {
                    trim_start(&sliced)
                } else {
                    sliced.clone()
                });
                let leading = previous_node.is_some_and(LayoutNode::is_matrix)
                    && sliced.0.first().is_some_and(|u| whitespace(u32::from(*u)));
                let trailing =
                    node.is_matrix() && sliced.0.last().is_some_and(|u| whitespace(u32::from(*u)));
                let text = if !trimmed.is_empty() {
                    text!(
                        if leading { " " } else { "" },
                        trimmed,
                        if trailing { " " } else { "" }
                    )
                } else if leading || trailing {
                    Text::from(" ")
                } else {
                    Text::new()
                };
                layouts.push(Layout::text(text));
            }
            let layout = match node {
                LayoutNode::Fraction {
                    numerator,
                    denominator,
                } => {
                    let numerator = render_layout(numerator, nodes);
                    let denominator = render_layout(denominator, nodes);
                    let content_width = numerator.width.max(denominator.width).max(1);
                    let width = content_width + 2;
                    let mut lines: Vec<_> = numerator
                        .lines
                        .iter()
                        .map(|l| pad_layout_line(l, width, true))
                        .collect();
                    lines.push(text!(' ', "─".repeat(content_width), ' '));
                    lines.extend(
                        denominator
                            .lines
                            .iter()
                            .map(|l| pad_layout_line(l, width, true)),
                    );
                    Layout {
                        lines,
                        width,
                        baseline: numerator.lines.len(),
                    }
                }
                LayoutNode::Operator {
                    operator,
                    lower,
                    upper,
                } => {
                    let content_width = visible_width(operator)
                        .max(lower.as_ref().map_or(0, visible_width))
                        .max(upper.as_ref().map_or(0, visible_width));
                    let mut lines = Vec::new();
                    if let Some(upper) = upper {
                        lines.push(text!(pad_layout_line(upper, content_width, true), ' '));
                    }
                    lines.push(text!(pad_layout_line(operator, content_width, true), ' '));
                    if let Some(lower) = lower {
                        lines.push(text!(pad_layout_line(lower, content_width, true), ' '));
                    }
                    Layout {
                        lines,
                        width: content_width + 1,
                        baseline: usize::from(upper.is_some()),
                    }
                }
                LayoutNode::Script { lower, upper } => {
                    let upper = upper.as_ref().map(|source| render_layout(source, nodes));
                    let lower = lower.as_ref().map(|source| render_layout(source, nodes));
                    let width = upper
                        .as_ref()
                        .map_or(0, |layout| layout.width)
                        .max(lower.as_ref().map_or(0, |layout| layout.width));
                    let mut lines = Vec::new();
                    if let Some(upper) = &upper {
                        lines.extend(
                            upper
                                .lines
                                .iter()
                                .map(|line| pad_layout_line(line, width, false)),
                        );
                    }
                    lines.push(text!(" ".repeat(width)));
                    if let Some(lower) = &lower {
                        lines.extend(
                            lower
                                .lines
                                .iter()
                                .map(|line| pad_layout_line(line, width, false)),
                        );
                    }
                    Layout {
                        lines,
                        width,
                        baseline: upper.as_ref().map_or(0, |layout| layout.lines.len()),
                    }
                }
                LayoutNode::Matrix { lines, baseline } => {
                    let width = lines.iter().map(visible_width).max().unwrap_or(0);
                    Layout {
                        lines: lines
                            .iter()
                            .map(|l| pad_layout_line(l, width, false))
                            .collect(),
                        width,
                        baseline: *baseline,
                    }
                }
            };
            layouts.push(layout);
            position = end;
            previous_node = Some(node);
        }
        if position < source_line.len() {
            let sliced = source_line.slice(position..);
            let trimmed = if previous_node.is_some() {
                trim_start(&sliced)
            } else {
                sliced.clone()
            };
            let text = if previous_node.is_some_and(LayoutNode::is_matrix)
                && sliced.0.first().is_some_and(|u| whitespace(u32::from(*u)))
            {
                text!(' ', trimmed)
            } else {
                trimmed
            };
            layouts.push(Layout::text(text));
        }
        let line_layout = join_layouts(&layouts);
        if rendered_lines.is_empty() {
            first_baseline = line_layout.baseline;
        }
        rendered_lines.extend(line_layout.lines);
    }
    let width = rendered_lines.iter().map(visible_width).max().unwrap_or(0);
    Layout {
        lines: rendered_lines,
        width,
        baseline: first_baseline,
    }
}

struct LatexParser<'source, 'nodes> {
    source: &'source Text,
    nodes: &'nodes mut Vec<LayoutNode>,
    display: bool,
    position: usize,
    supported: bool,
    stack_fractions: bool,
    script_depth: usize,
}

impl<'source, 'nodes> LatexParser<'source, 'nodes> {
    fn new(source: &'source Text, nodes: &'nodes mut Vec<LayoutNode>, display: bool) -> Self {
        Self {
            source,
            nodes,
            display,
            position: 0,
            supported: true,
            stack_fractions: true,
            script_depth: 0,
        }
    }
    fn peek(&self) -> Option<char> {
        self.source.syntax_at(self.position)
    }
    fn take(&mut self) -> Option<u16> {
        let unit = *self.source.0.get(self.position)?;
        self.position += 1;
        Some(unit)
    }
    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(is_whitespace_char) {
            self.take();
        }
    }
    fn render(mut self) -> Option<Text> {
        let rendered = self.parse_sequence(None);
        (self.supported && self.position == self.source.len()).then(|| normalize_output(&rendered))
    }
    fn parse_sequence(&mut self, end: Option<char>) -> Text {
        let mut result = Text::new();
        while let Some(character) = self.peek() {
            if end == Some(character) {
                self.take();
                return result;
            }
            match character {
                '}' => {
                    self.supported = false;
                    return result;
                }
                '{' => {
                    self.take();
                    result.push(self.parse_sequence(Some('}')));
                }
                '\\' => {
                    let command = self.parse_command();
                    if command == NEGATIVE_SPACE {
                        result = trim_end(&result);
                        if result.ends_with(NAMED_OPERATOR_END) {
                            result.truncate(result.len() - NAMED_OPERATOR_END.len_utf16());
                        }
                    } else {
                        result.push(command);
                    }
                }
                '^' | '_' => {
                    self.take();
                    result = trim_end(&result);
                    let script = self.parse_scripts(character);
                    if result.ends_with(NAMED_OPERATOR_END) {
                        result.truncate(result.len() - NAMED_OPERATOR_END.len_utf16());
                        result.push(script);
                        result.push(NAMED_OPERATOR_END);
                    } else {
                        result.push(script);
                    }
                }
                c if is_whitespace_char(c) => {
                    self.skip_whitespace();
                    result.push(' ');
                }
                '=' | '<' | '>' => {
                    result = text!(trim_end(&result), ' ', character, ' ');
                    self.take();
                }
                '&' => {
                    self.take();
                }
                '~' => {
                    self.take();
                    result.push(' ');
                }
                '.' => {
                    if let Some(LayoutNode::Matrix { lines, .. }) =
                        trailing_marker(&result).and_then(|i| self.nodes.get_mut(i))
                    {
                        if let Some(last) = lines.last_mut() {
                            last.push('.');
                        }
                    } else {
                        result.push('.');
                    }
                    self.take();
                }
                _ => {
                    result.push_unit(self.take().expect("peeked code unit"));
                }
            }
        }
        if end.is_some() {
            self.supported = false;
        }
        result
    }

    fn parse_command(&mut self) -> Text {
        self.take();
        let Some(first) = self.peek() else {
            self.supported = false;
            return Text::new();
        };
        if first == '\n' || first == '\r' {
            self.take();
            if first == '\r' && self.peek() == Some('\n') {
                self.take();
            }
            return Text::from(" ");
        }
        let start = self.position;
        if first.is_ascii_alphabetic() {
            while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.take();
            }
        } else {
            self.take();
        }
        // Every supported command is valid Unicode. Lone surrogates are
        // unknown upstream too; never normalize them into recognized commands.
        let Some(command) = self.source.slice(start..self.position).to_utf8() else {
            self.supported = false;
            return Text::new();
        };
        let command = command.as_str();
        if command == "\\" {
            return Text::from("\n");
        }
        if tables::spacing_commands(command) {
            return Text::from(" ");
        }
        if tables::negative_spacing_commands(command) {
            return Text::from(NEGATIVE_SPACE);
        }
        if font_switch_commands(command) {
            while self.peek().is_some_and(is_whitespace_char) {
                self.take();
            }
            return Text::new();
        }
        if tables::ignored_commands(command) {
            return Text::new();
        }
        if matches!(command, "{" | "}" | "$" | "%" | "#" | "_" | "&") {
            return Text::from(command);
        }
        if command == "|" {
            return Text::from("‖");
        }
        if command == "not" {
            let value = js_trim(&self.parse_required_argument(false));
            if let Some(negated) = value
                .to_utf8()
                .as_deref()
                .and_then(|v| tables::negated_symbols(v).or_else(|| object_prototype_member(v)))
            {
                return text!(' ', negated, ' ');
            }
            let mut points = value.points();
            let Some(first) = points.next() else {
                self.supported = false;
                return Text::new();
            };
            let mut result = Text::from(" ");
            result.push_point(first);
            result.push('\u{338}');
            for point in points {
                result.push_point(point);
            }
            result.push(' ');
            return result;
        }
        if tables::limit_operators(command) {
            return self.parse_operator(&Text::from(command), true, true, true);
        }
        if let Some(symbol) = tables::symbols(command).or_else(|| object_prototype_member(command))
        {
            if tables::display_limit_symbols(command) {
                return self.parse_operator(&Text::from(symbol), false, true, false);
            }
            return if matches!(command, "cdot" | "times") || tables::relation_commands(command) {
                text!(' ', symbol, ' ')
            } else {
                Text::from(symbol)
            };
        }
        if tables::named_operators(command) {
            return text!(NAMED_OPERATOR_START, command, NAMED_OPERATOR_END);
        }
        if tables::size_commands(command) {
            return Text::new();
        }
        if matches!(command, "left" | "middle" | "right") {
            if self.peek() == Some('.') {
                self.take();
            }
            return Text::new();
        }
        if matches!(command, "frac" | "dfrac" | "tfrac") {
            let should_stack = self.display && self.stack_fractions && command != "tfrac";
            let numerator = self.parse_required_argument(!should_stack);
            let denominator = self.parse_required_argument(!should_stack);
            if should_stack {
                let index = self.nodes.len();
                self.nodes.push(LayoutNode::Fraction {
                    numerator: normalize_output(&numerator),
                    denominator: normalize_output(&denominator),
                });
                return layout_marker(index);
            }
            return format_fraction(&numerator, &denominator);
        }
        if command == "sqrt" {
            let degree = self.parse_optional_argument().map(|s| js_trim(&s));
            let value = self.parse_required_argument(true);
            return match degree {
                None => format_root(&value, '√'),
                Some(degree) if degree == "2" => format_root(&value, '√'),
                Some(degree) if degree == "3" => format_root(&value, '∛'),
                Some(degree) if degree == "4" => format_root(&value, '∜'),
                Some(degree) => text!(format_script(&degree, false), format_root(&value, '√')),
            };
        }
        if matches!(command, "boxed" | "fbox") {
            return text!('[', js_trim(&self.parse_required_argument(true)), ']');
        }
        if matches!(command, "binom" | "dbinom" | "tbinom") {
            let first = self.parse_required_argument(true);
            let second = self.parse_required_argument(true);
            return text!('(', first, " choose ", second, ')');
        }
        if let Some(accent) = tables::accents(command).or_else(|| object_prototype_member(command))
        {
            let value = self.parse_required_argument(true);
            return if value.points().count() == 1 {
                text!(value, accent)
            } else {
                text!(command, '(', value, ')')
            };
        }
        if command == "mathbb" {
            let value = self.parse_required_argument(true);
            let mut result = Text::new();
            for point in value.points() {
                if let Some(replacement) = char::from_u32(point)
                    .and_then(|c| tables::blackboard(c.encode_utf8(&mut [0; 4])))
                {
                    result.push(replacement);
                } else {
                    result.push_point(point);
                }
            }
            return result;
        }
        if command == "operatorname" {
            let starred = self.peek() == Some('*');
            if starred {
                self.take();
            }
            let operator = normalize_output(&self.parse_required_argument(true));
            return self.parse_operator(&js_trim(&operator), true, starred, true);
        }
        if matches!(command, "mod" | "bmod") {
            return Text::from(" mod ");
        }
        if matches!(command, "pmod" | "pod") {
            let value = js_trim(&self.parse_required_argument(true));
            return text!(if command == "pmod" { " (mod " } else { " (" }, value, ')');
        }
        if matches!(command, "overset" | "stackrel" | "underset") {
            let script = self.parse_required_argument(true);
            let value = self.parse_required_argument(true);
            return text!(
                js_trim(&value),
                format_script(&script, command == "underset")
            );
        }
        if tables::plain_wrappers(command) {
            let value = self.parse_required_argument(true);
            return if command.starts_with("text") || command == "mbox" {
                value
            } else {
                js_trim(&value)
            };
        }
        if command == "begin" {
            return self.parse_environment();
        }
        self.supported = false;
        if command == "end" {
            Text::new()
        } else {
            text!('\\', command)
        }
    }

    fn parse_operator(
        &mut self,
        operator: &Text,
        bracket: bool,
        display_limits: bool,
        spaced: bool,
    ) -> Text {
        let mut use_display_limits = display_limits;
        let at = self.position
            + self.source.0[self.position..]
                .iter()
                .take_while(|u| matches!(u, 0x20 | 0x09))
                .count();
        for modifier in ["limits", "nolimits"] {
            let prefix = format!("\\{modifier}");
            if self.source.slice(at..).starts_with(&prefix)
                && !self
                    .source
                    .syntax_at(at + prefix.len())
                    .is_some_and(|c| c.is_ascii_alphabetic())
            {
                use_display_limits = modifier == "limits";
                self.position = at + prefix.len();
                break;
            }
        }
        let mut lower = None;
        let mut upper = None;
        loop {
            let at = self.position
                + self.source.0[self.position..]
                    .iter()
                    .take_while(|u| matches!(u, 0x20 | 0x09))
                    .count();
            let kind = self.source.syntax_at(at);
            if !matches!(kind, Some('_' | '^')) {
                break;
            }
            self.position = at + 1;
            let value = normalize_output(&self.parse_required_argument(false)).replace(' ', "");
            let slot = if kind == Some('_') {
                &mut lower
            } else {
                &mut upper
            };
            if slot.is_some() {
                self.supported = false;
            }
            *slot = Some(value);
        }
        if self.display && use_display_limits && (lower.is_some() || upper.is_some()) {
            let index = self.nodes.len();
            self.nodes.push(LayoutNode::Operator {
                operator: operator.clone(),
                lower,
                upper,
            });
            return layout_marker(index);
        }
        let mut rendered = operator.clone();
        if let Some(lower) = lower {
            rendered.push(if bracket {
                text!('[', lower, ']')
            } else {
                format_script(&lower, true)
            });
        }
        if let Some(upper) = upper {
            rendered.push(format_script(&upper, false));
        }
        if spaced {
            text!(' ', rendered, ' ')
        } else {
            rendered
        }
    }

    /// Upstream `parseScripts`: parse `x^2_3`-style script groups, choosing
    /// between Unicode scripts and a stacked layout node.
    fn parse_scripts(&mut self, initial_marker: char) -> Text {
        fn parse_marker(
            this: &mut LatexParser<'_, '_>,
            marker: char,
            sub: &mut Option<Text>,
            sup: &mut Option<Text>,
            order: &mut Vec<bool>,
        ) {
            let is_sub = marker == '_';
            this.script_depth += 1;
            let value = this.parse_required_argument(false);
            this.script_depth -= 1;
            if is_sub {
                *sub = Some(value);
            } else {
                *sup = Some(value);
            }
            order.push(is_sub);
        }

        let mut sub: Option<Text> = None;
        let mut sup: Option<Text> = None;
        let mut order: Vec<bool> = Vec::new(); // true = sub, false = sup
        parse_marker(self, initial_marker, &mut sub, &mut sup, &mut order);

        let mut next_position = self.position;
        while next_position < self.source.len()
            && self
                .source
                .syntax_at(next_position)
                .is_some_and(is_whitespace_char)
        {
            next_position += 1;
        }
        let next_marker = self.source.syntax_at(next_position);
        if matches!(next_marker, Some('^') | Some('_')) && next_marker != Some(initial_marker) {
            self.position = next_position + 1;
            parse_marker(
                self,
                next_marker.expect("checked above"),
                &mut sub,
                &mut sup,
                &mut order,
            );
        }

        let sub_unicode = sub
            .as_ref()
            .and_then(|value| format_unicode_script(value, true));
        let sup_unicode = sup
            .as_ref()
            .and_then(|value| format_unicode_script(value, false));
        let layout_marker_start = LAYOUT_MARKER_START.encode_utf8(&mut [0; 4]).to_string();
        let can_use_layout = [sub.as_ref(), sup.as_ref()]
            .into_iter()
            .flatten()
            .all(|value| {
                let has_slash = value.find("/").is_some();
                let marker_start = value.find(&layout_marker_start).is_some();
                let long_lowercase = value.points().count() > 1
                    && !value.points().any(|p| matches!(p, 65..=90 | 0x2a | 0x2217));
                !(has_slash || (!marker_start && long_lowercase))
            });
        let needs_layout = self.display
            && can_use_layout
            && (self.script_depth > 0
                || sub.is_some() && sub_unicode.is_none()
                || sup.is_some() && sup_unicode.is_none());
        if !needs_layout {
            let empty = Text::new();
            let mut result = Text::new();
            for is_sub in order {
                if is_sub {
                    result.push(
                        sub_unicode
                            .clone()
                            .unwrap_or_else(|| format_script(sub.as_ref().unwrap_or(&empty), true)),
                    );
                } else {
                    result.push(
                        sup_unicode.clone().unwrap_or_else(|| {
                            format_script(sup.as_ref().unwrap_or(&empty), false)
                        }),
                    );
                }
            }
            return result;
        }

        let index = self.nodes.len();
        self.nodes.push(LayoutNode::Script {
            lower: sub.map(|value| normalize_output(&value)),
            upper: sup.map(|value| normalize_output(&value)),
        });
        layout_marker(index)
    }

    fn parse_required_argument(&mut self, stack_fractions: bool) -> Text {
        let previous = self.stack_fractions;
        self.stack_fractions = previous && stack_fractions;
        let value = self.parse_required_argument_value();
        self.stack_fractions = previous;
        value
    }
    fn parse_required_argument_value(&mut self) -> Text {
        self.skip_whitespace();
        match self.peek() {
            None => {
                self.supported = false;
                Text::new()
            }
            Some('{') => {
                self.take();
                self.parse_sequence(Some('}'))
            }
            Some('\\') => self.parse_command(),
            Some(_) => Text(vec![self.take().expect("peeked code unit")]),
        }
    }
    fn parse_optional_argument(&mut self) -> Option<Text> {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.take();
        }
        if self.peek() != Some('[') {
            return None;
        }
        let Some(relative) = self.source.slice(self.position + 1..).find("]") else {
            self.supported = false;
            return None;
        };
        let end = self.position + 1 + relative;
        let value = self.source.slice(self.position + 1..end);
        self.position = end + 1;
        Some(self.render_nested(&value, true))
    }
    fn read_raw_group(&mut self) -> Option<Text> {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.take();
        }
        if self.peek() != Some('{') {
            self.supported = false;
            return None;
        }
        self.take();
        let start = self.position;
        let mut depth = 1;
        while let Some(character) = self.peek() {
            if character == '\\' {
                self.take();
                self.take();
                continue;
            }
            if character == '{' {
                depth += 1;
            }
            if character == '}' {
                depth -= 1;
            }
            if depth == 0 {
                let value = self.source.slice(start..self.position);
                self.take();
                return Some(value);
            }
            self.take();
        }
        self.supported = false;
        None
    }
    fn parse_environment(&mut self) -> Text {
        let Some(environment) = self.read_raw_group().filter(|s| !s.is_empty()) else {
            return Text::new();
        };
        let Some(environment) = environment.to_utf8() else {
            self.supported = false;
            return Text::new();
        };
        let environment = environment.as_str();
        let marker = format!("\\end{{{environment}}}");
        let Some(relative) = self.source.slice(self.position..).find(&marker) else {
            self.supported = false;
            return Text::new();
        };
        let body = self.source.slice(self.position..self.position + relative);
        self.position += relative + marker.encode_utf16().count();
        if matches!(environment, "equation" | "equation*" | "displaymath") {
            return js_trim(&self.render_nested(&body, true));
        }
        if matches!(
            environment,
            "aligned"
                | "align"
                | "align*"
                | "alignedat"
                | "alignat"
                | "alignat*"
                | "gather"
                | "gathered"
                | "multline"
                | "multline*"
                | "split"
        ) {
            let aligned_at = matches!(environment, "alignedat" | "alignat" | "alignat*");
            let aligned_body = if aligned_at {
                strip_preamble(&body)
            } else {
                body
            };
            let mut rendered = Vec::new();
            for row in environment_rows(&aligned_body) {
                let cells = row.split("&");
                let source = if aligned_at {
                    Text::join(cells.chunks(2).map(|p| Text::join(p, "")), " ")
                } else {
                    Text::join(&cells, "")
                };
                let value = js_trim(&self.render_nested(&source, true));
                if !value.is_empty() {
                    rendered.push(value);
                }
            }
            return Text::join(rendered, "\n");
        }
        if matches!(environment, "cases" | "cases*") {
            return self.render_cases(&body);
        }
        if matches!(
            environment,
            "array"
                | "matrix"
                | "smallmatrix"
                | "pmatrix"
                | "bmatrix"
                | "Bmatrix"
                | "vmatrix"
                | "Vmatrix"
        ) {
            return self.render_matrix(
                environment,
                &if environment == "array" {
                    strip_preamble(&body)
                } else {
                    body
                },
            );
        }
        self.supported = false;
        body
    }
    fn render_cells(&mut self, body: &Text) -> Vec<Vec<Text>> {
        environment_rows(body)
            .iter()
            .map(|row| {
                row.split("&")
                    .iter()
                    .map(|cell| js_trim(&self.render_nested(cell, false)))
                    .collect::<Vec<_>>()
            })
            .filter(|row| row.iter().any(|s| !s.is_empty()))
            .collect()
    }
    /// Upstream `renderCases` (v0.99.1 delta): pad the value column, drop the
    /// delimiter on condition-less rows, and stack multi-row cases as a matrix
    /// layout node with a bare delimiter gap between even rows.
    fn render_cases(&mut self, body: &Text) -> Text {
        let rows = self.render_cells(body);
        let strip_trailing_comma = |value: &Text| -> Text {
            // value.replace(/,\s*$/, ""): a trailing comma plus whitespace to
            // the end (all JS whitespace is single UTF-16 units).
            if let Some(comma_at) = value.0.iter().rposition(|&unit| unit == u16::from(b',')) {
                if value.0[comma_at + 1..]
                    .iter()
                    .all(|&unit| whitespace(u32::from(unit)))
                {
                    return value.slice(..comma_at);
                }
            }
            value.clone()
        };
        let empty = Text::new();
        let value_width = rows
            .iter()
            .map(|row| visible_width(&strip_trailing_comma(row.first().unwrap_or(&empty))))
            .max()
            .unwrap_or(0);
        let contents: Vec<Text> = rows
            .iter()
            .map(|row| {
                let value = strip_trailing_comma(row.first().unwrap_or(&empty));
                let condition = row.get(1).unwrap_or(&empty);
                if condition.is_empty() {
                    return value;
                }
                let prefix = if natural_condition(condition) {
                    " "
                } else {
                    " if "
                };
                let padding = PROTECTED_SPACE
                    .to_string()
                    .repeat(value_width.saturating_sub(visible_width(&value)));
                text!(value, padding, prefix, condition)
            })
            .collect();
        if contents.len() <= 1 {
            return match contents.into_iter().next() {
                None => Text::new(),
                Some(content) => text!('⎧', ' ', content),
            };
        }

        let length = contents.len();
        let middle = length / 2;
        let mut visual_rows: Vec<Option<Text>> = contents.into_iter().map(Some).collect();
        if length.is_multiple_of(2) {
            visual_rows.insert(middle, None);
        }
        let lines: Vec<Text> = visual_rows
            .iter()
            .enumerate()
            .map(|(index, content)| {
                let delimiter = if index == 0 {
                    '⎧'
                } else if index == visual_rows.len() - 1 {
                    '⎩'
                } else {
                    '⎨'
                };
                match content {
                    None => Text::from(delimiter),
                    Some(content) => text!(delimiter, ' ', content),
                }
            })
            .collect();
        let index = self.nodes.len();
        self.nodes.push(LayoutNode::Matrix {
            lines,
            baseline: middle,
        });
        layout_marker(index)
    }
    fn render_matrix(&mut self, environment: &str, body: &Text) -> Text {
        let matrix = self.render_cells(body);
        let columns = matrix.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<_> = (0..columns)
            .map(|column| {
                matrix
                    .iter()
                    .map(|row| row.get(column).map_or(0, visible_width))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let rows: Vec<_> = matrix
            .iter()
            .map(|row| {
                Text::join(
                    widths.iter().enumerate().map(|(column, width)| {
                        let cell = row.get(column).cloned().unwrap_or_default();
                        let padding = PROTECTED_SPACE
                            .to_string()
                            .repeat(width.saturating_sub(visible_width(&cell)));
                        text!(cell, padding)
                    }),
                    " │ ",
                )
            })
            .collect();
        let lines = if matches!(environment, "array" | "matrix" | "smallmatrix") {
            rows
        } else {
            let delimiters = match environment {
                "pmatrix" => ['⎛', '⎞', '⎜', '⎟', '⎝', '⎠'],
                "bmatrix" => ['⎡', '⎤', '⎢', '⎥', '⎣', '⎦'],
                "Bmatrix" => ['⎧', '⎫', '⎨', '⎬', '⎩', '⎭'],
                "vmatrix" => ['│'; 6],
                "Vmatrix" => ['║'; 6],
                _ => {
                    self.supported = false;
                    return Text::join(rows, "\n");
                }
            };
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let offset = if index == 0 {
                        0
                    } else if index == rows.len() - 1 {
                        4
                    } else {
                        2
                    };
                    text!(delimiters[offset], ' ', row, ' ', delimiters[offset + 1])
                })
                .collect()
        };
        if lines.len() <= 1 {
            return lines.into_iter().next().unwrap_or_default();
        }
        let index = self.nodes.len();
        self.nodes.push(LayoutNode::Matrix { lines, baseline: 0 });
        layout_marker(index)
    }
    fn render_nested(&mut self, source: &Text, stack_fractions: bool) -> Text {
        match LatexParser::new(source, self.nodes, self.display && stack_fractions).render() {
            Some(rendered) => rendered,
            None => {
                self.supported = false;
                source.clone()
            }
        }
    }
}

/// UTF-16 form of the upstream row-separator regex. An unclosed optional
/// row-spacing suffix remains in the following row, just as in JS splitting.
fn environment_rows(body: &Text) -> Vec<Text> {
    let mut rows = Vec::new();
    let mut start = 0;
    while let Some(relative) = body.slice(start..).find("\\\\") {
        let at = start + relative;
        rows.push(body.slice(start..at));
        start = at + 2;
        if body.syntax_at(start) == Some('[') {
            let suffix = body.slice(start + 1..);
            if let Some(end) = suffix.find("]") {
                if !suffix.0[..end].contains(&u16::from(b'\n')) {
                    start += end + 2;
                }
            }
        }
    }
    rows.push(body.slice(start..));
    rows
}
fn strip_preamble(body: &Text) -> Text {
    let trimmed = trim_start(body);
    if trimmed.starts_with("{") {
        if let Some(end) = trimmed.find("}") {
            return trimmed.slice(end + 1..);
        }
    }
    body.clone()
}
fn natural_condition(condition: &Text) -> bool {
    ["if", "when", "for", "otherwise"].iter().any(|word| {
        condition.0.len() >= word.len()
            && condition.0[..word.len()]
                .iter()
                .zip(word.bytes())
                .all(|(&unit, b)| {
                    char::from_u32(u32::from(unit))
                        .is_some_and(|c| c.eq_ignore_ascii_case(&char::from(b)))
                })
            && !condition
                .syntax_at(word.len())
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Render while preserving JavaScript UTF-16 input/output units, including lone
/// surrogates. Unsupported/malformed TeX returns None, not replacement text.
/// Use this at boundaries that must retain raw JavaScript string units.
pub fn render_latex_utf16(source: &[u16], options: RenderLatexOptions) -> Option<Vec<u16>> {
    let source = Text(source.to_vec());
    let mut nodes = Vec::new();
    let rendered = LatexParser::new(&source, &mut nodes, options.display).render()?;
    if nodes.is_empty() {
        return Some(rendered.replace(PROTECTED_SPACE, " ").0);
    }
    let layout = render_layout(&rendered, &nodes);
    let indentation = layout
        .lines
        .iter()
        .filter(|line| !js_trim(line).is_empty())
        .map(|line| {
            line.0
                .iter()
                .take_while(|u| whitespace(u32::from(**u)))
                .count()
        })
        .min();
    let lines = layout
        .lines
        .iter()
        .map(|line| trim_end(&line.slice(indentation.unwrap_or(usize::MAX).min(line.len())..)))
        .collect::<Vec<_>>();
    Some(
        trim_end(&Text::join(lines, "\n"))
            .replace(PROTECTED_SPACE, " ")
            .0,
    )
}

/// Render terminal UTF-8. Raw lone surrogate units are replaced only after
/// parsing, normalization, width calculation, and layout have all completed,
/// matching Node's UTF-8 output encoding. For raw units use render_latex_utf16.
pub fn render_latex(source: &str, options: RenderLatexOptions) -> Option<String> {
    render_latex_utf16(&source.encode_utf16().collect::<Vec<_>>(), options)
        .map(|units| String::from_utf16_lossy(&units))
}
