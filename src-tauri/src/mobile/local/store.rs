//! 本机 Runtime 持久化：Provider 配置与会话消息分别落盘为两个 JSON，
//! 全部写入走「临时文件 + rename」原子替换，避免中断留下半截文件。
//!
//! 安全约定：apiKey 只出现在 providers.json；任何对外 DTO 必须走
//! `ProviderProfile::redacted` 脱敏（仅保留末 4 位 keyHint）。
//! 资源上限见 docs/mobile-local-runtime.md：超限即淘汰最旧会话。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// 上限常量：手机存储与内存受限，宁可淘汰历史也不能无限增长。
pub const MAX_SESSIONS: usize = 50;
pub const MAX_MESSAGES_PER_SESSION: usize = 200;
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;
pub const MAX_TOTAL_SESSION_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// 仅落盘，不出现在任何 API 响应里。
    pub api_key: String,
    pub model: String,
    pub created_at: String,
}

/// 对外暴露的脱敏视图：前端只能看到 keyHint，无法读回完整密钥。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub has_key: bool,
    pub key_hint: String,
    pub created_at: String,
}

impl ProviderProfile {
    pub fn redacted(&self) -> RedactedProvider {
        let trimmed = self.api_key.trim();
        RedactedProvider {
            id: self.id.clone(),
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            has_key: !trimmed.is_empty(),
            key_hint: trimmed
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            created_at: self.created_at.clone(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    active_id: Option<String>,
    #[serde(default)]
    providers: Vec<ProviderProfile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSession {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub messages: Vec<LocalMessage>,
}

/// 会话列表的轻量视图：不把整段消息体抛给列表接口。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    sessions: Vec<LocalSession>,
}

pub struct LocalStore {
    providers_path: PathBuf,
    sessions_path: PathBuf,
    providers: ProviderFile,
    sessions: SessionFile,
}

impl LocalStore {
    pub fn load(dir: &Path) -> Self {
        let providers_path = dir.join("providers.json");
        let sessions_path = dir.join("sessions.json");
        let providers = fs::read_to_string(&providers_path)
            .ok()
            .and_then(|text| serde_json::from_str::<ProviderFile>(&text).ok())
            .filter(|file| file.version <= 1)
            .unwrap_or_default();
        let sessions = fs::read_to_string(&sessions_path)
            .ok()
            .and_then(|text| serde_json::from_str::<SessionFile>(&text).ok())
            .filter(|file| file.version <= 1)
            .unwrap_or_default();
        Self {
            providers_path,
            sessions_path,
            providers,
            sessions,
        }
    }

    fn save_providers(&self) -> Result<(), String> {
        let file = ProviderFile {
            version: 1,
            active_id: self.providers.active_id.clone(),
            providers: self.providers.providers.clone(),
        };
        write_atomic(&self.providers_path, &file)
    }

    fn save_sessions(&self) -> Result<(), String> {
        let file = SessionFile {
            version: 1,
            sessions: self.sessions.sessions.clone(),
        };
        write_atomic(&self.sessions_path, &file)
    }

    // ---- Provider ----

    pub fn providers(&self) -> Vec<RedactedProvider> {
        self.providers
            .providers
            .iter()
            .map(ProviderProfile::redacted)
            .collect()
    }

    pub fn active_provider_id(&self) -> Option<String> {
        self.providers.active_id.clone()
    }

    /// 仅本机 Runtime 内部使用：取出完整 Provider（含密钥）发起请求。
    pub fn active_provider(&self) -> Option<ProviderProfile> {
        let id = self.providers.active_id.as_deref()?;
        self.providers
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .cloned()
    }

    /// 新增或更新 Provider。apiKey 传 None 表示保留已有密钥（编辑时不回显）。
    pub fn upsert_provider(
        &mut self,
        id: Option<String>,
        name: String,
        base_url: String,
        api_key: Option<String>,
        model: String,
        now: String,
    ) -> Result<RedactedProvider, String> {
        let name = name.trim().to_string();
        let base_url = normalize_base_url(&base_url)?;
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err("模型名不能为空。".into());
        }
        let api_key = api_key.map(|key| key.trim().to_string());
        let profile = if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            let existing = self
                .providers
                .providers
                .iter_mut()
                .find(|provider| provider.id == id)
                .ok_or_else(|| "Provider 不存在。".to_string())?;
            existing.name = if name.is_empty() {
                existing.name.clone()
            } else {
                name
            };
            existing.base_url = base_url;
            existing.model = model;
            // 空密钥 = 不改动；非空才替换，避免前端“留空保留”误清空。
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                existing.api_key = key;
            }
            existing.clone()
        } else {
            let profile = ProviderProfile {
                id: new_id("locp"),
                name: if name.is_empty() {
                    default_provider_name(&base_url)
                } else {
                    name
                },
                base_url,
                api_key: api_key.unwrap_or_default(),
                model,
                created_at: now,
            };
            self.providers.providers.push(profile.clone());
            profile
        };
        self.providers.active_id = Some(profile.id.clone());
        self.save_providers()?;
        Ok(profile.redacted())
    }

