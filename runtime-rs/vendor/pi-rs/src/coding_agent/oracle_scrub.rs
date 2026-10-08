//! Shared scrubbing of environment-anchored oracle captures (test-only).
//!
//! The checked-in oracle fixtures are authoritative capture-time baselines
//! (taken on the Windows capture machine) and are never regenerated. Some of
//! them embed environment anchors: the capture machine's repo path, home
//! directory, and `C:` drive letter (root-relative fixture inputs resolve
//! against the process drive). Every comparison applies the same
//! normalization to **both** sides, so the assertions stay exact — only the
//! environment anchor is abstracted.
//!
//! (environment-anchored: both sides normalized)

/// The capture machine's repo checkout, baked into some oracle files.
const CAPTURE_REPO_ROOT: &str = r"C:\Users\13063\Desktop\code\agent work\pi-rust";

/// The capture machine's home directory, baked into some oracle files.
const CAPTURE_HOME: &str = r"C:\Users\13063";

/// `<REPO>` placeholder for the repo-root anchor.
const REPO_PLACEHOLDER: &str = "<REPO>";

/// `<HOME>` placeholder for the home-directory anchor.
const HOME_PLACEHOLDER: &str = "<HOME>";

/// `<DRV>:/` placeholder for a drive/root-anchored absolute path prefix.
const DRIVE_PLACEHOLDER: &str = "<DRV>:/";

fn live_repo_root() -> String {
    env!("CARGO_MANIFEST_DIR").to_string()
}

fn live_home() -> String {
    dirs::home_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Rewrite every `X:/` drive prefix (after separator unification) to
/// [`DRIVE_PLACEHOLDER`]. URLs like `https://` also match, but the rewrite is
/// applied identically to both comparison sides, so equality semantics are
/// preserved (only the drive letter becomes anonymous).
fn rewrite_drive_prefixes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + DRIVE_PLACEHOLDER.len());
    let mut index = 0;
    while index < chars.len() {
        let is_drive = chars[index].is_ascii_alphabetic()
            && chars.get(index + 1) == Some(&':')
            && chars.get(index + 2) == Some(&'/');
        if is_drive {
            out.push_str(DRIVE_PLACEHOLDER);
            index += 3;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

/// Normalize one string: unify separators to `/`, replace the capture/live
/// repo-root and home anchors with placeholders, and map drive-anchored or
/// root-anchored absolute path prefixes to `<DRV>:/`.
pub fn scrub_str(text: &str) -> String {
    // Separators first so every anchor matches in a single form.
    let mut out = text.replace('\\', "/");
    // The repo root must be replaced before the home root (it contains it).
    // Live anchors are replaced with the same placeholders as the capture
    // anchors; on the capture machine both forms coincide.
    let anchors = [
        (CAPTURE_REPO_ROOT.to_string(), REPO_PLACEHOLDER),
        (live_repo_root(), REPO_PLACEHOLDER),
        (CAPTURE_HOME.to_string(), HOME_PLACEHOLDER),
        (live_home(), HOME_PLACEHOLDER),
    ];
    for (anchor, placeholder) in anchors {
        if anchor.is_empty() {
            continue;
        }
        out = out.replace(&anchor.replace('\\', "/"), placeholder);
    }
    let out = rewrite_drive_prefixes(&out);
    // Root-anchored paths ("/non/existent") resolve against the live drive.
    let out = if out.starts_with('/') {
        format!("{DRIVE_PLACEHOLDER}{out}")
    } else {
        out
    };
    // Multi-line guidance texts indent drive- or root-anchored paths.
    out.replace("\n  /", &format!("\n  {DRIVE_PLACEHOLDER}"))
}

/// Recursively normalize every string in a JSON value.
pub fn scrub_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = scrub_str(text),
        serde_json::Value::Array(items) => {
            for item in items {
                scrub_value(item);
            }
        }
        serde_json::Value::Object(entries) => {
            for entry in entries.values_mut() {
                scrub_value(entry);
            }
        }
        _ => {}
    }
}

/// The live process drive letter (`D` for a `D:\...` cwd), if any.
pub fn live_drive_letter() -> Option<char> {
    std::env::current_dir().ok().and_then(|dir| {
        dir.to_string_lossy()
            .chars()
            .next()
            .filter(|c| c.is_ascii_alphabetic())
    })
}

/// environment-anchored: the win32 grids were captured on a machine whose
/// process cwd sat on `C:`; drive-relative grid inputs resolve against the
/// live drive's cwd, so retarget the capture drive in grid inputs to the live
/// drive (preserving the input's letter case) before comparing.
pub fn retarget_capture_drive(input: &str) -> String {
    let Some(live) = live_drive_letter() else {
        return input.to_string();
    };
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    while index < chars.len() {
        let letter = chars[index];
        let is_capture_drive =
            letter.eq_ignore_ascii_case(&'C') && chars.get(index + 1) == Some(&':');
        if is_capture_drive {
            let replacement = if letter.is_ascii_uppercase() {
                live.to_ascii_uppercase()
            } else {
                live.to_ascii_lowercase()
            };
            out.push(replacement);
            out.push(':');
            index += 2;
        } else {
            out.push(letter);
            index += 1;
        }
    }
    out
}
