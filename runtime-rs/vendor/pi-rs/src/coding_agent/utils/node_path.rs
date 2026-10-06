//! Support port: node's `path` module (lib/path.js, win32 + posix flavors)
//! as required by [`crate::coding_agent::utils::paths`]: `resolve`, `relative`,
//! `join`, `normalize`, `isAbsolute`.
//!
//! This is a behavioral port of the exact `path.js` embedded in the local
//! node v25.8.2 binary (extracted via `process.binding('natives')` into
//! `tests/fixtures/utils_oracle/node_path_source.js`, © Joyent / Node contributors,
//! MIT), including the win32 UNC/device-root parsing, the
//! `isWindowsReservedName` handling and the CVE-2024-36139 colon checks in
//! `win32.normalize`, and the case-length-mismatch branch of
//! `win32.relative`.
//!
//! Validated against the captured node oracle (45-case resolve grid, 20-case
//! relative grid, join/normalize probes for both flavors — see
//! `tests/fixtures/utils_oracle/`); the cwd-dependent oracle entries are asserted
//! against the process cwd instead of pinned values. node indexes UTF-16
//! code units; this port indexes `char`s (equivalent for BMP paths).
//!
//! Windows-only per-drive cwd (`env['=<drive>':]`) is reproduced via
//! `std::env::var("=<drive>:")`; `process.cwd()` is passed in by callers.

const CHAR_DOT: char = '.';
const CHAR_COLON: char = ':';
const CHAR_BACKWARD_SLASH: char = '\\';
const CHAR_FORWARD_SLASH: char = '/';

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

fn is_path_separator(code: char) -> bool {
    code == CHAR_FORWARD_SLASH || code == CHAR_BACKWARD_SLASH
}

fn is_posix_path_separator(code: char) -> bool {
    code == CHAR_FORWARD_SLASH
}

/// JS `isWindowsReservedName(path, colonIndex)`.
fn is_windows_reserved_name(chars: &[char], colon_index: isize) -> bool {
    // JS `slice(0, colonIndex)` with a negative end drops from the tail.
    let end = if colon_index < 0 {
        chars.len().saturating_sub((-colon_index) as usize)
    } else {
        (colon_index as usize).min(chars.len())
    };
    let device_part: String = chars[..end].iter().collect::<String>().to_uppercase();
    WINDOWS_RESERVED_NAMES.contains(&device_part.as_str())
}

fn is_windows_device_root(code: char) -> bool {
    code.is_ascii_alphabetic()
}

fn current_dir_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| String::from("."))
}

/// JS `String.prototype.slice(start, end)` including negative indices.
fn js_slice_chars(chars: &[char], start: isize, end: isize) -> String {
    let len = chars.len() as isize;
    let resolve = |mut v: isize| -> usize {
        if v < 0 {
            v += len;
            if v < 0 {
                v = 0;
            }
        }
        (v as usize).min(chars.len())
    };
    let s = resolve(start);
    let e = resolve(end);
    if s >= e {
        String::new()
    } else {
        chars[s..e].iter().collect()
    }
}

fn chars_of(path: &str) -> Vec<char> {
    path.chars().collect()
}

/// node `normalizeString` (lib/path.js), shared by both flavors.
pub(crate) fn normalize_string(
    path: &str,
    allow_above_root: bool,
    separator: char,
    is_sep: fn(char) -> bool,
) -> String {
    let chars = chars_of(path);
    let mut res = String::new();
    let mut last_segment_length: usize = 0;
    let mut last_slash: isize = -1;
    let mut dots: i32 = 0;
    let mut code: char = '\0';
    let len = chars.len();

    for i in 0..=len {
        if i < len {
            code = chars[i];
        } else if is_sep(code) {
            break;
        } else {
            code = CHAR_FORWARD_SLASH;
        }

        if is_sep(code) {
            if last_slash == i as isize - 1 || dots == 1 {
                // NOOP
            } else if dots == 2 {
                let res_chars = chars_of(&res);
                let not_double_dot = res_chars.len() < 2
                    || last_segment_length != 2
                    || res_chars[res_chars.len() - 1] != CHAR_DOT
                    || res_chars[res_chars.len() - 2] != CHAR_DOT;
                if not_double_dot {
                    if res_chars.len() > 2 {
                        let last_slash_index =
                            res_chars.len() as isize - last_segment_length as isize - 1;
                        if last_slash_index == -1 {
                            res = String::new();
                            last_segment_length = 0;
                        } else {
                            res = js_slice_chars(&res_chars, 0, last_slash_index);
                            last_segment_length =
                                res.rfind(separator).map_or(res.chars().count(), |index| {
                                    res.chars().count() - 1 - index
                                });
                        }
                        last_slash = i as isize;
                        dots = 0;
                        continue;
                    } else if !res.is_empty() {
                        res = String::new();
                        last_segment_length = 0;
                        last_slash = i as isize;
                        dots = 0;
                        continue;
                    }
                }
                if allow_above_root {
                    if res.is_empty() {
                        res.push_str("..");
                    } else {
                        res.push(separator);
                        res.push_str("..");
                    }
                    last_segment_length = 2;
                }
            } else {
                let segment = js_slice_chars(&chars, last_slash + 1, i as isize);
                if res.is_empty() {
                    res = segment;
                } else {
                    res.push(separator);
                    res.push_str(&segment);
                }
                last_segment_length = (i as isize - last_slash - 1) as usize;
            }
            last_slash = i as isize;
            dots = 0;
        } else if code == CHAR_DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
    }
    res
}

