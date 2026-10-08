//! 腾讯 iLink 2.4.6 协议；媒体采用平台指定的 AES-128-ECB/PKCS7。
use super::{ChannelError, Resource, Result};
use aes::Aes128;
use base64::{engine::general_purpose::STANDARD, Engine};
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use md5::{Digest, Md5};
use reqwest::{Client, Response};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub(crate) const WEIXIN_API_BASE: &str = "https://ilinkai.weixin.qq.com";
pub(crate) const WEIXIN_CDN_BASE: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
const CLIENT_VERSION: &str = "132102";

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
pub(crate) fn string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        Value::Array(v) => v
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    string(v)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}
pub(crate) fn text(value: &Value, key: &str) -> String {
    if truthy(&value[key]) {
        string(&value[key])
    } else {
        String::new()
    }
}
pub(crate) fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub(crate) fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char)
        } else {
            out.push_str(&format!("%{byte:02X}"))
        }
    }
    out
}
pub(crate) fn error(message: impl Into<String>) -> ChannelError {
    ChannelError::new(message)
}
pub(crate) async fn cancelled<T>(
    token: &CancellationToken,
    timeout_ms: u64,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! { biased; _ = token.cancelled() => Err(error("渠道请求已取消。")), result = async { if timeout_ms == 0 { future.await } else { tokio::time::timeout(Duration::from_millis(timeout_ms), future).await.map_err(|_| error("渠道请求已超时。"))? } } => result }
}
pub(crate) async fn pause(token: &CancellationToken, ms: u64) {
    tokio::select! { _ = token.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(ms)) => {} }
}

/// 断开时等候已借出的发送/下载 future 退出；状态对象不保存任务句柄。
#[derive(Default)]
pub(crate) struct Operations {
    count: AtomicUsize,
    settled: Notify,
}
pub(crate) struct Lease(Arc<Operations>);
impl Operations {
    pub(crate) fn lease(self: &Arc<Self>) -> Lease {
        self.count.fetch_add(1, Ordering::AcqRel);
        Lease(self.clone())
    }
    pub(crate) async fn join(&self) {
        loop {
            let settled = self.settled.notified();
            if self.count.load(Ordering::Acquire) == 0 {
                return;
            }
            settled.await;
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.settled.notify_waiters();
        }
    }
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|e| error(e.to_string()))?;
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = value.as_bytes().chunks_exact(2);
    for pair in &mut chars {
        let Some(a) = (pair[0] as char).to_digit(16) else {
            break;
        };
        let Some(b) = (pair[1] as char).to_digit(16) else {
            break;
        };
        out.push((a * 16 + b) as u8);
    }
    out
}
fn permissive_base64(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut accumulator = 0u32;
    let mut bits = 0;
    for byte in value.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | digit as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
        accumulator &= (1 << bits) - 1;
    }
    out
}
pub(crate) fn parse_aes_key(value: &str) -> Result<Vec<u8>> {
    let decoded = permissive_base64(value);
    if decoded.len() == 16 {
        return Ok(decoded);
    }
    if decoded.len() == 32 && decoded.iter().all(u8::is_ascii_hexdigit) {
        return Ok(unhex(&String::from_utf8_lossy(&decoded)));
    }
    Err(error("微信媒体 AES 密钥格式不正确。"))
}
pub(crate) fn encrypt_aes_ecb(plaintext: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes128::new_from_slice(key).map_err(|_| error("微信媒体 AES 密钥格式不正确。"))?;
    let padding = 16 - plaintext.len() % 16;
    let mut out = plaintext.to_vec();
    out.resize(out.len() + padding, padding as u8);
    for block in out.chunks_exact_mut(16) {
        cipher.encrypt_block(cipher::generic_array::GenericArray::from_mut_slice(block));
    }
    Ok(out)
}
pub(crate) fn decrypt_aes_ecb(ciphertext: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(error("微信媒体 AES 密文格式不正确。"));
    }
    let cipher = Aes128::new_from_slice(key).map_err(|_| error("微信媒体 AES 密钥格式不正确。"))?;
    let mut out = ciphertext.to_vec();
    for block in out.chunks_exact_mut(16) {
        cipher.decrypt_block(cipher::generic_array::GenericArray::from_mut_slice(block));
    }
    let pad = *out.last().unwrap() as usize;
    if pad == 0 || pad > 16 || !out[out.len() - pad..].iter().all(|b| *b as usize == pad) {
        return Err(error("微信媒体 AES 填充格式不正确。"));
    }
    out.truncate(out.len() - pad);
    Ok(out)
}
fn padded_size(size: usize) -> usize {
    (size / 16 + 1) * 16
}
pub(crate) fn mime_from_name(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
}
pub(crate) fn text_from_items(items: &Value) -> String {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| match item["type"].as_i64() {
            Some(1) => Some(text(&item["text_item"], "text")),
            Some(3) => Some(text(&item["voice_item"], "text")),
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}
pub(crate) fn media_items(items: &Value) -> Vec<Value> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| matches!(v["type"].as_i64(), Some(2 | 4 | 5)))
        .cloned()
        .collect()
}

