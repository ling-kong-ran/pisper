//! 移动端服务器档案存储：已配对的桌面端列表（endpoint、指纹、设备令牌），
//! 持久化为应用数据目录下的 JSON。令牌属于敏感数据，仅存放在应用私有目录。
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEndpoint {
    #[serde(rename = "t")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct_addresses: Vec<String>,
}

impl ServerEndpoint {
    pub fn lan(url: String) -> Self {
        Self {
            kind: "lan".into(),
            url,
            node_id: None,
            relay_url: None,
            direct_addresses: Vec::new(),
        }
    }

    pub fn iroh(endpoint: crate::iroh_tunnel::TunnelEndpoint) -> Self {
        Self {
            kind: "iroh".into(),
            url: String::new(),
            node_id: Some(endpoint.node_id),
            relay_url: endpoint.relay_url,
            direct_addresses: endpoint.direct_addresses,
        }
    }

    pub fn tunnel_endpoint(&self) -> Result<crate::iroh_tunnel::TunnelEndpoint, String> {
        if self.kind != "iroh" {
            return Err("端点不是 Iroh 类型。".into());
        }
        let node_id = self
            .node_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Iroh 端点缺少节点 ID。".to_string())?;
        Ok(crate::iroh_tunnel::TunnelEndpoint {
            node_id,
            relay_url: self.relay_url.clone(),
            direct_addresses: self.direct_addresses.clone(),
        })
    }

    pub fn display_address(&self) -> String {
        if self.kind == "iroh" {
            return self
                .node_id
                .as_deref()
                .map(|value| format!("Iroh {}", value.chars().take(12).collect::<String>()))
                .unwrap_or_else(|| "Iroh".into());
        }
        self.url.clone()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProfile {
    pub id: String,
    pub name: String,
    pub endpoints: Vec<ServerEndpoint>,
    /// 配对时带外获得的 TLS 证书指纹（SHA256 十六进制，可带 SHA256: 前缀）。
    pub fingerprint: String,
    pub device_id: String,
    pub token: String,
    pub paired_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCapabilities {
    #[serde(default)]
    pub contacts: bool,
    #[serde(default)]
    pub camera: bool,
    #[serde(default)]
    pub location: bool,
    #[serde(default)]
    pub external_apps: bool,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            contacts: true,
            camera: true,
            location: true,
            external_apps: true,
        }
    }
}

impl DeviceCapabilities {
    pub fn enabled(&self, capability: &str) -> bool {
        match capability {
            "contacts" => self.contacts,
            "camera" => self.camera,
            "location" => self.location,
            "externalApps" => self.external_apps,
            _ => false,
        }
    }

