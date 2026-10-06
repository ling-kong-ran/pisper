//! Oracle test ports for the pico3 storage engine, from
//! `packages/agent/test/harness/pico3/` (M3b Task 8's executable spec).
//!
//! # Disclosed test-harness substitution
//!
//! Upstream drives the pico3 core through `helpers.open()` — a full
//! `Harness` (Task 9) whose conversation handles commit with the host
//! invoker and pass the conversation's rewindable/sticky docs, and whose
//! scheduler runs registered kinds. The port drives the same objects the
//! harness would drive — [`Session::commit`] with explicit
//! host/kernel/task invokers and stub [`BasicKind`] metadata for the core
//! kinds (`pi.generation`/`pi.tool`/`pi.post_tools`/`pi.collapse`, turn)
//! and `pi.plugin` (background) — so every Session/storage/view behavior in
//! scope is exercised without the scheduler. Test files map as follows:
//!
//! | Upstream oracle | Ported here | Remaining (needs Task 9/10) |
//! | --- | --- | --- |
//! | `kinds.test.ts` | `oracle_kinds` (defineTask/defineEntry reservation, config facade, Defaults disjointness, validators) | tool/collapse/fork/section/turn flows |
//! | `membrane.test.ts` | `oracle_membrane` (liveness unit, failed-callback rollback) | escaped-proxy identity/clone assertions (borrow checker; disclosed) |
//! | `reads.test.ts` | `oracle_session` (read-your-writes, ReadAfterWrite, config, namespaces, line semantics) | task-runtime scope probes (partially ported via direct task invokers) |
//! | `spec-storage-history.test.ts` | `oracle_storage` | fork-granular history via namespace plugins (ported at storage level) |
//! | `atomicity.test.ts` | `oracle_storage` (chop/replay, torn tails, stale sidecars) | tool-execution crash halves |
//! | `spec-transactions.test.ts` | `oracle_session` | runtime-captured overlays (checkpoint/slot ported via task invokers) |
//! | `spec-context-capabilities.test.ts` | `oracle_session` (line context, task scope matrix, forged metadata) | phase-expiry (scheduler) |
//! | `authority.test.ts` | `oracle_session` (owner registry, host tx matrix, token lifetime, stale tokens) | registry/hook identity (harness) |
//! | `busy.test.ts` | `oracle_session` (prospective busy, GenerationInProgress) | phase-map contract (scheduler) |
//! | `spec-plugins-lifecycle.test.ts` | `oracle_session` (routing, token identity) | plugin task lifecycle (scheduler) |
//! | `spec-view-events.test.ts` | `oracle_view` | generation streaming halves needing models |
//! | `view.test.ts` / `watch.test.ts` | `oracle_view` (snapshot shape, revisions, buffering, listener isolation, fold==fresh) | turn-view streaming updates |
//! | `chord.test.ts` | `oracle_chord` (applyTracked ops, published view round-trip) | bridge/services (M6 + harness) |
//! | `recovery.test.ts`, `retention.test.ts`, `turn/subagent/tool-bounds/spec-scheduler-process/waiters` | — | scheduler/models/tools (Task 9/10) |
//! | `types.compile.ts` | capability hiding is structural (`pub` host vs `pub(crate)` core); runtime half asserted in `oracle_session` | negative type assertions have no Rust equivalent (disclosed) |

#[path = "tests/support.rs"]
mod support;

mod oracle_chord;
mod oracle_kinds;
mod oracle_membrane;
mod oracle_runtime_lifecycle;
mod oracle_runtime_queue;
mod oracle_runtime_turn;
mod oracle_runtime_waiters;
mod oracle_session;
mod oracle_storage;
mod oracle_view;
#[path = "tests/support_runtime.rs"]
mod support_runtime;

mod oracle_runtime_hooks;

mod oracle_runtime_authority;
mod oracle_runtime_recovery;
mod oracle_runtime_tool_bounds;

mod oracle_runtime_process;
mod support_process;

mod oracle_runtime_scheduler;
