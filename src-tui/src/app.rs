//! TUI 应用状态与交互逻辑（纯状态层，不直接触碰终端）。
//!
//! `App` 持有会话/消息/输入/弹窗等全部 UI 状态；按键输入经 `handle_key`
//! 转换为 `Action`（副作用动作），由事件循环异步执行后再把结果写回 `App`。
//! 流事件经 `apply_stream_event` 直接更新状态。

use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroize;

use crate::{
    api::MESSAGE_PAGE_LIMIT,
    model::{
        ChatMessage, ContextUsage, MessageAttachment, MessagePage, ModelOption, Plan,
        ProviderOption, RunActivity, SessionCwdUpdate, SessionModelUpdate, SessionSummary,
        SessionUsage, SkillDefinition, StreamEvent, ThinkingAvailability, ThinkingLevelUpdate,
        ToolActivity, ToolDefinition, VcsChanges, PROVIDER_APIS,
    },
    plan_protocol::{active_plan, is_plan_update_event, plan_from_payload},
    workspace::same_workspace,
};

// `/init` 的完整指令：让 Agent 分析仓库并创建/改进 AGENTS.md。
// 真实提示语对用户隐藏，只显示 `/init` 命令本身。
const INIT_PROMPT: &str = "/init\n\n---\nAttachment context (injected by Pisper):\nAnalyze this codebase and create or improve `AGENTS.md` in the current workspace root. The file is long-lived guidance for Pisper and other coding agents working in this repository. Inspect the repository before writing it. Capture only project-specific, durable information: the project purpose, important directories and architecture, build/test/lint/typecheck commands, coding conventions, and verification expectations. Keep it concise and practical. Do not include generic advice, temporary task details, secrets, exhaustive file listings, or information you cannot verify. If `AGENTS.md` already exists, preserve accurate useful instructions and update it carefully instead of replacing it blindly. Modify only `AGENTS.md`. After writing it, briefly summarize what you added.";
// 内存中最多保留的消息数（约 4 页），防止长时间会话内存无限增长。
const MAX_LOADED_MESSAGES: usize = MESSAGE_PAGE_LIMIT * 4;
// 空闲回收后保留的消息数（约 2 页）。
const HISTORY_KEEP_MESSAGES: usize = MESSAGE_PAGE_LIMIT * 2;
// 历史消息空闲多久后回收（用户停止浏览更早消息时释放内存）。
const HISTORY_IDLE_EVICT_DELAY: Duration = Duration::from_secs(90);
// 接近顶部多少行时触发加载更早历史。
const HISTORY_SCROLL_MARGIN: u16 = 8;

/// 当前计划项的索引：优先进行中，其次阻塞，再次待办；
/// 全部完成时指向最后一项（面板随即收起）。
fn current_plan_item_index(plan: &Plan) -> usize {
    plan.items
        .iter()
        .position(|item| item.status == "in_progress")
        .or_else(|| plan.items.iter().position(|item| item.status == "blocked"))
        .or_else(|| plan.items.iter().position(|item| item.status == "pending"))
        .unwrap_or_else(|| plan.items.len().saturating_sub(1))
}
// 单行滚动步长。
const LINE_SCROLL_STEP: u16 = 1;

/// 读取布尔环境变量开关（`PISPER_TUI_*`）：只有 1/true/yes/on 视为开启。
fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}
// 整页滚动步长。
const PAGE_SCROLL_STEP: u16 = 8;

/// 主视图：对话 / 工作区变更。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum View {
    #[default]
    Chat,
    Changes,
}

/// Slash 项类型：应用工具 / 技能 / 内置命令。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlashKind {
    Tool,
    Skill,
    Command,
}

/// Slash 目录筛选分类。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SlashCategory {
    #[default]
    All,
    Tools,
    Skills,
    Commands,
}

impl SlashCategory {
    /// 当前分类是否包含指定类型的项。
    fn includes(self, kind: SlashKind) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, kind),
                (Self::Tools, SlashKind::Tool)
                    | (Self::Skills, SlashKind::Skill)
                    | (Self::Commands, SlashKind::Command)
            )
    }

    /// 循环到下一个分类（右箭头）。
    fn next(self) -> Self {
        match self {
            Self::All => Self::Tools,
            Self::Tools => Self::Skills,
            Self::Skills => Self::Commands,
            Self::Commands => Self::All,
        }
    }

    /// 循环到上一个分类（左箭头）。
    fn previous(self) -> Self {
        match self {
            Self::All => Self::Commands,
            Self::Tools => Self::All,
            Self::Skills => Self::Tools,
            Self::Commands => Self::Skills,
        }
    }
}

/// Slash 目录中的一条建议项。
#[derive(Clone, Debug)]
pub struct SlashItem {
    pub kind: SlashKind,
    /// 输入框中被插入/提交的命令文本。
    pub command: String,
    /// 展示用描述。
    pub detail: String,
}

/// 正在流式输出的一轮 Agent 运行。
/// 关键设计：`*_target` 是流的真实目标文本，`*` 是已显示的文本；
/// 打字机动画逐帧把 `*` 推向 `*_target`。
#[derive(Clone, Debug, Default)]
pub struct LiveTurn {
    pub thinking: String,
    pub thinking_target: String,
    pub text: String,
    pub text_target: String,
    pub tools: Vec<ToolActivity>,
    pub streaming: bool,
}

/// 待用户确认的权限请求（工具审批）。
#[derive(Clone, Debug)]
pub struct Approval {
    pub id: String,
    pub tool_name: String,
    pub args: Value,
    pub risk: String,
    pub reason: String,
}

/// 附件草稿：发送前暂存在输入侧，提交时转成真实路径列表。
#[derive(Clone, Debug)]
pub struct AttachmentDraft {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub size: u64,
}

/// 附件选择器中的目录项。
#[derive(Clone, Debug)]
pub struct PathEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// 是否支持作为附件（目录总是支持进入）。
    pub supported: bool,
}

/// 设置弹窗类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsPicker {
    Model,
    Thinking,
}

/// 排队中的提示（运行期间提交的后续输入）。
#[derive(Clone, Debug)]
struct QueuedPrompt {
    message: String,
    /// 显示用消息（如 `/init` 显示命令名而非真实提示语）。
    display_message: Option<String>,
    requested_tool: Option<String>,
    attachments: Vec<AttachmentDraft>,
}

/// 粘贴折叠块的源区间（`[start, end)`，对应 `input` 字符下标）。
/// 折叠显示时整块粘贴显示为一行占位文本。
#[derive(Clone, Debug)]
struct PastedRange {
    start: usize,
    end: usize,
}

/// 按键产生的副作用动作；由事件循环异步执行。
/// 设计上 App 不直接调用 API，而是产出动作让主循环统一调度。
pub enum Action {
    /// 无操作。
    None,
    /// 退出程序。
    Quit,
    /// 提交一条新消息（可附带请求工具与附件路径）。
    Submit {
        message: String,
        requested_tool: Option<String>,
        attachment_paths: Vec<PathBuf>,
    },
    /// 向运行中的会话排队追加输入。
    QueueInput { message: String },
    /// 中止当前运行。
    Abort,
    /// 新建会话（回到草稿状态）。
    NewSession,
    /// 切换会话工作区。
    SetCwd(PathBuf),
    /// 切换执行模式。
    SetExecutionMode(String),
    /// 切换模型。
    SetModel { provider: String, model: String },
    /// 刷新思考级别选项。
    RefreshThinking,
    /// 压缩上下文。
    Compact,
    /// 加载更早的历史消息。
    LoadOlderMessages { before: u64 },
    /// 设置思考级别。
    SetThinkingLevel(String),
    /// 切换到指定会话。
    SwitchSession { id: String, request_id: u64 },
    /// 解析一个审批请求。
    ResolveApproval { id: String, approved: bool },
    /// 刷新工作区变更。
    RefreshVcs,
    /// 提交工作区变更（带提交信息）。
    CommitVcs(String),
    /// 推送工作区变更。
    PushVcs,
    /// 回退工作区变更。
    RevertVcs,
    /// 保存 Provider 连接（协议/Base URL/API Key）。
    SaveProviderConnection {
        provider: String,
        api: String,
        base_url: String,
        api_key: String,
    },
    /// 在浏览器打开 Web 设置。
    OpenWeb,
}

/// Slash 命令使用统计（持久化到 `~/.pisper/tui-slash-usage.json`，
/// 用于目录排序：更常用的命令排前面）。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SlashUsage {
    count: u64,
    last_used: u64,
}

/// TUI 的全部 UI 状态。
/// 字段可分几组：会话/消息（sessions/session/messages/live）、
/// 输入（input/input_cursor/pasted_ranges）、弹窗（session_picker/path_picker/…）、
/// 会话元信息（model/cwd/execution_mode/…）、以及 VCS/用量/通知等辅助状态。
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
    pub slash_category: SlashCategory,
    pub session_picker: bool,
    pub session_selected: usize,
    pub session_query: Vec<char>,
    pub session_query_cursor: usize,
    pub session_loading: Option<String>,
    session_load_generation: u64,
    session_picker_exit_on_cancel: bool,
    pub view: View,
    pub scroll: Cell<u16>,
    pub render_max_scroll: Cell<u16>,
    pub plan_scroll: Cell<u16>,
    pub plan_max_scroll: Cell<u16>,
    history_oldest_index: u64,
    history_loading: bool,
    history_touched_at: Option<Instant>,
    pub model: String,
    pub cwd: String,
    pub launch_workspace: PathBuf,
    pub execution_mode: String,
    pub thinking_level: String,
    pub thinking_availability: ThinkingAvailability,
    pub thinking_message: String,
    pub context_percent: Option<f64>,
    pub session_usage: SessionUsage,
    pub vcs: Option<VcsChanges>,
    pub vcs_loading: bool,
    pub vcs_confirm_revert: bool,
    pub vcs_selected: usize,
    pub vcs_scroll: Cell<u16>,
    pub vcs_max_scroll: Cell<u16>,
    pub compacting_context: bool,
    pub confirm_model_compaction: bool,
    pub status: String,
    pub status_error: bool,
    pub status_frame: u64,
    reduced_motion: bool,
    pub approval: Option<Approval>,
    pending_approvals: VecDeque<Approval>,
    approval_resolving: bool,
    pub approval_scroll: Cell<u16>,
    pub approval_max_scroll: Cell<u16>,
    pub attachments: Vec<AttachmentDraft>,
    pub path_picker: bool,
    pub path_input: Vec<char>,
    pub attachment_selected: usize,
    pub attachment_list_focused: bool,
    pub path_directory: PathBuf,
    pub path_entries: Vec<PathEntry>,
    pub path_selected: usize,
    pub model_options: Vec<ModelOption>,
    pub thinking_options: Vec<String>,
    pub settings_picker: Option<SettingsPicker>,
    pub settings_selected: usize,
    pub provider_options: Vec<ProviderOption>,
    pub api_key_dialog: bool,
    pub api_key_selected: usize,
    pub api_key_provider: Option<String>,
    pub provider_api: String,
    pub provider_base_url_input: Vec<char>,
    pub provider_base_url_cursor: usize,
    pub provider_connection_field: usize,
    pub api_key_input: Vec<char>,
    pub api_key_cursor: usize,
    pub provider_input_selected_all: bool,
    abort_pressed: bool,
    queued_prompts: VecDeque<QueuedPrompt>,
    runtime_queued_count: usize,
    pasted_ranges: Vec<PastedRange>,
    slash_usage: HashMap<String, SlashUsage>,
    slash_usage_path: Option<PathBuf>,
    pending_slash_command: Option<String>,
}

impl App {
    /// 构造初始 App：整理计划、限制消息数、初始化输入框与各弹窗状态。
    pub fn new(
        sessions: Vec<SessionSummary>,
        session: SessionSummary,
        messages: Vec<ChatMessage>,
        context_usage: Option<ContextUsage>,
        tools: Vec<ToolDefinition>,
        skills: Vec<SkillDefinition>,
    ) -> Self {
        let mut sessions = sessions;
        let mut session = session;
        session.plan = active_plan(session.plan.take());
        if let Some(summary) = sessions.iter_mut().find(|summary| summary.id == session.id) {
            summary.plan.clone_from(&session.plan);
        }
        let path_directory = PathBuf::from(&session.cwd);
        let mut messages = messages;
        cap_message_count(&mut messages);
        let initial_plan_scroll = session
            .plan
            .as_ref()
            .map(current_plan_item_index)
            .unwrap_or_default()
            .min(u16::MAX as usize) as u16;
        Self {
            model: session.model.clone(),
            cwd: session.cwd.clone(),
            launch_workspace: path_directory.clone(),
            execution_mode: session.execution_mode.clone(),
            thinking_level: session.thinking_level.clone(),
            thinking_availability: ThinkingAvailability::Loading,
            thinking_message: String::new(),
            context_percent: context_usage.and_then(|usage| usage.percent),
            session_usage: SessionUsage::default(),
            vcs: None,
            vcs_loading: false,
            vcs_confirm_revert: false,
            vcs_selected: 0,
            vcs_scroll: Cell::new(0),
            vcs_max_scroll: Cell::new(0),
            compacting_context: false,
            confirm_model_compaction: false,
            sessions,
            session,
            messages,
            live: None,
            tools,
            skills,
            input: Vec::new(),
            input_cursor: 0,
            slash_selected: 0,
            slash_category: SlashCategory::All,
            session_picker: false,
            session_selected: 0,
            session_query: Vec::new(),
            session_query_cursor: 0,
            session_loading: None,
            session_load_generation: 0,
            session_picker_exit_on_cancel: false,
            view: View::Chat,
            scroll: Cell::new(0),
            render_max_scroll: Cell::new(0),
            plan_scroll: Cell::new(initial_plan_scroll),
            plan_max_scroll: Cell::new(0),
            history_oldest_index: 0,
            history_loading: false,
            history_touched_at: None,
            status: String::new(),
            status_error: false,
            status_frame: 0,
            reduced_motion: env_flag("PISPER_TUI_REDUCED_MOTION"),
            approval: None,
            pending_approvals: VecDeque::new(),
            approval_resolving: false,
            approval_scroll: Cell::new(0),
            approval_max_scroll: Cell::new(0),
            attachments: Vec::new(),
            path_picker: false,
            path_input: Vec::new(),
            attachment_selected: 0,
            attachment_list_focused: false,
            path_directory,
            path_entries: Vec::new(),
            path_selected: 0,
            model_options: Vec::new(),
            thinking_options: Vec::new(),
            settings_picker: None,
            settings_selected: 0,
            provider_options: Vec::new(),
            api_key_dialog: false,
            api_key_selected: 0,
            api_key_provider: None,
            provider_api: PROVIDER_APIS[0].0.to_owned(),
            provider_base_url_input: Vec::new(),
            provider_base_url_cursor: 0,
            provider_connection_field: 2,
            api_key_input: Vec::new(),
            api_key_cursor: 0,
            provider_input_selected_all: false,
            abort_pressed: false,
            queued_prompts: VecDeque::new(),
            runtime_queued_count: 0,
            pasted_ranges: Vec::new(),
            slash_usage: load_slash_usage(),
            slash_usage_path: slash_usage_path(),
            pending_slash_command: None,
        }
    }

