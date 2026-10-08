//! Port of upstream `packages/tui/src/wheel-scroll.ts`: mouse-wheel
//! acceleration for `"auto"` scroll-line settings.

/// Lines moved per mouse-wheel event, or `Auto` to accelerate fast wheel
/// spins. `Fixed(NaN)` / `Fixed(infinity)` mirror upstream non-finite numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelScrollLines {
    Auto,
    Fixed(f64),
}

// Several events closer than this belong to one physical notch (Ghostty emits
// them ~4 ms apart) or come from a high-resolution source. They move one line
// each and do not accelerate.
const BURST_GAP_MS: f64 = 5.0;
// A pause longer than this ends a scroll gesture.
const GESTURE_GAP_MS: f64 = 200.0;
// Average event gap that maps to one line per event. Faster events scale up
// proportionally.
const REFERENCE_GAP_MS: f64 = 100.0;
const MAX_AUTO_LINES: f64 = 6.0;

/// `WheelScrollLines` from a JS `number | "auto"` option value.
pub fn wheel_scroll_lines_from_option(lines: Option<f64>) -> WheelScrollLines {
    // Upstream `options.wheelScrollLines ?? 1`: only null/undefined defaults.
    match lines {
        None => WheelScrollLines::Fixed(1.0),
        Some(value) => WheelScrollLines::Fixed(value),
    }
}

/**
 * Local macOS terminals receive wheel and trackpad deltas that the OS has
 * already accelerated, and they emit one event per line. Other platforms, and
 * SSH sessions where the client platform is unknown, usually send one event
 * per wheel notch.
 */
pub fn terminal_accelerates_wheel() -> bool {
    // process.platform === "darwin" && SSH_* are all undefined.
    let ssh_unset = |name: &str| std::env::var_os(name).is_none();
    cfg!(target_os = "macos")
        && ssh_unset("SSH_CONNECTION")
        && ssh_unset("SSH_CLIENT")
        && ssh_unset("SSH_TTY")
}

/// Converts wheel events into line counts.
///
/// In `Auto` mode on terminals that do not accelerate wheel input, the count
/// follows event velocity: an isolated notch moves one line, while a fast spin
/// moves up to six lines per event. For example, notches 100 ms apart move 1
/// line each, 50 ms apart move 2, and 20 ms apart move 5.
#[derive(Clone, Debug)]
pub struct WheelScrollAccelerator {
    lines: WheelScrollLines,
    accelerate: bool,
    last_time: f64,
    last_direction: i64,
    average_gap: Option<f64>,
    carry: f64,
}

impl Default for WheelScrollAccelerator {
    fn default() -> Self {
        Self::new(WheelScrollLines::Auto, None)
    }
}

impl WheelScrollAccelerator {
    /// Upstream constructor defaults: `lines = "auto"`,
    /// `accelerate = !terminalAcceleratesWheel()`.
    pub fn new(lines: WheelScrollLines, accelerate: Option<bool>) -> Self {
        Self {
            lines,
            accelerate: accelerate.unwrap_or(!terminal_accelerates_wheel()),
            last_time: f64::NEG_INFINITY,
            last_direction: 0,
            average_gap: None,
            carry: 0.0,
        }
    }

    pub fn set_lines(&mut self, lines: WheelScrollLines) {
        self.lines = lines;
        self.reset();
    }

    /// Return the positive line count for a wheel event in `direction` at time
    /// `now` (milliseconds). Direction: -1 or 1.
    pub fn next(&mut self, direction: i64, now: f64) -> f64 {
        match self.lines {
            WheelScrollLines::Fixed(lines) => {
                if lines.is_finite() {
                    (1.0f64).max(lines.floor())
                } else {
                    1.0
                }
            }
            WheelScrollLines::Auto => {
                if !self.accelerate {
                    return 1.0;
                }

                let gap = now - self.last_time;
                let same_gesture = direction == self.last_direction && gap <= GESTURE_GAP_MS;
                self.last_time = now;
                self.last_direction = direction;
                if !same_gesture {
                    self.average_gap = None;
                    self.carry = 0.0;
                    return 1.0;
                }
                if gap < BURST_GAP_MS {
                    return 1.0;
                }

                self.average_gap = Some(match self.average_gap {
                    None => gap,
                    Some(average) => (average + gap) / 2.0,
                });
                let lines = MAX_AUTO_LINES
                    .min(1.0f64.max(REFERENCE_GAP_MS / self.average_gap.expect("set above")))
                    + self.carry;
                let whole = lines.floor();
                self.carry = lines - whole;
                whole
            }
        }
    }

    fn reset(&mut self) {
        self.last_time = f64::NEG_INFINITY;
        self.last_direction = 0;
        self.average_gap = None;
        self.carry = 0.0;
    }
}
