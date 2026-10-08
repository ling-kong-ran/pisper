//! Port of upstream `coding-agent/src/extensions/tool-search/` (the
//! `tool_search` tool as an extension; upstream HEAD `2bbfcca43`,
//! `tool.ts` sha256
//! `49a58777e21cc824dc72c55c60c82c6182b178cb6741c8e4b75d132139b0ef0f`,
//! `index.ts` sha256
//! `212528d795f06a7b935670534efa35371277e31b074a6ecd7ff144bce09ee7b6`).
//!
//! Tool discovery: a BM25 ranker over tool metadata. `tool_search` searches
//! tools that are not declared to the model (`codemode` and `deferred`
//! exposure) and loads the matches, so they are declared for the next model
//! call. Loading goes through the active tool set, so it is recorded in the
//! transcript like any other tool change.
//!
//! Deterministic behavior (tokenization/stemming, document text, BM25 scores
//! and tie order, description text, schema JSON, execute semantics against a
//! fixed tool list, prepareLoadout descriptions) is oracle-pinned from the
//! verbatim upstream sources under node (type stripping) in
//! `tests/fixtures/extensions_delta_oracle/oracle/extensions_delta_oracle.json`.
//!
//! Disclosed seams:
//! - **TypeBox schema**: `toolSearchSchema` is the pinned serialized JSON of
//!   the upstream `Type.Object(...)` (byte-identical to the real typebox
//!   1.3.27 output in the oracle); the port has no TypeBox runtime.
//! - **`isToolSearchTool` identity**: upstream compares `tool.parameters ===
//!   toolSearchSchema` (object identity); the port compares parameter values
//!   structurally.
//! - **`tools` option seam**: upstream takes `Pick<ExtensionAPI,
//!   "getAllTools" | "getActiveTools" | "setActiveTools">`; the port takes
//!   the [`ToolSearchTools`] trait, implemented for [`ExtensionApi`] (stale
//!   runtime errors upstream surface as throws — the trait impl maps them to
//!   empty results / no-ops, matching the inactive-tool fallback path).
//! - **execute context**: the closure is sync-side; async handlers wrap it.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use super::loader::ExtensionApi;
use super::types::{
    ExtensionContext, HandlerError, ToolDefinition, ToolExposure, ToolInfo, ToolNamespace,
};

pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";
pub const DEFAULT_TOOL_SEARCH_LIMIT: usize = 8;

/// A tool as the ranker sees it: its name and the text built by
/// [`create_tool_search_document`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchDocument {
    pub name: String,
    pub text: String,
}

/// A ranked match.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSearchMatch {
    pub name: String,
    pub score: f64,
}

/// Ranks tools for a query. BM25 today; a hybrid ranker with embeddings can
/// replace it (upstream `ToolRanker`).
pub trait ToolRanker: Send + Sync {
    fn rank(
        &self,
        query: &str,
        documents: &[ToolSearchDocument],
        limit: usize,
    ) -> Vec<ToolSearchMatch>;
}

const STOP_WORDS: [&str; 21] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on",
    "or", "that", "the", "this", "to", "with",
];

/// Naive singular form, so `issues` matches `issue` and `searches` matches
/// `search` (upstream `stem`).
fn stem(term: &str) -> String {
    let len = term.chars().count();
    if len > 4 && term.ends_with("ies") {
        let head = &term[..term.len() - 3];
        return format!("{head}y");
    }
    if len > 4
        && ["ches", "shes", "sses", "xes", "zes"]
            .iter()
            .any(|suffix| term.ends_with(suffix))
    {
        return term[..term.len() - 2].to_string();
    }
    if len > 3 && term.ends_with('s') && !term.ends_with("ss") {
        return term[..term.len() - 1].to_string();
    }
    term.to_string()
}

