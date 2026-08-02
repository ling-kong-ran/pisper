use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
    },
    Frame,
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, Approval, LiveTurn, SettingsPicker, SlashKind, View},
    model::{ChatMessage, MessageAttachment, RunActivity, ToolActivity},
};

const BG: Color = Color::Rgb(9, 11, 15);
const SURFACE: Color = Color::Rgb(15, 19, 25);
const RAISED: Color = Color::Rgb(21, 27, 35);
const RULE: Color = Color::Rgb(38, 48, 59);
const TEXT: Color = Color::Rgb(228, 233, 240);
const MUTED: Color = Color::Rgb(135, 147, 161);
const FAINT: Color = Color::Rgb(82, 96, 110);
const ACCENT: Color = Color::Rgb(89, 208, 220);
const GREEN: Color = Color::Rgb(139, 212, 156);
const AMBER: Color = Color::Rgb(231, 183, 106);
const RED: Color = Color::Rgb(240, 124, 130);
const VIOLET: Color = Color::Rgb(192, 167, 242);
const BLUE: Color = Color::Rgb(130, 174, 239);
const CONVERSATION_WIDTH: u16 = 110;
const WELCOME_WIDTH: u16 = 88;
const SLASH_HEIGHT: u16 = 22;
const ROLE_GUTTER_WIDTH: usize = 3;
const ACTIVITY_GUTTER_WIDTH: usize = 6;
const PISPER_LOGO: [(&str, &str); 5] = [
    ("████  █ █████  ", "████  █████ ████ "),
    ("█   █ █ █      ", "█   █ █     █   █"),
    ("████  █ █████  ", "████  ████  ████ "),
    ("█     █     █  ", "█     █     █  █ "),
    ("█     █ █████  ", "█     █████ █   █"),
];
const PULSE_FRAMES: [&str; 8] = [
    "▁▂▅█▅▂▁",
    "▂▅█▅▂▁▁",
    "▅█▅▂▁▁▂",
    "█▅▂▁▁▂▅",
    "▅▂▁▁▂▅█",
    "▂▁▁▂▅█▅",
    "▁▁▂▅█▅▂",
    "▁▂▅█▅▂▁",
];

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    if app.approval.is_some() {
        let chat_minimum = if area.height >= 16 { 3 } else { 0 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(chat_minimum),
                Constraint::Length(approval_panel_height(app, area)),
                Constraint::Length(1),
            ])
            .split(area);
        match app.view {
            View::Chat => render_chat(frame, app, chunks[0]),
            View::Events => {
                render_events(frame, app, centered_width(chunks[0], CONVERSATION_WIDTH))
            }
        }
        render_approval(frame, app, chunks[1]);
        render_status(frame, app, chunks[2]);
        return;
    }

    if matches!(app.view, View::Chat) && app.messages.is_empty() && app.live.is_none() {
        let composer = render_welcome(frame, app, area);
        render_overlays(frame, app, composer, area);
        return;
    }

    let composer_height = if area.height >= 18 { 7 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    let run_state = chunks[1];
    let composer = chunks[2];
    let status = chunks[3];

    match app.view {
        View::Chat => render_chat(frame, app, chunks[0]),
        View::Events => render_events(frame, app, centered_width(chunks[0], CONVERSATION_WIDTH)),
    }
    render_run_state(frame, app, run_state);
    render_composer(frame, app, composer);
    render_status(frame, app, status);

    render_overlays(frame, app, composer, area);
}

fn render_welcome(frame: &mut Frame, app: &App, area: Rect) -> Rect {
    let body = area;
    let composer_height: u16 = if area.height >= 18 { 7 } else { 3 };
    let full_logo = area.width >= 52 && body.height >= composer_height.saturating_add(8);
    let logo_height: u16 = if full_logo {
        5
    } else if body.height >= composer_height.saturating_add(3) {
        1
    } else {
        0
    };
    let gap = u16::from(logo_height > 0);
    let content_height = logo_height
        .saturating_add(gap)
        .saturating_add(composer_height)
        .saturating_add(1)
        .min(body.height);
    let content_y = body
        .y
        .saturating_add(body.height.saturating_sub(content_height) / 2);
    let rail = centered_width(
        Rect::new(area.x, content_y, area.width, content_height),
        WELCOME_WIDTH,
    );
    let logo = Rect::new(rail.x, rail.y, rail.width, logo_height);
    let composer = Rect::new(
        rail.x,
        rail.y.saturating_add(logo_height).saturating_add(gap),
        rail.width,
        composer_height.min(content_height.saturating_sub(logo_height + gap)),
    );
    let status = Rect::new(
        rail.x,
        composer.y.saturating_add(composer.height),
        rail.width,
        u16::from(composer.y.saturating_add(composer.height) < body.y.saturating_add(body.height)),
    );

    if logo.height > 0 {
        render_welcome_logo(frame, logo, full_logo);
    }
    render_composer(frame, app, composer);
    render_status(frame, app, status);
    composer
}

fn render_welcome_logo(frame: &mut Frame, area: Rect, full: bool) {
    let lines = if full {
        PISPER_LOGO
            .iter()
            .map(|(left, right)| {
                Line::from(vec![
                    Span::styled(
                        *left,
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        *right,
                        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                ])
            })
            .collect::<Vec<_>>()
    } else {
        vec![Line::from(vec![
            Span::styled(
                "PIS",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "PER",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ])]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(BG)),
        area,
    );
}

fn render_overlays(frame: &mut Frame, app: &App, composer: Rect, area: Rect) {
    if app.slash_open() {
        render_slash(frame, app, composer);
    }
    if app.session_picker {
        render_sessions(frame, app, area);
    }
    if app.path_picker {
        render_path_picker(frame, app, area);
    }
    if app.settings_picker.is_some() {
        render_settings_picker(frame, app, area);
    }
}

fn render_chat(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    let content_width = area.width as usize;
    for message in &app.messages {
        push_message(&mut lines, message, content_width);
    }
    if let Some(live) = &app.live {
        push_live(
            &mut lines,
            live,
            app.status == "thinking",
            content_width,
            area.height.saturating_sub(2) as usize,
            app.status_frame,
        );
    }
    let viewport = inset(area, 0, 1);
    if viewport.width == 0 || viewport.height == 0 {
        return;
    }
    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().fg(TEXT).bg(BG))
        .wrap(Wrap { trim: false });
    let rendered_rows = paragraph.line_count(viewport.width);
    let max_scroll = rendered_rows
        .saturating_sub(viewport.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = max_scroll.saturating_sub(app.scroll.min(max_scroll));
    frame.render_widget(paragraph.scroll((scroll, 0)), viewport);
}

fn render_run_state(frame: &mut Frame, app: &App, area: Rect) {
    let (label, color, animate) = if app.approval.is_some() {
        ("Approval required".to_owned(), AMBER, false)
    } else if app.is_streaming() {
        let label = match app.status.as_str() {
            "thinking" => "Thinking".to_owned(),
            "streaming" => "Responding".to_owned(),
            value if value.starts_with("running ") => {
                format!("Running {}", value.trim_start_matches("running "))
            }
            _ => "Running".to_owned(),
        };
        (label, ACCENT, true)
    } else if app.status_error {
        (
            runtime_error_label(&app.status, area.width.saturating_sub(2) as usize),
            RED,
            false,
        )
    } else {
        return;
    };

    let line = if animate {
        const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        let frame = FRAMES[(app.status_frame as usize / 5) % FRAMES.len()];
        let dots = ".".repeat((app.status_frame as usize / 10) % 3 + 1);
        Line::from(vec![
            Span::styled(
                format!("{frame} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{label}{dots}"), Style::default().fg(color)),
        ])
    } else {
        Line::from(Span::styled(label, Style::default().fg(color)))
    };
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);
}

