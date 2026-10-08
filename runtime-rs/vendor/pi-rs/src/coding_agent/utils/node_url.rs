//! Support port: node's `fileURLToPath` (node:url), used by [`crate::coding_agent::utils::paths`].
//!
//! Faithful to node's algorithm (lib/internal/url.js), including the error
//! messages the oracle captured:
//! - `URI malformed` for percent sequences that do not decode to valid UTF-8
//!   (node re-throws the `decodeURIComponent` URIError),
//! - `File URL path must be absolute` when the pathname has no drive root on
//!   Windows / no leading slash on POSIX,
//! - `File URL path must not include encoded / or \ characters`.
//!
//! Divergence: node raises `ERR_INVALID_URL_SCHEME` (with the offending
//! scheme interpolated) for non-file URLs; this port raises
//! [`FileUrlError::Scheme`] with a fixed message because the upstream
//! callers only ever pass `file://` inputs.

use url::Url;

/// Error raised by [`file_url_to_path`]; `Display` matches the captured node
/// messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileUrlError {
    Scheme,
    EncodedSlash,
    NotAbsolute,
    BadHost,
    UriMalformed,
}

impl std::fmt::Display for FileUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scheme => write!(f, "File URL scheme must be 'file'"),
            Self::EncodedSlash => write!(
                f,
                "File URL path must not include encoded / or \\ characters"
            ),
            Self::NotAbsolute => write!(f, "File URL path must be absolute"),
            Self::BadHost => write!(f, "File URL host must be 'localhost' or empty"),
            Self::UriMalformed => write!(f, "URI malformed"),
        }
    }
}

impl std::error::Error for FileUrlError {}

fn percent_decode(input: &str) -> Result<String, FileUrlError> {
    let bytes = percent_decode_bytes(input).ok_or(FileUrlError::UriMalformed)?;
    String::from_utf8(bytes).map_err(|_| FileUrlError::UriMalformed)
}

/// Decode `%xx` sequences; leaves other bytes untouched. Returns `None` for
/// malformed sequences (mirrors `decodeURIComponent` throwing URIError).
fn percent_decode_bytes(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
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
    Some(out)
}

fn contains_encoded_slash(pathname: &str) -> bool {
    let bytes = pathname.as_bytes();
    for n in 0..bytes.len() {
        if bytes[n] != b'%' {
            continue;
        }
        // Node checks the two chars after '%': 2f / 5c, case-insensitive.
        let second = bytes.get(n + 1).map(|b| b.to_ascii_lowercase());
        let third = bytes.get(n + 2).map(|b| b.to_ascii_lowercase());
        if let (Some(b'2'), Some(b'f')) = (second, third) {
            return true;
        }
        if let (Some(b'5'), Some(b'c')) = (second, third) {
            return true;
        }
    }
    false
}

