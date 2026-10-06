//! Native fd/ripgrep discovery, release resolution and installation, ported from
//! `utils/tools-manager.ts`. HTTP and OS effects are injectable for offline tests.
use super::management_http::{
    fetch_with_retry_using, native_transport, FetchError, FetchRetryOptions, FetchTransport,
    ManagementRequest,
};
use super::node_path::{posix_join, win32_join};
use crate::coding_agent::core::get_agent_dir;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::Write,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Fd,
    Rg,
}
impl ToolKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "fd" => Some(Self::Fd),
            "rg" => Some(Self::Rg),
            _ => None,
        }
    }
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Fd => "fd",
            Self::Rg => "rg",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Fd => "fd",
            Self::Rg => "ripgrep",
        }
    }
    pub fn repo(self) -> &'static str {
        match self {
            Self::Fd => "sharkdp/fd",
            Self::Rg => "BurntSushi/ripgrep",
        }
    }
    pub fn tag_prefix(self) -> &'static str {
        match self {
            Self::Fd => "v",
            Self::Rg => "",
        }
    }
    pub fn system_names(self) -> &'static [&'static str] {
        match self {
            Self::Fd => &["fd", "fdfind"],
            Self::Rg => &["rg"],
        }
    }
    pub fn asset_name(self, version: &str, platform: &str, architecture: &str) -> Option<String> {
        let arch = if architecture == "arm64" {
            "aarch64"
        } else {
            "x86_64"
        };
        let suffix = match platform {
            "darwin" => "apple-darwin.tar.gz",
            "linux" => "unknown-linux-musl.tar.gz",
            "win32" => "pc-windows-msvc.zip",
            _ => return None,
        };
        Some(format!(
            "{}-{}{version}-{arch}-{suffix}",
            self.display_name(),
            self.tag_prefix()
        ))
    }
}
#[derive(Debug, Clone, Default)]
pub struct SpawnResult {
    pub error: Option<String>,
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
impl SpawnResult {
    fn failure(&self) -> String {
        if let Some(error) = self.error.as_ref().filter(|e| !e.is_empty()) {
            return error.clone();
        }
        for bytes in [&self.stderr, &self.stdout] {
            let text = String::from_utf8_lossy(bytes);
            let text = super::text::trim_js_whitespace(&text);
            if !text.is_empty() {
                return text.into();
            }
        }
        format!(
            "exit status {}",
            self.status
                .map_or_else(|| "unknown".into(), |v| v.to_string())
        )
    }
}
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_file: bool,
    pub is_directory: bool,
}
/// Synchronous filesystem/process collaborators match the upstream sync effects.
/// Download response chunks are streamed; the whole archive is never buffered.
pub trait ToolManagerHost: Send + Sync {
    fn exists(&self, path: &str) -> bool;
    fn spawn(&self, command: &str, args: &[String]) -> SpawnResult;
    fn mkdir(&self, path: &str) -> Result<(), FetchError>;
    fn create_file(&self, path: &str) -> Result<Box<dyn Write + Send>, FetchError>;
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FetchError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), FetchError>;
    fn chmod_executable(&self, path: &str) -> Result<(), FetchError>;
    fn remove_file(&self, path: &str) -> Result<(), FetchError>;
    fn remove_dir(&self, path: &str) -> Result<(), FetchError>;
}
#[derive(Default)]
pub struct NativeToolManagerHost;
fn io_error(error: std::io::Error) -> FetchError {
    FetchError::new("Error", error.to_string())
}
impl ToolManagerHost for NativeToolManagerHost {
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
    fn spawn(&self, command: &str, args: &[String]) -> SpawnResult {
        let mut process = std::process::Command::new(command);
        process.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            process.creation_flags(0x08000000);
        }
        match process.output() {
            Ok(output) => SpawnResult {
                error: None,
                status: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            },
            Err(error) => SpawnResult {
                error: Some(format!(
                    "spawnSync {command} {}",
                    crate::coding_agent::core::tools::io_code(&error)
                )),
                ..Default::default()
            },
        }
    }
    fn mkdir(&self, path: &str) -> Result<(), FetchError> {
        std::fs::create_dir_all(path).map_err(io_error)
    }
    fn create_file(&self, path: &str) -> Result<Box<dyn Write + Send>, FetchError> {
        std::fs::File::create(path)
            .map(|f| Box::new(f) as Box<dyn Write + Send>)
            .map_err(io_error)
    }
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FetchError> {
        let mut entries = std::fs::read_dir(path)
            .map_err(io_error)?
            .map(|entry| {
                let entry = entry.map_err(io_error)?;
                let kind = entry.file_type().map_err(io_error)?;
                Ok(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_file: kind.is_file(),
                    is_directory: kind.is_dir(),
                })
            })
            .collect::<Result<Vec<_>, FetchError>>()?;
        // libuv's scandir returns entries sorted by name. Do not follow symlinks.
        entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        Ok(entries)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), FetchError> {
        std::fs::rename(from, to).map_err(io_error)
    }
    fn chmod_executable(&self, path: &str) -> Result<(), FetchError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .map_err(io_error)?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }
    fn remove_file(&self, path: &str) -> Result<(), FetchError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(e)),
        }
    }
    fn remove_dir(&self, path: &str) -> Result<(), FetchError> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(e)),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolStatus {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}