fn runtime_error_label(error: &str, width: usize) -> String {
    let normalized = error.to_lowercase();
    let label = if normalized.contains("overloaded") {
        "Error · Provider overloaded · automatic retries exhausted"
    } else if normalized.contains("stream_read_error") || normalized.contains("stream read error") {
        "Error · Response stream interrupted · automatic retries exhausted"
    } else if normalized.contains("rate limit") || normalized.contains("too many requests") {
        "Error · Provider rate limit · automatic retries exhausted"
    } else {
        error
    };
    single_line(
        &format!("Error · {}", label.trim_start_matches("Error · ")),
        width,
    )
}

fn push_message(lines: &mut Vec<Line<'static>>, message: &ChatMessage, width: usize) {
    if message.role == "user" {
        push_labeled_text(lines, "›", BLUE, &message.text, width, false);
        push_attachment_lines(lines, &message.attachments, width);
        lines.push(Line::default());
        return;
    }
    if message.role != "agent" {
        return;
    }
    if let Some(activity) = &message.run_activity {
        push_activity(lines, activity, width);
    }
    if !message.text.is_empty() {
        push_markdown(lines, "●", ACCENT, &message.text, width);
    }
    push_attachment_lines(lines, &message.attachments, width);
    lines.push(Line::default());
}

fn push_live(
    lines: &mut Vec<Line<'static>>,
    live: &LiveTurn,
    thinking: bool,
    width: usize,
    viewport_rows: usize,
    animation_frame: u64,
) {
    let (thinking_rows, tool_rows) = live_activity_budget(viewport_rows);
    if !live.thinking.is_empty() || (thinking && live.text.is_empty()) {
        push_thinking(lines, &live.thinking, thinking, width, thinking_rows);
    }
    push_tool_group(lines, &live.tools, width, tool_rows, animation_frame);
    push_tool_agents(lines, &live.tools);
    if !live.text.is_empty() {
        push_markdown(lines, "●", ACCENT, &live.text, width);
    }
}

fn live_activity_budget(viewport_rows: usize) -> (usize, usize) {
    match viewport_rows {
        0..=7 => (1, 1),
        8..=12 => (2, 2),
        13..=20 => (3, 3),
        _ => (3, 4),
    }
}

fn push_activity(lines: &mut Vec<Line<'static>>, activity: &RunActivity, width: usize) {
    if !activity.thinking_text.is_empty() {
        push_thinking(lines, &activity.thinking_text, false, width, 2);
    }
    push_tool_group(lines, &activity.tools, width, 3, 0);
    if activity.agents.is_empty() {
        push_tool_agents(lines, &activity.tools);
    } else {
        push_agent_values(lines, activity.agents.iter());
    }
}

fn push_tool_agents(lines: &mut Vec<Line<'static>>, tools: &[ToolActivity]) {
    push_agent_values(lines, tools.iter().filter_map(|tool| tool.agent.as_ref()));
}

fn push_agent_values<'a>(lines: &mut Vec<Line<'static>>, agents: impl Iterator<Item = &'a Value>) {
    for agent in agents {
        let name = ["canonicalName", "taskName", "name"]
            .into_iter()
            .find_map(|key| agent.get(key).and_then(Value::as_str))
            .unwrap_or("subagent");
        let status = agent
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("running");
        let (glyph, color) = match status {
            "completed" | "done" => ("✓", GREEN),
            "failed" => ("×", RED),
            _ => ("●", AMBER),
        };
        lines.push(Line::from(vec![
            activity_label_span("SUB", VIOLET),
            Span::styled(format!("{glyph}  "), Style::default().fg(color)),
            Span::styled(name.to_owned(), Style::default().fg(VIOLET)),
            Span::styled(format!("  ·  {status}"), Style::default().fg(MUTED)),
        ]));
        let detail = agent
            .get("output")
            .and_then(Value::as_str)
            .or_else(|| agent.get("message").and_then(Value::as_str))
            .unwrap_or("isolated context · inherited execution mode");
        lines.push(Line::from(vec![
            activity_gutter(),
            Span::styled("└ ", Style::default().fg(FAINT)),
            Span::styled(single_line(detail, 80), Style::default().fg(MUTED)),
        ]));
    }
}

fn push_thinking(
    lines: &mut Vec<Line<'static>>,
    value: &str,
    active: bool,
    width: usize,
    max_lines: usize,
) {
    let content_width = width.saturating_sub(ACTIVITY_GUTTER_WIDTH).max(12);
    let content = wrapped_tail(value, content_width, max_lines);
    let label_style = if active {
        Style::default()
            .fg(AMBER)
            .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK)
    } else {
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
    };
    let first = content.first().cloned().unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(format!("{:<ACTIVITY_GUTTER_WIDTH$}", "THINK"), label_style),
        Span::styled(first, Style::default().fg(TEXT)),
    ]));
    for line in content.into_iter().skip(1) {
        lines.push(Line::from(vec![
            activity_gutter(),
            Span::styled(line, Style::default().fg(TEXT)),
        ]));
    }
    if active && value.is_empty() {
        if let Some(last) = lines.last_mut() {
            last.spans
                .push(Span::styled("Thinking…", Style::default().fg(MUTED)));
        }
    }
}

fn wrapped_tail(value: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    for source in value.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut line = String::new();
        let mut line_width = 0;
        for character in source.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if line_width + character_width > width && !line.is_empty() {
                wrapped.push(std::mem::take(&mut line));
                line_width = 0;
            }
            line.push(character);
            line_width += character_width;
        }
        if !line.is_empty() {
            wrapped.push(line);
        }
    }
    if wrapped.len() > max_lines {
        wrapped.drain(..wrapped.len() - max_lines);
        if let Some(first) = wrapped.first_mut() {
            *first = format!("…{}", first.chars().skip(1).collect::<String>());
        }
    }
    wrapped
}

fn push_tool_group(
    lines: &mut Vec<Line<'static>>,
    tools: &[ToolActivity],
    width: usize,
    max_rows: usize,
    animation_frame: u64,
) {
    if tools.is_empty() || max_rows == 0 {
        return;
    }
    if max_rows == 1 {
        lines.push(tool_line(
            tools.last().expect("tools is not empty"),
            width,
            true,
            animation_frame,
        ));
        return;
    }
    let visible_rows = tools.len().min(max_rows);
    let tool_rows = if tools.len() > max_rows {
        visible_rows.saturating_sub(1)
    } else {
        visible_rows
    };
    let hidden = tools.len().saturating_sub(tool_rows);
    if hidden > 0 {
        lines.push(Line::from(vec![
            activity_label_span("TOOL", ACCENT),
            Span::styled("│  ", Style::default().fg(RULE)),
            Span::styled(
                format!(
                    "{hidden} earlier tool{}",
                    if hidden == 1 { "" } else { "s" }
                ),
                Style::default().fg(MUTED),
            ),
        ]));
    }
    for (index, tool) in tools
        .iter()
        .skip(tools.len().saturating_sub(tool_rows))
        .enumerate()
    {
        lines.push(tool_line(
            tool,
            width,
            hidden == 0 && index == 0,
            animation_frame,
        ));
    }
}

