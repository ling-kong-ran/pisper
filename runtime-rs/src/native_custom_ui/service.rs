use super::{
    archive, assets,
    model::{encode_path, normalize_manifest, valid_component_id, valid_view_id},
    AssetResponse, Component, CustomUiError, Manifest, Result, View, ViewGrant, MAX_ARCHIVE_BYTES,
    MAX_ASSET_BYTES, MAX_MANIFEST_BYTES, MAX_VIEWS, VIEW_TTL_MS,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub struct CustomUiService {
    root: PathBuf,
    views: Mutex<HashMap<String, View>>,
    mutation: Mutex<()>,
    closed: AtomicBool,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}
impl CustomUiService {
    pub fn new(data_dir: impl AsRef<Path>) -> Arc<Self> {
        Self::with_clock(
            data_dir,
            Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
                    .unwrap_or(0)
            }),
        )
    }
    pub(crate) fn with_clock(
        data_dir: impl AsRef<Path>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root: data_dir.as_ref().join("custom-ui"),
            views: Mutex::new(HashMap::new()),
            mutation: Mutex::new(()),
            closed: AtomicBool::new(false),
            now,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(CustomUiError::new(
                503,
                "component_service_closed",
                "组件服务正在关闭。",
            ));
        }
        Ok(())
    }
    pub fn dispose(&self) {
        self.closed.store(true, Ordering::Release);
        // 安装不可在 stage 写入中被取消；等待有限大小的发布完成后再退出。
        let _guard = self.mutation.lock().unwrap_or_else(|p| p.into_inner());
        self.views.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
    pub fn read_manifest(&self, id: &str) -> Option<Manifest> {
        if !valid_component_id(id) {
            return None;
        }
        if id == "pisper-island" {
            return normalize_manifest(id, &json!({
            "name":"Pisper Island", "version":"1.0.0", "description":"灵动岛：时钟、可调整的专注计时与完成提醒 / Clock, adjustable focus timer and reminders", "entry":"index.html", "permissions":["notify"]
        })).ok();
        }
        let path = self.root.join(id).join("manifest.json");
        let bytes = bounded_read(&path, MAX_MANIFEST_BYTES).ok()?;
        let raw: Value = serde_json::from_slice(&bytes).ok()?;
        normalize_manifest(id, &raw).ok()
    }
    pub fn list_components(&self) -> Result<Value> {
        self.open()?;
        let builtin = self
            .read_manifest("pisper-island")
            .expect("static builtin manifest");
        let mut components = vec![Component {
            entry_url: format!(
                "/api/custom-ui/components/{}/assets/{}",
                builtin.id, builtin.entry
            ),
            manifest: builtin,
            directory: String::new(),
            built_in: Some(true),
        }];
        match fs::read_dir(&self.root) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let id = entry.file_name().to_string_lossy().into_owned();
                    if id == "pisper-island"
                        || !valid_component_id(&id)
                        || !entry.file_type().is_ok_and(|kind| kind.is_dir())
                    {
                        continue;
                    }
                    let Some(manifest) = self.read_manifest(&id) else {
                        continue;
                    };
                    components.push(Component {
                        entry_url: format!(
                            "/api/custom-ui/components/{id}/assets/{}",
                            encode_path(&manifest.entry)
                        ),
                        directory: display_path(&entry.path()),
                        manifest,
                        built_in: None,
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CustomUiError::io(e)),
        }
        components.sort_by(|a, b| {
            a.manifest
                .name
                .to_lowercase()
                .cmp(&b.manifest.name.to_lowercase())
                .then_with(|| a.manifest.id.cmp(&b.manifest.id))
        });
        Ok(json!({"root": display_path(&self.root), "components": components}))
    }
    pub fn import_bundle(&self, bytes: &[u8]) -> Result<Value> {
        self.open()?;
        let (id, files) = archive::unpack(bytes)?;
        if id == "pisper-island" {
            return Err(CustomUiError::new(
                409,
                "component_id_reserved",
                "component_id_reserved",
            ));
        }
        let manifest_bytes = files
            .get("manifest.json")
            .ok_or_else(|| CustomUiError::archive("component_manifest_missing"))?;
        if manifest_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(CustomUiError::archive("component_manifest_invalid"));
        }
        let raw: Value = serde_json::from_slice(manifest_bytes)
            .map_err(|_| CustomUiError::archive("component_manifest_invalid"))?;
        let manifest = normalize_manifest(&id, &raw)?;
        if !files.contains_key(&manifest.entry) {
            return Err(CustomUiError::archive("component_entry_missing"));
        }
        let _guard = self.mutation.lock().unwrap_or_else(|p| p.into_inner());
        self.open()?;
        fs::create_dir_all(&self.root).map_err(CustomUiError::io)?;
        let target = self.root.join(&id);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(installed());
        }
        let stage = self
            .root
            .join(format!(".component-import-{}", uuid::Uuid::new_v4()));
        create_private_dir(&stage).map_err(CustomUiError::io)?;
        let cleanup = StageCleanup(stage.clone());
        for (name, content) in &files {
            let file = stage.join(name);
            if let Some(parent) = file.parent() {
                create_private_tree(&stage, parent).map_err(CustomUiError::io)?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(file).map_err(CustomUiError::io)?;
            output.write_all(content).map_err(CustomUiError::io)?;
            output.sync_all().map_err(CustomUiError::io)?;
        }
        if let Err(error) = publish(&stage, &target) {
            return Err(if fs::symlink_metadata(&target).is_ok() {
                installed()
            } else {
                CustomUiError::io(error)
            });
        }
        drop(cleanup);
        Ok(json!({"id":id, "name":manifest.name, "version":manifest.version}))
    }
    // 真实导出供后续宿主使用；release 未公开 export HTTP，不冒充已有端点。
    pub fn export_bundle(&self, id: &str) -> Result<Vec<u8>> {
        self.open()?;
        let manifest = self
            .read_manifest(id)
            .ok_or_else(CustomUiError::missing_asset)?;
        let mut files = archive::Files::new();
        if id == "pisper-island" {
            files.insert(
                "manifest.json".into(),
                serde_json::to_vec_pretty(&manifest).map_err(CustomUiError::io)?,
            );
            files.insert("index.html".into(), assets::ISLAND_HTML.as_bytes().to_vec());
        } else {
            // manifest 保留未知字段与原始字节，导出不会重写用户配置。
            files.insert(
                "manifest.json".into(),
                bounded_read(
                    &self.root.join(id).join("manifest.json"),
                    MAX_MANIFEST_BYTES,
                )?,
            );
            collect_files(self, id, Path::new(""), &mut files)?;
        }
        archive::encode(id, &files)
    }
    pub fn create_view(&self, id: &str, owner: &str, origin: &str) -> Result<ViewGrant> {
        self.open()?;
        let url = reqwest::Url::parse(origin).map_err(|_| invalid_origin())?;
        if !["http", "https"].contains(&url.scheme())
            || url.origin().ascii_serialization() != origin
        {
            return Err(invalid_origin());
        }
        if !valid_component_id(id) {
            return Err(CustomUiError::new(404, "component_missing", "组件不存在。"));
        }
        let manifest = self
            .read_manifest(id)
            .ok_or_else(CustomUiError::missing_asset)?;
        self.read_asset_bytes(id, &manifest.entry)?;
        let now = (self.now)();
        let mut views = self.views.lock().unwrap_or_else(|p| p.into_inner());
        self.open()?;
        views.retain(|_, view| view.expires_at > now);
        if views.len() >= MAX_VIEWS {
            return Err(CustomUiError::new(
                429,
                "component_view_limit",
                "组件预览数量已达上限。",
            ));
        }
        let mut bytes = [0; 32];
        getrandom::getrandom(&mut bytes).map_err(CustomUiError::io)?;
        let view_id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        views.insert(
            view_id.clone(),
            View {
                component_id: id.into(),
                owner: owner.into(),
                origin: origin.into(),
                expires_at: now.saturating_add(VIEW_TTL_MS),
            },
        );
        Ok(ViewGrant {
            entry_url: format!(
                "/api/custom-ui/render/{view_id}/assets/{}",
                encode_path(&manifest.entry)
            ),
            id: view_id,
        })
    }
    pub fn get_view(&self, id: &str) -> Option<View> {
        if !valid_view_id(id) || self.closed.load(Ordering::Acquire) {
            return None;
        }
        let mut views = self.views.lock().unwrap_or_else(|p| p.into_inner());
        if views
            .get(id)
            .is_some_and(|view| view.expires_at > (self.now)())
        {
            return views.get(id).cloned();
        }
        views.remove(id);
        None
    }
    pub fn renew_view(&self, id: &str, owner: &str) -> Result<()> {
        self.open()?;
        let mut views = self.views.lock().unwrap_or_else(|p| p.into_inner());
        let now = (self.now)();
        let view = views
            .get_mut(id)
            .filter(|view| view.expires_at > now && view.owner == owner)
            .ok_or_else(CustomUiError::expired_view)?;
        view.expires_at = now.saturating_add(VIEW_TTL_MS);
        Ok(())
    }
    pub fn revoke_view(&self, id: &str, owner: &str) {
        let mut views = self.views.lock().unwrap_or_else(|p| p.into_inner());
        if views.get(id).is_some_and(|view| view.owner == owner) {
            views.remove(id);
        }
    }
    pub fn asset(
        &self,
        id: &str,
        path: &str,
        resource_base: Option<&str>,
    ) -> Result<AssetResponse> {
        self.open()?;
        assets::respond(path, self.read_asset_bytes(id, path)?, resource_base)
    }
    pub fn bridge(&self, resource_base: Option<&str>) -> AssetResponse {
        let mut headers = assets::headers(resource_base);
        if resource_base.is_none() {
            headers.clear();
            headers.insert("cache-control", "no-cache".into());
            headers.insert("x-content-type-options", "nosniff".into());
        }
        AssetResponse {
            body: assets::BRIDGE_SCRIPT.as_bytes().to_vec(),
            content_type: "text/javascript; charset=utf-8",
            headers,
        }
    }
    fn read_asset_bytes(&self, id: &str, raw: &str) -> Result<Vec<u8>> {
        if !valid_component_id(id) {
            return Err(CustomUiError::missing_asset());
        }
        let path = normalize_asset_path(raw).ok_or_else(CustomUiError::missing_asset)?;
        if id == "pisper-island" {
            return if path == Path::new("index.html") {
                Ok(assets::ISLAND_HTML.as_bytes().to_vec())
            } else {
                Err(CustomUiError::missing_asset())
            };
        }
        let (root, dir, file) = (
            self.root.canonicalize(),
            self.root.join(id).canonicalize(),
            self.root.join(id).join(&path).canonicalize(),
        );
        let (Ok(root), Ok(dir), Ok(file)) = (root, dir, file) else {
            return Err(CustomUiError::missing_asset());
        };
        if dir == root || !dir.starts_with(&root) || file == dir || !file.starts_with(&dir) {
            return Err(CustomUiError::missing_asset());
        }
        bounded_read(&file, MAX_ASSET_BYTES).map_err(|_| CustomUiError::missing_asset())
    }
}

