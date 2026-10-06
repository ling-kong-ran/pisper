//! Port of upstream `coding-agent/src/core/settings-manager.ts`: the layered
//! (global + project) settings store with deep merge, legacy migrations,
//! modified-field tracking (external edits to unmodified fields survive
//! saves), file locking, and the full typed accessor surface.
//!
//! Seams / disclosed substitutions:
//! - Settings documents are [`SettingsValue`] — a document-order JSON value
//!   (JS object semantics: first key position wins, last value wins) whose
//!   numbers are plain `f64`, so `applyOverrides` can carry the non-finite
//!   runtime values JS allows but JSON cannot (the compaction validation
//!   battery pins those error texts). Parsing reuses the tested
//!   [`super::model_config::OrderedValue`] deserializer; rendering is
//!   `JSON.stringify(value, null, 2)` (two-space indent, document order,
//!   non-finite numbers as `null`, `undefined`-valued keys dropped).
//! - Upstream queues file writes on a promise chain (`writeQueue`) and
//!   awaits it in `flush()`/`reload()`. The write path is fully synchronous
//!   (the lock is a directory protocol), so the port writes inside `save()`
//!   and `flush()` is a no-op kept for API parity; errors move from the
//!   async catch to the save path — the observable set (drainErrors after
//!   flush, file bytes after flush) is unchanged.
//! - Upstream accessors pass raw JSON values through unvalidated where the
//!   TS types allow it; the port returns the typed default for non-conforming
//!   values on the numeric/string pass-through getters (e.g. a string
//!   `editorPaddingX` would flow through upstream and fail downstream), and
//!   numeric getters return integers (fractional values truncate). Validated
//!   getters (`getTuiMode`, `getTreeFilterMode`, `getMermaidRenderingMode`,
//!   the compaction tokens, the timeout settings) mirror upstream exactly.
//! - The sync file lock is the `<path>.lock` directory protocol (see
//!   [`super::auth_storage`]'s module docs); settings storage additionally
//!   only locks/creates when the file exists or a write is pending, exactly
//!   like upstream `FileSettingsStorage.withLock` (reading a missing project
//!   file never creates `.pi`).
//! - `getDocsPath`-style config seams live in their own modules
//!   ([`super::auth_guidance`]); `getAgentDir`/`CONFIG_DIR_NAME` are vendored
//!   in [`super`] (the W3.3 precedent).
//! - V8 `JSON.parse` error texts differ from serde_json's; the recorded
//!   [`SettingsError`] carries the serde wording (parse failures are pinned
//!   by scope + path, not message, in the oracle).

use std::sync::{Arc, Mutex};

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

use crate::ai::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS;
use crate::ai::types::primitives::{ThinkingLevel, Transport};
use crate::coding_agent::core::get_agent_dir;
use crate::coding_agent::core::path_join;
use crate::coding_agent::core::CONFIG_DIR_NAME;
use crate::coding_agent::utils::paths::{normalize_path, resolve_path_auto_base};
use crate::coding_agent::utils::text::strip_bom;

// ---------------------------------------------------------------------------
// SettingsValue: document-order JSON with JS number semantics
// ---------------------------------------------------------------------------

/// `serde_json::Value` with object key order preserved and `f64` numbers, so
/// the `applyOverrides` surface can express the non-finite runtime values JS
/// carries but JSON cannot (rendered as `null` by `JSON.stringify`).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SettingsValue {
    #[default]
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<SettingsValue>),
    Obj(Vec<(String, SettingsValue)>),
}

