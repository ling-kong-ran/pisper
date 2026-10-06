//! 固定摘要的可移植图片引擎缓存；下载与 CPU 执行适配器共享此验证边界。
use super::{
    bundle::EngineBundleStore,
    media::{self, BundleFiles},
    Result, WorkflowError,
};
use axum::http::StatusCode;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;
#[derive(Clone)]
pub(crate) struct EngineFile {
    pub(crate) name: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
    pub(crate) mime_type: String,
    pub(crate) urls: Vec<String>,
}
#[derive(Clone)]
pub(crate) struct EngineDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) files: Vec<EngineFile>,
}
pub(crate) fn release_definitions() -> Vec<EngineDefinition> {
    let file = |name: &str, bytes: usize, sha: &str, mime: &str, url: &str| EngineFile {
        name: name.into(),
        bytes,
        sha256: sha.into(),
        mime_type: mime.into(),
        urls: vec![url.into()],
    };
    let mut background=vec![
        file("ort.wasm.min.js",49026,"e66568724f8848e57cc2a56e4bea3a5b86ce3ff81b2da11eac8ed7ab02b27bd4","text/javascript","https://cdn.jsdelivr.net/npm/onnxruntime-web@1.20.1/dist/ort.wasm.min.js"),
        file("ort-wasm-simd-threaded.mjs",24618,"745eb7c0ce6f18a6aa521971b2877babc7ffb27eecb58ab3bc6e5ef4692672e8","text/javascript","https://cdn.jsdelivr.net/npm/onnxruntime-web@1.20.1/dist/ort-wasm-simd-threaded.mjs"),
        file("ort-wasm-simd-threaded.wasm",11246032,"207d02be4591c156b0a98f024f3d58005b5b04c92274d759fb390338c63559ea","application/wasm","https://cdn.jsdelivr.net/npm/onnxruntime-web@1.20.1/dist/ort-wasm-simd-threaded.wasm"),
        file("u2netp.onnx",4574861,"309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8","application/octet-stream","https://gh-proxy.com/https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx"),
        file("LICENSE-ORT.txt",1073,"2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c","text/plain; charset=utf-8","https://cdn.jsdelivr.net/gh/microsoft/onnxruntime@v1.20.1/LICENSE"),
        file("LICENSE-U2NET.txt",11357,"c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4","text/plain; charset=utf-8","https://cdn.jsdelivr.net/gh/xuebinqin/U-2-Net@ac7e1c817ecab7c7dff5ce6b1abba61cd213ff29/LICENSE"),
    ];
    background[3].urls.push("https://ghfast.top/https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx".into());
    vec![EngineDefinition{id:"background".into(),name:"U²-Net small + ONNX Runtime Web".into(),version:"ort-1.20.1_u2netp-v0.0.0".into(),files:background},EngineDefinition{id:"inpaint".into(),name:"OpenCV.js inpainting".into(),version:"5.0.0-release.1".into(),files:vec![
        file("opencv.js",13298869,"b873c8211421da7b9bf41ae157a923f05a46a0b8d3e5904c44c6f3ad6d39a1bd","text/javascript","https://cdn.jsdelivr.net/npm/@techstark/opencv-js@5.0.0-release.1/dist/opencv.js"),
        file("LICENSE-OpenCV.txt",11358,"cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30","text/plain; charset=utf-8","https://cdn.jsdelivr.net/gh/opencv/opencv@5.0.0/LICENSE"),
        file("LICENSE-OpenCV-JS.txt",11357,"c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4","text/plain; charset=utf-8","https://cdn.jsdelivr.net/gh/TechStark/opencv-js@v5.0.0-release.1/LICENSE"),
    ]}]
}
pub(crate) struct EngineCache {
    root: PathBuf,
    definitions: Vec<EngineDefinition>,
    queue: Mutex<()>,
    closed: AtomicBool,
}
fn failure(code: &str, status: StatusCode) -> WorkflowError {
    let mut error = WorkflowError::coded(code, "本地图片引擎资源不可用，请重试。");
    error.status = status;
    error
}
fn invalid() -> WorkflowError {
    failure("sprite_engine_bundle_invalid", StatusCode::BAD_REQUEST)
}
fn unsafe_storage() -> WorkflowError {
    failure("sprite_engine_storage_unsafe", StatusCode::CONFLICT)
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        && name != ".."
}
fn validate(bytes: &[u8], file: &EngineFile) -> Result<()> {
    if bytes.len() != file.bytes || media::digest(bytes) != file.sha256 {
        Err(failure("sprite_engine_integrity", StatusCode::BAD_REQUEST))
    } else {
        Ok(())
    }
}
fn installation(path: &Path) -> Result<Value> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(_) => Err(unsafe_storage()),
        Ok(info) if !info.is_file() || info.file_type().is_symlink() => Err(unsafe_storage()),
        Ok(_) => {
            serde_json::from_slice(&media::read_bounded(path, 4096).map_err(|_| unsafe_storage())?)
                .map_err(|_| unsafe_storage())
        }
    }
}
impl EngineCache {
    pub(crate) fn open(data_dir: &Path, definitions: Vec<EngineDefinition>) -> Result<Arc<Self>> {
        let mut ids = HashSet::new();
        if definitions.is_empty()
            || definitions.len() > 2
            || definitions.iter().any(|definition| {
                !["background", "inpaint"].contains(&definition.id.as_str())
                    || !ids.insert(definition.id.clone())
                    || definition.version.is_empty()
                    || definition.files.is_empty()
                    || definition.files.len() > 30
            })
        {
            return Err(invalid());
        }
        for definition in &definitions {
            let mut names = HashSet::new();
            if definition.files.iter().any(|file| {
                !safe_name(&file.name)
                    || !names.insert(file.name.to_ascii_lowercase())
                    || file.bytes == 0
                    || file.bytes > 64 * 1024 * 1024
                    || file.sha256.len() != 64
                    || !file
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }) {
                return Err(invalid());
            }
        }
        std::fs::create_dir_all(data_dir).map_err(|_| unsafe_storage())?;
        let root = data_dir
            .canonicalize()
            .map_err(|_| unsafe_storage())?
            .join("sprite-engines");
        if !root.exists() {
            media::create_private_directory(&root).map_err(|_| unsafe_storage())?;
        }
        media::directory(&root).map_err(|_| unsafe_storage())?;
        for definition in &definitions {
            let directory = root.join(&definition.id);
            if !directory.exists() {
                media::create_private_directory(&directory).map_err(|_| unsafe_storage())?;
            }
            media::directory(&directory).map_err(|_| unsafe_storage())?;
            for entry in std::fs::read_dir(&directory).map_err(|_| unsafe_storage())? {
                let entry = entry.map_err(|_| unsafe_storage())?;
                if entry.file_name().to_string_lossy().starts_with("staging-") {
                    media::directory(&entry.path()).map_err(|_| unsafe_storage())?;
                    std::fs::remove_dir_all(entry.path()).map_err(|_| unsafe_storage())?;
                }
            }
        }
        Ok(Arc::new(Self {
            root,
            definitions,
            queue: Mutex::new(()),
            closed: AtomicBool::new(false),
        }))
    }
    pub(crate) fn definitions(&self) -> &[EngineDefinition] {
        &self.definitions
    }
    fn check(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(failure(
                "sprite_engine_closed",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
        media::directory(&self.root).map_err(|_| unsafe_storage())
    }
    fn definition(&self, id: &str) -> Result<&EngineDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
            .ok_or_else(|| failure("sprite_engine_not_found", StatusCode::NOT_FOUND))
    }
    fn root_for(&self, id: &str) -> Result<PathBuf> {
        self.definition(id)?;
        let path = self.root.join(id);
        media::directory(&path).map_err(|_| unsafe_storage())?;
        Ok(path)
    }
    fn installed(&self, id: &str) -> Result<PathBuf> {
        let definition = self.definition(id)?;
        let root = self.root_for(id)?;
        let metadata = installation(&root.join("installed.json"))?;
        let name = metadata["directory"]
            .as_str()
            .filter(|name| safe_name(name) && name.starts_with("install-"))
            .ok_or_else(|| failure("sprite_engine_missing", StatusCode::CONFLICT))?;
        if metadata["version"] != definition.version {
            return Err(failure("sprite_engine_missing", StatusCode::CONFLICT));
        }
        let path = root.join(name);
        media::directory(&path).map_err(|_| unsafe_storage())?;
        for file in &definition.files {
            let bytes = media::read_bounded(&path.join(&file.name), file.bytes as u64)
                .map_err(|_| failure("sprite_engine_integrity", StatusCode::BAD_REQUEST))?;
            validate(&bytes, file)?;
        }
        Ok(path)
    }
    pub(crate) async fn execution_directory(&self, id: &str) -> Result<PathBuf> {
        let _guard = self.queue.lock().await;
        self.check()?;
        self.installed(id)
    }
    /// 仅由 spawn_blocking 内的 CPU 执行器调用；返回已校验快照，删除不会影响运行中的模型。
    pub(crate) fn execution_files_blocking(&self, id: &str) -> Result<BundleFiles> {
        let _guard = self.queue.blocking_lock();
        self.check()?;
        let directory = self.installed(id)?;
        let mut files = BundleFiles::new();
        for file in &self.definition(id)?.files {
            let bytes = media::read_bounded(&directory.join(&file.name), file.bytes as u64)?;
            validate(&bytes, file)?;
            files.insert(file.name.clone(), bytes);
        }
        Ok(files)
    }
    pub(crate) async fn remove(&self, id: &str) -> Result<()> {
        let _guard = self.queue.lock().await;
        self.check()?;
        let root = self.root_for(id)?;
        let metadata = root.join("installed.json");
        if metadata.exists() {
            let info = std::fs::symlink_metadata(&metadata).map_err(|_| unsafe_storage())?;
            if !info.is_file() || info.file_type().is_symlink() {
                return Err(unsafe_storage());
            }
            std::fs::remove_file(&metadata).map_err(|_| unsafe_storage())?;
        }
        for entry in std::fs::read_dir(&root).map_err(|_| unsafe_storage())? {
            let entry = entry.map_err(|_| unsafe_storage())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name
                .strip_prefix("install-")
                .or_else(|| name.strip_prefix("staging-"))
            {
                if uuid::Uuid::parse_str(id).is_ok() {
                    let path = entry.path();
                    media::directory(&path).map_err(|_| unsafe_storage())?;
                    std::fs::remove_dir_all(path).map_err(|_| unsafe_storage())?;
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn catalog(&self) -> Result<Value> {
        let _guard = self.queue.lock().await;
        self.check()?;
        let mut engines = Vec::new();
        for definition in &self.definitions {
            let bytes = definition
                .files
                .iter()
                .map(|file| file.bytes)
                .sum::<usize>();
            let ready = self.installed(&definition.id).is_ok();
            engines.push(json!({"id":definition.id,"name":definition.name,"version":definition.version,"bytes":bytes,"status":if ready{"ready"}else{"missing"},"received":if ready{bytes}else{0},"total":bytes,"error":"","file":""}));
        }
        Ok(json!({"engines":engines}))
    }
    fn validate_bundle(&self, files: &BundleFiles) -> Result<Vec<String>> {
        let mut present = HashSet::new();
        for (name, bytes) in files {
            let parts = name.split('/').collect::<Vec<_>>();
            if parts.len() != 3
                || parts[0] != "engines"
                || !safe_name(parts[1])
                || !safe_name(parts[2])
            {
                return Err(invalid());
            }
            let definition = self.definition(parts[1]).map_err(|_| invalid())?;
            let file = definition
                .files
                .iter()
                .find(|file| file.name == parts[2])
                .ok_or_else(invalid)?;
            validate(bytes, file)?;
            present.insert(parts[1].to_string());
        }
        for id in &present {
            for file in &self.definition(id)?.files {
                if !files.contains_key(&format!("engines/{id}/{}", file.name)) {
                    return Err(invalid());
                }
            }
        }
        Ok(present.into_iter().collect())
    }
    fn publish(&self, id: &str, files: &BundleFiles) -> Result<()> {
        let root = self.root_for(id)?;
        let definition = self.definition(id)?;
        let installed_path = root.join("installed.json");
        if installed_path.exists() {
            let info = std::fs::symlink_metadata(&installed_path).map_err(|_| unsafe_storage())?;
            if !info.is_file() || info.file_type().is_symlink() {
                return Err(unsafe_storage());
            }
        }
        let old = installation(&installed_path).ok().and_then(|metadata| {
            metadata["directory"]
                .as_str()
                .filter(|name| safe_name(name) && name.starts_with("install-"))
                .map(|name| root.join(name))
        });
        let staging = root.join(format!("staging-{}", super::id()?));
        media::create_private_directory(&staging).map_err(|_| unsafe_storage())?;
        let name = format!("install-{}", super::id()?);
        let install = root.join(&name);
        let result = (|| {
            for file in &definition.files {
                media::write_private(
                    &staging.join(&file.name),
                    files
                        .get(&format!("engines/{id}/{}", file.name))
                        .ok_or_else(invalid)?,
                )
                .map_err(|_| unsafe_storage())?;
            }
            std::fs::rename(&staging, &install).map_err(|_| unsafe_storage())?;
            super::save_json(
                &installed_path,
                &json!({"version":definition.version,"directory":name}),
            )
            .map_err(|_| unsafe_storage())?;
            Ok(())
        })();
        if result.is_err() {
            if media::directory(&staging).is_ok() {
                let _ = std::fs::remove_dir_all(&staging);
            }
            if media::directory(&install).is_ok() {
                let _ = std::fs::remove_dir_all(&install);
            }
            return result;
        }
        if let Some(old) = old {
            if old != install {
                media::directory(&old).map_err(|_| unsafe_storage())?;
                std::fs::remove_dir_all(old).map_err(|_| unsafe_storage())?;
            }
        }
        Ok(())
    }
    pub(crate) async fn file(&self, id: &str, name: &str) -> Result<(Vec<u8>, String)> {
        let _guard = self.queue.lock().await;
        self.check()?;
        let definition = self.definition(id)?;
        let file = definition
            .files
            .iter()
            .find(|file| file.name == name && safe_name(name))
            .ok_or_else(|| failure("sprite_engine_file_not_found", StatusCode::NOT_FOUND))?;
        let directory = self.installed(id)?;
        let bytes = media::read_bounded(&directory.join(name), file.bytes as u64)?;
        validate(&bytes, file)?;
        Ok((bytes, file.mime_type.clone()))
    }
    pub(crate) async fn dispose(&self) {
        self.closed.store(true, Ordering::Release);
        let _guard = self.queue.lock().await;
    }
}
impl EngineBundleStore for EngineCache {
    fn export_files(&self, ids: Vec<String>) -> BoxFuture<'_, Result<BundleFiles>> {
        Box::pin(async move {
            let _guard = self.queue.lock().await;
            self.check()?;
            let mut files = BundleFiles::new();
            for id in ids {
                let directory = self.installed(&id)?;
                for file in &self.definition(&id)?.files {
                    let bytes =
                        media::read_bounded(&directory.join(&file.name), file.bytes as u64)?;
                    validate(&bytes, file)?;
                    files.insert(format!("engines/{id}/{}", file.name), bytes);
                }
            }
            Ok(files)
        })
    }
    fn validate_files(&self, files: &BundleFiles) -> Result<()> {
        self.validate_bundle(files).map(|_| ())
    }
    fn install_files(&self, files: BundleFiles) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let ids = self.validate_bundle(&files)?;
            let _guard = self.queue.lock().await;
            self.check()?;
            for id in ids {
                self.publish(&id, &files)?;
            }
            Ok(())
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::test_support::TempDirectory;
    fn definitions() -> Vec<EngineDefinition> {
        vec![EngineDefinition {
            id: "background".into(),
            name: "fixture".into(),
            version: "fixture-v1".into(),
            files: vec![EngineFile {
                name: "u2netp.onnx".into(),
                bytes: 3,
                sha256: media::digest(b"abc"),
                mime_type: "application/octet-stream".into(),
                urls: vec![],
            }],
        }]
    }
    #[tokio::test]
    async fn verified_engine_import_is_atomic_restartable_and_tamper_detected() {
        let directory = TempDirectory::new();
        let cache = EngineCache::open(&directory.path, definitions()).unwrap();
        assert_eq!(
            cache.catalog().await.unwrap()["engines"][0]["status"],
            "missing"
        );
        let mut files = BundleFiles::new();
        files.insert("engines/background/u2netp.onnx".into(), b"abc".to_vec());
        cache.install_files(files.clone()).await.unwrap();
        assert_eq!(
            cache.export_files(vec!["background".into()]).await.unwrap(),
            files
        );
        let installed = cache.execution_directory("background").await.unwrap();
        cache.dispose().await;
        let restarted = EngineCache::open(&directory.path, definitions()).unwrap();
        assert_eq!(
            restarted.catalog().await.unwrap()["engines"][0]["status"],
            "ready"
        );
        let mut invalid = files.clone();
        invalid.get_mut("engines/background/u2netp.onnx").unwrap()[0] = b'd';
        assert!(restarted.install_files(invalid).await.is_err());
        assert_eq!(
            std::fs::read(installed.join("u2netp.onnx")).unwrap(),
            b"abc"
        );
        std::fs::write(installed.join("u2netp.onnx"), b"abd").unwrap();
        assert_eq!(
            restarted
                .execution_directory("background")
                .await
                .err()
                .unwrap()
                .code,
            "sprite_engine_integrity"
        );
    }
    #[test]
    fn bundle_rejects_unknown_files_paths_and_incomplete_engine_sets() {
        let directory = TempDirectory::new();
        let mut definitions = definitions();
        definitions[0].files.push(EngineFile {
            name: "license.txt".into(),
            bytes: 1,
            sha256: media::digest(b"l"),
            mime_type: "text/plain".into(),
            urls: vec![],
        });
        let cache = EngineCache::open(&directory.path, definitions).unwrap();
        let mut files = BundleFiles::new();
        files.insert("engines/background/u2netp.onnx".into(), b"abc".to_vec());
        assert!(cache.validate_files(&files).is_err());
        files.insert("engines/background/license.txt".into(), b"l".to_vec());
        assert!(cache.validate_files(&files).is_ok());
        files.insert("engines/background/../unsafe".into(), b"l".to_vec());
        assert!(cache.validate_files(&files).is_err());
    }
}