    pub fn select_provider(&mut self, id: &str) -> Result<(), String> {
        if !self.providers.providers.iter().any(|p| p.id == id) {
            return Err("Provider 不存在。".into());
        }
        self.providers.active_id = Some(id.to_string());
        self.save_providers()
    }

    pub fn delete_provider(&mut self, id: &str) -> Result<(), String> {
        self.providers.providers.retain(|p| p.id != id);
        if self.providers.active_id.as_deref() == Some(id) {
            self.providers.active_id = self.providers.providers.first().map(|p| p.id.clone());
        }
        self.save_providers()
    }

    // ---- 会话 ----

    pub fn session_summaries(&self) -> Vec<SessionSummary> {
        // 最近更新在前：列表直接可用。
        let mut sessions = self.sessions.sessions.clone();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
            .into_iter()
            .map(|session| SessionSummary {
                id: session.id,
                title: session.title,
                created_at: session.created_at,
                updated_at: session.updated_at,
                message_count: session.messages.len(),
            })
            .collect()
    }

    pub fn session(&self, id: &str) -> Option<LocalSession> {
        self.sessions
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
    }

    pub fn create_session(&mut self, now: String) -> Result<LocalSession, String> {
        let session = LocalSession {
            id: new_id("locs"),
            title: String::new(),
            created_at: now.clone(),
            updated_at: now,
            messages: Vec::new(),
        };
        self.sessions.sessions.push(session.clone());
        self.enforce_limits();
        self.save_sessions()?;
        Ok(session)
    }

    pub fn delete_session(&mut self, id: &str) -> Result<(), String> {
        self.sessions.sessions.retain(|session| session.id != id);
        self.save_sessions()
    }

    /// 追加消息并维护 updated_at / 标题 / 上限。返回持久化后的消息。
    pub fn append_message(
        &mut self,
        session_id: &str,
        role: &str,
        content: String,
        now: String,
    ) -> Result<LocalMessage, String> {
        if content.len() > MAX_MESSAGE_BYTES {
            return Err("消息过长（上限 32 KiB）。".into());
        }
        let session = self
            .sessions
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "会话不存在。".to_string())?;
        let message = LocalMessage {
            id: new_id("locm"),
            role: role.to_string(),
            content,
            created_at: now.clone(),
        };
        session.messages.push(message.clone());
        session.updated_at = now;
        // 首轮对话用用户首条消息生成标题，避免列表全是“未命名”。
        if session.title.is_empty() && role == "user" {
            session.title = message
                .content
                .chars()
                .take(30)
                .collect::<String>()
                .trim()
                .to_string();
        }
        while session.messages.len() > MAX_MESSAGES_PER_SESSION {
            session.messages.remove(0);
        }
        self.enforce_limits();
        self.save_sessions()?;
        Ok(message)
    }

    /// 移除消息：流式失败且没有任何增量时，清掉空的助手占位气泡。
    pub fn remove_message(&mut self, session_id: &str, message_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "会话不存在。".to_string())?;
        session.messages.retain(|message| message.id != message_id);
        self.save_sessions()
    }

    /// 流式结束后回写助手消息内容（部分回复也保留，前端可看到“已中断”的上下文）。
    pub fn finalize_message(
        &mut self,
        session_id: &str,
        message_id: &str,
        content: String,
        now: String,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "会话不存在。".to_string())?;
        let message = session
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .ok_or_else(|| "消息不存在。".to_string())?;
        message.content = content;
        session.updated_at = now;
        self.enforce_limits();
        self.save_sessions()
    }

    /// 淘汰策略：先按会话数，再按总量；始终淘汰最旧更新的会话。
    fn enforce_limits(&mut self) {
        self.sessions
            .sessions
            .sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        while self.sessions.sessions.len() > MAX_SESSIONS {
            self.sessions.sessions.remove(0);
        }
        let total = |sessions: &[LocalSession]| {
            sessions
                .iter()
                .flat_map(|session| session.messages.iter())
                .map(|message| message.content.len())
                .sum::<usize>()
        };
        while self.sessions.sessions.len() > 1
            && total(&self.sessions.sessions) > MAX_TOTAL_SESSION_BYTES
        {
            self.sessions.sessions.remove(0);
        }
    }
}

