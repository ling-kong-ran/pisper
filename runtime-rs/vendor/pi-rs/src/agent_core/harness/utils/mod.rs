//! Port of the `harness/utils/` helper subtree needed by the execution
//! environment (M3b Task 6): [`truncate`], [`adaptive_publisher`],
//! [`output_capture`], and [`shell_output`]. Each module cites its upstream
//! source file.
//!
//! Partial port: upstream also carries `usage.ts` (35 lines) and the
//! grep-specific truncate helpers; they land with the modules that consume
//! them (M3b Task 11), which will extend this module.

pub mod adaptive_publisher;
pub mod output_capture;
pub mod shell_output;
pub mod truncate;

pub use adaptive_publisher::AdaptivePublisher;
pub use output_capture::{
    apply_shell_output_update, sanitize_shell_output, OutputCapture, OutputCaptureHandlers,
    OUTPUT_MIN_EMIT_INTERVAL_MS, OUTPUT_TARGET_BYTES_PER_SECOND,
};
pub use shell_output::{
    execute_shell_with_capture, sanitize_binary_output, ShellCaptureOptions, ShellCaptureProgress,
    ShellCaptureResult,
};
pub use truncate::{
    truncate_head, truncate_tail, utf8_byte_length, TruncationOptions, TruncationResult,
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
