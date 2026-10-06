//! Port of upstream `experimental/plugins/package.ts`.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::coding_agent::experimental::radius_relay::is_connection_id;

/// Upstream `PLUGIN_PACKAGE_PROFILE_VERSION`.
pub const PLUGIN_PACKAGE_PROFILE_VERSION: u64 = 1;
/// Upstream `DEFAULT_PLUGIN_FACETS` (`{ session, tui }`).
pub const DEFAULT_PLUGIN_FACETS_SESSION: &str = "src/session.ts";
/// Upstream `DEFAULT_PLUGIN_FACETS` tui entry.
pub const DEFAULT_PLUGIN_FACETS_TUI: &str = "src/tui.ts";
/// Upstream `FACET_BUNDLE_MANIFEST_FILE` (`@earendil-works/chord/node`).
pub const FACET_BUNDLE_MANIFEST_FILE: &str = "chord-facets.json";

/// Upstream `normalizePluginPackagePaths`: resolve every path, reject empty
/// entries and duplicates (exact upstream error strings).
pub fn normalize_plugin_package_paths(package_paths: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::with_capacity(package_paths.len());
    for package_path in package_paths {
        if package_path.is_empty() {
            return Err("Plugin package path must not be empty".to_string());
        }
        let resolved = crate::coding_agent::utils::paths::resolve_path_auto_base(package_path)
            .map_err(|error| error.to_string())?;
        normalized.push(resolved);
    }
    let mut seen = std::collections::HashSet::new();
    for path in &normalized {
        if !seen.insert(path.clone()) {
            return Err("Plugin package paths must be unique".to_string());
        }
    }
    Ok(normalized)
}

/// Upstream `sessionPluginProfilePath`:
/// `<dir>/session-plugin-packages-<serverId>-<sha256(sessionPath)[0..24]>.json`.
pub fn session_plugin_profile_path(
    directory: &str,
    server_id: &str,
    session_path: &str,
) -> PathBuf {
    let hash = sha256_hex(session_path.as_bytes())[..24].to_string();
    Path::new(directory).join(format!("session-plugin-packages-{server_id}-{hash}.json"))
}

/// Upstream `pluginBuildDirectoryName`:
/// `<label>-<sha256(packagePath)[0..12]>` where the label is the sanitized
/// base name with a `-plugin` suffix (existing suffixes are kept).
pub fn plugin_build_directory_name(package_path: &str) -> String {
    let package_directory = if Path::new(package_path)
        .file_name()
        .is_some_and(|name| name == "package.json")
    {
        Path::new(package_path)
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_else(|| package_path.to_string())
    } else {
        package_path.to_string()
    };
    let base = sanitize_build_label(
        &Path::new(&package_directory)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
    );
    let label = if base.ends_with("-plugin") {
        base
    } else {
        format!("{base}-plugin")
    };
    let hash = &sha256_hex(package_path.as_bytes())[..12];
    format!("{label}-{hash}")
}

fn sanitize_build_label(base: &str) -> String {
    let mut sanitized = String::with_capacity(base.len());
    for character in base.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            sanitized.push(character);
        } else {
            sanitized.push('-');
        }
    }
    if sanitized.is_empty() {
        "plugin".to_string()
    } else {
        sanitized
    }
}

fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Upstream `restoreServerPluginPackageProfile`: persist an explicit plugin
/// package selection or restore it for a later server generation.
pub fn restore_server_plugin_package_profile(
    directory: &str,
    server_id: &str,
    configured_package_paths: Option<&[String]>,
) -> Result<Vec<String>, String> {
    let path = Path::new(directory).join(format!("plugin-packages-{server_id}.json"));
    let Some(configured) = configured_package_paths else {
        return Ok(read_plugin_package_profile(&path, false, None)?.unwrap_or_default());
    };
    let package_paths = normalize_plugin_package_paths(configured)?;
    if package_paths.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    } else {
        write_plugin_package_profile(&path, &package_paths, None)?;
    }
    Ok(package_paths)
}

/// Upstream `readSessionPluginPackageProfile`.
pub fn read_session_plugin_package_profile(
    directory: &str,
    server_id: &str,
    session_path: &str,
) -> Result<Option<Vec<String>>, String> {
    read_plugin_package_profile(
        &session_plugin_profile_path(directory, server_id, session_path),
        true,
        Some(session_path),
    )
}