fn tool_line(
    tool: &ToolActivity,
    width: usize,
    show_label: bool,
    animation_frame: u64,
) -> Line<'static> {
    const NAME_WIDTH: usize = 9;
    const META_WIDTH: usize = 11;
    const PREFIX_WIDTH: usize = ACTIVITY_GUTTER_WIDTH + 3 + 2 + NAME_WIDTH;

    let status_color = match tool.status.as_str() {
        "error" => RED,
        "done" => GREEN,
        _ => AMBER,
    };
    let status_glyph = match tool.status.as_str() {
        "error" => "×",
        "done" => "✓",
        _ => {
            const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
            FRAMES[(animation_frame as usize / 3) % FRAMES.len()]
        }
    };
    let detail_width = width.saturating_sub(PREFIX_WIDTH + META_WIDTH).max(8);
    let detail = tool_detail(tool, detail_width);
    let detail_padding = detail_width.saturating_sub(detail.width());
    let name = single_line(&tool.name.to_lowercase(), NAME_WIDTH.saturating_sub(1));
    let name_padding = NAME_WIDTH.saturating_sub(name.width());
    Line::from(vec![
        if show_label {
            activity_label_span("TOOL", ACCENT)
        } else {
            activity_gutter()
        },
        Span::styled("│  ", Style::default().fg(RULE)),
        Span::styled(
            format!("{status_glyph} "),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{name}{}", " ".repeat(name_padding)),
            Style::default().fg(ACCENT),
        ),
        Span::styled(
            format!("{detail}{}", " ".repeat(detail_padding)),
            Style::default().fg(BLUE),
        ),
        Span::styled(
            format!("{:>META_WIDTH$}", tool_duration(tool)),
            Style::default().fg(if tool.status == "running" {
                ACCENT
            } else {
                MUTED
            }),
        ),
    ])
}

fn tool_duration(tool: &ToolActivity) -> String {
    if tool.finished_at > tool.started_at && tool.started_at > 0 {
        let elapsed = tool.finished_at - tool.started_at;
        if elapsed >= 1000 {
            format!("{:.1} s", elapsed as f64 / 1000.0)
        } else {
            format!("{elapsed} ms")
        }
    } else if tool.status == "running" {
        "running".to_owned()
    } else {
        String::new()
    }
}

fn tool_detail(tool: &ToolActivity, max_width: usize) -> String {
    if !tool.message.is_empty() {
        return single_line(&tool.message, max_width);
    }
    if tool.args.is_object() {
        let values = tool
            .args
            .as_object()
            .into_iter()
            .flat_map(|object| object.values());
        for value in values {
            if let Some(text) = value.as_str() {
                return single_line(text, max_width);
            }
        }
    }
    String::new()
}

fn padded_label_span(label: &str, width: usize, color: Color) -> Span<'static> {
    Span::styled(
        format!("{label:<width$}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn role_label_span(label: &str, color: Color) -> Span<'static> {
    padded_label_span(label, ROLE_GUTTER_WIDTH, color)
}

fn activity_label_span(label: &str, color: Color) -> Span<'static> {
    padded_label_span(label, ACTIVITY_GUTTER_WIDTH, color)
}

fn role_gutter() -> Span<'static> {
    Span::raw(" ".repeat(ROLE_GUTTER_WIDTH))
}

fn activity_gutter() -> Span<'static> {
    Span::raw(" ".repeat(ACTIVITY_GUTTER_WIDTH))
}

fn push_labeled_text(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    color: Color,
    value: &str,
    width: usize,
    bold: bool,
) {
    let content_width = width.saturating_sub(ROLE_GUTTER_WIDTH).max(8);
    let wrapped = wrap_text(value, content_width);
    let text_style = if bold {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    for (index, line) in wrapped.into_iter().enumerate() {
        lines.push(Line::from(vec![
            if index == 0 {
                role_label_span(label, color)
            } else {
                role_gutter()
            },
            Span::styled(line, text_style),
        ]));
    }
}

#[derive(Clone)]
struct StyledPiece {
    text: String,
    style: Style,
}

fn push_markdown(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    color: Color,
    value: &str,
    width: usize,
) {
    let content_width = width.saturating_sub(ROLE_GUTTER_WIDTH).max(8);
    let mut label_used = false;
    let mut code_language = None::<String>;
    for source in value.lines() {
        let trimmed = source.trim_end();
        if let Some(fence) = trimmed.trim_start().strip_prefix("```") {
            if code_language.is_none() {
                code_language = Some(fence.trim().to_owned());
                let language = code_language
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("code");
                push_markdown_line(
                    lines,
                    &mut label_used,
                    label,
                    color,
                    vec![Span::styled(
                        format!("┌─ {language}"),
                        Style::default().fg(MUTED).bg(RAISED),
                    )],
                );
            } else {
                push_markdown_line(
                    lines,
                    &mut label_used,
                    label,
                    color,
                    vec![Span::styled(
                        "└".to_owned() + &"─".repeat(content_width.saturating_sub(1)),
                        Style::default().fg(RULE).bg(RAISED),
                    )],
                );
                code_language = None;
            }
            continue;
        }
        if code_language.is_some() {
            let (marker, code, style) = if trimmed.starts_with('+') {
                ("│ ", trimmed, Style::default().fg(GREEN).bg(RAISED))
            } else if trimmed.starts_with('-') {
                ("│ ", trimmed, Style::default().fg(RED).bg(RAISED))
            } else if trimmed.starts_with("@@") {
                ("│ ", trimmed, Style::default().fg(AMBER).bg(RAISED))
            } else {
                ("│ ", trimmed, Style::default().fg(TEXT).bg(RAISED))
            };
            push_markdown_line(
                lines,
                &mut label_used,
                label,
                color,
                vec![
                    Span::styled(marker, Style::default().fg(FAINT).bg(RAISED)),
                    Span::styled(single_line(code, content_width.saturating_sub(2)), style),
                ],
            );
            continue;
        }
        if trimmed.trim().is_empty() {
            lines.push(Line::default());
            continue;
        }
        let leading = trimmed.trim_start();
        let heading_marks = leading
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if (1..=6).contains(&heading_marks)
            && leading
                .chars()
                .nth(heading_marks)
                .is_some_and(char::is_whitespace)
        {
            let text = leading[heading_marks..].trim();
            let style = Style::default().fg(TEXT).add_modifier(Modifier::BOLD);
            push_markdown_line(
                lines,
                &mut label_used,
                label,
                color,
                vec![
                    Span::styled(
                        format!("{} ", "#".repeat(heading_marks.min(2))),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text.to_owned(), style),
                ],
            );
            continue;
        }
        let numbered = leading.split_once(". ").filter(|(number, _)| {
            !number.is_empty() && number.chars().all(|value| value.is_ascii_digit())
        });
        let (prefix, body, prefix_style) = if let Some(body) = leading
            .strip_prefix("- ")
            .or_else(|| leading.strip_prefix("* "))
        {
            ("• ".to_owned(), body, Style::default().fg(ACCENT))
        } else if let Some((number, body)) = numbered {
            (format!("{number}. "), body, Style::default().fg(ACCENT))
        } else if let Some(body) = leading.strip_prefix("> ") {
            ("│ ".to_owned(), body, Style::default().fg(VIOLET))
        } else {
            (String::new(), leading, Style::default())
        };
        let mut pieces = inline_pieces(body);
        if !prefix.is_empty() {
            pieces.insert(
                0,
                StyledPiece {
                    text: prefix,
                    style: prefix_style,
                },
            );
        }
        for wrapped in wrap_styled_pieces(&pieces, content_width) {
            push_markdown_line(lines, &mut label_used, label, color, wrapped);
        }
    }
    if code_language.is_some() {
        push_markdown_line(
            lines,
            &mut label_used,
            label,
            color,
            vec![Span::styled(
                "└".to_owned() + &"─".repeat(content_width.saturating_sub(1)),
                Style::default().fg(RULE).bg(RAISED),
            )],
        );
    }
}

fn push_markdown_line(
    lines: &mut Vec<Line<'static>>,
    label_used: &mut bool,
    label: &str,
    color: Color,
    mut content: Vec<Span<'static>>,
) {
    let mut spans = vec![if *label_used {
        role_gutter()
    } else {
        *label_used = true;
        role_label_span(label, color)
    }];
    spans.append(&mut content);
    lines.push(Line::from(spans));
}

fn inline_pieces(value: &str) -> Vec<StyledPiece> {
    let mut pieces = Vec::new();
    let mut buffer = String::new();
    let mut strong = false;
    let mut emphasis = false;
    let mut code = false;
    let flush = |pieces: &mut Vec<StyledPiece>, buffer: &mut String, strong, emphasis, code| {
        if buffer.is_empty() {
            return;
        }
        let mut style = if code {
            Style::default().fg(AMBER).bg(RAISED)
        } else {
            Style::default().fg(TEXT)
        };
        if strong {
            style = style.add_modifier(Modifier::BOLD);
        }
        if emphasis {
            style = style.add_modifier(Modifier::ITALIC);
        }
        pieces.push(StyledPiece {
            text: std::mem::take(buffer),
            style,
        });
    };
    let mut rest = value;
    while !rest.is_empty() {
        if rest.starts_with("**") {
            flush(&mut pieces, &mut buffer, strong, emphasis, code);
            strong = !strong;
            rest = &rest[2..];
        } else if rest.starts_with('`') {
            flush(&mut pieces, &mut buffer, strong, emphasis, code);
            code = !code;
            rest = &rest[1..];
        } else if rest.starts_with('*') {
            flush(&mut pieces, &mut buffer, strong, emphasis, code);
            emphasis = !emphasis;
            rest = &rest[1..];
        } else if rest.starts_with('[') && !code {
            let link = rest
                .find("](")
                .and_then(|middle| rest[middle + 2..].find(')').map(|end| (middle, end)));
            if let Some((middle, end)) = link {
                flush(&mut pieces, &mut buffer, strong, emphasis, code);
                pieces.push(StyledPiece {
                    text: rest[1..middle].to_owned(),
                    style: Style::default()
                        .fg(ACCENT)
                        .add_modifier(Modifier::UNDERLINED),
                });
                rest = &rest[middle + 2 + end + 1..];
            } else {
                buffer.push('[');
                rest = &rest[1..];
            }
        } else {
            let character = rest.chars().next().expect("rest is not empty");
            buffer.push(character);
            rest = &rest[character.len_utf8()..];
        }
    }
    flush(&mut pieces, &mut buffer, strong, emphasis, code);
    pieces
}

fn wrap_styled_pieces(pieces: &[StyledPiece], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut result = vec![Vec::new()];
    let mut line_width = 0usize;
    for piece in pieces {
        for word in piece.text.split_whitespace() {
            let mut remaining = word;
            loop {
                let separator = usize::from(line_width > 0);
                let available = width.saturating_sub(line_width + separator);
                if available == 0 {
                    result.push(Vec::new());
                    line_width = 0;
                    continue;
                }
                let (part, rest) = split_at_width(remaining, available);
                if line_width > 0 {
                    result.last_mut().expect("line exists").push(Span::raw(" "));
                    line_width += 1;
                }
                result
                    .last_mut()
                    .expect("line exists")
                    .push(Span::styled(part.to_owned(), piece.style));
                line_width += part.width();
                if rest.is_empty() {
                    break;
                }
                result.push(Vec::new());
                line_width = 0;
                remaining = rest;
            }
        }
    }
    result
}

fn split_at_width(value: &str, width: usize) -> (&str, &str) {
    let mut used = 0;
    for (index, character) in value.char_indices() {
        let next = used + character.width().unwrap_or(0);
        if next > width {
            if index == 0 {
                let end = character.len_utf8();
                return (&value[..end], &value[end..]);
            }
            return (&value[..index], &value[index..]);
        }
        used = next;
    }
    (value, "")
}

fn push_attachment_lines(
    lines: &mut Vec<Line<'static>>,
    attachments: &[MessageAttachment],
    width: usize,
) {
    for attachment in attachments {
        let kind = if attachment.kind.is_empty() {
            attachment.mime_type.split('/').next().unwrap_or("file")
        } else {
            &attachment.kind
        };
        let size = if attachment.size > 0 {
            format!(" · {}", format_bytes(attachment.size))
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            role_gutter(),
            Span::styled("+ ", Style::default().fg(ACCENT)),
            Span::styled(
                single_line(
                    &format!("{} · {kind}{size}", attachment.name),
                    width.saturating_sub(ROLE_GUTTER_WIDTH + 2),
                ),
                Style::default().fg(MUTED),
            ),
        ]));
    }
}

