#[cfg(test)]
use super::run_tool_text;
use super::text_result;
use crate::agent_core::types::{make_tool, AgentTool};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// Path to the file, relative or absolute.
    pub path: String,
}

pub fn tool() -> AgentTool {
    make_tool(
        "read_file",
        "Read a text file from disk and return its contents",
        |a: ReadFileArgs| {
            Box::pin(async move {
                let content = std::fs::read_to_string(&a.path)
                    .map_err(|e| anyhow::anyhow!("read failed: {e}"))?;
                Ok(text_result(content))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_and_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "hello").unwrap();
        let t = tool();
        let ok = run_tool_text(&t, serde_json::json!({"path": path.to_str().unwrap()})).await;
        assert_eq!(ok.unwrap(), "hello");

        let missing = run_tool_text(&t, serde_json::json!({"path": "/nonexistent/x.txt"})).await;
        assert!(missing.unwrap_err().to_string().contains("read failed"));
    }
}
