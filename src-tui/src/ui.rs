//! 终端渲染层（ratatui）：把 `App` 状态绘制为 UI。
//!
//! 布局逻辑：普通对话视图（消息 + 计划 + 运行态 + 输入框 + 状态栏）、
//! 欢迎视图（空会话时居中品牌）、变更视图，以及各类弹窗（Slash 目录、
//! 会话选择、附件选择、设置、Provider 凭据、审批）。
//! 绘制完成后统一应用主题映射（truecolor / 256 色 / 单色）。

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use serde_json::Value;
use std::{sync::OnceLock, time::SystemTime};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{App, Approval, LiveTurn, SettingsPicker, SlashCategory, SlashKind, View},
    model::{
        ChatMessage, MessageAttachment, RunActivity, ThinkingAvailability, ToolActivity,
        PROVIDER_APIS,
    },
};

// 主题色板（语义色）：深色背景 + 高对比前景，
// 具体色值由 TerminalTheme 映射到 256 色或单色。
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
// 对话正文的最大渲染宽度（超宽终端不无限拉长行，提升可读性）。
const CONVERSATION_WIDTH: u16 = 110;
// 欢迎视图内容区宽度。
const WELCOME_WIDTH: u16 = 88;
// 完整 Logo 所需的最小宽度。
const WELCOME_FULL_LOGO_WIDTH: u16 = 48;
// Slash 目录最大高度。
const SLASH_HEIGHT: u16 = 22;
// 列表高亮符号。
const PICKER_HIGHLIGHT: &str = "▌";
// 角色标签占位宽度（目前为 0：不显示角色名，靠颜色区分）。
const ROLE_GUTTER_WIDTH: usize = 0;
// 活动轨道的左缩进宽度。
const ACTIVITY_GUTTER_WIDTH: usize = 3;
// 加载动画帧。
const SPINNER_FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
// 运行状态栏呼吸灯动画帧（一个完整周期：渐亮→峰值→渐暗→全暗）。
// 字形填充度从空心到实心表达光量，模拟 LED 呼吸效果。
const BREATHING_FRAMES: [&str; 8] = ["○", "◔", "◑", "◕", "●", "◕", "◑", "◔"];

/// 终端色彩能力：truecolor / 256 色 / 单色。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalTheme {
    TrueColor,
    Ansi256,
    Monochrome,
}

static TERMINAL_THEME: OnceLock<TerminalTheme> = OnceLock::new();

/// 探测终端主题：优先环境变量显式指定，其次按 `NO_COLOR`/`COLORTERM`/`TERM` 推断。
/// 测试环境固定为 TrueColor，保证测试断言的颜色值稳定。
fn terminal_theme() -> TerminalTheme {
    if cfg!(test) {
        return TerminalTheme::TrueColor;
    }
    *TERMINAL_THEME.get_or_init(|| {
        let requested = std::env::var("PISPER_TUI_THEME").ok();
        if std::env::var_os("NO_COLOR").is_some()
            || matches!(requested.as_deref(), Some("mono" | "monochrome"))
        {
            TerminalTheme::Monochrome
        } else if matches!(requested.as_deref(), Some("truecolor" | "24bit"))
            || std::env::var("COLORTERM").is_ok_and(|value| {
                value.eq_ignore_ascii_case("truecolor") || value.eq_ignore_ascii_case("24bit")
            })
        {
            TerminalTheme::TrueColor
        } else if matches!(requested.as_deref(), Some("ansi" | "ansi256"))
            || std::env::var("TERM").is_ok_and(|term| term.contains("256color"))
        {
            TerminalTheme::Ansi256
        } else if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
            TerminalTheme::Monochrome
        } else {
            TerminalTheme::TrueColor
        }
    })
}

/// 按当前主题改写整帧的颜色（TrueColor 时无操作）。
/// 在绘制完成后统一执行，避免每个 widget 各自处理主题。
fn apply_terminal_theme(frame: &mut Frame, area: Rect) {
    let theme = terminal_theme();
    if theme == TerminalTheme::TrueColor {
        return;
    }

    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_fg(theme.map_foreground(cell.fg));
                cell.set_bg(theme.map_background(cell.bg));
            }
        }
    }
}

impl TerminalTheme {
    /// 前景色映射：256 色取色板最近似色，单色按语义归并为白/灰/深灰。
    fn map_foreground(self, color: Color) -> Color {
        match self {
            Self::TrueColor => color,
            Self::Ansi256 => match color {
                BG => Color::Indexed(233),
                SURFACE => Color::Indexed(235),
                RAISED => Color::Indexed(237),
                RULE => Color::Indexed(240),
                TEXT => Color::Indexed(253),
                MUTED => Color::Indexed(250),
                FAINT => Color::Indexed(244),
                ACCENT => Color::Indexed(81),
                GREEN => Color::Indexed(114),
                AMBER => Color::Indexed(221),
                RED => Color::Indexed(203),
                VIOLET => Color::Indexed(183),
                BLUE => Color::Indexed(111),
                other => other,
            },
            Self::Monochrome => match color {
                Color::Reset => Color::Reset,
                Color::Black => Color::Black,
                TEXT | MUTED => Color::White,
                ACCENT | GREEN | AMBER | RED | VIOLET | BLUE => Color::Gray,
                Color::DarkGray | BG | SURFACE | RAISED | RULE | FAINT => Color::DarkGray,
                _ => Color::Gray,
            },
        }
    }

    /// 背景色映射（256 色与单色）。
    fn map_background(self, color: Color) -> Color {
        match self {
            Self::TrueColor => color,
            Self::Ansi256 => match color {
                BG => Color::Indexed(233),
                SURFACE => Color::Indexed(235),
                RAISED => Color::Indexed(237),
                RULE => Color::Indexed(240),
                ACCENT => Color::Indexed(81),
                GREEN => Color::Indexed(114),
                AMBER => Color::Indexed(221),
                RED => Color::Indexed(203),
                VIOLET => Color::Indexed(183),
                BLUE => Color::Indexed(111),
                other => other,
            },
            Self::Monochrome => match color {
                Color::Reset => Color::Reset,
                RAISED | RULE => Color::DarkGray,
                ACCENT | GREEN | AMBER | RED | VIOLET | BLUE => Color::White,
                _ => Color::Black,
            },
        }
    }
}

// 欢迎视图的 ASCII 品牌 Logo（每行两段，组合成完整 Logo）。
const PISPER_LOGO: [(&str, &str); 5] = [
    ("████  █ █████  ", "████  █████ ████ "),
    ("█   █ █ █      ", "█   █ █     █   █"),
    ("████  █ █████  ", "████  ████  ████ "),
    ("█     █     █  ", "█     █     █  █ "),
    ("█     █ █████  ", "█     █████ █   █"),
];

/// 输入框高度：矮终端用单行，常规高度用两行留白。
fn composer_height(area: Rect) -> u16 {
    if area.height >= 18 {
        4
    } else {
        3
    }
}

/// 测试用绘制入口：以整帧为区域调用真实绘制逻辑，供断言整屏输出。
#[cfg(test)]
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    draw_in(frame, app, area);
}

/// 顶层绘制入口：绘制内容后统一应用主题。
pub fn draw_in(frame: &mut Frame, app: &App, area: Rect) {
    draw_content(frame, app, area);
    apply_terminal_theme(frame, area);
}

/// 内容分派：审批弹层、欢迎视图与常规视图三条路径共用状态。
fn draw_content(frame: &mut Frame, app: &App, area: Rect) {
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
            View::Changes => render_changes(frame, app, chunks[0]),
        }
        render_approval(frame, app, chunks[1]);
        render_status(frame, app, chunks[2]);
        return;
    }

    if matches!(app.view, View::Chat)
        && app.messages.is_empty()
        && app.live.is_none()
        && app
            .session
            .plan
            .as_ref()
            .is_none_or(|plan| plan.items.is_empty())
    {
        let composer = render_welcome(frame, app, area);
        render_overlays(frame, app, composer, area);
        return;
    }

    let composer_height = composer_height(area);
    let plan_height = if matches!(app.view, View::Chat) {
        plan_panel_height(app, area)
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(plan_height),
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    let plan = chunks[1];
    let run_state = chunks[2];
    let composer = chunks[3];
    let status = chunks[4];

    match app.view {
        View::Chat => render_chat(frame, app, chunks[0]),
        View::Changes => render_changes(frame, app, chunks[0]),
    }
    if plan.height > 0 {
        render_plan(frame, app, plan);
    }
    render_run_state(frame, app, run_state);
    render_composer(frame, app, composer, false);
    render_status(frame, app, status);

    render_overlays(frame, app, composer, area);
}

/// 欢迎视图：空会话时居中渲染 Logo、工作区、输入框与状态栏。
/// 返回输入框区域，供弹窗定位使用。
fn render_welcome(frame: &mut Frame, app: &App, area: Rect) -> Rect {
    let composer_height = composer_height(area);
    let full_logo =
        area.width >= WELCOME_FULL_LOGO_WIDTH && area.height >= composer_height.saturating_add(8);
    let logo_height: u16 = if full_logo {
        PISPER_LOGO.len() as u16
    } else if area.width >= 6 && area.height >= composer_height.saturating_add(3) {
        1
    } else {
        0
    };
    let workspace_height = u16::from(
        area.width >= 12
            && area.height
                >= composer_height
                    .saturating_add(logo_height)
                    .saturating_add(3),
    );
    let logo_gap = u16::from(logo_height > 0);
    let workspace_gap = u16::from(workspace_height > 0);
    let leading_height = logo_height
        .saturating_add(logo_gap)
        .saturating_add(workspace_height)
        .saturating_add(workspace_gap);
    let content_height = leading_height
        .saturating_add(composer_height)
        .saturating_add(1)
        .min(area.height);
    let content_y = area
        .y
        .saturating_add(area.height.saturating_sub(content_height) / 2);
    let horizontal_margin = if area.width >= 48 {
        2
    } else {
        u16::from(area.width >= 28)
    };
    let rail_width = area
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2))
        .min(WELCOME_WIDTH);
    let rail = centered_width(
        Rect::new(area.x, content_y, area.width, content_height),
        rail_width,
    );
    let logo = Rect::new(rail.x, rail.y, rail.width, logo_height);
    let workspace = Rect::new(
        rail.x,
        rail.y.saturating_add(logo_height).saturating_add(logo_gap),
        rail.width,
        workspace_height,
    );
    let composer = Rect::new(
        rail.x,
        rail.y.saturating_add(leading_height),
        rail.width,
        composer_height.min(content_height.saturating_sub(leading_height)),
    );
    let status = Rect::new(
        rail.x,
        composer.y.saturating_add(composer.height),
        rail.width,
        u16::from(composer.y.saturating_add(composer.height) < area.y.saturating_add(area.height)),
    );

    if logo.height > 0 {
        render_welcome_logo(frame, logo, full_logo);
    }
    if workspace.height > 0 {
        render_welcome_workspace(frame, app, workspace);
    }
    render_composer(frame, app, composer, true);
    render_welcome_status(frame, app, status);
    composer
}

