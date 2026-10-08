//! Port of upstream `coding-agent/src/utils/paths.ts`.
//!
//! Path normalization / resolution helpers. Byte-exactness validated against
//! `oracle_data::NORMALIZE_WINDOWS_SHELL_PATH`, `NORMALIZE_PATH`,
//! `IS_LOCAL_PATH`, `RESOLVE_PATH`, `CWD_RELATIVE_PATH` and
//! `FORMAT_PATH_RELATIVE`, plus the upstream `paths.test.ts` suite.
//!
//! Platform differences are ported faithfully and dispatched through an
//! explicit `windows` flag on the internal `_with` variants (upstream
//! dispatches on `process.platform`); the public functions use the host
//! platform (`cfg!(windows)`).
//!
//! Seams and divergences (disclosed):
//! - `canonicalizePath` uses `std::fs::canonicalize`, which returns the
//!   Windows verbatim (`\\?\`) form; the prefix is stripped to match node's
//!   `realpathSync` rendering.
//! - `getFileRevision` reproduces the `${dev}:${ino}:${size}:${mtimeNs}:
//!   ${ctimeNs}` bigint-stat format. On Windows, `dev`/`ino` are pinned to 0
//!   (node gets the volume serial number / file index from libuv; stable
//!   Rust keeps those accessors behind an unstable feature and new
//!   dependencies are forbidden), while `ctime` maps to the creation time
//!   (libuv maps `st_ctim` to the creation time, which node's stats expose).
//! - `homedir()` maps to `dirs::home_dir()` (USERPROFILE on Windows, like
//!   node's `os.homedir()`).
//! - `markPathIgnoredByCloudSync` shells out to `xattr`/`setfattr` on
//!   darwin/linux via [`super::child_process`] and is a no-op on other
//!   platforms, exactly like upstream's empty attribute list.
//! - Errors thrown by node's `fileURLToPath` surface as [`PathError`] with
//!   the JS error class name (`TypeError` / `URIError`) in `Display`.

use std::fmt;

use super::hosted_git_info::js_trim;
use super::node_path;
use super::node_url::{file_url_to_path, FileUrlError};

/// Upstream `PathInputOptions`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathInputOptions {
    /// Trim leading/trailing whitespace before normalization.
    pub trim: bool,
    /// Expand leading `~` to a home directory. Defaults to true.
    pub expand_tilde: Option<bool>,
    /// Home directory used for `~` expansion. Defaults to `os.homedir()`.
    pub home_dir: Option<String>,
    /// Strip a leading `@`, used for CLI @file paths.
    pub strip_at_prefix: bool,
    /// Normalize unicode space variants to regular spaces.
    pub normalize_unicode_spaces: bool,
}

/// Error thrown by the `fileURLToPath` seam, carrying the JS error class
/// name so `Display` matches the oracle's `TypeError: ...` /
/// `URIError: ...` renderings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError {
    pub kind: &'static str,
    pub message: String,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for PathError {}

impl From<FileUrlError> for PathError {
    fn from(error: FileUrlError) -> Self {
        match error {
            // node re-throws decodeURIComponent's URIError unchanged.
            FileUrlError::UriMalformed => Self {
                kind: "URIError",
                message: error.to_string(),
            },
            // The remaining fileURLToPath failures are TypeErrors.
            other => Self {
                kind: "TypeError",
                message: other.to_string(),
            },
        }
    }
}

fn current_dir_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string())
}

/// node's `posixCwd()`: on a Windows host the process cwd is converted to
/// forward slashes and the drive indicator stripped; on POSIX it is the raw
/// cwd.
fn node_posix_cwd() -> String {
    let raw = current_dir_string();
    if cfg!(windows) {
        let replaced = raw.replace('\\', "/");
        let index = replaced.find('/').unwrap_or(0);
        replaced[index.min(replaced.len())..].to_string()
    } else {
        raw
    }
}