fn installed() -> CustomUiError {
    CustomUiError::new(
        409,
        "component_already_installed",
        "component_already_installed",
    )
}
fn invalid_origin() -> CustomUiError {
    CustomUiError::new(400, "component_origin_invalid", "组件来源无效。")
}
struct StageCleanup(PathBuf);
impl Drop for StageCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
fn create_private_tree(root: &Path, path: &Path) -> std::io::Result<()> {
    let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
    let mut current = root.to_owned();
    for part in relative.components() {
        current.push(part);
        if !current.exists() {
            create_private_dir(&current)?;
        }
    }
    Ok(())
}
fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(CustomUiError::io)?;
    let metadata = file.metadata().map_err(CustomUiError::io)?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(CustomUiError::missing_asset());
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(limit));
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(CustomUiError::io)?;
    if bytes.len() > limit {
        return Err(CustomUiError::missing_asset());
    }
    Ok(bytes)
}
fn normalize_asset_path(raw: &str) -> Option<PathBuf> {
    if raw.is_empty()
        || raw.starts_with(['/', '\\'])
        || raw.contains([':', '\0'])
        || raw.split(['/', '\\']).any(|part| part == "..")
    {
        return None;
    }
    let parts: Vec<_> = raw
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty()
        || parts.iter().any(|part| part.starts_with('.'))
        || parts == ["manifest.json"]
    {
        return None;
    }
    Some(parts.into_iter().collect())
}
fn collect_files(
    service: &CustomUiService,
    id: &str,
    relative: &Path,
    files: &mut archive::Files,
) -> Result<()> {
    for entry in fs::read_dir(service.root.join(id).join(relative)).map_err(CustomUiError::io)? {
        let entry = entry.map_err(CustomUiError::io)?;
        let path = relative.join(entry.file_name());
        let name = path.to_string_lossy().replace('\\', "/");
        if name == "manifest.json" || name.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        let kind = entry.file_type().map_err(CustomUiError::io)?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if path.components().count() >= 8 {
                return Err(CustomUiError::archive("component_archive_invalid"));
            }
            collect_files(service, id, &path, files)?;
        } else if kind.is_file() {
            files.insert(name.clone(), service.read_asset_bytes(id, &name)?);
            if files.len() > 64 || files.values().map(Vec::len).sum::<usize>() > MAX_ARCHIVE_BYTES {
                return Err(CustomUiError::archive("component_archive_too_large"));
            }
        }
    }
    Ok(())
}
fn display_path(path: &Path) -> String {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let Some(home) = home.filter(|home| home.is_absolute() && path.is_absolute()) else {
        return path.to_string_lossy().into_owned();
    };
    let relative = if cfg!(windows) {
        let path_text = path.to_string_lossy().replace('/', "\\");
        let home_text = home
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_owned();
        if path_text.eq_ignore_ascii_case(&home_text) {
            Some(String::new())
        } else if path_text
            .to_ascii_lowercase()
            .starts_with(&format!("{}\\", home_text.to_ascii_lowercase()))
        {
            Some(path_text[home_text.len() + 1..].to_owned())
        } else {
            None
        }
    } else {
        path.strip_prefix(&home)
            .ok()
            .map(|relative| relative.to_string_lossy().into_owned())
    };
    match relative {
        Some(relative) if !relative.split(['/', '\\']).any(|p| p == "..") => {
            let label = if cfg!(windows) { "%USERPROFILE%" } else { "~" };
            if relative.is_empty() {
                label.into()
            } else {
                format!("{label}{}{relative}", std::path::MAIN_SEPARATOR)
            }
        }
        _ => path.to_string_lossy().into_owned(),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn publish(stage: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(stage.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let to = CString::new(target.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            1_u32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn publish(stage: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(stage.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let to = CString::new(target.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(windows)]
fn publish(stage: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(stage, target)
}
#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn publish(_stage: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic exclusive directory rename unavailable",
    ))
}
