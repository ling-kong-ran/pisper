//! 移动宿主通过显式配置嵌入同一服务，不修改进程环境或退出整个 App。
use std::path::PathBuf;

pub(crate) const MOBILE_TOOLS: &[&str] = &[
    "read",
    "edit",
    "write",
    "ls",
    "web_search",
    "generate_visual",
    "memory_search",
    "memory_remember",
];

pub struct EmbeddedOptions {
    pub agent_dir: PathBuf,
    pub workspace: PathBuf,
    pub resource_dir: PathBuf,
    pub token: String,
    pub store: bool,
}

pub struct EmbeddedServer {
    pub bootstrap_url: String,
    stop: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl EmbeddedServer {
    /// 宿主同时观察服务退出和自己的关闭请求，异常退出不能伪装成健康线程。
    pub async fn wait(&mut self) -> anyhow::Result<()> {
        if let Some(task) = self.task.as_mut() {
            task.await??;
        }
        self.task.take();
        Ok(())
    }
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub async fn start_embedded(options: EmbeddedOptions) -> anyhow::Result<EmbeddedServer> {
    if options.token.len() < 32
        || !options
            .token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        anyhow::bail!("Embedded runtime requires a URL-safe random authentication token");
    }
    for directory in [
        &options.agent_dir,
        &options.workspace,
        &options.resource_dir,
    ] {
        if !directory.is_absolute() || !directory.is_dir() {
            anyhow::bail!("Embedded runtime requires existing absolute directories");
        }
    }
    if !options.resource_dir.join("dist/index.html").is_file() {
        anyhow::bail!("Embedded frontend is missing");
    }
    // 端口先绑定，避免端口失败时留下已启动的领域服务。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let state = super::boot(Some(&options)).await?;
    let stop = tokio_util::sync::CancellationToken::new();
    let cancellation = stop.clone();
    let task = tokio::spawn(super::serve_runtime(state, listener, async move {
        cancellation.cancelled().await
    }));
    Ok(EmbeddedServer {
        bootstrap_url: format!(
            "http://{address}/_pisper/desktop/bootstrap?token={}",
            options.token
        ),
        stop,
        task: Some(task),
    })
}

pub(crate) fn mobile_tool_allowed(name: &str) -> bool {
    MOBILE_TOOLS.contains(&name)
}

pub(crate) async fn capability_guard(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = request.uri().path();
    let session_operation = path
        .strip_prefix("/api/sessions/")
        .and_then(|suffix| suffix.split('/').nth(1));
    if state.runtime_profile != "desktop"
        && (matches!(session_operation, Some("agents" | "team" | "goal" | "plan"))
            || [
                "/api/desktop",
                "/api/mcp",
                "/mcp",
                "/api/plugins",
                "/api/tool-plugins",
                "/api/workflows",
                "/api/workflow-runs",
                "/api/schedules",
                "/api/channels",
                "/api/goals",
                "/api/teams",
                "/api/multi-agent",
                "/api/vcs",
                "/api/shell",
                "/api/speech",
                "/api/browser",
            ]
            .iter()
            .any(|prefix| {
                path == *prefix
                    || path
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            }))
    {
        return crate::ApiError::new(
            axum::http::StatusCode::NOT_IMPLEMENTED,
            "unsupported",
            "当前移动宿主不支持此功能。",
        )
        .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn embedded_host_preserves_data_and_auth_across_restart() {
        let root =
            std::env::temp_dir().join(format!("pisper-embedded-host-{}", uuid::Uuid::new_v4()));
        let options = || EmbeddedOptions {
            agent_dir: root.join("agent"),
            workspace: root.join("workspace"),
            resource_dir: root.join("resources"),
            token: "fixture-embedded-authentication-token-12345".into(),
            store: false,
        };
        for directory in [
            "agent",
            "workspace",
            "resources/dist",
            "resources/shared/speech",
            "resources/shared/speech-resources",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(
            root.join("resources/dist/index.html"),
            "<!doctype html><title>embedded fixture</title>",
        )
        .unwrap();
        let shared = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../shared");
        for entry in [
            "speech/speech-model-catalog.json",
            "speech/speech-resource-notices.json",
            "speech-resources/xasr-bpe.vocab",
        ] {
            std::fs::copy(
                shared.join(entry),
                root.join("resources/shared").join(entry),
            )
            .unwrap();
        }
        let previous_agent = std::env::var_os("PI_CODING_AGENT_DIR");
        let server = start_embedded(options()).await.unwrap();
        let base = server.bootstrap_url.split("/_pisper").next().unwrap();
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(format!("{base}/api/sessions"))
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        let cookie = format!("__pisper_desktop={}", options().token);
        let ready: serde_json::Value = client
            .get(format!("{base}/api/ready"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(ready["profile"], "mobile-embedded");
        assert_eq!(ready["ready"], true);
        let sessions: serde_json::Value = client
            .get(format!("{base}/api/sessions"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!sessions["sessions"].as_array().unwrap().is_empty());
        assert_eq!(
            client
                .get(format!("{base}/api/mcp"))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::NOT_IMPLEMENTED
        );
        let old_base = base.to_owned();
        server.shutdown().await.unwrap();
        assert!(client
            .get(format!("{old_base}/api/ready"))
            .send()
            .await
            .is_err());
        let restarted = start_embedded(options()).await.unwrap();
        let base = restarted.bootstrap_url.split("/_pisper").next().unwrap();
        let restored: serde_json::Value = client
            .get(format!("{base}/api/sessions"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(restored["sessions"][0]["id"], sessions["sessions"][0]["id"]);
        restarted.shutdown().await.unwrap();
        let state = crate::boot(Some(&options())).await.unwrap();
        let active = state.runtime.session().get_active_tool_names();
        for name in ["read", "web_search", "generate_visual"] {
            assert!(
                active.iter().any(|value| value == name),
                "missing mobile tool: {name}"
            );
        }
        assert!(!active.iter().any(|value| value == "bash"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        crate::serve_runtime(state, listener, async {})
            .await
            .unwrap();
        assert_eq!(std::env::var_os("PI_CODING_AGENT_DIR"), previous_agent);
        std::fs::remove_dir_all(root).unwrap();
    }
}
