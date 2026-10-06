//! release 的 Bing RSS 搜索：固定公开端点，配置来自规范 pisper.json。
mod config;
mod json;
mod rss;
mod service;
#[cfg(test)]
mod tests;
mod tools;

pub use config::{normalize_config, normalize_config_checked, WebSearchConfig};
pub use rss::{parse_bing_rss_results, SearchItem, SearchResult};
pub use service::{SearchError, WebSearchService};
pub use tools::{create_tool, manifest};

pub(crate) fn parse_config_json(input: &str) -> Result<serde_json::Value, serde_json::Error> {
    json::parse(input)
}
