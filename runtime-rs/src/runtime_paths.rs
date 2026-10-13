use std::path::PathBuf;

pub(crate) struct RuntimePaths {
    pub(crate) agent_dir: String,
    pub(crate) data_dir: String,
}

impl RuntimePaths {
    pub(crate) fn from_environment() -> anyhow::Result<Self> {
        Self::resolve(
            std::env::var_os("PISPER_AGENT_DIR").map(PathBuf::from),
            std::env::home_dir(),
        )
    }

    fn resolve(pisper: Option<PathBuf>, home: Option<PathBuf>) -> anyhow::Result<Self> {
        let agent = pisper
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| home.map(|home| home.join(".pisper").join("agent")))
            .ok_or_else(|| anyhow::anyhow!("Pisper agent directory cannot be resolved"))?;
        // 与 Node resolveAgentDataDir 一致：只有 PISPER_AGENT_DIR 可以覆盖应用目录。
        // Pi 的变量由入口向引擎传递，不能反向改变用户数据的来源。
        let agent = std::path::absolute(agent)?;
        Ok(Self {
            agent_dir: agent.to_string_lossy().into_owned(),
            data_dir: agent.to_string_lossy().into_owned(),
        })
    }

    pub(crate) fn prepare(&self) -> std::io::Result<()> {
        // 首次启动也使用原位置；仅创建缺少的目录，不迁移或覆盖已有内容。
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.agent_dir)?;
        builder.create(&self.data_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reuses_original_pisper_data() {
        let paths =
            RuntimePaths::resolve(None, Some(std::env::temp_dir().join("user-home"))).unwrap();
        assert_eq!(
            PathBuf::from(paths.agent_dir),
            std::env::temp_dir()
                .join("user-home")
                .join(".pisper")
                .join("agent")
        );
        assert_eq!(
            paths.data_dir,
            std::env::temp_dir()
                .join("user-home")
                .join(".pisper")
                .join("agent")
                .to_string_lossy()
        );
    }

    #[test]
    fn explicit_directory_is_absolute_and_shared_by_all_domains() {
        let paths = RuntimePaths::resolve(Some(PathBuf::from("test-agent")), None).unwrap();
        assert_eq!(
            PathBuf::from(&paths.agent_dir),
            std::env::current_dir().unwrap().join("test-agent")
        );
        assert_eq!(paths.data_dir, paths.agent_dir);
        let empty =
            RuntimePaths::resolve(Some(PathBuf::new()), Some(std::env::temp_dir())).unwrap();
        assert_eq!(
            PathBuf::from(empty.agent_dir),
            std::env::temp_dir().join(".pisper/agent")
        );
    }

    #[test]
    fn prepare_preserves_existing_data_and_rejects_files() {
        let root = std::env::temp_dir().join(format!("pisper-paths-{}", crate::product::new_id()));
        let paths = RuntimePaths::resolve(None, Some(root.clone())).unwrap();
        assert!(!root.exists());
        paths.prepare().unwrap();
        let sentinel = PathBuf::from(&paths.agent_dir).join("pisper.json");
        std::fs::write(&sentinel, b"{\"keep\":true}").unwrap();
        paths.prepare().unwrap();
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"{\"keep\":true}");
        let blocked = RuntimePaths::resolve(Some(sentinel), None).unwrap();
        assert!(blocked.prepare().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_workspace_prefixes_share_valid_session_storage() {
        use pi_rust::coding_agent::session_manager::{
            get_default_session_dir_with, SessionManager,
        };
        let root =
            std::env::temp_dir().join(format!("pisper-session-path-{}", crate::product::new_id()));
        let workspace = root.join("workspace with spaces");
        std::fs::create_dir_all(&workspace).unwrap();
        let agent = root.join("agent").to_string_lossy().into_owned();
        let native = workspace
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // Windows CI 的 TEMP 可能使用 8.3 短名；规范化同时展开短名，
        // 因而两种输入必须取自同一个规范路径，只改变待验证的命名空间前缀。
        let regular = if let Some(unc) = native.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            native
                .strip_prefix(r"\\?\")
                .expect("The fixture must have a Windows verbatim path")
                .to_owned()
        };
        let regular_dir = get_default_session_dir_with(&regular, &agent);
        let native_dir = get_default_session_dir_with(&native, &agent);
        let native_directory_exists = std::path::Path::new(&native_dir).is_dir();
        let session = SessionManager::create(&native, Some(&native_dir), None);
        let preserved_cwd = session
            .as_ref()
            .ok()
            .map(|session| session.get_cwd().to_owned());
        let created = session.is_ok();
        drop(session);
        // UNC 的输入仅用于目录键编码；实际创建的目录都在本地隔离 agent 下。
        let unc_dir = get_default_session_dir_with(r"\\pisper-fixture\share\workspace", &agent);
        let native_unc_dir =
            get_default_session_dir_with(r"\\?\UNC\pisper-fixture\share\workspace", &agent);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            native_dir, regular_dir,
            "Native and ordinary paths name the same workspace"
        );
        assert!(
            native_directory_exists,
            "The encoded directory must be a valid Windows path"
        );
        assert!(
            created,
            "The real SDK must create its persistent session manager"
        );
        assert_eq!(
            preserved_cwd.unwrap(),
            native,
            "Keep the original filesystem path for IO"
        );
        assert_eq!(native_unc_dir, unc_dir);
    }
}
