//! TUI 程序入口：CLI 解析、sidecar 启动、事件循环与异步动作调度。
//!
//! 结构：`main` → `run`（启动 sidecar、初始化 App）→ `run_event_loop`
//! （select 输入事件 / Runtime 消息 / 定时器）。所有耗时的 API 调用都以
//! tokio 任务 + mpsc 消息的方式异步完成，事件循环本身不做阻塞 IO。

mod api;
mod app;
mod component_update;
mod model;
mod notification;
mod paste_burst;
mod plan_protocol;
mod sidecar;
mod ui;
mod workspace;

use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use app::{Action, App};
use crossterm::{
    cursor::MoveTo,
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, Clear, ClearType,
        EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use futures_util::StreamExt;
use model::{RuntimeEvent, SessionSummary};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::Rect,
    Terminal,
};
use tokio::{sync::mpsc, time::Instant};
use zeroize::Zeroize;

use crate::{
    api::ApiClient,
    paste_burst::{CharDecision, FlushResult, PasteBurst},
    sidecar::SidecarConnection,
    workspace::{canonical_workspace, same_workspace, validate_session_workspace},
};

// 会话加载请求的超时：历史消息加载是用户可见操作，不能无限等待。
const SESSION_LOAD_TIMEOUT: Duration = Duration::from_secs(15);

const CLI_HELP: &str = "Pisper CLI

Start a coding session in your terminal.

Usage:
  pisper [OPTIONS]
  pisper resume [OPTIONS]
  pisper doctor [OPTIONS]
  pisper web [OPTIONS]
  pisper help [COMMAND]

Commands:
  resume    Choose and resume a conversation from any workspace
  doctor    Check the TUI, Runtime connection, and capability catalogs
  web       Open Provider settings in your browser
  help      Print this help or help for a command

Options:
  --cwd <directory>  Use a specific workspace (default: current directory)
  -h, --help         Print help
  -V, --version      Print the installed TUI version

Getting started:
  1. Change to your project directory.
  2. Run `pisper`. Use `/provider` to choose a Provider and save its API Key in the terminal.
  3. Type a request and press Enter. Type `/` to browse commands.
  4. Run `pisper web` for the optional visual settings and workspace UI.
  5. Press Ctrl+C to stop a running Agent, or press it while idle to exit.

Examples:
  pisper
  pisper --cwd /path/to/project
  pisper resume
  pisper doctor
  pisper web
  pisper help web";

const WEB_HELP: &str = "Pisper Web UI

Open the bundled or installed Web frontend and Provider settings in your default browser.
The local Runtime remains bound to 127.0.0.1 and browser access uses a one-time bootstrap URL.

Usage:
  pisper web [--cwd <directory>]

Options:
  --cwd <directory>  Use a specific workspace (default: current directory)
  -h, --help         Print Web UI help

Examples:
  pisper web
  pisper web --cwd /path/to/project";

/// 程序入口：执行启动流程，任何错误以非零退出码终止。
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("pisper: {error:#}");
        std::process::exit(1);
    }
}

/// 启动流程：解析参数 → 按需安装组件 → 启动 sidecar →
/// `web`/`doctor` 子命令直接执行并退出，否则初始化 App 进入交互事件循环。
async fn run() -> Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Some(help) = requested_help(&arguments) {
        println!("{help}");
        return Ok(());
    }
    let options = launch_options()?;
    if options.web && sidecar::configured_frontend_root().is_none() {
        component_update::ensure_web().await?;
    }
    if sidecar::needs_runtime_install() {
        component_update::ensure_runtime().await?;
    }
    let mut sidecar = match SidecarConnection::start(&options.workspace) {
        Ok(sidecar) => sidecar,
        Err(error) if sidecar::needs_runtime_install() => {
            eprintln!(
                "Pisper runtime startup failed; reinstalling the signed component: {error:#}"
            );
            component_update::ensure_runtime().await?;
            SidecarConnection::start(&options.workspace)?
        }
        Err(error) => return Err(error),
    };
    let api = ApiClient::new(&sidecar.url, &sidecar.token)?;
    if options.web {
        webbrowser::open(&api.bootstrap_url("/config")?)
            .context("failed to open the default browser")?;
    }
    if options.doctor {
        let sessions = api
            .sessions()
            .await
            .context("failed to list conversations")?;
        let diagnostics = api
            .runtime_diagnostics()
            .await
            .context("failed to load runtime diagnostics")?;
        let (tools, skills) = api
            .catalogs()
            .await
            .context("failed to load Slash catalog")?;
        let runtime_workspace = diagnostics
            .get("workspaceCwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let matching_session = sessions
            .iter()
            .find(|session| same_workspace(&session.cwd, &options.workspace))
            .map(|session| session.cwd.as_str())
            .unwrap_or("none");
        println!(
            "Pisper TUI ready\n  connection: {}\n  launch workspace: {}\n  runtime fallback: {}\n  matching session: {}\n  catalogs: {} conversations · {} tools · {} skills",
            sidecar.kind.label(),
            options.workspace.display(),
            runtime_workspace,
            matching_session,
            sessions.len(),
            tools.len(),
            skills.len()
        );
        sidecar.shutdown();
        return Ok(());
    }
    let sessions = api
        .sessions()
        .await
        .context("failed to list conversations")?;
    if options.resume && sessions.is_empty() {
        println!("No conversations are available to resume.");
        sidecar.shutdown();
        return Ok(());
    }
    let interactive_resume = options.resume;
    let mut session = resume_seed(&sessions, options.resume)
        .unwrap_or_else(|| draft_session(&options.workspace, "", ""));
    if !interactive_resume {
        validate_session_workspace(&session, None)?;
    }
    let (messages, context_usage, session_usage, thinking_state, history_start) =
        if interactive_resume || session.id.is_empty() {
            (Vec::new(), None, None, None, 0)
        } else {
            let (thinking_state, page) =
                tokio::join!(api.thinking_state(&session.id), api.messages(&session.id));
            if let Ok(state) = &thinking_state {
                if !state.thinking_level.is_empty() {
                    session.thinking_level.clone_from(&state.thinking_level);
                }
            }
            let page = page?;
            (
                page.messages,
                page.context_usage,
                page.session_usage,
                Some(thinking_state),
                page.page_info.start,
            )
        };
    let mut app = App::new(
        sessions,
        session,
        messages,
        context_usage,
        Vec::new(),
        Vec::new(),
    );
    app.set_history_window(history_start);
    if let Some(usage) = session_usage {
        app.set_session_usage(usage);
    }
    app.set_launch_workspace(options.workspace.clone());
    if options.web {
        app.status = "Web settings opened in the default browser".to_owned();
    }
    if interactive_resume {
        app.open_session_picker(true);
    } else if let Some(thinking_state) = thinking_state {
        match thinking_state {
            Ok(state) => app.set_thinking_state(state),
            Err(error) => app.set_thinking_error(format!("{error:#}")),
        }
    }

    let mut terminal = TerminalSession::start()?;
    let result = run_event_loop(&mut terminal.terminal, app, api).await;
    terminal.restore();
    sidecar.shutdown();
    result
}

