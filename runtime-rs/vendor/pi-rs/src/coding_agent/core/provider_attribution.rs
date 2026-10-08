//! Upstream provider-attribution.ts / telemetry.ts. No network or telemetry is
//! sent here. Header spelling, null deletion markers and later-source wins are
//! preserved. BTreeMap order is the native headers seam, not JS insertion order.
use super::settings_manager::SettingsManager;
use crate::ai::types::{Model, ProviderHeaders};

pub fn is_install_telemetry_enabled(settings: &SettingsManager) -> bool {
    telemetry_enabled(settings, std::env::var("PI_TELEMETRY").ok().as_deref())
}

pub(crate) fn telemetry_enabled(settings: &SettingsManager, env: Option<&str>) -> bool {
    env.map_or_else(
        || settings.get_enable_install_telemetry(),
        |value| {
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        },
    )
}

fn matches_host(base_url: &str, expected: &str) -> bool {
    url::Url::parse(base_url).is_ok_and(|url| url.host_str() == Some(expected))
}

pub fn merge_provider_attribution_headers(
    model: &Model,
    settings: &SettingsManager,
    session_id: Option<&str>,
    sources: &[Option<&ProviderHeaders>],
) -> Option<ProviderHeaders> {
    merge_headers(
        model,
        is_install_telemetry_enabled(settings),
        session_id,
        sources,
    )
}

pub(crate) fn merge_headers(
    model: &Model,
    telemetry: bool,
    session_id: Option<&str>,
    sources: &[Option<&ProviderHeaders>],
) -> Option<ProviderHeaders> {
    let mut headers = ProviderHeaders::new();
    if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
        if matches!(model.provider.as_str(), "opencode" | "opencode-go")
            || matches_host(&model.base_url, "opencode.ai")
        {
            headers.insert("x-opencode-session".into(), Some(session_id.into()));
            headers.insert("x-opencode-client".into(), Some("pi".into()));
        }
    }
    if telemetry {
        // Deliberately substring-based and case-sensitive, including paths and
        // malformed URLs. Do not replace this legacy OpenRouter rule with host matching.
        if model.provider == "openrouter" || model.base_url.contains("openrouter.ai") {
            headers.extend([
                ("HTTP-Referer".into(), Some("https://pi.dev".into())),
                ("X-OpenRouter-Title".into(), Some("pi".into())),
                ("X-OpenRouter-Categories".into(), Some("cli-agent".into())),
            ]);
        } else if model.provider == "nvidia"
            || matches_host(&model.base_url, "integrate.api.nvidia.com")
        {
            headers.insert("X-BILLING-INVOKE-ORIGIN".into(), Some("Pi".into()));
        } else if matches!(
            model.provider.as_str(),
            "cloudflare-workers-ai" | "cloudflare-ai-gateway"
        ) || matches_host(&model.base_url, "api.cloudflare.com")
            || matches_host(&model.base_url, "gateway.ai.cloudflare.com")
        {
            headers.insert("User-Agent".into(), Some("pi-coding-agent".into()));
        }
    }
    for source in sources.iter().flatten() {
        headers.extend((*source).clone());
    }
    (!headers.is_empty()).then_some(headers)
}
