//! 一个 CDP 连接负责请求关联和事件接收；后台任务只拥有通道，不反向持有驱动。
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct Protocol(Arc<Inner>);
struct Inner {
    outgoing: mpsc::UnboundedSender<Request>,
    sequence: AtomicU64,
    pub(super) events: broadcast::Sender<Value>,
    pub(super) snapshot: Arc<Mutex<Snapshot>>,
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}
struct Request {
    id: u64,
    method: String,
    params: Value,
    session: Option<String>,
    reply: oneshot::Sender<Result<Value, String>>,
}
#[derive(Default, Clone)]
pub(super) struct Snapshot {
    pub(super) frames: HashMap<String, Frame>,
    pub(super) sessions: HashMap<String, String>,
    pub(super) navigations: u64,
    pub(super) ready_sessions: HashSet<String>,
}
#[derive(Clone, Default)]
pub(super) struct Frame {
    pub(super) parent: Option<String>,
    pub(super) session: String,
    pub(super) loader: String,
    pub(super) url: String,
    pub(super) dom_loaded: bool,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Protocol {
    pub(super) async fn connect(url: &str) -> Result<Self, String> {
        // 与 release WebSocketTransport 的 256 MiB maxPayload 一致，避免默认 16 MiB 帧限制截断整页 PNG。
        let config = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024 * 1024))
            .max_frame_size(Some(256 * 1024 * 1024));
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(30),
            connect_async_with_config(url, Some(config), false),
        )
        .await
        .map_err(|_| "Browser connection timed out".to_owned())?
        .map_err(|error| error.to_string())?;
        let (mut sink, mut incoming) = socket.split();
        let (outgoing, mut requests) = mpsc::unbounded_channel::<Request>();
        let (events, _) = broadcast::channel(4096);
        let actor_events = events.clone();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let actor_snapshot = snapshot.clone();
        let cancel = CancellationToken::new();
        let actor_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut replies = HashMap::<u64, oneshot::Sender<Result<Value, String>>>::new();
            let mut internal_id = 2_000_000_000_u64;
            loop {
                tokio::select! {
                    biased;
                    _=actor_cancel.cancelled()=>break,
                    request=requests.recv()=>{
                        let Some(request)=request else{break;};
                        replies.retain(|_,reply|!reply.is_closed());
                        let mut value=json!({"id":request.id,"method":request.method,"params":request.params});
                        if let Some(session)=request.session{value["sessionId"]=json!(session);}
                        replies.insert(request.id,request.reply);
                        if sink.send(Message::Text(value.to_string().into())).await.is_err(){break;}
                    },
                    message=incoming.next()=>{
                        let Some(Ok(message))=message else{break;};
                        match message {
                            Message::Text(text)=>{
                                let parsed=serde_json::from_str::<Value>(&sanitize_surrogates(text.as_str()));
                                let Ok(value)=parsed else{continue;};
                                if let Some(id)=value["id"].as_u64(){
                                    if let Some(reply)=replies.remove(&id){let result=if value.get("error").is_some(){Err(value["error"]["message"].as_str().unwrap_or("Browser protocol error").to_owned())}else{Ok(value["result"].clone())};let _=reply.send(result);}
                                }else{
                                    if let Ok(mut snapshot)=actor_snapshot.lock(){update_snapshot(&mut snapshot,&value);}
                                    if value["method"]=="Page.javascriptDialogOpening"{
                                        let mut command=json!({"id":internal_id,"method":"Page.handleJavaScriptDialog","params":{"accept":value["params"]["type"]=="beforeunload"}});internal_id+=1;
                                        if let Some(session)=value["sessionId"].as_str(){command["sessionId"]=json!(session);}
                                        if sink.send(Message::Text(command.to_string().into())).await.is_err(){break;}
                                    }
                                    let _=actor_events.send(value);
                                }
                            },
                            Message::Ping(value)=>{if sink.send(Message::Pong(value)).await.is_err(){break;}},
                            Message::Close(_)=>break,
                            _=>{},
                        }
                    },
                }
            }
            actor_cancel.cancel();
            for (_, reply) in replies {
                let _ = reply.send(Err("Target page, context or browser has been closed".into()));
            }
            let _ = sink.close().await;
        });
        Ok(Self(Arc::new(Inner {
            outgoing,
            sequence: AtomicU64::new(1),
            events,
            snapshot,
            cancel,
            task: Mutex::new(Some(task)),
        })))
    }
    pub(super) async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.call_for(session, method, params, Duration::from_secs(30))
            .await
    }
    pub(super) async fn call_for(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if self.0.cancel.is_cancelled() {
            return Err("Target page, context or browser has been closed".into());
        }
        let id = self.0.sequence.fetch_add(1, Ordering::Relaxed);
        let (reply, receive) = oneshot::channel();
        self.0
            .outgoing
            .send(Request {
                id,
                method: method.into(),
                params,
                session: session.map(str::to_owned),
                reply,
            })
            .map_err(|_| "Target page, context or browser has been closed".to_owned())?;
        tokio::select! {biased;
            _=self.0.cancel.cancelled()=>Err("Target page, context or browser has been closed".into()),
            value=tokio::time::timeout(timeout,receive)=>value.map_err(|_|format!("Browser protocol command {method} timed out"))?.map_err(|_|"Browser protocol connection closed".to_owned())?,
        }
    }
    pub(super) fn events(&self) -> broadcast::Receiver<Value> {
        self.0.events.subscribe()
    }
    pub(super) fn cancellation(&self) -> CancellationToken {
        self.0.cancel.clone()
    }
    pub(super) async fn event(
        &self,
        events: &mut broadcast::Receiver<Value>,
    ) -> Result<Value, String> {
        loop {
            tokio::select! {biased;_=self.0.cancel.cancelled()=>return Err("Target page, context or browser has been closed".into()),event=events.recv()=>match event{Ok(event)=>return Ok(event),Err(broadcast::error::RecvError::Lagged(_))=>continue,Err(error)=>return Err(error.to_string())}}
        }
    }
    pub(super) fn session_ready(&self, session: &str) -> Result<(), String> {
        self.0
            .snapshot
            .lock()
            .map_err(|_| "Browser frame state lock failed".to_owned())?
            .ready_sessions
            .insert(session.into());
        Ok(())
    }
    pub(super) fn snapshot(&self) -> Result<Snapshot, String> {
        self.0
            .snapshot
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "Browser frame state lock failed".into())
    }
    pub(super) fn add_tree(&self, session: &str, tree: &Value) -> Result<(), String> {
        let mut snapshot = self
            .0
            .snapshot
            .lock()
            .map_err(|_| "Browser frame state lock failed".to_owned())?;
        add_tree(&mut snapshot, session, tree);
        Ok(())
    }
    pub(super) fn stop(&self) {
        self.0.cancel.cancel();
    }
    pub(super) async fn close(&self) {
        self.stop();
        let task = self.0.task.lock().ok().and_then(|mut task| task.take());
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}
fn add_tree(snapshot: &mut Snapshot, session: &str, tree: &Value) {
    let frame = &tree["frame"];
    if let Some(id) = frame["id"].as_str() {
        snapshot.frames.insert(
            id.into(),
            Frame {
                parent: frame["parentId"].as_str().map(str::to_owned),
                session: session.into(),
                loader: frame["loaderId"].as_str().unwrap_or("").into(),
                url: frame["url"].as_str().unwrap_or("").into(),
                dom_loaded: false,
            },
        );
    }
    for child in tree["childFrames"].as_array().into_iter().flatten() {
        add_tree(snapshot, session, child);
    }
}
fn update_snapshot(snapshot: &mut Snapshot, event: &Value) {
    let session = event["sessionId"].as_str().unwrap_or("");
    let params = &event["params"];
    match event["method"].as_str().unwrap_or("") {
        "Target.attachedToTarget" => {
            if params["targetInfo"]["type"] == "iframe" {
                if let (Some(id), Some(session)) = (
                    params["targetInfo"]["targetId"].as_str(),
                    params["sessionId"].as_str(),
                ) {
                    snapshot.sessions.insert(id.into(), session.into());
                }
            }
        }
        "Target.detachedFromTarget" => {
            if let Some(session) = params["sessionId"].as_str() {
                snapshot.sessions.retain(|_, value| value != session);
                snapshot.ready_sessions.remove(session);
            }
        }
        "Page.frameAttached" => {
            if let Some(id) = params["frameId"].as_str() {
                let frame = snapshot.frames.entry(id.into()).or_default();
                frame.parent = params["parentFrameId"].as_str().map(str::to_owned);
                frame.session = session.into();
            }
        }
        "Page.frameNavigated" => {
            let frame = &params["frame"];
            if let Some(id) = frame["id"].as_str() {
                let old_parent = snapshot
                    .frames
                    .get(id)
                    .and_then(|frame| frame.parent.clone());
                snapshot.frames.insert(
                    id.into(),
                    Frame {
                        parent: frame["parentId"].as_str().map(str::to_owned).or(old_parent),
                        session: session.into(),
                        loader: frame["loaderId"].as_str().unwrap_or("").into(),
                        url: frame["url"].as_str().unwrap_or("").into(),
                        dom_loaded: false,
                    },
                );
                snapshot.navigations += 1;
            }
        }
        "Page.navigatedWithinDocument" => {
            if let Some(id) = params["frameId"].as_str() {
                if let Some(frame) = snapshot.frames.get_mut(id) {
                    frame.url = params["url"].as_str().unwrap_or("").into();
                }
                snapshot.navigations += 1;
            }
        }
        "Page.lifecycleEvent" => {
            if params["name"] == "DOMContentLoaded" {
                if let Some(frame) = params["frameId"]
                    .as_str()
                    .and_then(|id| snapshot.frames.get_mut(id))
                {
                    if params["loaderId"] == frame.loader {
                        frame.dom_loaded = true;
                    }
                }
            }
        }
        "Page.frameDetached" => {
            if params["reason"] != "swap" {
                if let Some(id) = params["frameId"].as_str() {
                    snapshot.frames.remove(id);
                }
            }
        }
        _ => {}
    }
}

