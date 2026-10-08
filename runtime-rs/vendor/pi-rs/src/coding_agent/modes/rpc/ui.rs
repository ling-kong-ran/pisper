//! RPC extension UI bridge from upstream `modes/rpc/rpc-mode.ts`.
//! Dialogs are awaitable without blocking input dispatch; responses, aborts and
//! timers compete for one pending entry, so the first completion wins. The
//! native ExtensionUI API accepts typed strings, not arbitrary JS values.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::coding_agent::extensions::types::{
    AbortSubscription, ExtensionUI, ExtensionUiDialogOptions, ExtensionWidgetOptions,
    SetThemeResult, UiFuture,
};

/// Serialized by the mode's single output writer; never writes stdout itself.
pub type RpcUiOutput = Arc<dyn Fn(Value) + Send + Sync>;
pub type RpcThemeProvider = Arc<dyn Fn() -> Value + Send + Sync>;
type DialogResult = Result<Value, String>;

struct UiState {
    pending: Mutex<BTreeMap<String, oneshot::Sender<DialogResult>>>,
    output: RpcUiOutput,
    theme: RpcThemeProvider,
}

#[derive(Clone)]
pub struct RpcExtensionUi {
    state: Arc<UiState>,
}
impl RpcExtensionUi {
    pub fn new(output: RpcUiOutput) -> Self {
        Self::with_theme(output, Arc::new(|| Value::Null))
    }

    /// The native theme handle is supplied by the embedding host. This does not
    /// claim that a JSON object implements the upstream callable Theme API.
    pub fn with_theme(output: RpcUiOutput, theme: RpcThemeProvider) -> Self {
        Self {
            state: Arc::new(UiState {
                pending: Mutex::new(BTreeMap::new()),
                output,
                theme,
            }),
        }
    }

    /// Unknown/duplicate IDs are ignored, like the upstream pending Map lookup.
    /// Removal precedes waking the dialog, including when the callback responds
    /// synchronously while the request is being emitted.
    pub fn respond(&self, response: Value) -> bool {
        if response["type"] != "extension_ui_response" {
            return false;
        }
        let Some(id) = response["id"].as_str() else {
            return false;
        };
        let sender = self
            .state
            .pending
            .lock()
            .expect("rpc UI pending")
            .remove(id);
        if let Some(sender) = sender {
            let _ = sender.send(Ok(response));
            true
        } else {
            false
        }
    }

    /// Called by an owning host that is being disposed. No task is left waiting
    /// forever for a client that can no longer send a response.
    pub fn reject_pending(&self, reason: &str) {
        let pending = std::mem::take(&mut *self.state.pending.lock().expect("rpc UI pending"));
        for sender in pending.into_values() {
            let _ = sender.send(Err(reason.to_owned()));
        }
    }

    pub fn pending_count(&self) -> usize {
        self.state.pending.lock().expect("rpc UI pending").len()
    }

    fn emit(&self, request: Value) {
        (self.state.output)(request);
    }

    async fn dialog(&self, opts: &ExtensionUiDialogOptions, mut request: Value) -> DialogResult {
        if opts
            .signal
            .as_ref()
            .is_some_and(|signal| signal.is_aborted())
        {
            return Ok(json!({"cancelled": true}));
        }
        let id = request["id"].as_str().expect("request ID").to_owned();
        let (sender, receiver) = oneshot::channel();
        self.state
            .pending
            .lock()
            .expect("rpc UI pending")
            .insert(id.clone(), sender);
        let mut guard = PendingDialog {
            state: Arc::downgrade(&self.state),
            id: id.clone(),
            timer: None,
            abort: None,
        };
        if let Some(signal) = &opts.signal {
            let state = Arc::downgrade(&self.state);
            let id = id.clone();
            guard.abort = Some(signal.on_abort(Arc::new(move || cancel_dialog(&state, &id))));
        }
        if let Some(timeout) = opts.timeout.and_then(node_timeout) {
            let state = Arc::downgrade(&self.state);
            let id = id.clone();
            // Start the deadline before output (a reentrant sink may take time).
            let sleep = tokio::time::sleep(timeout);
            guard.timer = Some(tokio::spawn(async move {
                sleep.await;
                cancel_dialog(&state, &id);
            }));
        }
        // A concurrent abort may have removed the request during registration.
        // Never call user output under the pending lock: sinks may call respond.
        let pending = self
            .state
            .pending
            .lock()
            .expect("rpc UI pending")
            .contains_key(&id);
        if pending {
            if let Some(timeout) = opts.timeout {
                request["timeout"] = json!(timeout);
            }
            self.emit(request);
        }
        receiver
            .await
            .map_err(|_| "RPC UI request closed".to_owned())?
    }
}

