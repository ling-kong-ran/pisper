//! MCP wire protocol surface, ported from upstream
//! `packages/mcp/src/protocol/`: the JSON-RPC 2.0 message model and error
//! types ([`jsonrpc`]), tool-result content shapes with the
//! `toLlmContent` converter ([`content`]), and the protocol constants and
//! structural types ([`types`]).

pub mod content;
pub mod jsonrpc;
pub mod types;
