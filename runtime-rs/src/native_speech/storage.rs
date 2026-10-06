use super::error::{download, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub fn cancelled(token: &CancellationToken) -> Result<()> {
    if token.is_cancelled() {
        Err(download("cancelled"))
    } else {
        Ok(())
    }
}
pub fn directory(path: &Path, create: bool) -> Result<bool> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| download("storage"))?
            .join(path)
    };
    let mut current = PathBuf::new();
    for part in absolute.components() {
        current.push(part.as_os_str());
        if matches!(part, std::path::Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(info) => {
                if linked(&info) || !info.is_dir() {
                    return Err(download("path"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(false);
                }
                fs::create_dir(&current).map_err(|_| download("storage"))?;
                let info = fs::symlink_metadata(&current).map_err(|_| download("storage"))?;
                if linked(&info) || !info.is_dir() {
                    return Err(download("path"));
                }
            }
            Err(_) => return Err(download("storage")),
        }
    }
    Ok(true)
}
pub fn linked(info: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if info.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    info.file_type().is_symlink()
}
pub fn checked_file(path: &Path, write: bool, create: bool) -> Result<File> {
    if !directory(path.parent().ok_or_else(|| download("path"))?, false)? {
        return Err(download("path"));
    }
    if let Ok(info) = fs::symlink_metadata(path) {
        if linked(&info) || !info.is_file() {
            return Err(download("path"));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(write).create(create);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .mode(0o600);
    }
    let file = options.open(path).map_err(|_| download("storage"))?;
    let metadata = file.metadata().map_err(|_| download("storage"))?;
    if !metadata.is_file() || linked(&metadata) || file_identity(&file)?.1 != 1 {
        return Err(download("path"));
    }
    let after = fs::symlink_metadata(path).map_err(|_| download("path"))?;
    if linked(&after) || !after.is_file() {
        return Err(download("path"));
    }
    // Re-open to prove the path still resolves to the exact handle identity.
    let mut probe = OpenOptions::new();
    probe.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        probe.custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        probe.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let other = probe.open(path).map_err(|_| download("path"))?;
    if file_identity(&file)? != file_identity(&other)? {
        return Err(download("path"));
    }
    Ok(file)
}
pub fn digest_file(path: &Path, size: u64, token: &CancellationToken) -> Result<String> {
    let mut file = checked_file(path, false, false)?;
    if file.metadata().map_err(|_| download("storage"))?.len() != size {
        return Err(download("size"));
    }
    let before = file_identity(&file)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    let mut total = 0;
    loop {
        cancelled(token)?;
        let read = file.read(&mut buffer).map_err(|_| download("storage"))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > size {
            return Err(download("size"));
        }
        hash.update(&buffer[..read]);
    }
    if total != size || file_identity(&file)? != before {
        return Err(download("size"));
    }
    Ok(hash.finalize().iter().map(|v| format!("{v:02x}")).collect())
}
pub fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    directory(path.parent().ok_or_else(|| download("path"))?, true)?;
    let temporary = path.with_file_name(format!(".speech-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| download("storage"))?;
        file.write_all(&serde_json::to_vec(value).map_err(|_| download("storage"))?)
            .map_err(|_| download("storage"))?;
        file.sync_all().map_err(|_| download("storage"))?;
        drop(file);
        if path.exists() {
            checked_file(path, false, false)?;
        }
        fs::rename(&temporary, path).map_err(|_| download("storage"))
    })(); // atomic replace, no credential or source payload logging
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
pub fn file_identity(file: &File) -> Result<(String, u64)> {
    let info = file.metadata().map_err(|_| download("storage"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Ok((
            format!(
                "{}:{}:{}:{}:{}:{}:{}",
                info.dev(),
                info.ino(),
                info.len(),
                info.mtime(),
                info.mtime_nsec(),
                info.ctime(),
                info.ctime_nsec()
            ),
            info.nlink(),
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct Info {
            attributes: u32,
            creation: FileTime,
            access: FileTime,
            write: FileTime,
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct Basic {
            creation: i64,
            access: i64,
            write: i64,
            change: i64,
            attributes: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
            fn GetFileInformationByHandleEx(
                handle: *mut std::ffi::c_void,
                class: i32,
                info: *mut std::ffi::c_void,
                size: u32,
            ) -> i32;
        }
        let mut handle_info = Info::default();
        let mut basic = Basic::default();
        // SAFETY: repr(C) layouts match Win32 BY_HANDLE_FILE_INFORMATION and FILE_BASIC_INFO, handle is borrowed and valid.
        unsafe {
            if GetFileInformationByHandle(file.as_raw_handle(), &mut handle_info) == 0
                || GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    0,
                    &mut basic as *mut _ as *mut _,
                    std::mem::size_of::<Basic>() as u32,
                ) == 0
            {
                return Err(download("storage"));
            }
        }
        return Ok((
            format!(
                "{}:{}:{}:{}:{}:{}:{}",
                handle_info.volume,
                handle_info.index_high,
                handle_info.index_low,
                info.len(),
                basic.creation,
                basic.write,
                basic.change
            ),
            handle_info.links as u64,
        ));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let stamp = info
            .modified()
            .ok()
            .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|v| v.as_nanos())
            .unwrap_or(0);
        Ok((format!("{}:{stamp}", info.len()), 1))
    }
}
pub fn file_stamp(path: &Path) -> Result<String> {
    let file = checked_file(path, false, false)?;
    Ok(file_identity(&file)?.0)
}
pub fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut file = checked_file(path, false, false)?;
    if file.metadata().map_err(|_| download("storage"))?.len() > limit {
        return Err(download("size"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| download("storage"))?;
    if bytes.len() as u64 > limit {
        return Err(download("size"));
    }
    Ok(bytes)
}
