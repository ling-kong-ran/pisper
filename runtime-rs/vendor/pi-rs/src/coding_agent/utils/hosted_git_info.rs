//! Support port: the npm `hosted-git-info` package (v9.0.3, ISC), reduced to
//! the `fromUrl` surface consumed by [`crate::coding_agent::utils::git`].
//!
//! Ported from the vendored copies under `tests/fixtures/utils_oracle/node_modules/
//! hosted-git-info/lib/` (`from-url.js`, `parse-url.js`, `hosts.js`,
//! `index.js`) against the captured oracle (`oracle_data::HOSTED_FROM_URL`,
//! plus the `parse_git_url` grid that flows through it).
//!
//! Trimmed surface (disclosed): only `domain`, `user`, `project`,
//! `committish` are exposed — the URL formatting templates (`ssh()`,
//! `browse()`, `tarball()`, ...), the LRU cache, `fromManifest`, and the auth
//! string are not used by the upstream `git.ts` call sites in this slice.
//! `user: None` renders as JS `null` (and an absent `committish` segment
//! decodes to the JS coercion `"undefined"`, which the oracle pins).
//!
//! URL parsing delegates to the `url` crate (WHATWG URL, the same standard
//! node's `URL` implements), including opaque paths for non-special schemes
//! and the `%` / `\` path escaping rules the oracle relies on.

use url::Url;

/// A successfully parsed hosted-git-info record (upstream `GitHost`, trimmed
/// to the fields `git.ts` reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedInfo {
    /// Host table name, e.g. `"github"` (upstream `GitHost.type`).
    pub host_type: &'static str,
    /// Host domain from the host table, e.g. `"github.com"`.
    pub domain: &'static str,
    /// Upstream `GitHost.user`; `None` is JS `null`/`undefined`.
    pub user: Option<String>,
    /// Upstream `GitHost.project`; `None` is JS `null`/`undefined`.
    pub project: Option<String>,
    /// Upstream `GitHost.committish`; `None` is JS `null`/`undefined`.
    pub committish: Option<String>,
}

struct HostTable {
    name: &'static str,
    domain: &'static str,
    protocols: &'static [&'static str],
}

const HOSTS: &[HostTable] = &[
    HostTable {
        name: "github",
        domain: "github.com",
        protocols: &["git:", "http:", "git+ssh:", "git+https:", "ssh:", "https:"],
    },
    HostTable {
        name: "bitbucket",
        domain: "bitbucket.org",
        protocols: &["git+ssh:", "git+https:", "ssh:", "https:"],
    },
    HostTable {
        name: "gitlab",
        domain: "gitlab.com",
        protocols: &["git+ssh:", "git+https:", "ssh:", "https:"],
    },
    HostTable {
        name: "gist",
        domain: "gist.github.com",
        protocols: &["git:", "git+ssh:", "git+https:", "ssh:", "https:"],
    },
    HostTable {
        name: "sourcehut",
        domain: "git.sr.ht",
        protocols: &["git+ssh:", "https:"],
    },
];

fn by_shortcut(protocol: &str) -> Option<&'static HostTable> {
    // upstream `gitHosts.byShortcut[protocol]` where the keys are `<name>:`.
    HOSTS.iter().find(|h| protocol == format!("{}:", h.name))
}

fn by_domain(hostname: &str) -> Option<&'static HostTable> {
    let stripped = hostname.strip_prefix("www.").unwrap_or(hostname);
    HOSTS.iter().find(|h| h.domain == stripped)
}

/// JS `\s` (WhiteSpace plus U+FEFF).
fn js_is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// JS `String.prototype.trim` (includes U+FEFF, which Rust's `trim` does not).
pub(crate) fn js_trim(s: &str) -> &str {
    let mut current = s;
    loop {
        let next = current.trim().trim_matches('\u{feff}');
        if std::ptr::eq(next, current) {
            return next;
        }
        current = next;
    }
}

