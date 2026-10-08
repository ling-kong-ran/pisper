//! Port of upstream `coding-agent/src/extensions/mcp/log.ts` (HEAD
//! `2bbfcca43`): log messages MCP servers send with `notifications/message`,
//! appended to `mcp.log` in the agent directory. Several pi processes may
//! write to the same file, so every message is one synchronous append. The
//! file is rotated to `mcp.log.1` once it grows past `MAX_LOG_BYTES`.

use std::path::Path;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

fn is_record(value: &serde_json::Value) -> bool {
    value.is_object()
}

/// Upstream `formatData`: strings pass through, everything else is
/// `JSON.stringify` (falling back to `String(...)` when that returns
/// `undefined` or throws).
fn format_data(data: &serde_json::Value) -> String {
    match data {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => "null".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Replaces each `\r\n` / `\r` / `\n` with `\n    ` (upstream
/// `.replace(/\r?\n/g, "\n    ")`).
fn indent_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push_str("\n    ");
        } else if character == '\n' {
            out.push_str("\n    ");
        } else {
            out.push(character);
        }
    }
    out
}

/// Format one `notifications/message` from `server` as a log line;
/// continuation lines are indented (upstream `formatMcpLogMessage`). `now`
/// replaces `new Date()`; the ISO string is UTC `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn format_mcp_log_message(
    server: &str,
    params: &serde_json::Value,
    now: &time_point::Zoned,
) -> String {
    let level = if is_record(params) {
        params
            .get("level")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("info")
    } else {
        "info"
    };
    let logger = if is_record(params) {
        match params.get("logger").and_then(serde_json::Value::as_str) {
            Some(logger) if !logger.is_empty() => format!(" {logger}:"),
            _ => String::new(),
        }
    } else {
        String::new()
    };
    // Upstream: `const message = isRecord(params) ? params : { data: params }`,
    // then `formatData(message.data)`. A record without `data` makes
    // `formatData(undefined)` = `String(JSON.stringify(undefined))` =
    // `"undefined"`; a non-record is wrapped, so its value formats directly.
    let text = if is_record(params) {
        match params.get("data") {
            Some(data) => indent_newlines(&format_data(data)),
            // `formatData(undefined)` — `String(JSON.stringify(undefined))`.
            None => "undefined".to_string(),
        }
    } else {
        indent_newlines(&format_data(params))
    };
    format!("{} [{server}] {level}{logger} {text}\n", now.to_iso(),)
}

/// Appends server log messages to one file. Write errors are ignored: logging
/// must not break tools (upstream `McpServerLog`).
pub struct McpServerLog {
    pub path: String,
    size: std::sync::Mutex<Option<u64>>,
}

impl McpServerLog {
    pub fn new(path: impl Into<String>) -> Self {
        McpServerLog {
            path: path.into(),
            size: std::sync::Mutex::new(None),
        }
    }

    /// Upstream `write(server, params)`.
    pub fn write(&self, server: &str, params: &serde_json::Value) {
        let now = time_point::now_zoned();
        let line = format_mcp_log_message(server, params, &now);
        let line_bytes = line.len() as u64;
        let _ = self.write_inner(&line, line_bytes);
    }

    fn write_inner(&self, line: &str, line_bytes: u64) -> std::io::Result<()> {
        let path = Path::new(&self.path);
        let mut size = self
            .size
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if size.is_none() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            *size = Some(current_size(path));
        }
        if size.unwrap_or(0) > MAX_LOG_BYTES {
            // Another process may have rotated it already; check before
            // renaming.
            if current_size(path) > MAX_LOG_BYTES {
                let _ = std::fs::rename(path, path.with_extension("log.1"));
            }
            *size = Some(current_size(path));
        }
        append_line(path, line)?;
        if let Some(current) = size.as_mut() {
            *current += line_bytes;
        }
        Ok(())
    }
}

/// `fs.appendFileSync(path, line)` — create-then-append (no buffering).
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

fn current_size(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Wall-clock seam: `new Date().toISOString()`. Production reads the system
/// clock; tests inject fixed times.
pub mod time_point {
    /// A `Date` stand-in carrying the epoch milliseconds.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Zoned {
        pub epoch_ms: i64,
    }

    impl Zoned {
        pub fn from_epoch_ms(epoch_ms: i64) -> Self {
            Zoned { epoch_ms }
        }

        /// `Date.prototype.toISOString()`: always UTC with millisecond
        /// precision (`YYYY-MM-DDTHH:MM:SS.sssZ`).
        pub fn to_iso(&self) -> String {
            let (year, month, day, hour, minute, second, milli) =
                civil_from_epoch_ms(self.epoch_ms);
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                year, month, day, hour, minute, second, milli
            )
        }
    }

    /// System time, the production `new Date()`.
    pub fn now_zoned() -> Zoned {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Zoned {
            epoch_ms: now.as_millis() as i64,
        }
    }

    /// Civil UTC fields from epoch milliseconds (proleptic Gregorian).
    fn civil_from_epoch_ms(epoch_ms: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
        let seconds = epoch_ms.div_euclid(1000);
        let milli = epoch_ms.rem_euclid(1000) as u32;
        let days = seconds.div_euclid(86_400);
        let day_seconds = seconds.rem_euclid(86_400);
        let hour = (day_seconds / 3600) as u32;
        let minute = ((day_seconds % 3600) / 60) as u32;
        let second = (day_seconds % 60) as u32;
        // Howard Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = if m <= 2 { y + 1 } else { y };
        (year, m, d, hour, minute, second, milli)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_one_line() {
        // `new Date(1758240000000).toISOString()` — the same instant the
        // oracle's `log_format` scenario pins (upstream formats UTC).
        let now = time_point::Zoned::from_epoch_ms(1758240000000);
        let line = format_mcp_log_message(
            "docs",
            &json!({"level": "debug", "logger": "db", "data": "ready"}),
            &now,
        );
        assert_eq!(line, "2025-09-19T00:00:00.000Z [docs] debug db: ready\n");
    }

    #[test]
    fn indents_newlines() {
        let now = time_point::Zoned::from_epoch_ms(0);
        let line = format_mcp_log_message("s", &json!("a\r\nb\nc"), &now);
        assert_eq!(line, "1970-01-01T00:00:00.000Z [s] info a\n    b\n    c\n");
    }
}
