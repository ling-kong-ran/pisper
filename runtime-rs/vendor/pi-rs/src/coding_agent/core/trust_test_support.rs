use super::trust_manager::ProjectTrustStore;
use serde_json::Value;
use std::{fs, path::Path, path::PathBuf};

pub fn oracle() -> Value {
    serde_json::from_str(include_str!("trust_oracle.json")).unwrap()
}

pub struct Fixture {
    /// Keeps the fixture tree alive (deleted on drop); paths are joined from
    /// the canonicalized `root` so they stay stable across machines.
    _dir: tempfile::TempDir,
    pub root: String,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root =
            crate::coding_agent::utils::paths::canonicalize_path(dir.path().to_str().unwrap());
        Self { _dir: dir, root }
    }
    pub fn p(&self, portable: &str) -> String {
        if portable == "/" {
            return Path::new(&self.root)
                .ancestors()
                .last()
                .unwrap()
                .to_str()
                .unwrap()
                .into();
        }
        portable
            .strip_prefix("/root")
            .map(|suffix| {
                // environment-anchored: CI temp dirs can be 8.3 short paths
                // (RUNNER~1); join from the canonical root so the paths handed
                // to the product and the ones `text()`/`file()` map back match
                // byte-for-byte on every machine. Preserves `..` parts for
                // normalization tests.
                let mut p = PathBuf::from(&self.root);
                for part in suffix.trim_start_matches('/').split('/') {
                    p.push(part);
                }
                p.to_string_lossy().into_owned()
            })
            .unwrap_or_else(|| portable.into())
    }
    pub fn text(&self, value: &str) -> String {
        let platform_root = Path::new(&self.root)
            .ancestors()
            .last()
            .unwrap()
            .to_str()
            .unwrap();
        value
            .replace(&self.root, "/root")
            .replace(platform_root, "/")
            .replace('\\', "/")
    }
    pub fn value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(&s)),
            Value::Array(v) => Value::Array(v.into_iter().map(|x| self.value(x)).collect()),
            Value::Object(v) => Value::Object(
                v.into_iter()
                    .map(|(k, v)| (self.text(&k), self.value(v)))
                    .collect(),
            ),
            v => v,
        }
    }
    pub fn local_value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.p(&s)),
            Value::Array(v) => Value::Array(v.into_iter().map(|x| self.local_value(x)).collect()),
            Value::Object(v) => Value::Object(
                v.into_iter()
                    .map(|(k, v)| (self.p(&k), self.local_value(v)))
                    .collect(),
            ),
            v => v,
        }
    }
    pub fn write(&self, portable: &str, content: &str) {
        let p = self.p(portable);
        fs::create_dir_all(Path::new(&p).parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    pub fn initial(&self, content: &str) {
        // Preserve the exact seed bytes (BOM, spacing, duplicate/numeric keys).
        // JSON round-tripping here would silently change the oracle's input.
        let token = regex::Regex::new(r#""(?:[^"\\]|\\.)*""#).unwrap();
        let text = token.replace_all(content, |captures: &regex::Captures<'_>| {
            let raw = captures.get(0).unwrap().as_str();
            let value: String = serde_json::from_str(raw).unwrap();
            if value.starts_with("/root") {
                serde_json::to_string(&self.p(&value)).unwrap()
            } else {
                raw.to_owned()
            }
        });
        self.write("/root/agent/trust.json", &text);
    }
    /// Normalize only filesystem paths, not JSON whitespace or the trailing LF.
    pub fn file(&self) -> Value {
        let Ok(text) = fs::read_to_string(self.p("/root/agent/trust.json")) else {
            return Value::Null;
        };
        let escaped = serde_json::to_string(&self.root).unwrap();
        Value::String(
            text.replace(&escaped[1..escaped.len() - 1], "/root")
                .replace("\\\\", "/"),
        )
    }
    pub fn store(&self) -> ProjectTrustStore {
        ProjectTrustStore::new(&self.p("/root/agent")).unwrap()
    }
}
