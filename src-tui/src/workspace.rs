//! 工作区路径的校验与比较。
//!
//! 会话的工作区是安全边界：附件、工具调用都以它为准。sidecar 返回的 cwd
//! 必须真实存在、与启动时请求的一致，否则会话不能继续，防止被恶意或错误
//! 的 sidecar 数据把会话导向别的工作区。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::model::SessionSummary;

/// 解析并验证工作区路径：必须是真实存在的目录。
/// 路径统一规范化（canonicalize），后续所有比较都基于解析后的绝对路径，
/// 避免符号链接或相对路径造成「看起来相同、实则不同」的工作区。
pub fn canonical_workspace(path: &Path) -> Result<PathBuf> {
    let workspace = path
        .canonicalize()
        .with_context(|| format!("workspace directory does not exist: {}", path.display()))?;
    if !workspace.is_dir() {
        bail!("workspace is not a directory: {}", workspace.display());
    }
    Ok(workspace)
}

/// 判断一个字符串路径与工作区是否指向同一位置。
/// 用于比对 sidecar 上报的 cwd 与本地期望值；路径不存在时退化为字面量比较，
/// 尽量给出一个合理结论而不是直接失败。
pub fn same_workspace(value: &str, workspace: &Path) -> bool {
    let candidate = PathBuf::from(value);
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    workspace_keys_match(&candidate, &workspace)
}

/// 校验会话工作区：sidecar 返回的 cwd 不能为空、必须可解析；
/// 若调用方显式指定了工作区，两者必须一致。
/// 不一致时直接报错，让调用方（如会话恢复、创建会话）拒绝继续。
pub fn validate_session_workspace(
    session: &SessionSummary,
    requested: Option<&Path>,
) -> Result<PathBuf> {
    if session.cwd.trim().is_empty() {
        bail!(
            "sidecar returned an empty workspace for session {}",
            session.id
        );
    }
    let actual = canonical_workspace(Path::new(&session.cwd)).with_context(|| {
        format!(
            "sidecar returned an invalid workspace for session {}",
            session.id
        )
    })?;
    if let Some(requested) = requested {
        let requested = canonical_workspace(requested)?;
        if !workspace_keys_match(&actual, &requested) {
            bail!(
                "sidecar workspace mismatch: requested {}, returned {}",
                requested.display(),
                actual.display()
            );
        }
    }
    Ok(actual)
}

/// 工作区键比较：Windows 文件系统不区分大小写，需忽略大小写；
/// 其余平台直接按字节比较。
fn workspace_keys_match(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::{same_workspace, validate_session_workspace};
    use crate::model::SessionSummary;

    /// 验证工作区校验：目录必须存在且与请求一致，缺失时给出明确错误。
    #[test]
    fn session_workspace_must_exist_and_match_the_request() {
        let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
        let session = SessionSummary {
            id: "session-1".to_owned(),
            cwd: workspace.to_string_lossy().into_owned(),
            ..SessionSummary::default()
        };

        assert_eq!(
            validate_session_workspace(&session, Some(&workspace)).unwrap(),
            workspace
        );
        assert!(same_workspace(&session.cwd, &workspace));

        let parent = workspace.parent().unwrap();
        assert!(validate_session_workspace(&session, Some(parent))
            .unwrap_err()
            .to_string()
            .contains("workspace mismatch"));

        let empty = SessionSummary {
            id: "session-2".to_owned(),
            ..SessionSummary::default()
        };
        assert!(validate_session_workspace(&empty, None)
            .unwrap_err()
            .to_string()
            .contains("empty workspace"));
    }

    /// 验证 Windows 下工作区路径比较不区分大小写。
    #[cfg(windows)]
    #[test]
    fn windows_workspace_comparison_is_case_insensitive() {
        let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert!(same_workspace(
            &workspace.to_string_lossy().to_uppercase(),
            &workspace
        ));
    }
}
