//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/` (r16 slice: the
//! deterministic core of the interactive mode shell, selected to build
//! directly on the r14/r15 `agent_session` deliverables).
//!
//! r17 adds the theme stack and the external editor (see the r17 seams).
//!
//! Covered upstream files:
//! - `model-search.ts` (21 lines, sha256 `9be620174c0f2551…`) — search-text
//!   builders for the /model selector, ported verbatim in
//!   [`model_search`].
//! - `session-share.ts` (206 lines, sha256 `f5add6fccb39e7475…`)
//!   deterministic core plus its dependency `core/session-export.ts` —
//!   the JSONL share export in [`session_share`]. The Radius/gh network
//!   transports and the TUI loader choreography are presentation, not
//!   deterministic core, and remain unported (disclosed seam).
//! - `model-catalog-refresh.ts` (51 lines, sha256 `a7a10647deaba503…`) —
//!   the shared-refresh coordinator in [`model_catalog_refresh`].
//!
//! r17 (theme stack + external editor):
//! - `theme/theme.ts` (1234 lines, sha256 `c3bf2e3b72f6bb78…`) — the
//!   deterministic theme core (color utilities, `Theme` ANSI tables,
//!   setting/detection helpers, HTML export colors, language table) in
//!   [`theme`]. The fs watcher, the `globalThis` singleton, the chalk
//!   color-level probe, and the by-name fs registry are presentation seams
//!   (D1/D2 below).
//! - `theme/theme-json.ts` (146 lines, sha256 `144a2c1e6b9a92c5…`) — the
//!   document shape and validator in [`theme_json`].
//! - `external-editor.ts` (46 lines, sha256 `b05a9c1e8a88fd5c…`) — the editor
//!   choreography in [`external_editor`].
//! - `theme/dark.json` / `theme/light.json` are embedded byte-identical
//!   (sha256 `103a5aecb74a2dab…` / `14c7172ba7e75eab…`).
//!
//! Still unported from `modes/interactive/` (remaining slices): the 6648-line
//! `interactive-mode.ts` shell, `chat-viewport.ts`, `tui-renderer.ts`,
//! `theme/theme-controller.ts` (TUI-coupled controller), `components/*`.
//!
//! # Seams
//!
//! - **`crypto.randomUUID().slice(0, 8)`** (session-share.ts:30): the share
//!   entry id becomes an explicit [`session_share::export_session_for_share`]
//!   parameter, mirroring how the ported `SessionManager` injects minted ids;
//!   the oracle fixes it to `"abcd1234"`.
//! - **`new Date().toISOString()`** (session-export.ts:25,23): the export
//!   timestamp becomes a parameter on the `_at` core function; the
//!   non-`_at` wrappers read the real clock. The oracle fixes it to
//!   `"2026-02-03T04:05:06.789Z"`.
//! - **`session.state.systemPrompt` / `session.state.tools`**
//!   (session-share.ts:34-38): read through the ported
//!   [`crate::coding_agent::agent_session::AgentSession`] accessors
//!   (`system_prompt`, `get_all_tools`), which is the same state the upstream
//!   reads (the ported tool registry is the state.tools loadout).
//! - **`raceWithAbortSignal` detached-promise divergence** carries over from
//!   `utils/abort.rs`: the port's `AbortSignal` is
//!   `tokio_util::sync::CancellationToken` and Rust futures cancel at the
//!   select point where JS promises detach.
//!
//! r17 seams:
//! - **D1 — global theme singleton / fs watcher** (theme.ts:739-906): the
//!   `globalThis` `Symbol.for` sharing, `initTheme`/`setTheme` bookkeeping and
//!   the reload watcher are process/TUI presentation; the port exposes
//!   explicit documents (`get_builtin_theme_json`, `load_builtin_theme`) and
//!   leaves the watcher unported.
//! - **D2 — chalk color level** (theme.ts:335-353): `chalk.bold` and friends
//!   depend on env/TTY color-level detection; the port fixes the enabled-level
//!   codes (`\x1b[1m…\x1b[22m` etc.). Text passing through these stylers is
//!   env-dependent upstream and not oracle-compared.
//! - **D3 — typebox validator wording** (theme-json.ts): typebox is not
//!   installable offline; the schema semantics are re-stated and the
//!   required-colors / name-rule messages are byte-exact, but typebox's
//!   "Other errors" phrasing for malformed values is not reproduced.
//! - **D4 — `localeCompare`** (theme.ts:452): theme-name sorting uses
//!   byte-wise ordering, identical for the ASCII names the oracle covers.
//! - **D5 — `parseInt(_, 16)` leniency** (theme.ts:110-122): upstream parses
//!   slices like `"fg"` as 15 (leading-prefix parse); the port rejects
//!   non-hex slices outright. Only reachable for malformed hand-written
//!   themes; the oracle grid uses valid hex.
//! - **D6 — editor spawn** (external-editor.ts:27-34): `stdio: "inherit"` +
//!   win32 `shell: true` is mirrored via `cmd /d /s /c` with a raw argument
//!   line; the launching banner goes to `println!` instead of
//!   `process.stdout.write`.
//! - **D7 — auto-theme early return** (theme.ts:709-729): upstream returns
//!   the color-scheme result without waiting for the background query; the
//!   port awaits both (the color-scheme result still wins), so a hanging
//!   background query would delay the return. Tests model both queries
//!   resolving.

pub mod bug_report;
pub mod external_editor;
pub mod model_catalog_refresh;
pub mod model_search;
pub mod session_share;
pub mod system_theme;
pub mod theme;
pub mod theme_json;

#[cfg(test)]
#[path = "interactive_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "system_theme_oracle_tests.rs"]
mod system_theme_oracle_tests;

#[cfg(test)]
#[path = "interactive_delta_oracle_tests.rs"]
mod interactive_delta_oracle_tests;
#[cfg(test)]
#[path = "shell_oracle_tests.rs"]
mod shell_oracle_tests;

pub mod components;
pub mod interactive_mode;

/// r20 (shell): the interactive session shell — the r18 upper-half decision
/// cores ([`shell`]) plus the lower half ([`shell_lower`]): selectors, the
/// footer/status pump, the extension UI bridge, command handlers, and the
/// exit/cleanup paths.
pub mod shell;
pub mod shell_lower;

/// r19 (component small-pieces + view layer): see the per-module docs and the
/// seam register in `components/mod.rs`.
pub mod chat_viewport;
pub mod tui_renderer;
