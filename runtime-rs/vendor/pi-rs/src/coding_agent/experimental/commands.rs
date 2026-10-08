//! Port of upstream `experimental/commands.ts`
//! (sha256 5e9763c2a8774ee1fc0a2c28be69bc46464f6067c6ed2b6f575208d2759e35a5).
//!
//! Ported: `runExperimentalCommand`'s dispatch gate (experimental flag +
//! `server`/`client` argument, exact `Error: ` console face), the
//! `runServerCommand` relay-status description chain (including its
//! same-description and `connecting` skip rules), the `runClientCommand`
//! output formatting (attached/prompted/list lines, the streamed-text
//! trailing newline), and the CLI execute error face.
//!
//! D13 seam (disclosed in this module's docs): the live command handlers
//! (`startForegroundServer`, `runClient`, `runClientTui`), the CLI parser
//! (`cli/experimental/cli.ts`) and `process.exitCode`/signal handling are
//! embedder-owned; the port owns every user-visible string and decision.

use crate::coding_agent::experimental::radius_relay::RadiusRelayHostStatus;

/// Upstream `runServerCommand`'s `reportRelayStatus`: compute the
/// description and whether it should be printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayStatusReport {
    pub description: String,
    /// Upstream `return` before printing (same description or connecting).
    pub skipped: bool,
}

/// Upstream description chain:
/// connected | not connected; local only | reconnecting: <error> | connecting.
pub fn describe_relay_status(
    previous: Option<&str>,
    status: &RadiusRelayHostStatus,
) -> RelayStatusReport {
    let description = match status {
        RadiusRelayHostStatus::Connected => "connected".to_string(),
        RadiusRelayHostStatus::NotAuthenticated => "not connected; local only".to_string(),
        RadiusRelayHostStatus::Retrying { error } => format!("reconnecting: {error}"),
        RadiusRelayHostStatus::Connecting => "connecting".to_string(),
    };
    let skipped = Some(description.as_str()) == previous
        || matches!(status, RadiusRelayHostStatus::Connecting);
    RelayStatusReport {
        description,
        skipped,
    }
}

/// Upstream `reportRelayStatus`'s `previousRelayStatus` bookkeeping.
#[derive(Default)]
pub struct RelayStatusPrinter {
    previous: Option<String>,
}

impl RelayStatusPrinter {
    /// Returns the line to print (`Radius: <description>`), or `None` when
    /// upstream skips.
    pub fn report(&mut self, status: &RadiusRelayHostStatus) -> Option<String> {
        let report = describe_relay_status(self.previous.as_deref(), status);
        if report.skipped {
            return None;
        }
        self.previous = Some(report.description.clone());
        Some(format!("Radius: {}", report.description))
    }
}

/// Upstream `runExperimentalCommand`'s gate: only `server` or `client` run
/// when experimental features are enabled; everything else reports "not
/// handled".
pub fn run_experimental_command_gate(enabled: bool, args: &[String]) -> Result<bool, String> {
    if !enabled
        || (args.first().map(String::as_str) != Some("server")
            && args.first().map(String::as_str) != Some("client"))
    {
        return Ok(false);
    }
    Ok(true)
}

/// Upstream CLI execute failure face: each error prints as
/// `Error: <error>` and sets the exit code.
pub fn cli_error_lines(errors: &[String]) -> Vec<String> {
    errors
        .iter()
        .map(|error| format!("Error: {error}"))
        .collect()
}

/// Upstream catch face for a thrown error value.
pub fn thrown_error_line(message: &str) -> String {
    format!("Error: {message}")
}

/// Upstream `runClientCommand` output decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientCommandOutput {
    /// Lines written to stdout via `console.log`.
    Lines(Vec<String>),
    /// Raw `process.stdout.write` payloads (streamed deltas + final newline).
    Writes(Vec<String>),
}

/// Upstream `runClientCommand`'s result tail (with the streamed-text flag
/// captured from the event callback).
pub fn client_command_output(
    result: &crate::coding_agent::experimental::client::ClientResult,
    streamed_text: bool,
) -> ClientCommandOutput {
    let mut lines = Vec::new();
    let mut writes = Vec::new();
    match result {
        crate::coding_agent::experimental::client::ClientResult::List { sessions } => {
            for session in sessions {
                lines.push(format!("{}\t{}", session.server_id, session.session_id));
            }
        }
        crate::coding_agent::experimental::client::ClientResult::Attached {
            server_id,
            session_id,
        } => {
            lines.push(format!("{server_id}\t{session_id}\tattached"));
        }
        crate::coding_agent::experimental::client::ClientResult::Prompted {
            server_id: _,
            session_id: _,
            text,
        } => {
            if streamed_text {
                writes.push("\n".to_string());
            } else {
                lines.push(text.clone());
            }
        }
    }
    if !writes.is_empty() {
        ClientCommandOutput::Writes(writes)
    } else {
        ClientCommandOutput::Lines(lines)
    }
}

/// Upstream `onEvent` streaming filter: only `message_update` events with a
/// `text_delta` frame stream to stdout.
pub fn streams_to_stdout(event_type: &str, frame_type: Option<&str>) -> bool {
    event_type == "message_update" && frame_type == Some("text_delta")
}

#[cfg(test)]
mod tests;
