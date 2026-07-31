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
    app::{App, Approval, LiveTurn, SlashKind, View},
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

    if app.approval.is_some() {
        let chat_minimum = if area.height >= 16 { 3 } else { 0 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(chat_minimum),
                Constraint::Length(approval_panel_height(app, area)),
                Constraint::Length(1),
            ])
            .split(area);
        render_header(frame, app, chunks[0]);
        match app.view {
            View::Chat => render_chat(frame, app, chunks[1]),
            View::Events => render_events(frame, app, chunks[1]),
        }
        render_approval(frame, app, chunks[2]);
        render_status(frame, app, chunks[3]);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, chunks[0]);
    match app.view {
        View::Chat => render_chat(frame, app, chunks[1]),
        View::Events => render_events(frame, app, chunks[1]),
    }
    render_run_state(frame, app, chunks[2]);
    render_composer(frame, app, chunks[3]);
    render_status(frame, app, chunks[4]);

    if app.slash_open() {
        render_slash(frame, app, chunks[3]);
    }
    if app.session_picker {
        render_sessions(frame, app, area);
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
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
    frame.render_widget(
        Paragraph::new(runtime_meta(app, columns[1].width as usize))
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
    let viewport = inset(area, 2, 1);
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
    let render_area = if rendered_rows < viewport.height as usize {
        let content_rows = rendered_rows.max(1) as u16;
        Rect::new(
            viewport.x,
            viewport
                .y
                .saturating_add(viewport.height.saturating_sub(content_rows)),
            viewport.width,
            content_rows,
        )
    } else {
        viewport
    };
    let brand_height = brand_height(viewport);
    if render_area.y >= viewport.y.saturating_add(brand_height) {
        render_brand(
            frame,
            Rect::new(viewport.x, viewport.y, viewport.width, brand_height),
        );
    }
    frame.render_widget(paragraph.scroll((scroll, 0)), render_area);
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
        (app.status.clone(), RED, false)
    } else {
        return;
    };

    let line = if animate {
        const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
        let frame = FRAMES[(app.status_frame as usize / 5) % FRAMES.len()];
        let dots = ".".repeat((app.status_frame as usize / 10) % 3 + 1);
        Line::from(vec![
            Span::styled(
                format!("  {frame} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{label}{dots}"), Style::default().fg(color)),
        ])
    } else {
        Line::from(Span::styled(
            format!("  {label}"),
            Style::default().fg(color),
        ))
    };
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);
}

fn brand_height(area: Rect) -> u16 {
    if area.width < 40 || area.height < 7 {
        2.min(area.height)
    } else {
        5.min(area.height)
    }
}

fn render_brand(frame: &mut Frame, area: Rect) {
    let compact = area.width < 40 || area.height < 5;
    let text = if compact {
        Text::from(vec![Line::from(vec![
            Span::styled(
                "PISPER",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ›_", Style::default().fg(ACCENT)),
        ])])
    } else {
        let frame_style = Style::default().fg(FAINT);
        let mark_style = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        Text::from(vec![
            Line::from(Span::styled("╭─────────╮", frame_style)),
            Line::from(vec![
                Span::styled("│  ", frame_style),
                Span::styled("╭──╮", mark_style),
                Span::styled("   │    ", frame_style),
                Span::styled(
                    "PISPER",
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("│  ", frame_style),
                Span::styled("├──╯", mark_style),
                Span::styled("   │   ", frame_style),
            ]),
            Line::from(vec![
                Span::styled("│  ", frame_style),
                Span::styled("╵  ›_", mark_style),
                Span::styled("  │", frame_style),
            ]),
            Line::from(Span::styled("╰─────────╯", frame_style)),
        ])
    };
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Left)
            .style(Style::default().bg(BG)),
        area,
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
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);
    frame.render_widget(
        Paragraph::new(shorten_path(&app.cwd)).style(Style::default().fg(MUTED)),
        columns[0],
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
        columns[1],
    );
}

fn slash_menu_area(composer: Rect, item_count: usize) -> Rect {
    let inset: u16 = if composer.width > 4 { 2 } else { 0 };
    let width = composer
        .width
        .saturating_sub(inset.saturating_mul(2))
        .max(1);
    let desired_height = (item_count.min(8) as u16).saturating_add(2).max(3);
    let available_height = composer.y.saturating_sub(2).max(1);
    let height = desired_height.min(available_height);
    Rect::new(
        composer.x.saturating_add(inset),
        composer.y.saturating_sub(height),
        width,
        height,
    )
}

