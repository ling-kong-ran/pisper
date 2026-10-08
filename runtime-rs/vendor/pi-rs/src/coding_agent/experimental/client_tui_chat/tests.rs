//! Tests for `client_tui_chat.rs`: pinned to the node oracle
//! (tests/fixtures/experimental_final_oracle/oracle_view_out.json, `chatView`
//! section).

use super::*;
use crate::coding_agent::experimental::client::MessageContent;

#[test]
fn queue_item_text_matches_the_oracle() {
    // Oracle: chatView.queueText.
    let cases = [
        (
            QueueItem::Message {
                kind: "steer".to_string(),
                message: AgentMessage::User {
                    content: UserContent::Blocks(vec![MessageContent::Text {
                        text: "a\n b\t c".to_string(),
                    }]),
                },
            },
            "[steer] a b c",
        ),
        (
            QueueItem::Message {
                kind: "followUp".to_string(),
                message: AgentMessage::User {
                    content: UserContent::Text("plain".to_string()),
                },
            },
            "[followUp] plain",
        ),
        (
            QueueItem::Custom {
                kind: "custom".to_string(),
                custom_type: "memory.write".to_string(),
            },
            "[custom] <memory.write>",
        ),
        (
            QueueItem::Message {
                kind: "write".to_string(),
                message: AgentMessage::Assistant {
                    text: "ignored".to_string(),
                    tool_calls: vec![],
                },
            },
            "[write] ",
        ),
    ];
    for (item, expected) in cases {
        assert_eq!(queue_item_text(&item), expected);
    }
}

#[test]
fn transcript_sync_matches_the_oracle_divergence_and_append() {
    // Oracle: chatView.sync (diverged flag + appended + rendered ids).
    let entries = |ids: &[&str]| -> Vec<TranscriptEntry> {
        ids.iter()
            .map(|id| TranscriptEntry::Custom {
                id: id.to_string(),
                custom_type: "x".to_string(),
            })
            .collect()
    };

    let mut view = ExperimentalChatView::new();
    // {prev: [], transcript: [e1,e2]} -> append both.
    view.sync_transcript(&entries(&["e1", "e2"]), &mut NullSink);
    assert!(!view.rendered_entry_ids().is_empty());
    let rendered = view.rendered_entry_ids().to_vec();
    assert_eq!(rendered, vec!["e1".to_string(), "e2".to_string()]);

    // {prev: [e1], transcript: [e1,e2]} -> append e2 only.
    let mut view = ExperimentalChatView::new();
    view.sync_transcript(&entries(&["e1"]), &mut NullSink);
    view.sync_transcript(&entries(&["e1", "e2"]), &mut NullSink);
    assert_eq!(
        view.rendered_entry_ids(),
        &["e1".to_string(), "e2".to_string()]
    );

    // {prev: [e0], transcript: [e1,e2]} -> diverged rebuild.
    let mut view = ExperimentalChatView::new();
    view.sync_transcript(&entries(&["e0"]), &mut NullSink);
    view.sync_transcript(&entries(&["e1", "e2"]), &mut NullSink);
    assert_eq!(
        view.rendered_entry_ids(),
        &["e1".to_string(), "e2".to_string()]
    );

    // {prev: [e1,e2], transcript: [e1]} -> diverged rebuild (shrink).
    let mut view = ExperimentalChatView::new();
    view.sync_transcript(&entries(&["e1", "e2"]), &mut NullSink);
    view.sync_transcript(&entries(&["e1"]), &mut NullSink);
    assert_eq!(view.rendered_entry_ids(), &["e1".to_string()]);
}

#[test]
fn entry_branching_matches_the_oracle_render_sequence() {
    // Oracle: chatView.entries (draw sequence over the recording sink).
    let mut view = ExperimentalChatView::new();
    let mut sink = RecordingSink::default();
    view.add_entry(
        &TranscriptEntry::Compaction {
            id: "c1".to_string(),
            tokens_before: 4321,
            retained_tail: vec![AgentMessage::User {
                content: UserContent::Blocks(vec![MessageContent::Text {
                    text: "kept".to_string(),
                }]),
            }],
        },
        &mut sink,
    );
    view.add_entry(
        &TranscriptEntry::BranchSummary {
            id: "b1".to_string(),
            summary: "the branch did things".to_string(),
        },
        &mut sink,
    );
    view.add_entry(
        &TranscriptEntry::Custom {
            id: "u1".to_string(),
            custom_type: "pi.notice".to_string(),
        },
        &mut sink,
    );
    view.add_entry(
        &TranscriptEntry::Message {
            id: "m1".to_string(),
            message: AgentMessage::User {
                content: UserContent::Text("hi".to_string()),
            },
        },
        &mut sink,
    );
    view.add_entry(
        &TranscriptEntry::Message {
            id: "m2".to_string(),
            message: AgentMessage::Assistant {
                text: "hey".to_string(),
                tool_calls: vec![ToolCallRef {
                    name: "read".to_string(),
                    id: "call-1".to_string(),
                    arguments: Some(serde_json::json!({})),
                }],
            },
        },
        &mut sink,
    );
    view.add_entry(
        &TranscriptEntry::Message {
            id: "m3".to_string(),
            message: AgentMessage::ToolResult {
                tool_name: "read".to_string(),
                tool_call_id: "call-1".to_string(),
            },
        },
        &mut sink,
    );

    let rendered = sink.rendered_text();
    assert_eq!(
        rendered,
        vec![
            "[compaction] compacted from 4321 tokens",
            "user:kept",
            "[branch summary]",
            "the branch did things",
            "[pi.notice]",
            "user:hi",
            "assistant:hey",
            "toolCall:read:call-1",
            "toolResult:read:call-1",
        ]
    );
}