    /// 输入框当前文本（未折叠）。
    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    /// 输入框的「显示」内容与光标位置：粘贴块折叠为占位文本。
    /// 显示层与存储层分离，既避免长粘贴占据整个输入框，又不丢失原文。
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

    /// 输入框是否处于可接受输入状态：无审批、无路径/会话/设置/API Key 弹窗。
    pub fn accepts_composer_input(&self) -> bool {
        self.approval.is_none()
            && !self.path_picker
            && !self.session_picker
            && self.settings_picker.is_none()
            && !self.api_key_dialog
    }

    /// 当前是否有运行的 Agent 回合。
    pub fn is_streaming(&self) -> bool {
        self.live.as_ref().is_some_and(|turn| turn.streaming)
    }

    /// 是否处于「运行中」视觉状态（驱动状态栏动画）。
    pub fn is_running_state(&self) -> bool {
        self.is_streaming()
            || self.compacting_context
            || matches!(self.status.as_str(), "thinking" | "streaming")
            || self.status.starts_with("running ")
    }

    /// 是否有未完成流式渲染（打字机动画需要继续推进）。
    pub fn has_pending_render(&self) -> bool {
        self.live.as_ref().is_some_and(|turn| {
            turn.text != turn.text_target || turn.thinking != turn.thinking_target
        })
    }

    /// 推进流式渲染：滚动回溯或减动画模式直接跳到目标文本，
    /// 否则按打字机步长逐步展示；同时推进状态帧计数。
    pub fn advance_stream_render(&mut self) {
        if let Some(live) = self.live.as_mut() {
            if self.reduced_motion || self.scroll.get() > 0 {
                live.thinking.clone_from(&live.thinking_target);
                live.text.clone_from(&live.text_target);
            } else {
                advance_typewriter(&mut live.thinking, &live.thinking_target);
                advance_typewriter(&mut live.text, &live.text_target);
            }
        }
        if !self.reduced_motion {
            self.status_frame = self.status_frame.wrapping_add(1);
        }
    }

    /// 推进状态栏动画帧。
    pub fn advance_status_animation(&mut self) {
        if !self.reduced_motion {
            self.status_frame = self.status_frame.wrapping_add(1);
        }
    }

    /// 是否开启减动画模式（`PISPER_TUI_REDUCED_MOTION`）。
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Slash 目录是否打开：输入以 `/` 开头且无空白。
    pub fn slash_open(&self) -> bool {
        let input = self.input_text();
        input.starts_with('/') && !input.chars().any(char::is_whitespace)
    }

    /// 路径输入框文本。
    pub fn path_input_text(&self) -> String {
        self.path_input.iter().collect()
    }

    /// 排队中的消息总数（本地队列 + Runtime 侧队列）。
    pub fn queued_count(&self) -> usize {
        self.queued_prompts.len() + self.runtime_queued_count
    }

    /// 待审批总数（当前可见的 + 排队中的）。
    pub fn approval_count(&self) -> usize {
        usize::from(self.approval.is_some()) + self.pending_approvals.len()
    }

    /// 按 id 查找审批（含排队中的）。
    pub fn approval_by_id(&self, id: &str) -> Option<&Approval> {
        self.approval
            .iter()
            .chain(self.pending_approvals.iter())
            .find(|approval| approval.id == id)
    }

    /// 是否正在向 Runtime 提交审批结果。
    pub fn approval_is_resolving(&self) -> bool {
        self.approval_resolving
    }

    /// 入队审批：相同 id 覆盖；当前无可见审批时直接成为可见审批。
    fn enqueue_approval(&mut self, approval: Approval) {
        if self
            .approval
            .as_ref()
            .is_some_and(|item| item.id == approval.id)
        {
            self.approval = Some(approval);
        } else if let Some(existing) = self
            .pending_approvals
            .iter_mut()
            .find(|item| item.id == approval.id)
        {
            *existing = approval;
        } else if self.approval.is_none() {
            self.approval = Some(approval);
        } else {
            self.pending_approvals.push_back(approval);
        }
    }

    /// 审批解析成功：可见审批出队并展示下一个排队审批。
    pub fn approval_resolution_succeeded(&mut self, id: &str) {
        if self
            .approval
            .as_ref()
            .is_some_and(|approval| approval.id == id)
        {
            self.approval = self.pending_approvals.pop_front();
            self.approval_resolving = false;
            self.approval_scroll.set(0);
            self.approval_max_scroll.set(0);
        } else {
            self.pending_approvals.retain(|approval| approval.id != id);
        }
    }

    /// 审批解析失败：仅复位「解析中」标记，让用户能重新按键。
    pub fn approval_resolution_failed(&mut self) {
        self.approval_resolving = false;
    }

    /// 清空全部审批状态（运行结束/失败/中止时调用）。
    fn clear_approvals(&mut self) {
        self.approval = None;
        self.pending_approvals.clear();
        self.approval_resolving = false;
        self.approval_scroll.set(0);
        self.approval_max_scroll.set(0);
    }

    /// 写入启动数据：草稿会话在此阶段才拿到默认模型与思考级别，
    /// 同时刷新工具/Skill/模型/Provider 目录。
    pub fn set_startup_data(
        &mut self,
        default_model: String,
        thinking_level: String,
        model_options: Vec<ModelOption>,
        provider_options: Vec<ProviderOption>,
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
        self.set_provider_options(provider_options);
        self.tools = tools;
        self.skills = skills;
    }

    /// 是否草稿会话（尚未在 sidecar 中创建）。
    pub fn is_draft_session(&self) -> bool {
        self.session.id.is_empty()
    }

    /// 设置模型选项（按 Provider → 名称 → id 排序，便于选择器定位）。
    pub fn set_model_options(&mut self, mut options: Vec<ModelOption>) {
        options.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        self.model_options = options;
    }

