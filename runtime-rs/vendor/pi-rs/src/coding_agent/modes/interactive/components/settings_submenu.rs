//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/settings-submenu.ts` (239 lines,
//! sha256 `3fcce39ae104343e20a564fb06db52e7553759b0636a4252de5ae6adb03ad5ac`).
//!
//! `SelectSubmenu` / `SteppedSubmenu` landed with the settings-selector slice
//! ([`super::settings_selector`], including the upstream open/delegate/done
//! submenu flow and the recorded-outcome ownership seam); this module
//! documents the coverage and re-exports the types.

pub use super::settings_selector::{SelectSubmenu, SteppedSubmenu, SteppedSubmenuStep};

/// The upstream `SUBMENU_SELECT_LIST_LAYOUT` (12/32 primary-column widths) is
/// reproduced by the submenu slice's layout constants.
pub const SUBMENU_MIN_PRIMARY_COLUMN_WIDTH: usize = 12;
pub const SUBMENU_MAX_PRIMARY_COLUMN_WIDTH: usize = 32;
