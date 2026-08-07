use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::read::GzDecoder;
use fs2::FileExt as _;
use minisign_verify::{PublicKey, Signature};
use reqwest::header::{ACCEPT, USER_AGENT};
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    Tui,
    Runtime,
}

impl Component {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tui" => Ok(Self::Tui),
            "runtime" => Ok(Self::Runtime),
            _ => bail!("unsupported Pisper component: {value}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Runtime => "runtime",
        }
    }

    fn tag_prefix(self) -> &'static str {
        match self {
            Self::Tui => "tui-v",
            Self::Runtime => "runtime-v",
        }
    }

    fn executable_name(self) -> &'static str {
        match (self, cfg!(windows)) {
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
    pub size: u64,
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
}

#[derive(Clone, Debug)]
pub struct InstalledComponent {
    pub component: Component,
    pub version: Version,
    pub root: PathBuf,
}

impl InstalledComponent {
    pub fn executable(&self) -> PathBuf {
        self.root.join(self.component.executable_name())
    }

    pub fn runtime_root(&self) -> Option<PathBuf> {
        (self.component == Component::Runtime).then(|| self.root.join("sidecar-runtime"))
    }
}

#[derive(Clone, Deserialize)]
struct GitHubAsset {
    name: String,
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
        let releases = self
            .client
            .get(RELEASES_API)
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, "Pisper component updater")
            .send()
            .await
            .context("failed to request Pisper component releases")?
            .error_for_status()
            .context("Pisper component release request failed")?
            .json::<Vec<GitHubRelease>>()
            .await
            .context("invalid Pisper component release response")?;

        releases
            .into_iter()
            .filter_map(|release| release_info(component, release))
            .max_by(|left, right| version_of(&left.version).cmp(&version_of(&right.version)))
            .ok_or_else(|| anyhow!("no signed {} release is available", component.name()))
    }

    pub async fn install(&self, release: &ReleaseInfo) -> Result<InstalledComponent> {
        let archive = self
            .download(&release.archive_url, MAX_ARCHIVE_BYTES)
            .await?;
        let signature = self
            .download(&release.signature_url, MAX_SIGNATURE_BYTES)
            .await?;
        verify_archive(&self.public_key, &archive, &signature)?;

        let root = self.root.clone();
        let component = release.component;
        let version = Version::parse(&release.version).context("invalid component version")?;
        let installed_version = version.clone();
        tokio::task::spawn_blocking(move || install_archive(&root, component, &version, &archive))
            .await
            .context("component installer task failed")??;
        self.installed(component)?
            .filter(|installed| installed.version == installed_version)
            .ok_or_else(|| anyhow!("installed component pointer was not activated"))
    }

    async fn download(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let response = self
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
        let bytes = response
            .bytes()
            .await
            .context("component download was interrupted")?;
        if bytes.len() as u64 > limit {
            bail!("component download exceeds the size limit");
        }
        Ok(bytes.to_vec())
    }
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
    let version_root = root
        .join(component.name())
        .join("versions")
        .join(version.to_string());
    validate_layout(component, &version_root, &version)?;
    Ok(Some(InstalledComponent {
        component,
        version,
        root: version_root,
    }))
}

pub fn component_asset_name(component: Component, version: &str) -> String {
    match component {
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
    Version::parse(version).ok()?;
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
        archive_url: archive.browser_download_url.clone(),
        signature_url: signature.browser_download_url.clone(),
        size: archive.size,
    })
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
        .and_then(|_| validate_layout(component, &staging, version));
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
    validate_layout(component, &destination, version)?;
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

fn validate_layout(component: Component, root: &Path, version: &Version) -> Result<()> {
    let executable = root.join(component.executable_name());
    if !executable.is_file() {
        bail!("component executable is missing: {}", executable.display());
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
        || manifest.command != component.executable_name()
    {
        bail!("component manifest does not match the requested platform and version");
    }
    if component == Component::Runtime {
        let package = root.join("sidecar-runtime").join("package.json");
        if !package.is_file() {
            bail!("runtime payload is missing: {}", package.display());
        }
    }
    Ok(())
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
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pisper-component-updater-{}-{nonce}",
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
    fn component_assets_are_platform_specific() {
        let name = component_asset_name(Component::Tui, "1.2.3");
        assert!(name.starts_with("Pisper_TUI_Component_1.2.3_"));
        assert!(name.ends_with(".tar.gz"));
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
                    browser_download_url: "https://example.test/archive".into(),
                    size: 42,
                },
                GitHubAsset {
                    name: format!("{archive}.sig"),
                    browser_download_url: "https://example.test/signature".into(),
                    size: 512,
                },
            ],
        };
        let info = release_info(Component::Runtime, release).unwrap();
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.size, 42);
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
