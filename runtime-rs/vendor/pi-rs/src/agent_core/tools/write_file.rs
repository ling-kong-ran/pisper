#[cfg(test)]
use super::run_tool_text;
use super::text_result;
use crate::agent_core::types::{make_tool, AgentTool};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// Path to write to. Parent directories are created if needed.
    pub path: String,
    /// Full content to write (replaces the file).
    pub content: String,
}

pub fn tool() -> AgentTool {
    make_tool(
        "write_file",
        "Write content to a file, replacing it entirely",
        |a: WriteFileArgs| {
            Box::pin(async move {
                if let Some(parent) = std::path::Path::new(&a.path).parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| anyhow::anyhow!("mkdir failed: {e}"))?;
                }
                let bytes = a.content.len();
                std::fs::write(&a.path, &a.content)
                    .map_err(|e| anyhow::anyhow!("write failed: {e}"))?;
                Ok(text_result(format!("wrote {bytes} bytes to {}", a.path)))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/dir/a.txt");
        let t = tool();
        let out = run_tool_text(
            &t,
            serde_json::json!({"path": path.to_str().unwrap(), "content": "abc"}),
        )
        .await
        .unwrap();
        assert!(out.contains("3 bytes"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
    }
}