impl SettingsValue {
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Obj(_))
    }

    /// Upstream `isMergeableObject`: plain objects only (arrays excluded).
    fn is_mergeable(&self) -> bool {
        matches!(self, Self::Obj(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Num(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|value| value as i64)
    }

    pub fn as_array(&self) -> Option<&Vec<SettingsValue>> {
        match self {
            Self::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&SettingsValue> {
        match self {
            Self::Obj(entries) => entries
                .iter()
                .find(|(existing, _)| existing == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut SettingsValue> {
        match self {
            Self::Obj(entries) => entries
                .iter_mut()
                .find(|(existing, _)| existing == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Path lookup through nested objects (`get_in(&["retry", "provider"])`).
    pub fn get_in(&self, path: &[&str]) -> Option<&SettingsValue> {
        let mut current = self;
        for key in path {
            current = current.get(key)?;
        }
        Some(current)
    }

    /// JS assignment `object[key] = value`: update in place or append.
    pub fn set(&mut self, key: &str, value: SettingsValue) {
        if let Self::Obj(entries) = self {
            match entries.iter_mut().find(|(existing, _)| existing == key) {
                Some(slot) => slot.1 = value,
                None => entries.push((key.to_string(), value)),
            }
        }
    }

    /// JS `delete object[key]`.
    pub fn remove(&mut self, key: &str) {
        if let Self::Obj(entries) = self {
            entries.retain(|(existing, _)| existing != key);
        }
    }

    /// JS assignment where the value may be `undefined` (unrepresentable
    /// here): assigning `undefined` keeps the key in the JS object but
    /// `JSON.stringify` drops it — the port mirrors the observable result by
    /// removing the key.
    pub fn set_or_remove(&mut self, key: &str, value: Option<SettingsValue>) {
        match value {
            Some(value) => self.set(key, value),
            None => self.remove(key),
        }
    }

    pub fn obj(entries: Vec<(&str, SettingsValue)>) -> Self {
        Self::Obj(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    pub fn str(value: &str) -> Self {
        Self::Str(value.to_string())
    }

    pub fn num(value: f64) -> Self {
        Self::Num(value)
    }
}

impl serde::Serialize for SettingsValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            SettingsValue::Null => serializer.serialize_unit(),
            SettingsValue::Bool(value) => serializer.serialize_bool(*value),
            // Non-finite numbers serialize as `null` (JSON.stringify rule).
            SettingsValue::Num(number) => match serde_json::Number::from_f64(*number) {
                Some(number) => serializer.serialize_some(&number),
                None => serializer.serialize_none(),
            },
            SettingsValue::Str(value) => serializer.serialize_str(value),
            SettingsValue::Arr(items) => items.serialize(serializer),
            SettingsValue::Obj(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SettingsValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SettingsValueVisitor;
        impl<'de> Visitor<'de> for SettingsValueVisitor {
            type Value = SettingsValue;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(SettingsValue::Bool(value))
            }
            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(SettingsValue::Num(value as f64))
            }
            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(SettingsValue::Num(value as f64))
            }
            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(SettingsValue::Num(value))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(SettingsValue::Str(value.to_string()))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(SettingsValue::Null)
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(SettingsValue::Null)
            }
            fn visit_some<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                SettingsValue::deserialize(deserializer)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = access.next_element::<SettingsValue>()? {
                    items.push(item);
                }
                Ok(SettingsValue::Arr(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, SettingsValue)> = Vec::new();
                while let Some((key, value)) = access.next_entry::<String, SettingsValue>()? {
                    match entries.iter_mut().find(|(existing, _)| *existing == key) {
                        // JS JSON.parse semantics: first position, last value.
                        Some(slot) => slot.1 = value,
                        None => entries.push((key, value)),
                    }
                }
                Ok(SettingsValue::Obj(entries))
            }
        }
        deserializer.deserialize_any(SettingsValueVisitor)
    }
}

/// Parse a settings document (already BOM-stripped by the caller when it came
/// from a file). Test helper mirroring `JSON.parse`.
#[cfg(test)]
pub(crate) fn parse_settings_value(source: &str) -> Result<SettingsValue, String> {
    serde_json::from_str(source).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn stringify_pretty_for_tests(value: &SettingsValue) -> String {
    stringify_pretty(value)
}

#[cfg(test)]
pub(crate) fn js_to_string_for_tests(value: &SettingsValue) -> String {
    js_to_string(value)
}

#[cfg(test)]
pub(crate) fn js_number_to_string_for_tests(number: f64) -> String {
    js_number_to_string(number)
}

#[cfg(test)]
pub(crate) fn settings_value_to_serde_for_tests(value: &SettingsValue) -> serde_json::Value {
    settings_value_to_serde(value)
}

/// `String(value)` for the pinned validation error texts: numbers render like
/// JS (`NaN`, `Infinity`, positional integers), objects as
/// `[object Object]`, arrays as their elements joined by commas.
fn js_to_string(value: &SettingsValue) -> String {
    match value {
        SettingsValue::Null => "null".to_string(),
        SettingsValue::Bool(true) => "true".to_string(),
        SettingsValue::Bool(false) => "false".to_string(),
        SettingsValue::Num(number) => js_number_to_string(*number),
        SettingsValue::Str(value) => value.clone(),
        SettingsValue::Obj(_) => "[object Object]".to_string(),
        SettingsValue::Arr(items) => items
            .iter()
            .map(|item| match item {
                // JS Array::toString skips null/undefined elements.
                SettingsValue::Null => String::new(),
                other => js_to_string(other),
            })
            .collect::<Vec<String>>()
            .join(","),
    }
}

/// JS `Number::toString` for the value range the tests pin: integers below
/// 2^53 stay positional, non-integers use the shortest round-trip form, and
/// the non-finite constants render by name. (Exponential thresholds — JS
/// switches to exponent notation at ±1e21 / below 1e-6 — differ from Rust's
/// formatter for exotic magnitudes; disclosed, unpinned.)
fn js_number_to_string(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number.is_infinite() {
        return if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_string();
    }
    if number == 0.0 {
        return "0".to_string();
    }
    if number.trunc() == number && number.abs() < 1e21 {
        return format!("{number:.0}");
    }
    format!("{number}")
}

/// `JSON.stringify(value, null, 2)`: two-space indent, document order,
/// non-finite numbers as `null`, empty containers as `{}`/`[]`.
fn stringify_pretty(value: &SettingsValue) -> String {
    let mut out = String::new();
    write_pretty(value, 0, &mut out);
    out
}

fn write_pretty(value: &SettingsValue, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let inner_pad = "  ".repeat(indent + 1);
    match value {
        SettingsValue::Null => out.push_str("null"),
        SettingsValue::Bool(true) => out.push_str("true"),
        SettingsValue::Bool(false) => out.push_str("false"),
        // `JSON.stringify` renders non-finite numbers as `null`.
        SettingsValue::Num(number) if number.is_finite() => {
            out.push_str(&js_number_to_string(*number))
        }
        SettingsValue::Num(_) => out.push_str("null"),
        SettingsValue::Str(text) => {
            let json = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
            out.push_str(&json);
        }
        SettingsValue::Arr(items) if items.is_empty() => out.push_str("[]"),
        SettingsValue::Arr(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner_pad);
                write_pretty(item, indent + 1, out);
            }
            out.push('\n');
            out.push_str(&pad);
            out.push(']');
        }
        SettingsValue::Obj(entries) if entries.is_empty() => out.push_str("{}"),
        SettingsValue::Obj(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner_pad);
                let key_json = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                out.push_str(&key_json);
                out.push_str(": ");
                write_pretty(item, indent + 1, out);
            }
            out.push('\n');
            out.push_str(&pad);
            out.push('}');
        }
    }
}

fn settings_from_json(source: &str) -> Result<SettingsValue, String> {
    serde_json::from_str(source).map_err(|error| error.to_string())
}

/// Upstream `deepMergeObjects`: base key order preserved, overrides applied
/// in their own order, plain-object pairs merged recursively. (Upstream skips
/// `undefined` override values — unrepresentable in [`SettingsValue`], which
/// has no undefined.)
fn deep_merge_objects(base: &SettingsValue, overrides: &SettingsValue) -> SettingsValue {
    let mut result = base.clone();
    if let (SettingsValue::Obj(result_entries), SettingsValue::Obj(override_entries)) =
        (&mut result, overrides)
    {
        for (key, override_value) in override_entries {
            let merged = match result_entries
                .iter()
                .find(|(existing, _)| existing == key)
                .map(|(_, value)| value)
            {
                Some(base_value) if base_value.is_mergeable() && override_value.is_mergeable() => {
                    deep_merge_objects(base_value, override_value)
                }
                _ => override_value.clone(),
            };
            match result_entries
                .iter_mut()
                .find(|(existing, _)| existing == key)
            {
                Some(slot) => slot.1 = merged,
                None => result_entries.push((key.clone(), merged)),
            }
        }
    }
    result
}

/// Upstream `deepMergeSettings`: project/overrides take precedence.
fn deep_merge_settings(base: &SettingsValue, overrides: &SettingsValue) -> SettingsValue {
    deep_merge_objects(base, overrides)
}

// ---------------------------------------------------------------------------
// Migrations (upstream `migrateSettings`, in-place JS semantics)
// ---------------------------------------------------------------------------

/// Migrate old settings formats to the current one, preserving JS assignment
/// key-order semantics (assignments update in place or append; deletes
/// remove).
fn migrate_settings(settings: &mut SettingsValue) {
    // queueMode -> steeringMode
    let queue_mode = settings.get("queueMode").cloned();
    if let Some(queue_mode) = queue_mode {
        if settings.get("steeringMode").is_none() {
            settings.set("steeringMode", queue_mode);
        }
        settings.remove("queueMode");
    }

    // legacy websockets boolean -> transport enum
    if settings.get("transport").is_none() {
        let websockets = settings.get("websockets").and_then(SettingsValue::as_bool);
        if let Some(websockets) = websockets {
            settings.set(
                "transport",
                SettingsValue::str(if websockets { "websocket" } else { "sse" }),
            );
            settings.remove("websockets");
        }
    }

    // legacy skills object format -> array format
    let skills_is_object = matches!(settings.get("skills"), Some(value) if value.is_object());
    if skills_is_object {
        let skills = settings.get("skills").cloned().expect("checked above");
        let enable = skills.get("enableSkillCommands").cloned();
        if let Some(enable) = enable {
            if settings.get("enableSkillCommands").is_none() {
                settings.set("enableSkillCommands", enable);
            }
        }
        let custom = skills.get("customDirectories").cloned();
        let custom_is_array =
            matches!(&custom, Some(SettingsValue::Arr(items)) if !items.is_empty());
        if custom_is_array {
            settings.set("skills", custom.expect("checked above"));
        } else {
            settings.remove("skills");
        }
    }

    // retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    let retry_is_object = matches!(settings.get("retry"), Some(value) if value.is_object());
    if retry_is_object {
        let mut retry = settings.get("retry").cloned().expect("checked above");
        let max_delay_ms = retry.get("maxDelayMs").and_then(SettingsValue::as_f64);
        if let Some(max_delay_ms) = max_delay_ms {
            let provider = retry.get("provider");
            let provider_is_object = matches!(provider, Some(value) if value.is_object());
            let provider_missing_retry_delay = match provider {
                None => true,
                Some(value) => {
                    matches!(
                        value.get("maxRetryDelayMs"),
                        None | Some(SettingsValue::Null)
                    )
                }
            };
            if provider_missing_retry_delay {
                let base = if provider_is_object {
                    provider.cloned().expect("checked above")
                } else {
                    SettingsValue::Obj(Vec::new())
                };
                let mut merged_provider = base;
                merged_provider.set("maxRetryDelayMs", SettingsValue::Num(max_delay_ms));
                retry.set("provider", merged_provider);
            }
        }
        retry.remove("maxDelayMs");
        settings.set("retry", retry);
    }
}

// ---------------------------------------------------------------------------
// Storage backends
// ---------------------------------------------------------------------------

/// Upstream `SettingsScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsScope {
    Global,
    Project,
}

/// Upstream `SettingsError`: the scope, optional file path, and message (the
/// port carries serde_json wording for parse failures — see module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub scope: SettingsScope,
    pub path: Option<String>,
    pub error: String,
}

/// Callback mirror of upstream `withLock(scope, fn)`: `fn(current)` returning
/// `next` (write) or `None` (no write); `Err` throws upstream.
pub type SettingsLockCallback<'a> =
    Box<dyn FnOnce(Option<String>) -> Result<Option<String>, String> + 'a>;

/// Upstream `SettingsStorage`.
pub trait SettingsStorage: Send + Sync {
    fn with_lock(&self, scope: SettingsScope, f: SettingsLockCallback<'_>) -> Result<(), String>;
}

/// Sync lock handle (proper-lockfile protocol): removing `<path>.lock`
/// releases.
struct SettingsLockHandle {
    lock_path: String,
}

impl SettingsLockHandle {
    fn release(self) {
        let _ = std::fs::remove_dir(&self.lock_path);
    }
}

/// Upstream `acquireLockSyncWithRetry` (identical schedule to the auth
/// storage variant).
fn acquire_lock_sync_with_retry(path: &str) -> Result<SettingsLockHandle, String> {
    const MAX_ATTEMPTS: usize = 10;
    const DELAY_MS: u64 = 20;
    let lock_path = format!("{path}.lock");
    for attempt in 1..=MAX_ATTEMPTS {
        match std::fs::create_dir(&lock_path) {
            Ok(()) => return Ok(SettingsLockHandle { lock_path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if attempt == MAX_ATTEMPTS {
                    return Err(format!("ELOCKED: resource is locked: {lock_path}"));
                }
                let start = std::time::Instant::now();
                while start.elapsed() < std::time::Duration::from_millis(DELAY_MS) {
                    std::hint::spin_loop();
                }
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Failed to acquire settings lock".to_string())
}

/// Upstream `FileSettingsStorage`: global `<agentDir>/settings.json`, project
/// `<cwd>/.pi/settings.json` (resolved paths). Locks only when the file
/// exists or a write is pending; reading a missing file never creates the
/// directory.
pub struct FileSettingsStorage {
    global_settings_path: String,
    project_settings_path: String,
}

impl FileSettingsStorage {
    /// Upstream `new FileSettingsStorage(cwd, agentDir)`.
    pub fn new(
        cwd: &str,
        agent_dir: &str,
    ) -> Result<Self, crate::coding_agent::utils::paths::PathError> {
        let resolved_cwd = resolve_path_auto_base(cwd)?;
        let resolved_agent_dir = resolve_path_auto_base(agent_dir)?;
        Ok(Self {
            global_settings_path: path_join(&resolved_agent_dir, "settings.json"),
            project_settings_path: path_join(
                &resolved_cwd,
                &format!("{CONFIG_DIR_NAME}/settings.json"),
            ),
        })
    }

    fn path_for(&self, scope: SettingsScope) -> &str {
        match scope {
            SettingsScope::Global => &self.global_settings_path,
            SettingsScope::Project => &self.project_settings_path,
        }
    }
}

impl SettingsStorage for FileSettingsStorage {
    fn with_lock(&self, scope: SettingsScope, f: SettingsLockCallback<'_>) -> Result<(), String> {
        let path = self.path_for(scope).to_string();
        let dir = std::path::Path::new(&path)
            .parent()
            .map(|parent| parent.to_path_buf());

        let mut release: Option<SettingsLockHandle> = None;
        let result = (|| {
            // Only create directory and lock if file exists or we need to
            // write.
            let file_exists = std::path::Path::new(&path).exists();
            if file_exists {
                release = Some(acquire_lock_sync_with_retry(&path)?);
            }
            let current = if file_exists {
                std::fs::read_to_string(&path).ok()
            } else {
                None
            };
            let next = f(current)?;
            if let Some(next) = next {
                // Only create the directory when we actually need to write.
                if let Some(dir) = &dir {
                    if !dir.as_os_str().is_empty() && !dir.exists() {
                        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                    }
                }
                if release.is_none() {
                    release = Some(acquire_lock_sync_with_retry(&path)?);
                }
                std::fs::write(&path, next).map_err(|error| error.to_string())?;
            }
            Ok(())
        })();

        if let Some(handle) = release {
            handle.release();
        }
        result
    }
}

/// Upstream `InMemorySettingsStorage`.
#[derive(Default)]
pub struct InMemorySettingsStorage {
    slots: std::sync::Mutex<(Option<String>, Option<String>)>,
}

impl SettingsStorage for InMemorySettingsStorage {
    fn with_lock(&self, scope: SettingsScope, f: SettingsLockCallback<'_>) -> Result<(), String> {
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = match scope {
            SettingsScope::Global => slots.0.clone(),
            SettingsScope::Project => slots.1.clone(),
        };
        let next = f(current)?;
        if let Some(next) = next {
            match scope {
                SettingsScope::Global => slots.0 = Some(next),
                SettingsScope::Project => slots.1 = Some(next),
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Accessor value types
// ---------------------------------------------------------------------------

/// Upstream `TuiMode` (the renderer union reduced to the values the settings
/// surface validates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    Regular,
    Fullscreen,
}

/// Upstream `FullscreenExitOutput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenExitOutput {
    Transcript,
    ResumeHint,
}

/// Upstream `ScrollViewScrollbar` (the values the settings surface accepts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenScrollbar {
    Auto,
    Always,
    Hidden,
}

/// Upstream `MermaidRenderingMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MermaidRenderingMode {
    Off,
    Final,
    Streaming,
}

/// Upstream `DefaultProjectTrust`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultProjectTrust {
    Ask,
    Always,
    Never,
}

/// Upstream `QuietStartup` (v1.0.0): `true` hides all startup output,
/// `"header"` keeps only the startup header, `false` shows everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietStartup {
    /// Upstream `false` (the default): startup header and details.
    Off,
    /// Upstream `"header"`: only the startup header.
    Header,
    /// Upstream `true`: no startup output.
    Full,
}

/// Upstream `TreeFilterMode` (whitelist-validated on read).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeFilterMode {
    Default,
    NoTools,
    UserOnly,
    LabeledOnly,
    All,
}

/// The image-capability entry of the terminal capability overrides
/// (`"kitty"`/`"iterm2"` set the capability, `false` clears it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalImagesOverride {
    Kitty,
    Iterm2,
    Cleared,
}

/// Upstream `getTerminalCapabilityOverrides`' `Partial<TerminalCapabilities>`
/// result (pi-tui's type is outside the slice; the shape mirrors the partial:
/// absent keys are not overridden).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalCapabilityOverrides {
    pub images: Option<TerminalImagesOverride>,
    pub true_color: Option<bool>,
    pub hyperlinks: Option<bool>,
}

/// Upstream `getCompactionSettings` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: i64,
    pub keep_recent_tokens: i64,
}

