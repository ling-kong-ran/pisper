//! 素材只暴露严格引用；读取时重新检查文件类型、尺寸和摘要，路径在代理调用边界生成。
use super::{id, inputs, Result, WorkflowError};
use crate::workflow_engine::MediaInputs;
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;

pub(crate) type BundleFiles = BTreeMap<String, Vec<u8>>;
pub(crate) struct StoredMedia {
    pub(crate) metadata: Value,
    pub(crate) buffer: Vec<u8>,
    pub(crate) path: PathBuf,
}
pub(crate) struct ImportedMedia {
    pub(crate) mapping: HashMap<String, Value>,
    token: String,
}
pub(crate) struct MediaService {
    root: PathBuf,
    closed: AtomicBool,
    ownership: Mutex<HashMap<String, Vec<String>>>,
}
fn error(code: &str) -> WorkflowError {
    WorkflowError::coded(code, code)
}
fn missing(error_value: std::io::Error) -> WorkflowError {
    if error_value.kind() == std::io::ErrorKind::NotFound {
        WorkflowError {
            status: StatusCode::NOT_FOUND,
            code: "workflow_media_missing".into(),
            message: "workflow_media_missing".into(),
            partial_output: None,
        }
    } else {
        WorkflowError::io(error_value)
    }
}
fn too_large() -> WorkflowError {
    WorkflowError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        code: "workflow_media_too_large".into(),
        message: "workflow_media_too_large".into(),
        partial_output: None,
    }
}
fn safe_id(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(error("workflow_media_invalid"));
    }
    Ok(value)
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(crate) fn directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(missing)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || path.canonicalize().map_err(missing)? != path
    {
        return Err(error("workflow_media_invalid"));
    }
    Ok(())
}
pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir(path).map_err(WorkflowError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(WorkflowError::io)?;
    }
    directory(path)
}
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(WorkflowError::io)?;
    file.write_all(bytes).map_err(WorkflowError::io)?;
    file.sync_all().map_err(WorkflowError::io)
}
pub(crate) fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let before = std::fs::symlink_metadata(path).map_err(missing)?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > maximum {
        return Err(error("workflow_media_invalid"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000);
    }
    let file: File = options.open(path).map_err(missing)?;
    let current = file.metadata().map_err(WorkflowError::io)?;
    if !current.is_file() || current.len() == 0 || current.len() > maximum {
        return Err(error("workflow_media_invalid"));
    }
    let mut bytes = Vec::with_capacity(current.len() as usize);
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(WorkflowError::io)?;
    if bytes.len() as u64 != current.len() || bytes.len() as u64 > maximum {
        return Err(error("workflow_media_invalid"));
    }
    Ok(bytes)
}
pub(crate) fn raster_dimensions(bytes: &[u8]) -> Option<(u32, u32, &'static str)> {
    if bytes.len() >= 24 && bytes[..8] == *b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR" {
        return Some((
            u32::from_be_bytes(bytes[16..20].try_into().ok()?),
            u32::from_be_bytes(bytes[20..24].try_into().ok()?),
            "image/png",
        ));
    }
    if bytes.len() >= 30 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let uint24 =
            |offset| u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], 0]);
        match &bytes[12..16] {
            b"VP8X" => return Some((uint24(24) + 1, uint24(27) + 1, "image/webp")),
            b"VP8L" if bytes[20] == 47 => {
                return Some((
                    1 + bytes[21] as u32 + (((bytes[22] & 63) as u32) << 8),
                    1 + (bytes[22] >> 6) as u32
                        + ((bytes[23] as u32) << 2)
                        + (((bytes[24] & 15) as u32) << 10),
                    "image/webp",
                ))
            }
            b"VP8 " if bytes[23..26] == [157, 1, 42] => {
                return Some((
                    (u16::from_le_bytes(bytes[26..28].try_into().ok()?) & 16383) as u32,
                    (u16::from_le_bytes(bytes[28..30].try_into().ok()?) & 16383) as u32,
                    "image/webp",
                ))
            }
            _ => {}
        }
    }
    if bytes.len() >= 4 && bytes[..2] == [255, 216] {
        let mut offset = 2;
        while offset + 4 <= bytes.len() {
            if bytes[offset] != 255 {
                return None;
            }
            while bytes.get(offset) == Some(&255) {
                offset += 1;
            }
            let marker = *bytes.get(offset)?;
            offset += 1;
            if [217, 218].contains(&marker) {
                return None;
            }
            if marker == 1 || (208..=215).contains(&marker) {
                continue;
            }
            let size = u16::from_be_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as usize;
            if size < 2 || offset + size > bytes.len() {
                return None;
            }
            if [
                192, 193, 194, 195, 197, 198, 199, 201, 202, 203, 205, 206, 207,
            ]
            .contains(&marker)
            {
                if size < 8 {
                    return None;
                }
                return Some((
                    u16::from_be_bytes(bytes[offset + 5..offset + 7].try_into().ok()?) as u32,
                    u16::from_be_bytes(bytes[offset + 3..offset + 5].try_into().ok()?) as u32,
                    "image/jpeg",
                ));
            }
            offset += size;
        }
    }
    None
}
pub(crate) fn validate_bytes(bytes: &[u8], mime: &str) -> Result<()> {
    if bytes.is_empty()
        || bytes.len()
            > if mime.starts_with("image/") {
                8 * 1024 * 1024
            } else {
                64 * 1024 * 1024
            }
    {
        return Err(too_large());
    }
    if mime.starts_with("image/") {
        let (width, height, actual) =
            raster_dimensions(bytes).ok_or_else(|| error("workflow_media_invalid"))?;
        if actual != mime || width == 0 || height == 0 {
            return Err(error("workflow_media_invalid"));
        }
        if width > 4096 || height > 4096 || width as u64 * height as u64 > 16_000_000 {
            return Err(too_large());
        }
    } else if mime == "video/mp4" {
        if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
            return Err(error("workflow_media_invalid"));
        }
    } else if mime == "video/webm" {
        if bytes.len() < 8 || bytes[..4] != [0x1a, 0x45, 0xdf, 0xa3] {
            return Err(error("workflow_media_invalid"));
        }
    } else {
        let mut failure = error("workflow_media_invalid");
        failure.status = StatusCode::UNSUPPORTED_MEDIA_TYPE;
        return Err(failure);
    }
    Ok(())
}
fn metadata(value: Value) -> Result<Value> {
    if !value.is_object()
        || value["version"] != 1
        || !value["sha256"].as_str().is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err(error("workflow_media_invalid"));
    }
    Ok(json!({"version":1,"media":inputs::media(&value["media"])? ,"sha256":value["sha256"]}))
}
impl MediaService {
    pub(crate) fn open(data_dir: &Path) -> Result<Arc<Self>> {
        std::fs::create_dir_all(data_dir).map_err(WorkflowError::io)?;
        let root = data_dir
            .canonicalize()
            .map_err(WorkflowError::io)?
            .join("workflow-media");
        if !root.exists() {
            create_private_directory(&root)?;
        }
        directory(&root)?;
        Ok(Arc::new(Self {
            root,
            closed: AtomicBool::new(false),
            ownership: Mutex::new(HashMap::new()),
        }))
    }
    fn check_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            let mut failure = error("workflow_media_closed");
            failure.status = StatusCode::SERVICE_UNAVAILABLE;
            Err(failure)
        } else {
            directory(&self.root)
        }
    }
    fn media_directory(&self, media_id: &str) -> Result<PathBuf> {
        directory(&self.root)?;
        let path = self.root.join(safe_id(media_id)?);
        directory(&path)?;
        Ok(path)
    }
    fn write(&self, media: &Value, bytes: &[u8]) -> Result<Value> {
        directory(&self.root)?;
        let path = self.root.join(safe_id(media["id"].as_str().unwrap_or(""))?);
        create_private_directory(&path)?;
        let result = (|| {
            write_private(&path.join("data.bin"), bytes)?;
            write_private(
                &path.join("metadata.json"),
                &serde_json::to_vec(&json!({"version":1,"media":media,"sha256":digest(bytes)}))
                    .map_err(WorkflowError::io)?,
            )?;
            Ok(media.clone())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&path);
        }
        result
    }
    fn load(&self, media_id: &str) -> Result<StoredMedia> {
        let path = self.media_directory(media_id)?;
        let info = metadata(
            serde_json::from_slice(&read_bounded(&path.join("metadata.json"), 4096)?)
                .map_err(|_| error("workflow_media_invalid"))?,
        )?;
        let buffer = read_bounded(&path.join("data.bin"), 64 * 1024 * 1024)?;
        if info["media"]["id"] != media_id
            || info["media"]["size"].as_u64() != Some(buffer.len() as u64)
            || info["sha256"] != digest(&buffer)
        {
            return Err(error("workflow_media_invalid"));
        }
        validate_bytes(&buffer, info["media"]["mimeType"].as_str().unwrap_or(""))?;
        Ok(StoredMedia {
            metadata: info,
            buffer,
            path: path.join("data.bin"),
        })
    }
    pub(crate) async fn upload(&self, name: &str, mime: &str, bytes: &[u8]) -> Result<Value> {
        validate_bytes(bytes, mime)?;
        let name = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .trim()
            .encode_utf16()
            .take(160)
            .collect::<Vec<_>>();
        let name = String::from_utf16_lossy(&name);
        let name = if name.is_empty() { "media" } else { &name };
        let media =
            inputs::media(&json!({"id":id()?,"name":name,"mimeType":mime,"size":bytes.len()}))?;
        let _guard = self.ownership.lock().await;
        self.check_open()?;
        self.write(&media, bytes)
    }
    pub(crate) async fn read(&self, media_id: &str) -> Result<StoredMedia> {
        let _guard = self.ownership.lock().await;
        self.check_open()?;
        self.load(media_id)
    }
    pub(crate) async fn resolve_inputs(&self, supplied: &Value) -> Result<MediaInputs> {
        let _guard = self.ownership.lock().await;
        self.check_open()?;
        let mut attachments = Vec::new();
        let mut context = Vec::new();
        let mut image_bytes = 0_u64;
        for (name, value) in supplied
            .as_object()
            .ok_or_else(|| error("workflow_media_invalid"))?
        {
            if !value.is_object() {
                continue;
            }
            let media = inputs::media(value)?;
            let stored = self.load(media["id"].as_str().unwrap_or(""))?;
            if media != stored.metadata["media"] {
                return Err(error("workflow_media_invalid"));
            }
            let video = media["mimeType"]
                .as_str()
                .unwrap_or("")
                .starts_with("video/");
            if !video {
                image_bytes += media["size"].as_u64().unwrap_or(0);
                if attachments.len() >= 8 || image_bytes > 20 * 1024 * 1024 {
                    return Err(too_large());
                }
                attachments.push(json!({"kind":"image","name":media["name"],"mimeType":media["mimeType"],"size":media["size"],"data":STANDARD.encode(&stored.buffer)}));
            }
            context.push(format!("{}: {}\nLocal media path: {}{}",name,media["name"].as_str().unwrap_or(""),serde_json::to_string(&stored.path.to_string_lossy()).map_err(WorkflowError::io)?,if video { "\nThis is a video file reference. Use available file/media tools; it is not a native video model attachment." } else { "" }));
        }
        Ok(MediaInputs {
            attachments,
            context: context.join("\n\n"),
        })
    }
    pub(crate) async fn export_files(&self, workflow: &Value) -> Result<BundleFiles> {
        let definitions = inputs::definitions(workflow.get("inputs"))?;
        let _guard = self.ownership.lock().await;
        self.check_open()?;
        let mut included = HashMap::new();
        let mut files = BundleFiles::new();
        let mut total = 0_u64;
        for input in definitions {
            if !input["defaultValue"].is_object() {
                continue;
            }
            let media = inputs::media(&input["defaultValue"])?;
            let media_id = media["id"].as_str().unwrap_or("");
            if let Some(existing) = included.get(media_id) {
                if existing != &media {
                    return Err(error("workflow_media_invalid"));
                }
                continue;
            }
            total += media["size"].as_u64().unwrap_or(0);
            if total > 128 * 1024 * 1024 {
                return Err(too_large());
            }
            let stored = self.load(media_id)?;
            if stored.metadata["media"] != media {
                return Err(error("workflow_media_invalid"));
            }
            files.insert(
                format!("media/{media_id}/metadata.json"),
                serde_json::to_vec(&stored.metadata).map_err(WorkflowError::io)?,
            );
            files.insert(format!("media/{media_id}/data.bin"), stored.buffer);
            included.insert(media_id.to_string(), media);
        }
        Ok(files)
    }
    pub(crate) fn validate_bundle(files: &BundleFiles) -> Result<Vec<(Value, Vec<u8>)>> {
        let mut ids = HashSet::new();
        for name in files.keys() {
            let parts = name.split('/').collect::<Vec<_>>();
            if parts.len() != 3
                || parts[0] != "media"
                || !["metadata.json", "data.bin"].contains(&parts[2])
            {
                return Err(error("workflow_media_invalid"));
            }
            ids.insert(safe_id(parts[1])?.to_string());
        }
        let mut parsed = Vec::new();
        for media_id in ids {
            let data = files
                .get(&format!("media/{media_id}/metadata.json"))
                .filter(|bytes| bytes.len() <= 4096)
                .ok_or_else(|| error("workflow_media_missing"))?;
            let bytes = files
                .get(&format!("media/{media_id}/data.bin"))
                .ok_or_else(|| error("workflow_media_missing"))?;
            let info = metadata(
                serde_json::from_slice(data).map_err(|_| error("workflow_media_invalid"))?,
            )?;
            if info["media"]["id"] != media_id
                || info["media"]["size"].as_u64() != Some(bytes.len() as u64)
                || info["sha256"] != digest(bytes)
            {
                return Err(error("workflow_media_invalid"));
            }
            validate_bytes(bytes, info["media"]["mimeType"].as_str().unwrap_or(""))?;
            parsed.push((info, bytes.clone()));
        }
        Ok(parsed)
    }
    pub(crate) async fn import_files(&self, files: &BundleFiles) -> Result<ImportedMedia> {
        let parsed = Self::validate_bundle(files)?;
        let mut ownership = self.ownership.lock().await;
        self.check_open()?;
        let mut mapping = HashMap::new();
        let mut owned = Vec::new();
        for (info, bytes) in parsed {
            let mut media = info["media"].clone();
            let original = media["id"].as_str().unwrap_or("").to_string();
            media["id"] = json!(id()?);
            match self.write(&media, &bytes) {
                Ok(media) => {
                    owned.push(media["id"].as_str().unwrap_or("").to_string());
                    mapping.insert(original, media);
                }
                Err(failure) => {
                    for media_id in owned {
                        if let Ok(path) = self.media_directory(&media_id) {
                            let _ = std::fs::remove_dir_all(path);
                        }
                    }
                    return Err(failure);
                }
            }
        }
        let token = id()?;
        ownership.insert(token.clone(), owned);
        Ok(ImportedMedia { mapping, token })
    }
    pub(crate) async fn discard_imported(&self, mapping: ImportedMedia) -> Result<()> {
        let mut ownership = self.ownership.lock().await;
        self.check_open()?;
        let owned = ownership
            .get(&mapping.token)
            .ok_or_else(|| error("workflow_media_invalid"))?;
        for media_id in owned {
            std::fs::remove_dir_all(self.media_directory(media_id)?).map_err(WorkflowError::io)?;
        }
        ownership.remove(&mapping.token);
        Ok(())
    }
    pub(crate) async fn commit_imported(&self, mapping: ImportedMedia) -> Result<()> {
        let mut ownership = self.ownership.lock().await;
        ownership
            .remove(&mapping.token)
            .ok_or_else(|| error("workflow_media_invalid"))?;
        Ok(())
    }
    pub(crate) async fn dispose(&self) {
        self.closed.store(true, Ordering::Release);
        let _guard = self.ownership.lock().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::test_support::TempDirectory;
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes
    }
    #[tokio::test]
    async fn real_media_roundtrip_verifies_refs_hashes_attachments_and_restart() {
        let directory = TempDirectory::new();
        let service = MediaService::open(&directory.path).unwrap();
        let media = service
            .upload("../source.png", "image/png", &png(2, 3))
            .await
            .unwrap();
        assert_eq!(media["name"], "source.png");
        let resolved = service
            .resolve_inputs(&json!({"reference":media}))
            .await
            .unwrap();
        assert_eq!(resolved.attachments.len(), 1);
        assert_eq!(
            STANDARD
                .decode(resolved.attachments[0]["data"].as_str().unwrap())
                .unwrap(),
            png(2, 3)
        );
        assert!(resolved.context.contains("data.bin"));
        let mut forged = media.clone();
        forged["name"] = json!("forged.png");
        assert_eq!(
            service
                .resolve_inputs(&json!({"reference":forged}))
                .await
                .err()
                .unwrap()
                .code,
            "workflow_media_invalid"
        );
        service.dispose().await;
        let restarted = MediaService::open(&directory.path).unwrap();
        let stored = restarted.read(media["id"].as_str().unwrap()).await.unwrap();
        assert_eq!(stored.metadata["media"], media);
        std::fs::write(stored.path, png(3, 2)).unwrap();
        assert_eq!(
            restarted
                .read(media["id"].as_str().unwrap())
                .await
                .err()
                .unwrap()
                .code,
            "workflow_media_invalid"
        );
    }
    #[tokio::test]
    async fn imports_use_new_ids_and_rollback_only_owned_media() {
        let directory = TempDirectory::new();
        let service = MediaService::open(&directory.path).unwrap();
        let original = service
            .upload("source.png", "image/png", &png(2, 2))
            .await
            .unwrap();
        let workflow =
            json!({"inputs":[{"name":"reference","type":"image","defaultValue":original}]});
        let files = service.export_files(&workflow).await.unwrap();
        let imported = service.import_files(&files).await.unwrap();
        let new_id = imported.mapping[original["id"].as_str().unwrap()]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(new_id, original["id"]);
        service.discard_imported(imported).await.unwrap();
        assert_eq!(
            service.read(&new_id).await.err().unwrap().status,
            StatusCode::NOT_FOUND
        );
        assert!(service.read(original["id"].as_str().unwrap()).await.is_ok());
        let mut broken = files;
        broken
            .get_mut(&format!(
                "media/{}/data.bin",
                original["id"].as_str().unwrap()
            ))
            .unwrap()[20] ^= 1;
        assert_eq!(
            MediaService::validate_bundle(&broken).err().unwrap().code,
            "workflow_media_invalid"
        );
    }
    #[test]
    fn raster_and_video_headers_enforce_actual_type_and_pixel_bounds() {
        assert!(validate_bytes(&png(1, 1), "image/png").is_ok());
        assert_eq!(
            validate_bytes(&png(4096, 4096), "image/png")
                .unwrap_err()
                .status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert!(validate_bytes(&png(1, 1), "image/jpeg").is_err());
        let mut mp4 = vec![0; 16];
        mp4[4..8].copy_from_slice(b"ftyp");
        assert!(validate_bytes(&mp4, "video/mp4").is_ok());
        assert!(validate_bytes(b"video/mp4", "video/mp4").is_err());
    }
}