// ------------------------------------------------------------------ posix ---

/// node `path.posix.resolve` over `args` with `cwd` as the fallback base
/// (`process.cwd()`, already posix-formatted by the caller).
pub fn posix_resolve(args: &[&str], cwd: &str) -> String {
    // node fast path: no args, or a single empty/"." arg with an absolute cwd.
    let cwd_is_absolute = cwd.starts_with(CHAR_FORWARD_SLASH);
    if args.is_empty()
        || (args.len() == 1 && (args[0].is_empty() || args[0] == ".") && cwd_is_absolute)
    {
        return cwd.to_string();
    }

    let mut resolved_path = String::new();
    let mut resolved_absolute = false;

    for path in args.iter().rev() {
        if resolved_absolute {
            break;
        }
        if path.is_empty() {
            continue;
        }
        resolved_path = format!("{path}/{resolved_path}");
        resolved_absolute = path.starts_with(CHAR_FORWARD_SLASH);
    }

    if !resolved_absolute {
        resolved_path = format!("{cwd}/{resolved_path}");
        resolved_absolute = cwd_is_absolute;
    }

    let normalized = normalize_string(
        &resolved_path,
        !resolved_absolute,
        CHAR_FORWARD_SLASH,
        is_posix_path_separator,
    );
    if resolved_absolute {
        return format!("/{normalized}");
    }
    if !normalized.is_empty() {
        return normalized;
    }
    String::from(".")
}

/// node `path.posix.join`.
pub fn posix_join(args: &[&str]) -> String {
    if args.is_empty() {
        return String::from(".");
    }
    let parts: Vec<&str> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
    if parts.is_empty() {
        return String::from(".");
    }
    posix_normalize(&parts.join("/"))
}

/// node `path.posix.normalize`.
pub fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return String::from(".");
    }
    let is_absolute = path.starts_with(CHAR_FORWARD_SLASH);
    let trailing_separator = path.ends_with(CHAR_FORWARD_SLASH);

    let normalized = normalize_string(
        path,
        !is_absolute,
        CHAR_FORWARD_SLASH,
        is_posix_path_separator,
    );

    if normalized.is_empty() {
        if is_absolute {
            return String::from("/");
        }
        return if trailing_separator {
            String::from("./")
        } else {
            String::from(".")
        };
    }
    let mut normalized = normalized;
    if trailing_separator {
        normalized.push('/');
    }
    if is_absolute {
        return format!("/{normalized}");
    }
    normalized
}

/// node `path.posix.isAbsolute`.
pub fn posix_is_absolute(path: &str) -> bool {
    !path.is_empty() && path.starts_with(CHAR_FORWARD_SLASH)
}

/// node `path.posix.relative`. `from`/`to` are resolved against `cwd`
/// (`process.cwd()`, posix-formatted) exactly like node.
pub fn posix_relative(from: &str, to: &str, cwd: &str) -> String {
    if from == to {
        return String::new();
    }

    let from = posix_resolve(&[from], cwd);
    let to = posix_resolve(&[to], cwd);
    if from == to {
        return String::new();
    }

    let from_chars = chars_of(&from);
    let to_chars = chars_of(&to);

    let from_start = 1usize;
    let from_end = from_chars.len();
    let from_len = from_end - from_start;
    let to_start = 1usize;
    let to_len = to_chars.len() - to_start;

    let length = from_len.min(to_len);
    let mut last_common_sep: isize = -1;
    let mut i = 0usize;
    while i < length {
        let from_code = from_chars[from_start + i];
        if from_code != to_chars[to_start + i] {
            break;
        } else if from_code == CHAR_FORWARD_SLASH {
            last_common_sep = i as isize;
        }
        i += 1;
    }
    if i == length {
        if to_len > length {
            if to_chars[to_start + i] == CHAR_FORWARD_SLASH {
                return js_slice_chars(&to_chars, (to_start + i + 1) as isize, isize::MAX);
            }
            if i == 0 {
                return js_slice_chars(&to_chars, to_start as isize, isize::MAX);
            }
        } else if from_len > length {
            if from_chars[from_start + i] == CHAR_FORWARD_SLASH {
                last_common_sep = i as isize;
            } else if i == 0 {
                last_common_sep = 0;
            }
        }
    }

    let mut out = String::new();
    let mut j = from_start as isize + last_common_sep + 1;
    while j <= from_end as isize {
        if j == from_end as isize || from_chars[j as usize] == CHAR_FORWARD_SLASH {
            out.push_str(if out.is_empty() { ".." } else { "/.." });
        }
        j += 1;
    }

    format!(
        "{out}{}",
        js_slice_chars(&to_chars, to_start as isize + last_common_sep, isize::MAX)
    )
}

