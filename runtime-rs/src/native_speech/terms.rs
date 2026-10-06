use super::{
    error::{download, Result, SpeechError},
    storage,
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
};
pub const BUILTIN: &[&str] = &[
    "Pi Agent",
    "Pisper",
    "TypeScript",
    "JavaScript",
    "Python",
    "React",
    "useEffect",
    "useState",
    "Node.js",
    "npm install",
    "npm test",
    "cargo test",
    "Cargo",
    "Rust",
    "Tauri",
    "Vite",
    "Tailwind",
    "GitHub",
    "Git",
    "JSON",
    "TOML",
    "YAML",
    "API",
    "SDK",
    "MCP",
    "HTTP",
    "WebSocket",
    "Docker",
    "Kubernetes",
    "PostgreSQL",
];
fn invalid(message: &'static str) -> SpeechError {
    SpeechError {
        code: "bad_request",
        message,
        status: StatusCode::BAD_REQUEST,
    }
}
pub struct SpeechTerms {
    path: PathBuf,
    write: Mutex<()>,
}
impl SpeechTerms {
    pub fn new(agent_dir: &Path) -> Self {
        Self {
            path: agent_dir.join("speech-settings.json"),
            write: Mutex::new(()),
        }
    }
    fn read(&self) -> Result<Value> {
        let mut value = match storage::bounded_read(&self.path, 1024 * 1024) {
            Ok(bytes) => {
                serde_json::from_slice::<Value>(&bytes).map_err(|_| download("storage"))?
            }
            Err(_) if !self.path.exists() => json!({}),
            Err(error) => return Err(error),
        };
        if let Some(object) = value.as_object_mut() {
            object.remove("customTerms");
        }
        validate_update(&value)?;
        Ok(json!({"projectTermsEnabled":value["projectTermsEnabled"].as_bool().unwrap_or(true)}))
    }
    pub fn settings(&self) -> Result<Value> {
        let _lock = self.write.lock().map_err(|_| download("storage"))?;
        self.public(self.read()?)
    }
    fn public(&self, mut value: Value) -> Result<Value> {
        value["builtinTerms"] = json!(BUILTIN);
        Ok(value)
    }
    pub fn update(&self, input: &Value) -> Result<Value> {
        validate_update(input)?;
        let _lock = self.write.lock().map_err(|_| download("storage"))?;
        let mut stored = self.read()?;
        if let Some(enabled) = input.get("projectTermsEnabled") {
            stored["projectTermsEnabled"] = enabled.clone();
        }
        storage::write_json(&self.path, &stored)?;
        self.public(stored)
    }
    pub fn workspace(&self, cwd: Option<&Path>) -> Result<Vec<String>> {
        let settings = self.settings()?;
        let mut values = BUILTIN.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        if settings["projectTermsEnabled"] == true {
            if let Some(cwd) = cwd {
                values.extend(project_terms(cwd));
            }
        }
        let mut keys = HashSet::new();
        values.retain(|term| keys.insert(key(term)));
        values.truncate(128);
        Ok(values)
    }
}
fn validate_update(input: &Value) -> Result<()> {
    let Some(object) = input.as_object() else {
        return Err(invalid("语音设置必须是 JSON 对象。"));
    };
    if object.keys().any(|key| key != "projectTermsEnabled") {
        return Err(invalid("语音设置包含未知字段。"));
    }
    if object
        .get("projectTermsEnabled")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(invalid("projectTermsEnabled 必须是布尔值。"));
    }
    Ok(())
}
fn normalize(value: &str) -> Option<String> {
    if value.encode_utf16().count() > 64
        || value.chars().any(|c| c as u32 > 0xffff)
        || regex::Regex::new(r"[^\p{L}\p{N} .+_-]")
            .unwrap()
            .is_match(value)
        || !regex::Regex::new(r"[\p{L}\p{N}]").unwrap().is_match(value)
    {
        return None;
    }
    Some(
        regex::Regex::new(" +")
            .unwrap()
            .replace_all(value.trim(), " ")
            .into_owned(),
    )
}
fn key(value: &str) -> String {
    value.to_lowercase().replace([' ', '_', '-'], "")
}
fn project_term(value: &str) -> Option<String> {
    let name = if value.starts_with('@') {
        if !regex::Regex::new(r"^@[\p{L}\p{N}._-]+/[\p{L}\p{N}._-]+$")
            .unwrap()
            .is_match(value)
        {
            return None;
        }
        let pieces = value.split('/').collect::<Vec<_>>();
        if pieces.len() != 2 || pieces[0].len() < 2 {
            return None;
        }
        pieces[1]
    } else {
        value
    };
    normalize(name)?;
    let spaced = name.replace(['-', '_'], " ");
    let spaced = regex::Regex::new(r"([A-Z])([A-Z][a-z])")
        .unwrap()
        .replace_all(&spaced, "$1 $2");
    let spaced = regex::Regex::new(r"([a-z])([A-Z])")
        .unwrap()
        .replace_all(&spaced, "$1 $2");
    normalize(&spaced)
}
fn manifest(root: &Path, filename: &str) -> Option<Vec<u8>> {
    let path = root.join(filename);
    let actual = std::fs::canonicalize(&path).ok()?;
    if !actual.starts_with(root) || actual == root {
        return None;
    }
    storage::bounded_read(&actual, 256 * 1024).ok()
}
fn names(value: &Value, fields: &[&str], out: &mut Vec<String>) {
    for field in fields {
        if let Some(object) = value[*field].as_object() {
            out.extend(object.keys().cloned());
        }
    }
}
fn project_terms(cwd: &Path) -> Vec<String> {
    let Ok(root) = std::fs::canonicalize(cwd) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if let Some(npm) =
        manifest(&root, "package.json").and_then(|v| serde_json::from_slice::<Value>(&v).ok())
    {
        if let Some(name) = npm["name"].as_str() {
            found.push(name.to_owned());
        }
        names(
            &npm,
            &[
                "dependencies",
                "devDependencies",
                "peerDependencies",
                "optionalDependencies",
            ],
            &mut found,
        );
    }
    if let Some(cargo) = manifest(&root, "Cargo.toml")
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| toml::from_str::<toml::Value>(&v).ok())
        .and_then(|v| serde_json::to_value(v).ok())
    {
        if let Some(name) = cargo["package"]["name"].as_str() {
            found.push(name.to_owned());
        }
        let fields = ["dependencies", "dev-dependencies", "build-dependencies"];
        names(&cargo, &fields, &mut found);
        names(&cargo["workspace"], &fields, &mut found);
        if let Some(targets) = cargo["target"].as_object() {
            for value in targets.values() {
                names(value, &fields, &mut found);
            }
        }
    }
    found.iter().filter_map(|v| project_term(v)).collect()
}
pub fn spoken(term: &str) -> String {
    let value = regex::Regex::new(r"([a-z0-9])([A-Z])")
        .unwrap()
        .replace_all(term, "$1 $2");
    let value = regex::Regex::new(r"([A-Z])([A-Z][a-z])")
        .unwrap()
        .replace_all(&value, "$1 $2");
    regex::Regex::new(r"\s+")
        .unwrap()
        .replace_all(&value.replace(['_', '-'], " "), " ")
        .trim()
        .to_lowercase()
}
pub fn hotwords(terms: &[String]) -> String {
    let mut seen = HashSet::new();
    terms
        .iter()
        .map(|t| spoken(t))
        .filter(|t| !t.is_empty() && seen.insert(t.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn format(text: &str, terms: &[String]) -> String {
    let allowed = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9 ]*$").unwrap();
    let mut replacements = HashMap::new();
    let mut variants = Vec::new();
    for term in terms {
        if (!term.bytes().any(|b| b.is_ascii_uppercase()) && !term.contains(' '))
            || !allowed.is_match(term)
        {
            continue;
        }
        for variant in [spoken(term), term.to_lowercase()] {
            if variant.len() >= 3 && !replacements.contains_key(&variant) {
                variants.push(variant.clone());
                replacements.insert(variant, term);
            }
        }
    }
    if variants.is_empty() {
        return text.to_owned();
    }
    variants.sort_by_key(|v| std::cmp::Reverse(v.len()));
    let expression = regex::Regex::new(&format!(
        "(?i)(?:{})",
        variants
            .iter()
            .map(|v| regex::escape(v).replace(' ', "[ \\t]+"))
            .collect::<Vec<_>>()
            .join("|")
    ))
    .unwrap();
    let boundary = |c: char| c.is_ascii_alphanumeric() || "_./\\-".contains(c);
    let mut result = String::new();
    let mut last = 0;
    for matched in expression.find_iter(text) {
        let prior = text[..matched.start()].chars().next_back();
        let mut next = text[matched.end()..].chars();
        let first = next.next();
        let invalid_next = first.is_some_and(|c| c.is_ascii_alphanumeric() || "_/\\-".contains(c))
            || (first == Some('.')
                && next
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'));
        if prior.is_some_and(boundary) || invalid_next {
            continue;
        }
        result.push_str(&text[last..matched.start()]);
        let normalized = regex::Regex::new("[ \\t]+")
            .unwrap()
            .replace_all(&matched.as_str().to_lowercase(), " ")
            .into_owned();
        result.push_str(
            replacements
                .get(&normalized)
                .map(|v| v.as_str())
                .unwrap_or(matched.as_str()),
        );
        last = matched.end();
    }
    result.push_str(&text[last..]);
    result
}
