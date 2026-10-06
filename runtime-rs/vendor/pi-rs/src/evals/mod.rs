//! M6 slice: port of `pi/packages/evals` (the documentation-eval harness,
//! runner, pairing report and fixture servers).
//!
//! Upstream sources (sha256-registered in the slice report):
//! `src/plan.ts`, `src/report.ts`, `src/docker.ts`, `src/cli.ts`,
//! `src/harness.ts`, plus the fixture surfaces `evals/acme-server.ts` and
//! `evals/configured-runtime.ts`.
//!
//! Seam disclosures (upstream imports that are not ported here):
//! - S1 `@vitest-evals/core` / `@vitest-evals/core/node` are external npm
//!   packages with no source in the upstream repository checkout. The
//!   observable contract (vitest jest-format JSON report + assertion `meta`
//!   carrying `eval`/`harness` payloads) is implemented directly in
//!   [`report`] against the shapes the upstream test-suite exercises.
//! - S2 `@earendil-works/pi-coding-agent`'s agent-session run loop
//!   (`AgentSession`, `createAgentSessionFromServices`, inline extensions) is
//!   represented in [`harness`] by the [`harness::AgentRunner`] seam: the
//!   deterministic prompt/environment/sandbox helpers are ported verbatim,
//!   while the live model run is a caller-supplied closure.
//! - S3 `docker` CLI orchestration in [`docker`] mirrors the upstream
//!   spawn-per-step behavior one-to-one; it requires a local docker daemon,
//!   so tests exercise only the deterministic pieces (identity hashing, arg
//!   construction, auth-file validation).

pub mod acme_server;
pub mod cli;
pub mod configured_runtime;
pub mod docker;
pub mod harness;
pub mod plan;
pub mod report;
