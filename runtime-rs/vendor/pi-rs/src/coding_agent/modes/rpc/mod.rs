//! RPC JSONL transport, typed commands, live runtime dispatch and async extension UI.
//! The native process loop is available through `mode`; full CLI routing is separate.
pub mod dispatch;
pub mod jsonl;
pub mod mode;
pub mod types;
pub mod ui;
