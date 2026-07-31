mod api;
mod app;
mod model;
mod sidecar;
mod ui;

use std::{io, path::PathBuf};

use anyhow::{Context, Result};
use app::{Action, App};
use crossterm::{
    event::{Event, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use model::{RuntimeEvent, SessionSummary};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::{api::ApiClient, sidecar::SidecarConnection};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("pisper: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let options = launch_options()?;
    let mut sidecar = SidecarConnection::start(&options.workspace)?;
    let api = ApiClient::new(&sidecar.url, &sidecar.token)?;
    if options.doctor {
        let sessions = api
            .sessions()
            .await
            .context("failed to list conversations")?;
        let (tools, skills) = api
            .catalogs()
            .await
            .context("failed to load Slash catalog")?;
        println!(
            "Pisper TUI ready · {} conversations · {} tools · {} skills",
            sessions.len(),
            tools.len(),
            skills.len()
        );
        sidecar.shutdown();
        return Ok(());
    }
    let mut sessions = api
        .sessions()
        .await
        .context("failed to list conversations")?;
    let fallback_thinking_level = if sessions.is_empty()
        || sessions
            .iter()
            .any(|session| session.thinking_level.is_empty())
    {
        api.default_thinking_level()
            .await
            .unwrap_or_else(|_| "medium".to_owned())
    } else {
        "medium".to_owned()
    };
    for summary in &mut sessions {
        if summary.thinking_level.is_empty() {
            summary.thinking_level.clone_from(&fallback_thinking_level);
        }
    }
    let session = match resumable_session(&sessions, &options.workspace, options.resume) {
        Some(session) => session,
        None => {
            let mut created = api
                .create_session("New conversation", &options.workspace)
                .await?;
            if created.thinking_level.is_empty() {
                created.thinking_level.clone_from(&fallback_thinking_level);
            }
            sessions.insert(0, created.clone());
            created
        }
    };
    let page = api.messages(&session.id).await?;
    let (tools, skills) = api.catalogs().await.unwrap_or_default();
    let app = App::new(
        sessions,
        session,
        page.messages,
        page.context_usage,
        tools,
        skills,
    );

    let mut terminal = TerminalSession::start()?;
    let result = run_event_loop(&mut terminal.terminal, app, api).await;
    terminal.restore();
    sidecar.shutdown();
    result
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
    api: ApiClient,
) -> Result<()> {
    let mut input = EventStream::new();
    let (runtime_tx, mut runtime_rx) = mpsc::unbounded_channel::<RuntimeEvent>();
    let mut animation = tokio::time::interval(std::time::Duration::from_millis(24));
    animation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut status_animation = tokio::time::interval(std::time::Duration::from_millis(120));
    status_animation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            maybe_event = input.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        let action = app.handle_key(key);
                        if execute_action(action, &mut app, &api, &runtime_tx).await? {
                            break;
                        }
                    }
                    Some(Ok(Event::Paste(value))) => app.insert_paste(&value),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                }
            }
            maybe_runtime = runtime_rx.recv() => {
                match maybe_runtime {
                    Some(RuntimeEvent::Stream(event)) => app.apply_stream_event(event),
                    Some(RuntimeEvent::StreamFailed(message)) => app.stream_failed(message),
                    None => {}
                }
            }
            _ = animation.tick(), if app.has_pending_render() => {
                app.advance_stream_render();
            }
            _ = status_animation.tick(), if app.is_streaming() && !app.has_pending_render() => {
                app.advance_status_animation();
            }
        }
    }
    Ok(())
}

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
        } => {
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            tokio::spawn(async move {
                if let Err(error) = api
                    .stream_chat(session_id, message, requested_tool, sender.clone())
                    .await
                {
                    let _ = sender.send(RuntimeEvent::StreamFailed(format!("{error:#}")));
                }
            });
        }
        Action::Abort => {
            api.abort(&app.session.id).await?;
            app.status = "stopping".to_owned();
        }
        Action::ResolveApproval { id, approved } => {
            api.resolve_approval(&app.session.id, &id, approved).await?;
            app.status = if approved { "approved" } else { "denied" }.to_owned();
        }
        Action::NewSession => {
            let workspace = PathBuf::from(&app.session.cwd);
            let created = api.create_session("New conversation", &workspace).await?;
            let page = api.messages(&created.id).await?;
            app.sessions = refreshed_sessions(api, created.clone()).await?;
            app.replace_session(created, page.messages, page.context_usage);
        }
        Action::SetExecutionMode(mode) => {
            app.status = format!("changing mode · {mode}");
            match api.set_execution_mode(&app.session.id, &mode).await {
                Ok(updated) => app.set_execution_mode(updated.execution_mode),
                Err(error) => {
                    app.status = format!("mode change failed · {error}");
                    app.status_error = true;
                }
            }
        }
        Action::SwitchSession(id) => {
            let session = app
                .sessions
                .iter()
                .find(|session| session.id == id)
                .cloned()
                .context("conversation no longer exists")?;
            let page = api.messages(&session.id).await?;
            app.replace_session(session, page.messages, page.context_usage);
        }
    }
    Ok(false)
}

