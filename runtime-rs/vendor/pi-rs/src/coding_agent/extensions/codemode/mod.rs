//! Port of upstream `coding-agent/src/extensions/codemode` — the `codemode`
//! tool as an extension (upstream `index.ts`; the CLI loads it as a built-in
//! extension, SDK users add [`create_codemode_extension`] to their factories).
//!
//! Provenance map (upstream file → submodule):
//!
//! | upstream | submodule |
//! |---|---|
//! | `index.ts` | this facade |
//! | `tool.ts` | [`tool`] |
//! | `execute.ts` (+ `execute.lazy.ts`) | [`execute`] (the lazy split is a Node module-loader detail) |
//! | `renderer.ts` | cropped (TUI-only; the port's `ToolDefinition` has no render factories) |
//!
//! `codemode` is registered inactive. Activate it with `--tools`, the
//! `defaultTools` setting, or `setActiveTools()`; the MCP extension activates
//! it when MCP tools are only reachable from scripts.
//!
//! Disclosed divergences: `models.*` globals need the model-registry seam the
//! runner does not surface yet (the description still carries the Model API
//! section, the upstream `models: true` default); failed scripts keep their
//! partial output with the upstream `isError` key on the result JSON (the
//! agent-core `AgentToolResult` has no typed field for it); namespace
//! ordering in descriptions uses case-folded byte order where upstream uses
//! `localeCompare`.

pub mod execute;
pub mod tool;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::loader::ExtensionApi;
use super::types::{ToolInfo, ToolNamespace};
use tool::{CodemodeMode, CodemodeToolOptions};

/// Upstream `CodemodeExtensionOptions`.
#[derive(Default)]
pub struct CodemodeExtensionOptions {
    /// Overrides the `codemode.mode` setting.
    pub mode: Option<CodemodeMode>,
    /// Overrides the `codemode.inlineBudget` setting.
    pub inline_budget: Option<usize>,
    /// Expose the model catalog and classifiers to scripts as `models`.
    /// Default `true` (see the module docs for the runtime seam).
    pub models: Option<bool>,
}

/// Upstream `readMode(pi)`.
fn read_mode(pi: &ExtensionApi) -> CodemodeMode {
    let mode = pi.get_settings().ok().and_then(|settings| {
        settings
            .get("codemode")
            .and_then(|codemode| codemode.get("mode"))
            .cloned()
    });
    CodemodeMode::from_settings_value(mode.as_ref())
}

/// Upstream `readInlineBudget(pi)`: `typeof budget === "number" &&
/// Number.isFinite(budget) && budget >= 0 ? budget : undefined`.
fn read_inline_budget(pi: &ExtensionApi) -> Option<usize> {
    let budget = pi
        .get_settings()
        .ok()
        .and_then(|settings| {
            settings
                .get("codemode")
                .and_then(|codemode| codemode.get("inlineBudget"))
                .cloned()
        })
        .and_then(|value| value.as_f64())?;
    if budget.is_finite() && budget >= 0.0 {
        Some(budget as usize)
    } else {
        None
    }
}

/// Upstream `createCodemodeExtension(options)`: registers the codemode tool
/// definition with the session-wired options (`appendEntry`, the tool
/// namespace lookup, and the lazily read `codemode.mode` /
/// `codemode.inlineBudget` settings).
pub fn create_codemode_extension() -> super::loader::ExtensionFactory {
    Arc::new(move |pi: &ExtensionApi| {
        // `appendEntry: (customType, data) => pi.appendEntry(customType, data)`.
        let api = pi.clone();
        let append_entry = Arc::new(
            move |custom_type: &str, data: &tool::CodemodeStoreEntryData| {
                let payload = serde_json::json!({
                    "set": data.set,
                    "delete": data.delete,
                });
                let _ = api.append_entry(custom_type, Some(&payload));
            },
        );
        // `getToolNamespace: (name) => pi.getAllTools().find((tool) => tool.name === name)?.namespace`.
        let api = pi.clone();
        let get_tool_namespace = Arc::new(move |name: &str| -> Option<ToolNamespace> {
            let tools: Vec<ToolInfo> = api.get_all_tools().unwrap_or_default();
            tools
                .into_iter()
                .find(|tool: &ToolInfo| tool.name == name)
                .and_then(|tool| tool.namespace)
        });
        // `getMode: () => options.mode ?? readMode(pi)`.
        let api = pi.clone();
        let get_mode = Arc::new(move || -> CodemodeMode { read_mode(&api) });
        // `getInlineBudget: () => options.inlineBudget ?? readInlineBudget(pi)`.
        let api = pi.clone();
        let get_inline_budget = Arc::new(move || -> Option<usize> { read_inline_budget(&api) });
        let mut definition = tool::create_codemode_tool_definition(CodemodeToolOptions {
            get_tool_namespace: Some(get_tool_namespace),
            models: true,
            model_runtime: None,
            append_entry: Some(append_entry),
            get_mode: Some(get_mode),
            get_inline_budget: Some(get_inline_budget),
        });
        definition.default_active = Some(false);
        pi.register_tool(definition)
    })
}
