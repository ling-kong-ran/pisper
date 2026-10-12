use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::read::GzDecoder;
use fs2::FileExt as _;
use minisign_verify::{PublicKey, Signature};
use reqwest::{
    header::{ACCEPT, CONTENT_RANGE, RANGE, USER_AGENT},
    StatusCode,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Cursor,
    path::{Component as PathComponent, Path, PathBuf},
    time::Duration,
};

const RELEASES_API: &str =
    "https://api.github.com/repos/ling-kong-ran/pisper/releases?per_page=100";
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const NETWORK_ATTEMPTS: usize = 3;
const MIN_PARALLEL_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024;
const MIN_DOWNLOAD_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PARALLEL_DOWNLOADS: u64 = 8;

enum ParallelDownloadEvent {
    Progress { index: usize, transferred: u64 },
    Complete { index: usize, result: Result<()> },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Desktop,
    Tui,
    Runtime,
}

impl Component {
    pub const ALL: [Self; 3] = [Self::Desktop, Self::Tui, Self::Runtime];

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "desktop" => Ok(Self::Desktop),
            "tui" => Ok(Self::Tui),
            "runtime" => Ok(Self::Runtime),
            _ => bail!("unsupported Pisper component: {value}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Tui => "tui",
            Self::Runtime => "runtime",
        }
    }

    fn tag_prefix(self) -> &'static str {
        match self {
            Self::Desktop => "v",
            Self::Tui => "tui-v",
            Self::Runtime => "runtime-v",
        }
    }

    fn executable_name(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Self::Desktop, _) => "index.html",
            (Self::Tui, true) => "pisper.exe",
            (Self::Tui, false) => "pisper",
            (Self::Runtime, true) => "pisper-sidecar.exe",
            (Self::Runtime, false) => "pisper-sidecar",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub component: Component,
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub archive_url: String,
    pub signature_url: String,
    /// github.com 直连地址，作为 API 地址不可达时的回退候选（也用于镜像前缀拼接）。
    pub archive_fallback_url: String,
    /// 签名文件的 github.com 直连回退地址。
    pub signature_fallback_url: String,
    pub size: u64,
}

impl ReleaseInfo {
    /// 组件归档的下载候选链：API 资产地址优先，其次 github.com 直连，最后为镜像中转。
    pub fn archive_candidates(&self) -> Vec<String> {
        download_candidates(&self.archive_url, &self.archive_fallback_url)
    }

    /// 签名文件的下载候选链，顺序与归档一致。
    pub fn signature_candidates(&self) -> Vec<String> {
        download_candidates(&self.signature_url, &self.signature_fallback_url)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentPointer {
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComponentManifest {
    #[serde(default)]
    name: Option<String>,
    version: String,
    platform: String,
    arch: String,
    command: String,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    upgrade_notice: bool,
}

#[derive(Clone, Debug)]
pub struct InstalledComponent {
    pub component: Component,
    pub version: Version,
    pub root: PathBuf,
}

impl InstalledComponent {
    pub fn executable(&self) -> PathBuf {
        if self.component == Component::Desktop {
            return self
                .root
                .join("dist")
                .join(self.component.executable_name());
        }
        self.root.join(self.component.executable_name())
    }

    pub fn runtime_root(&self) -> Option<PathBuf> {
        (self.component == Component::Runtime).then(|| self.root.join("sidecar-runtime"))
    }

    pub fn frontend_root(&self) -> Option<PathBuf> {
        (self.component == Component::Desktop).then(|| self.root.join("dist"))
    }
}

#[derive(Clone, Deserialize)]
struct GitHubAsset {
    name: String,
    #[serde(rename = "url")]
    api_url: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Clone)]
pub struct ComponentUpdater {
    root: PathBuf,
    public_key: PublicKey,
    client: reqwest::Client,
}

impl ComponentUpdater {
    pub fn new(root: PathBuf, encoded_public_key: &str, user_agent: &str) -> Result<Self> {
        let public_key = decode_wrapped(encoded_public_key)
            .and_then(|value| PublicKey::decode(&value).map_err(anyhow::Error::from))
            .context("invalid Pisper component updater public key")?;
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(10 * 60))
            .build()
            .context("failed to create component update client")?;
        Ok(Self {
            root,
            public_key,
            client,
        })
    }

    pub fn components_root(&self) -> &Path {
        &self.root
    }

    pub fn installed(&self, component: Component) -> Result<Option<InstalledComponent>> {
        resolve_installed(&self.root, component)
    }

    pub async fn latest(&self, component: Component) -> Result<ReleaseInfo> {
        let releases = self.releases_with_retry().await?;
        releases
            .into_iter()
            .filter_map(|release| release_info(component, release))
            .max_by(|left, right| version_of(&left.version).cmp(&version_of(&right.version)))
            .ok_or_else(|| anyhow!("no signed {} release is available", component.name()))
    }

    async fn releases_with_retry(&self) -> Result<Vec<GitHubRelease>> {
        let urls = release_feed_candidates();
        let mut last_error = None;
        for url in &urls {
            for attempt in 0..NETWORK_ATTEMPTS {
                let result = async {
                    self.client
                        .get(url.as_str())
                        .header(ACCEPT, "application/vnd.github+json")
                        .header(USER_AGENT, "Pisper component updater")
                        .send()
                        .await
                        .context("failed to request Pisper component releases")?
                        .error_for_status()
                        .context("Pisper component release request failed")?
                        .json::<Vec<GitHubRelease>>()
                        .await
                        .context("invalid Pisper component release response")
                }
                .await;
                match result {
                    Ok(releases) => return Ok(releases),
                    Err(error) => last_error = Some(error),
                }
                if attempt + 1 < NETWORK_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                }
            }
        }
        let error = last_error.unwrap_or_else(|| anyhow!("component release request failed"));
        Err(error.context(format!(
            "component release request failed after {NETWORK_ATTEMPTS} attempts"
        )))
    }

    pub async fn install(&self, release: &ReleaseInfo) -> Result<InstalledComponent> {
        self.install_with_progress(release, |_, _| {}).await
    }

    pub async fn install_with_progress<F>(
        &self,
        release: &ReleaseInfo,
        mut on_progress: F,
    ) -> Result<InstalledComponent>
    where
        F: FnMut(u64, u64) + Send,
    {
        let archive = self
            .download_candidates_with_progress(
                &release.archive_candidates(),
                MAX_ARCHIVE_BYTES,
                release.size,
                &mut on_progress,
            )
            .await?;
        let signature = self
            .download_candidates_with_progress(
                &release.signature_candidates(),
                MAX_SIGNATURE_BYTES,
                0,
                |_, _| {},
            )
            .await?;
        verify_archive(&self.public_key, &archive, &signature)?;

        let root = self.root.clone();
        let component = release.component;
        let version = Version::parse(&release.version).context("invalid component version")?;
        tokio::task::spawn_blocking(move || install_archive(&root, component, &version, &archive))
            .await
            .context("component installer task failed")?
    }

    /// 依次尝试候选下载地址：每个地址内部仍会多次重试，
    /// 全部重试失败后回退到下一个候选地址，直到候选耗尽。
    async fn download_candidates_with_progress<F>(
        &self,
        urls: &[String],
        limit: u64,
        expected_size: u64,
        mut on_progress: F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(u64, u64),
    {
        let mut last_error = None;
        for url in urls {
            match self
                .download_with_progress(url, limit, expected_size, &mut on_progress)
                .await
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    last_error = Some(error);
                    on_progress(0, expected_size);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("component download has no candidate URLs")))
    }