fn render_slash(frame: &mut Frame, app: &App, composer: Rect) {
    let items = app.slash_items();
    let area = slash_menu_area(composer, items.len());
    frame.render_widget(Clear, area);
    let command_width = area.width.saturating_sub(12).clamp(8, 24) as usize;
    let detail_width = area
        .width
        .saturating_sub(command_width as u16)
        .saturating_sub(8) as usize;
    let rows = items.iter().take(8).map(|item| {
        let (kind, color) = match item.kind {
            SlashKind::Tool => ("T", ACCENT),
            SlashKind::Skill => ("S", VIOLET),
            SlashKind::Command => ("C", AMBER),
        };
        let command = single_line(&item.command, command_width);
        ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {kind}  "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{command:<command_width$}"),
                Style::default().fg(color),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                single_line(&item.detail, detail_width),
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

fn display_thinking_level(value: &str) -> &str {
    if value.is_empty() {
        "medium"
    } else {
        value
    }
}

fn display_execution_mode(value: &str) -> &str {
    if value.is_empty() {
        "workspace"
    } else {
        value
    }
}

fn runtime_meta(app: &App, width: usize) -> String {
    let mode = format!("[{}]", display_execution_mode(&app.execution_mode));
    let context = app
        .context_percent
        .map(|value| format!(" · {value:.0}%"))
        .unwrap_or_default();
    let full = format!(
        "{mode} · {} · {}{context}",
        display_model(&app.model),
        display_thinking_level(&app.thinking_level),
    );
    if full.width() <= width {
        return full;
    }
    let compact = format!(
        "{mode} · {}{context}",
        display_thinking_level(&app.thinking_level)
    );
    if compact.width() <= width {
        return compact;
    }
    mode
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};

    use super::{draw, push_live, slash_menu_area, visible_input};
    use crate::{
        app::{App, Approval, LiveTurn},
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
    fn short_conversations_anchor_above_the_composer_with_runtime_meta_and_status() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Bottom anchored".to_owned(),
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

        assert!(rows[0].contains("[workspace] · gpt-5.6-sol · high · 4%"));
        assert!(rows[4].contains("PISPER"));
        assert!(rows[4].chars().position(|ch| ch == 'P').unwrap() < 20);
        assert!(rows[19].contains("Thinking."));
        let message_row = rows
            .iter()
            .position(|row| row.contains("Pisper is ready."))
            .unwrap();
        assert!(
            message_row >= 15,
            "message rendered too high: row {message_row}"
        );
        assert!(message_row < 19, "message crossed into the run state");
    }

    #[test]
    fn narrow_headers_keep_the_execution_mode_visible() {
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
            let first_row = (0..width)
                .filter_map(|x| terminal.backend().buffer().cell((x, 0)))
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                first_row.contains(expected),
                "mode missing at width {width}: {first_row}"
            );
        }
    }

    #[test]
    fn approval_panel_keeps_the_command_and_keys_visible_at_terminal_sizes() {
        for (width, height) in [(80, 24), (36, 12)] {
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
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();

            assert!(rendered.contains("Approval required"));
            assert!(rendered.contains("date +%A"));
            assert!(rendered.contains("[Y]"));
            assert!(rendered.contains("Allow"));
            assert!(rendered.contains("[N"));
            assert!(rendered.contains("Deny"));
        }
    }

    #[test]
    fn slash_menu_aligns_with_the_composer_instead_of_centering() {
        let area = slash_menu_area(ratatui::layout::Rect::new(0, 30, 160, 3), 12);
        assert_eq!(area.x, 2);
        assert_eq!(area.width, 156);
        assert_eq!(area.y + area.height, 30);
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
    fn startup_brand_shares_the_viewport_with_existing_history() {
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
        assert!(rendered.contains("╭─────────╮"));
        assert!(rendered.contains("├──╯"));
        assert!(rendered.contains("PISPER"));
        assert!(rendered.contains("Existing history"));
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

        assert!(!rendered.contains("PISPER"));
        assert!(rendered.contains("History line 19"));
    }
}
