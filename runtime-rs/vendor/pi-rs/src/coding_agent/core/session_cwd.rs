//! Upstream `session-cwd.ts`: persisted-session cwd validation and diagnostics.
use crate::coding_agent::session_manager::SessionManager;
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCwdIssue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    pub session_cwd: String,
    pub fallback_cwd: String,
}

pub trait SessionCwdSource {
    fn get_cwd(&self) -> &str;
    fn get_session_file(&self) -> Option<&str>;
}
impl SessionCwdSource for SessionManager {
    fn get_cwd(&self) -> &str {
        self.get_cwd()
    }
    fn get_session_file(&self) -> Option<&str> {
        self.get_session_file()
    }
}

pub fn get_missing_session_cwd_issue(
    manager: &(impl SessionCwdSource + ?Sized),
    fallback_cwd: &str,
) -> Option<SessionCwdIssue> {
    let file = manager.get_session_file().filter(|file| !file.is_empty())?;
    let cwd = manager.get_cwd();
    // Upstream existsSync deliberately accepts files, not just directories.
    if cwd.is_empty() || Path::new(cwd).exists() {
        return None;
    }
    Some(SessionCwdIssue {
        session_file: Some(file.to_owned()),
        session_cwd: cwd.to_owned(),
        fallback_cwd: fallback_cwd.to_owned(),
    })
}

pub fn format_missing_session_cwd_error(issue: &SessionCwdIssue) -> String {
    let file = issue
        .session_file
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| format!("\nSession file: {s}"))
        .unwrap_or_default();
    format!(
        "Stored session working directory does not exist: {}{file}\nCurrent working directory: {}",
        issue.session_cwd, issue.fallback_cwd
    )
}

pub fn format_missing_session_cwd_prompt(issue: &SessionCwdIssue) -> String {
    format!(
        "cwd from session file does not exist\n{}\n\ncontinue in current cwd\n{}",
        issue.session_cwd, issue.fallback_cwd
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSessionCwdError {
    pub issue: SessionCwdIssue,
}
impl fmt::Display for MissingSessionCwdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_missing_session_cwd_error(&self.issue))
    }
}
impl std::error::Error for MissingSessionCwdError {}

pub fn assert_session_cwd_exists(
    manager: &(impl SessionCwdSource + ?Sized),
    fallback_cwd: &str,
) -> Result<(), MissingSessionCwdError> {
    match get_missing_session_cwd_issue(manager, fallback_cwd) {
        Some(issue) => Err(MissingSessionCwdError { issue }),
        None => Ok(()),
    }
}