/// JS `decodeURIComponent`: strict `%xx` decoding with UTF-8 validation.
/// Returns `None` where JS would throw `URIError`.
pub(crate) fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16))?;
            let lo = bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16))?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// JS `/\s/.exec(arg)` position: index of the first JS-whitespace char.
fn first_whitespace(arg: &str) -> Option<usize> {
    arg.char_indices()
        .find(|(_, c)| js_is_space(*c))
        .map(|(i, _)| i)
}

/// upstream `isGitHubShorthand` (from-url.js): detects `user/repo`-style
/// GitHub shorthand inputs.
fn is_github_shorthand(arg: &str) -> bool {
    let first_hash = arg.find('#');
    let first_slash = arg.find('/');
    let second_slash = first_slash.and_then(|i| arg[i + 1..].find('/').map(|j| i + 1 + j));
    let first_colon = arg.find(':');
    let first_space = first_whitespace(arg);
    let first_at = arg.find('@');

    let space_only_after_hash =
        first_space.is_none() || first_hash.is_some_and(|h| first_space.is_some_and(|s| s > h));
    let at_only_after_hash =
        first_at.is_none() || first_hash.is_some_and(|h| first_at.is_some_and(|a| a > h));
    let colon_only_after_hash =
        first_colon.is_none() || first_hash.is_some_and(|h| first_colon.is_some_and(|c| c > h));
    let second_slash_only_after_hash =
        second_slash.is_none() || first_hash.is_some_and(|h| second_slash.is_some_and(|s| s > h));
    let has_slash = first_slash.is_some_and(|i| i > 0);
    let does_not_end_with_slash = match first_hash {
        Some(h) => arg.as_bytes().get(h.wrapping_sub(1)) != Some(&b'/'),
        None => !arg.ends_with('/'),
    };
    let does_not_start_with_dot = !arg.starts_with('.');

    space_only_after_hash
        && has_slash
        && does_not_end_with_slash
        && does_not_start_with_dot
        && at_only_after_hash
        && colon_only_after_hash
        && second_slash_only_after_hash
}

/// upstream `#protocols` table (index.js) plus the per-host shortcut
/// protocols added by `addHost`. Only the `hasOwnProperty` membership and the
/// `auth` flag of `git:`-style entries matter for `fromUrl`.
fn known_protocol(proto: &str) -> bool {
    matches!(
        proto,
        "git+ssh:"
            | "ssh:"
            | "git+https:"
            | "git:"
            | "http:"
            | "https:"
            | "git+http:"
            | "github:"
            | "gitlab:"
            | "bitbucket:"
            | "gist:"
            | "sourcehut:"
    )
}

/// upstream `correctProtocol` (parse-url.js).
fn correct_protocol(arg: &str) -> String {
    let first_colon = arg.find(':');
    let Some(first_colon) = first_colon else {
        // JS: firstColon = -1 → proto = "" → not known; substr(-1,3) never
        // "://"; firstAt(-1) > firstColon(-1) is false → `//` + arg.
        return format!("//{arg}");
    };
    let proto = &arg[..first_colon + 1];
    if known_protocol(proto) {
        return arg.to_string();
    }
    if arg[first_colon..].starts_with("://") {
        return arg.to_string();
    }
    let first_at = arg.find('@');
    if let Some(first_at) = first_at {
        if first_at > first_colon {
            // <foo>:<bar>@<baz>: assume git+ssh URL.
            return format!("git+ssh://{arg}");
        }
        // 'git@github.com:user/repo.git' shape: leave as-is.
        return arg.to_string();
    }
    format!("{proto}//{}", &arg[first_colon + 1..])
}

/// JS `String.prototype.lastIndexOf(char, position)` with `Infinity` handled.
fn last_index_of_before(s: &str, needle: char, before: Option<usize>) -> Option<usize> {
    let limit = before.unwrap_or(usize::MAX);
    s.char_indices()
        .rev()
        .find(|(i, c)| *c == needle && *i <= limit)
        .map(|(i, _)| i)
}

