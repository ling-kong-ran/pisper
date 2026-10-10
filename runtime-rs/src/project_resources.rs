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
                let paths = |name: &str| {
                    let path = Path::new(cwd).join(".pisper").join(name);
                    if path.is_dir() {
                        vec![path.to_string_lossy().into_owned()]
                    } else {
                        Vec::new()
                    }
                };
                Ok(Some(HandlerResult::Json(
                    json!({"skillPaths":paths("skills"),"promptPaths":paths("prompts")}),
                )))
            }),
        )?;
        Ok(())
    })
}
