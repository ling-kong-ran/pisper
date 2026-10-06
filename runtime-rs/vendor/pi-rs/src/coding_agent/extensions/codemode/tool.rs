//! Port of upstream `coding-agent/src/extensions/codemode/tool.ts` — the
//! `codemode` tool definition: the model-facing description, the parameter
//! schema, loadout presentation, and the execute seam (the executor itself
//! lives in [`super::execute`], upstream's `execute.lazy.ts` split).
//!
//! The description pipeline (`DESCRIPTION_INTRO`, `MODEL_TYPES`,
//! `MODEL_GLOBAL_DECLARATIONS`, `createCodemodeDescription`, `selectCatalog`,
//! `renderToolSection`) is byte-pinned against the verbatim upstream
//! `tool.ts` in `src/codemode/codemode_oracle_tests.rs` (Part B of the
//! capture).
//!
//! Disclosed divergences:
//!
//! - The port's `AgentTool` (agent-core seam) carries no `outputSchema`, so
//!   [`to_codemode_declaration`] always renders the upstream
//!   `TEXT_OUTPUT_SCHEMA` default (`Promise<string>`). The byte-pinned core
//!   [`create_codemode_description_for_declarations`] takes
//!   [`Declarable`]s directly so tests can exercise the upstream output
//!   schema path.
//! - `isCodemodeTool` compares the parameter schema structurally where
//!   upstream compares object identity.
//! - `renderer.ts` is TUI-only and is cropped (the port's `ToolDefinition`
//!   has no render factories); `execute.lazy.ts` is a Node module-loader
//!   detail (the executor loads directly).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent_core::types::AgentTool;
use crate::codemode::declarations::{
    mcp_structured_content_schema, render_declarations, render_tool_sample, Declarable,
    RenderDeclarationsOptions, MCP_TYPESCRIPT_PREAMBLE,
};
use crate::codemode::identifier::to_codemode_identifier;
use crate::codemode::source::CODEMODE_SOURCE_GRAMMAR;
use crate::coding_agent::extensions::types::{
    PrepareLoadoutHandler, ToolDefinition, ToolExposure, ToolLoadout, ToolLoadoutChanges,
    ToolNamespace,
};

/// Upstream `CODEMODE_TOOL_NAME`.
pub const CODEMODE_TOOL_NAME: &str = "codemode";

/// Upstream `CODEMODE_STORE_ENTRY_TYPE`: custom entry type holding one
/// script's `store()` writes.
pub const CODEMODE_STORE_ENTRY_TYPE: &str = "codemode-store";

/// Upstream `CodemodeStoreEntryData`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodemodeStoreEntryData {
    pub set: BTreeMap<String, Value>,
    pub delete: Vec<String>,
}

/// Upstream `CodemodeMode` (`settings-manager.ts`): how the tool presents the
/// loadout while active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodemodeMode {
    On,
    Only,
}

impl CodemodeMode {
    /// Upstream `readMode`: `codemode?.mode === "only" ? "only" : "on"`.
    pub fn from_settings_value(value: Option<&Value>) -> Self {
        if value.and_then(Value::as_str) == Some("only") {
            CodemodeMode::Only
        } else {
            CodemodeMode::On
        }
    }
}

/// Upstream `CodemodeNestedCallStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodemodeNestedCallStatus {
    #[default]
    Running,
    Ok,
    Error,
    Cancelled,
}

impl CodemodeNestedCallStatus {
    pub fn to_str(&self) -> &'static str {
        match self {
            CodemodeNestedCallStatus::Running => "running",
            CodemodeNestedCallStatus::Ok => "ok",
            CodemodeNestedCallStatus::Error => "error",
            CodemodeNestedCallStatus::Cancelled => "cancelled",
        }
    }
}

/// Upstream `CodemodeNestedCall`: one nested call row of the tool details.
/// `id` is the tool call id of the nested call (`<codemode call id>/<n>`),
/// `args` compact JSON truncated for display, `error` the truncated error
/// text, `cost` the USD cost of a `models.*` call that reported usage.
#[derive(Debug, Clone, Default)]
pub struct CodemodeNestedCall {
    pub id: String,
    pub name: String,
    pub args: String,
    pub status: CodemodeNestedCallStatus,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    pub cost: Option<f64>,
}

