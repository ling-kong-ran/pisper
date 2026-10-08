//! 原生 CDP 驱动：独立无痕上下文、临时 profile 和真实浏览器输入由此处拥有。
#[path = "cdp/actions.rs"]
mod actions;
#[path = "cdp/contexts.rs"]
mod contexts;
#[path = "cdp/process.rs"]
mod process;
#[path = "cdp/protocol.rs"]
mod protocol;
#[path = "cdp/screenshot.rs"]
mod screenshot;
#[path = "cdp/world.rs"]
mod world;

use super::{
    coercion,
    types::{BrowserDriver, BrowserFactory, BrowserProgress, BrowserResult},
};
use futures::future::BoxFuture;
use process::BrowserProcess;
use protocol::Protocol;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as SyncMutex,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, Notify},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

struct State {
    session: String,
    root: String,
    viewport: Value,
    worlds: HashMap<String, world::World>,
    sessions: HashSet<String>,
}
struct CloseCompletion {
    done: AtomicBool,
    notify: Notify,
}
struct Work {
    input: Value,
    progress: Option<BrowserProgress>,
    reply: oneshot::Sender<BrowserResult<Value>>,
}
struct Core {
    outgoing: mpsc::UnboundedSender<Work>,
    closed: Arc<AtomicBool>,
    close: CancellationToken,
    completion: Arc<CloseCompletion>,
    task: SyncMutex<Option<JoinHandle<()>>>,
}
impl Drop for Core {
    fn drop(&mut self) {
        self.close.cancel();
    }
}
struct CdpDriver(Arc<Core>);