fn format_bytes(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}

fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let mut result = Vec::new();
    for source in value.lines() {
        if source.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0;
        for word in source.split_whitespace() {
            let word_width = word.width();
            let separator = usize::from(!current.is_empty());
            if current_width + separator + word_width > width && !current.is_empty() {
                result.push(std::mem::take(&mut current));
                current_width = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
        }
        if !current.is_empty() {
            result.push(current);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

fn render_events(frame: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(["TYPE", "EVENT", "STATE"])
        .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let rows = app.events.iter().rev().map(|event| {
        let color = match event.state.as_str() {
            "error" => RED,
            "waiting" | "active" => AMBER,
            "done" => GREEN,
            _ => MUTED,
        };
        Row::new([
            Cell::from(event.name.clone()).style(Style::default().fg(color)),
            Cell::from(event.detail.clone()).style(Style::default().fg(TEXT)),
            Cell::from(event.state.clone()).style(Style::default().fg(color)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Min(20),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .column_spacing(2)
    .style(Style::default().bg(BG));
    frame.render_widget(table, inset(area, 2, 1));
}

fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    let border = if app.status_error {
        RED
    } else if app.is_streaming() || !app.input.is_empty() || !app.attachments.is_empty() {
        ACCENT
    } else {
        RULE
    };
    let compact = area.height < 5;
    let composer = Block::default()
        .borders(if compact {
            Borders::TOP | Borders::BOTTOM
        } else {
            Borders::ALL
        })
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(SURFACE));
    let inner = composer.inner(area);
    frame.render_widget(composer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let prompt_width = 2usize;
    let available = inner
        .width
        .saturating_sub(prompt_width as u16)
        .saturating_sub(1) as usize;
    let (composer_input, composer_cursor) = app.composer_input();
    let (visible, cursor_width) = visible_input(&composer_input, composer_cursor, available);
    let input = Line::from(vec![
        Span::styled(
            "❯ ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if visible.is_empty() {
                "Message Pisper…".to_owned()
            } else {
                visible
            },
            Style::default().fg(if app.input.is_empty() { MUTED } else { TEXT }),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(input).style(Style::default().bg(SURFACE)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if !compact && inner.height >= 4 && !app.attachments.is_empty() {
        let names = app
            .attachments
            .iter()
            .map(|attachment| attachment.name.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("+{} ", app.attachments.len()),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    single_line(&names, inner.width.saturating_sub(5) as usize),
                    Style::default().fg(MUTED),
                ),
            ]))
            .style(Style::default().bg(SURFACE)),
            Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
        );
    }

    if !compact && inner.height >= 4 {
        let separator_y = inner.y.saturating_add(inner.height.saturating_sub(2));
        frame.render_widget(
            Paragraph::new("─".repeat(inner.width as usize))
                .style(Style::default().fg(RULE).bg(SURFACE)),
            Rect::new(inner.x, separator_y, inner.width, 1),
        );
        let queue = if app.queued_count() > 0 {
            format!("    {} queued", app.queued_count())
        } else if app.is_streaming() {
            "    current run active".to_owned()
        } else if app.slash_open() {
            "    Tab complete · Enter select".to_owned()
        } else {
            "    Enter submit".to_owned()
        };
        let controls = Line::from(vec![
            Span::styled("+ attach  / commands", Style::default().fg(MUTED)),
            Span::styled(
                queue,
                Style::default().fg(if app.is_streaming() { ACCENT } else { MUTED }),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(controls).style(Style::default().bg(SURFACE)),
            Rect::new(inner.x, separator_y.saturating_add(1), inner.width, 1),
        );
        if inner.width >= 4 {
            frame.render_widget(
                Paragraph::new("↑")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(ACCENT).bg(RAISED)),
                Rect::new(
                    inner.x.saturating_add(inner.width.saturating_sub(3)),
                    separator_y.saturating_add(1),
                    3,
                    1,
                ),
            );
        }
    }

    if !app.session_picker
        && !app.path_picker
        && app.settings_picker.is_none()
        && app.approval.is_none()
    {
        frame.set_cursor_position(Position::new(
            inner
                .x
                .saturating_add(prompt_width as u16)
                .saturating_add(cursor_width as u16),
            inner.y,
        ));
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);
    let context = app
        .context_percent
        .map(|value| format!(" · {value:.0}% ctx"))
        .unwrap_or_default();
    let mode = format!("[{}]", display_execution_mode(&app.execution_mode));
    let full = format!(
        "{} · {mode} · {}{context}",
        display_model(&app.model),
        shorten_path(&app.cwd),
    );
    let model_mode = format!("{} · {mode}{context}", display_model(&app.model));
    let compact = format!("{mode} · {}{context}", shorten_path(&app.cwd));
    let pulse = app
        .is_streaming()
        .then(|| PULSE_FRAMES[(app.status_frame as usize) % PULSE_FRAMES.len()]);
    let pulse_width = pulse.map_or(0, |value| value.width().saturating_add(2));
    let available = (columns[0].width as usize).saturating_sub(pulse_width);
    let left = if full.width() <= available {
        full
    } else if model_mode.width() <= available {
        model_mode
    } else if compact.width() <= available {
        compact
    } else {
        mode
    };
    let mut left_spans = Vec::new();
    if let Some(pulse) = pulse {
        left_spans.push(Span::styled(
            pulse,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        left_spans.push(Span::raw("  "));
    }
    left_spans.push(Span::styled(left, Style::default().fg(MUTED)));
    frame.render_widget(Paragraph::new(Line::from(left_spans)), columns[0]);
    let tasks = app
        .session
        .task_list
        .as_ref()
        .map(|list| {
            let done = list
                .items
                .iter()
                .filter(|item| item.status == "completed")
                .count();
            format!("{done}/{} tasks · ", list.items.len())
        })
        .unwrap_or_default();
    let agents = visible_agent_count(app);
    let agents = if agents > 0 {
        format!("{agents} subagent · ")
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{tasks}{agents}{}UTF-8",
            if app.queued_count() > 0 {
                format!("{} queued · ", app.queued_count())
            } else {
                String::new()
            }
        ))
        .alignment(Alignment::Right)
        .style(Style::default().fg(MUTED)),
        columns[1],
    );
}

fn slash_menu_area(composer: Rect, item_count: usize) -> Rect {
    let desired_height = (item_count.min(8) as u16)
        .saturating_mul(2)
        .saturating_add(5)
        .clamp(7, SLASH_HEIGHT);
    let available_height = composer.y.saturating_sub(2).max(1);
    let height = desired_height.min(available_height);
    Rect::new(
        composer.x,
        composer.y.saturating_sub(height).saturating_sub(1),
        composer.width.max(1),
        height,
    )
}

fn render_slash(frame: &mut Frame, app: &App, composer: Rect) {
    let items = app.slash_items();
    let area = slash_menu_area(composer, items.len());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            format!(" /{} ", app.input_text().trim_start_matches('/')),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(RULE))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let counts = slash_kind_counts(&items);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " ALL ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  TOOLS  ", Style::default().fg(MUTED)),
            Span::styled("SKILLS  ", Style::default().fg(MUTED)),
            Span::styled("COMMANDS", Style::default().fg(MUTED)),
            Span::styled(
                format!("    {} matches", items.len()),
                Style::default().fg(MUTED),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        )
        .style(Style::default().bg(SURFACE)),
        sections[0],
    );

    let command_width = sections[1].width.saturating_sub(8).clamp(8, 34) as usize;
    let detail_width = sections[1].width.saturating_sub(8) as usize;
    let visible_count = (sections[1].height as usize / 2).clamp(1, 8);
    let rows = items.iter().take(visible_count).map(|item| {
        let (kind, color) = match item.kind {
            SlashKind::Tool => ("T", ACCENT),
            SlashKind::Skill => ("S", VIOLET),
            SlashKind::Command => ("C", AMBER),
        };
        ListItem::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" {kind}  "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    single_line(&item.command, command_width),
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    single_line(&item.detail, detail_width),
                    Style::default().fg(MUTED),
                ),
            ]),
        ])
    });
    let list = List::new(rows)
        .highlight_symbol("▌")
        .highlight_style(Style::default().bg(RAISED).fg(TEXT));
    let mut state = ListState::default()
        .with_selected(Some(app.slash_selected.min(items.len().saturating_sub(1))));
    frame.render_stateful_widget(list, sections[1], &mut state);

    frame.render_widget(
        Paragraph::new(format!(
            "TOOLS {} · SKILLS {} · COMMANDS {}                         MOST USED",
            counts.0, counts.1, counts.2
        ))
        .style(Style::default().fg(MUTED).bg(SURFACE)),
        sections[2],
    );
}

