use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

fn external_url(value: &str) -> Result<tauri::Url, String> {
    if value.len() > 2048 {
        return Err("外部链接过长。".into());
    }
    let url = tauri::Url::parse(value).map_err(|_| "外部链接无效。".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("只允许打开不含凭据的 HTTP 或 HTTPS 链接。".into());
    }
    Ok(url)
}

#[tauri::command]
pub fn mobile_open_external_url(app: AppHandle, url: String) -> Result<bool, String> {
    let url = external_url(&url)?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map(|_| true)
        .map_err(|error| format!("无法在浏览器中打开链接：{error}"))
}

#[cfg(test)]
mod tests {
    use super::external_url;

    #[test]
    fn external_links_accept_web_urls_without_credentials() {
        assert!(external_url("https://github.com/ling-kong-ran/pisper").is_ok());
        assert!(external_url("http://example.com/path").is_ok());
        let too_long = "https://example.com/".repeat(200);
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://user:secret@example.com/",
            too_long.as_str(),
        ] {
            assert!(external_url(url).is_err(), "accepted {url}");
        }
    }
}