/// Upstream `getBranchSummarySettings` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummarySettings {
    pub reserve_tokens: i64,
    pub skip_prompt: bool,
}

/// Upstream `getRetrySettings` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySettings {
    pub enabled: bool,
    pub max_retries: i64,
    pub base_delay_ms: i64,
    pub max_agent_delay_ms: i64,
}

/// Upstream `getProviderRetrySettings` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRetrySettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<i64>,
    pub max_retry_delay_ms: i64,
}

impl TerminalCapabilityOverrides {
    /// The upstream partial spreads keys conditionally (`images: null`
    /// clears, `auto` values are omitted); this reproduces the object shape
    /// the oracle pins.
    pub fn to_json_value(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        match self.images {
            Some(TerminalImagesOverride::Kitty) => {
                map.insert(
                    "images".to_string(),
                    serde_json::Value::String("kitty".to_string()),
                );
            }
            Some(TerminalImagesOverride::Iterm2) => {
                map.insert(
                    "images".to_string(),
                    serde_json::Value::String("iterm2".to_string()),
                );
            }
            Some(TerminalImagesOverride::Cleared) => {
                map.insert("images".to_string(), serde_json::Value::Null);
            }
            None => {}
        }
        if let Some(value) = self.true_color {
            map.insert("trueColor".to_string(), serde_json::Value::Bool(value));
        }
        if let Some(value) = self.hyperlinks {
            map.insert("hyperlinks".to_string(), serde_json::Value::Bool(value));
        }
        serde_json::Value::Object(map)
    }
}

/// Upstream `SettingsManagerCreateOptions`.
#[derive(Debug, Clone, Copy)]
pub struct SettingsManagerCreateOptions {
    pub project_trusted: bool,
}

impl Default for SettingsManagerCreateOptions {
    fn default() -> Self {
        Self {
            project_trusted: true,
        }
    }
}

/// Upstream `DEFAULT_COMPACTION_TOKEN_SETTINGS`.
const DEFAULT_COMPACTION_RESERVE_TOKENS: f64 = 16_384.0;
const DEFAULT_COMPACTION_KEEP_RECENT_TOKENS: f64 = 20_000.0;

fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value && value.abs() <= 9_007_199_254_740_991.0
}

/// Insertion-ordered set (upstream `Set` iteration order drives the write
/// merge order, which is byte-observable in the rewritten files).
#[derive(Default)]
struct OrderedSet {
    items: Vec<String>,
}

impl OrderedSet {
    fn insert(&mut self, value: &str) {
        if !self.items.iter().any(|existing| existing == value) {
            self.items.push(value.to_string());
        }
    }

    fn clear(&mut self) {
        self.items.clear();
    }

    fn iter(&self) -> impl Iterator<Item = &String> {
        self.items.iter()
    }
}

#[derive(Default)]
struct ModifiedNestedFields {
    entries: Vec<(String, OrderedSet)>,
}

impl ModifiedNestedFields {
    fn mark(&mut self, field: &str, nested_key: &str) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == field)
        {
            Some((_, nested)) => nested.insert(nested_key),
            None => {
                let mut nested = OrderedSet::default();
                nested.insert(nested_key);
                self.entries.push((field.to_string(), nested));
            }
        }
    }

    fn has(&self, field: &str) -> bool {
        self.entries.iter().any(|(existing, _)| existing == field)
    }

    fn get(&self, field: &str) -> Option<&OrderedSet> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == field)
            .map(|(_, nested)| nested)
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Debug, Default, Clone)]
struct SettingsPaths {
    global: Option<String>,
    project: Option<String>,
}

struct Inner {
    storage: Arc<dyn SettingsStorage>,
    global_settings: SettingsValue,
    project_settings: SettingsValue,
    settings: SettingsValue,
    project_trusted: bool,
    modified_fields: OrderedSet,
    modified_nested_fields: ModifiedNestedFields,
    modified_project_fields: OrderedSet,
    modified_project_nested_fields: ModifiedNestedFields,
    global_settings_load_error: Option<String>,
    project_settings_load_error: Option<String>,
    errors: Vec<SettingsError>,
    settings_paths: SettingsPaths,
}

/// Upstream `SettingsManager`. Native clones share the same settings, storage,
/// pending modifications and error queue (JS object identity, not a snapshot).
#[derive(Clone)]
pub struct SettingsManager {
    inner: Arc<Mutex<Inner>>,
}

impl SettingsManager {
    /// Upstream `SettingsManager.create(cwd, agentDir?, options?)` — the
    /// agent dir defaults to [`get_agent_dir`].
    pub fn create(cwd: &str) -> Result<Self, crate::coding_agent::utils::paths::PathError> {
        Self::create_with(
            cwd,
            &get_agent_dir(),
            SettingsManagerCreateOptions::default(),
        )
    }

    /// Upstream `create` with explicit agent dir/options.
    pub fn create_with(
        cwd: &str,
        agent_dir: &str,
        options: SettingsManagerCreateOptions,
    ) -> Result<Self, crate::coding_agent::utils::paths::PathError> {
        let resolved_cwd = resolve_path_auto_base(cwd)?;
        let resolved_agent_dir = resolve_path_auto_base(agent_dir)?;
        let storage = FileSettingsStorage::new(&resolved_cwd, &resolved_agent_dir)?;
        let settings_paths = SettingsPaths {
            global: Some(path_join(&resolved_agent_dir, "settings.json")),
            project: Some(path_join(
                &resolved_cwd,
                &format!("{CONFIG_DIR_NAME}/settings.json"),
            )),
        };
        Ok(Self::from_storage_with_paths(
            Arc::new(storage),
            options,
            settings_paths,
        ))
    }

    /// Upstream `SettingsManager.fromStorage(storage, options?)`.
    pub fn from_storage(
        storage: Arc<dyn SettingsStorage>,
        options: SettingsManagerCreateOptions,
    ) -> Self {
        Self::from_storage_with_paths(storage, options, SettingsPaths::default())
    }