/// 渲染品牌 Logo：宽终端显示完整 ASCII 大图，窄终端退化为单行文字。
fn render_welcome_logo(frame: &mut Frame, area: Rect, full: bool) {
    let lines = if full {
        PISPER_LOGO
            .iter()
            .map(|(pis, per)| {
                Line::from(vec![
                    Span::styled(
                        *pis,
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*per, Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
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

/// 欢迎视图中的工作区路径（居中、弱化显示）。
fn render_welcome_workspace(frame: &mut Frame, app: &App, area: Rect) {
    let path = single_line(&shorten_path(&app.cwd), area.width as usize);
    frame.render_widget(
        Paragraph::new(path)
            .alignment(Alignment::Center)
            .style(Style::default().fg(FAINT).bg(BG)),
        area,
    );
}

/// 欢迎视图底部的状态栏（复用常规状态栏逻辑）。
fn render_welcome_status(frame: &mut Frame, app: &App, area: Rect) {
    render_status(frame, app, area);
}

/// 依次渲染各弹窗层（Slash、会话、附件、设置、Provider）。
/// 后渲染的弹窗覆盖先渲染的，顺序即视觉层级。
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
    if app.api_key_dialog {
        render_api_key_dialog(frame, app, area);
    }
}

/// 渲染对话区：逐条消息 + 当前 LiveTurn，末行留白后按滚动偏移显示。
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
    app.render_max_scroll.set(max_scroll);
    let scroll = max_scroll.saturating_sub(app.scroll.get().min(max_scroll));
    frame.render_widget(paragraph.scroll((scroll, 0)), viewport);
}

/// 计划面板高度：无计划/太矮/太窄时收起；
/// 高终端显示 5 项，低终端 3 项，极窄终端只留标题行。
fn plan_panel_height(app: &App, area: Rect) -> u16 {
    let Some(plan) = app.session.plan.as_ref() else {
        return 0;
    };
    if plan.items.is_empty() || area.height < 12 {
        return 0;
    }
    if area.width < 64 {
        return 2;
    }
    let maximum_items = if area.height >= 36 { 5 } else { 3 };
    1 + maximum_items
}

/// 渲染计划面板：标题带进度与滚动范围，每项用符号/颜色区分状态，
/// 附带 assignee/依赖/备注等元信息。
fn render_plan(frame: &mut Frame, app: &App, area: Rect) {
    let Some(plan) = app.session.plan.as_ref() else {
        return;
    };
    if area.height < 2 || plan.items.is_empty() {
        return;
    }
    let completed = plan
        .items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let compact = area.width < 64;
    let maximum_items = area.height.saturating_sub(1) as usize;
    let current = plan
        .items
        .iter()
        .position(|item| item.status == "in_progress")
        .or_else(|| plan.items.iter().position(|item| item.status == "blocked"))
        .or_else(|| plan.items.iter().position(|item| item.status == "pending"))
        .unwrap_or_default();
    let max_scroll = if compact {
        0
    } else {
        plan.items.len().saturating_sub(maximum_items)
    };
    app.plan_max_scroll
        .set(max_scroll.min(u16::MAX as usize) as u16);
    let scroll = if compact {
        current
    } else {
        (app.plan_scroll.get() as usize).min(max_scroll)
    };
    if !compact {
        app.plan_scroll.set(scroll as u16);
    }

    let title = if max_scroll > 0 {
        format!(
            " Plan  {completed}/{}  ·  {}-{}/{}  ·  Alt+↑/↓ ",
            plan.items.len(),
            scroll + 1,
            (scroll + maximum_items).min(plan.items.len()),
            plan.items.len()
        )
    } else {
        format!(" Plan  {completed}/{} ", plan.items.len())
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(VIOLET).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(RULE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    for item in plan.items.iter().skip(scroll).take(maximum_items) {
        let (symbol, color) = match item.status.as_str() {
            "completed" => ("✓", GREEN),
            "in_progress" => ("●", ACCENT),
            "blocked" => ("!", RED),
            _ => ("○", MUTED),
        };
        let mut metadata = Vec::new();
        if !item.assignee.is_empty() {
            metadata.push(format!("@{}", item.assignee));
        }
        if !item.depends_on.is_empty() {
            metadata.push(format!("waits {}", item.depends_on.join(",")));
        }
        if inner.width >= 100 && !item.note.is_empty() {
            metadata.push(item.note.clone());
        }
        let metadata = metadata.join(" · ");
        let prefix = format!(" {symbol} ");
        let available = inner.width.saturating_sub(prefix.width() as u16) as usize;
        let title_budget = available.saturating_sub(metadata.width().saturating_add(2));
        let item_title = single_line(
            &item.title,
            if metadata.is_empty() {
                available
            } else {
                title_budget
            },
        );
        let gap = available
            .saturating_sub(item_title.width())
            .saturating_sub(metadata.width());
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(color)),
            Span::styled(
                item_title,
                Style::default()
                    .fg(if item.status == "in_progress" {
                        TEXT
                    } else {
                        MUTED
                    })
                    .add_modifier(if item.status == "in_progress" {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(metadata, Style::default().fg(FAINT)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).style(Style::default().bg(BG)), inner);
}

/// 渲染运行态单行（审批/压缩中/错误等短暂状态）。
/// 流式输出中的状态由状态栏展示，这里直接返回避免重复。
fn render_run_state(frame: &mut Frame, app: &App, area: Rect) {
    let (label, color, animate) = if app.approval.is_some() {
        ("Approval required".to_owned(), AMBER, false)
    } else if app.compacting_context {
        ("Compacting context".to_owned(), ACCENT, true)
    } else if app.status == "context compacted" {
        ("Context compacted".to_owned(), GREEN, false)
    } else if app.is_streaming() {
        return;
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
        Line::from(vec![
            Span::styled(
                format!("{} ", spinner_frame(app.status_frame)),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(color)),
        ])
    } else {
        Line::from(Span::styled(label, Style::default().fg(color)))
    };
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);
}

/// 把常见 Runtime 错误折叠成简洁的单行标签（过载/流中断/限流），
/// 其余错误原样展示并裁剪到宽度。
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

/// 当前帧对应的加载动画帧。
fn spinner_frame(animation_frame: u64) -> &'static str {
    SPINNER_FRAMES[(animation_frame as usize / 2) % SPINNER_FRAMES.len()]
}

/// 当前帧对应的状态栏呼吸灯字形；每 2 tick 换一帧，呼吸节奏更舒缓。
fn run_animation_frame(animation_frame: u64) -> &'static str {
    BREATHING_FRAMES[(animation_frame as usize / 2) % BREATHING_FRAMES.len()]
}

/// 呼吸灯帧样式：光量越高颜色越亮，配合字形填充度模拟灯丝呼吸。
fn run_animation_spans(animation_frame: u64) -> Vec<Span<'static>> {
    let glyph = run_animation_frame(animation_frame);
    let style = match glyph {
        "○" => Style::default().fg(FAINT),
        "◔" | "◑" => Style::default().fg(MUTED),
        "◕" => Style::default().fg(BLUE),
        _ => Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    };
    vec![Span::styled(glyph.to_owned(), style)]
}

/// 活动轨道：用树形符号（╭/├/╰/│）把思考、工具、子代理串成一条活动流。
#[derive(Default)]
struct ActivityRail {
    started: bool,
    last_row: Option<(usize, bool)>,
}

impl ActivityRail {
    /// 推入一个分支行（弱化色）。
    fn push(&mut self, lines: &mut Vec<Line<'static>>, content: Vec<Span<'static>>) {
        self.push_with_color(lines, content, FAINT);
    }

    /// 推入一个分支行并指定轨道颜色；记录最后一行供结束时闭合。
    fn push_with_color(
        &mut self,
        lines: &mut Vec<Line<'static>>,
        mut content: Vec<Span<'static>>,
        color: Color,
    ) {
        let prefix = if self.started { "├─ " } else { "╭─ " };
        self.started = true;
        self.last_row = Some((lines.len(), true));
        let mut spans = vec![Span::styled(prefix, Style::default().fg(color))];
        spans.append(&mut content);
        lines.push(Line::from(spans));
    }

    /// 推入续行（竖线缩进，不带分支符号）。
    fn continuation(&mut self, lines: &mut Vec<Line<'static>>, mut content: Vec<Span<'static>>) {
        self.last_row = Some((lines.len(), false));
        let mut spans = vec![Span::styled("│  ", Style::default().fg(FAINT))];
        spans.append(&mut content);
        lines.push(Line::from(spans));
    }

    /// 把最后一行（若为分支）改成闭合符号 `╰`，形成完整的树形收尾。
    fn close_last(&self, lines: &mut [Line<'static>]) {
        let Some((index, branch)) = self.last_row else {
            return;
        };
        let Some(prefix) = lines[index].spans.first_mut() else {
            return;
        };
        let style = prefix.style;
        *prefix = Span::styled(if branch { "╰─ " } else { "╰  " }, style);
    }
}

/// 把一条消息压成渲染行：用户消息带蓝色标签，Agent 消息渲染活动 + Markdown。
fn push_message(lines: &mut Vec<Line<'static>>, message: &ChatMessage, width: usize) {
    let prose_width = width.min(CONVERSATION_WIDTH as usize);
    if message.role == "user" {
        push_labeled_text(lines, "", BLUE, &message.text, prose_width, false);
        push_attachment_lines(lines, &message.attachments, prose_width);
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
        push_markdown(lines, "", ACCENT, &message.text, prose_width);
    }
    push_attachment_lines(lines, &message.attachments, prose_width);
    lines.push(Line::default());
}

/// 把流式 LiveTurn 压成渲染行：思考、工具、子代理与正文按预算分层展示。
fn push_live(
    lines: &mut Vec<Line<'static>>,
    live: &LiveTurn,
    thinking: bool,
    width: usize,
    viewport_rows: usize,
    animation_frame: u64,
) {
    let (thinking_rows, tool_rows) = live_activity_budget(viewport_rows);
    let tool_running = live.tools.iter().any(|tool| tool.status == "running");
    let thinking_active = thinking && !tool_running;
    let mut rail = ActivityRail::default();
    if !live.thinking.is_empty() || (thinking && live.text.is_empty()) {
        push_thinking(
            lines,
            &mut rail,
            &live.thinking,
            thinking_active,
            width,
            thinking_rows,
            animation_frame,
        );
    }
    push_tool_group(
        lines,
        &mut rail,
        &live.tools,
        width,
        tool_rows,
        animation_frame,
    );
    push_tool_agents(lines, &mut rail, &live.tools, width);
    if rail.started && !thinking_active && !tool_running && !live.text.is_empty() {
        rail.push_with_color(
            lines,
            vec![Span::styled(
                "response",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )],
            ACCENT,
        );
        rail.close_last(lines);
    } else if rail.started && !live.streaming {
        rail.close_last(lines);
    }
    if !live.text.is_empty() {
        push_markdown(
            lines,
            "",
            ACCENT,
            &live.text,
            width.min(CONVERSATION_WIDTH as usize),
        );
    }
}

/// 视口行数 → （思考最大行数，工具最大行数）预算：
/// 视口越小预算越紧，避免活动区挤占正文。
fn live_activity_budget(viewport_rows: usize) -> (usize, usize) {
    match viewport_rows {
        0..=7 => (1, 1),
        8..=12 => (2, 2),
        13..=20 => (3, 3),
        _ => (3, 4),
    }
}

/// 把一条已提交消息的活动渲染为树形轨道。
fn push_activity(lines: &mut Vec<Line<'static>>, activity: &RunActivity, width: usize) {
    let mut rail = ActivityRail::default();
    if !activity.thinking_text.is_empty() {
        push_thinking(
            lines,
            &mut rail,
            &activity.thinking_text,
            false,
            width,
            2,
            0,
        );
    }
    push_tool_group(lines, &mut rail, &activity.tools, width, 3, 0);
    if activity.agents.is_empty() {
        push_tool_agents(lines, &mut rail, &activity.tools, width);
    } else {
        push_agent_values(lines, &mut rail, activity.agents.iter(), width);
    }
    rail.close_last(lines);
}

/// 把工具中携带的子代理信息渲染为树形节点。
fn push_tool_agents(
    lines: &mut Vec<Line<'static>>,
    rail: &mut ActivityRail,
    tools: &[ToolActivity],
    width: usize,
) {
    push_agent_values(
        lines,
        rail,
        tools.iter().filter_map(|tool| tool.agent.as_ref()),
        width,
    );
}

/// 渲染子代理列表：名称/状态/输出详情，按状态着色。
fn push_agent_values<'a>(
    lines: &mut Vec<Line<'static>>,
    rail: &mut ActivityRail,
    agents: impl Iterator<Item = &'a Value>,
    width: usize,
) {
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
        let name_color = if matches!(status, "completed" | "done") {
            MUTED
        } else {
            VIOLET
        };
        let rail_color = match status {
            "failed" => RED,
            "completed" | "done" => FAINT,
            _ => VIOLET,
        };
        rail.push_with_color(
            lines,
            vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(name.to_owned(), Style::default().fg(name_color)),
                Span::styled(format!("  ·  {status}"), Style::default().fg(MUTED)),
            ],
            rail_color,
        );
        let detail = agent
            .get("output")
            .and_then(Value::as_str)
            .or_else(|| agent.get("message").and_then(Value::as_str))
            .unwrap_or("isolated context · inherited execution mode");
        let detail_width = width.saturating_sub(ACTIVITY_GUTTER_WIDTH + 3).max(4);
        rail.continuation(
            lines,
            vec![
                Span::raw("   "),
                Span::styled(
                    single_line(detail, detail_width),
                    Style::default().fg(MUTED),
                ),
            ],
        );
    }
}

/// 渲染思考区：活动时带 spinner 与强调色，否则用静态符号；
/// 内容按预算换行并保留尾部（旧行被截掉）。
fn push_thinking(
    lines: &mut Vec<Line<'static>>,
    rail: &mut ActivityRail,
    value: &str,
    active: bool,
    width: usize,
    max_lines: usize,
    animation_frame: u64,
) {
    let label = if width >= 32 { "thinking" } else { "think" };
    let content_indent = 2 + label.width() + 2;
    let content_width = width
        .saturating_sub(ACTIVITY_GUTTER_WIDTH + content_indent)
        .max(4);
    let content = wrapped_tail(value, content_width, max_lines);
    let label_style = if active {
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD)
    };
    let mut first = vec![
        Span::styled(
            format!(
                "{} ",
                if active {
                    spinner_frame(animation_frame)
                } else {
                    "◆"
                }
            ),
            Style::default().fg(if active { AMBER } else { FAINT }),
        ),
        Span::styled(label.to_owned(), label_style),
    ];
    if let Some(content) = content.first() {
        first.push(Span::raw("  "));
        first.push(Span::styled(content.clone(), Style::default().fg(MUTED)));
    }
    rail.push_with_color(lines, first, if active { AMBER } else { FAINT });
    for content in content.into_iter().skip(1) {
        rail.continuation(
            lines,
            vec![
                Span::raw(" ".repeat(content_indent)),
                Span::styled(content, Style::default().fg(MUTED)),
            ],
        );
    }
}

/// 文本按宽度换行后只保留尾部 `max_lines` 行（丢弃更早的内容）。
fn wrapped_tail(value: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut wrapped = wrap_text(value, width);
    wrapped.retain(|line| !line.is_empty());
    if wrapped.len() > max_lines {
        wrapped.drain(..wrapped.len() - max_lines);
        if let Some(first) = wrapped.first_mut() {
            *first = format!("…{}", first.chars().skip(1).collect::<String>());
        }
    }
    wrapped
}

/// 渲染工具组：优先展示运行中的工具，其余从新到旧补位；
/// 超预算时折叠为「N earlier · M completed」摘要行。
fn push_tool_group(
    lines: &mut Vec<Line<'static>>,
    rail: &mut ActivityRail,
    tools: &[ToolActivity],
    width: usize,
    max_rows: usize,
    animation_frame: u64,
) {
    if tools.is_empty() || max_rows == 0 {
        return;
    }
    let active_index = tools.iter().rposition(|tool| tool.status == "running");
    let reserve_summary = tools.len() > max_rows && max_rows > 1;
    let tool_capacity = max_rows.saturating_sub(usize::from(reserve_summary));
    let mut selected = Vec::with_capacity(tool_capacity);
    if let Some(index) = active_index {
        selected.push(index);
    }
    for index in (0..tools.len()).rev() {
        if selected.len() >= tool_capacity {
            break;
        }
        if Some(index) != active_index {
            selected.push(index);
        }
    }
    selected.sort_unstable();
    let hidden = tools.len().saturating_sub(selected.len());
    if reserve_summary {
        let completed = tools.iter().filter(|tool| tool.status == "done").count();
        let active = tools.iter().filter(|tool| tool.status == "running").count();
        let active = if active > 0 {
            format!(" · {active} active")
        } else {
            String::new()
        };
        rail.push(
            lines,
            vec![
                Span::styled("… ", Style::default().fg(FAINT)),
                Span::styled(
                    format!("{hidden} earlier · {completed} completed{active}"),
                    Style::default().fg(MUTED),
                ),
            ],
        );
    }
    for index in selected {
        let active = Some(index) == active_index;
        let rail_color = match tools[index].status.as_str() {
            "running" if active => ACCENT,
            "error" => RED,
            _ => FAINT,
        };
        rail.push_with_color(
            lines,
            tool_spans(&tools[index], width, active, animation_frame),
            rail_color,
        );
    }
}

/// 构造工具单行：状态符号 + 名称 + 详情 + 耗时，按终端宽度自适应列宽。
fn tool_spans(
    tool: &ToolActivity,
    width: usize,
    animate: bool,
    animation_frame: u64,
) -> Vec<Span<'static>> {
    let name_width = match width {
        0..=43 => 7,
        44..=59 => 10,
        60..=79 => 12,
        80..=109 => 15,
        _ => 18,
    };
    let meta_width = if width >= 72 {
        10
    } else if width >= 48 {
        8
    } else {
        0
    };
    let available = width.saturating_sub(ACTIVITY_GUTTER_WIDTH);
    let detail_width = available.saturating_sub(2 + name_width + meta_width);
    let status_color = match tool.status.as_str() {
        "error" => RED,
        "done" => GREEN,
        _ => AMBER,
    };
    let status_glyph = match tool.status.as_str() {
        "error" => "×",
        "done" => "✓",
        "running" if animate => spinner_frame(animation_frame),
        _ => "•",
    };
    let name = single_line(&tool.name.to_lowercase(), name_width.saturating_sub(1));
    let name_padding = name_width.saturating_sub(name.width());
    let name_style = if tool.status == "running" && animate {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    };
    let mut spans = vec![
        Span::styled(
            format!("{status_glyph} "),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{name}{}", " ".repeat(name_padding)), name_style),
    ];
    if detail_width > 0 {
        let detail = tool_detail(tool, detail_width);
        let detail_padding = detail_width.saturating_sub(detail.width());
        spans.push(Span::styled(
            format!("{detail}{}", " ".repeat(detail_padding)),
            Style::default().fg(if tool.status == "running" {
                MUTED
            } else {
                FAINT
            }),
        ));
    }
    if meta_width > 0 {
        spans.push(Span::styled(
            format!("{:>meta_width$}", tool_duration(tool)),
            Style::default().fg(MUTED),
        ));
    }
    spans
}

/// 工具耗时文本（完成态显示用时，运行中显示 running）。
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

/// 工具详情：优先取 message，其次取 args 中的第一个字符串值。
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

/// 带标签的定宽 Span（用于角色标签）。
fn padded_label_span(label: &str, width: usize, color: Color) -> Span<'static> {
    Span::styled(
        format!("{label:<width$}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// 角色标签 Span（当前宽度为 0，即不渲染标签文本）。
fn role_label_span(label: &str, color: Color) -> Span<'static> {
    padded_label_span(label, ROLE_GUTTER_WIDTH, color)
}

/// 角色标签占位（用于非首行保持对齐）。
fn role_gutter() -> Span<'static> {
    Span::raw(" ".repeat(ROLE_GUTTER_WIDTH))
}

/// 带标签的纯文本（逐行前缀标签，支持换行）。
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
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
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

/// 带样式的文本片段（Markdown 解析中间产物）。
#[derive(Clone)]
struct StyledPiece {
    text: String,
    style: Style,
}

/// 使用标准 CommonMark/GFM 渲染器生成终端文本，再按当前对话宽度安全换行。
fn push_markdown(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    color: Color,
    value: &str,
    width: usize,
) {
    let content_width = width.saturating_sub(ROLE_GUTTER_WIDTH).max(8);
    let rendered = tui_markdown::from_str(value);
    let first_content = rendered
        .lines
        .iter()
        .position(|line| line.width() > 0)
        .unwrap_or(rendered.lines.len());
    let last_content = rendered
        .lines
        .iter()
        .rposition(|line| line.width() > 0)
        .map_or(first_content, |index| index + 1);
    let mut label_used = false;
    let mut index = first_content;

    while index < last_content {
        let source = &rendered.lines[index];
        if markdown_line_starts_with(source, '┌') && markdown_line_contains(source, '┬') {
            let table_end = rendered.lines[index..last_content]
                .iter()
                .position(|line| {
                    markdown_line_starts_with(line, '└') && markdown_line_contains(line, '┴')
                })
                .map_or(index + 1, |offset| index + offset + 1);
            for row in fit_markdown_table(&rendered.lines[index..table_end], content_width) {
                push_markdown_line(lines, &mut label_used, label, color, row.spans);
            }
            index = table_end;
            continue;
        }
        if source.width() == 0 {
            lines.push(Line::default());
            index += 1;
            continue;
        }
        let pieces = markdown_line_pieces(source);
        for wrapped in wrap_styled_pieces(&pieces, content_width) {
            push_markdown_line(lines, &mut label_used, label, color, wrapped);
        }
        index += 1;
    }
}

/// 判断 Markdown 渲染行的首个字符，用于识别表格边界。
fn markdown_line_starts_with(line: &Line<'_>, expected: char) -> bool {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .next()
        == Some(expected)
}

/// 判断 Markdown 渲染行是否包含指定字符（跨所有样式片段扫描）。
/// 供测试断言行内标记结构（如粗体符号、链接标记）是否按预期渲染。
fn markdown_line_contains(line: &Line<'_>, expected: char) -> bool {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .any(|character| character == expected)
}

/// 把 Markdown 行转换为拥有独立生命周期的样式片段。
fn markdown_line_pieces(line: &Line<'_>) -> Vec<StyledPiece> {
    line.spans
        .iter()
        .map(|span| StyledPiece {
            text: span.content.to_string(),
            style: line.style.patch(span.style),
        })
        .collect()
}

/// 表格单元格：保留每个字符的样式，便于重新分配列宽后换行。
type MarkdownTableCell = Vec<(char, Style)>;

#[derive(Clone)]
struct MarkdownTableRow {
    cells: Vec<MarkdownTableCell>,
    header: bool,
}

/// 按可用宽度重排 tui-markdown 的自然宽度表格，避免边框被普通文本换行拆散。
fn fit_markdown_table(source: &[Line<'_>], width: usize) -> Vec<Line<'static>> {
    let mut header = true;
    let mut rows = Vec::new();
    let mut border_style = Style::default().fg(Color::DarkGray);
    for line in source {
        if markdown_line_starts_with(line, '├') {
            header = false;
        } else if markdown_line_starts_with(line, '│') {
            rows.push(MarkdownTableRow {
                cells: markdown_table_cells(line),
                header,
            });
        } else if let Some(span) = line.spans.first() {
            border_style = line.style.patch(span.style);
        }
    }
    let columns = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    if columns == 0 {
        return Vec::new();
    }

    let overhead = columns.saturating_mul(3).saturating_add(1);
    if width < overhead.saturating_add(columns.saturating_mul(3)) {
        return markdown_table_as_key_values(&rows, width);
    }
    let column_widths = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.cells.get(column))
                .map(|cell| styled_characters_width(cell))
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect::<Vec<_>>();
    let header_widths = rows
        .iter()
        .find(|row| row.header)
        .map(|row| {
            (0..columns)
                .map(|column| {
                    row.cells
                        .get(column)
                        .map(|cell| styled_characters_width(cell))
                        .unwrap_or(1)
                        .max(1)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![1; columns]);
    let content_budget = width.saturating_sub(overhead);
    let mut column_widths = column_widths;
    while column_widths.iter().sum::<usize>() > content_budget {
        let Some((index, _)) = column_widths
            .iter()
            .enumerate()
            .filter(|(index, value)| **value > header_widths[*index])
            .max_by_key(|(_, value)| **value)
        else {
            break;
        };
        column_widths[index] -= 1;
    }
    while column_widths.iter().sum::<usize>() > content_budget {
        let Some((index, _)) = column_widths
            .iter()
            .enumerate()
            .filter(|(_, value)| **value > 1)
            .max_by_key(|(_, value)| **value)
        else {
            break;
        };
        column_widths[index] -= 1;
    }

    let mut output = vec![markdown_table_border(
        '┌',
        '┬',
        '┐',
        &column_widths,
        border_style,
    )];
    for (row_index, row) in rows.iter().enumerate() {
        output.extend(markdown_table_row(row, &column_widths, border_style));
        if row.header && rows.get(row_index + 1).is_some_and(|next| !next.header) {
            output.push(markdown_table_border(
                '├',
                '┼',
                '┤',
                &column_widths,
                border_style,
            ));
        }
    }
    output.push(markdown_table_border(
        '└',
        '┴',
        '┘',
        &column_widths,
        border_style,
    ));
    output
}

/// 从 `│ ... │` 行切出单元格，并移除渲染器添加的一层左右填充。
fn markdown_table_cells(line: &Line<'_>) -> Vec<MarkdownTableCell> {
    let mut cells = Vec::new();
    let mut cell = MarkdownTableCell::new();
    let mut inside = false;
    for span in &line.spans {
        let style = line.style.patch(span.style);
        for character in span.content.chars() {
            if character == '│' {
                if inside {
                    while cell
                        .first()
                        .is_some_and(|(character, _)| character.is_whitespace())
                    {
                        cell.remove(0);
                    }
                    while cell
                        .last()
                        .is_some_and(|(character, _)| character.is_whitespace())
                    {
                        cell.pop();
                    }
                    cells.push(std::mem::take(&mut cell));
                }
                inside = true;
            } else if inside {
                cell.push((character, style));
            }
        }
    }
    cells
}

/// 绘制一条宽度稳定的表格边框。
fn markdown_table_border(
    left: char,
    middle: char,
    right: char,
    widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut border = String::new();
    border.push(left);
    for (index, width) in widths.iter().enumerate() {
        border.push_str(&"─".repeat(width.saturating_add(2)));
        border.push(if index + 1 == widths.len() {
            right
        } else {
            middle
        });
    }
    Line::from(Span::styled(border, style))
}

/// 绘制一行表格；单元格内容独立换行，所有列在每个视觉行保持同宽。
fn markdown_table_row(
    row: &MarkdownTableRow,
    widths: &[usize],
    border_style: Style,
) -> Vec<Line<'static>> {
    let wrapped = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            let pieces = row
                .cells
                .get(index)
                .map(markdown_table_cell_pieces)
                .unwrap_or_default();
            wrap_styled_pieces(&pieces, *width)
        })
        .collect::<Vec<_>>();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
    (0..height)
        .map(|line_index| {
            let mut spans = vec![Span::styled("│", border_style)];
            for (column, width) in widths.iter().enumerate() {
                spans.push(Span::raw(" "));
                if let Some(content) = wrapped[column].get(line_index) {
                    spans.extend(content.iter().cloned());
                    let content_width = content.iter().map(Span::width).sum::<usize>();
                    spans.push(Span::raw(" ".repeat(width.saturating_sub(content_width))));
                } else {
                    spans.push(Span::raw(" ".repeat(*width)));
                }
                spans.push(Span::raw(" "));
                spans.push(Span::styled("│", border_style));
            }
            Line::from(spans)
        })
        .collect()
}

/// 极窄终端不强画网格，改为 `表头: 值`，确保内容和层级仍可阅读。
fn markdown_table_as_key_values(rows: &[MarkdownTableRow], width: usize) -> Vec<Line<'static>> {
    let headers = rows.iter().find(|row| row.header);
    let data = rows.iter().filter(|row| !row.header).collect::<Vec<_>>();
    let display_rows = if data.is_empty() {
        rows.iter().collect::<Vec<_>>()
    } else {
        data
    };
    let display_count = display_rows.len();
    let mut output = Vec::new();
    for (row_index, row) in display_rows.into_iter().enumerate() {
        for (column, cell) in row.cells.iter().enumerate() {
            let mut pieces = headers
                .and_then(|header| header.cells.get(column))
                .map(markdown_table_cell_pieces)
                .unwrap_or_default();
            if !pieces.is_empty() {
                pieces.push(StyledPiece {
                    text: ": ".to_owned(),
                    style: Style::default(),
                });
            }
            pieces.extend(markdown_table_cell_pieces(cell));
            output.extend(
                wrap_styled_pieces(&pieces, width)
                    .into_iter()
                    .map(Line::from),
            );
        }
        if row_index + 1 < display_count {
            output.push(Line::default());
        }
    }
    output
}

/// 把单元格字符重新合并成样式片段。
fn markdown_table_cell_pieces(cell: &MarkdownTableCell) -> Vec<StyledPiece> {
    let mut pieces = Vec::<StyledPiece>::new();
    for (character, style) in cell {
        if let Some(last) = pieces.last_mut().filter(|piece| piece.style == *style) {
            last.text.push(*character);
        } else {
            pieces.push(StyledPiece {
                text: character.to_string(),
                style: *style,
            });
        }
    }
    pieces
}

/// 推入一行 Markdown 渲染结果（首行带角色标签，续行仅占位）。
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

/// 按终端显示宽度换行，同时保留跨 Span 的原始词边界和样式。
fn wrap_styled_pieces(pieces: &[StyledPiece], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut result = vec![Vec::new()];
    let mut line_width = 0usize;
    let mut pending_whitespace = Vec::<(char, Style)>::new();
    let mut word = Vec::<(char, Style)>::new();
    let mut seen_word = false;

    for piece in pieces {
        for character in piece.text.chars() {
            if character == '\n' {
                flush_styled_word(
                    &mut result,
                    &mut line_width,
                    &mut word,
                    &mut pending_whitespace,
                    &mut seen_word,
                    width,
                );
                result.push(Vec::new());
                line_width = 0;
                pending_whitespace.clear();
                seen_word = false;
            } else if character.is_whitespace() {
                flush_styled_word(
                    &mut result,
                    &mut line_width,
                    &mut word,
                    &mut pending_whitespace,
                    &mut seen_word,
                    width,
                );
                if character == '\t' {
                    pending_whitespace.extend(std::iter::repeat_n((' ', piece.style), 4));
                } else {
                    pending_whitespace.push((character, piece.style));
                }
            } else {
                word.push((character, piece.style));
            }
        }
    }
    flush_styled_word(
        &mut result,
        &mut line_width,
        &mut word,
        &mut pending_whitespace,
        &mut seen_word,
        width,
    );
    result
}

/// 把一个保留样式的词放入当前行；超长词按 Unicode 显示宽度完整续行。
fn flush_styled_word(
    rows: &mut Vec<Vec<Span<'static>>>,
    line_width: &mut usize,
    word: &mut Vec<(char, Style)>,
    pending_whitespace: &mut Vec<(char, Style)>,
    seen_word: &mut bool,
    width: usize,
) {
    if word.is_empty() {
        return;
    }
    let word_width = styled_characters_width(word);
    let whitespace_width = styled_characters_width(pending_whitespace);
    if *seen_word
        && *line_width > 0
        && line_width.saturating_add(whitespace_width + word_width) > width
    {
        rows.push(Vec::new());
        *line_width = 0;
        pending_whitespace.clear();
    } else {
        for (character, style) in pending_whitespace.drain(..) {
            push_styled_character(rows.last_mut().expect("line exists"), character, style);
            *line_width = line_width.saturating_add(character.width().unwrap_or_default());
        }
    }

    for (character, style) in word.drain(..) {
        let character_width = character.width().unwrap_or_default();
        if *line_width > 0 && line_width.saturating_add(character_width) > width {
            rows.push(Vec::new());
            *line_width = 0;
        }
        push_styled_character(rows.last_mut().expect("line exists"), character, style);
        *line_width = line_width.saturating_add(character_width);
    }
    pending_whitespace.clear();
    *seen_word = true;
}

/// 计算保留样式字符序列的终端显示宽度。
fn styled_characters_width(characters: &[(char, Style)]) -> usize {
    characters
        .iter()
        .map(|(character, _)| character.width().unwrap_or_default())
        .sum()
}

/// 合并相邻同样式字符，避免按字符换行产生大量细碎 Span。
fn push_styled_character(spans: &mut Vec<Span<'static>>, character: char, style: Style) {
    if let Some(last) = spans.last_mut().filter(|span| span.style == style) {
        last.content.to_mut().push(character);
    } else {
        spans.push(Span::styled(character.to_string(), style));
    }
}

/// 把消息附件渲染为 `+ name · kind · size` 行。
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

/// 字节数格式化（B/KiB/MiB）。
fn format_bytes(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}

/// 单词按宽度硬切（超宽单词也不溢出，逐字符换行）。
fn hard_wrap_word(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if current_width + character_width > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(character);
        current_width += character_width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// 通用文本换行：按空格切分，超宽单词内部硬切。
fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
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
            if word_width > width {
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                let mut chunks = hard_wrap_word(word, width);
                if let Some(last) = chunks.pop() {
                    result.extend(chunks);
                    current_width = last.width();
                    current = last;
                }
                continue;
            }
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

/// 渲染工作区变更视图：文件导航条 + 彩色 diff。
fn render_changes(frame: &mut Frame, app: &App, area: Rect) {
    let footer = if app.vcs_confirm_revert {
        " V again confirm · Esc cancel "
    } else if app.vcs.as_ref().is_some_and(|changes| changes.vcs == "svn") {
        " ←→ file · ↑↓ diff · R refresh · C commit · V revert · Esc close "
    } else {
        " ←→ file · ↑↓ diff · R refresh · C commit · P push · V revert · Esc close "
    };
    let block = Block::default()
        .title(" Workspace changes ")
        .title_bottom(Span::styled(footer, Style::default().fg(MUTED)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.status_error { RED } else { ACCENT }))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let Some(changes) = app.vcs.as_ref() else {
        frame.render_widget(
            Paragraph::new(if app.vcs_loading {
                "Loading Git/SVN workspace…"
            } else {
                "No workspace change data · R refresh"
            })
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
            inner,
        );
        return;
    };
    let label = match changes.vcs.as_str() {
        "svn" => "SVN",
        "git" => "Git",
        _ => "No repository",
    };
    let file_position = if changes.files.is_empty() {
        "clean".to_owned()
    } else {
        format!(
            "file {}/{}",
            app.vcs_selected.min(changes.files.len() - 1) + 1,
            changes.files.len()
        )
    };
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(u16::from(!changes.files.is_empty())),
            Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "{label} · {}{file_position} · diff {}/{}{}",
            if changes.branch.is_empty() {
                String::new()
            } else {
                format!("{} · ", changes.branch)
            },
            app.vcs_scroll.get().saturating_add(1),
            app.vcs_max_scroll.get().saturating_add(1),
            if changes.diff_truncated {
                " · truncated"
            } else {
                ""
            }
        ))
        .style(Style::default().fg(MUTED)),
        sections[0],
    );

    if !changes.files.is_empty() {
        let index = app.vcs_selected.min(changes.files.len() - 1);
        let file = &changes.files[index];
        let has_previous = index > 0;
        let has_next = index + 1 < changes.files.len();
        let navigation_width = usize::from(has_previous) * 2 + usize::from(has_next) * 2;
        let label = single_line(
            &format!("{} {}", file.status, file.path),
            sections[1]
                .width
                .saturating_sub(navigation_width as u16)
                .saturating_sub(2) as usize,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    if has_previous { "← " } else { "  " },
                    Style::default().fg(MUTED),
                ),
                Span::styled(
                    format!(" {label} "),
                    Style::default()
                        .fg(TEXT)
                        .bg(RAISED)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(if has_next { " →" } else { "" }, Style::default().fg(MUTED)),
            ]))
            .style(Style::default().bg(SURFACE)),
            sections[1],
        );
    }

    let lines = if changes.diff.is_empty() {
        vec![Line::from(Span::styled(
            "No diff available",
            Style::default().fg(MUTED),
        ))]
    } else {
        changes
            .diff
            .lines()
            .map(|line| {
                let color = if line.starts_with("+++") || line.starts_with("---") {
                    BLUE
                } else if line.starts_with('+') {
                    GREEN
                } else if line.starts_with('-') {
                    RED
                } else if line.starts_with("@@") {
                    ACCENT
                } else if line.starts_with("diff ") || line.starts_with("Index: ") {
                    VIOLET
                } else {
                    MUTED
                };
                Line::from(Span::styled(line.to_owned(), Style::default().fg(color)))
            })
            .collect::<Vec<_>>()
    };
    let max_scroll = lines.len().saturating_sub(sections[2].height as usize) as u16;
    app.vcs_max_scroll.set(max_scroll);
    let scroll = app.vcs_scroll.get().min(max_scroll);
    if scroll != app.vcs_scroll.get() {
        app.vcs_scroll.set(scroll);
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((scroll, 0))
            .style(Style::default().bg(BG)),
        sections[2],
    );
}

