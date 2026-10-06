//! Port of upstream `packages/agent` (the `@earendil-works/pi-agent-core`
//! package): stateful agent with tool execution and event streaming.
//!
//! M3a Task 1 ports the shared type surface from
//! `packages/agent/src/types.ts` ([`types`]); M3a Task 2 ports the agent loop
//! core turns ([`agent_loop`]) — prompt list, per-turn context build,
//! streaming, tool execution, and toolResult messages; M3a Task 3 the loop's
//! steering/follow-up/abort sections with the `PendingMessageQueue`; M3a
//! Task 4 the stateful [`agent::Agent`] wrapper (lifecycle events, awaited
//! subscribers, prompt/continue, reset, queue delegation). This module is a
//! sibling of `ai`, not a child: the ai layer is upstream
//! `@earendil-works/pi-ai`.
//!
//! M3b adds the harness subtree under [`harness`] (upstream
//! `packages/agent/src/harness/`): M3b Task 1 the chord-support subset
//! ([`chord_support`]), M3b Task 2 the harness foundation types
//! ([`harness`]) — the harness module keeps its own namespace rather than
//! glob-re-exporting here, since it grows a full subpackage across M3b.

pub mod agent;
pub mod agent_loop;
pub mod chord_support;
pub mod harness;
pub mod session;
pub mod telemetry;
pub mod tools;
pub mod types;

pub use agent::*;
pub use agent_loop::*;
pub use session::*;
pub use tools::builtin_tools;
pub use types::*;