/// Upstream `removeSessionPluginPackageProfile`.
pub fn remove_session_plugin_package_profile(
    directory: &str,
    server_id: &str,
    session_path: &str,
) -> Result<(), String> {
    let path = session_plugin_profile_path(directory, server_id, session_path);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Upstream `writeSessionPluginPackageProfile`.
pub fn write_session_plugin_package_profile(
    directory: &str,
    server_id: &str,
    session_path: &str,
    package_paths: &[String],
) -> Result<(), String> {
    let normalized = normalize_plugin_package_paths(package_paths)?;
    write_plugin_package_profile(
        &session_plugin_profile_path(directory, server_id, session_path),
        &normalized,
        Some(session_path),
    )
}

/// Upstream `readPluginPackageProfile`: validate the on-disk document and
/// normalize its paths. `allow_empty` distinguishes server profiles from
/// session profiles; `session_path` pins the document to one session.
pub fn read_plugin_package_profile(
    path: &Path,
    allow_empty: bool,
    session_path: Option<&str>,
) -> Result<Option<Vec<String>>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // Upstream wraps every non-ENOENT read failure (and JSON parse
        // failures) in the same "Could not read ..." error.
        Err(_) => {
            return Err(format!(
                "Could not read experimental plugin package profile {}",
                path.display()
            ));
        }
    };
    let parsed: Value = serde_json::from_str(&text).map_err(|_| {
        format!(
            "Could not read experimental plugin package profile {}",
            path.display()
        )
    })?;
    let invalid = || {
        format!(
            "Invalid experimental plugin package profile {}",
            path.display()
        )
    };
    let Some(object) = parsed.as_object() else {
        return Err(invalid());
    };
    for key in object.keys() {
        let allowed = match key.as_str() {
            "version" | "packagePaths" => true,
            "sessionPath" => session_path.is_some(),
            _ => false,
        };
        if !allowed {
            return Err(invalid());
        }
    }
    if object.get("version") != Some(&json!(PLUGIN_PACKAGE_PROFILE_VERSION)) {
        return Err(invalid());
    }
    match session_path {
        None => {
            if object.contains_key("sessionPath") {
                return Err(invalid());
            }
        }
        Some(session_path) => {
            if object.get("sessionPath") != Some(&json!(session_path)) {
                return Err(invalid());
            }
        }
    }
    let Some(package_paths) = object.get("packagePaths").and_then(Value::as_array) else {
        return Err(invalid());
    };
    if !allow_empty && package_paths.is_empty() {
        return Err(invalid());
    }
    let mut normalized = Vec::with_capacity(package_paths.len());
    for package_path in package_paths {
        let Some(package_path) = package_path.as_str() else {
            return Err(invalid());
        };
        if package_path.is_empty() {
            return Err(invalid());
        }
        normalized.push(package_path.to_string());
    }
    Ok(Some(normalize_plugin_package_paths(&normalized)?))
}

/// Upstream `writePluginPackageProfile`: exact two-space JSON serialization
/// with a trailing newline (`JSON.stringify(document, null, 2) + "\n"`).
pub fn write_plugin_package_profile(
    path: &Path,
    package_paths: &[String],
    session_path: Option<&str>,
) -> Result<(), String> {
    let mut document = serde_json::Map::new();
    document.insert("version".to_string(), json!(PLUGIN_PACKAGE_PROFILE_VERSION));
    if let Some(session_path) = session_path {
        document.insert("sessionPath".to_string(), json!(session_path));
    }
    document.insert("packagePaths".to_string(), json!(package_paths.to_vec()));
    let serialized = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(document)).map_err(|e| e.to_string())?
    );
    let mut file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    file.write_all(serialized.as_bytes())
        .map_err(|error| error.to_string())
}

/// Upstream `ConfiguredServerPluginPackage`.
pub struct ServerPluginPackage {
    manifest_path: PathBuf,
    normalized_package_path: String,
}

/// Artifact production behind the D9 chord-bundler seam: the embedder builds
/// the facet bundle for the configured package into `outdir`.
pub type FacetBundleBuild =
    Arc<dyn Fn(&ServerPluginPackageBuildRequest) -> Result<Vec<Value>, String> + Send + Sync>;

use std::sync::Arc;

/// Inputs the embedder bundler receives (upstream `bundleFacetPackage` call).
#[derive(Debug, Clone)]
pub struct ServerPluginPackageBuildRequest {
    pub package_path: String,
    pub outdir: String,
    pub manifest_path: String,
}

impl ServerPluginPackage {
    /// Upstream `createServerPluginPackage`: the serialized build tail and
    /// manifest path layout
    /// `<dir>/plugin-builds/<serverId>/<pluginBuildDirectoryName>`.
    pub fn new(directory: &str, server_id: &str, package_path: &str) -> Result<Self, String> {
        if !is_connection_id(server_id) {
            // Upstream types serverId as `ServerId`; construction with a
            // non-canonical id is a caller bug, so mirror the server profile
            // validation text.
            return Err(format!("Invalid experimental server ID: {server_id}"));
        }
        let normalized_package_path =
            crate::coding_agent::utils::paths::resolve_path_auto_base(package_path)
                .map_err(|error| error.to_string())?;
        let outdir = Path::new(directory)
            .join("plugin-builds")
            .join(server_id)
            .join(plugin_build_directory_name(&normalized_package_path));
        Ok(Self {
            manifest_path: outdir.join(FACET_BUNDLE_MANIFEST_FILE),
            normalized_package_path,
        })
    }

    /// Upstream `manifestPath`.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Upstream `normalizedPackagePath` capture.
    pub fn package_path(&self) -> &str {
        &self.normalized_package_path
    }

    /// Upstream `build()`: produce tui artifacts through the embedder bundler.
    /// Upstream returns `[]` when the built manifest has no `tui` entry; the
    /// embedder enforces that on its side (D9).
    pub fn build(&self, bundler: &FacetBundleBuild) -> Result<Vec<Value>, String> {
        let request = ServerPluginPackageBuildRequest {
            package_path: self.normalized_package_path.clone(),
            outdir: self
                .manifest_path
                .parent()
                .map(|parent| parent.to_string_lossy().to_string())
                .unwrap_or_default(),
            manifest_path: self.manifest_path.to_string_lossy().to_string(),
        };
        bundler(&request)
    }
}
