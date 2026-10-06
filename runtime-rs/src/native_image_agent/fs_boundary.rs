//! File identities and no-follow opens for the Agent's narrow workspace boundary.
use crate::workflow_engine::RunCancellation;
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Failure {
    Invalid,
    Changed,
    Io,
    Cancelled,
}
pub(super) type BoundaryResult<T> = std::result::Result<T, Failure>;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    file: u64,
    mode: u64,
}
#[derive(Clone)]
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) metadata: Metadata,
    identity: Identity,
}
fn linked(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn open(path: &Path, directory: bool, creating: bool) -> BoundaryResult<File> {
    let mut options = OpenOptions::new();
    if creating {
        options.write(true).create_new(true);
    } else {
        options.read(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000 | if directory { 0x02000000 } else { 0 });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        if creating {
            options.mode(0o600);
        }
    }
    options.open(path).map_err(|_| Failure::Io)
}
#[cfg(windows)]
fn identity(file: &File, metadata: &Metadata) -> BoundaryResult<Identity> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Information {
        attributes: u32,
        created: FileTime,
        accessed: FileTime,
        modified: FileTime,
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            information: *mut Information,
        ) -> i32;
    }
    let mut information = Information::default();
    // The repr(C) buffer matches BY_HANDLE_FILE_INFORMATION and the borrowed
    // File keeps this valid Windows handle open for the entire native call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(Failure::Io);
    }
    Ok(Identity {
        device: information.volume.into(),
        file: (u64::from(information.index_high) << 32) | u64::from(information.index_low),
        mode: if metadata.permissions().readonly() {
            1
        } else {
            0
        },
    })
}
#[cfg(unix)]
fn identity(_file: &File, metadata: &Metadata) -> BoundaryResult<Identity> {
    use std::os::unix::fs::MetadataExt;
    Ok(Identity {
        device: metadata.dev(),
        file: metadata.ino(),
        mode: metadata.mode().into(),
    })
}
#[cfg(not(any(unix, windows)))]
fn identity(_file: &File, _metadata: &Metadata) -> BoundaryResult<Identity> {
    Err(Failure::Invalid)
}