    fn from_storage_with_paths(
        storage: Arc<dyn SettingsStorage>,
        options: SettingsManagerCreateOptions,
        settings_paths: SettingsPaths,
    ) -> Self {
        let project_trusted = options.project_trusted;
        let global_load =
            Self::try_load_from_storage(storage.as_ref(), SettingsScope::Global, true);
        let project_load =
            Self::try_load_from_storage(storage.as_ref(), SettingsScope::Project, project_trusted);
        let mut initial_errors = Vec::new();
        if let Some(error) = &global_load.error {
            initial_errors.push(to_settings_error(
                SettingsScope::Global,
                error,
                settings_paths.global.clone(),
            ));
        }
        if let Some(error) = &project_load.error {
            initial_errors.push(to_settings_error(
                SettingsScope::Project,
                error,
                settings_paths.project.clone(),
            ));
        }

        let global_settings = global_load.settings;
        let project_settings = project_load.settings;
        let settings = deep_merge_settings(&global_settings, &project_settings);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                storage,
                global_settings,
                project_settings,
                settings,
                project_trusted,
                modified_fields: OrderedSet::default(),
                modified_nested_fields: ModifiedNestedFields::default(),
                modified_project_fields: OrderedSet::default(),
                modified_project_nested_fields: ModifiedNestedFields::default(),
                global_settings_load_error: global_load.error,
                project_settings_load_error: project_load.error,
                errors: initial_errors,
                settings_paths,
            })),
        }
    }

    /// Upstream `SettingsManager.inMemory(settings?, options?)`.
    pub fn in_memory(settings: SettingsValue) -> Self {
        Self::in_memory_with_options(settings, SettingsManagerCreateOptions::default())
    }

    pub fn in_memory_with_options(
        settings: SettingsValue,
        options: SettingsManagerCreateOptions,
    ) -> Self {
        let storage = InMemorySettingsStorage::default();
        let mut initial_settings = settings;
        migrate_settings(&mut initial_settings);
        let document = stringify_pretty(&initial_settings);
        storage
            .with_lock(SettingsScope::Global, Box::new(|_| Ok(Some(document))))
            .expect("in-memory seed cannot fail");
        Self::from_storage(Arc::new(storage), options)
    }

    /// Upstream `loadFromStorage`.
    fn load_from_storage(
        storage: &dyn SettingsStorage,
        scope: SettingsScope,
        project_trusted: bool,
    ) -> Result<SettingsValue, String> {
        if scope == SettingsScope::Project && !project_trusted {
            return Ok(SettingsValue::Obj(Vec::new()));
        }

        let mut content: Option<String> = None;
        storage.with_lock(
            scope,
            Box::new(|current| {
                content = current;
                Ok(None)
            }),
        )?;

        let Some(content) = content.filter(|content| !content.is_empty()) else {
            return Ok(SettingsValue::Obj(Vec::new()));
        };
        let mut settings = settings_from_json(strip_bom(&content))?;
        migrate_settings(&mut settings);
        Ok(settings)
    }

    /// Upstream `tryLoadFromStorage`.
    fn try_load_from_storage(
        storage: &dyn SettingsStorage,
        scope: SettingsScope,
        project_trusted: bool,
    ) -> LoadedSettings {
        match Self::load_from_storage(storage, scope, project_trusted) {
            Ok(settings) => LoadedSettings {
                settings,
                error: None,
            },
            Err(error) => LoadedSettings {
                settings: SettingsValue::Obj(Vec::new()),
                error: Some(error),
            },
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // -- scope / trust management -------------------------------------------

    /// Upstream `getGlobalSettings()`.
    pub fn get_global_settings(&self) -> SettingsValue {
        self.lock().global_settings.clone()
    }

    /// Upstream `getProjectSettings()`.
    pub fn get_project_settings(&self) -> SettingsValue {
        self.lock().project_settings.clone()
    }

    /// Upstream `isProjectTrusted()`.
    pub fn is_project_trusted(&self) -> bool {
        self.lock().project_trusted
    }

    /// Upstream `setProjectTrusted(trusted)`.
    pub fn set_project_trusted(&self, trusted: bool) {
        let mut inner = self.lock();
        if inner.project_trusted == trusted {
            return;
        }
        inner.project_trusted = trusted;
        inner.modified_project_fields.clear();
        inner.modified_project_nested_fields.clear();

        if !trusted {
            inner.project_settings = SettingsValue::Obj(Vec::new());
            inner.project_settings_load_error = None;
            inner.settings = deep_merge_settings(&inner.global_settings, &inner.project_settings);
            return;
        }

        let project_load =
            Self::try_load_from_storage(inner.storage.as_ref(), SettingsScope::Project, trusted);
        inner.project_settings = project_load.settings;
        inner.project_settings_load_error = project_load.error.clone();
        if let Some(error) = project_load.error {
            let paths = inner.settings_paths.project.clone();
            inner
                .errors
                .push(to_settings_error(SettingsScope::Project, &error, paths));
        }
        inner.settings = deep_merge_settings(&inner.global_settings, &inner.project_settings);
    }

    /// Upstream `reload()`. The write queue is synchronous in the port (see
    /// module docs), so no await remains.
    pub fn reload(&self) {
        let mut inner = self.lock();
        let global_load =
            Self::try_load_from_storage(inner.storage.as_ref(), SettingsScope::Global, true);
        match &global_load.error {
            None => {
                inner.global_settings = global_load.settings;
                inner.global_settings_load_error = None;
            }
            Some(error) => {
                inner.global_settings_load_error = Some(error.clone());
                let paths = inner.settings_paths.global.clone();
                inner
                    .errors
                    .push(to_settings_error(SettingsScope::Global, error, paths));
            }
        }

        inner.modified_fields.clear();
        inner.modified_nested_fields.clear();
        inner.modified_project_fields.clear();
        inner.modified_project_nested_fields.clear();

        let project_load = Self::try_load_from_storage(
            inner.storage.as_ref(),
            SettingsScope::Project,
            inner.project_trusted,
        );
        match &project_load.error {
            None => {
                inner.project_settings = project_load.settings;
                inner.project_settings_load_error = None;
            }
            Some(error) => {
                inner.project_settings_load_error = Some(error.clone());
                let paths = inner.settings_paths.project.clone();
                inner
                    .errors
                    .push(to_settings_error(SettingsScope::Project, error, paths));
            }
        }

        inner.settings = deep_merge_settings(&inner.global_settings, &inner.project_settings);
    }

    /// Upstream `applyOverrides(overrides)`.
    pub fn apply_overrides(&self, overrides: &SettingsValue) {
        let mut inner = self.lock();
        inner.settings = deep_merge_settings(&inner.settings, overrides);
    }

    /// Upstream `flush()`. Writes are synchronous in the port; kept for API
    /// parity.
    pub fn flush(&self) {}

    /// Upstream `drainErrors()`.
    pub fn drain_errors(&self) -> Vec<SettingsError> {
        let mut inner = self.lock();
        std::mem::take(&mut inner.errors)
    }

    // -- save plumbing -------------------------------------------------------

    fn assert_project_trusted_for_write(inner: &Inner) -> Result<(), String> {
        if !inner.project_trusted {
            return Err("Project is not trusted; refusing to write project settings".to_string());
        }
        Ok(())
    }

    /// Upstream `enqueueWrite` (synchronous — see module docs): run the
    /// write, record failures, and only clear the modified tracking on
    /// success.
    fn enqueue_write(
        inner: &mut Inner,
        scope: SettingsScope,
        snapshot: &SettingsValue,
        modified_fields: &[String],
        modified_nested: &ModifiedNestedFields,
    ) {
        if scope == SettingsScope::Project {
            if let Err(error) = Self::assert_project_trusted_for_write(inner) {
                let paths = inner.settings_paths.project.clone();
                inner.errors.push(to_settings_error(scope, &error, paths));
                return;
            }
        }
        let write =
            Self::persist_scoped_settings(inner, scope, snapshot, modified_fields, modified_nested);
        match write {
            Ok(()) => match scope {
                SettingsScope::Global => {
                    inner.modified_fields.clear();
                    inner.modified_nested_fields.clear();
                }
                SettingsScope::Project => {
                    inner.modified_project_fields.clear();
                    inner.modified_project_nested_fields.clear();
                }
            },
            Err(error) => {
                let paths = match scope {
                    SettingsScope::Global => inner.settings_paths.global.clone(),
                    SettingsScope::Project => inner.settings_paths.project.clone(),
                };
                inner.errors.push(to_settings_error(scope, &error, paths));
            }
        }
    }

    /// Upstream `persistScopedSettings`: re-read + re-migrate the current
    /// file, then merge only the session-modified fields (nested fields
    /// per-key) and render `JSON.stringify(merged, null, 2)`.
    fn persist_scoped_settings(
        inner: &Inner,
        scope: SettingsScope,
        snapshot: &SettingsValue,
        modified_fields: &[String],
        modified_nested: &ModifiedNestedFields,
    ) -> Result<(), String> {
        inner.storage.with_lock(
            scope,
            Box::new(|current| {
                let current_file_settings = match current {
                    Some(current) if !current.is_empty() => {
                        let mut parsed = settings_from_json(strip_bom(&current))?;
                        migrate_settings(&mut parsed);
                        parsed
                    }
                    _ => SettingsValue::Obj(Vec::new()),
                };
                let mut merged_settings = current_file_settings.clone();
                for field in modified_fields {
                    let value = snapshot.get(field);
                    if modified_nested.has(field) && matches!(value, Some(SettingsValue::Obj(_))) {
                        let value = value.expect("checked above");
                        let base_nested = current_file_settings
                            .get(field)
                            .cloned()
                            .unwrap_or(SettingsValue::Obj(Vec::new()));
                        let mut merged_nested = base_nested;
                        if let Some(nested_keys) = modified_nested.get(field) {
                            for nested_key in nested_keys.iter() {
                                // `mergedNested[key] = inMemoryNested[key]` — an
                                // absent key would stringify as dropped.
                                merged_nested
                                    .set_or_remove(nested_key, value.get(nested_key).cloned());
                            }
                        }
                        merged_settings.set(field, merged_nested);
                    } else {
                        merged_settings.set_or_remove(field, value.cloned());
                    }
                }
                Ok(Some(stringify_pretty(&merged_settings)))
            }),
        )
    }

    /// Upstream `save()`.
    fn save(inner: &mut Inner) {
        inner.settings = deep_merge_settings(&inner.global_settings, &inner.project_settings);
        if inner.global_settings_load_error.is_some() {
            return;
        }
        let snapshot = inner.global_settings.clone();
        let modified_fields: Vec<String> = inner.modified_fields.iter().cloned().collect();
        let modified_nested = clone_nested(&inner.modified_nested_fields);
        Self::enqueue_write(
            inner,
            SettingsScope::Global,
            &snapshot,
            &modified_fields,
            &modified_nested,
        );
    }

    /// Upstream `saveProjectSettings(settings)`.
    fn save_project_settings(inner: &mut Inner, settings: SettingsValue) -> Result<(), String> {
        Self::assert_project_trusted_for_write(inner)?;
        inner.project_settings = settings;
        inner.settings = deep_merge_settings(&inner.global_settings, &inner.project_settings);
        if inner.project_settings_load_error.is_some() {
            return Ok(());
        }
        let snapshot = inner.project_settings.clone();
        let modified_fields: Vec<String> = inner.modified_project_fields.iter().cloned().collect();
        let modified_nested = clone_nested(&inner.modified_project_nested_fields);
        Self::enqueue_write(
            inner,
            SettingsScope::Project,
            &snapshot,
            &modified_fields,
            &modified_nested,
        );
        Ok(())
    }

    /// Upstream `updateProjectSettings(field, update)`.
    fn update_project_settings(
        &self,
        field: &str,
        update: impl FnOnce(&mut SettingsValue),
    ) -> Result<(), String> {
        let mut inner = self.lock();
        Self::assert_project_trusted_for_write(&inner)?;
        let mut project_settings = inner.project_settings.clone();
        update(&mut project_settings);
        inner.modified_project_fields.insert(field);
        Self::save_project_settings(&mut inner, project_settings)
    }

    // -- getters/setters -----------------------------------------------------

    pub fn get_last_changelog_version(&self) -> Option<String> {
        self.lock()
            .settings
            .get("lastChangelogVersion")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    pub fn set_last_changelog_version(&self, version: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("lastChangelogVersion", SettingsValue::str(version));
        inner.modified_fields.insert("lastChangelogVersion");
        Self::save(&mut inner);
    }

    /// Upstream `getSessionDir()` — `normalizePath`-expanded when set.
    pub fn get_session_dir(&self) -> Option<String> {
        let session_dir = self
            .lock()
            .settings
            .get("sessionDir")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)?;
        Some(normalize_path(&session_dir).unwrap_or(session_dir))
    }

    pub fn get_default_provider(&self) -> Option<String> {
        self.lock()
            .settings
            .get("defaultProvider")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    pub fn get_default_model(&self) -> Option<String> {
        self.lock()
            .settings
            .get("defaultModel")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    pub fn set_default_provider(&self, provider: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("defaultProvider", SettingsValue::str(provider));
        inner.modified_fields.insert("defaultProvider");
        Self::save(&mut inner);
    }

    pub fn set_default_model(&self, model_id: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("defaultModel", SettingsValue::str(model_id));
        inner.modified_fields.insert("defaultModel");
        Self::save(&mut inner);
    }

    pub fn set_default_model_and_provider(&self, provider: &str, model_id: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("defaultProvider", SettingsValue::str(provider));
        inner
            .global_settings
            .set("defaultModel", SettingsValue::str(model_id));
        inner.modified_fields.insert("defaultProvider");
        inner.modified_fields.insert("defaultModel");
        Self::save(&mut inner);
    }

    /// Upstream `getSteeringMode()` (`|| "one-at-a-time"`).
    pub fn get_steering_mode(&self) -> String {
        let inner = self.lock();
        inner
            .settings
            .get("steeringMode")
            .and_then(SettingsValue::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("one-at-a-time")
            .to_string()
    }

    pub fn set_steering_mode(&self, mode: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("steeringMode", SettingsValue::str(mode));
        inner.modified_fields.insert("steeringMode");
        Self::save(&mut inner);
    }

    /// Upstream `getFollowUpMode()` (`|| "one-at-a-time"`).
    pub fn get_follow_up_mode(&self) -> String {
        let inner = self.lock();
        inner
            .settings
            .get("followUpMode")
            .and_then(SettingsValue::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("one-at-a-time")
            .to_string()
    }

    pub fn set_follow_up_mode(&self, mode: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("followUpMode", SettingsValue::str(mode));
        inner.modified_fields.insert("followUpMode");
        Self::save(&mut inner);
    }

    /// Upstream `getThemeSetting()` (strings only).
    pub fn get_theme_setting(&self) -> Option<String> {
        self.lock()
            .settings
            .get("theme")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    /// Upstream `getTheme()` — slash-separated automatic themes are not a
    /// fixed theme name.
    pub fn get_theme(&self) -> Option<String> {
        let theme = self.get_theme_setting()?;
        if theme.contains('/') {
            return None;
        }
        Some(theme)
    }

    pub fn set_theme(&self, theme: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("theme", SettingsValue::str(theme));
        inner.modified_fields.insert("theme");
        Self::save(&mut inner);
    }

    /// Upstream `getDefaultThinkingLevel()` — raw pass-through (the stored
    /// value is whatever the file holds).
    pub fn get_default_thinking_level(&self) -> Option<String> {
        self.lock()
            .settings
            .get("defaultThinkingLevel")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    pub fn set_default_thinking_level(&self, level: ThinkingLevel) {
        let mut inner = self.lock();
        inner.global_settings.set(
            "defaultThinkingLevel",
            SettingsValue::str(&thinking_level_wire(level)),
        );
        inner.modified_fields.insert("defaultThinkingLevel");
        Self::save(&mut inner);
    }

    /// Upstream `getModelThinkingLevel(provider, modelId)` — raw.
    pub fn get_model_thinking_level(&self, provider: &str, model_id: &str) -> Option<String> {
        let key = format!("{provider}/{model_id}");
        self.lock()
            .settings
            .get_in(&["modelThinkingLevels", &key])
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    /// Upstream `getAllModelThinkingLevels()` — a copy of the map.
    pub fn get_all_model_thinking_levels(&self) -> Vec<(String, String)> {
        let inner = self.lock();
        match inner.settings.get("modelThinkingLevels") {
            Some(SettingsValue::Obj(entries)) => entries
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn set_model_thinking_level(&self, provider: &str, model_id: &str, level: ThinkingLevel) {
        let key = format!("{provider}/{model_id}");
        let mut inner = self.lock();
        let mut levels = inner
            .global_settings
            .get("modelThinkingLevels")
            .cloned()
            .unwrap_or(SettingsValue::Obj(Vec::new()));
        levels.set(&key, SettingsValue::str(&thinking_level_wire(level)));
        inner.global_settings.set("modelThinkingLevels", levels);
        inner.modified_fields.insert("modelThinkingLevels");
        Self::save(&mut inner);
    }

    pub fn remove_model_thinking_level(&self, provider: &str, model_id: &str) {
        let key = format!("{provider}/{model_id}");
        let mut inner = self.lock();
        let has_levels = inner.global_settings.get("modelThinkingLevels").is_some();
        if !has_levels {
            return;
        }
        let mut levels = inner
            .global_settings
            .get("modelThinkingLevels")
            .cloned()
            .expect("checked above");
        levels.remove(&key);
        if matches!(&levels, SettingsValue::Obj(entries) if entries.is_empty()) {
            inner.global_settings.remove("modelThinkingLevels");
        } else {
            inner.global_settings.set("modelThinkingLevels", levels);
        }
        inner.modified_fields.insert("modelThinkingLevels");
        Self::save(&mut inner);
    }

    /// Upstream `getTransport()` — raw pass-through with the `"auto"`
    /// default.
    pub fn get_transport(&self) -> String {
        let inner = self.lock();
        inner
            .settings
            .get("transport")
            .and_then(SettingsValue::as_str)
            .unwrap_or("auto")
            .to_string()
    }

    pub fn set_transport(&self, transport: Transport) {
        let wire = serde_json::to_value(transport)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .expect("transport serializes to a string");
        let mut inner = self.lock();
        inner
            .global_settings
            .set("transport", SettingsValue::Str(wire));
        inner.modified_fields.insert("transport");
        Self::save(&mut inner);
    }

    pub fn get_compaction_enabled(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["compaction", "enabled"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(true)
    }

    pub fn set_compaction_enabled(&self, enabled: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "compaction");
        if let Some(compaction) = inner.global_settings.get_mut("compaction") {
            compaction.set("enabled", SettingsValue::Bool(enabled));
        }
        inner.modified_nested_fields.mark("compaction", "enabled");
        inner.modified_fields.insert("compaction");
        Self::save(&mut inner);
    }

    /// Upstream `getCompactionTokenSetting` — validation order and error
    /// texts are oracle-pinned.
    fn get_compaction_token_setting(
        &self,
        field: &str,
        model: Option<(&str, &str)>,
    ) -> Result<i64, String> {
        let inner = self.lock();
        let compaction = inner.settings.get("compaction");
        let ordinary = compaction.and_then(|value| value.get(field));
        if let Some(value) = ordinary {
            let valid = matches!(value, SettingsValue::Num(number) if is_safe_integer(*number) && *number >= 0.0);
            if !valid {
                return Err(format!(
                    "Invalid compaction.{field} setting: {}. Expected a non-negative safe integer.",
                    js_to_string(value)
                ));
            }
        }

        let model_key = model.map(|(provider, id)| format!("{provider}/{id}"));
        let entry = match &model_key {
            Some(key) => compaction
                .and_then(|value| value.get("modelOverrides"))
                .and_then(|value| value.get(key)),
            None => None,
        };
        if let Some(entry) = entry {
            if !entry.is_object() {
                return Err(format!(
                    "Invalid compaction.modelOverrides[\"{}\"] setting: {}. Expected an object.",
                    model_key.expect("entry implies model key"),
                    js_to_string(entry)
                ));
            }
        }
        let override_value = entry.and_then(|value| value.get(field));
        if let Some(value) = override_value {
            let valid = matches!(value, SettingsValue::Num(number) if is_safe_integer(*number) && *number >= 0.0);
            if !valid {
                return Err(format!(
                    "Invalid compaction.modelOverrides[\"{}\"].{field} setting: {}. Expected a non-negative safe integer.",
                    model_key.expect("entry implies model key"),
                    js_to_string(value)
                ));
            }
        }

        let resolved = override_value
            .and_then(SettingsValue::as_f64)
            .or_else(|| ordinary.and_then(SettingsValue::as_f64))
            .unwrap_or(match field {
                "reserveTokens" => DEFAULT_COMPACTION_RESERVE_TOKENS,
                _ => DEFAULT_COMPACTION_KEEP_RECENT_TOKENS,
            });
        Ok(resolved as i64)
    }

    pub fn get_compaction_reserve_tokens(&self) -> Result<i64, String> {
        self.get_compaction_token_setting("reserveTokens", None)
    }

    pub fn get_compaction_reserve_tokens_for(
        &self,
        provider: &str,
        model_id: &str,
    ) -> Result<i64, String> {
        self.get_compaction_token_setting("reserveTokens", Some((provider, model_id)))
    }

    pub fn get_compaction_keep_recent_tokens(&self) -> Result<i64, String> {
        self.get_compaction_token_setting("keepRecentTokens", None)
    }

    pub fn get_compaction_keep_recent_tokens_for(
        &self,
        provider: &str,
        model_id: &str,
    ) -> Result<i64, String> {
        self.get_compaction_token_setting("keepRecentTokens", Some((provider, model_id)))
    }

    /// Upstream `getCompactionSettings(model?)`.
    pub fn get_compaction_settings(&self) -> Result<CompactionSettings, String> {
        self.compaction_settings_for(None)
    }

    pub fn get_compaction_settings_for(
        &self,
        provider: &str,
        model_id: &str,
    ) -> Result<CompactionSettings, String> {
        self.compaction_settings_for(Some((provider, model_id)))
    }

    fn compaction_settings_for(
        &self,
        model: Option<(&str, &str)>,
    ) -> Result<CompactionSettings, String> {
        Ok(CompactionSettings {
            enabled: self.get_compaction_enabled(),
            reserve_tokens: self.get_compaction_token_setting("reserveTokens", model)?,
            keep_recent_tokens: self.get_compaction_token_setting("keepRecentTokens", model)?,
        })
    }

    pub fn get_branch_summary_settings(&self) -> BranchSummarySettings {
        let inner = self.lock();
        BranchSummarySettings {
            reserve_tokens: inner
                .settings
                .get_in(&["branchSummary", "reserveTokens"])
                .and_then(SettingsValue::as_f64)
                .unwrap_or(16_384.0) as i64,
            skip_prompt: inner
                .settings
                .get_in(&["branchSummary", "skipPrompt"])
                .and_then(SettingsValue::as_bool)
                .unwrap_or(false),
        }
    }

    pub fn get_branch_summary_skip_prompt(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["branchSummary", "skipPrompt"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(false)
    }

    pub fn get_retry_enabled(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["retry", "enabled"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(true)
    }

    pub fn set_retry_enabled(&self, enabled: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "retry");
        if let Some(retry) = inner.global_settings.get_mut("retry") {
            retry.set("enabled", SettingsValue::Bool(enabled));
        }
        inner.modified_nested_fields.mark("retry", "enabled");
        inner.modified_fields.insert("retry");
        Self::save(&mut inner);
    }

    pub fn get_retry_settings(&self) -> RetrySettings {
        let inner = self.lock();
        RetrySettings {
            enabled: inner
                .settings
                .get_in(&["retry", "enabled"])
                .and_then(SettingsValue::as_bool)
                .unwrap_or(true),
            max_retries: inner
                .settings
                .get_in(&["retry", "maxRetries"])
                .and_then(SettingsValue::as_i64)
                .unwrap_or(3),
            base_delay_ms: inner
                .settings
                .get_in(&["retry", "baseDelayMs"])
                .and_then(SettingsValue::as_i64)
                .unwrap_or(2_000),
            max_agent_delay_ms: inner
                .settings
                .get_in(&["retry", "maxAgentDelayMs"])
                .and_then(SettingsValue::as_i64)
                .unwrap_or(DEFAULT_MAX_AGENT_RETRY_DELAY_MS as i64),
        }
    }

    /// Upstream `getHttpIdleTimeoutMs()` — parses + validates the raw value.
    pub fn get_http_idle_timeout_ms(&self) -> Result<u64, String> {
        let value = self.lock().settings.get("httpIdleTimeoutMs").cloned();
        let parsed = parse_timeout_setting(value.as_ref(), "httpIdleTimeoutMs")?;
        Ok(parsed.unwrap_or(
            crate::coding_agent::core::http_dispatcher::DEFAULT_HTTP_IDLE_TIMEOUT_MS as i64,
        ) as u64)
    }

    pub fn set_http_idle_timeout_ms(&self, timeout_ms: f64) -> Result<(), String> {
        if !timeout_ms.is_finite() || timeout_ms < 0.0 {
            return Err(format!(
                "Invalid httpIdleTimeoutMs setting: {}",
                js_number_to_string(timeout_ms)
            ));
        }
        let mut inner = self.lock();
        inner
            .global_settings
            .set("httpIdleTimeoutMs", SettingsValue::Num(timeout_ms.floor()));
        inner.modified_fields.insert("httpIdleTimeoutMs");
        Self::save(&mut inner);
        Ok(())
    }

    pub fn get_provider_retry_settings(&self) -> ProviderRetrySettings {
        let inner = self.lock();
        ProviderRetrySettings {
            timeout_ms: inner
                .settings
                .get_in(&["retry", "provider", "timeoutMs"])
                .and_then(SettingsValue::as_i64),
            max_retries: inner
                .settings
                .get_in(&["retry", "provider", "maxRetries"])
                .and_then(SettingsValue::as_i64),
            max_retry_delay_ms: inner
                .settings
                .get_in(&["retry", "provider", "maxRetryDelayMs"])
                .and_then(SettingsValue::as_i64)
                .unwrap_or(60_000),
        }
    }

    /// Upstream `getWebSocketConnectTimeoutMs()`.
    pub fn get_websocket_connect_timeout_ms(&self) -> Result<Option<i64>, String> {
        let value = self
            .lock()
            .settings
            .get("websocketConnectTimeoutMs")
            .cloned();
        parse_timeout_setting(value.as_ref(), "websocketConnectTimeoutMs")
    }

    pub fn get_hide_thinking_block(&self) -> bool {
        self.lock()
            .settings
            .get("hideThinkingBlock")
            .and_then(SettingsValue::as_bool)
            .unwrap_or(false)
    }

    pub fn get_show_cache_miss_notices(&self) -> bool {
        self.lock()
            .settings
            .get("showCacheMissNotices")
            .and_then(SettingsValue::as_bool)
            .unwrap_or(false)
    }

    /// Upstream `getExternalEditorCommand()`: setting (non-blank) →
    /// `VISUAL`/`EDITOR` env → platform default.
    pub fn get_external_editor_command(&self) -> String {
        let configured = {
            let inner = self.lock();
            inner
                .settings
                .get("externalEditor")
                .and_then(SettingsValue::as_str)
                .map(str::to_string)
        };
        if let Some(configured) = configured {
            if !configured.trim().is_empty() {
                return configured;
            }
        }
        for name in ["VISUAL", "EDITOR"] {
            if let Ok(value) = std::env::var(name) {
                // JS `||` treats empty strings as falsy.
                if !value.is_empty() {
                    return value;
                }
            }
        }
        if cfg!(windows) {
            "notepad".to_string()
        } else {
            "nano".to_string()
        }
    }

    pub fn set_hide_thinking_block(&self, hide: bool) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("hideThinkingBlock", SettingsValue::Bool(hide));
        inner.modified_fields.insert("hideThinkingBlock");
        Self::save(&mut inner);
    }

    pub fn set_show_cache_miss_notices(&self, show: bool) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("showCacheMissNotices", SettingsValue::Bool(show));
        inner.modified_fields.insert("showCacheMissNotices");
        Self::save(&mut inner);
    }

    /// Upstream `getShellPath()` — `normalizePath`-expanded when set.
    pub fn get_shell_path(&self) -> Option<String> {
        let shell_path = self
            .lock()
            .settings
            .get("shellPath")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)?;
        Some(normalize_path(&shell_path).unwrap_or(shell_path))
    }

    pub fn set_shell_path(&self, path: Option<&str>) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set_or_remove("shellPath", path.map(SettingsValue::str));
        inner.modified_fields.insert("shellPath");
        Self::save(&mut inner);
    }

    /// Upstream `getQuietStartup()` (v1.0.0): returns the stored `true`/`"header"`
    /// value, anything else (including invalid values) as `false`.
    pub fn get_quiet_startup(&self) -> QuietStartup {
        match self.lock().settings.get("quietStartup") {
            Some(SettingsValue::Bool(true)) => QuietStartup::Full,
            Some(SettingsValue::Str(s)) if s == "header" => QuietStartup::Header,
            _ => QuietStartup::Off,
        }
    }

    pub fn set_quiet_startup(&self, quiet: QuietStartup) {
        let mut inner = self.lock();
        match quiet {
            QuietStartup::Full => inner
                .global_settings
                .set("quietStartup", SettingsValue::Bool(true)),
            QuietStartup::Header => inner
                .global_settings
                .set("quietStartup", SettingsValue::str("header")),
            QuietStartup::Off => inner
                .global_settings
                .set("quietStartup", SettingsValue::Bool(false)),
        }
        inner.modified_fields.insert("quietStartup");
        Self::save(&mut inner);
    }

    /// Upstream `getDefaultProjectTrust()` — reads the **global** settings
    /// only, defaulting invalid values to `ask`.
    pub fn get_default_project_trust(&self) -> DefaultProjectTrust {
        let inner = self.lock();
        match inner
            .global_settings
            .get("defaultProjectTrust")
            .and_then(SettingsValue::as_str)
        {
            Some("always") => DefaultProjectTrust::Always,
            Some("never") => DefaultProjectTrust::Never,
            _ => DefaultProjectTrust::Ask,
        }
    }

    pub fn set_default_project_trust(&self, trust: DefaultProjectTrust) {
        let wire = match trust {
            DefaultProjectTrust::Ask => "ask",
            DefaultProjectTrust::Always => "always",
            DefaultProjectTrust::Never => "never",
        };
        let mut inner = self.lock();
        inner
            .global_settings
            .set("defaultProjectTrust", SettingsValue::str(wire));
        inner.modified_fields.insert("defaultProjectTrust");
        Self::save(&mut inner);
    }

    pub fn get_shell_command_prefix(&self) -> Option<String> {
        self.lock()
            .settings
            .get("shellCommandPrefix")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    pub fn set_shell_command_prefix(&self, prefix: Option<&str>) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set_or_remove("shellCommandPrefix", prefix.map(SettingsValue::str));
        inner.modified_fields.insert("shellCommandPrefix");
        Self::save(&mut inner);
    }

    pub fn get_npm_command(&self) -> Option<Vec<String>> {
        let inner = self.lock();
        string_list(inner.settings.get("npmCommand"))
    }

    pub fn set_npm_command(&self, command: Option<Vec<String>>) {
        let mut inner = self.lock();
        inner.global_settings.set_or_remove(
            "npmCommand",
            command.map(|items| {
                SettingsValue::Arr(items.into_iter().map(SettingsValue::Str).collect())
            }),
        );
        inner.modified_fields.insert("npmCommand");
        Self::save(&mut inner);
    }

    pub fn get_collapse_changelog(&self) -> bool {
        self.bool_setting("collapseChangelog", false)
    }

    pub fn set_collapse_changelog(&self, collapse: bool) {
        self.set_global_bool("collapseChangelog", collapse);
    }

    pub fn get_enable_install_telemetry(&self) -> bool {
        self.bool_setting("enableInstallTelemetry", true)
    }

    pub fn set_enable_install_telemetry(&self, enabled: bool) {
        self.set_global_bool("enableInstallTelemetry", enabled);
    }

    pub fn get_enable_analytics(&self) -> bool {
        self.bool_setting("enableAnalytics", false)
    }

    pub fn get_tracking_id(&self) -> Option<String> {
        self.lock()
            .settings
            .get("trackingId")
            .and_then(SettingsValue::as_str)
            .map(str::to_string)
    }

    /// Upstream `setEnableAnalytics(enabled)` — generates a tracking id on
    /// first opt-in.
    pub fn set_enable_analytics(&self, enabled: bool) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("enableAnalytics", SettingsValue::Bool(enabled));
        inner.modified_fields.insert("enableAnalytics");
        if enabled && inner.global_settings.get("trackingId").is_none() {
            inner
                .global_settings
                .set("trackingId", SettingsValue::str(&random_uuid_v4()));
            inner.modified_fields.insert("trackingId");
        }
        Self::save(&mut inner);
    }

    /// Upstream `getPackages()` — always an array.
    pub fn get_packages(&self) -> Vec<SettingsValue> {
        let inner = self.lock();
        inner
            .settings
            .get("packages")
            .and_then(SettingsValue::as_array)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_packages(&self, packages: Vec<SettingsValue>) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("packages", SettingsValue::Arr(packages));
        inner.modified_fields.insert("packages");
        Self::save(&mut inner);
    }

    pub fn set_project_packages(&self, packages: Vec<SettingsValue>) -> Result<(), String> {
        self.update_project_settings("packages", |settings| {
            settings.set("packages", SettingsValue::Arr(packages));
        })
    }

    pub fn get_extension_paths(&self) -> Vec<String> {
        self.string_list_setting("extensions")
    }

    pub fn set_extension_paths(&self, paths: Vec<String>) {
        self.set_global_string_list("extensions", paths);
    }

    pub fn set_project_extension_paths(&self, paths: Vec<String>) -> Result<(), String> {
        self.update_project_settings("extensions", |settings| {
            settings.set("extensions", string_list_value(&paths));
        })
    }

    pub fn get_skill_paths(&self) -> Vec<String> {
        self.string_list_setting("skills")
    }

    pub fn set_skill_paths(&self, paths: Vec<String>) {
        self.set_global_string_list("skills", paths);
    }

    pub fn set_project_skill_paths(&self, paths: Vec<String>) -> Result<(), String> {
        self.update_project_settings("skills", |settings| {
            settings.set("skills", string_list_value(&paths));
        })
    }

    pub fn get_prompt_template_paths(&self) -> Vec<String> {
        self.string_list_setting("prompts")
    }

    pub fn set_prompt_template_paths(&self, paths: Vec<String>) {
        self.set_global_string_list("prompts", paths);
    }

    pub fn set_project_prompt_template_paths(&self, paths: Vec<String>) -> Result<(), String> {
        self.update_project_settings("prompts", |settings| {
            settings.set("prompts", string_list_value(&paths));
        })
    }

    pub fn get_theme_paths(&self) -> Vec<String> {
        self.string_list_setting("themes")
    }

    pub fn set_theme_paths(&self, paths: Vec<String>) {
        self.set_global_string_list("themes", paths);
    }

    pub fn set_project_theme_paths(&self, paths: Vec<String>) -> Result<(), String> {
        self.update_project_settings("themes", |settings| {
            settings.set("themes", string_list_value(&paths));
        })
    }

    pub fn get_enable_skill_commands(&self) -> bool {
        self.bool_setting("enableSkillCommands", true)
    }

    pub fn set_enable_skill_commands(&self, enabled: bool) {
        self.set_global_bool("enableSkillCommands", enabled);
    }

    /// Upstream `getThinkingBudgets()` — raw clone.
    pub fn get_thinking_budgets(&self) -> Option<SettingsValue> {
        self.lock().settings.get("thinkingBudgets").cloned()
    }

    /// Upstream `getTerminalCapabilityOverrides()`.
    pub fn get_terminal_capability_overrides(&self) -> TerminalCapabilityOverrides {
        let inner = self.lock();
        let terminal = inner.settings.get("terminal");
        let images = terminal.and_then(|value| value.get("images"));
        let images_override = match images {
            Some(SettingsValue::Str(value)) if value == "kitty" => {
                Some(TerminalImagesOverride::Kitty)
            }
            Some(SettingsValue::Str(value)) if value == "iterm2" => {
                Some(TerminalImagesOverride::Iterm2)
            }
            Some(SettingsValue::Bool(false)) => Some(TerminalImagesOverride::Cleared),
            _ => None,
        };
        let true_color = terminal
            .and_then(|value| value.get("trueColor"))
            .and_then(SettingsValue::as_bool);
        let hyperlinks = terminal
            .and_then(|value| value.get("hyperlinks"))
            .and_then(SettingsValue::as_bool);
        TerminalCapabilityOverrides {
            images: images_override,
            true_color,
            hyperlinks,
        }
    }

    pub fn get_show_images(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["terminal", "showImages"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(true)
    }

    pub fn set_show_images(&self, show: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "terminal");
        if let Some(terminal) = inner.global_settings.get_mut("terminal") {
            terminal.set("showImages", SettingsValue::Bool(show));
        }
        inner.modified_nested_fields.mark("terminal", "showImages");
        inner.modified_fields.insert("terminal");
        Self::save(&mut inner);
    }

    /// Upstream `getImageWidthCells()` — validated to `max(1, floor(n))`.
    pub fn get_image_width_cells(&self) -> i64 {
        let inner = self.lock();
        match inner
            .settings
            .get_in(&["terminal", "imageWidthCells"])
            .and_then(SettingsValue::as_f64)
        {
            Some(width) if width.is_finite() => (width.floor().max(1.0)) as i64,
            _ => 60,
        }
    }

    pub fn set_image_width_cells(&self, width: f64) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "terminal");
        if let Some(terminal) = inner.global_settings.get_mut("terminal") {
            terminal.set(
                "imageWidthCells",
                SettingsValue::Num(width.floor().max(1.0)),
            );
        }
        inner
            .modified_nested_fields
            .mark("terminal", "imageWidthCells");
        inner.modified_fields.insert("terminal");
        Self::save(&mut inner);
    }

    /// Upstream `getClearOnShrink()`: settings first, then
    /// `PI_CLEAR_ON_SHRINK=1`.
    pub fn get_clear_on_shrink(&self) -> bool {
        {
            let inner = self.lock();
            if let Some(value) = inner.settings.get_in(&["terminal", "clearOnShrink"]) {
                return value.as_bool().unwrap_or(false);
            }
        }
        std::env::var("PI_CLEAR_ON_SHRINK").is_ok_and(|value| value == "1")
    }

    pub fn set_clear_on_shrink(&self, enabled: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "terminal");
        if let Some(terminal) = inner.global_settings.get_mut("terminal") {
            terminal.set("clearOnShrink", SettingsValue::Bool(enabled));
        }
        inner
            .modified_nested_fields
            .mark("terminal", "clearOnShrink");
        inner.modified_fields.insert("terminal");
        Self::save(&mut inner);
    }

    pub fn get_show_terminal_progress(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["terminal", "showTerminalProgress"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(false)
    }

    pub fn set_show_terminal_progress(&self, enabled: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "terminal");
        if let Some(terminal) = inner.global_settings.get_mut("terminal") {
            terminal.set("showTerminalProgress", SettingsValue::Bool(enabled));
        }
        inner
            .modified_nested_fields
            .mark("terminal", "showTerminalProgress");
        inner.modified_fields.insert("terminal");
        Self::save(&mut inner);
    }

    /// Upstream `getTuiMode()` (v1.0.0) — only `"regular"` is recognized;
    /// the default flips to fullscreen.
    pub fn get_tui_mode(&self) -> TuiMode {
        let inner = self.lock();
        match inner
            .settings
            .get("tuiMode")
            .and_then(SettingsValue::as_str)
        {
            Some("regular") => TuiMode::Regular,
            _ => TuiMode::Fullscreen,
        }
    }

    pub fn set_tui_mode(&self, mode: TuiMode) {
        let wire = match mode {
            TuiMode::Regular => "regular",
            TuiMode::Fullscreen => "fullscreen",
        };
        let mut inner = self.lock();
        inner
            .global_settings
            .set("tuiMode", SettingsValue::str(wire));
        inner.modified_fields.insert("tuiMode");
        Self::save(&mut inner);
    }

    pub fn get_fullscreen_exit_output(&self) -> FullscreenExitOutput {
        let inner = self.lock();
        match inner
            .settings
            .get("fullscreenExitOutput")
            .and_then(SettingsValue::as_str)
        {
            Some("resume-hint") => FullscreenExitOutput::ResumeHint,
            _ => FullscreenExitOutput::Transcript,
        }
    }

    pub fn set_fullscreen_exit_output(&self, output: FullscreenExitOutput) {
        let wire = match output {
            FullscreenExitOutput::Transcript => "transcript",
            FullscreenExitOutput::ResumeHint => "resume-hint",
        };
        let mut inner = self.lock();
        inner
            .global_settings
            .set("fullscreenExitOutput", SettingsValue::str(wire));
        inner.modified_fields.insert("fullscreenExitOutput");
        Self::save(&mut inner);
    }

    pub fn get_fullscreen_scrollbar(&self) -> FullscreenScrollbar {
        let inner = self.lock();
        match inner
            .settings
            .get("fullscreenScrollbar")
            .and_then(SettingsValue::as_str)
        {
            Some("always") => FullscreenScrollbar::Always,
            Some("hidden") => FullscreenScrollbar::Hidden,
            _ => FullscreenScrollbar::Auto,
        }
    }

    pub fn set_fullscreen_scrollbar(&self, mode: FullscreenScrollbar) {
        let wire = match mode {
            FullscreenScrollbar::Auto => "auto",
            FullscreenScrollbar::Always => "always",
            FullscreenScrollbar::Hidden => "hidden",
        };
        let mut inner = self.lock();
        inner
            .global_settings
            .set("fullscreenScrollbar", SettingsValue::str(wire));
        inner.modified_fields.insert("fullscreenScrollbar");
        Self::save(&mut inner);
    }

    pub fn get_fullscreen_copy_on_select(&self) -> bool {
        self.bool_setting("fullscreenCopyOnSelect", true)
    }

    pub fn set_fullscreen_copy_on_select(&self, enabled: bool) {
        self.set_global_bool("fullscreenCopyOnSelect", enabled);
    }

    pub fn get_image_auto_resize(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["images", "autoResize"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(true)
    }

    pub fn set_image_auto_resize(&self, enabled: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "images");
        if let Some(images) = inner.global_settings.get_mut("images") {
            images.set("autoResize", SettingsValue::Bool(enabled));
        }
        inner.modified_nested_fields.mark("images", "autoResize");
        inner.modified_fields.insert("images");
        Self::save(&mut inner);
    }

    pub fn get_block_images(&self) -> bool {
        self.lock()
            .settings
            .get_in(&["images", "blockImages"])
            .and_then(SettingsValue::as_bool)
            .unwrap_or(false)
    }

    pub fn set_block_images(&self, blocked: bool) {
        let mut inner = self.lock();
        ensure_global_nested(&mut inner.global_settings, "images");
        if let Some(images) = inner.global_settings.get_mut("images") {
            images.set("blockImages", SettingsValue::Bool(blocked));
        }
        inner.modified_nested_fields.mark("images", "blockImages");
        inner.modified_fields.insert("images");
        Self::save(&mut inner);
    }

    /// Upstream `getEnabledModels()` — raw pass-through (the merged value).
    pub fn get_enabled_models(&self) -> Option<Vec<String>> {
        let inner = self.lock();
        string_list(inner.settings.get("enabledModels"))
    }

    /// Upstream `getDefaultTools()` — empty lists are preserved.
    pub fn get_default_tools(&self) -> Option<Vec<String>> {
        let inner = self.lock();
        string_list(inner.settings.get("defaultTools"))
    }

    pub fn set_enabled_models(&self, patterns: Option<Vec<String>>) {
        let mut inner = self.lock();
        inner.global_settings.set_or_remove(
            "enabledModels",
            patterns.map(|items| string_list_value(&items)),
        );
        inner.modified_fields.insert("enabledModels");
        Self::save(&mut inner);
    }

    /// Upstream `getDoubleEscapeAction()` — `?? "tree"` without validation;
    /// the port returns the raw string for pass-through fidelity.
    pub fn get_double_escape_action(&self) -> String {
        let inner = self.lock();
        inner
            .settings
            .get("doubleEscapeAction")
            .and_then(SettingsValue::as_str)
            .unwrap_or("tree")
            .to_string()
    }

    pub fn set_double_escape_action(&self, action: &str) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("doubleEscapeAction", SettingsValue::str(action));
        inner.modified_fields.insert("doubleEscapeAction");
        Self::save(&mut inner);
    }

    /// Upstream `getTreeFilterMode()` — whitelist-validated.
    pub fn get_tree_filter_mode(&self) -> TreeFilterMode {
        let inner = self.lock();
        match inner
            .settings
            .get("treeFilterMode")
            .and_then(SettingsValue::as_str)
        {
            Some("no-tools") => TreeFilterMode::NoTools,
            Some("user-only") => TreeFilterMode::UserOnly,
            Some("labeled-only") => TreeFilterMode::LabeledOnly,
            Some("all") => TreeFilterMode::All,
            Some("default") => TreeFilterMode::Default,
            _ => TreeFilterMode::Default,
        }
    }

    pub fn set_tree_filter_mode(&self, mode: TreeFilterMode) {
        let wire = match mode {
            TreeFilterMode::Default => "default",
            TreeFilterMode::NoTools => "no-tools",
            TreeFilterMode::UserOnly => "user-only",
            TreeFilterMode::LabeledOnly => "labeled-only",
            TreeFilterMode::All => "all",
        };
        let mut inner = self.lock();
        inner
            .global_settings
            .set("treeFilterMode", SettingsValue::str(wire));
        inner.modified_fields.insert("treeFilterMode");
        Self::save(&mut inner);
    }

    /// Upstream `getShowHardwareCursor()`: settings first, then
    /// `PI_HARDWARE_CURSOR=1`.
    pub fn get_show_hardware_cursor(&self) -> bool {
        {
            let inner = self.lock();
            if let Some(value) = inner.settings.get("showHardwareCursor") {
                return value.as_bool().unwrap_or(false);
            }
        }
        std::env::var("PI_HARDWARE_CURSOR").is_ok_and(|value| value == "1")
    }

    pub fn set_show_hardware_cursor(&self, enabled: bool) {
        self.set_global_bool("showHardwareCursor", enabled);
    }

    pub fn get_editor_padding_x(&self) -> i64 {
        self.lock()
            .settings
            .get("editorPaddingX")
            .and_then(SettingsValue::as_i64)
            .unwrap_or(0)
    }

    pub fn set_editor_padding_x(&self, padding: f64) {
        let mut inner = self.lock();
        let clamped = padding.floor().clamp(0.0, 3.0);
        inner
            .global_settings
            .set("editorPaddingX", SettingsValue::Num(clamped));
        inner.modified_fields.insert("editorPaddingX");
        Self::save(&mut inner);
    }

    /// Upstream `getOutputPad()` — only exactly `0` is `0`.
    pub fn get_output_pad(&self) -> i64 {
        let inner = self.lock();
        match inner
            .settings
            .get("outputPad")
            .and_then(SettingsValue::as_f64)
        {
            Some(0.0) => 0,
            _ => 1,
        }
    }

    pub fn set_output_pad(&self, padding: i64) {
        let mut inner = self.lock();
        inner
            .global_settings
            .set("outputPad", SettingsValue::Num(padding as f64));
        inner.modified_fields.insert("outputPad");
        Self::save(&mut inner);
    }

    pub fn get_autocomplete_max_visible(&self) -> i64 {
        self.lock()
            .settings
            .get("autocompleteMaxVisible")
            .and_then(SettingsValue::as_i64)
            .unwrap_or(5)
    }

    pub fn set_autocomplete_max_visible(&self, max_visible: f64) {
        let mut inner = self.lock();
        let clamped = max_visible.floor().clamp(3.0, 20.0);
        inner
            .global_settings
            .set("autocompleteMaxVisible", SettingsValue::Num(clamped));
        inner.modified_fields.insert("autocompleteMaxVisible");
        Self::save(&mut inner);
    }

    pub fn get_code_block_indent(&self) -> String {
        let inner = self.lock();
        inner
            .settings
            .get_in(&["markdown", "codeBlockIndent"])
            .and_then(SettingsValue::as_str)
            .unwrap_or("  ")
            .to_string()
    }

    /// Upstream `getMermaidRenderingMode()` — only off/final recognized.
    pub fn get_mermaid_rendering_mode(&self) -> MermaidRenderingMode {
        let inner = self.lock();
        match inner
            .settings
            .get_in(&["markdown", "mermaid"])
            .and_then(SettingsValue::as_str)
        {
            Some("off") => MermaidRenderingMode::Off,
            Some("final") => MermaidRenderingMode::Final,
            _ => MermaidRenderingMode::Streaming,
        }
    }

    pub fn set_mermaid_rendering_mode(&self, mode: MermaidRenderingMode) {
        let wire = match mode {
            MermaidRenderingMode::Off => "off",
            MermaidRenderingMode::Final => "final",
            MermaidRenderingMode::Streaming => "streaming",
        };
        let mut inner = self.lock();
        if inner.global_settings.get("markdown").is_none() {
            inner
                .global_settings
                .set("markdown", SettingsValue::Obj(Vec::new()));
        }
        if let Some(markdown) = inner.global_settings.get_mut("markdown") {
            markdown.set("mermaid", SettingsValue::str(wire));
        }
        inner.modified_nested_fields.mark("markdown", "mermaid");
        inner.modified_fields.insert("markdown");
        Self::save(&mut inner);
    }

    /// Upstream `getWarnings()` — a copy of the warnings object.
    pub fn get_warnings(&self) -> SettingsValue {
        let inner = self.lock();
        inner
            .settings
            .get("warnings")
            .cloned()
            .unwrap_or(SettingsValue::Obj(Vec::new()))
    }

    pub fn set_warnings(&self, warnings: SettingsValue) {
        let mut inner = self.lock();
        inner.global_settings.set("warnings", warnings);
        inner.modified_fields.insert("warnings");
        Self::save(&mut inner);
    }

    // -- small helpers over the merged document ------------------------------

    fn bool_setting(&self, field: &str, default: bool) -> bool {
        self.lock()
            .settings
            .get(field)
            .and_then(SettingsValue::as_bool)
            .unwrap_or(default)
    }

    fn set_global_bool(&self, field: &str, value: bool) {
        let mut inner = self.lock();
        inner.global_settings.set(field, SettingsValue::Bool(value));
        inner.modified_fields.insert(field);
        Self::save(&mut inner);
    }

    fn string_list_setting(&self, field: &str) -> Vec<String> {
        let inner = self.lock();
        string_list(inner.settings.get(field)).unwrap_or_default()
    }

    fn set_global_string_list(&self, field: &str, values: Vec<String>) {
        let mut inner = self.lock();
        inner.global_settings.set(field, string_list_value(&values));
        inner.modified_fields.insert(field);
        Self::save(&mut inner);
    }
}

struct LoadedSettings {
    settings: SettingsValue,
    error: Option<String>,
}

fn to_settings_error(scope: SettingsScope, error: &str, path: Option<String>) -> SettingsError {
    SettingsError {
        scope,
        path,
        error: error.to_string(),
    }
}

fn clone_nested(source: &ModifiedNestedFields) -> ModifiedNestedFields {
    let mut cloned = ModifiedNestedFields::default();
    for (field, nested) in &source.entries {
        let mut set = OrderedSet::default();
        for item in nested.iter() {
            set.insert(item);
        }
        cloned.entries.push((field.clone(), set));
    }
    cloned
}

fn ensure_global_nested(global: &mut SettingsValue, field: &str) {
    if global.get(field).is_none() {
        global.set(field, SettingsValue::Obj(Vec::new()));
    }
}

fn string_list(value: Option<&SettingsValue>) -> Option<Vec<String>> {
    value.and_then(SettingsValue::as_array).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()
    })
}

