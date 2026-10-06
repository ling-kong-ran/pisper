//! One-time startup migrations from upstream `coding-agent/src/migrations.ts`.
//!
//! All paths are explicit so migration tests and embedding hosts never use a
//! developer's real credentials. Missing/malformed legacy inputs are ignored
//! at the same boundaries as upstream; final auth writes and bin-directory
//! creation propagate errors. Native OS error wording can differ from Node.
//! JSON input containing an escaped lone surrogate is rejected (left untouched),
//! rather than silently replacing credential bytes. String-spread output does
//! retain individual UTF-16 units with well-formed JSON escapes.
use crate::coding_agent::{
    core::{keybindings::migrate_keybindings_config, CONFIG_DIR_NAME},
    utils::{node_path, text::strip_bom},
};
use crate::serde_support::{js_number_string, order_json_object_keys};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationResult {
    pub migrated_auth_providers: Vec<String>,
    pub deprecation_warnings: Vec<String>,
}

fn read_json(path: &Path) -> io::Result<Value> {
    let bytes = fs::read(path)?;
    let mut value = serde_json::from_str(strip_bom(&String::from_utf8_lossy(&bytes)))
        .map_err(io::Error::other)?;
    order_json_object_keys(&mut value);
    Ok(value)
}
// Auth migrations spread JavaScript strings into UTF-16 code-unit properties.
// Keep the individual units until JSON output; String::from_utf16_lossy would
// irreversibly corrupt an astral character's two credential property values.
#[derive(Clone)]
enum AuthValue {
    Json(Value),
    Unit(u16),
    Object(Vec<(String, AuthValue)>),
}
fn own_entries(value: &Value) -> Vec<(String, AuthValue)> {
    spread_entries(&AuthValue::Json(value.clone()))
}
fn spread_entries(value: &AuthValue) -> Vec<(String, AuthValue)> {
    match value {
        AuthValue::Json(Value::Object(entries)) => entries
            .iter()
            .map(|(k, v)| (k.clone(), AuthValue::Json(v.clone())))
            .collect(),
        AuthValue::Json(Value::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), AuthValue::Json(v.clone())))
            .collect(),
        AuthValue::Json(Value::String(text)) => text
            .encode_utf16()
            .enumerate()
            .map(|(i, c)| (i.to_string(), AuthValue::Unit(c)))
            .collect(),
        AuthValue::Unit(unit) => vec![("0".into(), AuthValue::Unit(*unit))],
        AuthValue::Object(entries) => entries.clone(),
        _ => vec![],
    }
}
fn get<'a>(entries: &'a [(String, AuthValue)], key: &str) -> Option<&'a AuthValue> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}
fn insert(entries: &mut Vec<(String, AuthValue)>, key: String, value: AuthValue) {
    if let Some((_, v)) = entries.iter_mut().find(|(k, _)| *k == key) {
        *v = value;
    } else {
        entries.push((key, value));
    }
}
fn truthy(value: &AuthValue) -> bool {
    match value {
        AuthValue::Json(Value::Null) => false,
        AuthValue::Json(Value::Bool(value)) => *value,
        AuthValue::Json(Value::Number(value)) => value.as_f64().is_some_and(|n| n != 0.0),
        AuthValue::Json(Value::String(value)) => !value.is_empty(),
        _ => true,
    }
}
fn pretty_auth(value: &AuthValue) -> String {
    fn write(value: &AuthValue, depth: usize, out: &mut String) {
        match value {
            AuthValue::Unit(unit) => match char::from_u32(u32::from(*unit)) {
                Some(c) => out.push_str(&serde_json::to_string(&c.to_string()).expect("JSON char")),
                None => out.push_str(&format!("\"\\u{unit:04x}\"")),
            },
            AuthValue::Json(value) => {
                out.push_str(&pretty(value).replace('\n', &format!("\n{}", "  ".repeat(depth))))
            }
            AuthValue::Object(entries) if entries.is_empty() => out.push_str("{}"),
            AuthValue::Object(entries) => {
                let mut entries = entries.clone();
                crate::serde_support::order_js_object_entries(&mut entries);
                out.push_str("{\n");
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str(&serde_json::to_string(key).expect("JSON key"));
                    out.push_str(": ");
                    write(value, depth + 1, out);
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    write(value, 0, &mut out);
    out
}
fn inherited_key(key: &str) -> bool {
    matches!(
        key,
        "constructor"
            | "__proto__"
            | "__defineGetter__"
            | "__defineSetter__"
            | "hasOwnProperty"
            | "__lookupGetter__"
            | "__lookupSetter__"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "valueOf"
            | "toLocaleString"
    )
}
/// JSON.stringify(value, null, 2), including JS numeric-key enumeration and
/// binary64 number text. serde_json's default 1.0/exponent text is not equal.
fn pretty(value: &Value) -> String {
    fn write(value: &Value, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match value {
            Value::Number(n) => out.push_str(&js_number_string(n.as_f64().expect("JSON number"))),
            Value::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&inner);
                    write(item, depth + 1, out);
                }
                out.push('\n');
                out.push_str(&pad);
                out.push(']');
            }
            Value::Object(items) if !items.is_empty() => {
                out.push_str("{\n");
                for (i, (key, item)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&inner);
                    out.push_str(&serde_json::to_string(key).expect("JSON key"));
                    out.push_str(": ");
                    write(item, depth + 1, out);
                }
                out.push('\n');
                out.push_str(&pad);
                out.push('}');
            }
            _ => out.push_str(&serde_json::to_string(value).expect("JSON value")),
        }
    }
    let mut ordered = value.clone();
    order_json_object_keys(&mut ordered);
    let mut out = String::new();
    write(&ordered, 0, &mut out);
    out
}