/// 核心事件循环：每轮先按需重绘，然后 select 三类输入——
/// 键盘/粘贴/缩放事件、Runtime 异步消息、各类定时器（粘贴 flush、动画、历史回收）。
/// 返回值表示是否退出循环。
async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
    api: ApiClient,
) -> Result<()> {
    let mut input = EventStream::new();
    let (runtime_tx, mut runtime_rx) = mpsc::unbounded_channel::<RuntimeEvent>();
    // 启动数据（偏好/目录）与交互输入并行加载，首帧无需等待网络。
    let startup_api = api.clone();
    let startup_sender = runtime_tx.clone();
    tokio::spawn(async move {
        let (preferences, catalogs) =
            tokio::join!(startup_api.runtime_preferences(), startup_api.catalogs());
        let (default_model, thinking_level, model_options, provider_options) =
            preferences.unwrap_or_else(|_| (String::new(), String::new(), Vec::new(), Vec::new()));
        let (tools, skills) = catalogs.unwrap_or_default();
        let _ = startup_sender.send(RuntimeEvent::StartupData {
            default_model,
            thinking_level,
            model_options,
            provider_options,
            tools,
            skills,
        });
    });
    // 60ms 流式打字机动画；错过 tick 直接跳过，避免长时间卡顿后连播追帧。
    let mut animation = tokio::time::interval(Duration::from_millis(60));
    animation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 状态栏扫描动画（80ms）。
    let mut status_animation = tokio::time::interval(Duration::from_millis(80));
    status_animation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 历史消息空闲回收检查（5s 一次）。
    let mut history_eviction = tokio::time::interval(Duration::from_secs(5));
    history_eviction.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // JetBrains 终端不渲染末列（光标列），需整体让出一列，详见 `is_jetbrains_terminal`。
    let jetbrains_terminal = is_jetbrains_terminal();
    let mut redraw = true;
    let mut pending_resize = None;
    let mut last_resize_at = None;
    let mut resize_to_draw = None;
    let mut paste_burst = PasteBurst::default();
    // 已通知过「等待审批」的请求 id 集合：同一审批只弹一次通知，避免重复打扰。
    let mut notified_approval_ids = HashSet::new();
    loop {
        if redraw {
            draw_frame(terminal, &app, resize_to_draw.take(), jetbrains_terminal)?;
        }
        // 粘贴缓冲有截止时间时，把它并入 select 的睡眠分支，到时自动 flush。
        let paste_burst_deadline = paste_burst.deadline();
        redraw = tokio::select! {
            maybe_event = input.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if handle_key_with_paste_burst(
                            key,
                            &mut paste_burst,
                            &mut app,
                            &api,
                            &runtime_tx,
                        )
                        .await?
                        {
                            break;
                        }
                        pending_resize.is_none() && !paste_burst.is_buffering()
                    }
                    Some(Ok(Event::Paste(value))) => {
                        // bracketed-paste 事件：先取出突发缓冲，再插入粘贴内容。
                        apply_paste_flush(paste_burst.flush(), &mut app);
                        app.insert_paste(&value);
                        paste_burst.clear_after_explicit_paste();
                        pending_resize.is_none()
                    }
                    Some(Ok(Event::Resize(width, height))) => {
                        // 终端缩放会产生连续事件，先记录区域并等待其稳定后再重绘。
                        if let Some(area) = resize_area(width, height) {
                            pending_resize = Some(area);
                            last_resize_at = Some(Instant::now());
                        }
                        false
                    },
                    Some(Ok(_)) => false,
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                }
            }
            maybe_runtime = runtime_rx.recv() => {
                match maybe_runtime {
                    Some(runtime_event) => {
                        let terminal_event = match runtime_event {
                            RuntimeEvent::StartupData {
                                default_model,
                                thinking_level,
                                model_options,
                                provider_options,
                                tools,
                                skills,
                            } => {
                                app.set_startup_data(
                                    default_model,
                                    thinking_level,
                                    model_options,
                                    provider_options,
                                    tools,
                                    skills,
                                );
                                false
                            }
                            RuntimeEvent::Stream(event) => {
                                // 终态事件（done/error）时本分支返回 true，触发后续收尾。
                                let terminal = matches!(event.name.as_str(), "done" | "error");
                                let completed = event.name == "done";
                                // 审批事件的去重与通知：请求到达时弹一次，解析后移除。
                                let approval_event = matches!(
                                    event.name.as_str(),
                                    "permission_request" | "permission_resolved"
                                )
                                .then(|| {
                                    (
                                        event.name.clone(),
                                        event.data["id"].as_str().unwrap_or_default().to_owned(),
                                    )
                                });
                                app.apply_stream_event(event);
                                if let Some((name, approval_id)) = approval_event {
                                    if name == "permission_resolved" {
                                        notified_approval_ids.remove(&approval_id);
                                    } else if !approval_id.is_empty()
                                        && notified_approval_ids.insert(approval_id.clone())
                                    {
                                        if let Some(approval) = app.approval_by_id(&approval_id) {
                                            spawn_waiting_notification(
                                                &api,
                                                notification::chat_waiting(&app, approval),
                                            );
                                        }
                                    }
                                }
                                if should_notify_completion(completed, app.queued_count()) {
                                    spawn_completion_notification(
                                        &api,
                                        notification::chat_completion(&app),
                                    );
                                }
                                if terminal {
                                    notified_approval_ids.clear();
                                }
                                terminal
                            }
                            RuntimeEvent::StreamFailed(message) => {
                                app.stream_failed(message);
                                true
                            }
                            RuntimeEvent::QueueInputFinished {
                                session_id,
                                message,
                                result,
                            } => {
                                if app.session.id != session_id {
                                    false
                                } else {
                                    match result {
                                        Ok(queued_count) => {
                                            app.queue_input_succeeded(message, queued_count);
                                            false
                                        }
                                        Err(error) if is_ended_session_queue_error(&error) => {
                                            app.defer_input_after_run(message);
                                            !app.is_streaming()
                                        }
                                        Err(error) => {
                                            app.queue_input_failed(message, error);
                                            false
                                        }
                                    }
                                }
                            }
                            RuntimeEvent::AbortFinished { session_id, result } => {
                                if app.session.id == session_id {
                                    match result {
                                        Ok(()) if app.is_streaming() => {
                                            app.status = "stopping".to_owned();
                                            app.status_error = false;
                                        }
                                        Ok(()) => {}
                                        Err(error) if app.is_streaming() => {
                                            app.status = format!("abort failed · {error}");
                                            app.status_error = true;
                                        }
                                        Err(_) => {}
                                    }
                                }
                                false
                            }
                            RuntimeEvent::ExecutionModeFinished { session_id, result } => {
                                if app.session.id == session_id {
                                    match result {
                                        Ok(updated) => {
                                            app.set_execution_mode(updated.execution_mode)
                                        }
                                        Err(error) => {
                                            app.status = format!("mode change failed · {error}");
                                            app.status_error = true;
                                        }
                                    }
                                }
                                false
                            }
                            RuntimeEvent::HistoryPage { before, result } => {
                                match result {
                                    Ok(page) => app.apply_history_page(page, before),
                                    Err(error) => app.history_load_failed(error),
                                }
                                true
                            }
                            RuntimeEvent::SessionLoaded {
                                request_id,
                                session,
                                result,
                            } => {
                                if app.is_current_session_load(request_id, &session.id) {
                                    match result {
                                        Ok(page) => {
                                            let history_start = page.page_info.start;
                                            let session_usage = page.session_usage.clone();
                                            app.replace_session(
                                                *session,
                                                page.messages,
                                                page.context_usage,
                                            );
                                            app.set_history_window(history_start);
                                            if let Some(usage) = session_usage {
                                                app.set_session_usage(usage);
                                            }
                                            app.begin_thinking_load();
                                            spawn_session_thinking_load(
                                                &api,
                                                app.session.id.clone(),
                                                &runtime_tx,
                                            );
                                        }
                                        Err(error) => app.session_load_failed(
                                            request_id,
                                            &session.id,
                                            error,
                                        ),
                                    }
                                }
                                false
                            }
                            RuntimeEvent::SessionThinkingLoaded { session_id, result } => {
                                if app.session.id == session_id {
                                    match result {
                                        Ok(state) => app.set_thinking_state(state),
                                        Err(error) => app.set_thinking_error(error),
                                    }
                                }
                                false
                            }
                            RuntimeEvent::CompactionFinished {
                                context_usage,
                                error,
                            } => {
                                app.finish_context_compaction(context_usage, error);
                                false
                            }
                            RuntimeEvent::ApprovalResolved {
                                session_id,
                                approval_id,
                                approved,
                                result,
                            } => {
                                if app.session.id == session_id {
                                    match result {
                                        Ok(()) => {
                                            app.approval_resolution_succeeded(&approval_id);
                                            app.status = if approved {
                                                "approved"
                                            } else {
                                                "denied"
                                            }
                                            .to_owned();
                                            app.status_error = false;
                                        }
                                        Err(error) => {
                                            app.approval_resolution_failed();
                                            app.status = format!("approval failed · {error}");
                                            app.status_error = true;
                                        }
                                    }
                                }
                                false
                            }
                            RuntimeEvent::VcsResult { session_id, result } => {
                                if app.session.id == session_id {
                                    match result {
                                        Ok(changes) => app.set_vcs(changes),
                                        Err(error) => app.set_vcs_error(error),
                                    }
                                }
                                false
                            }
                        };
                        if terminal_event {
                            if let Some(action) = app.take_queued_action() {
                                match execute_action(action, &mut app, &api, &runtime_tx).await {
                                    Ok(true) => break,
                                    Ok(false) => {}
                                    Err(error) => {
                                        app.stream_failed(format!("{error:#}"));
                                    }
                                }
                            }
                        }
                        pending_resize.is_none()
                    }
                    None => false,
                }
            }
            _ = tokio::time::sleep_until(
                paste_burst_deadline.unwrap_or_else(Instant::now)
            ), if paste_burst_deadline.is_some() => {
                apply_paste_flush(paste_burst.flush_if_due(Instant::now()), &mut app);
                pending_resize.is_none()
            }
            _ = animation.tick(), if app.has_pending_render() || pending_resize.is_some() => {
                let resize_settled = pending_resize.is_some()
                    && last_resize_at.is_some_and(|last| {
                        resize_has_settled(last, Instant::now())
                    });
                if resize_settled {
                    resize_to_draw = pending_resize.take();
                    last_resize_at = None;
                }
                if app.has_pending_render() && pending_resize.is_none() {
                    app.advance_stream_render();
                }
                resize_settled || pending_resize.is_none()
            }
            _ = status_animation.tick(), if app.is_running_state() && !app.has_pending_render() && !app.reduced_motion() && pending_resize.is_none() => {
                app.advance_status_animation();
                true
            }
            _ = history_eviction.tick() => {
                app.evict_idle_history(Instant::now().into())
            }
        };
    }
    Ok(())
}

