//! 原生静态组件目录、受限 ZIP 和短期资源凭证；组件代码只在浏览器沙箱执行。
mod archive;
mod assets;
mod attribute;
mod model;
mod service;
#[cfg(test)]
mod tests;

pub use assets::AssetResponse;
pub use model::{Component, CustomUiError, Manifest, Result, View, ViewGrant};
pub use service::CustomUiService;

pub(crate) const MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub(crate) const VIEW_TTL_MS: i64 = 5 * 60_000;
pub(crate) const MAX_VIEWS: usize = 128;
