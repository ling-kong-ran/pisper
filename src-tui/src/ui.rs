use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
    },
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, LiveTurn, SlashKind, View},
    model::{ChatMessage, RunActivity, ToolActivity},
};

const BG: Color = Color::Rgb(15, 16, 16);
const SURFACE: Color = Color::Rgb(21, 23, 23);
const RAISED: Color = Color::Rgb(27, 30, 29);
const RULE: Color = Color::Rgb(41, 45, 43);
const TEXT: Color = Color::Rgb(231, 229, 223);
const MUTED: Color = Color::Rgb(150, 155, 150);
const FAINT: Color = Color::Rgb(120, 128, 122);
const ACCENT: Color = Color::Rgb(121, 201, 181);
const GREEN: Color = Color::Rgb(145, 201, 120);
const AMBER: Color = Color::Rgb(216, 182, 106);
const RED: Color = Color::Rgb(220, 124, 124);
const VIOLET: Color = Color::Rgb(190, 167, 216);
const BLUE: Color = Color::Rgb(158, 180, 207);

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, chunks[0]);
    match app.view {
        View::Chat => render_chat(frame, app, chunks[1]),
        View::Events => render_events(frame, app, chunks[1]),
    }
    render_composer(frame, app, chunks[2]);
    render_status(frame, app, chunks[3]);

    if app.slash_open() {
        render_slash(frame, app, chunks[2]);
    }
    if app.session_picker {
        render_sessions(frame, app, area);
    }
    if app.approval.is_some() {
        render_approval(frame, app, area);
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);
    let title = Line::from(vec![
        Span::styled(
            "pisper",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  /  ", Style::default().fg(FAINT)),
        Span::styled(session_name(app), Style::default().fg(TEXT)),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        ),
        columns[0],
    );
    let context = app
        .context_percent
        .map(|value| format!(" · {value:.0}%"))
        .unwrap_or_default();
    let meta = format!(
        "{} · {}{}",
        display_model(&app.model),
        display_mode(&app.execution_mode),
        context
    );
    frame.render_widget(
        Paragraph::new(meta)
            .alignment(Alignment::Right)
            .style(Style::default().fg(MUTED))
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(RULE)),
            ),
        columns[1],
    );
}

fn render_chat(frame: &mut Frame, app: &App, area: Rect) {
    if app.show_startup_brand || (app.messages.is_empty() && app.live.is_none()) {
        render_welcome(frame, area);
        return;
    }
    let mut lines = Vec::new();
    let content_width = area.width.saturating_sub(4) as usize;
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
        );
    }
    let max_scroll = lines
        .len()
        .saturating_sub(area.height.saturating_sub(2) as usize) as u16;
    let scroll = max_scroll.saturating_sub(app.scroll.min(max_scroll));
    let paragraph = Paragraph::new(Text::from(lines))
        .style(Style::default().fg(TEXT).bg(BG))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inset(area, 2, 1));
}

fn render_welcome(frame: &mut Frame, area: Rect) {
    let compact = area.width < 40 || area.height < 6;
    let (height, text) = if compact {
        (
            2,
            Text::from(vec![
                Line::from(vec![
                    Span::styled(
                        "PISPER",
                        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ›_", Style::default().fg(ACCENT)),
                ]),
                Line::from(Span::styled(
                    "Pi-powered Agent workspace",
                    Style::default().fg(MUTED),
                )),
            ]),
        )
    } else {
        let wordmark = Style::default().fg(TEXT).add_modifier(Modifier::BOLD);
        (
            4,
            Text::from(vec![
                Line::from(Span::styled("╭─╮  ╷  ╭──  ╭─╮  ╭──  ╭─╮", wordmark)),
                Line::from(Span::styled("├─╯  │  ╰─╮  ├─╯  ├─   ├┬╯", wordmark)),
                Line::from(Span::styled("╵    ╵  ──╯  ╵    ╰──  ╵ ╰─", wordmark)),
                Line::from(vec![
                    Span::styled("›_", Style::default().fg(ACCENT)),
                    Span::styled("  Pi-powered Agent workspace", Style::default().fg(MUTED)),
                ]),
            ]),
        )
    };
    let height = height.min(area.height);
    let welcome = Rect::new(
        area.x,
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        area.width,
        height,
    );
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(Style::default().bg(BG)),
        welcome,
    );
}

fn push_message(lines: &mut Vec<Line<'static>>, message: &ChatMessage, width: usize) {
    if message.role == "user" {
        push_prefixed_text(lines, "› ", BLUE, &message.text, true);
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
        push_prefixed_text(lines, "● ", ACCENT, &message.text, false);
    }
    lines.push(Line::default());
}