impl CodemodeNestedCall {
    /// The details JSON row (upstream `CodemodeNestedCall` at the JSON seam).
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("id".to_string(), json!(self.id));
        object.insert("name".to_string(), json!(self.name));
        object.insert("args".to_string(), json!(self.args));
        object.insert("status".to_string(), json!(self.status.to_str()));
        if let Some(duration) = self.duration_ms {
            object.insert("durationMs".to_string(), json!(duration));
        }
        if let Some(error) = &self.error {
            object.insert("error".to_string(), json!(error));
        }
        if let Some(cost) = self.cost {
            object.insert("cost".to_string(), json!(cost));
        }
        Value::Object(object)
    }
}

/// Upstream `CodemodeToolDetails`.
#[derive(Debug, Clone, Default)]
pub struct CodemodeToolDetails {
    pub calls: Vec<CodemodeNestedCall>,
    pub full_output_path: Option<String>,
}

impl CodemodeToolDetails {
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert(
            "calls".to_string(),
            json!(self
                .calls
                .iter()
                .map(CodemodeNestedCall::to_value)
                .collect::<Vec<_>>()),
        );
        if let Some(path) = &self.full_output_path {
            object.insert("fullOutputPath".to_string(), json!(path));
        }
        Value::Object(object)
    }
}

/// Upstream `CodemodeModelRuntime`: the part of the model registry that
/// scripts reach through `models`. Results travel as JSON at the seam. The
/// port's runner has no model-registry surface yet (see the module docs of
/// `extensions`); the machinery here is complete and wired as soon as a
/// handle is supplied.
pub trait CodemodeModelRuntime: Send + Sync {
    /// Upstream `getModelsOfType(type, provider?)`: every known model of a
    /// type, optionally for one provider.
    fn get_models_of_type(
        &self,
        model_type: &str,
        provider: Option<&str>,
    ) -> Result<Vec<Value>, String>;
    /// Upstream `getAvailableOfType(type, provider?, { signal })`.
    fn get_available_of_type<'a>(
        &'a self,
        model_type: &str,
        provider: Option<&str>,
        signal: Option<Arc<crate::coding_agent::extensions::types::AbortSignal>>,
    ) -> futures::future::BoxFuture<'a, Result<Vec<Value>, String>>;
    /// Upstream `getModelOfType(type, provider, id)`: one catalog entry.
    fn get_model_of_type(
        &self,
        model_type: &str,
        provider: &str,
        id: &str,
    ) -> Result<Option<Value>, String>;
    /// Upstream `classify(model, context, { signal })`.
    fn classify<'a>(
        &'a self,
        model: &Value,
        context: &Value,
        signal: Option<Arc<crate::coding_agent::extensions::types::AbortSignal>>,
    ) -> futures::future::BoxFuture<'a, Result<Value, String>>;
}

/// Upstream `CodemodeToolOptions`.
#[derive(Clone, Default)]
pub struct CodemodeToolOptions {
    /// Namespace of a tool, for `searchTools()` ranking and its `namespace`
    /// filter.
    pub get_tool_namespace: Option<GetToolNamespaceFn>,
    /// Expose the `models` namespace to scripts. Without a model runtime the
    /// globals cannot be created (disclosed).
    pub models: bool,
    /// Model runtime behind the `models` globals (port-only option: upstream
    /// reads `ctx.modelRegistry`, which the runner does not surface yet).
    pub model_runtime: Option<Arc<dyn CodemodeModelRuntime>>,
    /// Persists `store()` writes as a session custom entry.
    pub append_entry: Option<AppendCodemodeEntryFn>,
    /// How the tool presents the loadout while active. Default `on`.
    pub get_mode: Option<Arc<dyn Fn() -> CodemodeMode + Send + Sync>>,
    /// Token budget for tool declarations in the description.
    pub get_inline_budget: Option<Arc<dyn Fn() -> Option<usize> + Send + Sync>>,
}

/// Backs `CodemodeToolOptions.getToolNamespace`.
pub type GetToolNamespaceFn = Arc<dyn Fn(&str) -> Option<ToolNamespace> + Send + Sync>;

/// Backs `CodemodeToolOptions.appendEntry`.
pub type AppendCodemodeEntryFn = Arc<dyn Fn(&str, &CodemodeStoreEntryData) + Send + Sync>;

