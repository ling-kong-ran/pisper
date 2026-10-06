//! Agent-only image assets. Media, imported files and exported workspace artifacts
//! have a lifecycle independent of workflow and workbench entities.
mod export;
mod fs_boundary;
mod import;
mod schema;
mod service;
#[cfg(test)]
mod tests;
mod tools;

pub(crate) use service::{
    AgentEnabledPort, AgentMedia, AgentOperations, GeneratedFile, GeneratedFilePort,
    ImageAgentService, ToolContext,
};
pub(crate) use tools::create_extension;

pub(crate) use crate::native_workflow::{Result, WorkflowError};
use crate::workflow_engine::RunCancellation;
use axum::http::StatusCode;

pub(crate) const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
fn error(code: &str, status: StatusCode) -> WorkflowError {
    let mut error = WorkflowError::coded(code, code);
    error.status = status;
    error
}
fn invalid() -> WorkflowError {
    error("workflow_image_invalid", StatusCode::BAD_REQUEST)
}
fn active(cancellation: &RunCancellation, export: bool) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(error(
            if export {
                "image_tools_export_cancelled"
            } else {
                "workflow_image_cancelled"
            },
            StatusCode::CONFLICT,
        ))
    } else {
        Ok(())
    }
}
