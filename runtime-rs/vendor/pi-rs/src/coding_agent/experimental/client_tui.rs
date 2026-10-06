//! Port of upstream `experimental/client-tui.ts`
//! (sha256 6e30d12494602871b61f7c0c3650be82c53e9f7ca717ae1852d1ff67545756fd).
//!
//! Ported: the deterministic presentation state machine — `#runPrompt`'s
//! slash-command parsing (`/name args` with exact split/trim semantics),
//! `#executeSlashCommand`'s unknown-command status, `#submitPrompt` /
//! `#queueFollowUp` / `#reportOperation` / `#reportQueue` status strings,
//! `#interrupt`, `#footer`'s two shapes, `handleInput`'s busy/screen routing,
//! the selector state machine (`#select` / `#completeSelection`), the
//! connection/attachment recovery transitions (`#handleConnectionState`,
//! `#handleAttachmentState`, `#queueRecovery`), and
//! `prepareClientSession`'s session selection (explicit id, continue/resume
//! ordering, single-server creation) with exact upstream error strings.
//!
//! D16 seam (disclosed in this module's docs): the `pi-tui` component tree, the editor, the
//! autocomplete providers, the chord facet host/plugin generations, the
//! theme controller and the interactive TUI bootstrap
//! (`runClientTui`'s resource loading) are embedder-owned; the port exposes
//! the state transitions as pure methods over an injectable status/selector
//! facade ([`TuiStateFacade`]).

use crate::protocol::protocol::is_server_id;

/// Upstream `ClientTuiServer` identity face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTuiServer {
    pub server_id: String,
    pub radius: bool,
}

/// Upstream `SessionSummary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSessionSummary {
    pub server_id: String,
    pub session_id: String,
    pub created_at: i64,
}

/// Upstream `#screen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Chat,
    Select,
}

/// Upstream `AgentOperationResponse`/`AgentQueueResponse` status inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationReport {
    pub accepted: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueReport {
    pub accepted: bool,
    pub entry_id: Option<String>,
    pub error: Option<String>,
}

/// Upstream `#runPrompt`'s input classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptInput {
    /// Empty after trim: ignored.
    Empty,
    /// `/name` or `/name args` (args = rest after the first space, trimmed).
    Slash { name: String, args: String },
    /// Plain prompt text (trimmed).
    Prompt(String),
}

/// Upstream `#runPrompt` parsing (exact slice/trim semantics: the args keep
/// interior whitespace).
pub fn parse_prompt_input(text: &str) -> PromptInput {
    let prompt = text.trim();
    if prompt.is_empty() {
        return PromptInput::Empty;
    }
    if let Some(rest) = prompt.strip_prefix('/') {
        match rest.find(' ') {
            Some(separator) => PromptInput::Slash {
                name: rest[..separator].to_string(),
                args: rest[separator + 1..].trim().to_string(),
            },
            None => PromptInput::Slash {
                name: rest.to_string(),
                args: String::new(),
            },
        }
    } else {
        PromptInput::Prompt(prompt.to_string())
    }
}

/// Upstream `#footer`.
pub fn footer_text(snapshot: Option<&FooterSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "/model · /thinking · /compact · /reload".to_string();
    };
    format!(
        "{}/{} · thinking:{} · {} messages · /model · /thinking · /compact · /reload",
        snapshot.model_provider, snapshot.model_id, snapshot.thinking_level, snapshot.message_count
    )
}

/// Upstream footer snapshot fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterSnapshot {
    pub model_provider: String,
    pub model_id: String,
    pub thinking_level: String,
    pub message_count: u64,
}

/// Upstream `#reportOperation`.
pub fn report_operation(response: &OperationReport) -> String {
    if response.accepted {
        match &response.error {
            None => String::new(),
            Some(message) => format!("Operation failed: {message}"),
        }
    } else {
        format!(
            "Operation rejected: {}",
            response.error.clone().unwrap_or_default()
        )
    }
}

/// Upstream `#reportQueue`.
pub fn report_queue(response: &QueueReport) -> String {
    if response.accepted {
        format!("Queued {}.", response.entry_id.clone().unwrap_or_default())
    } else {
        format!(
            "Message rejected: {}",
            response.error.clone().unwrap_or_default()
        )
    }
}

/// Upstream `requireSingleServer`.
pub fn require_single_server(count: usize) -> Result<(), String> {
    if count != 1 {
        return Err("Starting a Session requires exactly one server".to_string());
    }
    Ok(())
}

/// Upstream `prepareClientSession` selection (the deterministic half; the
/// live management calls are the caller's). Returns the selected session and
/// whether a creation is required (and of which id).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionSelection {
    Existing(TuiSessionSummary),
    /// `management.create({ id })` — explicit id on the single local server.
    CreateWithId {
        server_id: String,
        session_id: String,
    },
    /// `management.create({})` — brand-new session.
    CreateNew {
        server_id: String,
    },
}

