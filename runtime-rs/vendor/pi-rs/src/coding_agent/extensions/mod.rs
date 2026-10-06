//! Port of upstream `coding-agent/src/core/extensions` — the extension system
//! (slice M5 W3.5).
//!
//! Provenance map (upstream file → submodule), upstream SHA256 at migration
//! time:
//!
//! | upstream | submodule | sha256 |
//! |---|---|---|
//! | `core/extensions/types.ts` | [`types`] | `e764ac48…6439f839f5b8` |
//! | `core/extensions/loader.ts` | [`loader`] | `753576ea…0be83a69ffc14a5` |
//! | `core/extensions/runner.ts` | [`runner`] | `92da35c1…8c7` (full hashes in the slice report) |
//! | `core/extensions/index.ts` | this facade | `9b980770…dc14212` |
//! | `core/extensions/wrapper.ts` | [`wrapper`] | `f71eae fd…345188cd` |
//!
//! (Full 64-hex digests are in the slice report; the oracle tree under
//! `tests/fixtures/ext_oracle/src` pins byte-identical copies.)
//!
//! The `src/extensions/` product directory contains only the `llama`
//! extension (local-LLM provider registration over a llama.cpp bridge and
//! HuggingFace model discovery). It depends on the llama bridge, the pi-ai
//! provider/stream surface, and the full TUI component set — none ported yet
//! — so the product extension is cropped (disclosed); the loader that would
//! discover it is fully ported.
//!
//! Deterministic outputs (pi-manifest parsing, discovery order, loader error
//! texts, flag/provider queueing semantics, runner event order, shortcut
//! conflict diagnostics, command invocation-name suffixing, user_bash
//! validation, input transform chaining, before_agent_start chaining,
//! ui_prompt nesting) were captured from the verbatim upstream TypeScript
//! sources under node (type stripping) into
//! `tests/fixtures/ext_oracle/{loader,runner}.oracle.json` and are pinned in
//! [`oracle_data`] (test-only) with byte-comparison tests. The generator
//! scripts live next to the captures; the captured `loader.ts` / `runner.ts`
//! are byte-identical to upstream (hashes above), with only the surrounding
//! module graph stubbed (jiti, bundled pi packages, theme, system-prompt
//! section rendering) — see the `tests/fixtures/ext_oracle/src` file headers.
//!
//! Shared seams (upstream imports modules outside this slice):
//!
//! - **jiti module loading** (loader.ts): Rust has no runtime TS module
//!   system; path → factory resolution is the [`loader::ExtensionModuleLoader`]
//!   trait, with the upstream factory cache ported over it. Everything after
//!   resolution is oracle-pinned; the jiti "Cannot find module" text is not.
//! - **SessionManager / ModelRegistry** (session-manager.ts /
//!   model-registry.ts, not yet ported): [`types::SessionManagerHandle`] and
//!   the [`types::ProviderRegistryHandle`] trait.
//! - **Provider / ScopedModel / SlashCommandInfo / ProviderConfig /
//!   ProviderModelConfig** (pi-ai + model-registry slices): JSON at the seam.
//!   The `registerProvider` queueing/routing semantics are fully ported;
//!   model-registry-owned validation text (e.g. `"api" is required when
//!   registering streamSimple`) is reproduced by test mocks, not here.
//! - **AbortSignal** (DOM): vendored minimal [`types::AbortSignal`].
//! - **TUI component surface** (`Theme`, `Component`, `TUI`,
//!   `OverlayOptions`, `AutocompleteProvider`, editor components): the UI
//!   context is the [`types::ExtensionUI`] trait with the exact
//!   `noOpUIContext` defaults; `ToolDefinition`'s `renderCall`/`renderResult`
//!   component factories are omitted.
//! - **buildSystemPrompt section rendering** (system-prompt.ts): options and
//!   normalization are ported verbatim; the renderer is the
//!   [`types::NormalizedSystemPromptRenderer`] seam handed to
//!   [`runner::ExtensionRunner::emit_before_agent_start`], pinning the
//!   `forceSystemPrompt` short-circuit.
//! - **BashOperations / BashResult** (tools slices): `exec` is an opaque
//!   closure seam; `user_bash` validation is ported structurally, with
//!   callable operations traveling through
//!   [`types::HandlerResult::UserBashOperations`].
//! - **Async**: general handlers return borrowed [`types::HandlerFuture`] values;
//!   dispatchers and AgentSession consumers await them sequentially (r12).
//!   Native handlers/dispatch are poll-driven, whereas command actions, setModel,
//!   and setup/withSession use eager shared [`types::CommandFuture`] results.
//!   Factories and the UI facade remain synchronous. Neither API is a JS host:
//!   Promise/microtask timing, arbitrary thrown values/stacks and JavaScript
//!   object identity/aliasing across the JSON seam are not fully reproduced.
//!   `ExtensionError.stack` stays `None`.
//! - **Detached notifications**: ui_prompt and the upstream void session-info /
//!   thinking-level notifications schedule owned, awaited work on Tokio. A
//!   missing runtime is explicitly reported, not blocked on or silently dropped.
//!   Nesting/outer-only semantics are retained; Tokio scheduling does not emulate
//!   JS `queueMicrotask` or the eager prefix of an async JavaScript handler.
//!
//! Extensions-delta additions (upstream `590144609..2bbfcca43`), oracle-pinned
//! from the verbatim HEAD sources under node into
//! `tests/fixtures/extensions_delta_oracle/` (see
//! [`extensions_delta_tests`]):
//!
//! - **Tool context** (`ExtensionToolContext`): the runner's
//!   [`runner::ExtensionRunner::create_tool_context`] attaches `tools` and
//!   `executeTool` to a fresh extension context (upstream
//!   `Object.defineProperties`); [`types::ExtensionContext::execute_tool`]
//!   defaults the nested-call signal and falls back to the exact upstream
//!   "Nested tool calls are not available in this context" outcome. Tool
//!   wrappers (this module's `wrapper`) hand the tool context to `execute`.
//! - **Boundary events** (`turn_end` / `agent_before_settle`):
//!   [`runner::ExtensionRunner::emit_boundary`] chains draft entries and the
//!   continue flag through handlers, rebuilding the context preview after
//!   each; a failed rebuild zeroes the decision (`valid: false`).
//! - **Cache warming**: [`runner::ExtensionRunner::emit_cache_warming_decision`]
//!   returns the event's action unless a handler overrides it (last override
//!   wins).
//! - **Two-phase context**: `context` handlers see the conversation without
//!   system messages and the folded leading system message is restored after
//!   each (`restoreSystemMessages` + pi-ai `getCurrentSystemMessage`, ported
//!   at the JSON seam in the runner); `context_with_system` handlers see the
//!   full transcript and their output is used as returned, with the
//!   leading-system-removal error pinned verbatim.
//! - **MCP servers**: `pi.registerMcpServer`/`unregisterMcpServer`/
//!   `getMcpServers` over the ported [`crate::coding_agent::core::mcp_servers`]
//!   registry; the runner's change sink fires `mcp_servers_change`
//!   (fire-and-forget) and reports unhandled servers once each. The upstream
//!   registry `setChangeListener` hook cannot re-enter the port's locked
//!   runtime state, so mutations fire the runner-installed sink after the
//!   state lock releases — same observable behavior (disclosed).
//! - **Virtual models**: `pi.registerVirtualModel`/`unregisterVirtualModel`
//!   queue pre-bind and route post-bind (provider actions, else the
//!   [`types::ProviderRegistryHandle`] fallback); the wrapped route binds
//!   `runtime.createContext()` per request and pre-bind calls reject with the
//!   exact `notInitialized` message.
//! - **tool_search**: the [`tool_search`] module ports
//!   `extensions/tool-search/` (BM25 ranker, document text, description,
//!   schema JSON, execute semantics, prepareLoadout); the built-in extension
//!   table ([`built_in_extensions`]) mirrors `src/extensions/index.ts` with
//!   the tool-search factory (llama.cpp stays cropped; codemode/mcp belong to
//!   their own slices). `isToolSearchTool` compares parameter values
//!   structurally where upstream compares object identity (disclosed).