/// 统一换行符：剪贴板文本可能带 CRLF 或 CR，全部归一为 `\n`。
fn normalize_clipboard_text(text: impl AsRef<str>) -> String {
    text.as_ref().replace("\r\n", "\n").replace('\r', "\n")
}

/// 键盘入口：先处理粘贴突发（显式粘贴快捷键、突发捕获），
/// 其余按键交给 App 产生 Action 再异步执行。
async fn handle_key_with_paste_burst(
    key: KeyEvent,
    paste_burst: &mut PasteBurst,
    app: &mut App,
    api: &ApiClient,
    runtime_tx: &mpsc::UnboundedSender<RuntimeEvent>,
) -> Result<bool> {
    if !should_handle_key_kind(key.kind) {
        return Ok(false);
    }

    let composer_active = app.accepts_composer_input() && !app.slash_open();
    let now = Instant::now();

    apply_paste_flush(paste_burst.flush_if_due(now), app);

    if composer_active && key.kind == KeyEventKind::Press && is_paste_shortcut(&key) {
        apply_paste_flush(paste_burst.flush(), app);
        if let Some(text) = read_clipboard_text().map(normalize_clipboard_text) {
            app.insert_paste(&text);
            paste_burst.clear_after_explicit_paste();
        } else {
            paste_burst.arm();
        }
        return Ok(false);
    }

    if composer_active
        && key.kind == KeyEventKind::Press
        && is_inline_attachment_shortcut(
            &key,
            app.input_text().is_empty(),
            paste_burst.is_buffering(),
        )
    {
        let action = app.handle_key(key);
        return execute_action(action, app, api, runtime_tx).await;
    }

    if composer_active && key.kind == KeyEventKind::Repeat {
        apply_paste_flush(paste_burst.flush(), app);
        let action = app.handle_key(key);
        return execute_action(action, app, api, runtime_tx).await;
    }

    if composer_active {
        if key.code == KeyCode::Esc {
            paste_burst.cancel();
        } else if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Char(character) => match paste_burst.on_char(character, now) {
                    CharDecision::RetainFirst => return Ok(false),
                    CharDecision::BeginFromPending | CharDecision::Append => {
                        paste_burst.append_char(character, now);
                        return Ok(false);
                    }
                },
                KeyCode::Enter => {
                    if paste_burst.append_newline_if_active(now) {
                        return Ok(false);
                    }
                }
                KeyCode::Tab if paste_burst.try_append_char_if_active('\t', now) => {
                    return Ok(false);
                }
                _ => {}
            }
        }
    }

    apply_paste_flush(paste_burst.flush(), app);
    let action = app.handle_key(key);
    execute_action(action, app, api, runtime_tx).await
}

