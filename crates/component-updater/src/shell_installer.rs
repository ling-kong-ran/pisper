//! 桌面壳安装包的后台下载。
//!
//! 组件更新器只替换 Desktop 前端和 Runtime；壳本身（src-tauri 编出的 Pisper.exe / Pisper.app /
//! AppImage）只有用户重装时才会变，Rust 侧的修复因此可能几个版本都到不了用户手上。这里按发布里的
//! `latest.json` 找到当前平台的安装包，下载后用组件更新器同一把公钥验签，放进专用目录，
//! 最后由用户自己打开安装：Pisper 不退出、也不静默执行任何东西。

use super::{
    decode_public_key, decode_signature, download_candidates_with_mirrors, ensure_success,
    http_client, is_permanent_failure, mirror_prefixes, rate_limited_until,
    release_feed_candidates, unix_now, RateLimited, MAX_RATE_LIMIT_BACKOFF_SECS,
    MAX_SIGNATURE_BYTES, NETWORK_ATTEMPTS,
};
use anyhow::{anyhow, bail, Context, Result};
use fs2::FileExt as _;
use minisign_verify::PublicKey;
use reqwest::header::ACCEPT;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    cmp::Ordering as VersionOrdering,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// releases/latest/download 直链不计入匿名 API 额度（每小时 60 次），定时检查不会挤占组件更新。
pub const SHELL_MANIFEST_URL: &str =
    "https://github.com/ling-kong-ran/pisper/releases/latest/download/latest.json";
const RELEASE_DOWNLOAD_BASE: &str = "https://github.com/ling-kong-ran/pisper/releases/download/";
const RELEASE_TAG_BASE: &str = "https://github.com/ling-kong-ran/pisper/releases/tag/";
/// 安装包所在的子目录名（位于应用本地数据目录下）。
pub const INSTALLERS_DIR: &str = "installers";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
// 现有安装包 80–200 MB；上限只防异常响应把磁盘写满。
const MAX_INSTALLER_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_NOTES_CHARS: usize = 64 * 1024;
const MAX_VERSION_CHARS: usize = 64;
const MAX_DATE_CHARS: usize = 64;
const MAX_URL_CHARS: usize = 2048;
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(60);
// 慢速网络下 200 MB 可能要很久，总时长放宽；真正卡住的连接靠 STALL_TIMEOUT 发现。
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
const STALL_TIMEOUT: Duration = Duration::from_secs(90);
const SETTINGS_FILE: &str = "settings.json";
const STATE_FILE: &str = "manifest-state.json";
const LOCK_FILE: &str = "download.lock";
const STATE_SCHEMA: u32 = 1;

/// 发布里带安装包的平台。其他平台（例如 Intel Mac）只能去发布页手动下载。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InstallerPlatform {
    WindowsX64,
    DarwinAarch64,
    LinuxX64,
}

impl InstallerPlatform {
    pub const ALL: [Self; 3] = [Self::WindowsX64, Self::DarwinAarch64, Self::LinuxX64];

    pub fn current() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Some(Self::WindowsX64),
            ("macos", "aarch64") => Some(Self::DarwinAarch64),
            ("linux", "x86_64") => Some(Self::LinuxX64),
            _ => None,
        }
    }

    /// latest.json 里 platforms 的键，与 Tauri 更新器的命名一致。
    pub fn manifest_key(self) -> &'static str {
        match self {
            Self::WindowsX64 => "windows-x86_64",
            Self::DarwinAarch64 => "darwin-aarch64",
            Self::LinuxX64 => "linux-x86_64",
        }
    }

    fn asset_platform(self) -> &'static str {
        match self {
            Self::WindowsX64 => "windows_x86_64",
            Self::DarwinAarch64 => "darwin_aarch64",
            Self::LinuxX64 => "linux_x86_64",
        }
    }

    // 与 scripts/stage-tauri-artifacts.mjs 暂存的文件名一致；Windows 用普通包而不是离线包，
    // 和 Tauri 更新器的选择相同（离线包多带一份 WebView2，只给首次部署用）。
    fn installer_suffix(self) -> &'static str {
        match self {
            Self::WindowsX64 => "-setup.exe",
            Self::DarwinAarch64 => ".dmg",
            Self::LinuxX64 => ".AppImage",
        }
    }

    pub fn installer_name(self, version: &Version) -> String {
        format!(
            "Pisper_{version}_{}{}",
            self.asset_platform(),
            self.installer_suffix()
        )
    }

    /// 文件名是本平台的 Pisper 安装包时返回其版本。清理只删认得出的名字，靠的就是这里。
    pub fn installer_version(self, name: &str) -> Option<Version> {
        let suffix = format!("_{}{}", self.asset_platform(), self.installer_suffix());
        let text = name.strip_prefix("Pisper_")?.strip_suffix(&suffix)?;
        let version = Version::parse(text).ok()?;
        // 只认规范写法，避免 "Pisper_01.2.3_…" 之类的外来文件被当成自己的。
        (version.to_string() == text).then_some(version)
    }
}

/// 当前平台可下载的安装包：地址已确认是这次发布的资产，签名已能解析。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallerAsset {
    pub file_name: String,
    pub url: String,
    pub signature: String,
}

/// 从 latest.json 解析出的最新桌面发布。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellRelease {
    pub version: Version,
    pub notes: String,
    pub published_at: Option<String>,
    pub release_url: String,
    /// 没有时只能提示有新版本：例如 macOS 的旧发布没有 dmg 签名。
    pub installer: Option<InstallerAsset>,
}

impl ShellRelease {
    pub fn is_newer_than(&self, current: &Version) -> bool {
        is_newer(current, &self.version)
    }
}

/// 只比较版本优先级：semver 的 `Ord` 连构建元数据也比，`1.0.0+a` 和 `1.0.0` 会被当成不同版本。
pub fn is_newer(current: &Version, candidate: &Version) -> bool {
    candidate.cmp_precedence(current) == VersionOrdering::Greater
}

pub fn release_url(version: &Version) -> String {
    format!("{RELEASE_TAG_BASE}v{version}")
}

/// 解析并校验 latest.json。任何字段不合规都整体拒绝，不拿半份数据去下载。
pub fn parse_manifest(bytes: &[u8], platform: InstallerPlatform) -> Result<ShellRelease> {
    parse_manifest_from(bytes, platform, RELEASE_DOWNLOAD_BASE)
}

