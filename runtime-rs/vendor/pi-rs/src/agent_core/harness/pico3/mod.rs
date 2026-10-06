//! Port of `packages/agent/src/harness/pico3/` — the harness-v3 chord-
//! transactional storage engine (M3b Task 8) plus the Task 9 runtime: the
//! strict-JSON record and document vocabulary ([`types`]), the revocable
//! document membrane ([`membrane`]), the model-context projection
//! ([`context`]), the in-memory reference backend ([`memory`]), the durable
//! JSONL backend ([`jsonl`]), the line-serialized Session with
//! capability-checked transactions ([`session`]), the commit-granular
//! conversation view ([`view`]), the chord-glue pure subset ([`chord`]), the
//! bounded output collector ([`bounded`]), the hook registry and runner
//! ([`hooks`]), the bash tool ([`bash`]), the managed system-instructions
//! layer ([`system`]), the kind-execution surface and runtime capability
//! object ([`runtime`]), the built-in kinds ([`kinds`]), the erased-kind
//! scheduler ([`scheduler`]), and the assembled [`harness::Harness`].
//!
//! `pico3/index.ts` (153 lines) upstream is a re-export barrel; the port's
//! equivalent is the `pub use` list at the bottom of this module (the Task 9
//! brief's additions rule index.ts lands as module re-exports).
//!
//! # Disclosed substitutions (module docs carry the per-item notes)
//!
//! - Numeric ids: upstream `Id`/`Seq` are JS numbers; the port uses `i64`
//!   ([`types::Id`], [`types::Seq`]) — all upstream ids are safe integers.
//! - Documents are strict JSON objects ([`crate::agent_core::JsonValue`]);
//!   the "typed" document shapes (`RewindableState`, `StickyState`,
//!   `SessionState`, `ToolSlot`, ...) are accessed through typed accessor
//!   helpers over the same JSON trees, preserving the upstream wire format
//!   and the `[key: string]: JsonValue` open-index behavior.
//! - Upstream hands transactions out as JS `Proxy` capability surfaces
//!   (`callbackSurface()`) and wraps documents in a revocable [`membrane::Membrane`].
//!   Rust has no proxies: capability separation is API visibility (the
//!   callback receives `&mut Tx`; core-only operations are `pub(crate)`),
//!   handle revocation is the explicit liveness flag checked by every
//!   document access, and "a handle cannot outlive its transaction" is the
//!   borrow checker. See [`membrane`] module docs.
//! - Upstream serializes operations on an async promise tail (`Session#enter`)
//!   with reentrancy detection through a context value; the port uses a
//!   tokio mutex for the tail plus the same context-value check
//!   ([`session::NestedLineOperation`]).
//! - `Session::commit` takes the invoker explicitly ([`types::Invoker`]);
//!   upstream's tests reach it through `Harness` conversation handles, whose
//!   commit is a thin host-invoker wrapper. The oracle tests ported here
//!   drive `Session` directly with host/kernel/task invokers and stub kinds —
//!   the same objects upstream's scheduler would construct (disclosed in the
//!   test module docs).

pub mod bash;
pub mod bounded;
pub mod chord;
pub mod context;
pub mod harness;
pub mod hooks;
pub mod jsonl;
pub mod kinds;
pub mod membrane;
pub mod memory;
pub mod runtime;
pub mod scheduler;
pub mod session;
pub mod system;
pub mod types;
pub mod view;

#[cfg(test)]
mod tests;

pub use bash::bash_tool;
pub use bounded::{Bounded, Retain as BoundedRetain};
pub use chord::*;
pub use context::*;
pub use harness::{
    apply_envelope as harness_apply_envelope, builtin_kinds, capture_active_transcript,
    is_core_kind, ConversationHandle, Harness, HarnessOptions, InputHandle, RootOptions,
    WATCH_CAPACITY as HARNESS_WATCH_CAPACITY,
};
pub use hooks::{HookRegistration, HookRunner};
pub use jsonl::*;
pub use membrane::*;
pub use memory::*;
pub use runtime::{
    Kind, Models, Next, PhaseFn, ProcessHost, ProcessStatus, Runtime as PicoRuntime, Step, ToolApi,
    ToolDeclaration, ToolResult,
};
pub use scheduler::{Scheduler, SchedulerDeps};
pub use session::*;
pub use system::{define_system_section, system_sections, SystemSection};
pub use types::*;
pub use view::*;