/// 只处理按下与重复两类按键事件，释放事件一律忽略。
fn should_handle_key_kind(kind: KeyEventKind) -> bool {
    matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

/// 粘贴快捷键：Ctrl+V / Ctrl+Shift+V 或 Shift+Insert。
fn is_paste_shortcut(key: &KeyEvent) -> bool {
    (matches!(key.code, KeyCode::Char('v' | 'V')) && key.modifiers.contains(KeyModifiers::CONTROL))
        || (key.code == KeyCode::Insert && key.modifiers.contains(KeyModifiers::SHIFT))
}

/// 内联附件快捷键：输入为空、未在粘贴缓冲时按 `+` 打开附件选择器。
fn is_inline_attachment_shortcut(
    key: &KeyEvent,
    composer_empty: bool,
    paste_buffering: bool,
) -> bool {
    key.code == KeyCode::Char('+')
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && composer_empty
        && !paste_buffering
}

/// 读剪贴板：仅 Windows 支持直读（通过 win32 API）；
/// 其他平台依赖终端自身的 bracketed-paste，返回 None。
#[cfg(windows)]
fn read_clipboard_text() -> Option<String> {
    clipboard_win::get_clipboard(clipboard_win::formats::Unicode)
        .ok()
        .filter(|text: &String| !text.is_empty())
}

/// 非 Windows 平台不提供剪贴板直读（返回 None，粘贴走终端 bracketed-paste）。
#[cfg(not(windows))]
fn read_clipboard_text() -> Option<String> {
    None
}

/// 把粘贴 flush 结果应用到输入框：Paste 折叠显示，Typed 普通插入。
fn apply_paste_flush(result: FlushResult, app: &mut App) {
    match result {
        FlushResult::Paste(text) => app.insert_detected_paste(&text),
        FlushResult::Typed(text) => app.insert_paste(&text),
        FlushResult::None => {}
    }
}

/// 是否应发「对话完成」通知：完成且没有排队消息时才发。
fn should_notify_completion(completed: bool, queued_count: usize) -> bool {
    completed && queued_count == 0
}

/// 识别「会话已结束」类排队错误：运行结束瞬间提交的消息会被 Runtime 拒绝，
/// 此时把消息留到下一轮运行（defer）而不是直接失败。
fn is_ended_session_queue_error(error: &str) -> bool {
    let normalized = error.to_lowercase();
    normalized.contains("当前会话已经结束运行")
        || (normalized.contains("session")
            && ["ended", "finished", "no longer running"]
                .iter()
                .any(|value| normalized.contains(value)))
}

/// 异步发送「等待审批」通知；系统通知开关开启时再弹本地系统通知。
fn spawn_waiting_notification(api: &ApiClient, waiting: notification::ChatWaiting) {
    let api = api.clone();
    tokio::spawn(async move {
        let Ok(dispatch) = api
            .notify_chat_waiting(
                &waiting.title,
                &waiting.tool,
                &waiting.reason,
                &waiting.model,
            )
            .await
        else {
            return;
        };
        if dispatch.system_notification_enabled {
            let title = format!("{} · Waiting for confirmation", waiting.title);
            let body = format!("{} requires confirmation. {}", waiting.tool, waiting.reason);
            tokio::task::spawn_blocking(move || notification::show_system(&title, &body));
        }
        let _ = dispatch.channel_error;
    });
}

/// 异步发送「对话完成」通知。
fn spawn_completion_notification(api: &ApiClient, completion: notification::ChatCompletion) {
    let api = api.clone();
    tokio::spawn(async move {
        let Ok(dispatch) = api
            .notify_chat_completed(&completion.title, &completion.summary, &completion.model)
            .await
        else {
            return;
        };
        if dispatch.system_notification_enabled {
            let title = format!("{} · Agent completed", completion.title);
            let body = completion.summary;
            tokio::task::spawn_blocking(move || notification::show_system(&title, &body));
        }
        let _ = dispatch.channel_error;
    });
}

/// 尺寸 → 绘制区域（0 尺寸无效）。
fn resize_area(width: u16, height: u16) -> Option<Rect> {
    (width > 0 && height > 0).then(|| Rect::new(0, 0, width, height))
}

/// 缩放是否已稳定：距最后一次缩放事件至少 80ms 才认为稳定，
/// 避免缩放过程中反复 resize 抖动。
fn resize_has_settled(last_resize_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_resize_at) >= Duration::from_millis(80)
}

