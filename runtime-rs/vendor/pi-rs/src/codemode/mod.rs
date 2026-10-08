//! Port of upstream `@earendil-works/pi-codemode`
//! (`pi/packages/codemode/src`, upstream HEAD `a276dabe5`, **v1.0.0**): a
//! sandboxed JavaScript runtime where the only capability is calling
//! injected tools.
//!
//! Provenance map (upstream file → submodule), byte-identical logic:
//!
//! | upstream | submodule |
//! |---|---|
//! | `identifier.ts` | [`identifier`] |
//! | `declarations.ts` | [`declarations`] |
//! | `source.ts` | [`source`] |
//! | `types.ts` | [`types`] |
//! | `runtime/prelude-source.ts` | [`runtime::prelude`] (embedded byte-exact) |
//! | `runtime/protocol.ts` | [`runtime::protocol`] |
//! | `runtime/worker.ts` + `runtime/host.ts` + `wasm.ts` | [`runtime::engine`] |
//! | `index.ts` | this facade |
//!
//! # Engine (disclosed divergence)
//!
//! Upstream embeds Bellard-line QuickJS compiled to WASM (`quickjs-wasi`
//! 3.6.2, which builds **quickjs-ng**) and drives it from a Node worker
//! thread. This port embeds **quickjs-ng natively** through
//! `rquickjs = "=0.14.0"`, keeping the upstream architecture one-to-one: one
//! fresh VM per execution on its own OS thread (the "worker"), the prelude
//! evaluated before the script (byte-exact, see
//! `tests/fixtures/codemode_oracle/src/runtime/prelude.js`, generated from the
//! verbatim `prelude-source.ts`), the host bridge as a Rust closure, and the
//! settle/drain loop of `worker.ts`.
//!
//! The two engines share the same QuickJS-ng core, so observable semantics
//! match: sloppy-mode eval (upstream `evalCode`), the stack-frame format
//! (`    at f (codemode.js:1:27)` — byte-identical in engine probes),
//! `RangeError: Maximum call stack size exceeded` under the same
//! `MAX_STACK_SIZE = 524288`, JSON/number/Date/BigInt formatting, and the
//! script-visible surface (no timers, no `Intl`, no `fetch`). Remaining
//! engine-level divergences are listed in the module docs of
//! [`runtime::engine`].

pub mod declarations;
pub mod identifier;
pub mod runtime;
pub mod source;
pub mod types;

#[cfg(test)]
mod codemode_oracle_tests;

pub use declarations::{
    mcp_structured_content_schema, render_declarations, render_tool_output_type,
    render_tool_sample, render_tool_signature, schema_to_type, Declarable,
    RenderDeclarationsOptions, DEFAULT_INPUT_SCHEMA_MAX_CHARS, MCP_TYPESCRIPT_PREAMBLE,
};
pub use identifier::to_codemode_identifier;
pub use runtime::engine::{CodemodeSandbox, NO_TIMEOUT};
pub use runtime::prelude::{
    MAX_OUTPUT_CHARS, MAX_OUTPUT_ITEMS, MAX_STORE_TOTAL_CHARS, MAX_STORE_VALUE_CHARS,
};
pub use source::{
    parse_codemode_source, CodemodeSourceError, CODEMODE_OPTIONS_PREFIX, CODEMODE_SOURCE_GRAMMAR,
};
pub use types::{
    CodemodeCall, CodemodeCallStatus, CodemodeError, CodemodeErrorKind, CodemodeExecuteOptions,
    CodemodeJsonSchema, CodemodeOutputItem, CodemodeResult, CodemodeSandboxOptions,
    CodemodeStoreWrites, CodemodeTool, CodemodeToolContext,
};
