//! OpenAI-compatible chat-completions API (upstream
//! `packages/ai/src/api/openai-completions.ts`). This M2b stage ports the
//! compatibility auto-detection and per-field merge ([`compat_detect`]) and
//! the pure request assembly — message conversion, tools, sampling/thinking
//! params, caching, routing, and headers ([`request`]); the streaming
//! implementation lands with the remaining M2b tasks.

pub mod compat_detect;
pub mod request;
pub mod stream;

pub use compat_detect::{
    detect_openai_completions_compat, detect_openai_completions_compat_for_model, merge_compat,
};
pub use request::{build_request, RequestAssembly};
pub use stream::OpenAiCompletions;
