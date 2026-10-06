# Pisper patches to pi-rs 0.2.2

This directory was copied from the local Cargo registry's published crates.io
`pi-rs` **0.2.2** package. The upstream MIT `LICENSE`, normalized `Cargo.toml`,
`Cargo.toml.orig`, and `.cargo_vcs_info.json` are retained. No dependency version
upgrade or broad port refactor is included. `target`, `.git`, and Cargo's local
`.cargo-ok` marker were not copied.

Published package provenance:

- Crate archive: `pi-rs-0.2.2.crate`.
- Archive SHA-256: `efaa8ab7e7d928f480a277fc848636301b51db1d069efc5e193fb28803c53cf7`.
- Published VCS metadata commit: `d69b4751dee5a2404225b7dedde694ebcaef078c`.
- The published VCS metadata reports `dirty: true`; this is preserved upstream
  metadata, not a claim that the package matches a clean Git checkout.

Pisper uses a local Cargo `[patch.crates-io]` override for this package. The
following changes make the native built-in MCP extension work in ordinary
headless session startup:

1. `src/coding_agent/extensions/mcp/index.rs`: release the initialization
   `SharedState` mutex before calling `ensure_discovery_active`, which acquires
   that same mutex. The published implementation deadlocks even with no servers.
2. The same module: actively drive the shared startup future on Tokio. A stored
   `Shared<BoxFuture>` is lazy and was only awaited by the `/mcp` command.
   Populate each server's ready handle synchronously before returning so the
   ordinary first prompt can wait for direct tools. Preserve the shared pending
   handle, existing generation checks, connection failures, and shutdown path.
   Recheck generation and the server index under the creation guard so a
   concurrent shutdown/reload cannot create a stale connection or index a
   cleared server table.
   Honor the existing optional credential-store override rather than silently
   ignoring it, so native hosts and isolated tests can select their auth path.
3. `extensions/types.rs`, `loader.rs`, and `runner.rs`: retain a shared overlay
   for tools registered after the extension factory returns. The original
   factory result clones a value-based tool map, so subsequent MCP registrations
   otherwise never reach the runner. Initial static registrations stay in the
   original map; dynamic replacements retain order and do not duplicate names.
   A fresh extension/runtime on reload receives a fresh overlay. The single
   direct `Extension` test initializer in `core/bug_report_tests.rs` is updated
   for the new field. The general `OrderedMap` type is unchanged.
4. `Cargo.toml` registers the isolated `pisper_mcp_regression` integration test
   target. Its tests dispatch the actual native extension's startup and shutdown,
   exercise static/dynamic tool visibility, and use the real MCP client with an
   in-memory JSON-RPC mock server to initialize, list tools, and invoke a tool
   twice during ordinary startup. They also check a fresh reload drops removed
   server tools and immediate shutdown prevents deferred connection creation.
   Temporary paths isolate authentication and logs.
5. `extensions/mcp/runtime.rs`: track the client and transport during setup,
   including a hanging initialize or initial tools/list. Closing the connection
   removes the cached shared opening future and closes both setup and established
   resources. Initial tools/list failures close their transport before returning.
   Publishing Connected rechecks closed/generation under the same state guard,
   so shutdown cannot be undone by a late successful setup. Cached future cleanup
   compares future identity, and the initial future reference is weak. Regression
   tests cancel a real in-memory initialize request, reject initial tools/list,
   and, on Windows, start and reap an isolated actual stdio fixture process whose
   initialize never completes.
6. `coding_agent/agent_session.rs`: Hidden tool definitions are excluded from
   actual active, prompt, and transcript-restored callable tool sets. Refresh
   clears hidden pending selections; changing a tool from Hidden back to Direct
   permits its normal default activation even though its name already existed.
   This closes the case where a disabled MCP tool remained executable after
   reload, and supports re-enabling the same tool name.
7. The same module exposes `set_tool_execution_wrapper` as a narrow native host
   adapter. Registry rebuilds apply it to fresh built-in and extension executors,
   so loadout changes, reload, and late MCP registrations retain workspace asset
   capture. Replacing/removing the adapter rebuilds from original definitions;
   it never stacks wrappers. Capture locks belong inside executable futures:
   parallel preflight awaits every before-tool hook before any tool executes.

8. `AgentSession::continue_queued` resumes previously queued input through the
   same idle/abort, persistence, retry and compaction lifecycle as a normal
   prompt. Pisper uses this when an Agent completion arrives just as a public
   run settles. The durable completion mailbox is acknowledged only after its
   delivery marker appears in the real session transcript; an in-memory queue
   append alone does not count as delivery.
   Consumed user messages remain observable as in flight from MessageStart
   until their MessageEnd append succeeds, covering awaited extension hooks
   after the UI queue removes them. Failed transcript appends are latched and
   returned by the native run, preventing further paid retries in that runtime.
   Pisper validates completion receipts against the actual saved JSONL, rather
   than the session manager's memory (which is updated before persistence).
   The optional `PromptOptions.start_cancellation` and
   `continue_queued_cancellable` preserve a host cancellation through preflight
   to the native start boundary and keep polling started native runs to settle.
   Default prompt and continuation behavior remain compatible.
   Server-target `execution_adapter::completion_regressions` tests use an
   isolated actual AgentSession with an offline faux model to hold the consumed
   queue window, force a real session-file I/O failure, and cancel awaited
   before_agent_start preflight without a model call.

Run the targeted tests from Pisper's root:

```powershell
cargo test --manifest-path runtime-rs/Cargo.toml --package pi-rs --test pisper_mcp_regression
```

The crates.io package excludes much of `tests/fixtures` even though upstream
library test modules reference those fixtures. The dedicated integration target
avoids enabling the unrelated upstream library test modules.

Read-only audit: `emit_change` invokes menu subscribers synchronously while its
state guard is held, and a subscriber that rebuilds `servers_menu` can reenter
the same mutex. Pisper's headless native extension does not install the TUI
manager subscribers. That separate TUI issue is documented here and remains
outside these startup/tool-registration changes.

Pisper supplies cwd-bound host tools through `CliCustomToolFactory` in the
production CLI/runtime factory. The SDK keeps these definitions across reload;
session switches rebuild them for the effective cwd. `set_active_tools_inner`
deduplicates names after default, custom and pending tool lists are combined,
so a custom `bash` override is sent to the model exactly once. The executable
Pisper regression lives in `src/host_factory_tests.rs` and exercises native
creation, reload and two session switches without enabling unpublished vendor
oracle fixtures. The asset execution wrapper composes archival and file-change
capture around the same final tool registry.
