//! 附件归档和模型输入准备独立于 Chat；文档解析运行在 blocking pool，调用者等待收尾。
#[path = "documents.rs"]
pub mod documents;
use super::store::{content, AssetStore};
use anyhow::{bail, Context, Result};
use pi_rust::ai::types::ImageContent;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct PreparedAttachments {
    pub images: Vec<ImageContent>,
    pub contexts: Vec<String>,
}
pub async fn prepare(
    store: Arc<Mutex<AssetStore>>,
    session_id: String,
    session_name: String,
    attachments: Vec<Value>,
    cancel: CancellationToken,
) -> Result<PreparedAttachments> {
    // 等待 blocking task 是所有权边界：取消后也不留下仍在解析或写盘的 detached task。
    tokio::task::spawn_blocking(move || {
        prepare_sync(&store, &session_id, &session_name, &attachments, &cancel)
    })
    .await
    .context("Attachment preparation task failed")?
}
pub fn prepare_sync(
    store: &Mutex<AssetStore>,
    session_id: &str,
    session_name: &str,
    attachments: &[Value],
    cancel: &CancellationToken,
) -> Result<PreparedAttachments> {
    let attachments = &attachments[..attachments.len().min(8)];
    let mut archived = Vec::new();
    for attachment in attachments {
        if cancel.is_cancelled() {
            bail!("Attachment preparation cancelled");
        }
        if attachment["kind"] == "path" {
            archived.push(None);
            continue;
        }
        let mut input = json!({"name":attachment["name"],"mimeType":attachment["mimeType"],"data":attachment["data"],"source":"attachment","sessionId":session_id,"sessionName":session_name});
        if attachment["kind"] == "text" {
            input["text"] = attachment["text"].clone();
        }
        let result = store
            .lock()
            .map_err(|_| anyhow::anyhow!("Asset store lock failed"))
            .and_then(|mut store| {
                let asset = store.create(&input)?;
                store.download(asset["id"].as_str().context("Asset id missing")?)
            });
        archived.push(result.ok().flatten().map(|download| download.path));
    }
    let mut prepared = PreparedAttachments {
        images: Vec::new(),
        contexts: Vec::new(),
    };
    for (index, attachment) in attachments.iter().enumerate() {
        if cancel.is_cancelled() {
            bail!("Attachment preparation cancelled");
        }
        let name = content::safe_name(attachment["name"].as_str().unwrap_or(""));
        match attachment["kind"].as_str().unwrap_or("") {
            "path" => {
                let path = attachment["path"].as_str().unwrap_or("").trim();
                if path.is_empty() || !Path::new(path).is_absolute() {
                    bail!("{name} 不是有效的本地绝对路径");
                }
                prepared.contexts.push(format!("[Local path attachment] {name}\nPath: {path}\nThis attachment is a path reference only. Read it with available workspace tools when needed."));
            }
            "image" => {
                let data = attachment["data"].as_str().unwrap_or("");
                let mime = attachment["mimeType"].as_str().unwrap_or("");
                if !mime.starts_with("image/") || data.is_empty() {
                    bail!("{name} 不是有效图片");
                }
                if data.encode_utf16().count() > 15_000_000 {
                    bail!("{name} 图片数据过大");
                }
                prepared.images.push(ImageContent {
                    data: data.to_owned(),
                    mime_type: mime.to_owned(),
                });
                let path = archived[index].as_ref().map(|path| format!("\nLocal path: {}\nTo edit this image, pass this path in generate_visual sourceImages.",path.to_string_lossy())).unwrap_or_default();
                prepared
                    .contexts
                    .push(format!("[Image attachment] {name}{path}"));
            }
            "text" => {
                let text =
                    content::truncate_utf16(attachment["text"].as_str().unwrap_or(""), 400000);
                let truncated = if attachment["truncated"].as_bool().unwrap_or(false) {
                    "\n(Content truncated)"
                } else {
                    ""
                };
                prepared
                    .contexts
                    .push(format!("[Text attachment: {name}]\n{text}{truncated}"));
            }
            "document" => {
                let data = content::decode_base64(attachment["data"].as_str().unwrap_or(""));
                if data.is_empty() {
                    bail!("{name} 内容为空");
                }
                let extension = attachment["extension"]
                    .as_str()
                    .unwrap_or("")
                    .trim_start_matches('.')
                    .to_lowercase();
                let text = documents::extract(&data, &extension, cancel)?;
                if text.trim().is_empty() {
                    bail!("{name} 未提取到可分析文本");
                }
                prepared.contexts.push(format!(
                    "[Document attachment: {name}]\n{}",
                    content::truncate_utf16(&text, 400000)
                ));
            }
            _ => {}
        }
    }
    if cancel.is_cancelled() {
        bail!("Attachment preparation cancelled");
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_archive_paths_text_and_limits_match_release() {
        let root =
            std::env::temp_dir().join(format!("pisper-attachment-test-{}", uuid::Uuid::new_v4()));
        let store = Mutex::new(AssetStore::open(&root).unwrap());
        let input = vec![
            json!({"kind":"text","name":"note.txt","text":"Synthetic fixture","truncated":true}),
            json!({"kind":"image","name":"fixture.png","mimeType":"image/png","data":"YWJj"}),
            json!({"kind":"path","name":"local.txt","path":root.join("local.txt")}),
        ];
        let result = prepare_sync(
            &store,
            "synthetic-session",
            "Synthetic session",
            &input,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(
            result.contexts[0],
            "[Text attachment: note.txt]\nSynthetic fixture\n(Content truncated)"
        );
        assert!(result.contexts[1].contains("Local path:"));
        assert!(result.contexts[2].ends_with("Read it with available workspace tools when needed."));
        drop(store);
        let mut reopened = AssetStore::open(&root).unwrap();
        assert_eq!(reopened.list("", "", "").unwrap().len(), 2);
        let token = CancellationToken::new();
        token.cancel();
        assert!(prepare_sync(&Mutex::new(reopened), "s", "s", &input, &token).is_err());
        fs_cleanup(&root);
    }
    fn fs_cleanup(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
    }
}