struct PendingDialog {
    state: Weak<UiState>,
    id: String,
    timer: Option<tokio::task::JoinHandle<()>>,
    abort: Option<AbortSubscription>,
}
impl Drop for PendingDialog {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
        if let Some(state) = self.state.upgrade() {
            state
                .pending
                .lock()
                .expect("rpc UI pending")
                .remove(&self.id);
        }
    }
}
fn cancel_dialog(state: &Weak<UiState>, id: &str) {
    if let Some(state) = state.upgrade() {
        let sender = state.pending.lock().expect("rpc UI pending").remove(id);
        if let Some(sender) = sender {
            let _ = sender.send(Ok(json!({"cancelled":true})));
        }
    }
}

/// `if (opts.timeout) setTimeout(...)`: zero/NaN disable the timer; Node clamps
/// out-of-range delays to 1ms and truncates fractional in-range values.
fn node_timeout(timeout: f64) -> Option<Duration> {
    if timeout == 0.0 || timeout.is_nan() {
        return None;
    }
    let millis = if !(1.0..=2_147_483_647.0).contains(&timeout) {
        1
    } else {
        timeout as u64
    };
    Some(Duration::from_millis(millis))
}
fn request(method: &str) -> Value {
    // crypto.randomUUID(): RFC 4122 version 4, variant 1.
    let mut bytes: [u8; 16] = rand::random();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    use std::fmt::Write;
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        write!(&mut id, "{byte:02x}").expect("writing string");
    }
    json!({"type":"extension_ui_request", "id":id, "method":method})
}
fn string_result(response: &Value) -> Option<String> {
    if response["cancelled"] == true {
        None
    } else {
        response["value"].as_str().map(str::to_owned)
    }
}
impl ExtensionUI for RpcExtensionUi {
    fn select<'a>(
        &'a self,
        title: &'a str,
        options: &'a [String],
        opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move {
            let mut req = request("select");
            req["title"] = json!(title);
            req["options"] = json!(options);
            Ok(string_result(&self.dialog(opts, req).await?))
        })
    }
    fn confirm<'a>(
        &'a self,
        title: &'a str,
        message: &'a str,
        opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, bool> {
        Box::pin(async move {
            let mut req = request("confirm");
            req["title"] = json!(title);
            req["message"] = json!(message);
            let response = self.dialog(opts, req).await?;
            Ok(response["cancelled"] != true && response["confirmed"] == true)
        })
    }
    fn input<'a>(
        &'a self,
        title: &'a str,
        placeholder: Option<&'a str>,
        opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move {
            let mut req = request("input");
            req["title"] = json!(title);
            if let Some(placeholder) = placeholder {
                req["placeholder"] = json!(placeholder);
            }
            Ok(string_result(&self.dialog(opts, req).await?))
        })
    }
    fn editor<'a>(
        &'a self,
        title: &'a str,
        prefill: Option<&'a str>,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move {
            let mut req = request("editor");
            req["title"] = json!(title);
            if let Some(prefill) = prefill {
                req["prefill"] = json!(prefill);
            }
            Ok(string_result(
                &self
                    .dialog(&ExtensionUiDialogOptions::default(), req)
                    .await?,
            ))
        })
    }
    fn notify(&self, message: &str, kind: Option<&str>) {
        let mut req = request("notify");
        req["message"] = json!(message);
        if let Some(kind) = kind {
            req["notifyType"] = json!(kind);
        }
        self.emit(req);
    }
    fn set_status(&self, key: &str, text: Option<&str>) {
        let mut req = request("setStatus");
        req["statusKey"] = json!(key);
        if let Some(text) = text {
            req["statusText"] = json!(text);
        }
        self.emit(req);
    }
    fn set_widget(&self, key: &str, content: Option<&Value>, options: &ExtensionWidgetOptions) {
        if content.is_some_and(|value| !value.is_array()) {
            return;
        }
        let mut req = request("setWidget");
        req["widgetKey"] = json!(key);
        if let Some(content) = content {
            req["widgetLines"] = content.clone();
        }
        if let Some(placement) = options.placement {
            req["widgetPlacement"] = json!(placement.as_str());
        }
        self.emit(req);
    }
    fn set_title(&self, title: &str) {
        let mut req = request("setTitle");
        req["title"] = json!(title);
        self.emit(req);
    }
    fn paste_to_editor(&self, text: &str) {
        self.set_editor_text(text);
    }
    fn set_editor_text(&self, text: &str) {
        let mut req = request("set_editor_text");
        req["text"] = json!(text);
        self.emit(req);
    }
    fn theme(&self) -> Value {
        (self.state.theme)()
    }
    fn set_theme(&self, _theme: &Value) -> SetThemeResult {
        SetThemeResult {
            success: false,
            error: Some("Theme switching not supported in RPC mode".into()),
        }
    }
    // All other operations deliberately keep the upstream no-op defaults:
    // custom, terminal input, working indicator, footer/header, editor component,
    // completion providers, theme lookup and tool expansion.
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
