//! Port of `src/harness/prompt.ts`: system-prompt section replay, rendering,
//! and the `pi.system` entry planning that keeps the replayed sections and
//! tools equal to the desired loadout.
//!
//! Divergences (structural, disclosed): the upstream render/section maps are
//! insertion-ordered JS objects; the port uses `serde_json`'s
//! `preserve_order` maps and `Vec` pairs so replay order matches. Section
//! renders are `async` upstream because a section may await; the port's
//! render closures return boxed futures ([`SectionRenderFuture`]).

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::ai::transcript::{declarations_equal, get_current_tools, to_tool_declaration};
use crate::ai::types::{Message, Sections, SystemMessage, Tool as AiTool, ToolReference};

use super::super::entries::{system_entry as system_entry_kind, SYSTEM_ENTRY_KIND};
use super::super::errors::PlainError;
use super::super::types::{ContextEdit, ContextEditAction, EntryDraft};
use super::types::{ContextView, PromptInput, PromptSection, ToolRegistration};

/// `PromptSection.render` result future.
pub type SectionRenderFuture =
    Pin<Box<dyn Future<Output = Result<Option<String>, PlainError>> + Send + 'static>>;

/// Hook handler invocation future (`Value | None`).
pub type HookFuture =
    Pin<Box<dyn Future<Output = Result<Option<Value>, PlainError>> + Send + 'static>>;

/// Memo surface future (`Option<Value>`).
pub type MemoFuture =
    Pin<Box<dyn Future<Output = Result<Option<Value>, PlainError>> + Send + 'static>>;

/// `ConversationSetup` / `ConversationInit` invocation future.
pub type SetupFuture = Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + 'static>>;

/// Sections in effect after replaying system messages in order: set in place,
/// `null` deletes, re-adding appends (`prompt.ts` `replaySections`).
pub fn replay_sections(messages: &[Message]) -> Vec<(String, String)> {
    let mut shown: Vec<(String, String)> = Vec::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        let Some(sections) = &system.sections else {
            continue;
        };
        for (key, value) in sections.as_slice() {
            let position = shown.iter().position(|(name, _)| name == key);
            match value {
                None => {
                    if let Some(position) = position {
                        shown.remove(position);
                    }
                }
                Some(value) => match position {
                    Some(position) => shown[position].1 = value.clone(),
                    None => shown.push((key.clone(), value.clone())),
                },
            }
        }
    }
    shown
}

/// Render sections in registry order (`prompt.ts` `renderSections`).
/// `None` omits a section; tagged text is wrapped as
/// `<key>\n...\n</key>`. A section that fails keeps its shown text, if any,
/// and is reported; failures after `context` is aborted propagate.
pub async fn render_sections(
    sections: &[PromptSection],
    input: &PromptInput,
    shown: &[(String, String)],
    report: &(dyn Fn(&PlainError) + Send + Sync),
    context: &Context,
) -> Result<Vec<(String, String)>, PlainError> {
    let mut desired: Vec<(String, String)> = Vec::new();
    for section in sections {
        let rendered = (section.render)(input, context).await;
        match rendered {
            Err(error) => {
                if context
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled())
                {
                    return Err(error);
                }
                report(&error);
                if let Some((_, kept)) = shown.iter().find(|(key, _)| *key == section.key) {
                    desired.push((section.key.clone(), kept.clone()));
                }
            }
            Ok(None) => {}
            Ok(Some(text)) => {
                let wrapped = if section.tag == Some(false) {
                    text
                } else {
                    format!("<{}>\n{}\n</{}>", section.key, text, section.key)
                };
                desired.push((section.key.clone(), wrapped));
            }
        }
    }
    Ok(desired)
}

/// Active names the snapshot resolves, first occurrence of each, in
/// configured order, as composed by wrappers (`prompt.ts` `desiredTools`).
pub fn desired_tools(
    active_tools: &[String],
    resolve: &dyn Fn(&str) -> Option<Arc<ToolRegistration>>,
) -> Vec<Arc<ToolRegistration>> {
    let mut tools: Vec<Arc<ToolRegistration>> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for name in active_tools {
        if seen.contains(name) {
            continue;
        }
        seen.insert(name.clone());
        if let Some(tool) = resolve(name) {
            tools.push(tool);
        }
    }
    tools
}

/// Tool changes from `offered` to `desired` (`prompt.ts` `planTools`): a
/// changed declaration is removed and re-added; replay keeps retained tools
/// in place and appends additions, unless that would not yield the desired
/// order, in which case every offered tool is removed and every desired tool
/// re-added in order.
fn plan_tools(offered: &[AiTool], desired: &[AiTool]) -> ToolChanges {
    let kept: Vec<&AiTool> = offered
        .iter()
        .filter(|tool| {
            desired
                .iter()
                .any(|next| next.name == tool.name && declarations_equal(tool, next))
        })
        .collect();
    let kept_names: BTreeSet<&str> = kept.iter().map(|tool| tool.name.as_str()).collect();
    let added: Vec<&AiTool> = desired
        .iter()
        .filter(|tool| !kept_names.contains(tool.name.as_str()))
        .collect();
    let replayed: Vec<&str> = kept
        .iter()
        .map(|tool| tool.name.as_str())
        .chain(added.iter().map(|tool| tool.name.as_str()))
        .collect();
    if replayed
        .iter()
        .zip(desired.iter())
        .any(|(name, tool)| *name != tool.name.as_str())
    {
        return ToolChanges {
            tools_removed: offered
                .iter()
                .map(|tool| ToolReference {
                    name: tool.name.clone(),
                })
                .collect(),
            tools_added: desired.iter().map(to_tool_declaration).collect(),
        };
    }
    ToolChanges {
        tools_removed: offered
            .iter()
            .filter(|tool| !kept_names.contains(tool.name.as_str()))
            .map(|tool| ToolReference {
                name: tool.name.clone(),
            })
            .collect(),
        tools_added: added.iter().map(|tool| to_tool_declaration(tool)).collect(),
    }
}