/// Lowercase terms, split at camelCase boundaries and non-alphanumerics,
/// without stop words (upstream `tokenize`: two regex passes —
/// `([a-z0-9])([A-Z])` and `([A-Z]+)([A-Z][a-z])` insert spaces, then split
/// on `[^a-z0-9]+`).
pub fn tokenize(text: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(text.len() + 8);
    let chars: Vec<char> = text.chars().collect();
    for (index, &current) in chars.iter().enumerate() {
        let previous = if index > 0 {
            Some(chars[index - 1])
        } else {
            None
        };
        let next = chars.get(index + 1).copied();
        let split_before = match previous {
            // ([a-z0-9])([A-Z]) → "aB" → "a B", "9A" → "9 A".
            Some(p)
                if (p.is_ascii_lowercase() || p.is_ascii_digit())
                    && current.is_ascii_uppercase() =>
            {
                true
            }
            // ([A-Z]+)([A-Z][a-z]) → "HTTPServer" → "HTTP Server",
            // "IDn" → "I Dn": split an acronym from a following capital.
            Some(p)
                if p.is_ascii_uppercase()
                    && current.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase()) =>
            {
                true
            }
            _ => false,
        };
        if split_before {
            spaced.push(' ');
        }
        spaced.push(current);
    }
    spaced
        .to_lowercase()
        .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit())
        .filter(|term| !term.is_empty() && !STOP_WORDS.contains(term))
        .map(stem)
        .collect()
}

/// JS `text.split(/\r?\n/)[0]`: the text before the first newline; a lone
/// `\r` stays inside the line.
fn first_line(text: &str) -> &str {
    match text.find('\n') {
        Some(index) => {
            let mut end = index;
            if end > 0 && text.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
            &text[..end]
        }
        None => text,
    }
}

/// Schema descriptions and property names, recursively (upstream
/// `schemaText`).
fn schema_text(schema: &Value, parts: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        return;
    };
    if let Some(description) = object.get("description").and_then(Value::as_str) {
        parts.push(description.to_string());
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            parts.push(name.clone());
            schema_text(property, parts);
        }
    }
    if let Some(items) = object.get("items") {
        schema_text(items, parts);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = object.get(key).and_then(Value::as_array) {
            for variant in variants {
                schema_text(variant, parts);
            }
        }
    }
}

/// Search text of a tool: the name, the name with `_` as spaces, the
/// description, schema descriptions and property names, and the namespace
/// with its description and instructions (upstream
/// `createToolSearchDocument`).
pub fn create_tool_search_document(
    name: &str,
    description: &str,
    parameters: &Value,
    namespace: Option<&ToolNamespace>,
) -> ToolSearchDocument {
    let mut parts = vec![
        name.to_string(),
        name.replace('_', " "),
        description.to_string(),
    ];
    schema_text(parameters, &mut parts);
    if let Some(namespace) = namespace {
        parts.push(namespace.name.clone());
        parts.push(namespace.description.clone().unwrap_or_default());
        parts.push(namespace.instructions.clone().unwrap_or_default());
    }
    let text = parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    ToolSearchDocument {
        name: name.to_string(),
        text,
    }
}

/// Okapi BM25 with the usual parameters. Ties keep document order (upstream
/// `Bm25Ranker`; k1 1.2, b 0.75).
#[derive(Debug, Clone)]
pub struct Bm25Ranker {
    k1: f64,
    b: f64,
}

impl Default for Bm25Ranker {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

impl Bm25Ranker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_parameters(k1: f64, b: f64) -> Self {
        Self { k1, b }
    }
}