fn push_live(
    lines: &mut Vec<Line<'static>>,
    live: &LiveTurn,
    thinking: bool,
    width: usize,
    viewport_rows: usize,
) {
    let (thinking_rows, tool_rows) = live_activity_budget(viewport_rows);
    if !live.thinking.is_empty() || (thinking && live.text.is_empty()) {
        push_thinking(lines, &live.thinking, thinking, width, thinking_rows);
    }
    push_tool_group(lines, &live.tools, width, tool_rows);
    if !live.text.is_empty() {
        push_prefixed_text(lines, "● ", ACCENT, &live.text, false);
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
    push_tool_group(lines, &activity.tools, width, 3);
    if !activity.agents.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  ▸ "),
            Span::styled(
                format!(
                    "{} subagent{}",
                    activity.agents.len(),
                    if activity.agents.len() == 1 { "" } else { "s" }
                ),
                Style::default().fg(VIOLET),
            ),
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
    let content_width = width.saturating_sub(13).max(12);
    let content = wrapped_tail(value, content_width, max_lines);
    let indicator_style = if active {
        Style::default()
            .fg(AMBER)
            .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK)
    } else {
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
    };
    let indicator = if active {
        const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
        FRAMES[value.chars().count() % FRAMES.len()]
    } else {
        "·"
    };
    let first = content.first().cloned().unwrap_or_default();
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(indicator, indicator_style),
        Span::styled(" THINK  ", Style::default().fg(AMBER)),
        Span::styled(first, Style::default().fg(MUTED)),
    ]));
    for line in content.into_iter().skip(1) {
        lines.push(Line::from(vec![
            Span::styled("          └ ", Style::default().fg(FAINT)),
            Span::styled(line, Style::default().fg(MUTED)),
        ]));
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
) {
    if tools.is_empty() || max_rows == 0 {
        return;
    }
    if max_rows == 1 {
        lines.push(tool_line(tools.last().expect("tools is not empty"), width));
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
            Span::styled("  · ", Style::default().fg(FAINT)),
            Span::styled(
                format!(
                    "{hidden} earlier tool{}",
                    if hidden == 1 { "" } else { "s" }
                ),
                Style::default().fg(MUTED),
            ),
        ]));
    }
    for tool in tools.iter().skip(tools.len().saturating_sub(tool_rows)) {
        lines.push(tool_line(tool, width));
    }
}

fn tool_line(tool: &ToolActivity, width: usize) -> Line<'static> {
    const PREFIX_WIDTH: usize = 4;
    const NAME_WIDTH: usize = 9;
    const RUNNING_WIDTH: usize = 9;

    let status_color = match tool.status.as_str() {
        "error" => RED,
        "done" => GREEN,
        _ => AMBER,
    };
    let running = tool.status == "running";
    let running_label = if running { "  running" } else { "" };
    let detail_width =
        width.saturating_sub(PREFIX_WIDTH + NAME_WIDTH + usize::from(running) * RUNNING_WIDTH);
    let detail = tool_detail(tool, detail_width);
    let name = single_line(&tool.name.to_uppercase(), NAME_WIDTH.saturating_sub(1));
    let name_padding = NAME_WIDTH.saturating_sub(name.width());
    Line::from(vec![
        Span::styled("  ▸ ", Style::default().fg(FAINT)),
        Span::styled(
            format!("{name}{}", " ".repeat(name_padding)),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(detail, Style::default().fg(MUTED)),
        Span::styled(running_label, Style::default().fg(status_color)),
    ])
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

fn push_prefixed_text(
    lines: &mut Vec<Line<'static>>,
    prefix: &'static str,
    color: Color,
    value: &str,
    bold: bool,
) {
    let mut source = value.lines();
    let first = source.next().unwrap_or_default().to_owned();
    let style = if bold {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    lines.push(Line::from(vec![
        Span::styled(
            prefix,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(first, style),
    ]));
    for line in source {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line.to_owned(), style),
        ]));
    }
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
    let inner = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(RULE))
        .inner(area);
    frame.render_widget(
        Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(RULE))
            .style(Style::default().bg(BG)),
        area,
    );
    let available = inner.width.saturating_sub(4) as usize;
    let (visible, cursor_width) = visible_input(&app.input, app.input_cursor, available);
    let line = Line::from(vec![
        Span::styled(
            "› ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if visible.is_empty() {
                "Message Pisper…".to_owned()
            } else {
                visible
            },
            Style::default().fg(if app.input.is_empty() { FAINT } else { TEXT }),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), inner);
    if !app.session_picker
        && app.approval.is_none()
        && !app.is_streaming()
        && !app.has_pending_render()
    {
        frame.set_cursor_position(Position::new(
            inner
                .x
                .saturating_add(2)
                .saturating_add(cursor_width as u16),
            inner.y,
        ));
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(55),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(shorten_path(&app.cwd)).style(Style::default().fg(MUTED)),
        columns[0],
    );
    let run_color = if app.status.contains("error") || app.status.contains("failed") {
        RED
    } else if app.is_streaming() || app.approval.is_some() {
        AMBER
    } else {
        GREEN
    };
    frame.render_widget(
        Paragraph::new(app.status.clone())
            .alignment(Alignment::Center)
            .style(Style::default().fg(run_color)),
        columns[1],
    );
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
            format!("{done}/{} tasks", list.items.len())
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(tasks)
            .alignment(Alignment::Right)
            .style(Style::default().fg(MUTED)),
        columns[2],
    );
}

