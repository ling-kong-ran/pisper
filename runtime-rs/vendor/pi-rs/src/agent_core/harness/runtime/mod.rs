//! The independent AgentHarness runtime (not pico3).
//!
//! This first slice implements durable operation vocabulary and coherent lane
//! restoration. It deliberately does not export a fake create_harness: lane
//! admission, the drive dispatcher and the public AgentHarness adapter remain
//! migration work. See docs/migration/RUNTIME_COMPATIBILITY.md.
pub mod durable;
pub mod events;
pub mod lane;
pub mod progress;
pub mod projection;
pub mod reducer;
pub mod restore;
pub mod transcript;

#[cfg(test)]
mod tests;

pub mod drive;

pub mod drive_pass;