/// CDP 是 JS JSON；只将字符串内孤立的 UTF-16 转义替换为合法标量，数字保持原文。
fn sanitize_surrogates(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut string = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' {
            string = !string;
            result.push(byte);
            index += 1;
            continue;
        }
        if string && byte == b'\\' && index + 1 < bytes.len() {
            if bytes[index + 1] == b'u' && index + 5 < bytes.len() {
                let unit = std::str::from_utf8(&bytes[index + 2..index + 6])
                    .ok()
                    .and_then(|value| u16::from_str_radix(value, 16).ok());
                if let Some(unit) = unit {
                    if (0xd800..=0xdbff).contains(&unit) {
                        let pair =
                            if index + 11 < bytes.len() && bytes[index + 6..index + 8] == *b"\\u" {
                                std::str::from_utf8(&bytes[index + 8..index + 12])
                                    .ok()
                                    .and_then(|value| u16::from_str_radix(value, 16).ok())
                                    .is_some_and(|value| (0xdc00..=0xdfff).contains(&value))
                            } else {
                                false
                            };
                        if pair {
                            result.extend_from_slice(&bytes[index..index + 12]);
                            index += 12;
                            continue;
                        }
                        result.extend_from_slice(b"\\ufffd");
                        index += 6;
                        continue;
                    }
                    if (0xdc00..=0xdfff).contains(&unit) {
                        result.extend_from_slice(b"\\ufffd");
                        index += 6;
                        continue;
                    }
                }
            }
            result.extend_from_slice(&bytes[index..index + 2]);
            index += 2;
            continue;
        }
        result.push(byte);
        index += 1;
    }
    String::from_utf8(result).unwrap_or_else(|_| text.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_unicode_replacement_preserves_pairs_escaped_backslashes_and_numbers() {
        let raw = r#"{"text":"a\ud800b\udc00","pair":"\ud83d\ude00","literal":"\\ud800","number":123.456}"#;
        let value: Value = serde_json::from_str(&sanitize_surrogates(raw)).unwrap();
        assert_eq!(value["text"], "a�b�");
        assert_eq!(value["pair"], "😀");
        assert_eq!(value["literal"], r"\ud800");
        assert_eq!(value["number"], json!(123.456));
    }
    #[test]
    fn protocol_tracks_document_completion_by_exact_loader_and_removes_child_readiness() {
        let mut snapshot = Snapshot::default();
        update_snapshot(
            &mut snapshot,
            &json!({"method":"Page.frameNavigated","sessionId":"main","params":{"frame":{"id":"root","loaderId":"one","url":"http://127.0.0.1/"}}}),
        );
        update_snapshot(
            &mut snapshot,
            &json!({"method":"Page.lifecycleEvent","params":{"frameId":"root","loaderId":"old","name":"DOMContentLoaded"}}),
        );
        assert!(!snapshot.frames["root"].dom_loaded);
        update_snapshot(
            &mut snapshot,
            &json!({"method":"Page.lifecycleEvent","params":{"frameId":"root","loaderId":"one","name":"DOMContentLoaded"}}),
        );
        assert!(snapshot.frames["root"].dom_loaded);
        update_snapshot(
            &mut snapshot,
            &json!({"method":"Page.frameNavigated","sessionId":"main","params":{"frame":{"id":"root","loaderId":"two","url":"http://127.0.0.1/next"}}}),
        );
        assert!(!snapshot.frames["root"].dom_loaded);
        snapshot.ready_sessions.insert("child".into());
        snapshot
            .sessions
            .insert("child-frame".into(), "child".into());
        update_snapshot(
            &mut snapshot,
            &json!({"method":"Target.detachedFromTarget","params":{"sessionId":"child"}}),
        );
        assert!(snapshot.ready_sessions.is_empty());
        assert!(snapshot.sessions.is_empty());
    }
}