fn string_list_value(values: &[String]) -> SettingsValue {
    SettingsValue::Arr(
        values
            .iter()
            .map(|value| SettingsValue::Str(value.clone()))
            .collect(),
    )
}

fn parse_timeout_setting(
    value: Option<&SettingsValue>,
    setting_name: &str,
) -> Result<Option<i64>, String> {
    let Some(value) = value else {
        // Absent (upstream `undefined`): no error, no value.
        return Ok(None);
    };
    let serde_value = settings_value_to_serde(value);
    let parsed =
        crate::coding_agent::core::http_dispatcher::parse_http_idle_timeout_ms(&serde_value);
    if let Some(parsed) = parsed {
        return Ok(Some(parsed as i64));
    }
    // Upstream throws when a value is present but unparseable (JSON `null`
    // included).
    Err(format!(
        "Invalid {setting_name} setting: {}",
        js_to_string(value)
    ))
}

fn settings_value_to_serde(value: &SettingsValue) -> serde_json::Value {
    match value {
        SettingsValue::Null => serde_json::Value::Null,
        SettingsValue::Bool(value) => serde_json::Value::Bool(*value),
        SettingsValue::Num(number) => serde_json::Number::from_f64(*number)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        SettingsValue::Str(value) => serde_json::Value::String(value.clone()),
        SettingsValue::Arr(items) => {
            serde_json::Value::Array(items.iter().map(settings_value_to_serde).collect())
        }
        SettingsValue::Obj(entries) => entries
            .iter()
            .map(|(key, value)| (key.clone(), settings_value_to_serde(value)))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into(),
    }
}

fn thinking_level_wire(level: ThinkingLevel) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .expect("thinking level serializes to a string")
}

/// `randomUUID()` (node crypto): a random v4 UUID.
fn random_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
#[path = "settings_manager_tests.rs"]
mod tests;