fn default_home_dir() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Upstream `UNICODE_SPACES`: `[\u00A0\u2000-\u200A\u202F\u205F\u3000]`.
fn is_unicode_space_variant(c: char) -> bool {
    matches!(c, '\u{a0}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
        || ('\u{2000}'..='\u{200a}').contains(&c)
}

/// Convert Git Bash, MSYS, Cygwin, and WSL drive paths to a form native
/// Windows APIs accept (upstream `normalizeWindowsShellPath`, regex
/// `/^\/(?:mnt\/|cygdrive\/)?([a-z])(?:\/(.*))?$/i`).
pub fn normalize_windows_shell_path(file_path: &str) -> String {
    if !file_path.starts_with('/') || file_path.starts_with("//") || file_path.contains('\\') {
        return file_path.to_string();
    }
    let Some(rest) = file_path.strip_prefix('/') else {
        return file_path.to_string();
    };
    let rest = rest
        .strip_prefix("mnt/")
        .or_else(|| rest.strip_prefix("cygdrive/"))
        .unwrap_or(rest);
    let mut chars = rest.chars();
    let Some(drive) = chars.next() else {
        return file_path.to_string();
    };
    if !drive.is_ascii_alphabetic() {
        return file_path.to_string();
    }
    let after_drive = chars.as_str();
    let suffix = if let Some(stripped) = after_drive.strip_prefix('/') {
        Some(stripped)
    } else if after_drive.is_empty() {
        None
    } else {
        return file_path.to_string();
    };
    let suffix = suffix.map(|s| s.replace('/', "\\")).unwrap_or_default();
    format!("{}:\\{}", drive.to_ascii_uppercase(), suffix)
}

/// Internal `normalizePath` with the platform flag explicit.
pub fn normalize_path_with(
    input: &str,
    options: &PathInputOptions,
    windows: bool,
) -> Result<String, PathError> {
    let mut normalized = if options.trim {
        js_trim(input).to_string()
    } else {
        input.to_string()
    };
    if options.normalize_unicode_spaces {
        normalized = normalized
            .chars()
            .map(|c| if is_unicode_space_variant(c) { ' ' } else { c })
            .collect();
    }
    if options.strip_at_prefix && normalized.starts_with('@') {
        normalized = normalized[1..].to_string();
    }
    if windows {
        normalized = normalize_windows_shell_path(&normalized);
    }

    if options.expand_tilde.unwrap_or(true) {
        let home = options.home_dir.clone().unwrap_or_else(default_home_dir);
        if normalized == "~" {
            return Ok(home);
        }
        let tilde_slash = normalized.starts_with("~/");
        let tilde_backslash = windows && normalized.starts_with("~\\");
        if tilde_slash || tilde_backslash {
            let joined = if windows {
                node_path::win32_join(&[&home, &normalized[2..]])
            } else {
                node_path::posix_join(&[&home, &normalized[2..]])
            };
            return Ok(joined);
        }
    }

    if normalized.starts_with("file://") {
        return Ok(file_url_to_path(&normalized, windows)?);
    }

    Ok(normalized)
}

/// `normalizePath(input)` on the host platform (default options).
pub fn normalize_path(input: &str) -> Result<String, PathError> {
    normalize_path_with(input, &PathInputOptions::default(), cfg!(windows))
}

/// `normalizePath(input, options)` on the host platform.
pub fn normalize_path_with_options(
    input: &str,
    options: &PathInputOptions,
) -> Result<String, PathError> {
    normalize_path_with(input, options, cfg!(windows))
}

fn node_resolve(args: &[&str], windows: bool) -> String {
    if windows {
        let cwd = current_dir_string();
        node_path::win32_resolve(args, &cwd)
    } else {
        // node posix.resolve falls back to posixCwd(), which strips the
        // drive and converts separators on a Windows host.
        let cwd = node_posix_cwd();
        node_path::posix_resolve(args, &cwd)
    }
}

fn node_is_absolute(path: &str, windows: bool) -> bool {
    if windows {
        node_path::win32_is_absolute(path)
    } else {
        node_path::posix_is_absolute(path)
    }
}

fn node_relative(from: &str, to: &str, windows: bool) -> String {
    if windows {
        let cwd = current_dir_string();
        node_path::win32_relative(from, to, &cwd)
    } else {
        let cwd = node_posix_cwd();
        node_path::posix_relative(from, to, &cwd)
    }
}

/// Internal `resolvePath` with the platform flag explicit.
pub fn resolve_path_with(
    input: &str,
    base_dir: &str,
    options: &PathInputOptions,
    windows: bool,
) -> Result<String, PathError> {
    let normalized = normalize_path_with(input, options, windows)?;
    let normalized_base_dir = normalize_path_with(base_dir, &PathInputOptions::default(), windows)?;
    if node_is_absolute(&normalized, windows) {
        Ok(node_resolve(&[&normalized], windows))
    } else {
        Ok(node_resolve(&[&normalized_base_dir, &normalized], windows))
    }
}

/// `resolvePath(input, baseDir)` on the host platform.
pub fn resolve_path(input: &str, base_dir: &str) -> Result<String, PathError> {
    resolve_path_with(input, base_dir, &PathInputOptions::default(), cfg!(windows))
}

/// `resolvePath(input)` on the host platform (base directory defaults to the
/// process cwd, like upstream).
pub fn resolve_path_auto_base(input: &str) -> Result<String, PathError> {
    resolve_path_with(
        input,
        &current_dir_string(),
        &PathInputOptions::default(),
        cfg!(windows),
    )
}

/// Resolve a path to its canonical (real) form, following symlinks. Falls
/// back to the raw path if resolution fails (upstream `canonicalizePath`).
pub fn canonicalize_path(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(real) => strip_verbatim_prefix(&real.to_string_lossy()),
        Err(_) => path.to_string(),
    }
}