pub mod command_future;

#[cfg(test)]
mod async_event_tests;

#[cfg(test)]
mod command_context_tests;

#[cfg(test)]
mod extensions_delta_tests;

#[path = "loader.rs"]
pub mod loader;
#[path = "mcp/mod.rs"]
pub mod mcp;
#[path = "runner.rs"]
pub mod runner;
#[path = "tool_search.rs"]
pub mod tool_search;
#[path = "types.rs"]
pub mod types;

pub mod codemode;

#[cfg(test)]
#[path = "oracle_data.rs"]
pub mod oracle_data;

pub use loader::ExtensionApi;
pub use loader::{
    clear_extension_cache, discover_and_load_extensions, load_extension_from_factory,
    load_extensions, load_extensions_cached, ExecOptions, ExecResult, ExtensionCacheToken,
    ExtensionFactory, ExtensionModuleLoader, ExtensionRuntime, NullModuleLoader, PiManifest,
    TrackedEventBusUnsubscribe,
};
pub use runner::{
    emit_project_trust_event, emit_session_shutdown_event, BoundaryDispatchResult, ExtensionRunner,
};
pub use tool_search::{
    create_tool_search_extension, create_tool_search_tool_definition, tokenize, Bm25Ranker,
    ToolRanker, ToolSearchDocument, ToolSearchMatch, ToolSearchTools, TOOL_SEARCH_DESCRIPTION,
};
pub use types::{
    Extension, ExtensionCommandContext, ExtensionContext, ExtensionError, ExtensionLoadError,
    ExtensionLoadWarning, HandlerFn, LoadExtensionsResult, ToolAnnotations, ToolExposure, ToolInfo,
    ToolLoadout, ToolLoadoutChanges, ToolNamespace,
};