pub fn migrate_auth_to_auth_json(agent_dir: &Path) -> io::Result<Vec<String>> {
    let auth_path = agent_dir.join("auth.json");
    if auth_path.exists() {
        return Ok(vec![]);
    }
    let oauth_path = agent_dir.join("oauth.json");
    let settings_path = agent_dir.join("settings.json");
    let mut migrated = Vec::new();
    // Upstream uses {} rather than a null-prototype dictionary.
    let mut prototype = Vec::new();
    let mut providers = vec![];
    let mut oauth_migration = || -> io::Result<()> {
        let oauth = read_json(&oauth_path)?;
        if oauth.is_null() {
            return Err(io::Error::other("Cannot convert null to object"));
        }
        for (provider, credential) in own_entries(&oauth) {
            let mut value = vec![("type".into(), AuthValue::Json("oauth".into()))];
            for (k, v) in spread_entries(&credential) {
                insert(&mut value, k, v);
            }
            if provider == "__proto__" {
                prototype = value;
            } else {
                insert(&mut migrated, provider.clone(), AuthValue::Object(value));
            }
            providers.push(provider);
        }
        fs::rename(&oauth_path, agent_dir.join("oauth.json.migrated"))
    };
    let _ = oauth_migration();
    let mut settings_migration = || -> io::Result<()> {
        let mut settings = read_json(&settings_path)?;
        let Some(api_keys) = settings
            .get("apiKeys")
            .filter(|v| v.is_object() || v.is_array())
            .cloned()
        else {
            return Ok(());
        };
        for (provider, key) in own_entries(&api_keys) {
            let already = get(&migrated, &provider)
                .or_else(|| get(&prototype, &provider))
                .map(truthy)
                .unwrap_or_else(|| inherited_key(&provider));
            if !already {
                if let AuthValue::Json(Value::String(key)) = key {
                    insert(
                        &mut migrated,
                        provider.clone(),
                        AuthValue::Json(json!({"type":"api_key","key":key})),
                    );
                    providers.push(provider);
                }
            }
        }
        if let Some(settings) = settings.as_object_mut() {
            settings.shift_remove("apiKeys");
        }
        fs::write(&settings_path, pretty(&settings))
    };
    let _ = settings_migration();
    if !migrated.is_empty() {
        fs::create_dir_all(agent_dir)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&auth_path)?;
        file.write_all(pretty_auth(&AuthValue::Object(migrated)).as_bytes())?;
    }
    Ok(providers)
}

