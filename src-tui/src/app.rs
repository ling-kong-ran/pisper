use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::{
        ChatMessage, ContextUsage, MessageAttachment, ModelOption, Plan, RunActivity,
        SessionCwdUpdate, SessionModelUpdate, SessionSummary, SkillDefinition, StreamEvent,
        ThinkingAvailability, ThinkingLevelUpdate, ToolActivity, ToolDefinition,
    },
    plan_protocol::{is_plan_update_event, plan_from_payload},
    workspace::same_workspace,
};

const INIT_PROMPT: &str = "/init\n\n---\nAttachment context (injected by Pisper):\nAnalyze this codebase and create or improve `AGENTS.md` in the current workspace root. The file is long-lived guidance for Pisper and other coding agents working in this repository. Inspect the repository before writing it. Capture only project-specific, durable information: the project purpose, important directories and architecture, build/test/lint/typecheck commands, coding conventions, and verification expectations. Keep it concise and practical. Do not include generic advice, temporary task details, secrets, exhaustive file listings, or information you cannot verify. If `AGENTS.md` already exists, preserve accurate useful instructions and update it carefully instead of replacing it blindly. Modify only `AGENTS.md`. After writing it, briefly summarize what you added.";
const MAX_TRANSCRIPT_MESSAGES: usize = 100;
const LINE_SCROLL_STEP: u16 = 1;
const PAGE_SCROLL_STEP: u16 = 8;

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
    pub args: Value,
    pub risk: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct EventLine {
    pub name: String,
    pub detail: String,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct AttachmentDraft {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct PathEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub supported: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsPicker {
    Model,
    Thinking,
}

#[derive(Clone, Debug)]
struct QueuedPrompt {
    message: String,
    display_message: Option<String>,
    requested_tool: Option<String>,
    attachments: Vec<AttachmentDraft>,
}

#[derive(Clone, Debug)]
struct PastedRange {
    start: usize,
    end: usize,
}

#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    Submit {
        message: String,
        requested_tool: Option<String>,
        attachment_paths: Vec<PathBuf>,
    },
    Abort,
    NewSession,
    SetCwd(PathBuf),
    SetExecutionMode(String),
    SetModel {
        provider: String,
        model: String,
    },
    RefreshThinking,
    SetThinkingLevel(String),
    SwitchSession {
        id: String,
        exit_on_failure: bool,
    },
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
    session_picker_exit_on_cancel: bool,
    pub view: View,
    pub scroll: u16,
    pub model: String,
    pub cwd: String,
    pub launch_workspace: PathBuf,
    pub execution_mode: String,
    pub thinking_level: String,
    pub thinking_availability: ThinkingAvailability,
    pub thinking_message: String,
    pub context_percent: Option<f64>,
    pub status: String,
    pub status_error: bool,
    pub status_frame: u64,
    pub approval: Option<Approval>,
    pub events: Vec<EventLine>,
    pub attachments: Vec<AttachmentDraft>,
    pub path_picker: bool,
    pub path_input: Vec<char>,
    pub attachment_selected: usize,
    pub path_directory: PathBuf,
    pub path_entries: Vec<PathEntry>,
    pub path_selected: usize,
    pub model_options: Vec<ModelOption>,
    pub thinking_options: Vec<String>,
    pub settings_picker: Option<SettingsPicker>,
    pub settings_selected: usize,
    queued_prompts: VecDeque<QueuedPrompt>,
    pasted_ranges: Vec<PastedRange>,
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
        let path_directory = PathBuf::from(&session.cwd);
        let mut messages = messages;
        retain_latest_messages(&mut messages);
        Self {
            model: session.model.clone(),
            cwd: session.cwd.clone(),
            launch_workspace: path_directory.clone(),
            execution_mode: session.execution_mode.clone(),
            thinking_level: session.thinking_level.clone(),
            thinking_availability: ThinkingAvailability::Loading,
            thinking_message: String::new(),
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
            session_picker_exit_on_cancel: false,
            view: View::Chat,
            scroll: 0,
            status: String::new(),
            status_error: false,
            status_frame: 0,
            approval: None,
            events: Vec::new(),
            attachments: Vec::new(),
            path_picker: false,
            path_input: Vec::new(),
            attachment_selected: 0,
            path_directory,
            path_entries: Vec::new(),
            path_selected: 0,
            model_options: Vec::new(),
            thinking_options: Vec::new(),
            settings_picker: None,
            settings_selected: 0,
            queued_prompts: VecDeque::new(),
            pasted_ranges: Vec::new(),
            slash_usage: load_slash_usage(),
            slash_usage_path: slash_usage_path(),
            pending_slash_command: None,
        }
    }

    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    pub fn composer_input(&self) -> (Vec<char>, usize) {
        if self.pasted_ranges.is_empty() {
            return (self.input.clone(), self.input_cursor);
        }
        let mut display = Vec::new();
        let mut display_cursor = None;
        let mut source = 0;
        for range in self
            .pasted_ranges
            .iter()
            .filter(|range| range.start <= range.end && range.end <= self.input.len())
        {
            while source < range.start {
                if source == self.input_cursor {
                    display_cursor = Some(display.len());
                }
                display.push(self.input[source]);
                source += 1;
            }
            if self.input_cursor == range.start {
                display_cursor = Some(display.len());
            }
            let pasted = &self.input[range.start..range.end];
            let lines = pasted
                .iter()
                .filter(|character| **character == '\n')
                .count()
                + 1;
            let label = if lines > 1 {
                format!("[Pasted text · {lines} lines]")
            } else {
                format!("[Pasted text · {} chars]", pasted.len())
            };
            display.extend(label.chars());
            if self.input_cursor > range.start && self.input_cursor <= range.end {
                display_cursor = Some(display.len());
            }
            source = range.end;
        }
        while source < self.input.len() {
            if source == self.input_cursor {
                display_cursor = Some(display.len());
            }
            display.push(self.input[source]);
            source += 1;
        }
        if self.input_cursor == self.input.len() {
            display_cursor = Some(display.len());
        }
        let cursor = display_cursor.unwrap_or(display.len());
        (display, cursor)
    }

    pub fn accepts_composer_input(&self) -> bool {
        self.approval.is_none()
            && !self.path_picker
            && !self.session_picker
            && self.settings_picker.is_none()
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
        self.status_frame = self.status_frame.wrapping_add(1);
    }

    pub fn advance_status_animation(&mut self) {
        self.status_frame = self.status_frame.wrapping_add(5);
    }

    pub fn slash_open(&self) -> bool {
        let input = self.input_text();
        input.starts_with('/') && !input.chars().any(char::is_whitespace)
    }

    pub fn path_input_text(&self) -> String {
        self.path_input.iter().collect()
    }

    pub fn queued_count(&self) -> usize {
        self.queued_prompts.len()
    }

    pub fn set_startup_data(
        &mut self,
        default_model: String,
        thinking_level: String,
        model_options: Vec<ModelOption>,
        tools: Vec<ToolDefinition>,
        skills: Vec<SkillDefinition>,
    ) {
        if self.is_draft_session() {
            if self.model.is_empty() {
                self.model.clone_from(&default_model);
                self.session.model = default_model;
            }
            if self.thinking_level.is_empty() {
                self.thinking_level.clone_from(&thinking_level);
                self.session.thinking_level = thinking_level;
            }
        }
        self.set_model_options(model_options);
        self.tools = tools;
        self.skills = skills;
    }

    pub fn is_draft_session(&self) -> bool {
        self.session.id.is_empty()
    }

    pub fn set_model_options(&mut self, mut options: Vec<ModelOption>) {
        options.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        self.model_options = options;
    }

    pub fn visible_path_entries(&self) -> Vec<&PathEntry> {
        let query = self.path_input_text().to_lowercase();
        self.path_entries
            .iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .collect()
    }

    pub fn thinking_levels(&self) -> &[String] {
        &self.thinking_options
    }

    pub fn set_launch_workspace(&mut self, workspace: PathBuf) {
        self.launch_workspace = workspace;
    }

    pub fn new_session_workspace(&self) -> &Path {
        &self.launch_workspace
    }

    pub fn open_session_picker(&mut self, exit_on_cancel: bool) {
        let active_id = self.session.id.clone();
        self.open_session_picker_at(exit_on_cancel, &active_id);
    }

    pub fn open_session_picker_at(&mut self, exit_on_cancel: bool, selected_id: &str) {
        self.session_selected = self
            .sessions
            .iter()
            .position(|session| session.id == selected_id)
            .unwrap_or(0);
        self.session_picker_exit_on_cancel = exit_on_cancel;
        self.session_picker = true;
    }

    pub fn begin_thinking_load(&mut self) {
        self.thinking_options.clear();
        self.thinking_message.clear();
        self.thinking_availability = ThinkingAvailability::Loading;
        self.status = "loading thinking levels".to_owned();
        self.status_error = false;
    }

    pub fn set_thinking_state(&mut self, updated: ThinkingLevelUpdate) {
        let was_loading = self.thinking_availability == ThinkingAvailability::Loading
            && self.status == "loading thinking levels";
        self.thinking_options = updated.available_levels;
        self.thinking_message = updated.message;
        self.thinking_availability = match updated.status.as_str() {
            "unsupported" => ThinkingAvailability::Unsupported,
            "supported" => ThinkingAvailability::Supported,
            _ if self.thinking_options.is_empty() => ThinkingAvailability::Unsupported,
            _ => ThinkingAvailability::Supported,
        };
        if !updated.thinking_level.is_empty() {
            self.thinking_level = updated.thinking_level;
            self.session.thinking_level.clone_from(&self.thinking_level);
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.id == self.session.id)
            {
                session.thinking_level.clone_from(&self.thinking_level);
            }
        }
        if was_loading {
            self.status = match &self.thinking_availability {
                ThinkingAvailability::Unsupported => {
                    "thinking levels unavailable for this model".to_owned()
                }
                _ => format!("thinking · {}", self.thinking_level),
            };
        }
        self.status_error = false;
    }

    pub fn set_thinking_error(&mut self, error: String) {
        self.thinking_options.clear();
        self.thinking_message.clone_from(&error);
        self.thinking_availability = ThinkingAvailability::Error(error.clone());
        self.status = format!("thinking levels unavailable · {error}");
        self.status_error = true;
    }

    pub fn record_event(&mut self, name: &str, detail: String, state: &str) {
        self.events.push(EventLine {
            name: name.to_owned(),
            detail,
            state: state.to_owned(),
        });
        if self.events.len() > 200 {
            self.events.drain(..self.events.len() - 200);
        }
    }

    pub fn open_thinking_picker(&mut self) {
        self.open_settings_picker(SettingsPicker::Thinking);
    }

    pub fn open_path_picker(&mut self) {
        self.path_picker = true;
        self.path_input.clear();
        self.path_directory = PathBuf::from(&self.cwd);
        self.refresh_path_entries();
    }

    fn refresh_path_entries(&mut self) {
        self.path_entries = fs::read_dir(&self.path_directory)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| {
                        let metadata = entry.metadata().ok()?;
                        let path = entry.path();
                        let is_dir = metadata.is_dir();
                        let supported = is_dir || attachment_kind(&path).is_some();
                        Some(PathEntry {
                            name: entry.file_name().to_string_lossy().into_owned(),
                            path,
                            is_dir,
                            size: metadata.len(),
                            supported,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.path_entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        self.path_selected = 0;
    }

    fn open_settings_picker(&mut self, picker: SettingsPicker) {
        if self.is_streaming() {
            self.status = "Stop the active run before changing runtime settings".to_owned();
            self.status_error = true;
            return;
        }
        self.settings_picker = Some(picker);
        self.settings_selected = match picker {
            SettingsPicker::Model => self
                .model_options
                .iter()
                .position(|option| format!("{}/{}", option.provider, option.id) == self.model)
                .unwrap_or(0),
            SettingsPicker::Thinking => self
                .thinking_levels()
                .iter()
                .position(|level| level == &self.thinking_level)
                .unwrap_or(0),
        };
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
            command("/init", "Create or improve workspace AGENTS.md"),
            command("/new", "Start a new conversation"),
            command("/sessions", "Resume a conversation from any workspace"),
            command("/dir", "Change the active conversation directory"),
            command("/events", "Open the event ledger"),
            command("/chat", "Return to the conversation"),
            command("/model", "Switch the active session model"),
            command("/thinking", "Switch the active session thinking level"),
            command("/attach", "Add image, text, code, or document files"),
            command("/mode read-only", "Allow low-risk analysis tools only"),
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
            let left_prefix = left
                .command
                .strip_prefix('/')
                .unwrap_or(&left.command)
                .starts_with(&query);
            let right_prefix = right
                .command
                .strip_prefix('/')
                .unwrap_or(&right.command)
                .starts_with(&query);
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
            right_prefix
                .cmp(&left_prefix)
                .then_with(|| right_usage.count.cmp(&left_usage.count))
                .then_with(|| right_usage.last_used.cmp(&left_usage.last_used))
                .then_with(|| left.command.cmp(&right.command))
        });
        items
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return if self.is_streaming() || self.approval.is_some() {
                self.approval = None;
                Action::Abort
            } else {
                Action::Quit
            };
        }
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.open_path_picker();
            return Action::None;
        }
        if self.path_picker {
            return self.handle_path_picker(key);
        }
        if self.settings_picker.is_some() {
            return self.handle_settings_picker(key);
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
                KeyCode::Tab => {
                    self.complete_slash();
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
                    if self.input.is_empty() && character == '+' {
                        self.open_path_picker();
                    } else {
                        self.insert_input_character(character);
                    }
                }
                Action::None
            }
            KeyCode::Backspace => {
                self.delete_input_before_cursor();
                Action::None
            }
            KeyCode::Delete => {
                self.delete_input_at_cursor();
                Action::None
            }
            KeyCode::Left => {
                self.move_input_cursor_left();
                Action::None
            }
            KeyCode::Right => {
                self.move_input_cursor_right();
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
            KeyCode::Up if self.view == View::Chat => {
                self.scroll = self.scroll.saturating_add(LINE_SCROLL_STEP);
                Action::None
            }
            KeyCode::Down if self.view == View::Chat => {
                self.scroll = self.scroll.saturating_sub(LINE_SCROLL_STEP);
                Action::None
            }
            KeyCode::PageUp if self.view == View::Chat => {
                self.scroll = self.scroll.saturating_add(PAGE_SCROLL_STEP);
                Action::None
            }
            KeyCode::PageDown if self.view == View::Chat => {
                self.scroll = self.scroll.saturating_sub(PAGE_SCROLL_STEP);
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

    fn handle_path_picker(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.path_picker = false;
                self.path_input.clear();
            }
            KeyCode::Enter | KeyCode::Right => {
                let typed = self.path_input_text();
                let typed_path = (!typed.trim().is_empty()).then(|| {
                    let path = PathBuf::from(typed.trim().trim_matches(['"', '\'']));
                    if path.is_absolute() {
                        path
                    } else {
                        self.path_directory.join(path)
                    }
                });
                let selected = self
                    .visible_path_entries()
                    .get(self.path_selected)
                    .map(|entry| (*entry).clone());
                let target = typed_path.filter(|path| path.exists()).or_else(|| {
                    selected
                        .as_ref()
                        .filter(|entry| entry.supported)
                        .map(|entry| entry.path.clone())
                });
                if let Some(target) = target {
                    if target.is_dir() {
                        self.navigate_path_directory(&target);
                    } else {
                        self.add_attachment_path(&target);
                    }
                } else if selected.is_some_and(|entry| !entry.supported) {
                    self.status = "Unsupported attachment type".to_owned();
                    self.status_error = true;
                }
            }
            KeyCode::Backspace if self.path_input.is_empty() => {
                self.navigate_path_parent();
            }
            KeyCode::Backspace => {
                self.path_input.pop();
                self.path_selected = 0;
            }
            KeyCode::Left => self.navigate_path_parent(),
            KeyCode::Delete => {
                if !self.attachments.is_empty() {
                    self.attachments.remove(self.attachment_selected);
                    self.attachment_selected = self
                        .attachment_selected
                        .min(self.attachments.len().saturating_sub(1));
                }
            }
            KeyCode::Up => {
                self.path_selected = self.path_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let max = self.visible_path_entries().len().saturating_sub(1);
                self.path_selected = (self.path_selected + 1).min(max);
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.path_input.push(character);
                self.path_selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    fn navigate_path_parent(&mut self) {
        let workspace = PathBuf::from(&self.cwd)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&self.cwd));
        let Some(parent) = self.path_directory.parent().map(Path::to_path_buf) else {
            return;
        };
        if parent.starts_with(workspace) {
            self.navigate_path_directory(&parent);
        }
    }

    fn navigate_path_directory(&mut self, directory: &Path) {
        let workspace = PathBuf::from(&self.cwd)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&self.cwd));
        let directory = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        if directory.starts_with(workspace) && directory.is_dir() {
            self.path_directory = directory;
            self.path_input.clear();
            self.refresh_path_entries();
        }
    }

    fn add_attachment_path(&mut self, path: &Path) {
        match attachment_draft(&self.cwd, &path.to_string_lossy()) {
            Ok(attachment) => {
                if self.attachments.len() >= 8 {
                    self.status = "Attachment limit reached · 8 files".to_owned();
                    self.status_error = true;
                } else if self
                    .attachments
                    .iter()
                    .any(|item| item.path == attachment.path)
                {
                    self.status = "Attachment already added".to_owned();
                    self.status_error = true;
                } else if self
                    .attachments
                    .iter()
                    .map(|item| item.size)
                    .sum::<u64>()
                    .saturating_add(attachment.size)
                    > 20 * 1024 * 1024
                {
                    self.status = "Total attachment size cannot exceed 20 MiB".to_owned();
                    self.status_error = true;
                } else {
                    self.attachments.push(attachment);
                    self.attachment_selected = self.attachments.len().saturating_sub(1);
                    self.path_input.clear();
                    self.path_selected = 0;
                    self.status = format!("{} attachment(s) ready", self.attachments.len());
                    self.status_error = false;
                }
            }
            Err(error) => {
                self.status = error;
                self.status_error = true;
            }
        }
    }

    fn handle_settings_picker(&mut self, key: KeyEvent) -> Action {
        let Some(picker) = self.settings_picker else {
            return Action::None;
        };
        let count = match picker {
            SettingsPicker::Model => self.model_options.len(),
            SettingsPicker::Thinking => self.thinking_levels().len(),
        };
        match key.code {
            KeyCode::Esc => {
                self.settings_picker = None;
                Action::None
            }
            KeyCode::Up => {
                self.settings_selected = self.settings_selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.settings_selected = (self.settings_selected + 1).min(count.saturating_sub(1));
                Action::None
            }
            KeyCode::Enter => {
                self.settings_picker = None;
                match picker {
                    SettingsPicker::Model => self
                        .model_options
                        .get(self.settings_selected)
                        .map(|model| Action::SetModel {
                            provider: model.provider.clone(),
                            model: model.id.clone(),
                        })
                        .unwrap_or(Action::None),
                    SettingsPicker::Thinking => self
                        .thinking_levels()
                        .get(self.settings_selected)
                        .map(|level| Action::SetThinkingLevel(level.clone()))
                        .unwrap_or(Action::None),
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') if picker == SettingsPicker::Thinking => {
                Action::RefreshThinking
            }
            _ => Action::None,
        }
    }

    fn handle_session_picker(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.session_picker = false;
                if std::mem::take(&mut self.session_picker_exit_on_cancel) {
                    Action::Quit
                } else {
                    Action::None
                }
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
                let Some(session) = self.sessions.get(self.session_selected) else {
                    return Action::None;
                };
                let id = session.id.clone();
                let exit_on_failure = self.session_picker_exit_on_cancel;
                self.session_picker = false;
                Action::SwitchSession {
                    id,
                    exit_on_failure,
                }
            }
            _ => Action::None,
        }
    }

    fn complete_slash(&mut self) {
        let items = self.slash_items();
        let Some(item) = items.get(self.slash_selected).cloned() else {
            return;
        };
        let completed = if item.kind == SlashKind::Command && item.command != "/dir" {
            item.command
        } else {
            format!("{} ", item.command)
        };
        self.set_input(&completed);
    }

    fn choose_slash(&mut self) -> Action {
        let items = self.slash_items();
        let Some(item) = items.get(self.slash_selected).cloned() else {
            return Action::None;
        };
        if item.kind == SlashKind::Command && item.command != "/dir" {
            self.set_input(&item.command);
            return self.submit_action();
        }
        self.set_input(&format!("{} ", item.command));
        Action::None
    }

    fn submit_action(&mut self) -> Action {
        let mut message = self.input_text().trim().to_owned();
        if message.is_empty() && self.attachments.is_empty() {
            return Action::None;
        }
        if message.is_empty() {
            message = "Please analyze these attachments.".to_owned();
        }
        self.status_error = false;
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
            "/init" => {
                if self.execution_mode == "read-only" {
                    self.status = "/init requires full-access mode to write AGENTS.md".to_owned();
                    self.status_error = true;
                    self.clear_input();
                    return Action::None;
                }
                let attachments = std::mem::take(&mut self.attachments);
                let prompt = QueuedPrompt {
                    message: INIT_PROMPT.to_owned(),
                    display_message: Some("/init".to_owned()),
                    requested_tool: None,
                    attachments,
                };
                self.clear_input();
                if self.is_streaming() {
                    self.queued_prompts.push_back(prompt);
                    self.status = format!("{} message(s) queued", self.queued_prompts.len());
                    Action::None
                } else {
                    self.start_prompt(prompt)
                }
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
                self.open_session_picker(false);
                Action::None
            }
            "/dir" => {
                self.status = format!("directory · {} · use /dir <path>", self.cwd);
                self.clear_input();
                Action::None
            }
            _ if message.starts_with("/dir ") => {
                if self.is_streaming() {
                    self.status = "Stop the active run before changing directory".to_owned();
                    return Action::None;
                }
                let value = message.trim_start_matches("/dir ").trim();
                if value.is_empty() {
                    self.status = "usage · /dir <path>".to_owned();
                    self.clear_input();
                    return Action::None;
                }
                let requested = PathBuf::from(value);
                let requested = if requested.is_absolute() {
                    requested
                } else {
                    Path::new(&self.cwd).join(requested)
                };
                self.mark_slash_use("/dir");
                self.clear_input();
                Action::SetCwd(requested)
            }
            "/model" => {
                self.mark_slash_use("/model");
                self.clear_input();
                self.open_settings_picker(SettingsPicker::Model);
                Action::None
            }
            "/thinking" => {
                self.mark_slash_use("/thinking");
                self.clear_input();
                Action::RefreshThinking
            }
            "/attach" => {
                self.mark_slash_use(&message);
                self.clear_input();
                self.open_path_picker();
                Action::None
            }
            "/mode" => {
                self.status = format!(
                    "mode · {} · use /mode read-only|full-access",
                    self.execution_mode
                );
                self.clear_input();
                Action::None
            }
            _ if execution_mode_command(&message).is_some() => {
                let mode = execution_mode_command(&message).unwrap_or_default();
                self.clear_input();
                Action::SetExecutionMode(mode.to_owned())
            }
            _ if message.starts_with("/mode ") => {
                self.status = "usage · /mode read-only|full-access".to_owned();
                self.clear_input();
                Action::None
            }
            _ if self.is_streaming() => {
                let requested_tool = self.requested_tool(&message);
                let attachments = std::mem::take(&mut self.attachments);
                self.queued_prompts.push_back(QueuedPrompt {
                    message,
                    display_message: None,
                    requested_tool,
                    attachments,
                });
                self.clear_input();
                self.status = format!("{} message(s) queued", self.queued_prompts.len());
                Action::None
            }
            _ => {
                let requested_tool = self.requested_tool(&message);
                let attachments = std::mem::take(&mut self.attachments);
                self.clear_input();
                self.start_prompt(QueuedPrompt {
                    message,
                    display_message: None,
                    requested_tool,
                    attachments,
                })
            }
        }
    }

    fn start_prompt(&mut self, prompt: QueuedPrompt) -> Action {
        let display_message = prompt
            .display_message
            .clone()
            .unwrap_or_else(|| prompt.message.clone());
        self.pending_slash_command = display_message
            .split_whitespace()
            .next()
            .filter(|value| value.starts_with('/'))
            .map(str::to_owned);
        self.commit_live();
        let message_attachments = prompt
            .attachments
            .iter()
            .map(|attachment| MessageAttachment {
                kind: attachment.kind.clone(),
                name: attachment.name.clone(),
                size: attachment.size,
                ..MessageAttachment::default()
            })
            .collect();
        self.push_transcript_message(ChatMessage {
            role: "user".to_owned(),
            text: display_message.clone(),
            run_activity: None,
            attachments: message_attachments,
        });
        self.live = Some(LiveTurn {
            streaming: true,
            ..LiveTurn::default()
        });
        self.record_event("YOU", display_message, "queued");
        self.status = "thinking".to_owned();
        self.scroll = 0;
        Action::Submit {
            message: prompt.message,
            requested_tool: prompt.requested_tool,
            attachment_paths: prompt
                .attachments
                .into_iter()
                .map(|attachment| attachment.path)
                .collect(),
        }
    }

    pub fn take_queued_action(&mut self) -> Option<Action> {
        (!self.is_streaming())
            .then(|| self.queued_prompts.pop_front())
            .flatten()
            .map(|prompt| self.start_prompt(prompt))
    }

    fn requested_tool(&self, message: &str) -> Option<String> {
        let command = message.split_whitespace().next()?.strip_prefix('/')?;
        self.tools
            .iter()
            .any(|tool| tool.id == command)
            .then(|| command.to_owned())
    }

    pub fn insert_paste(&mut self, value: &str) {
        self.insert_paste_inner(value, false);
    }

    pub fn insert_detected_paste(&mut self, value: &str) {
        self.insert_paste_inner(value, true);
    }

    fn insert_paste_inner(&mut self, value: &str, force_range: bool) {
        if self.path_picker {
            self.path_input.extend(value.trim().chars());
            return;
        }
        let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
        let characters = normalized.chars().collect::<Vec<_>>();
        if characters.is_empty() {
            return;
        }
        let start = self.input_cursor;
        self.shift_pasted_ranges_for_insert(start, characters.len());
        self.input.splice(start..start, characters.iter().copied());
        self.input_cursor += characters.len();
        if force_range || characters.len() >= 80 || characters.contains(&'\n') {
            self.pasted_ranges.push(PastedRange {
                start,
                end: self.input_cursor,
            });
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
        self.thinking_level = session.thinking_level.clone();
        self.thinking_options.clear();
        self.thinking_availability = ThinkingAvailability::Loading;
        self.thinking_message.clear();
        self.context_percent = context_usage.and_then(|usage| usage.percent);
        self.session = session;
        self.messages = messages;
        retain_latest_messages(&mut self.messages);
        self.live = None;
        self.attachments.clear();
        self.path_picker = false;
        self.path_input.clear();
        self.path_directory = PathBuf::from(&self.cwd);
        self.path_entries.clear();
        self.path_selected = 0;
        self.settings_picker = None;
        self.session_picker_exit_on_cancel = false;
        self.queued_prompts.clear();
        self.pending_slash_command = None;
        self.events.clear();
        self.status.clear();
        self.status_error = false;
        self.view = View::Chat;
        self.scroll = 0;
    }

    pub fn materialize_session(&mut self, session: SessionSummary) {
        self.model.clone_from(&session.model);
        self.cwd.clone_from(&session.cwd);
        self.execution_mode.clone_from(&session.execution_mode);
        self.thinking_level.clone_from(&session.thinking_level);
        self.path_directory = PathBuf::from(&session.cwd);
        self.session = session.clone();
        if let Some(existing) = self
            .sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            *existing = session;
        } else {
            self.sessions.insert(0, session);
        }
    }

    pub fn set_draft_model(&mut self, provider: String, model: String) {
        self.model = format!("{provider}/{model}");
        self.session.model.clone_from(&self.model);
        self.thinking_level.clear();
        self.session.thinking_level.clear();
        self.thinking_options.clear();
        self.thinking_availability = ThinkingAvailability::Loading;
        self.thinking_message.clear();
        self.status = format!("model selected · {}", self.model);
        self.status_error = false;
    }

    pub fn set_draft_thinking_level(&mut self, level: String) {
        self.thinking_level.clone_from(&level);
        self.session.thinking_level = level;
        self.status = format!("thinking selected · {}", self.thinking_level);
        self.status_error = false;
    }

    pub fn set_cwd(&mut self, updated: SessionCwdUpdate) {
        self.cwd.clone_from(&updated.cwd);
        self.session.cwd.clone_from(&updated.cwd);
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == self.session.id)
        {
            session.cwd.clone_from(&updated.cwd);
        }
        self.attachments.clear();
        self.path_picker = false;
        self.path_input.clear();
        self.path_directory = PathBuf::from(&updated.cwd);
        self.path_entries.clear();
        self.path_selected = 0;
        self.status = format!("directory changed · {}", updated.cwd);
        self.status_error = false;
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
        self.status = format!("mode changed · {mode}");
        self.status_error = false;
    }

    pub fn set_model(&mut self, updated: SessionModelUpdate) {
        self.model = updated.model;
        self.session.model.clone_from(&self.model);
        self.set_thinking_state(ThinkingLevelUpdate {
            thinking_level: updated.thinking_level,
            available_levels: updated.available_thinking_levels,
            status: updated.thinking_status,
            message: updated.thinking_message,
        });
        self.context_percent = updated.context_usage.and_then(|usage| usage.percent);
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == self.session.id)
        {
            session.model.clone_from(&self.model);
        }
        self.status = format!("model changed · {}", self.model);
        self.status_error = false;
    }

    pub fn set_thinking_level(&mut self, updated: ThinkingLevelUpdate) {
        self.set_thinking_state(updated);
        self.status = format!("thinking changed · {}", self.thinking_level);
        self.status_error = false;
    }

    pub fn set_plan(&mut self, plan: Option<Plan>) {
        self.session.plan = plan.clone();
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == self.session.id)
        {
            session.plan = plan;
        }
    }

    pub fn apply_stream_event(&mut self, event: StreamEvent) {
        self.record_event(
            &event.name.to_uppercase(),
            event_detail(&event),
            event_state(&event),
        );
        if let Some(plan) = plan_from_payload(&event.data) {
            self.set_plan(plan);
        }
        if is_plan_update_event(&event.name) {
            return;
        }
        let Some(live) = self.live.as_mut() else {
            return;
        };
        match event.name.as_str() {
            "meta" => {
                self.model = string_field(&event.data, "model");
                let event_cwd = string_field(&event.data, "cwd");
                if !event_cwd.is_empty() && same_workspace(&event_cwd, Path::new(&self.session.cwd))
                {
                    self.cwd = event_cwd;
                } else if !event_cwd.is_empty() {
                    self.status = format!(
                        "workspace mismatch · session {} · runtime {event_cwd}",
                        self.session.cwd
                    );
                    self.status_error = true;
                }
                self.execution_mode = string_field(&event.data, "executionMode");
                self.thinking_level = string_field(&event.data, "thinkingLevel");
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
                    started_at: event.data["startedAt"].as_u64().unwrap_or_default(),
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
                    args: event.data["args"].clone(),
                    risk: string_field(&event.data, "risk"),
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
                self.status_error = false;
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
                self.status_error = true;
            }
            _ => {}
        }
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
        self.status_error = true;
    }

    fn commit_live(&mut self) {
        let Some(mut live) = self.live.take() else {
            return;
        };
        sync_live_display(&mut live);
        if live.text.is_empty() && live.thinking.is_empty() && live.tools.is_empty() {
            return;
        }
        self.push_transcript_message(ChatMessage {
            role: "agent".to_owned(),
            text: live.text,
            run_activity: Some(RunActivity {
                thinking_text: live.thinking,
                tools: live.tools,
                ..RunActivity::default()
            }),
            attachments: Vec::new(),
        });
    }

    fn push_transcript_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        retain_latest_messages(&mut self.messages);
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

    fn insert_input_character(&mut self, character: char) {
        self.shift_pasted_ranges_for_insert(self.input_cursor, 1);
        self.input.insert(self.input_cursor, character);
        self.input_cursor += 1;
        self.slash_selected = 0;
    }

    fn delete_input_before_cursor(&mut self) {
        if let Some(index) = self
            .pasted_ranges
            .iter()
            .position(|range| range.end == self.input_cursor)
        {
            self.remove_pasted_range(index);
        } else if self.input_cursor > 0 {
            let removed = self.input_cursor - 1;
            self.input.remove(removed);
            self.input_cursor = removed;
            self.shift_pasted_ranges_after_removal(removed);
        }
        self.slash_selected = 0;
    }

    fn delete_input_at_cursor(&mut self) {
        if let Some(index) = self
            .pasted_ranges
            .iter()
            .position(|range| range.start == self.input_cursor)
        {
            self.remove_pasted_range(index);
        } else if self.input_cursor < self.input.len() {
            let removed = self.input_cursor;
            self.input.remove(removed);
            self.shift_pasted_ranges_after_removal(removed);
        }
        self.slash_selected = 0;
    }

    fn move_input_cursor_left(&mut self) {
        if let Some(range) = self
            .pasted_ranges
            .iter()
            .find(|range| range.start < self.input_cursor && self.input_cursor <= range.end)
        {
            self.input_cursor = range.start;
        } else {
            self.input_cursor = self.input_cursor.saturating_sub(1);
        }
    }

    fn move_input_cursor_right(&mut self) {
        if let Some(range) = self
            .pasted_ranges
            .iter()
            .find(|range| range.start <= self.input_cursor && self.input_cursor < range.end)
        {
            self.input_cursor = range.end;
        } else {
            self.input_cursor = (self.input_cursor + 1).min(self.input.len());
        }
    }

    fn shift_pasted_ranges_for_insert(&mut self, position: usize, amount: usize) {
        for range in &mut self.pasted_ranges {
            if range.start >= position {
                range.start += amount;
                range.end += amount;
            }
        }
    }

    fn shift_pasted_ranges_after_removal(&mut self, position: usize) {
        for range in &mut self.pasted_ranges {
            if position < range.start {
                range.start -= 1;
                range.end -= 1;
            }
        }
    }

    fn remove_pasted_range(&mut self, index: usize) {
        let range = self.pasted_ranges.remove(index);
        let removed = range.end.saturating_sub(range.start);
        self.input.drain(range.start..range.end);
        self.input_cursor = range.start;
        for following in &mut self.pasted_ranges {
            if following.start >= range.end {
                following.start -= removed;
                following.end -= removed;
            }
        }
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.pasted_ranges.clear();
        self.slash_selected = 0;
    }

    fn set_input(&mut self, value: &str) {
        self.input = value.chars().collect();
        self.input_cursor = self.input.len();
        self.pasted_ranges.clear();
        self.slash_selected = 0;
    }
}

