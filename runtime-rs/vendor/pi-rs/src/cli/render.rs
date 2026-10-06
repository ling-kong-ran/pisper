use crate::agent_core::{AgentEvent, AgentMessage};
use crate::ai::types::events::AssistantMessageEvent;

/// Pure formatting helpers; the REPL prints their results.
pub fn tool_start_line(tool_name: &str, arguments: &serde_json::Value) -> String {
    format!("[tool] {tool_name}({arguments})")
}

pub fn tool_end_line(tool_name: &str, is_error: bool, output: &str) -> String {
    let first_line = output.lines().next().unwrap_or("").to_string();
    let tag = if is_error { "error" } else { "ok" };
    format!("[{tag}] {tool_name}: {first_line}")
}

/// First text block of a serialized `AgentToolResult` (the
/// `tool_execution_end` payload), for the one-line preview.
fn first_result_text(result: &serde_json::Value) -> &str {
    result["content"]
        .as_array()
        .and_then(|blocks| blocks.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or_default()
}

/// Print one agent event to stdout. Returns nothing; deltas print inline.
///
/// Event mapping from the M1 mini-loop surface: assistant/thinking deltas
/// arrive inside `message_update` (via the assistant stream event); the
/// trailing newline prints when the assistant message completes; stream
/// errors no longer have a dedicated event — they settle as an assistant
/// message carrying `error_message`, which the REPL surfaces after the run.
pub fn render_event(ev: &AgentEvent) {
    match ev.kind() {
        AgentEvent::AgentStart
        | AgentEvent::AgentEnd { .. }
        | AgentEvent::TurnStart
        | AgentEvent::TurnEnd { .. }
        | AgentEvent::MessageStart { .. } => {}
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => {
            if let AssistantMessageEvent::TextDelta { delta, .. } = assistant_message_event {
                use std::io::Write;
                print!("{delta}");
                let _ = std::io::stdout().flush();
            }
        }
        AgentEvent::MessageEnd { message } => {
            if matches!(message, AgentMessage::Assistant(_)) {
                println!();
            }
        }
        AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } => {
            println!("{}", tool_start_line(tool_name, args));
        }
        AgentEvent::ToolExecutionEnd {
            tool_name,
            result,
            is_error,
            ..
        } => {
            // preview line only; the full result text lives in the conversation context
            println!(
                "{}",
                tool_end_line(tool_name, *is_error, first_result_text(result))
            );
        }
        AgentEvent::ToolExecutionUpdate { .. } => {}
        AgentEvent::Preserved(_) => unreachable!("kind() unwraps ingress"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_tool_lines() {
        let start = tool_start_line("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(start, r#"[tool] bash({"command":"ls"})"#);

        let end = tool_end_line("bash", false, "file1\nfile2");
        assert_eq!(end, "[ok] bash: file1");

        let err = tool_end_line("bash", true, "boom\nmore");
        assert_eq!(err, "[error] bash: boom");

        let empty = tool_end_line("bash", false, "");
        assert_eq!(empty, "[ok] bash: ");
    }

    #[test]
    fn first_result_text_reads_the_first_text_block() {
        let result = serde_json::json!({
            "content": [
                {"type": "text", "text": "total 0"},
                {"type": "text", "text": "second"}
            ]
        });
        assert_eq!(first_result_text(&result), "total 0");
        assert_eq!(first_result_text(&serde_json::json!({"content": []})), "");
        assert_eq!(first_result_text(&serde_json::json!({})), "");
    }
}
