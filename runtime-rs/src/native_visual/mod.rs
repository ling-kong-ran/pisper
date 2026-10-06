//! Complete release visual generation. Configuration/session/archive remain
//! narrow host ports; this module never reads a personal profile on its own.
mod catalog;
mod drivers;
mod input;
mod output;
mod service;
mod tools;
mod transport;
mod types;

pub use service::VisualGenerationService;
pub use tools::{create_tool, manifest};
use types::{DriverResult, PreparedRequest, SourceImage, VisualModel};
pub use types::{
    Result, VisualConfigPort, VisualConfigRead, VisualConfigSnapshot, VisualContextPort,
    VisualError, VisualGeneratedFilePort, VisualKind, VisualOperation, VisualOptions,
    VisualPreferenceWrite, VisualProgress, VisualRequest, VisualResult, VisualToolContext,
};

#[cfg(test)]
mod tests;
