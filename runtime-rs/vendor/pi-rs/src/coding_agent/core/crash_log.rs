//! Port of upstream `coding-agent/src/core/crash-log.ts`: the crash journal
//! (`crashes.json` under the agent dir) that survives a crashing process so
//! the next run can surface what happened. Best-effort persistence for
//! callers that are already crashing — every I/O failure swallows into
//! `None`/`()` like upstream's try/catch.
//!
//! The JSON encoding is byte-pinned: `JSON.stringify(records, null, 2)` +
//! a trailing newline (serde_json pretty prints with the same two-space
//! indent), and record fields keep the upstream camelCase order.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::coding_agent::core::get_agent_dir;
use crate::coding_agent::extensions::types::{SourceInfo, SourceOrigin};

/// Upstream `isSyntheticPath` (source-info.ts:27): builtin or angle-bracketed
/// synthetic sources never match real files.
fn is_synthetic_path(path: &str) -> bool {
    path.starts_with("builtin:") || path.starts_with('<')
}

/// Upstream `CrashRecord`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrashRecord {
    pub timestamp: String,
    pub version: String,
    pub kind: CrashKind,
    pub message: String,
    pub stack: Option<String>,
    pub session_file: Option<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notified: Option<bool>,
}

impl Default for CrashRecord {
    fn default() -> Self {
        Self {
            timestamp: String::new(),
            version: String::new(),
            kind: CrashKind::FatalError,
            message: String::new(),
            stack: None,
            session_file: None,
            cwd: String::new(),
            notified: None,
        }
    }
}

/// Upstream `CrashRecord["kind"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CrashKind {
    #[serde(rename = "uncaught_exception")]
    UncaughtException,
    #[serde(rename = "fatal_error")]
    #[default]
    FatalError,
}

const MAX_CRASH_RECORDS: usize = 5;
const MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub fn crash_log_path() -> String {
    // Upstream `join(getAgentDir(), "crashes.json")`.
    let agent_dir = get_agent_dir();
    let separator = if agent_dir.ends_with('/') || agent_dir.ends_with('\\') {
        ""
    } else if agent_dir.contains('\\') {
        "\\"
    } else {
        "/"
    };
    format!("{agent_dir}{separator}crashes.json")
}

