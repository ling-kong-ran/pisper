//! Port of upstream `experimental/client-tui-chat.ts`
//! (sha256 ca088b5d58711be5ce997fc76fef6ddf4d74ec5dd0bf0b17ae8e788fb9428ebf).
//!
//! Ported: the snapshot-driven transcript data model — `userMessageText`,
//! the queue text transform (whitespace collapse, `[kind] text` /
//! `<customType>` shapes), `#syncTranscript` divergence detection and
//! append-only replay, `#addEntry`'s compaction/branch-summary/custom/message
//! branching, the streaming-component adoption protocol, the tool-component
//! registry keyed by tool call id, and the working-indicator state switch.
//!
//! D15 seam (disclosed in this module's docs): the actual draw components (`pi-tui`
//! Container/Text/TruncatedText, `AssistantMessageComponent`,
//! `ToolExecutionComponent`, `WorkingStatusIndicator`, the tool renderer
//! registry and the interactive theme) are the
//! [`crate::tui`] component face, embedder-wired behind the [`ChatViewSink`]
//! trait; the port owns the deterministic sequencing of draw commands and
//! every rendered text.

use crate::coding_agent::experimental::client::MessageContent;

/// Upstream `Entry` face (only the fields the view reads).
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEntry {
    Compaction {
        id: String,
        tokens_before: i64,
        retained_tail: Vec<AgentMessage>,
    },
    BranchSummary {
        id: String,
        summary: String,
    },
    Custom {
        id: String,
        custom_type: String,
    },
    Message {
        id: String,
        message: AgentMessage,
    },
}

impl TranscriptEntry {
    pub fn id(&self) -> &str {
        match self {
            TranscriptEntry::Compaction { id, .. }
            | TranscriptEntry::BranchSummary { id, .. }
            | TranscriptEntry::Custom { id, .. }
            | TranscriptEntry::Message { id, .. } => id,
        }
    }
}

/// Upstream `AgentMessage` face (only what the view reads).
#[derive(Debug, Clone, PartialEq)]
pub enum AgentMessage {
    User {
        content: UserContent,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallRef>,
    },
    ToolResult {
        tool_name: String,
        tool_call_id: String,
    },
}

/// Upstream user message content: a plain string or a block list.
#[derive(Debug, Clone, PartialEq)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<MessageContent>),
}

/// Upstream tool call reference inside an assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRef {
    pub name: String,
    pub id: String,
    /// Serialized arguments (upstream `content.arguments`).
    pub arguments: Option<serde_json::Value>,
}

/// Upstream `LaneSnapshot.queues` item face.
#[derive(Debug, Clone, PartialEq)]
pub enum QueueItem {
    /// `{ kind, type: "message", message }`.
    Message { kind: String, message: AgentMessage },
    /// `{ kind, type: "custom", customType }`.
    Custom { kind: String, custom_type: String },
}

/// Upstream running-tool slot face.
#[derive(Debug, Clone, PartialEq)]
pub struct RunningToolSlot {
    pub tool_name: String,
    pub tool_call_id: String,
    pub args: Option<serde_json::Value>,
    pub status: ToolSlotStatus,
    pub is_error: bool,
    pub has_result: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSlotStatus {
    Running,
    Done,
}

/// Upstream `LaneSnapshot` face (only what the view reads).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaneSnapshotView {
    pub transcript: Vec<TranscriptEntry>,
    pub has_streaming_message: bool,
    pub streaming_tool_calls: Vec<ToolCallRef>,
    pub running_tools: Vec<RunningToolSlot>,
    pub queues: Vec<QueueItem>,
    pub operation_active: bool,
}

/// Upstream `userMessageText`.
pub fn user_message_text(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User { content } => match content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    MessageContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect(),
        },
        _ => String::new(),
    }
}

/// Upstream `#syncQueues` text transform: `[kind] text` with whitespace
/// collapsed for messages, `<customType>` for custom entries.
pub fn queue_item_text(item: &QueueItem) -> String {
    let text = match item {
        QueueItem::Message { message, .. } => collapse_whitespace(&user_message_text(message)),
        QueueItem::Custom { custom_type, .. } => format!("<{custom_type}>"),
    };
    match item {
        QueueItem::Message { kind, .. } | QueueItem::Custom { kind, .. } => {
            format!("[{kind}] {text}")
        }
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !in_space {
                collapsed.push(' ');
                in_space = true;
            }
        } else {
            collapsed.push(character);
            in_space = false;
        }
    }
    collapsed
}