/// upstream `correctUrl` (parse-url.js): repairs scp-style URLs so the
/// WHATWG parser accepts them.
fn correct_url(giturl: &str) -> String {
    // JS lastIndexOfBefore(..., '#'): indexOf('#') is -1 when absent and the
    // limit becomes +Infinity; `firstAt` of -1 means "no @".
    let first_at = last_index_of_before(giturl, '@', giturl.find('#'));
    let first_at_signed = first_at.map_or(-1i64, |i| i as i64);

    let mut giturl = giturl.to_string();
    if let Some(colon) = last_index_of_before(&giturl, ':', giturl.find('#')) {
        if (colon as i64) > first_at_signed {
            // Replace the last ':' before the hash with '/'.
            giturl.replace_range(colon..colon + 1, "/");
        }
    }
    let last_colon_before_hash = last_index_of_before(&giturl, ':', giturl.find('#'));
    if last_colon_before_hash.is_none() && !giturl.contains("//") {
        giturl = format!("git+ssh://{giturl}");
    }
    giturl
}

/// upstream `safeUrl`: WHATWG parse, `None` on failure.
fn safe_url(u: &str) -> Option<Url> {
    Url::parse(u).ok()
}

struct ParsedUrl {
    protocol: String,
    hostname: String,
    pathname: String,
    /// Fragment including the leading `#`, or `""` (JS `url.hash`).
    hash: String,
}

/// upstream `parseUrl(giturl, protocols)`.
fn parse_url_str(giturl: &str) -> Option<ParsedUrl> {
    let with_protocol = correct_protocol(giturl);
    let parsed = safe_url(&with_protocol).or_else(|| safe_url(&correct_url(&with_protocol)))?;
    Some(ParsedUrl {
        protocol: format!("{}:", parsed.scheme()),
        hostname: parsed.host_str().unwrap_or("").to_string(),
        pathname: parsed.path().to_string(),
        hash: parsed
            .fragment()
            .map(|f| format!("#{f}"))
            .unwrap_or_default(),
    })
}

/// Extracted segments from a host's `extract` (from-url.js). `user` being
/// absent is distinct from `""` (JS `undefined` vs `""`), and `committish`
/// absent is JS `undefined`.
struct Segments {
    user: Option<Option<String>>,
    project: Option<String>,
    committish: Option<String>,
}

fn split_limited(pathname: &str, sep: char, limit: usize) -> Vec<Option<String>> {
    // JS `pathname.split(sep, limit)` then destructuring: missing slots are
    // `undefined`, present slots are strings (possibly "").
    pathname
        .splitn(limit, sep)
        .map(|s| Some(s.to_string()))
        .chain(std::iter::repeat(None))
        .take(limit)
        .collect()
}

fn strip_git_suffix(project: &str) -> String {
    project.strip_suffix(".git").unwrap_or(project).to_string()
}

/// JS `url.hash.slice(1)` — the hash always carries a leading `#` or is "".
fn hash_value(hash: &str) -> &str {
    hash.strip_prefix('#').unwrap_or("")
}

fn extract_github(parsed: &ParsedUrl) -> Option<Segments> {
    let parts = split_limited(&parsed.pathname, '/', 5);
    let user = parts.get(1).cloned().flatten();
    let project = parts.get(2).cloned().flatten();
    let type_ = parts.get(3).cloned().flatten();
    let mut committish = parts.get(4).cloned().flatten();
    // JS `if (type && type !== 'tree')` — "" is falsy too.
    if type_
        .as_deref()
        .is_some_and(|t| !t.is_empty() && t != "tree")
    {
        return None;
    }
    // JS `if (!type)` — undefined or "".
    if type_.as_deref().is_none_or(str::is_empty) {
        committish = Some(hash_value(&parsed.hash).to_string());
    }
    let project = project.map(|p| strip_git_suffix(&p));
    match (user, project) {
        (Some(user), Some(project)) if !user.is_empty() && !project.is_empty() => Some(Segments {
            user: Some(Some(user)),
            project: Some(project),
            committish,
        }),
        _ => None,
    }
}