    /// 设置 Provider 选项（按类型 → 名称 → id 排序）。
    pub fn set_provider_options(&mut self, mut options: Vec<ProviderOption>) {
        options.sort_by(|left, right| {
            left.provider_type
                .cmp(&right.provider_type)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        self.provider_options = options;
    }

    /// 打开 API Key 对话框：运行中禁止修改 Provider 凭据。
    pub fn open_api_key_dialog(&mut self) {
        if self.is_streaming() {
            self.status = "Stop the active run before changing provider credentials".to_owned();
            self.status_error = true;
            return;
        }
        self.api_key_selected = self
            .provider_options
            .iter()
            .position(|provider| provider.configured)
            .unwrap_or(0);
        self.api_key_provider = None;
        self.clear_provider_connection_input();
        self.api_key_dialog = true;
        self.status_error = false;
    }

    /// 进入指定 Provider 的连接编辑态（协议/Base URL/API Key 三个字段）。
    fn edit_provider_connection(&mut self, provider_id: String) {
        let Some(provider) = self
            .provider_options
            .iter()
            .find(|provider| provider.id == provider_id)
        else {
            return;
        };
        self.provider_api = PROVIDER_APIS
            .iter()
            .find(|(api, _)| *api == provider.api)
            .map(|(api, _)| (*api).to_owned())
            .unwrap_or_else(|| PROVIDER_APIS[0].0.to_owned());
        self.provider_base_url_input = provider.base_url.chars().collect();
        self.provider_base_url_cursor = self.provider_base_url_input.len();
        self.provider_connection_field = 2;
        self.provider_input_selected_all = false;
        self.clear_api_key_input();
        self.api_key_provider = Some(provider_id);
    }

    /// Provider 连接保存成功：回写选项并退出对话框。
    pub fn provider_connection_saved(
        &mut self,
        provider_id: &str,
        api: String,
        base_url: String,
        api_key_updated: bool,
    ) {
        if let Some(provider) = self
            .provider_options
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            provider.api = api;
            provider.base_url = base_url;
            provider.configured |= api_key_updated;
        }
        self.api_key_dialog = false;
        self.api_key_provider = None;
        self.clear_provider_connection_input();
        self.status = format!("Provider connection saved · {provider_id}");
        self.status_error = false;
    }

    /// Provider 连接保存失败：清空 API Key 输入并显示错误。
    pub fn provider_connection_save_failed(&mut self, error: String) {
        self.clear_api_key_input();
        self.status = format!("Provider connection save failed · {error}");
        self.status_error = true;
    }

    /// 清空 API Key 输入（敏感字段，务必清零内存）。
    fn clear_api_key_input(&mut self) {
        self.api_key_input.zeroize();
        self.api_key_cursor = 0;
        self.provider_input_selected_all = false;
    }

    /// 清空 Provider 连接表单的全部输入。
    fn clear_provider_connection_input(&mut self) {
        self.clear_api_key_input();
        self.provider_base_url_input.clear();
        self.provider_base_url_cursor = 0;
        self.provider_input_selected_all = false;
        self.provider_api = PROVIDER_APIS[0].0.to_owned();
        self.provider_connection_field = 2;
    }

    /// 附件选择器中按当前过滤词可见的条目。
    pub fn visible_path_entries(&self) -> Vec<&PathEntry> {
        let query = self.path_input_text().to_lowercase();
        self.path_entries
            .iter()
            .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .collect()
    }

    /// 可用的思考级别列表。
    pub fn thinking_levels(&self) -> &[String] {
        &self.thinking_options
    }

    /// 记录启动工作区：`/new` 与全局 resume 都以它为准。
    pub fn set_launch_workspace(&mut self, workspace: PathBuf) {
        self.launch_workspace = workspace;
    }

    /// 新建会话应使用的工作区。
    pub fn new_session_workspace(&self) -> &Path {
        &self.launch_workspace
    }

    /// 打开会话选择器（定位到当前会话）。
    pub fn open_session_picker(&mut self, exit_on_cancel: bool) {
        let active_id = self.session.id.clone();
        self.open_session_picker_at(exit_on_cancel, &active_id);
    }

    /// 打开会话选择器并定位到指定会话。
    pub fn open_session_picker_at(&mut self, exit_on_cancel: bool, selected_id: &str) {
        self.session_query.clear();
        self.session_query_cursor = 0;
        self.session_selected = self
            .sessions
            .iter()
            .position(|session| session.id == selected_id)
            .unwrap_or(0);
        self.session_loading = None;
        self.session_picker_exit_on_cancel = exit_on_cancel;
        self.session_picker = true;
    }

    /// 按搜索词过滤可见会话（名称/模型/工作区）。
    pub fn visible_sessions(&self) -> Vec<&SessionSummary> {
        let query = self.session_query.iter().collect::<String>().to_lowercase();
        self.sessions
            .iter()
            .filter(|session| {
                query.is_empty()
                    || session.name.to_lowercase().contains(&query)
                    || session.model.to_lowercase().contains(&query)
                    || session.cwd.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// 会话加载结果是否仍有效：请求代次与会话 id 都须匹配，
    /// 丢弃切换期间返回的过期结果。
    pub fn is_current_session_load(&self, request_id: u64, session_id: &str) -> bool {
        self.session_load_generation == request_id
            && self.session_loading.as_deref() == Some(session_id)
    }

    /// 会话加载失败处理（仅当前请求才生效）。
    pub fn session_load_failed(&mut self, request_id: u64, session_id: &str, error: String) {
        if !self.is_current_session_load(request_id, session_id) {
            return;
        }
        self.session_loading = None;
        self.status = format!("cannot resume conversation · {error}");
        self.status_error = true;
    }

    /// 开始加载思考级别（复位选项并显示加载状态）。
    pub fn begin_thinking_load(&mut self) {
        self.thinking_options.clear();
        self.thinking_message.clear();
        self.thinking_availability = ThinkingAvailability::Loading;
        self.status = "loading thinking levels".to_owned();
        self.status_error = false;
    }

    /// 应用思考级别状态：更新选项、可用性与当前级别；
    /// 若正处于加载态则把状态栏切换为结果提示。
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

    /// 思考级别加载失败：转为错误态并提示。
    pub fn set_thinking_error(&mut self, error: String) {
        self.thinking_options.clear();
        self.thinking_message.clone_from(&error);
        self.thinking_availability = ThinkingAvailability::Error(error.clone());
        self.status = format!("thinking levels unavailable · {error}");
        self.status_error = true;
    }

    /// 打开思考级别选择器。
    pub fn open_thinking_picker(&mut self) {
        self.open_settings_picker(SettingsPicker::Thinking);
    }

    /// 打开附件路径选择器（定位到会话工作区）。
    pub fn open_path_picker(&mut self) {
        self.path_picker = true;
        self.path_input.clear();
        self.attachment_list_focused = false;
        self.attachment_selected = self
            .attachment_selected
            .min(self.attachments.len().saturating_sub(1));
        self.path_directory = PathBuf::from(&self.cwd);
        self.refresh_path_entries();
    }

    /// 刷新当前目录的条目列表：目录排前、按名称排序，支持附件类型才可选中。
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

    /// 打开设置选择器（模型/思考级别）；运行中禁止修改运行时设置。
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

    /// 当前分类下可见的 Slash 项（已按分类过滤）。
    pub fn slash_items(&self) -> Vec<SlashItem> {
        self.filtered_slash_items()
            .into_iter()
            .filter(|item| self.slash_category.includes(item.kind))
            .collect()
    }

    /// 各类型 Slash 项数量（目录标题用）。
    pub fn slash_kind_counts(&self) -> (usize, usize, usize) {
        self.filtered_slash_items()
            .iter()
            .fold((0, 0, 0), |mut counts, item| {
                match item.kind {
                    SlashKind::Tool => counts.0 += 1,
                    SlashKind::Skill => counts.1 += 1,
                    SlashKind::Command => counts.2 += 1,
                }
                counts
            })
    }

    /// 构造并按相关性排序 Slash 项：前缀匹配优先、其次使用频率、再次最近使用。
    fn filtered_slash_items(&self) -> Vec<SlashItem> {
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
            command("/changes", "Inspect Git or SVN workspace changes"),
            command("/chat", "Return to the conversation"),
            command("/model", "Switch the active session model"),
            command("/thinking", "Switch the active session thinking level"),
            command(
                "/provider",
                "Edit a Provider protocol, Base URL, and API Key securely",
            ),
            command("/web", "Open the installed Web settings"),
            command("/compact", "Summarize older context now"),
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

    /// 循环切换 Slash 分类。
    fn cycle_slash_category(&mut self, forward: bool) {
        self.slash_category = if forward {
            self.slash_category.next()
        } else {
            self.slash_category.previous()
        };
        self.slash_selected = 0;
    }

    /// 按键总入口：按弹窗/状态优先级分发到对应处理器，
    /// 最后回退到对话区通用按键处理。
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == crossterm::event::KeyEventKind::Release {
            return Action::None;
        }
        if key.kind == crossterm::event::KeyEventKind::Repeat {
            let navigation = matches!(
                key.code,
                KeyCode::Backspace
                    | KeyCode::Delete
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Home
                    | KeyCode::End
            );
            let composer_character = matches!(key.code, KeyCode::Char(_))
                && self.view == View::Chat
                && self.accepts_composer_input()
                && !self.confirm_model_compaction
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            if !navigation && !composer_character {
                return Action::None;
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
            && self.confirm_model_compaction
        {
            self.confirm_model_compaction = false;
            self.status = "model changed · context kept".to_owned();
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            // Ctrl+C：运行中先中止，第二次再按强制退出；空闲时直接退出。
            return if self.is_streaming() || self.approval.is_some() {
                self.clear_approvals();
                if self.abort_pressed {
                    Action::Quit
                } else {
                    self.abort_pressed = true;
                    Action::Abort
                }
            } else {
                Action::Quit
            };
        }
        if self.confirm_model_compaction {
            // 换模型后的「是否压缩上下文」确认弹层。
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_model_compaction = false;
                    Action::Compact
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_model_compaction = false;
                    self.status = "model changed · context kept".to_owned();
                    Action::None
                }
                _ => Action::None,
            };
        }
        if let Some(approval) = self.approval.clone() {
            // 有可见审批时，只响应 Y/N/Esc 与滚动键。
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') if !self.approval_resolving => {
                    self.approval_resolving = true;
                    self.status = "resolving approval".to_owned();
                    Action::ResolveApproval {
                        id: approval.id,
                        approved: true,
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc
                    if !self.approval_resolving =>
                {
                    self.approval_resolving = true;
                    self.status = "resolving approval".to_owned();
                    Action::ResolveApproval {
                        id: approval.id,
                        approved: false,
                    }
                }
                KeyCode::Up => {
                    self.approval_scroll
                        .set(self.approval_scroll.get().saturating_sub(1));
                    Action::None
                }
                KeyCode::Down => {
                    self.approval_scroll.set(
                        self.approval_scroll
                            .get()
                            .saturating_add(1)
                            .min(self.approval_max_scroll.get()),
                    );
                    Action::None
                }
                KeyCode::PageUp => {
                    self.approval_scroll
                        .set(self.approval_scroll.get().saturating_sub(PAGE_SCROLL_STEP));
                    Action::None
                }
                KeyCode::PageDown => {
                    self.approval_scroll.set(
                        self.approval_scroll
                            .get()
                            .saturating_add(PAGE_SCROLL_STEP)
                            .min(self.approval_max_scroll.get()),
                    );
                    Action::None
                }
                KeyCode::Home => {
                    self.approval_scroll.set(0);
                    Action::None
                }
                KeyCode::End => {
                    self.approval_scroll.set(self.approval_max_scroll.get());
                    Action::None
                }
                _ => Action::None,
            };
        }
        if self.api_key_dialog {
            return self.handle_api_key_dialog(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            // Ctrl+O：直接打开附件选择器（不丢弃输入框草稿）。
            self.open_path_picker();
            return Action::None;
        }
        if self.path_picker {
            return self.handle_path_picker(key);
        }
        if self.settings_picker.is_some() {
            return self.handle_settings_picker(key);
        }
        if self.view == View::Changes {
            // 变更视图：文件导航、滚动、VCS 动作（含两次 V 确认回退）。
            return match key.code {
                KeyCode::Esc => {
                    self.view = View::Chat;
                    self.vcs_confirm_revert = false;
                    Action::None
                }
                KeyCode::Up => {
                    self.vcs_scroll.set(self.vcs_scroll.get().saturating_sub(1));
                    Action::None
                }
                KeyCode::Down => {
                    self.vcs_scroll.set(
                        self.vcs_scroll
                            .get()
                            .saturating_add(1)
                            .min(self.vcs_max_scroll.get()),
                    );
                    Action::None
                }
                KeyCode::PageUp => {
                    self.vcs_scroll
                        .set(self.vcs_scroll.get().saturating_sub(PAGE_SCROLL_STEP));
                    Action::None
                }
                KeyCode::PageDown => {
                    self.vcs_scroll.set(
                        self.vcs_scroll
                            .get()
                            .saturating_add(PAGE_SCROLL_STEP)
                            .min(self.vcs_max_scroll.get()),
                    );
                    Action::None
                }
                KeyCode::Home => {
                    self.vcs_scroll.set(0);
                    Action::None
                }
                KeyCode::End => {
                    self.vcs_scroll.set(self.vcs_max_scroll.get());
                    Action::None
                }
                KeyCode::Left => {
                    self.select_vcs_file(self.vcs_selected.saturating_sub(1));
                    Action::None
                }
                KeyCode::Right => {
                    self.select_vcs_file(self.vcs_selected.saturating_add(1));
                    Action::None
                }
                KeyCode::Char('r') | KeyCode::Char('R') => Action::RefreshVcs,
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    Action::CommitVcs("Agent changes".to_owned())
                }
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    if self
                        .vcs
                        .as_ref()
                        .is_some_and(|changes| changes.vcs == "svn")
                    {
                        self.status = "SVN workspace needs no push; commit is already synchronized"
                            .to_owned();
                        Action::None
                    } else {
                        Action::PushVcs
                    }
                }
                KeyCode::Char('v') | KeyCode::Char('V') if self.vcs_confirm_revert => {
                    self.vcs_confirm_revert = false;
                    Action::RevertVcs
                }
                KeyCode::Char('v') | KeyCode::Char('V') => {
                    self.vcs_confirm_revert = true;
                    self.status = "press V again to revert workspace changes".to_owned();
                    Action::None
                }
                _ => Action::None,
            };
        }
        if self.session_picker {
            return self.handle_session_picker(key);
        }
        if self.slash_open() {
            // Slash 目录打开时的导航/补全/选择。
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
                KeyCode::Right => {
                    self.cycle_slash_category(true);
                    return Action::None;
                }
                KeyCode::BackTab | KeyCode::Left => {
                    self.cycle_slash_category(false);
                    return Action::None;
                }
                KeyCode::Enter => return self.choose_slash(),
                _ => {}
            }
        }
        if self.view == View::Chat
            && key.modifiers.contains(KeyModifiers::ALT)
            && matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Home
                    | KeyCode::End
            )
            && self
                .session
                .plan
                .as_ref()
                .is_some_and(|plan| !plan.items.is_empty())
        {
            // Alt+方向键：仅滚动计划面板，与对话滚动互不干扰。
            return match key.code {
                KeyCode::Up => {
                    self.plan_scroll
                        .set(self.plan_scroll.get().saturating_sub(LINE_SCROLL_STEP));
                    Action::None
                }
                KeyCode::Down => {
                    self.plan_scroll.set(
                        self.plan_scroll
                            .get()
                            .saturating_add(LINE_SCROLL_STEP)
                            .min(self.plan_max_scroll.get()),
                    );
                    Action::None
                }
                KeyCode::PageUp => {
                    self.plan_scroll
                        .set(self.plan_scroll.get().saturating_sub(PAGE_SCROLL_STEP));
                    Action::None
                }
                KeyCode::PageDown => {
                    self.plan_scroll.set(
                        self.plan_scroll
                            .get()
                            .saturating_add(PAGE_SCROLL_STEP)
                            .min(self.plan_max_scroll.get()),
                    );
                    Action::None
                }
                KeyCode::Home => {
                    self.plan_scroll.set(0);
                    Action::None
                }
                KeyCode::End => {
                    self.plan_scroll.set(self.plan_max_scroll.get());
                    Action::None
                }
                _ => Action::None,
            };
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
                // 上滚时靠近顶部会触发加载更早历史。
                self.scroll
                    .set(self.scroll.get().saturating_add(LINE_SCROLL_STEP));
                self.history_touched_at = Some(Instant::now());
                self.maybe_history_action()
            }
            KeyCode::Down if self.view == View::Chat => {
                self.scroll
                    .set(self.scroll.get().saturating_sub(LINE_SCROLL_STEP));
                Action::None
            }
            KeyCode::PageUp if self.view == View::Chat => {
                self.scroll
                    .set(self.scroll.get().saturating_add(PAGE_SCROLL_STEP));
                self.history_touched_at = Some(Instant::now());
                self.maybe_history_action()
            }
            KeyCode::PageDown if self.view == View::Chat => {
                self.scroll
                    .set(self.scroll.get().saturating_sub(PAGE_SCROLL_STEP));
                Action::None
            }
            KeyCode::Esc => {
                if self.view != View::Chat {
                    self.view = View::Chat;
                    self.vcs_confirm_revert = false;
                } else {
                    self.clear_input();
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    /// 选中 VCS 文件并把 diff 视口滚动到该文件的位置。
    fn select_vcs_file(&mut self, index: usize) {
        let Some(changes) = self.vcs.as_ref() else {
            self.vcs_selected = 0;
            self.vcs_scroll.set(0);
            return;
        };
        self.vcs_selected = index.min(changes.files.len().saturating_sub(1));
        let Some(path) = changes.files.get(self.vcs_selected).map(|file| &file.path) else {
            self.vcs_scroll.set(0);
            return;
        };
        let line = changes
            .diff
            .lines()
            .position(|line| line.contains(path))
            .unwrap_or_default();
        self.vcs_scroll
            .set((line.min(self.vcs_max_scroll.get() as usize)) as u16);
    }

    /// API Key 对话框按键处理：先选 Provider，进入后编辑三个字段。
    fn handle_api_key_dialog(&mut self, key: KeyEvent) -> Action {
        if self.api_key_provider.is_none() {
            let count = self.provider_options.len();
            return match key.code {
                KeyCode::Esc => {
                    self.api_key_dialog = false;
                    self.clear_provider_connection_input();
                    Action::None
                }
                KeyCode::Up => {
                    self.api_key_selected = self.api_key_selected.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    self.api_key_selected =
                        (self.api_key_selected + 1).min(count.saturating_sub(1));
                    Action::None
                }
                KeyCode::Enter => {
                    if let Some(provider) = self.provider_options.get(self.api_key_selected) {
                        self.edit_provider_connection(provider.id.clone());
                    }
                    Action::None
                }
                _ => Action::None,
            };
        }

        match key.code {
            KeyCode::Esc => {
                self.api_key_provider = None;
                self.clear_provider_connection_input();
                Action::None
            }
            KeyCode::Tab | KeyCode::Down => {
                self.provider_connection_field = (self.provider_connection_field + 1) % 3;
                self.provider_input_selected_all = false;
                Action::None
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.provider_connection_field = (self.provider_connection_field + 2) % 3;
                self.provider_input_selected_all = false;
                Action::None
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if matches!(self.provider_connection_field, 1 | 2) {
                    self.provider_input_selected_all = true;
                }
                Action::None
            }
            KeyCode::Left if self.provider_connection_field == 0 => {
                self.cycle_provider_api(false);
                Action::None
            }
            KeyCode::Right if self.provider_connection_field == 0 => {
                self.cycle_provider_api(true);
                Action::None
            }
            KeyCode::Left if self.provider_connection_field == 1 => {
                move_field_cursor_left(
                    &self.provider_base_url_input,
                    &mut self.provider_base_url_cursor,
                    &mut self.provider_input_selected_all,
                );
                Action::None
            }
            KeyCode::Right if self.provider_connection_field == 1 => {
                move_field_cursor_right(
                    &self.provider_base_url_input,
                    &mut self.provider_base_url_cursor,
                    &mut self.provider_input_selected_all,
                );
                Action::None
            }
            KeyCode::Left if self.provider_connection_field == 2 => {
                move_field_cursor_left(
                    &self.api_key_input,
                    &mut self.api_key_cursor,
                    &mut self.provider_input_selected_all,
                );
                Action::None
            }
            KeyCode::Right if self.provider_connection_field == 2 => {
                move_field_cursor_right(
                    &self.api_key_input,
                    &mut self.api_key_cursor,
                    &mut self.provider_input_selected_all,
                );
                Action::None
            }
            KeyCode::Home if self.provider_connection_field == 1 => {
                self.provider_base_url_cursor = 0;
                self.provider_input_selected_all = false;
                Action::None
            }
            KeyCode::End if self.provider_connection_field == 1 => {
                self.provider_base_url_cursor = self.provider_base_url_input.len();
                self.provider_input_selected_all = false;
                Action::None
            }
            KeyCode::Home if self.provider_connection_field == 2 => {
                self.api_key_cursor = 0;
                self.provider_input_selected_all = false;
                Action::None
            }
            KeyCode::End if self.provider_connection_field == 2 => {
                self.api_key_cursor = self.api_key_input.len();
                self.provider_input_selected_all = false;
                Action::None
            }
            KeyCode::Enter => {
                let base_url = self.provider_base_url_input.iter().collect::<String>();
                if base_url.trim().is_empty() {
                    self.status = "Provider Base URL cannot be empty".to_owned();
                    self.status_error = true;
                    self.provider_connection_field = 1;
                    return Action::None;
                }
                let provider = self.api_key_provider.clone().unwrap_or_default();
                let api = self.provider_api.clone();
                let api_key = self
                    .api_key_input
                    .iter()
                    .filter(|character| !character.is_whitespace())
                    .collect::<String>();
                self.clear_api_key_input();
                self.status = format!("saving Provider connection · {provider}");
                self.status_error = false;
                Action::SaveProviderConnection {
                    provider,
                    api,
                    base_url: base_url.trim().to_owned(),
                    api_key,
                }
            }
            KeyCode::Backspace if self.provider_connection_field == 1 => {
                delete_field_character(
                    &mut self.provider_base_url_input,
                    &mut self.provider_base_url_cursor,
                    &mut self.provider_input_selected_all,
                    true,
                );
                Action::None
            }
            KeyCode::Delete if self.provider_connection_field == 1 => {
                delete_field_character(
                    &mut self.provider_base_url_input,
                    &mut self.provider_base_url_cursor,
                    &mut self.provider_input_selected_all,
                    false,
                );
                Action::None
            }
            KeyCode::Backspace if self.provider_connection_field == 2 => {
                delete_field_character(
                    &mut self.api_key_input,
                    &mut self.api_key_cursor,
                    &mut self.provider_input_selected_all,
                    true,
                );
                Action::None
            }
            KeyCode::Delete if self.provider_connection_field == 2 => {
                delete_field_character(
                    &mut self.api_key_input,
                    &mut self.api_key_cursor,
                    &mut self.provider_input_selected_all,
                    false,
                );
                Action::None
            }
            KeyCode::Char(character)
                if self.provider_connection_field == 1
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                insert_field_characters(
                    &mut self.provider_base_url_input,
                    &mut self.provider_base_url_cursor,
                    &mut self.provider_input_selected_all,
                    std::iter::once(character),
                    4_096,
                );
                Action::None
            }
            KeyCode::Char(character)
                if self.provider_connection_field == 2
                    && !character.is_whitespace()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                insert_field_characters(
                    &mut self.api_key_input,
                    &mut self.api_key_cursor,
                    &mut self.provider_input_selected_all,
                    std::iter::once(character),
                    16_384,
                );
                Action::None
            }
            _ => Action::None,
        }
    }

    /// 循环切换 Provider 协议（左/右箭头）。
    fn cycle_provider_api(&mut self, forward: bool) {
        let current = PROVIDER_APIS
            .iter()
            .position(|(api, _)| *api == self.provider_api)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % PROVIDER_APIS.len()
        } else {
            (current + PROVIDER_APIS.len() - 1) % PROVIDER_APIS.len()
        };
        self.provider_api = PROVIDER_APIS[next].0.to_owned();
    }

    /// 附件路径选择器按键处理：导航目录、过滤、添加/移除附件。
    fn handle_path_picker(&mut self, key: KeyEvent) -> Action {
        if self.attachment_list_focused {
            match key.code {
                KeyCode::Esc => {
                    self.path_picker = false;
                    self.path_input.clear();
                    self.attachment_list_focused = false;
                }
                KeyCode::Tab | KeyCode::BackTab => self.attachment_list_focused = false,
                KeyCode::Left => {
                    self.attachment_selected = self.attachment_selected.saturating_sub(1);
                }
                KeyCode::Right => {
                    self.attachment_selected = (self.attachment_selected + 1)
                        .min(self.attachments.len().saturating_sub(1));
                }
                KeyCode::Delete => self.remove_selected_attachment(),
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Esc => {
                self.path_picker = false;
                self.path_input.clear();
            }
            KeyCode::Tab | KeyCode::BackTab if !self.attachments.is_empty() => {
                self.attachment_list_focused = true;
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

    /// 移除当前选中的附件。
    fn remove_selected_attachment(&mut self) {
        if self.attachments.is_empty() {
            self.attachment_list_focused = false;
            return;
        }
        let removed = self.attachments.remove(self.attachment_selected);
        self.attachment_selected = self
            .attachment_selected
            .min(self.attachments.len().saturating_sub(1));
        self.attachment_list_focused = !self.attachments.is_empty();
        self.status = format!("Attachment removed · {}", removed.name);
        self.status_error = false;
    }

    /// 回到父目录（不越过工作区边界）。
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

    /// 进入子目录（同样限制在工作区内且必须是目录）。
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

    /// 添加附件：校验类型/数量/去重/总大小后入列。
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

    /// 设置选择器按键处理（模型/思考级别）。
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

    /// 会话选择器按键处理：搜索、导航、切换（含 Esc 取消逻辑）。
    fn handle_session_picker(&mut self, key: KeyEvent) -> Action {
        if self.session_loading.is_some() {
            return if key.code == KeyCode::Esc {
                self.session_load_generation = self.session_load_generation.wrapping_add(1);
                self.session_loading = None;
                self.session_picker = false;
                if std::mem::take(&mut self.session_picker_exit_on_cancel) {
                    Action::Quit
                } else {
                    Action::None
                }
            } else {
                Action::None
            };
        }
        match key.code {
            KeyCode::Esc => {
                self.session_picker = false;
                self.session_query.clear();
                self.session_query_cursor = 0;
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
                self.session_selected = (self.session_selected + 1)
                    .min(self.visible_sessions().len().saturating_sub(1));
                Action::None
            }
            KeyCode::Left => {
                self.session_query_cursor = self.session_query_cursor.saturating_sub(1);
                Action::None
            }
            KeyCode::Right => {
                self.session_query_cursor =
                    (self.session_query_cursor + 1).min(self.session_query.len());
                Action::None
            }
            KeyCode::Home => {
                self.session_query_cursor = 0;
                Action::None
            }
            KeyCode::End => {
                self.session_query_cursor = self.session_query.len();
                Action::None
            }
            KeyCode::Backspace => {
                if self.session_query_cursor > 0 {
                    self.session_query_cursor -= 1;
                    self.session_query.remove(self.session_query_cursor);
                    self.session_selected = 0;
                }
                Action::None
            }
            KeyCode::Delete => {
                if self.session_query_cursor < self.session_query.len() {
                    self.session_query.remove(self.session_query_cursor);
                    self.session_selected = 0;
                }
                Action::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.session_query.clear();
                self.session_query_cursor = 0;
                self.session_selected = 0;
                Action::None
            }
            KeyCode::Char(character)
                if self.session_query.len() < 256
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.session_query
                    .insert(self.session_query_cursor, character);
                self.session_query_cursor += 1;
                self.session_selected = 0;
                Action::None
            }
            KeyCode::Enter => {
                let Some((id, name)) = self
                    .visible_sessions()
                    .get(self.session_selected)
                    .map(|session| (session.id.clone(), session.name.clone()))
                else {
                    return Action::None;
                };
                self.session_load_generation = self.session_load_generation.wrapping_add(1);
                let request_id = self.session_load_generation;
                self.session_loading = Some(id.clone());
                self.status = format!("loading conversation · {name}");
                self.status_error = false;
                Action::SwitchSession { id, request_id }
            }
            _ => Action::None,
        }
    }

    /// Tab 补全当前选中的 Slash 项（命令项不带尾随空格，
    /// 便于直接回车提交；`/dir` 例外需后续参数）。
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

    /// 回车选择 Slash 项：命令类直接提交，工具/技能类补全到输入框。
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

    /// 提交输入框内容：解析内置 `/` 命令，其余按普通消息发送。
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
            "/chat" => {
                self.mark_slash_use("/chat");
                self.view = View::Chat;
                self.clear_input();
                Action::None
            }
            "/changes" => {
                self.mark_slash_use("/changes");
                self.view = View::Changes;
                self.clear_input();
                Action::RefreshVcs
            }
            _ if message.starts_with("/changes commit ") => {
                let commit_message = message.trim_start_matches("/changes commit ").trim();
                self.mark_slash_use("/changes commit");
                self.view = View::Changes;
                self.clear_input();
                if commit_message.is_empty() {
                    Action::None
                } else {
                    Action::CommitVcs(commit_message.to_owned())
                }
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
            "/provider" | "/apikey" => {
                self.mark_slash_use("/provider");
                self.clear_input();
                self.open_api_key_dialog();
                Action::None
            }
            _ if message.starts_with("/provider ") || message.starts_with("/apikey ") => {
                self.mark_slash_use("/provider");
                self.clear_input();
                if self.is_streaming() {
                    self.status =
                        "Stop the active run before changing provider credentials".to_owned();
                    self.status_error = true;
                    return Action::None;
                }
                let provider = message.split_whitespace().nth(1).unwrap_or_default();
                if self
                    .provider_options
                    .iter()
                    .any(|option| option.id == provider)
                {
                    self.api_key_dialog = true;
                    self.edit_provider_connection(provider.to_owned());
                    self.status_error = false;
                    Action::None
                } else {
                    self.status = format!("provider not found · {provider}");
                    self.status_error = true;
                    Action::None
                }
            }
            "/web" => {
                self.mark_slash_use("/web");
                self.clear_input();
                Action::OpenWeb
            }
            "/compact" => {
                self.clear_input();
                if self.is_draft_session() {
                    self.status =
                        "Context compaction is available after the first message".to_owned();
                    self.status_error = true;
                    return Action::None;
                }
                if self.is_streaming() {
                    self.status = "Stop the active run before compacting context".to_owned();
                    self.status_error = true;
                    return Action::None;
                }
                if self.compacting_context {
                    self.status = "Context compaction is already running".to_owned();
                    self.status_error = true;
                    return Action::None;
                }
                self.mark_slash_use("/compact");
                Action::Compact
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
                self.clear_input();
                if attachments.is_empty() && requested_tool.is_none() {
                    Action::QueueInput { message }
                } else {
                    self.queued_prompts.push_back(QueuedPrompt {
                        message,
                        display_message: None,
                        requested_tool,
                        attachments,
                    });
                    self.status = format!("{} message(s) queued", self.queued_count());
                    Action::None
                }
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

    /// 启动一轮提示：写入用户消息、创建流式 LiveTurn，
    /// 并返回 Submit 动作（草稿会话由事件循环先物化再提交）。
    fn start_prompt(&mut self, prompt: QueuedPrompt) -> Action {
        let display_message = prompt
            .display_message
            .clone()
            .unwrap_or_else(|| prompt.message.clone());
        self.abort_pressed = false;
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
        self.status = "thinking".to_owned();
        self.scroll.set(0);
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

    /// 取出下一个排队提示（仅当当前无运行中的回合）。
    pub fn take_queued_action(&mut self) -> Option<Action> {
        (!self.is_streaming())
            .then(|| self.queued_prompts.pop_front())
            .flatten()
            .map(|prompt| self.start_prompt(prompt))
    }

    /// 排队输入成功：写入用户消息并更新排队计数。
    pub fn queue_input_succeeded(&mut self, message: String, queued_count: usize) {
        self.push_transcript_message(ChatMessage {
            role: "user".to_owned(),
            text: message,
            ..ChatMessage::default()
        });
        self.runtime_queued_count = queued_count;
        self.status = if queued_count > 0 {
            format!("{} message(s) queued", self.queued_count())
        } else {
            "append sent".to_owned()
        };
        self.status_error = false;
        self.scroll.set(0);
    }

    /// 排队输入因会话已结束被拒：推迟到下一轮运行再发。
    pub fn defer_input_after_run(&mut self, message: String) {
        self.queued_prompts.push_back(QueuedPrompt {
            message,
            display_message: None,
            requested_tool: None,
            attachments: Vec::new(),
        });
        self.status = format!("{} message(s) queued", self.queued_count());
        self.status_error = false;
    }

    /// 排队输入失败：输入框为空时恢复草稿，便于用户重试。
    pub fn queue_input_failed(&mut self, message: String, error: String) {
        if self.input.is_empty() {
            self.set_input(&message);
        }
        self.status = format!("append failed · {error}");
        self.status_error = true;
    }

    /// 提取消息开头以 `/` 开头的工具名（若存在对应工具）。
    fn requested_tool(&self, message: &str) -> Option<String> {
        let command = message.split_whitespace().next()?.strip_prefix('/')?;
        self.tools
            .iter()
            .any(|tool| tool.id == command)
            .then(|| command.to_owned())
    }

    /// 显式粘贴（bracketed-paste 事件）插入。
    pub fn insert_paste(&mut self, value: &str) {
        self.insert_paste_inner(value, false);
    }

    /// 检测到的粘贴（突发判定）插入：强制折叠为 Paste 块。
    pub fn insert_detected_paste(&mut self, value: &str) {
        self.insert_paste_inner(value, true);
    }

    /// 插入粘贴文本：路径/Provider 表单场景直接追加到对应输入；
    /// 输入框场景按规则折叠为 Paste 块并维护折叠区间。
    fn insert_paste_inner(&mut self, value: &str, force_range: bool) {
        if self.path_picker {
            self.path_input.extend(value.trim().chars());
            return;
        }
        if self.api_key_dialog && self.api_key_provider.is_some() {
            if self.provider_connection_field == 1 {
                insert_field_characters(
                    &mut self.provider_base_url_input,
                    &mut self.provider_base_url_cursor,
                    &mut self.provider_input_selected_all,
                    value.trim().chars(),
                    4_096,
                );
            } else if self.provider_connection_field == 2 {
                insert_field_characters(
                    &mut self.api_key_input,
                    &mut self.api_key_cursor,
                    &mut self.provider_input_selected_all,
                    value.chars().filter(|character| !character.is_whitespace()),
                    16_384,
                );
            }
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

    /// 替换为另一个会话：重置几乎所有 UI 状态（会话切换的唯一入口）。
    pub fn replace_session(
        &mut self,
        mut session: SessionSummary,
        messages: Vec<ChatMessage>,
        context_usage: Option<ContextUsage>,
    ) {
        session.plan = active_plan(session.plan.take());
        if let Some(summary) = self
            .sessions
            .iter_mut()
            .find(|summary| summary.id == session.id)
        {
            summary.plan.clone_from(&session.plan);
        }
        self.model = session.model.clone();
        self.cwd = session.cwd.clone();
        self.execution_mode = session.execution_mode.clone();
        self.thinking_level = session.thinking_level.clone();
        self.thinking_options.clear();
        self.thinking_availability = ThinkingAvailability::Loading;
        self.thinking_message.clear();
        self.context_percent = context_usage.and_then(|usage| usage.percent);
        self.session_usage = SessionUsage::default();
        self.vcs = None;
        self.vcs_loading = false;
        self.vcs_confirm_revert = false;
        self.vcs_selected = 0;
        self.vcs_scroll.set(0);
        self.vcs_max_scroll.set(0);
        self.compacting_context = false;
        self.confirm_model_compaction = false;
        self.session = session;
        self.messages = messages;
        cap_message_count(&mut self.messages);
        self.reset_history_window();
        self.live = None;
        self.clear_approvals();
        self.attachments.clear();
        self.path_picker = false;
        self.path_input.clear();
        self.attachment_selected = 0;
        self.attachment_list_focused = false;
        self.path_directory = PathBuf::from(&self.cwd);
        self.path_entries.clear();
        self.path_selected = 0;
        self.settings_picker = None;
        self.api_key_dialog = false;
        self.api_key_provider = None;
        self.clear_api_key_input();
        self.session_picker = false;
        self.session_loading = None;
        self.session_picker_exit_on_cancel = false;
        self.queued_prompts.clear();
        self.runtime_queued_count = 0;
        self.pending_slash_command = None;
        self.status.clear();
        self.status_error = false;
        self.view = View::Chat;
        self.scroll.set(0);
        self.follow_current_plan_item();
    }

    /// 记录历史窗口起点（最旧已加载消息的下标）。
    pub fn set_history_window(&mut self, oldest_loaded_index: u64) {
        self.history_oldest_index = oldest_loaded_index;
    }

    /// 复位历史窗口（会话切换/替换时）。
    fn reset_history_window(&mut self) {
        self.history_oldest_index = 0;
        self.history_loading = false;
        self.history_touched_at = None;
        self.render_max_scroll.set(0);
    }

    /// 是否还有更早的历史可加载。
    pub fn has_older_history(&self) -> bool {
        self.history_oldest_index > 0
    }

    /// 滚到接近顶部且仍有更早历史时，触发一次历史加载（每个加载窗口只触发一次）。
    fn maybe_history_action(&mut self) -> Action {
        if self.view == View::Chat
            && self.has_older_history()
            && !self.history_loading
            && self.scroll.get().saturating_add(HISTORY_SCROLL_MARGIN)
                >= self.render_max_scroll.get()
        {
            self.history_loading = true;
            self.history_touched_at = Some(Instant::now());
            return Action::LoadOlderMessages {
                before: self.history_oldest_index,
            };
        }
        Action::None
    }

    /// 应用一页更早历史：验证游标后前置插入；
    /// 返回空页说明已到最早，关闭历史窗口。
    pub fn apply_history_page(&mut self, page: MessagePage, before: u64) {
        self.history_loading = false;
        if before != self.history_oldest_index {
            return;
        }
        let MessagePage {
            messages,
            context_usage,
            session_usage,
            page_info,
        } = page;
        if let Some(usage) = context_usage {
            self.context_percent = usage.percent;
        }
        if let Some(usage) = session_usage {
            self.session_usage = usage;
        }
        if messages.is_empty() {
            self.history_oldest_index = 0;
            return;
        }
        let mut older = messages;
        older.append(&mut self.messages);
        self.messages = older;
        self.history_oldest_index = if page_info.has_more {
            page_info.start
        } else {
            0
        };
        self.history_touched_at = Some(Instant::now());
        self.cap_loaded_messages();
    }

    /// 历史加载失败：复位加载态并显示错误。
    pub fn history_load_failed(&mut self, message: String) {
        self.history_loading = false;
        self.status = message;
        self.status_error = true;
    }

    /// 空闲历史回收：超过保留窗口的早期消息被丢弃（仅限未在加载时），
    /// 以限制长会话的内存占用；返回是否发生了回收。
    pub fn evict_idle_history(&mut self, now: Instant) -> bool {
        if self.history_loading {
            return false;
        }
        let Some(touched) = self.history_touched_at else {
            return false;
        };
        if now.duration_since(touched) < HISTORY_IDLE_EVICT_DELAY {
            return false;
        }
        let excess = self.messages.len().saturating_sub(HISTORY_KEEP_MESSAGES);
        if excess == 0 {
            self.history_touched_at = None;
            return false;
        }
        self.messages.drain(..excess);
        self.history_oldest_index = self.history_oldest_index.saturating_add(excess as u64);
        self.history_touched_at = None;
        true
    }

    /// 把消息数裁剪到内存上限（丢弃最早的超额消息）。
    fn cap_loaded_messages(&mut self) {
        let excess = self.messages.len().saturating_sub(MAX_LOADED_MESSAGES);
        if excess == 0 {
            return;
        }
        self.messages.drain(..excess);
        self.history_oldest_index = self.history_oldest_index.saturating_add(excess as u64);
    }

    /// 物化会话：草稿 → 真实会话（`submit` 前调用），并插入会话列表。
    pub fn materialize_session(&mut self, mut session: SessionSummary) {
        session.plan = active_plan(session.plan.take());
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

    /// 草稿会话中预选模型（仅本地记录，物化时同步到 Runtime）。
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

    /// 草稿会话中预选思考级别。
    pub fn set_draft_thinking_level(&mut self, level: String) {
        self.thinking_level.clone_from(&level);
        self.session.thinking_level = level;
        self.status = format!("thinking selected · {}", self.thinking_level);
        self.status_error = false;
    }

    /// 应用工作区变更（清空附件等路径相关状态）。
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

    /// 应用执行模式变更。
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

    /// 应用模型变更；若已有消息则提示是否顺带压缩上下文。
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
        self.confirm_model_compaction = !self.messages.is_empty();
        self.status = if self.confirm_model_compaction {
            format!("model changed · {} · compact context? [y/N]", self.model)
        } else {
            format!("model changed · {}", self.model)
        };
        self.status_error = false;
    }

    /// 应用思考级别变更。
    pub fn set_thinking_level(&mut self, updated: ThinkingLevelUpdate) {
        self.set_thinking_state(updated);
        self.status = format!("thinking changed · {}", self.thinking_level);
        self.status_error = false;
    }

    /// 让计划面板跟随当前计划项（会话切换/计划更新时调用）。
    fn follow_current_plan_item(&self) {
        let Some(plan) = self.session.plan.as_ref() else {
            self.plan_scroll.set(0);
            self.plan_max_scroll.set(0);
            return;
        };
        self.plan_scroll
            .set(current_plan_item_index(plan).min(u16::MAX as usize) as u16);
    }

    /// 设置会话计划（隐藏已全部完成的计划），并同步到会话列表。
    pub fn set_plan(&mut self, plan: Option<Plan>) {
        let plan = active_plan(plan);
        self.session.plan = plan.clone();
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == self.session.id)
        {
            session.plan = plan;
        }
        self.follow_current_plan_item();
    }

    /// 处理一条对话流事件：先提取计划（新旧协议别名），
    /// 再按事件名分发到状态更新；计划类事件不参与后续对话渲染。
    pub fn apply_stream_event(&mut self, event: StreamEvent) {
        if let Some(plan) = plan_from_payload(&event.data) {
            self.set_plan(plan);
        }
        if is_plan_update_event(&event.name) {
            return;
        }
        if event.name == "session_usage" {
            if let Ok(usage) = serde_json::from_value::<SessionUsage>(event.data) {
                self.session_usage = usage;
            }
            return;
        }
        if event.name == "permission_resolved" {
            self.approval_resolution_succeeded(&string_field(&event.data, "id"));
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
                if let Ok(usage) = serde_json::from_value::<SessionUsage>(
                    event.data.get("sessionUsage").cloned().unwrap_or_default(),
                ) {
                    self.session_usage = usage;
                }
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
            "queue_update" => {
                self.runtime_queued_count = event.data["queuedInputs"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default();
                self.status = format!("{} message(s) queued", self.queued_count());
            }
            "permission_request" => {
                sync_live_display(live);
                self.enqueue_approval(Approval {
                    id: string_field(&event.data, "id"),
                    tool_name: string_field(&event.data, "toolName"),
                    args: event.data["args"].clone(),
                    risk: string_field(&event.data, "risk"),
                    reason: string_field(&event.data, "reason"),
                });
                self.approval_scroll.set(0);
                self.approval_max_scroll.set(0);
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
                self.runtime_queued_count = 0;
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
                if let Ok(usage) =
                    serde_json::from_value::<SessionUsage>(event.data["sessionUsage"].clone())
                {
                    self.session_usage = usage;
                }
                self.status = "complete".to_owned();
                self.abort_pressed = false;
            }
            "error" => {
                self.runtime_queued_count = 0;
                live.streaming = false;
                self.status = string_field(&event.data, "message");
                self.status_error = true;
                self.abort_pressed = false;
            }
            _ => {}
        }
        if event.name == "done" {
            self.clear_approvals();
            if let Some(command) = self.pending_slash_command.take() {
                self.mark_slash_use(&command);
            }
        } else if event.name == "error" {
            self.clear_approvals();
            self.pending_slash_command = None;
        }
    }

    /// 记录会话 token 用量（独立事件，不依赖流式 live 回合）。
    pub fn set_session_usage(&mut self, usage: SessionUsage) {
        self.session_usage = usage;
    }

    /// 设置 VCS 加载态（加载时显示状态栏提示）。
    pub fn set_vcs_loading(&mut self, loading: bool) {
        self.vcs_loading = loading;
        if loading {
            self.status = "loading workspace changes".to_owned();
            self.status_error = false;
        }
    }

    /// 应用 VCS 变更结果并复位滚动/确认态。
    pub fn set_vcs(&mut self, changes: VcsChanges) {
        self.vcs_loading = false;
        self.vcs_selected = self.vcs_selected.min(changes.files.len().saturating_sub(1));
        self.vcs = Some(changes);
        self.vcs_confirm_revert = false;
        self.vcs_scroll.set(0);
        self.vcs_max_scroll.set(0);
        self.status = "workspace changes refreshed".to_owned();
        self.status_error = false;
    }

    /// VCS 请求失败：显示错误并退出加载态。
    pub fn set_vcs_error(&mut self, error: String) {
        self.vcs_loading = false;
        self.status = format!("workspace changes unavailable · {error}");
        self.status_error = true;
    }

    /// 开始上下文压缩（显示进行中状态）。
    pub fn begin_context_compaction(&mut self) {
        self.compacting_context = true;
        self.status = "compacting context".to_owned();
        self.status_error = false;
    }

    /// 压缩完成：成功则更新上下文占用，失败则显示错误。
    pub fn finish_context_compaction(
        &mut self,
        context_usage: Option<ContextUsage>,
        error: Option<String>,
    ) {
        self.compacting_context = false;
        if let Some(error) = error {
            self.status = format!("context compaction failed · {error}");
            self.status_error = true;
        } else {
            self.context_percent = context_usage.and_then(|usage| usage.percent);
            self.status = "context compacted".to_owned();
            self.status_error = false;
        }
    }

    /// 对话流整体失败：结束当前回合并复位运行相关状态。
    pub fn stream_failed(&mut self, message: String) {
        if let Some(live) = self.live.as_mut() {
            live.streaming = false;
        }
        self.clear_approvals();
        self.runtime_queued_count = 0;
        self.abort_pressed = false;
        self.pending_slash_command = None;
        self.status = message;
        self.status_error = true;
    }

    /// 把当前 LiveTurn 提交为正式消息（新一轮开始时把上一轮收进历史）。
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

    /// 追加一条消息到转写区（自动裁剪到内存上限）。
    fn push_transcript_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.cap_loaded_messages();
    }

    /// 记录一次 Slash 命令使用并持久化（用于目录排序）。
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

    /// 在光标处插入一个字符（同时维护粘贴折叠区间偏移）。
    fn insert_input_character(&mut self, character: char) {
        self.shift_pasted_ranges_for_insert(self.input_cursor, 1);
        self.input.insert(self.input_cursor, character);
        self.input_cursor += 1;
        self.slash_selected = 0;
    }

    /// 退格删除：优先整块删除光标前的粘贴块。
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

    /// Delete 删除：优先整块删除光标处的粘贴块。
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

    /// 光标左移：遇到粘贴块时跳到块首，否则逐字符。
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

    /// 光标右移：遇到粘贴块时跳到块尾，否则逐字符。
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

    /// 插入后把光标位置之后的折叠区间整体右移。
    fn shift_pasted_ranges_for_insert(&mut self, position: usize, amount: usize) {
        for range in &mut self.pasted_ranges {
            if range.start >= position {
                range.start += amount;
                range.end += amount;
            }
        }
    }

    /// 删除后把光标位置之后的折叠区间整体左移。
    fn shift_pasted_ranges_after_removal(&mut self, position: usize) {
        for range in &mut self.pasted_ranges {
            if position < range.start {
                range.start -= 1;
                range.end -= 1;
            }
        }
    }

    /// 移除整个粘贴块（连同原文与区间记录）。
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

    /// 清空输入框（含折叠区间与 Slash 状态）。
    fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.pasted_ranges.clear();
        self.slash_selected = 0;
        self.slash_category = SlashCategory::All;
    }

    /// 用给定文本整体替换输入框内容。
    fn set_input(&mut self, value: &str) {
        self.input = value.chars().collect();
        self.input_cursor = self.input.len();
        self.pasted_ranges.clear();
        self.slash_selected = 0;
        self.slash_category = SlashCategory::All;
    }
}

/// 向表单字段插入字符：全选态先清空再插入，且不超字段上限。
fn insert_field_characters(
    input: &mut Vec<char>,
    cursor: &mut usize,
    selected_all: &mut bool,
    characters: impl IntoIterator<Item = char>,
    maximum: usize,
) {
    let characters = characters.into_iter().collect::<Vec<_>>();
    if characters.is_empty() {
        return;
    }
    if *selected_all {
        input.zeroize();
        *cursor = 0;
        *selected_all = false;
    }
    let remaining = maximum.saturating_sub(input.len());
    let characters = characters.into_iter().take(remaining).collect::<Vec<_>>();
    input.splice(*cursor..*cursor, characters.iter().copied());
    *cursor += characters.len();
}

/// 删除表单字段的一个字符（全选态清空整个字段）。
fn delete_field_character(
    input: &mut Vec<char>,
    cursor: &mut usize,
    selected_all: &mut bool,
    before_cursor: bool,
) {
    if *selected_all {
        input.zeroize();
        *cursor = 0;
        *selected_all = false;
    } else if before_cursor && *cursor > 0 {
        *cursor -= 1;
        input.remove(*cursor);
    } else if !before_cursor && *cursor < input.len() {
        input.remove(*cursor);
    }
}

/// 表单字段光标左移（全选态先取消全选）。
fn move_field_cursor_left(input: &[char], cursor: &mut usize, selected_all: &mut bool) {
    if *selected_all {
        *cursor = 0;
        *selected_all = false;
    } else {
        *cursor = cursor.saturating_sub(1);
    }
    *cursor = (*cursor).min(input.len());
}

/// 表单字段光标右移（全选态先取消全选）。
fn move_field_cursor_right(input: &[char], cursor: &mut usize, selected_all: &mut bool) {
    if *selected_all {
        *cursor = input.len();
        *selected_all = false;
    } else {
        *cursor = (*cursor + 1).min(input.len());
    }
}

/// 构造附件草稿：规范化路径（支持 `file://` 前缀与 Windows 盘符路径）、
/// 校验工作区边界/文件类型/大小。
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

/// 按扩展名判定附件类型（image/text/document），不支持的类型返回 None。
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

/// 解析 `/mode <read-only|full-access>` 形式的命令，格式非法返回 None。
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

/// 构造一个内置命令 SlashItem。
fn command(command: &str, detail: &str) -> SlashItem {
    SlashItem {
        kind: SlashKind::Command,
        command: command.to_owned(),
        detail: detail.to_owned(),
    }
}

/// 把消息列表裁剪到内存上限（丢弃最早的超额消息）。
fn cap_message_count(messages: &mut Vec<ChatMessage>) {
    let excess = messages.len().saturating_sub(MAX_LOADED_MESSAGES);
    if excess > 0 {
        messages.drain(..excess);
    }
}

/// Slash 使用统计文件路径（用户主目录下）。
fn slash_usage_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pisper").join("tui-slash-usage.json"))
}

/// 加载 Slash 使用统计（文件缺失或损坏时用空表）。
fn load_slash_usage() -> HashMap<String, SlashUsage> {
    slash_usage_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

/// 从 JSON 值中取字符串字段（缺失/非字符串返回空串）。
fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 同步显示文本到目标文本（流结束/工具切换时立即对齐）。
fn sync_live_display(live: &mut LiveTurn) {
    live.thinking.clone_from(&live.thinking_target);
    live.text.clone_from(&live.text_target);
}

/// 打字机步进：把 `shown` 向 `target` 推进一小步。
/// 目标改写（patch 替换）时先回退到公共前缀再继续；
/// 剩余量大时步长加大，保证长文本也能较快收敛。
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

/// 两个字符串的公共前缀字节数（按字符边界）。
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

/// 应用文本 patch：Runtime 以 UTF-16 code unit 报告 `start`，
/// 先把目标截断到该偏移再拼上新文本。
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

/// 把 UTF-16 code unit 偏移映射为字节偏移（按字符边界取整）。
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

/// 用事件负载更新工具活动（`done` 为真时写入结束时间与终态）。
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        advance_typewriter, apply_patch, attachment_draft, Action, App, Approval, AttachmentDraft,
        LiveTurn, SettingsPicker, SlashCategory, View, HISTORY_IDLE_EVICT_DELAY,
        HISTORY_KEEP_MESSAGES, INIT_PROMPT, MAX_LOADED_MESSAGES, PAGE_SCROLL_STEP,
    };
    use crate::model::{
        ChatMessage, ContextUsage, MessagePage, ModelOption, PageInfo, Plan, PlanCounts, PlanItem,
        ProviderOption, SessionCwdUpdate, SessionModelUpdate, SessionSummary, StreamEvent,
        ThinkingAvailability, ThinkingLevelUpdate, ToolDefinition, VcsChanges, VcsFile,
    };
    use serde_json::json;

    /// 验证文本补丁从指定位置替换文本尾部。
    #[test]
    fn text_patch_replaces_the_existing_tail() {
        let mut text = "hello wor".to_owned();
        apply_patch(&mut text, &json!({ "start": 6, "text": "world" }));
        assert_eq!(text, "hello world");
    }

    /// 验证补丁偏移按 JavaScript UTF-16 单位映射（多字节字符/代理对计数一致）。
    #[test]
    fn text_patch_maps_javascript_utf16_offsets() {
        let mut text = "你好😀 wor".to_owned();
        apply_patch(&mut text, &json!({ "start": 5, "text": "world" }));
        assert_eq!(text, "你好😀 world");
    }

    /// 验证打字机平滑大段 Unicode 增量并最终追上目标文本。
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

    /// 验证 session_usage 事件只替换活动会话的用量合计。
    #[test]
    fn session_usage_events_replace_only_the_active_session_totals() {
        let mut app = test_app(Vec::new());
        app.apply_stream_event(StreamEvent {
            name: "session_usage".to_owned(),
            data: json!({
                "input": 100,
                "output": 40,
                "cacheRead": 75,
                "cacheWrite": 25,
                "reasoning": 10,
                "totalTokens": 240,
                "promptTokens": 200,
                "requests": 2,
                "cacheHitRate": 37.5
            }),
        });
        assert_eq!(app.session_usage.total_tokens, 240);
        assert_eq!(app.session_usage.cache_hit_rate, Some(37.5));
    }

    /// 验证 meta 事件恢复运行时维护的会话用量快照。
    #[test]
    fn meta_events_restore_the_runtime_session_usage_snapshot() {
        let mut app = test_app(Vec::new());
        app.set_input("continue");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.apply_stream_event(StreamEvent {
            name: "meta".to_owned(),
            data: json!({
                "model": "provider/model",
                "cwd": "/workspace",
                "executionMode": "full-access",
                "thinkingLevel": "high",
                "sessionUsage": {
                    "input": 100,
                    "output": 40,
                    "cacheRead": 75,
                    "cacheWrite": 25,
                    "totalTokens": 240,
                    "promptTokens": 200,
                    "requests": 2,
                    "cacheHitRate": 37.5
                }
            }),
        });

        assert_eq!(app.session_usage.cache_read, 75);
        assert_eq!(app.session_usage.cache_hit_rate, Some(37.5));
    }

    /// 验证流事件先更新目标文本（text_target），渲染推进后才落到可见文本。
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

    /// 验证“减少动态效果”或已滚动离开底部时跳过增量显示，直接落到最终文本。
    #[test]
    fn reduced_motion_and_scrollback_skip_incremental_stream_reveal() {
        let mut reduced = test_app(Vec::new());
        reduced.reduced_motion = true;
        reduced.live = Some(LiveTurn {
            thinking_target: "Inspect the entire response".to_owned(),
            text_target: "Render it without animation".to_owned(),
            streaming: true,
            ..LiveTurn::default()
        });
        reduced.advance_stream_render();
        let live = reduced.live.as_ref().unwrap();
        assert_eq!(live.thinking, live.thinking_target);
        assert_eq!(live.text, live.text_target);
        assert_eq!(reduced.status_frame, 0);

        let mut scrolled = test_app(Vec::new());
        scrolled.scroll.set(1);
        scrolled.live = Some(LiveTurn {
            text_target: "Do not animate below the visible history".to_owned(),
            streaming: true,
            ..LiveTurn::default()
        });
        scrolled.advance_stream_render();
        let live = scrolled.live.as_ref().unwrap();
        assert_eq!(live.text, live.text_target);
    }

    /// 验证 /mode 命令只改会话执行模式，不触发 Agent 调用。
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

    /// 验证按键 Repeat 事件只编辑输入框，不重复触发命令/退出等动作。
    #[test]
    fn safe_key_repeats_edit_the_composer_without_repeating_commands() {
        let mut app = test_app(Vec::new());
        app.set_input("ab");
        let mut backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        backspace.kind = crossterm::event::KeyEventKind::Repeat;
        assert!(matches!(app.handle_key(backspace), Action::None));
        assert_eq!(app.input_text(), "a");

        let mut character = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        character.kind = crossterm::event::KeyEventKind::Repeat;
        app.handle_key(character);
        assert_eq!(app.input_text(), "ax");

        let mut quit = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        quit.kind = crossterm::event::KeyEventKind::Repeat;
        assert!(matches!(app.handle_key(quit), Action::None));
    }

    /// 验证 y/n 快捷键解析当前可见审批，且解决过程中/失败/成功状态流转正确。
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
            app.approval_scroll.set(3);
            app.approval_max_scroll.set(8);

            assert!(matches!(
                app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Action::ResolveApproval { id, approved: actual }
                    if id == "approval-1" && actual == approved
            ));
            assert!(app.approval.is_some());
            assert!(app.approval_is_resolving());
            assert!(matches!(
                app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Action::None
            ));
            app.approval_resolution_failed();
            assert!(!app.approval_is_resolving());
            app.approval_resolution_succeeded("approval-1");
            assert!(app.approval.is_none());
            assert_eq!(app.approval_scroll.get(), 0);
            assert_eq!(app.approval_max_scroll.get(), 0);
        }
    }

    /// 验证审批面板的键盘滚动（方向键/翻页/Home/End）有界且不越界。
    #[test]
    fn approval_command_supports_bounded_keyboard_scrolling() {
        let mut app = test_app(Vec::new());
        app.approval = Some(Approval {
            id: "approval-1".to_owned(),
            tool_name: "bash".to_owned(),
            args: json!({ "command": "one\ntwo\nthree" }),
            risk: "high".to_owned(),
            reason: "Runs as the current OS user.".to_owned(),
        });
        app.approval_max_scroll.set(12);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.approval_scroll.get(), 1);
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(app.approval_scroll.get() > 1);
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(app.approval_scroll.get(), 12);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.approval_scroll.get(), 12);
        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(app.approval_scroll.get() < 12);
        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.approval_scroll.get(), 0);
        assert!(app.approval.is_some());
    }