/// 渲染输入框（含附件行、控制提示与提交按钮），欢迎/常规两种模式。
fn render_composer(frame: &mut Frame, app: &App, area: Rect, welcome: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.width < 8 || area.height < 3 {
        let input = if app.input.is_empty() {
            "❯".to_owned()
        } else {
            format!("❯ {}", app.input_text())
        };
        frame.render_widget(
            Paragraph::new(single_line(&input, area.width as usize))
                .style(Style::default().fg(ACCENT).bg(BG)),
            area,
        );
        return;
    }

    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let focused = matches!(app.view, View::Chat) && app.accepts_composer_input();
    let has_draft = !app.input.is_empty() || !app.attachments.is_empty();
    let command_mode = app.slash_open();
    let controls_offset = area.height - 2;
    let attachment_offset = (!app.attachments.is_empty() && area.height >= 4)
        .then_some(controls_offset.saturating_sub(1));

    for offset in 0..area.height {
        let y = area.y.saturating_add(offset);
        let rail_color = if app.status_error {
            RED
        } else if focused && (has_draft || (welcome && offset == 0)) {
            ACCENT
        } else {
            RULE
        };
        if offset == area.height - 1 {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("╰", Style::default().fg(rail_color)),
                    Span::styled(
                        "─".repeat(area.width.saturating_sub(1) as usize),
                        Style::default().fg(RULE),
                    ),
                ]))
                .style(Style::default().bg(BG)),
                Rect::new(area.x, y, area.width, 1),
            );
            continue;
        }
        let marker = if offset == 0 {
            "╭─ "
        } else if offset == controls_offset || attachment_offset == Some(offset) {
            "├─ "
        } else {
            "│  "
        };
        frame.render_widget(
            Paragraph::new(marker).style(Style::default().fg(rail_color).bg(BG)),
            Rect::new(area.x, y, 3, 1),
        );
    }

    let content = Rect::new(
        area.x.saturating_add(3),
        area.y,
        area.width.saturating_sub(3),
        area.height.saturating_sub(1),
    );
    let prompt_width = 2usize;
    let available = content
        .width
        .saturating_sub(prompt_width as u16)
        .saturating_sub(1) as usize;
    let (composer_input, composer_cursor) = app.composer_input();
    let (visible, cursor_width) = visible_input(&composer_input, composer_cursor, available);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "❯ ",
                Style::default()
                    .fg(if focused { ACCENT } else { MUTED })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if visible.is_empty() {
                    "Message Pisper…".to_owned()
                } else {
                    visible
                },
                Style::default().fg(if app.input.is_empty() { MUTED } else { TEXT }),
            ),
        ]))
        .style(Style::default().bg(BG)),
        Rect::new(content.x, content.y, content.width, 1),
    );

    if let Some(offset) = attachment_offset {
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
                    single_line(&names, content.width.saturating_sub(5) as usize),
                    Style::default().fg(MUTED),
                ),
            ]))
            .style(Style::default().bg(BG)),
            Rect::new(content.x, area.y.saturating_add(offset), content.width, 1),
        );
    }

    let base_controls = if attachment_offset.is_none() && !app.attachments.is_empty() {
        format!("+{} attached  / commands", app.attachments.len())
    } else {
        "+ attach  / commands".to_owned()
    };
    let prioritize_hint = app.queued_count() > 0 || app.is_streaming() || app.slash_open();
    let controls_hint = if app.queued_count() > 0 {
        format!("{} queued", app.queued_count())
    } else if app.is_streaming() {
        "Ctrl+C stop · Enter steer".to_owned()
    } else if app.slash_open() {
        "Tab complete · Enter select".to_owned()
    } else {
        "Enter submit".to_owned()
    };
    let full_controls = format!("{base_controls}    {controls_hint}");
    let controls_width = content.width.saturating_sub(4) as usize;
    let controls = if full_controls.width() <= controls_width {
        full_controls
    } else if prioritize_hint && controls_hint.width() <= controls_width {
        controls_hint
    } else if base_controls.width() <= controls_width {
        base_controls
    } else if prioritize_hint {
        single_line(&controls_hint, controls_width)
    } else {
        single_line(&base_controls, controls_width)
    };
    frame.render_widget(
        Paragraph::new(controls).style(Style::default().fg(MUTED).bg(BG)),
        Rect::new(
            content.x,
            area.y.saturating_add(controls_offset),
            controls_width as u16,
            1,
        ),
    );

    if content.width >= 4 {
        let submit_ready = focused && has_draft && !command_mode;
        let submit_style = if submit_ready {
            Style::default()
                .fg(BG)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED).bg(RAISED)
        };
        frame.render_widget(
            Paragraph::new("↑")
                .alignment(Alignment::Center)
                .style(submit_style),
            Rect::new(
                content.x.saturating_add(content.width.saturating_sub(3)),
                area.y.saturating_add(controls_offset),
                3,
                1,
            ),
        );
    }

    if focused {
        frame.set_cursor_position(Position::new(
            content
                .x
                .saturating_add(prompt_width as u16)
                .saturating_add(cursor_width as u16),
            content.y,
        ));
    }
}

