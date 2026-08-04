mod api;
mod app;
mod model;
mod plan_protocol;
mod sidecar;
mod ui;
mod workspace;

use std::{io, path::PathBuf};

use anyhow::{Context, Result};
use app::{Action, App};
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use futures_util::StreamExt;
use model::{RuntimeEvent, SessionSummary};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use tokio::sync::mpsc;

use crate::{
    api::ApiClient,
    sidecar::SidecarConnection,
    workspace::{canonical_workspace, same_workspace, validate_session_workspace},
};

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
    let mut sessions = api
        .sessions()
        .await
        .context("failed to list conversations")?;
    if options.resume && sessions.is_empty() {
        println!("No conversations are available to resume.");
        sidecar.shutdown();
        return Ok(());
    }
    let interactive_resume = options.resume;
    let mut session = match resume_seed(&sessions, options.resume) {
        Some(session) => session,
        None => {
            let created = api
                .create_session("New conversation", &options.workspace)
                .await?;
            sessions.insert(0, created.clone());
            created
        }
    };
    if !interactive_resume {
        validate_session_workspace(&session, None)?;
    }
    let (messages, context_usage, thinking_state) = if interactive_resume {
        (Vec::new(), None, None)
    } else {
        let (thinking_state, page) =
            tokio::join!(api.thinking_state(&session.id), api.messages(&session.id));
        if let Ok(state) = &thinking_state {
            if !state.thinking_level.is_empty() {
                session.thinking_level.clone_from(&state.thinking_level);
            }
        }
        let page = page?;
        (page.messages, page.context_usage, Some(thinking_state))
    };
    let mut app = App::new(
        sessions,
        session,
        messages,
        context_usage,
        Vec::new(),
        Vec::new(),
    );
    app.set_launch_workspace(options.workspace.clone());
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

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
    api: ApiClient,
) -> Result<()> {
    let mut input = EventStream::new();
    let (runtime_tx, mut runtime_rx) = mpsc::unbounded_channel::<RuntimeEvent>();
    let startup_api = api.clone();
    let startup_sender = runtime_tx.clone();
    tokio::spawn(async move {
        let (preferences, catalogs) =
            tokio::join!(startup_api.runtime_preferences(), startup_api.catalogs());
        let (_, model_options) = preferences.unwrap_or_else(|_| (String::new(), Vec::new()));
        let (tools, skills) = catalogs.unwrap_or_default();
        let _ = startup_sender.send(RuntimeEvent::StartupData {
            model_options,
            tools,
            skills,
        });
    });
    let mut animation = tokio::time::interval(std::time::Duration::from_millis(24));
    animation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut status_animation = tokio::time::interval(std::time::Duration::from_millis(120));
    status_animation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut redraw = true;
    let mut reset_before_redraw = false;
    loop {
        if redraw {
            draw_frame(terminal, &app, std::mem::take(&mut reset_before_redraw))?;
        }
        redraw = tokio::select! {
            maybe_event = input.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        let action = app.handle_key(key);
                        if execute_action(action, &mut app, &api, &runtime_tx).await? {
                            break;
                        }
                        true
                    }
                    Some(Ok(Event::Paste(value))) => {
                        app.insert_paste(&value);
                        true
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        reset_before_redraw = true;
                        true
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
                                model_options,
                                tools,
                                skills,
                            } => {
                                app.set_startup_data(model_options, tools, skills);
                                false
                            }
                            RuntimeEvent::Stream(event) => {
                                let terminal = matches!(event.name.as_str(), "done" | "error");
                                app.apply_stream_event(event);
                                terminal
                            }
                            RuntimeEvent::StreamFailed(message) => {
                                app.stream_failed(message);
                                true
                            }
                        };
                        if terminal_event {
                            if let Some(action) = app.take_queued_action() {
                                if execute_action(action, &mut app, &api, &runtime_tx).await? {
                                    break;
                                }
                            }
                        }
                        true
                    }
                    None => false,
                }
            }
            _ = animation.tick(), if app.has_pending_render() => {
                app.advance_stream_render();
                true
            }
            _ = status_animation.tick(), if app.is_streaming() && !app.has_pending_render() => {
                app.advance_status_animation();
                true
            }
        };
    }
    Ok(())
}

fn synchronize_terminal_size<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    terminal.autoresize()?;
    terminal.clear()?;
    Ok(())
}

fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &App,
    reset_before_draw: bool,
) -> Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let draw_result = (|| -> Result<()> {
        if reset_before_draw {
            synchronize_terminal_size(terminal)?;
        }
        terminal.draw(|frame| ui::draw(frame, app)).map(drop)?;
        Ok(())
    })();
    let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    draw_result?;
    end_result?;
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
            attachment_paths,
        } => {
            let api = api.clone();
            let sender = runtime_tx.clone();
            let session_id = app.session.id.clone();
            tokio::spawn(async move {
                if let Err(error) = api
                    .stream_chat(
                        session_id,
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
        Action::Abort => {
            api.abort(&app.session.id).await?;
            app.status = "stopping".to_owned();
        }
        Action::ResolveApproval { id, approved } => {
            api.resolve_approval(&app.session.id, &id, approved).await?;
            app.status = if approved { "approved" } else { "denied" }.to_owned();
        }
        Action::NewSession => {
            let workspace = app.new_session_workspace().to_path_buf();
            let created = api.create_session("New conversation", &workspace).await?;
            let (page, thinking_state) =
                tokio::join!(api.messages(&created.id), api.thinking_state(&created.id));
            let page = page?;
            app.sessions = refreshed_sessions(api, created.clone()).await?;
            app.replace_session(created, page.messages, page.context_usage);
            match thinking_state {
                Ok(state) => app.set_thinking_state(state),
                Err(error) => app.set_thinking_error(format!("{error:#}")),
            }
        }
        Action::SetCwd(requested) => {
            app.status = format!("changing directory · {}", requested.display());
            match canonical_workspace(&requested) {
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
            match api.set_execution_mode(&app.session.id, &mode).await {
                Ok(updated) => app.set_execution_mode(updated.execution_mode),
                Err(error) => {
                    app.status = format!("mode change failed · {error}");
                    app.status_error = true;
                }
            }
        }
        Action::SetModel { provider, model } => {
            app.status = format!("changing model · {provider}/{model}");
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
        Action::RefreshThinking => {
            app.begin_thinking_load();
            let result = api.thinking_state(&app.session.id).await;
            match result {
                Ok(state) => {
                    let detail = format!(
                        "session {} · model {} · current {} · available {}",
                        app.session.id,
                        app.model,
                        state.thinking_level,
                        state.available_levels.join(",")
                    );
                    app.set_thinking_state(state);
                    app.record_event("THINKING", detail, "done");
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    app.set_thinking_error(error.clone());
                    app.record_event(
                        "THINKING",
                        format!(
                            "session {} · model {} · load failed · {error}",
                            app.session.id, app.model
                        ),
                        "error",
                    );
                }
            }
            app.open_thinking_picker();
        }
        Action::SetThinkingLevel(level) => {
            app.status = format!("changing thinking · {level}");
            match api.set_thinking_level(&app.session.id, &level).await {
                Ok(updated) => {
                    let detail = format!(
                        "session {} · model {} · requested {level} · current {} · available {}",
                        app.session.id,
                        app.model,
                        updated.thinking_level,
                        updated.available_levels.join(",")
                    );
                    app.set_thinking_level(updated);
                    app.record_event("THINKING", detail, "done");
                }
                Err(error) => {
                    app.status = format!("thinking change failed · {error}");
                    app.status_error = true;
                    app.record_event(
                        "THINKING",
                        format!(
                            "session {} · model {} · requested {level} · failed · {error}",
                            app.session.id, app.model
                        ),
                        "error",
                    );
                }
            }
        }
        Action::SwitchSession {
            id,
            exit_on_failure,
        } => {
            let Some(session) = app
                .sessions
                .iter()
                .find(|session| session.id == id)
                .cloned()
            else {
                app.status = "conversation no longer exists".to_owned();
                app.status_error = true;
                app.open_session_picker_at(exit_on_failure, &id);
                return Ok(false);
            };
            if let Err(error) = validate_session_workspace(&session, None) {
                app.status = format!("cannot resume conversation · {error}");
                app.status_error = true;
                app.open_session_picker_at(exit_on_failure, &id);
                return Ok(false);
            }
            let (page, thinking_state) =
                tokio::join!(api.messages(&session.id), api.thinking_state(&session.id));
            let page = match page {
                Ok(page) => page,
                Err(error) => {
                    app.status = format!("cannot resume conversation · {error}");
                    app.status_error = true;
                    app.open_session_picker_at(exit_on_failure, &id);
                    return Ok(false);
                }
            };
            app.replace_session(session, page.messages, page.context_usage);
            match thinking_state {
                Ok(state) => app.set_thinking_state(state),
                Err(error) => app.set_thinking_error(format!("{error:#}")),
            }
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

fn resume_seed(sessions: &[SessionSummary], resume: bool) -> Option<SessionSummary> {
    resume.then(|| sessions.first().cloned()).flatten()
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
            println!("Pisper terminal client\n\nUsage: pisper [--cwd <directory>]\n       pisper resume\n       pisper doctor [--cwd <directory>]\n\n`pisper resume` opens an interactive list of conversations from every workspace.\n");
            std::process::exit(0);
        } else {
            anyhow::bail!("unknown argument: {}", argument.to_string_lossy());
        }
    }
    if doctor && resume {
        anyhow::bail!("doctor and resume cannot be used together");
    }
    let workspace = workspace.unwrap_or(std::env::current_dir()?);
    let workspace = canonical_workspace(&workspace)?;
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
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
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
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
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
    use super::resume_seed;
    use crate::model::SessionSummary;

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
