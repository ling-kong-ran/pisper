//! Inline test module for [`super`] (kept in a sibling file with
//! `#[cfg(test)] #[path]`, the `session_picker` precedent, so the long test
//! bodies can be authored with whole-file tooling).

use super::*;
use crate::agent_core::CustomAgentMessage;
use crate::ai::types::message::UserMessage;
use crate::ai::types::message::{AssistantMessage, ToolResultMessage};
use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};
use crate::coding_agent::session_manager::{
    BranchSummaryEntry, CompactionEntry, CustomEntry, CustomMessageEntry, MessageEntry,
    ModelChangeEntry, SessionInfoEntry, ThinkingLevelChangeEntry,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The oracle's FakeDate instant (`tests/fixtures/interactive_r20_components_oracle`).
const FIXED_NOW: i64 = 1_780_000_000_000;

fn fixed_now() -> i64 {
    FIXED_NOW
}

fn theme() -> Arc<Theme> {
    Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark theme"))
}

// Upstream test isolation note: upstream resets the process-global
// keybinding singleton per test (`setKeybindings(new KeybindingsManager())`).
// The port's tests never mutate the global registry (concurrent test threads
// share it); the default registry already carries the tui.* defaults and
// [`super::keybindings_match`] resolves `app.*` ids from the merged
// coding-agent table, so every binding resolves identically.
// -- fixture helpers (test/tree-selector.test.ts) ---------------------------

fn user_message(id: &str, parent_id: Option<&str>, content: &str) -> SessionEntry {
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        message: AgentMessage::User(UserMessage {
            content: StringOrBlocks::Text(content.to_string()),
            timestamp: FIXED_NOW,
        }),
    })
}

fn assistant_message(id: &str, parent_id: Option<&str>, text: &str) -> SessionEntry {
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        message: AgentMessage::Assistant(AssistantMessage {
            content: vec![AssistantBlock::Text(crate::ai::types::TextContent {
                text: text.to_string(),
                text_signature: None,
            })],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: crate::ai::types::Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: FIXED_NOW,
        }),
    })
}

/// Tool-call-only assistant message (filtered out in default mode).
fn tool_call_only_assistant(id: &str, parent_id: Option<&str>) -> SessionEntry {
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        message: AgentMessage::Assistant(AssistantMessage {
            content: vec![AssistantBlock::ToolCall(crate::ai::types::ToolCall {
                id: format!("tc-{id}"),
                name: "read".to_string(),
                arguments: serde_json::json!({ "path": "test.ts" }),
                thought_signature: None,
                namespace: None,
            })],
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: crate::ai::types::Usage::default(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: FIXED_NOW,
        }),
    })
}

fn assistant_raw(
    id: &str,
    parent_id: Option<&str>,
    content: Vec<AssistantBlock>,
    stop_reason: StopReason,
    error_message: Option<&str>,
) -> SessionEntry {
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        message: AgentMessage::Assistant(AssistantMessage {
            content,
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: crate::ai::types::Usage::default(),
            stop_reason,
            deferred: None,
            error_message: error_message.map(str::to_string),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: FIXED_NOW,
        }),
    })
}

fn tool_result_message(
    id: &str,
    parent_id: Option<&str>,
    tool_call_id: &str,
    tool_name: &str,
) -> SessionEntry {
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        message: AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: vec![TextOrImageBlock::Text(crate::ai::types::TextContent {
                text: "file body".to_string(),
                text_signature: None,
            })],
            details: None,
            usage: None,
            is_error: false,
            timestamp: FIXED_NOW,
        }),
    })
}

fn bash_execution_message(id: &str, parent_id: Option<&str>, command: &str) -> SessionEntry {
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        message: AgentMessage::Custom(CustomAgentMessage {
            role: "bashExecution".to_string(),
            data: [
                ("command".to_string(), Value::String(command.to_string())),
                ("timestamp".to_string(), Value::from(FIXED_NOW)),
            ]
            .into_iter()
            .collect(),
        }),
    })
}

fn model_change(id: &str, parent_id: Option<&str>) -> SessionEntry {
    SessionEntry::ModelChange(ModelChangeEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        provider: "anthropic".to_string(),
        model_id: "claude-sonnet-4".to_string(),
    })
}

fn thinking_level_change(id: &str, parent_id: Option<&str>) -> SessionEntry {
    SessionEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2026-05-28T21:46:40.000Z".to_string(),
        thinking_level: "high".to_string(),
    })
}

