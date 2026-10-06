//! Port of the upstream `coding-agent` package
//! (`pi/packages/coding-agent`, slice W3.1: the `src/utils` leaf modules).
//!
//! Each upstream `.ts` file maps to one Rust submodule with the matching
//! snake_case name; see `utils::` module docs for the per-file provenance,
//! seams and divergences.

pub mod agent_session;
pub mod core;
pub mod extensions;
pub mod package_manager;
pub mod session_manager;
pub mod utils;

#[cfg(test)]
pub mod oracle_scrub;

pub mod modes;

pub mod cli;

pub mod experimental;

pub mod main;

pub mod migrations;