fn parse_manifest_from(
    bytes: &[u8],
    platform: InstallerPlatform,
    download_base: &str,
) -> Result<ShellRelease> {
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        bail!("desktop release manifest exceeds the size limit");
    }
    let manifest = serde_json::from_slice::<Value>(bytes)
        .context("desktop release manifest is not valid JSON")?;
    let object = manifest
        .as_object()
        .ok_or_else(|| anyhow!("desktop release manifest is not an object"))?;
    let version_text = required_string(object, "version", MAX_VERSION_CHARS)?;
    let version = Version::parse(version_text).context("desktop release version is invalid")?;
    if !version.pre.is_empty() || !version.build.is_empty() || version.to_string() != version_text {
        bail!("desktop release version is not a stable release version");
    }
    let notes = optional_string(object, "notes", usize::MAX)?
        .map(|value| value.chars().take(MAX_NOTES_CHARS).collect())
        .unwrap_or_default();
    let published_at = optional_string(object, "pub_date", MAX_DATE_CHARS)?
        .map(str::to_string)
        .filter(|value| !value.is_empty());
    if published_at
        .as_deref()
        .is_some_and(|value| !value.chars().all(|value| value.is_ascii_graphic()))
    {
        bail!("desktop release date is invalid");
    }
    let platforms = object
        .get("platforms")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("desktop release manifest has no platforms"))?;
    let installer = match platforms.get(platform.manifest_key()) {
        None => None,
        Some(entry) => {
            let entry = entry
                .as_object()
                .ok_or_else(|| anyhow!("desktop release platform entry is not an object"))?;
            select_installer(entry, platform, &version, download_base)?
        }
    };
    Ok(ShellRelease {
        release_url: release_url(&version),
        version,
        notes,
        published_at,
        installer,
    })
}

fn select_installer(
    entry: &Map<String, Value>,
    platform: InstallerPlatform,
    version: &Version,
    download_base: &str,
) -> Result<Option<InstallerAsset>> {
    // installer 是给壳下载用的附加字段（目前只有 macOS 的 dmg）；Tauri 更新器只读 url/signature，会忽略它。
    if let Some(installer) = entry.get("installer") {
        let installer = installer
            .as_object()
            .ok_or_else(|| anyhow!("desktop installer entry is not an object"))?;
        return validate_asset(installer, platform, version, download_base).map(Some);
    }
    match platform {
        // macOS 的 url 指向 .app.tar.gz 更新包，不是用户能双击安装的东西；没有 dmg 就只提示有新版本。
        InstallerPlatform::DarwinAarch64 => Ok(None),
        InstallerPlatform::WindowsX64 | InstallerPlatform::LinuxX64 => {
            validate_asset(entry, platform, version, download_base).map(Some)
        }
    }
}

fn validate_asset(
    object: &Map<String, Value>,
    platform: InstallerPlatform,
    version: &Version,
    download_base: &str,
) -> Result<InstallerAsset> {
    let url = required_string(object, "url", MAX_URL_CHARS)?;
    let signature = required_string(object, "signature", MAX_SIGNATURE_BYTES as usize)?.trim();
    let file_name = platform.installer_name(version);
    // 地址必须逐字等于这次发布里本平台安装包的直链：别的仓库、别的版本目录、离线包、带查询串的都不收。
    // 镜像由下载时按 PISPER_GITHUB_MIRROR 自己拼，清单里不接受镜像地址。
    if url != format!("{download_base}v{version}/{file_name}") {
        bail!("desktop installer URL is not a Pisper release asset for v{version}");
    }
    decode_signature(signature.as_bytes()).context("desktop installer signature is invalid")?;
    Ok(InstallerAsset {
        file_name,
        url: url.to_string(),
        signature: signature.to_string(),
    })
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str, limit: usize) -> Result<&'a str> {
    optional_string(object, key, limit)?
        .ok_or_else(|| anyhow!("desktop release manifest is missing {key}"))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    limit: usize,
) -> Result<Option<&'a str>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= limit => Ok(Some(value.as_str())),
        Some(Value::String(_)) => bail!("desktop release manifest field {key} is too long"),
        Some(_) => bail!("desktop release manifest field {key} is not a string"),
    }
}

/// 用户设置。它放在壳这边而不是前端的页面偏好里：壳启动后几十秒就要决定是否下载，
/// 那时前端可能还没加载，Runtime 侧的偏好文件壳也读不到。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellInstallerSettings {
    pub auto_download: bool,
}

impl Default for ShellInstallerSettings {
    fn default() -> Self {
        // 用户要求默认开启：只下载并验签，安装仍由用户自己点。
        Self {
            auto_download: true,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    schema: u32,
    auto_download: bool,
}

/// 读不出或格式不对时按默认值处理，绝不让设置文件挡住启动。
pub fn read_settings(dir: &Path) -> ShellInstallerSettings {
    fs::read(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|value| serde_json::from_slice::<SettingsFile>(&value).ok())
        .filter(|file| file.schema == STATE_SCHEMA)
        .map(|file| ShellInstallerSettings {
            auto_download: file.auto_download,
        })
        .unwrap_or_default()
}

pub fn write_settings(dir: &Path, settings: ShellInstallerSettings) -> Result<()> {
    fs::create_dir_all(dir).context("failed to create the installers directory")?;
    let content = serde_json::to_vec(&SettingsFile {
        schema: STATE_SCHEMA,
        auto_download: settings.auto_download,
    })?;
    write_atomically(&dir.join(SETTINGS_FILE), &content)
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestState {
    schema: u32,
    rate_limited_until: Option<u64>,
}

fn write_atomically(path: &Path, content: &[u8]) -> Result<()> {
    static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid file name"))?;
    let temporary = path.with_file_name(format!(
        "{name}.{}-{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let written = File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(content)?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&temporary, path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("failed to write {name}"));
    }
    Ok(())
}

/// 目录里由本模块产生的文件。认不出的名字一律不碰。
#[derive(Debug, Eq, PartialEq)]
enum OwnedFile {
    Installer(Version),
    Signature(Version),
    Partial,
}

fn classify(platform: InstallerPlatform, name: &str) -> Option<OwnedFile> {
    if let Some(inner) = name
        .strip_prefix('.')
        .and_then(|name| name.strip_suffix(".part"))
    {
        let (installer, tag) = inner.rsplit_once('.')?;
        let (process, sequence) = tag.split_once('-')?;
        let digits =
            |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
        // 签名文件也先写成同样命名的临时文件再改名。
        let installer = installer.strip_suffix(".sig").unwrap_or(installer);
        return (digits(process)
            && digits(sequence)
            && platform.installer_version(installer).is_some())
        .then_some(OwnedFile::Partial);
    }
    if let Some(installer) = name.strip_suffix(".sig") {
        return platform
            .installer_version(installer)
            .map(OwnedFile::Signature);
    }
    platform.installer_version(name).map(OwnedFile::Installer)
}

fn partial_name(file_name: &str) -> String {
    static NEXT_PARTIAL: AtomicU64 = AtomicU64::new(0);
    format!(
        ".{file_name}.{}-{}.part",
        std::process::id(),
        NEXT_PARTIAL.fetch_add(1, Ordering::Relaxed)
    )
}

/// 列出目录里属于本平台的普通文件；目录和符号链接即使名字对得上也不算。
fn owned_files(dir: &Path, platform: InstallerPlatform) -> Vec<(OwnedFile, String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((classify(platform, &name)?, name, entry.path()))
        })
        .collect()
}

/// 删掉没下完的临时文件，以及 `keep` 之外的所有安装包和签名。调用方必须持有下载锁。
/// 返回删掉的文件名，供日志和测试使用。
fn remove_stale_files(
    dir: &Path,
    platform: InstallerPlatform,
    keep: Option<&Version>,
    keep_installers: bool,
) -> Vec<String> {
    let mut removed = Vec::new();
    for (kind, name, path) in owned_files(dir, platform) {
        let stale = match &kind {
            OwnedFile::Partial => true,
            OwnedFile::Installer(version) | OwnedFile::Signature(version) => {
                !keep_installers && keep != Some(version)
            }
        };
        // 删不掉（例如 Windows 上安装程序正在运行）就留到下次，不影响别的文件。
        if stale && fs::remove_file(&path).is_ok() {
            removed.push(name);
        }
    }
    removed.sort();
    removed
}

/// 用组件更新器同一把公钥、同样的 minisign 预哈希签名验整个文件；边读边算，不把 200 MB 读进内存。
fn verify_file(public_key: &PublicKey, path: &Path, signature: &str) -> Result<()> {
    let signature = decode_signature(signature.as_bytes())?;
    let mut verifier = public_key
        .verify_stream(&signature)
        .context("desktop installer signature does not match the Pisper key")?;
    let mut file = File::open(path).context("failed to open the downloaded installer")?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .context("failed to read the downloaded installer")?;
        if count == 0 {
            break;
        }
        verifier.update(&buffer[..count]);
    }
    verifier
        .finalize()
        .context("desktop installer signature verification failed")
}

/// 已验签、可以交给用户安装的安装包。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadedInstaller {
    pub version: Version,
    pub file_name: String,
    pub path: PathBuf,
    pub size: u64,
}