/// Upstream `buildTree` test helper: link by id first (JS objects are
/// references), then materialize the owned tree.
fn build_tree(entries: Vec<SessionEntry>) -> Vec<SessionTreeNode> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut children_of: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Some(id) = entry.id() {
            by_id.insert(id.to_string(), index);
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        let parent = entry
            .parent_id()
            .and_then(|parent_id| by_id.get(parent_id).copied());
        children_of.entry(parent).or_default().push(index);
    }

    fn materialize(
        entries: &[SessionEntry],
        children_of: &HashMap<Option<usize>, Vec<usize>>,
        index: usize,
        _parent: Option<usize>,
    ) -> SessionTreeNode {
        let children = children_of
            .get(&Some(index))
            .map(|children| {
                children
                    .iter()
                    .map(|&child| materialize(entries, children_of, child, Some(index)))
                    .collect()
            })
            .unwrap_or_default();
        SessionTreeNode {
            entry: entries[index].clone(),
            children,
            label: None,
            label_timestamp: None,
        }
    }

    children_of
        .get(&None)
        .map(|roots| {
            roots
                .iter()
                .map(|&root| materialize(&entries, &children_of, root, None))
                .collect()
        })
        .unwrap_or_default()
}

/// Attach a label to the node with the given id (searches recursively).
#[allow(dead_code)] // helper for label-edit scenarios driven by later batches
fn attach_label_by_id(tree: &mut [SessionTreeNode], id: &str, label: &str, timestamp: &str) {
    for node in tree {
        if node.entry.id() == Some(id) {
            node.label = Some(label.to_string());
            node.label_timestamp = Some(timestamp.to_string());
            return;
        }
        attach_label_by_id(&mut node.children, id, label, timestamp);
    }
}

fn attach_label(node: &mut SessionTreeNode, label: &str, timestamp: &str) {
    node.label = Some(label.to_string());
    node.label_timestamp = Some(timestamp.to_string());
}

// -- oracle loading ----------------------------------------------------------

fn oracle() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/interactive_r20_components_oracle/component_r20_oracle.json");
    let raw = std::fs::read_to_string(&path).expect("oracle json");
    serde_json::from_str(&raw).expect("oracle json parse")
}

fn scenario<'a>(value: &'a Value, name: &str) -> &'a Value {
    &value["scenarios"]
        .as_array()
        .expect("scenarios")
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("scenario {name} missing"))["result"]
}

fn oracle_lines(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("lines array")
        .iter()
        .map(|line| line.as_str().expect("line string").to_string())
        .collect()
}

/// The component constructor with the oracle clock installed.
fn selector_with_clock(
    tree: &[SessionTreeNode],
    current_leaf_id: Option<&str>,
    initial_selected_id: Option<&str>,
    initial_filter_mode: Option<FilterMode>,
) -> TreeSelectorComponent {
    let mut selector = TreeSelectorComponent::new(
        tree,
        current_leaf_id,
        24,
        Box::new(|_| {}),
        Box::new(|| {}),
        None,
        initial_selected_id,
        initial_filter_mode,
        theme(),
    );
    selector.tree_list_mut().set_clock(fixed_now);
    selector
}

fn plain(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| crate::tui::utils::strip_terminal_sequences(line))
        .collect()
}

// -- upstream test suite (test/tree-selector.test.ts) ------------------------

#[test]
fn initial_selection_with_metadata_entries() {
    let oracle = oracle();

    // focuses nearest visible ancestor when currentLeafId is a model_change
    // with a sibling branch
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        model_change("model-1", Some("user-2")),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);
    let selector = selector_with_clock(&tree, Some("model-1"), None, None);
    assert_eq!(
        selector.tree_list().selected_entry_id(),
        Some("user-2"),
        "model_change leaf"
    );
    let expected = scenario(&oracle, "tree_initial_selection")["modelChangeLeaf"]
        .as_str()
        .expect("oracle id");
    assert_eq!(selector.tree_list().selected_entry_id(), Some(expected));

    // thinking_level_change leaf behaves the same
    let tree2 = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        thinking_level_change("thinking-1", Some("user-2")),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);
    let selector = selector_with_clock(&tree2, Some("thinking-1"), None, None);
    assert_eq!(selector.tree_list().selected_entry_id(), Some("user-2"));
    assert_eq!(
        scenario(&oracle, "tree_initial_selection")["thinkingLeaf"].as_str(),
        Some("user-2")
    );
}

#[test]
fn filter_switching_with_parent_traversal() {
    let oracle = oracle();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        assistant_message("asst-2", Some("user-2"), "response"),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);
    let mut selector = selector_with_clock(&tree, Some("asst-2"), None, None);
    assert_eq!(selector.tree_list().selected_entry_id(), Some("asst-2"));
    // Ctrl+U: user-only filter
    selector.handle_input("\u{15}");
    assert_eq!(selector.tree_list().selected_entry_id(), Some("user-2"));
    // Ctrl+D: back to default
    selector.handle_input("\u{4}");
    assert_eq!(selector.tree_list().selected_entry_id(), Some("user-2"));

    let expected = scenario(&oracle, "tree_filter_parent_traversal")["steps"]
        .as_array()
        .expect("steps");
    let actual: Vec<Option<String>> = expected
        .iter()
        .map(|step| step["selected"].as_str().map(str::to_string))
        .collect();
    assert_eq!(actual.len(), 3);
    assert_eq!(actual[1].as_deref(), Some("user-2"));
    assert_eq!(actual[2].as_deref(), Some("user-2"));
}

