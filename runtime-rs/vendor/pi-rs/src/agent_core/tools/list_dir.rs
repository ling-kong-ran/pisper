#[cfg(test)]
use super::run_tool_text;
use super::text_result;
use crate::agent_core::types::{make_tool, AgentTool};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct ListDirArgs {
    /// Directory to list.
    pub path: String,
}

pub fn tool() -> AgentTool {
    make_tool(
        "list_dir",
        "List the entries of a directory",
        |a: ListDirArgs| {
            Box::pin(async move {
                let mut out: Vec<String> = Vec::new();
                let entries =
                    std::fs::read_dir(&a.path).map_err(|e| anyhow::anyhow!("list failed: {e}"))?;
                for entry in entries {
                    let entry = entry.map_err(|e| anyhow::anyhow!("list failed: {e}"))?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    out.push(if is_dir { format!("{name}/") } else { name });
                }
                out.sort();
                if out.is_empty() {
                    Ok(text_result("(empty directory)"))
                } else {
                    Ok(text_result(out.join("\n")))
                }
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_and_marks_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let t = tool();
        let out = run_tool_text(
            &t,
            serde_json::json!({"path": dir.path().to_str().unwrap()}),
        )
        .await
        .unwrap();
        assert!(out.lines().any(|l| l == "b.txt"));
        assert!(out.lines().any(|l| l == "sub/"));
    }
}
