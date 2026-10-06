use super::{PluginError, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc};

pub type ConfigMutation = Arc<dyn Fn(&mut Value) -> Result<()> + Send + Sync>;
#[derive(Clone)]
pub struct ConfigPort {
    pub read: Arc<dyn Fn() -> Result<Value> + Send + Sync>,
    pub update: Arc<dyn Fn(ConfigMutation) -> BoxFuture<'static, Result<Value>> + Send + Sync>,
    pub normalize_web_search: Arc<dyn Fn(Value) -> Result<Value> + Send + Sync>,
}
#[derive(Clone)]
pub struct Catalog {
    pub tools: Vec<Value>,
    pub presets: Value,
}
impl Catalog {
    pub fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for tool in &self.tools {
            let id = tool["id"]
                .as_str()
                .ok_or_else(|| PluginError::new("工具目录 id 无效。"))?;
            if !seen.insert(id) {
                return Err(PluginError::new("工具目录 id 重复。"));
            }
        }
        if !self.presets.is_object() {
            return Err(PluginError::new("工具预设目录无效。"));
        }
        Ok(())
    }
    pub fn ids(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter_map(|tool| tool["id"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn tools_from_config(&self, config: &Value) -> Vec<String> {
        let requested = config["toolMode"].as_str().unwrap_or("");
        if !requested.is_empty() && !["full", "custom"].contains(&requested) {
            return strings(&self.presets["full"]);
        }
        if let Some(tools) = config["enabledTools"].as_array() {
            let allowed: HashSet<_> = self.ids().into_iter().collect();
            return tools
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| allowed.contains(*id))
                .map(str::to_owned)
                .collect();
        }
        let preset = self.presets.get(requested).unwrap_or(&self.presets["full"]);
        strings(preset)
    }
    pub fn preset(&self, enabled: &[String]) -> String {
        for (name, tools) in self.presets.as_object().into_iter().flatten() {
            let tools = strings(tools);
            if tools.len() == enabled.len() && tools.iter().all(|id| enabled.contains(id)) {
                return name.clone();
            }
        }
        "custom".into()
    }
    pub(crate) fn builtin_plugins(&self, enabled: &[String]) -> Vec<Value> {
        let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
        for tool in &self.tools {
            let category = tool["category"].as_str().unwrap_or("system").to_owned();
            let index = match groups.iter().position(|(name, _)| *name == category) {
                Some(index) => index,
                None => {
                    groups.push((category, Vec::new()));
                    groups.len() - 1
                }
            };
            let mut value = tool.clone();
            value["name"] = tool["id"].clone();
            value["label"] = tool["name"].clone();
            value["enabled"] = json!(tool["id"]
                .as_str()
                .is_some_and(|id| enabled.iter().any(|v| v == id)));
            value["effectiveRisk"] = tool["risk"].clone();
            groups[index].1.push(value);
        }
        groups.into_iter().map(|(category, capabilities)| json!({
            "id":format!("builtin.{category}"),"name":category,"description":"","version":"builtin",
            "source":"builtin","builtIn":true,"enabled":capabilities.iter().any(|cap| cap["enabled"] == true),
            "capabilities":capabilities,
        })).collect()
    }
}
pub(crate) fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
pub(crate) fn unique_strings(value: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    strings(value)
        .into_iter()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty() && seen.insert(v.clone()))
        .collect()
}