fn render_sessions(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(72, 64, area);
    frame.render_widget(Clear, popup);
    let rows = app.sessions.iter().map(|session| {
        let streaming = if session.streaming { " · running" } else { "" };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{:<32}", session.name), Style::default().fg(TEXT)),
            Span::styled(
                format!("{}{}", display_model(&session.model), streaming),
                Style::default().fg(MUTED),
            ),
        ]))
    });
    let list = List::new(rows)
        .block(
            Block::default()
                .title(Span::styled(
                    " Conversations ",
                    Style::default().fg(VIOLET).add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(VIOLET))
                .style(Style::default().bg(SURFACE)),
        )
        .highlight_symbol(" ❯ ")
        .highlight_style(
            Style::default()
                .bg(RAISED)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_selected(Some(app.session_selected));
    frame.render_stateful_widget(list, popup, &mut state);
}

fn render_path_picker(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(4).clamp(1, 96);
    let height = area.height.saturating_sub(2).clamp(1, 20);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(Span::styled(
            " Attach files ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);
    let (query, cursor) = visible_input(
        &app.path_input,
        app.path_input.len(),
        sections[0].width.saturating_sub(3) as usize,
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("❯ ", Style::default().fg(ACCENT)),
                Span::styled(
                    if query.is_empty() {
                        "Filter files or paste a path…".to_owned()
                    } else {
                        query
                    },
                    Style::default().fg(if app.path_input.is_empty() {
                        MUTED
                    } else {
                        TEXT
                    }),
                ),
            ]),
            Line::from(Span::styled(
                single_line(
                    &app.path_directory.to_string_lossy(),
                    sections[0].width as usize,
                ),
                Style::default().fg(MUTED),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        )
        .style(Style::default().bg(SURFACE)),
        sections[0],
    );
    let entries = app.visible_path_entries();
    let rows = if entries.is_empty() {
        vec![ListItem::new(Span::styled(
            " No matching files",
            Style::default().fg(MUTED),
        ))]
    } else {
        entries
            .iter()
            .map(|entry| {
                let marker = if entry.is_dir { "▸ DIR " } else { "  FILE" };
                let color = if entry.is_dir {
                    ACCENT
                } else if entry.supported {
                    TEXT
                } else {
                    FAINT
                };
                let size = if entry.is_dir {
                    String::new()
                } else {
                    format!("  {}", format_bytes(entry.size))
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {marker:<7}"), Style::default().fg(color)),
                    Span::styled(
                        single_line(&entry.name, sections[1].width.saturating_sub(25) as usize),
                        Style::default().fg(color),
                    ),
                    Span::styled(size, Style::default().fg(MUTED)),
                ]))
            })
            .collect()
    };
    let list = List::new(rows)
        .highlight_symbol("▌")
        .highlight_style(Style::default().bg(RAISED).fg(TEXT));
    let mut state = ListState::default().with_selected(
        (!entries.is_empty()).then_some(app.path_selected.min(entries.len().saturating_sub(1))),
    );
    frame.render_stateful_widget(list, sections[1], &mut state);
    let selected = if app.attachments.is_empty() {
        "Selected · none".to_owned()
    } else {
        format!(
            "Selected {} · {}",
            app.attachments.len(),
            app.attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        )
    };
    frame.render_widget(
        Paragraph::new(single_line(&selected, sections[2].width as usize))
            .style(Style::default().fg(ACCENT).bg(SURFACE))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(RULE)),
            ),
        sections[2],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "↑↓ choose · Enter open/add · ←/Backspace parent · Delete remove · Esc close",
                Style::default().fg(MUTED),
            )),
            Line::from(Span::styled(
                "8 files max · 10 MiB each · 20 MiB total · workspace only",
                Style::default().fg(MUTED),
            )),
        ])
        .style(Style::default().bg(SURFACE)),
        sections[3],
    );
    frame.set_cursor_position(Position::new(
        sections[0]
            .x
            .saturating_add(2)
            .saturating_add(cursor as u16),
        sections[0].y,
    ));
}

