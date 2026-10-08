use super::{CustomUiError, Result};
use lol_html::{element, rewrite_str, RewriteStrSettings};
use std::{collections::BTreeMap, path::Path};

// 原 release 资源逐字迁移；这是浏览器 postMessage 脚本，不是服务端 JS 执行路径。
pub const BRIDGE_SCRIPT: &str = include_str!("bridge.js");
pub(crate) const ISLAND_HTML: &str = include_str!("island.html");

#[derive(Debug)]
pub struct AssetResponse {
    pub body: Vec<u8>,
    pub content_type: &'static str,
    pub headers: BTreeMap<&'static str, String>,
}
pub(crate) fn mime(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
pub(crate) fn headers(resource_base: Option<&str>) -> BTreeMap<&'static str, String> {
    let source = resource_base.unwrap_or("'none'");
    BTreeMap::from([
        ("cache-control", "no-store".into()), ("x-content-type-options", "nosniff".into()),
        ("referrer-policy", "no-referrer".into()), ("access-control-allow-origin", "*".into()),
        ("content-security-policy", format!("sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline' {source}; style-src 'unsafe-inline' {source}; img-src data: blob: {source}; font-src data: {source}; connect-src {source}; base-uri 'none'; form-action 'none'; frame-ancestors 'self'")),
    ])
}
pub(crate) fn respond(
    path: &str,
    bytes: Vec<u8>,
    resource_base: Option<&str>,
) -> Result<AssetResponse> {
    let content_type = mime(path);
    let body = if let Some(base) = resource_base.filter(|_| content_type.starts_with("text/html")) {
        let html = String::from_utf8_lossy(&bytes);
        let rewritten = rewrite_str(
            &html,
            RewriteStrSettings {
                element_content_handlers: vec![element!("script[src]", |element| {
                    if element
                        .get_attribute("src")
                        .as_deref()
                        .is_some_and(super::attribute::is_bridge_src)
                    {
                        element.set_attribute("src", &format!("{base}bridge.js"))?;
                    }
                    Ok(())
                })],
                ..RewriteStrSettings::new()
            },
        )
        .map_err(|_| CustomUiError::new(500, "component_html_invalid", "组件 HTML 无法加载。"))?;
        rewritten.into_bytes()
    } else {
        bytes
    };
    Ok(AssetResponse {
        body,
        content_type,
        headers: headers(resource_base),
    })
}