/// 把 token 数压缩成 K/M/B 形式（状态栏用量展示）。
fn compact_token_count(value: u64) -> String {
    let (divisor, suffix) = if value >= 1_000_000_000 {
        (1_000_000_000.0, "B")
    } else if value >= 1_000_000 {
        (1_000_000.0, "M")
    } else if value >= 1_000 {
        (1_000.0, "K")
    } else {
        return value.to_string();
    };
    let scaled = value as f64 / divisor;
    if scaled >= 100.0 || (scaled - scaled.round()).abs() < 0.05 {
        format!("{scaled:.0}{suffix}")
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

/// 渲染底部状态栏：运行动画/活动提示 + 模式/模型/思考级别/用量。
fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let activity = status_activity(app);
    let metrics = status_metrics(app);
    let mut spans = Vec::new();
    if status_is_running(app) {
        spans.extend(run_animation_spans(app.status_frame));
        spans.push(Span::raw("  "));
    } else if let Some((label, color)) = activity {
        spans.push(Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        format!("[{}]", display_execution_mode(&app.execution_mode)),
        Style::default().fg(MUTED),
    ));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        display_model(&app.model).to_owned(),
        Style::default().fg(TEXT),
    ));
    if !app.thinking_level.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            app.thinking_level.clone(),
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        ));
    }
    if !metrics.is_empty() {
        spans.push(Span::styled("  ·  ", Style::default().fg(FAINT)));
        spans.push(Span::styled(metrics, Style::default().fg(MUTED)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(BG)),
        area,
    );
}

/// 状态栏是否处于「运行中」动画态（无审批/无错误且运行中）。
fn status_is_running(app: &App) -> bool {
    app.approval.is_none() && !app.status_error && app.is_running_state()
}

/// 状态栏的活动提示（审批中/错误）。
fn status_activity(app: &App) -> Option<(String, Color)> {
    if app.approval.is_some() {
        return Some(("Approval required".to_owned(), AMBER));
    }
    if app.status_error {
        return Some((single_line(&app.status, 28), RED));
    }
    None
}

/// 状态栏的用量指标：token/缓存命中率/上下文占用/排队/审批。
fn status_metrics(app: &App) -> String {
    let mut metrics = Vec::new();
    if app.session_usage.requests > 0
        || app.session_usage.total_tokens > 0
        || app.session_usage.cache_hit_rate.is_some()
    {
        metrics.push(compact_token_count(app.session_usage.total_tokens));
        if let Some(rate) = app.session_usage.effective_cache_hit_rate() {
            metrics.push(format!("cache {:.0}%", rate));
        }
    } else if let Some(percent) = app.context_percent {
        metrics.push(format!("ctx {:.0}%", percent));
    }
    if app.queued_count() > 0 {
        metrics.push(format!("queued {}", app.queued_count()));
    }
    if app.approval_count() > 1 {
        metrics.push(format!("approval 1/{}", app.approval_count()));
    }
    metrics.join(" · ")
}

/// 列表高亮样式（统一用于各选择器）。
fn picker_highlight_style() -> Style {
    Style::default()
        .bg(RAISED)
        .fg(TEXT)
        .add_modifier(Modifier::BOLD)
}

/// 计算 Slash 目录弹窗区域：位于输入框上方，高度随条目数自适应。
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

/// 渲染 Slash 目录：分类页签 + 命令列表 + 底部操作提示。
fn render_slash(frame: &mut Frame, app: &App, composer: Rect) {
    let items = app.slash_items();
    let area = slash_menu_area(composer, items.len());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            format!(" /{} ", app.input_text().trim_start_matches('/')),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
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
    let counts = app.slash_kind_counts();
    let category = |value, label| {
        Span::styled(
            label,
            if app.slash_category == value {
                Style::default()
                    .fg(ACCENT)
                    .bg(RAISED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            },
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            category(SlashCategory::All, " ALL "),
            Span::raw(" "),
            category(SlashCategory::Tools, " TOOLS "),
            Span::raw(" "),
            category(SlashCategory::Skills, " SKILLS "),
            Span::raw(" "),
            category(SlashCategory::Commands, " COMMANDS "),
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
    let rows = items.iter().map(|item| {
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
        .highlight_symbol(PICKER_HIGHLIGHT)
        .highlight_style(picker_highlight_style());
    let selected = (!items.is_empty()).then(|| app.slash_selected.min(items.len() - 1));
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(list, sections[1], &mut state);

    frame.render_widget(
        Paragraph::new(format!(
            "{}/{} · ←→ category · ↑↓ select · Tab complete · T {} · S {} · C {}",
            selected.map_or(0, |index| index + 1),
            items.len(),
            counts.0,
            counts.1,
            counts.2
        ))
        .style(Style::default().fg(MUTED).bg(SURFACE)),
        sections[2],
    );
}

/// 会话修改时间 → 相对时间文本（`just now`/`5m ago`/日期）。
fn format_session_time(modified: &str, now: SystemTime) -> String {
    let Ok(timestamp) = humantime::parse_rfc3339(modified) else {
        return String::new();
    };
    let Ok(age) = now.duration_since(timestamp) else {
        return "just now".to_owned();
    };
    let seconds = age.as_secs();
    if seconds < 60 {
        "just now".to_owned()
    } else if seconds < 60 * 60 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h ago", seconds / (60 * 60))
    } else if seconds < 7 * 24 * 60 * 60 {
        format!("{}d ago", seconds / (24 * 60 * 60))
    } else {
        modified.get(..10).unwrap_or(modified).to_owned()
    }
}

/// 渲染会话选择器弹窗：搜索框 + 会话列表（名称/模型/工作区/时间）。
fn render_sessions(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(72, 64, area);
    frame.render_widget(Clear, popup);
    let sessions = app.visible_sessions();
    let selected = (!sessions.is_empty()).then(|| app.session_selected.min(sessions.len() - 1));
    let block = Block::default()
        .title(Span::styled(
            format!(" Resume conversation · {} matches ", sessions.len()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            if app.session_loading.is_some() {
                " Loading conversation… · Esc cancel "
            } else {
                " Type to search · ↑↓ choose · Enter resume · Esc close "
            },
            Style::default().fg(if app.session_loading.is_some() {
                AMBER
            } else {
                MUTED
            }),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.session_loading.is_some() {
            AMBER
        } else {
            ACCENT
        }))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);
    let (query, query_cursor) = visible_input(
        &app.session_query,
        app.session_query_cursor,
        sections[0].width.saturating_sub(4) as usize,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⌕ ", Style::default().fg(ACCENT)),
            Span::styled(
                if query.is_empty() {
                    "Search name, model, or workspace…".to_owned()
                } else {
                    query
                },
                Style::default().fg(if app.session_query.is_empty() {
                    MUTED
                } else {
                    TEXT
                }),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        ),
        sections[0],
    );

    let now = SystemTime::now();
    let content_width = sections[1].width.saturating_sub(4) as usize;
    let show_model = content_width >= 44;
    let name_width = if show_model {
        (content_width * 55 / 100).clamp(16, 38)
    } else {
        content_width
    };
    let model_width = content_width.saturating_sub(name_width);
    let rows = if sessions.is_empty() {
        vec![ListItem::new(Span::styled(
            " No matching conversations",
            Style::default().fg(MUTED),
        ))]
    } else {
        sessions
            .iter()
            .map(|session| {
                let streaming = if session.streaming { " · running" } else { "" };
                let loading = if app.session_loading.as_deref() == Some(session.id.as_str()) {
                    " · loading…"
                } else {
                    ""
                };
                let workspace = shorten_path(&session.cwd);
                let modified = format_session_time(&session.modified, now);
                let mut metadata = vec![Span::raw("   ")];
                if !modified.is_empty() {
                    metadata.push(Span::styled(modified, Style::default().fg(BLUE)));
                    metadata.push(Span::styled(" · ", Style::default().fg(FAINT)));
                }
                metadata.push(Span::styled(
                    single_line(&workspace, content_width.saturating_sub(3)),
                    Style::default().fg(MUTED),
                ));
                let model = if show_model {
                    single_line(
                        &format!("{}{}{}", display_model(&session.model), streaming, loading),
                        model_width,
                    )
                } else {
                    String::new()
                };
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            padded_single_line(&session.name, name_width),
                            Style::default().fg(TEXT),
                        ),
                        Span::styled(model, Style::default().fg(MUTED)),
                    ]),
                    Line::from(metadata),
                ])
            })
            .collect()
    };
    let list = List::new(rows)
        .highlight_symbol(PICKER_HIGHLIGHT)
        .highlight_style(picker_highlight_style());
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(list, sections[1], &mut state);
    if app.session_loading.is_none() {
        frame.set_cursor_position(Position::new(
            sections[0]
                .x
                .saturating_add(2)
                .saturating_add(query_cursor as u16),
            sections[0].y,
        ));
    }
}