/// node `fileURLToPath`. `windows` selects the platform branch; callers pass
/// `cfg!(windows)` (node dispatches on the running platform).
///
/// Ported against the exact `internal/url.js` embedded in the local node
/// v25.8.2 (`getPathFromURLWin32` / `getPathFromURLPosix`, extracted to
/// `tests/fixtures/utils_oracle/node_internal_url_source.js`): the encoded-slash
/// check runs on the raw pathname, the win32 branch converts `/` → `\`
/// before decoding, decodes before the drive check, and the posix branch
/// checks the hostname before the encoded-slash scan.
///
/// Divergences: node re-throws the `decodeURIComponent` URIError (pinned as
/// [`FileUrlError::UriMalformed`]); non-file URLs raise
/// [`FileUrlError::Scheme`] instead of `ERR_INVALID_URL_SCHEME`; win32 UNC
/// hostnames are not passed through `domainToUnicode` (the callers only ever
/// feed ASCII `pathToFileURL` output); and the posix branch keeps the older
/// "localhost is allowed" rule the rest of this port was written against.
pub fn file_url_to_path(input: &str, windows: bool) -> Result<String, FileUrlError> {
    let url = Url::parse(input).map_err(|_| FileUrlError::Scheme)?;
    if url.scheme() != "file" {
        return Err(FileUrlError::Scheme);
    }

    let hostname = url.host_str().unwrap_or("");
    let pathname = url.path();

    if windows {
        if contains_encoded_slash(pathname) {
            return Err(FileUrlError::EncodedSlash);
        }
        // node replaces `/` with `\` before decoding.
        let converted = pathname.replace('/', "\\");
        let decoded = if converted.contains('%') {
            percent_decode(&converted)?
        } else {
            converted
        };
        if !hostname.is_empty() {
            // If hostname is set, then we have a UNC path.
            return Ok(format!("\\\\{hostname}{decoded}"));
        }
        // Otherwise, it's a local path that requires a drive letter.
        let chars: Vec<char> = decoded.chars().collect();
        let letter = chars.get(1).copied().unwrap_or('\0').to_ascii_lowercase();
        let sep = chars.get(2).copied().unwrap_or('\0');
        if !letter.is_ascii_lowercase() || sep != ':' {
            return Err(FileUrlError::NotAbsolute);
        }
        Ok(chars[1..].iter().collect())
    } else {
        if !hostname.is_empty() && hostname != "localhost" {
            return Err(FileUrlError::BadHost);
        }
        let encoded_slash_only = {
            // posix checks encoded `/` only (not `\`).
            let bytes = pathname.as_bytes();
            let mut found = false;
            for n in 0..bytes.len() {
                if bytes[n] != b'%' {
                    continue;
                }
                let second = bytes.get(n + 1).map(|b| b.to_ascii_lowercase());
                let third = bytes.get(n + 2).map(|b| b.to_ascii_lowercase());
                if let (Some(b'2'), Some(b'f')) = (second, third) {
                    found = true;
                    break;
                }
            }
            found
        };
        if encoded_slash_only {
            return Err(FileUrlError::EncodedSlash);
        }
        let decoded = percent_decode(pathname)?;
        if !decoded.starts_with('/') {
            return Err(FileUrlError::NotAbsolute);
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_drive_paths() {
        assert_eq!(
            file_url_to_path("file:///C:/dir/file.txt", true).expect("ok"),
            "C:\\dir\\file.txt"
        );
        assert_eq!(
            file_url_to_path("file:///C:/dir/file%20with%20spaces.txt", true).expect("ok"),
            "C:\\dir\\file with spaces.txt"
        );
    }

    #[test]
    fn win32_rejects_driveless_and_bad_percent() {
        assert_eq!(
            file_url_to_path("file:///dir/file.txt", true),
            Err(FileUrlError::NotAbsolute)
        );
        assert_eq!(
            file_url_to_path("file:///C:/bad/%E0%A4%A", true),
            Err(FileUrlError::UriMalformed)
        );
    }

    #[test]
    fn win32_unc() {
        assert_eq!(
            file_url_to_path("file://server/share/file.txt", true).expect("ok"),
            "\\\\server\\share\\file.txt"
        );
    }

    #[test]
    fn posix_paths() {
        assert_eq!(
            file_url_to_path("file:///dir/file.txt", false).expect("ok"),
            "/dir/file.txt"
        );
        assert_eq!(
            file_url_to_path("file:///C:/dir/file.txt", false).expect("ok"),
            "/C:/dir/file.txt"
        );
        assert_eq!(
            file_url_to_path("file:///C:/bad/%E0%A4%A", false),
            Err(FileUrlError::UriMalformed)
        );
        assert_eq!(
            file_url_to_path("file://other/dir", false),
            Err(FileUrlError::BadHost)
        );
        assert_eq!(
            file_url_to_path("file://localhost/dir", false).expect("ok"),
            "/dir"
        );
    }

    #[test]
    fn rejects_encoded_slashes_and_bad_scheme() {
        assert_eq!(
            file_url_to_path("file:///C:/a%2Fb", true),
            Err(FileUrlError::EncodedSlash)
        );
        assert_eq!(
            file_url_to_path("file:///C:/a%5cb", true),
            Err(FileUrlError::EncodedSlash)
        );
        assert_eq!(
            file_url_to_path("https://x/y", true),
            Err(FileUrlError::Scheme)
        );
    }
}
