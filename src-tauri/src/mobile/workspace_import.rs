use std::path::{Component, Path, PathBuf};

pub(crate) fn ensure_local_import(mode: Option<&str>, running: bool) -> Result<(), String> {
    if mode != Some("local") {
        return Err("只能在本机模式导入工作区。".into());
    }
    if !running {
        return Err("本机 Runtime 尚未运行。".into());
    }
    Ok(())
}

pub(crate) fn resolve_imported_workspace(
    host_root: &Path,
    imported: &Path,
    runtime_root: Option<&Path>,
) -> Result<PathBuf, String> {
    // 先拒绝原始路径中的上跳，再用规范路径阻止符号链接逃出应用工作区。
    if !imported.is_absolute()
        || imported
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("导入结果不是有效的绝对工作区路径。".into());
    }
    let root = host_root
        .canonicalize()
        .map_err(|error| format!("无法验证工作区根目录：{error}"))?;
    let path = imported
        .canonicalize()
        .map_err(|error| format!("无法验证导入目录：{error}"))?;
    if !path.is_dir() {
        return Err("导入结果不是文件夹。".into());
    }
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "导入目录不在应用工作区内。".to_string())?;
    if relative.as_os_str().is_empty() {
        return Err("不能把工作区根目录作为导入结果。".into());
    }
    Ok(match runtime_root {
        Some(runtime_root) => runtime_root.join(relative),
        None => path,
    })
}

#[cfg(test)]
mod tests {
    use super::{ensure_local_import, resolve_imported_workspace};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pisper-workspace-import-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join("workspace/project/nested")).unwrap();
            std::fs::create_dir_all(root.join("workspace-other/outside")).unwrap();
            std::fs::write(root.join("workspace/file.txt"), "test").unwrap();
            Self(root)
        }

        fn root(&self) -> PathBuf {
            self.0.join("workspace")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn import_requires_explicit_local_mode_and_running_runtime() {
        assert!(ensure_local_import(Some("local"), true).is_ok());
        for mode in [None, Some("remote"), Some("")] {
            assert!(ensure_local_import(mode, true).is_err());
        }
        assert!(ensure_local_import(Some("local"), false).is_err());
    }

    #[test]
    fn embedded_import_returns_canonical_host_directory() {
        let fixture = Fixture::new();
        let imported = fixture.root().join("project/nested");
        assert_eq!(
            resolve_imported_workspace(&fixture.root(), &imported, None).unwrap(),
            imported.canonicalize().unwrap()
        );
    }

    #[test]
    fn root_import_maps_only_the_validated_relative_directory() {
        let fixture = Fixture::new();
        assert_eq!(
            resolve_imported_workspace(
                &fixture.root(),
                &fixture.root().join("project/nested"),
                Some(Path::new("/workspace")),
            )
            .unwrap(),
            Path::new("/workspace").join("project/nested")
        );
    }

    #[test]
    fn rejects_root_outside_traversal_files_missing_and_nonabsolute_paths() {
        let fixture = Fixture::new();
        for imported in [
            fixture.root(),
            fixture.0.join("workspace-other/outside"),
            fixture.root().join("project/../project/nested"),
            fixture.root().join("../workspace-other/outside"),
            fixture.root().join("file.txt"),
            fixture.root().join("missing"),
            PathBuf::from("project"),
            PathBuf::from("content://provider/tree/project"),
        ] {
            assert!(
                resolve_imported_workspace(&fixture.root(), &imported, None).is_err(),
                "accepted invalid path: {imported:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_and_root_alias() {
        let fixture = Fixture::new();
        for (name, target) in [
            ("outside-link", fixture.0.join("workspace-other/outside")),
            ("root-link", fixture.root()),
        ] {
            let link = fixture.root().join(name);
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert!(resolve_imported_workspace(&fixture.root(), &link, None).is_err());
        }
    }
}