    /// 验证首次 Ctrl+C 中止流式/清除审批，再次 Ctrl+C 才退出。
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

    /// 验证流式中连续两次 Ctrl+C：第一次中止，第二次强制退出。
    #[test]
    fn a_second_ctrl_c_while_still_streaming_forces_a_quit() {
        let mut app = test_app(Vec::new());
        app.live = Some(LiveTurn {
            thinking: String::new(),
            thinking_target: String::new(),
            text: String::new(),
            text_target: String::new(),
            tools: Vec::new(),
            streaming: true,
        });

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Abort
        ));
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        ));
    }

    /// 验证运行结束后强制退出闩锁被复位（下次 Ctrl+C 不会误触发退出）。
    #[test]
    fn a_finished_run_resets_the_forced_quit_latch() {
        let mut app = test_app(Vec::new());
        app.live = Some(LiveTurn {
            thinking: String::new(),
            thinking_target: String::new(),
            text: String::new(),
            text_target: String::new(),
            tools: Vec::new(),
            streaming: true,
        });
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        app.stream_failed("disconnected".to_owned());
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        ));
    }

    /// 验证粘贴文本折叠展示但提交时保留完整原文。
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

    /// 验证多次独立粘贴在输入框中保持为独立的折叠块。
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

    /// 验证退格键删除整个折叠的粘贴块（而非逐字符）。
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

    /// 验证审批事件保留待审命令供用户复核（滚动位置归零，便于查看）。
    #[test]
    fn permission_events_keep_the_command_for_review() {
        let mut app = test_app(Vec::new());
        app.approval_scroll.set(4);
        app.approval_max_scroll.set(9);
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
        assert_eq!(app.approval_scroll.get(), 0);
        assert_eq!(app.approval_max_scroll.get(), 0);
    }

    /// 验证多个审批请求排队，按 id 逐个解决后从队列移除。
    #[test]
    fn permission_events_queue_requests_and_remove_resolved_items_by_id() {
        let mut app = test_app(Vec::new());
        app.set_input("run tools");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        for id in ["approval-1", "approval-2"] {
            app.apply_stream_event(StreamEvent {
                name: "permission_request".to_owned(),
                data: json!({
                    "id": id,
                    "toolName": "bash",
                    "args": { "command": id },
                    "risk": "high",
                    "reason": "Runs as the current OS user."
                }),
            });
        }

        assert_eq!(app.approval_count(), 2);
        assert_eq!(
            app.approval.as_ref().map(|item| item.id.as_str()),
            Some("approval-1")
        );
        assert!(app.approval_by_id("approval-2").is_some());
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            Action::ResolveApproval { id, approved: true } if id == "approval-1"
        ));
        assert!(app.approval_is_resolving());

        app.apply_stream_event(StreamEvent {
            name: "permission_resolved".to_owned(),
            data: json!({ "id": "approval-2", "approved": false }),
        });
        assert_eq!(app.approval_count(), 1);
        assert!(app.approval_is_resolving());
        assert_eq!(
            app.approval.as_ref().map(|item| item.id.as_str()),
            Some("approval-1")
        );

        app.apply_stream_event(StreamEvent {
            name: "permission_resolved".to_owned(),
            data: json!({ "id": "approval-1", "approved": true }),
        });
        assert_eq!(app.approval_count(), 0);
        assert!(!app.approval_is_resolving());
        assert!(app.approval.is_none());
    }

    /// 验证 /compact 只执行一次（压缩进行中再提交被拒绝），完成后更新上下文状态。
    #[test]
    fn compact_command_runs_once_and_updates_context_status() {
        let mut app = test_app(Vec::new());
        assert!(app
            .slash_items()
            .iter()
            .any(|item| item.command == "/compact"));
        app.set_input("/compact");
        assert!(matches!(app.submit_action(), Action::Compact));

        app.begin_context_compaction();
        app.set_input("/compact");
        assert!(matches!(app.submit_action(), Action::None));
        assert!(app.status_error);

        app.finish_context_compaction(
            Some(ContextUsage {
                percent: Some(12.5),
            }),
            None,
        );
        assert_eq!(app.context_percent, Some(12.5));
        assert_eq!(app.status, "context compacted");
        assert!(!app.compacting_context);
    }

    /// 验证 /init 以隐藏工作区指令提交，同时保留用户可见的命令行。
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
        assert!(!app
            .messages
            .last()
            .unwrap()
            .text
            .contains("Analyze this codebase"));
    }

    /// 验证只读模式下 /init 被拒绝（需要 full-access）。
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

    /// 验证 /工具名 命令请求所选运行时工具。
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

    /// 验证 Tab 补全高亮斜杠工具但不提交。
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

    /// 验证附件选择器限制在工作区内，选中文件以附件提交。
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

    /// 验证附件快捷键（Ctrl+O / Shift++）打开选择器但不丢弃输入框草稿。
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
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::SHIFT));
        assert!(app.path_picker);
        assert_eq!(app.input_text(), "");
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        app.handle_key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE));
        assert!(!app.path_picker);
        assert_eq!(app.input_text(), "@");
    }

    /// 验证切换模型时，已有消息的会话会询问是否顺带压缩上下文。
    #[test]
    fn model_switch_offers_context_compaction_for_existing_messages() {
        let mut app = test_app(Vec::new());
        app.messages.push(ChatMessage {
            role: "user".to_owned(),
            text: "hello".to_owned(),
            ..Default::default()
        });
        app.set_model(SessionModelUpdate {
            model: "provider/model-b".to_owned(),
            ..Default::default()
        });
        assert!(app.confirm_model_compaction);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            Action::Compact
        ));
        assert!(!app.confirm_model_compaction);
    }

    /// 验证空会话切换模型不询问压缩（无上下文可压）。
    #[test]
    fn model_switch_does_not_offer_compaction_for_empty_sessions() {
        let mut app = test_app(Vec::new());
        app.set_model(SessionModelUpdate {
            model: "provider/model-b".to_owned(),
            ..Default::default()
        });
        assert!(!app.confirm_model_compaction);
    }

    /// 验证 /model 与 /thinking 斜杠命令打开选择器并应用所选值。
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

    /// 验证思考强度空/错误状态不会回退到硬编码等级。
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

    /// 验证全局恢复会话与显式 /dir 切换都保留启动时的工作区作为新会话默认。
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

    /// 验证会话选择器加载中保持可见，过期请求结果被忽略。
    #[test]
    fn session_picker_keeps_loading_visible_and_ignores_stale_results() {
        let mut app = test_app(Vec::new());
        app.open_session_picker(false);
        let request_id = match app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
            Action::SwitchSession { id, request_id } => {
                assert_eq!(id, "session-1");
                request_id
            }
            _ => panic!("expected a session switch action"),
        };
        assert!(app.session_picker);
        assert_eq!(app.session_loading.as_deref(), Some("session-1"));

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.session_selected, 0);
        app.session_load_failed(request_id.wrapping_add(1), "session-1", "stale".to_owned());
        assert_eq!(app.session_loading.as_deref(), Some("session-1"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.session_picker);
        assert!(app.session_loading.is_none());
    }

    /// 验证当前会话加载失败时选择器保持打开并显示错误。
    #[test]
    fn current_session_load_failure_keeps_the_picker_open() {
        let mut app = test_app(Vec::new());
        app.open_session_picker(false);
        let request_id = match app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
            Action::SwitchSession { request_id, .. } => request_id,
            _ => panic!("expected a session switch action"),
        };

        app.session_load_failed(request_id, "session-1", "request timed out".to_owned());
        assert!(app.session_picker);
        assert!(app.session_loading.is_none());
        assert!(app.status_error);
        assert!(app.status.contains("request timed out"));
    }

    /// 验证启动恢复选择器在取消（Esc）时退出程序。
    #[test]
    fn startup_resume_picker_exits_on_cancel() {
        let mut app = test_app(Vec::new());
        app.open_session_picker(true);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Quit
        ));
    }

    /// 验证计划全部完成后清除计划状态，后续新计划能重新出现（含旧字段别名）。
    #[test]
    fn completed_plan_events_clear_state_and_later_plans_reappear() {
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

        app.plan_scroll.set(4);
        app.plan_max_scroll.set(8);
        app.apply_stream_event(StreamEvent {
            name: "plan_update".to_owned(),
            data: json!({
                "plan": {
                    "items": [{ "id": "one", "title": "Inspect", "status": "completed", "note": "Verified", "assignee": "agent", "dependsOn": [] }],
                    "counts": { "pending": 0, "inProgress": 0, "completed": 1, "blocked": 0, "total": 1 }
                }
            }),
        });
        assert!(app.session.plan.is_none());
        assert!(app.sessions[0].plan.is_none());
        assert_eq!(app.plan_scroll.get(), 0);
        assert_eq!(app.plan_max_scroll.get(), 0);

        app.apply_stream_event(StreamEvent {
            name: "plan_update".to_owned(),
            data: json!({
                "plan": {
                    "items": [{ "id": "two", "title": "Ship", "status": "pending", "note": "", "assignee": "", "dependsOn": [] }],
                    "counts": { "pending": 1, "inProgress": 0, "completed": 0, "blocked": 0, "total": 1 }
                }
            }),
        });
        assert_eq!(app.session.plan.as_ref().unwrap().items[0].id, "two");

        app.apply_stream_event(StreamEvent {
            name: "task_list_update".to_owned(),
            data: json!({ "taskList": null }),
        });
        assert!(app.session.plan.is_none());
    }

    /// 验证工具事件保留时间戳与子代理结果。
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

    /// 验证运行中提交的消息进入运行时队列（QueueInput），队列清空后不再有挂起动作。
    #[test]
    fn messages_submitted_during_a_run_are_queued_in_the_active_runtime() {
        let mut app = test_app(Vec::new());
        app.set_input("first");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.set_input("second");
        assert!(matches!(
            app.submit_action(),
            Action::QueueInput { message } if message == "second"
        ));
        assert_eq!(app.queued_count(), 0);

        app.queue_input_succeeded("second".to_owned(), 1);
        assert_eq!(app.queued_count(), 1);
        assert!(app.take_queued_action().is_none());
        assert_eq!(
            app.messages
                .iter()
                .filter(|message| message.role == "user")
                .count(),
            2
        );

        app.apply_stream_event(StreamEvent {
            name: "queue_update".to_owned(),
            data: json!({ "queuedInputs": [] }),
        });
        assert_eq!(app.queued_count(), 0);
        app.apply_stream_event(StreamEvent {
            name: "done".to_owned(),
            data: json!({ "text": "final answer", "tools": [], "contextUsage": {} }),
        });
        assert!(app.take_queued_action().is_none());
    }

    /// 验证运行时追加失败时恢复输入框草稿并提示错误。
    #[test]
    fn a_failed_runtime_append_restores_the_composer_draft() {
        let mut app = test_app(Vec::new());
        app.set_input("first");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        app.set_input("second");
        assert!(matches!(app.submit_action(), Action::QueueInput { .. }));

        app.queue_input_failed("second".to_owned(), "network unavailable".to_owned());
        assert_eq!(app.input_text(), "second");
        assert!(app.status_error);
    }

    /// 验证计划更新跟随当前项，Alt+方向键只滚动计划面板。
    #[test]
    fn plan_updates_follow_the_current_item_and_alt_arrows_scroll_the_plan_only() {
        let session = SessionSummary {
            id: "session-plan".to_owned(),
            plan: Some(Plan {
                items: (0..8)
                    .map(|index| PlanItem {
                        id: format!("item-{index}"),
                        title: format!("Task {index}"),
                        status: if index == 6 {
                            "in_progress".to_owned()
                        } else if index < 6 {
                            "completed".to_owned()
                        } else {
                            "pending".to_owned()
                        },
                        ..PlanItem::default()
                    })
                    .collect(),
                counts: PlanCounts {
                    completed: 6,
                    in_progress: 1,
                    pending: 1,
                    total: 8,
                    ..PlanCounts::default()
                },
                updated_at: None,
            }),
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
        assert_eq!(app.plan_scroll.get(), 6);
        app.plan_max_scroll.set(5);
        app.scroll.set(3);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
        assert_eq!(app.plan_scroll.get(), 5);
        assert_eq!(app.scroll.get(), 3);
        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::ALT));
        assert_eq!(app.plan_scroll.get(), 0);
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::ALT));
        assert_eq!(app.plan_scroll.get(), 5);
    }

    /// 验证聊天区方向键/翻页滚动不因流更新丢失位置，回到底部再滚出。
    #[test]
    fn chat_arrow_and_page_keys_scroll_without_losing_position_to_stream_updates() {
        let mut app = test_app(Vec::new());

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.scroll.get(), 1);
        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.scroll.get(), 1 + PAGE_SCROLL_STEP);

        app.live = Some(LiveTurn {
            streaming: true,
            ..LiveTurn::default()
        });
        app.apply_stream_event(StreamEvent {
            name: "text_delta".to_owned(),
            data: json!({ "delta": "new output" }),
        });
        assert_eq!(app.scroll.get(), 1 + PAGE_SCROLL_STEP);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.scroll.get(), 0);
    }

    /// 验证转录只保留最新一页消息（超出上限时从头部裁剪）。
    #[test]
    fn transcript_retains_only_the_latest_message_page() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: "/workspace".to_owned(),
            ..SessionSummary::default()
        };
        let messages = (0..MAX_LOADED_MESSAGES + 25)
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

        assert_eq!(app.messages.len(), MAX_LOADED_MESSAGES);
        assert_eq!(app.messages.first().unwrap().text, "message-25");
        app.set_input("next message");
        assert!(matches!(app.submit_action(), Action::Submit { .. }));
        assert_eq!(app.messages.len(), MAX_LOADED_MESSAGES);
        assert_eq!(app.messages.last().unwrap().text, "next message");
    }

    /// 验证滚动到顶部时只请求一次更早消息页（防重复触发）。
    #[test]
    fn scrolling_near_the_top_requests_an_older_page_once() {
        let mut app = test_app(Vec::new());
        app.set_history_window(40);
        app.render_max_scroll.set(12);

        let action = app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(matches!(action, Action::LoadOlderMessages { before: 40 }));

        let action = app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(matches!(action, Action::None));
    }

    /// 验证应用更早消息页后前置合并并关闭加载窗口。
    /// 验证更早页应用后不会重复合并（过期/陈旧页被忽略）。
    #[test]
    fn applying_an_older_page_prepends_and_closes_the_window() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: "/workspace".to_owned(),
            ..SessionSummary::default()
        };
        let recent = (40..80)
            .map(|index| ChatMessage {
                role: "agent".to_owned(),
                text: format!("message-{index}"),
                ..ChatMessage::default()
            })
            .collect();
        let mut app = App::new(
            vec![session.clone()],
            session,
            recent,
            None,
            Vec::new(),
            Vec::new(),
        );
        app.set_history_window(40);

        let older = (0..40)
            .map(|index| ChatMessage {
                role: "agent".to_owned(),
                text: format!("message-{index}"),
                ..ChatMessage::default()
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
            40,
        );

        assert_eq!(app.messages.len(), 80);
        assert_eq!(app.messages.first().unwrap().text, "message-0");
        assert!(!app.has_older_history());

        let stale = MessagePage {
            messages: vec![ChatMessage {
                role: "agent".to_owned(),
                text: "duplicate".to_owned(),
                ..ChatMessage::default()
            }],
            context_usage: None,
            session_usage: None,
            page_info: PageInfo::default(),
        };
        app.apply_history_page(stale, 40);
        assert_eq!(app.messages.len(), 80);
    }

    /// 验证闲置历史消息被驱逐以节省内存，之后仍可重新加载更早页。
    #[test]
    fn idle_history_eviction_keeps_recent_pages_and_allows_reloading() {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: "/workspace".to_owned(),
            ..SessionSummary::default()
        };
        let messages = (0..120)
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
        app.set_history_window(40);

        let now = Instant::now();
        app.history_touched_at = Some(now);
        assert!(!app.evict_idle_history(now));
        assert_eq!(app.messages.len(), 120);

        let later = now + HISTORY_IDLE_EVICT_DELAY + Duration::from_secs(1);
        assert!(app.evict_idle_history(later));
        assert_eq!(app.messages.len(), HISTORY_KEEP_MESSAGES);
        assert_eq!(app.messages.first().unwrap().text, "message-40");

        let action = app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(matches!(action, Action::LoadOlderMessages { before: 80 }));
    }

    /// 验证 Tab 补全内置命令但不执行（如 /quit 不退出）。
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

    /// 验证 Provider 对话框选择 Provider、屏蔽输入框（API 密钥遮罩）、支持取消。
    #[test]
    fn provider_dialog_selects_a_provider_masks_composer_input_and_supports_cancel() {
        let mut app = test_app(Vec::new());
        app.set_provider_options(vec![ProviderOption {
            id: "kimi-coding".to_owned(),
            name: "Kimi Code".to_owned(),
            provider_type: "chat".to_owned(),
            enabled: true,
            configured: false,
            api: "openai-responses".to_owned(),
            base_url: "https://api.kimi.com/coding/".to_owned(),
        }]);
        app.set_input("/provider");

        assert!(matches!(app.submit_action(), Action::None));
        assert!(app.api_key_dialog);
        assert!(!app.accepts_composer_input());
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::None
        ));
        assert_eq!(app.provider_api, "openai-responses");
        assert_eq!(
            app.provider_base_url_input.iter().collect::<String>(),
            "https://api.kimi.com/coding/"
        );
        assert_eq!(app.provider_connection_field, 2);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.provider_api, "openai-completions");
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.insert_paste("  secret- key\r\n");
        assert_eq!(app.input_text(), "");
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SaveProviderConnection { provider, api, base_url, api_key }
                if provider == "kimi-coding"
                    && api == "openai-responses"
                    && base_url == "https://api.kimi.com/coding/"
                    && api_key == "secret-key"
        ));

        app.provider_connection_save_failed("rejected".to_owned());
        assert!(app.api_key_input.is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.api_key_dialog);
    }

    /// 验证 /provider <name> 带参数时直接打开该 Provider 的凭据对话框。
    #[test]
    fn provider_command_with_an_argument_opens_that_provider_directly() {
        let mut app = test_app(Vec::new());
        app.set_provider_options(vec![
            ProviderOption {
                id: "openai".to_owned(),
                name: "OpenAI".to_owned(),
                provider_type: "chat".to_owned(),
                enabled: true,
                configured: false,
                api: "openai-responses".to_owned(),
                base_url: "https://api.openai.com/v1".to_owned(),
            },
            ProviderOption {
                id: "deepseek".to_owned(),
                name: "DeepSeek".to_owned(),
                provider_type: "chat".to_owned(),
                enabled: true,
                configured: false,
                api: "openai-completions".to_owned(),
                base_url: "https://api.deepseek.com".to_owned(),
            },
        ]);
        app.set_input("/provider deepseek");
        assert!(matches!(app.submit_action(), Action::None));
        assert!(app.api_key_dialog);
        assert_eq!(app.api_key_provider.as_deref(), Some("deepseek"));

        assert_eq!(app.provider_api, "openai-completions");
        for character in "sk-test".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SaveProviderConnection { provider, api, base_url, api_key }
                if provider == "deepseek"
                    && api == "openai-completions"
                    && base_url == "https://api.deepseek.com"
                    && api_key == "sk-test"
        ));
    }

    /// 验证 /provider 指向未知 Provider 时拒绝并提示错误。
    #[test]
    fn provider_command_rejects_an_unknown_provider() {
        let mut app = test_app(Vec::new());
        app.set_input("/provider does-not-exist");
        assert!(matches!(app.submit_action(), Action::None));
        assert!(!app.api_key_dialog);
        assert!(app.status_error);
        assert!(app.status.contains("provider not found"));
    }

    /// 验证 /apikey 别名仍打开 Provider 对话框（带/不带参数均可）。
    #[test]
    fn apikey_alias_still_opens_the_provider_dialog() {
        let mut app = test_app(Vec::new());
        app.set_provider_options(vec![ProviderOption {
            id: "openai".to_owned(),
            name: "OpenAI".to_owned(),
            provider_type: "chat".to_owned(),
            enabled: true,
            configured: false,
            api: "openai-responses".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
        }]);
        app.set_input("/apikey");
        assert!(matches!(app.submit_action(), Action::None));
        assert!(app.api_key_dialog);
        assert!(app.api_key_provider.is_none());

        app.set_input("/apikey openai");
        assert!(matches!(app.submit_action(), Action::None));
        assert!(app.api_key_dialog);
        assert_eq!(app.api_key_provider.as_deref(), Some("openai"));
    }

    /// 验证斜杠目录列出 Provider 与 Web 配置命令（/apikey 别名不显示）。
    #[test]
    fn slash_catalog_lists_provider_and_web_configuration_commands() {
        let app = test_app(Vec::new());
        let commands = app
            .slash_items()
            .into_iter()
            .map(|item| item.command)
            .collect::<Vec<_>>();
        assert!(commands.contains(&"/provider".to_owned()));
        assert!(commands.contains(&"/web".to_owned()));
        assert!(!commands.contains(&"/apikey".to_owned()));
    }

    /// 验证草稿会话默认值及实体化时保留首条待发送消息。
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

    /// 验证会话选择器过滤 Unicode 字段且支持光标处编辑。
    #[test]
    fn session_picker_filters_unicode_fields_and_edits_at_the_cursor() {
        let current = SessionSummary {
            id: "session-1".to_owned(),
            name: "Current".to_owned(),
            model: "openai/model".to_owned(),
            cwd: "/workspace/current".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        let target = SessionSummary {
            id: "session-2".to_owned(),
            name: "中文 session".to_owned(),
            model: "deepseek/chat".to_owned(),
            cwd: "/workspace/other".to_owned(),
            execution_mode: "read-only".to_owned(),
            ..SessionSummary::default()
        };
        let mut app = App::new(
            vec![current.clone(), target],
            current,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        );
        app.open_session_picker(false);
        for character in "中文".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.visible_sessions()[0].id, "session-2");

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('会'), KeyModifiers::NONE));
        assert_eq!(app.session_query.iter().collect::<String>(), "中会文");
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.session_query.iter().collect::<String>(), "中文");
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(app.session_query.iter().collect::<String>(), "中文");
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::SwitchSession { id, .. } if id == "session-2"
        ));
    }

    /// 验证选择器查询有长度上限且 Ctrl+U 一键清空。
    #[test]
    fn session_picker_caps_and_clears_the_query() {
        let mut app = test_app(Vec::new());
        app.open_session_picker(false);
        for _ in 0..300 {
            app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        }
        assert_eq!(app.session_query.len(), 256);
        assert_eq!(app.session_query_cursor, 256);

        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(app.session_query.is_empty());
        assert_eq!(app.session_query_cursor, 0);
        assert_eq!(app.session_selected, 0);
    }

    /// 验证 Provider 连接字段支持光标编辑与全选（Ctrl+A 后输入替换）。
    #[test]
    fn provider_connection_fields_support_cursor_editing_and_select_all() {
        let mut app = test_app(Vec::new());
        app.set_provider_options(vec![ProviderOption {
            id: "custom".to_owned(),
            name: "Custom".to_owned(),
            provider_type: "chat".to_owned(),
            enabled: true,
            configured: true,
            api: "openai-responses".to_owned(),
            base_url: "https://old.example/v1".to_owned(),
        }]);
        app.open_api_key_dialog();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        for character in "https://new.example/v1".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(
            app.provider_base_url_input.iter().collect::<String>(),
            "ttps://new.example/v1"
        );
        assert_eq!(app.provider_base_url_cursor, 0);
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(
            app.provider_base_url_input.iter().collect::<String>(),
            "ttps://new.example/v1"
        );

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(app.api_key_input.iter().collect::<String>(), "k");
    }

    /// 验证附件选择器聚焦时删除已选附件。
    #[test]
    fn attachment_picker_focus_removes_the_selected_attachment() {
        let mut app = test_app(Vec::new());
        app.attachments = vec![
            AttachmentDraft {
                path: "/workspace/one.txt".into(),
                name: "one.txt".to_owned(),
                kind: "text".to_owned(),
                size: 10,
            },
            AttachmentDraft {
                path: "/workspace/two.txt".into(),
                name: "two.txt".to_owned(),
                kind: "text".to_owned(),
                size: 20,
            },
        ];
        app.open_path_picker();
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.attachment_list_focused);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(app.attachments.len(), 1);
        assert_eq!(app.attachments[0].name, "one.txt");
        app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert!(app.attachments.is_empty());
        assert!(!app.attachment_list_focused);
    }

    /// 验证变更视图在文件间移动并跳到对应 diff 位置。
    #[test]
    fn changes_view_moves_between_files_and_jumps_to_their_diff() {
        let mut app = test_app(Vec::new());
        app.view = View::Changes;
        app.vcs = Some(VcsChanges {
            vcs: "git".to_owned(),
            files: vec![
                VcsFile {
                    path: "src/one.rs".to_owned(),
                    status: "M".to_owned(),
                },
                VcsFile {
                    path: "src/two.rs".to_owned(),
                    status: "A".to_owned(),
                },
            ],
            diff: "diff --git a/src/one.rs b/src/one.rs\n+one\ndiff --git a/src/two.rs b/src/two.rs\n+two\n"
                .to_owned(),
            ..VcsChanges::default()
        });
        app.vcs_max_scroll.set(20);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.vcs_selected, 1);
        assert_eq!(app.vcs_scroll.get(), 2);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.vcs_selected, 0);
        assert_eq!(app.vcs_scroll.get(), 0);
    }

    /// 验证斜杠分类循环（右/左箭头）不影响 Tab 补全。
    #[test]
    fn slash_categories_cycle_without_replacing_tab_completion() {
        let mut app = test_app(vec![ToolDefinition {
            id: "read".to_owned(),
            name: "Read".to_owned(),
            description: "Read a file".to_owned(),
            enabled: true,
        }]);
        app.set_input("/");
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.slash_category, SlashCategory::Tools);
        assert!(app
            .slash_items()
            .iter()
            .all(|item| item.kind == super::SlashKind::Tool));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.slash_category, SlashCategory::All);

        app.set_input("/rea");
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.input_text(), "/read ");
    }

    /// 构造标准测试 App（一个固定会话 + 给定工具，其余字段取默认）。
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