/// `prompt.ts` `ToolChanges`.
struct ToolChanges {
    tools_removed: Vec<ToolReference>,
    tools_added: Vec<AiTool>,
}

/// Section patches (`prompt.ts` `planSections`): none, the minimal patch, or
/// a remove-all/re-add-all pair when the order would differ. Pairs preserve
/// JS object insertion order as `Vec<(String, Option<String>)>`; `None` is
/// the JSON `null` removal.
fn plan_sections(
    shown: &[(String, String)],
    desired: &[(String, String)],
) -> Vec<Vec<(String, Option<String>)>> {
    let mut patched_order: Vec<String> = shown
        .iter()
        .map(|(key, _)| key.clone())
        .filter(|key| desired.iter().any(|(desired_key, _)| desired_key == key))
        .collect();
    patched_order.extend(
        desired
            .iter()
            .map(|(key, _)| key.clone())
            .filter(|key| !shown.iter().any(|(shown_key, _)| shown_key == key)),
    );
    let desired_order: Vec<String> = desired.iter().map(|(key, _)| key.clone()).collect();
    if patched_order != desired_order {
        let removals: Vec<(String, Option<String>)> =
            shown.iter().map(|(key, _)| (key.clone(), None)).collect();
        return vec![
            removals,
            desired
                .iter()
                .map(|(key, value)| (key.clone(), Some(value.clone())))
                .collect(),
        ];
    }
    let mut patch: Vec<(String, Option<String>)> = Vec::new();
    for (key, value) in shown {
        let next = desired
            .iter()
            .find(|(desired_key, _)| desired_key == key)
            .map(|(_, v)| v);
        if next != Some(value) {
            patch.push((key.clone(), next.cloned()));
        }
    }
    for (key, value) in desired {
        if !shown.iter().any(|(shown_key, _)| shown_key == key) {
            patch.push((key.clone(), Some(value.clone())));
        }
    }
    if patch.is_empty() {
        Vec::new()
    } else {
        vec![patch]
    }
}

/// Build one `pi.system` entry draft (`prompt.ts` `systemEntry`).
#[allow(clippy::type_complexity)]
fn system_entry(
    sections: Option<Vec<(String, Option<String>)>>,
    tools: Option<&ToolChanges>,
    timestamp: f64,
) -> EntryDraft {
    let message = SystemMessage {
        content: crate::ai::types::StringOrBlocks::Text(String::new()),
        sections: sections.map(Sections::new),
        tools_added: tools
            .filter(|changes| !changes.tools_added.is_empty())
            .map(|changes| changes.tools_added.clone()),
        tools_removed: tools
            .filter(|changes| !changes.tools_removed.is_empty())
            .map(|changes| changes.tools_removed.clone()),
        timestamp: timestamp as i64,
    };
    EntryDraft {
        kind: SYSTEM_ENTRY_KIND.to_string(),
        model: Some(vec![Message::System(message)]),
        data: None,
        edits: None,
        head: None,
    }
}

/// Plan the `pi.system` entries (`prompt.ts` `planSystemEntries`) that make
/// the replayed sections and tools of `view` equal `desired` and `tools` in
/// values and order.
pub fn plan_system_entries(
    view: &ContextView,
    desired: &[(String, String)],
    tools: &[AiTool],
    timestamp: f64,
) -> Vec<EntryDraft> {
    if let Some(head) = &view.head {
        let has_later_system = view
            .entries
            .iter()
            .any(|entry| entry.kind == SYSTEM_ENTRY_KIND && entry.id > head.id);
        if !has_later_system {
            let edits: Vec<ContextEdit> = view
                .entries
                .iter()
                .filter(|entry| entry.kind == SYSTEM_ENTRY_KIND)
                .map(|entry| ContextEdit {
                    target: entry.id,
                    action: ContextEditAction::Omit,
                    messages: None,
                })
                .collect();
            let baseline_sections: Vec<(String, Option<String>)> = desired
                .iter()
                .map(|(key, value)| (key.clone(), Some(value.clone())))
                .collect();
            let changes = ToolChanges {
                tools_removed: Vec::new(),
                tools_added: tools.iter().map(to_tool_declaration).collect(),
            };
            let mut baseline = system_entry(Some(baseline_sections), Some(&changes), timestamp);
            if !edits.is_empty() {
                baseline.edits = Some(edits);
            }
            return vec![baseline];
        }
    }
    let shown = replay_sections(&view.messages);
    let sections = plan_sections(&shown, desired);
    let offered = get_current_tools(&view.messages);
    let changes = plan_tools(&offered, tools);
    if changes.tools_removed.is_empty() && changes.tools_added.is_empty() {
        return sections
            .iter()
            .map(|patch| system_entry(Some(patch.clone()), None, timestamp))
            .collect();
    }
    if sections.is_empty() {
        return vec![system_entry(None, Some(&changes), timestamp)];
    }
    sections
        .iter()
        .enumerate()
        .map(|(index, patch)| {
            let ride = if index == sections.len() - 1 {
                Some(&changes)
            } else {
                None
            };
            system_entry(Some(patch.clone()), ride, timestamp)
        })
        .collect()
}

/// The `pi.system` entry guard (`SystemEntry.is`).
pub fn is_system_entry(kind: &str) -> bool {
    kind == system_entry_kind().kind
}