pub(crate) struct WeixinProtocol {
    client: Client,
    api_base: String,
    cdn_base: String,
}
impl WeixinProtocol {
    pub(crate) fn new() -> Self {
        Self::with_endpoints(
            Client::new(),
            WEIXIN_API_BASE.into(),
            WEIXIN_CDN_BASE.into(),
        )
    }
    pub(crate) fn with_endpoints(client: Client, api_base: String, cdn_base: String) -> Self {
        Self {
            client,
            api_base,
            cdn_base,
        }
    }
    pub(crate) fn api_base(&self) -> &str {
        &self.api_base
    }
    pub(crate) fn cdn_base(&self) -> &str {
        &self.cdn_base
    }
    fn common(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", CLIENT_VERSION)
    }
    async fn response_json(response: Response, label: &str) -> Result<Value> {
        let status = response.status();
        let body = response.text().await.map_err(|e| error(e.to_string()))?;
        if !status.is_success() {
            let mut e = error(format!(
                "{label} 请求失败（HTTP {}）：{}",
                status.as_u16(),
                body.chars().take(300).collect::<String>()
            ));
            e.status = Some(status.as_u16());
            return Err(e);
        }
        if body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&body).map_err(|_| error(format!("{label} 返回了无法解析的数据。")))
    }
    async fn request(
        &self,
        url: String,
        method: reqwest::Method,
        token: Option<&str>,
        body: Option<Value>,
        timeout: u64,
        cancellation: &CancellationToken,
        authenticated: bool,
        label: &str,
    ) -> Result<Value> {
        cancelled(cancellation, timeout, async {
            let mut request = self.common(self.client.request(method, url));
            if authenticated {
                let uin = u32::from_be_bytes(random::<4>()?);
                request = request
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("X-WECHAT-UIN", STANDARD.encode(uin.to_string()));
                if let Some(token) = token.filter(|s| !s.trim().is_empty()) {
                    request = request.bearer_auth(token.trim());
                }
            }
            if let Some(body) = body {
                request = request.body(body.to_string());
            }
            let response = request.send().await.map_err(|e| error(e.to_string()))?;
            Self::response_json(response, label).await
        })
        .await
    }
    pub(crate) async fn start_qr(
        &self,
        local_tokens: Value,
        token: &CancellationToken,
    ) -> Result<Value> {
        let values = local_tokens.as_array().cloned().unwrap_or_default();
        let start = values.len().saturating_sub(10);
        self.request(
            format!(
                "{}/ilink/bot/get_bot_qrcode?bot_type=3",
                self.api_base.trim_end_matches('/')
            ),
            reqwest::Method::POST,
            None,
            Some(json!({"local_token_list":&values[start..]})),
            0,
            token,
            true,
            "获取微信登录二维码",
        )
        .await
    }
    pub(crate) async fn poll_qr(
        &self,
        qrcode: &str,
        base_url: &str,
        verify_code: &str,
        token: &CancellationToken,
    ) -> Result<Value> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/ilink/bot/get_qrcode_status",
            base_url.trim_end_matches('/')
        ))
        .map_err(|e| error(e.to_string()))?;
        url.query_pairs_mut().append_pair("qrcode", qrcode);
        if !verify_code.is_empty() {
            url.query_pairs_mut()
                .append_pair("verify_code", verify_code);
        }
        let result = self
            .request(
                url.to_string(),
                reqwest::Method::GET,
                None,
                None,
                35_000,
                token,
                false,
                "查询微信扫码状态",
            )
            .await;
        match result {
            Err(e) if !token.is_cancelled() && e.message == "渠道请求已超时。" => {
                Ok(json!({"status":"wait"}))
            }
            other => other,
        }
    }
    pub(crate) async fn auth_post(
        &self,
        connection: &Value,
        endpoint: &str,
        mut body: Value,
        timeout: u64,
        token: &CancellationToken,
    ) -> Result<Value> {
        body["base_info"] = json!({"channel_version":"2.4.6","bot_agent":"Pisper/0.0.0"});
        let base = text(connection, "baseUrl");
        let base = if base.is_empty() {
            &self.api_base
        } else {
            &base
        };
        self.request(
            format!("{}/{}", base.strip_suffix('/').unwrap_or(base), endpoint),
            reqwest::Method::POST,
            Some(&text(connection, "token")),
            Some(body),
            timeout,
            token,
            true,
            "微信 iLink",
        )
        .await
    }
    pub(crate) async fn notify_start(
        &self,
        connection: &Value,
        token: &CancellationToken,
    ) -> Result<Value> {
        let result = self
            .auth_post(
                connection,
                "ilink/bot/msg/notifystart",
                json!({}),
                15_000,
                token,
            )
            .await?;
        if truthy(&result["ret"]) && result["ret"] != 0 {
            return Err(error(if truthy(&result["errmsg"]) {
                text(&result, "errmsg")
            } else {
                format!("微信连接失败（{}）", string(&result["ret"]))
            }));
        }
        Ok(result)
    }
    pub(crate) async fn notify_stop(&self, connection: &Value) -> Result<Value> {
        self.auth_post(
            connection,
            "ilink/bot/msg/notifystop",
            json!({}),
            8_000,
            &CancellationToken::new(),
        )
        .await
    }
    pub(crate) async fn get_updates(
        &self,
        connection: &Value,
        sync_buf: &Value,
        token: &CancellationToken,
        timeout: u64,
    ) -> Result<Value> {
        let result = self
            .auth_post(
                connection,
                "ilink/bot/getupdates",
                json!({"get_updates_buf":if truthy(sync_buf){sync_buf.clone()}else{json!("")}}),
                timeout,
                token,
            )
            .await;
        match result {
            Err(e) if token.is_cancelled() || e.message == "渠道请求已超时。" => {
                Ok(json!({"ret":0,"msgs":[],"get_updates_buf":sync_buf}))
            }
            other => other,
        }
    }
    fn check_send(result: &Value, media: bool) -> Result<()> {
        if truthy(&result["ret"]) && result["ret"] != 0 {
            if result["ret"] == -2 && result["errmsg"] == "prepare failed" {
                return Err(error(
                    "微信会话上下文已失效，请先在微信中向机器人发送一条消息后重试。",
                ));
            }
            return Err(error(if truthy(&result["errmsg"]) {
                text(result, "errmsg")
            } else {
                format!(
                    "微信{}发送失败（{}）",
                    if media { "媒体" } else { "消息" },
                    string(&result["ret"])
                )
            }));
        }
        Ok(())
    }
    async fn send_items(
        &self,
        connection: &Value,
        to: Value,
        items: Vec<Value>,
        context: Value,
        media: bool,
        token: &CancellationToken,
    ) -> Result<Value> {
        let client_id = format!("pisper-{}", uuid::Uuid::new_v4());
        let mut msg = json!({"from_user_id":"","to_user_id":to,"client_id":client_id,"message_type":2,"message_state":2,"item_list":items});
        if truthy(&context) {
            msg["context_token"] = context;
        }
        let result = self
            .auth_post(
                connection,
                "ilink/bot/sendmessage",
                json!({"msg":msg}),
                15_000,
                token,
            )
            .await?;
        Self::check_send(&result, media)?;
        Ok(json!({"messageId":client_id}))
    }
    pub(crate) async fn send_text(
        &self,
        connection: &Value,
        to: Value,
        content: Value,
        context: Value,
        token: &CancellationToken,
    ) -> Result<Value> {
        self.send_items(connection,to,vec![json!({"type":1,"text_item":{"text":if truthy(&content){string(&content)}else{String::new()}}})],context,false,token).await
    }
    pub(crate) async fn download_item(
        &self,
        connection: &Value,
        item: &Value,
        token: &CancellationToken,
    ) -> Result<Option<Resource>> {
        let kind = item["type"].as_i64();
        let id = if truthy(&item["msg_id"]) {
            string(&item["msg_id"])
        } else {
            now_ms().to_string()
        };
        let (media, name, mime, raw_key) = match kind {
            Some(2) => (
                &item["image_item"]["media"],
                format!("weixin-image-{id}.png"),
                "image/png".to_owned(),
                if truthy(&item["image_item"]["aeskey"]) {
                    Some(unhex(&text(&item["image_item"], "aeskey")))
                } else {
                    None
                },
            ),
            Some(4) => {
                let name = if truthy(&item["file_item"]["file_name"]) {
                    text(&item["file_item"], "file_name")
                } else {
                    format!("weixin-file-{}", now_ms())
                };
                let mime = mime_from_name(&name).to_owned();
                (&item["file_item"]["media"], name, mime, None)
            }
            Some(5) => (
                &item["video_item"]["media"],
                format!("weixin-video-{id}.mp4"),
                "video/mp4".to_owned(),
                None,
            ),
            _ => return Ok(None),
        };
        let base = text(connection, "cdnBaseUrl");
        let base = if base.is_empty() {
            &self.cdn_base
        } else {
            &base
        };
        let url = if truthy(&media["full_url"]) {
            text(media, "full_url")
        } else if truthy(&media["encrypt_query_param"]) {
            format!(
                "{}/download?encrypted_query_param={}",
                base.strip_suffix('/').unwrap_or(base),
                encode_component(&text(media, "encrypt_query_param"))
            )
        } else {
            return Ok(None);
        };
        let bytes = cancelled(token, 30_000, async {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| error(e.to_string()))?;
            if !response.status().is_success() {
                return Err(error(format!(
                    "微信附件下载失败（HTTP {}）",
                    response.status().as_u16()
                )));
            }
            response
                .bytes()
                .await
                .map(|v| v.to_vec())
                .map_err(|e| error(e.to_string()))
        })
        .await?;
        let key = match raw_key {
            Some(v) => Some(v),
            None if truthy(&media["aes_key"]) => Some(parse_aes_key(&text(media, "aes_key"))?),
            _ => None,
        };
        let bytes = if let Some(key) = key {
            decrypt_aes_ecb(&bytes, &key)?
        } else {
            bytes
        };
        let kind = if mime.starts_with("image/") {
            "image"
        } else if mime.starts_with("video/") {
            "video"
        } else {
            "file"
        };
        Ok(Some(Resource {
            name,
            kind: kind.into(),
            mime_type: Some(mime),
            bytes,
        }))
    }
    pub(crate) async fn send_media(
        &self,
        connection: &Value,
        to: Value,
        asset: &Value,
        context: Value,
        token: &CancellationToken,
    ) -> Result<Value> {
        let path = text(asset, "path");
        let plaintext = cancelled(token, 0, async {
            tokio::fs::read(&path)
                .await
                .map_err(|e| error(e.to_string()))
        })
        .await?;
        let key = random::<16>()?;
        let filekey = hex(&random::<16>()?);
        let name = text(asset, "name");
        let mime = text(asset, "mimeType");
        let mime = if mime.is_empty() {
            mime_from_name(if name.is_empty() { &path } else { &name }).to_owned()
        } else {
            mime
        };
        let result=self.auth_post(connection,"ilink/bot/getuploadurl",json!({"filekey":filekey,"media_type":if mime.starts_with("image/"){1}else if mime.starts_with("video/"){2}else{3},"to_user_id":to,"rawsize":plaintext.len(),"rawfilemd5":hex(&Md5::digest(&plaintext)),"filesize":padded_size(plaintext.len()),"no_need_thumb":true,"aeskey":hex(&key)}),15_000,token).await?;
        let base = text(connection, "cdnBaseUrl");
        let base = if base.is_empty() {
            &self.cdn_base
        } else {
            &base
        };
        let url = if truthy(&result["upload_full_url"]) {
            text(&result, "upload_full_url")
        } else if truthy(&result["upload_param"]) {
            format!(
                "{}/upload?encrypted_query_param={}&filekey={filekey}",
                base.strip_suffix('/').unwrap_or(base),
                encode_component(&text(&result, "upload_param"))
            )
        } else {
            return Err(error("微信媒体上传地址为空。"));
        };
        let encrypted = encrypt_aes_ecb(&plaintext, &key)?;
        let download_param = cancelled(token, 60_000, async {
            let response = self
                .client
                .post(url)
                .header("Content-Type", "application/octet-stream")
                .body(encrypted)
                .send()
                .await
                .map_err(|e| error(e.to_string()))?;
            if !response.status().is_success() {
                return Err(error(format!(
                    "微信媒体上传失败（HTTP {}）",
                    response.status().as_u16()
                )));
            }
            response
                .headers()
                .get("x-encrypted-param")
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| error("微信媒体上传响应缺少下载参数。"))
        })
        .await?;
        let media = json!({"encrypt_query_param":download_param,"aes_key":STANDARD.encode(hex(&key)),"encrypt_type":1});
        let item = if mime.starts_with("image/") {
            json!({"type":2,"image_item":{"media":media,"mid_size":padded_size(plaintext.len())}})
        } else if mime.starts_with("video/") {
            json!({"type":5,"video_item":{"media":media,"video_size":padded_size(plaintext.len())}})
        } else {
            json!({"type":4,"file_item":{"media":media,"file_name":if name.is_empty(){Path::new(&path).file_name().unwrap_or_default().to_string_lossy().to_string()}else{name},"len":plaintext.len().to_string()}})
        };
        self.send_items(connection, to, vec![item], context, true, token)
            .await
    }
}

#[cfg(test)]
#[path = "telegram_weixin_tests.rs"]
mod tests;