pub fn factory(profile_root: PathBuf) -> BrowserFactory {
    Arc::new(move |viewport| {
        let root = profile_root.clone();
        Box::pin(async move {
            let value = CdpDriver::launch(root, viewport).await?;
            Ok(Arc::new(value) as Arc<dyn BrowserDriver>)
        })
    })
}
impl CdpDriver {
    async fn launch(root: PathBuf, viewport: Value) -> BrowserResult<Self> {
        let (process, endpoint) = BrowserProcess::launch(&root).await?;
        let protocol = match Protocol::connect(&endpoint).await {
            Ok(protocol) => protocol,
            Err(error) => {
                process.close().await;
                return Err(error);
            }
        };
        let initializer = contexts::ContextInitializer::new(protocol.clone());
        let initialized = async {
            let context = protocol
                .call(
                    None,
                    "Target.createBrowserContext",
                    json!({"disposeOnDetach":true}),
                )
                .await?["browserContextId"]
                .as_str()
                .ok_or_else(|| "Browser context was not created".to_owned())?
                .to_owned();
            protocol
                .call(
                    None,
                    "Browser.setDownloadBehavior",
                    json!({"behavior":"deny","browserContextId":context}),
                )
                .await?;
            let target = protocol
                .call(
                    None,
                    "Target.createTarget",
                    json!({"url":"about:blank","browserContextId":context}),
                )
                .await?["targetId"]
                .as_str()
                .ok_or_else(|| "Browser page was not created".to_owned())?
                .to_owned();
            let session = protocol
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({"targetId":target,"flatten":true}),
                )
                .await?["sessionId"]
                .as_str()
                .ok_or_else(|| "Browser page was not attached".to_owned())?
                .to_owned();
            for method in [
                "Page.enable",
                "Runtime.enable",
                "DOM.enable",
                "Network.enable",
            ] {
                protocol.call(Some(&session), method, json!({})).await?;
            }
            initialize_defaults(&protocol, &session).await?;
            protocol
                .call(
                    Some(&session),
                    "Emulation.setFocusEmulationEnabled",
                    json!({"enabled":true}),
                )
                .await?;
            protocol
                .call(
                    Some(&session),
                    "Page.setLifecycleEventsEnabled",
                    json!({"enabled":true}),
                )
                .await?;
            protocol
                .call(
                    Some(&session),
                    "Target.setAutoAttach",
                    json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
                )
                .await?;
            let tree = protocol
                .call(Some(&session), "Page.getFrameTree", json!({}))
                .await?["frameTree"]
                .clone();
            protocol.add_tree(&session, &tree)?;
            let root = tree["frame"]["id"]
                .as_str()
                .ok_or_else(|| "Browser root frame was not found".to_owned())?
                .to_owned();
            set_viewport(&protocol, &session, &viewport).await?;
            Ok::<_, String>(State {
                session: session.clone(),
                root,
                viewport,
                worlds: HashMap::new(),
                sessions: world::initial_sessions(&session),
            })
        }
        .await;
        match initialized {
            Ok(state) => {
                let close = CancellationToken::new();
                let completion = Arc::new(CloseCompletion {
                    done: AtomicBool::new(false),
                    notify: Notify::new(),
                });
                let closed = Arc::new(AtomicBool::new(false));
                let (outgoing, mut work) = mpsc::unbounded_channel::<Work>();
                let actor_close = close.clone();
                let actor_completion = completion.clone();
                let actor_closed = closed.clone();
                let actor_protocol = protocol.clone();
                let task = tokio::spawn(async move {
                    let mut state = state;
                    loop {
                        let request = tokio::select! {biased;_=actor_close.cancelled()=>break,value=work.recv()=>{let Some(value)=value else{break};value}};
                        let result = tokio::select! {biased;_=actor_close.cancelled()=>break,result=execute(&actor_protocol,&mut state,request.input,request.progress)=>result};
                        let _ = request.reply.send(result);
                    }
                    actor_closed.store(true, Ordering::Release);
                    work.close();
                    while let Ok(request) = work.try_recv() {
                        let _ = request
                            .reply
                            .send(Err("Target page, context or browser has been closed".into()));
                    }
                    let _ = actor_protocol
                        .call_for(None, "Browser.close", json!({}), Duration::from_secs(2))
                        .await;
                    initializer.close().await;
                    actor_protocol.close().await;
                    process.close().await;
                    actor_completion.done.store(true, Ordering::Release);
                    actor_completion.notify.notify_waiters();
                });
                Ok(Self(Arc::new(Core {
                    outgoing,
                    closed,
                    close,
                    completion,
                    task: SyncMutex::new(Some(task)),
                })))
            }
            Err(error) => {
                let _ = protocol
                    .call_for(None, "Browser.close", json!({}), Duration::from_secs(2))
                    .await;
                initializer.close().await;
                protocol.close().await;
                process.close().await;
                Err(error)
            }
        }
    }
}
impl BrowserDriver for CdpDriver {
    fn execute(
        &self,
        input: Value,
        on_progress: Option<BrowserProgress>,
    ) -> BoxFuture<'static, BrowserResult<Value>> {
        let outgoing = self.0.outgoing.clone();
        let closed = self.0.closed.clone();
        Box::pin(async move {
            if closed.load(Ordering::Acquire) {
                return Err("Target page, context or browser has been closed".into());
            }
            let (reply, result) = oneshot::channel();
            outgoing
                .send(Work {
                    input,
                    progress: on_progress,
                    reply,
                })
                .map_err(|_| "Target page, context or browser has been closed".to_owned())?;
            result
                .await
                .map_err(|_| "Target page, context or browser has been closed".to_owned())?
        })
    }
    fn close(&self) -> BoxFuture<'static, BrowserResult<()>> {
        let core = self.0.clone();
        Box::pin(async move {
            core.closed.store(true, Ordering::Release);
            core.close.cancel();
            loop {
                let notified = core.completion.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if core.completion.done.load(Ordering::Acquire) {
                    break;
                }
                notified.await;
            }
            let task = core.task.lock().ok().and_then(|mut task| task.take());
            if let Some(task) = task {
                let _ = task.await;
            }
            Ok(())
        })
    }
}
async fn execute(
    protocol: &Protocol,
    state: &mut State,
    input: Value,
    progress: Option<BrowserProgress>,
) -> BrowserResult<Value> {
    let action = input["action"].as_str().unwrap_or("inspect");
    let notify = |value: String| {
        if let Some(progress) = &progress {
            progress(value)
        }
    };
    match action {
        "open" => {
            let url = input["url"]
                .as_str()
                .ok_or_else(|| "Browser URL was not normalized".to_owned())?;
            notify(format!("Opening {url}"));
            set_viewport(protocol, &state.session, &input["viewport"]).await?;
            state.viewport = input["viewport"].clone();
            navigate(protocol, state, url).await?;
            optional_wait(protocol, &input).await?;
            let (url, title) = metadata(protocol, state).await?;
            Ok(json!({"action":action,"url":url,"title":title,"viewport":input["viewport"]}))
        }
        "inspect" => {
            let world = world::world(protocol, state, &state.root.clone()).await?;
            let mut result =
                world::evaluate(protocol, &world, include_str!("browser_scripts/inspect.js"))
                    .await?;
            result["action"] = json!(action);
            Ok(result)
        }
        "click" | "type" => {
            let selector = input["selector"]
                .as_str()
                .ok_or_else(|| "A selector is required for this browser action.".to_owned())?;
            notify(if action == "click" {
                format!("Clicking {selector}")
            } else {
                format!("Typing into {selector}")
            });
            if action == "click" {
                actions::locator(protocol, state, selector, None, false).await?;
            } else {
                let text = input["text"].as_str().unwrap_or("");
                actions::locator(protocol, state, selector, Some(text), false).await?;
                if coercion::truthy(&input["submit"]) {
                    actions::locator(protocol, state, selector, None, true).await?;
                }
            }
            optional_wait(protocol, &input).await?;
            let (url, title) = metadata(protocol, state).await?;
            let mut result = json!({"action":action,"selector":selector,"url":url,"title":title});
            if action == "type" {
                result["submitted"] = json!(coercion::truthy(&input["submit"]));
            }
            Ok(result)
        }
        "wait" => {
            let milliseconds = coercion::wait_ms(input.get("waitMs"), 1000.)?;
            wait(protocol, milliseconds).await?;
            let (url, title) = metadata(protocol, state).await?;
            Ok(json!({"action":action,"waitMs":milliseconds,"url":url,"title":title}))
        }
        "screenshot" => {
            let (url, _title) = metadata(protocol, state).await?;
            notify(format!("Capturing {url}"));
            screenshot::capture(protocol, state, &input).await
        }
        other => Err(format!("Unsupported browser action: {other}")),
    }
}
async fn optional_wait(protocol: &Protocol, input: &Value) -> Result<(), String> {
    if input.get("waitMs").is_some_and(coercion::truthy) {
        wait(protocol, coercion::wait_ms(input.get("waitMs"), 0.)?).await?;
    }
    Ok(())
}
async fn wait(protocol: &Protocol, milliseconds: f64) -> Result<(), String> {
    let cancellation = protocol.cancellation();
    tokio::select! {biased;_=cancellation.cancelled()=>Err("Target page, context or browser has been closed".into()),_=tokio::time::sleep(Duration::from_secs_f64(milliseconds/1000.))=>Ok(())}
}
async fn set_viewport(protocol: &Protocol, session: &str, viewport: &Value) -> Result<(), String> {
    if let Ok(window) = protocol
        .call(Some(session), "Browser.getWindowForTarget", json!({}))
        .await
    {
        protocol.call(Some(session),"Browser.setWindowBounds",json!({"windowId":window["windowId"],"bounds":{"width":viewport["width"],"height":viewport["height"]}})).await?;
    }
    protocol.call(Some(session),"Emulation.setDeviceMetricsOverride",json!({"width":viewport["width"],"height":viewport["height"],"deviceScaleFactor":1,"mobile":false,"screenWidth":viewport["width"],"screenHeight":viewport["height"],"screenOrientation":{"angle":0,"type":"landscapePrimary"}})).await?;
    Ok(())
}
async fn initialize_defaults(protocol: &Protocol, session: &str) -> Result<(), String> {
    let fonts: Value = serde_json::from_str(include_str!(
        "browser_scripts/playwright-font-families.json"
    ))
    .map_err(|e| e.to_string())?;
    let platform = if cfg!(windows) {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    };
    protocol
        .call(
            Some(session),
            "Page.setFontFamilies",
            fonts[platform].clone(),
        )
        .await?;
    protocol
        .call(Some(session), "Emulation.setGeolocationOverride", json!({}))
        .await?;
    protocol.call(Some(session),"Emulation.setEmulatedMedia",json!({"media":"","features":[{"name":"prefers-color-scheme","value":"light"},{"name":"prefers-reduced-motion","value":"no-preference"},{"name":"forced-colors","value":"none"},{"name":"prefers-contrast","value":"no-preference"}]})).await?;
    Ok(())
}
async fn metadata(protocol: &Protocol, state: &mut State) -> Result<(Value, Value), String> {
    for _ in 0..3 {
        let world = world::world(protocol, state, &state.root.clone()).await?;
        match world::evaluate(
            protocol,
            &world,
            "({url:location.href,title:document.title})",
        )
        .await
        {
            Ok(value) => return Ok((value["url"].clone(), value["title"].clone())),
            Err(error) if error.contains("context") || error.contains("object") => {
                state.worlds.clear();
            }
            Err(error) => return Err(error),
        }
    }
    Err("Browser document changed while reading page metadata".into())
}
async fn navigate(protocol: &Protocol, state: &mut State, url: &str) -> Result<(), String> {
    let operation = async {
        let mut events = protocol.events();
        let result = protocol
            .call(Some(&state.session), "Page.navigate", json!({"url":url}))
            .await?;
        if let Some(error) = result["errorText"].as_str() {
            return Err(format!("page.goto: {error} at {url}"));
        }
        if result["isDownload"] == true {
            return Err("page.goto: Download is starting".into());
        }
        let Some(loader) = result["loaderId"].as_str() else {
            state.worlds.clear();
            return Ok(());
        };
        if let Some(frame) = result["frameId"].as_str() {
            state.root = frame.into()
        }
        state.worlds.clear();
        loop {
            if protocol
                .snapshot()?
                .frames
                .get(&state.root)
                .is_some_and(|frame| frame.loader == loader && frame.dom_loaded)
            {
                return Ok(());
            }
            let event = protocol.event(&mut events).await?;
            if event["method"] == "Page.lifecycleEvent"
                && event["params"]["frameId"] == state.root
                && event["params"]["loaderId"] == loader
                && event["params"]["name"] == "DOMContentLoaded"
            {
                return Ok(());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), operation)
        .await
        .map_err(|_| "page.goto: Timeout 30000ms exceeded.".to_owned())?
}

#[cfg(test)]
#[path = "cdp/lifecycle_tests.rs"]
mod lifecycle_tests;
