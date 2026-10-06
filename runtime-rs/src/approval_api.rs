//! Real suspended tool approvals. No execution is resumed until a matching user decision settles it.
use crate::{execution_modes, ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use pi_rust::{
    agent_core::{
        harness::tools::edit_diff::{
            apply_edits_to_normalized_content, generate_unified_patch, normalize_to_lf, strip_bom,
            Edit,
        },
        types::BeforeToolCallResult,
    },
    ai::now_ms,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    io::Write,
    path::{Path as FsPath, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::oneshot;

pub(crate) type CancelFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type Listener = Arc<dyn Fn(&str, &str, &Value) + Send + Sync>;
const RESOLVED_TTL: i64 = 5 * 60_000;
const MAX_RESOLVED: usize = 256;
const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
const MAX_DIFF_CHARS: usize = 200_000;

pub(crate) struct AuthorizationRequest {
    pub session_id: String,
    pub cwd: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub args: Value,
    pub permission_mode: String,
    pub execution_mode: String,
    pub owned_files: Vec<String>,
}
struct Pending {
    public: Value,
    tx: oneshot::Sender<Value>,
}
#[derive(Default)]
struct SecureRefs {
    refs: HashSet<String>,
    focus_secure: bool,
}
struct Store {
    pending: HashMap<String, Pending>,
    resolved: BTreeMap<(i64, String), Value>,
    document: Value,
    listeners: HashMap<u64, (Option<String>, Listener)>,
    next_listener: u64,
    risks: HashMap<String, String>,
    secure: HashMap<String, SecureRefs>,
    closed: bool,
}
pub(crate) struct ApprovalService {
    path: PathBuf,
    store: Mutex<Store>,
    timeout: Duration,
}
pub(crate) struct ApprovalSubscription {
    service: Weak<ApprovalService>,
    id: u64,
}
impl Drop for ApprovalSubscription {
    fn drop(&mut self) {
        if let Some(service) = self.service.upgrade() {
            service
                .store
                .lock()
                .expect("approval store")
                .listeners
                .remove(&self.id);
        }
    }
}

fn timestamp() -> String {
    pi_rust::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms() as i64)
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn stable(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let ordered: BTreeMap<_, _> = object
                .iter()
                .map(|(key, value)| (key.clone(), stable(value)))
                .collect();
            serde_json::to_value(ordered).expect("stable JSON")
        }
        Value::Array(items) => json!(items.iter().map(stable).collect::<Vec<_>>()),
        _ => value.clone(),
    }
}
fn approval_key(request: &AuthorizationRequest) -> String {
    let args = if request.tool_name == "bash" {
        json!({"command":request.args["command"].as_str().unwrap_or_default()})
    } else {
        stable(&request.args)
    };
    // Node resolve(cwd) is lexical rather than realpath; retain its per-workspace key semantics.
    let cwd = FsPath::new(&request.cwd).to_path_buf();
    hash(
        serde_json::to_vec(&json!([cwd.to_string_lossy(), request.tool_name, args]))
            .expect("approval key"),
    )
}
fn safe_args(value: &Value, depth: usize, key: &str) -> Value {
    if depth > 3 {
        return json!("[内容已省略]");
    }
    let lower = key.to_ascii_lowercase();
    if [
        "apikey", "api_key", "api-key", "password", "passwd", "secret", "token",
    ]
    .iter()
    .any(|part| lower.contains(part))
    {
        return json!("[已隐藏敏感信息]");
    }
    match value {
        Value::String(text)
            if matches!(lower.as_str(), "data" | "image" | "content")
                && text.chars().count() > 500 =>
        {
            json!(format!("[内容已省略，共 {} 字符]", text.chars().count()))
        }
        Value::String(text) if text.chars().count() > 800 => {
            json!(format!("{}…", text.chars().take(800).collect::<String>()))
        }
        Value::Array(items) => json!(items
            .iter()
            .take(12)
            .map(|item| safe_args(item, depth + 1, key))
            .collect::<Vec<_>>()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .take(30)
                .map(|(key, child)| (key.clone(), safe_args(child, depth + 1, key)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[derive(Clone)]
struct FilePreview {
    public: Value,
    source_hash: String,
    exists: bool,
    path: PathBuf,
}
fn preview(request: &AuthorizationRequest) -> anyhow::Result<FilePreview> {
    let raw = request.args["path"]
        .as_str()
        .or_else(|| request.args["file_path"].as_str())
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("文件修改参数缺少 path，无法生成预览。"))?;
    let path = execution_modes::absolute(&request.cwd, raw);
    let (source, exists) = match std::fs::read(&path) {
        Ok(bytes) => (bytes, true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (vec![], false),
        Err(error) => return Err(error.into()),
    };
    if source.len() > MAX_PREVIEW_BYTES {
        anyhow::bail!("当前文件超过 2 MB，无法安全生成修改预览。");
    }
    let text = std::str::from_utf8(&source)?;
    let mut before = normalize_to_lf(text);
    let after = if request.tool_name == "edit" {
        if !exists {
            anyhow::bail!("无法编辑文件：{raw} 不存在。");
        }
        let edits_value = if let Some(raw) = request.args["edits"].as_str() {
            serde_json::from_str(raw)?
        } else {
            request.args["edits"].clone()
        };
        let edits: Vec<Edit> = if edits_value.is_array() {
            serde_json::from_value(edits_value)?
        } else {
            vec![Edit {
                old_text: request.args["oldText"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("编辑参数无效，缺少 oldText。"))?
                    .into(),
                new_text: request.args["newText"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("编辑参数无效，缺少 newText。"))?
                    .into(),
            }]
        };
        if edits.is_empty() {
            anyhow::bail!("编辑参数无效，无法生成修改预览。");
        }
        let result =
            apply_edits_to_normalized_content(&normalize_to_lf(strip_bom(text).1), &edits, raw)?;
        before = result.base_content;
        result.new_content
    } else {
        normalize_to_lf(
            request.args["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("写入参数无效，缺少 content。"))?,
        )
    };
    if after.len() > MAX_PREVIEW_BYTES {
        anyhow::bail!("拟写入内容超过 2 MB，无法安全生成修改预览。");
    }
    let root = execution_modes::canonical(FsPath::new(&request.cwd));
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| anyhow::anyhow!("文件路径无效，无法生成修改预览。"))?
        .to_string_lossy()
        .replace('\\', "/");
    if relative.is_empty() || relative.contains(['\r', '\n', '\0']) {
        anyhow::bail!("文件路径无效，无法生成修改预览。");
    }
    let quote = |text: String| {
        if text
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\'))
        {
            serde_json::to_string(&text).expect("diff path")
        } else {
            text
        }
    };
    let old_path = if exists {
        quote(format!("a/{relative}"))
    } else {
        "/dev/null".into()
    };
    let new_path = quote(format!("b/{relative}"));
    let patch = generate_unified_patch(&relative, &before, &after, 3);
    let patch = patch
        .lines()
        .enumerate()
        .map(|(index, line)| match index {
            0 => format!("--- {old_path}"),
            1 => format!("+++ {new_path}"),
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let diff = format!(
        "diff --git {} {new_path}\n{}{}",
        quote(format!("a/{relative}")),
        if exists { "" } else { "new file mode 100644\n" },
        patch
    );
    Ok(FilePreview {
        public: json!({"path":relative,"diff":diff.chars().take(MAX_DIFF_CHARS).collect::<String>(),"truncated":diff.chars().count()>MAX_DIFF_CHARS}),
        source_hash: hash(&source),
        exists,
        path,
    })
}
fn unchanged(original: &FilePreview, request: &AuthorizationRequest) -> anyhow::Result<bool> {
    // Resolve the requested path again: a symlink may change while approval is pending.
    let current = preview(request)?;
    Ok(original.path == current.path
        && original.exists == current.exists
        && original.source_hash == current.source_hash)
}

impl ApprovalService {
    pub(crate) fn new(path: impl Into<PathBuf>) -> anyhow::Result<Arc<Self>> {
        Self::with_timeout(path, Duration::from_secs(600))
    }
    pub(crate) fn with_timeout(
        path: impl Into<PathBuf>,
        timeout: Duration,
    ) -> anyhow::Result<Arc<Self>> {
        let path = path.into();
        let document: Value = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                json!({"version":1,"approvals":{}})
            }
            Err(error) => return Err(error.into()),
        };
        if !document.is_object() || !document["approvals"].is_object() {
            anyhow::bail!("审批记忆文件格式无效，保留原文件并停止写入。");
        }
        Ok(Arc::new(Self {
            path,
            timeout,
            store: Mutex::new(Store {
                pending: HashMap::new(),
                resolved: BTreeMap::new(),
                document,
                listeners: HashMap::new(),
                next_listener: 1,
                risks: HashMap::new(),
                secure: HashMap::new(),
                closed: false,
            }),
        }))
    }
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        session: Option<&str>,
        listener: Listener,
    ) -> ApprovalSubscription {
        let mut store = self.store.lock().expect("approval store");
        let id = store.next_listener;
        store.next_listener += 1;
        store
            .listeners
            .insert(id, (session.map(str::to_string), listener));
        ApprovalSubscription {
            service: Arc::downgrade(self),
            id,
        }
    }
    fn emit(&self, session: &str, event: &str, data: &Value) {
        let listeners: Vec<_> = self
            .store
            .lock()
            .expect("approval store")
            .listeners
            .values()
            .filter(|(id, _)| id.as_deref().is_none_or(|id| id == session))
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            listener(session, event, data);
        }
    }
    pub(crate) fn set_tool_risk(&self, name: &str, risk: &str) {
        self.store
            .lock()
            .expect("approval store")
            .risks
            .insert(name.into(), risk.into());
    }
    pub(crate) fn pending(&self, session: &str) -> Vec<Value> {
        let mut values: Vec<_> = self
            .store
            .lock()
            .expect("approval store")
            .pending
            .values()
            .filter(|pending| pending.public["sessionId"] == session)
            .map(|pending| pending.public.clone())
            .collect();
        values.sort_by_key(|value| value["createdAt"].as_str().unwrap_or_default().to_string());
        values
    }
    pub(crate) fn resolve(&self, session: &str, id: &str, approved: bool) -> Value {
        self.settle(
            session,
            id,
            approved,
            if approved {
                "用户已授权执行。"
            } else {
                "用户拒绝执行该工具。"
            },
        )
    }
    fn settle(&self, session: &str, id: &str, approved: bool, reason: &str) -> Value {
        let mut store = self.store.lock().expect("approval store");
        let cutoff = now_ms().saturating_sub(RESOLVED_TTL);
        store.resolved.retain(|(at, _), _| *at >= cutoff);
        if !store
            .pending
            .get(id)
            .is_some_and(|pending| pending.public["sessionId"] == session)
        {
            if let Some(previous) = store
                .resolved
                .values()
                .find(|resolution| resolution["id"] == id && resolution["sessionId"] == session)
            {
                let mut response = previous.clone();
                response["found"] = json!(true);
                response["alreadyResolved"] = json!(true);
                return response;
            }
            return json!({"found":false,"alreadyResolved":false,"id":id,"sessionId":session});
        }
        let pending = store.pending.remove(id).expect("matching pending");
        let resolution = json!({"id":id,"sessionId":session,"approved":approved,"reason":reason,"resolvedAt":timestamp()});
        store
            .resolved
            .insert((now_ms(), id.to_string()), resolution.clone());
        while store.resolved.len() > MAX_RESOLVED {
            store.resolved.pop_first();
        }
        drop(store);
        // Publish before waking the actual tool; streaming UI sees the decision before tool_start.
        self.emit(session, "permission_resolved", &resolution);
        let _ = pending.tx.send(resolution.clone());
        let mut response = resolution;
        response["found"] = json!(true);
        response["alreadyResolved"] = json!(false);
        response
    }
    pub(crate) fn resolve_session(&self, session: &str, approved: bool, reason: &str) -> usize {
        let ids: Vec<_> = self
            .store
            .lock()
            .expect("approval store")
            .pending
            .iter()
            .filter(|(_, pending)| pending.public["sessionId"] == session)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.settle(session, id, approved, reason);
        }
        ids.len()
    }
    pub(crate) fn cancel_session(&self, session: &str) -> usize {
        self.resolve_session(session, false, "操作已停止，工具未执行。")
    }
    pub(crate) fn shutdown(&self) {
        let mut store = self.store.lock().expect("approval store");
        store.closed = true;
        let sessions: HashSet<_> = store
            .pending
            .values()
            .filter_map(|pending| pending.public["sessionId"].as_str().map(str::to_string))
            .collect();
        drop(store);
        for session in sessions {
            self.resolve_session(&session, false, "应用正在关闭，工具未执行。");
        }
    }
    fn remembered(&self, key: &str) -> bool {
        let store = self.store.lock().expect("approval store");
        let value = &store.document["approvals"][key];
        !value.is_null() && value != false && value != 0 && value != ""
    }
    fn remember(&self, key: &str, request: &AuthorizationRequest) -> anyhow::Result<()> {
        let mut store = self.store.lock().expect("approval store");
        let mut document = store.document.clone();
        document["approvals"][key] = json!({"approvedAt":timestamp(),"toolName":request.tool_name,"cwd":request.cwd,"command":if request.tool_name=="bash" { request.args["command"].as_str().unwrap_or_default() } else { "" }});
        if request.tool_name != "bash" {
            document["approvals"][key]["args"] = safe_args(&request.args, 0, "");
        }
        std::fs::create_dir_all(self.path.parent().unwrap_or_else(|| FsPath::new(".")))?;
        let temporary = self
            .path
            .with_extension(format!("json.{}.tmp", crate::product::new_id()));
        let bytes = serde_json::to_vec_pretty(&document)?;
        let result = (|| -> anyhow::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        store.document = document;
        Ok(())
    }
    pub(crate) fn observe_computer_use_result(&self, session: &str, name: &str, result: &Value) {
        if !matches!(
            name,
            "find_roots"
                | "observe_ui"
                | "search_ui"
                | "expand_ui"
                | "inspect_ui"
                | "act_ui"
                | "wait_for"
        ) {
            return;
        }
        fn visit(value: &Value, depth: usize, nodes: &mut usize, refs: &mut HashSet<String>) {
            if depth > 8 || *nodes >= 4000 {
                return;
            }
            *nodes += 1;
            match value {
                Value::String(text) => {
                    for line in text
                        .lines()
                        .filter(|line| line.contains("AXSecureTextField"))
                    {
                        if refs.len() >= 4000 {
                            break;
                        }
                        if let Some(start) = line.find("@e") {
                            let digits: String = line[start + 2..]
                                .chars()
                                .take_while(char::is_ascii_digit)
                                .collect();
                            if !digits.is_empty() {
                                refs.insert(format!("@e{digits}"));
                            }
                        }
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        visit(item, depth + 1, nodes, refs)
                    }
                }
                Value::Object(items) => {
                    for item in items.values() {
                        visit(item, depth + 1, nodes, refs)
                    }
                }
                _ => {}
            }
        }
        let mut store = self.store.lock().expect("approval store");
        visit(
            result,
            0,
            &mut 0,
            &mut store.secure.entry(session.into()).or_default().refs,
        );
    }
    fn secure_requirement(
        &self,
        request: &AuthorizationRequest,
    ) -> Option<(execution_modes::Requirement, Value)> {
        if request.tool_name != "act_ui" {
            return None;
        }
        let mut store = self.store.lock().expect("approval store");
        let secure = store.secure.entry(request.session_id.clone()).or_default();
        let mut reason = None;
        for action in request.args["actions"].as_array().into_iter().flatten() {
            let kind = action["action"].as_str().unwrap_or_default();
            let reference = action["ref"].as_str().unwrap_or_default();
            let by_ref = !reference.is_empty() && secure.refs.contains(reference);
            if matches!(kind, "click" | "press") {
                secure.focus_secure = by_ref;
                continue;
            }
            if matches!(kind, "setText" | "typeText" | "keypress") {
                if by_ref || (reference.is_empty() && secure.focus_secure) {
                    reason = Some(if by_ref {
                        "此操作的目标是密码框（AXSecureTextField），将向其输入内容或提交密码，需要确认后执行。"
                    } else {
                        "当前键盘焦点位于密码框，此操作将向密码框输入内容或提交密码，需要确认后执行。"
                    });
                    break;
                }
                if !reference.is_empty() && matches!(kind, "setText" | "typeText") {
                    secure.focus_secure = false;
                }
            }
        }
        if request.execution_mode == "full-access" {
            return None;
        }
        let reason = reason?;
        let mut masked = request.args.clone();
        for action in masked["actions"].as_array_mut().into_iter().flatten() {
            if action["text"].as_str().is_some_and(|text| !text.is_empty()) {
                action["text"] = json!("••••••");
            }
            if action["keys"].is_array() {
                action["keys"] = json!(["•••"]);
            }
        }
        Some((
            execution_modes::Requirement {
                risk: "high".into(),
                reason: reason.into(),
                block: false,
                skip_remember: true,
            },
            masked,
        ))
    }
    pub(crate) async fn authorize(
        self: &Arc<Self>,
        request: AuthorizationRequest,
        cancel: Option<CancelFuture>,
    ) -> Option<BeforeToolCallResult> {
        let block = |reason: String| {
            Some(BeforeToolCallResult {
                block: Some(true),
                reason: Some(reason),
                ..Default::default()
            })
        };
        if let Some(scope) = execution_modes::ownership(
            &request.cwd,
            &request.tool_name,
            &request.args,
            &request.owned_files,
        ) {
            return block(scope.reason);
        }
        let risk = self
            .store
            .lock()
            .expect("approval store")
            .risks
            .get(&request.tool_name)
            .cloned();
        let secure = self.secure_requirement(&request);
        let requirement = secure
            .as_ref()
            .map(|(requirement, _)| requirement.clone())
            .or_else(|| {
                execution_modes::requirement(
                    &request.permission_mode,
                    &request.execution_mode,
                    &request.cwd,
                    &request.tool_name,
                    &request.args,
                    risk.as_deref(),
                )
            });
        let requirement = requirement?;
        if requirement.block {
            return block(requirement.reason);
        }
        let preview = if matches!(request.tool_name.as_str(), "edit" | "write") {
            match preview(&request) {
                Ok(preview) => Some(preview),
                Err(error) => return block(error.to_string()),
            }
        } else {
            None
        };
        let key = approval_key(&request);
        if !requirement.skip_remember && preview.is_none() && self.remembered(&key) {
            return None;
        }
        let id = crate::product::new_id();
        let (tx, rx) = oneshot::channel();
        let mut public = json!({"id":id,"sessionId":request.session_id,"toolName":request.tool_name,"toolCallId":request.tool_call_id,"args":safe_args(secure.as_ref().map(|(_,args)|args).unwrap_or(&request.args),0,""),"mode":request.permission_mode,"risk":requirement.risk,"reason":requirement.reason,"createdAt":timestamp()});
        if let Some(preview) = &preview {
            public["fileChange"] = preview.public.clone();
        }
        {
            let mut store = self.store.lock().expect("approval store");
            if store.closed {
                return block("应用正在关闭，工具未执行。".into());
            }
            store.pending.insert(
                id.clone(),
                Pending {
                    public: public.clone(),
                    tx,
                },
            );
        }
        self.emit(&request.session_id, "permission_request", &public);
        let cancelled = async {
            if let Some(cancel) = cancel {
                cancel.await
            } else {
                std::future::pending::<()>().await
            }
        };
        let resolution = tokio::select! { biased;
            _=cancelled=>self.settle(&request.session_id,&id,false,"操作已停止，工具未执行。"),
            result=rx=>result.unwrap_or_else(|_|json!({"approved":false,"reason":"审批通道已关闭，工具未执行。"})),
            _=tokio::time::sleep(self.timeout)=>self.settle(&request.session_id,&id,false,"等待授权超时，工具未执行。"),
        };
        if resolution["approved"] != true {
            return block(
                resolution["reason"]
                    .as_str()
                    .unwrap_or("用户拒绝执行该工具。")
                    .into(),
            );
        }
        if let Some(preview) = &preview {
            match unchanged(preview, &request) {
                Ok(true) => {}
                Ok(false) => {
                    return block(
                        "目标文件在审核期间发生了变化，请重新请求修改并查看最新 Diff。".into(),
                    )
                }
                Err(error) => return block(error.to_string()),
            }
        } else if !requirement.skip_remember {
            if let Err(error) = self.remember(&key, &request) {
                return block(format!("审批已接受，但记忆存盘失败，工具未执行：{error}"));
            }
        }
        None
    }
}

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sessions/{id}/approvals", get(pending))
        .route("/api/sessions/{id}/approvals/{approval}", post(resolve))
        .route(
            "/api/sessions/{id}/execution-mode",
            put(execution_mode).post(execution_mode),
        )
        .route("/api/sessions/{id}/permission", put(permission))
}
async fn pending(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::session_api::find_session_path(&state, &id)?;
    Ok(Json(json!({"approvals":state.approvals.pending(&id)})))
}
async fn resolve(
    State(state): State<Arc<AppState>>,
    Path((id, approval)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    crate::session_api::find_session_path(&state, &id)?;
    let approved = body["approved"]
        .as_bool()
        .ok_or_else(|| ApiError::bad_request("approved must be boolean"))?;
    let value = state.approvals.resolve(&id, &approval, approved);
    if value["found"] != true {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "approval_not_found",
            "授权请求不存在。",
        ));
    }
    Ok(Json(value))
}
async fn execution_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mode = execution_modes::normalize(body["mode"].as_str().unwrap_or_default())
        .ok_or_else(|| ApiError::bad_request("执行模式无效。"))?;
    crate::session_api::find_session_path(&state, &id)?;
    {
        let mut metadata = state
            .session_meta
            .lock()
            .map_err(|_| ApiError::internal("session metadata"))?;
        let meta = metadata.entry(id.clone()).or_default();
        meta.execution_mode = Some(mode.into());
        meta.permission_mode = Some(execution_modes::permission_mode(mode).into());
    }
    crate::session_api::save_metadata(&state)?;
    state.approvals.resolve_session(
        &id,
        mode == "full-access",
        if mode == "full-access" {
            "已切换为完全访问。"
        } else {
            "执行模式已切换，请按新权限重新发起工具调用。"
        },
    );
    Ok(Json(
        json!({"id":id,"executionMode":mode,"permissionMode":execution_modes::permission_mode(mode)}),
    ))
}
async fn permission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mode = body["mode"]
        .as_str()
        .filter(|mode| matches!(*mode, "ask" | "auto" | "ignore"))
        .ok_or_else(|| ApiError::bad_request("权限模式无效。"))?;
    crate::session_api::find_session_path(&state, &id)?;
    if state.sessions.busy(&id) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请完成或停止后再切换权限模式。",
        ));
    }
    let execution = {
        let mut metadata = state
            .session_meta
            .lock()
            .map_err(|_| ApiError::internal("session metadata"))?;
        let meta = metadata.entry(id.clone()).or_default();
        meta.permission_mode = Some(mode.into());
        meta.execution_mode
            .clone()
            .unwrap_or_else(|| execution_modes::DEFAULT_EXECUTION_MODE.into())
    };
    crate::session_api::save_metadata(&state)?;
    if mode != "ask" {
        state.approvals.resolve_session(
            &id,
            true,
            if mode == "ignore" {
                "权限模式已切换为忽略。"
            } else {
                "权限模式已切换为自动。"
            },
        );
    }
    Ok(Json(
        json!({"id":id,"executionMode":execution,"permissionMode":mode}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn sandbox() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("pisper-approval-test-{}", crate::product::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn request(cwd: &FsPath, tool: &str, args: Value) -> AuthorizationRequest {
        AuthorizationRequest {
            session_id: "s".into(),
            cwd: cwd.to_string_lossy().into(),
            tool_name: tool.into(),
            tool_call_id: "call".into(),
            args,
            permission_mode: "ask".into(),
            execution_mode: "approval-required".into(),
            owned_files: vec![],
        }
    }
    async fn until_pending(service: &ApprovalService) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(pending) = service.pending("s").first() {
                    return pending.clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn approval_really_suspends_then_resumes_and_repeats_idempotently() {
        let dir = sandbox();
        let service = ApprovalService::new(dir.join("approvals.json")).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(vec![]));
        let events = observed.clone();
        let _subscription = service.subscribe(
            Some("s"),
            Arc::new(move |_, event, _| events.lock().unwrap().push(event.to_string())),
        );
        let own = service.clone();
        let increment = count.clone();
        let cwd = dir.clone();
        let task = tokio::spawn(async move {
            if own
                .authorize(
                    request(&cwd, "bash", json!({"command":"echo approval-fixture"})),
                    None,
                )
                .await
                .is_none()
            {
                increment.fetch_add(1, Ordering::SeqCst);
            }
        });
        let pending = until_pending(&service).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(!task.is_finished());
        assert_eq!(
            service.resolve("wrong", pending["id"].as_str().unwrap(), true)["found"],
            false
        );
        let id = pending["id"].as_str().unwrap();
        assert_eq!(service.resolve("s", id, true)["alreadyResolved"], false);
        assert_eq!(service.resolve("s", id, false)["approved"], true);
        task.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *observed.lock().unwrap(),
            ["permission_request", "permission_resolved"]
        );
        let restarted = ApprovalService::new(dir.join("approvals.json")).unwrap();
        assert!(restarted
            .authorize(
                request(
                    &dir,
                    "bash",
                    json!({"timeout":99,"command":"echo approval-fixture"})
                ),
                None
            )
            .await
            .is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn cancellation_denial_and_timeout_never_execute() {
        let dir = sandbox();
        let service =
            ApprovalService::with_timeout(dir.join("approvals.json"), Duration::from_millis(100))
                .unwrap();
        for reject in ["deny", "cancel", "timeout", "shutdown"] {
            let own = service.clone();
            let cwd = dir.clone();
            let task = tokio::spawn(async move {
                own.authorize(
                    request(&cwd, "bash", json!({"command":"echo no-execution"})),
                    None,
                )
                .await
            });
            let pending = until_pending(&service).await;
            match reject {
                "deny" => {
                    service.resolve("s", pending["id"].as_str().unwrap(), false);
                }
                "cancel" => {
                    service.cancel_session("s");
                }
                "shutdown" => service.shutdown(),
                _ => {}
            };
            assert_eq!(task.await.unwrap().unwrap().block, Some(true));
            assert!(service.pending("s").is_empty());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn exact_file_preview_rechecks_source_and_is_never_remembered() {
        let dir = sandbox();
        std::fs::write(dir.join("file.txt"), "original\n").unwrap();
        let service = ApprovalService::new(dir.join("approvals.json")).unwrap();
        for changed in [true, false] {
            let own = service.clone();
            let cwd = dir.clone();
            let task = tokio::spawn(async move {
                own.authorize(
                    request(
                        &cwd,
                        "write",
                        json!({"path":"file.txt","content":"replacement\n"}),
                    ),
                    None,
                )
                .await
            });
            let pending = until_pending(&service).await;
            assert!(pending["fileChange"]["diff"]
                .as_str()
                .unwrap()
                .contains("+replacement"));
            if changed {
                std::fs::write(dir.join("file.txt"), "another writer\n").unwrap();
            }
            service.resolve("s", pending["id"].as_str().unwrap(), true);
            let result = task.await.unwrap();
            assert_eq!(result.is_some(), changed);
        }
        assert!(!dir.join("approvals.json").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn secure_inputs_are_redacted_and_not_remembered() {
        let dir = sandbox();
        let service = ApprovalService::new(dir.join("approvals.json")).unwrap();
        service.observe_computer_use_result(
            "s",
            "observe_ui",
            &json!({"content":[{"text":"@e1 AXTextField/AXSecureTextField password"}]}),
        );
        let own = service.clone();
        let cwd = dir.clone();
        let task = tokio::spawn(async move {
            let mut request = request(
                &cwd,
                "act_ui",
                json!({"actions":[{"action":"typeText","ref":"@e1","text":"sensitive-test-text"}]}),
            );
            request.permission_mode = "auto".into();
            request.execution_mode = "workspace-write".into();
            own.authorize(request, None).await
        });
        let pending = until_pending(&service).await;
        assert!(!pending.to_string().contains("sensitive-test-text"));
        service.resolve("s", pending["id"].as_str().unwrap(), true);
        assert!(task.await.unwrap().is_none());
        assert!(!dir.join("approvals.json").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
