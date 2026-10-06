//! Port of `modes/interactive/components/pi-logo.ts` (26 lines): the pi logo.
//!
//! 4 cells wide and 2 lines tall. Each cell shows two square pixels with half
//! blocks:
//!
//! ```text
//!   coral coral coral .
//!   blue  .     coral .
//!   blue  blue  .     yellow
//!   blue  .     .     yellow
//! ```
//!
//! The brand colors stay fixed across themes; they follow the terminal's
//! color mode. The theme is threaded as a parameter (the port's components
//! read the active [`Theme`] explicitly instead of a module singleton).

use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::colors::{background_ansi, foreground_ansi, rgb_color};

const RESET: &str = "\x1b[0m";

/// Upstream `piLogoLines()` — the `(top, bottom)` half-block pair.
pub fn pi_logo_lines(theme: &Theme) -> (String, String) {
    let mode = theme.get_color_mode();
    // `rgbColor` never fails for in-gamma components; the error arm is
    // unreachable but must not panic, so it degrades to a no-op escape.
    let fg = |r: f64, g: f64, b: f64| -> String {
        match rgb_color(r, g, b) {
            Ok(color) => foreground_ansi(color, mode),
            Err(_) => String::new(),
        }
    };
    let bg = |r: f64, g: f64, b: f64| -> String {
        match rgb_color(r, g, b) {
            Ok(color) => background_ansi(color, mode),
            Err(_) => String::new(),
        }
    };
    let coral = fg(228.0, 138.0, 122.0);
    let blue_bg = bg(79.0, 142.0, 179.0);
    let blue = fg(79.0, 142.0, 179.0);
    let yellow = fg(234.0, 182.0, 93.0);
    // The fourth cell of the top line is empty, so it is padded to the same
    // width as the bottom line.
    let top = format!("{coral}{blue_bg}▀{RESET}{coral}▀█{RESET} ");
    let bottom = format!("{blue}█▀{RESET} {yellow}█{RESET}");
    (top, bottom)
}

/// Whether the terminal renders the half-block logo correctly. Apple Terminal
/// draws gaps between rows and misaligns the half blocks, so it gets the text
/// wordmark instead (v1.0.0 `supportsPiLogo`).
pub fn supports_pi_logo() -> bool {
    !crate::tui::terminal::is_apple_terminal_session()
}

/// Text fallback for the logo: "Pi" with the logo's coral and yellow
/// (v1.0.0 `piWordmark`).
pub fn pi_wordmark(theme: &Theme) -> String {
    let mode = theme.get_color_mode();
    let fg = |r: f64, g: f64, b: f64| -> String {
        match rgb_color(r, g, b) {
            Ok(color) => foreground_ansi(color, mode),
            Err(_) => String::new(),
        }
    };
    format!(
        "{}P{RESET}{}i{RESET}",
        fg(228.0, 138.0, 122.0),
        fg(234.0, 182.0, 93.0)
    )
}