    async fn download_with_progress<F>(
        &self,
        url: &str,
        limit: u64,
        expected_size: u64,
        mut on_progress: F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(u64, u64),
    {
        if expected_size >= MIN_PARALLEL_DOWNLOAD_BYTES
            && expected_size <= limit
            && self.supports_range_download(url, expected_size).await
        {
            match self
                .download_in_parallel(url, limit, expected_size, &mut on_progress)
                .await
            {
                Ok(bytes) => return Ok(bytes),
                Err(_) => on_progress(0, expected_size),
            }
        }

        let mut last_error = None;
        for attempt in 0..NETWORK_ATTEMPTS {
            if attempt > 0 {
                on_progress(0, expected_size);
            }
            match self
                .download_once(url, limit, expected_size, &mut on_progress)
                .await
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < NETWORK_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            }
        }
        let error = last_error.unwrap_or_else(|| anyhow!("component download failed"));
        Err(error.context(format!(
            "component download failed after {NETWORK_ATTEMPTS} attempts: {url}"
        )))
    }

    async fn supports_range_download(&self, url: &str, expected_size: u64) -> bool {
        let result = async {
            let mut response = self
                .client
                .get(url)
                .header(ACCEPT, "application/octet-stream")
                .header(RANGE, "bytes=0-0")
                .send()
                .await?;
            if response.status() != StatusCode::PARTIAL_CONTENT
                || parse_content_range(response.headers().get(CONTENT_RANGE))
                    != Some((0, 0, expected_size))
                || response.content_length().is_some_and(|size| size != 1)
            {
                return Ok::<bool, reqwest::Error>(false);
            }
            let mut received = 0_u64;
            while let Some(chunk) = response.chunk().await? {
                received = received.saturating_add(chunk.len() as u64);
                if received > 1 {
                    return Ok(false);
                }
            }
            Ok(received == 1)
        }
        .await;
        result.unwrap_or(false)
    }

    async fn download_in_parallel<F>(
        &self,
        url: &str,
        limit: u64,
        expected_size: u64,
        mut on_progress: F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(u64, u64),
    {
        if expected_size > limit {
            bail!("component download exceeds the size limit");
        }
        let buffer_size = usize::try_from(expected_size)
            .context("component download is too large for this platform")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(buffer_size)
            .context("failed to reserve component download buffer")?;
        bytes.resize(buffer_size, 0);
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(bytes));

        let segment_count = expected_size
            .div_ceil(MIN_DOWNLOAD_SEGMENT_BYTES)
            .clamp(2, MAX_PARALLEL_DOWNLOADS);
        let segment_size = expected_size.div_ceil(segment_count);
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::with_capacity(segment_count as usize);
        for index in 0..segment_count as usize {
            let start = index as u64 * segment_size;
            let end = (start + segment_size - 1).min(expected_size - 1);
            let client = self.client.clone();
            let url = url.to_string();
            let sender = sender.clone();
            let bytes = bytes.clone();
            tasks.push(tokio::spawn(async move {
                let result = download_range_with_retry(
                    &client,
                    &url,
                    start,
                    end,
                    expected_size,
                    bytes,
                    index,
                    &sender,
                )
                .await;
                let _ = sender.send(ParallelDownloadEvent::Complete { index, result });
            }));
        }
        drop(sender);

        let mut segment_progress = vec![0_u64; segment_count as usize];
        let mut completed = 0_usize;
        let mut failure = None;
        while completed < segment_count as usize {
            let Some(event) = receiver.recv().await else {
                failure = Some(anyhow!(
                    "parallel component download task stopped unexpectedly"
                ));
                break;
            };
            match event {
                ParallelDownloadEvent::Progress { index, transferred } => {
                    segment_progress[index] = transferred;
                    on_progress(segment_progress.iter().sum(), expected_size);
                }
                ParallelDownloadEvent::Complete { index, result } => match result {
                    Ok(()) => {
                        let start = index as u64 * segment_size;
                        let end = (start + segment_size - 1).min(expected_size - 1);
                        segment_progress[index] = end - start + 1;
                        completed += 1;
                        on_progress(segment_progress.iter().sum(), expected_size);
                    }
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                },
            }
        }

        if let Some(error) = failure {
            for task in &tasks {
                task.abort();
            }
            for task in tasks {
                let _ = task.await;
            }
            return Err(error.context("parallel component download failed"));
        }
        for task in tasks {
            task.await
                .context("parallel component download task failed")?;
        }
        let bytes = std::sync::Arc::try_unwrap(bytes)
            .map_err(|_| anyhow!("parallel component download buffer is still in use"))?
            .into_inner()
            .map_err(|_| anyhow!("parallel component download buffer was poisoned"))?;
        Ok(bytes)
    }

    async fn download_once<F>(
        &self,
        url: &str,
        limit: u64,
        expected_size: u64,
        mut on_progress: F,
    ) -> Result<Vec<u8>>
    where
        F: FnMut(u64, u64),
    {
        let mut response = self
            .client
            .get(url)
            .header(ACCEPT, "application/octet-stream")
            .send()
            .await
            .with_context(|| format!("failed to download {url}"))?
            .error_for_status()
            .with_context(|| format!("component download failed: {url}"))?;
        if response.content_length().is_some_and(|size| size > limit) {
            bail!("component download exceeds the size limit");
        }
        let total = response.content_length().unwrap_or(expected_size);
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("component download was interrupted")?
        {
            let transferred = (bytes.len() as u64)
                .checked_add(chunk.len() as u64)
                .context("component download size overflow")?;
            if transferred > limit {
                bail!("component download exceeds the size limit");
            }
            bytes.extend_from_slice(&chunk);
            on_progress(transferred, total);
        }
        Ok(bytes)
    }
}

fn parse_content_range(value: Option<&reqwest::header::HeaderValue>) -> Option<(u64, u64, u64)> {
    let value = value?.to_str().ok()?.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

#[allow(clippy::too_many_arguments)]
async fn download_range_with_retry(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
    total: u64,
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    index: usize,
    sender: &tokio::sync::mpsc::UnboundedSender<ParallelDownloadEvent>,
) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..NETWORK_ATTEMPTS {
        let _ = sender.send(ParallelDownloadEvent::Progress {
            index,
            transferred: 0,
        });
        match download_range_once(client, url, start, end, total, bytes.clone(), index, sender)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < NETWORK_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
    }
    let error = last_error.unwrap_or_else(|| anyhow!("component range download failed"));
    Err(error.context(format!(
        "component range download failed after {NETWORK_ATTEMPTS} attempts: {start}-{end}"
    )))
}

#[allow(clippy::too_many_arguments)]
async fn download_range_once(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
    total: u64,
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    index: usize,
    sender: &tokio::sync::mpsc::UnboundedSender<ParallelDownloadEvent>,
) -> Result<()> {
    let mut response = client
        .get(url)
        .header(ACCEPT, "application/octet-stream")
        .header(RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .with_context(|| format!("failed to download component range {start}-{end}"))?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        bail!(
            "component server ignored range {start}-{end}: HTTP {}",
            response.status()
        );
    }
    if parse_content_range(response.headers().get(CONTENT_RANGE)) != Some((start, end, total)) {
        bail!("component server returned an invalid Content-Range for {start}-{end}");
    }
    let expected = end - start + 1;
    if response
        .content_length()
        .is_some_and(|length| length != expected)
    {
        bail!("component range {start}-{end} has an invalid size");
    }

    let mut transferred = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("component range download was interrupted")?
    {
        let next = transferred
            .checked_add(chunk.len() as u64)
            .context("component range download size overflow")?;
        if next > expected {
            bail!("component range {start}-{end} exceeds its expected size");
        }
        let write_start = usize::try_from(start + transferred)
            .context("component range offset is too large for this platform")?;
        let write_end = write_start
            .checked_add(chunk.len())
            .context("component range offset overflow")?;
        bytes
            .lock()
            .map_err(|_| anyhow!("parallel component download buffer was poisoned"))?
            [write_start..write_end]
            .copy_from_slice(&chunk);
        transferred = next;
        let _ = sender.send(ParallelDownloadEvent::Progress { index, transferred });
    }
    if transferred != expected {
        bail!("component range {start}-{end} ended after {transferred} of {expected} bytes");
    }
    Ok(())
}