/// Upstream `TEXT_OUTPUT_SCHEMA` (`{ type: "string" }`).
pub fn text_output_schema() -> Value {
    json!({ "type": "string" })
}

/// Upstream `codemodeSchema` (TypeBox). Byte-pinned by the oracle
/// (`partB.codemode_schema_json`).
pub fn codemode_schema() -> Value {
    json!({
        "type": "object",
        "required": ["code"],
        "properties": {
            "code": {
                "type": "string",
                "description": "Raw JavaScript source. Top-level await and return work. May start with a `// @options: {\"max_output_tokens\": 1000}` line."
            }
        }
    })
}

/// Upstream `isCodemodeTool`: whether a registered tool is this package's
/// `codemode` tool rather than another extension's tool with the same name.
/// Compares the parameter schema structurally where upstream compares object
/// identity (disclosed).
pub fn is_codemode_tool(name: &str, parameters: &Value) -> bool {
    name == CODEMODE_TOOL_NAME && parameters == &codemode_schema()
}

/// Upstream `codemodeToolSystemPromptContribution`.
pub fn codemode_system_prompt_contribution() -> (String, Vec<String>) {
    (
        "Run JavaScript that calls other tools (chains, loops, Promise.all, filtering large results)"
            .to_string(),
        vec![
            "Use codemode to batch or chain several tool calls, or to filter large tool output down to what you need, instead of issuing many individual tool calls. Batch independent calls in one codemode call using await Promise.allSettled([...]).".to_string(),
        ],
    )
}

/// Upstream `DESCRIPTION_INTRO`.
pub const DESCRIPTION_INTRO: &str = "Run JavaScript code to orchestrate/compose tool calls\n- Evaluates the provided JavaScript code in a fresh QuickJS sandbox as the body of an async function: top-level `await` and `return` work.\n- All nested tools are available on the global `tools` object, for example `await tools.read(...)`. Tool names are exposed as normalized JavaScript identifiers, for example `await tools.mcp__ologs__get_profile(...)`.\n- Nested tool methods take an object as their input argument.\n- Nested tools return either an object or a string, based on the description.\n- A nested tool call that fails, is blocked, or gets invalid arguments rejects with an Error carrying the tool's error text.\n- Runs raw JavaScript -- no Node, no file system, no network access, no timers.\n- Accepts raw JavaScript source text, not JSON, quoted strings, or markdown code fences.\n- You may optionally start the tool input with a first line like `// @options: {\"max_output_tokens\": 1000, \"timeout_ms\": 60000}`.\n- `max_output_tokens` sets the token budget for the script's output. Defaults to 10000 tokens.\n- `timeout_ms` sets a hard deadline for the whole script. By default there is none.\n- When the JS code is fully evaluated, calls that are still running are cancelled and unawaited promises are silently discarded.\n- Tool calls are real and have side effects. If the script fails partway, earlier calls are not undone.\n- Scripts have a 256 MB memory limit; exceeding it throws `InternalError: out of memory`. Filter or aggregate large data instead of accumulating it.\n\n- Global helpers:\n- `exit()`: Immediately ends the current script successfully (like an early return from the top level).\n- `text(value: string | number | boolean | undefined | null)`: Appends a text item. Non-string values are stringified with `JSON.stringify(...)` when possible.\n- `image(imageUrlOrItem: string | { image_url: string } | ImageContent)`: Appends an image item. `image_url` should be a base64-encoded `data:` URL. To forward an MCP tool image, pass an individual `ImageContent` block from `result.content`, for example `image(result.content[0])`.\n- `store(key: string, value: any)`: stores a serializable value under a string key for later `codemode` calls in the same session. Storing `undefined` deletes the key. Writes are kept only if the script succeeds.\n- `load(key: string)`: returns the stored value for a string key, or `undefined` if it is missing.\n- `ALL_TOOLS`: metadata for the enabled nested tools as `{ name, description }` entries.\n- `searchTools(query: string, options?: { limit?: number; namespace?: string })`: resolves to the nested tools that best match the query (BM25, default limit 8), as `{ name, description }` entries like `ALL_TOOLS`.\n- `describeTool(name: string)`: resolves to the description and declaration of a nested tool, or `undefined`.\n- `describeNamespace(name: string)`: resolves to `{ name, description?, instructions?, tools }` for a namespace of nested tools, such as an MCP server: its usage instructions and the names of its tools, or `undefined`.\n- `console.log(...)` and the other `console` methods append a text item like `text()`.\n- `return value` at the top level appends the value like `text()`.";

