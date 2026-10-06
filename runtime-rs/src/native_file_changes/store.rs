//! 快照索引沿用 release v2；磁盘写入在会话捕获锁内同步原子提交。
use super::Result;
use crate::ApiError;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub(crate) const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 200;
pub(crate) const MAX_INDEX_BYTES: usize = 512 * 1024;
pub(crate) const MAX_SUMMARY_FILE_BYTES: usize = 512 * 1024;
pub(crate) const MAX_SUMMARY_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_SUMMARY_LINES: usize = 2_000;

pub(crate) fn key(value: &str, length: usize) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..length]
        .to_owned()
}
pub(crate) fn check_storage(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        if component == Component::ParentDir {
            return Err(ApiError::bad_request("快照存储路径无效。"));
        }
        current.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if meta.file_type().is_symlink() || reparse {
                    return Err(ApiError::bad_request("快照存储路径不能是链接。"));
                }
                if current != absolute && !meta.is_dir() {
                    return Err(ApiError::bad_request("快照存储上级路径无效。"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}
pub(crate) fn io_error(error: std::io::Error) -> ApiError {
    ApiError::internal(crate::security::redact_secret_text(&error.to_string()))
}
pub(crate) fn real(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    #[cfg(windows)]
    {
        let value = canonical.to_string_lossy();
        return Ok(PathBuf::from(
            if let Some(unc) = value.strip_prefix("\\\\?\\UNC\\") {
                format!("\\\\{unc}")
            } else {
                value.strip_prefix("\\\\?\\").unwrap_or(&value).to_owned()
            },
        ));
    }
    #[cfg(not(windows))]
    Ok(canonical)
}
fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
pub(crate) fn nested(root: &Path, path: &Path) -> bool {
    let root = path_key(root).trim_end_matches('/').to_owned();
    let path = path_key(path);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('/'))
}
pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}
pub(crate) fn lexical(path: &Path) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
                }
            }
            _ => result.push(component),
        }
    }
    Ok(result)
}
pub(crate) fn relative(cwd: &Path, input: &str, api: bool) -> Result<String> {
    if input.is_empty() {
        return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
    }
    let root = lexical(cwd)?;
    let path = Path::new(input);
    let absolute = lexical(&if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    })?;
    if !nested(&root, &absolute) {
        return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
    }
    let root_key = path_key(&root);
    let target_key = path_key(&absolute);
    let offset = root_key.trim_end_matches('/').len();
    let rel = target_key
        .get(offset..)
        .unwrap_or("")
        .trim_start_matches('/');
    // 路径大小写保留原文件名；Windows 根目录的大小写只用于边界核验。
    let absolute_string = absolute.to_string_lossy().replace('\\', "/");
    let preserved = absolute_string
        .get(offset..)
        .unwrap_or(rel)
        .trim_start_matches('/')
        .to_owned();
    if preserved.is_empty() || (api && preserved.starts_with("..")) {
        return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
    }
    Ok(preserved)
}
/// 检查每个现有祖先的真实位置。缺失目标也不能借助工作区外的链接目录写入。
pub(crate) fn target(cwd: &Path, rel: &str) -> Result<PathBuf> {
    let rel = relative(cwd, rel, false)?;
    let root = real(cwd)?;
    let target = lexical(&cwd.join(rel))?;
    let mut parent = target.clone();
    loop {
        match fs::symlink_metadata(&parent) {
            Ok(_) => {
                if !nested(&root, &real(&parent)?) {
                    return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !parent.pop() {
                    return Err(io_error(error));
                }
            }
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(target)
}
/// 恢复操作使用已核验的真实祖先路径，工作区内的合法链接也能恢复原内容。
/// 删除新文件时保留最后一个目录项，避免将 unlink 链接变成删除链接目标。
pub(crate) fn mutation_target(cwd: &Path, rel: &str, follow_leaf: bool) -> Result<PathBuf> {
    let target = target(cwd, rel)?;
    let root = real(cwd)?;
    let mut parent = if follow_leaf {
        target.clone()
    } else {
        target
            .parent()
            .ok_or_else(|| ApiError::bad_request("文件路径无效。"))?
            .to_owned()
    };
    let mut suffix = if follow_leaf {
        Vec::new()
    } else {
        vec![target
            .file_name()
            .ok_or_else(|| ApiError::bad_request("文件路径无效。"))?
            .to_os_string()]
    };
    loop {
        match fs::symlink_metadata(&parent) {
            Ok(_) => {
                let mut actual = real(&parent)?;
                if !nested(&root, &actual) {
                    return Err(ApiError::bad_request("文件路径超出会话工作区范围。"));
                }
                for part in suffix.iter().rev() {
                    actual.push(part);
                }
                return Ok(actual);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                suffix.push(
                    parent
                        .file_name()
                        .ok_or_else(|| ApiError::bad_request("文件路径无效。"))?
                        .to_os_string(),
                );
                if !parent.pop() {
                    return Err(io_error(error));
                }
            }
            Err(error) => return Err(io_error(error)),
        }
    }
}
pub(crate) fn bounded(path: &Path, maximum: usize, storage: bool) -> Result<Option<Vec<u8>>> {
    if storage {
        check_storage(path)?;
    }
    match fs::metadata(path) {
        Ok(info) if !info.is_file() || info.len() > maximum as u64 => {
            return Err(ApiError::bad_request("文件过大或不是普通文件。"))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | if storage { libc::O_NOFOLLOW } else { 0 });
    }
    if storage {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000);
        }
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let info = file.metadata().map_err(io_error)?;
    if !info.is_file() || info.len() > maximum as u64 {
        return Err(ApiError::bad_request("文件过大或不是普通文件。"));
    }
    let expected = info.len();
    let mut bytes = Vec::with_capacity(expected as usize);
    (&mut file)
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > maximum || bytes.len() as u64 != expected {
        return Err(ApiError::bad_request("文件在读取过程中发生变化。"));
    }
    if storage {
        check_storage(path)?;
    }
    Ok(Some(bytes))
}
pub(crate) fn read_index(path: &Path) -> Result<Option<Value>> {
    bounded(path, 16 * 1024 * 1024, true)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::internal("文件快照索引 JSON 无效。"))
        })
        .transpose()
}
pub(crate) fn empty() -> Value {
    json!({"entries":[]})
}
pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    check_storage(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("快照目录无效。"))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    check_storage(parent)?;
    let temp = parent.join(format!(".file-change-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        check_storage(path)?;
        fs::rename(&temp, path).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub(crate) fn valid_key(key: &str) -> bool {
    key.len() == 24
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(crate) fn prune(root: &Path) -> Result<()> {
    check_storage(root)?;
    let dirs = match fs::read_dir(root) {
        Ok(dirs) => dirs,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    let mut snapshots = Vec::new();
    let mut markers = Vec::new();
    for dir in dirs.flatten() {
        let name = dir.file_name().to_string_lossy().into_owned();
        if name.len() != 32
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            continue;
        }
        if !dir.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || check_storage(&dir.path()).is_err()
        {
            continue;
        }
        let modified = dir
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let marker = bounded(&dir.path().join("index.json"), 8 * 1024, true)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|index| index["entries"].as_array().is_some_and(Vec::is_empty));
        if marker {
            markers.push((modified, dir.path()));
        } else {
            snapshots.push((modified, dir.path()));
        }
    }
    for (list, retain) in [(&mut snapshots, 50), (&mut markers, 500)] {
        list.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, path) in list.iter().skip(retain) {
            // 只删除经校验的本服务直接子目录；不会遍历或清理其他工作区。
            if path.parent() == Some(root) && check_storage(path).is_ok() {
                fs::remove_dir_all(path).map_err(io_error)?;
            }
        }
    }
    Ok(())
}
