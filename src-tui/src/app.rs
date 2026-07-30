use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{
    ChatMessage, ContextUsage, RunActivity, SessionSummary, SkillDefinition, StreamEvent,
    ToolActivity, ToolDefinition,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum View {
    #[default]
    Chat,
    Events,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlashKind {
    Tool,
    Skill,
    Command,
}

#[derive(Clone, Debug)]
pub struct SlashItem {
    pub kind: SlashKind,
    pub command: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct LiveTurn {
    pub thinking: String,
    pub thinking_target: String,
    pub text: String,
    pub text_target: String,
    pub tools: Vec<ToolActivity>,
    pub streaming: bool,
}

#[derive(Clone, Debug)]
pub struct Approval {
    pub id: String,
    pub tool_name: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct EventLine {
    pub name: String,
    pub detail: String,
    pub state: String,
}

#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    Submit {
        message: String,
        requested_tool: Option<String>,
    },
    Abort,
    NewSession,
    SetExecutionMode(String),
    SwitchSession(String),
    ResolveApproval {
        id: String,
        approved: bool,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SlashUsage {
    count: u64,
    last_used: u64,
}

pub struct App {
    pub sessions: Vec<SessionSummary>,
    pub session: SessionSummary,
    pub messages: Vec<ChatMessage>,
    pub live: Option<LiveTurn>,
    pub tools: Vec<ToolDefinition>,
    pub skills: Vec<SkillDefinition>,
    pub input: Vec<char>,
    pub input_cursor: usize,
    pub slash_selected: usize,
    pub session_picker: bool,
    pub session_selected: usize,
    pub view: View,
    pub scroll: u16,
    pub show_startup_brand: bool,
    pub model: String,
    pub cwd: String,
    pub execution_mode: String,
    pub context_percent: Option<f64>,
    pub status: String,
    pub approval: Option<Approval>,
    pub events: Vec<EventLine>,
    slash_usage: HashMap<String, SlashUsage>,
    slash_usage_path: Option<PathBuf>,
    pending_slash_command: Option<String>,
}

impl App {
    pub fn new(
        sessions: Vec<SessionSummary>,
        session: SessionSummary,
        messages: Vec<ChatMessage>,
        context_usage: Option<ContextUsage>,
        tools: Vec<ToolDefinition>,
        skills: Vec<SkillDefinition>,
    ) -> Self {
        Self {
            model: session.model.clone(),
            cwd: session.cwd.clone(),
            execution_mode: session.execution_mode.clone(),
            context_percent: context_usage.and_then(|usage| usage.percent),
            sessions,
            session,
            messages,
            live: None,
            tools,
            skills,
            input: Vec::new(),
            input_cursor: 0,
            slash_selected: 0,
            session_picker: false,
            session_selected: 0,
            view: View::Chat,
            scroll: 0,
            show_startup_brand: true,
            status: String::new(),
            approval: None,
            events: Vec::new(),
            slash_usage: load_slash_usage(),
            slash_usage_path: slash_usage_path(),
            pending_slash_command: None,
        }
    }

    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    pub fn is_streaming(&self) -> bool {
        self.live.as_ref().is_some_and(|turn| turn.streaming)
    }

    pub fn has_pending_render(&self) -> bool {
        self.live.as_ref().is_some_and(|turn| {
            turn.text != turn.text_target || turn.thinking != turn.thinking_target
        })
    }

    pub fn advance_stream_render(&mut self) {
        if let Some(live) = self.live.as_mut() {
            advance_typewriter(&mut live.thinking, &live.thinking_target);
            advance_typewriter(&mut live.text, &live.text_target);
        }
    }

    pub fn slash_open(&self) -> bool {
        let input = self.input_text();
        input.starts_with('/') && !input.chars().any(char::is_whitespace)
    }

    pub fn slash_items(&self) -> Vec<SlashItem> {
        let query = self
            .input_text()
            .strip_prefix('/')
            .unwrap_or_default()
            .to_lowercase();
        let mut items = Vec::new();
        for tool in &self.tools {
            items.push(SlashItem {
                kind: SlashKind::Tool,
                command: format!("/{}", tool.id),
                detail: if tool.description.is_empty() {
                    tool.name.clone()
                } else {
                    tool.description.clone()
                },
            });
        }
        for skill in &self.skills {
            items.push(SlashItem {
                kind: SlashKind::Skill,
                command: skill.command.clone(),
                detail: skill.description.clone(),
            });
        }
        items.extend([
            command("/new", "Start a new conversation"),
            command("/sessions", "Switch conversation"),
            command("/events", "Open the event ledger"),
            command("/chat", "Return to the conversation"),
            command("/model", "Show the active model"),
            command("/mode read-only", "Allow low-risk analysis tools only"),
            command(
                "/mode workspace",
                "Allow changes inside the workspace sandbox",
            ),
            command(
                "/mode full-access",
                "Allow unrestricted files, network, and shell",
            ),
            command("/quit", "Exit Pisper"),
        ]);
        items.retain(|item| {
            query.is_empty()
                || format!("{} {}", item.command, item.detail)
                    .to_lowercase()
                    .contains(&query)
        });
        items.sort_by(|left, right| {
            let left_usage = self
                .slash_usage
                .get(&left.command)
                .cloned()
                .unwrap_or_default();
            let right_usage = self
                .slash_usage
                .get(&right.command)
                .cloned()
                .unwrap_or_default();
            right_usage
                .count
                .cmp(&left_usage.count)
                .then_with(|| right_usage.last_used.cmp(&left_usage.last_used))
                .then_with(|| left.command.cmp(&right.command))
        });
        items
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Action::None;
        }
        self.show_startup_brand = false;
        if let Some(approval) = self.approval.clone() {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.approval = None;
                    Action::ResolveApproval {
                        id: approval.id,
                        approved: true,
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.approval = None;
                    Action::ResolveApproval {
                        id: approval.id,
                        approved: false,
                    }
                }
                _ => Action::None,
            };
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return if self.is_streaming() {
                Action::Abort
            } else {
                Action::Quit
            };
        }
        if self.session_picker {
            return self.handle_session_picker(key);
        }
        if self.slash_open() {
            match key.code {
                KeyCode::Up => {
                    self.slash_selected = self.slash_selected.saturating_sub(1);
                    return Action::None;
                }
                KeyCode::Down => {
                    let max = self.slash_items().len().saturating_sub(1);
                    self.slash_selected = (self.slash_selected + 1).min(max);
                    return Action::None;
                }
                KeyCode::Esc => {
                    self.clear_input();
                    return Action::None;
                }
                KeyCode::Enter => return self.choose_slash(),
                _ => {}
            }
        }
        match key.code {
            KeyCode::Enter => self.submit_action(),
            KeyCode::Char(character) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.input.insert(self.input_cursor, character);
                    self.input_cursor += 1;
                    self.slash_selected = 0;
                }
                Action::None
            }
            KeyCode::Backspace => {
                if self.input_cursor > 0 {
                    self.input_cursor -= 1;
                    self.input.remove(self.input_cursor);
                    self.slash_selected = 0;
                }
                Action::None
            }
            KeyCode::Delete => {
                if self.input_cursor < self.input.len() {
                    self.input.remove(self.input_cursor);
                }
                Action::None
            }
            KeyCode::Left => {
                self.input_cursor = self.input_cursor.saturating_sub(1);
                Action::None
            }
            KeyCode::Right => {
                self.input_cursor = (self.input_cursor + 1).min(self.input.len());
                Action::None
            }
            KeyCode::Home => {
                self.input_cursor = 0;
                Action::None
            }
            KeyCode::End => {
                self.input_cursor = self.input.len();
                Action::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(8);
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(8);
                Action::None
            }
            KeyCode::Esc => {
                if self.view == View::Events {
                    self.view = View::Chat;
                } else {
                    self.clear_input();
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_session_picker(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.session_picker = false;
                Action::None
            }
            KeyCode::Up => {
                self.session_selected = self.session_selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.session_selected =
                    (self.session_selected + 1).min(self.sessions.len().saturating_sub(1));
                Action::None
            }
            KeyCode::Enter => {
                self.session_picker = false;
                self.sessions
                    .get(self.session_selected)
                    .map(|session| Action::SwitchSession(session.id.clone()))
                    .unwrap_or(Action::None)
            }
            _ => Action::None,
        }
    }

    fn choose_slash(&mut self) -> Action {
        let items = self.slash_items();
        let Some(item) = items.get(self.slash_selected).cloned() else {
            return Action::None;
        };
        if item.kind == SlashKind::Command {
            self.set_input(&item.command);
            return self.submit_action();
        }
        self.set_input(&format!("{} ", item.command));
        Action::None
    }

    fn submit_action(&mut self) -> Action {
        let message = self.input_text().trim().to_owned();
        if message.is_empty() {
            return Action::None;
        }
        match message.as_str() {
            "/quit" => {
                self.mark_slash_use("/quit");
                self.clear_input();
                Action::Quit
            }
            "/events" => {
                self.mark_slash_use("/events");
                self.view = View::Events;
                self.clear_input();
                Action::None
            }
            "/chat" => {
                self.mark_slash_use("/chat");
                self.view = View::Chat;
                self.clear_input();
                Action::None
            }
            "/new" => {
                if self.is_streaming() {
                    self.status = "Stop the active run before creating a conversation".to_owned();
                    return Action::None;
                }
                self.mark_slash_use("/new");
                self.clear_input();
                Action::NewSession
            }
            "/sessions" => {
                if self.is_streaming() {
                    self.status = "Stop the active run before switching conversations".to_owned();
                    return Action::None;
                }
                self.mark_slash_use("/sessions");
                self.clear_input();
                self.session_selected = self
                    .sessions
                    .iter()
                    .position(|session| session.id == self.session.id)
                    .unwrap_or(0);
                self.session_picker = true;
                Action::None
            }
            "/model" => {
                self.mark_slash_use("/model");
                self.status = format!("model · {}", self.model);
                self.clear_input();
                Action::None
            }
            "/mode" => {
                self.status = format!(
                    "mode · {} · use /mode read-only|workspace|full-access",
                    self.execution_mode
                );
                self.clear_input();
                Action::None
            }
            _ if execution_mode_command(&message).is_some() => {
                if self.is_streaming() {
                    self.status = "Stop the active run before changing mode".to_owned();
                    return Action::None;
                }
                let mode = execution_mode_command(&message).unwrap_or_default();
                self.clear_input();
                Action::SetExecutionMode(mode.to_owned())
            }
            _ if message.starts_with("/mode ") => {
                self.status = "usage · /mode read-only|workspace|full-access".to_owned();
                self.clear_input();
                Action::None
            }
            _ if self.is_streaming() => {
                self.status = "Agent is running · Ctrl+C to stop".to_owned();
                Action::None
            }
            _ => {
                self.pending_slash_command = message
                    .split_whitespace()
                    .next()
                    .filter(|value| value.starts_with('/'))
                    .map(str::to_owned);
                self.commit_live();
                self.messages.push(ChatMessage {
                    role: "user".to_owned(),
                    text: message.clone(),
                    run_activity: None,
                });
                self.live = Some(LiveTurn {
                    streaming: true,
                    ..LiveTurn::default()
                });
                self.events.push(EventLine {
                    name: "YOU".to_owned(),
                    detail: message.clone(),
                    state: "queued".to_owned(),
                });
                self.status = "thinking".to_owned();
                self.scroll = 0;
                self.clear_input();
                let requested_tool = self.requested_tool(&message);
                Action::Submit {
                    message,
                    requested_tool,
                }
            }
        }
    }

    fn requested_tool(&self, message: &str) -> Option<String> {
        let command = message.split_whitespace().next()?.strip_prefix('/')?;
        self.tools
            .iter()
            .any(|tool| tool.id == command)
            .then(|| command.to_owned())
    }

    pub fn insert_paste(&mut self, value: &str) {
        self.show_startup_brand = false;
        for character in value.replace(['\r', '\n'], " ").chars() {
            self.input.insert(self.input_cursor, character);
            self.input_cursor += 1;
        }
        self.slash_selected = 0;
    }

    pub fn replace_session(
        &mut self,
        session: SessionSummary,
        messages: Vec<ChatMessage>,
        context_usage: Option<ContextUsage>,
    ) {
        self.model = session.model.clone();
        self.cwd = session.cwd.clone();
        self.execution_mode = session.execution_mode.clone();
        self.context_percent = context_usage.and_then(|usage| usage.percent);
        self.session = session;
        self.messages = messages;
        self.live = None;
        self.pending_slash_command = None;
        self.events.clear();
        self.status.clear();
        self.view = View::Chat;
        self.scroll = 0;
        self.show_startup_brand = false;
    }

    pub fn set_execution_mode(&mut self, mode: String) {
        self.mark_slash_use(&format!("/mode {mode}"));
        self.execution_mode.clone_from(&mode);
        self.session.execution_mode.clone_from(&mode);
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == self.session.id)
        {
            session.execution_mode.clone_from(&mode);
        }
        self.status = format!("mode · {mode}");
    }

    pub fn apply_stream_event(&mut self, event: StreamEvent) {
        let detail = event_detail(&event);
        self.events.push(EventLine {
            name: event.name.to_uppercase(),
            detail,
            state: event_state(&event).to_owned(),
        });
        if self.events.len() > 200 {
            self.events.drain(..self.events.len() - 200);
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        match event.name.as_str() {
            "meta" => {
                self.model = string_field(&event.data, "model");
                self.cwd = string_field(&event.data, "cwd");
                self.execution_mode = string_field(&event.data, "executionMode");
                self.context_percent = event.data["contextUsage"]["percent"].as_f64();
            }
            "thinking_reset" => {
                live.thinking_target = string_field(&event.data, "thinkingText");
                live.thinking.clone_from(&live.thinking_target);
                self.status = "thinking".to_owned();
            }
            "thinking_patch" => {
                apply_patch(&mut live.thinking_target, &event.data);
                self.status = "thinking".to_owned();
            }
            "text_patch" => {
                apply_patch(&mut live.text_target, &event.data);
                self.status = "streaming".to_owned();
            }
            "text_delta" => {
                live.text_target
                    .push_str(&string_field(&event.data, "delta"));
                self.status = "streaming".to_owned();
            }
            "text_end" => {
                if event.data["text"].is_string() {
                    live.text_target = string_field(&event.data, "text");
                }
            }
            "tool_start" => {
                sync_live_display(live);
                live.tools.push(ToolActivity {
                    id: string_field(&event.data, "id"),
                    name: string_field(&event.data, "name"),
                    status: "running".to_owned(),
                    args: event.data.get("args").cloned().unwrap_or_default(),
                    ..ToolActivity::default()
                });
                self.status = format!("running {}", string_field(&event.data, "name"));
            }
            "tool_update" => update_tool(live, &event.data, false),
            "tool_end" => {
                update_tool(live, &event.data, true);
                self.status = "streaming".to_owned();
            }
            "context_usage" => self.context_percent = event.data["percent"].as_f64(),
            "permission_request" => {
                sync_live_display(live);
                self.approval = Some(Approval {
                    id: string_field(&event.data, "id"),
                    tool_name: string_field(&event.data, "toolName"),
                    reason: string_field(&event.data, "reason"),
                });
                self.status = "approval required".to_owned();
            }
            "session_title" => {
                self.session.name = string_field(&event.data, "name");
                if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|item| item.id == self.session.id)
                {
                    session.name.clone_from(&self.session.name);
                }
            }
            "done" => {
                if event.data["text"].is_string() {
                    live.text_target = string_field(&event.data, "text");
                }
                if let Ok(tools) =
                    serde_json::from_value::<Vec<ToolActivity>>(event.data["tools"].clone())
                {
                    live.tools = tools;
                }
                live.streaming = false;
                self.context_percent = event.data["contextUsage"]["percent"]
                    .as_f64()
                    .or(self.context_percent);
                self.status = "complete".to_owned();
            }
            "error" => {
                live.streaming = false;
                self.status = string_field(&event.data, "message");
            }
            _ => {}
        }
        self.scroll = 0;
        if event.name == "done" {
            if let Some(command) = self.pending_slash_command.take() {
                self.mark_slash_use(&command);
            }
        } else if event.name == "error" {
            self.pending_slash_command = None;
        }
    }

    pub fn stream_failed(&mut self, message: String) {
        if let Some(live) = self.live.as_mut() {
            live.streaming = false;
        }
        self.pending_slash_command = None;
        self.status = message;
    }

    fn commit_live(&mut self) {
        let Some(mut live) = self.live.take() else {
            return;
        };
        sync_live_display(&mut live);
        if live.text.is_empty() && live.thinking.is_empty() && live.tools.is_empty() {
            return;
        }
        self.messages.push(ChatMessage {
            role: "agent".to_owned(),
            text: live.text,
            run_activity: Some(RunActivity {
                thinking_text: live.thinking,
                tools: live.tools,
                ..RunActivity::default()
            }),
        });
    }

    fn mark_slash_use(&mut self, command: &str) {
        let usage = self.slash_usage.entry(command.to_owned()).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last_used = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default();
        if let Some(path) = &self.slash_usage_path {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(data) = serde_json::to_vec_pretty(&self.slash_usage) {
                let _ = fs::write(path, data);
            }
        }
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.slash_selected = 0;
    }

    fn set_input(&mut self, value: &str) {
        self.input = value.chars().collect();
        self.input_cursor = self.input.len();
        self.slash_selected = 0;
    }
}

fn execution_mode_command(message: &str) -> Option<&str> {
    let mut parts = message.split_whitespace();
    if parts.next()? != "/mode" {
        return None;
    }
    let mode = parts.next()?;
    if parts.next().is_some() || !matches!(mode, "read-only" | "workspace" | "full-access") {
        return None;
    }
    Some(mode)
}

fn command(command: &str, detail: &str) -> SlashItem {
    SlashItem {
        kind: SlashKind::Command,
        command: command.to_owned(),
        detail: detail.to_owned(),
    }
}

fn slash_usage_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pisper").join("tui-slash-usage.json"))
}

