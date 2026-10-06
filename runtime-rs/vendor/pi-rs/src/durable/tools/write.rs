//! Port of `src/tools/write.ts`: write content to a file. Creates the file if
//! it doesn't exist, overwrites if it does. Automatically creates parent
//! directories (through the environment's `writeFile`).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::{TextContent, TextOrImageBlock, Tool as AiTool};
use crate::durable::env::FileContent;
use crate::durable::errors::PlainError;
use crate::durable::harness::types::{ToolExecutionApiLike, ToolExecutionResult, ToolRegistration};
use crate::durable::tools::env::require_env;
use crate::durable::tools::file_mutation_queue::with_file_mutation_queue;
use crate::durable::tools::path_utils::resolve_tool_path;

/// `WriteToolInput` (`tools/write.ts`); validated upstream by the tool
/// declaration before `execute` runs (D32).
#[derive(Debug, Clone, Deserialize)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

/// `createWriteTool()` (`tools/write.ts`).
pub fn create_write_tool() -> ToolRegistration {
    ToolRegistration {
        tool: AiTool {
            name: String::from("write"),
            description: String::from(
                "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
            ),
            parameters: json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file to write (relative or absolute)"},
                    "content": {"type": "string", "description": "Content to write to the file"},
                },
            }),
            constrained_sampling: None,
        },
        replay: None,
        execution_mode: None,
        prepare_arguments: None,
        output_limits: None,
        execute: Arc::new(
            |args: serde_json::Map<String, serde_json::Value>,
             api: Arc<dyn ToolExecutionApiLike>,
             context: Context| {
                Box::pin(async move {
                    let input: WriteToolInput =
                        serde_json::from_value(serde_json::Value::Object(args))
                            .map_err(|error| PlainError::new(error.to_string()))?;
                    let env = require_env(api.as_ref())?;
                    let absolute_path = resolve_tool_path(env.as_ref(), &input.path, &context)?;
                    with_file_mutation_queue(&env, &absolute_path, || async {
                        if context
                            .abort_signal()
                            .is_some_and(|signal| signal.is_cancelled())
                        {
                            return Err(PlainError::new("Operation aborted"));
                        }
                        env.write_file(&absolute_path, FileContent::Text(&input.content), &context)
                            .map_err(|error| PlainError::new(error.message))?;
                        if context
                            .abort_signal()
                            .is_some_and(|signal| signal.is_cancelled())
                        {
                            return Err(PlainError::new("Operation aborted"));
                        }
                        Ok(ToolExecutionResult {
                            content: Some(vec![TextOrImageBlock::Text(TextContent {
                                text: format!("Successfully wrote to {}", input.path),
                                text_signature: None,
                            })]),
                            is_error: None,
                            details: None,
                            diagnostics: None,
                            usage: None,
                            control: None,
                        })
                    }, &context)
                    .await
                })
            },
        ),
    }
}
