//! Port of upstream `experimental/session-catalog.ts` (v1.0.0): the durable
//! catalog of server-hosted Sessions — a directory holding `meta.json` per
//! Session next to the worker-owned `session.sqlite`.

use std::path::Path;

use serde_json::{json, Value};

use crate::coding_agent::core::path_join;

/// One server-hosted Session (upstream `SessionCatalogMetadata`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCatalogMetadata {
    pub id: String,
    pub created_at: f64,
    /// The working directory the Session's agent runs in.
    pub cwd: String,
    /// The Session directory. Workers lock it and own the storage inside it.
    pub path: String,
}

const METADATA_FILE: &str = "meta.json";
const STORAGE_FILE: &str = "session.sqlite";

/// Upstream `SESSION_ID`: `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/`.
fn is_session_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    let rest = chars.count();
    rest <= 127
        && id.chars().skip(1).all(|character: char| {
            character.is_ascii_alphanumeric()
                || character == '.'
                || character == '_'
                || character == '-'
        })
}

/// The durable storage file of a Session. Only the Session's worker opens it
/// (upstream `sessionStoragePath`).
pub fn session_storage_path(metadata: &SessionCatalogMetadata) -> String {
    path_join(&metadata.path, STORAGE_FILE)
}

/// Every Session in the directory. Entries without valid metadata are skipped
/// (upstream `listSessions`).
pub fn list_sessions(session_dir: &str) -> Result<Vec<SessionCatalogMetadata>, String> {
    let entries = match std::fs::read_dir(session_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut sessions = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_session_id(&name) {
            continue;
        }
        if let Some(metadata) = read_session(session_dir, &name) {
            sessions.push(metadata);
        }
    }
    Ok(sessions)
}

/// One Session by ID, or `None` when it does not exist (upstream
/// `readSession`).
pub fn read_session(session_dir: &str, id: &str) -> Option<SessionCatalogMetadata> {
    if !is_session_id(id) {
        return None;
    }
    let path = path_join(session_dir, id);
    let text = std::fs::read_to_string(path_join(&path, METADATA_FILE)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let object = value.as_object()?;
    let created_at = object.get("createdAt")?.as_f64()?;
    let cwd = object.get("cwd")?.as_str()?.to_string();
    Some(SessionCatalogMetadata {
        id: id.to_string(),
        created_at,
        cwd,
        path,
    })
}

/// v4-shaped random UUID (upstream `crypto.randomUUID`, the S6 seam also used
/// by the session manager).
fn random_uuid() -> String {
    let bytes: [u8; 16] = rand::random();
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let variant = format!("{:02x}", (bytes[8] & 0x3f) | 0x80);
    format!(
        "{}-{}-4{}-{}{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[7..8].concat(),
        variant,
        hex[9..10].concat(),
        hex[10..16].concat()
    )
}

/// Create an empty Session. Its worker creates the storage on first open
/// (upstream `createSession`).
pub fn create_session(
    session_dir: &str,
    id: Option<&str>,
    cwd: &str,
) -> Result<SessionCatalogMetadata, String> {
    let id = id.unwrap_or(&random_uuid()).to_string();
    if !is_session_id(&id) {
        return Err(format!("Invalid session ID: {id}"));
    }
    let path = path_join(session_dir, &id);
    std::fs::create_dir_all(session_dir).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::create_dir(&path) {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(format!("Session {id} already exists"));
        }
        return Err(error.to_string());
    }
    let metadata = SessionCatalogMetadata {
        id: id.clone(),
        // `Date.now()`.
        created_at: crate::ai::now_ms() as f64,
        cwd: cwd.to_string(),
        path,
    };
    // `JSON.stringify({ createdAt, cwd }, null, "\t")`.
    let document = serde_json::to_value(json!({
        "createdAt": metadata.created_at,
        "cwd": metadata.cwd,
    }))
    .unwrap_or(Value::Null);
    let text = match document {
        Value::Object(map) => {
            crate::coding_agent::extensions::mcp::config::stringify_indent(&map, "\t")
        }
        _ => String::new(),
    };
    std::fs::write(
        path_join(path_of(&metadata), METADATA_FILE),
        format!("{text}\n"),
    )
    .map_err(|error| error.to_string())?;
    Ok(metadata)
}

fn path_of(metadata: &SessionCatalogMetadata) -> &str {
    &metadata.path
}

/// Delete a Session directory. Its worker must be closed first (upstream
/// `deleteSession`).
pub fn delete_session(metadata: &SessionCatalogMetadata) -> Result<(), String> {
    let path = Path::new(&metadata.path);
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        // `force: true` ignores a missing directory.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