#[test]
fn help_renders_semantic_rows() {
    let oracle = oracle();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let mut selector = selector_with_clock(&tree, Some("asst-1"), None, None);
    let raw = selector.render(30);
    let expected = oracle_lines(&scenario(&oracle, "tree_help")["raw"]);
    assert_eq!(raw, expected, "help render bytes");
    let plain_lines = plain(&raw);
    let joined = plain_lines.join("\n");
    assert!(joined.contains("branch"));
    assert!(joined.contains("copy"));
    assert!(joined.contains("filters"));
    assert!(joined.contains("cycle"));
    assert!(joined.contains("label time"));
    assert!(!joined.contains("..."));
    assert!(raw.iter().all(|line| visible_width(line) <= 30));
}

#[test]
fn copies_full_selected_message_with_ctrl_x() {
    let oracle = oracle();
    let message = format!("{}\nsecond line", "long message ".repeat(30));
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), &message),
    ]);
    let mut selector = selector_with_clock(&tree, Some("asst-1"), None, None);
    let copied: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    selector.on_copy = {
        let copied = Arc::clone(&copied);
        Some(Box::new(move |text: Option<&str>| {
            copied
                .lock()
                .expect("copied")
                .push(text.map(str::to_string));
        }))
    };
    selector.handle_input("\u{18}");
    let copied = copied.lock().expect("copied").clone();
    assert_eq!(copied, vec![Some(message.clone())]);
    assert_eq!(
        scenario(&oracle, "tree_copy")["copied"].as_str(),
        Some(message.as_str())
    );
}

#[test]
fn toggles_label_timestamps_for_labeled_nodes() {
    let oracle = oracle();
    let mut tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    attach_label(&mut tree[0], "checkpoint", "2026-03-28T14:32:00.000Z");
    let mut selector = selector_with_clock(&tree, Some("asst-1"), None, None);
    {
        let list = selector.tree_list_mut();
        let before = list.render_list(200).join("\n");
        assert!(before.contains("[checkpoint]"));
        assert!(!before.contains("3/28 14:32"));
        assert!(!before.contains("[+label time]"));
    }
    selector.handle_input("T");
    let after = selector.tree_list_mut().render_list(200);
    assert!(after.join("\n").contains("3/28 14:32"));
    assert!(after.join("\n").contains("[+label time]"));
    assert_eq!(
        after,
        oracle_lines(&scenario(&oracle, "tree_label_timestamps")["after"]),
        "after bytes"
    );
}

#[test]
fn preserves_selection_through_empty_filters() {
    // first: labeled-only filter with no labels, then back
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "bye"),
        assistant_message("asst-2", Some("user-2"), "goodbye"),
    ]);
    let mut selector = selector_with_clock(&tree, Some("asst-2"), None, None);
    assert_eq!(selector.tree_list().selected_entry_id(), Some("asst-2"));
    selector.handle_input("\u{c}"); // Ctrl+L labeled-only
    assert!(selector.tree_list().selected_entry_id().is_none());
    selector.handle_input("\u{4}"); // Ctrl+D default
    assert_eq!(selector.tree_list().selected_entry_id(), Some("asst-2"));

    // second: repeated empty switches
    let tree2 = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let mut selector = selector_with_clock(&tree2, Some("asst-1"), None, None);
    assert_eq!(selector.tree_list().selected_entry_id(), Some("asst-1"));
    selector.handle_input("\u{c}");
    assert!(selector.tree_list().selected_entry_id().is_none());
    selector.handle_input("\u{c}");
    assert_eq!(selector.tree_list().selected_entry_id(), Some("asst-1"));
    selector.handle_input("\u{c}");
    assert!(selector.tree_list().selected_entry_id().is_none());
    selector.handle_input("\u{4}");
    assert_eq!(selector.tree_list().selected_entry_id(), Some("asst-1"));

    let oracle = oracle();
    let expected = scenario(&oracle, "tree_empty_filter_preservation");
    for (group, selections) in [
        ("first", vec![Some("asst-2"), None, Some("asst-2")]),
        (
            "second",
            vec![Some("asst-1"), None, Some("asst-1"), None, Some("asst-1")],
        ),
    ] {
        let steps = expected[group].as_array().expect("steps");
        let actual: Vec<Option<String>> = steps
            .iter()
            .map(|step| step["selected"].as_str().map(str::to_string))
            .collect();
        let expected: Vec<Option<String>> = selections
            .into_iter()
            .map(|s| s.map(str::to_string))
            .collect();
        assert_eq!(actual, expected, "{group}");
    }
}

// -- branch navigation (oracle: tree_branch_navigation) ----------------------

