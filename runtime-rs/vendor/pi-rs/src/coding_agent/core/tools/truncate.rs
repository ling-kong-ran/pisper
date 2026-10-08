//! Same truncateHead/truncateTail algorithms as harness/utils/truncate.ts.
//! The coding-agent result always includes truncatedBy (null when untruncated).
pub use crate::agent_core::harness::utils::truncate::{
    format_size, truncate_head, truncate_tail, TruncationOptions, TruncationResult,
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
pub fn as_value(result: &TruncationResult) -> serde_json::Value {
    let mut value = serde_json::to_value(result).expect("truncation JSON");
    if result.truncated_by.is_none() {
        value["truncatedBy"] = serde_json::Value::Null;
    }
    value
}
