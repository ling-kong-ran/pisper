//! 本机 Runtime（M1）：移动端壳内嵌的受限 Runtime，提供 OpenAI 兼容
//! Provider 的本机流式对话。边界与上限见 docs/mobile-local-runtime.md。
//!
//! 子模块：store（持久化与资源上限）、provider（Provider 客户端与 SSE 解析）、
//! server（回环 HTTP/SSE 服务与内置对话页）。

pub mod provider;
pub mod server;
pub mod store;

pub use server::{start_runtime, LocalRuntime};
