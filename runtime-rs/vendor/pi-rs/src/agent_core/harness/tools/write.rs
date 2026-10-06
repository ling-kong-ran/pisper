//! Upstream tools/write.ts.
use super::super::{AgentHarnessTool, FileContent};
use super::file_mutation_queue::with_file_mutation_queue;
use super::path_utils::resolve_tool_path;
use super::{check_abort, text_result, ExecutionToolContext, HasExecutionEnv};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

pub fn create_write_tool() -> AgentHarnessTool<ExecutionToolContext> {
    create_write_tool_for()
}
pub fn create_write_tool_for<T: HasExecutionEnv>() -> AgentHarnessTool<T> {
    AgentHarnessTool {
        name: "write".into(), label: "write".into(),
        description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}},"required":["path","content"]}),
        constrained_sampling: None, prepare_arguments: None, replay: None, execution_mode: None,
        execute: Arc::new(|_, input, _, tool_context: T, _, context| Box::pin(async move {
            let input: WriteToolInput = serde_json::from_value(input)?;
            let env = tool_context.execution_env();
            let absolute = resolve_tool_path(env.as_ref(), &input.path, context.clone()).await?;
            with_file_mutation_queue(&env, &absolute, || async {
                check_abort(&context)?;
                env.write_file(&absolute, FileContent::Text(input.content), context.clone()).await?;
                check_abort(&context)?;
                Ok(text_result(format!("Successfully wrote to {}", input.path)))
            }, context.clone()).await
        })),
    }
}
