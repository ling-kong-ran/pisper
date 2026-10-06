//! OOPIF 在脚本开始前配置默认环境；初始化任务只拥有连接和取消令牌，不反向拥有驱动。
use super::{initialize_defaults, protocol::Protocol};
use serde_json::json;
use std::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct ContextInitializer {
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}
impl Drop for ContextInitializer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl ContextInitializer {
    pub(super) fn new(protocol: Protocol) -> Self {
        let cancel = CancellationToken::new();
        let owner_cancel = cancel.clone();
        let mut events = protocol.events();
        let task = tokio::spawn(async move {
            loop {
                let event = tokio::select! {biased;_=owner_cancel.cancelled()=>break,event=protocol.event(&mut events)=>match event{Ok(event)=>event,Err(_)=>break}};
                if event["method"] != "Target.attachedToTarget" {
                    continue;
                }
                // 浏览器层手动 attach 的主页面由构造器配置；这里只配置自动 attach 的子目标。
                if event["sessionId"].as_str().is_none() {
                    continue;
                }
                let params = &event["params"];
                let kind = params["targetInfo"]["type"].as_str().unwrap_or("");
                let Some(session) = params["sessionId"].as_str() else {
                    continue;
                };
                let session = session.to_owned();
                let operation = async {
                    if matches!(kind, "iframe" | "page") {
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
                                "Page.setLifecycleEventsEnabled",
                                json!({"enabled":true}),
                            )
                            .await?;
                        protocol.call(Some(&session),"Target.setAutoAttach",json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true})).await?;
                    }
                    // workers 也可能被自动附加；没有领域动作需要暂停它们。
                    protocol
                        .call(Some(&session), "Runtime.runIfWaitingForDebugger", json!({}))
                        .await?;
                    protocol.session_ready(&session)?;
                    Ok::<_, String>(())
                };
                tokio::select! {biased;_=owner_cancel.cancelled()=>break,result=operation=>{if result.is_err(){tokio::select!{biased;_=owner_cancel.cancelled()=>break,_=protocol.call(Some(&session),"Runtime.runIfWaitingForDebugger",json!({}))=>{}};let _=protocol.session_ready(&session);}}}
            }
        });
        Self {
            cancel,
            task: Mutex::new(Some(task)),
        }
    }
    pub(super) async fn close(&self) {
        self.cancel.cancel();
        let task = self.task.lock().ok().and_then(|mut task| task.take());
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}
