//! Port of `src/tools/**`: the durable-flavored agent tool factories
//! (`bash`, `edit`, `read`, `write`) and their shared internals. The factories
//! build [`ToolRegistration`] values (`harness/types.ts`) whose `execute`
//! closes over the [`ToolExecutionApiLike`] invocation surface; upstream's
//! typebox declarations are reproduced as JSON Schema literals (verified
//! byte-for-byte against typebox 1.3.27 by the oracle fixtures).
//!
//! Ported files (one-to-one): [`bash`] (`bash.ts`), [`edit`] (`edit.ts`),
//! [`edit_diff`] (`edit-diff.ts`), [`env`] (`env.ts`),
//! [`file_mutation_queue`] (`file-mutation-queue.ts`), [`image`]
//! (`image.ts`), [`path_utils`] (`path-utils.ts`), [`read`] (`read.ts`),
//! [`write`] (`write.ts`); `index.ts` maps to this module's re-exports.
//!
//! Per-file divergences, continuing the module-wide numbering:
//!
//! - **D31 (duplicated upstream twin).** Upstream
//!   `packages/agent/src/harness/tools/` carries byte-identical twins of
//!   `edit-diff.ts` (and near-identical `image.ts` / `path-utils.ts` /
//!   `file-mutation-queue.ts`) that the port already landed in
//!   `agent_core::harness::tools`; the durable implementations here are
//!   ported from the durable files, adapting to the durable seams (sync env
//!   per D3, the chord `Context`, the `PlainError` channel per D4) instead of
//!   cross-referencing the other package's port.
//! - **D32 (lenient argument extraction).** `execute` decodes its input with
//!   `serde` against the declaration's shape; a schema violation that reaches
//!   `execute` surfaces as the port's plain error, where upstream would
//!   dereference `undefined` or fail inside the shell call. Unreachable
//!   through the harness, which validates against the same declaration first.
//! - **D33 (mutation-queue release).** Upstream eagerly deletes the finished
//!   per-key queue entry when it is still the chain tail; the port drops the
//!   per-key lock weakly, so an entry lingers until the next registration.
//!   Identical serialization and ordering.
//! - **D34 (onOutput rejections).** Upstream `onOutput: (text) =>
//!   api.output(text)` propagates an `api.output` rejection into the exec
//!   callback path; the port's [`crate::durable::env::OnOutput`] callback
//!   cannot propagate, so a rejection after the call settled is dropped.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod env;
pub mod file_mutation_queue;
pub mod image;
pub mod path_utils;
pub mod read;
pub mod write;

pub use bash::{create_bash_tool, BashExecution, BashPrepare, BashToolInput, BashToolOptions};
pub use edit::{create_edit_tool, EditToolDetails, EditToolInput};
pub use edit_diff::Edit;
pub use read::{create_read_tool, ReadToolDetails, ReadToolInput};
pub use write::{create_write_tool, WriteToolInput};

/// Upstream `CodingTools` (v1.0.0, `tools/index.ts`): `read`, `write`,
/// `edit`, and `bash`; nothing installs it automatically.
///
/// Divergence (D-types, disclosed): upstream's extension carries the four
/// executable `ToolRegistration` values; the port's erased
/// [`ExtensionSpec`](crate::durable::harness::agent::ExtensionSpec) holds
/// JSON payloads, so each spec carries the registration's pi-ai declaration
/// (name, description, parameters) and the host binds executables through
/// its own registration seam. `bash` uses the default
/// [`BashToolOptions`], matching the option-free upstream call.
pub fn coding_tools() -> crate::durable::harness::agent::ExtensionSpec {
    use crate::durable::harness::agent::{ExtensionSpec, ToolSpec};
    use crate::durable::harness::define::define_extension;
    let tools = [
        create_read_tool(),
        create_write_tool(),
        create_edit_tool(),
        create_bash_tool(BashToolOptions::default()),
    ];
    define_extension(ExtensionSpec {
        name: String::from("coding-tools"),
        tools: tools
            .iter()
            .map(|registration| ToolSpec {
                name: registration.name().to_string(),
                payload: serde_json::to_value(&registration.tool)
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect(),
        ..ExtensionSpec::default()
    })
}

#[cfg(test)]
mod oracle_tests;