fn extract_bitbucket(parsed: &ParsedUrl) -> Option<Segments> {
    let parts = split_limited(&parsed.pathname, '/', 4);
    let user = parts.get(1).cloned().flatten();
    let project = parts.get(2).cloned().flatten();
    let aux = parts.get(3).cloned().flatten();
    if aux.as_deref() == Some("get") {
        return None;
    }
    let project = project.map(|p| strip_git_suffix(&p));
    match (user, project) {
        (Some(user), Some(project)) if !user.is_empty() && !project.is_empty() => Some(Segments {
            user: Some(Some(user)),
            project: Some(project),
            committish: Some(hash_value(&parsed.hash).to_string()),
        }),
        _ => None,
    }
}

fn extract_gitlab(parsed: &ParsedUrl) -> Option<Segments> {
    let path = &parsed.pathname[1.min(parsed.pathname.len())..];
    if path.contains("/-/") || path.contains("/archive.tar.gz") {
        return None;
    }
    let mut segments: Vec<&str> = path.split('/').collect();
    let project = segments.pop().unwrap_or("");
    let project = strip_git_suffix(project);
    let user = segments.join("/");
    if user.is_empty() || project.is_empty() {
        return None;
    }
    Some(Segments {
        user: Some(Some(user)),
        project: Some(project),
        committish: Some(hash_value(&parsed.hash).to_string()),
    })
}

fn extract_gist(parsed: &ParsedUrl) -> Option<Segments> {
    let parts = split_limited(&parsed.pathname, '/', 4);
    let mut user = parts.get(1).cloned().flatten();
    let mut project = parts.get(2).cloned().flatten();
    let aux = parts.get(3).cloned().flatten();
    if aux.as_deref() == Some("raw") {
        return None;
    }
    // JS `if (!project) { if (!user) return; project = user; user = null }`:
    // both undefined and "" are falsy.
    if project.as_deref().is_none_or(str::is_empty) {
        if user.as_deref().is_none_or(str::is_empty) {
            return None;
        }
        project = user.take();
    }
    let project = project.map(|p| strip_git_suffix(&p))?;
    Some(Segments {
        user: Some(user),
        project: Some(project),
        committish: Some(hash_value(&parsed.hash).to_string()),
    })
}

fn extract_sourcehut(parsed: &ParsedUrl) -> Option<Segments> {
    let parts = split_limited(&parsed.pathname, '/', 4);
    let user = parts.get(1).cloned().flatten();
    let project = parts.get(2).cloned().flatten();
    let aux = parts.get(3).cloned().flatten();
    if aux.as_deref() == Some("archive") {
        return None;
    }
    let project = project.map(|p| strip_git_suffix(&p));
    match (user, project) {
        (Some(user), Some(project)) if !user.is_empty() && !project.is_empty() => Some(Segments {
            user: Some(Some(user)),
            project: Some(project),
            committish: Some(hash_value(&parsed.hash).to_string()),
        }),
        _ => None,
    }
}

fn extract(host: &HostTable, parsed: &ParsedUrl) -> Option<Segments> {
    match host.name {
        "github" => extract_github(parsed),
        "bitbucket" => extract_bitbucket(parsed),
        "gitlab" => extract_gitlab(parsed),
        "gist" => extract_gist(parsed),
        "sourcehut" => extract_sourcehut(parsed),
        _ => None,
    }
}

