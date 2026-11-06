// 右侧浏览器的输入校验和导航判定，不碰 WebView，所有平台都能测。
use serde::Deserialize;
use tauri::Url;

const MAX_BROWSER_ID_BYTES: usize = 100;
const MAX_BROWSER_URL_BYTES: usize = 8192;
// 与 runtime/services/html-preview-service.mjs 的 HTML_PREVIEW_PREFIX 一致。
const HTML_PREVIEW_PREFIX: &str = "/_pisper/html-preview/";
const MAX_COORDINATE: f64 = 100_000.0;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NavigationDecision {
    Allow,
    OpenExternally,
    Deny,
}

pub fn validate_browser_id(browser_id: &str) -> Result<(), String> {
    let valid = !browser_id.is_empty()
        && browser_id.len() <= MAX_BROWSER_ID_BYTES
        && browser_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err("Invalid browser identifier.".into())
    }
}

pub fn parse_browser_url(value: &str) -> Result<Url, String> {
    if value.len() > MAX_BROWSER_URL_BYTES {
        return Err("Browser address is too long.".into());
    }
    let url = Url::parse(value).map_err(|_| "Invalid browser address.".to_string())?;
    if matches!(url.scheme(), "http" | "https") {
        Ok(url)
    } else {
        Err("The browser only opens http and https addresses.".into())
    }
}

pub fn validate_bounds(bounds: &BrowserBounds) -> Result<(), String> {
    let finite = [bounds.x, bounds.y, bounds.width, bounds.height]
        .iter()
        .all(|value| value.is_finite() && value.abs() <= MAX_COORDINATE);
    if finite && bounds.width >= 0.0 && bounds.height >= 0.0 {
        Ok(())
    } else {
        Err("Invalid browser bounds.".into())
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

// macOS 上这个判定也会收到页面子框架的导航，所以 about/blob/data 要放行，否则内嵌内容会空白。
// 应用自己的地址只放行 HTML 预览：其他路径是应用界面和 API，不该出现在网页浏览器里。
pub fn navigation_decision(target: &Url, app_origin: &Url) -> NavigationDecision {
    match target.scheme() {
        "http" | "https" => {
            if same_origin(target, app_origin) && !target.path().starts_with(HTML_PREVIEW_PREFIX) {
                NavigationDecision::Deny
            } else {
                NavigationDecision::Allow
            }
        }
        "about" | "blob" | "data" => NavigationDecision::Allow,
        "mailto" | "tel" => NavigationDecision::OpenExternally,
        _ => NavigationDecision::Deny,
    }
}

// 空白页是内部状态，不算用户到过的地址，不回写地址栏。
pub fn reportable_url(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(target: &str) -> NavigationDecision {
        navigation_decision(
            &Url::parse(target).unwrap(),
            &Url::parse("http://127.0.0.1:4100/").unwrap(),
        )
    }

    #[test]
    fn websites_stay_in_the_browser_and_the_app_itself_does_not() {
        assert_eq!(decide("https://github.com/"), NavigationDecision::Allow);
        assert_eq!(decide("http://localhost:5173/"), NavigationDecision::Allow);
        assert_eq!(decide("http://127.0.0.1:5173/"), NavigationDecision::Allow);
        assert_eq!(decide("http://127.0.0.1:4100/"), NavigationDecision::Deny);
        assert_eq!(
            decide("http://127.0.0.1:4100/api/sessions"),
            NavigationDecision::Deny
        );
        assert_eq!(
            decide("http://127.0.0.1:4100/_pisper/html-preview/token/index.html"),
            NavigationDecision::Allow
        );
        // URL 解析已折叠 ..，不能借预览前缀走回应用路径。
        assert_eq!(
            decide("http://127.0.0.1:4100/_pisper/html-preview/../../api/sessions"),
            NavigationDecision::Deny
        );
    }

    #[test]
    fn page_internals_load_and_other_schemes_do_not() {
        assert_eq!(decide("about:blank"), NavigationDecision::Allow);
        assert_eq!(
            decide("blob:https://example.com/1"),
            NavigationDecision::Allow
        );
        assert_eq!(decide("data:text/html,x"), NavigationDecision::Allow);
        assert_eq!(
            decide("mailto:a@example.com"),
            NavigationDecision::OpenExternally
        );
        assert_eq!(decide("tel:10086"), NavigationDecision::OpenExternally);
        assert_eq!(decide("file:///etc/passwd"), NavigationDecision::Deny);
        assert_eq!(decide("javascript:alert(1)"), NavigationDecision::Deny);
        assert_eq!(decide("tauri://localhost/"), NavigationDecision::Deny);
        assert_eq!(decide("weixin://dl/"), NavigationDecision::Deny);
    }

    #[test]
    fn untrusted_input_is_checked_before_it_reaches_the_webview() {
        assert!(parse_browser_url("https://example.com/").is_ok());
        assert!(parse_browser_url("javascript:alert(1)").is_err());
        assert!(parse_browser_url("file:///etc/passwd").is_err());
        assert!(parse_browser_url("not a url").is_err());
        assert!(parse_browser_url(&format!("https://e.com/{}", "a".repeat(9000))).is_err());

        assert!(validate_browser_id("native-browser-1").is_ok());
        assert!(validate_browser_id("").is_err());
        assert!(validate_browser_id("../x").is_err());
        assert!(validate_browser_id(&"a".repeat(101)).is_err());

        let bounds = BrowserBounds {
            x: 10.0,
            y: 20.0,
            width: 300.0,
            height: 200.0,
        };
        assert!(validate_bounds(&bounds).is_ok());
        assert!(validate_bounds(&BrowserBounds {
            width: 0.0,
            height: 0.0,
            ..bounds
        })
        .is_ok());
        assert!(validate_bounds(&BrowserBounds {
            x: f64::NAN,
            ..bounds
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            width: -1.0,
            ..bounds
        })
        .is_err());
        assert!(validate_bounds(&BrowserBounds {
            height: f64::INFINITY,
            ..bounds
        })
        .is_err());
    }

    #[test]
    fn only_web_addresses_reach_the_address_bar() {
        assert_eq!(
            reportable_url("https://example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(reportable_url("about:blank"), None);
        assert_eq!(reportable_url(""), None);
    }
}