// ------------------------------------------------------------------ win32 ---

/// Per-drive cwd lookup (`env['=<drive>:'] || process.cwd()`), mirroring
/// node's win32 resolve fallback for drive-relative arguments.
fn per_device_cwd(device: &str) -> Option<String> {
    let env_key = format!("={device}");
    std::env::var_os(&env_key)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
}

/// node's per-path root parse inside `win32.resolve`:
/// returns `(device, rootEnd, isAbsolute)`.
fn parse_win32_root(path: &[char]) -> (String, usize, bool) {
    let len = path.len();
    let mut root_end = 0usize;
    let mut device = String::new();
    let mut is_absolute = false;
    let code = path[0];

    if len == 1 {
        // `path` contains just a path separator
        if is_path_separator(code) {
            root_end = 1;
            is_absolute = true;
        }
    } else if is_path_separator(code) {
        // Possible UNC root. A leading separator means we at least have an
        // absolute path of some kind (UNC or otherwise).
        is_absolute = true;
        if is_path_separator(path[1]) {
            // Matched double path separator at beginning
            let mut j = 2usize;
            let mut last = 2usize;
            // Match 1 or more non-path separators
            while j < len && !is_path_separator(path[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part: String = js_slice_chars(path, last as isize, j as isize);
                // Matched!
                last = j;
                // Match 1 or more path separators
                while j < len && is_path_separator(path[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    // Matched!
                    last = j;
                    // Match 1 or more non-path separators
                    while j < len && !is_path_separator(path[j]) {
                        j += 1;
                    }
                    if j == len || j != last {
                        if first_part != "." && first_part != "?" {
                            // We matched a UNC root
                            device = format!(
                                "\\\\{first_part}\\{}",
                                js_slice_chars(path, last as isize, j as isize)
                            );
                            root_end = j;
                        } else {
                            // We matched a device root (e.g. \\.\PHYSICALDRIVE0)
                            device = format!("\\\\{first_part}");
                            root_end = 4;
                        }
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if is_windows_device_root(code) && len > 1 && path[1] == CHAR_COLON {
        // Possible device root
        device = js_slice_chars(path, 0, 2);
        root_end = 2;
        if len > 2 && is_path_separator(path[2]) {
            // Treat separator following drive name as an absolute path
            // indicator
            is_absolute = true;
            root_end = 3;
        }
    }
    (device, root_end, is_absolute)
}

/// node `path.win32.resolve` over `args` with `cwd` as the fallback base
/// (`process.cwd()`).
pub fn win32_resolve(args: &[&str], cwd: &str) -> String {
    let mut resolved_device = String::new();
    let mut resolved_tail = String::new();
    let mut resolved_absolute = false;

    // One iteration of node's resolve loop body; returns `true` when the
    // loop breaks.
    fn feed(
        path_str: &str,
        resolved_device: &mut String,
        resolved_tail: &mut String,
        resolved_absolute: &mut bool,
    ) -> bool {
        let path = chars_of(path_str);
        let (device, root_end, is_absolute) = parse_win32_root(&path);

        if !device.is_empty() {
            if !resolved_device.is_empty() {
                if device.to_lowercase() != resolved_device.to_lowercase() {
                    // This path points to another device so it is not
                    // applicable
                    return false;
                }
            } else {
                *resolved_device = device;
            }
        }

        if *resolved_absolute {
            if !resolved_device.is_empty() {
                return true;
            }
        } else {
            let tail = js_slice_chars(&path, root_end as isize, isize::MAX);
            *resolved_tail = format!("{tail}\\{resolved_tail}");
            *resolved_absolute = is_absolute;
            if is_absolute && !resolved_device.is_empty() {
                return true;
            }
        }
        false
    }

    for path_arg in args.iter().rev() {
        if path_arg.is_empty() {
            continue;
        }
        if feed(
            path_arg,
            &mut resolved_device,
            &mut resolved_tail,
            &mut resolved_absolute,
        ) {
            break;
        }
    }

    // i == -1: fall back to the (per-device) cwd.
    let cwd_path: String = if resolved_device.is_empty() {
        // node fast path for current directory
        let cwd_starts_sep = cwd.chars().next().is_some_and(is_path_separator);
        if args.is_empty()
            || (args.len() == 1 && (args[0].is_empty() || args[0] == ".") && cwd_starts_sep)
        {
            return cwd.to_string();
        }
        cwd.to_string()
    } else {
        // Windows has drive-specific cwds: get the cwd for that drive, or
        // the process cwd when unavailable. If it does not point at our
        // drive, default to the drive root.
        let candidate = per_device_cwd(&resolved_device).unwrap_or_else(current_dir_string);
        let candidate_chars = chars_of(&candidate);
        let points_elsewhere = !cwd_matches_device(&candidate, &resolved_device)
            && candidate_chars.len() > 2
            && candidate_chars[2] == CHAR_BACKWARD_SLASH;
        if points_elsewhere {
            format!("{resolved_device}\\")
        } else {
            candidate
        }
    };

    feed(
        &cwd_path,
        &mut resolved_device,
        &mut resolved_tail,
        &mut resolved_absolute,
    );

    // Normalize the tail path
    let resolved_tail = normalize_string(
        &resolved_tail,
        !resolved_absolute,
        CHAR_BACKWARD_SLASH,
        is_path_separator,
    );

    if resolved_absolute {
        format!("{resolved_device}\\{resolved_tail}")
    } else {
        let combined = format!("{resolved_device}{resolved_tail}");
        if combined.is_empty() {
            String::from(".")
        } else {
            combined
        }
    }
}

fn cwd_matches_device(path: &str, device: &str) -> bool {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    let prefix: String = chars[..2].iter().collect();
    prefix.eq_ignore_ascii_case(device)
}

/// node `path.win32.isAbsolute`.
pub fn win32_is_absolute(path: &str) -> bool {
    let chars = chars_of(path);
    let len = chars.len();
    if len == 0 {
        return false;
    }
    let code = chars[0];
    is_path_separator(code)
        || (len > 2
            && is_windows_device_root(code)
            && chars[1] == CHAR_COLON
            && is_path_separator(chars[2]))
}

/// node `path.win32.normalize`.
pub fn win32_normalize(path: &str) -> String {
    let len = chars_of(path).len();
    if len == 0 {
        return String::from(".");
    }
    let chars = chars_of(path);
    let mut root_end = 0usize;
    let mut device: Option<String> = None;
    let mut is_absolute = false;
    let code = chars[0];

    // Try to match a root
    if len == 1 {
        // `path` contains just a single char, exit early to avoid
        // unnecessary work
        return if is_posix_path_separator(code) {
            String::from("\\")
        } else {
            path.to_string()
        };
    }
    if is_path_separator(code) {
        // Possible UNC root
        is_absolute = true;
        if is_path_separator(chars[1]) {
            // Matched double path separator at beginning
            let mut j = 2usize;
            let mut last = 2usize;
            while j < len && !is_path_separator(chars[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part: String = js_slice_chars(&chars, last as isize, j as isize);
                last = j;
                while j < len && is_path_separator(chars[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_path_separator(chars[j]) {
                        j += 1;
                    }
                    if j == len || j != last {
                        if first_part == "." || first_part == "?" {
                            // We matched a device root (e.g. \\.\PHYSICALDRIVE0)
                            device = Some(format!("\\\\{first_part}"));
                            root_end = 4;
                            let colon_index = path.find(':').map(|i| i as isize).unwrap_or(-1);
                            // Special case: \\?\COM1: style reserved device paths
                            let possible_device = js_slice_chars(&chars, 4, colon_index + 1);
                            let possible_chars = chars_of(&possible_device);
                            if is_windows_reserved_name(
                                &possible_chars,
                                possible_chars.len() as isize - 1,
                            ) {
                                device = Some(format!("\\\\?\\{possible_device}"));
                                root_end = 4 + possible_device.chars().count();
                            }
                        } else if j == len {
                            // We matched a UNC root only: return its normalized
                            // version since there is nothing left to process.
                            let tail = js_slice_chars(&chars, last as isize, isize::MAX);
                            return format!("\\\\{first_part}\\{tail}\\");
                        } else {
                            // We matched a UNC root with leftovers
                            device = Some(format!(
                                "\\\\{first_part}\\{}",
                                js_slice_chars(&chars, last as isize, j as isize)
                            ));
                            root_end = j;
                        }
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else {
        let colon_index = path.find(':');
        if let Some(colon_index) = colon_index {
            if colon_index > 0 {
                if is_windows_device_root(code) && colon_index == 1 {
                    device = Some(js_slice_chars(&chars, 0, 2));
                    root_end = 2;
                    if len > 2 && is_path_separator(chars[2]) {
                        is_absolute = true;
                        root_end = 3;
                    }
                } else if is_windows_reserved_name(&chars, colon_index as isize) {
                    device = Some(js_slice_chars(&chars, 0, colon_index as isize + 1));
                    root_end = colon_index + 1;
                }
            }
        }
    }

    let mut tail = if root_end < len {
        normalize_string(
            &js_slice_chars(&chars, root_end as isize, isize::MAX),
            !is_absolute,
            CHAR_BACKWARD_SLASH,
            is_path_separator,
        )
    } else {
        String::new()
    };
    if tail.is_empty() && !is_absolute {
        tail = String::from(".");
    }
    if !tail.is_empty() && chars.last().is_some_and(|c| is_path_separator(*c)) {
        tail.push(CHAR_BACKWARD_SLASH);
    }
    if !is_absolute && device.is_none() && path.contains(':') {
        // If the original path was not absolute and if we have not been able
        // to resolve it relative to a particular device, ensure the tail has
        // not become something Windows might interpret as an absolute path
        // (CVE-2024-36139).
        if tail.chars().count() >= 2 {
            let tail_chars = chars_of(&tail);
            if is_windows_device_root(tail_chars[0]) && tail_chars[1] == CHAR_COLON {
                return format!(".\\{tail}");
            }
        }
        let mut index = path.find(':').map(|i| i as isize).unwrap_or(-1);
        loop {
            if index == len as isize - 1
                || ((index + 1) as usize) < len && is_path_separator(chars[(index + 1) as usize])
            {
                return format!(".\\{tail}");
            }
            let next = path[(index + 1).max(0) as usize..]
                .find(':')
                .map(|i| i as isize + index + 1);
            match next {
                Some(next) if next != -1 => index = next,
                _ => break,
            }
        }
    }
    let colon_index = path.find(':').map(|i| i as isize).unwrap_or(-1);
    if is_windows_reserved_name(&chars, colon_index) {
        return format!(".\\{}{tail}", device.as_deref().unwrap_or(""));
    }
    match device {
        None => {
            if is_absolute {
                return format!("\\{tail}");
            }
            tail
        }
        Some(device) => {
            if is_absolute {
                format!("{device}\\{tail}")
            } else {
                format!("{device}{tail}")
            }
        }
    }
}

/// node `path.win32.join`.
pub fn win32_join(args: &[&str]) -> String {
    if args.is_empty() {
        return String::from(".");
    }
    let parts: Vec<&str> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
    if parts.is_empty() {
        return String::from(".");
    }

    let first_part = parts[0];
    let mut joined = parts.join("\\");

    // Make sure that the joined path doesn't start with two slashes, because
    // normalize() will mistake it for a UNC path then (unless the first part
    // clearly starts with a UNC root).
    let first_chars = chars_of(first_part);
    let mut needs_replace = true;
    let mut slash_count = 0usize;
    if let Some(&first) = first_chars.first() {
        if is_path_separator(first) {
            slash_count += 1;
            if first_chars.len() > 1 && is_path_separator(first_chars[1]) {
                slash_count += 1;
                if first_chars.len() > 2 {
                    if is_path_separator(first_chars[2]) {
                        slash_count += 1;
                    } else {
                        // We matched a UNC path in the first part
                        needs_replace = false;
                    }
                }
            }
        }
    }
    if needs_replace {
        let joined_chars = chars_of(&joined);
        while slash_count < joined_chars.len() && is_path_separator(joined_chars[slash_count]) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            joined = format!(
                "\\{}",
                js_slice_chars(&joined_chars, slash_count as isize, isize::MAX)
            );
        }
    }

    // Skip normalization when reserved device names are present.
    let joined_chars = chars_of(&joined);
    let mut split_parts: Vec<String> = Vec::new();
    let mut part = String::new();
    let mut i = 0usize;
    while i < joined_chars.len() {
        if joined_chars[i] == CHAR_BACKWARD_SLASH {
            if !part.is_empty() {
                split_parts.push(std::mem::take(&mut part));
            }
            // Skip consecutive backslashes
            while i + 1 < joined_chars.len() && joined_chars[i + 1] == CHAR_BACKWARD_SLASH {
                i += 1;
            }
        } else {
            part.push(joined_chars[i]);
        }
        i += 1;
    }
    if !part.is_empty() {
        split_parts.push(part);
    }

    let has_reserved = split_parts.iter().any(|part| {
        let part_chars = chars_of(part);
        match part.find(':') {
            Some(colon_index) => is_windows_reserved_name(&part_chars, colon_index as isize),
            None => false,
        }
    });
    if has_reserved {
        // Replace forward slashes with backslashes
        return joined_chars
            .iter()
            .map(|c| if *c == '/' { CHAR_BACKWARD_SLASH } else { *c })
            .collect();
    }

    win32_normalize(&joined)
}

/// node `path.win32.relative`.
pub fn win32_relative(from: &str, to: &str, cwd: &str) -> String {
    if from == to {
        return String::new();
    }

    let from_orig = win32_resolve(&[from], cwd);
    let to_orig = win32_resolve(&[to], cwd);
    if from_orig == to_orig {
        return String::new();
    }

    let from = from_orig.to_lowercase();
    let to = to_orig.to_lowercase();
    if from == to {
        return String::new();
    }

    if from_orig.chars().count() != from.chars().count()
        || to_orig.chars().count() != to.chars().count()
    {
        // Case-only length mismatch (e.g. Unicode folding): node falls back
        // to a split-based comparison.
        let mut from_split: Vec<String> = from_orig
            .split(CHAR_BACKWARD_SLASH)
            .map(str::to_string)
            .collect();
        let mut to_split: Vec<String> = to_orig
            .split(CHAR_BACKWARD_SLASH)
            .map(str::to_string)
            .collect();
        if from_split.last().is_some_and(|s| s.is_empty()) {
            from_split.pop();
        }
        if to_split.last().is_some_and(|s| s.is_empty()) {
            to_split.pop();
        }

        let from_len = from_split.len();
        let to_len = to_split.len();
        let length = from_len.min(to_len);

        let mut i = 0usize;
        while i < length {
            if from_split[i].to_lowercase() != to_split[i].to_lowercase() {
                break;
            }
            i += 1;
        }

        if i == 0 {
            return to_orig;
        } else if i == length {
            if to_len > length {
                return to_split[i..].join("\\");
            }
            if from_len > length {
                return format!("{}..", "..\\".repeat(from_len - 1 - i));
            }
            return String::new();
        }
        return format!(
            "{}{}",
            "..\\".repeat(from_len - i),
            to_split[i..].join("\\")
        );
    }

    let from_chars = chars_of(&from);
    let to_chars = chars_of(&to);
    let to_orig_chars = chars_of(&to_orig);

    // Trim any leading backslashes
    let mut from_start = 0usize;
    while from_start < from_chars.len() && from_chars[from_start] == CHAR_BACKWARD_SLASH {
        from_start += 1;
    }
    // Trim trailing backslashes (applicable to UNC paths only)
    let mut from_end = from_chars.len();
    while from_end > from_start + 1 && from_chars[from_end - 1] == CHAR_BACKWARD_SLASH {
        from_end -= 1;
    }
    let from_len = from_end - from_start;

    // Trim any leading backslashes
    let mut to_start = 0usize;
    while to_start < to_chars.len() && to_chars[to_start] == CHAR_BACKWARD_SLASH {
        to_start += 1;
    }
    let mut to_end = to_chars.len();
    while to_end > to_start + 1 && to_chars[to_end - 1] == CHAR_BACKWARD_SLASH {
        to_end -= 1;
    }
    let to_len = to_end - to_start;

    // Compare paths to find the longest common path from root
    let length = from_len.min(to_len);
    let mut last_common_sep: isize = -1;
    let mut i = 0usize;
    while i < length {
        let from_code = from_chars[from_start + i];
        if from_code != to_chars[to_start + i] {
            break;
        } else if from_code == CHAR_BACKWARD_SLASH {
            last_common_sep = i as isize;
        }
        i += 1;
    }

    // We found a mismatch before the first common path separator was seen, so
    // return the original `to`.
    if i != length {
        if last_common_sep == -1 {
            return to_orig;
        }
    } else {
        if to_len > length {
            if to_chars[to_start + i] == CHAR_BACKWARD_SLASH {
                // `from` is the exact base path for `to`.
                return js_slice_chars(&to_orig_chars, (to_start + i + 1) as isize, isize::MAX);
            }
            if i == 2 {
                // `from` is the device root.
                return js_slice_chars(&to_orig_chars, (to_start + i) as isize, isize::MAX);
            }
        }
        if from_len > length {
            if from_chars[from_start + i] == CHAR_BACKWARD_SLASH {
                // `to` is the exact base path for `from`.
                last_common_sep = i as isize;
            } else if i == 2 {
                // `to` is the device root.
                last_common_sep = 3;
            }
        }
        if last_common_sep == -1 {
            last_common_sep = 0;
        }
    }

    let mut out = String::new();
    // Generate the relative path based on the path difference between `to`
    // and `from`.
    let mut j = from_start as isize + last_common_sep + 1;
    while j <= from_end as isize {
        if j == from_end as isize || from_chars[j as usize] == CHAR_BACKWARD_SLASH {
            out.push_str(if out.is_empty() { ".." } else { "\\.." });
        }
        j += 1;
    }

    let mut to_start = to_start as isize + last_common_sep;

    // Append the rest of the destination (`to`) path that comes after the
    // common path parts.
    if !out.is_empty() {
        return format!(
            "{out}{}",
            js_slice_chars(&to_orig_chars, to_start, to_end as isize)
        );
    }

    if to_orig_chars[to_start as usize] == CHAR_BACKWARD_SLASH {
        to_start += 1;
    }
    js_slice_chars(&to_orig_chars, to_start, to_end as isize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    /// node's `posixCwd()` on a Windows host strips the drive and converts
    /// separators; on POSIX it is the raw cwd.
    fn posix_cwd() -> String {
        let raw = current_dir_string();
        if cfg!(windows) {
            let replaced = raw.replace(CHAR_BACKWARD_SLASH, "/");
            let index = replaced.find('/').unwrap_or(0);
            replaced[index.min(replaced.len())..].to_string()
        } else {
            raw
        }
    }

    /// environment-anchored: the win32 grids' !CWD/!REL/!DEPTH markers expand
    /// against the process cwd, and node resolves drive-relative paths
    /// through `env['=<drive>:'] || process.cwd()` — the real process cwd,
    /// not a parameter. Runner cwd shapes vary and can even share segments
    /// with grid paths (GitHub's Windows runner checks out under `D:\a\...`,
    /// so retargeting the capture drive turns grid paths into `D:\a...` and
    /// steals the `..` chain). Anchor both grids on a synthetic fixed-depth
    /// directory on the live drive instead: create it, point the process at
    /// it, and clear the shell-injected per-drive cwd var, so both comparison
    /// sides expand against one deterministic value on any machine. Restored
    /// on drop; run the suite serially (the gate protocol) when other tests
    /// read the process cwd.
    #[cfg(windows)] // only the win32 grids resolve against a process cwd
    struct GridCwd {
        // field order is drop order: the original cwd is restored before the
        // lock releases, so the synthetic cwd is only live while held
        _restore: RestoreOriginalCwd,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(windows)]
    struct RestoreOriginalCwd {
        original: String,
        synthetic: String,
    }

    #[cfg(windows)]
    impl Drop for RestoreOriginalCwd {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
            // best-effort cleanup of the empty synthetic chain (recreated on
            // demand by the next grid test)
            let mut tail = self.synthetic.clone();
            for name in ["\\q2", "\\q1", "\\q0"] {
                if !tail.ends_with(name) || std::fs::remove_dir(&tail).is_err() {
                    break;
                }
                tail.truncate(tail.len() - name.len());
            }
        }
    }

    #[cfg(windows)]
    impl GridCwd {
        fn enter() -> Self {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            // serialize the two grids (both anchor the process cwd) so
            // concurrent cwd switches cannot race each other
            let lock = LOCK.lock().unwrap_or_else(|error| error.into_inner());
            let original = current_dir_string();
            if let Some(live) = crate::coding_agent::oracle_scrub::live_drive_letter() {
                std::env::remove_var(format!("={live}:"));
            }
            // Device prefix of the original cwd (drive root, or UNC share
            // root), always terminating in a separator. `q0`/`q1`/`q2` appear
            // in no grid path, so no retargeted grid path shares a prefix
            // with the synthetic directory.
            let device = if original.starts_with("\\\\") {
                let parts: Vec<&str> = original
                    .split('\\')
                    .filter(|part| !part.is_empty())
                    .collect();
                match parts.as_slice() {
                    [server, share, ..] => format!("\\\\{server}\\{share}\\"),
                    _ => original.clone(),
                }
            } else {
                let end = original
                    .find('\\')
                    .map(|index| index + 1)
                    .unwrap_or(original.len());
                original[..end].to_string()
            };
            let device = if device.ends_with('\\') {
                device
            } else {
                format!("{device}\\")
            };
            let synthetic = format!("{device}q0\\q1\\q2");
            std::fs::create_dir_all(&synthetic).expect("create synthetic grid cwd");
            std::env::set_current_dir(&synthetic).expect("enter synthetic grid cwd");
            Self {
                _restore: RestoreOriginalCwd {
                    original,
                    synthetic,
                },
                _lock: lock,
            }
        }
    }

    /// oracle values may embed the captured cwd as a "!CWD" prefix marker, as
    /// "!REL<relative>" (re-resolved against the live cwd with the same
    /// flavor), or as "!DEPTH<suffix>" for relative()-style results whose
    /// ".." chain has one link per cwd level (the suffix survives across
    /// machines).
    fn expand_marker(expected: &str, windows: bool, cwd: &str) -> String {
        let separator = if windows { '\\' } else { '/' };
        if let Some(suffix) = expected.strip_prefix("!DEPTH") {
            let separator_string = separator.to_string();
            // environment-anchored: one ".." per cwd segment below the device
            // root (matching path.relative to that root; a root cwd
            // contributes none), not per raw separator. Windows paths carry a
            // leading device segment ("D:"), POSIX ones do not.
            let segments: Vec<&str> = cwd
                .split(separator)
                .filter(|part| !part.is_empty())
                .collect();
            let depth = if windows {
                segments.len().saturating_sub(1)
            } else {
                segments.len()
            };
            let chain = vec![".."; depth].join(&separator_string);
            if chain.is_empty() {
                return suffix.trim_start_matches(separator).to_string();
            }
            return format!("{chain}{suffix}");
        }
        if let Some(relative) = expected.strip_prefix("!REL") {
            if windows {
                win32_resolve(&[relative], cwd)
            } else {
                posix_resolve(&[relative], cwd)
            }
        } else {
            // Strip a trailing separator so a device-root cwd (X:\)
            // substitutes cleanly into "!CWD\<suffix>" forms.
            let base = cwd.strip_suffix(separator).unwrap_or(cwd);
            expected.replace("!CWD", base)
        }
    }

    /// environment-anchored: a device-root cwd (X:\) makes drive-relative
    /// resolution surface the root with its trailing separator while the
    /// capture (a non-root cwd) stored none; drop trailing separators on both
    /// comparison sides.
    #[cfg(windows)] // only the win32 grids compare resolved absolute paths
    fn strip_trailing_separator(value: &str, windows: bool) -> &str {
        let separator = if windows { '\\' } else { '/' };
        value.strip_suffix(separator).unwrap_or(value)
    }

    /// environment-anchored: the win32 grids were captured on a machine whose
    /// process cwd sat on `C:`. Entries with marker expectations (!REL/!CWD/
    /// !DEPTH) resolve drive-relative inputs against the capture drive, so
    /// those inputs are retargeted to the live drive; entries with literal
    /// expectations are cwd-independent and keep the captured inputs verbatim.
    /// (Literal entries with synthetic devices, e.g. `x:`, additionally assume
    /// the live drive differs from the synthetic letter, as on any real
    /// runner: only then does node's per-device fallback land on the device
    /// root like the capture.)
    #[cfg(windows)] // only the win32 grids retarget capture-drive inputs
    fn retarget_args(args: &[&str], expected: &str) -> Vec<String> {
        let marker = expected.starts_with("!REL")
            || expected.starts_with("!CWD")
            || expected.starts_with("!DEPTH");
        args.iter()
            .map(|arg| {
                if marker {
                    crate::coding_agent::oracle_scrub::retarget_capture_drive(arg)
                } else {
                    arg.to_string()
                }
            })
            .collect()
    }

    #[test]
    #[cfg(windows)] // grid captured with a win32 process cwd
    fn node_path_win32_resolve_grid() {
        let _grid_cwd = GridCwd::enter();
        let c = current_dir_string();
        for (args, expected) in oracle::NODE_PATH_WIN32_RESOLVE {
            let retargeted = retarget_args(args, expected);
            let refs: Vec<&str> = retargeted.iter().map(String::as_str).collect();
            let got = win32_resolve(&refs, &c);
            // node resolves win32 paths case-insensitively and preserves the
            // input's drive case, so compare case-insensitively.
            assert!(
                strip_trailing_separator(&got, true).eq_ignore_ascii_case(
                    strip_trailing_separator(&expand_marker(expected, true, &c), true)
                ),
                "win32 resolve {args:?}: {got:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn node_path_posix_resolve_grid() {
        let cwd = posix_cwd();
        for (args, expected) in oracle::NODE_PATH_POSIX_RESOLVE {
            let got = posix_resolve(args, &cwd);
            assert_eq!(
                got,
                expand_marker(expected, false, &cwd),
                "posix resolve {args:?}"
            );
        }
    }

    #[test]
    fn node_path_win32_is_absolute_grid() {
        for (i, expected) in oracle::NODE_PATH_WIN32_IS_ABSOLUTE {
            let args = oracle::NODE_PATH_WIN32_RESOLVE[*i].0;
            assert_eq!(
                win32_is_absolute(args[0]),
                *expected,
                "entry {i}: {:?}",
                args[0]
            );
        }
    }

    #[test]
    fn node_path_posix_is_absolute_grid() {
        for (i, expected) in oracle::NODE_PATH_POSIX_IS_ABSOLUTE {
            let args = oracle::NODE_PATH_POSIX_RESOLVE[*i].0;
            assert_eq!(
                posix_is_absolute(args[0]),
                *expected,
                "entry {i}: {:?}",
                args[0]
            );
        }
    }

    #[test]
    #[cfg(windows)] // grid captured with a win32 process cwd
    fn node_path_win32_relative_grid() {
        let _grid_cwd = GridCwd::enter();
        let c = current_dir_string();
        for (from, to, expected) in oracle::NODE_PATH_WIN32_RELATIVE {
            let retargeted = retarget_args(&[from, to], expected);
            assert_eq!(
                strip_trailing_separator(&win32_relative(&retargeted[0], &retargeted[1], &c), true),
                strip_trailing_separator(&expand_marker(expected, true, &c), true),
                "win32 relative({from:?}, {to:?})"
            );
        }
    }

    #[test]
    fn node_path_posix_relative_grid() {
        let c = posix_cwd();
        for (from, to, expected) in oracle::NODE_PATH_POSIX_RELATIVE {
            assert_eq!(
                posix_relative(from, to, &c),
                expand_marker(expected, false, &c),
                "posix relative({from:?}, {to:?})"
            );
        }
    }

    #[test]
    fn node_path_win32_join_grid() {
        for (args, expected) in oracle::NODE_PATH_WIN32_JOIN {
            assert_eq!(win32_join(args), *expected, "win32 join {args:?}");
        }
    }

    #[test]
    fn node_path_posix_join_grid() {
        for (args, expected) in oracle::NODE_PATH_POSIX_JOIN {
            assert_eq!(posix_join(args), *expected, "posix join {args:?}");
        }
    }

    #[test]
    fn node_path_win32_normalize_grid() {
        for (input, expected) in oracle::NODE_PATH_WIN32_NORMALIZE {
            assert_eq!(
                win32_normalize(input),
                *expected,
                "win32 normalize {input:?}"
            );
        }
    }

    #[test]
    fn node_path_posix_normalize_grid() {
        for (input, expected) in oracle::NODE_PATH_POSIX_NORMALIZE {
            assert_eq!(
                posix_normalize(input),
                *expected,
                "posix normalize {input:?}"
            );
        }
    }

    #[test]
    fn normalize_string_matches_node_on_samples() {
        // Pins captured from node's normalizeString through path.normalize.
        assert_eq!(
            normalize_string("a\\b\\..\\c", true, '\\', is_path_separator),
            "a\\c"
        );
        assert_eq!(
            normalize_string("..\\..\\a", true, '\\', is_path_separator),
            "..\\..\\a"
        );
        assert_eq!(
            normalize_string("/a/b/../c", true, '/', is_posix_path_separator),
            "a/c"
        );
        assert_eq!(
            normalize_string("a/../../b", true, '/', is_posix_path_separator),
            "../b"
        );
    }

    #[test]
    fn per_device_cwd_falls_back_to_cwd() {
        // Per-drive cwd entries (`=Z:`) are a Windows env convention; glibc
        // rejects `=` in env names, so the removal is a no-op there and the
        // variable can never be set to begin with.
        #[cfg(windows)]
        std::env::remove_var("=Z:");
        assert!(per_device_cwd("Z:").is_none());
    }
}
