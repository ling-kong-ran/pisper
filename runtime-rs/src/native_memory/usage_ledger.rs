//! 与 release 共用 pisper-usage.json；回调和会话扫描必须复用同一个写入器。
use super::runtime::{UsageRecord, UsageRecorder};
use anyhow::{bail, Context, Result};
use chrono::{Local, TimeZone};
use serde_json::{json, Map, Value};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const FIELDS: [&str; 6] = [
    "input",
    "output",
    "cacheRead",
    "cacheWrite",
    "reasoning",
    "totalTokens",
];

pub struct UsageLedger {
    path: PathBuf,
    write: Mutex<()>,
}
impl UsageLedger {
    pub fn new(agent_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: agent_dir.join("pisper-usage.json"),
            write: Mutex::new(()),
        })
    }
    pub fn recorder(self: &Arc<Self>) -> UsageRecorder {
        let ledger = self.clone();
        Arc::new(move |record: UsageRecord| {
            let ledger = ledger.clone();
            Box::pin(async move {
                // 小型账本原子写必须在 callback 返回前完成；capture 取消不会遗留 detached writer。
                ledger
                    .record(&record.day, &record.key, &record.usage)
                    .map(|_| ())
                    .map_err(|_| "usage_write_failed".to_owned())
            })
        })
    }
    pub fn record(&self, day: &str, key: &str, usage: &Value) -> Result<bool> {
        if day.is_empty() || key.is_empty() {
            return Ok(false);
        }
        let usage = normalized_usage(usage);
        if ["totalTokens", "input", "output"]
            .iter()
            .all(|key| number(&usage[*key]) == 0.0)
        {
            return Ok(false);
        }
        let _write = self
            .write
            .lock()
            .map_err(|_| anyhow::anyhow!("usage writer lock poisoned"))?;
        let mut ledger = self.load()?;
        let days = object_field(&mut ledger, "days")?;
        let records = object_field(
            days.entry(day).or_insert_with(|| json!({"records":{}})),
            "records",
        )?;
        if records.contains_key(key) {
            return Ok(false);
        }
        records.insert(key.to_owned(), usage);
        retain_days(days);
        self.save(&ledger)?;
        Ok(true)
    }
    pub fn today_usage(&self) -> Result<Value> {
        self.usage_for_day(&Local::now().format("%Y-%m-%d").to_string())
    }
    pub fn usage_for_day(&self, day: &str) -> Result<Value> {
        let _write = self
            .write
            .lock()
            .map_err(|_| anyhow::anyhow!("usage writer lock poisoned"))?;
        let ledger = self.load()?;
        let mut totals = normalized_usage(&Value::Null);
        if let Some(records) = ledger["days"][day]["records"].as_object() {
            for value in records.values() {
                let usage = normalized_usage(value);
                for key in FIELDS {
                    totals[key] = json!(number(&totals[key]) + number(&usage[key]));
                }
            }
        }
        totals["day"] = json!(day);
        Ok(totals)
    }
    /// 只处理完整 JSONL 行；追加写入的尾行留待下次，截断或路径变化从头扫描。
    pub fn scan_session(&self, path: &Path, session_id: &str, day: &str) -> Result<bool> {
        let _write = self
            .write
            .lock()
            .map_err(|_| anyhow::anyhow!("usage writer lock poisoned"))?;
        let mut ledger = self.load()?;
        let mut file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        let path_text = path.to_string_lossy();
        let previous = ledger["sessionScans"][session_id].clone();
        let has_usage = ledger["days"][day]["records"]
            .as_object()
            .is_some_and(|records| {
                records
                    .keys()
                    .any(|key| key.starts_with(&format!("session:{session_id}:")))
            });
        if previous.is_null() && has_usage {
            object_field(&mut ledger, "sessionScans")?
                .insert(session_id.to_owned(), json!({"path":path_text,"size":size}));
            self.save(&ledger)?;
            return Ok(true);
        }
        let previous_size = previous["size"].as_u64().unwrap_or(0);
        let offset = if previous["path"].as_str() == Some(&path_text) && size >= previous_size {
            previous_size
        } else {
            0
        };
        if offset >= size {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file.take(size - offset));
        let mut scanned = offset;
        let mut changed = false;
        loop {
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                break;
            }
            scanned += line.len() as u64;
            let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if entry["type"] != "message"
                || entry["message"]["role"] != "assistant"
                || !entry["message"]["usage"].is_object()
            {
                continue;
            }
            let timestamp = entry["message"]
                .get("timestamp")
                .filter(|value| !value.is_null())
                .unwrap_or(&entry["timestamp"]);
            if local_day(timestamp).as_deref() != Some(day) {
                continue;
            }
            let Some(entry_id) = entry["id"].as_str() else {
                continue;
            };
            let records = object_field(
                object_field(&mut ledger, "days")?
                    .entry(day)
                    .or_insert_with(|| json!({"records":{}})),
                "records",
            )?;
            let key = format!("session:{session_id}:{entry_id}");
            if !records.contains_key(&key) {
                records.insert(key, normalized_usage(&entry["message"]["usage"]));
                changed = true;
            }
        }
        if scanned != previous_size || previous["path"].as_str() != Some(&path_text) {
            object_field(&mut ledger, "sessionScans")?.insert(
                session_id.to_owned(),
                json!({"path":path_text,"size":scanned}),
            );
            changed = true;
        }
        if changed {
            self.save(&ledger)?;
        }
        Ok(changed)
    }
    fn load(&self) -> Result<Value> {
        let mut value = match fs::read(&self.path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("Usage ledger contains invalid JSON")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                json!({"days":{},"sessionScans":{}})
            }
            Err(e) => return Err(e.into()),
        };
        object_field(&mut value, "days")?;
        object_field(&mut value, "sessionScans")?;
        Ok(value)
    }
    fn save(&self, value: &Value) -> Result<()> {
        let parent = self.path.parent().context("Usage ledger has no parent")?;
        fs::create_dir_all(parent)?;
        let temp = parent.join(format!(".pisper-usage-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec_pretty(value)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}
fn object_field<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut Map<String, Value>> {
    let Some(object) = value.as_object_mut() else {
        bail!("Usage ledger object is invalid");
    };
    let field = object.entry(key).or_insert_with(|| json!({}));
    if field.is_null() {
        *field = json!({});
    }
    field
        .as_object_mut()
        .context("Usage ledger field is invalid")
}
fn retain_days(days: &mut Map<String, Value>) {
    let mut keys = days.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    let remove = keys.len().saturating_sub(45);
    for key in keys.into_iter().take(remove) {
        days.remove(&key);
    }
}
fn local_day(value: &Value) -> Option<String> {
    if let Some(ms) = value.as_i64() {
        return Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|value| value.format("%Y-%m-%d").to_string());
    }
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|value| value.with_timezone(&Local).format("%Y-%m-%d").to_string())
}
fn number(value: &Value) -> f64 {
    let result = match value {
        Value::Null => 0.0,
        Value::Bool(value) => {
            if *value {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(value) => value.as_f64().unwrap_or(0.0),
        Value::String(value) => {
            if value.trim().is_empty() {
                0.0
            } else {
                value.trim().parse().unwrap_or(0.0)
            }
        }
        Value::Array(values) if values.is_empty() => 0.0,
        Value::Array(values) if values.len() == 1 => number(&values[0]),
        _ => 0.0,
    };
    if result.is_finite() {
        result.max(0.0)
    } else {
        0.0
    }
}
pub fn normalized_usage(usage: &Value) -> Value {
    let mut result = Map::new();
    for key in FIELDS {
        let value = if key == "totalTokens" {
            usage
                .get(key)
                .filter(|value| !value.is_null())
                .or_else(|| usage.get("total"))
        } else {
            usage.get(key)
        };
        result.insert(key.to_owned(), json!(value.map(number).unwrap_or(0.0)));
    }
    Value::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pisper-usage-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn restart_idempotency_retention_and_unknown_fields_match_release() {
        let dir = Fixture::new();
        let path = dir.path().join("pisper-usage.json");
        fs::write(&path, r#"{"unknown":{"preserve":true},"days":{},"sessionScans":{"old":{"path":"fixture","size":7}}}"#).unwrap();
        let ledger = UsageLedger::new(dir.path());
        assert!(ledger
            .record(
                "2026-01-01",
                "memory:s:1",
                &json!({"input":"12","output":-4,"cacheRead":"Infinity","total":14})
            )
            .unwrap());
        assert!(!ledger
            .record("2026-01-01", "memory:s:1", &json!({"totalTokens":99}))
            .unwrap());
        assert!(!ledger
            .record("2026-01-01", "empty", &json!({"cacheRead":1}))
            .unwrap());
        assert_eq!(
            UsageLedger::new(dir.path())
                .usage_for_day("2026-01-01")
                .unwrap()["totalTokens"],
            json!(14.0)
        );
        for index in 0..50 {
            ledger
                .record(
                    &format!("2026-02-{index:02}"),
                    "fixture",
                    &json!({"totalTokens":1}),
                )
                .unwrap();
        }
        let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["days"].as_object().unwrap().len(), 45);
        assert_eq!(saved["unknown"]["preserve"], true);
        assert_eq!(saved["sessionScans"]["old"]["size"], 7);
    }
    #[test]
    fn incremental_scan_waits_for_complete_lines_and_preserves_bad_ledger() {
        let dir = Fixture::new();
        let ledger = UsageLedger::new(dir.path());
        let session = dir.path().join("synthetic-session.jsonl");
        let day = "2026-10-05";
        let entry = json!({"type":"message","id":"a","message":{"role":"assistant","timestamp":"2026-10-05T12:00:00+09:00","usage":{"input":2,"output":3,"totalTokens":5}}});
        let line = serde_json::to_string(&entry).unwrap();
        fs::write(&session, format!("malformed\n{line}")).unwrap();
        assert!(ledger.scan_session(&session, "s", day).unwrap());
        assert_eq!(ledger.usage_for_day(day).unwrap()["totalTokens"], 0.0);
        OpenOptions::new()
            .append(true)
            .open(&session)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        assert!(ledger.scan_session(&session, "s", day).unwrap());
        assert_eq!(ledger.usage_for_day(day).unwrap()["totalTokens"], 5.0);
        assert!(!ledger.scan_session(&session, "s", day).unwrap());
        fs::write(&ledger.path, b"broken json").unwrap();
        assert!(ledger
            .record(day, "memory:s:2", &json!({"input":1}))
            .is_err());
        assert_eq!(fs::read(&ledger.path).unwrap(), b"broken json");
    }
    #[tokio::test]
    async fn concurrent_callbacks_do_not_lose_records() {
        let dir = Fixture::new();
        let ledger = UsageLedger::new(dir.path());
        let recorder = ledger.recorder();
        let mut tasks = Vec::new();
        for index in 0..12 {
            let callback = recorder.clone();
            tasks.push(tokio::spawn(async move {
                callback(UsageRecord {
                    day: "2026-10-05".into(),
                    key: format!("memory:s:{index}"),
                    usage: json!({"totalTokens":2}),
                })
                .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(
            ledger.usage_for_day("2026-10-05").unwrap()["totalTokens"],
            24.0
        );
    }
}
