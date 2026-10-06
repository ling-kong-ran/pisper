use crate::agent_core::types::AgentMessage;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Append-only JSONL transcript: one AgentMessage per line. Messages
/// serialize in the upstream wire format — flat role-tagged objects
/// (`role`, camelCase fields, Unix-millisecond timestamps), custom messages
/// as their flat `{ role, ... }` payload — so each line parses as an
/// upstream AgentMessage. Note: upstream's SessionManager wraps entries in
/// an envelope (`{type:"message", id, parentId, timestamp, message}` —
/// coding-agent session-manager.ts), so the upstream SessionManager would
/// not load this file as-is; the envelope lands with M3b's harness session
/// layer. The M1-era `{"kind":"message",...}` wrapper is gone: it was a
/// port-only shape, and M1 session files are not migrated.
pub struct SessionWriter {
    file: std::fs::File,
    path: PathBuf,
}

impl SessionWriter {
    pub fn create(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        // Unix milliseconds (upstream `session-${Date.now()}.jsonl`); seconds
        // collide when two sessions start within the same second.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        let path = dir.join(format!("session-{ts}.jsonl"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(SessionWriter { file, path })
    }

    pub fn append(&mut self, message: &AgentMessage) -> anyhow::Result<()> {
        let line = serde_json::to_string(message)?;
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::types::CustomAgentMessage;
    use crate::ai::types::message::{StringOrBlocks, UserMessage};

    #[test]
    fn appends_parsable_lines_in_upstream_wire_format() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = SessionWriter::create(dir.path()).unwrap();
        s.append(&AgentMessage::User(UserMessage {
            content: StringOrBlocks::Text("hello".into()),
            timestamp: 1758240000000,
        }))
        .unwrap();
        // App-only custom messages (the M1 Notification intent) serialize as
        // their flat role-tagged payload.
        let mut notification = CustomAgentMessage::new("notification");
        notification.data.insert("text".into(), "ui".into());
        s.append(&AgentMessage::Custom(notification)).unwrap();

        let content = std::fs::read_to_string(s.path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        // Upstream-compatible wire format: flat role tag + camelCase fields.
        assert_eq!(
            lines[0],
            r#"{"role":"user","content":"hello","timestamp":1758240000000}"#
        );
        assert_eq!(lines[1], r#"{"role":"notification","text":"ui"}"#);
        match serde_json::from_str::<AgentMessage>(lines[0]).unwrap() {
            AgentMessage::User(user) => {
                assert_eq!(user.content, StringOrBlocks::Text("hello".into()));
                assert_eq!(user.timestamp, 1758240000000);
            }
            other => panic!("expected user line, got {other:?}"),
        }
        match serde_json::from_str::<AgentMessage>(lines[1]).unwrap() {
            AgentMessage::Custom(custom) => {
                assert_eq!(custom.role, "notification");
                assert_eq!(custom.data["text"], "ui");
            }
            other => panic!("expected custom line, got {other:?}"),
        }
    }

    /// The filename carries Unix milliseconds (upstream `session-${Date.now()}`),
    /// not seconds — seconds collide when two sessions start in the same second.
    #[test]
    fn filename_uses_unix_millis() {
        let dir = tempfile::tempdir().unwrap();
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let s = SessionWriter::create(dir.path()).unwrap();
        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let name = s.path().file_name().unwrap().to_string_lossy().to_string();
        let ts: u128 = name
            .strip_prefix("session-")
            .and_then(|rest| rest.strip_suffix(".jsonl"))
            .expect("session-{millis}.jsonl filename")
            .parse()
            .expect("millisecond timestamp");
        assert!(
            ts >= before && ts <= after,
            "timestamp {ts} outside [{before}, {after}]"
        );
    }
}
