//! Port of upstream `packages/tui/src/components/*`: widget components.

pub mod editor;
pub mod input;
pub mod layout_widgets;
pub mod markdown;
pub mod scroll_view;
pub mod select_list;
pub mod settings_list;
pub mod text;
pub mod truncate_primary;

pub mod stack;
pub use stack::{HStack, VStack};

pub mod container;
pub mod mouse_region;

pub mod alt_screen_flash;