#[test]
fn working_indicator_transitions_only_on_change() {
    // Upstream #setWorking: same-state is a no-op.
    let mut view = ExperimentalChatView::new();
    let mut sink = RecordingSink::default();
    view.set_working(true, &mut sink);
    assert_eq!(sink.working_calls, vec![true]);
    view.set_working(true, &mut sink);
    assert_eq!(sink.working_calls, vec![true]);
    view.set_working(false, &mut sink);
    assert_eq!(sink.working_calls, vec![true, false]);
    assert!(!view.is_working());
}

#[test]
fn streaming_and_tool_registry_follow_upstream_sequencing() {
    let mut view = ExperimentalChatView::new();
    let mut sink = RecordingSink::default();

    // Streaming assistant parks a component; tool calls register by id.
    view.sync_streaming(
        true,
        &[ToolCallRef {
            name: "bash".to_string(),
            id: "call-9".to_string(),
            arguments: Some(serde_json::json!({"command": "ls"})),
        }],
        &mut sink,
    );
    assert!(sink.commands.contains(&DrawCommand::Assistant {
        text: String::new(),
        streaming: true,
    }));
    assert!(sink.commands.contains(&DrawCommand::Tool(ToolDraw::Create {
        tool_name: "bash".to_string(),
        tool_call_id: "call-9".to_string(),
        args: Some(serde_json::json!({"command": "ls"})),
    })));
    assert_eq!(view.tool_call_ids(), &["call-9".to_string()]);

    // An existing tool with new args updates instead of recreating.
    view.tool(
        "bash",
        "call-9",
        Some(&serde_json::json!({"command": "ls -la"})),
        &mut sink,
    );
    assert!(sink
        .commands
        .contains(&DrawCommand::Tool(ToolDraw::UpdateArgs {
            tool_call_id: "call-9".to_string(),
            args: serde_json::json!({"command": "ls -la"}),
        })));

    // Settled assistant message adopts (clears) the streaming component.
    view.add_message(
        &AgentMessage::Assistant {
            text: "done".to_string(),
            tool_calls: vec![],
        },
        &mut sink,
    );
    assert!(sink.commands.contains(&DrawCommand::Assistant {
        text: "done".to_string(),
        streaming: false,
    }));
    // A second settled assistant creates another component (streaming
    // already adopted), never a duplicate adoption.
    view.add_message(
        &AgentMessage::Assistant {
            text: "more".to_string(),
            tool_calls: vec![],
        },
        &mut sink,
    );
}

#[test]
fn reset_matches_refresh_theme() {
    // Upstream refreshTheme: drop generations and replay from scratch.
    let mut view = ExperimentalChatView::new();
    view.sync_transcript(
        &[TranscriptEntry::Custom {
            id: "e1".to_string(),
            custom_type: "x".to_string(),
        }],
        &mut NullSink,
    );
    view.set_working(true, &mut NullSink);
    view.reset();
    assert!(view.rendered_entry_ids().is_empty());
    assert!(view.tool_call_ids().is_empty());
    assert!(!view.is_working());
}

/// Sink that discards commands (for flows whose text is not asserted).
struct NullSink;

impl ChatViewSink for NullSink {
    fn draw(&mut self, _command: &DrawCommand) {}
}

/// Recording sink rendering the oracle's text sequence.
#[derive(Default)]
struct RecordingSink {
    commands: Vec<DrawCommand>,
    working_calls: Vec<bool>,
}

impl ChatViewSink for RecordingSink {
    fn draw(&mut self, command: &DrawCommand) {
        if let DrawCommand::Working(working) = command {
            self.working_calls.push(*working);
        }
        self.commands.push(command.clone());
    }
}

impl RecordingSink {
    fn rendered_text(&self) -> Vec<String> {
        let mut rendered = Vec::new();
        for command in &self.commands {
            match command {
                DrawCommand::Text(text) => rendered.push(text.clone()),
                DrawCommand::Spacer => {
                    // Upstream emits a spacer before user messages; the oracle
                    // records only the message itself.
                }
                DrawCommand::UserMessage(text) => rendered.push(format!("user:{text}")),
                DrawCommand::Assistant { text, streaming } => {
                    if !streaming {
                        rendered.push(format!("assistant:{text}"));
                    }
                }
                DrawCommand::Tool(ToolDraw::SetArgsComplete { tool_call_id }) => {
                    // Recover the tool name from the create command order.
                    let name = self
                        .commands
                        .iter()
                        .find_map(|command| match command {
                            DrawCommand::Tool(ToolDraw::Create {
                                tool_name,
                                tool_call_id: id,
                                ..
                            }) if id == tool_call_id => Some(tool_name.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    rendered.push(format!("toolCall:{name}:{tool_call_id}"));
                }
                DrawCommand::Tool(ToolDraw::UpdateResult { tool_call_id, .. }) => {
                    let name = self
                        .commands
                        .iter()
                        .find_map(|command| match command {
                            DrawCommand::Tool(ToolDraw::Create {
                                tool_name,
                                tool_call_id: id,
                                ..
                            }) if id == tool_call_id => Some(tool_name.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    rendered.push(format!("toolResult:{name}:{tool_call_id}"));
                }
                _ => {}
            }
        }
        rendered
    }
}