/// 渲染附件路径选择器：目录浏览 + 过滤 + 已选附件管理。
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
        .highlight_symbol(PICKER_HIGHLIGHT)
        .highlight_style(picker_highlight_style());
    let mut state = ListState::default().with_selected(
        (!entries.is_empty() && !app.attachment_list_focused)
            .then_some(app.path_selected.min(entries.len().saturating_sub(1))),
    );
    frame.render_stateful_widget(list, sections[1], &mut state);
    let total_size = app
        .attachments
        .iter()
        .map(|attachment| attachment.size)
        .sum();
    let selected = if app.attachments.is_empty() {
        "Selected · none".to_owned()
    } else {
        let index = app.attachment_selected.min(app.attachments.len() - 1);
        let attachment = &app.attachments[index];
        format!(
            "Selected {}/{} · {} · {} · total {}",
            index + 1,
            app.attachments.len(),
            attachment.name,
            format_bytes(attachment.size),
            format_bytes(total_size)
        )
    };
    frame.render_widget(
        Paragraph::new(single_line(&selected, sections[2].width as usize))
            .style(
                Style::default()
                    .fg(if app.attachment_list_focused {
                        TEXT
                    } else {
                        ACCENT
                    })
                    .bg(if app.attachment_list_focused {
                        RAISED
                    } else {
                        SURFACE
                    })
                    .add_modifier(if app.attachment_list_focused {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(if app.attachment_list_focused {
                        ACCENT
                    } else {
                        RULE
                    })),
            ),
        sections[2],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                if app.attachment_list_focused {
                    "←→ selected file · Delete remove · Tab browse · Esc close"
                } else if app.attachments.is_empty() {
                    "↑↓ choose · Enter open/add · ←/Backspace parent · Esc close"
                } else {
                    "↑↓ choose · Enter open/add · Tab manage selected · Esc close"
                },
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
    if !app.attachment_list_focused {
        frame.set_cursor_position(Position::new(
            sections[0]
                .x
                .saturating_add(2)
                .saturating_add(cursor as u16),
            sections[0].y,
        ));
    }
}

/// 渲染设置选择器（模型/思考级别）。
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
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
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
        SettingsPicker::Thinking if app.thinking_levels().is_empty() => {
            let (message, color) = match &app.thinking_availability {
                ThinkingAvailability::Loading => ("Loading thinking levels…", MUTED),
                ThinkingAvailability::Unsupported => (
                    if app.thinking_message.is_empty() {
                        "The current model has no configurable thinking levels"
                    } else {
                        app.thinking_message.as_str()
                    },
                    MUTED,
                ),
                ThinkingAvailability::Error(error) => (error.as_str(), RED),
                ThinkingAvailability::Supported => ("No thinking levels returned", RED),
            };
            vec![ListItem::new(Span::styled(
                format!(" {message}"),
                Style::default().fg(color),
            ))]
        }
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
                    if picker == SettingsPicker::Thinking && count == 0 {
                        " R retry · Esc close "
                    } else {
                        " ↑↓ choose · Enter apply · Esc cancel "
                    },
                    Style::default().fg(MUTED),
                ))
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        )
        .highlight_symbol(PICKER_HIGHLIGHT)
        .highlight_style(picker_highlight_style());
    let mut state = ListState::default()
        .with_selected((count > 0).then_some(app.settings_selected.min(count.saturating_sub(1))));
    frame.render_stateful_widget(list, inner, &mut state);
}

/// 渲染 Provider 凭据对话框：先选 Provider，再编辑协议/Base URL/API Key；
/// API Key 输入一律掩码显示，绝不回显明文。
fn render_api_key_dialog(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(4).clamp(1, 72);
    let height = area.height.saturating_sub(4).clamp(1, 22);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let title = if app.api_key_provider.is_some() {
        " Provider Connection "
    } else {
        " Choose Provider "
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if let Some(provider_id) = &app.api_key_provider {
        let provider = app
            .provider_options
            .iter()
            .find(|provider| &provider.id == provider_id);
        let provider_name = provider
            .map(|provider| provider.name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(provider_id);
        let configured = provider.is_some_and(|provider| provider.configured);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Provider  ", Style::default().fg(MUTED)),
                Span::styled(provider_name.to_owned(), Style::default().fg(TEXT)),
                Span::styled(format!("  {provider_id}"), Style::default().fg(FAINT)),
            ])),
            sections[0],
        );

        let api_label = PROVIDER_APIS
            .iter()
            .find(|(api, _)| *api == app.provider_api)
            .map(|(_, label)| *label)
            .unwrap_or(PROVIDER_APIS[0].1);
        frame.render_widget(
            Paragraph::new(format!("< {api_label} >"))
                .style(Style::default().fg(TEXT))
                .block(
                    Block::default()
                        .title(" Protocol ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(if app.provider_connection_field == 0 {
                            ACCENT
                        } else {
                            RULE
                        })),
                ),
            sections[1],
        );

        let base_width = sections[2].width.saturating_sub(4) as usize;
        let (base_url, base_cursor) = visible_input(
            &app.provider_base_url_input,
            app.provider_base_url_cursor,
            base_width,
        );
        frame.render_widget(
            Paragraph::new(if base_url.is_empty() {
                "https://api.example.com/v1".to_owned()
            } else {
                base_url
            })
            .style(
                Style::default()
                    .fg(if app.provider_base_url_input.is_empty() {
                        MUTED
                    } else {
                        TEXT
                    })
                    .bg(
                        if app.provider_input_selected_all && app.provider_connection_field == 1 {
                            RAISED
                        } else {
                            SURFACE
                        },
                    ),
            )
            .block(
                Block::default()
                    .title(" Base URL ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if app.provider_connection_field == 1 {
                        ACCENT
                    } else {
                        RULE
                    })),
            ),
            sections[2],
        );

        let key_width = sections[3].width.saturating_sub(4) as usize;
        let masked_input = vec!['*'; app.api_key_input.len()];
        let (masked, key_cursor) = visible_input(&masked_input, app.api_key_cursor, key_width);
        let key_placeholder = if configured {
            "Configured - leave blank to keep"
        } else {
            "Optional API Key"
        };
        frame.render_widget(
            Paragraph::new(if masked.is_empty() {
                key_placeholder.to_owned()
            } else {
                masked
            })
            .style(
                Style::default()
                    .fg(if app.api_key_input.is_empty() {
                        MUTED
                    } else {
                        TEXT
                    })
                    .bg(
                        if app.provider_input_selected_all && app.provider_connection_field == 2 {
                            RAISED
                        } else {
                            SURFACE
                        },
                    ),
            )
            .block(
                Block::default()
                    .title(" API Key ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if app.provider_connection_field == 2 {
                        ACCENT
                    } else {
                        RULE
                    })),
            ),
            sections[3],
        );
        frame.render_widget(
            Paragraph::new("Up/Down field · ←→ edit · Ctrl+A select · Enter save · Esc back")
                .style(Style::default().fg(MUTED))
                .alignment(Alignment::Center),
            sections[5],
        );
        if app.provider_connection_field == 1 {
            frame.set_cursor_position(Position::new(
                sections[2]
                    .x
                    .saturating_add(1)
                    .saturating_add(base_cursor as u16),
                sections[2].y.saturating_add(1),
            ));
        } else if app.provider_connection_field == 2 {
            frame.set_cursor_position(Position::new(
                sections[3]
                    .x
                    .saturating_add(1)
                    .saturating_add(key_cursor as u16),
                sections[3].y.saturating_add(1),
            ));
        }
        return;
    }

    let rows = if app.provider_options.is_empty() {
        vec![ListItem::new(Span::styled(
            " No Providers available",
            Style::default().fg(MUTED),
        ))]
    } else {
        app.provider_options
            .iter()
            .map(|provider| {
                let name = if provider.name.is_empty() {
                    provider.id.as_str()
                } else {
                    provider.name.as_str()
                };
                let state = if provider.configured {
                    "configured"
                } else {
                    "not configured"
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {name}"), Style::default().fg(TEXT)),
                    Span::styled(format!("  {}", provider.id), Style::default().fg(FAINT)),
                    Span::styled(
                        format!("  {state}"),
                        Style::default().fg(if provider.configured { GREEN } else { AMBER }),
                    ),
                    Span::styled(
                        if provider.enabled { "" } else { "  disabled" },
                        Style::default().fg(MUTED),
                    ),
                ]))
            })
            .collect()
    };
    let count = app.provider_options.len();
    let list = List::new(rows)
        .block(
            Block::default()
                .title_bottom(Span::styled(
                    " ↑↓ choose · Enter continue · Esc cancel ",
                    Style::default().fg(MUTED),
                ))
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(RULE)),
        )
        .highlight_symbol(PICKER_HIGHLIGHT)
        .highlight_style(picker_highlight_style());
    let mut state = ListState::default()
        .with_selected((count > 0).then_some(app.api_key_selected.min(count.saturating_sub(1))));
    frame.render_stateful_widget(list, inner, &mut state);
}

/// 审批的命令文本（取 args 的 `command`，无则序列化整个 args）。
fn approval_command_text(approval: &Approval) -> Text<'static> {
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
    Text::from(lines)
}

/// 审批风险等级 →（显示文本，颜色）。
fn approval_risk(approval: &Approval) -> (String, Color) {
    let risk = approval.risk.trim();
    let color = if risk.eq_ignore_ascii_case("high") || risk.eq_ignore_ascii_case("critical") {
        RED
    } else if risk.eq_ignore_ascii_case("low") {
        GREEN
    } else if risk.is_empty() {
        MUTED
    } else {
        AMBER
    };
    (
        if risk.is_empty() {
            "Unspecified".to_owned()
        } else if risk.eq_ignore_ascii_case("critical") {
            "Critical".to_owned()
        } else if risk.eq_ignore_ascii_case("high") {
            "High".to_owned()
        } else if risk.eq_ignore_ascii_case("medium") {
            "Medium".to_owned()
        } else if risk.eq_ignore_ascii_case("low") {
            "Low".to_owned()
        } else {
            risk.to_owned()
        },
        color,
    )
}