/// One deterministic draw command handed to the component face (D15).
#[derive(Debug, Clone, PartialEq)]
pub enum DrawCommand {
    /// `transcript.addChild(new Spacer(1))`.
    Spacer,
    /// Plain muted text line (`#addText`).
    Text(String),
    /// `UserMessageComponent(text)`.
    UserMessage(String),
    /// `AssistantMessageComponent.updateContent(message, streaming)`; the
    /// bool is upstream's `streaming` flag.
    Assistant { text: String, streaming: bool },
    /// `ToolExecutionComponent` lifecycle keyed by tool call id.
    Tool(ToolDraw),
    /// Working indicator state (upstream `#setWorking`).
    Working(bool),
    /// Invalidate/repaint request (upstream `invalidate()` calls).
    Invalidate,
    /// `Container.clear()` on the given region.
    Clear(ChatRegion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRegion {
    Transcript,
    PendingMessages,
    Status,
}

/// Upstream `ToolExecutionComponent` method calls.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolDraw {
    Create {
        tool_name: String,
        tool_call_id: String,
        args: Option<serde_json::Value>,
    },
    UpdateArgs {
        tool_call_id: String,
        args: serde_json::Value,
    },
    SetArgsComplete {
        tool_call_id: String,
    },
    MarkExecutionStarted {
        tool_call_id: String,
    },
    UpdateResult {
        tool_call_id: String,
        is_error: bool,
        streaming: bool,
    },
}

/// The draw face: upstream's `Container` children sequence (D15 seam).
pub trait ChatViewSink {
    fn draw(&mut self, command: &DrawCommand);
}

/// Upstream `ExperimentalChatView` deterministic state machine.
pub struct ExperimentalChatView {
    rendered_entry_ids: Vec<String>,
    tools: Vec<String>,
    streaming_active: bool,
    working: bool,
}

impl Default for ExperimentalChatView {
    fn default() -> Self {
        Self::new()
    }
}

impl ExperimentalChatView {
    pub fn new() -> Self {
        Self {
            rendered_entry_ids: Vec::new(),
            tools: Vec::new(),
            streaming_active: false,
            working: false,
        }
    }

    /// Upstream `refreshTheme`: drop every generation and replay.
    pub fn reset(&mut self) {
        self.rendered_entry_ids.clear();
        self.tools.clear();
        self.streaming_active = false;
        self.working = false;
    }

    pub fn rendered_entry_ids(&self) -> &[String] {
        &self.rendered_entry_ids
    }

    pub fn tool_call_ids(&self) -> &[String] {
        &self.tools
    }

    pub fn is_working(&self) -> bool {
        self.working
    }

    /// Upstream `apply(snapshot)` over the sink.
    pub fn apply(&mut self, snapshot: &LaneSnapshotView, sink: &mut dyn ChatViewSink) {
        self.sync_transcript(&snapshot.transcript, sink);
        self.sync_streaming(
            snapshot.has_streaming_message,
            &snapshot.streaming_tool_calls,
            sink,
        );
        for tool in &snapshot.running_tools {
            self.tool(
                &tool.tool_name,
                &tool.tool_call_id,
                tool.args.as_ref(),
                sink,
            );
            if tool.status == ToolSlotStatus::Running {
                sink.draw(&DrawCommand::Tool(ToolDraw::MarkExecutionStarted {
                    tool_call_id: tool.tool_call_id.clone(),
                }));
                if tool.has_result {
                    sink.draw(&DrawCommand::Tool(ToolDraw::UpdateResult {
                        tool_call_id: tool.tool_call_id.clone(),
                        is_error: false,
                        streaming: true,
                    }));
                }
            } else {
                sink.draw(&DrawCommand::Tool(ToolDraw::UpdateResult {
                    tool_call_id: tool.tool_call_id.clone(),
                    is_error: tool.is_error,
                    streaming: false,
                }));
            }
        }
        self.sync_queues(&snapshot.queues, sink);
        self.set_working(snapshot.operation_active, sink);
        sink.draw(&DrawCommand::Invalidate);
    }

    /// Upstream `#syncQueues`.
    pub fn sync_queues(&self, queues: &[QueueItem], sink: &mut dyn ChatViewSink) {
        sink.draw(&DrawCommand::Clear(ChatRegion::PendingMessages));
        for item in queues {
            sink.draw(&DrawCommand::Text(queue_item_text(item)));
        }
    }

