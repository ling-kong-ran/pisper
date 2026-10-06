//! Upstream `SelectListTruncatePrimaryContext` and the custom primary-cell
//! truncation callback type (`SelectListLayoutOptions.truncatePrimary`).

use super::select_list::SelectItem;

/// Context passed to the custom primary truncation callback.
pub struct TruncatePrimaryContext<'a> {
    pub text: &'a str,
    pub max_width: usize,
    pub column_width: usize,
    pub item: &'a SelectItem,
    pub is_selected: bool,
}

/// Custom primary truncation callback.
pub type TruncatePrimaryFn = Box<dyn Fn(&TruncatePrimaryContext<'_>) -> String + Send + Sync>;
