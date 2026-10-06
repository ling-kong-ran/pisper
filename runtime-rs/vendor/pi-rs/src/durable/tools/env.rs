//! Port of `src/tools/env.ts`: the call's execution environment. A tool
//! without one fails with an ordinary error result (the port's `Err`, per
//! divergence D4).

use std::sync::Arc;

use crate::durable::env::ExecutionEnv;
use crate::durable::errors::PlainError;
use crate::durable::harness::types::ToolExecutionApiLike;

/// `requireEnv(api)` (`tools/env.ts`).
pub fn require_env(api: &dyn ToolExecutionApiLike) -> Result<Arc<dyn ExecutionEnv>, PlainError> {
    api.env()
        .ok_or_else(|| PlainError::new("No execution environment is configured"))
}