fn load_slash_usage() -> HashMap<String, SlashUsage> {
    slash_usage_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn sync_live_display(live: &mut LiveTurn) {
    live.thinking.clone_from(&live.thinking_target);
    live.text.clone_from(&live.text_target);
}

fn advance_typewriter(shown: &mut String, target: &str) -> bool {
    if shown == target {
        return false;
    }
    if !target.starts_with(shown.as_str()) {
        let prefix = common_prefix_bytes(shown, target);
        shown.truncate(prefix);
    }
    let suffix = &target[shown.len()..];
    let remaining = suffix.chars().count();
    if remaining == 0 {
        return true;
    }
    let reveal = if remaining >= 160 {
        remaining.div_ceil(8).clamp(12, 48)
    } else {
        (36 + remaining * 4).div_ceil(40).clamp(1, 16)
    };
    let end = suffix
        .char_indices()
        .nth(reveal)
        .map(|(index, _)| index)
        .unwrap_or(suffix.len());
    shown.push_str(&suffix[..end]);
    true
}

fn common_prefix_bytes(left: &str, right: &str) -> usize {
    let mut bytes = 0;
    for (left_character, right_character) in left.chars().zip(right.chars()) {
        if left_character != right_character {
            break;
        }
        bytes += left_character.len_utf8();
    }
    bytes
}

fn apply_patch(target: &mut String, value: &Value) {
    let utf16_start = value.get("start").and_then(Value::as_u64).unwrap_or(0) as usize;
    let start = utf16_offset_to_byte(target, utf16_start);
    target.truncate(start);
    target.push_str(
        value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
}

fn utf16_offset_to_byte(value: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, character) in value.char_indices() {
        if units >= offset {
            return index;
        }
        let next = units + character.len_utf16();
        if next > offset {
            return index;
        }
        units = next;
    }
    value.len()
}

fn update_tool(live: &mut LiveTurn, value: &Value, done: bool) {
    let id = string_field(value, "id");
    let Some(tool) = live.tools.iter_mut().find(|tool| tool.id == id) else {
        return;
    };
    if value["message"].is_string() {
        tool.message = string_field(value, "message");
    }
    if value["output"].is_string() {
        tool.output = string_field(value, "output");
    }
    if done {
        tool.status = if value["error"].as_bool().unwrap_or(false) {
            "error"
        } else {
            "done"
        }
        .to_owned();
    }
}

fn event_detail(event: &StreamEvent) -> String {
    match event.name.as_str() {
        "meta" => string_field(&event.data, "model"),
        "thinking_patch" => string_field(&event.data, "text"),
        "text_patch" | "text_delta" => "Agent response".to_owned(),
        "tool_start" | "tool_update" | "tool_end" => {
            let name = string_field(&event.data, "name");
            let message = string_field(&event.data, "message");
            [name, message]
                .into_iter()
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        }
        "permission_request" => string_field(&event.data, "reason"),
        "error" => string_field(&event.data, "message"),
        _ => event.name.replace('_', " "),
    }
}

fn event_state(event: &StreamEvent) -> &'static str {
    match event.name.as_str() {
        "tool_start" | "thinking_patch" | "text_patch" | "text_delta" => "active",
        "error" => "error",
        "permission_request" => "waiting",
        "done" | "tool_end" | "text_end" => "done",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{advance_typewriter, apply_patch, Action, App};
    use crate::model::{SessionSummary, StreamEvent, ToolDefinition};
    use serde_json::json;

    #[test]
    fn text_patch_replaces_the_existing_tail() {
        let mut text = "hello wor".to_owned();
        apply_patch(&mut text, &json!({ "start": 6, "text": "world" }));
        assert_eq!(text, "hello world");
    }

    #[test]
    fn text_patch_maps_javascript_utf16_offsets() {
        let mut text = "你好😀 wor".to_owned();
        apply_patch(&mut text, &json!({ "start": 5, "text": "world" }));
        assert_eq!(text, "你好😀 world");
    }

    #[test]
    fn typewriter_smooths_large_unicode_deltas_and_eventually_catches_up() {
        let target = format!("{}{}", "streaming ".repeat(30), "你好😀");
        let mut shown = String::new();

        assert!(advance_typewriter(&mut shown, &target));
        assert!(!shown.is_empty());
        assert!(shown.chars().count() <= 48);
        while shown != target {
            advance_typewriter(&mut shown, &target);
        }
        assert_eq!(shown, target);
    }

    #[test]
    fn stream_events_update_the_target_before_the_visible_text() {
        let mut app = test_app(Vec::new());
        app.set_input("stream this");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.apply_stream_event(StreamEvent {
            name: "text_delta".to_owned(),
            data: json!({ "delta": "A complete provider chunk" }),
        });

        let live = app.live.as_ref().unwrap();
        assert!(live.text.is_empty());
        assert_eq!(live.text_target, "A complete provider chunk");
        assert!(app.has_pending_render());
        app.advance_stream_render();
        assert!(!app.live.as_ref().unwrap().text.is_empty());
    }

    #[test]
    fn mode_command_changes_the_current_session_without_calling_the_agent() {
        let mut app = test_app(Vec::new());
        app.set_input("/mode full-access");
        assert!(matches!(
            app.submit_action(),
            Action::SetExecutionMode(mode) if mode == "full-access"
        ));
    }

    #[test]
    fn tool_slash_requests_the_selected_runtime_tool() {
        let mut app = test_app(vec![ToolDefinition {
            id: "read".to_owned(),
            name: "Read".to_owned(),
            description: "Read a file".to_owned(),
            enabled: true,
        }]);
        app.set_input("/read README.md");
        assert!(matches!(
            app.submit_action(),
            Action::Submit { requested_tool: Some(tool), .. } if tool == "read"
        ));
    }

    fn test_app(tools: Vec<ToolDefinition>) -> App {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "workspace".to_owned(),
            ..SessionSummary::default()
        };
        App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            tools,
            Vec::new(),
        )
    }
}
