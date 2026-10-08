pub mod agent_core;
pub mod ai;
// M6 chord slice: port of `pi/packages/chord`.
pub mod chord;
// Codemode slice: port of `pi/packages/codemode` (the sandboxed JS runtime
// whose only capability is calling injected tools), on the embedded rquickjs
// (quickjs-ng) engine.
pub mod cli;
pub mod codemode;
// M6 client slice: port of `pi/packages/client`.
pub mod client;
pub mod coding_agent;
pub mod config;
// Durable-execution slice: port of `pi/packages/durable`
// (`@earendil-works/pi-durable`).
pub mod durable;
// M6 evals slice: port of `pi/packages/evals`.
pub mod evals;
// MCP client slice: port of `pi/packages/mcp`.
pub mod mcp;
// M6 first slice: port of `pi/packages/protocol` (server/client/evals leaf).
pub mod protocol;
// M6 server slice: port of `pi/packages/server`.
pub mod server;
pub mod tui;

pub(crate) mod serde_support;