/// Upstream `MODEL_TYPES`.
pub const MODEL_TYPES: &str = "type ModelType = \"chat\" | \"image\" | \"classifier\";\n/** A model catalog entry. `provider` and `id` identify it; the other fields depend on the type. */\ninterface ModelInfo {\n  type?: ModelType;\n  provider: string;\n  id: string;\n  name: string;\n  api: string;\n  input: (\"text\" | \"image\")[];\n  contextWindow?: number;\n  [key: string]: unknown;\n}\ntype ClassifierQuestion =\n  | { type: \"choice\"; instructions: string; criteria: Record<string, string> }\n  | { type: \"score\"; instructions: string; criteria: string[] }\n  | { type: \"bool\"; instructions: string; criteria: { true: string; false: string } };\ntype ClassifierAnswer =\n  | { type: \"choice\"; choice: string; probabilities: Record<string, number>; confidence: number }\n  | { type: \"score\"; score: number; confidence: number }\n  | { type: \"bool\"; probability: number };\ninterface ClassifierContext {\n  state: Record<string, unknown>;\n  questions: Record<string, ClassifierQuestion>;\n}\ninterface ClassifierResult {\n  api: string;\n  provider: string;\n  model: string;\n  answers: Record<string, ClassifierAnswer>;\n  /** Set when the service reports token counts. Cost is in USD. */\n  usage?: { input: number; output: number; totalTokens: number; cost: { total: number } };\n  stopReason: \"stop\" | \"error\" | \"aborted\";\n  errorMessage?: string;\n  timestamp: number;\n}";

/// Upstream `MODEL_GLOBAL_DECLARATIONS`: declarations of the `models`
/// globals; codemode-execute implements them. Byte-pinned by the oracle
/// (`partB.model_global_declarations_json`).
pub fn model_global_declarations() -> Vec<Declarable> {
    [
        (
            "models.getModelsOfType",
            "Every known model of a type, optionally for one provider.",
            "(type: ModelType, provider?: string): Promise<ModelInfo[]>",
        ),
        (
            "models.getAvailableOfType",
            "Models of a type whose provider has working credentials.",
            "(type: ModelType, provider?: string): Promise<ModelInfo[]>",
        ),
        (
            "models.getModelOfType",
            "One catalog entry, or undefined.",
            "(type: ModelType, provider: string, id: string): Promise<ModelInfo | undefined>",
        ),
        (
            "models.classify",
            "Run a classifier model on one state. Only `provider` and `id` of `model` are used. Provider errors do not throw: check `stopReason` and `errorMessage`.",
            "(model: ModelInfo, context: ClassifierContext): Promise<ClassifierResult>",
        ),
    ]
    .into_iter()
    .map(|(name, description, signature)| Declarable {
        name: name.to_string(),
        description: Some(description.to_string()),
        input_schema: None,
        output_schema: None,
        spread: false,
        signature: Some(signature.to_string()),
    })
    .collect()
}

/// Upstream `DEFERRED_TOOLS_GUIDANCE`.
pub const DEFERRED_TOOLS_GUIDANCE: &str = "Some deferred nested tools may be omitted from this description. They are still available on the global `tools` object and listed in `ALL_TOOLS`.\nTo find one, call `await searchTools(query)` (pass `{ namespace }` to search one namespace), or filter `ALL_TOOLS` by `name` and `description`. `await describeNamespace(name)` returns a namespace's usage instructions and the names of its tools.";

/// Upstream `DEFAULT_CODEMODE_INLINE_BUDGET` (estimated tokens).
pub const DEFAULT_CODEMODE_INLINE_BUDGET: usize = 3000;
/// Upstream `CHARS_PER_TOKEN` (tool-section cost estimate).
pub const CHARS_PER_TOKEN: usize = 4;

/// Upstream `toCodemodeDeclaration`: what a script sees of a tool. The port's
/// `AgentTool` carries no `outputSchema`, so the upstream
/// `TEXT_OUTPUT_SCHEMA` default applies (disclosed; see the module docs).
pub fn to_codemode_declaration(tool: &AgentTool) -> Declarable {
    Declarable {
        name: tool.name.clone(),
        description: Some(tool.description.clone()),
        input_schema: Some(tool.parameters.clone()),
        output_schema: Some(text_output_schema()),
        spread: false,
        signature: None,
    }
}