fn attachment_draft(workspace: &str, raw_path: &str) -> Result<AttachmentDraft, String> {
    let cleaned = raw_path.trim().trim_matches(['"', '\'']);
    let file_uri_path = cleaned.strip_prefix("file://").unwrap_or(cleaned);
    let value = if cfg!(windows)
        && file_uri_path.starts_with('/')
        && file_uri_path.as_bytes().get(2) == Some(&b':')
    {
        &file_uri_path[1..]
    } else {
        file_uri_path
    };
    let path = Path::new(value)
        .canonicalize()
        .map_err(|error| format!("Cannot open attachment · {error}"))?;
    let workspace = Path::new(workspace)
        .canonicalize()
        .map_err(|error| format!("Cannot resolve workspace · {error}"))?;
    if !path.starts_with(&workspace) {
        return Err("Attachments must be inside the current workspace".to_owned());
    }
    let metadata = path
        .metadata()
        .map_err(|error| format!("Cannot inspect attachment · {error}"))?;
    if !metadata.is_file() {
        return Err("Attachment path must point to a file".to_owned());
    }
    if metadata.len() > 10 * 1024 * 1024 {
        return Err("Attachment cannot exceed 10 MiB".to_owned());
    }
    let kind = attachment_kind(&path).ok_or_else(|| {
        "Unsupported attachment type · use an image, text/code file, or document".to_owned()
    })?;
    Ok(AttachmentDraft {
        name: path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_owned()),
        path,
        kind: kind.to_owned(),
        size: metadata.len(),
    })
}