pub(super) fn inspect(path: &Path, directory: bool) -> BoundaryResult<Entry> {
    let before = fs::symlink_metadata(path).map_err(|_| Failure::Io)?;
    if linked(&before)
        || (if directory {
            !before.is_dir()
        } else {
            !before.is_file()
        })
    {
        return Err(Failure::Invalid);
    }
    let file = open(path, directory, false)?;
    let metadata = file.metadata().map_err(|_| Failure::Io)?;
    if linked(&metadata)
        || (if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        })
    {
        return Err(Failure::Invalid);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != metadata.dev()
            || before.ino() != metadata.ino()
            || before.mode() != metadata.mode()
        {
            return Err(Failure::Changed);
        }
    }
    Ok(Entry {
        path: path.to_owned(),
        identity: identity(&file, &metadata)?,
        metadata,
    })
}
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}
pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    let (left, right) = (display_path(left), display_path(right));
    let (left, right) = (Path::new(&left), Path::new(&right));
    let (mut left, mut right) = (left.components(), right.components());
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if component_eq(a.as_os_str(), b.as_os_str()) => {}
            _ => return false,
        }
    }
}
pub(super) fn inside(root: &Path, path: &Path) -> Option<PathBuf> {
    let (root, path) = (display_path(root), display_path(path));
    let (root, path) = (Path::new(&root), Path::new(&path));
    let mut children = path.components();
    for component in root.components() {
        if !component_eq(component.as_os_str(), children.next()?.as_os_str()) {
            return None;
        }
    }
    let suffix = children.collect::<PathBuf>();
    (!suffix.as_os_str().is_empty()).then_some(suffix)
}
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}
pub(super) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            let bytes = rest.as_bytes();
            if bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'\\'
            {
                return rest.to_owned();
            }
            if let Some(unc) = rest.strip_prefix(r"UNC\") {
                let mut parts = unc.split('\\');
                if parts.next().is_some_and(|value| !value.is_empty())
                    && parts.next().is_some_and(|value| !value.is_empty())
                {
                    return format!(r"\\{unc}");
                }
            }
        }
    }
    value.into_owned()
}
pub(super) fn root(cwd: &Path) -> BoundaryResult<Entry> {
    let canonical = fs::canonicalize(cwd).map_err(|_| Failure::Io)?;
    inspect(&canonical, true)
}
pub(super) fn verify(root: &Path, entries: &[Entry]) -> BoundaryResult<()> {
    for entry in entries {
        if !same_path(&entry.path, root) && inside(root, &entry.path).is_none() {
            return Err(Failure::Invalid);
        }
        let current = inspect(&entry.path, entry.metadata.is_dir())?;
        if current.identity != entry.identity
            || !same_path(
                &fs::canonicalize(&entry.path).map_err(|_| Failure::Io)?,
                &entry.path,
            )
        {
            return Err(Failure::Changed);
        }
    }
    Ok(())
}
pub(super) fn inspect_file(root: &Entry, suffix: &Path) -> BoundaryResult<Vec<Entry>> {
    let mut entries = vec![root.clone()];
    let mut path = root.path.clone();
    let components = suffix.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Failure::Invalid);
    }
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        entries.push(inspect(&path, index + 1 < components.len())?);
    }
    verify(&root.path, &entries)?;
    Ok(entries)
}
pub(super) fn read_file(
    root: &Entry,
    entries: &[Entry],
    maximum: usize,
    cancellation: &RunCancellation,
) -> BoundaryResult<Vec<u8>> {
    let source = entries.last().ok_or(Failure::Invalid)?;
    let mut file = open(&source.path, false, false)?;
    let opened = file.metadata().map_err(|_| Failure::Io)?;
    if !opened.is_file()
        || linked(&opened)
        || identity(&file, &opened)? != source.identity
        || opened.len() != source.metadata.len()
        || opened.modified().ok() != source.metadata.modified().ok()
    {
        return Err(Failure::Changed);
    }
    let mut bytes = Vec::with_capacity(maximum.min(opened.len() as usize));
    let mut chunk = [0_u8; 64 * 1024];
    while bytes.len() < opened.len() as usize {
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        let wanted = chunk.len().min(opened.len() as usize - bytes.len());
        let size = file.read(&mut chunk[..wanted]).map_err(|_| Failure::Io)?;
        if size == 0 || bytes.len() + size > maximum {
            return Err(Failure::Changed);
        }
        bytes.extend_from_slice(&chunk[..size]);
    }
    let finished = file.metadata().map_err(|_| Failure::Io)?;
    if identity(&file, &finished)? != source.identity
        || finished.len() != opened.len()
        || finished.modified().ok() != opened.modified().ok()
    {
        return Err(Failure::Changed);
    }
    let after = inspect_file(
        root,
        &source
            .path
            .strip_prefix(&root.path)
            .map_err(|_| Failure::Changed)?,
    )?;
    if after.len() != entries.len()
        || after
            .iter()
            .zip(entries)
            .any(|(after, before)| after.identity != before.identity)
    {
        return Err(Failure::Changed);
    }
    Ok(bytes)
}
pub(super) fn child(root: &Path, entries: &mut Vec<Entry>, name: &str) -> BoundaryResult<()> {
    verify(root, entries)?;
    let path = entries.last().ok_or(Failure::Invalid)?.path.join(name);
    match create_directory(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Failure::Io),
    }
    let entry = inspect(&path, true)?;
    if !same_path(&fs::canonicalize(&path).map_err(|_| Failure::Io)?, &path) {
        return Err(Failure::Invalid);
    }
    entries.push(entry);
    Ok(())
}
pub(super) fn new_directory(
    root: &Path,
    entries: &mut Vec<Entry>,
    name: &str,
) -> BoundaryResult<()> {
    verify(root, entries)?;
    let path = entries.last().ok_or(Failure::Invalid)?.path.join(name);
    create_directory(&path).map_err(|_| Failure::Io)?;
    entries.push(inspect(&path, true)?);
    verify(root, entries)
}
fn create_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
pub(super) fn write_file(
    root: &Path,
    entries: &[Entry],
    name: &str,
    bytes: &[u8],
    cancellation: &RunCancellation,
) -> BoundaryResult<PathBuf> {
    if cancellation.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    verify(root, entries)?;
    let path = entries.last().ok_or(Failure::Invalid)?.path.join(name);
    let mut file = open(&path, false, true)?;
    let opened = file.metadata().map_err(|_| Failure::Io)?;
    if !opened.is_file() || linked(&opened) {
        return Err(Failure::Invalid);
    }
    let identity = identity(&file, &opened)?;
    file.write_all(bytes).map_err(|_| Failure::Io)?;
    if cancellation.is_cancelled() {
        return Err(Failure::Cancelled);
    }
    file.sync_all().map_err(|_| Failure::Io)?;
    verify(root, entries)?;
    let current = inspect(&path, false)?;
    if current.identity != identity
        || current.metadata.len() != bytes.len() as u64
        || !same_path(&fs::canonicalize(&path).map_err(|_| Failure::Io)?, &path)
    {
        return Err(Failure::Changed);
    }
    Ok(path)
}
pub(super) fn cleanup(root: &Path, entries: &[Entry]) -> BoundaryResult<()> {
    let own = entries.last().ok_or(Failure::Invalid)?;
    if entries.len() != 4
        || inside(root, &own.path).is_none()
        || own
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .is_none()
    {
        return Err(Failure::Invalid);
    }
    // Changed identity intentionally leaves the foreign path alone, matching the
    // release cleanup rule. The absolute target was checked against this workspace.
    if verify(root, entries).is_err() {
        return Ok(());
    }
    fs::remove_dir_all(&own.path).map_err(|_| Failure::Io)
}