pub fn deactivate_component(root: &Path, component: Component) -> Result<()> {
    let pointer = root.join(component.name()).join("current.json");
    ensure_component_path(root, &pointer)?;
    match fs::remove_file(pointer) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to deactivate component"),
    }
}

pub fn resolve_installed(root: &Path, component: Component) -> Result<Option<InstalledComponent>> {
    let component_root = root.join(component.name());
    ensure_component_path(root, &component_root)?;
    if !component_root.exists() {
        return Ok(None);
    }
    let lock_path = component_root.join("install.lock");
    ensure_component_path(root, &lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .context("failed to open component pointer lock")?;
    // 读取完整指针及布局使用同一共享锁，不能观察安装器切换目录时的中间状态。
    fs2::FileExt::lock_shared(&lock).context("failed to lock component pointer")?;
    resolve_installed_locked(root, component)
}

fn resolve_installed_locked(
    root: &Path,
    component: Component,
) -> Result<Option<InstalledComponent>> {
    let pointer_path = root.join(component.name()).join("current.json");
    ensure_component_path(root, &pointer_path)?;
    let pointer = match fs::read(&pointer_path) {
        Ok(value) => {
            serde_json::from_slice::<CurrentPointer>(&value).context("invalid component pointer")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to read component pointer"),
    };
    let version =
        Version::parse(&pointer.version).context("invalid installed component version")?;
    let version_root = root
        .join(component.name())
        .join("versions")
        .join(version.to_string());
    ensure_component_path(root, &version_root)?;
    validate_layout(component, &version_root, &version)?;
    Ok(Some(InstalledComponent {
        component,
        version,
        root: version_root,
    }))
}

pub fn component_asset_name(component: Component, version: &str) -> String {
    match component {
        Component::Desktop => format!(
            "Pisper_Desktop_Rust_{}_{}_{}.tar.gz",
            version,
            platform_name(),
            architecture_name()
        ),
        Component::Tui => format!(
            "Pisper_TUI_Component_{}_{}_{}.tar.gz",
            version,
            platform_name(),
            architecture_name()
        ),
        Component::Runtime => format!(
            "Pisper_Runtime_Rust_{}_{}_{}.tar.gz",
            version,
            platform_name(),
            architecture_name()
        ),
    }
}

fn release_info(component: Component, release: GitHubRelease) -> Option<ReleaseInfo> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.strip_prefix(component.tag_prefix())?;
    Version::parse(version).ok()?;
    let mut archive_names = vec![component_asset_name(component, version)];
    // v0.6.0 已使用 Rust，却仍沿用 Node 前端资产名；只在读取这一个历史版本时兼容。
    // 新产物必须隔离身份，防止旧 Node 更新器只升级前端后继续启动旧 Runtime。
    if component == Component::Desktop && version == "0.6.0" {
        archive_names.push(format!(
            "Pisper_Desktop_{}_{}_{}.tar.gz",
            version,
            platform_name(),
            architecture_name()
        ));
    }
    let (archive, signature) = archive_names.iter().find_map(|archive_name| {
        let signature_name = format!("{archive_name}.sig");
        Some((
            release
                .assets
                .iter()
                .find(|asset| asset.name == *archive_name)?,
            release
                .assets
                .iter()
                .find(|asset| asset.name == signature_name)?,
        ))
    })?;
    Some(ReleaseInfo {
        component,
        version: version.to_string(),
        tag: release.tag_name,
        notes: release.body.unwrap_or_default(),
        release_url: release.html_url,
        published_at: release.published_at,
        archive_url: archive.api_url.clone(),
        signature_url: signature.api_url.clone(),
        archive_fallback_url: archive.browser_download_url.clone(),
        signature_fallback_url: signature.browser_download_url.clone(),
        size: archive.size,
    })
}

/// 组装组件下载候选地址列表：GitHub API 资产地址优先（对直连 github.com
/// 受限的网络更友好），随后回退到 github.com 直连地址；若设置了
/// PISPER_GITHUB_MIRROR（逗号分隔的镜像前缀），再追加经镜像中转的直连地址。
/// 组件包经过 minisign 签名校验，镜像只改善可达性，不影响完整性。
fn download_candidates(primary: &str, browser_url: &str) -> Vec<String> {
    download_candidates_with_mirrors(primary, browser_url, &mirror_prefixes())
}

fn download_candidates_with_mirrors(
    primary: &str,
    browser_url: &str,
    mirrors: &[String],
) -> Vec<String> {
    let mut urls = vec![primary.to_string()];
    if !browser_url.is_empty() && browser_url != primary {
        urls.push(browser_url.to_string());
        urls.extend(
            mirrors
                .iter()
                .map(|prefix| format!("{prefix}{browser_url}")),
        );
    }
    urls
}

/// 组件发布列表的候选地址：GitHub API 优先，镜像（若配置）兜底。
fn release_feed_candidates() -> Vec<String> {
    let mut urls = vec![RELEASES_API.to_string()];
    urls.extend(
        mirror_prefixes()
            .iter()
            .map(|prefix| format!("{prefix}{RELEASES_API}")),
    );
    urls
}

/// 解析 PISPER_GITHUB_MIRROR：逗号分隔的 http(s) 镜像前缀，统一补全结尾斜杠，
/// 非法前缀直接忽略，避免拼出意外地址。
fn parse_mirror_prefixes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|prefix| prefix.starts_with("https://") || prefix.starts_with("http://"))
        .map(|prefix| format!("{}/", prefix.trim_end_matches('/')))
        .collect()
}

fn mirror_prefixes() -> Vec<String> {
    std::env::var("PISPER_GITHUB_MIRROR")
        .ok()
        .map(|value| parse_mirror_prefixes(&value))
        .unwrap_or_default()
}

fn version_of(value: &str) -> Version {
    Version::parse(value).unwrap_or_else(|_| Version::new(0, 0, 0))
}

fn decode_wrapped(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if let Ok(decoded) = BASE64.decode(trimmed) {
        if let Ok(text) = String::from_utf8(decoded) {
            return Ok(text);
        }
    }
    Ok(trimmed.to_string())
}

fn verify_archive(public_key: &PublicKey, archive: &[u8], signature: &[u8]) -> Result<()> {
    let encoded = std::str::from_utf8(signature).context("component signature is not UTF-8")?;
    let decoded = decode_wrapped(encoded)?;
    let signature = Signature::decode(&decoded).context("invalid component signature")?;
    public_key
        .verify(archive, &signature, false)
        .context("component signature verification failed")
}

fn install_archive(
    root: &Path,
    component: Component,
    version: &Version,
    bytes: &[u8],
) -> Result<InstalledComponent> {
    let component_root = root.join(component.name());
    ensure_component_path(root, &component_root)?;
    fs::create_dir_all(&component_root).context("failed to create component directory")?;
    ensure_component_path(root, &component_root.join("install.lock"))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(component_root.join("install.lock"))
        .context("failed to open the component install lock")?;
    lock.lock_exclusive()
        .context("failed to lock the component installer")?;
    let versions_root = component_root.join("versions");
    let destination = versions_root.join(version.to_string());
    ensure_component_path(root, &destination)?;
    fs::create_dir_all(&versions_root).context("failed to create component versions directory")?;

    let staging = versions_root.join(format!(".{}.tmp-{}", version, std::process::id()));
    let backup = versions_root.join(format!(".{}.previous-{}", version, std::process::id()));
    ensure_component_path(root, &staging)?;
    ensure_component_path(root, &backup)?;
    ensure_component_path(root, &component_root.join("current.json"))?;
    ensure_component_path(root, &component_root.join("current.json.tmp"))?;
    ensure_component_path(root, &component_root.join("current.json.bak"))?;
    if staging.exists() {
        fs::remove_dir_all(&staging).context("failed to clear stale component staging")?;
    }
    let _ = fs::remove_dir_all(&backup);
    fs::create_dir_all(&staging).context("failed to create component staging directory")?;
    let result = extract_archive(bytes, &staging)
        .and_then(|_| validate_layout(component, &staging, version))
        .context("staged component validation failed");
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let had_destination = destination.exists();
    if had_destination {
        fs::rename(&destination, &backup)
            .context("failed to back up the installed component version")?;
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        if had_destination {
            let _ = fs::rename(&backup, &destination);
        }
        return Err(error).context("failed to activate component version directory");
    }
    let _ = fs::remove_dir_all(backup);
    validate_layout(component, &destination, version)
        .context("activated component validation failed")?;
    write_pointer(&component_root, version)?;
    cleanup_versions(&versions_root, component, version);
    // 返回仍在安装锁保护下验证的版本，避免另一个安装器切换current之后二次查询。
    Ok(InstalledComponent {
        component,
        version: version.clone(),
        root: destination,
    })
}