fn branch_tree() -> Vec<SessionTreeNode> {
    build_tree(vec![
        user_message("user-1", None, "first message"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", Some("asst-1"), "second message"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
        user_message("user-3a", Some("asst-2"), "branch A start"),
        assistant_message("asst-3a", Some("user-3a"), "branch A response"),
        user_message("user-4a", Some("asst-3a"), "branch A deep"),
        assistant_message("asst-4a", Some("user-4a"), "branch A leaf"),
        user_message("user-3b", Some("asst-2"), "branch B start"),
        assistant_message("asst-3b", Some("user-3b"), "branch B response"),
        user_message("user-4b", Some("asst-3b"), "branch B deep"),
    ])
}

fn drive_keys(
    tree: &[SessionTreeNode],
    leaf_id: &str,
    keys: &[&str],
) -> Vec<(String, Option<String>)> {
    let mut selector = selector_with_clock(tree, Some(leaf_id), None, None);
    let mut steps = vec![(
        "init".to_string(),
        selector.tree_list().selected_entry_id().map(str::to_string),
    )];
    for key in keys {
        selector.handle_input(key);
        steps.push((
            (*key).to_string(),
            selector.tree_list().selected_entry_id().map(str::to_string),
        ));
    }
    steps
}

fn assert_steps_match_oracle(actual: &[(String, Option<String>)], expected: &Value) {
    let expected = expected.as_array().expect("steps");
    assert_eq!(actual.len(), expected.len(), "step count");
    for (actual, expected) in actual.iter().zip(expected) {
        let expected_key = expected["key"].as_str().expect("key");
        // "down"/"up" mark the fixture's key loop; escape-sequence steps are
        // compared exactly.
        if expected_key != "init" && expected_key != "down" && expected_key != "up" {
            assert_eq!(&actual.0, expected_key, "key order");
        }
        let expected_selected = expected["selected"].as_str();
        assert_eq!(
            actual.1.as_deref(),
            expected_selected,
            "selection after {expected_key}"
        );
    }
}

#[test]
fn branch_navigation_matches_oracle() {
    let oracle = oracle();
    let expected = scenario(&oracle, "tree_branch_navigation");

    // ctrl+left/right fold-or-up / unfold-or-down
    let steps = drive_keys(
        &branch_tree(),
        "asst-4a",
        &[
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
            "\u{1b}[B",
            "\u{1b}[A",
            "\u{1b}[1;5C",
            "\u{1b}[B",
            "\u{1b}[1;5D",
            "\u{1b}[1;5C",
        ],
    );
    assert_steps_match_oracle(&steps, &expected["ctrl"]);

    // alt+left/right are aliases
    let steps = drive_keys(
        &branch_tree(),
        "asst-4a",
        &["\u{1b}[1;3D", "\u{1b}[1;3D", "\u{1b}[1;3C", "\u{1b}[1;3C"],
    );
    assert_steps_match_oracle(&steps, &expected["alt"]);

    // folding the root hides the whole subtree; nested fold preserved
    let steps = drive_keys(
        &branch_tree(),
        "asst-4a",
        &[
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
            "\u{1b}[B",
            "\u{1b}[1;5C",
            "\u{1b}[1;5C",
            "\u{1b}[B",
        ],
    );
    assert_steps_match_oracle(&steps, &expected["foldRoot"]);

    // fold and navigate on the non-active branch
    {
        let mut selector = selector_with_clock(&branch_tree(), Some("asst-4a"), None, None);
        let mut found = false;
        let mut steps: Vec<(String, Option<String>)> = Vec::new();
        for _ in 0..20 {
            selector.handle_input("\u{1b}[B");
            let id = selector.tree_list().selected_entry_id().map(str::to_string);
            steps.push(("\u{1b}[B".to_string(), id.clone()));
            if id.as_deref() == Some("user-3b") {
                found = true;
                break;
            }
        }
        for key in ["\u{1b}[1;5C", "\u{1b}[1;5D", "\u{1b}[1;5D", "\u{1b}[1;5D"] {
            selector.handle_input(key);
            steps.push((
                key.to_string(),
                selector.tree_list().selected_entry_id().map(str::to_string),
            ));
        }
        assert!(found, "reached user-3b");
        assert_steps_match_oracle(&steps, &expected["nonActiveBranch"]["steps"]);
    }

    // multiple roots
    let multiple = build_tree(vec![
        user_message("user-1", None, "first root"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", None, "second root"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
    ]);
    let steps = drive_keys(
        &multiple,
        "asst-1",
        &[
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
            "\u{1b}[B",
            "\u{1b}[1;5C",
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
            "\u{1b}[1;5D",
        ],
    );
    assert_steps_match_oracle(&steps, &expected["multipleRoots"]);

    // folding roots when intermediate nodes are filtered out
    let filtered = build_tree(vec![
        user_message("user-1", None, "hello"),
        tool_call_only_assistant("tool-asst-1", Some("user-1")),
        user_message("user-2", Some("tool-asst-1"), "follow up"),
        assistant_message("asst-2", Some("user-2"), "response"),
    ]);
    let steps = drive_keys(
        &filtered,
        "asst-2",
        &["\u{1b}[1;5D", "\u{1b}[1;5D", "\u{1b}[B"],
    );
    assert_steps_match_oracle(&steps, &expected["filteredIntermediate"]);

    // search resets fold state
    {
        let mut selector = selector_with_clock(&branch_tree(), Some("asst-4a"), None, None);
        selector.handle_input("\u{1b}[1;5D");
        selector.handle_input("\u{1b}[1;5D");
        selector.handle_input("\u{1b}[B");
        let after_fold = selector.tree_list().selected_entry_id().map(str::to_string);
        selector.handle_input("b");
        selector.handle_input("\u{1b}");
        let mut steps = vec![("afterFold".to_string(), after_fold)];
        for _ in 0..20 {
            selector.handle_input("\u{1b}[B");
            let current_id = selector
                .tree_list()
                .selected_entry_id()
                .unwrap_or("")
                .to_string();
            steps.push(("\u{1b}[B".to_string(), Some(current_id.clone())));
            if current_id == "user-3a" {
                break;
            }
        }
        selector.handle_input("\u{1b}[B");
        steps.push((
            "\u{1b}[B".to_string(),
            selector.tree_list().selected_entry_id().map(str::to_string),
        ));
        assert_steps_match_oracle(&steps, &expected["searchResetsFold"]);
    }

    // filter mode change resets fold state
    {
        let mut selector = selector_with_clock(&branch_tree(), Some("asst-4a"), None, None);
        selector.handle_input("\u{1b}[1;5D");
        selector.handle_input("\u{1b}[1;5D");
        selector.handle_input("\u{15}");
        selector.handle_input("\u{4}");
        let mut steps: Vec<(String, Option<String>)> = Vec::new();
        for _ in 0..20 {
            selector.handle_input("\u{1b}[B");
            let current_id = selector
                .tree_list()
                .selected_entry_id()
                .unwrap_or("")
                .to_string();
            steps.push(("\u{1b}[B".to_string(), Some(current_id.clone())));
            if current_id == "user-3a" {
                break;
            }
        }
        selector.handle_input("\u{1b}[B");
        steps.push((
            "\u{1b}[B".to_string(),
            selector.tree_list().selected_entry_id().map(str::to_string),
        ));
        assert_steps_match_oracle(&steps, &expected["filterResetsFold"]);
    }
}

// -- render matrix (oracle: tree_render_matrix) ------------------------------

/// The full entry-type tree the render-matrix scenario drives.
fn matrix_tree() -> Vec<SessionTreeNode> {
    let text_block = |text: &str| {
        AssistantBlock::Text(crate::ai::types::TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    };
    let entries: Vec<SessionEntry> = vec![
        user_message("u1", None, "plan the rollout"),
        assistant_message("a1", Some("u1"), "Starting the rollout"),
        tool_call_only_assistant("t1", Some("a1")),
        user_message("u2", Some("t1"), "follow up question"),
        assistant_message("a2", Some("u2"), "done \n with newlines"),
        SessionEntry::Compaction(CompactionEntry {
            id: "c1".to_string(),
            parent_id: Some("a2".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            summary: "compacted summary".to_string(),
            first_kept_entry_id: None,
            tokens_before: 123_456,
            details: None,
            usage: None,
            from_hook: None,
            system_message: None,
            first_kept_entry_index: None,
        }),
        SessionEntry::BranchSummary(BranchSummaryEntry {
            id: "b1".to_string(),
            parent_id: Some("c1".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            from_id: "a2".to_string(),
            summary: "branch\nsummary text".to_string(),
            details: None,
            usage: None,
            from_hook: None,
        }),
        SessionEntry::SessionInfo(SessionInfoEntry {
            id: "s1".to_string(),
            parent_id: Some("b1".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            name: Some("my session".to_string()),
        }),
        SessionEntry::SessionInfo(SessionInfoEntry {
            id: "s2".to_string(),
            parent_id: Some("s1".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            name: None,
        }),
        model_change("m1", Some("s2")),
        thinking_level_change("th1", Some("m1")),
        SessionEntry::Custom(CustomEntry {
            id: "cu1".to_string(),
            parent_id: Some("th1".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            custom_type: "my-extension".to_string(),
            data: None,
        }),
        SessionEntry::CustomMessage(CustomMessageEntry {
            id: "cm1".to_string(),
            parent_id: Some("cu1".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            custom_type: "announcement".to_string(),
            content: Some(
                crate::coding_agent::core::messages::CustomMessageContent::Text(
                    "custom text".to_string(),
                ),
            ),
            details: None,
            display: true,
        }),
        SessionEntry::CustomMessage(CustomMessageEntry {
            id: "cm2".to_string(),
            parent_id: Some("cm1".to_string()),
            timestamp: "2026-05-28T21:46:40.000Z".to_string(),
            custom_type: "notes".to_string(),
            content: Some(
                crate::coding_agent::core::messages::CustomMessageContent::Blocks(vec![
                    TextOrImageBlock::Text(crate::ai::types::TextContent {
                        text: "block content".to_string(),
                        text_signature: None,
                    }),
                ]),
            ),
            details: None,
            display: true,
        }),
        bash_execution_message("bash1", Some("cm2"), "echo hi\nthere"),
        tool_result_message("tr1", Some("bash1"), "tc-t1", "read"),
        tool_result_message("tr2", Some("tr1"), "missing-id", "grep"),
        assistant_raw("ab1", Some("tr2"), vec![], StopReason::Aborted, None),
        assistant_raw(
            "er1",
            Some("ab1"),
            vec![],
            StopReason::Error,
            Some("rate limited\nbad"),
        ),
        assistant_raw("ea1", Some("er1"), vec![], StopReason::Stop, None),
    ];
    // The oracle scenario's milestone "label" rides on the entry object, not
    // on the tree node, so the node-level label rendering never sees it.
    let tree = build_tree(entries);
    let _ = text_block;
    tree
}

#[test]
fn render_matrix_matches_oracle() {
    let oracle = oracle();
    let tree = matrix_tree();
    let expected = scenario(&oracle, "tree_render_matrix");
    for width in [80usize, 200] {
        for (mode_name, mode) in [
            ("default", None),
            ("user-only", Some(FilterMode::UserOnly)),
            ("no-tools", Some(FilterMode::NoTools)),
            ("labeled-only", Some(FilterMode::LabeledOnly)),
            ("all", Some(FilterMode::All)),
        ] {
            let mut selector = selector_with_clock(&tree, Some("tr1"), None, mode);
            let key = format!("render_{width}_{mode_name}");
            assert_eq!(
                selector.render(width),
                oracle_lines(&expected[&key]),
                "{key}"
            );
        }
    }

    // search flow
    {
        let mut selector = selector_with_clock(&tree, Some("tr1"), None, None);
        for ch in ["r", "o", "l", "l"] {
            selector.handle_input(ch);
        }
        assert_eq!(
            selector.render(80),
            oracle_lines(&expected["searchRoll"]),
            "searchRoll"
        );
        assert_eq!(selector.tree_list().search_query(), "roll");
        selector.handle_input("\u{7f}");
        assert_eq!(
            selector.render(80),
            oracle_lines(&expected["searchRol"]),
            "searchRol"
        );
        selector.handle_input("\u{1b}");
        assert_eq!(
            selector.render(80),
            oracle_lines(&expected["searchCleared"]),
            "searchCleared"
        );
    }

    // horizontal viewport panning
    {
        let long_tree = build_tree(vec![
            user_message(
                "u1",
                None,
                &format!("{}end", "very long user message content ".repeat(6)),
            ),
            assistant_message("a1", Some("u1"), "ok"),
        ]);
        let mut selector = selector_with_clock(&long_tree, Some("a1"), None, None);
        selector.handle_input("\u{1b}[A");
        assert_eq!(
            selector.render(40),
            oracle_lines(&expected["hscroll40"]),
            "hscroll40"
        );
        assert_eq!(
            selector.render(20),
            oracle_lines(&expected["hscroll20"]),
            "hscroll20"
        );
    }

    // empty filtered list rendering
    {
        let empty_tree = build_tree(vec![user_message("u1", None, "hello")]);
        let mut selector =
            selector_with_clock(&empty_tree, Some("u1"), None, Some(FilterMode::LabeledOnly));
        assert_eq!(
            selector.render(80),
            oracle_lines(&expected["emptyLabeled"]),
            "emptyLabeled"
        );
    }
}

// -- nav keys (oracle: tree_nav_keys) ----------------------------------------

fn nav_tree() -> Vec<SessionTreeNode> {
    build_tree(vec![
        user_message("u1", None, "one"),
        assistant_message("a1", Some("u1"), "two"),
        user_message("u2", Some("a1"), "three"),
        assistant_message("a2", Some("u2"), "four"),
        user_message("u3", Some("a2"), "five"),
    ])
}

#[test]
fn nav_keys_match_oracle() {
    let oracle = oracle();
    let expected = scenario(&oracle, "tree_nav_keys");
    let tree = nav_tree();

    // wrap-around up/down
    {
        let mut selector = selector_with_clock(&tree, Some("u3"), None, None);
        let mut steps: Vec<(String, Option<String>)> = Vec::new();
        for key in [
            "\u{1b}[A", "\u{1b}[A", "\u{1b}[A", "\u{1b}[A", "\u{1b}[B", "\u{1b}[B", "\u{1b}[B",
            "\u{1b}[B",
        ] {
            selector.handle_input(key);
            steps.push((
                key.to_string(),
                selector.tree_list().selected_entry_id().map(str::to_string),
            ));
        }
        assert_steps_match_oracle(&steps, &expected["wrap"]);
    }

    // page up/down
    {
        let mut selector = selector_with_clock(&tree, Some("u3"), None, None);
        let mut steps: Vec<(String, Option<String>)> = Vec::new();
        for key in [
            "\u{1b}[1;5D",
            "\u{1b}[1;5C",
            "\u{1b}[5~",
            "\u{1b}[6~",
            "\u{1b}[5~",
            "\u{1b}[6~",
        ] {
            selector.handle_input(key);
            steps.push((
                key.to_string(),
                selector.tree_list().selected_entry_id().map(str::to_string),
            ));
        }
        assert_steps_match_oracle(&steps, &expected["pages"]);
    }

    // confirm + cancel
    {
        let selected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(Mutex::new(0usize));
        let mut selector = TreeSelectorComponent::new(
            &tree,
            Some("u3"),
            24,
            {
                let selected = Arc::clone(&selected);
                Box::new(move |entry_id: &str| {
                    selected
                        .lock()
                        .expect("selected")
                        .push(entry_id.to_string());
                })
            },
            {
                let cancelled = Arc::clone(&cancelled);
                Box::new(move || {
                    *cancelled.lock().expect("cancelled") += 1;
                })
            },
            None,
            None,
            None,
            theme(),
        );
        selector.tree_list_mut().set_clock(fixed_now);
        selector.handle_input("\r");
        selector.handle_input("\u{1b}[A");
        selector.handle_input("\r");
        selector.handle_input("\u{1b}");
        assert_eq!(selected.lock().expect("selected").clone(), vec!["u3", "a2"]);
        assert_eq!(*cancelled.lock().expect("cancelled"), 1);
        assert_eq!(
            scenario(&oracle, "tree_nav_keys")["confirm"]["selected"],
            serde_json::json!(["u3", "a2"])
        );
    }

    // cancel with an active search clears the query instead
    {
        let cancelled = Arc::new(Mutex::new(0usize));
        let mut selector = TreeSelectorComponent::new(
            &tree,
            Some("u3"),
            24,
            Box::new(|_| {}),
            {
                let cancelled = Arc::clone(&cancelled);
                Box::new(move || {
                    *cancelled.lock().expect("cancelled") += 1;
                })
            },
            None,
            None,
            None,
            theme(),
        );
        selector.tree_list_mut().set_clock(fixed_now);
        selector.handle_input("one");
        selector.handle_input("\u{1b}");
        let after_clear = selector.tree_list().selected_entry_id().map(str::to_string);
        selector.handle_input("\u{1b}");
        assert_eq!(after_clear.as_deref(), Some("u1"));
        assert_eq!(*cancelled.lock().expect("cancelled"), 1);
        let _ = &expected;
    }

    // filter key cycle (status suffixes)
    {
        let mut selector = selector_with_clock(&tree, Some("u3"), None, None);
        let mut status_lines: Vec<String> = Vec::new();
        for key in RAW_FILTER_KEYS {
            selector.handle_input(key);
            let rendered = selector.render(200);
            let plain_lines: Vec<String> = plain(&rendered);
            let status = plain_lines
                .iter()
                .rev()
                .find(|line| line.trim_start().starts_with('('))
                .cloned()
                .unwrap_or_default();
            status_lines.push(status);
        }
        let expected_lines: Vec<String> = expected["filterStatusLines"]
            .as_array()
            .expect("filterStatusLines")
            .iter()
            .map(|l| l.as_str().expect("status line").to_string())
            .collect();
        assert_eq!(status_lines, expected_lines);
    }
}

const RAW_FILTER_KEYS: [&str; 11] = [
    "\u{14}",        // app.tree.filter.noTools (toggle on)
    "\u{14}",        // toggle off
    "\u{15}",        // user-only (toggle on)
    "\u{c}",         // labeled-only (toggle on)
    "\u{1}",         // all (toggle on)
    "\u{1}",         // toggle off
    "\u{f}",         // cycleForward
    "\u{f}",         // cycleForward
    "\u{1b}[111;6u", // cycleBackward
    "\u{1b}[111;6u", // cycleBackward
    "\u{4}",         // default
];

// -- label edit (oracle: tree_label_edit) ------------------------------------

#[test]
fn label_edit_flow_matches_oracle() {
    let oracle = oracle();
    let expected = scenario(&oracle, "tree_label_edit");
    let tree = build_tree(vec![
        user_message("u1", None, "one"),
        assistant_message("a1", Some("u1"), "two"),
    ]);
    let changes: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut selector = TreeSelectorComponent::new(
        &tree,
        Some("a1"),
        24,
        Box::new(|_| {}),
        Box::new(|| {}),
        {
            let changes = Arc::clone(&changes);
            Some(Box::new(move |entry_id: &str, label: Option<&str>| {
                changes
                    .lock()
                    .expect("changes")
                    .push((entry_id.to_string(), label.map(str::to_string)));
            }))
        },
        None,
        None,
        theme(),
    );
    selector.tree_list_mut().set_clock(fixed_now);

    selector.handle_input("\u{1b}[A"); // select u1
    selector.handle_input("L"); // app.tree.editLabel
    assert_eq!(
        selector.render(80),
        oracle_lines(&expected["editOpen"]),
        "editOpen"
    );
    selector.handle_input("a");
    selector.handle_input("b");
    selector.handle_input("\r");
    {
        let changes = changes.lock().expect("changes").clone();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "u1");
        assert_eq!(changes[0].1.as_deref(), Some("ab"));
    }
    assert_eq!(
        selector.render(80),
        oracle_lines(&expected["afterSave"]),
        "afterSave"
    );
    let rendered = selector.tree_list_mut().render_list(200);
    assert!(rendered.join("\n").contains("[ab]"));

    // clear attempt: the fresh label input keeps the cursor at column 0, so
    // the backspace is a no-op and the submit re-saves "ab" (oracle
    // `changesAfterClear` records the same pair).
    selector.handle_input("L");
    selector.handle_input("\u{7f}");
    selector.handle_input("\r");
    {
        let changes = changes.lock().expect("changes").clone();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[1].0, "u1");
        assert_eq!(changes[1].1.as_deref(), Some("ab"));
    }

    // cancel path
    selector.handle_input("L");
    selector.handle_input("z");
    selector.handle_input("\u{1b}");
    assert_eq!(
        selector.render(80),
        oracle_lines(&expected["afterCancel"]),
        "afterCancel"
    );
    let _ = &expected["listLabel"];
}

// -- initial selection/filter (oracle: tree_select_initial) ------------------

#[test]
fn initial_selected_id_and_filter_mode() {
    let oracle = oracle();
    let tree = build_tree(vec![
        user_message("u1", None, "one"),
        assistant_message("a1", Some("u1"), "two"),
        user_message("u2", Some("a1"), "three"),
    ]);
    let mut selector =
        selector_with_clock(&tree, Some("a1"), Some("u2"), Some(FilterMode::UserOnly));
    assert_eq!(selector.tree_list().selected_entry_id(), Some("u2"));
    assert_eq!(
        selector.render(120),
        oracle_lines(&scenario(&oracle, "tree_select_initial")["render"]),
        "tree_select_initial render"
    );
}

// -- empty tree auto-cancel (oracle: tree_empty_auto_cancel) -----------------

#[test]
fn empty_tree_sets_auto_cancel() {
    let tree: Vec<SessionTreeNode> = Vec::new();
    let mut selector = selector_with_clock(&tree, None, None, None);
    assert!(selector.auto_cancel_pending());
    assert!(selector.take_auto_cancel());
    assert!(!selector.auto_cancel_pending());
    let oracle = oracle();
    let result = scenario(&oracle, "tree_empty_auto_cancel");
    assert_eq!(result["cancelled"], 1);
}

// -- formatLabelTimestamp unit coverage (S20.5 clock seam) -------------------

#[test]
fn label_timestamp_formats() {
    let mut tree = build_tree(vec![user_message("u1", None, "hello")]);
    attach_label(&mut tree[0], "x", "2026-03-28T14:32:00.000Z");
    let mut selector = selector_with_clock(&tree, Some("u1"), None, None);
    let list = selector.tree_list_mut();
    // same-year, different day
    assert_eq!(
        list.format_label_timestamp_at("2026-03-28T14:32:00.000Z", FIXED_NOW),
        "3/28 14:32"
    );
    // same day renders time only
    assert_eq!(
        list.format_label_timestamp_at("2026-05-28T21:46:00.000Z", FIXED_NOW),
        "21:46"
    );
    // previous year renders the short year prefix
    assert_eq!(
        list.format_label_timestamp_at("2025-12-31T23:05:00.000Z", FIXED_NOW),
        "25/12/31 23:05"
    );
}

#[test]
fn debug_multiple_roots_maps() {
    let tree = build_tree(vec![
        user_message("user-1", None, "first root"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", None, "second root"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
    ]);
    let mut selector = selector_with_clock(&tree, Some("asst-1"), None, None);
    {
        let list = selector.tree_list_mut();
        eprintln!("ids={:?}", list.filtered_ids());
        eprintln!("selected={:?}", list.selected_entry_id());
        eprintln!("parents={:?}", list.visible_parent_map);
        eprintln!("children={:?}", list.visible_children_map);
        eprintln!("foldable a1={}", list.is_foldable("asst-1"));
    }
    selector.handle_input("\u{1b}[1;5D");
    eprintln!("after L: {:?}", selector.tree_list().selected_entry_id());
}
