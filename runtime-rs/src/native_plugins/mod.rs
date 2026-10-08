//! Native local-plugin lifecycle and execution. Node compatibility is tracked explicitly;
//! the host must not advertise complete plugin support until every required ABI is covered.
mod config;
mod executor;
mod manifest;
mod node_host;
mod paths;
mod service;
mod store;
#[cfg(test)]
mod tests;
mod tools;

pub use config::{Catalog, ConfigMutation, ConfigPort};
pub use executor::{worker_main, ExecutionContext, WorkerRequest};
pub use manifest::{Manifest, ToolManifest};
pub use service::ToolPluginService;
pub use tools::{create_extension, ExecutionScope, ExecutionScopePort, OnPluginsChanged};

use std::fmt;

pub type Result<T> = std::result::Result<T, PluginError>;
#[derive(Debug, Clone)]
pub struct PluginError {
    pub message: String,
}
impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for PluginError {}
impl From<std::io::Error> for PluginError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}
impl From<serde_json::Error> for PluginError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub const MANIFEST_FILE: &str = "pisper-plugin.json";
pub const MAX_PLUGIN_FILES: usize = 512;
pub const MAX_PLUGIN_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const INSPECTION_TTL_MS: i64 = 10 * 60 * 1000;
pub const EXECUTION_TIMEOUT_MS: u64 = 2 * 60 * 1000;

/// This is evidence for the host capability projection, not a restriction on manifests.
pub fn compatibility() -> serde_json::Value {
    serde_json::json!({
        "complete":false,"engine":"QuickJS 0.14.0 native Rust subprocess",
        "implemented":["ESM named/default execute","relative ESM imports","CommonJS execute exports",
            "empty process.env","native filesystem operations","native path operations",
            "Buffer UTF-8/base64/hex","timers","120-second process termination","cancellation"],
        "remaining":["complete Node built-in APIs and edge semantics","npm package exports/imports resolution",
            "Node streams and event emitters","HTTP/TLS/DNS/socket APIs","child_process APIs",
            "worker_threads APIs","Node native addon ABI","Intl collation data-version parity"]
    })
}
