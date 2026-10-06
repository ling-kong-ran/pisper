//! Minimal filesystem-backed [`FileSystem`] fixture for the harness resource
//! loader tests (skills, prompt templates). It stands in for the
//! `NodeExecutionEnv` port (M3b Task 6) with the behavior those tests rely
//! on: addressed paths resolved against `cwd`, `lstat`-style kinds that do
//! not follow symlinks, `read_dir`-order listings, and realpath-based
//! canonicalization (upstream `env/nodejs.ts:440-470, 700-740, 800-870`).
//! Tests write fixtures through `std::fs` directly; the fixture is
//! read-side only and every unused operation reports `not_supported`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use futures::future::BoxFuture;

use crate::agent_core::harness::types::{
    CreateDirOptions, FileContent, FileError, FileErrorCode, FileInfo, FileKind, FileSystem,
    ReadTextLinesOptions, RemoveOptions, TempFileOptions, TextLineReader,
};
use crate::agent_core::harness::Context;

pub(crate) struct TestFsEnv {
    cwd: String,
}

impl TestFsEnv {
    pub(crate) fn new(cwd: &Path) -> Self {
        TestFsEnv {
            cwd: display_path(cwd),
        }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let candidate = Path::new(path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            Path::new(&self.cwd).join(candidate)
        };
        // Normalize separators and `.` components like node's `path.resolve`.
        let mut normalized = PathBuf::new();
        for component in joined.components() {
            normalized.push(component.as_os_str());
        }
        normalized
    }
}

fn display_path(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    // Windows `fs::canonicalize` returns a `\\?\C:\...` verbatim path; strip
    // the prefix so addressed and canonical paths stay comparable.
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        text = stripped.to_string();
    }
    text
}

fn to_file_error(error: io::Error, path: &str) -> FileError {
    let code = match error.kind() {
        io::ErrorKind::NotFound => FileErrorCode::NotFound,
        io::ErrorKind::PermissionDenied => FileErrorCode::PermissionDenied,
        _ => FileErrorCode::Unknown,
    };
    FileError::new(code, error.to_string(), Some(path.to_string()))
        .with_cause(Some(Box::new(error)))
}

fn file_info_from_metadata(path: PathBuf, metadata: &fs::Metadata) -> FileInfo {
    let kind = if metadata.is_symlink() {
        FileKind::Symlink
    } else if metadata.is_dir() {
        FileKind::Directory
    } else {
        FileKind::File
    };
    FileInfo {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: display_path(&path),
        kind,
        size: metadata.len(),
        mtime_ms: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as f64)
            .unwrap_or(0.0),
    }
}

fn not_supported(operation: &str) -> FileError {
    FileError::new(
        FileErrorCode::NotSupported,
        format!("TestFsEnv does not implement {operation}"),
        None,
    )
}

impl FileSystem for TestFsEnv {
    fn cwd(&self) -> &str {
        &self.cwd
    }

    fn absolute_path<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let resolved = self.resolve(path);
        Box::pin(async move { Ok(display_path(&resolved)) })
    }

    fn join_path<'a>(
        &'a self,
        parts: &[String],
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let mut joined = PathBuf::new();
        for part in parts {
            joined.push(part);
        }
        Box::pin(async move { Ok(display_path(&joined)) })
    }

    fn read_text_file<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let resolved = self.resolve(path);
        let displayed = display_path(&resolved);
        Box::pin(async move {
            fs::read_to_string(&resolved).map_err(|error| to_file_error(error, &displayed))
        })
    }

    fn open_text_line_reader<'a>(
        &'a self,
        _path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<std::sync::Arc<dyn TextLineReader>, FileError>> {
        Box::pin(async move { Err(not_supported("open_text_line_reader")) })
    }

    fn read_text_lines<'a>(
        &'a self,
        _path: &str,
        _options: Option<&ReadTextLinesOptions>,
        _context: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        Box::pin(async move { Err(not_supported("read_text_lines")) })
    }

    fn read_binary_file<'a>(
        &'a self,
        _path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        Box::pin(async move { Err(not_supported("read_binary_file")) })
    }

    fn write_file<'a>(
        &'a self,
        _path: &str,
        _content: FileContent,
        _context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move { Err(not_supported("write_file")) })
    }

    fn append_file<'a>(
        &'a self,
        _path: &str,
        _content: FileContent,
        _context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move { Err(not_supported("append_file")) })
    }

    fn rename_file<'a>(
        &'a self,
        _source_path: &str,
        _destination_path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move { Err(not_supported("rename_file")) })
    }

    fn file_info<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        let resolved = self.resolve(path);
        let displayed = display_path(&resolved);
        Box::pin(async move {
            fs::symlink_metadata(&resolved)
                .map(|metadata| file_info_from_metadata(resolved, &metadata))
                .map_err(|error| to_file_error(error, &displayed))
        })
    }

    fn list_dir<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        let resolved = self.resolve(path);
        let displayed = display_path(&resolved);
        Box::pin(async move {
            let entries = fs::read_dir(&resolved)
                .and_then(|entries| {
                    entries
                        .map(|entry| {
                            let entry = entry?;
                            let entry_path = entry.path();
                            let metadata = fs::symlink_metadata(&entry_path)?;
                            Ok(file_info_from_metadata(entry_path, &metadata))
                        })
                        .collect::<Result<Vec<FileInfo>, io::Error>>()
                })
                .map_err(|error| to_file_error(error, &displayed))?;
            Ok(entries)
        })
    }

    fn canonical_path<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let resolved = self.resolve(path);
        let displayed = display_path(&resolved);
        Box::pin(async move {
            fs::canonicalize(&resolved)
                .map(|canonical| display_path(&canonical))
                .map_err(|error| to_file_error(error, &displayed))
        })
    }

    fn exists<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<bool, FileError>> {
        let resolved = self.resolve(path);
        let displayed = display_path(&resolved);
        Box::pin(async move {
            match fs::symlink_metadata(&resolved) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(to_file_error(error, &displayed)),
            }
        })
    }

    fn create_dir<'a>(
        &'a self,
        _path: &str,
        _options: Option<&CreateDirOptions>,
        _context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move { Err(not_supported("create_dir")) })
    }

    fn remove<'a>(
        &'a self,
        _path: &str,
        _options: Option<&RemoveOptions>,
        _context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move { Err(not_supported("remove")) })
    }

    fn create_temp_dir<'a>(
        &'a self,
        _prefix: Option<&str>,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move { Err(not_supported("create_temp_dir")) })
    }

    fn create_temp_file<'a>(
        &'a self,
        _options: Option<&TempFileOptions>,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move { Err(not_supported("create_temp_file")) })
    }

    fn cleanup<'a>(&'a self, _context: Context) -> BoxFuture<'a, ()> {
        Box::pin(async move {})
    }
}
