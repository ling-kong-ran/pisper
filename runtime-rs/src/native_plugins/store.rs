use super::{PluginError, Result, MAX_PLUGIN_BYTES, MAX_PLUGIN_FILES};
use icu_collator::Collator;
use icu_locale_core::Locale;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct FileSnapshot {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub permissions: fs::Permissions,
}
#[derive(Clone, Debug)]
pub(crate) struct Scan {
    pub digest: String,
    pub file_count: usize,
    pub byte_count: usize,
    pub files: Vec<FileSnapshot>,
}
pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
pub(crate) fn directory(path: &Path, error: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(PluginError::new(error));
    }
    Ok(())
}
pub(crate) fn scan(root: &Path, locale: &str) -> Result<Scan> {
    directory(root, "插件来源必须是不含符号链接的目录。")?;
    let locale: Locale = locale
        .parse()
        .map_err(|_| PluginError::new("插件目录排序 locale 无效。"))?;
    let collator = Collator::try_new(locale.into(), Default::default())
        .map_err(|error| PluginError::new(error.to_string()))?;
    let mut output = Scan {
        digest: String::new(),
        file_count: 0,
        byte_count: 0,
        files: Vec::new(),
    };
    let mut hash = Sha256::new();
    fn visit(
        root: &Path,
        directory: &Path,
        collator: &icu_collator::CollatorBorrowed<'_>,
        output: &mut Scan,
        hash: &mut Sha256,
    ) -> Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by(|left, right| {
            collator.compare(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| PluginError::new("插件目录边界无效。"))?
                .to_owned();
            let name = relative.to_string_lossy().replace('\\', "/");
            let metadata = fs::symlink_metadata(&path)?;
            if is_link(&metadata) {
                return Err(PluginError::new(format!(
                    "插件目录不能包含符号链接：{name}"
                )));
            }
            if metadata.is_dir() {
                visit(root, &path, collator, output, hash)?;
                continue;
            }
            if !metadata.is_file() {
                return Err(PluginError::new(format!(
                    "插件目录包含不支持的文件类型：{name}"
                )));
            }
            output.file_count += 1;
            if output.file_count > MAX_PLUGIN_FILES {
                return Err(PluginError::new("插件文件数不能超过 512。"));
            }
            if metadata.len() > (MAX_PLUGIN_BYTES - output.byte_count) as u64 {
                return Err(PluginError::new("插件目录大小不能超过 20 MB。"));
            }
            let bytes = fs::read(&path)?;
            output.byte_count += bytes.len();
            if output.byte_count > MAX_PLUGIN_BYTES {
                return Err(PluginError::new("插件目录大小不能超过 20 MB。"));
            }
            hash.update(name.as_bytes());
            hash.update([0]);
            hash.update(&bytes);
            hash.update([0]);
            output.files.push(FileSnapshot {
                path: relative,
                bytes,
                permissions: metadata.permissions(),
            });
        }
        Ok(())
    }
    visit(root, root, &collator, &mut output, &mut hash)?;
    output.digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(output)
}
pub(crate) fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if is_link(&metadata) || !metadata.is_file() {
            return Err(PluginError::new("插件状态文件必须是普通文件。"));
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| PluginError::new("插件状态目录无效。"))?;
    directory(parent, "插件状态目录必须是普通目录。")?;
    let stage = parent.join(format!(".pisper-plugins-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&stage, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(stage);
    }
    result
}
pub(crate) fn copy_snapshot(stage: &Path, scan: &Scan) -> Result<()> {
    fs::create_dir(stage)?;
    for entry in &scan.files {
        let path = stage.join(&entry.path);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| PluginError::new("插件文件路径无效。"))?,
        )?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&entry.bytes)?;
        file.set_permissions(entry.permissions.clone())?;
    }
    Ok(())
}
