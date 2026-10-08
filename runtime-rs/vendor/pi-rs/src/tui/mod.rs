//! Port of the upstream `packages/tui` package (M4): text utilities,
//! differential terminal rendering, input decoding, screens and widgets.
//! See `docs/migration/TUI_COMPATIBILITY.md` for the remaining package gaps;
//! the presence of a module does not imply whole-package compatibility.
//!
//! Disclosed representation boundaries:
//! - Most public text/component APIs accept and return Rust UTF-8 strings.
//!   Their index arithmetic uses byte offsets and scalar boundaries. Terminal
//!   spacing-mark measurement reproduces JS lengths with `char::len_utf16`.
//! - LaTeX parsing/layout and Markdown rendered lines retain actual UTF-16
//!   code units, including lone surrogates. [`utf16::Utf16Text`],
//!   [`latex::render_latex_utf16`] and
//!   [`components::markdown::Markdown::render_utf16`] expose lossless results.
//!   Markdown style callbacks also receive/return raw units. The UTF-8
//!   convenience APIs encode lossily only at their respective return boundary;
//!   this is not yet a whole-TUI or OS-terminal-boundary guarantee. Markdown
//!   source input, transform and highlighting hooks still use UTF-8.
//! - Upstream's 512-entry shared `widthCache` is an optimization, not
//!   observable behavior, and is not ported.
//! - `Intl.Segmenter` grapheme segmentation becomes `unicode-segmentation`
//!   (UAX #29); `\p{...}` v-flag properties become ICU4X property data; the
//!   `get-east-asian-width` tables and the `\p{RGI_Emoji}` sequence set are
//!   generated tables (see `east_asian_width.rs` / `rgi_emoji.rs` and the
//!   generator `docs/migration/reference/generate-tui-width-tables.mjs`).
//!   Raw-unit width/wrapping uses a mapped segmentation view while retaining
//!   original units and distinguishing lone surrogates from actual U+FFFD.

pub mod alt_screen;
pub mod autocomplete;
pub mod colors; // upstream src/colors.ts
pub mod component;
pub mod component_mouse;
pub mod components;
pub mod fuzzy;
pub mod keybindings;
pub mod keys;
pub mod latex;
pub mod layout;
pub mod layout_node;
pub mod markdown_lexer;
pub mod mouse_dispatch;
pub mod oklab; // upstream src/oklab.ts
pub mod overlay;
pub mod rendered_lines;
pub mod renderer;
pub mod screen;
pub mod stdin_buffer;
pub mod terminal;
pub mod terminal_colors;
pub mod terminal_image;
pub mod undo_stack;
pub mod utf16;
pub mod utils;
pub mod viewport_mouse;
pub mod wheel_scroll; // upstream src/wheel-scroll.ts
pub mod word_navigation;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod delta_oracle_tests;

pub mod component_gesture;
pub mod component_overlay;

pub mod component_focus;

pub mod component_selection;

pub mod component_selection_paint;

pub mod component_clipboard;
pub mod component_image;
pub mod component_screen_widgets;

pub mod alt_screen_search_index;

pub mod alt_screen_search_component;

#[allow(clippy::module_inception)]
pub mod tui; // upstream src/tui.ts

pub mod main_screen; // upstream src/tui-main-screen.ts
