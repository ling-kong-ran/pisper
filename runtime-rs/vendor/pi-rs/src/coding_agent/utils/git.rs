//! Port of upstream `coding-agent/src/utils/git.ts`.
//!
//! Parses git package sources into [`GitSource`]. The hosted-shorthand
//! recognition delegates to [`hosted_git_info`] (vendored `hosted-git-info`
//! 9.0.3 port) and the protocol/scp URL splitting uses the `url` crate (the
//! WHATWG parser, matching node's `URL`).
//!
//! Byte-exactness validated against `oracle_data::PARSE_GIT_URL` (36 upstream
//! cases captured under node) and the upstream `git-ssh-url.test.ts` suite.
//!
//! Divergence: upstream matches scp-style and protocol prefixes with regexes
//! (`^git@([^:]+):(.+)$`, `^(https?|ssh|git):\/\//i`); the port reproduces
//! exactly those matches with string operations (including the empty-host /
//! empty-path non-matches of the scp pattern).

use url::Url;

use super::hosted_git_info::{decode_uri_component, from_url, js_trim};

/// Parsed git URL information (upstream `GitSource`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// Always `"git"` for git sources.
    pub repo_type: &'static str,
    /// Clone URL (always valid for git clone, without ref suffix).
    pub repo: String,
    /// Git host domain (e.g. `"github.com"`).
    pub host: String,
    /// Repository path (e.g. `"user/repo"`).
    pub path: String,
    /// Git ref (branch, tag, commit) if specified.
    pub ref_: Option<String>,
    /// True if ref was specified (package won't be auto-updated).
    pub pinned: bool,
}

