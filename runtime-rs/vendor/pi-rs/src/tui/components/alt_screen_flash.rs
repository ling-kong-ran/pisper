//! Actual `components/alt-screen-flash.ts`: transient message stack.
//!
//! Timers are single-threaded host services, not worker threads. Queue expiry
//! with the owning FlashId, never call back synchronously while scheduling.
//! Call dispose before discarding a live controller to cancel host timers.
//! The full screen compositor is responsible for placing these rendered lines.
use crate::tui::component::Component;
use crate::tui::utils::truncate_to_width;
use std::rc::Rc;

pub const DEFAULT_DURATION_MS: f64 = 1000.0;

/// A timer closure in JS captures its container, not just a numeric id. Owning
/// identity prevents a stale/cross-container tick from deleting another stack.
#[derive(Clone, Debug)]
pub struct FlashId {
    owner: Rc<()>,
    sequence: u64,
}
impl FlashId {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
impl PartialEq for FlashId {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.owner, &other.owner) && self.sequence == other.sequence
    }
}
impl Eq for FlashId {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashEntry {
    pub id: FlashId,
    pub message: String,
    pub timer: u64,
}

/// Schedule and unref are distinct observable operations, in that order.
/// The host supplies the timer policy for JS fractional/NaN/infinite durations;
/// this layer implements Math.max(0, duration), not Node timer coercion.
pub trait AltScreenFlashHost {
    fn set_flash_timeout(&mut self, id: FlashId, duration_ms: f64) -> u64;
    fn unref_flash_timeout(&mut self, timer: u64);
    fn clear_flash_timeout(&mut self, timer: u64);
    fn request_flash_render(&mut self);
}

#[derive(Debug, Default)]
pub struct AltScreenFlashContainer {
    entries: Vec<FlashEntry>,
    next_id: u64,
    owner: Rc<()>,
}
impl AltScreenFlashContainer {
    pub fn entries(&self) -> &[FlashEntry] {
        &self.entries
    }
    pub fn next_id(&self) -> u64 {
        self.next_id
    }
    pub fn flash(
        &mut self,
        host: &mut impl AltScreenFlashHost,
        message: String,
        duration_ms: Option<f64>,
    ) {
        let id = FlashId {
            owner: self.owner.clone(),
            sequence: self.next_id,
        };
        self.next_id = self.next_id.checked_add(1).expect("flash id exhausted");
        let duration = duration_ms.unwrap_or(DEFAULT_DURATION_MS);
        // f64::max ignores NaN, whereas JS Math.max propagates it.
        let delay = if duration.is_nan() {
            f64::NAN
        } else {
            duration.max(0.0)
        };
        let timer = host.set_flash_timeout(id.clone(), delay);
        host.unref_flash_timeout(timer);
        self.entries.push(FlashEntry { id, message, timer });
        host.request_flash_render();
    }
    /// Delivery of a queued timeout. Missing, disposed or already-fired entries
    /// do not request another render. Expiry does not clearTimeout its own token.
    pub fn expire(&mut self, id: &FlashId, host: &mut impl AltScreenFlashHost) {
        if let Some(index) = self.entries.iter().position(|entry| entry.id == *id) {
            self.entries.remove(index);
            host.request_flash_render();
        }
    }
    pub fn dispose(&mut self, host: &mut impl AltScreenFlashHost) {
        for entry in &self.entries {
            host.clear_flash_timeout(entry.timer);
        }
        self.entries.clear();
        // Upstream does not reset nextId or request a render here.
    }
}
impl Component for AltScreenFlashContainer {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| {
                let message = truncate_to_width(&format!(" {} ", entry.message), width, "", false);
                format!("\x1b[7m{message}\x1b[27m")
            })
            .collect()
    }
    fn invalidate(&mut self) {}
}