/// node's `realpathSync` returns non-verbatim paths; `std::fs::canonicalize`
/// returns `\\?\`-prefixed ones on Windows.
pub(crate) fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// `${dev}:${ino}:${size}:${mtimeNs}:${ctimeNs}` from bigint stats; `None`
/// when the path cannot be stat-ed (upstream `getFileRevision`).
pub fn get_file_revision(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(file_revision_from_metadata(&metadata))
}

#[cfg(unix)]
fn file_revision_from_metadata(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    let mtime_ns = metadata.mtime_nsec() as i128 + metadata.mtime() as i128 * 1_000_000_000;
    let ctime_ns = metadata.ctime_nsec() as i128 + metadata.ctime() as i128 * 1_000_000_000;
    format!(
        "{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        mtime_ns,
        ctime_ns
    )
}

#[cfg(windows)]
fn file_revision_from_metadata(metadata: &std::fs::Metadata) -> String {
    use std::os::windows::fs::MetadataExt;
    const EPOCH_DIFF_100NS: u64 = 11_644_473_600_000_000;
    // libuv maps st_ctim to the file creation time on Windows.
    let mtime_ns = (metadata.last_write_time().saturating_sub(EPOCH_DIFF_100NS)) as i128 * 100;
    let ctime_ns = (metadata.creation_time().saturating_sub(EPOCH_DIFF_100NS)) as i128 * 100;
    // Divergence: node's stats expose the volume serial number (st_dev) and
    // the file index (st_ino), but stable Rust's MetadataExt keeps
    // `volume_serial_number`/`file_index` behind the unstable
    // `windows_by_handle` feature and new dependencies are forbidden. The
    // revision keeps the `${dev}:${ino}:...` shape with those fields pinned
    // to 0; change detection still works through size/mtime/ctime, which is
    // this revision string's purpose.
    let (dev, ino) = (0u64, 0u64);
    format!(
        "{}:{}:{}:{}:{}",
        dev,
        ino,
        metadata.file_size(),
        mtime_ns,
        ctime_ns
    )
}

/// True when the value is NOT a package source (npm:, git:, etc.) or a
/// remote URL protocol; bare names, relative paths, and file: URLs are
/// considered local (upstream `isLocalPath`).
pub fn is_local_path(value: &str) -> bool {
    let trimmed = js_trim(value);
    !trimmed.starts_with("npm:")
        && !trimmed.starts_with("git:")
        && !trimmed.starts_with("github:")
        && !trimmed.starts_with("http:")
        && !trimmed.starts_with("https:")
        && !trimmed.starts_with("ssh:")
}

fn sep_char(windows: bool) -> char {
    if windows {
        '\\'
    } else {
        '/'
    }
}