/// baseUrl 策略：默认要求 https；loopback 允许 http（本机模型服务/开发调试）。
pub fn normalize_base_url(input: &str) -> Result<String, String> {
    let url = input.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err("Provider 地址不能为空。".into());
    }
    if url.starts_with("https://") {
        return Ok(url);
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest
            .split(['/', ':'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]" {
            return Ok(url);
        }
        return Err("仅本机回环地址允许使用 http://，其他 Provider 必须使用 https://。".into());
    }
    Err("Provider 地址必须以 https:// 开头。".into())
}

fn default_provider_name(base_url: &str) -> String {
    base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("Provider")
        .to_string()
}

fn new_id(prefix: &str) -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("{prefix}_{millis:x}{}", hex_lower(&bytes))
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pisper-local-store-{tag}-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn provider_upsert_keeps_key_when_blank_and_redacts() {
        let dir = test_dir("provider");
        let mut store = LocalStore::load(&dir);
        let created = store
            .upsert_provider(
                None,
                "Kimi".into(),
                "https://api.moonshot.cn/v1".into(),
                Some("sk-1234567890abcdef".into()),
                "kimi-k2".into(),
                "1".into(),
            )
            .unwrap();
        assert_eq!(created.key_hint, "cdef");
        assert!(created.has_key);

        // 编辑时留空密钥：保留原密钥。
        let updated = store
            .upsert_provider(
                Some(created.id.clone()),
                "Kimi 2".into(),
                "https://api.moonshot.cn/v1".into(),
                None,
                "kimi-k3".into(),
                "2".into(),
            )
            .unwrap();
        assert_eq!(updated.name, "Kimi 2");
        let full = store.active_provider().unwrap();
        assert_eq!(full.api_key, "sk-1234567890abcdef");
        assert_eq!(full.model, "kimi-k3");

        // 重载后仍在，且序列化结果不含密钥字段泄露到 redacted 视图。
        let reloaded = LocalStore::load(&dir);
        assert_eq!(reloaded.providers().len(), 1);
        let json = serde_json::to_string(&reloaded.providers()[0]).unwrap();
        assert!(!json.contains("sk-1234567890abcdef"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn base_url_policy() {
        assert!(normalize_base_url("https://api.openai.com/v1/").is_ok());
        assert!(normalize_base_url("http://127.0.0.1:11434/v1").is_ok());
        assert!(normalize_base_url("http://localhost:8080/v1").is_ok());
        assert!(normalize_base_url("http://192.168.1.5:11434/v1").is_err());
        assert!(normalize_base_url("api.openai.com").is_err());
    }

    #[test]
    fn session_lifecycle_and_limits() {
        let dir = test_dir("session");
        let mut store = LocalStore::load(&dir);
        let session = store.create_session("1".into()).unwrap();
        let message = store
            .append_message(&session.id, "user", "你好，本机运行".into(), "2".into())
            .unwrap();
        assert_eq!(message.role, "user");

        // 标题取自首条用户消息。
        let reloaded = LocalStore::load(&dir);
        let session = reloaded.session(&session.id).unwrap();
        assert_eq!(session.title, "你好，本机运行");

        // 单会话消息数上限：淘汰最旧。
        let mut store = reloaded;
        for index in 0..(MAX_MESSAGES_PER_SESSION + 10) {
            store
                .append_message(&session.id, "user", format!("m{index}"), "3".into())
                .unwrap();
        }
        let session = store.session(&session.id).unwrap();
        assert_eq!(session.messages.len(), MAX_MESSAGES_PER_SESSION);
        assert!(!session.messages.iter().any(|m| m.content == "m0"));

        // 会话数上限：最旧的先淘汰。
        for _ in 0..(MAX_SESSIONS + 5) {
            store.create_session("4".into()).unwrap();
        }
        assert!(store.session_summaries().len() <= MAX_SESSIONS);
        assert!(store.session(&session.id).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finalize_updates_content_and_summary_view_is_light() {
        let dir = test_dir("finalize");
        let mut store = LocalStore::load(&dir);
        let session = store.create_session("1".into()).unwrap();
        let pending = store
            .append_message(&session.id, "assistant", String::new(), "2".into())
            .unwrap();
        store
            .finalize_message(&session.id, &pending.id, "完整回复".into(), "3".into())
            .unwrap();
        let session = store.session(&session.id).unwrap();
        assert_eq!(session.messages[0].content, "完整回复");
        assert_eq!(store.session_summaries()[0].message_count, 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
