use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::read::GzDecoder;
use fs2::FileExt as _;
use minisign_verify::{PublicKey, Signature};
use reqwest::{
    header::{HeaderMap, ACCEPT, CONTENT_RANGE, RANGE, RETRY_AFTER, USER_AGENT},
    StatusCode,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Cursor,
    path::{Component as PathComponent, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const GITHUB_API_ORIGIN: &str = "https://api.github.com/";
const RELEASES_API: &str =
    "https://api.github.com/repos/ling-kong-ran/pisper/releases?per_page=100";
// Git 的引用列表和 releases/download 直链都不计入 GitHub API 额度，限流时靠它们继续工作。
const RELEASE_TAG_REFS: &str =
    "https://github.com/ling-kong-ran/pisper.git/info/refs?service=git-upload-pack";
const RELEASES_BASE: &str = "https://github.com/ling-kong-ran/pisper/releases";
const FEED_CACHE_FILE: &str = "release-feed.json";
const FEED_CACHE_SCHEMA: u32 = 1;
/// 手动检查要能尽快看到新版本；GitHub 自身也把这个接口缓存 60 秒，再短没有意义。
pub const CHECK_FEED_MAX_AGE: Duration = Duration::from_secs(60);
/// 安装只是执行刚才检查到的结果，用户读更新说明的时间不值得再花一次额度。
pub const INSTALL_FEED_MAX_AGE: Duration = Duration::from_secs(30 * 60);
const FEED_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MIN_RATE_LIMIT_BACKOFF_SECS: u64 = 60;
// 匿名额度按小时重置；封顶可防止错误的响应头或本机时钟把更新检查长期关掉。
const MAX_RATE_LIMIT_BACKOFF_SECS: u64 = 60 * 60;
const MAX_TAG_REFS_BYTES: u64 = 8 * 1024 * 1024;
// 标签先于资产推送，且并非每个版本都有全部平台的包，所以允许往回找几个版本。
const MAX_TAG_PROBES: usize = 5;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;
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

    fn is_withdrawn(self, version: &Version) -> bool {
        // 只撤销已发布后撤回的版本身份，不能封禁未来仍使用 Node 的 0.6.x。
        // 忽略预发布/构建后缀，避免旧产物仅改后缀就绕过撤回规则。
        version.major == 0
            && version.minor == 6
            && match self {
                Self::Desktop => matches!(version.patch, 0 | 1),
                Self::Tui | Self::Runtime => matches!(version.patch, 1..=3),
            }
    }

    fn ensure_not_withdrawn(self, version: &Version) -> Result<()> {
        if self.is_withdrawn(version) {
            bail!(
                "withdrawn Pisper {} component version: {version}",
                self.name()
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

/// 一轮检查共用的发布信息：只含当前平台各组件的最新可安装版本。
#[derive(Clone, Debug)]
pub struct ReleaseFeed {
    releases: Vec<ReleaseInfo>,
}

impl ReleaseFeed {
    pub fn latest(&self, component: Component) -> Result<ReleaseInfo> {
        self.releases
            .iter()
            .find(|release| release.component == component)
            .cloned()
            .ok_or_else(|| anyhow!("no signed {} release is available", component.name()))
    }
}

/// GitHub API 限流，且不占额度的来源也没能给出结果。
#[derive(Debug)]
struct RateLimited {
    retry_at: u64,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let minutes = self.retry_at.saturating_sub(unix_now()).div_ceil(60).max(1);
        write!(
            formatter,
            "GitHub rate limit reached; try again in about {minutes} min"
        )
    }
}

impl std::error::Error for RateLimited {}

/// 错误由限流引起时返回可重试的 Unix 时间（秒），供界面显示。
pub fn rate_limited_until(error: &anyhow::Error) -> Option<u64> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<RateLimited>())
        .map(|limited| limited.retry_at)
}

/// 落盘的发布信息和限流状态。桌面端每次命令、TUI 每次启动都会新建更新器，
/// 只有放在磁盘上才能跨调用共用，并让限流状态在重启后依然有效。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedCache {
    schema: u32,
    platform: String,
    fetched_at: u64,
    releases: Vec<ReleaseInfo>,
    rate_limited_until: Option<u64>,
}

impl FeedCache {
    fn empty() -> Self {
        Self {
            schema: FEED_CACHE_SCHEMA,
            platform: feed_platform(),
            fetched_at: 0,
            releases: Vec::new(),
            rate_limited_until: None,
        }
    }

    // 缓存只是省请求用的：读不出来就当没有，绝不能让更新检查因此失败。
    fn read(root: &Path) -> Self {
        fs::read(root.join(FEED_CACHE_FILE))
            .ok()
            .and_then(|value| serde_json::from_slice::<Self>(&value).ok())
            .filter(|cache| cache.schema == FEED_CACHE_SCHEMA && cache.platform == feed_platform())
            .map(|mut cache| {
                // 新版本可能撤回了缓存写入时还有效的版本，读出时要重新过滤。
                cache.releases.retain(|release| {
                    Version::parse(&release.version)
                        .is_ok_and(|version| !release.component.is_withdrawn(&version))
                });
                cache
            })
            .unwrap_or_else(Self::empty)
    }

    fn write(&self, root: &Path) {
        static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
        let temporary = root.join(format!(
            "{FEED_CACHE_FILE}.{}-{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::create_dir_all(root)
            .and_then(|_| {
                let content = serde_json::to_vec(self).map_err(std::io::Error::other)?;
                fs::write(&temporary, content)
            })
            .and_then(|_| fs::rename(&temporary, root.join(FEED_CACHE_FILE)));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
    }

    fn fresh_releases(&self, now: u64, max_age: Duration) -> Option<Vec<ReleaseInfo>> {
        // 时钟回拨时 fetched_at 会落在未来，这种缓存不能算新鲜。
        let age = now.checked_sub(self.fetched_at)?;
        (age < max_age.as_secs() && !self.releases.is_empty()).then(|| self.releases.clone())
    }

    fn active_rate_limit(&self, now: u64) -> Option<u64> {
        // 超过上限说明本机时钟被调过，继续信它可能把 API 关很久。
        self.rate_limited_until.filter(|until| {
            *until > now && *until <= now.saturating_add(MAX_RATE_LIMIT_BACKOFF_SECS)
        })
    }
}

fn feed_platform() -> String {
    format!("{}-{}", platform_name(), architecture_name())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[derive(Clone)]
struct ReleaseSources {
    api_origin: String,
    feed_url: String,
    tag_refs_url: String,
    releases_base: String,
}

impl Default for ReleaseSources {
    fn default() -> Self {
        Self {
            api_origin: GITHUB_API_ORIGIN.into(),
            feed_url: RELEASES_API.into(),
            tag_refs_url: RELEASE_TAG_REFS.into(),
            releases_base: RELEASES_BASE.into(),
        }
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
    version: String,
    platform: String,
    arch: String,
    command: String,
    backend: Option<String>,
    #[serde(default)]
    upgrade_notice: bool,
}

impl ComponentManifest {
    fn is_node_compatible(&self) -> bool {
        // 历史 Node 包没有 backend 字段；显式声明其他后端或升级提示页的包不能执行。
        !self.upgrade_notice && matches!(self.backend.as_deref(), None | Some("node"))
    }

    fn ensure_node_compatible(&self) -> Result<()> {
        if !self.is_node_compatible() {
            bail!("component payload is incompatible with the Node application");
        }
        Ok(())
    }
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
    sources: ReleaseSources,
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
            sources: ReleaseSources::default(),
        })
    }

    pub fn components_root(&self) -> &Path {
        &self.root
    }

    pub fn installed(&self, component: Component) -> Result<Option<InstalledComponent>> {
        resolve_installed(&self.root, component)
    }

    pub async fn latest(&self, component: Component) -> Result<ReleaseInfo> {
        self.release_feed(CHECK_FEED_MAX_AGE)
            .await?
            .latest(component)
    }

    /// 取发布信息：优先复用 `max_age` 内的缓存，其次 GitHub API（含配置的镜像），
    /// 都不行时改从 Git 标签定位。三个组件共用同一份结果，一轮只花一个 API 额度。
    pub async fn release_feed(&self, max_age: Duration) -> Result<ReleaseFeed> {
        let now = unix_now();
        let mut cache = FeedCache::read(&self.root);
        if let Some(releases) = cache.fresh_releases(now, max_age) {
            return Ok(ReleaseFeed { releases });
        }
        let mut rate_limit = cache.active_rate_limit(now);
        let previous = std::mem::take(&mut cache.releases);

        let mut feed_error = None;
        let mut releases = None;
        for url in release_feed_candidates(&self.sources.feed_url) {
            let is_api = url == self.sources.feed_url;
            if is_api && rate_limit.is_some() {
                continue;
            }
            match self.fetch_releases(&url).await {
                Ok(value) => {
                    releases = Some(latest_releases(value));
                    break;
                }
                Err(error) => {
                    // 镜像的出口 IP 不是用户的，它被限流不代表用户自己的额度耗尽。
                    if is_api {
                        rate_limit = rate_limited_until(&error);
                    }
                    feed_error = Some(error);
                }
            }
        }

        let mut tags_error = None;
        if releases.is_none() {
            match self.releases_from_tags(&previous).await {
                Ok(value) => releases = Some(value),
                Err(error) => tags_error = Some(error),
            }
        }

        cache.rate_limited_until = rate_limit;
        match releases {
            Some(releases) => {
                cache.fetched_at = now;
                cache.releases = releases.clone();
                cache.write(&self.root);
                Ok(ReleaseFeed { releases })
            }
            None => {
                // 不拿旧缓存充数：列表可能早已过期，会把用户误导成“已是最新”。
                cache.releases = previous;
                cache.write(&self.root);
                if let Some(retry_at) = rate_limit {
                    return Err(RateLimited { retry_at }.into());
                }
                let error =
                    feed_error.unwrap_or_else(|| anyhow!("component release request failed"));
                Err(match tags_error {
                    Some(tags_error) => {
                        error.context(format!("release tag lookup failed: {tags_error:#}"))
                    }
                    None => error,
                })
            }
        }
    }

    async fn fetch_releases(&self, url: &str) -> Result<Vec<GitHubRelease>> {
        let mut last_error = None;
        for attempt in 0..NETWORK_ATTEMPTS {
            let result = async {
                let response = self
                    .client
                    .get(url)
                    .header(ACCEPT, "application/vnd.github+json")
                    .header(USER_AGENT, "Pisper component updater")
                    .timeout(FEED_REQUEST_TIMEOUT)
                    .send()
                    .await
                    .context("failed to request Pisper component releases")?;
                ensure_success(response)
                    .await
                    .context("Pisper component release request failed")?
                    .json::<Vec<GitHubRelease>>()
                    .await
                    .context("invalid Pisper component release response")
            }
            .await;
            match result {
                Ok(releases) => return Ok(releases),
                Err(error) if is_permanent_failure(&error) => return Err(error),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < NETWORK_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
            }
        }
        let error = last_error.unwrap_or_else(|| anyhow!("component release request failed"));
        Err(error.context(format!(
            "component release request failed after {NETWORK_ATTEMPTS} attempts"
        )))
    }

    /// 不经 API 定位版本：标签给出版本号，直链探测确认当前平台的包和签名都已上传。
    /// 这条路拿不到更新说明，所以已知的版本直接沿用 `previous` 里的完整信息。
    async fn releases_from_tags(&self, previous: &[ReleaseInfo]) -> Result<Vec<ReleaseInfo>> {
        let tags = self.fetch_release_tags().await?;
        let mut releases = Vec::new();
        for component in Component::ALL {
            for tag in stable_tags_newest_first(component, &tags)
                .into_iter()
                .take(MAX_TAG_PROBES)
            {
                let known = previous
                    .iter()
                    .find(|release| release.component == component && release.tag == tag);
                let found = match known {
                    Some(release) => Some(release.clone()),
                    None => self.probe_tag_release(component, &tag).await?,
                };
                if let Some(release) = found {
                    releases.push(release);
                    break;
                }
            }
        }
        if releases.is_empty() {
            bail!("no signed component release matches the release tags");
        }
        Ok(releases)
    }

    async fn fetch_release_tags(&self) -> Result<Vec<String>> {
        let response = self
            .client
            .get(&self.sources.tag_refs_url)
            .timeout(FEED_REQUEST_TIMEOUT)
            .send()
            .await
            .context("failed to request Pisper release tags")?;
        let mut response = ensure_success(response)
            .await
            .context("Pisper release tag request failed")?;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("Pisper release tag request was interrupted")?
        {
            if body.len() as u64 + chunk.len() as u64 > MAX_TAG_REFS_BYTES {
                bail!("Pisper release tag list exceeds the size limit");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(parse_tag_refs(&body))
    }

    async fn probe_tag_release(
        &self,
        component: Component,
        tag: &str,
    ) -> Result<Option<ReleaseInfo>> {
        let Some(version) = tag.strip_prefix(component.tag_prefix()) else {
            return Ok(None);
        };
        let archive_name = component_asset_name(component, version);
        let base = format!("{}/download/{tag}", self.sources.releases_base);
        let archive_url = format!("{base}/{archive_name}");
        let signature_url = format!("{archive_url}.sig");
        let Some(size) = self.probe_asset(&archive_url).await? else {
            return Ok(None);
        };
        if self.probe_asset(&signature_url).await?.is_none() {
            return Ok(None);
        }
        Ok(Some(ReleaseInfo {
            component,
            version: version.to_string(),
            tag: tag.to_string(),
            notes: String::new(),
            release_url: format!("{}/tag/{tag}", self.sources.releases_base),
            published_at: None,
            archive_url: archive_url.clone(),
            signature_url: signature_url.clone(),
            archive_fallback_url: archive_url,
            signature_fallback_url: signature_url,
            size,
        }))
    }

    /// 只请求一个字节确认资产存在并顺带拿到总大小；404 表示这个版本没有当前平台的包。
    async fn probe_asset(&self, url: &str) -> Result<Option<u64>> {
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/octet-stream")
            .header(RANGE, "bytes=0-0")
            .timeout(FEED_REQUEST_TIMEOUT)
            .send()
            .await
            .context("failed to probe a Pisper release asset")?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = ensure_success(response)
            .await
            .context("Pisper release asset probe failed")?;
        let size = parse_content_range(response.headers().get(CONTENT_RANGE))
            .map(|(_, _, total)| total)
            .or_else(|| response.content_length())
            .unwrap_or(0);
        Ok(Some(size))
    }

    fn rate_limit_active(&self) -> bool {
        FeedCache::read(&self.root)
            .active_rate_limit(unix_now())
            .is_some()
    }

    fn record_rate_limit(&self, retry_at: u64) {
        let mut cache = FeedCache::read(&self.root);
        cache.rate_limited_until = Some(retry_at);
        cache.write(&self.root);
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
        let version = Version::parse(&release.version).context("invalid component version")?;
        release.component.ensure_not_withdrawn(&version)?;
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
        let installed_version = version.clone();
        tokio::task::spawn_blocking(move || install_archive(&root, component, &version, &archive))
            .await
            .context("component installer task failed")??;
        self.installed(component)?
            .filter(|installed| installed.version == installed_version)
            .ok_or_else(|| anyhow!("installed component pointer was not activated"))
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
            let is_api = url.starts_with(&self.sources.api_origin);
            // 限流期间 API 地址只会再失败一次，直接走不占额度的直链。
            if is_api && self.rate_limit_active() {
                continue;
            }
            match self
                .download_with_progress(url, limit, expected_size, &mut on_progress)
                .await
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    if let Some(retry_at) = rate_limited_until(&error).filter(|_| is_api) {
                        self.record_rate_limit(retry_at);
                    }
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
        if expected_size >= MIN_PARALLEL_DOWNLOAD_BYTES && expected_size <= limit {
            if let Some(range_url) = self.resolve_range_url(url, expected_size).await? {
                match self
                    .download_in_parallel(&range_url, limit, expected_size, &mut on_progress)
                    .await
                {
                    Ok(bytes) => return Ok(bytes),
                    Err(_) => on_progress(0, expected_size),
                }
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
                Err(error) if is_permanent_failure(&error) => return Err(error),
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

    /// 探测分段下载，可用时返回重定向后的最终地址。GitHub API 的资产地址每请求一次
    /// 就占一个额度，分段都打最终地址，一个大包只花探测这一个额度。
    async fn resolve_range_url(&self, url: &str, expected_size: u64) -> Result<Option<String>> {
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/octet-stream")
            .header(RANGE, "bytes=0-0")
            .send()
            .await;
        let Ok(mut response) = response else {
            return Ok(None);
        };
        if response.status() != StatusCode::PARTIAL_CONTENT {
            // 被限流时普通下载同样会失败，直接上报，省掉一次注定失败的请求。
            return match ensure_success(response).await {
                Err(error) if rate_limited_until(&error).is_some() => Err(error),
                _ => Ok(None),
            };
        }
        if parse_content_range(response.headers().get(CONTENT_RANGE)) != Some((0, 0, expected_size))
            || response.content_length().is_some_and(|size| size != 1)
        {
            return Ok(None);
        }
        let range_url = response.url().to_string();
        let mut received = 0_u64;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    received = received.saturating_add(chunk.len() as u64);
                    if received > 1 {
                        return Ok(None);
                    }
                }
                Ok(None) => break,
                Err(_) => return Ok(None),
            }
        }
        Ok((received == 1).then_some(range_url))
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
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/octet-stream")
            .send()
            .await
            .with_context(|| format!("failed to download {url}"))?;
        let mut response = ensure_success(response)
            .await
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

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn rate_limit_retry_at(headers: &HeaderMap, now: u64) -> u64 {
    let wanted = header_u64(headers, RETRY_AFTER.as_str())
        .map(|seconds| now.saturating_add(seconds))
        .or_else(|| {
            (header_u64(headers, "x-ratelimit-remaining") == Some(0))
                .then(|| header_u64(headers, "x-ratelimit-reset"))
                .flatten()
        })
        .unwrap_or(0);
    wanted.clamp(
        now.saturating_add(MIN_RATE_LIMIT_BACKOFF_SECS),
        now.saturating_add(MAX_RATE_LIMIT_BACKOFF_SECS),
    )
}

/// 区分限流和普通失败。错误里不带响应正文（GitHub 会写入用户的出口 IP），
/// 也不带 URL（重定向后的地址含临时签名），调用方自己补原始地址。
async fn ensure_success(mut response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        let headers = response.headers().clone();
        let limited = status == StatusCode::TOO_MANY_REQUESTS
            || headers.contains_key(RETRY_AFTER)
            || header_u64(&headers, "x-ratelimit-remaining") == Some(0)
            // 次级限流可能不带上面这些响应头，只能看正文。
            || response
                .chunk()
                .await
                .ok()
                .flatten()
                .is_some_and(|body| {
                    String::from_utf8_lossy(&body)
                        .to_ascii_lowercase()
                        .contains("rate limit")
                });
        if limited {
            let retry_at = rate_limit_retry_at(&headers, unix_now());
            return Err(RateLimited { retry_at }.into());
        }
    }
    response
        .error_for_status()
        .map_err(|error| error.without_url().into())
}

/// 限流和明确的 4xx 马上重试也不会变，只会白白消耗额度。
fn is_permanent_failure(error: &anyhow::Error) -> bool {
    rate_limited_until(error).is_some()
        || error.chain().any(|cause| {
            cause
                .downcast_ref::<reqwest::Error>()
                .and_then(reqwest::Error::status)
                .is_some_and(|status| {
                    status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT
                })
        })
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
    // 这里的 url 可能是带临时签名的重定向地址，不能跟着错误进日志。
    let mut response = client
        .get(url)
        .header(ACCEPT, "application/octet-stream")
        .header(RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .map_err(reqwest::Error::without_url)
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
        .map_err(reqwest::Error::without_url)
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
    match fs::remove_file(pointer) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to deactivate component"),
    }
}

pub fn resolve_installed(root: &Path, component: Component) -> Result<Option<InstalledComponent>> {
    let pointer_path = root.join(component.name()).join("current.json");
    let pointer = match fs::read(&pointer_path) {
        Ok(value) => serde_json::from_slice::<CurrentPointer>(&value)
            .with_context(|| format!("invalid component pointer: {}", pointer_path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to read component pointer"),
    };
    let version =
        Version::parse(&pointer.version).context("invalid installed component version")?;
    // 撤回状态由本地策略判定，离线覆盖安装也能恢复；不改指针、不删组件或用户数据。
    if component.is_withdrawn(&version) {
        return Ok(None);
    }
    let version_root = root
        .join(component.name())
        .join("versions")
        .join(version.to_string());
    if !validate_layout(component, &version_root, &version)?.is_node_compatible() {
        return Ok(None);
    }
    Ok(Some(InstalledComponent {
        component,
        version,
        root: version_root,
    }))
}

pub fn component_asset_name(component: Component, version: &str) -> String {
    match component {
        Component::Desktop => format!(
            "Pisper_Desktop_{}_{}_{}.tar.gz",
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
            "Pisper_Runtime_{}_{}_{}.tar.gz",
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
    if component.is_withdrawn(&Version::parse(version).ok()?) {
        return None;
    }
    let archive_name = component_asset_name(component, version);
    let signature_name = format!("{archive_name}.sig");
    let archive = release
        .assets
        .iter()
        .find(|asset| asset.name == archive_name)?;
    let signature = release
        .assets
        .iter()
        .find(|asset| asset.name == signature_name)?;
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

/// 按组件挑出最新的可安装版本，只留这几条，缓存不必保存整份列表。
fn latest_releases(releases: Vec<GitHubRelease>) -> Vec<ReleaseInfo> {
    Component::ALL
        .into_iter()
        .filter_map(|component| {
            releases
                .iter()
                .filter_map(|release| release_info(component, release.clone()))
                .max_by(|left, right| version_of(&left.version).cmp(&version_of(&right.version)))
        })
        .collect()
}

/// 解析 Git Smart HTTP 的引用列表（pkt-line 格式），只取标签名。
fn parse_tag_refs(body: &[u8]) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = body;
    while rest.len() >= 4 {
        let Some(length) = std::str::from_utf8(&rest[..4])
            .ok()
            .and_then(|value| usize::from_str_radix(value, 16).ok())
        else {
            break;
        };
        // 长度 0 是分隔包，只占长度字段本身。
        if length == 0 {
            rest = &rest[4..];
            continue;
        }
        if length < 4 || length > rest.len() {
            break;
        }
        let line = &rest[4..length];
        rest = &rest[length..];
        // 首条引用后面用 NUL 接能力列表。
        let line = line.split(|byte| *byte == 0).next().unwrap_or_default();
        let name = std::str::from_utf8(line)
            .ok()
            .and_then(|value| value.trim_end().split_once(' '))
            .map(|(_, name)| name);
        if let Some(tag) = name.and_then(|name| name.strip_prefix("refs/tags/")) {
            if !tag.ends_with("^{}") {
                tags.push(tag.to_string());
            }
        }
    }
    tags
}

fn stable_tags_newest_first(component: Component, tags: &[String]) -> Vec<String> {
    let mut versions = tags
        .iter()
        .filter_map(|tag| {
            let version = Version::parse(tag.strip_prefix(component.tag_prefix())?).ok()?;
            (version.pre.is_empty() && !component.is_withdrawn(&version))
                .then_some((version, tag.clone()))
        })
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    versions.into_iter().map(|(_, tag)| tag).collect()
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
    if browser_url.is_empty() {
        return urls;
    }
    if browser_url != primary {
        urls.push(browser_url.to_string());
    }
    // 从标签定位到的版本主地址就是直连地址，同样要能走镜像。
    urls.extend(
        mirrors
            .iter()
            .map(|prefix| format!("{prefix}{browser_url}")),
    );
    urls
}

/// 组件发布列表的候选地址：GitHub API 优先，镜像（若配置）兜底。
fn release_feed_candidates(feed_url: &str) -> Vec<String> {
    let mut urls = vec![feed_url.to_string()];
    urls.extend(
        mirror_prefixes()
            .iter()
            .map(|prefix| format!("{prefix}{feed_url}")),
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
) -> Result<()> {
    component.ensure_not_withdrawn(version)?;
    let component_root = root.join(component.name());
    fs::create_dir_all(&component_root).context("failed to create component directory")?;
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
    fs::create_dir_all(&versions_root).context("failed to create component versions directory")?;

    let staging = versions_root.join(format!(".{}.tmp-{}", version, std::process::id()));
    let backup = versions_root.join(format!(".{}.previous-{}", version, std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).context("failed to clear stale component staging")?;
    }
    let _ = fs::remove_dir_all(&backup);
    fs::create_dir_all(&staging).context("failed to create component staging directory")?;
    let result = extract_archive(bytes, &staging)
        .and_then(|_| validate_layout(component, &staging, version))
        .and_then(|manifest| manifest.ensure_node_compatible());
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
    validate_layout(component, &destination, version)?.ensure_node_compatible()?;
    write_pointer(&component_root, version)?;
    cleanup_versions(&versions_root, version);
    Ok(())
}

fn cleanup_versions(versions_root: &Path, active: &Version) {
    let Ok(entries) = fs::read_dir(versions_root) else {
        return;
    };
    let mut versions = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let version = Version::parse(entry.file_name().to_str()?).ok()?;
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

fn validate_layout(
    component: Component,
    root: &Path,
    version: &Version,
) -> Result<ComponentManifest> {
    let entry = if component == Component::Desktop {
        root.join("dist").join(component.executable_name())
    } else {
        root.join(component.executable_name())
    };
    if !entry.is_file() {
        bail!("component entry is missing: {}", entry.display());
    }
    let manifest_path = root.join("manifest.json");
    let manifest =
        serde_json::from_slice::<ComponentManifest>(&fs::read(&manifest_path).with_context(
            || format!("component manifest is missing: {}", manifest_path.display()),
        )?)
        .context("component manifest is invalid")?;
    if manifest.version != version.to_string()
        || manifest.platform != platform_name()
        || manifest.arch != architecture_name()
        || manifest.command
            != if component == Component::Desktop {
                "dist/index.html"
            } else {
                component.executable_name()
            }
    {
        bail!("component manifest does not match the requested platform and version");
    }
    if component == Component::Runtime {
        let package = root.join("sidecar-runtime").join("package.json");
        if !package.is_file() {
            bail!("runtime payload is missing: {}", package.display());
        }
    }
    Ok(manifest)
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
            // macOS 的时钟精度可能让并行测试得到相同纳秒值，序号保证目录互不覆盖。
            static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pisper-component-updater-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
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
        desktop_archive_with_manifest(version, serde_json::json!({}))
    }

    fn desktop_archive_with_manifest(version: &str, extra: serde_json::Value) -> Vec<u8> {
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
        let mut manifest = serde_json::json!({
            "version": version,
            "platform": super::platform_name(),
            "arch": super::architecture_name(),
            "command": "dist/index.html",
        });
        manifest
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let manifest = serde_json::to_vec(&manifest).unwrap();
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
        assert!(desktop.starts_with("Pisper_Desktop_1.2.3_"));
        assert!(desktop.ends_with(".tar.gz"));
        let tui = component_asset_name(Component::Tui, "1.2.3");
        assert!(tui.starts_with("Pisper_TUI_Component_1.2.3_"));
        assert!(tui.ends_with(".tar.gz"));
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

    // 直接重建旧版本已落盘的组件，避免用修复后的安装器生成无效历史状态。
    fn seed_component(root: &Path, component: Component, version: &str) -> PathBuf {
        let payload = root.join(component.name()).join("versions").join(version);
        let entry = if component == Component::Desktop {
            payload.join("dist/index.html")
        } else {
            payload.join(component.executable_name())
        };
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(entry, b"old component payload").unwrap();
        if component == Component::Runtime {
            fs::create_dir_all(payload.join("sidecar-runtime")).unwrap();
            fs::write(payload.join("sidecar-runtime/package.json"), b"{}").unwrap();
        }
        fs::write(
            payload.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": version,
                "platform": super::platform_name(),
                "arch": super::architecture_name(),
                "command": if component == Component::Desktop {
                    "dist/index.html"
                } else {
                    component.executable_name()
                },
            }))
            .unwrap(),
        )
        .unwrap();
        super::write_pointer(
            &root.join(component.name()),
            &Version::parse(version).unwrap(),
        )
        .unwrap();
        payload
    }

    fn file_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        fn visit(
            root: &Path,
            path: &Path,
            files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
        ) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut files = std::collections::BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    #[test]
    fn withdrawn_components_are_ignored_on_every_start_without_changing_user_data() {
        let directory = TestDirectory::new();
        let root = directory.0.join("components");
        let agent = directory.0.join("agent/sessions");
        fs::create_dir_all(&agent).unwrap();
        fs::write(agent.join("session.jsonl"), b"fixture session\n").unwrap();
        fs::write(directory.0.join("preferences.json"), b"fixture preferences").unwrap();
        for version in ["0.6.1", "0.6.1-beta.1", "0.6.1+build.2"] {
            for component in Component::ALL {
                seed_component(&root, component, version);
            }
            let before = file_snapshot(&directory.0);
            for _ in 0..2 {
                for component in Component::ALL {
                    assert!(
                        resolve_installed(&root, component).unwrap().is_none(),
                        "{} {version}",
                        component.name()
                    );
                }
                assert_eq!(file_snapshot(&directory.0), before);
            }
        }
    }

    #[test]
    fn node_components_remain_loadable_with_legacy_or_explicit_backend_manifests() {
        let directory = TestDirectory::new();
        for component in Component::ALL {
            let payload = seed_component(&directory.0, component, "0.5.82");
            assert!(resolve_installed(&directory.0, component)
                .unwrap()
                .is_some());
            let path = payload.join("manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            manifest["backend"] = "node".into();
            fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            assert!(resolve_installed(&directory.0, component)
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn incompatible_backend_and_upgrade_notice_payloads_are_not_loaded() {
        let directory = TestDirectory::new();
        for component in Component::ALL {
            for extra in [
                serde_json::json!({"backend": "rust"}),
                serde_json::json!({"backend": "upgrade-notice", "upgradeNotice": true}),
                serde_json::json!({"upgradeNotice": true}),
                serde_json::json!({"backend": "future-backend"}),
            ] {
                let payload = seed_component(&directory.0, component, "0.5.82");
                let path = payload.join("manifest.json");
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                manifest
                    .as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
                fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
                let before = file_snapshot(&directory.0);
                assert!(resolve_installed(&directory.0, component)
                    .unwrap()
                    .is_none());
                assert_eq!(file_snapshot(&directory.0), before);
            }
        }
    }

    fn signed_release(component: Component, version: &str) -> GitHubRelease {
        let archive = component_asset_name(component, version);
        GitHubRelease {
            tag_name: format!("{}{version}", component.tag_prefix()),
            html_url: "https://example.test/release".into(),
            body: None,
            published_at: None,
            draft: false,
            prerelease: false,
            assets: [archive.clone(), format!("{archive}.sig")]
                .into_iter()
                .map(|name| GitHubAsset {
                    name,
                    api_url: "https://example.test/asset".into(),
                    browser_download_url: "https://example.test/asset".into(),
                    size: 42,
                })
                .collect(),
        }
    }

    #[test]
    fn cached_release_feeds_cannot_offer_withdrawn_components() {
        for component in Component::ALL {
            let selected = ["0.5.81", "0.6.1", "0.5.82", "0.6.1+build.2"]
                .into_iter()
                .filter_map(|version| release_info(component, signed_release(component, version)))
                .max_by_key(|release| Version::parse(&release.version).unwrap())
                .unwrap();
            assert_eq!(selected.version, "0.5.82");
        }
    }

    #[test]
    fn withdrawal_is_scoped_to_published_versions_not_the_minor_series() {
        let directory = TestDirectory::new();
        for component in Component::ALL {
            let (withdrawn, future) = match component {
                Component::Desktop => (
                    vec!["0.6.0", "0.6.1"],
                    vec!["0.6.2", "0.6.3", "0.6.4", "0.6.99", "1.0.0"],
                ),
                Component::Tui | Component::Runtime => (
                    vec!["0.6.1", "0.6.2", "0.6.3"],
                    vec!["0.6.0", "0.6.4", "0.6.99", "1.0.0"],
                ),
            };
            for version in withdrawn {
                seed_component(&directory.0, component, version);
                assert!(resolve_installed(&directory.0, component)
                    .unwrap()
                    .is_none());
                assert!(release_info(component, signed_release(component, version)).is_none());
            }
            for version in future {
                seed_component(&directory.0, component, version);
                assert!(resolve_installed(&directory.0, component)
                    .unwrap()
                    .is_some());
                assert!(release_info(component, signed_release(component, version)).is_some());
            }
        }
        install_archive(
            &directory.0,
            Component::Desktop,
            &Version::parse("0.6.4").unwrap(),
            &desktop_archive("0.6.4"),
        )
        .unwrap();
        assert_eq!(
            resolve_installed(&directory.0, Component::Desktop)
                .unwrap()
                .unwrap()
                .version,
            Version::parse("0.6.4").unwrap()
        );
    }

    #[test]
    fn withdrawn_archives_cannot_replace_an_active_node_component() {
        let directory = TestDirectory::new();
        for component in Component::ALL {
            seed_component(&directory.0, component, "0.5.81");
            let before = file_snapshot(&directory.0);
            let error = install_archive(
                &directory.0,
                component,
                &Version::parse("0.6.1").unwrap(),
                &desktop_archive("0.6.1"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("withdrawn"));
            assert_eq!(file_snapshot(&directory.0), before);
        }
    }

    #[test]
    fn direct_install_rejects_withdrawn_versions_before_any_download() {
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper rollback test",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for component in Component::ALL {
            let mut release = release_info(component, signed_release(component, "0.5.82")).unwrap();
            release.version = "0.6.1".into();
            // 非 URL 使回归时也不会访问外网；正确实现应在下载器之前拒绝该版本。
            release.archive_url = "invalid-url".into();
            release.archive_fallback_url.clear();
            let mut progressed = false;
            let error = runtime
                .block_on(updater.install_with_progress(&release, |_, _| progressed = true))
                .unwrap_err();
            assert!(error.to_string().contains("withdrawn"), "{error:#}");
            assert!(!progressed);
            assert!(file_snapshot(&directory.0).is_empty());
        }
    }

    #[test]
    fn a_node_update_can_replace_a_withdrawn_pointer_without_deleting_user_data() {
        let directory = TestDirectory::new();
        let root = directory.0.join("components");
        let previous = seed_component(&root, Component::Desktop, "0.6.1");
        fs::write(directory.0.join("preferences.json"), b"fixture preferences").unwrap();
        let previous_files = file_snapshot(&previous);
        install_archive(
            &root,
            Component::Desktop,
            &Version::parse("0.5.82").unwrap(),
            &desktop_archive("0.5.82"),
        )
        .unwrap();
        let installed = resolve_installed(&root, Component::Desktop)
            .unwrap()
            .unwrap();
        assert_eq!(installed.version, Version::parse("0.5.82").unwrap());
        assert_eq!(file_snapshot(&previous), previous_files);
        assert_eq!(
            fs::read(directory.0.join("preferences.json")).unwrap(),
            b"fixture preferences"
        );
    }

    #[test]
    fn incompatible_archive_cannot_activate_over_an_existing_node_component() {
        let directory = TestDirectory::new();
        let previous = seed_component(&directory.0, Component::Desktop, "0.5.81");
        let before = file_snapshot(&previous);
        for extra in [
            serde_json::json!({"backend": "rust"}),
            serde_json::json!({"upgradeNotice": true}),
        ] {
            let error = install_archive(
                &directory.0,
                Component::Desktop,
                &Version::parse("0.6.4").unwrap(),
                &desktop_archive_with_manifest("0.6.4", extra),
            )
            .unwrap_err();
            assert!(error.to_string().contains("incompatible"));
            assert_eq!(
                resolve_installed(&directory.0, Component::Desktop)
                    .unwrap()
                    .unwrap()
                    .version,
                Version::parse("0.5.81").unwrap()
            );
            assert_eq!(file_snapshot(&previous), before);
            assert!(!directory.0.join("desktop/versions/0.6.4").exists());
        }
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
        // 直连地址与主地址相同时去重，但仍要能走镜像；为空时没有可中转的地址。
        assert_eq!(
            super::download_candidates_with_mirrors("https://same", "https://same", &mirrors),
            vec![
                "https://same".to_string(),
                "https://gh-proxy.com/https://same".to_string(),
            ]
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

    // 按路径应答的本地服务器，记下每个请求的路径，用来断言到底发了多少次请求。
    struct RecordingServer {
        address: std::net::SocketAddr,
        requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecordingServer {
        fn start(respond: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let respond = std::sync::Arc::new(respond);
            // 线程随测试进程退出，监听线程不需要回收。
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
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

        fn updater(&self, root: &Path) -> super::ComponentUpdater {
            let mut updater = super::ComponentUpdater::new(
                root.to_path_buf(),
                include_str!("../../../src-tauri/updater.pubkey"),
                "Pisper rate limit test",
            )
            .unwrap();
            let base = format!("http://{}", self.address);
            updater.sources = super::ReleaseSources {
                api_origin: format!("{base}/api/"),
                feed_url: format!("{base}/api/releases"),
                tag_refs_url: format!("{base}/info/refs"),
                releases_base: format!("{base}/releases"),
            };
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

    fn http_response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn rate_limited_response() -> Vec<u8> {
        http_response(
            "403 Forbidden",
            &format!(
                "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: {}\r\n",
                super::unix_now() + 600
            ),
            "{\"message\":\"API rate limit exceeded for 203.0.113.9.\"}",
        )
    }

    fn pkt_line(value: &str) -> String {
        format!("{:04x}{value}", value.len() + 4)
    }

    fn tag_refs(tags: &[&str]) -> String {
        let mut body = pkt_line("# service=git-upload-pack\n");
        body.push_str("0000");
        body.push_str(&pkt_line(&format!(
            "{} HEAD\0multi_ack symref=HEAD:refs/heads/release\n",
            "a".repeat(40)
        )));
        for tag in tags {
            body.push_str(&pkt_line(&format!("{} refs/tags/{tag}\n", "b".repeat(40))));
            body.push_str(&pkt_line(&format!(
                "{} refs/tags/{tag}^{{}}\n",
                "c".repeat(40)
            )));
        }
        body.push_str("0000");
        body
    }

    fn feed_json(address: std::net::SocketAddr, version: &str) -> String {
        let release = |component: Component| {
            let archive = component_asset_name(component, version);
            let asset = |name: String| {
                serde_json::json!({
                    "name": name,
                    "url": format!("http://{address}/api/assets/{name}"),
                    "browser_download_url": format!("http://{address}/direct/{name}"),
                    "size": 42,
                })
            };
            serde_json::json!({
                "tag_name": format!("{}{version}", component.tag_prefix()),
                "html_url": "https://example.test/release",
                "body": "release notes",
                "published_at": null,
                "draft": false,
                "prerelease": false,
                "assets": [asset(archive.clone()), asset(format!("{archive}.sig"))],
            })
        };
        serde_json::to_string(&Component::ALL.map(release)).unwrap()
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_check_round_requests_the_feed_once_and_install_reuses_it() {
        let server = std::sync::Arc::new(std::sync::OnceLock::new());
        let address = server.clone();
        let started = RecordingServer::start(move |_| {
            let address: &std::net::SocketAddr = address.get().unwrap();
            http_response("200 OK", "", &feed_json(*address, "1.2.3"))
        });
        server.set(started.address).unwrap();
        let directory = TestDirectory::new();
        let updater = started.updater(&directory.0);

        for component in Component::ALL {
            assert_eq!(run(updater.latest(component)).unwrap().version, "1.2.3");
        }
        // 桌面端每条命令都新建更新器，缓存必须跨实例生效。
        let next_command = started.updater(&directory.0);
        let feed = run(next_command.release_feed(super::INSTALL_FEED_MAX_AGE)).unwrap();
        assert_eq!(
            feed.latest(Component::Runtime).unwrap().notes,
            "release notes"
        );
        assert_eq!(started.count("/api/releases"), 1);

        // 缓存过期后才会重新请求。
        let mut cache = super::FeedCache::read(&directory.0);
        cache.fetched_at -= super::CHECK_FEED_MAX_AGE.as_secs() + 1;
        cache.write(&directory.0);
        run(updater.latest(Component::Desktop)).unwrap();
        assert_eq!(started.count("/api/releases"), 2);
    }

    #[test]
    fn a_rate_limited_feed_falls_back_to_tags_without_more_api_requests() {
        let newest = component_asset_name(Component::Runtime, "2.0.0");
        let older = component_asset_name(Component::Runtime, "1.9.0");
        let server = RecordingServer::start(move |path| {
            if path.starts_with("/api/") {
                return rate_limited_response();
            }
            if path == "/info/refs" {
                return http_response(
                    "200 OK",
                    "",
                    &tag_refs(&[
                        "app-v9.9.9",
                        "runtime-v1.9.0",
                        "runtime-v2.0.0",
                        "runtime-v3.0.0-beta.1",
                        "runtime-v0.6.2",
                    ]),
                );
            }
            // 2.0.0 的包已上传但签名还没传完：不能选它，要退回 1.9.0。
            if path.ends_with(&newest) {
                return http_response(
                    "206 Partial Content",
                    "Content-Range: bytes 0-0/1000\r\n",
                    "x",
                );
            }
            if path.ends_with(&older) {
                return http_response(
                    "206 Partial Content",
                    "Content-Range: bytes 0-0/900\r\n",
                    "x",
                );
            }
            if path.ends_with(&format!("{older}.sig")) {
                return http_response(
                    "206 Partial Content",
                    "Content-Range: bytes 0-0/100\r\n",
                    "x",
                );
            }
            http_response("404 Not Found", "", "")
        });
        let directory = TestDirectory::new();
        let updater = server.updater(&directory.0);

        let feed = run(updater.release_feed(super::CHECK_FEED_MAX_AGE)).unwrap();
        let release = feed.latest(Component::Runtime).unwrap();
        assert_eq!(release.version, "1.9.0");
        assert_eq!(release.size, 900);
        assert!(release
            .archive_url
            .contains("/releases/download/runtime-v1.9.0/"));
        assert!(feed.latest(Component::Desktop).is_err());
        assert_eq!(server.count("/api/"), 1);

        // 恢复时间之前绕开 API，即使缓存已过期或重新启动。
        let mut cache = super::FeedCache::read(&directory.0);
        cache.fetched_at = 0;
        cache.write(&directory.0);
        let restarted = server.updater(&directory.0);
        run(restarted.release_feed(super::CHECK_FEED_MAX_AGE)).unwrap();
        assert_eq!(server.count("/api/"), 1);
        assert_eq!(server.count("/info/refs"), 2);
    }

    #[test]
    fn tag_fallback_keeps_notes_for_versions_it_already_knows() {
        let directory = TestDirectory::new();
        let server = RecordingServer::start(|path| {
            if path == "/info/refs" {
                return http_response("200 OK", "", &tag_refs(&["runtime-v1.2.3"]));
            }
            rate_limited_response()
        });
        let mut known = release_info(
            Component::Runtime,
            signed_release(Component::Runtime, "1.2.3"),
        )
        .unwrap();
        known.notes = "known notes".into();
        let mut cache = super::FeedCache::empty();
        cache.releases = vec![known];
        cache.write(&directory.0);

        let updater = server.updater(&directory.0);
        let feed = run(updater.release_feed(super::CHECK_FEED_MAX_AGE)).unwrap();
        assert_eq!(
            feed.latest(Component::Runtime).unwrap().notes,
            "known notes"
        );
        // 已知版本不用再探测资产。
        assert_eq!(server.count("/releases/"), 0);
    }

    #[test]
    fn exhausted_sources_report_a_rate_limit_without_leaking_the_client_ip() {
        let server = RecordingServer::start(|path| {
            if path == "/info/refs" {
                return http_response("503 Service Unavailable", "", "");
            }
            rate_limited_response()
        });
        let directory = TestDirectory::new();
        let updater = server.updater(&directory.0);

        let error = run(updater.release_feed(super::CHECK_FEED_MAX_AGE)).unwrap_err();
        let retry_at = super::rate_limited_until(&error).unwrap();
        let now = super::unix_now();
        assert!(retry_at > now + 500 && retry_at <= now + 600);
        assert!(!format!("{error:#}").contains("203.0.113.9"));
        assert_eq!(server.count("/api/"), 1);

        // 第二次检查不再碰 API，并且仍能告诉用户恢复时间。
        let error = run(updater.release_feed(super::CHECK_FEED_MAX_AGE)).unwrap_err();
        assert_eq!(super::rate_limited_until(&error), Some(retry_at));
        assert_eq!(server.count("/api/"), 1);
    }

    #[test]
    fn stale_feed_is_not_reused_when_every_source_fails() {
        let server = RecordingServer::start(|_| http_response("404 Not Found", "", ""));
        let directory = TestDirectory::new();
        let mut cache = super::FeedCache::empty();
        cache.releases =
            vec![release_info(Component::Tui, signed_release(Component::Tui, "1.0.0")).unwrap()];
        cache.fetched_at = 1;
        cache.write(&directory.0);

        let updater = server.updater(&directory.0);
        assert!(run(updater.release_feed(super::CHECK_FEED_MAX_AGE)).is_err());
        // 明确的 404 不重试。
        assert_eq!(server.count("/api/"), 1);
        assert_eq!(super::FeedCache::read(&directory.0).releases.len(), 1);
    }

    #[test]
    fn transient_feed_errors_still_retry() {
        let server = RecordingServer::start(|path| {
            if path == "/info/refs" {
                return http_response("404 Not Found", "", "");
            }
            http_response("502 Bad Gateway", "", "")
        });
        let directory = TestDirectory::new();
        let updater = server.updater(&directory.0);
        assert!(run(updater.release_feed(super::CHECK_FEED_MAX_AGE)).is_err());
        assert_eq!(server.count("/api/"), super::NETWORK_ATTEMPTS);
    }

    #[test]
    fn a_rate_limited_download_moves_on_and_skips_the_api_afterwards() {
        let server = RecordingServer::start(|path| {
            if path.starts_with("/api/") {
                return rate_limited_response();
            }
            http_response("200 OK", "", "abcdef")
        });
        let directory = TestDirectory::new();
        let updater = server.updater(&directory.0);
        let base = format!("http://{}", server.address);
        let candidates = [format!("{base}/api/asset"), format!("{base}/direct/asset")];

        for _ in 0..2 {
            let bytes =
                run(updater.download_candidates_with_progress(&candidates, 1024, 6, |_, _| {}))
                    .unwrap();
            assert_eq!(bytes, b"abcdef");
        }
        // 第一次只试一下 API 就转直链，第二次连试都不试。
        assert_eq!(server.count("/api/"), 1);
        assert_eq!(server.count("/direct/"), 2);
    }

    #[test]
    fn parallel_segments_go_to_the_redirect_target_not_the_api() {
        let size = super::MIN_PARALLEL_DOWNLOAD_BYTES as usize;
        let payload = std::sync::Arc::new(vec![7_u8; size]);
        let ranges = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_ranges = ranges.clone();
        let server_payload = payload.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let ranges = server_ranges.clone();
                let payload = server_payload.clone();
                thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                        let count = stream.read(&mut chunk).unwrap();
                        request.extend_from_slice(&chunk[..count]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.split_whitespace().nth(1).unwrap().to_string();
                    let range = request.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("range").then_some(value.trim())
                    });
                    let (start, end) = range
                        .and_then(|value| value.strip_prefix("bytes="))
                        .and_then(|value| value.split_once('-'))
                        .map(|(start, end)| {
                            (start.parse::<u64>().unwrap(), end.parse::<u64>().unwrap())
                        })
                        .unwrap();
                    ranges.lock().unwrap().push((path.clone(), start));
                    if path.starts_with("/api/") {
                        stream
                            .write_all(
                                b"HTTP/1.1 302 Found\r\nLocation: /cdn/asset\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .unwrap();
                        return;
                    }
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                            end - start + 1,
                            payload.len(),
                        )
                        .as_bytes(),
                    );
                    let _ = stream.write_all(&payload[start as usize..=end as usize]);
                });
            }
        });
        let directory = TestDirectory::new();
        let updater = super::ComponentUpdater::new(
            directory.0.clone(),
            include_str!("../../../src-tauri/updater.pubkey"),
            "Pisper redirect test",
        )
        .unwrap();
        let bytes = run(updater.download_with_progress(
            &format!("http://{address}/api/asset"),
            size as u64,
            size as u64,
            |_, _| {},
        ))
        .unwrap();

        assert_eq!(bytes, *payload);
        let ranges = ranges.lock().unwrap();
        assert_eq!(
            ranges
                .iter()
                .filter(|(path, _)| path.starts_with("/api/"))
                .count(),
            1
        );
        assert!(
            ranges
                .iter()
                .filter(|(path, _)| path == "/cdn/asset")
                .count()
                >= 3
        );
    }

    #[test]
    fn rate_limit_backoff_is_bounded() {
        let now = 1_000_000;
        let headers = |pairs: &[(&'static str, String)]| {
            let mut map = reqwest::header::HeaderMap::new();
            for (name, value) in pairs {
                map.insert(*name, value.parse().unwrap());
            }
            map
        };
        let reset = |value: u64| {
            headers(&[
                ("x-ratelimit-remaining", "0".into()),
                ("x-ratelimit-reset", value.to_string()),
            ])
        };
        assert_eq!(
            super::rate_limit_retry_at(&reset(now + 600), now),
            now + 600
        );
        // 响应头或本机时钟异常时收敛到上下限。
        assert_eq!(super::rate_limit_retry_at(&reset(now - 5), now), now + 60);
        assert_eq!(
            super::rate_limit_retry_at(&reset(now + 99_999), now),
            now + 3600
        );
        assert_eq!(
            super::rate_limit_retry_at(&headers(&[("retry-after", "120".into())]), now),
            now + 120
        );
        assert_eq!(
            super::rate_limit_retry_at(&headers(&[("retry-after", "soon".into())]), now),
            now + 60
        );

        // 时钟被调回后，超出上限的旧记录不再生效。
        let mut cache = super::FeedCache::empty();
        cache.rate_limited_until = Some(now + 3601);
        assert_eq!(cache.active_rate_limit(now), None);
        cache.rate_limited_until = Some(now + 100);
        assert_eq!(cache.active_rate_limit(now), Some(now + 100));
        assert_eq!(cache.active_rate_limit(now + 100), None);
    }

    #[test]
    fn broken_or_foreign_feed_caches_are_ignored() {
        let directory = TestDirectory::new();
        let path = directory.0.join(super::FEED_CACHE_FILE);
        fs::write(&path, b"{not json").unwrap();
        assert!(super::FeedCache::read(&directory.0).releases.is_empty());

        let mut foreign = super::FeedCache::empty();
        foreign.releases =
            vec![release_info(Component::Tui, signed_release(Component::Tui, "1.0.0")).unwrap()];
        foreign.platform = "other-platform".into();
        foreign.write(&directory.0);
        assert!(super::FeedCache::read(&directory.0).releases.is_empty());

        // 写缓存时还有效、读取时已被撤回的版本不能再提供。
        let mut withdrawn = super::FeedCache::empty();
        let mut release =
            release_info(Component::Tui, signed_release(Component::Tui, "1.0.0")).unwrap();
        release.version = "0.6.2".into();
        withdrawn.releases = vec![release];
        withdrawn.write(&directory.0);
        assert!(super::FeedCache::read(&directory.0).releases.is_empty());
    }

    #[test]
    fn tag_refs_are_parsed_and_filtered_per_component() {
        let tags = super::parse_tag_refs(
            tag_refs(&[
                "v1.2.3",
                "tui-v0.9.0",
                "runtime-v2.0.0",
                "app-v0.1.0",
                "not a tag",
            ])
            .as_bytes(),
        );
        assert_eq!(
            tags,
            [
                "v1.2.3",
                "tui-v0.9.0",
                "runtime-v2.0.0",
                "app-v0.1.0",
                "not a tag"
            ]
        );
        assert!(super::parse_tag_refs(b"zzzz").is_empty());
        assert!(super::parse_tag_refs(b"ffff0000").is_empty());

        let tags = [
            "v0.6.1",
            "v0.6.4",
            "v0.10.0",
            "v0.9.0",
            "v1.0.0-beta.1",
            "tui-v5.0.0",
            "vnext",
        ]
        .map(String::from);
        assert_eq!(
            super::stable_tags_newest_first(Component::Desktop, &tags),
            ["v0.10.0", "v0.9.0", "v0.6.4"]
        );
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