/// Internal `getCwdRelativePath` with the platform flag explicit.
pub fn get_cwd_relative_path_with(
    file_path: &str,
    cwd: &str,
    windows: bool,
) -> Result<Option<String>, PathError> {
    let resolved_cwd = resolve_path_with(
        cwd,
        &current_dir_string(),
        &PathInputOptions::default(),
        windows,
    )?;
    let resolved_path = resolve_path_with(
        file_path,
        &resolved_cwd,
        &PathInputOptions::default(),
        windows,
    )?;
    let relative_path = node_relative(&resolved_cwd, &resolved_path, windows);
    let parent_prefix = format!("..{}", sep_char(windows));
    let is_inside_cwd = relative_path.is_empty()
        || (relative_path != ".."
            && !relative_path.starts_with(&parent_prefix)
            && !node_is_absolute(&relative_path, windows));

    Ok(if is_inside_cwd {
        Some(if relative_path.is_empty() {
            ".".to_string()
        } else {
            relative_path
        })
    } else {
        None
    })
}

/// `getCwdRelativePath(filePath, cwd)` on the host platform.
pub fn get_cwd_relative_path(file_path: &str, cwd: &str) -> Result<Option<String>, PathError> {
    get_cwd_relative_path_with(file_path, cwd, cfg!(windows))
}

/// Internal `formatPathRelativeToCwdOrAbsolute` with the platform flag
/// explicit.
pub fn format_path_relative_to_cwd_or_absolute_with(
    file_path: &str,
    cwd: &str,
    windows: bool,
) -> Result<String, PathError> {
    let absolute_path = resolve_path_with(file_path, cwd, &PathInputOptions::default(), windows)?;
    let relative = get_cwd_relative_path_with(&absolute_path, cwd, windows)?;
    let value = relative.unwrap_or(absolute_path);
    Ok(if windows {
        value.replace(sep_char(windows), "/")
    } else {
        value
    })
}

/// `formatPathRelativeToCwdOrAbsolute(filePath, cwd)` on the host platform.
pub fn format_path_relative_to_cwd_or_absolute(
    file_path: &str,
    cwd: &str,
) -> Result<String, PathError> {
    format_path_relative_to_cwd_or_absolute_with(file_path, cwd, cfg!(windows))
}

