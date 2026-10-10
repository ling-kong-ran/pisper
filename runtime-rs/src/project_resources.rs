//! Pisper 的项目资源继续从 .pisper 读取；每个会话按自身 cwd 发现，不复制到测试目录。
use pi_rust::coding_agent::extensions::{
    loader::ExtensionFactory,
    types::{sync_handler, HandlerResult},
};
use serde_json::json;
use std::{path::Path, sync::Arc};

pub(crate) fn extension() -> ExtensionFactory {
    Arc::new(|api| {
        api.on(
            "resources_discover",
            sync_handler(|event, _| {
                let cwd = event
                    .get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("Missing resource workspace")?;
                Ok(Some(HandlerResult::Json(
                    json!({"skillPaths":paths(cwd, "skills"),"promptPaths":paths(cwd, "prompts")}),
                )))
            }),
        )?;
        Ok(())
    })
}

// 引擎发现与会话命令目录共享同一来源，避免列表漏掉引擎已经加载的项目资源。
pub(crate) fn paths(cwd: &str, name: &str) -> Vec<String> {
    let path = Path::new(cwd).join(".pisper").join(name);
    if path.is_dir() {
        vec![path.to_string_lossy().into_owned()]
    } else {
        Vec::new()
    }
}
