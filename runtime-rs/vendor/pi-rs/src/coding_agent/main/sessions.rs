//! Real SessionManager selection, preserving main.ts lookup and exit ordering.
//!
//! The host owns terminal output, confirmation and the resume picker. Only
//! those UI effects are injectable; filesystem discovery/open/fork/continue
//! always use the production SessionManager. No process exit from this library.
use crate::coding_agent::{
    cli::args::Args,
    core::settings_manager::SettingsManager,
    session_manager::{NewSessionOptions, SessionInfo, SessionManager, SessionManagerError},
    utils::paths::resolve_path,
};
use anyhow::Result;
use futures::future::BoxFuture;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolvedSession {
    Path { path: String },
    Local { path: String },
    Global { path: String, cwd: String },
    NotFound { arg: String },
}
fn looks_like_path(arg: &str) -> bool {
    arg.contains('/') || arg.contains('\\') || arg.ends_with(".jsonl")
}
fn find<'a>(sessions: &'a [SessionInfo], arg: &str) -> Option<&'a SessionInfo> {
    sessions
        .iter()
        .find(|s| s.id == arg)
        .or_else(|| sessions.iter().find(|s| s.id.starts_with(arg)))
}
pub fn resolve_session_path(
    arg: &str,
    cwd: &str,
    session_dir: Option<&str>,
) -> Result<ResolvedSession> {
    if looks_like_path(arg) {
        return Ok(ResolvedSession::Path {
            path: resolve_path(arg, cwd)?,
        });
    }
    let local = SessionManager::list(cwd, session_dir, None);
    if let Some(s) = find(&local, arg) {
        return Ok(ResolvedSession::Local {
            path: s.path.clone(),
        });
    }
    let global = SessionManager::list_all(session_dir, None);
    Ok(match find(&global, arg) {
        Some(s) => ResolvedSession::Global {
            path: s.path.clone(),
            cwd: s.cwd.clone(),
        },
        None => ResolvedSession::NotFound { arg: arg.into() },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupOutput {
    Error,
    Warning,
    Notice,
    Dim,
}

pub trait SessionStartupUi: Send + Sync {
    /// Error/Warning use stderr; Notice/Dim use stdout (red/yellow/yellow/dim).
    fn report(&self, kind: StartupOutput, message: &str);
    /// The host appends the upstream ` [y/N] ` prompt and matches y/yes only.
    fn confirm<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<bool>>;
    /// Hosts may call the production list/list_all using these exact arguments.
    fn select_session<'a>(
        &'a self,
        cwd: &'a str,
        session_dir: Option<&'a str>,
        settings: &'a SettingsManager,
    ) -> BoxFuture<'a, Result<Option<String>>>;
    fn stop_theme_watcher(&self);
}

pub enum SessionSelection {
    Open(Box<SessionManager>),
    Exit(i32),
}

/// Main's process-exit branches are explicit values, not exceptions or fake sessions.
pub async fn create_session_manager(
    parsed: &Args,
    cwd: &str,
    session_dir: Option<&str>,
    settings: &SettingsManager,
    ui: Arc<dyn SessionStartupUi>,
) -> Result<SessionSelection> {
    let options = NewSessionOptions {
        id: parsed.session_id.clone(),
        parent_session: None,
    };
    if parsed.no_session == Some(true) || parsed.help == Some(true) || parsed.list_models.is_some()
    {
        return Ok(SessionSelection::Open(Box::new(SessionManager::in_memory(
            cwd,
            Some(&options),
            None,
        )?)));
    }
    let handled = |result: std::result::Result<SessionManager, SessionManagerError>| match result {
        Ok(session) => SessionSelection::Open(Box::new(session)),
        Err(error) => {
            ui.report(StartupOutput::Error, &format!("Error: {error}"));
            SessionSelection::Exit(1)
        }
    };
    if let Some(fork) = parsed.fork.as_ref().filter(|s| !s.is_empty()) {
        if let Some(id) = parsed.session_id.as_ref().filter(|s| !s.is_empty()) {
            if SessionManager::find_by_id(cwd, id, session_dir).is_some() {
                ui.report(
                    StartupOutput::Error,
                    &format!("Session already exists with id '{id}'"),
                );
                return Ok(SessionSelection::Exit(1));
            }
        }
        return Ok(match resolve_session_path(fork, cwd, session_dir)? {
            ResolvedSession::Path { path }
            | ResolvedSession::Local { path }
            | ResolvedSession::Global { path, .. } => handled(SessionManager::fork_from(
                &path,
                cwd,
                session_dir,
                Some(&options),
            )),
            ResolvedSession::NotFound { arg } => {
                ui.report(
                    StartupOutput::Error,
                    &format!("No session found matching '{arg}'"),
                );
                SessionSelection::Exit(1)
            }
        });
    }
    if let Some(arg) = parsed.session.as_ref().filter(|s| !s.is_empty()) {
        return Ok(match resolve_session_path(arg, cwd, session_dir)? {
            ResolvedSession::Path { path } | ResolvedSession::Local { path } => {
                handled(SessionManager::open(&path, session_dir, None))
            }
            ResolvedSession::Global {
                path,
                cwd: other_cwd,
            } => {
                ui.report(
                    StartupOutput::Notice,
                    &format!("Session found in different project: {other_cwd}"),
                );
                if !ui
                    .confirm("Fork this session into current directory?")
                    .await?
                {
                    ui.report(StartupOutput::Dim, "Aborted.");
                    SessionSelection::Exit(0)
                } else {
                    handled(SessionManager::fork_from(&path, cwd, session_dir, None))
                }
            }
            ResolvedSession::NotFound { arg } => {
                ui.report(
                    StartupOutput::Error,
                    &format!("No session found matching '{arg}'"),
                );
                SessionSelection::Exit(1)
            }
        });
    }
    if parsed.resume == Some(true) {
        struct StopWatcher(Arc<dyn SessionStartupUi>);
        impl Drop for StopWatcher {
            fn drop(&mut self) {
                self.0.stop_theme_watcher();
            }
        }
        let _guard = StopWatcher(ui.clone());
        let path = ui.select_session(cwd, session_dir, settings).await?;
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            ui.report(StartupOutput::Dim, "No session selected");
            return Ok(SessionSelection::Exit(0));
        };
        return Ok(SessionSelection::Open(Box::new(SessionManager::open(
            &path,
            session_dir,
            None,
        )?)));
    }
    if parsed.r#continue == Some(true) {
        return Ok(SessionSelection::Open(Box::new(
            SessionManager::continue_recent(cwd, session_dir)?,
        )));
    }
    if let Some(id) = parsed.session_id.as_ref().filter(|s| !s.is_empty()) {
        if let Some(path) = SessionManager::find_by_id(cwd, id, session_dir) {
            return Ok(SessionSelection::Open(Box::new(SessionManager::open(
                &path,
                session_dir,
                None,
            )?)));
        }
        ui.report(StartupOutput::Warning,&format!("Warning: No project session found with id '{id}'; creating a new session with that id."));
    }
    Ok(SessionSelection::Open(Box::new(SessionManager::create(
        cwd,
        session_dir,
        Some(&options),
    )?)))
}
