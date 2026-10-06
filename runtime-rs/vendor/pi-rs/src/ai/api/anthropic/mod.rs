//! Anthropic Messages API (upstream `packages/ai/src/api/anthropic-messages.ts`).
//! The port splits into the pure request assembly — message conversion with
//! full-fidelity thinking replay, tools, thinking/effort configs, caching, and
//! headers ([`request`]) — and the streaming implementation ([`stream`]) that
//! sends the assembled request and reduces the SSE protocol into canonical
//! `AssistantMessageEvent`s.

pub mod request;
pub mod stream;

pub use request::{build_request, options_from_simple, AnthropicOptions, RequestAssembly};
pub use stream::AnthropicMessages;
