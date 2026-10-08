// Test-only access to the oracle captures in `tests/fixtures/ext_oracle/`. Both
// files were produced by running the actual upstream TypeScript extension
// sources with node (type stripping); see the generator scripts next to the
// captures (`oracle_loader.mjs`, `oracle_runner.mjs`).
//
// - loader.oracle.json — 16 scenarios: runtime stubs, invalidate message,
//   event-bus subscription tracking, flag lifecycle and default-mismatch
//   error, tool parameter-schema errors, factory throws, non-factory export,
//   provider queueing/unregister/discard, inert failed-extension API, stale
//   runtime errors, synthetic source info, pi-manifest battery, discovery
//   battery, discovery pipeline.
// - runner.oracle.json — 45 scenarios: event subscription semantics
//   (self-removal, duplicates, pending removals, deferred registrations,
//   nested dispatch), tool/command/shortcut collection and conflict
//   diagnostics, user_bash validation, input chaining, tool_result chaining,
//   context/provider-payload/headers dispatch, before_agent_start chaining,
//   resources_discover attribution, session_before short-circuit,
//   message_end role guard, tool_call blocking, project_trust, ui_prompt
//   nesting, context defaults, stale contexts, bindCore provider flush,
//   command-context passthrough, session shutdown, flags/renderers.

/// Upstream loader behavior over the scenario battery.
pub const LOADER: &str = include_str!("../../../tests/fixtures/ext_oracle/loader.oracle.json");

/// Upstream runner behavior over the scenario battery.
pub const RUNNER: &str = include_str!("../../../tests/fixtures/ext_oracle/runner.oracle.json");
