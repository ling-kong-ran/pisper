use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::model::SessionSummary;

pub fn canonical_workspace(path: &Path) -> Result<PathBuf> {
    let workspace = path
        .canonicalize()
        .with_context(|| format!("workspace directory does not exist: {}", path.display()))?;
    if !workspace.is_dir() {
        bail!("workspace is not a directory: {}", workspace.display());
    }
    Ok(workspace)
}

pub fn same_workspace(value: &str, workspace: &Path) -> bool {
    let candidate = PathBuf::from(value);
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    workspace_keys_match(&candidate, &workspace)
}

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