/// upstream `hostedGitInfo.fromUrl(giturl)` (no options — the `git.ts` call
/// sites pass none).
pub fn from_url(giturl: &str) -> Option<HostedInfo> {
    if giturl.is_empty() {
        return None;
    }

    let corrected_url = if is_github_shorthand(giturl) {
        format!("github:{giturl}")
    } else {
        giturl.to_string()
    };
    let parsed = parse_url_str(&corrected_url)?;

    let git_host_shortcut = by_shortcut(&parsed.protocol);
    let git_host_domain = by_domain(&parsed.hostname);
    let host = git_host_shortcut.or(git_host_domain)?;

    let mut user: Option<String> = None;
    let mut project: Option<String> = None;
    let mut committish: Option<String> = None;

    // JS wraps the body in try/catch returning undefined on URIError.
    let decode_step: Option<()> = (|| {
        if git_host_shortcut.is_some() {
            let mut pathname = parsed.pathname.clone();
            if pathname.starts_with('/') {
                pathname = pathname[1..].to_string();
            }
            if let Some(first_at) = pathname.find('@') {
                // auth is ignored for shortcuts; trim it out
                pathname = pathname[first_at + 1..].to_string();
            }
            if let Some(last_slash) = pathname.rfind('/') {
                let decoded = decode_uri_component(&pathname[..last_slash])?;
                // nulls only, never empty strings
                user = Some(decoded).filter(|u| !u.is_empty());
                project = Some(decode_uri_component(&pathname[last_slash + 1..])?);
            } else {
                project = Some(decode_uri_component(&pathname)?);
            }
            if project.as_deref().is_some_and(|p| p.ends_with(".git")) {
                let stripped = project.as_deref().unwrap_or_default();
                project = Some(stripped[..stripped.len() - 4].to_string());
            }
            if !parsed.hash.is_empty() {
                committish = Some(decode_uri_component(hash_value(&parsed.hash))?);
            }
        } else {
            if !host.protocols.contains(&parsed.protocol.as_str()) {
                return None;
            }
            let segments = extract(host, &parsed)?;
            // user = segments.user && decodeURIComponent(segments.user)
            user = match &segments.user {
                None => None,
                Some(None) => None,
                Some(Some(u)) if u.is_empty() => Some(String::new()),
                Some(Some(u)) => Some(decode_uri_component(u)?),
            };
            project = match &segments.project {
                Some(p) => Some(decode_uri_component(p)?),
                // JS: decodeURIComponent(undefined) coerces to "undefined".
                None => Some("undefined".to_string()),
            };
            // JS: decodeURIComponent(undefined) coerces to the string
            // "undefined" — the oracle pins that.
            committish = match &segments.committish {
                Some(s) => Some(decode_uri_component(s)?),
                None => Some("undefined".to_string()),
            };
        }
        Some(())
    })();

    // Either a URIError-equivalent decode failure or an unknown-host early
    // return.
    decode_step?;

    Some(HostedInfo {
        host_type: host.name,
        domain: host.domain,
        user,
        project,
        committish,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    fn encode(info: &HostedInfo) -> String {
        [
            info.domain.to_string(),
            info.user.clone().unwrap_or_default(),
            info.project.clone().unwrap_or_default(),
            info.committish.clone().unwrap_or_default(),
        ]
        .join("|")
    }

    #[test]
    fn matches_vendored_from_url_byte_for_byte() {
        for (url, expected) in oracle::HOSTED_FROM_URL {
            let got = from_url(url)
                .map(|i| encode(&i))
                .unwrap_or_else(|| "null".to_string());
            assert_eq!(got, *expected, "fromUrl({url:?})");
        }
    }

    #[test]
    fn github_shorthand_detection_matches_oracle_flows() {
        assert!(is_github_shorthand("user/repo"));
        assert!(is_github_shorthand("user/repo#v1"));
        assert!(!is_github_shorthand("git:git@github.com:user/repo"));
        assert!(!is_github_shorthand("github.com/user/repo.GIT"));
        assert!(!is_github_shorthand("github.com/user/repo#"));
        assert!(!is_github_shorthand("https://github.com/user/repo"));
    }

    #[test]
    fn decode_uri_component_is_strict() {
        assert_eq!(
            decode_uri_component("user%2Frepo"),
            Some("user/repo".to_string())
        );
        assert_eq!(decode_uri_component("%25"), Some("%".to_string()));
        assert_eq!(decode_uri_component("a+b"), Some("a+b".to_string()));
        assert_eq!(decode_uri_component("%E0%A4%A"), None);
        assert_eq!(decode_uri_component("%ZZ"), None);
    }
}