/// Attach the cloud-sync "ignore" extended attributes (upstream
/// `markPathIgnoredByCloudSync`): a no-op off darwin/linux, exactly like the
/// upstream empty attribute list on other platforms.
pub fn mark_path_ignored_by_cloud_sync(path: &str) {
    #[cfg(target_os = "macos")]
    {
        use super::child_process;
        for attr in ["com.dropbox.ignored", "com.apple.fileprovider.ignore#P"] {
            let _ = child_process::spawn_process_sync_discarding_output(
                "xattr",
                &["-w", attr, "1", path],
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        use super::child_process;
        let attr = "user.com.dropbox.ignored";
        let _ = child_process::spawn_process_sync_discarding_output(
            "setfattr",
            &["-n", attr, "-v", "1", path],
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    fn current_dir() -> String {
        current_dir_string()
    }

    fn options_from_tag(tag: &str) -> PathInputOptions {
        let mut options = PathInputOptions::default();
        match tag {
            "trim" => options.trim = true,
            "unicode" => options.normalize_unicode_spaces = true,
            "at" => options.strip_at_prefix = true,
            "noexpand" => options.expand_tilde = Some(false),
            "" => {}
            other => {
                if let Some(home) = other.strip_prefix("home=") {
                    options.home_dir = Some(home.to_string());
                } else {
                    panic!("unknown options tag {other:?}");
                }
            }
        }
        options
    }

    #[test]
    fn normalize_windows_shell_path_matches_oracle() {
        for (input, expected) in oracle::NORMALIZE_WINDOWS_SHELL_PATH {
            assert_eq!(
                normalize_windows_shell_path(input),
                *expected,
                "input {input:?}"
            );
        }
    }

    #[test]
    fn normalize_path_matches_oracle_for_both_flavors() {
        for (platform, input, tag, expected) in oracle::NORMALIZE_PATH {
            let windows = *platform == "win32";
            let got = normalize_path_with(input, &options_from_tag(tag), windows);
            match (*expected).strip_prefix("!ERR") {
                Some(expected_error) => {
                    let error = got.expect_err("expected error");
                    assert_eq!(
                        error.to_string(),
                        expected_error,
                        "error for {input:?} ({platform})"
                    );
                }
                None => {
                    assert_eq!(
                        &got.expect("ok"),
                        expected,
                        "normalize {input:?} ({platform})"
                    );
                }
            }
        }
    }

    #[test]
    fn is_local_path_matches_oracle() {
        for (input, expected) in oracle::IS_LOCAL_PATH {
            assert_eq!(is_local_path(input), *expected, "input {input:?}");
        }
    }

    /// The oracle was captured with HOME=C:\Users\13063 and cwd =
    /// tests/fixtures/utils_oracle; entries pinned to those substitute the live
    /// values so the grid stays machine-independent.
    #[cfg(windows)] // only the win32-host resolve grid consumes it
    fn adjust_oracle_value(expected: &str) -> String {
        let captured_cwd =
            "C:\\Users\\13063\\Desktop\\code\\agent work\\pi-rust\\scratch\\utils_oracle";
        if expected == captured_cwd {
            // environment-anchored: the capture resolved `C:` against its C:
            // process cwd. On machines whose cwd sits on another drive, node
            // falls back to that device's root.
            let live = current_dir();
            let on_capture_drive = live
                .chars()
                .next()
                .is_some_and(|drive| drive.eq_ignore_ascii_case(&'C'));
            return if on_capture_drive {
                live
            } else {
                r"C:\".to_string()
            };
        }
        let live_home = default_home_dir();
        if live_home.is_empty() || live_home == "C:\\Users\\13063" {
            return expected.to_string();
        }
        expected.replace("C:\\Users\\13063", &live_home)
    }

    #[test]
    #[cfg(windows)] // captured on the win32 oracle host (drive/cwd interplay)
    fn resolve_path_matches_oracle() {
        for (input, base, expected) in oracle::RESOLVE_PATH {
            match (*expected).strip_prefix("!ERR") {
                Some(expected_error) => {
                    let error = resolve_path_with(input, base, &PathInputOptions::default(), true)
                        .expect_err("expected error");
                    assert_eq!(error.to_string(), expected_error, "error for {input:?}");
                }
                None => {
                    let adjusted = adjust_oracle_value(expected);
                    let got = resolve_path_with(input, base, &PathInputOptions::default(), true)
                        .expect("ok");
                    // environment-anchored: both sides normalized (drive and
                    // home anchors).
                    assert_eq!(
                        crate::coding_agent::oracle_scrub::scrub_str(&got),
                        crate::coding_agent::oracle_scrub::scrub_str(&adjusted),
                        "resolve {input:?} against {base:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn cwd_relative_path_matches_oracle() {
        for (file_path, cwd, expected) in oracle::CWD_RELATIVE_PATH {
            let got = get_cwd_relative_path_with(file_path, cwd, true).expect("no throw");
            let got_encoded = match got {
                None => "undefined".to_string(),
                Some(value) => value,
            };
            assert_eq!(
                got_encoded, *expected,
                "getCwdRelativePath({file_path:?}, {cwd:?})"
            );
        }
    }

    #[test]
    fn format_path_relative_matches_oracle() {
        for (file_path, cwd, expected) in oracle::FORMAT_PATH_RELATIVE {
            let got = format_path_relative_to_cwd_or_absolute_with(file_path, cwd, true)
                .expect("no throw");
            assert_eq!(
                got, *expected,
                "formatPathRelativeToCwdOrAbsolute({file_path:?}, {cwd:?})"
            );
        }
    }

    // -------------------------------------------------------------- //
    // upstream paths.test.ts ports                                    //
    // -------------------------------------------------------------- //

    fn create_temp_dir(tag: &str) -> tempfile::TempDir {
        tempfile::TempDir::with_prefix(format!("pi-paths-{tag}-")).expect("tempdir")
    }

    #[test]
    fn canonicalize_path_returns_the_real_path_for_a_regular_file() {
        let dir = create_temp_dir("real");
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "hello").expect("write");
        let real = strip_verbatim_prefix(
            &std::fs::canonicalize(&file)
                .expect("realpath")
                .to_string_lossy(),
        );
        assert_eq!(canonicalize_path(file.to_str().expect("utf8")), real);
    }

    #[cfg(unix)]
    #[test]
    fn canonicalize_path_resolves_symlinks_to_their_targets() {
        let dir = create_temp_dir("sym");
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&target, "hello").expect("write");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert_eq!(
            canonicalize_path(link.to_str().expect("utf8")),
            canonicalize_path(target.to_str().expect("utf8"))
        );
    }

    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).expect("symlink");
        }
        #[cfg(windows)]
        {
            // Directory junctions need no privileges, matching the upstream
            // symlink-creation intent on Windows where real symlinks are
            // privilege-gated.
            let output = std::process::Command::new("cmd")
                .args([
                    "/c",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output()
                .expect("mklink runs");
            assert!(
                output.status.success(),
                "mklink /J failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn canonicalize_path_resolves_directory_links() {
        let dir = create_temp_dir("dirsym");
        let target_dir = dir.path().join("target-dir");
        let link_dir = dir.path().join("link-dir");
        std::fs::create_dir(&target_dir).expect("mkdir");
        create_dir_link(&target_dir, &link_dir);
        assert_eq!(
            canonicalize_path(link_dir.to_str().expect("utf8")),
            canonicalize_path(target_dir.to_str().expect("utf8"))
        );
    }

    #[test]
    fn canonicalize_path_falls_back_to_the_raw_path_when_the_target_does_not_exist() {
        let dir = create_temp_dir("missing");
        let nonexistent = dir.path().join("no-such-file");
        assert_eq!(
            canonicalize_path(nonexistent.to_str().expect("utf8")),
            nonexistent.to_str().expect("utf8")
        );
    }

    #[test]
    fn canonicalize_path_falls_back_for_a_dangling_link() {
        let dir = create_temp_dir("dangle");
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &link).expect("symlink");
        }
        #[cfg(windows)]
        {
            // Dangling junctions are creatable without privileges; the
            // canonicalize must fail and fall back to the link path.
            let output = std::process::Command::new("cmd")
                .args([
                    "/c",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output()
                .expect("mklink runs");
            assert!(
                output.status.success(),
                "mklink /J failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(
            canonicalize_path(link.to_str().expect("utf8")),
            link.to_str().expect("utf8")
        );
    }

    #[test]
    fn get_cwd_relative_path_keeps_cwd_relative_names_that_start_with_dots() {
        let cwd = std::env::temp_dir().join("pi-paths-cwd");
        let file = std::path::Path::new(&cwd)
            .join("..config")
            .join("AGENTS.md");
        let got = get_cwd_relative_path(file.to_str().expect("utf8"), cwd.to_str().expect("utf8"))
            .expect("no throw")
            .expect("inside cwd");
        assert_eq!(
            got,
            std::path::Path::new("..config")
                .join("AGENTS.md")
                .to_str()
                .expect("utf8")
        );
    }

    #[test]
    fn get_cwd_relative_path_rejects_parent_directory_traversals() {
        let cwd = std::env::temp_dir().join("pi-paths-cwd");
        let file = std::path::Path::new(&cwd).join("..").join("AGENTS.md");
        let got = get_cwd_relative_path(file.to_str().expect("utf8"), cwd.to_str().expect("utf8"))
            .expect("no throw");
        assert!(got.is_none());
    }

    #[test]
    fn expands_only_home_tilde_shortcuts() {
        let cwd = std::env::temp_dir().join("pi-paths-cwd");
        let home = default_home_dir();
        assert_eq!(normalize_path("~").expect("ok"), home);
        let expected_home_file = if cfg!(windows) {
            node_path::win32_join(&[&home, "file.txt"])
        } else {
            node_path::posix_join(&[&home, "file.txt"])
        };
        assert_eq!(
            normalize_path("~/file.txt").expect("ok"),
            expected_home_file
        );
        assert_eq!(
            resolve_path("~draft.md", cwd.to_str().expect("utf8")).expect("ok"),
            node_resolve(&[cwd.to_str().expect("utf8"), "~draft.md"], cfg!(windows))
        );
        assert_eq!(normalize_path("~draft.md").expect("ok"), "~draft.md");
    }

    #[test]
    fn resolves_relative_paths_against_the_base_directory() {
        let cwd = std::env::temp_dir().join("pi-paths-cwd");
        let cwd_str = cwd.to_str().expect("utf8");
        assert_eq!(
            resolve_path("subdir/file.txt", cwd_str).expect("ok"),
            node_resolve(&[cwd_str, "subdir/file.txt"], cfg!(windows))
        );
        // file URL base directory
        let base_url = url::Url::from_file_path(&cwd).expect("file url");
        assert_eq!(
            resolve_path("subdir/file.txt", base_url.as_str()).expect("ok"),
            node_resolve(&[cwd_str, "subdir/file.txt"], cfg!(windows))
        );
    }

    #[test]
    fn accepts_file_urls() {
        let dir = create_temp_dir("fileurl");
        let file_path = dir.path().join("file with spaces.txt");
        let file_url = url::Url::from_file_path(&file_path).expect("file url");
        let base = dir.path().join("base");
        assert_eq!(
            resolve_path(file_url.as_str(), base.to_str().expect("utf8")).expect("ok"),
            node_resolve(&[file_path.to_str().expect("utf8")], cfg!(windows))
        );
    }

    #[test]
    fn throws_for_invalid_file_urls() {
        let result = resolve_path("file:///%E0%A4%A", &current_dir());
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_posix_absolute_paths_with_literal_percent_sequences() {
        let dir = create_temp_dir("percent");
        for file_name in ["report%2026.md", "foo%2Fbar", "malformed%A.md"] {
            let file_path = dir.path().join(file_name);
            let base = dir.path().join("base");
            assert_eq!(
                resolve_path(
                    file_path.to_str().expect("utf8"),
                    base.to_str().expect("utf8")
                )
                .expect("ok"),
                node_resolve(&[file_path.to_str().expect("utf8")], false)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn does_not_treat_windows_file_url_pathname_strings_as_native_paths() {
        let dir = create_temp_dir("pathname");
        let file_path = dir.path().join("dir").join("SKILL.md");
        let pathname = url::Url::from_file_path(&file_path)
            .expect("file url")
            .path()
            .to_string();
        assert!(
            pathname.len() > 2
                && pathname.starts_with('/')
                && pathname.as_bytes()[1].is_ascii_alphabetic()
                && pathname.as_bytes()[2] == b':'
        );
        assert_eq!(
            resolve_path(&pathname, "E:\\project").expect("ok"),
            node_resolve(&[&pathname], true)
        );
    }

    #[test]
    fn normalize_windows_shell_path_converts_shell_drive_paths() {
        // upstream paths.test.ts
        assert_eq!(
            normalize_windows_shell_path("/c/Users/example/project"),
            "C:\\Users\\example\\project"
        );
        assert_eq!(normalize_windows_shell_path("/cygdrive/d/work"), "D:\\work");
        assert_eq!(normalize_windows_shell_path("/mnt/e/source"), "E:\\source");
        assert_eq!(normalize_windows_shell_path("/c"), "C:\\");
    }

    #[test]
    fn normalize_windows_shell_path_leaves_other_path_forms_unchanged() {
        for path in [
            "C:/Users/example",
            "C:\\Users\\example",
            "//server/share/file",
            "/c/Users\\example",
            "relative/file",
            "/tmp/file",
        ] {
            assert_eq!(normalize_windows_shell_path(path), path);
        }
    }

    #[cfg(windows)]
    #[test]
    fn is_applied_by_normal_path_handling_on_windows() {
        assert_eq!(
            normalize_path("/c/Users/example").expect("ok"),
            "C:\\Users\\example"
        );
        assert_eq!(
            resolve_path("/mnt/c/Users/example", "D:\\work").expect("ok"),
            node_resolve(&["C:/Users/example"], true)
        );
    }

    #[test]
    fn is_local_path_upstream_cases() {
        assert!(is_local_path("my-package"));
        assert!(is_local_path("./foo"));
        assert!(is_local_path("file:///tmp/foo"));
        assert!(!is_local_path("npm:package"));
        assert!(!is_local_path("git://repo"));
        assert!(!is_local_path("https://example.com"));
    }

    #[test]
    fn get_file_revision_tracks_metadata_changes() {
        let dir = create_temp_dir("rev");
        let file = dir.path().join("rev.txt");
        assert!(get_file_revision(file.to_str().expect("utf8")).is_none());
        std::fs::write(&file, "v1").expect("write");
        let revision_1 = get_file_revision(file.to_str().expect("utf8")).expect("revision");
        assert_eq!(
            revision_1,
            get_file_revision(file.to_str().expect("utf8")).expect("revision")
        );
        std::fs::write(&file, "v2-with-longer-content").expect("rewrite");
        let revision_2 = get_file_revision(file.to_str().expect("utf8")).expect("revision");
        assert_ne!(
            revision_1, revision_2,
            "size change must change the revision"
        );
        let fields: Vec<&str> = revision_1.split(':').collect();
        assert_eq!(fields.len(), 5, "dev:ino:size:mtimeNs:ctimeNs shape");
    }
}