fn render_slash(frame: &mut Frame, app: &App, composer: Rect) {
    let items = app.slash_items();
    let height = (items.len().min(8) as u16).saturating_add(2).max(3);
    let width = composer.width.saturating_sub(2).clamp(1, 110);
    let x = composer.x + composer.width.saturating_sub(width) / 2;
    let y = composer.y.saturating_sub(height);
    let area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, area);
    let rows = items.iter().take(8).map(|item| {
        let (kind, color) = match item.kind {
            SlashKind::Tool => ("T", ACCENT),
            SlashKind::Skill => ("S", VIOLET),
            SlashKind::Command => ("C", AMBER),
        };
        ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {kind}  "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{:<24}", item.command), Style::default().fg(color)),
            Span::styled(
                single_line(&item.detail, width.saturating_sub(34) as usize),
                Style::default().fg(MUTED),
            ),
        ]))
    });
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RULE))
                .style(Style::default().bg(SURFACE)),
        )
        .highlight_style(
            Style::default()
                .bg(RAISED)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default()
        .with_selected(Some(app.slash_selected.min(items.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
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
                .title(" Conversations ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(RULE))
                .style(Style::default().bg(SURFACE)),
        )
        .highlight_style(
            Style::default()
                .bg(RAISED)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_selected(Some(app.session_selected));
    frame.render_stateful_widget(list, popup, &mut state);
}

fn render_approval(frame: &mut Frame, app: &App, area: Rect) {
    let Some(approval) = &app.approval else {
        return;
    };
    let popup = centered_rect(64, 28, area);
    frame.render_widget(Clear, popup);
    let body = Text::from(vec![
        Line::from(vec![
            Span::styled("Tool  ", Style::default().fg(MUTED)),
            Span::styled(
                &approval.tool_name,
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        Line::from(Span::styled(&approval.reason, Style::default().fg(TEXT))),
        Line::default(),
        Line::from(vec![
            Span::styled(
                "Y  Approve",
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "     N  Deny",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: true }).block(
            Block::default()
                .title(" Approval required ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(AMBER))
                .style(Style::default().bg(SURFACE)),
        ),
        popup,
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

fn session_name(app: &App) -> &str {
    if app.session.name.is_empty() {
        "New conversation"
    } else {
        &app.session.name
    }
}

fn display_model(value: &str) -> &str {
    value
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("model")
}

fn display_mode(value: &str) -> &str {
    if value.is_empty() {
        "workspace"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};

    use super::{draw, push_live, visible_input};
    use crate::{
        app::{App, LiveTurn},
        model::{ChatMessage, SessionSummary, ToolActivity},
    };

    #[test]
    fn input_window_keeps_the_cursor_visible() {
        let input: Vec<char> = "1234567890".chars().collect();
        let (visible, cursor) = visible_input(&input, input.len(), 5);
        assert_eq!(visible, "7890");
        assert_eq!(cursor, 4);
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
        assert!(lines[0].spans[1]
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
        assert!(rendered[2].contains("TOOL-7"));
        assert!(lines.iter().all(|line| line.width() <= 48));
    }

    #[test]
    fn conversation_and_slash_menu_render_at_terminal_sizes() {
        for (width, height) in [(120, 40), (60, 20)] {
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
        }
    }

    #[test]
    fn empty_conversation_renders_the_startup_brand() {
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
                role: "agent".to_owned(),
                text: "Existing history".to_owned(),
                run_activity: None,
            }],
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
        assert!(rendered.contains("╭─╮"));
        assert!(rendered.contains("├┬╯"));
        assert!(rendered.contains("Pi-powered Agent workspace"));
    }

    #[test]
    fn startup_brand_has_a_compact_narrow_terminal_variant() {
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
        assert!(rendered.contains("›_"));
    }
}
