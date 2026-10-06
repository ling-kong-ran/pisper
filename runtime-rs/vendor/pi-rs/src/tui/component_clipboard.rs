//! Alternate-screen clipboard delivery (`tui-alt-screen.ts:301–305,1445–1468`).
//!
//! Initiation is eager, like a JavaScript async call before its first await.
//! Completion is an owned, non-Send Future: the single-threaded host must poll
//! it (including fire-and-forget selection releases). Dropping an uncompleted
//! Rust future cancels its continuation, unlike dropping a JS Promise; hosts
//! must retain pending operations. No selection or mutable host borrow crosses
//! an await. This is not an OS clipboard adapter or the complete TUI event loop.
use crate::tui::component_selection::{ComponentSelection, ComponentSelectionHost};
use std::future::{ready, Future};
use std::pin::Pin;
use std::rc::Rc;

pub const COPY_ERROR_FLASH_DURATION_MS: f64 = 5000.0;

/// Only `Boolean(true)` is success. An empty message is still a message, not
/// "Copy failed". Other models runtime JS values outside boolean/string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardResult {
    Boolean(bool),
    Message(String),
    Other,
}
pub type ClipboardDelivery<E> = Pin<Box<dyn Future<Output = Result<ClipboardResult, E>>>>;
pub type ClipboardCompletion<E> = Pin<Box<dyn Future<Output = Result<bool, E>>>>;

/// All services are required; none silently default to successful/no-op IO.
/// Implementations may use short interior borrows, but must release them before
/// returning a delivery future. Route flash requests to AltScreenFlashContainer
/// and arrange its single-threaded timers. Native delivery success is the
/// injected service's responsibility; OSC52 success only means the writes ran.
pub trait ComponentClipboardHost: 'static {
    type Error: 'static;
    fn has_copy_selection(&self) -> bool;
    /// Synchronous failure corresponds to throwing before returning a Promise;
    /// an error from the returned future corresponds to Promise rejection.
    fn copy_selection(&self, text: String) -> Result<ClipboardDelivery<Self::Error>, Self::Error>;
    fn write_clipboard_sequence(&self, sequence: &str) -> Result<(), Self::Error>;
    fn flash_clipboard(&self, message: &str, duration_ms: Option<f64>) -> Result<(), Self::Error>;
}

/// Exact standard base64 with padding, over valid UTF-8 bytes, followed by BEL.
/// Kept local to the terminal protocol instead of coupling TUI to an AI backend.
pub fn osc52_sequence(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("\x1b]52;c;");
    for chunk in text.as_bytes().chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[usize::from(a >> 2)] as char);
        out.push(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[usize::from(c & 63)] as char
        } else {
            '='
        });
    }
    out.push('\x07');
    out
}

/// Start copying immediately, returning the eventual success or host error.
/// Injected delivery is awaited before any flash, with no OSC52 fallback on
/// failure. Rejection and write/flash failures propagate; they are not false.
/// This low-level entry accepts empty text, just like copyTextToClipboard.
pub fn copy_text_to_clipboard<H: ComponentClipboardHost>(
    host: Rc<H>,
    text: String,
) -> ClipboardCompletion<H::Error> {
    if host.has_copy_selection() {
        match host.copy_selection(text) {
            Ok(delivery) => Box::pin(async move {
                let result = delivery.await?;
                let ok = result == ClipboardResult::Boolean(true);
                let message = match &result {
                    ClipboardResult::Boolean(true) => "Copied!",
                    ClipboardResult::Message(message) => message.as_str(),
                    _ => "Copy failed",
                };
                host.flash_clipboard(message, (!ok).then_some(COPY_ERROR_FLASH_DURATION_MS))?;
                Ok(ok)
            }),
            Err(error) => Box::pin(ready(Err(error))),
        }
    } else {
        let result = host
            .write_clipboard_sequence(&osc52_sequence(&text))
            .and_then(|()| host.flash_clipboard("Copied!", None))
            .map(|()| true);
        Box::pin(ready(result))
    }
}

impl ComponentSelection {
    /// Read selection text NOW, not at the first future poll or resolution.
    /// Both upstream active-selection entries skip an absent/empty selection.
    /// Later selection/frame changes cannot change the text already submitted.
    pub fn copy_active_selection_to_clipboard<H: ComponentClipboardHost>(
        &self,
        source: &impl ComponentSelectionHost,
        clipboard: Rc<H>,
    ) -> ClipboardCompletion<H::Error> {
        match self.active_text(source).filter(|text| !text.is_empty()) {
            Some(text) => copy_text_to_clipboard(clipboard, text),
            None => Box::pin(ready(Ok(false))),
        }
    }
}
