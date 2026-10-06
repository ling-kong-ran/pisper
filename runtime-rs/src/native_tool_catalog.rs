//! release 固定工具目录与本机 Pi 注册工具的交集；配置保留尚未迁移的工具名。
use crate::native_plugins::{Catalog, PluginError};
use serde_json::Value;

pub(crate) fn release_catalog() -> Result<Catalog, PluginError> {
    let resource: Value = serde_json::from_str(include_str!("../resources/tool-catalog.json"))?;
    if resource["sourceCommit"] != "582160235671903d9f1c7034b457557b1df74b68" {
        return Err(PluginError::new("工具目录的 release 来源不匹配。"));
    }
    let catalog = Catalog {
        tools: resource["tools"]
            .as_array()
            .cloned()
            .ok_or_else(|| PluginError::new("工具目录无效。"))?,
        presets: resource["presets"].clone(),
    };
    catalog.validate()?;
    Ok(catalog)
}

pub(crate) fn is_hot(name: &str) -> bool {
    matches!(
        name,
        "read"
            | "ls"
            | "grep"
            | "find"
            | "edit"
            | "write"
            | "bash"
            | "get_plan"
            | "update_plan"
            | "discover_tools"
            | "call_tool"
            | "spawn_agent"
            | "followup_task"
            | "list_agents"
            | "send_message"
            | "wait_agent"
            | "interrupt_agent"
    )
}

pub(crate) fn is_legacy(name: &str) -> bool {
    matches!(name, "get_task_list" | "update_task_list")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_metadata_and_presets_keep_all_original_ids() {
        let catalog = release_catalog().unwrap();
        assert_eq!(catalog.tools.len(), 19);
        assert_eq!(catalog.presets["full"].as_array().unwrap().len(), 18);
        assert!(catalog.ids().contains(&"image_assets".to_owned()));
        assert!(!catalog.presets["full"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("image_assets")));
        assert_eq!(
            catalog
                .tools
                .iter()
                .find(|tool| tool["id"] == "web_search")
                .unwrap()["risk"],
            "medium"
        );
    }
}
