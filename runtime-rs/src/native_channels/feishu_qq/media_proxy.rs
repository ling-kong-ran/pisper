//! Axios 1.19.0 + proxy-from-env 2.1.0 routing used by SDK 1.72.0.
use crate::native_channels::{ChannelError, Result};
use std::collections::HashMap;
pub(super) type Environment = HashMap<String, String>;
pub(super) fn current() -> Environment {
    [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "no_proxy",
        "NO_PROXY",
        "NODE_EXTRA_CA_CERTS",
    ]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
    .collect()
}
fn env<'a>(values: &'a Environment, key: &str) -> &'a str {
    values
        .get(key)
        .filter(|value| !value.is_empty())
        .or_else(|| values.get(&key.to_ascii_uppercase()))
        .map(String::as_str)
        .unwrap_or("")
}
fn whitespace(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}'
}
fn raw_no_proxy(host: &str, port: u16, entry: &str) -> bool {
    let (entry_host, entry_port) = entry
        .rsplit_once(':')
        .filter(|(host, port)| {
            !host.is_empty() && !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
        })
        .map(|(host, port)| (host, port.parse::<u32>().unwrap_or(u32::MAX)))
        .unwrap_or((entry, 0));
    if entry_port != 0 && entry_port != u32::from(port) {
        return false;
    }
    if entry_host.starts_with(['.', '*']) {
        host.ends_with(entry_host.strip_prefix('*').unwrap_or(entry_host))
    } else {
        host == entry_host
    }
}
fn normalized(host: &str) -> String {
    let host = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if let Ok(std::net::IpAddr::V6(ip)) = host.parse() {
        if let Some(ip) = ip.to_ipv4_mapped() {
            return ip.to_string();
        }
    }
    // Axios normalizes 2..4-part IPv4 shorthand/hex/octal, not single parts.
    if (2..=4).contains(&host.split('.').count()) && !host.contains(':') {
        if let Ok(url) = reqwest::Url::parse(&format!("http://{host}/")) {
            if let Some(ip) = url
                .host_str()
                .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            {
                return ip.to_string();
            }
        }
    }
    host
}
fn loopback(host: &str) -> bool {
    if host == "localhost" || host == "0.0.0.0" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}
fn normalized_no_proxy(host: &str, port: u16, entry: &str) -> bool {
    if entry == "*" {
        return true;
    }
    let (entry_host, entry_port) = if entry.starts_with('[') {
        let Some(end) = entry.find(']') else {
            return false;
        };
        let suffix = &entry[end + 1..];
        (
            &entry[1..end],
            suffix
                .strip_prefix(':')
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0),
        )
    } else if entry.matches(':').count() == 1 {
        entry
            .rsplit_once(':')
            .filter(|(_, port)| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|(host, port)| (host, port.parse::<u32>().unwrap_or(u32::MAX)))
            .unwrap_or((entry, 0))
    } else {
        (entry, 0)
    };
    if entry_port != 0 && entry_port != u32::from(port) {
        return false;
    }
    let entry_host = normalized(entry_host);
    let entry_host = entry_host.strip_prefix('*').unwrap_or(&entry_host);
    if entry_host.is_empty() {
        return false;
    }
    if entry_host.starts_with('.') {
        host.ends_with(entry_host)
    } else {
        host == entry_host || (loopback(host) && loopback(entry_host))
    }
}
pub(super) fn select(url: &reqwest::Url, values: &Environment) -> Result<Option<reqwest::Url>> {
    let host = url.host_str().unwrap_or("");
    let port = url.port_or_known_default().unwrap_or(0);
    let no_proxy = env(values, "no_proxy").to_lowercase();
    let normalized_host = normalized(host);
    if no_proxy
        .split(|character| character == ',' || whitespace(character))
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            raw_no_proxy(host, port, entry) || normalized_no_proxy(&normalized_host, port, entry)
        })
    {
        return Ok(None);
    }
    let key = format!("{}_proxy", url.scheme());
    let proxy = env(values, &key);
    let proxy = if proxy.is_empty() {
        env(values, "all_proxy")
    } else {
        proxy
    };
    if proxy.is_empty() {
        return Ok(None);
    }
    let proxy = if proxy.contains("://") {
        proxy.into()
    } else {
        format!("{}://{proxy}", url.scheme())
    };
    reqwest::Url::parse(&proxy)
        .map(Some)
        .map_err(|_| ChannelError::new("fetch source URL failed"))
}