fn render_settings_picker(frame: &mut Frame, app: &App, area: Rect) {
    let picker = app.settings_picker.unwrap_or(SettingsPicker::Model);
    let width = area.width.saturating_sub(4).clamp(1, 88);
    let height = area.height.saturating_sub(4).clamp(1, 18);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let title = match picker {
        SettingsPicker::Model => " Switch model ",
        SettingsPicker::Thinking => " Thinking level ",
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(VIOLET).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(VIOLET))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let rows = match picker {
        SettingsPicker::Model if app.model_options.is_empty() => vec![ListItem::new(Span::styled(
            " No enabled chat models",
            Style::default().fg(MUTED),
        ))],
        SettingsPicker::Model => app
            .model_options
            .iter()
            .map(|model| {
                let id = format!("{}/{}", model.provider, model.id);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if id == app.model { " ● " } else { "   " },
                        Style::default().fg(GREEN),
                    ),
                    Span::styled(
                        if model.name.is_empty() {
                            model.id.clone()
                        } else {
                            model.name.clone()
                        },
                        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {id}"), Style::default().fg(MUTED)),
                    Span::styled(
                        if model.reasoning { "  reasoning" } else { "" },
                        Style::default().fg(VIOLET),
                    ),
                ]))
            })
            .collect(),
        SettingsPicker::Thinking => app
            .thinking_levels()
            .iter()
            .map(|level| {
                let detail = match level.as_str() {
                    "off" => "No reasoning tokens",
                    "minimal" => "Fastest reasoning",
                    "low" => "Light reasoning",
                    "medium" => "Balanced",
                    "high" => "Deeper reasoning",
                    _ => "Maximum supported reasoning",
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        if level == &app.thinking_level {
                            " ● "
                        } else {
                            "   "
                        },
                        Style::default().fg(GREEN),
                    ),
                    Span::styled(format!("{level:<9}"), Style::default().fg(TEXT)),
                    Span::styled(detail, Style::default().fg(MUTED)),
                ]))
            })
            .collect(),
    };
    let count = match picker {
        SettingsPicker::Model => app.model_options.len(),
        SettingsPicker::Thinking => app.thinking_levels().len(),
    };
    let list = List::new(rows)
        .block(
            Block::default()
                .title_bottom(Span::styled(
                    " ↑↓ choose · Enter apply · Esc cancel ",
                    Style::default().fg(MUTED),
                ))
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        )
        .highlight_symbol("▌")
        .highlight_style(Style::default().bg(RAISED).fg(TEXT));
    let mut state = ListState::default()
        .with_selected((count > 0).then_some(app.settings_selected.min(count.saturating_sub(1))));
    frame.render_stateful_widget(list, inner, &mut state);
}

fn approval_detail_text(approval: &Approval) -> Text<'_> {
    let command = approval
        .args
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            serde_json::to_string(&approval.args).unwrap_or_else(|_| "{}".to_owned())
        });
    let mut lines = command
        .lines()
        .enumerate()
        .map(|(index, line)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 { "$ " } else { "  " },
                    Style::default().fg(ACCENT),
                ),
                Span::styled(line.to_owned(), Style::default().fg(TEXT)),
            ])
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("$", Style::default().fg(ACCENT))));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        &approval.reason,
        Style::default().fg(MUTED),
    )));
    Text::from(lines)
}

fn approval_panel_height(app: &App, area: Rect) -> u16 {
    let Some(approval) = &app.approval else {
        return 0;
    };
    let inner_width = area.width.saturating_sub(4).max(1);
    let details = Paragraph::new(approval_detail_text(approval)).wrap(Wrap { trim: false });
    let desired = details
        .line_count(inner_width)
        .saturating_add(3)
        .max(7)
        .min(u16::MAX as usize) as u16;
    let chat_reserve = if area.height >= 16 { 3 } else { 0 };
    let maximum = area
        .height
        .saturating_sub(3)
        .saturating_sub(chat_reserve)
        .max(1);
    desired.min(maximum)
}

fn render_approval(frame: &mut Frame, app: &App, area: Rect) {
    let Some(approval) = &app.approval else {
        return;
    };
    let risk = if approval.risk.is_empty() {
        String::new()
    } else {
        format!(" · {} risk", approval.risk)
    };
    let block = Block::default()
        .title(format!(
            " Approval required · {}{risk} ",
            approval.tool_name
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(AMBER))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(approval_detail_text(approval))
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(SURFACE)),
        rows[0],
    );
    let actions = if rows[1].width >= 52 {
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(MUTED)),
            Span::styled(
                "[Y]",
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Allow once     ", Style::default().fg(TEXT)),
            Span::styled("[N]", Style::default().fg(RED).add_modifier(Modifier::BOLD)),
            Span::styled(" Deny     ", Style::default().fg(TEXT)),
            Span::styled(
                "[Esc]",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Deny", Style::default().fg(TEXT)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "[Y]",
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Allow  ", Style::default().fg(TEXT)),
            Span::styled(
                "[N/Esc]",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Deny", Style::default().fg(TEXT)),
        ])
    };
    frame.render_widget(
        Paragraph::new(actions).style(Style::default().bg(SURFACE)),
        rows[1],
    );
}

fn visible_input(input: &[char], cursor: usize, available: usize) -> (String, usize) {
    if input.is_empty() || available == 0 {
        return (String::new(), 0);
    }
    let cursor = cursor.min(input.len());
    let mut start = 0;
    let width_to_cursor: usize = input[..cursor]
        .iter()
        .map(|character| character.width().unwrap_or(0))
        .sum();
    while input[start..cursor]
        .iter()
        .map(|character| character.width().unwrap_or(0))
        .sum::<usize>()
        >= available
        && start < cursor
    {
        start += 1;
    }
    let mut end = start;
    let mut width = 0;
    while end < input.len() {
        let next = input[end].width().unwrap_or(0);
        if width + next > available {
            break;
        }
        width += next;
        end += 1;
    }
    let visible: String = input[start..end].iter().collect();
    let hidden_width: usize = input[..start]
        .iter()
        .map(|character| character.width().unwrap_or(0))
        .sum();
    (visible, width_to_cursor.saturating_sub(hidden_width))
}

fn centered_width(area: Rect, maximum: u16) -> Rect {
    let width = area.width.min(maximum);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y,
        width,
        area.height,
    )
}

