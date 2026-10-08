//! The builtin coding tools, ported from the M1 mini-loop onto
//! [`agent_core::types::AgentTool`] (execute closures over typed schemars
//! arguments via [`make_tool`]). Behavior is unchanged from the M1 versions.

pub mod bash;
pub mod edit_file;
pub mod list_dir;
pub mod read_file;
pub mod write_file;

use crate::agent_core::types::{AgentTool, AgentToolResult};
use crate::ai::types::content::TextContent;
use crate::ai::types::message::TextOrImageBlock;

pub fn builtin_tools() -> Vec<AgentTool> {
    vec![
        read_file::tool(),
        write_file::tool(),
        edit_file::tool(),
        list_dir::tool(),
        bash::tool(),
    ]
}

/// A successful tool result carrying one text block.
pub(crate) fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })],
        ..AgentToolResult::default()
    }
}

#[cfg(test)]
pub(crate) async fn run_tool_text(
    tool: &AgentTool,
    args: serde_json::Value,
) -> anyhow::Result<String> {
    let result = (tool.execute)("test-call".into(), args, None, None).await?;
    match result.content.first() {
        Some(TextOrImageBlock::Text(text)) => Ok(text.text.clone()),
        other => anyhow::bail!("expected a text block, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The builtin loadout the CLI installs is exactly these five tools, in
    /// this order (the M1 builtin surface, unchanged by the agent-core swap).
    #[test]
    fn builtin_tools_declare_the_five_builtin_names() {
        let tools = builtin_tools();
        assert_eq!(
            tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["read_file", "write_file", "edit_file", "list_dir", "bash"]
        );
        // Every tool carries a description and a schemars object schema, and
        // the declaration is the full interface sent to the LLM.
        for tool in &tools {
            assert!(!tool.description.is_empty(), "{}", tool.name);
            assert_eq!(tool.label, tool.name);
            assert_eq!(tool.parameters["type"], serde_json::json!("object"));
            let declaration = tool.declaration();
            assert_eq!(declaration.name, tool.name);
            assert_eq!(declaration.parameters, tool.parameters);
        }
    }
}