fn native_join(parts: &[&str]) -> String {
    if cfg!(windows) {
        node_path::win32_join(parts)
    } else {
        node_path::posix_join(parts)
    }
}
/// Preserves upstream's Windows basename quirk (`split("/").pop()` wins even
/// for a backslash path). That rename fails closed on Windows; do not silently
/// repair it and claim byte-for-byte migration parity.
pub fn migrate_sessions_from_agent_root(agent_dir: &Path) {
    let Ok(files) = fs::read_dir(agent_dir) else {
        return;
    };
    for file in files
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl"))
    {
        let migrate = || -> io::Result<()> {
            let bytes = fs::read(file.path())?;
            let text = String::from_utf8_lossy(&bytes);
            let header: Value = serde_json::from_str(text.split('\n').next().unwrap_or_default())
                .map_err(io::Error::other)?;
            if header["type"] != "session" {
                return Ok(());
            }
            let Some(cwd) = header["cwd"].as_str().filter(|s| !s.is_empty()) else {
                return Ok(());
            };
            let cwd = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
            let safe = format!("--{}--", cwd.replace(['/', '\\', ':'], "-"));
            let dir = agent_dir.join("sessions").join(safe);
            fs::create_dir_all(&dir)?;
            let source = file.path().to_string_lossy().into_owned();
            let name = source
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .or_else(|| source.rsplit('\\').next())
                .unwrap_or_default();
            let target = PathBuf::from(native_join(&[&dir.to_string_lossy(), name]));
            if !target.exists() {
                fs::rename(file.path(), target)?;
            }
            Ok(())
        };
        let _ = migrate();
    }
}
fn migrate_commands_to_prompts(base: &Path, label: &str, log: &dyn Fn(&str)) {
    let source = base.join("commands");
    let target = base.join("prompts");
    if source.exists() && !target.exists() {
        match fs::rename(source, target) {
            Ok(()) => log(&format!("Migrated {label} commands/ → prompts/")),
            Err(error) => log(&format!(
                "Warning: Could not migrate {label} commands/ to prompts/: {error}"
            )),
        }
    }
}
fn migrate_keybindings_config_file(agent_dir: &Path) {
    let path = agent_dir.join("keybindings.json");
    let update = || -> io::Result<()> {
        let Value::Object(raw) = read_json(&path)? else {
            return Ok(());
        };
        let (config, migrated) = migrate_keybindings_config(&raw.into_iter().collect::<Vec<_>>());
        if migrated {
            fs::write(
                &path,
                pretty(&Value::Object(config.into_entries().into_iter().collect())) + "\n",
            )?;
        }
        Ok(())
    };
    let _ = update();
}
fn migrate_tools_to_bin(agent_dir: &Path, log: &dyn Fn(&str)) -> io::Result<()> {
    let tools = agent_dir.join("tools");
    let bin_dir = agent_dir.join("bin");
    if !tools.exists() {
        return Ok(());
    }
    let mut moved = false;
    for bin in ["fd", "rg", "fd.exe", "rg.exe"] {
        let old = tools.join(bin);
        let new = bin_dir.join(bin);
        if old.exists() {
            if !bin_dir.exists() {
                fs::create_dir_all(&bin_dir)?;
            }
            if !new.exists() {
                moved |= fs::rename(&old, new).is_ok();
            } else {
                let _ = fs::remove_file(old);
            }
        }
    }
    if moved {
        log("Migrated managed binaries tools/ → bin/");
    }
    Ok(())
}
fn deprecated_extension_dirs(base: &Path, label: &str) -> Vec<String> {
    let mut warnings = vec![];
    if base.join("hooks").exists() {
        warnings.push(format!(
            "{label} hooks/ directory found. Hooks have been renamed to extensions."
        ));
    }
    if let Ok(entries) = fs::read_dir(base.join("tools")) {
        let custom = entries.flatten().any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.')
                && !matches!(
                    name.to_lowercase().as_str(),
                    "fd" | "rg" | "fd.exe" | "rg.exe"
                )
        });
        if custom {
            warnings.push(format!("{label} tools/ directory contains custom tools. Custom tools have been merged into extensions."));
        }
    }
    warnings
}

/// Startup orchestration, including stdout reports in upstream order. The
/// interactive warning/key-press presentation remains owned by its TUI host.
pub fn run_migrations(
    cwd: &Path,
    agent_dir: &Path,
    log: &dyn Fn(&str),
) -> io::Result<MigrationResult> {
    let migrated_auth_providers = migrate_auth_to_auth_json(agent_dir)?;
    migrate_sessions_from_agent_root(agent_dir);
    migrate_tools_to_bin(agent_dir, log)?;
    migrate_keybindings_config_file(agent_dir);
    let project = cwd.join(CONFIG_DIR_NAME);
    migrate_commands_to_prompts(agent_dir, "Global", log);
    migrate_commands_to_prompts(&project, "Project", log);
    let mut deprecation_warnings = deprecated_extension_dirs(agent_dir, "Global");
    deprecation_warnings.extend(deprecated_extension_dirs(&project, "Project"));
    Ok(MigrationResult {
        migrated_auth_providers,
        deprecation_warnings,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationWarningStyle {
    Warning,
    Dim,
    Plain,
}
/// Native or embedding terminal boundary. `wait_for_data` completes on the
/// first input data event, not EOF. No JSON/RPC mode calls this presentation.
pub trait MigrationWarningUi: Send + Sync {
    fn log(&self, style: MigrationWarningStyle, text: &str);
    fn set_raw_mode(&self, _enabled: bool) {}
    fn resume(&self) {}
    fn pause(&self) {}
    fn wait_for_data(&self) -> futures::future::BoxFuture<'_, ()>;
}
struct RestoreWarningInput<'a>(&'a dyn MigrationWarningUi);
impl Drop for RestoreWarningInput<'_> {
    fn drop(&mut self) {
        self.0.set_raw_mode(false);
        self.0.pause();
    }
}
/// Upstream's interactive warning text, colors, raw-input lifecycle and key
/// acknowledgement. Dropping this Rust future also restores the terminal.
pub async fn show_deprecation_warnings(warnings: &[String], ui: &dyn MigrationWarningUi) {
    if warnings.is_empty() {
        return;
    }
    use MigrationWarningStyle::{Dim, Plain, Warning};
    for warning in warnings {
        ui.log(Warning, &format!("Warning: {warning}"));
    }
    ui.log(
        Warning,
        "\nMove your extensions to the extensions/ directory.",
    );
    ui.log(Warning, "Migration guide: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/CHANGELOG.md#extensions-migration");
    ui.log(Warning, "Documentation: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md");
    ui.log(Dim, "\nPress any key to continue...");
    ui.set_raw_mode(true);
    let input = RestoreWarningInput(ui);
    ui.resume();
    ui.wait_for_data().await;
    drop(input);
    ui.log(Plain, "");
}

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod tests;