fn visible_agent_count(app: &App) -> usize {
    let historic = app
        .messages
        .iter()
        .filter_map(|message| message.run_activity.as_ref())
        .map(|activity| activity.agents.len())
        .sum::<usize>();
    let live = app.live.as_ref().map(|_| 0).unwrap_or_default();
    historic.saturating_add(live)
}

fn slash_kind_counts(items: &[crate::app::SlashItem]) -> (usize, usize, usize) {
    items.iter().fold((0, 0, 0), |mut counts, item| {
        match item.kind {
            SlashKind::Tool => counts.0 += 1,
            SlashKind::Skill => counts.1 += 1,
            SlashKind::Command => counts.2 += 1,
        }
        counts
    })
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn inset(area: Rect, horizontal: u16, vertical: u16) -> Rect {
    Rect::new(
        area.x.saturating_add(horizontal),
        area.y.saturating_add(vertical),
        area.width.saturating_sub(horizontal.saturating_mul(2)),
        area.height.saturating_sub(vertical.saturating_mul(2)),
    )
}

fn single_line(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.width() <= max {
        return compact;
    }
    let mut result = String::new();
    for character in compact.chars() {
        if result.width() + character.width().unwrap_or(0) + 1 > max {
            break;
        }
        result.push(character);
    }
    result.push('…');
    result
}

fn shorten_path(value: &str) -> String {
    if value.width() <= 70 {
        value.to_owned()
    } else {
        format!(
            "…{}",
            value
                .chars()
                .rev()
                .take(68)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        )
    }
}

fn display_model(value: &str) -> &str {
    value
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("model")
}

fn display_execution_mode(value: &str) -> &str {
    if value.is_empty() {
        "workspace"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};
    use unicode_width::UnicodeWidthStr;

    use super::{
        draw, push_live, push_markdown, runtime_error_label, slash_menu_area, visible_input,
        CONVERSATION_WIDTH, GREEN, PULSE_FRAMES,
    };
    use crate::{
        app::{App, Approval, LiveTurn, PathEntry, SettingsPicker},
        model::{ChatMessage, ModelOption, SessionSummary, ToolActivity},
    };

    #[test]
    fn exhausted_provider_errors_are_concise_and_bounded() {
        let label = runtime_error_label(
            "overloaded_error: Message: Pisper is overloaded. Please try again later.",
            48,
        );
        assert_eq!(label, "Error · Provider overloaded · automatic retries…");
        assert!(label.width() <= 48);
    }

    #[test]
    fn input_window_keeps_the_cursor_visible() {
        let input: Vec<char> = "1234567890".chars().collect();
        let (visible, cursor) = visible_input(&input, input.len(), 5);
        assert_eq!(visible, "7890");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn markdown_renderer_styles_headings_lists_and_diffs() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "●",
            super::ACCENT,
            "## Root cause\n\n- Keep **cleanup** scoped.\n\n```diff\n-old\n+new\n```",
            80,
        );
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("●"));
        assert!(rendered.contains("## Root cause"));
        assert!(rendered.contains("• Keep cleanup scoped."));
        assert!(rendered.contains("┌─ diff"));
        assert!(rendered.contains("+new"));
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| { span.content.contains("+new") && span.style.fg == Some(GREEN) }));
    }

    #[test]
    fn completion_state_does_not_change_the_transcript_buffer() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Explain the render path".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        app.status = "streaming".to_owned();
        app.live = Some(LiveTurn {
            text: "The transcript is already final.".to_owned(),
            text_target: "The transcript is already final.".to_owned(),
            streaming: true,
            ..LiveTurn::default()
        });

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let before = terminal.backend().buffer().content[..80 * 15].to_vec();

        app.live.as_mut().unwrap().streaming = false;
        app.status = "complete".to_owned();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let after = terminal.backend().buffer().content[..80 * 15].to_vec();

        assert_eq!(before, after);
    }

    #[test]
    fn live_thinking_without_reasoning_tokens_does_not_panic() {
        let mut lines = Vec::new();
        push_live(
            &mut lines,
            &LiveTurn {
                streaming: true,
                ..LiveTurn::default()
            },
            true,
            80,
            20,
            0,
        );

        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .spans
            .iter()
            .any(|span| span.content.contains("THINK")));
    }

    #[test]
    fn live_thinking_is_expanded_before_the_response_and_uses_terminal_blink() {
        let mut lines = Vec::new();
        push_live(
            &mut lines,
            &LiveTurn {
                thinking: "Inspect the runtime.\nTrace the event stream.\nRender the reasoning."
                    .to_owned(),
                text: "The implementation is ready.".to_owned(),
                streaming: true,
                ..LiveTurn::default()
            },
            true,
            80,
            20,
            0,
        );

        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rendered[0].contains("THINK"));
        assert!(rendered[0].contains("Inspect the runtime."));
        assert!(rendered[1].contains("Trace the event stream."));
        assert!(rendered
            .last()
            .unwrap()
            .contains("The implementation is ready."));
        assert!(lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::SLOW_BLINK));
    }

    #[test]
    fn live_tools_use_a_bounded_single_line_group_below_thinking() {
        let tools = (0..8)
            .map(|index| ToolActivity {
                name: format!("tool-{index}"),
                status: if index == 7 { "running" } else { "done" }.to_owned(),
                message: "A deliberately long tool detail that must stay on one terminal row."
                    .to_owned(),
                ..ToolActivity::default()
            })
            .collect();
        let mut lines = Vec::new();
        push_live(
            &mut lines,
            &LiveTurn {
                thinking: "Keep the reasoning visible.".to_owned(),
                tools,
                streaming: true,
                ..LiveTurn::default()
            },
            true,
            48,
            12,
            0,
        );

        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rendered[0].contains("THINK"));
        assert!(rendered[1].contains("7 earlier tools"));
        assert!(rendered[2].contains("tool-7"));
        assert!(lines.iter().all(|line| line.width() <= 48));
    }

    #[test]
    fn short_conversations_start_at_the_top_with_runtime_meta_and_status() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Top aligned".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            thinking_level: "high".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "agent".to_owned(),
                text: "Pisper is ready.".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            Some(crate::model::ContextUsage { percent: Some(4.0) }),
            Vec::new(),
            Vec::new(),
        );
        app.status = "thinking".to_owned();
        app.live = Some(LiveTurn {
            streaming: true,
            ..LiveTurn::default()
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows = (0..24)
            .map(|y| {
                (0..80)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(!rows.join("\n").contains("PISPER /"));
        assert!(rows
            .iter()
            .any(|row| row.contains("gpt-5.6-sol · [workspace]")));
        let message_row = rows
            .iter()
            .position(|row| row.contains("Pisper is ready."))
            .unwrap();
        let thinking_row = rows
            .iter()
            .rposition(|row| row.contains("Thinking"))
            .unwrap();
        assert!(rows[message_row].starts_with("●  Pisper is ready."));
        assert!(
            rows[thinking_row].starts_with("⠋ Thinking."),
            "run state is not left aligned: {}",
            rows[thinking_row]
        );
        assert!(
            message_row <= 2,
            "message did not start at the top: {rows:?}"
        );
        assert!(
            message_row < thinking_row,
            "message crossed into the run state"
        );
        assert!(thinking_row < 17, "run state crossed into the composer");
    }

    #[test]
    fn narrow_status_keeps_the_execution_mode_visible_without_a_header() {
        for (width, expected) in [(60, "[full-access]"), (36, "[full-access]")] {
            let session = SessionSummary {
                id: "session-1".to_owned(),
                name: "Mode visibility".to_owned(),
                model: "openai/gpt-5.6-sol".to_owned(),
                cwd: "/workspace".to_owned(),
                execution_mode: "full-access".to_owned(),
                thinking_level: "high".to_owned(),
                ..SessionSummary::default()
            };
            let app = App::new(
                vec![session.clone()],
                session,
                Vec::new(),
                Some(crate::model::ContextUsage { percent: Some(4.0) }),
                Vec::new(),
                Vec::new(),
            );
            let backend = TestBackend::new(width, 12);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let rows = (0..12)
                .map(|y| {
                    (0..width)
                        .filter_map(|x| terminal.backend().buffer().cell((x, y)))
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            assert!(
                rows.iter().any(|row| row.contains(expected)),
                "mode missing from status at width {width}: {rows:?}"
            );
        }
    }

    #[test]
    fn approval_panel_keeps_the_command_and_keys_visible_at_terminal_sizes() {
        for (width, height) in [(160, 40), (80, 24), (36, 12)] {
            let session = SessionSummary {
                id: "session-1".to_owned(),
                name: "Approval".to_owned(),
                model: "openai/gpt-5.6-sol".to_owned(),
                cwd: "/workspace".to_owned(),
                execution_mode: "workspace".to_owned(),
                ..SessionSummary::default()
            };
            let mut app = App::new(
                vec![session.clone()],
                session,
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
            );
            app.approval = Some(Approval {
                id: "approval-1".to_owned(),
                tool_name: "bash".to_owned(),
                args: serde_json::json!({ "command": "date +%A" }),
                risk: "high".to_owned(),
                reason: "Runs as the current OS user and may access files or network services."
                    .to_owned(),
            });

            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let rows = (0..height)
                .map(|y| {
                    (0..width)
                        .filter_map(|x| terminal.backend().buffer().cell((x, y)))
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            let rendered = rows.join("\n");

            assert!(rendered.contains("Approval required"));
            assert!(rendered.contains("date +%A"));
            assert!(rendered.contains("[Y]"));
            assert!(rendered.contains("Allow"));
            assert!(rendered.contains("[N"));
            assert!(rendered.contains("Deny"));
            if width > CONVERSATION_WIDTH {
                let title = rows
                    .iter()
                    .find(|row| row.contains("Approval required"))
                    .unwrap();
                assert!(title.starts_with('┌'));
                assert!(title.ends_with('┐'));
            }
        }
    }

    #[test]
    fn slash_menu_shares_the_composer_content_rail() {
        let area = slash_menu_area(ratatui::layout::Rect::new(25, 30, 110, 7), 12);
        assert_eq!(area.x, 25);
        assert_eq!(area.width, 110);
        assert_eq!(area.y + area.height, 29);
    }

    #[test]
    fn tab_completion_hint_only_appears_with_the_slash_menu() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let idle = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!idle.contains("Tab complete"));
        assert!(idle.contains("Enter submit"));

        app.input = vec!['/'];
        app.input_cursor = 1;
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let slash = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(slash.contains("Tab complete · Enter select"));
    }

    #[test]
    fn conversation_and_slash_menu_render_at_terminal_sizes() {
        for (width, height) in [(160, 40), (60, 20), (36, 12)] {
            let session = SessionSummary {
                id: "session-1".to_owned(),
                name: "Terminal smoke".to_owned(),
                model: "openai/gpt-5.6-sol".to_owned(),
                cwd: "/workspace".to_owned(),
                execution_mode: "workspace".to_owned(),
                ..SessionSummary::default()
            };
            let mut app = App::new(
                vec![session.clone()],
                session,
                vec![ChatMessage {
                    role: "agent".to_owned(),
                    text: "Pisper is ready.".to_owned(),
                    run_activity: None,
                    attachments: Vec::new(),
                }],
                None,
                Vec::new(),
                Vec::new(),
            );
            app.input = vec!['/'];
            app.input_cursor = 1;
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            app.path_picker = true;
            terminal.draw(|frame| draw(frame, &app)).unwrap();
        }
    }

    #[test]
    fn file_model_and_thinking_pickers_render_real_choices() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Pickers".to_owned(),
            model: "provider/model-a".to_owned(),
            cwd: "/workspace".to_owned(),
            thinking_level: "medium".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        );
        app.path_picker = true;
        app.path_entries = vec![
            PathEntry {
                path: "/workspace/docs".into(),
                name: "docs".to_owned(),
                is_dir: true,
                size: 0,
                supported: true,
            },
            PathEntry {
                path: "/workspace/README.md".into(),
                name: "README.md".to_owned(),
                is_dir: false,
                size: 128,
                supported: true,
            },
        ];
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = format!("{:?}", terminal.backend().buffer());
        assert!(rendered.contains("README.md"));
        assert!(rendered.contains("docs"));

        app.path_picker = false;
        app.set_model_options(vec![ModelOption {
            provider: "provider".to_owned(),
            id: "model-a".to_owned(),
            name: "Model A".to_owned(),
            reasoning: true,
        }]);
        app.settings_picker = Some(SettingsPicker::Model);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("Model A"));

        app.settings_picker = Some(SettingsPicker::Thinking);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("medium"));
    }

    #[test]
    fn empty_session_centers_the_brand_and_composer() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let app = App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        );
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows = (0..40)
            .map(|y| {
                (0..160)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let input_row = rows
            .iter()
            .position(|row| row.contains("Message Pisper"))
            .unwrap();

        assert!(rows.iter().any(|row| row.contains("████")));
        assert!((18..=24).contains(&input_row));
        assert_eq!(buffer.cell((36, input_row as u16)).unwrap().symbol(), "│");
        assert_eq!(buffer.cell((123, input_row as u16)).unwrap().symbol(), "│");
    }

    #[test]
    fn active_conversation_moves_the_composer_to_the_full_width_bottom() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Start the task".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let input_row = (0..40)
            .find(|y| {
                (0..160)
                    .filter_map(|x| buffer.cell((x, *y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .contains("Message Pisper")
            })
            .unwrap();

        assert!(input_row >= 32);
        assert_eq!(buffer.cell((0, input_row)).unwrap().symbol(), "│");
        assert_eq!(buffer.cell((159, input_row)).unwrap().symbol(), "│");
    }

    #[test]
    fn streaming_activity_uses_a_bottom_left_pulse() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Run".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        app.live = Some(LiveTurn {
            streaming: true,
            ..LiveTurn::default()
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let pulse = PULSE_FRAMES[0];
        let rows = (0..24)
            .map(|y| {
                (0..80)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(!rows[..23].iter().any(|row| row.contains(pulse)));
        assert!(rows[23].starts_with(pulse));
    }

    #[test]
    fn conversation_messages_use_the_full_width_left_edge_without_a_startup_logo() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Header must stay hidden".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "agent".to_owned(),
                text: "Existing history".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        let backend = TestBackend::new(160, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rows = (0..40)
            .map(|y| {
                (0..160)
                    .filter_map(|x| terminal.backend().buffer().cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let message = rows
            .iter()
            .find(|row| row.contains("Existing history"))
            .unwrap();
        assert!(message.starts_with("●  Existing history"));
        let content_byte = message.find("Existing history").unwrap();
        assert_eq!(message[..content_byte].width(), 3);
        assert!(!rows.join("\n").contains("Header must stay hidden"));
        assert!(!rows.join("\n").contains("╭─────────╮"));
    }

    #[test]
    fn narrow_terminal_keeps_the_welcome_logo_and_composer() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let app = App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        );
        let backend = TestBackend::new(36, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("PISPER"));
        assert!(rendered.contains("Message Pisper"));
    }

    #[test]
    fn long_conversations_use_the_full_viewport_without_the_logo() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        let messages = (0..20)
            .map(|index| ChatMessage {
                role: "agent".to_owned(),
                text: format!("History line {index}"),
                run_activity: None,
                attachments: Vec::new(),
            })
            .collect();
        let app = App::new(
            vec![session.clone()],
            session,
            messages,
            None,
            Vec::new(),
            Vec::new(),
        );
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(!rendered.contains("╭─────────╮"));
        assert!(!rendered.contains("History line 0"));
        assert!(rendered.contains("History line 19"));
    }
}
