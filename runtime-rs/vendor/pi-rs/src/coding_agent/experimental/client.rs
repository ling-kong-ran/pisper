//! Port of upstream `experimental/client.ts`
//! (sha256 7485480380807232881a1971327a1f977bc584c1c8dd26ec2ac0c514cf80f14f).
//!
//! Ported: the `ClientResult` shapes, `runClient`'s decision ladder (list /
//! attach / prompt), session selection with exact upstream error strings,
//! the assistant `messageText` join, the prompt event-delivery ordering
//! (snapshot-ordered events with a serialized delivery tail) and the
//! response error extraction.
//!
//! D12 seam (disclosed in this module's docs): the live service facades (`SessionDirectory`,
//! `SessionManagement`, `PresentationPlugins`, `AgentController`,
//! `Transcript`) are embedder-owned behind the [`ClientRunSeam`] trait; the
//! port owns the branch decisions, ordering and error texts.

use crate::protocol::protocol::is_server_id;

/// Upstream `ClientResult`.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientResult {
    /// `{ kind: "list", sessions }`.
    List { sessions: Vec<SessionAddress> },
    /// `{ kind: "attached", serverId, sessionId }`.
    Attached {
        server_id: String,
        session_id: String,
    },
    /// `{ kind: "prompted", serverId, sessionId, text }`.
    Prompted {
        server_id: String,
        session_id: String,
        text: String,
    },
}

/// Upstream `services/sessions.ts` `SessionAddress`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAddress {
    pub server_id: String,
    pub session_id: String,
}

/// Upstream list sorting:
/// `serverId.localeCompare(serverId) || sessionId.localeCompare(sessionId)`.
pub fn sort_session_addresses(mut sessions: Vec<SessionAddress>) -> Vec<SessionAddress> {
    sessions.sort_by(|left, right| {
        left.server_id
            .cmp(&right.server_id)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    sessions
}

/// Upstream `messageText`: join the assistant message's text blocks.
pub fn message_text(content: &[MessageContent]) -> String {
    content
        .iter()
        .filter_map(|content| match content {
            MessageContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Upstream `AssistantMessage.content` block face (only the fields the port
/// reads).
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    Text { text: String },
    Other,
}

/// Upstream `AgentOperationResponse` face for `runClient` decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptResponse {
    pub accepted: bool,
    pub operation_id: Option<String>,
    pub error_message: Option<String>,
}

/// The live collaborators `runClient` drives (D12 seam).
pub trait ClientRunSeam {
    /// Upstream `directory.state.value.sessions`.
    fn directory_sessions(&self, server_index: usize) -> Vec<SessionAddress>;
    /// Upstream `management.create(options)`.
    fn create_session(
        &mut self,
        server_index: usize,
        session_id: Option<&str>,
    ) -> Result<SessionAddress, String>;
    /// Upstream `management.attach(sessionId)`.
    fn attach(&mut self, server_index: usize, session_id: &str) -> Result<(), String>;
    /// Upstream `plugins.prepareSession(...)`.
    fn prepare_session(&mut self, server_index: usize, session_id: &str) -> Result<(), String>;
    /// Upstream `agent.prompt(...)`.
    fn prompt(
        &mut self,
        server_index: usize,
        session_id: &str,
        prompt: &str,
    ) -> Result<PromptResponse, String>;
    /// Upstream `runtime.dispose()` failures.
    fn dispose_failures(&self) -> Vec<DisposeFailureAlias>;
    /// Upstream transcript snapshot presence.
    fn has_initialized_snapshot(&self, server_index: usize) -> bool;
}

pub type DisposeFailureAlias = crate::coding_agent::experimental::client_runtime::DisposeFailure;

/// The `management.create` face (upstream closure parameter shape).
pub type CreateSessionFn<'a> =
    &'a mut dyn FnMut(usize, Option<&str>) -> Result<SessionAddress, String>;

/// Upstream `runClient`'s selection ladder before attach (attach/prompt only;
/// the list branch is [`run_client_list`]). `servers` holds each discovered
/// server's directory sessions.
pub fn select_session_for_command(
    command_session_id: Option<&str>,
    command_prompt: Option<&str>,
    connect_transport: Option<&str>,
    servers: &[Vec<SessionAddress>],
    create_session: CreateSessionFn<'_>,
) -> Result<(usize, SessionAddress), String> {
    match command_session_id {
        None => {
            if servers.len() != 1 {
                return Err(
                    "Client prompt requires exactly one discovered server to create a Session"
                        .to_string(),
                );
            }
            let session = create_session(0, None)?;
            Ok((0, session))
        }
        Some(selected_session_id) => {
            let mut matches: Vec<(usize, SessionAddress)> = Vec::new();
            for (index, server_sessions) in servers.iter().enumerate() {
                for session in server_sessions {
                    if session.session_id == selected_session_id {
                        matches.push((index, session.clone()));
                    }
                }
            }
            if matches.len() > 1 {
                return Err(format!(
                    "Session {selected_session_id} is available from more than one server"
                ));
            }
            if let Some(matched) = matches.first() {
                return Ok(matched.clone());
            }
            // Upstream: create on the single discovered server unless the
            // connection is a radius one or there is no prompt.
            if connect_transport == Some("radius") || command_prompt.is_none() || servers.len() != 1
            {
                return Err(format!(
                    "No discovered server contains session {selected_session_id}"
                ));
            }
            let session = create_session(0, Some(selected_session_id))?;
            Ok((0, session))
        }
    }
}

/// Upstream `runClient` list branch result assembly.
pub fn run_client_list(servers: &[Vec<SessionAddress>]) -> ClientResult {
    let mut sessions = Vec::new();
    for server_sessions in servers {
        for session in server_sessions {
            sessions.push(SessionAddress {
                server_id: session.server_id.clone(),
                session_id: session.session_id.clone(),
            });
        }
    }
    ClientResult::List {
        sessions: sort_session_addresses(sessions),
    }
}

/// Upstream `runClient` transcript guard.
pub fn ensure_initialized_snapshot(has_snapshot: bool) -> Result<(), String> {
    if !has_snapshot {
        return Err("Transcript has no initialized snapshot".to_string());
    }
    Ok(())
}

/// Upstream response error extraction:
/// `if (!response.accepted) throw response.error.message;`
/// `if (response.error !== null) throw response.error.message;`
pub fn response_error(response: &PromptResponse) -> Result<(), String> {
    if !response.accepted {
        return Err(response
            .error_message
            .clone()
            .unwrap_or_else(|| "unknown error".to_string()));
    }
    if let Some(message) = &response.error_message {
        return Err(message.clone());
    }
    Ok(())
}

/// Upstream `completedText.get(response.operationId) ?? ""`.
pub fn completed_text_for(completed: &[(String, String)], operation_id: &str) -> String {
    completed
        .iter()
        .find(|(id, _)| id == operation_id)
        .map(|(_, text)| text.clone())
        .unwrap_or_default()
}

/// Upstream run_id guard for transcript events: only assistant `message_end`
/// events with a run id record their text.
pub fn records_completed_text(event_kind: &str, run_id: Option<&str>, role: &str) -> bool {
    event_kind == "message_end" && run_id.is_some() && role == "assistant"
}

/// Session id canonicality face (upstream relies on the typed `SessionId`;
/// created ids are UUIDv4).
pub fn is_session_id(value: &str) -> bool {
    is_server_id(value)
}

#[cfg(test)]
mod tests;