/// JS `^git@([^:]+):(.+)$`.
fn scp_like_match(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("git@")?;
    let sep = rest.find(':')?;
    let host = &rest[..sep];
    let path = &rest[sep + 1..];
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

fn strip_leading_slashes(value: &str) -> &str {
    value.trim_start_matches('/')
}

fn split_ref(url: &str) -> (String, Option<String>) {
    if let Some((host, path_with_maybe_ref)) = scp_like_match(url) {
        return match path_with_maybe_ref.find('@') {
            None => (url.to_string(), None),
            Some(ref_separator) => {
                let repo_path = &path_with_maybe_ref[..ref_separator];
                let ref_ = &path_with_maybe_ref[ref_separator + 1..];
                if repo_path.is_empty() || ref_.is_empty() {
                    (url.to_string(), None)
                } else {
                    (format!("git@{host}:{repo_path}"), Some(ref_.to_string()))
                }
            }
        };
    }

    if url.contains("://") {
        return match Url::parse(url) {
            Err(_) => (url.to_string(), None),
            Ok(parsed) => {
                let path_with_maybe_ref = strip_leading_slashes(parsed.path()).to_string();
                match path_with_maybe_ref.find('@') {
                    None => (url.to_string(), None),
                    Some(ref_separator) => {
                        let repo_path = &path_with_maybe_ref[..ref_separator];
                        let ref_ = &path_with_maybe_ref[ref_separator + 1..];
                        if repo_path.is_empty() || ref_.is_empty() {
                            (url.to_string(), None)
                        } else {
                            let mut rebuilt = parsed;
                            rebuilt.set_path(&format!("/{repo_path}"));
                            let rendered = rebuilt.as_str().to_string();
                            // `.replace(/\/$/, "")` strips one trailing '/'.
                            let repo = match rendered.strip_suffix('/') {
                                Some(stripped) => stripped.to_string(),
                                None => rendered,
                            };
                            (repo, Some(ref_.to_string()))
                        }
                    }
                }
            }
        };
    }

    split_ref_fallback(url)
}

fn split_ref_fallback(url: &str) -> (String, Option<String>) {
    let Some(slash_index) = url.find('/') else {
        return (url.to_string(), None);
    };
    let host = &url[..slash_index];
    let path_with_maybe_ref = &url[slash_index + 1..];
    match path_with_maybe_ref.find('@') {
        None => (url.to_string(), None),
        Some(ref_separator) => {
            let repo_path = &path_with_maybe_ref[..ref_separator];
            let ref_ = &path_with_maybe_ref[ref_separator + 1..];
            if repo_path.is_empty() || ref_.is_empty() {
                (url.to_string(), None)
            } else {
                (format!("{host}/{repo_path}"), Some(ref_.to_string()))
            }
        }
    }
}

fn has_unsafe_git_install_part(value: &str, allow_slash: bool) -> bool {
    let Some(decoded) = decode_for_validation(value) else {
        return true;
    };
    for candidate in [value.to_string(), decoded] {
        if candidate.contains('\0') || candidate.contains('\\') || candidate.starts_with('/') {
            return true;
        }
        if !allow_slash && candidate.contains('/') {
            return true;
        }
        if candidate.split('/').any(|segment| segment == "..") {
            return true;
        }
    }
    false
}

fn decode_for_validation(value: &str) -> Option<String> {
    decode_uri_component(value)
}

struct GitSourceArgs {
    repo: String,
    host: String,
    path: String,
    ref_: Option<String>,
}

fn build_git_source(args: GitSourceArgs) -> Option<GitSource> {
    if args.path.starts_with('/') {
        return None;
    }
    // `.replace(/\.git$/, "").replace(/^\/+/, "")`.
    let without_git_suffix = args.path.strip_suffix(".git").unwrap_or(&args.path);
    let normalized_path = strip_leading_slashes(without_git_suffix);
    if args.host.is_empty() || normalized_path.is_empty() || normalized_path.split('/').count() < 2
    {
        return None;
    }
    if has_unsafe_git_install_part(&args.host, false)
        || has_unsafe_git_install_part(normalized_path, true)
    {
        return None;
    }

    let pinned = args.ref_.is_some();
    Some(GitSource {
        repo_type: "git",
        repo: args.repo,
        host: args.host,
        path: normalized_path.to_string(),
        ref_: args.ref_,
        pinned,
    })
}

fn parse_generic_git_url(url: &str) -> Option<GitSource> {
    let (repo_without_ref, ref_) = split_ref(url);
    let mut repo = repo_without_ref.clone();
    let host;
    let path;

    if let Some((scp_host, scp_path)) = scp_like_match(&repo_without_ref) {
        host = scp_host;
        path = scp_path;
    } else if repo_without_ref.starts_with("https://")
        || repo_without_ref.starts_with("http://")
        || repo_without_ref.starts_with("ssh://")
        || repo_without_ref.starts_with("git://")
    {
        let parsed = Url::parse(&repo_without_ref).ok()?;
        host = parsed.host_str().unwrap_or("").to_string();
        path = strip_leading_slashes(parsed.path()).to_string();
    } else {
        let slash_index = repo_without_ref.find('/')?;
        host = repo_without_ref[..slash_index].to_string();
        path = repo_without_ref[slash_index + 1..].to_string();
        if !host.contains('.') && host != "localhost" {
            return None;
        }
        repo = format!("https://{repo_without_ref}");
    }

    build_git_source(GitSourceArgs {
        repo,
        host,
        path,
        ref_,
    })
}

/// JS template-literal rendering of a nullable value: `null` → `"null"`.
fn js_render(value: Option<&str>) -> &str {
    value.unwrap_or("null")
}

/// JS truthiness for the `info.committish || split.ref || undefined` chain.
fn truthy(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

/// JS `^(https?|ssh|git):\/\//i`.
fn has_explicit_protocol(url: &str) -> bool {
    let Some(sep) = url.find("://") else {
        return false;
    };
    matches!(
        url[..sep].to_ascii_lowercase().as_str(),
        "https" | "http" | "ssh" | "git"
    )
}

/// Parse git source into a [`GitSource`].
///
/// Rules (upstream doc):
/// - With git: prefix, accept all historical shorthand forms.
/// - Without git: prefix, only accept explicit protocol URLs.
pub fn parse_git_url(source: &str) -> Option<GitSource> {
    let trimmed = js_trim(source);
    let has_git_prefix = trimmed.starts_with("git:");
    let url = if has_git_prefix {
        js_trim(&trimmed[4..])
    } else {
        trimmed
    };

    if !has_git_prefix && !has_explicit_protocol(url) {
        return None;
    }

    let (split_repo, split_ref_value) = split_ref(url);

    let hosted_candidates: Vec<String> = [
        split_ref_value
            .as_ref()
            .map(|r| format!("{split_repo}#{r}")),
        Some(url.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect();
    for candidate in &hosted_candidates {
        if let Some(info) = from_url(candidate) {
            if split_ref_value.is_some() && info.project.as_deref().is_some_and(|p| p.contains('@'))
            {
                continue;
            }
            let use_https_prefix = !split_repo.starts_with("http://")
                && !split_repo.starts_with("https://")
                && !split_repo.starts_with("ssh://")
                && !split_repo.starts_with("git://")
                && !split_repo.starts_with("git@");
            return build_git_source(GitSourceArgs {
                repo: if use_https_prefix {
                    format!("https://{split_repo}")
                } else {
                    split_repo.clone()
                },
                host: if info.domain.is_empty() {
                    String::new()
                } else {
                    info.domain.to_string()
                },
                path: format!(
                    "{}/{}",
                    js_render(info.user.as_deref()),
                    js_render(info.project.as_deref())
                ),
                ref_: truthy(info.committish).or_else(|| split_ref_value.clone()),
            });
        }
    }

    let https_candidates: Vec<String> = [
        split_ref_value
            .as_ref()
            .map(|r| format!("https://{split_repo}#{r}")),
        Some(format!("https://{url}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for candidate in &https_candidates {
        if let Some(info) = from_url(candidate) {
            if split_ref_value.is_some() && info.project.as_deref().is_some_and(|p| p.contains('@'))
            {
                continue;
            }
            return build_git_source(GitSourceArgs {
                repo: format!("https://{split_repo}"),
                host: if info.domain.is_empty() {
                    String::new()
                } else {
                    info.domain.to_string()
                },
                path: format!(
                    "{}/{}",
                    js_render(info.user.as_deref()),
                    js_render(info.project.as_deref())
                ),
                ref_: truthy(info.committish).or_else(|| split_ref_value.clone()),
            });
        }
    }

    parse_generic_git_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    fn encode(source: &Option<GitSource>) -> String {
        match source {
            None => "null".to_string(),
            Some(s) => [
                s.repo.clone(),
                s.host.clone(),
                s.path.clone(),
                s.ref_.clone().unwrap_or_default(),
                s.pinned.to_string(),
            ]
            .join("|"),
        }
    }

    #[test]
    fn matches_upstream_parse_git_url_byte_for_byte() {
        for (source, expected) in oracle::PARSE_GIT_URL {
            let got = encode(&parse_git_url(source));
            assert_eq!(got, *expected, "parseGitUrl({source:?})");
        }
    }

    #[test]
    fn parses_protocol_urls_without_git_prefix() {
        // upstream git-ssh-url.test.ts
        let result = parse_git_url("https://github.com/user/repo").expect("parses");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "https://github.com/user/repo");

        let result = parse_git_url("ssh://git@github.com/user/repo").expect("parses");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "ssh://git@github.com/user/repo");

        let result = parse_git_url("https://github.com/user/repo@v1.0.0").expect("parses");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.ref_.as_deref(), Some("v1.0.0"));
        assert_eq!(result.repo, "https://github.com/user/repo");
    }

    #[test]
    fn parses_shorthand_urls_only_with_git_prefix() {
        let result = parse_git_url("git:git@github.com:user/repo").expect("parses");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "git@github.com:user/repo");

        let result = parse_git_url("git:github.com/user/repo").expect("parses");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "https://github.com/user/repo");

        let result = parse_git_url("git:git@github.com:user/repo@v1.0.0").expect("parses");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.ref_.as_deref(), Some("v1.0.0"));
        assert_eq!(result.repo, "git@github.com:user/repo");
    }

    #[test]
    fn rejects_unsafe_git_install_path_inputs() {
        for source in [
            "git:git@evil.example:../../victim/repo",
            "https://evil.example/..%2F..%2Fvictim/repo",
            "https://evil.example/..%2F..%2Fvictim/repo%",
            "git:git@evil.example:/absolute/repo",
            "git:git@evil.example:user\\repo/name",
            "git:git@evil.example:user/repo\0name",
        ] {
            assert!(parse_git_url(source).is_none(), "rejects {source:?}");
        }
    }

    #[test]
    fn rejects_unsupported_forms_without_git_prefix() {
        assert!(parse_git_url("git@github.com:user/repo").is_none());
        assert!(parse_git_url("github.com/user/repo").is_none());
        assert!(parse_git_url("user/repo").is_none());
    }
}