/// 审批原因行（风险 + 原因）。
fn approval_reason_text(approval: &Approval) -> Text<'static> {
    let (risk, risk_color) = approval_risk(approval);
    let reason = if approval.reason.trim().is_empty() {
        "Not provided by the runtime".to_owned()
    } else {
        approval.reason.clone()
    };
    Text::from(Line::from(vec![
        Span::styled(
            "Risk · ",
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(risk, Style::default().fg(risk_color)),
        Span::raw("   "),
        Span::styled(
            "Reason · ",
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(reason, Style::default().fg(TEXT)),
    ]))
}

/// 审批原因区高度（按宽度换行后 1-3 行）。
fn approval_reason_height(approval: &Approval, width: u16) -> u16 {
    Paragraph::new(approval_reason_text(approval))
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .clamp(1, 3) as u16
}

/// 审批面板高度：按命令行数 + 原因行数计算，且不压垮聊天区。
fn approval_panel_height(app: &App, area: Rect) -> u16 {
    let Some(approval) = &app.approval else {
        return 0;
    };
    let inner_width = area.width.saturating_sub(4).max(1);
    let command_lines = Paragraph::new(approval_command_text(approval))
        .wrap(Wrap { trim: false })
        .line_count(inner_width);
    let desired = command_lines
        .saturating_add(approval_reason_height(approval, inner_width) as usize)
        .saturating_add(3)
        .max(8)
        .min(u16::MAX as usize) as u16;
    let chat_reserve = if area.height >= 16 { 3 } else { 0 };
    let maximum = area
        .height
        .saturating_sub(3)
        .saturating_sub(chat_reserve)
        .max(1);
    desired.min(maximum)
}

/// 渲染审批面板：命令（可滚动）+ 原因 + 操作提示。
fn render_approval(frame: &mut Frame, app: &App, area: Rect) {
    let Some(approval) = &app.approval else {
        return;
    };
    let (risk, risk_color) = approval_risk(approval);
    let risk_title = if risk == "Unspecified" {
        "Unspecified risk".to_owned()
    } else {
        format!("{risk} risk")
    };
    let position = if app.approval_count() > 1 {
        format!("  1/{}", app.approval_count())
    } else {
        String::new()
    };
    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(
                format!(" Approval{position} "),
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {risk_title} "), Style::default().fg(risk_color)),
            Span::styled(
                format!(" {} ", approval.tool_name),
                Style::default().fg(TEXT),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(risk_color))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let action_height = 1.min(inner.height);
    let content_height = inner.height.saturating_sub(action_height);
    let reason_height = if content_height >= 2 {
        approval_reason_height(approval, inner.width).min(content_height.saturating_sub(1))
    } else {
        0
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(reason_height),
            Constraint::Length(action_height),
        ])
        .split(inner);

    let command = Paragraph::new(approval_command_text(approval)).wrap(Wrap { trim: false });
    let max_scroll = command
        .line_count(rows[0].width.max(1))
        .saturating_sub(rows[0].height as usize)
        .min(u16::MAX as usize) as u16;
    app.approval_max_scroll.set(max_scroll);
    let scroll = app.approval_scroll.get().min(max_scroll);
    app.approval_scroll.set(scroll);
    if rows[0].height > 0 {
        frame.render_widget(
            command
                .scroll((scroll, 0))
                .style(Style::default().bg(SURFACE)),
            rows[0],
        );
    }
    if rows[1].height > 0 {
        frame.render_widget(
            Paragraph::new(approval_reason_text(approval))
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(SURFACE)),
            rows[1],
        );
    }

    let mut actions = if app.approval_is_resolving() {
        vec![Span::styled(
            "Resolving approval...",
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        )]
    } else if rows[2].width >= 68 {
        vec![
            Span::styled("Press ", Style::default().fg(MUTED)),
            Span::styled(
                "[N/Esc]",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Deny     ", Style::default().fg(TEXT)),
            Span::styled(
                "[Y]",
                Style::default().fg(risk_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Allow once", Style::default().fg(TEXT)),
        ]
    } else {
        vec![
            Span::styled(
                "[N/Esc]",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Deny  ", Style::default().fg(TEXT)),
            Span::styled(
                "[Y]",
                Style::default().fg(risk_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Allow", Style::default().fg(TEXT)),
        ]
    };
    if max_scroll > 0 {
        actions.push(Span::styled(
            if rows[2].width >= 40 {
                "  ↑↓ inspect"
            } else {
                "  ↑↓"
            },
            Style::default().fg(MUTED),
        ));
    }
    if rows[2].height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(actions)).style(Style::default().bg(SURFACE)),
            rows[2],
        );
    }
}

/// 计算输入框可见窗口：保证光标可见，超宽时滚动（按字符显示宽度）。
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

/// 区域居中（限定最大宽度）。
fn centered_width(area: Rect, maximum: u16) -> Rect {
    let width = area.width.min(maximum);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y,
        width,
        area.height,
    )
}

/// 按百分比在区域内居中一个子矩形（弹窗布局用）。
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

/// 区域内缩进。
fn inset(area: Rect, horizontal: u16, vertical: u16) -> Rect {
    Rect::new(
        area.x.saturating_add(horizontal),
        area.y.saturating_add(vertical),
        area.width.saturating_sub(horizontal.saturating_mul(2)),
        area.height.saturating_sub(vertical.saturating_mul(2)),
    )
}

/// 单行化文本：空白折叠，超宽截断并追加省略号。
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

/// 单行化并补齐到指定宽度（列表对齐用）。
fn padded_single_line(value: &str, width: usize) -> String {
    let mut value = single_line(value, width);
    value.push_str(&" ".repeat(width.saturating_sub(value.width())));
    value
}

/// 路径缩短：去掉 Windows `\\?\` 前缀，超宽时保留尾部。
fn shorten_path(value: &str) -> String {
    let normalized = if let Some(path) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{path}")
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(value).to_owned()
    };
    if normalized.width() <= 70 {
        normalized
    } else {
        format!(
            "…{}",
            normalized
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

/// 模型展示名：取 `provider/model` 的 model 段。
fn display_model(value: &str) -> &str {
    value
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("model")
}

/// 执行模式展示：空值退回 full-access。
fn display_execution_mode(value: &str) -> &str {
    if value.is_empty() {
        "full-access"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, style::Modifier, Terminal};
    use unicode_width::UnicodeWidthStr;

    use super::{
        compact_token_count, draw, format_session_time, padded_single_line, push_live,
        push_markdown, push_message, render_slash, runtime_error_label, shorten_path,
        slash_menu_area, visible_input, TerminalTheme, ACCENT, AMBER, BG, BLUE, CONVERSATION_WIDTH,
        FAINT, MUTED, RAISED, RED, RULE, TEXT,
    };
    use crate::{
        app::{App, Approval, AttachmentDraft, LiveTurn, PathEntry, SettingsPicker},
        model::{
            ChatMessage, MessagePage, ModelOption, PageInfo, Plan, PlanCounts, PlanItem,
            ProviderOption, SessionSummary, ThinkingLevelUpdate, ToolActivity,
        },
    };

    /// 把整帧 buffer 展平为纯文本（按行序拼接所有 cell），供整屏断言。
    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    /// 把 buffer 拆成逐行文本数组（按给定宽高裁剪），供逐行断言。
    fn buffer_rows(buffer: &ratatui::buffer::Buffer, width: u16, height: u16) -> Vec<String> {
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// 用指定尺寸的 TestBackend 渲染应用并返回整帧 buffer。
    fn render_test_buffer(app: &App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// 构造“欢迎视图”测试应用（空会话、无活动），供空态 UI 断言。
    fn welcome_test_app() -> App {
        let session = SessionSummary {
            id: "session-welcome-ui".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            thinking_level: "high".to_owned(),
            ..SessionSummary::default()
        };
        App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        )
    }

    /// 构造“运行中”测试应用：带一条用户消息与给定 LiveTurn 状态，
    /// 供流式/工具/思考等运行态 UI 断言。
    fn live_test_app(status: &str, live: LiveTurn) -> App {
        let session = SessionSummary {
            id: "session-live-ui".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            thinking_level: "high".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Polish the streaming view".to_owned(),
                ..ChatMessage::default()
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        app.status = status.to_owned();
        app.live = Some(live);
        app
    }

    /// 验证 Provider 过载错误的展示标签简洁且不超过给定宽度。
    #[test]
    fn exhausted_provider_errors_are_concise_and_bounded() {
        let label = runtime_error_label(
            "overloaded_error: Message: Pisper is overloaded. Please try again later.",
            48,
        );
        assert_eq!(label, "Error · Provider overloaded · automatic retries…");
        assert!(label.width() <= 48);
    }

    /// 验证输入窗口滚动保证光标始终可见。
    #[test]
    fn input_window_keeps_the_cursor_visible() {
        let input: Vec<char> = "1234567890".chars().collect();
        let (visible, cursor) = visible_input(&input, input.len(), 5);
        assert_eq!(visible, "7890");
        assert_eq!(cursor, 4);
    }

    /// 验证会话时间展示紧凑且非法时间值安全返回空串。
    #[test]
    fn session_times_stay_compact_and_handle_invalid_values() {
        assert_eq!(shorten_path(r"\\?\E:\code\pi-coder"), r"E:\code\pi-coder");
        assert_eq!(
            shorten_path(r"\\?\UNC\server\share\project"),
            r"\\server\share\project"
        );
        let now = humantime::parse_rfc3339("2026-08-04T12:00:00Z").unwrap();
        assert_eq!(format_session_time("2026-08-04T11:59:30Z", now), "just now");
        assert_eq!(format_session_time("2026-08-04T11:55:00Z", now), "5m ago");
        assert_eq!(format_session_time("2026-08-04T09:00:00Z", now), "3h ago");
        assert_eq!(format_session_time("2026-08-02T12:00:00Z", now), "2d ago");
        assert_eq!(
            format_session_time("2026-07-01T12:00:00Z", now),
            "2026-07-01"
        );
        assert_eq!(format_session_time("not-a-time", now), "");
    }

    /// 验证 Markdown 渲染支持 GFM 结构与嵌套行内样式。
    #[test]
    fn markdown_renderer_supports_gfm_structure_and_nested_inline_styles() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "●",
            super::ACCENT,
            "## Root cause\n\n- [x] Keep **cleanup and *tests*** scoped.\n  - Preserve [the link](https://example.com).\n\n| File | State |\n| --- | ---: |\n| ui.rs | ready |\n\n```rust\nlet value = 42;\n```",
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
        assert!(rendered.contains("- [x] Keep cleanup and tests scoped."));
        assert!(rendered.contains("\n    - Preserve the link (https://example.com)."));
        assert!(rendered.contains("ui.rs"));
        assert!(rendered.contains("ready"));
        assert!(rendered.contains("```rust"));
        assert!(rendered.contains("let value = 42;"));
        assert!(lines.iter().all(|line| line.width() <= 80));
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| span
            .content
            .contains("tests")
            && span.style.add_modifier.contains(Modifier::BOLD)
            && span.style.add_modifier.contains(Modifier::ITALIC)));
    }

    /// 验证 Markdown 表格换行重排不破坏网格对齐。
    #[test]
    fn markdown_tables_reflow_without_breaking_the_grid() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "",
            super::ACCENT,
            "| Package | Description |\n| --- | --- |\n| tui-markdown | Standards-based terminal renderer |",
            26,
        );
        let rows = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rows.iter().all(|row| row.width() <= 26));
        assert!(rows
            .first()
            .is_some_and(|row| row.starts_with('┌') && row.ends_with('┐')));
        assert!(rows
            .last()
            .is_some_and(|row| row.starts_with('└') && row.ends_with('┘')));
        assert!(rows
            .iter()
            .filter(|row| row.starts_with('│'))
            .all(|row| row.ends_with('│') && row.matches('│').count() == 3));
        assert!(rows.iter().any(|row| row.contains("Standards")));
        assert!(rows.iter().any(|row| row.contains("terminal")));
        assert!(rows.iter().any(|row| row.contains("renderer")));
    }

    /// 验证长代码块换行不丢失内容。
    #[test]
    fn markdown_code_wraps_without_losing_long_content() {
        let source = "const_result=alpha_beta_gamma_delta+epsilon_zeta_eta_theta;";
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "",
            super::ACCENT,
            &format!("```text\n{source}\n```"),
            14,
        );
        let rendered = lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(lines.iter().all(|line| line.width() <= 14));
        assert!(rendered.contains(source));
        assert!(!rendered.contains('…'));
    }

    /// 验证非真彩色主题下语义色对比度保持（不靠真彩色也能区分）。
    #[test]
    fn terminal_themes_preserve_semantic_contrast_without_truecolor() {
        assert_eq!(
            TerminalTheme::Ansi256.map_foreground(ACCENT),
            ratatui::style::Color::Indexed(81)
        );
        assert_eq!(
            TerminalTheme::Monochrome.map_foreground(TEXT),
            ratatui::style::Color::White
        );
        assert_eq!(
            TerminalTheme::Monochrome.map_background(RAISED),
            ratatui::style::Color::DarkGray
        );
        assert_eq!(
            TerminalTheme::Monochrome.map_foreground(ratatui::style::Color::Cyan),
            ratatui::style::Color::Gray
        );
        assert_eq!(TerminalTheme::TrueColor.map_foreground(BLUE), BLUE);
    }

    /// 验证对话角色靠颜色与间距区分（不显示可见角色标签）。
    #[test]
    fn conversation_roles_use_color_and_spacing_without_visible_labels() {
        let session = SessionSummary {
            id: "session-role-style".to_owned(),
            model: "provider/model".to_owned(),
            cwd: "/workspace".to_owned(),
            ..SessionSummary::default()
        };
        let app = App::new(
            vec![session.clone()],
            session,
            vec![
                ChatMessage {
                    role: "user".to_owned(),
                    text: "Inspect the renderer".to_owned(),
                    ..ChatMessage::default()
                },
                ChatMessage {
                    role: "agent".to_owned(),
                    text: "The renderer is stable".to_owned(),
                    ..ChatMessage::default()
                },
            ],
            None,
            Vec::new(),
            Vec::new(),
        );
        let buffer = render_test_buffer(&app, 80, 20);
        let rows = buffer_rows(&buffer, 80, 20);
        let user_row = rows
            .iter()
            .position(|row| row.starts_with("Inspect the renderer"))
            .unwrap() as u16;
        let agent_row = rows
            .iter()
            .position(|row| row.starts_with("The renderer is stable"))
            .unwrap() as u16;
        assert_eq!(buffer.cell((0, user_row)).unwrap().fg, BLUE);
        assert_eq!(buffer.cell((0, agent_row)).unwrap().fg, TEXT);
        assert!(!rows.join("\n").contains("YOU"));
        assert!(!rows.join("\n").contains("PIS  "));
    }

    /// 验证流完成状态不改变转录 buffer（完成前后前 15 行一致）。
    #[test]
    fn completion_state_does_not_change_the_transcript_buffer() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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

    /// 验证手动压缩有可见的运行中与完成两种状态。
    #[test]
    fn manual_compaction_has_visible_running_and_completed_states() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Keep enough context to compact".to_owned(),
                ..ChatMessage::default()
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        app.begin_context_compaction();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("Compacting context"));

        app.finish_context_compaction(None, None);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("Context compacted"));
    }

    /// 验证无思考 token 的流式思考不 panic，且不使用慢闪烁样式。
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
        let rendered = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.starts_with("╭─ ⠋ thinking"));
        assert!(!lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.add_modifier.contains(Modifier::SLOW_BLINK)));
    }

    /// 验证流式思考展开在回复之前且只显示一个加载指示器。
    #[test]
    fn live_thinking_is_expanded_before_the_response_with_one_spinner() {
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
        assert!(rendered[0].starts_with("╭─ ⠋ thinking"));
        assert!(rendered[0].contains("Inspect the runtime."));
        assert!(rendered[1].starts_with("│"));
        assert!(rendered[1].contains("Trace the event stream."));
        assert!(rendered
            .last()
            .unwrap()
            .contains("The implementation is ready."));
        assert_eq!(rendered.join("\n").matches('⠋').count(), 1);
        assert!(!lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.add_modifier.contains(Modifier::SLOW_BLINK)));
    }

    /// 验证工具组在思考下方用有界的单行组展示（含已办/运行统计）。
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
        assert!(rendered[0].starts_with("╭─ ◆ thinking"));
        assert!(rendered[1].contains("7 earlier · 7 completed · 1 active"));
        assert!(rendered[2].starts_with("├─ ⠋ tool-7"));
        assert_eq!(rendered.join("\n").matches('⠋').count(), 1);
        assert!(lines.iter().all(|line| line.width() <= 48));
    }

    /// 验证活动区响应式列布局，且已完成工具安静展示（不抢眼）。
    #[test]
    fn live_activity_uses_responsive_columns_and_quiet_completed_tools() {
        let mut lines = Vec::new();
        push_live(
            &mut lines,
            &LiveTurn {
                tools: vec![
                    ToolActivity {
                        name: "read_workspace_manifest".to_owned(),
                        status: "done".to_owned(),
                        message: "src-tui/Cargo.toml".to_owned(),
                        started_at: 1_000,
                        finished_at: 1_018,
                        ..ToolActivity::default()
                    },
                    ToolActivity {
                        name: "bash".to_owned(),
                        status: "running".to_owned(),
                        message: "npm run tui:test".to_owned(),
                        ..ToolActivity::default()
                    },
                ],
                streaming: true,
                ..LiveTurn::default()
            },
            false,
            120,
            20,
            0,
        );

        let spans = lines
            .iter()
            .flat_map(|line| &line.spans)
            .collect::<Vec<_>>();
        let completed = spans
            .iter()
            .find(|span| span.content.contains("read_workspace"))
            .unwrap();
        let active = spans
            .iter()
            .find(|span| span.content.trim() == "bash")
            .unwrap();
        let completed_detail = spans
            .iter()
            .find(|span| span.content.contains("src-tui/Cargo.toml"))
            .unwrap();
        assert_eq!(completed.style.fg, Some(MUTED));
        assert_eq!(active.style.fg, Some(ACCENT));
        assert_eq!(completed_detail.style.fg, Some(super::FAINT));
        assert!(lines.iter().all(|line| line.width() <= 120));
    }

    /// 验证宽屏下正文限制最大宽度，活动区仍占满整行。
    #[test]
    fn wide_conversations_bound_prose_without_narrowing_activity() {
        let mut prose = Vec::new();
        push_message(
            &mut prose,
            &ChatMessage {
                role: "agent".to_owned(),
                text: (0..36)
                    .map(|index| format!("word-{index}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                ..ChatMessage::default()
            },
            160,
        );
        assert!(prose
            .iter()
            .all(|line| line.width() <= CONVERSATION_WIDTH as usize));

        let mut activity = Vec::new();
        push_live(
            &mut activity,
            &LiveTurn {
                tools: vec![ToolActivity {
                    name: "bash".to_owned(),
                    status: "running".to_owned(),
                    message: "cargo test".to_owned(),
                    ..ToolActivity::default()
                }],
                streaming: true,
                ..LiveTurn::default()
            },
            false,
            160,
            20,
            0,
        );
        assert_eq!(activity[0].width(), 160);
    }

    /// 验证流式视觉状态在宽/标准/紧凑三种终端尺寸下都保持布局。
    #[test]
    fn live_visual_states_hold_at_wide_standard_and_compact_sizes() {
        for (width, height) in [(120, 30), (80, 24), (48, 16)] {
            let thinking = live_test_app(
                "thinking",
                LiveTurn {
                    streaming: true,
                    ..LiveTurn::default()
                },
            );
            let thinking_buffer = render_test_buffer(&thinking, width, height);
            let thinking_rows = buffer_rows(&thinking_buffer, width, height);
            let thinking_text = thinking_rows.join("\n");
            assert!(thinking_text.contains("╭─ ⠋ thinking"));
            let thinking_row = thinking_rows
                .iter()
                .position(|row| row.contains("╭─ ⠋ thinking"))
                .unwrap() as u16;
            assert_eq!(thinking_buffer.cell((0, thinking_row)).unwrap().fg, AMBER);
            assert!(thinking_rows.last().unwrap().trim_start().starts_with("○"));
            assert!(thinking_rows.last().unwrap().contains("gpt-5.6-sol  high"));
            assert!(!thinking_rows.last().unwrap().contains("Thinking"));
            assert!(!thinking_text.contains("THINK"));
            assert!(!thinking_text.contains("current run active"));
            assert!(!thinking_text.contains("token: 0"));
            assert!(!thinking_text.contains("cache —"));
            let composer_y = if height >= 18 { height - 5 } else { height - 4 };
            assert_eq!(thinking_buffer.cell((0, composer_y)).unwrap().symbol(), "╭");
            assert_eq!(
                thinking_buffer.cell((0, composer_y)).unwrap().fg,
                RULE,
                "streaming composer rail should stay quiet at {width}x{height}"
            );
            assert!(thinking_text.contains("Ctrl+C stop · Enter steer"));

            let running = live_test_app(
                "running bash",
                LiveTurn {
                    thinking: "Inspect the current rendering hierarchy.".to_owned(),
                    tools: vec![
                        ToolActivity {
                            name: "read".to_owned(),
                            status: "done".to_owned(),
                            message: "src-tui/src/ui.rs".to_owned(),
                            ..ToolActivity::default()
                        },
                        ToolActivity {
                            name: "bash".to_owned(),
                            status: "running".to_owned(),
                            message: "npm run tui:test".to_owned(),
                            ..ToolActivity::default()
                        },
                    ],
                    streaming: true,
                    ..LiveTurn::default()
                },
            );
            let running_buffer = render_test_buffer(&running, width, height);
            let running_rows = buffer_rows(&running_buffer, width, height);
            let running_text = running_rows.join("\n");
            assert!(running_text.contains("╭─ ◆ thinking"));
            assert!(running_text.contains("✓ read"));
            assert!(running_text.contains("⠋ bash"));
            let completed_row = running_rows
                .iter()
                .position(|row| row.contains("✓ read"))
                .unwrap() as u16;
            let active_row = running_rows
                .iter()
                .position(|row| row.contains("⠋ bash"))
                .unwrap() as u16;
            assert_eq!(running_buffer.cell((0, completed_row)).unwrap().fg, FAINT);
            assert_eq!(running_buffer.cell((0, active_row)).unwrap().fg, ACCENT);
            assert!(running_rows.last().unwrap().trim_start().starts_with("○"));
            assert!(!running_rows.last().unwrap().contains("Running bash"));
            assert_eq!(running_text.matches('⠋').count(), 1);
            assert!(!running_text.contains("TOOL"));

            let responding = live_test_app(
                "streaming",
                LiveTurn {
                    thinking: "Inspect the current rendering hierarchy.".to_owned(),
                    tools: vec![ToolActivity {
                        name: "read".to_owned(),
                        status: "done".to_owned(),
                        message: "src-tui/src/ui.rs".to_owned(),
                        ..ToolActivity::default()
                    }],
                    text: "The hierarchy is now stable.".to_owned(),
                    text_target: "The hierarchy is now stable.".to_owned(),
                    streaming: true,
                    ..LiveTurn::default()
                },
            );
            let responding_buffer = render_test_buffer(&responding, width, height);
            let responding_rows = buffer_rows(&responding_buffer, width, height);
            let responding_text = responding_rows.join("\n");
            assert!(responding_text.contains("╰─ response"));
            let response_row = responding_rows
                .iter()
                .position(|row| row.contains("╰─ response"))
                .unwrap() as u16;
            assert_eq!(
                responding_buffer.cell((0, response_row)).unwrap().fg,
                ACCENT
            );
            assert!(responding_text.contains("The hierarchy is now stable."));
            assert!(!responding_text.contains("PIS  The hierarchy"));
            assert!(responding_rows
                .last()
                .unwrap()
                .trim_start()
                .starts_with("○"));
            assert!(!responding_rows.last().unwrap().contains("Responding"));

            let mut error = live_test_app("stream_read_error", LiveTurn::default());
            error.live = None;
            error.status_error = true;
            let error_buffer = render_test_buffer(&error, width, height);
            let error_text = buffer_rows(&error_buffer, width, height).join("\n");
            assert!(error_text.contains("Response stream interrupted"));
            assert_eq!(error_buffer.cell((0, composer_y)).unwrap().fg, RED);
        }
    }

    /// 验证窄终端下输入框轨道优先展示停止/排队控制。
    #[test]
    fn active_composer_rail_prioritizes_run_and_queue_controls_when_narrow() {
        let mut app = live_test_app(
            "thinking",
            LiveTurn {
                streaming: true,
                ..LiveTurn::default()
            },
        );
        let streaming = render_test_buffer(&app, 48, 16);
        let streaming_rows = buffer_rows(&streaming, 48, 16);
        assert!(streaming_rows
            .iter()
            .any(|row| row.starts_with("├─ Ctrl+C stop · Enter steer")));
        assert_eq!(streaming.cell((46, 13)).unwrap().bg, RAISED);

        app.queue_input_succeeded("Queued direction".to_owned(), 1);
        let queued = render_test_buffer(&app, 36, 16);
        let queued_rows = buffer_rows(&queued, 36, 16);
        assert!(queued_rows.iter().any(|row| row.starts_with("├─ 1 queued")));
        assert!(!queued_rows
            .iter()
            .any(|row| row.starts_with("├─ Ctrl+C stop")));
    }

    /// 验证短会话从顶部开始展示，含运行时元信息与状态栏（无大 Logo）。
    #[test]
    fn short_conversations_start_at_the_top_with_runtime_meta_and_status() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Top aligned".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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
            .any(|row| row.contains("[full-access]  gpt-5.6-sol")));
        assert!(!rows.last().unwrap().contains("token: 0"));
        assert!(rows.last().unwrap().trim_start().starts_with("○"));
        assert!(!rows.last().unwrap().contains("Thinking"));
        assert!(rows.last().unwrap().contains("gpt-5.6-sol  high"));
        assert!(rows.last().unwrap().contains("ctx 4%"));
        let message_row = rows
            .iter()
            .position(|row| row.contains("Pisper is ready."))
            .unwrap();
        let thinking_row = rows
            .iter()
            .rposition(|row| row.contains("╭─ ⠋ thinking"))
            .unwrap();
        assert!(rows[message_row].starts_with("Pisper is ready."));
        assert!(
            rows[thinking_row].starts_with("╭─ ⠋ thinking"),
            "activity rail is not left aligned: {}",
            rows[thinking_row]
        );
        assert!(
            message_row <= 2,
            "message did not start at the top: {rows:?}"
        );
        assert!(
            message_row < thinking_row,
            "message crossed into the activity rail"
        );
        assert!(thinking_row < 17, "activity rail crossed into the composer");
    }

    /// 验证窄状态栏仍保留执行模式指示（不依赖页头）。
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

    /// 验证会话用量紧凑展示且只在底部状态栏出现（不进输入框区域）。
    #[test]
    fn session_usage_is_compact_and_rendered_only_in_the_bottom_status() {
        assert_eq!(compact_token_count(88_000_000), "88M");
        assert_eq!(compact_token_count(14_853), "14.9K");

        let session = SessionSummary {
            id: "session-usage".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Keep usage outside the composer".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        app.session_usage = crate::model::SessionUsage {
            total_tokens: 88_000_000,
            cache_hit_rate: Some(79.0),
            input: 80_000_000,
            output: 8_000_000,
            reasoning: 1_000_000,
            cache_read: 70_000_000,
            cache_write: 2_000_000,
            prompt_tokens: 0,
            requests: 10,
        };

        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rows = (0..24)
            .map(|y| {
                (0..120)
                    .filter_map(|x| terminal.backend().buffer().cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rows[23].contains("88M · cache 79%"));
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("88M · cache 79%"))
                .count(),
            1
        );
        assert!(!rows.join("\n").contains("cache R/W"));
        assert!(!rows.join("\n").contains("reasoning 1000000"));
    }

    /// 验证审批面板在多种终端尺寸下都保持命令与快捷键可见。
    #[test]
    fn approval_panel_keeps_the_command_and_keys_visible_at_terminal_sizes() {
        for (width, height) in [(160, 40), (80, 24), (36, 12)] {
            let session = SessionSummary {
                id: "session-1".to_owned(),
                name: "Approval".to_owned(),
                model: "openai/gpt-5.6-sol".to_owned(),
                cwd: "/workspace".to_owned(),
                execution_mode: "full-access".to_owned(),
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

            assert!(rendered.contains("Approval"));
            assert!(rendered.contains("High risk"));
            assert!(rendered.contains("Risk · High"));
            assert!(rendered.contains("date +%A"));
            assert!(rendered.contains("Reason ·"));
            assert!(rendered.contains("Runs"));
            assert!(rendered.contains("current OS user"));
            assert!(rendered.contains("[Y]"));
            assert!(rendered.contains("Allow"));
            assert!(rendered.contains("[N"));
            assert!(rendered.contains("Deny"));
            if width > CONVERSATION_WIDTH {
                let title = rows
                    .iter()
                    .find(|row| row.contains("Approval") && row.contains("High risk"))
                    .unwrap();
                assert!(title.starts_with('┌'));
                assert!(title.ends_with('┐'));
            }
        }
    }

    /// 验证滚动长命令时审批原因始终可见（不随命令滚动隐藏）。
    #[test]
    fn approval_keeps_the_reason_visible_while_scrolling_long_commands() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Approval".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let command = (0..30)
            .map(|index| format!("line-{index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
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
            args: serde_json::json!({ "command": command }),
            risk: "high".to_owned(),
            reason: "DO NOT HIDE THIS REASON".to_owned(),
        });

        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let first_page = format!("{:?}", terminal.backend().buffer());
        assert!(first_page.contains("line-00"));
        assert!(first_page.contains("DO NOT HIDE THIS REASON"));
        assert!(app.approval_max_scroll.get() > 0);

        app.approval_scroll.set(app.approval_max_scroll.get());
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let last_page = format!("{:?}", terminal.backend().buffer());
        assert!(last_page.contains("line-29"));
        assert!(last_page.contains("DO NOT HIDE THIS REASON"));
        assert!(last_page.contains("↑↓ inspect"));
    }

    /// 验证斜杠菜单滚动保证选中命令可见。
    #[test]
    fn slash_menu_scrolls_the_selected_command_into_view() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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
        app.input = vec!['/'];
        app.input_cursor = 1;
        let items = app.slash_items();
        let first = items.first().unwrap().command.clone();
        let selected = items.last().unwrap().command.clone();
        app.slash_selected = items.len() - 1;

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                render_slash(frame, &app, ratatui::layout::Rect::new(5, 22, 70, 2));
            })
            .unwrap();
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains(&selected));
        assert!(!rendered.contains(&first));
        assert!(rendered.contains(&format!("{}/{}", items.len(), items.len())));
    }

    /// 验证斜杠菜单与输入框共享内容轨道（不覆盖输入区）。
    #[test]
    fn slash_menu_shares_the_composer_content_rail() {
        let area = slash_menu_area(ratatui::layout::Rect::new(25, 30, 110, 7), 12);
        assert_eq!(area.x, 25);
        assert_eq!(area.width, 110);
        assert_eq!(area.y + area.height, 29);
    }

    /// 验证 Tab 补全提示只在斜杠菜单打开时出现。
    #[test]
    fn tab_completion_hint_only_appears_with_the_slash_menu() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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

    /// 验证对话与斜杠菜单在多种终端尺寸下都能渲染（冒烟测试）。
    #[test]
    fn conversation_and_slash_menu_render_at_terminal_sizes() {
        for (width, height) in [(160, 40), (60, 20), (36, 12)] {
            let session = SessionSummary {
                id: "session-1".to_owned(),
                name: "Terminal smoke".to_owned(),
                model: "openai/gpt-5.6-sol".to_owned(),
                cwd: "/workspace".to_owned(),
                execution_mode: "full-access".to_owned(),
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

    /// 验证 Unicode 列按终端宽度补齐与换行。
    #[test]
    fn unicode_columns_pad_and_wrap_by_terminal_width() {
        assert_eq!(padded_single_line("中文", 8).width(), 8);
        assert_eq!(padded_single_line("conversation", 8), "convers…");
        assert_eq!(
            super::wrap_text("rendering hierarchy", 9),
            ["rendering", "hierarchy"]
        );
        assert!(super::wrap_text("持续渲染中的上下文", 6)
            .iter()
            .all(|line| line.width() <= 6));
    }

    /// 验证会话选择器渲染最后活动时间与加载态文案。
    #[test]
    fn session_picker_renders_the_last_activity_time() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Timed conversation".to_owned(),
            model: "provider/model-a".to_owned(),
            cwd: "/workspace".to_owned(),
            modified: "1970-01-01T00:00:00Z".to_owned(),
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
        app.open_session_picker(false);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = format!("{:?}", terminal.backend().buffer());
        assert!(rendered.contains("Timed conversation"));
        assert!(rendered.contains("1970-01-01"));

        app.session_loading = Some("session-1".to_owned());
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let loading = format!("{:?}", terminal.backend().buffer());
        assert!(loading.contains("loading…"));
        assert!(loading.contains("Loading conversation…"));
        assert!(loading.contains("Esc cancel"));
    }

    /// 验证文件/模型/思考强度/Provider 凭据各选择器渲染真实选项内容，
    /// 且 API 密钥输入脱敏（星号遮罩，不泄露明文）。
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

        app.set_thinking_state(ThinkingLevelUpdate {
            thinking_level: "off".to_owned(),
            available_levels: vec!["off".to_owned(), "xhigh".to_owned(), "max".to_owned()],
            status: "supported".to_owned(),
            message: String::new(),
        });
        app.settings_picker = Some(SettingsPicker::Thinking);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let thinking = format!("{:?}", terminal.backend().buffer());
        assert!(thinking.contains("xhigh"));
        assert!(thinking.contains("max"));

        app.set_thinking_state(ThinkingLevelUpdate {
            thinking_level: "off".to_owned(),
            available_levels: Vec::new(),
            status: "unsupported".to_owned(),
            message: "Fixed reasoning for this model".to_owned(),
        });
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("Fixed reasoning"));

        app.settings_picker = None;
        app.set_provider_options(vec![ProviderOption {
            id: "kimi-coding".to_owned(),
            name: "Kimi Code".to_owned(),
            provider_type: "chat".to_owned(),
            enabled: true,
            configured: false,
            api: "openai-responses".to_owned(),
            base_url: "https://api.kimi.com/coding/".to_owned(),
        }]);
        app.open_api_key_dialog();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("Kimi Code"));
        app.api_key_provider = Some("kimi-coding".to_owned());
        app.provider_api = "openai-responses".to_owned();
        app.provider_base_url_input = "https://api.kimi.com/coding/".chars().collect();
        app.provider_connection_field = 2;
        app.api_key_input = "do-not-render-this-secret".chars().collect();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let api_key = format!("{:?}", terminal.backend().buffer());
        assert!(api_key.contains("Provider Connection"));
        assert!(api_key.contains("OpenAI Responses"));
        assert!(api_key.contains("api.kimi.com/coding/"));
        assert!(api_key.contains("********"));
        assert!(!api_key.contains("do-not-render-this-secret"));
    }

    /// 验证计划面板紧凑可滚动，全部完成后自动消失。
    #[test]
    fn plan_panel_is_compact_scrollable_and_disappears_when_completed() {
        for (width, height) in [(80, 24), (120, 40)] {
            let session = SessionSummary {
                id: "session-plan".to_owned(),
                model: "provider/model".to_owned(),
                cwd: "/workspace".to_owned(),
                plan: Some(Plan {
                    items: vec![
                        PlanItem {
                            id: "done-1".to_owned(),
                            title: "Inspect".to_owned(),
                            status: "completed".to_owned(),
                            ..PlanItem::default()
                        },
                        PlanItem {
                            id: "done-2".to_owned(),
                            title: "Design".to_owned(),
                            status: "completed".to_owned(),
                            ..PlanItem::default()
                        },
                        PlanItem {
                            id: "done-3".to_owned(),
                            title: "Prototype".to_owned(),
                            status: "completed".to_owned(),
                            ..PlanItem::default()
                        },
                        PlanItem {
                            id: "done-4".to_owned(),
                            title: "Review".to_owned(),
                            status: "completed".to_owned(),
                            ..PlanItem::default()
                        },
                        PlanItem {
                            id: "active".to_owned(),
                            title: "Implement protocol".to_owned(),
                            status: "in_progress".to_owned(),
                            note: "Keep clients compatible".to_owned(),
                            assignee: "builder".to_owned(),
                            depends_on: Vec::new(),
                        },
                        PlanItem {
                            id: "blocked".to_owned(),
                            title: "Verify migration".to_owned(),
                            status: "blocked".to_owned(),
                            depends_on: vec!["active".to_owned()],
                            ..PlanItem::default()
                        },
                        PlanItem {
                            id: "pending".to_owned(),
                            title: "Update docs".to_owned(),
                            status: "pending".to_owned(),
                            ..PlanItem::default()
                        },
                    ],
                    counts: PlanCounts {
                        in_progress: 1,
                        blocked: 1,
                        pending: 1,
                        completed: 4,
                        total: 7,
                    },
                    updated_at: None,
                }),
                ..SessionSummary::default()
            };
            let mut app = App::new(
                vec![session.clone()],
                session,
                vec![ChatMessage {
                    role: "agent".to_owned(),
                    text: "Working".to_owned(),
                    ..ChatMessage::default()
                }],
                None,
                Vec::new(),
                Vec::new(),
            );
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
            let plan_row = rows
                .iter()
                .position(|row| row.contains("Plan  4/7"))
                .unwrap();
            let composer_row = rows
                .iter()
                .position(|row| row.contains("Message Pisper"))
                .unwrap();
            assert!(rows.iter().any(|row| row.contains("Implement protocol")));
            assert!(rows.iter().any(|row| row.contains("Alt+↑/↓")));
            assert!(!rows.iter().any(|row| row.contains("Inspect")));
            assert!(plan_row < composer_row);

            app.set_plan(Some(Plan {
                items: vec![PlanItem {
                    id: "done".to_owned(),
                    title: "Finished".to_owned(),
                    status: "completed".to_owned(),
                    ..PlanItem::default()
                }],
                counts: PlanCounts {
                    completed: 1,
                    total: 1,
                    ..PlanCounts::default()
                },
                updated_at: None,
            }));
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let cleared = format!("{:?}", terminal.backend().buffer());
            assert!(app.session.plan.is_none());
            assert!(!cleared.contains("Plan  "));
            assert!(!cleared.contains("Plan 1/1"));
            assert!(cleared.contains("Message Pisper"));
        }
    }

    /// 验证空会话居中展示品牌且输入框轨道开放。
    #[test]
    fn empty_session_centers_the_brand_and_open_composer_rail() {
        let app = welcome_test_app();
        let buffer = render_test_buffer(&app, 160, 40);
        let rows = buffer_rows(&buffer, 160, 40);
        let input_row = rows
            .iter()
            .position(|row| row.contains("Message Pisper"))
            .unwrap();
        let controls_row = input_row + 2;
        let bottom_row = input_row + 3;

        assert!(rows.iter().any(|row| row.contains("████  █ █████")));
        assert!(!rows.iter().any(|row| row.contains("___  ___  ___")));
        assert!(rows[..input_row]
            .iter()
            .any(|row| row.trim() == "/workspace"));
        assert!(!rows.join("\n").contains("token: 0"));
        assert!(!rows.join("\n").contains("cache —"));
        assert!(rows.iter().any(|row| {
            row.trim_start()
                .starts_with("[full-access]  gpt-5.6-sol  high")
        }));
        assert!((18..=24).contains(&input_row));
        assert!(rows[input_row]
            .trim_start()
            .starts_with("╭─ ❯ Message Pisper"));
        assert!(rows[controls_row]
            .trim_start()
            .starts_with("├─ + attach  / commands"));
        assert!(rows[bottom_row].trim_start().starts_with("╰────"));
        assert_eq!(buffer.cell((36, input_row as u16)).unwrap().fg, ACCENT);
        assert_eq!(buffer.cell((36, input_row as u16 + 1)).unwrap().fg, RULE);
        assert_eq!(buffer.cell((123, input_row as u16)).unwrap().symbol(), " ");
        assert_eq!(
            buffer.cell((122, controls_row as u16)).unwrap().symbol(),
            "↑"
        );
        assert_eq!(buffer.cell((122, controls_row as u16)).unwrap().fg, MUTED);
        assert_eq!(buffer.cell((122, controls_row as u16)).unwrap().bg, RAISED);
        assert_eq!(buffer.cell((123, bottom_row as u16)).unwrap().symbol(), "─");
    }

    /// 验证欢迎页输入框轨道随草稿/命令输入改变状态（发送就绪/命令模式）。
    #[test]
    fn welcome_composer_rail_responds_to_draft_and_command_input() {
        let mut app = welcome_test_app();
        let empty = render_test_buffer(&app, 80, 24);
        let empty_rows = buffer_rows(&empty, 80, 24);
        let input_row = empty_rows
            .iter()
            .position(|row| row.contains("Message Pisper"))
            .unwrap() as u16;
        let controls_row = input_row + 2;
        assert_eq!(empty.cell((2, input_row)).unwrap().fg, ACCENT);
        assert_eq!(empty.cell((2, input_row + 1)).unwrap().fg, RULE);
        assert_eq!(empty.cell((76, controls_row)).unwrap().bg, RAISED);

        app.input = "Review this change".chars().collect();
        app.input_cursor = app.input.len();
        let ready = render_test_buffer(&app, 80, 24);
        assert_eq!(ready.cell((2, input_row)).unwrap().fg, ACCENT);
        assert_eq!(ready.cell((2, input_row + 1)).unwrap().fg, ACCENT);
        assert_eq!(ready.cell((76, controls_row)).unwrap().symbol(), "↑");
        assert_eq!(ready.cell((76, controls_row)).unwrap().fg, BG);
        assert_eq!(ready.cell((76, controls_row)).unwrap().bg, ACCENT);
        assert!(buffer_text(&ready).contains("Review this change"));

        app.input = vec!['/'];
        app.input_cursor = 1;
        let command = render_test_buffer(&app, 80, 24);
        assert_eq!(command.cell((2, input_row)).unwrap().fg, ACCENT);
        assert_eq!(command.cell((76, controls_row)).unwrap().fg, MUTED);
        assert_eq!(command.cell((76, controls_row)).unwrap().bg, RAISED);
        assert!(buffer_text(&command).contains("Tab complete"));
    }

    /// 验证欢迎页附件有自己的输入框分支展示。
    #[test]
    fn welcome_composer_rail_gives_attachments_their_own_branch() {
        let mut app = welcome_test_app();
        app.attachments.push(AttachmentDraft {
            path: "/workspace/mock.png".into(),
            name: "mock.png".to_owned(),
            kind: "image".to_owned(),
            size: 1_024,
        });
        let buffer = render_test_buffer(&app, 80, 24);
        let rows = buffer_rows(&buffer, 80, 24);
        let input_row = rows
            .iter()
            .position(|row| row.contains("Message Pisper"))
            .unwrap();

        assert!(rows[input_row + 1]
            .trim_start()
            .starts_with("├─ +1 mock.png"));
        assert!(rows[input_row + 2]
            .trim_start()
            .starts_with("├─ + attach  / commands"));
        assert!(rows[input_row + 3].trim_start().starts_with("╰────"));
        assert_eq!(buffer.cell((2, input_row as u16)).unwrap().fg, ACCENT);
        assert_eq!(buffer.cell((76, input_row as u16 + 2)).unwrap().bg, ACCENT);
    }

    /// 验证活跃会话使用全宽响应式输入框轨道（草稿/附件状态切换）。
    #[test]
    fn active_conversation_uses_the_full_width_responsive_composer_rail() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
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
        let empty = render_test_buffer(&app, 160, 40);
        let rows = buffer_rows(&empty, 160, 40);
        let input_row = rows
            .iter()
            .position(|row| row.contains("Message Pisper"))
            .unwrap() as u16;
        let controls_row = input_row + 2;
        let bottom_row = input_row + 3;

        assert_eq!(input_row, 35);
        assert!(rows[input_row as usize].starts_with("╭─ ❯ Message Pisper"));
        assert!(rows[controls_row as usize].starts_with("├─ + attach  / commands"));
        assert!(rows[bottom_row as usize].starts_with("╰────"));
        assert_eq!(empty.cell((0, input_row)).unwrap().fg, RULE);
        assert_eq!(empty.cell((159, input_row)).unwrap().symbol(), " ");
        assert_eq!(empty.cell((158, controls_row)).unwrap().fg, MUTED);
        assert_eq!(empty.cell((158, controls_row)).unwrap().bg, RAISED);
        assert_eq!(empty.cell((159, bottom_row)).unwrap().symbol(), "─");

        app.input = "Steer the active run".chars().collect();
        app.input_cursor = app.input.len();
        let ready = render_test_buffer(&app, 160, 40);
        assert_eq!(ready.cell((0, input_row)).unwrap().fg, ACCENT);
        assert_eq!(ready.cell((158, controls_row)).unwrap().fg, BG);
        assert_eq!(ready.cell((158, controls_row)).unwrap().bg, ACCENT);

        app.input.clear();
        app.input_cursor = 0;
        app.attachments.push(AttachmentDraft {
            path: "/workspace/mock.png".into(),
            name: "mock.png".to_owned(),
            kind: "image".to_owned(),
            size: 1_024,
        });
        let attached = render_test_buffer(&app, 160, 40);
        let attached_rows = buffer_rows(&attached, 160, 40);
        assert_eq!(
            attached_rows
                .iter()
                .position(|row| row.contains("Message Pisper"))
                .unwrap(),
            input_row as usize
        );
        assert!(attached_rows[input_row as usize + 1].starts_with("├─ +1 mock.png"));
        assert!(attached_rows[controls_row as usize].starts_with("├─ + attach  / commands"));
    }

    /// 验证活跃会话输入框中粘贴文本保持折叠展示。
    #[test]
    fn active_conversation_keeps_pasted_text_collapsed_in_the_composer() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
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
        app.insert_detected_paste("first line\nsecond line\nthird line");

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("[Pasted text · 3 lines]"));
        assert!(!rendered.contains("second line"));
        assert_eq!(app.input_text(), "first line\nsecond line\nthird line");
    }

    /// 验证输入框在反复改变终端尺寸后保持完整（不散架/不错位）。
    #[test]
    fn composer_stays_whole_across_repeated_height_changes() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let app = App::new(
            vec![session.clone()],
            session,
            vec![ChatMessage {
                role: "user".to_owned(),
                text: "Resize the terminal".to_owned(),
                run_activity: None,
                attachments: Vec::new(),
            }],
            None,
            Vec::new(),
            Vec::new(),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

        for (width, height) in [(80, 24), (42, 10), (120, 30), (60, 18)] {
            terminal.backend_mut().resize(width, height);
            let area = crate::resize_area(width, height).unwrap();
            crate::synchronize_terminal_size(&mut terminal, area).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            assert_eq!(terminal.size().unwrap(), area.into());
            let rows = (0..height)
                .map(|y| {
                    (0..width)
                        .filter_map(|x| terminal.backend().buffer().cell((x, y)))
                        .map(|cell| cell.symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            let input_rows = rows
                .iter()
                .enumerate()
                .filter_map(|(index, row)| row.contains("Message Pisper").then_some(index))
                .collect::<Vec<_>>();
            let expected = if height >= 18 {
                height.saturating_sub(5)
            } else {
                height.saturating_sub(4)
            } as usize;
            assert_eq!(input_rows, [expected]);
        }
    }

    /// 验证运行状态栏使用呼吸灯动画且不显示阶段文案（与工作区呼吸灯改动配套）。
    #[test]
    fn running_status_bar_uses_the_breathing_light_without_phase_text() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            thinking_level: "high".to_owned(),
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
        app.status = "streaming".to_owned();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let bottom = (0..80)
            .filter_map(|x| buffer.cell((x, 23)))
            .map(|cell| cell.symbol())
            .collect::<String>();

        // 呼吸灯每 2 tick 换一帧：推进 2 帧后字形应从最暗变为微亮。
        app.advance_status_animation();
        app.advance_status_animation();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let next_bottom = (0..80)
            .filter_map(|x| terminal.backend().buffer().cell((x, 23)))
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(bottom.trim_start().starts_with("○"));
        assert!(bottom.contains("[full-access]  gpt-5.6-sol  high"));
        assert!(next_bottom.trim_start().starts_with("◔"));
        assert!(!bottom.contains("Thinking"));
        assert!(!bottom.contains("Responding"));

        app.live = None;
        app.status = "running bash".to_owned();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let tool_bottom = (0..80)
            .filter_map(|x| terminal.backend().buffer().cell((x, 23)))
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(tool_bottom.trim_start().starts_with("◔"));
        assert!(!tool_bottom.contains("Running bash"));
    }

    /// 验证会话消息使用全宽左边缘（无启动 Logo 遮挡）。
    #[test]
    fn conversation_messages_use_the_full_width_left_edge_without_a_startup_logo() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Header must stay hidden".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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
        assert!(message.starts_with("Existing history"));
        let content_byte = message.find("Existing history").unwrap();
        assert_eq!(message[..content_byte].width(), 0);
        assert!(!rows.join("\n").contains("Header must stay hidden"));
        assert!(!rows.join("\n").contains("╭─────────╮"));
    }

    /// 验证窄终端下欢迎页输入框轨道保持紧凑清晰。
    #[test]
    fn narrow_terminal_keeps_the_compact_welcome_rail_clear() {
        let mut app = welcome_test_app();
        let buffer = render_test_buffer(&app, 36, 12);
        let rows = buffer_rows(&buffer, 36, 12);
        let rendered = rows.join("\n");
        let input_row = rows
            .iter()
            .position(|row| row.contains("Message Pisper"))
            .unwrap();

        assert!(rendered.contains("PISPER"));
        assert!(rows[input_row]
            .trim_start()
            .starts_with("╭─ ❯ Message Pisper"));
        assert!(rows[input_row + 1]
            .trim_start()
            .starts_with("├─ + attach  / commands"));
        assert!(rows[input_row + 1].contains('↑'));
        assert!(!rows[input_row + 1].contains("Enter↑"));
        assert!(rows[input_row + 2].trim_start().starts_with("╰────"));
        assert_eq!(buffer.cell((1, input_row as u16)).unwrap().fg, ACCENT);
        assert_eq!(buffer.cell((1, input_row as u16 + 1)).unwrap().fg, RULE);
        assert!(rows.iter().all(|row| row.width() == 36));

        app.attachments.push(AttachmentDraft {
            path: "/workspace/mock.png".into(),
            name: "mock.png".to_owned(),
            kind: "image".to_owned(),
            size: 1_024,
        });
        let attached = render_test_buffer(&app, 36, 12);
        assert!(buffer_text(&attached).contains("+1 attached"));
        assert_eq!(
            attached.cell((33, input_row as u16 + 1)).unwrap().bg,
            ACCENT
        );
    }

    /// 验证长会话用满整个视口（历史滚动、无 Logo）。
    #[test]
    fn long_conversations_use_the_full_viewport_without_the_logo() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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

        let scrolled = app;
        scrolled.scroll.set(u16::MAX);
        terminal.draw(|frame| draw(frame, &scrolled)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("History line 0"));
        assert!(!rendered.contains("History line 19"));
    }

    /// 验证前置追加更早历史后保持可见位置不变。
    #[test]
    fn prepended_history_keeps_the_visible_position() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            model: "openai/gpt-5.6-sol".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let messages = (0..20)
            .map(|index| ChatMessage {
                role: "agent".to_owned(),
                text: format!("Visible line {index}"),
                run_activity: None,
                attachments: Vec::new(),
            })
            .collect();
        let mut app = App::new(
            vec![session.clone()],
            session,
            messages,
            None,
            Vec::new(),
            Vec::new(),
        );
        app.set_history_window(20);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &app)).unwrap();
        app.scroll.set(app.render_max_scroll.get());
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Visible line 0"));
        let anchored_scroll = app.scroll.get();

        let older = (0..20)
            .map(|index| ChatMessage {
                role: "agent".to_owned(),
                text: format!("Older line {index}"),
                run_activity: None,
                attachments: Vec::new(),
            })
            .collect();
        app.apply_history_page(
            MessagePage {
                messages: older,
                context_usage: None,
                session_usage: None,
                page_info: PageInfo {
                    start: 0,
                    has_more: false,
                },
            },
            20,
        );
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(app.scroll.get(), anchored_scroll);
        assert!(rendered.contains("Visible line 0"));
        assert!(!rendered.contains("Older line 0"));
    }
}