/// JetBrains 终端检测：其渲染器保留最后一列，需让出一列避免换行闪动。
fn is_jetbrains_terminal() -> bool {
    ["TERMINAL_EMULATOR", "TERM_PROGRAM"]
        .into_iter()
        .filter_map(|name| env::var(name).ok())
        .any(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("jetbrains") || value.contains("jediterm")
        })
}

/// 计算实际可用内容区：JetBrains 终端宽度减一。
fn terminal_content_area(area: Rect, jetbrains_terminal: bool) -> Rect {
    if jetbrains_terminal && area.width > 1 {
        Rect::new(area.x, area.y, area.width - 1, area.height)
    } else {
        area
    }
}

/// 同步终端尺寸并清屏（缩放稳定后的完整重绘路径）。
fn synchronize_terminal_size<B>(terminal: &mut Terminal<B>, area: Rect) -> Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    terminal.resize(area)?;
    terminal.clear()?;
    Ok(())
}

/// 绘制一帧：非 JetBrains 终端用 synchronized update 包裹，
/// 避免多段输出在慢终端上被拆成可见的多次刷新。
fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &App,
    resize: Option<Rect>,
    jetbrains_terminal: bool,
) -> Result<()> {
    if !jetbrains_terminal {
        execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    }
    let draw_result = (|| -> Result<()> {
        if let Some(area) = resize {
            execute!(terminal.backend_mut(), MoveTo(0, 0), Clear(ClearType::All))?;
            synchronize_terminal_size(terminal, area)?;
        }
        terminal
            .draw(|frame| {
                let area = terminal_content_area(frame.area(), jetbrains_terminal);
                ui::draw_in(frame, app, area);
            })
            .map(drop)?;
        Ok(())
    })();
    let end_result = (!jetbrains_terminal)
        .then(|| execute!(terminal.backend_mut(), EndSynchronizedUpdate))
        .transpose();
    draw_result?;
    end_result?;
    Ok(())
}

/// VCS 请求类型：刷新 / 提交（带消息）/ 推送 / 回退。
enum VcsRequest {
    Refresh,
    Commit(String),
    Push,
    Revert,
}

/// 后台加载会话历史，超时与错误统一转为消息发回主循环。
fn spawn_session_load(
    api: &ApiClient,
    request_id: u64,
    session: SessionSummary,
    sender: &mpsc::UnboundedSender<RuntimeEvent>,
) {
    let api = api.clone();
    let sender = sender.clone();
    tokio::spawn(async move {
        let result =
            match tokio::time::timeout(SESSION_LOAD_TIMEOUT, api.messages(&session.id)).await {
                Ok(Ok(page)) => Ok(page),
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err(format!(
                    "request timed out after {} seconds",
                    SESSION_LOAD_TIMEOUT.as_secs()
                )),
            };
        let _ = sender.send(RuntimeEvent::SessionLoaded {
            request_id,
            session: Box::new(session),
            result,
        });
    });
}

/// 后台加载会话思考级别。
fn spawn_session_thinking_load(
    api: &ApiClient,
    session_id: String,
    sender: &mpsc::UnboundedSender<RuntimeEvent>,
) {
    let api = api.clone();
    let sender = sender.clone();
    tokio::spawn(async move {
        let result = api
            .thinking_state(&session_id)
            .await
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send(RuntimeEvent::SessionThinkingLoaded { session_id, result });
    });
}

/// 后台执行 VCS 请求并把结果发回主循环。
fn spawn_vcs_request(
    api: &ApiClient,
    session_id: String,
    request: VcsRequest,
    sender: &mpsc::UnboundedSender<RuntimeEvent>,
) {
    let api = api.clone();
    let sender = sender.clone();
    tokio::spawn(async move {
        let result = match request {
            VcsRequest::Refresh => api.vcs_changes(&session_id).await,
            VcsRequest::Commit(message) => api.commit_vcs(&session_id, &message).await,
            VcsRequest::Push => api.push_vcs(&session_id).await,
            VcsRequest::Revert => api.revert_vcs(&session_id).await,
        }
        .map_err(|error| format!("{error:#}"));
        let _ = sender.send(RuntimeEvent::VcsResult { session_id, result });
    });
}