pub type ToolStatusCallback = Arc<dyn Fn(ToolStatus) + Send + Sync>;
#[derive(Clone)]
pub struct ToolsManager {
    pub platform: String,
    pub architecture: String,
    pub tools_dir: String,
    pub env: HashMap<String, String>,
    pub host: Arc<dyn ToolManagerHost>,
    pub transport: FetchTransport,
    /// The suffix must be a single filename component. Native values are unique.
    pub unique_suffix: Arc<dyn Fn() -> String + Send + Sync>,
}
impl Default for ToolsManager {
    fn default() -> Self {
        let platform = if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "android") {
            "android"
        } else {
            std::env::consts::OS
        };
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            v => v,
        };
        let agent_dir = get_agent_dir();
        let tools_dir = if platform == "win32" {
            win32_join(&[&agent_dir, "bin"])
        } else {
            posix_join(&[&agent_dir, "bin"])
        };
        Self {
            platform: platform.into(),
            architecture: architecture.into(),
            tools_dir,
            env: std::env::vars().collect(),
            host: Arc::new(NativeToolManagerHost),
            transport: native_transport(),
            unique_suffix: Arc::new(|| {
                format!(
                    "{}_{}_{:016x}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis(),
                    rand::random::<u64>()
                )
            }),
        }
    }
}
impl ToolsManager {
    fn join(&self, parts: &[&str]) -> String {
        if self.platform == "win32" {
            win32_join(parts)
        } else {
            posix_join(parts)
        }
    }
    fn environment(&self, key: &str) -> Option<&str> {
        if self.platform == "win32" {
            self.env
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str())
        } else {
            self.env.get(key).map(String::as_str)
        }
    }
    pub fn is_offline(&self) -> bool {
        self.environment("PI_OFFLINE").is_some_and(|v| {
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
        })
    }
    pub fn get_tool_path(&self, tool: ToolKind) -> Option<String> {
        let binary = format!(
            "{}{}",
            tool.binary_name(),
            if self.platform == "win32" { ".exe" } else { "" }
        );
        let local = self.join(&[&self.tools_dir, &binary]);
        if self.host.exists(&local) {
            return Some(local);
        }
        // A successfully spawned nonzero --version still proves the command exists.
        tool.system_names()
            .iter()
            .find(|name| self.host.spawn(name, &["--version".into()]).error.is_none())
            .map(|s| (*s).into())
    }
    pub async fn get_latest_version(&self, repo: &str) -> Result<String, FetchError> {
        let mut request =
            ManagementRequest::get(format!("https://github.com/{repo}/releases/latest"));
        request.manual_redirect = true;
        request.headers.insert(
            "user-agent",
            "pi-coding-agent".parse().expect("static header"),
        );
        let response = fetch_with_retry_using(
            request,
            FetchRetryOptions {
                timeout: Some(Duration::from_secs(10)),
                ..Default::default()
            },
            &self.transport,
        )
        .await?;
        let status = response.status();
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let _ = response.cancel_body().await;
        let location = location
            .filter(|value| (300..400).contains(&status) && !value.is_empty())
            .ok_or_else(|| {
                FetchError::new(
                    "Error",
                    format!(
                        "Failed to resolve latest {repo} release: HTTP {status} without redirect"
                    ),
                )
            })?;
        let parsed = url::Url::parse("https://github.com")
            .expect("static URL")
            .join(&location)
            .map_err(|_| FetchError::new("TypeError", "Invalid URL"))?;
        let tag = parsed.path().rsplit('/').next().unwrap_or_default();
        if tag.is_empty() || !location.contains("/releases/tag/") {
            return Err(FetchError::new(
                "Error",
                format!(
                    "Failed to resolve latest {repo} release: unexpected redirect to {location}"
                ),
            ));
        }
        let decoded = super::hosted_git_info::decode_uri_component(tag)
            .ok_or_else(|| FetchError::new("URIError", "URI malformed"))?;
        Ok(decoded.strip_prefix('v').unwrap_or(&decoded).to_string())
    }
    async fn download_file(&self, url: &str, dest: &str) -> Result<(), FetchError> {
        let mut response = fetch_with_retry_using(
            ManagementRequest::get(url),
            FetchRetryOptions {
                timeout: Some(Duration::from_secs(120)),
                ..Default::default()
            },
            &self.transport,
        )
        .await?;
        if !response.is_success() {
            return Err(FetchError::new(
                "Error",
                format!("Download failed with HTTP {}: {url}", response.status()),
            ));
        }
        if !response.has_body() {
            return Err(FetchError::new("Error", "No response body"));
        }
        let mut file = self.host.create_file(dest)?;
        while let Some(chunk) = response.next_chunk().await? {
            file.write_all(&chunk).map_err(io_error)?;
        }
        file.flush().map_err(io_error)
    }
    fn extraction_command(&self, command: &str, args: &[&str]) -> Option<String> {
        let result = self.host.spawn(
            command,
            &args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
        );
        if result.error.is_none() && result.status == Some(0) {
            None
        } else {
            Some(format!("{command}: {}", result.failure()))
        }
    }
    fn windows_tar(&self) -> String {
        if let Some(root) = self
            .environment("SystemRoot")
            .or_else(|| self.environment("WINDIR"))
            .filter(|s| !s.is_empty())
        {
            let path = self.join(&[root, "System32", "tar.exe"]);
            if self.host.exists(&path) {
                return path;
            }
        }
        "tar.exe".into()
    }
    fn extract(&self, archive: &str, dest: &str, asset: &str) -> Result<(), FetchError> {
        let mut failures = vec![];
        if asset.ends_with(".tar.gz") {
            if let Some(failure) = self.extraction_command("tar", &["xzf", archive, "-C", dest]) {
                failures.push(failure);
            }
        } else if asset.ends_with(".zip") {
            if self.platform == "win32" {
                if let Some(failure) =
                    self.extraction_command(&self.windows_tar(), &["xf", archive, "-C", dest])
                {
                    failures.push(failure);
                } else {
                    return Ok(());
                }
                let script="& { param($archive, $destination) $ErrorActionPreference = 'Stop'; Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force }";
                if let Some(failure) = self.extraction_command(
                    "powershell.exe",
                    &[
                        "-NoLogo",
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        script,
                        archive,
                        dest,
                    ],
                ) {
                    failures.push(failure);
                } else {
                    return Ok(());
                }
            } else {
                if let Some(failure) =
                    self.extraction_command("unzip", &["-q", archive, "-d", dest])
                {
                    failures.push(failure);
                } else {
                    return Ok(());
                }
                if let Some(failure) = self.extraction_command("tar", &["xf", archive, "-C", dest])
                {
                    failures.push(failure);
                } else {
                    return Ok(());
                }
            }
        } else {
            return Err(FetchError::new(
                "Error",
                format!("Unsupported archive format: {asset}"),
            ));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(FetchError::new(
                "Error",
                format!("Failed to extract {asset}: {}", failures.join("; ")),
            ))
        }
    }
    fn find_binary(&self, root: &str, binary: &str) -> Result<Option<String>, FetchError> {
        let mut stack = vec![root.to_owned()];
        while let Some(current) = stack.pop() {
            for entry in self.host.read_dir(&current)? {
                let path = self.join(&[&current, &entry.name]);
                if entry.is_file && entry.name == binary {
                    return Ok(Some(path));
                }
                if entry.is_directory {
                    stack.push(path);
                }
            }
        }
        Ok(None)
    }
    pub async fn download_tool(&self, tool: ToolKind) -> Result<String, FetchError> {
        let version =
            if tool == ToolKind::Fd && self.platform == "darwin" && self.architecture == "x64" {
                "10.3.0".into()
            } else {
                self.get_latest_version(tool.repo()).await?
            };
        let asset = tool
            .asset_name(&version, &self.platform, &self.architecture)
            .ok_or_else(|| {
                FetchError::new(
                    "Error",
                    format!(
                        "Unsupported platform: {}/{}",
                        self.platform, self.architecture
                    ),
                )
            })?;
        self.host.mkdir(&self.tools_dir)?;
        let url = format!(
            "https://github.com/{}/releases/download/{}{version}/{asset}",
            tool.repo(),
            tool.tag_prefix()
        );
        let archive = self.join(&[&self.tools_dir, &asset]);
        let binary = format!(
            "{}{}",
            tool.binary_name(),
            if self.platform == "win32" { ".exe" } else { "" }
        );
        let target = self.join(&[&self.tools_dir, &binary]);
        // Upstream creates the extraction guard after a completed download, so a
        // failed transfer can leave the partial archive for subsequent overwrite.
        self.download_file(&url, &archive).await?;
        let suffix = (self.unique_suffix)();
        if suffix.is_empty() || suffix.contains(['/', '\\']) || suffix == ".." {
            return Err(FetchError::new(
                "Error",
                "Invalid extraction directory suffix",
            ));
        }
        let extract_dir = self.join(&[
            &self.tools_dir,
            &format!("extract_tmp_{}_{suffix}", tool.binary_name()),
        ]);
        self.host.mkdir(&extract_dir)?;
        let result = (|| {
            self.extract(&archive, &extract_dir, &asset)?;
            let stem = asset
                .strip_suffix(".tar.gz")
                .or_else(|| asset.strip_suffix(".zip"))
                .unwrap_or(&asset);
            let candidates = [
                self.join(&[&extract_dir, stem, &binary]),
                self.join(&[&extract_dir, &binary]),
            ];
            let mut extracted = candidates.into_iter().find(|p| self.host.exists(p));
            if extracted.is_none() {
                extracted = self.find_binary(&extract_dir, &binary)?;
            }
            let extracted = extracted.ok_or_else(|| {
                FetchError::new(
                    "Error",
                    format!("Binary not found in archive: expected {binary} under {extract_dir}"),
                )
            })?;
            self.host.rename(&extracted, &target)?;
            if self.platform != "win32" {
                self.host.chmod_executable(&target)?;
            }
            Ok(target)
        })();
        // Preserve upstream finally ordering, including cleanup error precedence.
        self.host.remove_file(&archive)?;
        self.host.remove_dir(&extract_dir)?;
        result
    }
    pub async fn ensure_tool(
        &self,
        tool: ToolKind,
        on_status: Option<&ToolStatusCallback>,
    ) -> Option<String> {
        if let Some(path) = self.get_tool_path(tool) {
            return Some(path);
        }
        let emit = |kind: &str, message: String| {
            if let Some(callback) = on_status {
                callback(ToolStatus {
                    kind: kind.into(),
                    message,
                });
            }
        };
        let name = tool.display_name();
        if self.is_offline() {
            emit(
                "warning",
                format!("{name} not found. Offline mode enabled, skipping download."),
            );
            return None;
        }
        if self.platform == "android" {
            emit(
                "warning",
                format!("{name} not found. Install with: pkg install {name}"),
            );
            return None;
        }
        emit("info", format!("{name} not found. Downloading..."));
        match self.download_tool(tool).await {
            Ok(path) => {
                emit("info", format!("{name} installed to {path}"));
                Some(path)
            }
            Err(error) => {
                let mut messages = vec![];
                for message in std::iter::once(&error.message)
                    .chain(error.causes.iter())
                    .take(5)
                {
                    if !messages.contains(message) {
                        messages.push(message.clone());
                    }
                }
                emit(
                    "warning",
                    format!("Failed to download {name}: {}", messages.join(": ")),
                );
                None
            }
        }
    }
}
pub fn get_tool_path(tool: &str) -> Option<String> {
    ToolKind::parse(tool).and_then(|tool| ToolsManager::default().get_tool_path(tool))
}
pub async fn ensure_tool(tool: &str, on_status: Option<&ToolStatusCallback>) -> Option<String> {
    let tool = ToolKind::parse(tool)?;
    ToolsManager::default().ensure_tool(tool, on_status).await
}
pub async fn get_latest_version(repo: &str) -> Result<String, FetchError> {
    ToolsManager::default().get_latest_version(repo).await
}
#[cfg(test)]
#[path = "tools_manager_tests.rs"]
mod tests;
