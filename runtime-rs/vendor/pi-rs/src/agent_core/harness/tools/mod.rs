//! Harness-native execution tools, ported from upstream harness/tools/.
//! These are distinct from the earlier minimal agent_core::tools CLI tools.
//! Concrete factories use ExecutionToolContext; *_for factories accept custom
//! contexts implementing HasExecutionEnv (the Rust equivalent of TS extends).

mod bash;
mod edit;
pub mod edit_diff;
mod file_mutation_queue;
pub mod image;
mod line_diff;
pub mod path_utils;
mod read;
mod write;

use super::{Context, ExecutionEnv};
use crate::agent_core::types::AgentToolResult;
use crate::ai::types::{TextContent, TextOrImageBlock};
use std::sync::Arc;

pub use bash::*;
pub use edit::*;
pub use read::*;
pub use write::*;

#[derive(Clone)]
pub struct ExecutionToolContext {
    pub env: Arc<dyn ExecutionEnv>,
}

pub trait HasExecutionEnv: Send + Sync + 'static {
    fn execution_env(&self) -> Arc<dyn ExecutionEnv>;
}
impl HasExecutionEnv for ExecutionToolContext {
    fn execution_env(&self) -> Arc<dyn ExecutionEnv> {
        self.env.clone()
    }
}

fn check_abort(context: &Context) -> anyhow::Result<()> {
    if context
        .abort_signal()
        .is_some_and(|signal| signal.is_cancelled())
    {
        anyhow::bail!("Operation aborted");
    }
    Ok(())
}
fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