/// 执行 App 产生的 Action。多数动作直接派发后台任务；
/// 返回 `true` 表示需要退出事件循环（Quit）。
async fn execute_action(
    action: Action,
    app: &mut App,
    api: &ApiClient,
    runtime_tx: &mpsc::UnboundedSender<RuntimeEvent>,
) -> Result<bool> {
    match action {
        Action::None => {}
        Action::Quit => return Ok(true),
        Action::Submit {
            message,
            requested_tool,
            attachment_paths,
        } => {
            // 草稿会话先物化为真实会话（创建 + 应用默认模型/思考级别/模式）。
            if let Err(error) = materialize_draft_session(app, api).await {
                app.stream_failed(format!("{error:#}"));
                return Ok(false);
            }
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            let workspace = PathBuf::from(&app.cwd);
            tokio::spawn(async move {
                if let Err(error) = api
                    .stream_chat(
                        session_id,
                        workspace,
                        message,
                        requested_tool,
                        attachment_paths,
                        sender.clone(),
                    )
                    .await
                {
                    let _ = sender.send(RuntimeEvent::StreamFailed(format!("{error:#}")));
                }
            });
        }
        Action::QueueInput { message } => {
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            tokio::spawn(async move {
                let result = api
                    .queue_session_input(&session_id, &message)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(RuntimeEvent::QueueInputFinished {
                    session_id,
                    message,
                    result,
                });
            });
        }
        Action::Abort => {
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            tokio::spawn(async move {
                let result = api
                    .abort(&session_id)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(RuntimeEvent::AbortFinished { session_id, result });
            });
        }
        Action::LoadOlderMessages { before } => {
            let api = api.clone();
            let session_id = app.session.id.clone();
            let sender = runtime_tx.clone();
            tokio::spawn(async move {
                let result = api
                    .messages_page(&session_id, Some(before))
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(RuntimeEvent::HistoryPage { before, result });
            });
        }
        Action::Compact => {
            app.begin_context_compaction();
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            tokio::spawn(async move {
                let result = api.compact_session(&session_id).await;
                let (context_usage, error) = match result {
                    Ok(context_usage) => (context_usage, None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                };
                let _ = sender.send(RuntimeEvent::CompactionFinished {
                    context_usage,
                    error,
                });
            });
        }
        Action::ResolveApproval { id, approved } => {
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            tokio::spawn(async move {
                let result = api
                    .resolve_approval(&session_id, &id, approved)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(RuntimeEvent::ApprovalResolved {
                    session_id,
                    approval_id: id,
                    approved,
                    result,
                });
            });
        }
        Action::RefreshVcs => {
            if app.vcs_loading {
                return Ok(false);
            }
            if app.is_draft_session() {
                app.set_vcs_error(
                    "workspace changes become available after the first message".to_owned(),
                );
            } else {
                app.set_vcs_loading(true);
                spawn_vcs_request(api, app.session.id.clone(), VcsRequest::Refresh, runtime_tx);
            }
        }
        Action::CommitVcs(message) => {
            if app.vcs_loading {
                return Ok(false);
            }
            if app.is_draft_session() || app.is_streaming() {
                app.set_vcs_error(
                    "commit is available only after the active run finishes".to_owned(),
                );
            } else {
                app.set_vcs_loading(true);
                spawn_vcs_request(
                    api,
                    app.session.id.clone(),
                    VcsRequest::Commit(message),
                    runtime_tx,
                );
            }
        }
        Action::PushVcs => {
            if app.vcs_loading {
                return Ok(false);
            }
            if app.is_draft_session() || app.is_streaming() {
                app.set_vcs_error(
                    "push is available only after the active run finishes".to_owned(),
                );
            } else {
                app.set_vcs_loading(true);
                spawn_vcs_request(api, app.session.id.clone(), VcsRequest::Push, runtime_tx);
            }
        }
        Action::RevertVcs => {
            if app.vcs_loading {
                return Ok(false);
            }
            if app.is_draft_session() || app.is_streaming() {
                app.set_vcs_error(
                    "revert is available only after the active run finishes".to_owned(),
                );
            } else {
                app.set_vcs_loading(true);
                spawn_vcs_request(api, app.session.id.clone(), VcsRequest::Revert, runtime_tx);
            }
        }
        Action::NewSession => {
            let draft = draft_session(app.new_session_workspace(), &app.model, &app.thinking_level);
            app.replace_session(draft, Vec::new(), None);
        }
        Action::SetCwd(requested) => {
            app.status = format!("changing directory · {}", requested.display());
            match canonical_workspace(&requested) {
                Ok(requested) if app.is_draft_session() => app.set_cwd(model::SessionCwdUpdate {
                    cwd: requested.to_string_lossy().into_owned(),
                }),
                Ok(requested) => match api.set_session_cwd(&app.session.id, &requested).await {
                    Ok(updated) if same_workspace(&updated.cwd, &requested) => app.set_cwd(updated),
                    Ok(updated) => {
                        app.status = format!(
                            "directory change failed · requested {} · received {}",
                            requested.display(),
                            updated.cwd
                        );
                        app.status_error = true;
                    }
                    Err(error) => {
                        app.status = format!("directory change failed · {error}");
                        app.status_error = true;
                    }
                },
                Err(error) => {
                    app.status = format!("directory change failed · {error}");
                    app.status_error = true;
                }
            }
        }
        Action::SetExecutionMode(mode) => {
            app.status = format!("changing mode · {mode}");
            if app.is_draft_session() {
                app.set_execution_mode(mode);
            } else {
                let api = api.clone();
                let sender = runtime_tx.clone();
                let session_id = app.session.id.clone();
                tokio::spawn(async move {
                    let result = api
                        .set_execution_mode(&session_id, &mode)
                        .await
                        .map_err(|error| format!("{error:#}"));
                    let _ = sender.send(RuntimeEvent::ExecutionModeFinished { session_id, result });
                });
            }
        }
        Action::SetModel { provider, model } => {
            app.status = format!("changing model · {provider}/{model}");
            if app.is_draft_session() {
                app.set_draft_model(provider, model);
            } else {
                match api
                    .set_session_model(&app.session.id, &provider, &model)
                    .await
                {
                    Ok(updated) => app.set_model(updated),
                    Err(error) => {
                        app.status = format!("model change failed · {error}");
                        app.status_error = true;
                    }
                }
            }
        }
        Action::RefreshThinking => {
            if app.is_draft_session() {
                app.status = "Thinking options become available after the first message".to_owned();
                app.status_error = false;
                return Ok(false);
            }
            app.begin_thinking_load();
            let result = api.thinking_state(&app.session.id).await;
            match result {
                Ok(state) => app.set_thinking_state(state),
                Err(error) => app.set_thinking_error(format!("{error:#}")),
            }
            app.open_thinking_picker();
        }
        Action::SetThinkingLevel(level) => {
            app.status = format!("changing thinking · {level}");
            if app.is_draft_session() {
                app.set_draft_thinking_level(level);
            } else {
                match api.set_thinking_level(&app.session.id, &level).await {
                    Ok(updated) => app.set_thinking_level(updated),
                    Err(error) => {
                        app.status = format!("thinking change failed · {error}");
                        app.status_error = true;
                    }
                }
            }
        }
        Action::SaveProviderConnection {
            provider,
            api: provider_api,
            base_url,
            mut api_key,
        } => {
            let result = api
                .set_provider_connection(&provider, &provider_api, &base_url, &api_key)
                .await;
            api_key.zeroize();
            match result {
                Ok(updated) if updated.connection_updated => {
                    let provider_id = if updated.updated_provider_id.is_empty() {
                        provider
                    } else {
                        updated.updated_provider_id
                    };
                    app.provider_connection_saved(
                        &provider_id,
                        provider_api,
                        base_url,
                        updated.api_key_updated,
                    );
                    if let Ok((_, _, model_options, provider_options)) =
                        api.runtime_preferences().await
                    {
                        app.set_model_options(model_options);
                        app.set_provider_options(provider_options);
                    }
                }
                Ok(_) => app.provider_connection_save_failed(
                    "Runtime did not confirm the update".to_owned(),
                ),
                Err(error) => app.provider_connection_save_failed(format!("{error:#}")),
            }
        }
        Action::OpenWeb => {
            if sidecar::configured_frontend_root().is_none() {
                app.status = "Web UI is not installed · exit and run `pisper web`".to_owned();
                app.status_error = true;
            } else {
                match api
                    .bootstrap_url("/config")
                    .and_then(|url| webbrowser::open(&url).map(|_| ()).map_err(Into::into))
                {
                    Ok(()) => {
                        app.status = "Web settings opened in the default browser".to_owned();
                        app.status_error = false;
                    }
                    Err(error) => {
                        app.status = format!("failed to open Web settings · {error:#}");
                        app.status_error = true;
                    }
                }
            }
        }
        Action::SwitchSession { id, request_id } => {
            let Some(session) = app
                .sessions
                .iter()
                .find(|session| session.id == id)
                .cloned()
            else {
                app.session_load_failed(
                    request_id,
                    &id,
                    "conversation no longer exists".to_owned(),
                );
                return Ok(false);
            };
            if let Err(error) = validate_session_workspace(&session, None) {
                app.session_load_failed(request_id, &id, format!("{error:#}"));
                return Ok(false);
            }
            spawn_session_load(api, request_id, session, runtime_tx);
        }
    }
    Ok(false)
}

/// 把草稿会话物化为 sidecar 中的真实会话：
/// 创建会话后按草稿上配置的模型/思考级别/执行模式逐一同步（与默认值不同才调用），
/// 使会话一出生就带用户预选的设置。
async fn materialize_draft_session(app: &mut App, api: &ApiClient) -> Result<()> {
    if !app.is_draft_session() {
        return Ok(());
    }

    let requested_model = app.model.clone();
    let requested_thinking = app.thinking_level.clone();
    let requested_mode = app.execution_mode.clone();
    let mut session = api
        // 标题交给 Runtime 统一生成，首条用户消息到达后会自动命名。
        .create_session("", Path::new(&app.cwd))
        .await?;

    if !requested_model.is_empty() && requested_model != session.model {
        let (provider, model) = requested_model
            .split_once('/')
            .context("draft model is missing its Provider")?;
        let updated = api.set_session_model(&session.id, provider, model).await?;
        session.model = updated.model;
        session.thinking_level = updated.thinking_level;
    }
    if !requested_thinking.is_empty() && requested_thinking != session.thinking_level {
        let updated = api
            .set_thinking_level(&session.id, &requested_thinking)
            .await?;
        session.thinking_level = updated.thinking_level;
    }
    if !requested_mode.is_empty() && requested_mode != session.execution_mode {
        let updated = api.set_execution_mode(&session.id, &requested_mode).await?;
        session.execution_mode = updated.execution_mode;
    }

    app.materialize_session(session);
    Ok(())
}

/// 构造草稿会话：尚无 id，模型/思考级别继承自当前上下文，
/// 工作区取启动工作区，默认 full-access 模式。
fn draft_session(workspace: &Path, model: &str, thinking_level: &str) -> SessionSummary {
    SessionSummary {
        name: String::new(),
        model: model.to_owned(),
        cwd: workspace.to_string_lossy().into_owned(),
        execution_mode: "full-access".to_owned(),
        thinking_level: thinking_level.to_owned(),
        ..SessionSummary::default()
    }
}

/// `resume` 模式下的会话种子：取列表中的第一个会话。
fn resume_seed(sessions: &[SessionSummary], resume: bool) -> Option<SessionSummary> {
    resume.then(|| sessions.first().cloned()).flatten()
}

/// 解析后的启动选项。
struct LaunchOptions {
    workspace: PathBuf,
    doctor: bool,
    resume: bool,
    web: bool,
}

/// 帮助请求处理：`help` / `--help` / `-h` 均返回对应帮助文本（不启动任何组件）。
fn requested_help(arguments: &[OsString]) -> Option<&'static str> {
    let first = arguments.first().and_then(|argument| argument.to_str());
    if first == Some("help") {
        return Some(
            match arguments.get(1).and_then(|argument| argument.to_str()) {
                Some("web") => WEB_HELP,
                _ => CLI_HELP,
            },
        );
    }
    let help_requested = arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h");
    help_requested.then_some(match first {
        Some("web") => WEB_HELP,
        _ => CLI_HELP,
    })
}