/// Upstream `getCodemodeCallableTools`: tools a script may call, except the
/// codemode tool itself.
pub fn get_codemode_callable_tools(tools: &[AgentTool]) -> Vec<AgentTool> {
    tools
        .iter()
        .filter(|tool| tool.name != CODEMODE_TOOL_NAME)
        .cloned()
        .collect()
}

/// Upstream `CodemodeDescriptionOptions`.
#[derive(Clone, Default)]
pub struct CodemodeDescriptionOptions {
    /// Declare the `models` namespace; only for tools created with model
    /// access.
    pub models: bool,
    /// Namespace of each tool, by tool name. Tools of one namespace are
    /// listed under one heading.
    pub namespaces: BTreeMap<String, ToolNamespace>,
    /// Tools that are callable but never listed with their declaration
    /// (`deferred` exposure).
    pub deferred: std::collections::BTreeSet<String>,
    /// Estimated tokens (characters / 4) the tool sections may use. Tools
    /// that do not fit are left out, like deferred tools. `None` lists every
    /// tool that is not deferred.
    pub inline_budget: Option<usize>,
}

/// Upstream `renderToolSection`: `### \`id\` (\`raw name\`)` followed by the
/// tool's description and declaration.
fn render_tool_section(declaration: &Declarable) -> String {
    let id = to_codemode_identifier(&declaration.name);
    let heading = if id == declaration.name {
        format!("### `{id}`")
    } else {
        format!("### `{id}` (`{}`)", declaration.name)
    };
    format!(
        "{heading}\n{}",
        render_tool_sample(declaration, None).trim()
    )
}

#[derive(Clone)]
struct CatalogEntry {
    name: String,
    section: String,
    cost: usize,
    deferred: bool,
}

#[derive(Clone)]
struct CatalogGroup {
    namespace: Option<ToolNamespace>,
    entries: Vec<CatalogEntry>,
}

/// Upstream `selectCatalog`: pick the tool sections that fit the budget, like
/// OpenCode's catalog — in each round every group (tools without a namespace
/// first, then namespaces by name) places its cheapest remaining tool; a
/// group whose next tool does not fit drops out while the others continue.
fn select_catalog(
    groups: &[CatalogGroup],
    budget: Option<usize>,
) -> std::collections::BTreeSet<String> {
    let listable: Vec<Vec<&CatalogEntry>> = groups
        .iter()
        .map(|group| {
            group
                .entries
                .iter()
                .filter(|entry| !entry.deferred)
                .collect()
        })
        .collect();
    let Some(budget) = budget else {
        return listable
            .into_iter()
            .flatten()
            .map(|entry| entry.name.clone())
            .collect();
    };
    // `queues = listable.map((entries) => [...entries].sort((a, b) => a.cost - b.cost))`
    let queues: Vec<Vec<&CatalogEntry>> = listable
        .into_iter()
        .map(|mut entries| {
            entries.sort_by_key(|entry| entry.cost);
            entries
        })
        .collect();
    let mut shown = std::collections::BTreeSet::new();
    let mut remaining = budget as i64;
    // `active = queues.filter((queue) => queue.length > 0)`; the filter below
    // advances each queue by one cheapest entry per round (`queue.shift()`),
    // keeping only queues that placed an entry and are not yet empty.
    let mut active: Vec<Vec<&CatalogEntry>> = queues
        .iter()
        .filter(|queue| !queue.is_empty())
        .cloned()
        .collect();
    while !active.is_empty() {
        active = active
            .into_iter()
            .filter(|queue| {
                let next = queue[0];
                if next.cost as i64 > remaining {
                    return false;
                }
                remaining -= next.cost as i64;
                shown.insert(next.name.clone());
                true
            })
            .map(|queue| queue[1..].to_vec())
            .filter(|queue| !queue.is_empty())
            .collect();
    }
    shown
}

