pub(crate) mod cdp;
mod coercion;
mod service;
mod tools;
pub(crate) mod types;

pub use service::BrowserAutomationService;
pub use tools::create_tool;

#[cfg(test)]
mod tests;
