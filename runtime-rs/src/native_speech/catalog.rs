use super::error::{download, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::OnceLock};

pub const CATALOG: &str = include_str!("../../../shared/speech/speech-model-catalog.json");
pub const NOTICES: &str = include_str!("../../../shared/speech/speech-resource-notices.json");
pub const BPE: &str = include_str!("../../../shared/speech-resources/xasr-bpe.vocab");
pub const MARKER: &str = ".installation.json";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Archive {
    pub format: String,
    pub bytes: u64,
    pub sha256: String,
    pub strip_prefix: String,
    pub urls: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub kind: String,
    pub engine: String,
    pub name: String,
    pub languages: Vec<String>,
    pub license: Value,
    pub files: Vec<ModelFile>,
    pub archive: Option<Archive>,
    pub config: Value,
    #[serde(default)]
    pub voices: Vec<Value>,
    #[serde(skip)]
    pub fingerprint: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Catalog {
    pub defaults: Value,
    pub models: Vec<Model>,
}
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|value| format!("{value:02x}"))
        .collect()
}
pub fn safe_segment(part: &str) -> bool {
    static FORMAT: OnceLock<regex::Regex> = OnceLock::new();
    static DEVICE: OnceLock<regex::Regex> = OnceLock::new();
    if part.is_empty()
        || part.encode_utf16().count() > 255
        || part == "."
        || part == ".."
        || part.ends_with(['.', ' '])
        || part
            .chars()
            .any(|c| c.is_control() || "\\/<>:\"|?*".contains(c))
        || FORMAT
            .get_or_init(|| regex::Regex::new(r"\p{Cf}").unwrap())
            .is_match(part)
    {
        return false;
    }
    let device = part.split('.').next().unwrap_or("").to_lowercase();
    !["con", "prn", "aux", "nul", "conin$", "conout$", "clock$"].contains(&device.as_str())
        && !DEVICE
            .get_or_init(|| regex::Regex::new(r"^(com|lpt)[0-9¹²³]$").unwrap())
            .is_match(&device)
}
pub fn safe_relative(value: &str) -> bool {
    value.encode_utf16().count() <= 512
        && value.split('/').count() <= 16
        && value.split('/').all(safe_segment)
        && value.to_lowercase() != MARKER
        && !value.to_lowercase().starts_with(&format!("{MARKER}/"))
}
pub fn public_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    let host = url.host_str().unwrap_or("");
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && host.len() <= 253
        && host.contains('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && host.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part.as_bytes()[0].is_ascii_alphanumeric()
                && part.as_bytes()[part.len() - 1].is_ascii_alphanumeric()
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        && !["localhost", "local", "internal", "lan", "home", "arpa"]
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}
impl Catalog {
    pub fn shared() -> Result<Self> {
        Self::parse(CATALOG)
    }
    pub fn parse(text: &str) -> Result<Self> {
        let mut catalog: Self = serde_json::from_str(text).map_err(|_| download("catalog"))?;
        let mut ids = HashSet::new();
        for model in &mut catalog.models {
            if !safe_segment(&model.id)
                || model.id.len() > 128
                || !model
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                || !model
                    .id
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                || !ids.insert(model.id.to_lowercase())
                || !["asr", "tts", "vad"].contains(&model.kind.as_str())
                || model.name.trim().is_empty()
                || model.files.is_empty()
                || model.files.len() > 1024
            {
                return Err(download("catalog"));
            }
            let mut paths = Vec::<String>::new();
            for file in &mut model.files {
                let key = file.path.to_lowercase();
                if !safe_relative(&file.path)
                    || !digest_valid(&file.sha256)
                    || file.bytes > 9_007_199_254_740_991
                    || paths.iter().any(|other| {
                        other == &key
                            || key.starts_with(&format!("{other}/"))
                            || other.starts_with(&format!("{key}/"))
                    })
                    || (model.archive.is_none() && file.urls.is_empty())
                    || file.urls.len() > 2
                    || !file.urls.iter().all(|url| public_url(url))
                {
                    return Err(download("catalog"));
                }
                file.sha256.make_ascii_lowercase();
                paths.push(key);
            }
            if model
                .files
                .iter()
                .try_fold(32 * 1024 * 1024u64, |total, file| {
                    total.checked_add(file.bytes)
                })
                .is_none_or(|total| total > 9_007_199_254_740_991)
            {
                return Err(download("catalog"));
            }
            if let Some(archive) = &mut model.archive {
                if archive.format != "tar.bz2"
                    || archive.bytes == 0
                    || archive.bytes > 9_007_199_254_740_991
                    || !digest_valid(&archive.sha256)
                    || archive.strip_prefix.encode_utf16().count() > 512
                    || !archive.strip_prefix.ends_with('/')
                    || !safe_relative(&archive.strip_prefix[..archive.strip_prefix.len() - 1])
                    || archive.urls.is_empty()
                    || archive.urls.len() > 2
                    || !archive.urls.iter().all(|url| public_url(url))
                {
                    return Err(download("catalog"));
                }
                archive.sha256.make_ascii_lowercase();
            }
            if model.kind == "tts" {
                validate_tts(model)?;
            }
            let mut manifest =
                json!({"id":model.id,"kind":model.kind,"files":model.manifest_files()});
            if let Some(archive) = &model.archive {
                manifest["archive"] = json!({"format":archive.format,"bytes":archive.bytes,"sha256":archive.sha256,"stripPrefix":archive.strip_prefix});
            }
            model.fingerprint = hash(
                serde_json::to_string(&manifest)
                    .map_err(|_| download("catalog"))?
                    .as_bytes(),
            );
        }
        Ok(catalog)
    }
    pub fn model(&self, id: &str) -> Result<&Model> {
        self.models
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| download("unknown"))
    }
    pub fn default_model(&self, kind: &str) -> Result<&Model> {
        self.model(
            self.defaults[kind]
                .as_str()
                .ok_or_else(|| download("catalog"))?,
        )
    }
}
fn digest_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn validate_tts(model: &Model) -> Result<()> {
    let Some(config) = model.config.as_object() else {
        return Err(download("catalog"));
    };
    if model.engine != "vits"
        || config.keys().any(|key| {
            ![
                "model",
                "tokens",
                "lexicon",
                "dictDir",
                "numThreads",
                "maxTextCodePoints",
                "noiseScale",
                "noiseScaleW",
                "lengthScale",
                "ruleFsts",
            ]
            .contains(&key.as_str())
        })
    {
        return Err(download("catalog"));
    }
    for key in ["model", "tokens", "lexicon", "dictDir"] {
        let path = config
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| download("catalog"))?;
        if !safe_relative(path)
            || path.contains(',')
            || if key == "dictDir" {
                model.files.iter().any(|f| f.path == path)
                    || !model
                        .files
                        .iter()
                        .any(|f| f.path.starts_with(&format!("{path}/")))
            } else {
                !model.files.iter().any(|f| f.path == path)
            }
        {
            return Err(download("catalog"));
        }
    }
    for (key, limit) in [("numThreads", 16), ("maxTextCodePoints", 400)] {
        if let Some(value) = config.get(key) {
            if !value.as_u64().is_some_and(|v| v > 0 && v <= limit) {
                return Err(download("catalog"));
            }
        }
    }
    for key in ["noiseScale", "noiseScaleW", "lengthScale"] {
        if let Some(value) = config.get(key) {
            if !value.as_f64().is_some_and(|v| v.is_finite() && v > 0.0) {
                return Err(download("catalog"));
            }
        }
    }
    if let Some(rules) = config.get("ruleFsts") {
        let list = rules.as_array().ok_or_else(|| download("catalog"))?;
        if list.is_empty()
            || list.len() > 16
            || list.iter().any(|v| {
                !v.as_str()
                    .is_some_and(|p| !p.contains(',') && model.files.iter().any(|f| f.path == p))
            })
        {
            return Err(download("catalog"));
        }
    }
    Ok(())
}
impl Model {
    pub fn manifest_files(&self) -> Vec<Value> {
        self.files
            .iter()
            .map(|f| json!({"path":f.path,"bytes":f.bytes,"sha256":f.sha256}))
            .collect()
    }
    pub fn total_bytes(&self) -> u64 {
        self.archive
            .as_ref()
            .map(|a| a.bytes)
            .unwrap_or_else(|| self.files.iter().map(|f| f.bytes).sum())
    }
    pub fn marker(&self) -> Value {
        json!({"version":1,"id":self.id,"fingerprint":self.fingerprint,"files":self.manifest_files()})
    }
    pub fn public(&self, status: &str, downloaded: u64, error: Option<&str>) -> Value {
        let mut result = json!({"id":self.id,"kind":self.kind,"engine":self.engine,"name":self.name,"languages":self.languages,"license":self.license,"status":status,"downloadedBytes":downloaded,"totalBytes":self.total_bytes(),"filesBytes":self.files.iter().map(|f|f.bytes).sum::<u64>()});
        if !self.voices.is_empty() {
            result["voices"] = json!(self
                .voices
                .iter()
                .map(|v| json!({"id":v["id"],"name":v["name"],"language":v["language"]}))
                .collect::<Vec<_>>());
        }
        if let Some(error) = error {
            result["error"] = json!(error);
        }
        result
    }
}