fn cleanup_versions(versions_root: &Path, component: Component, active: &Version) {
    let Ok(entries) = fs::read_dir(versions_root) else {
        return;
    };
    let mut versions = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let version = Version::parse(entry.file_name().to_str()?).ok()?;
            if entry.file_name() != std::ffi::OsStr::new(&version.to_string())
                || validate_layout(component, &entry.path(), &version).is_err()
            {
                return None;
            }
            Some((version, entry.path()))
        })
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    let mut retained_previous = false;
    for (version, path) in versions {
        if &version == active {
            continue;
        }
        if !retained_previous {
            retained_previous = true;
            continue;
        }
        let _ = fs::remove_dir_all(path);
    }
}

fn extract_archive(bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut extracted_bytes = 0_u64;
    let mut entry_count = 0_usize;
    for entry in archive.entries().context("invalid component archive")? {
        entry_count += 1;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            bail!("component archive contains too many entries");
        }
        let mut entry = entry.context("invalid component archive entry")?;
        let entry_type = entry.header().entry_type();
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .context("component archive size overflow")?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            bail!("component archive expands beyond the size limit");
        }
        if !entry_type.is_file() && !entry_type.is_dir() {
            bail!("component archive contains a link or unsupported entry");
        }
        let path = entry.path().context("invalid component archive path")?;
        let relative = strip_archive_root(&path)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry_type.is_dir() {
            fs::create_dir_all(&target).context("failed to create component directory")?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .context("failed to create component parent directory")?;
            }
            entry
                .unpack(&target)
                .context("failed to extract component file")?;
        }
    }
    Ok(())
}

fn strip_archive_root(path: &Path) -> Result<PathBuf> {
    let mut normal = Vec::new();
    for component in path.components() {
        match component {
            PathComponent::Normal(value) => normal.push(value.to_os_string()),
            PathComponent::CurDir => {}
            _ => bail!("component archive path escapes its root"),
        }
    }
    if normal.is_empty() {
        return Ok(PathBuf::new());
    }
    Ok(normal.into_iter().skip(1).collect())
}

// 只检查调用方拥有的组件根及其相对后代；不把符号链接规范化成另一个可删除目录。
fn ensure_component_path(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .context("component path escapes its root")?;
    let mut current = root.to_path_buf();
    for part in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(part) = part {
            let PathComponent::Normal(name) = part else {
                bail!("component path contains an invalid segment");
            };
            current.push(name);
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("component path contains a symbolic link");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to inspect component path"),
        }
    }
    Ok(())
}

fn component_file(root: &Path, path: &Path) -> Result<()> {
    ensure_component_path(root, path)?;
    if !fs::metadata(path)?.is_file() {
        bail!("component entry is not a regular file");
    }
    Ok(())
}

fn read_component_manifest(root: &Path) -> Result<ComponentManifest> {
    let path = root.join("manifest.json");
    component_file(root, &path)?;
    if fs::metadata(&path)?.len() > MAX_MANIFEST_BYTES {
        bail!("component manifest exceeds the size limit");
    }
    serde_json::from_slice(&fs::read(path)?).context("component manifest is invalid")
}

fn manifest_matches(component: Component, manifest: &ComponentManifest, version: &Version) -> bool {
    manifest.version == version.to_string()
        && manifest.platform == platform_name()
        && manifest.arch == architecture_name()
        && manifest.command
            == if component == Component::Desktop {
                "dist/index.html"
            } else {
                component.executable_name()
            }
        && match component {
            Component::Desktop => manifest.name.as_deref() == Some("pisper-desktop"),
            Component::Runtime => manifest.name.as_deref() == Some("pisper-runtime"),
            Component::Tui => true,
        }
}

fn validate_layout(component: Component, root: &Path, version: &Version) -> Result<()> {
    let entry = if component == Component::Desktop {
        root.join("dist").join(component.executable_name())
    } else {
        root.join(component.executable_name())
    };
    component_file(root, &entry).context("component entry is missing or unsafe")?;
    let manifest = read_component_manifest(root)?;
    if !manifest_matches(component, &manifest, version) {
        bail!("component manifest does not match the requested identity, platform and version");
    }
    if component == Component::Desktop {
        // 完整 Rust 安装不能再激活旧 Node 前端或用于指导旧壳升级的提示页。
        let historical_rust = version == &Version::new(0, 6, 0) && manifest.backend.is_none();
        if manifest.upgrade_notice
            || !(manifest.backend.as_deref() == Some("rust") || historical_rust)
        {
            bail!("desktop component backend is incompatible; expected rust");
        }
    }
    if component == Component::Runtime {
        if manifest.backend.as_deref() != Some("rust") || manifest.upgrade_notice {
            bail!("runtime component backend is incompatible; expected rust");
        }
        let package = root.join("sidecar-runtime").join("package.json");
        component_file(root, &package).context("runtime payload is missing or unsafe")?;
        let metadata: serde_json::Value = serde_json::from_slice(&fs::read(package)?)?;
        if metadata.get("backend").and_then(|value| value.as_str()) != Some("rust") {
            bail!("runtime payload backend does not match its manifest");
        }
    }
    Ok(())
}

fn obsolete_legacy_component(component: Component, path: &Path, version: &Version) -> bool {
    let Ok(manifest) = read_component_manifest(path) else {
        return false;
    };
    if !manifest_matches(component, &manifest, version) {
        return false;
    }
    if component == Component::Desktop
        && manifest.backend.as_deref() == Some("upgrade-notice")
        && manifest.upgrade_notice
    {
        return component_file(path, &path.join("dist/index.html")).is_ok();
    }
    // 只清除能够识别的 0.5.x 及更早 Node 组件，未知或 Rust 组件整个保留。
    if version.major != 0
        || version.minor >= 6
        || manifest.upgrade_notice
        || !matches!(manifest.backend.as_deref(), None | Some("node"))
    {
        return false;
    }
    match component {
        Component::Desktop => component_file(path, &path.join("dist/index.html")).is_ok(),
        Component::Runtime => {
            let package = path.join("sidecar-runtime/package.json");
            if component_file(path, &path.join(component.executable_name())).is_err()
                || component_file(path, &path.join("sidecar-runtime/runtime/sidecar.mjs")).is_err()
                || component_file(path, &package).is_err()
                || !fs::metadata(&package)
                    .is_ok_and(|metadata| metadata.len() <= MAX_MANIFEST_BYTES)
            {
                return false;
            }
            fs::read(package)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|metadata| match metadata.get("backend") {
                    None | Some(serde_json::Value::Null) => true,
                    Some(serde_json::Value::String(backend)) => backend == "node",
                    _ => false,
                })
        }
        Component::Tui => false,
    }
}