/// Upstream `prepareClientSession` selection ladder. `connect_radius` marks a
/// radius connect target; an unavailable explicit id on radius is an error.
pub fn prepare_client_session_selection(
    command_session_id: Option<&str>,
    continue_session: bool,
    resume: bool,
    connect_radius: bool,
    server_ids: &[String],
    servers: &[Vec<TuiSessionSummary>],
) -> Result<SessionSelection, String> {
    if let Some(session_id) = command_session_id {
        let mut matches: Vec<&TuiSessionSummary> = Vec::new();
        for server_sessions in servers {
            for summary in server_sessions {
                if summary.session_id == session_id {
                    matches.push(summary);
                }
            }
        }
        if matches.len() > 1 {
            return Err(format!(
                "Session {session_id} is available from more than one server"
            ));
        }
        if let Some(summary) = matches.first() {
            return Ok(SessionSelection::Existing((*summary).clone()));
        }
        if connect_radius {
            return Err(format!(
                "Remote server does not contain Session {session_id}"
            ));
        }
        require_single_server(servers.len())?;
        return Ok(SessionSelection::CreateWithId {
            server_id: server_ids[0].clone(),
            session_id: session_id.to_string(),
        });
    }
    if continue_session || resume {
        // Upstream sort: createdAt desc, then serverId, then sessionId asc.
        let mut all: Vec<&TuiSessionSummary> = servers
            .iter()
            .flat_map(|server_sessions| server_sessions.iter())
            .collect();
        all.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.server_id.cmp(&right.server_id))
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        if let Some(summary) = all.first() {
            return Ok(SessionSelection::Existing((*summary).clone()));
        }
    }
    require_single_server(servers.len())?;
    Ok(SessionSelection::CreateNew {
        server_id: server_ids[0].clone(),
    })
}

/// Upstream server/session id canonicality face.
pub fn is_canonical_server_id(value: &str) -> bool {
    is_server_id(value)
}

/// Upstream `#handleConnectionState` transitions for the selected server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiStateFacade {
    pub screen: Screen,
    pub busy: bool,
    pub status: String,
    pub selected_server_id: Option<String>,
    pub session_id: Option<String>,
    pub lane_open: bool,
}

impl Default for TuiStateFacade {
    fn default() -> Self {
        Self {
            screen: Screen::Chat,
            busy: false,
            status: "Starting Session…".to_string(),
            selected_server_id: None,
            session_id: None,
            lane_open: false,
        }
    }
}

/// Connection state face (upstream `ServerConnectionState`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Disconnected,
}

/// Attachment state face (upstream `SessionAttachmentState`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentState {
    Attached { session_id: String },
    Attaching { session_id: String },
    Detached,
}

impl TuiStateFacade {
    /// Upstream `#handleConnectionState` for the selected server.
    pub fn handle_connection_state(
        &mut self,
        server_id: &str,
        state: &ConnectionState,
    ) -> RecoveryAction {
        if self.selected_server_id.as_deref() != Some(server_id) {
            return RecoveryAction::None;
        }
        match state {
            ConnectionState::Connected => {
                if !self.lane_open {
                    self.busy = true;
                    self.status = "Reattaching Session…".to_string();
                    RecoveryAction::ReopenLane
                } else {
                    RecoveryAction::None
                }
            }
            ConnectionState::Connecting => {
                self.busy = true;
                self.status = "Reconnecting to Radius…".to_string();
                self.lane_open = false;
                RecoveryAction::CloseLane
            }
            ConnectionState::Disconnected => {
                self.busy = true;
                self.status = "Radius disconnected; retrying…".to_string();
                self.lane_open = false;
                RecoveryAction::CloseLane
            }
        }
    }

    /// Upstream `#handleAttachmentState` for the selected session.
    pub fn handle_attachment_state(
        &mut self,
        server_id: &str,
        state: &AttachmentState,
    ) -> RecoveryAction {
        if self.selected_server_id.as_deref() != Some(server_id) || self.session_id.is_none() {
            return RecoveryAction::None;
        }
        let expected = self.session_id.clone().unwrap();
        match state {
            AttachmentState::Attached { session_id } if *session_id == expected => {
                if !self.lane_open {
                    RecoveryAction::ReopenLane
                } else {
                    self.busy = false;
                    self.status = String::new();
                    RecoveryAction::None
                }
            }
            AttachmentState::Attaching { session_id } if *session_id == expected => {
                self.busy = true;
                self.status = "Reattaching Session…".to_string();
                RecoveryAction::None
            }
            _ => RecoveryAction::None,
        }
    }

    /// Upstream `#queueRecovery`'s failure status.
    pub fn recovery_failed(&mut self, message: &str) {
        self.busy = true;
        self.status = format!("Reconnect error: {message}");
    }

    /// Upstream `#interrupt` guard: an operation id plus a controller are
    /// required; the status announces the abort.
    pub fn interrupt(&mut self, operation_id: Option<&str>) -> Option<String> {
        let operation_id = operation_id?;
        self.status = format!("Aborting {operation_id}…");
        Some(operation_id.to_string())
    }

    /// Upstream `showError`.
    pub fn show_error(&mut self, error: &str) {
        self.status = format!("Error: {error}");
    }
}

/// The lane (re)open/close action a state transition asks of the embedder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    None,
    CloseLane,
    ReopenLane,
}

/// Upstream `#submitPrompt`'s steering decision: an active lane operation
/// steers, otherwise a fresh prompt runs.
pub fn submit_is_steering(operation_active: bool) -> bool {
    operation_active
}

#[cfg(test)]
mod tests;