// ============================================================================
// Built-in extensions (upstream `src/extensions/index.ts`)
// ============================================================================

/// Upstream `InlineExtension` object form: `{ name, factory, hidden?,
/// replaceable?, builtin? }` for the CLI's built-in extensions. `builtin`
/// marks the extension as a `builtin:<name>` extension resource (loads by
/// default, disabled by `-builtin:<name>`/`--no-extensions`, loads after
/// project trust); `replaceable` leaves registration conflicts to a
/// third-party extension registering the same tool/command/flag name.
#[derive(Clone)]
pub struct BuiltInExtension {
    pub name: &'static str,
    pub factory: ExtensionFactory,
    pub replaceable: bool,
    pub builtin: bool,
}

/// Upstream `builtInExtensions`. The port carries the `codemode` and
/// `tool-search` factories (in the upstream table's relative order —
/// `llama.cpp` stays cropped: it needs the llama bridge, pi-ai
/// provider/stream surface, and the TUI — and the `mcp` factory belongs to
/// its own slice).
pub fn built_in_extensions() -> Vec<BuiltInExtension> {
    vec![
        BuiltInExtension {
            name: "codemode",
            factory: codemode::create_codemode_extension(),
            replaceable: true,
            builtin: true,
        },
        BuiltInExtension {
            name: "tool-search",
            factory: tool_search::create_tool_search_extension(),
            replaceable: true,
            builtin: true,
        },
    ]
}

/// Port of upstream `wrapper.ts`: tool wrappers for extension-registered
/// tools. The wrappers adapt tool execution so extension tools receive the
/// runner context; tool call/result interception is handled by AgentSession
/// via agent-core hooks (not part of this slice).
///
/// The full upstream `wrapToolDefinition` (tools slice) does argument
/// re-validation after `prepareArguments`; this port copies the declaration
/// fields, applies the shim, and executes against the shared runner context.
pub mod wrapper {
    use std::sync::Arc;

    use anyhow::anyhow;

    use super::runner::ExtensionRunner;
    use super::types::{AbortSignal, RegisteredTool};
    use crate::agent_core::types::{
        AgentTool, AgentToolResult, ExecuteFn, PrepareArgumentsFn, ToolExecutionMode,
    };
    use crate::ai::types::ConstrainedSampling;

    struct AbortForwarder(Option<tokio::task::JoinHandle<()>>);
    impl Drop for AbortForwarder {
        fn drop(&mut self) {
            if let Some(task) = self.0.take() {
                task.abort();
            }
        }
    }

