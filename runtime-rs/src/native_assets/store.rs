//! 与 release pisper-assets.json/ 兼容的索引和内容管理。
#[path = "content.rs"]
pub mod content;
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
use uuid::Uuid;

fn time() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    })
}
pub fn public_asset(mut asset: Value) -> Value {
    if let Some(value) = asset.as_object_mut() {
        value.remove("storagePath");
        value.remove("references");
    }
    asset
}
fn session_asset(asset: &Value, session: &str) -> Option<Value> {
    if let Some(reference) = asset["references"]
        .as_array()
        .and_then(|r| r.iter().rev().find(|r| r["sessionId"] == session))
    {
        let mut value = asset.clone();
        for key in ["source", "sessionId", "sessionName"] {
            value[key] = reference[key].clone();
        }
        Some(value)
    } else if asset["sessionId"] == session {
        Some(asset.clone())
    } else {
        None
    }
}
fn reference(asset: &Value, input: &Value, size: u64) -> Value {
    let mut result = json!({"source":input["source"].as_str().unwrap_or("upload"),"sessionId":string(input,"sessionId"),"sessionName":string(input,"sessionName"),"name":input["name"].as_str().unwrap_or_else(||string(asset,"name")),"kind":input["kind"].as_str().unwrap_or_else(||string(asset,"kind")),"mimeType":input["mimeType"].as_str().unwrap_or_else(||string(asset,"mimeType")),"size":size,"created":input["created"].as_str().map(str::to_owned).unwrap_or_else(time)});
    if let Some(path) = input["filePath"].as_str() {
        result["filePath"] = json!(path);
    }
    result
}
fn references_mut(asset: &mut Value) -> Result<&mut Vec<Value>> {
    if !asset["references"].is_array() {
        asset["references"] = json!([])
    }
    asset["references"]
        .as_array_mut()
        .context("Invalid asset references")
}

