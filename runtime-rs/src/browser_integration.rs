//! 将浏览器工具绑定到真实主会话及截图归档；服务和驱动不持有整个应用。
use crate::{native_browser::types::*, AppState};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock, Weak},
};

pub(crate) struct BrowserIntegration {
    state: OnceLock<Weak<AppState>>,
}
impl BrowserIntegration {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: OnceLock::new(),
        })
    }
    pub(crate) fn attach(&self, state: &Arc<AppState>) -> BrowserResult<()> {
        self.state
            .set(Arc::downgrade(state))
            .map_err(|_| "Browser integration already attached".to_owned())
    }
    fn state(&self) -> BrowserResult<Arc<AppState>> {
        self.state
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "Browser tool runtime unavailable".to_owned())
    }
    pub(crate) fn context_port(self: &Arc<Self>) -> BrowserContextPort {
        let integration = self.clone();
        Arc::new(move |native_id, cwd: PathBuf| {
            let integration = integration.clone();
            Box::pin(async move {
                let state = integration.state()?;
                if state.executor.tool_limit(&native_id).is_some() {
                    return Err("Browser automation is available only to the primary Agent. Subagents cannot open or control nested browser sessions.".into());
                }
                let session_id = state
                    .sessions
                    .public_id_for_native(&native_id)
                    .ok_or_else(|| "Browser tool session context unavailable".to_owned())?;
                if !cwd.is_absolute() {
                    return Err("Browser tool working directory unavailable".into());
                }
                Ok(BrowserContext { session_id, cwd })
            })
        })
    }
    pub(crate) fn generated_file_port(self: &Arc<Self>) -> BrowserGeneratedFilePort {
        let integration = self.clone();
        Arc::new(move |context, result| {
            let integration = integration.clone();
            Box::pin(async move {
                let state = integration.state()?;
                let path = PathBuf::from(
                    result["path"]
                        .as_str()
                        .ok_or_else(|| "Browser screenshot path unavailable".to_owned())?,
                );
                let assets = state.assets.clone();
                let directory = state.agent_dir.clone();
                tokio::task::spawn_blocking(move || {
                    let name = crate::session_api::session_infos(&directory)
                        .into_iter()
                        .find(|session| session.id == context.session_id)
                        .and_then(|session| session.name)
                        .unwrap_or_default();
                    assets
                        .lock()
                        .map_err(|error| error.to_string())?
                        .archive_generated(&path, &context.session_id, &name)
                        .map_err(|error| error.to_string())?;
                    Ok(())
                })
                .await
                .map_err(|error| error.to_string())?
            })
        })
    }
}