impl ToolRanker for Bm25Ranker {
    fn rank(
        &self,
        query: &str,
        documents: &[ToolSearchDocument],
        limit: usize,
    ) -> Vec<ToolSearchMatch> {
        // JS `[...new Set(tokenize(query))]`: deduplicated in first-seen
        // order.
        let mut seen = HashSet::new();
        let query_terms: Vec<String> = tokenize(query)
            .into_iter()
            .filter(|term| seen.insert(term.clone()))
            .collect();
        if query_terms.is_empty() || documents.is_empty() || limit == 0 {
            return Vec::new();
        }
        let term_counts: Vec<std::collections::HashMap<String, u64>> = documents
            .iter()
            .map(|document| {
                let mut counts = std::collections::HashMap::new();
                for term in tokenize(&document.text) {
                    *counts.entry(term).or_insert(0u64) += 1;
                }
                counts
            })
            .collect();
        let lengths: Vec<u64> = term_counts
            .iter()
            .map(|counts| counts.values().copied().sum())
            .collect();
        let total: f64 = lengths.iter().map(|length| *length as f64).sum();
        // `sum / documents.length || 1`: a zero total falls back to 1.
        let average_length = {
            let average = total / documents.len() as f64;
            if average == 0.0 {
                1.0
            } else {
                average
            }
        };
        let idf = |term: &str| -> f64 {
            let frequency = term_counts
                .iter()
                .filter(|counts| counts.contains_key(term))
                .count();
            (1.0 + (documents.len() as f64 - frequency as f64 + 0.5) / (frequency as f64 + 0.5))
                .ln()
        };
        let mut matches: Vec<ToolSearchMatch> = Vec::new();
        for (index, document) in documents.iter().enumerate() {
            let mut score = 0.0;
            for term in &query_terms {
                let Some(count) = term_counts[index].get(term) else {
                    continue;
                };
                let count = *count as f64;
                let norm =
                    self.k1 * (1.0 - self.b + (self.b * lengths[index] as f64) / average_length);
                score += idf(term) * ((count * (self.k1 + 1.0)) / (count + norm));
            }
            if score > 0.0 {
                matches.push(ToolSearchMatch {
                    name: document.name.clone(),
                    score,
                });
            }
        }
        // Stable descending sort: ties keep document order (JS sort is
        // stable).
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(limit);
        matches
    }
}

/// The serialized JSON of the upstream TypeBox schema
/// (`Type.Object({ query: Type.String({ description }), limit:
/// Type.Optional(Type.Number({ description })) })`), byte-identical to the
/// real typebox 1.3.27 output pinned in the oracle.
pub fn tool_search_schema() -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query for deferred tools.",
            },
            "limit": {
                "type": "number",
                "description": "Maximum number of tools to return. Defaults to 8.",
            },
        },
    })
}

/// Whether the tool is this `tool_search`, not another extension's tool of
/// the same name. Upstream compares `parameters` by object identity; the port
/// compares values structurally (disclosed seam).
pub fn is_tool_search_tool(name: &str, parameters: &Value) -> bool {
    name == TOOL_SEARCH_TOOL_NAME && *parameters == tool_search_schema()
}

/// The session's tools seam (upstream `Pick<ExtensionAPI, "getAllTools" |
/// "getActiveTools" | "setActiveTools">`).
pub trait ToolSearchTools: Send + Sync {
    fn get_all_tools(&self) -> Vec<ToolInfo>;
    fn get_active_tools(&self) -> Vec<String>;
    fn set_active_tools(&self, tool_names: &[String]);
}

impl ToolSearchTools for ExtensionApi {
    fn get_all_tools(&self) -> Vec<ToolInfo> {
        self.get_all_tools().unwrap_or_default()
    }
    fn get_active_tools(&self) -> Vec<String> {
        self.get_active_tools().unwrap_or_default()
    }
    fn set_active_tools(&self, tool_names: &[String]) {
        let _ = self.set_active_tools(tool_names);
    }
}

/// Result entries of a `tool_search` call (upstream
/// `ToolSearchResultTool`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchResultTool {
    pub name: String,
    pub description: String,
}

/// Whether `tool_search` can load a tool with this exposure (upstream
/// `isSearchable`).
fn is_searchable(exposure: ToolExposure) -> bool {
    matches!(exposure, ToolExposure::Codemode | ToolExposure::Deferred)
}

/// Rank the searchable tools that are not active yet and activate the
/// matches, so the next model call declares them (upstream `searchAndLoad`).
fn search_and_load(
    tools: &dyn ToolSearchTools,
    query: &str,
    limit: usize,
) -> Vec<ToolSearchResultTool> {
    let active = tools.get_active_tools();
    let candidates: Vec<ToolInfo> = tools
        .get_all_tools()
        .into_iter()
        .filter(|tool| {
            is_searchable(tool.exposure) && !active.iter().any(|name| name == &tool.name)
        })
        .collect();
    let documents: Vec<ToolSearchDocument> = candidates
        .iter()
        .map(|tool| {
            create_tool_search_document(
                &tool.name,
                &tool.description,
                &tool.parameters,
                tool.namespace.as_ref(),
            )
        })
        .collect();
    let matches = Bm25Ranker::new().rank(query, &documents, limit);
    if !matches.is_empty() {
        let mut next = active.clone();
        next.extend(matches.iter().map(|match_| match_.name.clone()));
        tools.set_active_tools(&next);
    }
    matches
        .into_iter()
        .map(|match_| ToolSearchResultTool {
            description: candidates
                .iter()
                .find(|tool| tool.name == match_.name)
                .map(|tool| tool.description.clone())
                .unwrap_or_default(),
            name: match_.name,
        })
        .collect()
}