fn attachment_kind(path: &Path) -> Option<&'static str> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    ) {
        Some("image")
    } else if matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "json"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "css"
            | "html"
            | "xml"
            | "yaml"
            | "yml"
            | "csv"
            | "log"
            | "py"
            | "java"
            | "go"
            | "rs"
            | "sh"
            | "ps1"
            | "toml"
            | "sql"
    ) {
        Some("text")
    } else if matches!(
        extension.as_str(),
        "pdf" | "docx" | "pptx" | "xlsx" | "odt" | "odp" | "ods" | "rtf" | "epub"
    ) {
        Some("document")
    } else {
        None
    }
}

fn execution_mode_command(message: &str) -> Option<&str> {
    let mut parts = message.split_whitespace();
    if parts.next()? != "/mode" {
        return None;
    }
    let mode = parts.next()?;
    if parts.next().is_some() || !matches!(mode, "read-only" | "full-access") {
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

fn retain_latest_messages(messages: &mut Vec<ChatMessage>) {
    let excess = messages.len().saturating_sub(MAX_TRANSCRIPT_MESSAGES);
    if excess > 0 {
        messages.drain(..excess);
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
    if value["agent"].is_object() {
        tool.agent = value.get("agent").cloned();
    }
    if done {
        tool.finished_at = value["finishedAt"].as_u64().unwrap_or_default();
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
        _ if is_plan_update_event(&event.name) => "Plan updated".to_owned(),
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
        _ if is_plan_update_event(&event.name) => "done",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        advance_typewriter, apply_patch, attachment_draft, Action, App, Approval, LiveTurn,
        SettingsPicker, INIT_PROMPT, MAX_TRANSCRIPT_MESSAGES, PAGE_SCROLL_STEP,
    };
    use crate::model::{
        ChatMessage, ModelOption, SessionCwdUpdate, SessionSummary, StreamEvent,
        ThinkingAvailability, ThinkingLevelUpdate, ToolDefinition,
    };
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
        app.set_input("keep running");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        assert!(app.is_streaming());

        app.set_input("/mode full-access");
        assert!(matches!(
            app.submit_action(),
            Action::SetExecutionMode(mode) if mode == "full-access"
        ));

        app.set_execution_mode("full-access".to_owned());
        assert_eq!(app.execution_mode, "full-access");
        assert_eq!(app.session.execution_mode, "full-access");
        assert_eq!(app.status, "mode changed · full-access");
        assert!(!app.status_error);
    }

    #[test]
    fn approval_keys_resolve_the_visible_request() {
        for (key, approved) in [('y', true), ('Y', true), ('n', false), ('N', false)] {
            let mut app = test_app(Vec::new());
            app.approval = Some(Approval {
                id: "approval-1".to_owned(),
                tool_name: "bash".to_owned(),
                args: json!({ "command": "date" }),
                risk: "high".to_owned(),
                reason: "Runs as the current OS user.".to_owned(),
            });

            assert!(matches!(
                app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Action::ResolveApproval { id, approved: actual }
                    if id == "approval-1" && actual == approved
            ));
            assert!(app.approval.is_none());
        }
    }

    #[test]
    fn ctrl_c_aborts_streaming_and_pending_approval_before_it_quits() {
        let mut app = test_app(Vec::new());
        app.set_input("run date");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.approval = Some(Approval {
            id: "approval-1".to_owned(),
            tool_name: "bash".to_owned(),
            args: json!({ "command": "date" }),
            risk: "high".to_owned(),
            reason: "Runs as the current OS user.".to_owned(),
        });

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Abort
        ));
        assert!(app.approval.is_none());

        app.live = None;
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        ));
    }

    #[test]
    fn pasted_text_is_collapsed_for_display_but_submitted_in_full() {
        let mut app = test_app(Vec::new());
        let pasted = format!("first line\n{}", "payload ".repeat(30));
        app.insert_paste(&pasted);

        let (display, display_cursor) = app.composer_input();
        let display = display.iter().collect::<String>();
        assert_eq!(display, "[Pasted text · 2 lines]");
        assert!(!display.contains("payload payload"));
        assert_eq!(display_cursor, display.chars().count());
        assert_eq!(app.input_text(), pasted);

        assert!(matches!(
            app.submit_action(),
            Action::Submit { message, .. } if message == pasted.trim()
        ));
    }

    #[test]
    fn independent_pastes_remain_separate_blocks() {
        let mut app = test_app(Vec::new());
        app.insert_detected_paste("first");
        app.insert_detected_paste("second");

        assert_eq!(app.input_text(), "firstsecond");
        assert_eq!(
            app.composer_input().0.iter().collect::<String>(),
            "[Pasted text · 5 chars][Pasted text · 6 chars]"
        );
    }

    #[test]
    fn backspace_removes_a_collapsed_paste_as_one_block() {
        let mut app = test_app(Vec::new());
        app.insert_paste("first line\nsecond line\nthird line");
        assert_eq!(
            app.composer_input().0.iter().collect::<String>(),
            "[Pasted text · 3 lines]"
        );

        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert!(app.input_text().is_empty());
        assert!(app.composer_input().0.is_empty());
    }

    #[test]
    fn permission_events_keep_the_command_for_review() {
        let mut app = test_app(Vec::new());
        app.set_input("run date");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.apply_stream_event(StreamEvent {
            name: "permission_request".to_owned(),
            data: json!({
                "id": "approval-1",
                "toolName": "bash",
                "args": { "command": "date +%A" },
                "risk": "high",
                "reason": "Runs as the current OS user."
            }),
        });

        let approval = app.approval.as_ref().unwrap();
        assert_eq!(approval.args["command"], "date +%A");
        assert_eq!(approval.risk, "high");
    }

    #[test]
    fn init_runs_a_hidden_workspace_instruction_and_keeps_the_command_visible() {
        let mut app = test_app(Vec::new());
        assert!(app.slash_items().iter().any(|item| item.command == "/init"));
        app.set_input("/init");

        assert!(matches!(
            app.submit_action(),
            Action::Submit {
                message,
                requested_tool: None,
                attachment_paths,
            } if message == INIT_PROMPT && attachment_paths.is_empty()
        ));
        assert_eq!(app.messages.last().unwrap().text, "/init");
        assert_eq!(app.events.last().unwrap().detail, "/init");
        assert!(!app
            .messages
            .last()
            .unwrap()
            .text
            .contains("Analyze this codebase"));
    }

    #[test]
    fn init_is_rejected_in_read_only_mode() {
        let mut app = test_app(Vec::new());
        app.execution_mode = "read-only".to_owned();
        app.set_input("/init");

        assert!(matches!(app.submit_action(), Action::None));
        assert!(app.status_error);
        assert!(app.status.contains("requires full-access"));
        assert!(!app.is_streaming());
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

    #[test]
    fn tab_completes_the_highlighted_slash_tool_without_submitting() {
        let mut app = test_app(vec![ToolDefinition {
            id: "read".to_owned(),
            name: "Read".to_owned(),
            description: "Read a file".to_owned(),
            enabled: true,
        }]);
        app.set_input("/rea");

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Action::None
        ));
        assert_eq!(app.input_text(), "/read ");
        assert!(!app.is_streaming());
    }

    #[test]
    fn attachment_picker_enforces_workspace_and_submits_selected_files() {
        let workspace = std::env::temp_dir().join(format!(
            "pisper-tui-attachment-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let nested = workspace.join("docs");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("context.md");
        std::fs::write(&file, "# Context\nImportant details").unwrap();

        let draft = attachment_draft(workspace.to_str().unwrap(), file.to_str().unwrap()).unwrap();
        assert_eq!(draft.kind, "text");
        let mut app = test_app(Vec::new());
        app.cwd = workspace.to_string_lossy().into_owned();
        app.session.cwd.clone_from(&app.cwd);
        app.open_path_picker();
        assert_eq!(app.path_entries.len(), 1);
        assert!(app.path_entries[0].is_dir);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.path_directory, nested.canonicalize().unwrap());
        assert_eq!(app.path_entries[0].name, "context.md");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.attachments.len(), 1);

        app.path_picker = false;
        app.set_input("Analyze this context");
        assert!(matches!(
            app.submit_action(),
            Action::Submit { attachment_paths, .. }
                if attachment_paths == vec![file.canonicalize().unwrap()]
        ));
        assert!(app.attachments.is_empty());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn attachment_shortcuts_open_without_discarding_the_composer_draft() {
        let mut app = test_app(Vec::new());
        app.set_input("keep this draft");
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            Action::None
        ));
        assert!(app.path_picker);
        assert_eq!(app.input_text(), "keep this draft");
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        app.clear_input();
        app.handle_key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE));
        assert!(!app.path_picker);
        assert_eq!(app.input_text(), "@");
    }

    #[test]
    fn model_and_thinking_slash_commands_open_pickers_and_apply_selection() {
        let mut app = test_app(Vec::new());
        app.model = "provider/model-a".to_owned();
        app.set_model_options(vec![
            ModelOption {
                provider: "provider".to_owned(),
                id: "model-a".to_owned(),
                name: "Model A".to_owned(),
                reasoning: true,
            },
            ModelOption {
                provider: "provider".to_owned(),
                id: "model-b".to_owned(),
                name: "Model B".to_owned(),
                reasoning: true,
            },
        ]);
        app.set_input("/model");
        assert!(matches!(app.submit_action(), Action::None));
        assert_eq!(app.settings_picker, Some(SettingsPicker::Model));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SetModel { provider, model }
                if provider == "provider" && model == "model-b"
        ));

        app.set_input("/thinking");
        assert!(matches!(app.submit_action(), Action::RefreshThinking));
        assert_eq!(app.settings_picker, None);
        app.set_thinking_state(ThinkingLevelUpdate {
            thinking_level: "off".to_owned(),
            available_levels: vec!["off".to_owned(), "xhigh".to_owned(), "max".to_owned()],
            status: "supported".to_owned(),
            message: String::new(),
        });
        app.open_thinking_picker();
        app.settings_selected = 1;
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SetThinkingLevel(level) if level == "xhigh"
        ));
    }

    #[test]
    fn thinking_empty_and_error_states_never_restore_hard_coded_levels() {
        let mut app = test_app(Vec::new());
        assert!(app.thinking_levels().is_empty());
        assert_eq!(app.thinking_availability, ThinkingAvailability::Loading);

        app.set_thinking_state(ThinkingLevelUpdate {
            thinking_level: "off".to_owned(),
            available_levels: Vec::new(),
            status: "unsupported".to_owned(),
            message: "Fixed reasoning".to_owned(),
        });
        assert!(app.thinking_levels().is_empty());
        assert_eq!(app.thinking_availability, ThinkingAvailability::Unsupported);

        app.set_thinking_error("sidecar unavailable".to_owned());
        app.open_thinking_picker();
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            Action::RefreshThinking
        ));
    }

    #[test]
    fn global_resume_and_explicit_dir_changes_preserve_the_launch_workspace() {
        let root = std::env::temp_dir().join(format!(
            "pisper-tui-workspaces-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let launch = root.join("launch");
        let other = root.join("other");
        std::fs::create_dir_all(&launch).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let launch = launch.canonicalize().unwrap();
        let other = other.canonicalize().unwrap();

        let mut app = test_app(Vec::new());
        app.set_launch_workspace(launch.clone());
        app.sessions.push(SessionSummary {
            id: "other-session".to_owned(),
            cwd: other.to_string_lossy().into_owned(),
            ..SessionSummary::default()
        });
        app.open_session_picker(false);
        app.session_selected = 1;

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SwitchSession { id, .. } if id == "other-session"
        ));
        assert_eq!(app.new_session_workspace(), launch);

        app.set_input(&format!("/dir {}", other.display()));
        assert!(matches!(
            app.submit_action(),
            Action::SetCwd(path) if path == other
        ));
        app.set_cwd(SessionCwdUpdate {
            cwd: other.to_string_lossy().into_owned(),
        });
        assert_eq!(app.cwd, other.to_string_lossy());
        assert_eq!(app.new_session_workspace(), launch);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_resume_picker_exits_on_cancel() {
        let mut app = test_app(Vec::new());
        app.open_session_picker(true);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Quit
        ));
    }

    #[test]
    fn plan_events_update_items_in_place_and_clear_legacy_state() {
        let mut app = test_app(Vec::new());
        app.set_input("implement the plan");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));

        app.apply_stream_event(StreamEvent {
            name: "meta".to_owned(),
            data: json!({
                "model": "provider/model",
                "cwd": "/workspace",
                "plan": {
                    "items": [{ "id": "one", "title": "Inspect", "status": "pending", "note": "Read files", "assignee": "agent", "dependsOn": [] }],
                    "counts": { "pending": 1, "inProgress": 0, "completed": 0, "blocked": 0, "total": 1 }
                }
            }),
        });
        assert_eq!(app.session.plan.as_ref().unwrap().items[0].title, "Inspect");

        app.apply_stream_event(StreamEvent {
            name: "plan_update".to_owned(),
            data: json!({
                "plan": {
                    "items": [{ "id": "one", "title": "Inspect", "status": "completed", "note": "Verified", "assignee": "agent", "dependsOn": [] }],
                    "counts": { "pending": 0, "inProgress": 0, "completed": 1, "blocked": 0, "total": 1 }
                }
            }),
        });
        let plan = app.session.plan.as_ref().unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].status, "completed");

        app.apply_stream_event(StreamEvent {
            name: "task_list_update".to_owned(),
            data: json!({ "taskList": null }),
        });
        assert!(app.session.plan.is_none());
    }

    #[test]
    fn tool_events_preserve_timestamps_and_subagent_results() {
        let mut app = test_app(Vec::new());
        app.set_input("inspect");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.apply_stream_event(StreamEvent {
            name: "tool_start".to_owned(),
            data: json!({ "id": "tool-1", "name": "read", "args": {}, "startedAt": 1000 }),
        });
        app.apply_stream_event(StreamEvent {
            name: "tool_end".to_owned(),
            data: json!({
                "id": "tool-1",
                "name": "read",
                "finishedAt": 1512,
                "agent": { "canonicalName": "log-analysis", "status": "completed" }
            }),
        });
        let tool = &app.live.as_ref().unwrap().tools[0];
        assert_eq!(tool.started_at, 1000);
        assert_eq!(tool.finished_at, 1512);
        assert_eq!(
            tool.agent.as_ref().unwrap()["canonicalName"],
            "log-analysis"
        );
    }

    #[test]
    fn messages_submitted_during_a_run_are_sent_fifo_after_completion() {
        let mut app = test_app(Vec::new());
        app.set_input("first");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.set_input("second");
        assert!(matches!(app.submit_action(), Action::None));
        assert_eq!(app.queued_count(), 1);

        app.apply_stream_event(StreamEvent {
            name: "done".to_owned(),
            data: json!({ "text": "first answer", "tools": [], "contextUsage": {} }),
        });
        assert!(matches!(
            app.take_queued_action(),
            Some(Action::Submit { message, .. }) if message == "second"
        ));
        assert_eq!(app.queued_count(), 0);
        assert_eq!(
            app.messages
                .iter()
                .filter(|message| message.role == "user")
                .count(),
            2
        );
    }

    #[test]
    fn chat_arrow_and_page_keys_scroll_without_losing_position_to_stream_updates() {
        let mut app = test_app(Vec::new());

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.scroll, 1);
        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.scroll, 1 + PAGE_SCROLL_STEP);

        app.live = Some(LiveTurn {
            streaming: true,
            ..LiveTurn::default()
        });
        app.apply_stream_event(StreamEvent {
            name: "text_delta".to_owned(),
            data: json!({ "delta": "new output" }),
        });
        assert_eq!(app.scroll, 1 + PAGE_SCROLL_STEP);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn transcript_retains_only_the_latest_message_page() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: "/workspace".to_owned(),
            ..SessionSummary::default()
        };
        let messages = (0..MAX_TRANSCRIPT_MESSAGES + 25)
            .map(|index| ChatMessage {
                role: "agent".to_owned(),
                text: format!("message-{index}"),
                ..ChatMessage::default()
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

        assert_eq!(app.messages.len(), MAX_TRANSCRIPT_MESSAGES);
        assert_eq!(app.messages.first().unwrap().text, "message-25");
        app.set_input("next message");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        assert_eq!(app.messages.len(), MAX_TRANSCRIPT_MESSAGES);
        assert_eq!(app.messages.last().unwrap().text, "next message");
    }

    #[test]
    fn tab_completes_a_builtin_command_without_executing_it() {
        let mut app = test_app(Vec::new());
        app.set_input("/quit");

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Action::None
        ));
        assert_eq!(app.input_text(), "/quit");
        assert!(app.input_cursor > 0);
    }

    #[test]
    fn draft_defaults_and_materialization_preserve_the_first_pending_turn() {
        let draft = SessionSummary {
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(Vec::new(), draft, Vec::new(), None, Vec::new(), Vec::new());

        assert!(app.is_draft_session());
        app.set_startup_data(
            "provider/model".to_owned(),
            "high".to_owned(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(app.model, "provider/model");
        assert_eq!(app.thinking_level, "high");

        app.set_input("first message");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        assert!(app.live.is_some());
        assert_eq!(app.messages.last().unwrap().text, "first message");

        app.materialize_session(SessionSummary {
            id: "session-1".to_owned(),
            model: "provider/model".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            thinking_level: "high".to_owned(),
            ..SessionSummary::default()
        });
        assert!(!app.is_draft_session());
        assert!(app.live.is_some());
        assert_eq!(app.messages.last().unwrap().text, "first message");
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[0].id, "session-1");
    }

    fn test_app(tools: Vec<ToolDefinition>) -> App {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
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