pub struct AssetStore {
    root: PathBuf,
    index_path: PathBuf,
    index: Value,
}
pub struct Download {
    pub asset: Value,
    pub path: PathBuf,
    pub size: u64,
}
impl AssetStore {
    pub fn open(agent_dir: impl AsRef<Path>) -> Result<Self> {
        let root = absolute(&agent_dir.as_ref().join("pisper-assets"))?;
        fs::create_dir_all(&root)?;
        let index_path = agent_dir.as_ref().join("pisper-assets.json");
        let index = match fs::read(&index_path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .context("资产索引 JSON 已损坏，拒绝覆盖。")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"assets":[]}),
            Err(error) => return Err(error.into()),
        };
        if !index["assets"].is_array() {
            bail!("资产索引必须包含 assets 数组，原文件保持不变。");
        }
        let mut result = Self {
            root,
            index_path,
            index,
        };
        result.reconcile()?;
        Ok(result)
    }
    fn assets(&self) -> &[Value] {
        self.index["assets"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    fn assets_mut(&mut self) -> Result<&mut Vec<Value>> {
        self.index["assets"]
            .as_array_mut()
            .context("Invalid asset index")
    }
    fn save(&self) -> Result<()> {
        let path = self
            .index_path
            .with_extension(format!("{}.tmp", Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(&self.index)?;
        fs::write(&path, bytes)?;
        let file = File::options().write(true).open(&path)?;
        file.sync_all()?;
        drop(file);
        if let Err(error) = fs::rename(&path, &self.index_path) {
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(())
    }
    fn managed(&self, path: &Path) -> bool {
        if path == self.root {
            return false;
        }
        let root = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        path.starts_with(root)
    }
    pub fn create(&mut self, input: &Value) -> Result<Value> {
        if !input.is_object() {
            bail!("Invalid asset input.");
        }
        self.reconcile()?;
        let now = time();
        let source = input["source"].as_str().unwrap_or("upload");
        if input["kind"] == "link" || input.get("url").is_some() {
            let url = reqwest::Url::parse(string(input, "url"))?;
            if !["http", "https"].contains(&url.scheme()) {
                bail!("链接只支持 http 或 https。");
            }
            if let Some(existing) = self
                .assets()
                .iter()
                .find(|a| a["kind"] == "link" && a["url"] == url.as_str())
            {
                return Ok(public_asset(existing.clone()));
            }
            let name = content::safe_name(
                input["name"]
                    .as_str()
                    .unwrap_or_else(|| url.host_str().unwrap_or("附件")),
            );
            let asset = json!({"id":Uuid::new_v4().to_string(),"kind":"link","name":name,"url":url.as_str(),"mimeType":"text/uri-list","size":0,"source":source,"sessionId":string(input,"sessionId"),"sessionName":string(input,"sessionName"),"created":now,"modified":now});
            self.assets_mut()?.insert(0, asset.clone());
            self.save()?;
            return Ok(public_asset(asset));
        }
        let name = content::safe_name(string(input, "name"));
        let bytes = if let Some(text) = input.get("text") {
            text.as_str()
                .map(str::as_bytes)
                .map(Vec::from)
                .unwrap_or_else(|| text.to_string().into_bytes())
        } else {
            content::decode_base64(string(input, "data"))
        };
        if bytes.is_empty() {
            bail!("{name} 内容为空。");
        }
        if bytes.len() > 24 * 1024 * 1024 {
            bail!("{name} 超过 24 MB 资产限制。");
        }
        let mime = input["mimeType"]
            .as_str()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| content::mime_from_name(&name));
        let kind = if mime.starts_with("image/") || content::is_image(&name) {
            "image"
        } else {
            "file"
        };
        let hash = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if let Some(position) = self.assets().iter().position(|a| {
            a["kind"] != "link"
                && a["hash"] == hash
                && Path::new(string(a, "storagePath")).is_file()
        }) {
            let mut normalized = input.clone();
            normalized["name"] = json!(name);
            normalized["kind"] = json!(kind);
            normalized["mimeType"] = json!(mime);
            normalized["created"] = json!(now);
            let asset = &mut self.assets_mut()?[position];
            let occurrence = reference(asset, &normalized, bytes.len() as u64);
            references_mut(asset)?.push(occurrence);
            asset["modified"] = json!(now);
            if string(asset, "sessionId").is_empty() {
                asset["sessionId"] = json!(string(input, "sessionId"));
            }
            if string(asset, "sessionName").is_empty() {
                asset["sessionName"] = json!(string(input, "sessionName"));
            }
            let result = public_asset(asset.clone());
            self.save()?;
            return Ok(result);
        }
        let asset_id = Uuid::new_v4().to_string();
        let extension = content::extension(&name)
            .chars()
            .take(12)
            .collect::<String>();
        let path = self.root.join(format!("{asset_id}{extension}"));
        fs::write(&path, &bytes)?;
        let asset = json!({"id":asset_id,"kind":kind,"name":name,"mimeType":mime,"size":bytes.len(),"hash":hash,"storagePath":path,"source":source,"sessionId":string(input,"sessionId"),"sessionName":string(input,"sessionName"),"created":now,"modified":now});
        self.assets_mut()?.insert(0, asset.clone());
        if let Err(error) = self.save() {
            self.assets_mut()?.remove(0);
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(public_asset(asset))
    }
    pub fn list(&mut self, query: &str, kind: &str, session: &str) -> Result<Vec<Value>> {
        self.reconcile()?;
        let query = query.trim().to_lowercase();
        let mut seen = HashSet::new();
        Ok(self
            .assets()
            .iter()
            .filter_map(|asset| {
                if session.is_empty() {
                    Some(asset.clone())
                } else {
                    session_asset(asset, session)
                }
            })
            .filter(|a| kind.is_empty() || a["kind"] == kind)
            .filter(|a| {
                query.is_empty()
                    || format!(
                        "{} {} {}",
                        string(a, "name"),
                        string(a, "sessionName"),
                        string(a, "url")
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .filter(|a| {
                let path = string(a, "filePath")
                    .trim()
                    .replace('\\', "/")
                    .to_lowercase();
                let hash = string(a, "hash");
                let identity = if !path.is_empty() {
                    format!("path:{path}")
                } else if !hash.is_empty() {
                    format!("hash:{hash}")
                } else {
                    format!("id:{}", string(a, "id"))
                };
                seen.insert(identity)
            })
            .map(public_asset)
            .collect())
    }
    pub fn find(&self, asset_id: &str) -> Option<&Value> {
        self.assets().iter().find(|a| a["id"] == asset_id)
    }
    pub fn content(&mut self, asset_id: &str, preview: bool) -> Result<Option<Value>> {
        self.reconcile()?;
        let Some(asset) = self.find(asset_id) else {
            return Ok(None);
        };
        let name = string(asset, "name");
        let mime = string(asset, "mimeType");
        if asset["kind"] == "link" {
            let url = string(asset, "url");
            return Ok(Some(
                json!({"id":asset_id,"kind":"text","name":format!("{name}.url.txt"),"mimeType":"text/plain","size":url.encode_utf16().count(),"text":format!("链接：{url}")}),
            ));
        }
        let path = asset["storagePath"]
            .as_str()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| string(asset, "filePath"));
        let size = fs::metadata(path)?.len();
        if !preview && size > 10 * 1024 * 1024 {
            bail!("资产超过 10 MB，无法直接加入对话；仍可下载或在工作目录中读取。");
        }
        let mut file = File::open(path)?;
        let mut prefix = Vec::new();
        file.by_ref()
            .take(2 * 1024 * 1024)
            .read_to_end(&mut prefix)?;
        let known = content::known_text(name, mime);
        if known || !content::is_document(name) {
            if let Some(text) = content::decode_text(&prefix) {
                let truncated = size > prefix.len() as u64 || text.encode_utf16().count() > 400000;
                return Ok(Some(
                    json!({"id":asset_id,"kind":"text","name":name,"mimeType":if known||content::mime_text(mime){mime}else{"text/plain"},"size":size,"text":content::truncate_utf16(text,400000),"truncated":truncated}),
                ));
            }
        }
        if preview {
            return Ok(Some(
                json!({"id":asset_id,"kind":if content::is_document(name){"document"}else{"file"},"name":name,"mimeType":mime,"size":size}),
            ));
        }
        if asset["kind"] == "image" || content::is_document(name) {
            let data = STANDARD.encode(fs::read(path)?);
            let mut value = json!({"id":asset_id,"kind":if asset["kind"]=="image"{"image"}else{"document"},"name":name,"mimeType":mime,"size":size,"data":data});
            if asset["kind"] != "image" {
                value["extension"] = json!(content::extension(name).trim_start_matches('.'));
            }
            return Ok(Some(value));
        }
        let source = string(asset, "filePath");
        Ok(Some(
            json!({"id":asset_id,"kind":"text","name":format!("{name}.path.txt"),"mimeType":"text/plain","size":path.encode_utf16().count(),"text":if !source.is_empty(){format!("本地文件路径：{source}")}else{format!("资产 {name} 是二进制文件，请结合文件名称和元数据分析。")}}),
        ))
    }
    pub fn download(&mut self, asset_id: &str) -> Result<Option<Download>> {
        self.reconcile()?;
        let Some(asset) = self.find(asset_id) else {
            return Ok(None);
        };
        if asset["kind"] == "link" {
            return Ok(None);
        }
        let path = asset["storagePath"]
            .as_str()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| string(asset, "filePath"));
        Ok(Some(Download {
            asset: public_asset(asset.clone()),
            path: path.into(),
            size: fs::metadata(path)?.len(),
        }))
    }
    pub fn delete(&mut self, asset_id: &str) -> Result<bool> {
        self.reconcile()?;
        let Some(index) = self.assets().iter().position(|a| a["id"] == asset_id) else {
            return Ok(false);
        };
        let asset = self.assets_mut()?.remove(index);
        if let Err(error) = self.save() {
            self.assets_mut()?.insert(index, asset);
            return Err(error);
        }
        let path = Path::new(string(&asset, "storagePath"));
        if !path.as_os_str().is_empty() && self.managed(path) {
            let _ = fs::remove_file(path);
        }
        Ok(true)
    }
    pub fn archive_generated(
        &mut self,
        path: &Path,
        session_id: &str,
        session_name: &str,
    ) -> Result<Option<Value>> {
        self.reconcile()?;
        let path = absolute(path)?;
        let Ok(metadata) = fs::metadata(&path) else {
            return Ok(None);
        };
        if !metadata.is_file() {
            return Ok(None);
        }
        let path_value = path.to_string_lossy().to_string();
        if let Some(existing) = self.assets().iter().find(|a| {
            string(a, "filePath") == path_value
                || a["references"]
                    .as_array()
                    .is_some_and(|rs| rs.iter().any(|r| string(r, "filePath") == path_value))
        }) {
            return Ok(Some(public_asset(existing.clone())));
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let now = time();
        let asset = json!({"id":Uuid::new_v4().to_string(),"kind":if content::is_image(&name){"image"}else{"file"},"name":name,"mimeType":content::mime_from_name(&name),"size":metadata.len(),"filePath":path,"source":"agent","sessionId":session_id,"sessionName":session_name,"created":now,"modified":now});
        self.assets_mut()?.insert(0, asset.clone());
        self.save()?;
        Ok(Some(public_asset(asset)))
    }
    pub fn generated_for_session(&self, session: &str) -> Vec<Value> {
        let mut result = Vec::new();
        for asset in self.assets() {
            let occurrences = std::iter::once(asset.clone()).chain(
                asset["references"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        let mut value = asset.clone();
                        if let (Some(target), Some(reference)) =
                            (value.as_object_mut(), r.as_object())
                        {
                            target.extend(reference.clone());
                        }
                        value
                    }),
            );
            result.extend(
                occurrences
                    .filter(|a| {
                        a["sessionId"] == session
                            && a["source"] == "agent"
                            && (string(a, "mimeType").starts_with("image/")
                                || string(a, "mimeType").starts_with("video/")
                                || !string(a, "filePath").is_empty())
                    })
                    .map(public_asset),
            );
        }
        result.sort_by(|a, b| string(a, "created").cmp(string(b, "created")));
        result
    }
    pub fn reconcile(&mut self) -> Result<bool> {
        let original = self.index.clone();
        let mut retained = Vec::<Value>::new();
        let mut cleanup = Vec::<PathBuf>::new();
        for mut asset in self.assets().to_vec() {
            if asset["kind"] == "link" {
                retained.push(asset);
                continue;
            }
            let generated = !string(&asset, "filePath").is_empty();
            let mut paths = vec![if generated {
                string(&asset, "filePath")
            } else {
                string(&asset, "storagePath")
            }
            .to_owned()];
            if generated {
                if let Some(rs) = asset["references"].as_array() {
                    paths.extend(rs.iter().map(|r| string(r, "filePath").to_owned()));
                }
            }
            let readable = paths.iter().find(|p| Path::new(p).is_file());
            let Some(path) = readable else {
                let storage = PathBuf::from(string(&asset, "storagePath"));
                if self.managed(&storage) {
                    cleanup.push(storage)
                }
                continue;
            };
            let metadata = fs::metadata(path)?;
            asset["size"] = json!(metadata.len());
            if generated {
                asset["filePath"] = json!(absolute(Path::new(path))?);
                if let Some(storage) = asset["storagePath"].as_str() {
                    let storage = PathBuf::from(storage);
                    if self.managed(&storage) {
                        cleanup.push(storage)
                    }
                    asset
                        .as_object_mut()
                        .context("Invalid asset")?
                        .remove("storagePath");
                }
            } else if string(&asset, "hash").is_empty() {
                let mut file = File::open(path)?;
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 65536];
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&buffer[..n]);
                }
                asset["hash"] = json!(hash
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>());
            }
            let key = if generated {
                format!(
                    "path:{}",
                    string(&asset, "filePath").replace('\\', "/").to_lowercase()
                )
            } else {
                format!("hash:{}", string(&asset, "hash"))
            };
            if let Some(existing) = retained.iter_mut().find(|a| {
                if generated {
                    format!(
                        "path:{}",
                        string(a, "filePath").replace('\\', "/").to_lowercase()
                    ) == key
                } else {
                    a["kind"] != "link" && format!("hash:{}", string(a, "hash")) == key
                }
            }) {
                let own = reference(existing, &asset, asset["size"].as_u64().unwrap_or(0));
                references_mut(existing)?.push(own);
                if let Some(rs) = asset["references"].as_array() {
                    references_mut(existing)?.extend(rs.clone());
                }
                let storage = PathBuf::from(string(&asset, "storagePath"));
                if self.managed(&storage)
                    && string(existing, "storagePath") != storage.to_string_lossy()
                {
                    cleanup.push(storage)
                }
            } else {
                retained.push(asset)
            }
        }
        self.index["assets"] = json!(retained);
        if self.index == original {
            return Ok(false);
        }
        if let Err(error) = self.save() {
            self.index = original;
            return Err(error);
        }
        for path in cleanup {
            if !self
                .assets()
                .iter()
                .any(|a| Path::new(string(a, "storagePath")) == path)
            {
                let _ = fs::remove_file(path);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, AssetStore) {
        let root = std::env::temp_dir().join(format!("pisper-assets-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let store = AssetStore::open(&root).unwrap();
        (root, store)
    }
    #[test]
    fn release_index_restart_dedup_and_session_references() {
        let (root, mut store) = fixture();
        let first = store
            .create(&json!({"name":"first.txt","text":"中文🦀","sessionId":"a"}))
            .unwrap();
        let duplicate=store.create(&json!({"name":"second.txt","text":"中文🦀","sessionId":"b","sessionName":"第二会话"})).unwrap();
        assert_eq!(first["id"], duplicate["id"]);
        assert!(first.get("storagePath").is_none());
        drop(store);
        let mut reopened = AssetStore::open(&root).unwrap();
        assert_eq!(reopened.list("", "", "a").unwrap().len(), 1);
        assert_eq!(
            reopened.list("", "", "b").unwrap()[0]["sessionName"],
            "第二会话"
        );
        assert_eq!(
            reopened
                .content(first["id"].as_str().unwrap(), false)
                .unwrap()
                .unwrap()["text"],
            "中文🦀"
        );
        assert!(reopened.delete(first["id"].as_str().unwrap()).unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn generated_files_are_live_and_never_deleted_by_asset_removal() {
        let (root, mut store) = fixture();
        let file = root.join("generated.rs");
        fs::write(&file, "old").unwrap();
        let asset = store
            .archive_generated(&file, "session", "name")
            .unwrap()
            .unwrap();
        fs::write(&file, "new content").unwrap();
        assert_eq!(
            store
                .content(asset["id"].as_str().unwrap(), false)
                .unwrap()
                .unwrap()["text"],
            "new content"
        );
        assert_eq!(store.generated_for_session("session").len(), 1);
        assert!(store.delete(asset["id"].as_str().unwrap()).unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), "new content");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn binary_document_preview_and_broken_index_preservation() {
        let (root, mut store) = fixture();
        let asset = store
            .create(&json!({"name":"test.docx","data":STANDARD.encode([0,255,1,2])}))
            .unwrap();
        assert_eq!(
            store
                .content(asset["id"].as_str().unwrap(), true)
                .unwrap()
                .unwrap()["kind"],
            "document"
        );
        assert_eq!(
            store
                .content(asset["id"].as_str().unwrap(), false)
                .unwrap()
                .unwrap()["extension"],
            "docx"
        );
        drop(store);
        let index = root.join("pisper-assets.json");
        fs::write(&index, "invalid JSON").unwrap();
        assert!(AssetStore::open(&root).is_err());
        assert_eq!(fs::read_to_string(index).unwrap(), "invalid JSON");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reconciliation_preserves_extensions_and_never_removes_external_files() {
        let (root, store) = fixture();
        drop(store);
        let external = root.join("workspace-source.bin");
        fs::write(&external, [0, 255, 1]).unwrap();
        fs::write(root.join("pisper-assets.json"), serde_json::to_vec(&json!({"futureIndexField":{"keep":true},"assets":[{"id":"synthetic-external","kind":"file","name":"source.bin","storagePath":external,"size":3,"futureAssetField":"keep"}]})).unwrap()).unwrap();
        let mut reopened = AssetStore::open(&root).unwrap();
        assert_eq!(
            reopened.list("", "", "").unwrap()[0]["futureAssetField"],
            "keep"
        );
        reopened.delete("synthetic-external").unwrap();
        assert!(external.exists());
        let index: Value =
            serde_json::from_slice(&fs::read(root.join("pisper-assets.json")).unwrap()).unwrap();
        assert_eq!(index["futureIndexField"]["keep"], true);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
}
