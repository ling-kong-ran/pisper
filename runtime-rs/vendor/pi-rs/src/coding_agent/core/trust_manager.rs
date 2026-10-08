//! Project trust persistence and resource discovery, from upstream trust-manager.ts.
//!
//! Locks use the existing `<trust.json>.lock` directory protocol, including
//! 10 attempts / 20ms retry. Stale-lock heartbeat/reclamation from proper-lockfile
//! is not implemented (matching the other native settings/auth stores); a stale
//! lock fails closed. JSON syntax and native OS error prose are platform-owned.
//! Authored validation errors, valid JSON bytes, UTF-16 sorting, nearest-ancestor
//! decisions and prompt options are compared against the real upstream oracle.
use super::{path_join, CONFIG_DIR_NAME};
use crate::coding_agent::utils::{
    paths::{canonicalize_path, resolve_path_auto_base},
    text::strip_bom,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub type ProjectTrustDecision = Option<bool>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTrustStoreEntry {
    pub path: String,
    pub decision: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTrustUpdate {
    pub path: String,
    pub decision: ProjectTrustDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustOption {
    pub label: String,
    pub trusted: bool,
    pub updates: Vec<ProjectTrustUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<String>,
}

fn normalize_cwd(cwd: &str) -> Result<String, String> {
    resolve_path_auto_base(cwd)
        .map(|p| canonicalize_path(&p))
        .map_err(|e| e.to_string())
}

fn parent(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty() && p != path)
}

pub fn get_project_trust_parent_path(cwd: &str) -> Result<Option<String>, String> {
    Ok(parent(&normalize_cwd(cwd)?))
}

pub fn get_project_trust_options(
    cwd: &str,
    include_session_only: bool,
) -> Result<Vec<ProjectTrustOption>, String> {
    let trust_path = normalize_cwd(cwd)?;
    let saved = |label: &str, trusted| ProjectTrustOption {
        label: label.into(),
        trusted,
        updates: vec![ProjectTrustUpdate {
            path: trust_path.clone(),
            decision: Some(trusted),
        }],
        saved_path: Some(trust_path.clone()),
    };
    let mut options = vec![saved("Trust", true)];
    if let Some(parent_path) = get_project_trust_parent_path(cwd)? {
        options.push(ProjectTrustOption {
            label: format!("Trust parent folder ({parent_path})"),
            trusted: true,
            updates: vec![
                ProjectTrustUpdate {
                    path: parent_path.clone(),
                    decision: Some(true),
                },
                ProjectTrustUpdate {
                    path: trust_path.clone(),
                    decision: None,
                },
            ],
            saved_path: Some(parent_path),
        });
    }
    if include_session_only {
        options.push(ProjectTrustOption {
            label: "Trust (this session only)".into(),
            trusted: true,
            updates: vec![],
            saved_path: None,
        });
    }
    options.push(saved("Do not trust", false));
    if include_session_only {
        options.push(ProjectTrustOption {
            label: "Do not trust (this session only)".into(),
            trusted: false,
            updates: vec![],
            saved_path: None,
        });
    }
    Ok(options)
}

/// Only cwd/.pi and cwd/ancestor .agents/skills require project consent.
/// Global HOME/.agents/skills stays a user resource even at cwd == HOME.
pub fn has_trust_requiring_project_resources(cwd: &str) -> Result<bool, String> {
    let home = std::env::var("HOME")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    has_trust_requiring_project_resources_with_home(cwd, &home)
}

pub(crate) fn has_trust_requiring_project_resources_with_home(
    cwd: &str,
    home: &str,
) -> Result<bool, String> {
    let user_skills = path_join(&normalize_cwd(home)?, ".agents/skills");
    let mut current = normalize_cwd(cwd)?;
    let config = path_join(&current, CONFIG_DIR_NAME);
    if [
        "settings.json",
        "extensions",
        "skills",
        "prompts",
        "themes",
        "SYSTEM.md",
        "APPEND_SYSTEM.md",
    ]
    .iter()
    .any(|name| Path::new(&path_join(&config, name)).exists())
    {
        return Ok(true);
    }
    loop {
        let skills = path_join(&current, ".agents/skills");
        if skills != user_skills && Path::new(&skills).exists() {
            return Ok(true);
        }
        let Some(next) = parent(&current) else {
            return Ok(false);
        };
        current = next;
    }
}

/// JS Object.keys/entries enumerate array-index keys first, numerically.
fn array_index(key: &str) -> Option<u32> {
    let value = key.parse::<u32>().ok()?;
    (value != u32::MAX && value.to_string() == key).then_some(value)
}

fn enumeration_keys(data: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<_> = data.keys().collect();
    keys.sort_by(|a, b| match (array_index(a), array_index(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    keys
}

fn read_trust_file(path: &str) -> Result<Map<String, Value>, String> {
    if !Path::new(path).exists() {
        return Ok(Map::new());
    }
    let parse = || -> Result<Value, String> {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        serde_json::from_str(strip_bom(&text)).map_err(|e| e.to_string())
    };
    let parsed = parse().map_err(|e| format!("Failed to read trust store {path}: {e}"))?;
    let Value::Object(parsed) = parsed else {
        return Err(format!("Invalid trust store {path}: expected an object"));
    };
    let mut data = Map::new();
    for key in enumeration_keys(&parsed) {
        let value = &parsed[key];
        if !matches!(value, Value::Bool(_) | Value::Null) {
            let quoted = serde_json::to_string(key).expect("string is serializable");
            return Err(format!(
                "Invalid trust store {path}: value for {quoted} must be true, false, or null"
            ));
        }
        // Upstream assigns to {}; the legacy __proto__ setter ignores boolean
        // values and changes only the prototype for null (never an own key).
        if key != "__proto__" {
            data.insert(key.clone(), value.clone());
        }
    }
    Ok(data)
}

fn write_trust_file(path: &str, data: Map<String, Value>) -> Result<(), String> {
    let mut keys: Vec<_> = data.keys().collect();
    keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let mut sorted = Map::new();
    for key in keys {
        sorted.insert(key.clone(), data[key].clone());
    }
    // JSON.stringify enumerates array-index keys before the sorted string keys.
    let mut enumerated = Map::new();
    for key in enumeration_keys(&sorted) {
        enumerated.insert(key.clone(), sorted[key].clone());
    }
    if let Some(dir) = Path::new(path).parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_string_pretty(&enumerated).map_err(|e| e.to_string())? + "\n";
    fs::write(path, bytes).map_err(|e| e.to_string())
}

struct TrustLock(PathBuf);
impl Drop for TrustLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
fn acquire_lock(path: &str) -> Result<TrustLock, String> {
    if let Some(dir) = Path::new(path).parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let lock = PathBuf::from(format!("{path}.lock"));
    for attempt in 1..=10 {
        match fs::create_dir(&lock) {
            Ok(()) => return Ok(TrustLock(lock)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if attempt == 10 {
                    return Err(format!("ELOCKED: resource is locked: {}", lock.display()));
                }
                // No event-loop progress is observable here; do not busy-spin.
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!("ten attempts return success or an error")
}

#[derive(Debug, Clone)]
pub struct ProjectTrustStore {
    trust_path: String,
}
impl ProjectTrustStore {
    pub fn new(agent_dir: &str) -> Result<Self, String> {
        Ok(Self {
            trust_path: path_join(
                &resolve_path_auto_base(agent_dir).map_err(|e| e.to_string())?,
                "trust.json",
            ),
        })
    }
    pub fn get(&self, cwd: &str) -> Result<ProjectTrustDecision, String> {
        Ok(self.get_entry(cwd)?.map(|e| e.decision))
    }
    pub fn get_entry(&self, cwd: &str) -> Result<Option<ProjectTrustStoreEntry>, String> {
        let _lock = acquire_lock(&self.trust_path)?;
        let data = read_trust_file(&self.trust_path)?;
        let mut current = normalize_cwd(cwd)?;
        loop {
            if let Some(Value::Bool(decision)) = data.get(&current) {
                return Ok(Some(ProjectTrustStoreEntry {
                    path: current,
                    decision: *decision,
                }));
            }
            let Some(next) = parent(&current) else {
                return Ok(None);
            };
            current = next;
        }
    }
    pub fn set(&self, cwd: &str, decision: ProjectTrustDecision) -> Result<(), String> {
        self.set_many(&[ProjectTrustUpdate {
            path: cwd.into(),
            decision,
        }])
    }
    pub fn set_many(&self, decisions: &[ProjectTrustUpdate]) -> Result<(), String> {
        let _lock = acquire_lock(&self.trust_path)?;
        let mut data = read_trust_file(&self.trust_path)?;
        for update in decisions {
            let key = normalize_cwd(&update.path)?;
            match update.decision {
                Some(value) => {
                    data.insert(key, Value::Bool(value));
                }
                None => {
                    data.shift_remove(&key);
                }
            }
        }
        write_trust_file(&self.trust_path, data)
    }
}

#[cfg(test)]
#[path = "trust_manager_tests.rs"]
mod tests;