/// 完整 Rust 应用已健康启动后，回收应用自管的旧 Node 与升级提示组件。
/// 未成功启动时不触碰缓存；正在安装的组件和未知目录保持原样。
pub fn cleanup_legacy_components(root: &Path, rust_started: bool) -> Result<usize> {
    if !rust_started || !root.exists() {
        return Ok(0);
    }
    ensure_component_path(root, root)?;
    let mut removed = 0;
    for component in [Component::Desktop, Component::Runtime] {
        let component_root = root.join(component.name());
        if !component_root.exists() {
            continue;
        }
        ensure_component_path(root, &component_root)?;
        let lock_path = component_root.join("install.lock");
        ensure_component_path(root, &lock_path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .context("failed to open component cleanup lock")?;
        // 安装期间不等待或改写指针，由下次健康启动重试。
        if lock.try_lock_exclusive().is_err() {
            continue;
        }
        let versions = component_root.join("versions");
        if !versions.exists() {
            continue;
        }
        ensure_component_path(root, &versions)?;
        let pointer_path = component_root.join("current.json");
        ensure_component_path(root, &pointer_path)?;
        let pointer = fs::read(&pointer_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CurrentPointer>(&bytes).ok());
        for entry in fs::read_dir(&versions).context("failed to list component cache")? {
            let entry = entry.context("failed to inspect component cache")?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(version) = entry
                .file_name()
                .to_str()
                .and_then(|name| Version::parse(name).ok())
            else {
                continue;
            };
            if entry.file_name() != std::ffi::OsStr::new(&version.to_string()) {
                continue;
            }
            let path = entry.path();
            ensure_component_path(root, &path)?;
            if !obsolete_legacy_component(component, &path, &version) {
                continue;
            }
            if pointer
                .as_ref()
                .is_some_and(|pointer| pointer.version == version.to_string())
            {
                // 先使已判定不兼容的指针失效，删除失败也不会重新激活旧前端。
                fs::remove_file(&pointer_path)
                    .context("failed to deactivate obsolete component")?;
            }
            fs::remove_dir_all(path).context("failed to remove obsolete component cache")?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn write_pointer(component_root: &Path, version: &Version) -> Result<()> {
    fs::create_dir_all(component_root).context("failed to create component directory")?;
    let destination = component_root.join("current.json");
    let temporary = component_root.join("current.json.tmp");
    let backup = component_root.join("current.json.bak");
    let content = serde_json::to_vec_pretty(&CurrentPointer {
        version: version.to_string(),
    })?;
    fs::write(&temporary, [content.as_slice(), b"\n"].concat())
        .context("failed to stage component pointer")?;
    let had_destination = destination.exists();
    if had_destination {
        let _ = fs::remove_file(&backup);
        fs::rename(&destination, &backup).context("failed to back up component pointer")?;
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        if had_destination {
            let _ = fs::rename(&backup, &destination);
        }
        return Err(error).context("failed to activate component pointer");
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        _ => "linux",
    }
}

fn architecture_name() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        component_asset_name, extract_archive, install_archive, release_info, resolve_installed,
        strip_archive_root, Component, GitHubAsset, GitHubRelease,
    };
    use flate2::{write::GzEncoder, Compression};
    use semver::Version;
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::{Path, PathBuf},
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pisper-component-updater-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read_request_range(stream: &mut impl Read) -> Option<(u64, u64)> {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let count = stream.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|value| value == b"\r\n\r\n") {
                break;
            }
            assert!(request.len() <= 16 * 1024);
        }
        let request = String::from_utf8(request).unwrap();
        let value = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("range").then_some(value.trim())
        })?;
        let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
        Some((start.parse().ok()?, end.parse().ok()?))
    }

    fn desktop_archive(version: &str) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let contents = b"<!doctype html>";
        let mut index_header = tar::Header::new_gnu();
        index_header.set_size(contents.len() as u64);
        index_header.set_mode(0o644);
        index_header.set_cksum();
        builder
            .append_data(
                &mut index_header,
                "root/dist/index.html",
                contents.as_slice(),
            )
            .unwrap();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "version": version,
            "platform": super::platform_name(),
            "arch": super::architecture_name(),
            "command": "dist/index.html",
            "name": "pisper-desktop",
            "backend": "rust",
        }))
        .unwrap();
        let mut manifest_header = tar::Header::new_gnu();
        manifest_header.set_size(manifest.len() as u64);
        manifest_header.set_mode(0o644);
        manifest_header.set_cksum();
        builder
            .append_data(
                &mut manifest_header,
                "root/manifest.json",
                manifest.as_slice(),
            )
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn tui_archive(contents: &[u8]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut executable_header = tar::Header::new_gnu();
        executable_header.set_size(contents.len() as u64);
        executable_header.set_mode(0o755);
        executable_header.set_cksum();
        builder
            .append_data(
                &mut executable_header,
                format!("root/{}", Component::Tui.executable_name()),
                contents,
            )
            .unwrap();
        let manifest = serde_json::to_vec(&serde_json::json!({
            "version": std::str::from_utf8(contents).unwrap(),
            "platform": super::platform_name(),
            "arch": super::architecture_name(),
            "command": Component::Tui.executable_name(),
        }))
        .unwrap();
        let mut manifest_header = tar::Header::new_gnu();
        manifest_header.set_size(manifest.len() as u64);
        manifest_header.set_mode(0o644);
        manifest_header.set_cksum();
        builder
            .append_data(
                &mut manifest_header,
                "root/manifest.json",
                manifest.as_slice(),
            )
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn component_download_reports_response_progress() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc")
                .unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(100));
            stream.write_all(b"def").unwrap();
        });
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper progress test",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut progress = Vec::new();
        let bytes = runtime
            .block_on(updater.download_with_progress(
                &format!("http://{address}/component.tar.gz"),
                1024,
                6,
                |transferred, total| progress.push((transferred, total)),
            ))
            .unwrap();
        server.join().unwrap();

        assert_eq!(bytes, b"abcdef");
        assert_eq!(progress.last(), Some(&(6, 6)));
        assert!(progress.iter().all(|(_, total)| *total == 6));
        assert!(progress.iter().any(|(transferred, _)| *transferred < 6));
    }

    #[test]
    fn interrupted_component_download_retries_and_resets_progress() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request).unwrap();
                let body = if attempt == 0 {
                    &b"abc"[..]
                } else {
                    &b"abcdef"[..]
                };
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n")
                    .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper retry test",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut progress = Vec::new();
        let bytes = runtime
            .block_on(updater.download_with_progress(
                &format!("http://{address}/component.tar.gz"),
                1024,
                6,
                |transferred, total| progress.push((transferred, total)),
            ))
            .unwrap();
        server.join().unwrap();

        assert_eq!(bytes, b"abcdef");
        assert!(progress.contains(&(0, 6)));
        assert_eq!(progress.last(), Some(&(6, 6)));
    }

    #[test]
    fn large_component_download_uses_parallel_ranges() {
        let size = super::MIN_PARALLEL_DOWNLOAD_BYTES as usize;
        let payload = std::sync::Arc::new(
            (0..size)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let ranges = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_payload = payload.clone();
        let server_ranges = ranges.clone();
        let server = thread::spawn(move || {
            let mut workers = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let payload = server_payload.clone();
                let ranges = server_ranges.clone();
                workers.push(thread::spawn(move || {
                    let (start, end) = read_request_range(&mut stream).unwrap();
                    ranges.lock().unwrap().push((start, end));
                    stream
                        .write_all(
                            format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                                end - start + 1,
                                payload.len(),
                            )
                            .as_bytes(),
                        )
                        .unwrap();
                    stream
                        .write_all(&payload[start as usize..=end as usize])
                        .unwrap();
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper parallel download test",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut progress = Vec::new();
        let bytes = runtime
            .block_on(updater.download_with_progress(
                &format!("http://{address}/component.tar.gz"),
                size as u64,
                size as u64,
                |transferred, total| progress.push((transferred, total)),
            ))
            .unwrap();
        server.join().unwrap();

        assert_eq!(bytes, *payload);
        assert_eq!(progress.last(), Some(&(size as u64, size as u64)));
        let mut ranges = ranges.lock().unwrap().clone();
        ranges.sort_unstable();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0], (0, 0));
        assert!(ranges.iter().any(|(start, _)| *start > 0));
    }

    #[test]
    fn component_download_falls_back_when_ranges_are_ignored() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let server = thread::spawn(move || {
            for body in [&b"x"[..], &b"fallback"[..]] {
                let (mut stream, _) = listener.accept().unwrap();
                let range = read_request_range(&mut stream);
                server_requests.lock().unwrap().push(range);
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                let _ = stream.write_all(body);
            }
        });
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper range fallback test",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bytes = runtime
            .block_on(updater.download_with_progress(
                &format!("http://{address}/component.tar.gz"),
                super::MIN_PARALLEL_DOWNLOAD_BYTES,
                super::MIN_PARALLEL_DOWNLOAD_BYTES,
                |_, _| {},
            ))
            .unwrap();
        server.join().unwrap();

        assert_eq!(bytes, b"fallback");
        assert_eq!(requests.lock().unwrap().as_slice(), &[Some((0, 0)), None]);
    }

    #[test]
    fn component_assets_are_platform_specific() {
        let desktop = component_asset_name(Component::Desktop, "1.2.3");
        assert!(desktop.starts_with("Pisper_Desktop_Rust_1.2.3_"));
        assert!(desktop.ends_with(".tar.gz"));
        let tui = component_asset_name(Component::Tui, "1.2.3");
        assert!(tui.starts_with("Pisper_TUI_Component_1.2.3_"));
        assert!(tui.ends_with(".tar.gz"));
        assert!(component_asset_name(Component::Runtime, "1.2.3")
            .starts_with("Pisper_Runtime_Rust_1.2.3_"));
    }

    fn desktop_release(version: &str, archive: String) -> GitHubRelease {
        GitHubRelease {
            tag_name: format!("v{version}"),
            html_url: "https://example.test/release".into(),
            body: Some("desktop fixture".into()),
            published_at: None,
            draft: false,
            prerelease: false,
            assets: vec![
                GitHubAsset {
                    name: archive.clone(),
                    api_url: "https://api.example.test/archive".into(),
                    browser_download_url: "https://github.example.test/archive".into(),
                    size: 42,
                },
                GitHubAsset {
                    name: format!("{archive}.sig"),
                    api_url: "https://api.example.test/signature".into(),
                    browser_download_url: "https://github.example.test/signature".into(),
                    size: 512,
                },
            ],
        }
    }

    #[test]
    fn rust_desktop_releases_are_invisible_to_the_legacy_node_selector() {
        let version = "0.6.1";
        let legacy_name = format!(
            "Pisper_Desktop_{}_{}_{}.tar.gz",
            version,
            super::platform_name(),
            super::architecture_name()
        );
        let release = desktop_release(version, component_asset_name(Component::Desktop, version));
        // 0.5.80/81 的选择器要求这一个完整名字与签名，不能识别 Rust 专属身份。
        assert!(!release.assets.iter().any(|asset| asset.name == legacy_name));
        assert!(!release
            .assets
            .iter()
            .any(|asset| asset.name == format!("{legacy_name}.sig")));
        assert_eq!(
            release_info(Component::Desktop, release).unwrap().version,
            version
        );
    }

    #[test]
    fn rust_desktop_selector_only_accepts_the_renamed_historical_rust_release() {
        for (version, allowed) in [
            ("0.5.80", false),
            ("0.5.81", false),
            ("0.6.0", true),
            ("0.6.1", false),
        ] {
            let legacy_name = format!(
                "Pisper_Desktop_{}_{}_{}.tar.gz",
                version,
                super::platform_name(),
                super::architecture_name()
            );
            assert_eq!(
                release_info(Component::Desktop, desktop_release(version, legacy_name)).is_some(),
                allowed
            );
        }
    }

    #[test]
    fn rust_desktop_release_requires_the_matching_signature_and_stable_public_release() {
        for rejection in ["signature", "draft", "prerelease"] {
            let mut release =
                desktop_release("0.6.1", component_asset_name(Component::Desktop, "0.6.1"));
            match rejection {
                "signature" => {
                    release.assets.pop();
                }
                "draft" => release.draft = true,
                "prerelease" => release.prerelease = true,
                _ => unreachable!(),
            }
            assert!(release_info(Component::Desktop, release).is_none());
        }
    }

    #[test]
    fn runtime_layout_rejects_legacy_and_mismatched_backends() {
        let directory = TestDirectory::new();
        let root = &directory.0;
        let version = Version::parse("1.2.3").unwrap();
        fs::create_dir_all(root.join("sidecar-runtime")).unwrap();
        fs::write(root.join(Component::Runtime.executable_name()), b"fixture").unwrap();
        for (backend, payload, valid) in [
            (None, "node", false),
            (Some("rust"), "node", false),
            (Some("rust"), "rust", true),
        ] {
            fs::write(root.join("manifest.json"), serde_json::to_vec(&serde_json::json!({
                "version": "1.2.3", "platform": super::platform_name(), "arch": super::architecture_name(),
                "command": Component::Runtime.executable_name(), "backend": backend,
                "name": "pisper-runtime",
            })).unwrap()).unwrap();
            fs::write(
                root.join("sidecar-runtime/package.json"),
                serde_json::to_vec(&serde_json::json!({"backend":payload})).unwrap(),
            )
            .unwrap();
            assert_eq!(
                super::validate_layout(Component::Runtime, root, &version).is_ok(),
                valid
            );
        }
    }

    #[test]
    fn desktop_archives_install_only_the_frontend_payload() {
        let directory = TestDirectory::new();
        let version = Version::parse("1.2.3").unwrap();
        install_archive(
            &directory.0,
            Component::Desktop,
            &version,
            &desktop_archive("1.2.3"),
        )
        .unwrap();
        let installed = resolve_installed(&directory.0, Component::Desktop)
            .unwrap()
            .unwrap();
        assert_eq!(installed.version, version);
        assert!(installed
            .frontend_root()
            .unwrap()
            .join("index.html")
            .is_file());
        assert!(!installed.root.join("pisper-sidecar").exists());
    }

    fn cached_component(
        root: &Path,
        component: Component,
        version: &str,
        backend: Option<&str>,
    ) -> PathBuf {
        let path = root.join(component.name()).join("versions").join(version);
        fs::create_dir_all(&path).unwrap();
        let command = if component == Component::Desktop {
            "dist/index.html"
        } else {
            component.executable_name()
        };
        let entry = path.join(command);
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(entry, b"fixture").unwrap();
        let name = format!("pisper-{}", component.name());
        fs::write(
            path.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": name, "version": version, "platform": super::platform_name(),
                "arch": super::architecture_name(), "command": command, "backend": backend,
                "upgradeNotice": backend == Some("upgrade-notice"),
            }))
            .unwrap(),
        )
        .unwrap();
        if component == Component::Runtime {
            fs::create_dir_all(path.join("sidecar-runtime/runtime")).unwrap();
            fs::write(
                path.join("sidecar-runtime/runtime/sidecar.mjs"),
                b"node worker",
            )
            .unwrap();
            fs::write(
                path.join("sidecar-runtime/package.json"),
                serde_json::to_vec(&serde_json::json!({"backend": backend})).unwrap(),
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn rust_desktop_layout_rejects_node_and_upgrade_notice_payloads() {
        let directory = TestDirectory::new();
        for (version, backend, accepted) in [
            ("0.5.81", None, false),
            ("0.6.0", None, true),
            ("0.6.1", None, false),
            ("0.6.1", Some("node"), false),
            ("0.6.1", Some("upgrade-notice"), false),
            ("0.6.1", Some("rust"), true),
        ] {
            let path = cached_component(&directory.0, Component::Desktop, version, backend);
            super::write_pointer(
                &directory.0.join("desktop"),
                &Version::parse(version).unwrap(),
            )
            .unwrap();
            assert_eq!(
                resolve_installed(&directory.0, Component::Desktop).is_ok(),
                accepted
            );
            if accepted {
                assert!(resolve_installed(&directory.0, Component::Desktop)
                    .unwrap()
                    .is_some())
            }
            fs::remove_dir_all(path).unwrap();
        }
        let path = cached_component(&directory.0, Component::Desktop, "0.6.1", Some("rust"));
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
        manifest["name"] = serde_json::json!("unrelated-component");
        fs::write(
            path.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(resolve_installed(&directory.0, Component::Desktop).is_err());
    }

    #[test]
    fn successful_rust_startup_only_reclaims_identified_legacy_cache() {
        let directory = TestDirectory::new();
        let root = directory.0.join("components");
        let old_desktop = cached_component(&root, Component::Desktop, "0.5.81", None);
        let old_runtime = cached_component(&root, Component::Runtime, "0.5.81", None);
        let notice = cached_component(&root, Component::Desktop, "0.6.1", Some("upgrade-notice"));
        let desktop = cached_component(&root, Component::Desktop, "0.6.2", Some("rust"));
        let runtime = cached_component(&root, Component::Runtime, "0.6.2", Some("rust"));
        let old_rust = cached_component(&root, Component::Runtime, "0.5.90", Some("rust"));
        // Rust 组件中的 Node 语音工作进程仍是有效资源，不能按文件名回收。
        fs::create_dir_all(runtime.join("sidecar-runtime/node_modules/speech")).unwrap();
        fs::write(
            runtime.join("sidecar-runtime/node_modules/speech/worker.mjs"),
            b"required worker",
        )
        .unwrap();
        let unknown = root.join("desktop/versions/0.5.79");
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("personal.txt"), b"unknown cache").unwrap();
        let foreign = cached_component(&root, Component::Desktop, "0.5.78", None);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(foreign.join("manifest.json")).unwrap()).unwrap();
        manifest["name"] = serde_json::json!("foreign-data");
        fs::write(
            foreign.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let agent = directory.0.join("agent/sessions");
        fs::create_dir_all(&agent).unwrap();
        fs::write(agent.join("history.jsonl"), b"user sessions").unwrap();
        super::write_pointer(&root.join("desktop"), &Version::parse("0.6.1").unwrap()).unwrap();
        super::write_pointer(&root.join("runtime"), &Version::parse("0.6.2").unwrap()).unwrap();
        assert_eq!(super::cleanup_legacy_components(&root, true).unwrap(), 3);
        for path in [old_desktop, old_runtime, notice] {
            assert!(!path.exists())
        }
        for path in [desktop, runtime.clone(), old_rust, unknown, foreign] {
            assert!(path.exists())
        }
        assert_eq!(
            fs::read(runtime.join("sidecar-runtime/node_modules/speech/worker.mjs")).unwrap(),
            b"required worker"
        );
        assert_eq!(
            fs::read(agent.join("history.jsonl")).unwrap(),
            b"user sessions"
        );
        assert!(!root.join("desktop/current.json").exists());
        assert_eq!(
            resolve_installed(&root, Component::Runtime)
                .unwrap()
                .unwrap()
                .version,
            Version::parse("0.6.2").unwrap()
        );
        assert_eq!(super::cleanup_legacy_components(&root, true).unwrap(), 0);
    }

    #[test]
    fn cleanup_keeps_unknown_identity_and_conflicting_rust_payload_metadata() {
        let directory = TestDirectory::new();
        for (index, (field, value)) in [
            ("name", "foreign-data"),
            ("version", "0.5.1"),
            ("arch", "other-architecture"),
            ("platform", "other-platform"),
            ("command", "other-command"),
            ("backend", "unknown-backend"),
        ]
        .into_iter()
        .enumerate()
        {
            let path = cached_component(
                &directory.0,
                Component::Desktop,
                &format!("0.5.{}", 70 + index),
                None,
            );
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
            manifest[field] = serde_json::json!(value);
            fs::write(
                path.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
        }
        let conflicting = cached_component(&directory.0, Component::Runtime, "0.5.81", None);
        fs::write(
            conflicting.join("sidecar-runtime/package.json"),
            br#"{"backend":"rust"}"#,
        )
        .unwrap();
        assert_eq!(
            super::cleanup_legacy_components(&directory.0, true).unwrap(),
            0
        );
        assert!(conflicting
            .join("sidecar-runtime/runtime/sidecar.mjs")
            .exists());
        assert_eq!(
            fs::read_dir(directory.0.join("desktop/versions"))
                .unwrap()
                .count(),
            6
        );
    }

    #[test]
    fn failed_startup_and_busy_installer_leave_legacy_cache_unchanged() {
        use fs2::FileExt as _;
        let directory = TestDirectory::new();
        let root = &directory.0;
        let old = cached_component(root, Component::Desktop, "0.5.81", None);
        super::write_pointer(&root.join("desktop"), &Version::parse("0.5.81").unwrap()).unwrap();
        let pointer = fs::read(root.join("desktop/current.json")).unwrap();
        assert_eq!(super::cleanup_legacy_components(root, false).unwrap(), 0);
        assert!(!root.join("desktop/install.lock").exists());
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("desktop/install.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        assert_eq!(super::cleanup_legacy_components(root, true).unwrap(), 0);
        assert_eq!(
            fs::read(root.join("desktop/current.json")).unwrap(),
            pointer
        );
        assert!(old.is_dir());
        drop(lock);
        assert_eq!(super::cleanup_legacy_components(root, true).unwrap(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn component_cleanup_and_resolution_do_not_follow_symbolic_links() {
        use std::os::unix::fs::symlink;
        let directory = TestDirectory::new();
        let outside = directory.0.join("outside");
        let old = cached_component(&outside, Component::Desktop, "0.5.81", None);
        super::write_pointer(&outside.join("desktop"), &Version::parse("0.5.81").unwrap()).unwrap();
        let root = directory.0.join("components");
        symlink(&outside, &root).unwrap();
        assert!(super::cleanup_legacy_components(&root, true).is_err());
        assert!(resolve_installed(&root, Component::Desktop).is_err());
        assert!(old.exists());
        fs::remove_file(&root).unwrap();
        fs::create_dir_all(root.join("desktop/versions")).unwrap();
        symlink(&old, root.join("desktop/versions/0.5.81")).unwrap();
        assert_eq!(super::cleanup_legacy_components(&root, true).unwrap(), 0);
        assert!(old.exists());
        fs::remove_file(root.join("desktop/versions/0.5.81")).unwrap();
        let cache = cached_component(&root, Component::Desktop, "0.5.81", None);
        fs::remove_file(cache.join("manifest.json")).unwrap();
        symlink(old.join("manifest.json"), cache.join("manifest.json")).unwrap();
        assert_eq!(super::cleanup_legacy_components(&root, true).unwrap(), 0);
        assert!(old.join("manifest.json").is_file());
        assert!(cache.exists());
        assert!(install_archive(
            &directory.0.join("components-link"),
            Component::Desktop,
            &Version::parse("0.6.1").unwrap(),
            &desktop_archive("0.6.1")
        )
        .is_ok());
        let unsafe_root = directory.0.join("unsafe");
        symlink(&outside, &unsafe_root).unwrap();
        assert!(install_archive(
            &unsafe_root,
            Component::Desktop,
            &Version::parse("0.6.1").unwrap(),
            &desktop_archive("0.6.1")
        )
        .is_err());
        assert!(!outside.join("desktop/versions/0.6.1").exists());
    }

    #[test]
    fn release_requires_the_archive_and_its_signature() {
        let archive = component_asset_name(Component::Runtime, "1.2.3");
        let release = GitHubRelease {
            tag_name: "runtime-v1.2.3".into(),
            html_url: "https://example.test/release".into(),
            body: Some("notes".into()),
            published_at: None,
            draft: false,
            prerelease: false,
            assets: vec![
                GitHubAsset {
                    name: archive.clone(),
                    api_url: "https://api.example.test/archive".into(),
                    browser_download_url: "https://github.example.test/archive".into(),
                    size: 42,
                },
                GitHubAsset {
                    name: format!("{archive}.sig"),
                    api_url: "https://api.example.test/signature".into(),
                    browser_download_url: "https://github.example.test/signature".into(),
                    size: 512,
                },
            ],
        };
        let info = release_info(Component::Runtime, release).unwrap();
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.archive_url, "https://api.example.test/archive");
        assert_eq!(info.signature_url, "https://api.example.test/signature");
        assert_eq!(
            info.archive_fallback_url,
            "https://github.example.test/archive"
        );
        assert_eq!(
            info.signature_fallback_url,
            "https://github.example.test/signature"
        );
        assert_eq!(info.size, 42);
    }

    #[test]
    fn mirror_prefixes_are_parsed_and_normalized() {
        assert!(super::parse_mirror_prefixes("").is_empty());
        assert!(super::parse_mirror_prefixes("ftp://bad, not-a-url").is_empty());
        assert_eq!(
            super::parse_mirror_prefixes("https://gh-proxy.com, https://mirror.example/root/"),
            vec![
                "https://gh-proxy.com/".to_string(),
                "https://mirror.example/root/".to_string(),
            ]
        );
    }

    #[test]
    fn download_candidates_prefer_api_then_browser_then_mirrors() {
        let mirrors = vec!["https://gh-proxy.com/".to_string()];
        assert_eq!(
            super::download_candidates_with_mirrors(
                "https://api.example.test/asset",
                "https://github.example.test/asset",
                &mirrors,
            ),
            vec![
                "https://api.example.test/asset".to_string(),
                "https://github.example.test/asset".to_string(),
                "https://gh-proxy.com/https://github.example.test/asset".to_string(),
            ]
        );
        // 直连地址与主地址相同或为空时去重，且不再追加镜像候选。
        assert_eq!(
            super::download_candidates_with_mirrors("https://same", "https://same", &mirrors),
            vec!["https://same".to_string()]
        );
        assert_eq!(
            super::download_candidates_with_mirrors("https://same", "", &mirrors),
            vec!["https://same".to_string()]
        );
    }

    #[test]
    fn component_download_falls_back_to_the_next_candidate() {
        // 第一个候选地址持续返回 500（重试 NETWORK_ATTEMPTS 次后放弃），
        // 第二个候选地址正常提供内容，验证候选链回退能拿到完整数据。
        let failing = TcpListener::bind("127.0.0.1:0").unwrap();
        let failing_address = failing.local_addr().unwrap();
        let failing_server = thread::spawn(move || {
            for _ in 0..super::NETWORK_ATTEMPTS {
                let (mut stream, _) = failing.accept().unwrap();
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            }
        });
        let serving = TcpListener::bind("127.0.0.1:0").unwrap();
        let serving_address = serving.local_addr().unwrap();
        let serving_server = thread::spawn(move || {
            let (mut stream, _) = serving.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
                )
                .unwrap();
        });
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper fallback test",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bytes = runtime
            .block_on(updater.download_candidates_with_progress(
                &[
                    format!("http://{failing_address}/component.tar.gz"),
                    format!("http://{serving_address}/component.tar.gz"),
                ],
                1024,
                6,
                |_, _| {},
            ))
            .unwrap();
        failing_server.join().unwrap();
        serving_server.join().unwrap();

        assert_eq!(bytes, b"abcdef");
    }

    #[test]
    fn signed_archive_install_shape_switches_versions_and_bounds_history() {
        let directory = TestDirectory::new();
        for version in ["1.0.0", "1.1.0", "1.2.0"] {
            install_archive(
                &directory.0,
                Component::Tui,
                &Version::parse(version).unwrap(),
                &tui_archive(version.as_bytes()),
            )
            .unwrap();
        }
        let installed = resolve_installed(&directory.0, Component::Tui)
            .unwrap()
            .unwrap();
        assert_eq!(installed.version, Version::parse("1.2.0").unwrap());
        assert_eq!(fs::read(installed.executable()).unwrap(), b"1.2.0");
        let retained = fs::read_dir(directory.0.join("tui/versions"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .count();
        assert_eq!(retained, 2);
    }

    #[test]
    fn normal_component_installation_preserves_unknown_cache_directories() {
        let directory = TestDirectory::new();
        let unknown = directory.0.join("tui/versions/1.0.0");
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("unrecognized-data"), b"keep").unwrap();
        for version in ["2.0.0", "2.1.0", "2.2.0"] {
            install_archive(
                &directory.0,
                Component::Tui,
                &Version::parse(version).unwrap(),
                &tui_archive(version.as_bytes()),
            )
            .unwrap();
        }
        assert_eq!(
            fs::read(unknown.join("unrecognized-data")).unwrap(),
            b"keep"
        );
        assert!(!directory.0.join("tui/versions/2.0.0").exists());
        assert!(directory.0.join("tui/versions/2.1.0").exists());
        assert!(directory.0.join("tui/versions/2.2.0").exists());
    }

    #[test]
    fn concurrent_installers_leave_one_valid_active_version() {
        let directory = TestDirectory::new();
        let root = std::sync::Arc::new(directory.0.clone());
        let handles = ["2.0.0", "2.1.0"].map(|version| {
            let root = root.clone();
            let archive = tui_archive(version.as_bytes());
            let version = Version::parse(version).unwrap();
            std::thread::spawn(move || install_archive(&root, Component::Tui, &version, &archive))
        });
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let installed = resolve_installed(&root, Component::Tui).unwrap().unwrap();
        assert!(matches!(
            installed.version.to_string().as_str(),
            "2.0.0" | "2.1.0"
        ));
        assert!(installed.executable().is_file());
    }

    #[test]
    fn concurrent_reinstallers_and_pointer_readers_never_observe_partial_layouts() {
        let directory = TestDirectory::new();
        let root = std::sync::Arc::new(directory.0.clone());
        let version = Version::parse("2.0.0").unwrap();
        install_archive(&root, Component::Tui, &version, &tui_archive(b"2.0.0")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let writers = [0, 1].map(|_| {
            let root = root.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..16 {
                    let installed = install_archive(
                        &root,
                        Component::Tui,
                        &Version::parse("2.0.0").unwrap(),
                        &tui_archive(b"2.0.0"),
                    )
                    .unwrap();
                    assert_eq!(installed.version, Version::parse("2.0.0").unwrap());
                }
            })
        });
        let read_root = root.clone();
        let read_barrier = barrier.clone();
        let reader = std::thread::spawn(move || {
            read_barrier.wait();
            for _ in 0..128 {
                let installed = resolve_installed(&read_root, Component::Tui)
                    .unwrap()
                    .unwrap();
                assert_eq!(installed.version, Version::parse("2.0.0").unwrap());
            }
        });
        barrier.wait();
        for writer in writers {
            writer.join().unwrap()
        }
        reader.join().unwrap();
        assert_eq!(
            fs::read(
                resolve_installed(&root, Component::Tui)
                    .unwrap()
                    .unwrap()
                    .executable()
            )
            .unwrap(),
            b"2.0.0"
        );
    }

    #[test]
    fn extraction_rejects_links() {
        let directory = TestDirectory::new();
        let encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        builder
            .append_link(&mut header, "root/pisper", "../../outside")
            .unwrap();
        let archive = builder.into_inner().unwrap().finish().unwrap();
        let error = extract_archive(&archive, &directory.0).unwrap_err();
        assert!(error.to_string().contains("link or unsupported entry"));
    }

    #[test]
    fn extraction_strips_one_root_and_rejects_traversal() {
        assert_eq!(
            strip_archive_root(Path::new(
                "pisper-runtime-1.2.3/sidecar-runtime/package.json"
            ))
            .unwrap(),
            PathBuf::from("sidecar-runtime/package.json")
        );
        assert!(strip_archive_root(Path::new("root/../secret")).is_err());
        assert!(strip_archive_root(Path::new("/absolute/path")).is_err());
    }
}