struct DownloadLock(#[allow(dead_code)] File);

#[derive(Clone)]
pub struct ShellInstallerUpdater {
    dir: PathBuf,
    platform: InstallerPlatform,
    public_key: PublicKey,
    client: reqwest::Client,
    manifest_url: String,
    download_base: String,
}

impl ShellInstallerUpdater {
    pub fn new(
        dir: PathBuf,
        platform: InstallerPlatform,
        encoded_public_key: &str,
        user_agent: &str,
    ) -> Result<Self> {
        Ok(Self {
            dir,
            platform,
            public_key: decode_public_key(encoded_public_key)?,
            client: http_client(user_agent, INSTALLER_TIMEOUT)?,
            manifest_url: SHELL_MANIFEST_URL.into(),
            download_base: RELEASE_DOWNLOAD_BASE.into(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn platform(&self) -> InstallerPlatform {
        self.platform
    }

    pub fn settings(&self) -> ShellInstallerSettings {
        read_settings(&self.dir)
    }

    pub fn set_settings(&self, settings: ShellInstallerSettings) -> Result<()> {
        write_settings(&self.dir, settings)
    }

    // 进程内由调用方保证只有一个下载；文件锁再挡住同时运行的另一个壳进程（例如单实例失效时）。
    fn try_lock(&self) -> Result<Option<DownloadLock>> {
        fs::create_dir_all(&self.dir).context("failed to create the installers directory")?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join(LOCK_FILE))
            .context("failed to open the installer download lock")?;
        Ok(file.try_lock_exclusive().ok().map(|_| DownloadLock(file)))
    }

    fn read_state(&self) -> ManifestState {
        fs::read(self.dir.join(STATE_FILE))
            .ok()
            .and_then(|value| serde_json::from_slice::<ManifestState>(&value).ok())
            .filter(|state| state.schema == STATE_SCHEMA)
            .unwrap_or(ManifestState {
                schema: STATE_SCHEMA,
                rate_limited_until: None,
            })
    }

    /// GitHub 直连处于限流退避期时返回可重试的时间。上限与组件更新器相同：
    /// 超过一小时说明本机时钟被调过，不能因此长期停掉检查。
    pub fn active_rate_limit(&self) -> Option<u64> {
        let now = unix_now();
        self.read_state().rate_limited_until.filter(|until| {
            *until > now && *until <= now.saturating_add(MAX_RATE_LIMIT_BACKOFF_SECS)
        })
    }

    fn record_rate_limit(&self, retry_at: u64) {
        let state = ManifestState {
            schema: STATE_SCHEMA,
            rate_limited_until: Some(retry_at),
        };
        if fs::create_dir_all(&self.dir).is_ok() {
            if let Ok(content) = serde_json::to_vec(&state) {
                let _ = write_atomically(&self.dir.join(STATE_FILE), &content);
            }
        }
    }

    /// 取最新桌面发布：GitHub 直链优先，镜像（PISPER_GITHUB_MIRROR）兜底。
    /// 直连被限流后，退避期内只走镜像；没有镜像时直接报限流，不再白发请求。
    pub async fn fetch_release(&self) -> Result<ShellRelease> {
        let mut rate_limit = self.active_rate_limit();
        let mut last_error = None;
        for url in release_feed_candidates(&self.manifest_url) {
            let direct = url == self.manifest_url;
            if direct && rate_limit.is_some() {
                continue;
            }
            match self.fetch_manifest(&url).await {
                Ok(bytes) => {
                    return parse_manifest_from(&bytes, self.platform, &self.download_base)
                }
                Err(error) => {
                    // 镜像的出口 IP 不是用户的，它被限流不代表用户自己被限流。
                    if direct {
                        if let Some(retry_at) = rate_limited_until(&error) {
                            self.record_rate_limit(retry_at);
                            rate_limit = Some(retry_at);
                        }
                    }
                    last_error = Some(error);
                }
            }
        }
        match (last_error, rate_limit) {
            (Some(error), _) if rate_limited_until(&error).is_none() => Err(error),
            (_, Some(retry_at)) => Err(RateLimited { retry_at }.into()),
            (Some(error), None) => Err(error),
            (None, None) => Err(anyhow!("desktop release manifest has no candidate URLs")),
        }
    }

    async fn fetch_manifest(&self, url: &str) -> Result<Vec<u8>> {
        let mut last_error = None;
        for attempt in 0..NETWORK_ATTEMPTS {
            let result = async {
                let response = self
                    .client
                    .get(url)
                    .header(ACCEPT, "application/json")
                    .timeout(MANIFEST_TIMEOUT)
                    .send()
                    .await
                    .map_err(reqwest::Error::without_url)
                    .context("failed to request the desktop release manifest")?;
                let mut response = ensure_success(response)
                    .await
                    .context("desktop release manifest request failed")?;
                if response
                    .content_length()
                    .is_some_and(|length| length > MAX_MANIFEST_BYTES)
                {
                    bail!("desktop release manifest exceeds the size limit");
                }
                let mut body = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(reqwest::Error::without_url)
                    .context("desktop release manifest request was interrupted")?
                {
                    if body.len() as u64 + chunk.len() as u64 > MAX_MANIFEST_BYTES {
                        bail!("desktop release manifest exceeds the size limit");
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(body)
            }
            .await;
            match result {
                Ok(body) => return Ok(body),
                Err(error) if is_permanent_failure(&error) => return Err(error),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < NETWORK_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("desktop release manifest request failed")))
    }

    /// 已下载且验签通过、比 `current` 新的安装包。会读整个文件，调用方应放到阻塞线程。
    pub fn verified_installer(&self, current: &Version) -> Option<DownloadedInstaller> {
        let files = owned_files(&self.dir, self.platform);
        let newest = files
            .iter()
            .filter_map(|(kind, _, _)| match kind {
                OwnedFile::Installer(version) if is_newer(current, version) => Some(version),
                _ => None,
            })
            .filter(|version| {
                files
                    .iter()
                    .any(|(kind, _, _)| kind == &OwnedFile::Signature((*version).clone()))
            })
            .max_by(|left, right| left.cmp_precedence(right))?
            .clone();
        self.check_downloaded(&newest, None)
    }

    /// 校验目录里指定版本的安装包；给了 `expected_signature` 时还要求签名文件与清单一致，
    /// 免得同版本重新发布后还拿旧包充数。
    fn check_downloaded(
        &self,
        version: &Version,
        expected_signature: Option<&str>,
    ) -> Option<DownloadedInstaller> {
        let file_name = self.platform.installer_name(version);
        let path = self.dir.join(&file_name);
        let signature = fs::read_to_string(self.dir.join(format!("{file_name}.sig"))).ok()?;
        let signature = signature.trim();
        if expected_signature.is_some_and(|expected| expected != signature) {
            return None;
        }
        let metadata = fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        verify_file(&self.public_key, &path, signature).ok()?;
        Some(DownloadedInstaller {
            version: version.clone(),
            file_name,
            path,
            size: metadata.len(),
        })
    }

    /// 启动时整理目录：只留一个比 `current` 新且验签通过的安装包，其余的（含已经装上的版本、
    /// 验不过的文件、上次没下完的临时文件）都删掉。另一个进程正在下载时什么都不动。
    /// 会读整个文件，调用方应放到阻塞线程。
    pub fn prune(&self, current: &Version) -> Option<DownloadedInstaller> {
        let lock = self.try_lock().ok().flatten();
        let kept = self.verified_installer(current);
        if lock.is_some() {
            remove_stale_files(
                &self.dir,
                self.platform,
                kept.as_ref().map(|installer| &installer.version),
                false,
            );
        }
        kept
    }

    /// 下载并验签安装包：先写进同目录的临时文件，验签通过才原子改名成正式文件名，
    /// 半截或验不过的文件永远不会出现在正式文件名下。成功后删掉旧安装包，只留这一个。
    /// 已经下好且验签通过的同一个包不再重下。
    pub async fn download<F>(
        &self,
        release: &ShellRelease,
        mut on_progress: F,
    ) -> Result<DownloadedInstaller>
    where
        F: FnMut(u64, u64) + Send,
    {
        let asset = release
            .installer
            .as_ref()
            .ok_or_else(|| anyhow!("this release has no signed installer for this platform"))?;
        let lock = self
            .try_lock()?
            .ok_or_else(|| anyhow!("another installer download is already running"))?;
        let this = self.clone();
        let version = release.version.clone();
        let signature = asset.signature.clone();
        let existing =
            tokio::task::spawn_blocking(move || this.check_downloaded(&version, Some(&signature)))
                .await
                .context("installer verification task failed")?;
        if let Some(installer) = existing {
            remove_stale_files(&self.dir, self.platform, Some(&installer.version), false);
            drop(lock);
            return Ok(installer);
        }
        // 新包验签通过之前，旧的（仍比当前壳新的）安装包先留着：下载失败时用户手里还有一个能装的。
        remove_stale_files(&self.dir, self.platform, None, true);

        let partial = self.dir.join(partial_name(&asset.file_name));
        let result = async {
            let size = self.download_to(asset, &partial, &mut on_progress).await?;
            let public_key = self.public_key.clone();
            let path = partial.clone();
            let signature = asset.signature.clone();
            tokio::task::spawn_blocking(move || verify_file(&public_key, &path, &signature))
                .await
                .context("installer verification task failed")??;
            Ok::<_, anyhow::Error>(size)
        }
        .await;
        let size = match result {
            Ok(size) => size,
            Err(error) => {
                let _ = fs::remove_file(&partial);
                return Err(error);
            }
        };

        let final_path = self.dir.join(&asset.file_name);
        // 先落签名再改名安装包：中途崩溃时最多留下没有安装包的签名，下次启动会被清掉。
        let activated = write_signature(&self.dir, &asset.file_name, &asset.signature)
            .and_then(|_| make_launchable(self.platform, &partial))
            .and_then(|_| {
                fs::rename(&partial, &final_path)
                    .context("failed to activate the downloaded installer")
            });
        if let Err(error) = activated {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        remove_stale_files(&self.dir, self.platform, Some(&release.version), false);
        drop(lock);
        Ok(DownloadedInstaller {
            version: release.version.clone(),
            file_name: asset.file_name.clone(),
            path: final_path,
            size,
        })
    }

    async fn download_to<F>(
        &self,
        asset: &InstallerAsset,
        partial: &Path,
        on_progress: &mut F,
    ) -> Result<u64>
    where
        F: FnMut(u64, u64) + Send,
    {
        let rate_limit = self.active_rate_limit();
        let mut last_error = None;
        for url in download_candidates_with_mirrors(&asset.url, &asset.url, &mirror_prefixes()) {
            let direct = url == asset.url;
            if direct && rate_limit.is_some() {
                continue;
            }
            for attempt in 0..NETWORK_ATTEMPTS {
                if attempt > 0 {
                    on_progress(0, 0);
                    tokio::time::sleep(Duration::from_millis(250 * attempt as u64)).await;
                }
                match self.download_once(&url, partial, on_progress).await {
                    Ok(size) => return Ok(size),
                    Err(error) => {
                        if direct {
                            if let Some(retry_at) = rate_limited_until(&error) {
                                self.record_rate_limit(retry_at);
                            }
                        }
                        let permanent = is_permanent_failure(&error);
                        last_error = Some(error);
                        if permanent {
                            break;
                        }
                    }
                }
            }
            on_progress(0, 0);
        }
        Err(match (last_error, rate_limit) {
            (Some(error), _) => error,
            (None, Some(retry_at)) => RateLimited { retry_at }.into(),
            (None, None) => anyhow!("desktop installer download has no candidate URLs"),
        })
    }

    async fn download_once<F>(&self, url: &str, partial: &Path, on_progress: &mut F) -> Result<u64>
    where
        F: FnMut(u64, u64) + Send,
    {
        // 这里的地址可能是镜像或带临时签名的重定向地址，错误里一律不带 URL。
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/octet-stream")
            .timeout(INSTALLER_TIMEOUT)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("failed to request the desktop installer")?;
        let mut response = ensure_success(response)
            .await
            .context("desktop installer download failed")?;
        let total = response.content_length().unwrap_or(0);
        if total > MAX_INSTALLER_BYTES {
            bail!("desktop installer exceeds the size limit");
        }
        let mut file = File::create(partial).context("failed to create the installer file")?;
        let mut transferred = 0_u64;
        on_progress(0, total);
        loop {
            let chunk = tokio::time::timeout(STALL_TIMEOUT, response.chunk())
                .await
                .map_err(|_| anyhow!("desktop installer download stalled"))?
                .map_err(reqwest::Error::without_url)
                .context("desktop installer download was interrupted")?;
            let Some(chunk) = chunk else {
                break;
            };
            transferred = transferred
                .checked_add(chunk.len() as u64)
                .filter(|value| *value <= MAX_INSTALLER_BYTES)
                .ok_or_else(|| anyhow!("desktop installer exceeds the size limit"))?;
            // 块不大（几十 KB），同步写盘的停顿可以忽略，省得为此引入 tokio 的 io-util。
            file.write_all(&chunk)
                .context("failed to write the installer file")?;
            on_progress(transferred, total);
        }
        if transferred == 0 || (total > 0 && transferred != total) {
            bail!("desktop installer download ended after {transferred} of {total} bytes");
        }
        file.sync_all()
            .context("failed to flush the installer file")?;
        Ok(transferred)
    }
}

fn write_signature(dir: &Path, file_name: &str, signature: &str) -> Result<()> {
    let name = format!("{file_name}.sig");
    let temporary = dir.join(partial_name(&name));
    let written = File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(signature.as_bytes())?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&temporary, dir.join(&name)));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error).context("failed to write the installer signature");
    }
    Ok(())
}

/// AppImage 要有执行权限才能双击运行；Windows 和 dmg 不需要。
fn make_launchable(platform: InstallerPlatform, path: &Path) -> Result<()> {
    #[cfg(unix)]
    if platform == InstallerPlatform::LinuxX64 {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .context("failed to mark the AppImage as executable")?;
    }
    #[cfg(not(unix))]
    let _ = (platform, path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        classify, is_newer, parse_manifest, parse_manifest_from, read_settings, remove_stale_files,
        write_settings, InstallerPlatform, OwnedFile, ShellInstallerSettings,
        ShellInstallerUpdater, RELEASE_DOWNLOAD_BASE,
    };
    use semver::Version;
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    // 仅用于测试的密钥对（私钥未入库）：签的是 FIXTURE_PAYLOAD。
    const TEST_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IERBMDdGMDY4MzcwMERCMwpSV1N6RFhDREJuK2dEWWZNTksyMzFCbGUrTkRWSlJnMlZWSkQ1R09qMWx5SmJIaVpQSGtVVzZKdgo=";
    const FIXTURE_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTekRYQ0RCbitnRFZiUXExU0oyM2VyclV0WE1xTG16clhacXA1aEpsenJESk1XRkc3OERGZ3lHN0pMV1pIVGhHTUREMXlkdkdXbW5scGxxajdqQnhDaG54SjBoaTJPekFBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0NDc5OTY1CWZpbGU6cGF5bG9hZC5iaW4KMVhlYXZYcjhRdlZvTDRXYmFJeWtyOVZnTkhBcDFTNzQ0MjJhUnVFNFJjZzBnbERlWmpJYURNNnRRNU44Und1R1pJemlPdGg1RWFSVWNQRmIvQ2FQQ2c9PQo=";
    const FIXTURE_PAYLOAD: &[u8] = b"pisper installer fixture";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pisper-shell-installer-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn version(value: &str) -> Version {
        Version::parse(value).unwrap()
    }

    fn manifest(version: &str, platforms: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "version": version,
            "notes": "## What's Changed\n\n- fix",
            "pub_date": "2026-11-12T08:00:00.000Z",
            "platforms": platforms,
        }))
        .unwrap()
    }

    fn asset_url(base: &str, version: &str, name: &str) -> String {
        format!("{base}v{version}/{name}")
    }

    fn full_platforms(base: &str, version: &str, with_dmg: bool) -> serde_json::Value {
        let mut darwin = serde_json::json!({
            "url": asset_url(base, version, &format!("Pisper_{version}_darwin_aarch64.app.tar.gz")),
            "signature": FIXTURE_SIGNATURE,
        });
        if with_dmg {
            darwin["installer"] = serde_json::json!({
                "url": asset_url(base, version, &format!("Pisper_{version}_darwin_aarch64.dmg")),
                "signature": FIXTURE_SIGNATURE,
            });
        }
        serde_json::json!({
            "darwin-aarch64": darwin,
            "linux-x86_64": {
                "url": asset_url(base, version, &format!("Pisper_{version}_linux_x86_64.AppImage")),
                "signature": FIXTURE_SIGNATURE,
            },
            "windows-x86_64": {
                "url": asset_url(base, version, &format!("Pisper_{version}_windows_x86_64-setup.exe")),
                "signature": FIXTURE_SIGNATURE,
            },
        })
    }

    #[test]
    fn manifest_picks_the_installer_for_each_platform() {
        let bytes = manifest(
            "0.6.38",
            full_platforms(RELEASE_DOWNLOAD_BASE, "0.6.38", true),
        );
        let windows = parse_manifest(&bytes, InstallerPlatform::WindowsX64).unwrap();
        assert_eq!(windows.version, version("0.6.38"));
        assert_eq!(
            windows.release_url,
            "https://github.com/ling-kong-ran/pisper/releases/tag/v0.6.38"
        );
        assert_eq!(
            windows.published_at.as_deref(),
            Some("2026-11-12T08:00:00.000Z")
        );
        assert!(windows.notes.contains("fix"));
        let installer = windows.installer.unwrap();
        assert_eq!(
            installer.file_name,
            "Pisper_0.6.38_windows_x86_64-setup.exe"
        );
        assert_eq!(
            installer.url,
            "https://github.com/ling-kong-ran/pisper/releases/download/v0.6.38/Pisper_0.6.38_windows_x86_64-setup.exe"
        );
        assert_eq!(installer.signature, FIXTURE_SIGNATURE);

        let linux = parse_manifest(&bytes, InstallerPlatform::LinuxX64).unwrap();
        assert_eq!(
            linux.installer.unwrap().file_name,
            "Pisper_0.6.38_linux_x86_64.AppImage"
        );

        // macOS 用附加的 installer 字段里的 dmg，而不是 url 指向的 .app.tar.gz 更新包。
        let darwin = parse_manifest(&bytes, InstallerPlatform::DarwinAarch64).unwrap();
        assert_eq!(
            darwin.installer.unwrap().file_name,
            "Pisper_0.6.38_darwin_aarch64.dmg"
        );
    }

    #[test]
    fn macos_without_a_signed_dmg_only_reports_the_new_version() {
        let bytes = manifest(
            "0.6.38",
            full_platforms(RELEASE_DOWNLOAD_BASE, "0.6.38", false),
        );
        let darwin = parse_manifest(&bytes, InstallerPlatform::DarwinAarch64).unwrap();
        assert_eq!(darwin.version, version("0.6.38"));
        assert_eq!(darwin.installer, None);

        // 清单里没有本平台时同样只提示新版本。
        let bytes = manifest("0.6.38", serde_json::json!({}));
        let windows = parse_manifest(&bytes, InstallerPlatform::WindowsX64).unwrap();
        assert_eq!(windows.installer, None);
    }

    #[test]
    fn manifest_rejects_foreign_or_mismatched_urls() {
        let name = "Pisper_0.6.38_windows_x86_64-setup.exe";
        let foreign = [
            format!("https://github.com/someone-else/pisper/releases/download/v0.6.38/{name}"),
            format!("https://github.com/ling-kong-ran/pisper/releases/download/v0.6.37/{name}"),
            format!("http://github.com/ling-kong-ran/pisper/releases/download/v0.6.38/{name}"),
            format!("https://github.com/ling-kong-ran/pisper/releases/download/v0.6.38/{name}?x=1"),
            format!("https://example.test/https://github.com/ling-kong-ran/pisper/releases/download/v0.6.38/{name}"),
            "https://github.com/ling-kong-ran/pisper/releases/download/v0.6.38/Pisper_0.6.38_windows_x86_64-offline-setup.exe".into(),
            "https://github.com/ling-kong-ran/pisper/releases/download/v0.6.38/../v0.6.38/evil.exe".into(),
        ];
        for url in foreign {
            let bytes = manifest(
                "0.6.38",
                serde_json::json!({ "windows-x86_64": { "url": url, "signature": FIXTURE_SIGNATURE } }),
            );
            let error = parse_manifest(&bytes, InstallerPlatform::WindowsX64).unwrap_err();
            assert!(
                format!("{error:#}").contains("not a Pisper release asset"),
                "{url}: {error:#}"
            );
        }
        // 附加的 dmg 字段同样只能指向本次发布。
        let bytes = manifest(
            "0.6.38",
            serde_json::json!({ "darwin-aarch64": { "installer": {
                "url": "https://evil.example/Pisper_0.6.38_darwin_aarch64.dmg",
                "signature": FIXTURE_SIGNATURE,
            } } }),
        );
        assert!(parse_manifest(&bytes, InstallerPlatform::DarwinAarch64).is_err());
    }

    #[test]
    fn manifest_rejects_invalid_fields() {
        let good = full_platforms(RELEASE_DOWNLOAD_BASE, "0.6.38", true);
        for bad_version in [
            "0.6",
            "v0.6.38",
            "0.6.38-beta.1",
            "0.6.38+build",
            " 0.6.38",
            "",
        ] {
            let bytes = manifest(bad_version, good.clone());
            assert!(
                parse_manifest(&bytes, InstallerPlatform::WindowsX64).is_err(),
                "{bad_version}"
            );
        }
        let bad_signature = manifest(
            "0.6.38",
            serde_json::json!({ "windows-x86_64": {
                "url": asset_url(RELEASE_DOWNLOAD_BASE, "0.6.38", "Pisper_0.6.38_windows_x86_64-setup.exe"),
                "signature": "not a signature",
            } }),
        );
        assert!(parse_manifest(&bad_signature, InstallerPlatform::WindowsX64).is_err());
        let wrong_types = [
            serde_json::json!({ "version": "0.6.38", "notes": 1, "platforms": good }),
            serde_json::json!({ "version": "0.6.38", "pub_date": "x y", "platforms": good }),
            serde_json::json!({ "version": "0.6.38", "platforms": [] }),
            serde_json::json!({ "version": "0.6.38" }),
            serde_json::json!({ "version": "0.6.38", "platforms": { "windows-x86_64": "x" } }),
            serde_json::json!({ "version": "0.6.38", "platforms": { "windows-x86_64": { "url": 1, "signature": FIXTURE_SIGNATURE } } }),
            serde_json::json!([]),
        ];
        for value in wrong_types {
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(
                parse_manifest(&bytes, InstallerPlatform::WindowsX64).is_err(),
                "{value}"
            );
        }
        assert!(parse_manifest(b"not json", InstallerPlatform::WindowsX64).is_err());
        let huge = vec![b' '; 2 * 1024 * 1024];
        assert!(parse_manifest(&huge, InstallerPlatform::WindowsX64).is_err());
    }

    #[test]
    fn versions_compare_by_precedence() {
        assert!(is_newer(&version("0.6.37"), &version("0.6.38")));
        assert!(is_newer(&version("0.6.9"), &version("0.6.10")));
        assert!(!is_newer(&version("0.6.38"), &version("0.6.38")));
        assert!(!is_newer(&version("0.6.38"), &version("0.6.37")));
        // 构建元数据不算新版本。
        assert!(!is_newer(&version("0.6.38"), &version("0.6.38+abc")));
        assert!(is_newer(&version("0.6.38-beta.1"), &version("0.6.38")));
    }

    #[test]
    fn installer_names_round_trip_only_for_the_platform() {
        let v = version("0.6.38");
        for platform in InstallerPlatform::ALL {
            let name = platform.installer_name(&v);
            assert_eq!(platform.installer_version(&name), Some(v.clone()));
            for other in InstallerPlatform::ALL
                .into_iter()
                .filter(|other| *other != platform)
            {
                assert_eq!(other.installer_version(&name), None);
            }
        }
        let windows = InstallerPlatform::WindowsX64;
        for name in [
            "Pisper_0.6.38_windows_x86_64-offline-setup.exe",
            "Pisper_00.6.38_windows_x86_64-setup.exe",
            "Pisper_0.6_windows_x86_64-setup.exe",
            "pisper_0.6.38_windows_x86_64-setup.exe",
            "Pisper_0.6.38_windows_x86_64-setup.exe.bak",
            "Pisper_Desktop_0.6.38_windows_x86_64.tar.gz",
        ] {
            assert_eq!(windows.installer_version(name), None, "{name}");
        }
        assert_eq!(
            classify(
                windows,
                ".Pisper_0.6.38_windows_x86_64-setup.exe.123-4.part"
            ),
            Some(OwnedFile::Partial)
        );
        assert_eq!(
            classify(windows, ".Pisper_0.6.38_windows_x86_64-setup.exe.part"),
            None
        );
        assert_eq!(classify(windows, ".notes.123-4.part"), None);
        assert_eq!(
            classify(
                windows,
                ".Pisper_0.6.38_windows_x86_64-setup.exe.sig.1-2.part"
            ),
            Some(OwnedFile::Partial)
        );
        assert_eq!(
            classify(windows, "Pisper_0.6.38_windows_x86_64-setup.exe.sig"),
            Some(OwnedFile::Signature(version("0.6.38")))
        );
    }

    #[test]
    fn cleanup_keeps_only_the_newest_and_touches_only_owned_names() {
        let directory = TestDirectory::new();
        let root = &directory.0;
        let platform = InstallerPlatform::WindowsX64;
        let owned = [
            "Pisper_0.6.36_windows_x86_64-setup.exe",
            "Pisper_0.6.36_windows_x86_64-setup.exe.sig",
            "Pisper_0.6.37_windows_x86_64-setup.exe",
            "Pisper_0.6.38_windows_x86_64-setup.exe",
            "Pisper_0.6.38_windows_x86_64-setup.exe.sig",
            ".Pisper_0.6.38_windows_x86_64-setup.exe.77-0.part",
        ];
        let foreign = [
            "settings.json",
            "download.lock",
            "notes.txt",
            "Pisper_0.6.36_windows_x86_64-offline-setup.exe",
            "Pisper_0.6.36_linux_x86_64.AppImage",
            "Pisper_0.6.36_windows_x86_64-setup.exe.bak",
            "My Pisper_0.6.36_windows_x86_64-setup.exe",
        ];
        for name in owned.iter().chain(foreign.iter()) {
            fs::write(root.join(name), name).unwrap();
        }
        // 名字对得上的目录也不删。
        fs::create_dir(root.join("Pisper_0.6.35_windows_x86_64-setup.exe")).unwrap();

        let removed = remove_stale_files(root, platform, Some(&version("0.6.38")), false);
        assert_eq!(
            removed,
            vec![
                ".Pisper_0.6.38_windows_x86_64-setup.exe.77-0.part",
                "Pisper_0.6.36_windows_x86_64-setup.exe",
                "Pisper_0.6.36_windows_x86_64-setup.exe.sig",
                "Pisper_0.6.37_windows_x86_64-setup.exe",
            ]
        );
        assert!(root
            .join("Pisper_0.6.38_windows_x86_64-setup.exe")
            .is_file());
        assert!(root
            .join("Pisper_0.6.38_windows_x86_64-setup.exe.sig")
            .is_file());
        for name in foreign {
            assert!(root.join(name).is_file(), "{name}");
        }
        assert!(root.join("Pisper_0.6.35_windows_x86_64-setup.exe").is_dir());

        // 下载开始前只清临时文件，旧安装包要等新包验签通过才删。
        fs::write(
            root.join(".Pisper_0.6.39_windows_x86_64-setup.exe.1-1.part"),
            "x",
        )
        .unwrap();
        let removed = remove_stale_files(root, platform, None, true);
        assert_eq!(
            removed,
            vec![".Pisper_0.6.39_windows_x86_64-setup.exe.1-1.part"]
        );
        assert!(root
            .join("Pisper_0.6.38_windows_x86_64-setup.exe")
            .is_file());
    }

    #[test]
    fn settings_default_to_automatic_download_and_round_trip() {
        let directory = TestDirectory::new();
        assert!(read_settings(&directory.0).auto_download);
        write_settings(
            &directory.0,
            ShellInstallerSettings {
                auto_download: false,
            },
        )
        .unwrap();
        assert!(!read_settings(&directory.0).auto_download);
        fs::write(directory.0.join("settings.json"), "{broken").unwrap();
        assert!(read_settings(&directory.0).auto_download);
    }

    struct Server {
        address: std::net::SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        fn start(respond: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let respond = Arc::new(respond);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        return;
                    };
                    let recorded = recorded.clone();
                    let respond = respond.clone();
                    thread::spawn(move || {
                        let mut request = Vec::new();
                        let mut chunk = [0_u8; 1024];
                        while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                            let count = stream.read(&mut chunk).unwrap_or(0);
                            if count == 0 {
                                return;
                            }
                            request.extend_from_slice(&chunk[..count]);
                        }
                        let request = String::from_utf8_lossy(&request).to_string();
                        let path = request.split_whitespace().nth(1).unwrap().to_string();
                        let response = respond(&path);
                        recorded.lock().unwrap().push(path);
                        let _ = stream.write_all(&response);
                    });
                }
            });
            Self { address, requests }
        }

        fn base(&self) -> String {
            format!("http://{}/releases/download/", self.address)
        }

        fn updater(&self, root: &Path, platform: InstallerPlatform) -> ShellInstallerUpdater {
            let mut updater = ShellInstallerUpdater::new(
                root.to_path_buf(),
                platform,
                TEST_PUBLIC_KEY,
                "Pisper shell installer test",
            )
            .unwrap();
            updater.manifest_url = format!("http://{}/latest.json", self.address);
            updater.download_base = self.base();
            updater
        }

        fn count(&self, prefix: &str) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|path| path.starts_with(prefix))
                .count()
        }
    }

    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut value = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        value.extend_from_slice(body);
        value
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn release_server(version: &'static str, payload: &'static [u8]) -> Server {
        let manifest_cell: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let cell = manifest_cell.clone();
        let server = Server::start(move |path| {
            if path == "/latest.json" {
                return response("200 OK", "", &cell.lock().unwrap());
            }
            if path.ends_with("-setup.exe") || path.ends_with(".dmg") || path.ends_with(".AppImage")
            {
                return response("200 OK", "", payload);
            }
            response("404 Not Found", "", b"")
        });
        *manifest_cell.lock().unwrap() =
            manifest(version, full_platforms(&server.base(), version, true));
        server
    }

    #[test]
    fn a_verified_download_replaces_older_installers_and_is_not_downloaded_twice() {
        let server = release_server("0.6.38", FIXTURE_PAYLOAD);
        let directory = TestDirectory::new();
        let root = &directory.0;
        fs::write(root.join("Pisper_0.6.37_darwin_aarch64.dmg"), "old").unwrap();
        fs::write(root.join("Pisper_0.6.37_darwin_aarch64.dmg.sig"), "old").unwrap();
        fs::write(root.join("keep-me.txt"), "user file").unwrap();
        let updater = server.updater(root, InstallerPlatform::DarwinAarch64);

        let release = run(updater.fetch_release()).unwrap();
        assert!(release.is_newer_than(&version("0.6.37")));
        let mut progress = Vec::new();
        let installer =
            run(updater.download(&release, |done, total| progress.push((done, total)))).unwrap();
        assert_eq!(installer.file_name, "Pisper_0.6.38_darwin_aarch64.dmg");
        assert_eq!(fs::read(&installer.path).unwrap(), FIXTURE_PAYLOAD);
        assert_eq!(installer.size, FIXTURE_PAYLOAD.len() as u64);
        assert_eq!(
            progress.last(),
            Some(&(FIXTURE_PAYLOAD.len() as u64, FIXTURE_PAYLOAD.len() as u64))
        );
        assert!(!root.join("Pisper_0.6.37_darwin_aarch64.dmg").exists());
        assert!(!root.join("Pisper_0.6.37_darwin_aarch64.dmg.sig").exists());
        assert!(root.join("keep-me.txt").is_file());
        assert_eq!(server.count("/releases/download/"), 1);

        // 再来一轮：同一个已验签的包不重下。
        let again = run(updater.download(&release, |_, _| {})).unwrap();
        assert_eq!(again, installer);
        assert_eq!(server.count("/releases/download/"), 1);
        assert_eq!(
            updater.verified_installer(&version("0.6.37")),
            Some(installer)
        );
    }

    #[test]
    fn signature_failure_leaves_no_installer_under_the_final_name() {
        let server = release_server("0.6.38", b"tampered installer bytes");
        let directory = TestDirectory::new();
        let root = &directory.0;
        let updater = server.updater(root, InstallerPlatform::WindowsX64);
        let release = run(updater.fetch_release()).unwrap();
        let error = run(updater.download(&release, |_, _| {})).unwrap_err();
        assert!(
            format!("{error:#}").contains("verification failed"),
            "{error:#}"
        );
        let leftovers = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.contains("Pisper"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert_eq!(updater.verified_installer(&version("0.6.37")), None);
    }

    #[test]
    fn signatures_from_another_key_are_rejected() {
        let server = release_server("0.6.38", FIXTURE_PAYLOAD);
        let directory = TestDirectory::new();
        let mut updater = server.updater(&directory.0, InstallerPlatform::LinuxX64);
        updater.public_key =
            super::decode_public_key(include_str!("../../../src-tauri/updater.pubkey")).unwrap();
        let release = run(updater.fetch_release()).unwrap();
        assert!(run(updater.download(&release, |_, _| {})).is_err());
        assert!(!directory
            .0
            .join("Pisper_0.6.38_linux_x86_64.AppImage")
            .exists());
    }

    #[test]
    fn prune_removes_installers_the_shell_already_runs() {
        let server = release_server("0.6.38", FIXTURE_PAYLOAD);
        let directory = TestDirectory::new();
        let root = &directory.0;
        let updater = server.updater(root, InstallerPlatform::WindowsX64);
        let release = run(updater.fetch_release()).unwrap();
        run(updater.download(&release, |_, _| {})).unwrap();
        fs::write(
            root.join(".Pisper_0.6.38_windows_x86_64-setup.exe.9-9.part"),
            "x",
        )
        .unwrap();

        // 还在跑旧壳：安装包留着，只清掉残留的临时文件。
        let kept = updater.prune(&version("0.6.37")).unwrap();
        assert_eq!(kept.version, version("0.6.38"));
        assert!(!root
            .join(".Pisper_0.6.38_windows_x86_64-setup.exe.9-9.part")
            .exists());

        // 用户装好了 0.6.38：留下的安装包没用了。
        assert_eq!(updater.prune(&version("0.6.38")), None);
        assert!(!root.join("Pisper_0.6.38_windows_x86_64-setup.exe").exists());
        assert!(!root
            .join("Pisper_0.6.38_windows_x86_64-setup.exe.sig")
            .exists());

        // 被改过的安装包验不过签，启动时同样删掉。
        run(updater.download(&release, |_, _| {})).unwrap();
        fs::write(
            root.join("Pisper_0.6.38_windows_x86_64-setup.exe"),
            "tampered",
        )
        .unwrap();
        assert_eq!(updater.prune(&version("0.6.37")), None);
        assert!(!root.join("Pisper_0.6.38_windows_x86_64-setup.exe").exists());
    }

    #[test]
    fn a_rate_limited_manifest_is_not_requested_again_during_backoff() {
        let server = Server::start(|_| {
            response(
                "429 Too Many Requests",
                "Retry-After: 600\r\n",
                b"{\"message\":\"rate limit\"}",
            )
        });
        let directory = TestDirectory::new();
        let updater = server.updater(&directory.0, InstallerPlatform::WindowsX64);
        let error = run(updater.fetch_release()).unwrap_err();
        let retry_at = super::rate_limited_until(&error).unwrap();
        assert!(retry_at > super::unix_now());
        assert_eq!(server.count("/latest.json"), 1);
        let again = run(updater.fetch_release()).unwrap_err();
        assert_eq!(super::rate_limited_until(&again), Some(retry_at));
        assert_eq!(server.count("/latest.json"), 1);
    }

    #[test]
    fn a_missing_manifest_is_a_plain_error_without_retries() {
        let server = Server::start(|_| response("404 Not Found", "", b""));
        let directory = TestDirectory::new();
        let updater = server.updater(&directory.0, InstallerPlatform::WindowsX64);
        let error = run(updater.fetch_release()).unwrap_err();
        assert_eq!(super::rate_limited_until(&error), None);
        assert_eq!(server.count("/latest.json"), 1);
    }

    #[test]
    fn manifests_from_the_test_server_must_use_its_own_download_base() {
        let bytes = manifest(
            "0.6.38",
            full_platforms(RELEASE_DOWNLOAD_BASE, "0.6.38", true),
        );
        assert!(parse_manifest_from(
            &bytes,
            InstallerPlatform::WindowsX64,
            "http://127.0.0.1:1/releases/download/"
        )
        .is_err());
    }
}