    fn set(&mut self, capability: &str, enabled: bool) -> Result<(), String> {
        match capability {
            "contacts" => self.contacts = enabled,
            "camera" => self.camera = enabled,
            "location" => self.location = enabled,
            "externalApps" => self.external_apps = enabled,
            _ => return Err("不支持的移动设备能力。".into()),
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    active_id: Option<String>,
    /// 上次使用的模式（remote/local）：冷启动据此决定进入远程还是本机界面。
    #[serde(default)]
    last_mode: Option<String>,
    #[serde(default)]
    servers: Vec<ServerProfile>,
    #[serde(default)]
    device_capabilities: DeviceCapabilities,
}

pub struct ProfileStore {
    path: PathBuf,
    file: StoreFile,
}

impl ProfileStore {
    pub fn load(path: &Path) -> Self {
        let file = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<StoreFile>(&text).ok())
            .filter(|file| file.version <= 1)
            .unwrap_or_default();
        Self {
            path: path.to_path_buf(),
            file,
        }
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let file = StoreFile {
            version: 1,
            active_id: self.file.active_id.clone(),
            last_mode: self.file.last_mode.clone(),
            servers: self.file.servers.clone(),
            device_capabilities: self.file.device_capabilities.clone(),
        };
        // 临时文件 + rename：避免写入中断留下半截 JSON。
        let temporary = self.path.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        fs::rename(&temporary, &self.path).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn servers(&self) -> &[ServerProfile] {
        &self.file.servers
    }

    pub fn active(&self) -> Option<&ServerProfile> {
        let id = self.file.active_id.as_deref()?;
        self.file.servers.iter().find(|server| server.id == id)
    }

    pub fn upsert(&mut self, profile: ServerProfile) -> Result<(), String> {
        // 同一 deviceId 重复配对视为更新（例如重新扫码）。
        if let Some(existing) = self
            .file
            .servers
            .iter_mut()
            .find(|server| server.device_id == profile.device_id)
        {
            *existing = profile.clone();
        } else {
            self.file.servers.push(profile.clone());
        }
        self.file.active_id = Some(profile.id);
        self.save()
    }

    pub fn select(&mut self, id: &str) -> Result<(), String> {
        if !self.file.servers.iter().any(|server| server.id == id) {
            return Err("服务器不存在。".into());
        }
        self.file.active_id = Some(id.to_string());
        self.save()
    }

    pub fn forget(&mut self, id: &str) -> Result<(), String> {
        self.file.servers.retain(|server| server.id != id);
        if self.file.active_id.as_deref() == Some(id) {
            self.file.active_id = None;
        }
        self.save()
    }

    /// 上次模式只接受远程与同源本机 Runtime 两种路由，未知值按未设置处理。
    pub fn last_mode(&self) -> Option<&str> {
        match self.file.last_mode.as_deref() {
            Some("remote") => Some("remote"),
            Some("local") => Some("local"),
            _ => None,
        }
    }

    pub fn set_last_mode(&mut self, mode: &str) -> Result<(), String> {
        if !matches!(mode, "remote" | "local") {
            return Err("不支持的移动端运行模式。".into());
        }
        self.file.last_mode = Some(mode.to_string());
        self.save()
    }

    pub fn device_capabilities(&self) -> DeviceCapabilities {
        self.file.device_capabilities.clone()
    }

    pub fn set_device_capability(
        &mut self,
        capability: &str,
        enabled: bool,
    ) -> Result<DeviceCapabilities, String> {
        self.file.device_capabilities.set(capability, enabled)?;
        self.save()?;
        Ok(self.device_capabilities())
    }
}

/// 归一化指纹输入：去掉 `SHA256:` 前缀与分隔符，转大写。
pub fn normalize_fingerprint(input: &str) -> String {
    let stripped = input
        .trim()
        .trim_start_matches("SHA256:")
        .trim_start_matches("sha256:");
    stripped
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .collect::<String>()
        .to_uppercase()
}

pub type SharedStore = Mutex<ProfileStore>;

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, device: &str) -> ServerProfile {
        ServerProfile {
            id: id.into(),
            name: format!("server-{id}"),
            endpoints: vec![ServerEndpoint::lan("https://192.168.1.5:5174".into())],
            fingerprint: "SHA256:ABCD".into(),
            device_id: device.into(),
            token: "pst_x".into(),
            paired_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn upsert_select_forget_roundtrip() {
        let path =
            std::env::temp_dir().join(format!("pisper-store-test-{}.json", std::process::id()));
        let mut store = ProfileStore::load(&path);
        store.upsert(profile("a", "dev-a")).unwrap();
        store.upsert(profile("b", "dev-b")).unwrap();
        assert_eq!(store.active().unwrap().id, "b");
        store.select("a").unwrap();
        assert_eq!(store.active().unwrap().id, "a");

        // 重新加载验证持久化。
        let reloaded = ProfileStore::load(&path);
        assert_eq!(reloaded.servers().len(), 2);
        assert_eq!(reloaded.active().unwrap().id, "a");

        let mut reloaded = reloaded;
        reloaded.forget("a").unwrap();
        assert!(reloaded.active().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn device_capabilities_default_on_and_persist_independently() {
        let path = std::env::temp_dir().join(format!(
            "pisper-device-capabilities-test-{}.json",
            std::process::id()
        ));
        let mut store = ProfileStore::load(&path);
        let initial = store.device_capabilities();
        assert!(initial.contacts && initial.camera && initial.location && initial.external_apps);
        let updated = store.set_device_capability("camera", false).unwrap();
        assert!(updated.contacts && !updated.camera && updated.location && updated.external_apps);
        let updated = store.set_device_capability("externalApps", false).unwrap();
        assert!(!updated.camera && !updated.external_apps);
        assert!(store.set_device_capability("sms", true).is_err());
        let persisted = ProfileStore::load(&path).device_capabilities();
        assert!(
            persisted.contacts
                && !persisted.camera
                && persisted.location
                && !persisted.external_apps
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fingerprint_normalization() {
        assert_eq!(normalize_fingerprint("SHA256:ab:cd:ef"), "ABCDEF");
        assert_eq!(normalize_fingerprint("  abcd  "), "ABCD");
    }

    #[test]
    fn last_mode_roundtrip_and_unknown_values() {
        let path =
            std::env::temp_dir().join(format!("pisper-mode-test-{}.json", std::process::id()));
        let mut store = ProfileStore::load(&path);
        assert_eq!(store.last_mode(), None);
        store.set_last_mode("local").unwrap();
        let reloaded = ProfileStore::load(&path);
        assert_eq!(reloaded.last_mode(), Some("local"));
        assert!(store.set_last_mode("something-else").is_err());
        // 非法持久化值不生效，也不破坏文件。
        fs::write(
            &path,
            r#"{"version":1,"lastMode":"something-else","servers":[]}"#,
        )
        .unwrap();
        assert_eq!(ProfileStore::load(&path).last_mode(), None);
        let _ = std::fs::remove_file(&path);
    }
}
