//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/` (slices fill the modules).

pub mod config_selector;
pub mod model_selector;
pub mod scoped_models_selector;
pub mod session_selector;
pub mod settings_selector;
pub mod tree_selector;

// ---------------------------------------------------------------------------
// r19 component small-pieces slice. Covered upstream files (the surface list
// in `components/index.ts` is this directory; the large selectors above were
// landed by their own slices and are unchanged):
//
// - `support.rs`           — shared faithful `Box` port (`MessageBgBox`), the
//                            Markdown theme/style helpers and the OSC-133
//                            zone markers. Seam S19.1: the vendored tui
//                            `layout_widgets::Box` predates the bg-fn setter
//                            its private `bg_fn` needs.
// - `keybinding-hints.ts`  → `keybinding_hints.rs` (re-export of the
//                            `model_selector` helpers, coverage note)
// - `dynamic-border.ts`    → `dynamic_border.rs` (same coverage note)
// - `settings-submenu.ts`  → `settings_submenu.rs` (re-export of the
//                            `settings_selector` submenu port)
// - `visual-truncate.ts`   → `visual_truncate.rs`
// - `diff.ts`              → `diff.rs` (jsdiff 8.0.4 wordDiff re-stated;
//                            seam S19.2: UTF-16 vs char indexing)
// - `markdown-transform.ts`→ `markdown_transform.rs`
// - `mermaid.ts`           → `mermaid.rs` (parse face; seam S19.8: the
//                            grok-mermaid renderer + Marked lexer stay
//                            unported, the gate/plan surface is ported)
// - `countdown-timer.ts`   → `countdown_timer.rs` (explicit-tick timer seam)
// - tui `loader.ts` +      → `loader.rs` (hosted here because only these
//   `cancellable-loader.ts`  components consume them; seam S19.3)
// - `status-indicator.ts`  → `status_indicator.rs`
// - `bordered-loader.ts`   → `bordered_loader.rs` (seam S19.4: the
//                            non-cancellable loader unification)
// - `assistant-message.ts` → `assistant_message.rs`
// - `user-message.ts`      → `user_message.rs`
// - `bash-execution.ts`    → `bash_execution.rs`
// - `branch-summary-message.ts`   → `branch_summary_message.rs`
// - `compaction-summary-message.ts` → `compaction_summary_message.rs`
//   (seam S19.5: `toLocaleString` → en-US grouping; seam S19.6 shared:
//   MouseRegion click toggles become `handle_mouse` on the component)
// - `skill-invocation-message.ts` → `skill_invocation_message.rs`
// - `custom-message.ts`    → `custom_message.rs`
// - `custom-entry.ts`      → `custom_entry.rs`
// - `tool-execution.ts`    → `tool_execution.rs` (seam S19.7: convertToPng +
//                            render-utils getTextOutput subset)
// - `footer.ts`            → `footer.rs` (FooterSession/FooterDataProvider
//                            traits over the AgentSession/provider reads)
// - `earendil-announcement.ts` → `earendil_announcement.rs`
// - `session-selector-search.ts` → `session_selector_search.rs` (seam S19.9:
//                            `RegExp` → the vendored `regex` crate)
// - `oauth-selector.ts`    → `oauth_selector.rs`
// - `login-dialog.ts`      → `login_dialog.rs`
// - `first-time-setup.ts`  → `first_time_setup.rs`
// - `extension-input.ts`   → `extension_input.rs`
// - `extension-selector.ts`→ `extension_selector.rs`
// - `extension-editor.ts`  → `extension_editor.rs`
// - `theme-selector.ts` + `show-images-selector.ts` +
//   `thinking-selector.ts` → `simple_selectors.rs`
// - `trust-selector.ts` + `user-message-selector.ts` → `trust_selector.rs`
//
// Not ported (already covered elsewhere): `armin.ts` and `custom-editor.ts`
// are excluded from this slice's scope; `index.ts` is the module list above;
// `chat-viewport.ts` → `super::chat_viewport` and `tui-renderer.ts` →
// `super::tui_renderer` live one level up with their own seam registers
// (D8/D9).

pub mod assistant_message;
pub mod auth_url;
pub mod bash_execution;
pub mod bordered_loader;
pub mod branch_summary_message;
pub mod compaction_summary_message;
pub mod countdown_timer;
pub mod custom_entry;
pub mod custom_message;
pub mod diff;
pub mod dynamic_border;
pub mod earendil_announcement;
pub mod extension_editor;
pub mod extension_input;
pub mod extension_selector;
pub mod footer;
pub mod keybinding_hints;
pub mod loader;
pub mod login_dialog;
pub mod markdown_transform;
pub mod mermaid;
pub mod oauth_selector;
pub mod pi_logo;
pub mod radius_login_selector;
pub mod session_selector_search;
pub mod settings_submenu;
pub mod skill_invocation_message;
pub mod status_indicator;
pub mod support;
pub mod themed_text;
pub mod tool_execution;
pub mod user_message;
pub mod visual_truncate;