/// Upstream `readCrashLog`: parse and keep only well-formed records (an
/// object with a string `timestamp` and a string `message`); any parse
/// failure or wrong shape yields an empty list.
pub fn read_crash_log(path: &str) -> Vec<CrashRecord> {
    let bytes = match std::fs::read_to_string(path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_str::<serde_json::Value>(&bytes) {
        Ok(serde_json::Value::Array(records)) => records
            .into_iter()
            .filter_map(|record| {
                let object = record.as_object()?;
                if !object
                    .get("timestamp")
                    .map(serde_json::Value::is_string)
                    .unwrap_or(false)
                    || !object
                        .get("message")
                        .map(serde_json::Value::is_string)
                        .unwrap_or(false)
                {
                    return None;
                }
                serde_json::from_value::<CrashRecord>(serde_json::Value::Object(object.clone()))
                    .ok()
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn write_crash_log(records: &[CrashRecord], path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = serde_json::to_string_pretty(records).unwrap_or_default();
    let _ = std::fs::write(path, format!("{body}\n"));
}

/// Normalize a stack-path candidate: backslashes to forward slashes, trailing
/// slashes stripped.
fn normalize_stack_path(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    normalized
        .trim_end_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn stack_contains_path(stack: &str, target_path: &str, include_descendants: bool) -> bool {
    let target = normalize_stack_path(target_path);
    if target.is_empty() || is_synthetic_path(&target) {
        return false;
    }
    // Upstream: a drive-letter path (`c:/...`) is matched case-insensitively
    // (Windows paths in stacks).
    let case_insensitive = {
        let bytes = target.as_bytes();
        bytes.len() >= 3 && bytes[0].is_ascii_lowercase() && bytes[1] == b':' && bytes[2] == b'/'
    };
    let haystack = if case_insensitive {
        stack.to_lowercase()
    } else {
        stack.to_string()
    };
    let needle = if case_insensitive {
        target.to_lowercase()
    } else {
        target.clone()
    };
    if include_descendants {
        return haystack.contains(&format!("{needle}/"));
    }
    let mut index = haystack.find(&needle);
    while let Some(start) = index {
        let next = haystack[start + needle.len()..].chars().next();
        match next {
            None | Some(':') | Some(')') => return true,
            Some(character) if character.is_whitespace() => return true,
            _ => {}
        }
        index = haystack[start + needle.len()..]
            .find(&needle)
            .map(|offset| start + offset);
    }
    false
}

/// Metadata the matcher needs from a loaded extension (`Pick<Extension,
/// "path" | "resolvedPath" | "sourceInfo">`).
pub struct ExtensionStackMetadata {
    pub path: String,
    pub resolved_path: String,
    pub source_info: SourceInfo,
}

/// Upstream `findExtensionStackMatches`: loaded extensions whose source files
/// appear in the stack trace.
pub fn find_extension_stack_matches(
    stack: Option<&str>,
    extensions: &[ExtensionStackMetadata],
) -> Vec<String> {
    let Some(stack) = stack else {
        return Vec::new();
    };
    // Drop the message line, keep `at ...` frames, decode percent-escapes,
    // and normalize separators.
    let normalized_stack = stack
        .split('\n')
        .skip(1)
        .filter(|line| {
            line.starts_with(char::is_whitespace) && line.trim_start().starts_with("at ")
        })
        .map(decode_uri_component)
        .collect::<Vec<_>>()
        .join("\n")
        .replace('\\', "/");
    let mut matches: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for extension in extensions {
        let resolved_path = normalize_stack_path(&extension.resolved_path);
        // Upstream single-file package: origin "package", the source is not a
        // registry/git/URL scheme, and it names a JS/TS file.
        let source = &extension.source_info.source;
        let single_file_package = extension.source_info.origin == SourceOrigin::Package
            && !["npm:", "git:", "https://", "ssh://"]
                .iter()
                .any(|scheme| source.starts_with(scheme))
            && [".js", ".cjs", ".mjs", ".ts", ".cts", ".mts"]
                .iter()
                .any(|suffix| source.ends_with(suffix));
        let package_root =
            if extension.source_info.origin == SourceOrigin::Package && !single_file_package {
                extension.source_info.base_dir.clone()
            } else {
                None
            }
            .filter(|base_dir| !base_dir.is_empty());
        let slash_index = resolved_path.rfind('/');
        let directory_entry = [
            "/index.js",
            "/index.cjs",
            "/index.mjs",
            "/index.ts",
            "/index.cts",
            "/index.mts",
        ]
        .iter()
        .any(|suffix| resolved_path.ends_with(suffix));
        let matched = if let Some(package_root) = package_root {
            stack_contains_path(&normalized_stack, &package_root, true)
        } else if directory_entry {
            match slash_index {
                Some(index) => {
                    stack_contains_path(&normalized_stack, &resolved_path[..index], true)
                }
                None => false,
            }
        } else {
            stack_contains_path(&normalized_stack, &resolved_path, false)
        };
        if !matched {
            continue;
        }
        let label = if extension.source_info.origin == SourceOrigin::Package && !source.is_empty() {
            source.clone()
        } else {
            extension.path.clone()
        };
        if seen.insert(label.clone()) {
            matches.push(label);
        }
    }
    matches
}

/// `decodeURI` on a stack line: percent-escapes only, leaving `/ ? : @ & = + $
/// , #` and unreserved characters alone; malformed escapes keep the original.
fn decode_uri_component(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &line[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Best-effort persistence for callers that are already crashing.
///
/// Upstream timestamps with `new Date().toISOString()` at call time; the
/// `_at_ms` variant exists so tests (and the fatal-error handler) can pin the
/// instant deterministically.
pub fn record_crash(crash: CrashInput, path: &str) -> Option<CrashRecord> {
    let now_ms = crate::ai::now_ms();
    record_crash_at_ms(crash, now_ms, path)
}

/// Upstream `CrashRecord["kind"]` input shape of `recordCrash`.
pub struct CrashInput {
    pub kind: CrashKind,
    /// Upstream `error: unknown` — an `Error` (message + stack) or any other
    /// value `String(error)` renders.
    pub error_message: String,
    pub error_stack: Option<String>,
    pub session_file: Option<String>,
    pub cwd: String,
}

/// `recordCrash` with the timestamp injected (ISO string pre-rendered).
pub fn record_crash_at_ms(crash: CrashInput, now_ms: i64, path: &str) -> Option<CrashRecord> {
    let record = CrashRecord {
        timestamp: crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms),
        version: env!("CARGO_PKG_VERSION").to_string(),
        kind: crash.kind,
        message: crash.error_message,
        stack: crash.error_stack,
        session_file: crash.session_file,
        cwd: crash.cwd,
        notified: None,
    };
    let mut records = read_crash_log(path);
    records.push(record.clone());
    let start = records.len().saturating_sub(MAX_CRASH_RECORDS);
    let window = records[start..].to_vec();
    write_crash_log(&window, path);
    Some(record)
}

/// Upstream `takeUnnotifiedCrash`: the newest recent unannounced crash,
/// marking every pending record as announced.
pub fn take_unnotified_crash(path: &str, now_ms: i64) -> Option<CrashRecord> {
    let records = read_crash_log(path);
    let crash = records
        .iter()
        .rev()
        .find(|record| {
            !record.notified.unwrap_or(false)
                && now_ms - parse_iso8601_ms(&record.timestamp).unwrap_or(i64::MAX) <= MAX_AGE_MS
        })
        .cloned()?;
    let marked = records
        .into_iter()
        .map(|mut record| {
            if record.notified.is_none() {
                record.notified = Some(true);
            }
            record
        })
        .collect::<Vec<_>>();
    write_crash_log(&marked, path);
    Some(crash)
}

/// Upstream `clearCrashLog`.
pub fn clear_crash_log(path: &str) {
    let _ = std::fs::remove_file(path);
}

/// `Date.parse` for the ISO strings this module writes; anything else is NaN
/// upstream (`now - NaN <= MAX_AGE` is false → the record never qualifies).
fn parse_iso8601_ms(timestamp: &str) -> Option<i64> {
    crate::agent_core::harness::session::jsonl::iso8601::parse_iso8601_utc(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::extensions::types::{SourceInfo, SourceOrigin, SourceScope};

    fn temp_path(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("crash-log-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("crashes.json").to_string_lossy().into_owned()
    }

    fn input(kind: CrashKind, message: &str) -> CrashInput {
        CrashInput {
            kind,
            error_message: message.to_string(),
            error_stack: None,
            session_file: None,
            cwd: "/work".to_string(),
        }
    }

    #[test]
    fn record_json_is_byte_pinned() {
        let path = temp_path("json");
        let record = record_crash_at_ms(
            input(CrashKind::UncaughtException, "boom"),
            1_759_200_000_123,
            &path,
        )
        .unwrap();
        assert_eq!(record.version, env!("CARGO_PKG_VERSION"));
        let body = std::fs::read_to_string(&path).unwrap();
        let expected = format!(
            r#"[
  {{
    "timestamp": "2025-09-30T02:40:00.123Z",
    "version": "{}",
    "kind": "uncaught_exception",
    "message": "boom",
    "stack": null,
    "sessionFile": null,
    "cwd": "/work"
  }}
]
"#,
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(body, expected);
    }

    #[test]
    fn records_cap_at_five() {
        let path = temp_path("cap");
        for index in 0..7 {
            record_crash_at_ms(
                input(CrashKind::FatalError, &format!("crash-{index}")),
                1_759_200_000_000 + index * 1_000,
                &path,
            );
        }
        let records = read_crash_log(&path);
        assert_eq!(records.len(), 5);
        assert_eq!(records[0].message, "crash-2");
        assert_eq!(records[4].message, "crash-6");
    }

    #[test]
    fn read_drops_malformed_records() {
        let path = temp_path("filter");
        std::fs::write(
            &path,
            r#"[{"timestamp": "2026-01-01T00:00:00.000Z", "message": "ok"},
                {"timestamp": 5, "message": "bad"},
                {"nope": true},
                "string",
                {"timestamp": "2026-01-02T00:00:00.000Z"}]"#,
        )
        .unwrap();
        let records = read_crash_log(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].message, "ok");
        // A non-array or unparseable file reads as empty.
        std::fs::write(&path, "{}").unwrap();
        assert!(read_crash_log(&path).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(read_crash_log(&path).is_empty());
    }

    #[test]
    fn take_unnotified_marks_everything_and_picks_the_newest_recent() {
        let path = temp_path("notify");
        let now = 1_759_200_000_000i64;
        record_crash_at_ms(
            input(CrashKind::FatalError, "old"),
            now - MAX_AGE_MS - 1,
            &path,
        );
        record_crash_at_ms(input(CrashKind::FatalError, "first"), now - 5_000, &path);
        record_crash_at_ms(input(CrashKind::FatalError, "second"), now - 1_000, &path);
        let crash = take_unnotified_crash(&path, now).unwrap();
        assert_eq!(crash.message, "second");
        // Every pending record is marked; a second take finds nothing.
        assert!(take_unnotified_crash(&path, now).is_none());
        let records = read_crash_log(&path);
        assert!(records.iter().all(|record| record.notified == Some(true)));
        // The mark persists in the file (upstream `{ ...record, notified: true }`).
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"notified\": true"));
        // A record older than MAX_AGE never qualifies even when unnotified.
        let path2 = temp_path("expired");
        record_crash_at_ms(
            input(CrashKind::FatalError, "ancient"),
            now - MAX_AGE_MS - 1,
            &path2,
        );
        assert!(take_unnotified_crash(&path2, now).is_none());
    }

    #[test]
    fn clear_removes_the_file() {
        let path = temp_path("clear");
        record_crash_at_ms(input(CrashKind::FatalError, "x"), 1, &path);
        clear_crash_log(&path);
        assert!(!Path::new(&path).exists());
    }

    fn metadata(
        path: &str,
        source: &str,
        origin: SourceOrigin,
        base_dir: Option<&str>,
    ) -> ExtensionStackMetadata {
        ExtensionStackMetadata {
            path: path.to_string(),
            resolved_path: path.to_string(),
            source_info: SourceInfo {
                path: path.to_string(),
                source: source.to_string(),
                scope: SourceScope::Temporary,
                origin,
                base_dir: base_dir.map(str::to_string),
            },
        }
    }

    #[test]
    fn stack_matcher_finds_extension_frames() {
        let stack = "Error: x
    at hello (/ext/tool.ts:1:1)
    at wrap (node:internal/x)";
        // Single top-level file extension: exact path match.
        let extensions = [metadata(
            "/ext/tool.ts",
            "tool.ts",
            SourceOrigin::TopLevel,
            None,
        )];
        assert_eq!(
            find_extension_stack_matches(Some(stack), &extensions),
            vec!["/ext/tool.ts"]
        );
        // Directory entry (index.ts) matches its directory.
        let directory = [metadata(
            "/ext/index.ts",
            "pkg",
            SourceOrigin::TopLevel,
            None,
        )];
        assert_eq!(
            find_extension_stack_matches(Some(stack), &directory),
            vec!["/ext/index.ts"]
        );
        // Package root matches descendants (base dir covers the frame).
        let package = [metadata(
            "/ext/dist/entry.ts",
            "some-package",
            SourceOrigin::Package,
            Some("/ext"),
        )];
        assert_eq!(
            find_extension_stack_matches(Some(stack), &package),
            vec!["some-package"]
        );
        // No stack, no match; unrelated path, no match.
        assert!(find_extension_stack_matches(None, &extensions).is_empty());
        let other = [metadata(
            "/other/tool.ts",
            "other.ts",
            SourceOrigin::TopLevel,
            None,
        )];
        assert!(find_extension_stack_matches(Some(stack), &other).is_empty());
        // Synthetic paths never match.
        let builtin = [metadata(
            "builtin:core",
            "builtin",
            SourceOrigin::TopLevel,
            None,
        )];
        assert!(find_extension_stack_matches(Some(stack), &builtin).is_empty());
    }

    #[test]
    fn stack_matcher_is_case_insensitive_on_drive_paths() {
        let stack = "Error
    at f (C:/Ext/tool.ts:1:1)";
        let extensions = [metadata(
            "c:/ext/tool.ts",
            "tool.ts",
            SourceOrigin::TopLevel,
            None,
        )];
        assert_eq!(
            find_extension_stack_matches(Some(stack), &extensions),
            vec!["c:/ext/tool.ts"]
        );
    }
}
