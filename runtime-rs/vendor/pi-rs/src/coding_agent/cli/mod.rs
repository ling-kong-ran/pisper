//! Port of upstream `coding-agent` `src/cli` (slice W3.17, M5).
//!
//! Provenance map (upstream file → submodule), upstream SHA256 at migration
//! time (see each submodule's docs for the full table):
//!
//! | upstream                   | submodule            | sha256 (first 12) |
//! |----------------------------|----------------------|-------------------|
//! | args.ts                    | [`args`]             | f8ba813e8acb      |
//! | auth-check.ts              | [`auth_check`]       | bed735dae4b5      |
//! | auth-command.ts            | [`auth_command`]     | c313c83c6a89      |
//! | config-selector.ts         | [`config_selector`]  | 780107f9a3a2      |
//! | credential-print.ts        | [`credential_print`] | 67400cdf9459      |
//! | file-processor.ts          | [`file_processor`]   | 86e6da37b408      |
//! | initial-message.ts         | [`initial_message`]  | b9df15ffa876      |
//! | list-models.ts             | [`list_models`]      | a8a47f1de640      |
//! | project-trust.ts           | [`project_trust`]    | f53dace04498      |
//! | session-picker.ts          | [`session_picker`]   | 3418d62b3792      |
//! | setup.ts                   | [`setup`]            | 94e76bf5f1f4      |
//! | startup-ui.ts              | [`startup_ui`]       | 109f7896fbc8      |
//! | experimental/cli.ts        | [`experimental::cli`]        | 65d7601b4946 |
//! | experimental/command-options.ts | [`experimental::command_options`] | d6251c5e21bd |
//! | experimental/command.ts    | [`experimental::command`]    | e5fed217d6fa |
//! | experimental/commands/server.ts | [`experimental::commands::server`] | 63537bdf1aab |
//! | experimental/commands/client.ts | [`experimental::commands::client`] | 9ffed74a3acd |
//!
//! Deterministic outputs (the full `parseArgs` battery, help/error strings,
//! auth-command parsing, credential-extraction rules, the `--list-models`
//! table, and the experimental `Command` grammar) were captured from the real
//! upstream TypeScript sources under `node --experimental-strip-types` into
//! `tests/fixtures/cli_oracle/oracle.json` and are byte-compared in tests.
//!
//! Seams (upstream interactive TUI surfaces not yet ported — the interactive
//! session shell lives in `modes/interactive` upstream and has no Rust port
//! yet):
//! - [`config_selector`], [`session_picker`], [`startup_ui`] keep the pure
//!   decision logic (`shouldRunFirstTimeSetup`, distribution identity, theme
//!   dedup) and route the actual TUI screens through the
//!   [`startup_ui::StartupUiHost`] trait.
//! - [`file_processor`] ports the text path fully; `processImage` (upstream
//!   `utils/image-process.ts`, outside this slice) is an injectable closure.
//!
//! Divergences (numbered, disclosed per submodule as well):
//! 1. `printHelp` / `printAuthCommandHelp` / `listModels` return the rendered
//!    text instead of writing to `console.log` (upstream prints); callers
//!    print. `chalk` is rendered plain (identical to upstream under
//!    non-TTY/node, where chalk emits no escape codes).
//! 2. `setup.ts` `process.title = APP_NAME` has no `std` equivalent and is
//!    skipped; the env-var writes and `configureHttpDispatcher` are ported.
//! 3. Locale-aware `String.localeCompare` sorts are `str` byte-order sorts
//!    (identical for ASCII provider/model ids, which is the test corpus).

pub mod args;
pub mod auth_check;
pub mod auth_command;
pub mod config_selector;
pub mod credential_print;
pub mod experimental;
pub mod file_processor;
pub mod initial_message;
pub mod list_models;
pub mod project_trust;
pub mod session_picker;
pub mod setup;
pub mod startup_ui;

/// Upstream `APP_NAME` (`config.ts:502`, `piConfigName || "pi"`).
pub const APP_NAME: &str = crate::coding_agent::package_manager::cli::APP_NAME;

/// Upstream `CONFIG_DIR_NAME` (`config.ts:504`, default `".pi"`).
pub const CONFIG_DIR_NAME: &str = crate::coding_agent::core::CONFIG_DIR_NAME;

/// Upstream `ENV_AGENT_DIR` (`config.ts:508`).
pub const ENV_AGENT_DIR: &str = crate::coding_agent::core::ENV_AGENT_DIR;

/// Upstream `ENV_SESSION_DIR` (`config.ts:509`).
pub const ENV_SESSION_DIR: &str = "PI_CODING_AGENT_SESSION_DIR";
