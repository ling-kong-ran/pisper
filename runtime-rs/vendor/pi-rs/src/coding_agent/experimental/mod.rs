//! Port of the upstream `coding-agent` `src/experimental/` cluster
//! (`pi/packages/coding-agent/src/experimental`, slice W3.16).
//!
//! Coverage boundary (declared up front): this slice ports the deterministic
//! core face of the cluster — `process.ts`, `source-resolver.ts`, the
//! coordinator protocol types/validation from `coordinator.ts`, the worker
//! protocol schemas plus `WorkerLifecycle` from `session-worker.ts`, the
//! `SessionWorkerManager` state machine from `session-worker-manager.ts`, and
//! the service bookkeeping from `services/worker.ts` + the concrete service
//! payload types from `services/*.ts`. The heavy Node I/O and UI faces are
//! left for later slices with the seams below; upstream sources were hashed
//! at port time and remain authoritative.
//!
//! Seams (each disclosed in the owning submodule's docs):
//! - D1 `coordinator`: the Unix-socket control server/connector
//!   (`CoordinatorConnection.connect`, `startCoordinator` process supervision)
//!   is ported over the `ControlSocket`/`ControlListener`/`ControlConnector`
//!   transport seam; `cfg(unix)` builds a true `UnixListener`/`UnixStream`
//!   backend, Windows builds a loopback-TCP backend (upstream's win32 named
//!   pipes have no stable-`std` server) with identical frames, routing and
//!   error strings. The wire frames, protocol version and validation errors
//!   are byte-compatible; signal handlers stay embedder-owned.
//! - D2 `source_resolver`: Node ESM `registerHooks` module-resolution hooks
//!   are Node-runtime-only; the tsconfig alias model, matching and candidate
//!   resolution are ported verbatim and oracle-tested.
//! - D3 `session_worker`: the worker process main loop is ported over the
//!   transport seam (`connect_control`, `run_command_loop`, the `run(...)`
//!   handler bodies). The Node-runtime-bound pieces stay behind embedder
//!   seams: `proper-lockfile` session ownership, the
//!   `AgentHarness`/`JsonlSessionRepo`/`NodeExecutionEnv` construction
//!   behind `createHarness`, and SIGINT/SIGTERM registration. The protocol
//!   schema surface and the full `WorkerLifecycle` state machine are ported
//!   and test-parity verified.
//! - D4 `process`: `spawnInternalProcess`/`terminateInternalProcess` are
//!   ported over a `ProcessSpawner`/`ChildProcess` seam (real
//!   `std::process::Command` impl provided; child-exit notification via
//!   channel). Env-role consume uses `std::env`.
//! - D5 `session_worker_manager`: upstream `@earendil-works/pi-server`
//!   `RoutedSessionHandle`/`ServerError` are replaced by the local
//!   [`session_worker_manager::RoutedSessionHandle`] /
//!   [`session_worker_manager::WorkerOperationFailure`] equivalents;
//!   coordinator event routing is an explicit `CoordinatorLink` seam rather
//!   than a live socket. The chord control-call codec is real:
//!   `ControlCallCodec::chord()` delegates to
//!   `crate::chord::services::wire`.
//! - D6 `services`: the chord `FacetHost` wiring is real —
//!   `ChordSessionWorkerServices` builds one host over the builtin + plugin
//!   facet generations (`crate::chord::facets`), one
//!   `createRemoteServiceEndpoint` per scope and the serialized reload tail;
//!   the seam-based `SessionWorkerServices` keeps a trait face for
//!   harness-less embedders.

pub mod client;
pub mod client_runtime;
pub mod client_tui;
pub mod client_tui_chat;
pub mod commands;
pub mod coordinator;
pub mod coordinator_entry;
pub mod plugin;
pub mod plugins;
pub mod process;
pub mod radius_auth;
pub mod radius_relay;
pub mod server;
pub mod services;
pub mod session_catalog;
pub mod session_worker;
pub mod session_worker_manager;
pub mod source_resolver;
