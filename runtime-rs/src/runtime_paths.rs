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
}