    /// Upstream `wrapRegisteredTool(registeredTool, runner)` — uses the
    /// runner's `createToolContext()` for consistent context across tools and
    /// event handlers: the context handed to `execute` is the
    /// `ExtensionToolContext` (extension context plus `tools` and
    /// `executeTool`), keyed by the call's id with the call's signal as the
    /// nested-call default.
    pub fn wrap_registered_tool(
        registered_tool: &RegisteredTool,
        runner: &ExtensionRunner,
    ) -> AgentTool {
        let definition = registered_tool.definition.clone();
        let runner = runner.clone();
        // Upstream `wrapToolDefinition(definition, (toolCallId, signal) =>
        // runner.createToolContext(toolCallId, signal))`.
        let context_provider = Arc::new(
            move |tool_call_id: &str,
                  signal: Option<Arc<AbortSignal>>|
                  -> super::types::ExtensionContext {
                runner.create_tool_context(tool_call_id, signal)
            },
        );

        let execute: Arc<ExecuteFn> = {
            let definition = Arc::clone(&definition);
            let context_provider = Arc::clone(&context_provider);
            Arc::new(move |tool_call_id, params, signal, on_update| {
                let definition = Arc::clone(&definition);
                let context_provider = Arc::clone(&context_provider);
                type ValueUpdateCallback = Arc<dyn Fn(&serde_json::Value) + Send + Sync>;
                let on_update: Option<ValueUpdateCallback> = on_update.map(|callback| {
                    let forward: ValueUpdateCallback =
                        Arc::new(move |value: &serde_json::Value| {
                            if let Ok(typed) =
                                serde_json::from_value::<AgentToolResult>(value.clone())
                            {
                                callback(&typed);
                            }
                        });
                    forward
                });
                Box::pin(async move {
                    // The forwarding task is scoped to this execution, including
                    // future cancellation: never leave an idle token waiter behind.
                    let mut forwarding = AbortForwarder(None);
                    let signal: Option<Arc<AbortSignal>> = signal.map(|token| {
                        let aborted = Arc::new(AbortSignal::new());
                        if token.is_cancelled() {
                            aborted.abort();
                        } else {
                            let forward = Arc::clone(&aborted);
                            forwarding.0 = Some(tokio::spawn(async move {
                                token.cancelled().await;
                                forward.abort();
                            }));
                        }
                        aborted
                    });
                    // The tool context is created per execution with this
                    // call's id and (forwarded) signal.
                    let ctx = context_provider(&tool_call_id, signal.clone());
                    let result = if let Some(execute) = &definition.execute_async {
                        execute(tool_call_id, params, signal, on_update, ctx).await
                    } else if let Some(execute) = &definition.execute {
                        execute(
                            &tool_call_id,
                            &params,
                            signal.as_ref(),
                            on_update.as_ref(),
                            &ctx,
                        )
                    } else {
                        return Err(anyhow!(
                            "Tool \"{}\" does not define an execute implementation",
                            definition.name
                        ));
                    }
                    .map_err(|error| anyhow!("{error}"))?;
                    serde_json::from_value::<AgentToolResult>(result).map_err(|error| {
                        anyhow!(
                            "Tool \"{}\" produced an invalid result: {error}",
                            definition.name
                        )
                    })
                })
            })
        };

        AgentTool {
            name: definition.name.clone(),
            label: definition.label.clone(),
            description: definition.description.clone(),
            parameters: definition.parameters.clone(),
            constrained_sampling: definition.constrained_sampling.as_ref().and_then(|value| {
                serde_json::from_value::<ConstrainedSampling>(value.clone()).ok()
            }),
            execute,
            prepare_arguments: definition.prepare_arguments.clone().map(|prepare| {
                Arc::new(move |value: serde_json::Value| prepare(&value)) as Arc<PrepareArgumentsFn>
            }),
            replay: None,
            execution_mode: definition
                .execution_mode
                .as_deref()
                .and_then(|mode| match mode {
                    "sequential" => Some(ToolExecutionMode::Sequential),
                    "parallel" => Some(ToolExecutionMode::Parallel),
                    _ => None,
                }),
        }
    }

    /// Upstream `wrapRegisteredTools(registeredTools, runner)`.
    pub fn wrap_registered_tools(
        registered_tools: &[RegisteredTool],
        runner: &ExtensionRunner,
    ) -> Vec<AgentTool> {
        registered_tools
            .iter()
            .map(|tool| wrap_registered_tool(tool, runner))
            .collect()
    }
}

#[cfg(test)]
mod async_tool_tests;