/// Upstream `createCodemodeDescription` core: the model-facing description
/// from already-mapped declarations (the upstream function after its
/// `toCodemodeDeclaration` map). Byte-pinned by the oracle (Part B).
pub fn create_codemode_description_for_declarations(
    declarations: &[Declarable],
    options: &CodemodeDescriptionOptions,
) -> String {
    // `groups = new Map([["", { namespace: undefined, entries: [] }]])` —
    // insertion-ordered; the ungrouped bucket exists even when unused.
    let mut group_keys: Vec<String> = vec![String::new()];
    let mut groups: BTreeMap<String, CatalogGroup> = BTreeMap::new();
    groups.insert(
        String::new(),
        CatalogGroup {
            namespace: None,
            entries: Vec::new(),
        },
    );
    for declaration in declarations {
        let namespace = options.namespaces.get(&declaration.name).cloned();
        let key = match &namespace {
            Some(namespace) => format!("ns:{}", namespace.name),
            None => String::new(),
        };
        let group = groups.entry(key.clone()).or_insert_with(|| {
            group_keys.push(key.clone());
            CatalogGroup {
                namespace,
                entries: Vec::new(),
            }
        });
        let section = render_tool_section(declaration);
        group.entries.push(CatalogEntry {
            name: declaration.name.clone(),
            // `Math.ceil(section.length / CHARS_PER_TOKEN)`; `section.length`
            // is the UTF-16 length.
            cost: section.encode_utf16().count().div_ceil(CHARS_PER_TOKEN),
            section,
            deferred: options.deferred.contains(&declaration.name),
        });
    }
    // `ordered = [...groups.values()].sort(...)` — ungrouped first, then by
    // namespace name (`localeCompare`, ASCII-equal to byte order here).
    let mut ordered: Vec<CatalogGroup> = group_keys.iter().map(|key| groups[key].clone()).collect();
    ordered.sort_by(|a, b| match (&a.namespace, &b.namespace) {
        (None, _) => std::cmp::Ordering::Less,
        (_, None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    let shown = select_catalog(&ordered, options.inline_budget);
    let complete = shown.len() == declarations.len();

    let mut sections: Vec<String> = vec![DESCRIPTION_INTRO.to_string()];
    if !complete {
        sections.push(DEFERRED_TOOLS_GUIDANCE.to_string());
    }
    if declarations.iter().any(|declaration| {
        mcp_structured_content_schema(declaration.output_schema.as_ref()).is_some()
    }) {
        sections.push(format!(
            "Shared MCP Types:\n```ts\n{MCP_TYPESCRIPT_PREAMBLE}\n```"
        ));
    }
    if options.models {
        let models = render_declarations(RenderDeclarationsOptions {
            tools: &[],
            globals: &model_global_declarations(),
        });
        sections.push(format!("Model API:\n```ts\n{MODEL_TYPES}\n\n{models}\n```"));
    }
    if declarations.is_empty() {
        return sections.join("\n\n");
    }

    let mut tool_sections = vec![if complete {
        "Nested tools: COMPLETE list.".to_string()
    } else {
        "Nested tools: PARTIAL list. Find the tools that are not listed with `searchTools()`."
            .to_string()
    }];
    for group in &ordered {
        let visible: Vec<&CatalogEntry> = group
            .entries
            .iter()
            .filter(|entry| shown.contains(&entry.name))
            .collect();
        if let Some(namespace) = &group.namespace {
            let listing = if visible.len() == group.entries.len() {
                ""
            } else if visible.is_empty() {
                " (tools not listed)"
            } else {
                " (some tools not listed)"
            };
            let description = namespace
                .description
                .as_deref()
                .map(str::trim)
                .unwrap_or("");
            tool_sections.push(if description.is_empty() {
                format!("## {}{listing}", namespace.name)
            } else {
                format!("## {}{listing}\n{description}", namespace.name)
            });
        }
        for entry in visible {
            tool_sections.push(entry.section.clone());
        }
    }
    sections.push(tool_sections.join("\n\n"));
    sections.join("\n\n")
}

/// Upstream `createCodemodeDescription`: the description from agent tools.
/// Every callable tool is mapped through [`to_codemode_declaration`] (the
/// `TEXT_OUTPUT_SCHEMA` default applies; see the module docs).
pub fn create_codemode_description(
    tools: &[AgentTool],
    options: &CodemodeDescriptionOptions,
) -> String {
    let declarations: Vec<Declarable> = get_codemode_callable_tools(tools)
        .iter()
        .map(to_codemode_declaration)
        .collect();
    create_codemode_description_for_declarations(&declarations, options)
}

/// Upstream `prepareCodemodeLoadout`: how the codemode tool presents tools
/// that are both declared and callable from scripts.
pub fn prepare_codemode_loadout(
    loadout: &ToolLoadout,
    options: &CodemodeToolOptions,
) -> ToolLoadoutChanges {
    let mode = (options.get_mode.as_ref().map(|get| get())).unwrap_or(CodemodeMode::On);
    let is_direct = |tool: &AgentTool| loadout.get_exposure(&tool.name) == ToolExposure::Direct;
    let callable = get_codemode_callable_tools(&loadout.callable);
    let callable_names: std::collections::BTreeSet<&str> =
        callable.iter().map(|tool| tool.name.as_str()).collect();
    let mut descriptions: BTreeMap<String, String> = BTreeMap::new();
    if mode == CodemodeMode::On {
        for tool in &loadout.declared {
            if callable_names.contains(tool.name.as_str()) {
                descriptions.insert(
                    tool.name.clone(),
                    render_tool_sample(&to_codemode_declaration(tool), None),
                );
            }
        }
    }
    let listed: Vec<&AgentTool> = if mode == CodemodeMode::Only {
        callable.iter().collect()
    } else {
        callable.iter().filter(|tool| !is_direct(tool)).collect()
    };
    let namespaces: BTreeMap<String, ToolNamespace> = listed
        .iter()
        .filter_map(|tool| {
            loadout
                .get_namespace(&tool.name)
                .map(|namespace| (tool.name.clone(), namespace))
        })
        .collect();
    descriptions.insert(
        CODEMODE_TOOL_NAME.to_string(),
        create_codemode_description(
            &listed
                .iter()
                .map(|tool| (*tool).clone())
                .collect::<Vec<_>>(),
            &CodemodeDescriptionOptions {
                models: options.models,
                namespaces,
                deferred: listed
                    .iter()
                    .filter(|tool| loadout.get_exposure(&tool.name) == ToolExposure::Deferred)
                    .map(|tool| tool.name.clone())
                    .collect(),
                inline_budget: (options.get_inline_budget.as_ref().map(|get| get()))
                    .unwrap_or(Some(DEFAULT_CODEMODE_INLINE_BUDGET)),
            },
        ),
    );
    let declared_names: std::collections::BTreeSet<&str> = loadout
        .declared
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    ToolLoadoutChanges {
        descriptions: Some(descriptions),
        hidden_declarations: Some(if mode == CodemodeMode::Only {
            callable
                .iter()
                .filter(|tool| is_direct(tool) && declared_names.contains(tool.name.as_str()))
                .map(|tool| tool.name.clone())
                .collect()
        } else {
            Vec::new()
        }),
    }
}

/// Upstream `createCodemodeToolDefinition`. Registered inactive by the
/// extension (`defaultActive: false`); scripts must not start other scripts
/// (`model-only`); the sandbox loads on the first call, not at startup.
pub fn create_codemode_tool_definition(options: CodemodeToolOptions) -> ToolDefinition {
    let mut definition = ToolDefinition::new(
        CODEMODE_TOOL_NAME,
        CODEMODE_TOOL_NAME,
        // Replaced with the declarations of the callable tools when the tool
        // is activated.
        &create_codemode_description(
            &[],
            &CodemodeDescriptionOptions {
                models: options.models,
                ..Default::default()
            },
        ),
        codemode_schema(),
    );
    let (snippet, guidelines) = codemode_system_prompt_contribution();
    definition.prompt_snippet = Some(snippet);
    definition.prompt_guidelines = Some(guidelines);
    definition.exposure = ToolExposure::ModelOnly;
    // Scripts must not start other scripts: exposure model-only already
    // removes them from the callable set (upstream `exposure: "model-only"`).
    let prepare: PrepareLoadoutHandler = {
        let options = options.clone();
        Arc::new(move |loadout: &ToolLoadout| Some(prepare_codemode_loadout(loadout, &options)))
    };
    definition.prepare_loadout = Some(prepare);
    // Capable models write the script as raw text instead of a JSON-escaped
    // string.
    definition.constrained_sampling = Some(json!({
        "type": "grammar",
        "variants": { "openai_lark": CODEMODE_SOURCE_GRAMMAR }
    }));
    // The sandbox runtime loads on the first call, not at startup.
    definition.execute_async = Some(super::execute::codemode_execute_handler(options));
    definition
}