async fn refreshed_sessions(
    api: &ApiClient,
    fallback: SessionSummary,
) -> Result<Vec<SessionSummary>> {
    let mut sessions = api.sessions().await?;
    if !sessions.iter().any(|session| session.id == fallback.id) {
        sessions.insert(0, fallback);
    }
    Ok(sessions)
}

fn resumable_session(
    sessions: &[SessionSummary],
    workspace: &std::path::Path,
    resume: bool,
) -> Option<SessionSummary> {
    if !resume {
        return None;
    }
    sessions
        .iter()
        .find(|session| same_workspace(&session.cwd, workspace))
        .cloned()
}

fn same_workspace(value: &str, workspace: &std::path::Path) -> bool {
    let candidate = PathBuf::from(value);
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    #[cfg(windows)]
    {
        candidate.to_string_lossy().to_lowercase() == workspace.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        candidate == workspace
    }
}

struct LaunchOptions {
    workspace: PathBuf,
    doctor: bool,
    resume: bool,
}

fn launch_options() -> Result<LaunchOptions> {
    let mut args = std::env::args_os().skip(1);
    let mut workspace = None;
    let mut doctor = false;
    let mut resume = false;
    while let Some(argument) = args.next() {
        if argument == "--cwd" {
            workspace = Some(PathBuf::from(
                args.next().context("--cwd requires a directory")?,
            ));
        } else if argument == "doctor" {
            doctor = true;
        } else if argument == "resume" {
            resume = true;
        } else if argument == "--version" || argument == "-V" {
            println!("pisper {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        } else if argument == "--help" || argument == "-h" {
            println!("Pisper terminal client\n\nUsage: pisper [--cwd <directory>]\n       pisper resume [--cwd <directory>]\n       pisper doctor [--cwd <directory>]\n");
            std::process::exit(0);
        } else {
            anyhow::bail!("unknown argument: {}", argument.to_string_lossy());
        }
    }
    if doctor && resume {
        anyhow::bail!("doctor and resume cannot be used together");
    }
    let workspace = workspace.unwrap_or(std::env::current_dir()?);
    let workspace = workspace
        .canonicalize()
        .context("workspace directory does not exist")?;
    if !workspace.is_dir() {
        anyhow::bail!("workspace is not a directory: {}", workspace.display());
    }
    Ok(LaunchOptions {
        workspace,
        doctor,
        resume,
    })
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn start() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::resumable_session;
    use crate::model::SessionSummary;

    #[test]
    fn startup_only_restores_history_when_resume_is_explicit() {
        let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
        let sessions = vec![SessionSummary {
            id: "recent-session".to_owned(),
            cwd: workspace.to_string_lossy().into_owned(),
            ..SessionSummary::default()
        }];

        assert!(resumable_session(&sessions, &workspace, false).is_none());
        assert_eq!(
            resumable_session(&sessions, &workspace, true)
                .map(|session| session.id)
                .as_deref(),
            Some("recent-session")
        );
    }
}
