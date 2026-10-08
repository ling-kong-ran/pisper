//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/countdown-timer.ts` (39 lines, sha256
//! `6ade4aa41c2c2400bdb79d47701c95504be48e3ab9f2b1aee3836a888c2d1da9`).
//!
//! The `setInterval`/`clearInterval` choreography is a host seam (the ported
//! shell has no timer loop yet): the port exposes an explicit [`CountdownTimer::tick`]
//! driving one second at a time, keeping the remaining-seconds arithmetic,
//! expire edge and dispose semantics faithful.

use std::cell::Cell;
use std::rc::Rc;

/// Callbacks (upstream `onTick` / `onExpire`).
pub struct CountdownCallbacks {
    pub on_tick: Box<dyn Fn(u64)>,
    pub on_expire: Box<dyn Fn()>,
}

/// Reusable countdown timer for dialog components (upstream `CountdownTimer`).
pub struct CountdownTimer {
    active: Rc<Cell<bool>>,
    remaining_seconds: u64,
    callbacks: CountdownCallbacks,
}

impl CountdownTimer {
    /// Upstream constructor: `remainingSeconds = Math.ceil(timeoutMs / 1000)`
    /// and an immediate `onTick` call. The upstream timer only starts ticking
    /// one second later; the port keeps that via [`Self::tick`].
    pub fn new(timeout_ms: u64, callbacks: CountdownCallbacks) -> Self {
        let remaining_seconds = timeout_ms.div_ceil(1000);
        (callbacks.on_tick)(remaining_seconds);
        Self {
            active: Rc::new(Cell::new(true)),
            remaining_seconds,
            callbacks,
        }
    }

    /// One upstream interval tick (`remainingSeconds--`, `onTick`, render
    /// request, expire check). Returns the remaining seconds after the tick.
    pub fn tick(&mut self) -> u64 {
        if !self.active.get() {
            return self.remaining_seconds;
        }
        self.remaining_seconds = self.remaining_seconds.saturating_sub(1);
        (self.callbacks.on_tick)(self.remaining_seconds);
        if self.remaining_seconds == 0 {
            self.dispose();
            (self.callbacks.on_expire)();
        }
        self.remaining_seconds
    }

    /// Upstream `dispose` (clearInterval). Subsequent ticks are no-ops.
    pub fn dispose(&mut self) {
        self.active.set(false);
    }

    pub fn remaining_seconds(&self) -> u64 {
        self.remaining_seconds
    }

    /// Shared activity flag so owner components can drop their countdown
    /// reference when it expires (upstream `this.countdown = undefined`).
    pub fn is_active(&self) -> bool {
        self.active.get()
    }

    pub fn active_handle(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `countdown_timer`
    /// (3500ms → ticks [4,3,2,1,0], one expiry, cleared interval afterwards).
    #[test]
    fn countdown_matches_oracle() {
        let ticks: Rc<std::cell::RefCell<Vec<u64>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
        let expired = Rc::new(Cell::new(0u32));
        let ticks_handle = Rc::clone(&ticks);
        let expired_handle = Rc::clone(&expired);
        let mut timer = CountdownTimer::new(
            3500,
            CountdownCallbacks {
                on_tick: Box::new(move |s| ticks_handle.borrow_mut().push(s)),
                on_expire: Box::new(move || expired_handle.set(expired_handle.get() + 1)),
            },
        );
        // constructor fires the initial tick immediately (ceil(3500/1000) = 4)
        assert_eq!(ticks.borrow().as_slice(), &[4]);

        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(timer.tick());
        }
        assert_eq!(ticks.borrow().as_slice(), &[4, 3, 2, 1, 0]);
        assert_eq!(seen, vec![3, 2, 1, 0]);
        assert_eq!(expired.get(), 1);
        assert!(!timer.is_active());

        timer.dispose();
        timer.tick();
        assert_eq!(
            ticks.borrow().as_slice(),
            &[4, 3, 2, 1, 0],
            "no ticks after dispose"
        );
        assert_eq!(expired.get(), 1);
    }
}