    /// Upstream `#syncTranscript`: append-only with divergence rebuild.
    pub fn sync_transcript(&mut self, transcript: &[TranscriptEntry], sink: &mut dyn ChatViewSink) {
        let diverged = self
            .rendered_entry_ids
            .iter()
            .enumerate()
            .any(|(index, id)| transcript.get(index).map(|entry| entry.id()) != Some(id.as_str()));
        if diverged {
            sink.draw(&DrawCommand::Clear(ChatRegion::Transcript));
            self.tools.clear();
            self.rendered_entry_ids.clear();
            self.streaming_active = false;
        }
        for entry in transcript.iter().skip(self.rendered_entry_ids.len()) {
            self.add_entry(entry, sink);
            self.rendered_entry_ids.push(entry.id().to_string());
        }
    }

    /// Upstream `#addEntry`.
    pub fn add_entry(&mut self, entry: &TranscriptEntry, sink: &mut dyn ChatViewSink) {
        match entry {
            TranscriptEntry::Compaction {
                tokens_before,
                retained_tail,
                ..
            } => {
                sink.draw(&DrawCommand::Text(format!(
                    "[compaction] compacted from {tokens_before} tokens"
                )));
                for retained in retained_tail {
                    self.add_message(retained, sink);
                }
            }
            TranscriptEntry::BranchSummary { summary, .. } => {
                sink.draw(&DrawCommand::Text("[branch summary]".to_string()));
                sink.draw(&DrawCommand::Text(summary.clone()));
            }
            TranscriptEntry::Custom { custom_type, .. } => {
                sink.draw(&DrawCommand::Text(format!("[{custom_type}]")));
            }
            TranscriptEntry::Message { message, .. } => self.add_message(message, sink),
        }
    }

    /// Upstream `#addMessage`.
    pub fn add_message(&mut self, message: &AgentMessage, sink: &mut dyn ChatViewSink) {
        match message {
            AgentMessage::User { .. } => {
                sink.draw(&DrawCommand::Spacer);
                sink.draw(&DrawCommand::UserMessage(user_message_text(message)));
            }
            AgentMessage::Assistant { text, tool_calls } => {
                // Adopt the streaming component if one is parked.
                let adopted = self.streaming_active;
                self.streaming_active = false;
                sink.draw(&DrawCommand::Assistant {
                    text: text.clone(),
                    streaming: false,
                });
                let _ = adopted;
                for call in tool_calls {
                    self.tool(&call.name, &call.id, call.arguments.as_ref(), sink);
                    sink.draw(&DrawCommand::Tool(ToolDraw::SetArgsComplete {
                        tool_call_id: call.id.clone(),
                    }));
                }
            }
            AgentMessage::ToolResult {
                tool_name,
                tool_call_id,
            } => {
                self.tool(tool_name, tool_call_id, None, sink);
                sink.draw(&DrawCommand::Tool(ToolDraw::UpdateResult {
                    tool_call_id: tool_call_id.clone(),
                    is_error: false,
                    streaming: false,
                }));
            }
        }
    }

    /// Upstream `#syncStreaming`.
    pub fn sync_streaming(
        &mut self,
        has_message: bool,
        tool_calls: &[ToolCallRef],
        sink: &mut dyn ChatViewSink,
    ) {
        if !has_message {
            return;
        }
        if !self.streaming_active {
            self.streaming_active = true;
        }
        sink.draw(&DrawCommand::Assistant {
            text: String::new(),
            streaming: true,
        });
        for call in tool_calls {
            self.tool(&call.name, &call.id, call.arguments.as_ref(), sink);
        }
    }

    /// Upstream `#tool`: get or create the component for a tool call; omit
    /// args to look up without overwriting them.
    pub fn tool(
        &mut self,
        tool_name: &str,
        tool_call_id: &str,
        args: Option<&serde_json::Value>,
        sink: &mut dyn ChatViewSink,
    ) {
        if self.tools.iter().any(|id| id == tool_call_id) {
            if let Some(args) = args {
                sink.draw(&DrawCommand::Tool(ToolDraw::UpdateArgs {
                    tool_call_id: tool_call_id.to_string(),
                    args: args.clone(),
                }));
            }
            return;
        }
        sink.draw(&DrawCommand::Tool(ToolDraw::Create {
            tool_name: tool_name.to_string(),
            tool_call_id: tool_call_id.to_string(),
            args: args.cloned(),
        }));
        self.tools.push(tool_call_id.to_string());
    }

    /// Upstream `#setWorking`: only transitions on change.
    pub fn set_working(&mut self, working: bool, sink: &mut dyn ChatViewSink) {
        if working == self.working {
            return;
        }
        self.working = working;
        sink.draw(&DrawCommand::Clear(ChatRegion::Status));
        sink.draw(&DrawCommand::Working(working));
    }
}

#[cfg(test)]
mod tests;
