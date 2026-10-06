#[cfg(test)]
use super::run_tool_text;
use super::text_result;
use crate::agent_core::types::{make_tool, AgentTool};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct EditFileArgs {
    /// Path to the file to edit.
    pub path: String,
    /// Exact text to replace. Must appear exactly once in the file.
    pub old_string: String,
    /// Replacement text.
    pub new_string: String,
}

pub fn tool() -> AgentTool {
    make_tool(
        "edit_file",
        "Replace an exact, unique occurrence of text in a file",
        |a: EditFileArgs| {
            Box::pin(async move {
                let content = std::fs::read_to_string(&a.path)
                    .map_err(|e| anyhow::anyhow!("read failed: {e}"))?;
                let count = content.matches(&a.old_string).count();
                if count == 0 {
                    anyhow::bail!("old_string not found in file");
                }
                if count > 1 {
                    anyhow::bail!("old_string appears {count} times; it must be unique");
                }
                let updated = content.replacen(&a.old_string, &a.new_string, 1);
                std::fs::write(&a.path, updated)
                    .map_err(|e| anyhow::anyhow!("write failed: {e}"))?;
                Ok(text_result(format!("edited {}", a.path)))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one two two").unwrap();
        let t = tool();

        let dup = run_tool_text(
            &t,
            serde_json::json!({
                "path": path.to_str().unwrap(), "old_string": "two", "new_string": "three"
            }),
        )
        .await;
        assert!(dup.unwrap_err().to_string().contains("2 times"));

        let ok = run_tool_text(
            &t,
            serde_json::json!({
                "path": path.to_str().unwrap(), "old_string": "one", "new_string": "uno"
            }),
        )
        .await;
        assert!(ok.is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "uno two two");
    }
}
