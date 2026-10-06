use std::path::Path;

/// Node-facing text uses ordinary Windows absolute paths. Keep canonical PathBufs
/// for filesystem operations and containment checks, including long-path prefixes.
pub(crate) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        regular_windows_path(&value)
    }
    #[cfg(not(windows))]
    {
        value.into_owned()
    }
}

#[cfg(any(windows, test))]
pub(crate) fn regular_windows_path(value: &str) -> String {
    let Some(rest) = value.strip_prefix(r"\\?\") else {
        return value.to_owned();
    };
    let bytes = rest.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return rest.to_owned();
    }
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        let mut components = unc.split('\\');
        if components.next().is_some_and(|server| !server.is_empty())
            && components.next().is_some_and(|share| !share.is_empty())
        {
            return format!(r"\\{unc}");
        }
    }
    // Device namespaces, malformed UNC paths and other verbatim forms must not
    // become a different relative or local path by blindly removing the prefix.
    value.to_owned()
}
