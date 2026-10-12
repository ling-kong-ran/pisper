use std::path::PathBuf;

pub(crate) struct RuntimePaths {
    pub(crate) agent_dir: String,
    pub(crate) data_dir: String,
}

impl RuntimePaths {
    pub(crate) fn from_environment() -> anyhow::Result<Self> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        Self::resolve(
            std::env::var_os("PISPER_AGENT_DIR").map(PathBuf::from),
            std::env::var_os("PI_CODING_AGENT_DIR").map(PathBuf::from),
            std::env::var_os("PISPER_RS_DATA_DIR").map(PathBuf::from),
            home.map(PathBuf::from),
        )
    }

    fn resolve(
        pisper: Option<PathBuf>,
        pi: Option<PathBuf>,
        product: Option<PathBuf>,
        home: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let agent = pisper
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| pi.filter(|path| !path.as_os_str().is_empty()))
            .or_else(|| home.map(|home| home.join(".pisper").join("agent")))
            .ok_or_else(|| anyhow::anyhow!("Pisper agent directory cannot be resolved"))?;
        let data = product
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| agent.clone());
        Ok(Self {
            agent_dir: agent.to_string_lossy().into_owned(),
            data_dir: data.to_string_lossy().into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reuses_original_pisper_data() {
        let paths =
            RuntimePaths::resolve(None, None, None, Some(PathBuf::from("user-home"))).unwrap();
        assert_eq!(
            PathBuf::from(paths.agent_dir),
            PathBuf::from("user-home").join(".pisper").join("agent")
        );
        assert_eq!(
            paths.data_dir,
            PathBuf::from("user-home")
                .join(".pisper")
                .join("agent")
                .to_string_lossy()
        );
    }

    #[test]
    fn explicit_test_and_product_paths_stay_isolated() {
        let paths = RuntimePaths::resolve(
            None,
            Some(PathBuf::from("test-agent")),
            Some(PathBuf::from("test-product")),
            None,
        )
        .unwrap();
        assert_eq!(paths.agent_dir, "test-agent");
        assert_eq!(paths.data_dir, "test-product");
        let paths = RuntimePaths::resolve(
            Some(PathBuf::from("pisper-agent")),
            Some(PathBuf::from("pi-agent")),
            None,
            None,
        )
        .unwrap();
        assert_eq!(paths.agent_dir, "pisper-agent");
        assert_eq!(paths.data_dir, "pisper-agent");
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