/// 解析 CLI 参数：`--cwd`/`doctor`/`resume`/`web`/版本与帮助。
/// doctor/resume/web 互斥；未知参数直接报错。
fn launch_options() -> Result<LaunchOptions> {
    let mut args = std::env::args_os().skip(1);
    let mut workspace = None;
    let mut doctor = false;
    let mut resume = false;
    let mut web = false;
    while let Some(argument) = args.next() {
        if argument == "--cwd" {
            workspace = Some(PathBuf::from(
                args.next().context("--cwd requires a directory")?,
            ));
        } else if argument == "doctor" {
            doctor = true;
        } else if argument == "resume" {
            resume = true;
        } else if argument == "web" {
            web = true;
        } else if argument == "--version" || argument == "-V" {
            println!("pisper {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        } else if argument == "--help" || argument == "-h" {
            println!("{CLI_HELP}");
            std::process::exit(0);
        } else {
            anyhow::bail!("unknown argument: {}", argument.to_string_lossy());
        }
    }
    if usize::from(doctor) + usize::from(resume) + usize::from(web) > 1 {
        anyhow::bail!("doctor, resume, and web cannot be used together");
    }
    let workspace = workspace.unwrap_or(std::env::current_dir()?);
    let workspace = canonical_workspace(&workspace)?;
    Ok(LaunchOptions {
        workspace,
        doctor,
        resume,
        web,
    })
}

/// 终端会话管理：进入原始模式与备用屏、开启 bracketed-paste；
/// 退出时恢复终端状态（幂等，Drop 兜底）。
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalSession {
    /// 进入终端会话：启用 raw mode、切到备用屏、开启 bracketed-paste，
    /// 返回包装后的 Terminal；任一步失败时中止（保持终端原状）。
    fn start() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    /// 恢复终端：重复调用只执行一次。
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

impl Drop for TerminalSession {
    /// 兜底恢复终端（即使提前 return 也保证还原，幂等）。
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        draft_session, is_ended_session_queue_error, is_inline_attachment_shortcut,
        is_paste_shortcut, requested_help, resize_area, resize_has_settled, resume_seed,
        should_handle_key_kind, should_notify_completion, terminal_content_area, CLI_HELP,
        WEB_HELP,
    };
    use crate::model::SessionSummary;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::time::Duration;
    use tokio::time::Instant;

    /// 验证 help 路由：全局命令与 web 子命令都能命中对应帮助，且不触发组件更新。
    #[test]
    fn help_routes_cover_global_and_web_commands_without_component_updates() {
        use std::ffi::OsString;

        assert_eq!(requested_help(&[OsString::from("--help")]), Some(CLI_HELP));
        assert_eq!(requested_help(&[OsString::from("help")]), Some(CLI_HELP));
        assert_eq!(
            requested_help(&[OsString::from("help"), OsString::from("web")]),
            Some(WEB_HELP)
        );
        assert_eq!(
            requested_help(&[OsString::from("web"), OsString::from("--help")]),
            Some(WEB_HELP)
        );
        assert!(CLI_HELP.contains("pisper web"));
        assert!(!CLI_HELP.contains("pisper update"));
        assert!(CLI_HELP.contains("/provider"));
        assert!(!CLI_HELP.contains("/apikey"));
    }

    /// 验证 Windows 粘贴快捷键（Ctrl+V / Shift+Insert）会武装粘贴突发检测。
    #[test]
    fn windows_paste_shortcuts_arm_the_paste_burst() {
        assert!(is_paste_shortcut(&KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
        )));
        assert!(is_paste_shortcut(&KeyEvent::new(
            KeyCode::Insert,
            KeyModifiers::SHIFT,
        )));
        assert!(!is_paste_shortcut(&KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::NONE,
        )));
    }

    /// 验证按键释放事件不会冲刷粘贴突发（只处理 Press/Repeat）。
    #[test]
    fn key_releases_do_not_flush_paste_bursts() {
        assert!(should_handle_key_kind(KeyEventKind::Press));
        assert!(should_handle_key_kind(KeyEventKind::Repeat));
        assert!(!should_handle_key_kind(KeyEventKind::Release));
    }

    /// 验证内联附件快捷键（Shift+加号）不被单字符粘贴检测误吞。
    #[test]
    fn inline_attachment_shortcut_bypasses_single_character_paste_detection() {
        let plus = KeyEvent::new(KeyCode::Char('+'), KeyModifiers::SHIFT);
        assert!(is_inline_attachment_shortcut(&plus, true, false));
        assert!(!is_inline_attachment_shortcut(&plus, false, false));
        assert!(!is_inline_attachment_shortcut(&plus, true, true));

        let controlled_plus = KeyEvent::new(KeyCode::Char('+'), KeyModifiers::CONTROL);
        assert!(!is_inline_attachment_shortcut(
            &controlled_plus,
            true,
            false
        ));
    }

    /// 验证“会话已结束”排队错误识别兼容中英文运行时文案。
    #[test]
    fn ended_session_queue_errors_accept_runtime_languages() {
        assert!(is_ended_session_queue_error(
            "当前会话已经结束运行，请作为新消息发送。"
        ));
        assert!(is_ended_session_queue_error(
            "Session has finished and is no longer running"
        ));
        assert!(!is_ended_session_queue_error("Provider request failed"));
    }

    /// 验证完成通知只在无排队消息时发送（有排队则等最后一条）。
    #[test]
    fn completion_notifications_wait_for_the_final_queued_turn() {
        assert!(should_notify_completion(true, 0));
        assert!(!should_notify_completion(true, 1));
        assert!(!should_notify_completion(false, 0));
    }

    /// 验证窗口尺寸事件保留宽高，0 值（未知尺寸）被忽略。
    #[test]
    fn resize_events_keep_the_reported_terminal_area() {
        assert_eq!(resize_area(120, 30).unwrap().width, 120);
        assert_eq!(resize_area(120, 30).unwrap().height, 30);
        assert!(resize_area(0, 30).is_none());
        assert!(resize_area(120, 0).is_none());
    }

    /// 验证 resize 重绘防抖：80ms 内多次 resize 合并为一次重绘。
    #[test]
    fn resize_redraw_waits_until_the_event_stream_settles() {
        let last_resize_at = Instant::now();
        assert!(!resize_has_settled(
            last_resize_at,
            last_resize_at + Duration::from_millis(79),
        ));
        assert!(resize_has_settled(
            last_resize_at,
            last_resize_at + Duration::from_millis(80),
        ));
    }

    /// 验证 JetBrains 终端下保留自动换行列（内容区少一列防折行）。
    #[test]
    fn jetbrains_terminal_reserves_the_auto_wrap_column() {
        let area = ratatui::layout::Rect::new(0, 0, 120, 30);
        assert_eq!(terminal_content_area(area, true).width, 119);
        assert_eq!(terminal_content_area(area, false), area);
    }

    /// 验证欢迎页使用未持久化的新会话草稿（空 id、全权限模式）。
    #[test]
    fn a_fresh_logo_page_uses_an_unpersisted_session_draft() {
        let draft = draft_session(std::path::Path::new("/workspace"), "provider/model", "high");

        assert!(draft.id.is_empty());
        assert!(draft.name.is_empty());
        assert_eq!(draft.cwd, "/workspace");
        assert_eq!(draft.model, "provider/model");
        assert_eq!(draft.thinking_level, "high");
        assert_eq!(draft.execution_mode, "full-access");
    }

    /// 验证 resume 时全局会话选择器不受工作区过滤限制（跨工作区可恢复）。
    #[test]
    fn resume_seeds_the_global_picker_without_filtering_by_workspace() {
        let sessions = vec![SessionSummary {
            id: "other-workspace-session".to_owned(),
            cwd: "/another/workspace".to_owned(),
            ..SessionSummary::default()
        }];

        assert!(resume_seed(&sessions, false).is_none());
        assert_eq!(
            resume_seed(&sessions, true)
                .map(|session| session.id)
                .as_deref(),
            Some("other-workspace-session")
        );
        assert!(resume_seed(&[], true).is_none());
    }
}