/// The `tool_search` description (v1.0.0 `TOOL_SEARCH_DESCRIPTION`). It does
/// not list the searchable tools or their namespaces, so it stays the same
/// while tools are registered, for example when MCP servers connect.
pub const TOOL_SEARCH_DESCRIPTION: &str = "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nSome of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search`.";

/// Options for [`create_tool_search_tool_definition`] (upstream
/// `ToolSearchToolOptions`). `tools` is the session's tools; without it the
/// tool finds nothing. An `ExtensionApi` fits.
#[derive(Clone, Default)]
pub struct ToolSearchToolOptions {
    pub tools: Option<Arc<dyn ToolSearchTools>>,
}

/// The `tool_search` tool definition (upstream
/// `createToolSearchToolDefinition`). Registered inactive:
/// `defaultActive: false`.
pub fn create_tool_search_tool_definition(options: ToolSearchToolOptions) -> ToolDefinition {
    let mut definition = ToolDefinition::new(
        TOOL_SEARCH_TOOL_NAME,
        TOOL_SEARCH_TOOL_NAME,
        TOOL_SEARCH_DESCRIPTION,
        tool_search_schema(),
    );
    definition.prompt_snippet =
        Some("Search for tools that are not loaded yet and load the matches".to_string());
    // Searching is not something scripts need; it changes what the model sees.
    definition.exposure = ToolExposure::ModelOnly;
    definition.default_active = None;
    let tools = options.tools;
    definition.execute = Some(Arc::new(
        move |_tool_call_id: &str,
              params: &Value,
              _signal: Option<&Arc<super::types::AbortSignal>>,
              _on_update: Option<&super::types::AgentToolUpdateCallbackValue>,
              _ctx: &ExtensionContext|
              -> Result<Value, HandlerError> {
            let query = params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if query.trim().is_empty() {
                return Err("query must not be empty".to_string());
            }
            // `limit ?? DEFAULT` (JSON null counts as absent), then
            // `Number.isInteger(max) && max > 0`.
            let max = match params.get("limit") {
                None | Some(Value::Null) => Some(DEFAULT_TOOL_SEARCH_LIMIT),
                Some(value) => value.as_f64().and_then(limit_from_json),
            };
            let Some(max) = max else {
                return Err("limit must be a positive integer".to_string());
            };
            let found = match &tools {
                Some(tools) => search_and_load(tools.as_ref(), query, max),
                None => Vec::new(),
            };
            let text = if found.is_empty() {
                "No matching tools found.".to_string()
            } else {
                let plural = if found.len() == 1 { "" } else { "s" };
                let lines = found
                    .iter()
                    .map(|tool| {
                        format!(
                            "- {name}: {description}",
                            name = tool.name,
                            description = first_line(tool.description.trim())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "Loaded {count} tool{plural}. They are available from your next call:\n{lines}",
                    count = found.len()
                )
            };
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "details": { "loaded": found.into_iter().map(|tool| tool.name).collect::<Vec<_>>() },
            }))
        },
    ));
    definition
}

/// `limit` validation (upstream `Number.isInteger(max) && max > 0`).
fn limit_from_json(value: f64) -> Option<usize> {
    if value.is_finite() && value.fract() == 0.0 && value > 0.0 {
        Some(value as usize)
    } else {
        None
    }
}

/// Upstream `createToolSearchExtension()`: the CLI loads it as a built-in
/// extension; SDK users add it to their extension factories. `tool_search` is
/// registered inactive — activate it with `--tools`, the `defaultTools`
/// setting, or `setActiveTools()`.
pub fn create_tool_search_extension() -> super::loader::ExtensionFactory {
    Arc::new(|pi: &ExtensionApi| {
        let mut definition = create_tool_search_tool_definition(ToolSearchToolOptions {
            tools: Some(Arc::new(pi.clone())),
        });
        definition.default_active = Some(false);
        pi.register_tool(definition)
    })
}
