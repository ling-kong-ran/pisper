use super::*;
use crate::coding_agent::{
    core::{
        event_bus::EventBusController,
        trust_test_support::{oracle, Fixture},
    },
    extensions::{
        loader::{load_extension_from_factory, ExtensionRuntime},
        types::{ExtensionMode, ExtensionUI, HandlerFn, HandlerResult, UiFuture},
    },
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Trace = Arc<Mutex<Vec<Value>>>;
struct SelectUi {
    spec: Value,
    trace: Trace,
}
impl ExtensionUI for SelectUi {
    fn select<'a>(
        &'a self,
        title: &'a str,
        options: &'a [String],
        _: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move {
            self.trace
                .lock()
                .unwrap()
                .push(json!(["select", title, options]));
            tokio::task::yield_now().await;
            if let Some(error) = self.spec["uiError"].as_str() {
                return Err(error.into());
            }
            Ok(self.spec["label"].as_str().map(str::to_string).or_else(|| {
                self.spec["selection"]
                    .as_u64()
                    .and_then(|i| options.get(i as usize).cloned())
            }))
        })
    }
}
fn extensions(cwd: &str, spec: &Value, trace: &Trace) -> LoadExtensionsResult {
    let runtime = ExtensionRuntime::new();
    let items = spec["handlers"].as_array().cloned().unwrap_or_default();
    let trace = trace.clone();
    let extension = load_extension_from_factory(
        Arc::new(move |api| {
            for (i, item) in items.iter().enumerate() {
                let item = item.clone();
                let trace = trace.clone();
                let handler: HandlerFn = Arc::new(move |event, ctx| {
                    let item = item.clone();
                    let trace = trace.clone();
                    let event = event.clone();
                    let cwd = ctx.cwd().unwrap();
                    let mode = ctx.mode().unwrap().as_str();
                    let has_ui = ctx.has_ui().unwrap();
                    Box::pin(async move {
                        trace
                            .lock()
                            .unwrap()
                            .push(json!(["handler", i, event, cwd, mode, has_ui]));
                        tokio::task::yield_now().await;
                        if let Some(error) = item["error"].as_str() {
                            Err(error.into())
                        } else {
                            Ok(Some(HandlerResult::Json(item)))
                        }
                    })
                });
                api.on("project_trust", handler)?;
            }
            Ok(())
        }),
        cwd,
        EventBusController::new().bus().clone(),
        &runtime,
        Some("fixture-extension"),
    )
    .unwrap();
    LoadExtensionsResult {
        extensions: vec![extension],
        errors: vec![],
        warnings: vec![],
        runtime,
    }
}

#[tokio::test]
async fn trust_resolution_priority_ui_errors_and_persistence_match_upstream() {
    for case in oracle()["resolveCases"].as_array().unwrap() {
        let f = Fixture::new();
        let cwd = f.p("/root/home/project");
        std::fs::create_dir_all(&cwd).unwrap();
        if case["noResources"] != true {
            f.write("/root/home/project/.pi/settings.json", "{}");
        }
        let store = f.store();
        if let Some(v) = case["stored"].as_bool() {
            store.set(&cwd, Some(v)).unwrap();
        }
        let trace: Trace = Arc::default();
        let ui = Arc::new(SelectUi {
            spec: case.clone(),
            trace: trace.clone(),
        });
        let ctx = ProjectTrustContext {
            cwd: cwd.clone(),
            mode: ExtensionMode::Print,
            has_ui: case["ui"] == true,
            ui: Some(ui),
        };
        let ext = extensions(&cwd, case, &trace);
        let errors = {
            let trace = trace.clone();
            move |e: String| trace.lock().unwrap().push(json!(["error", e]))
        };
        let result = resolve_project_trusted(ResolveProjectTrustedOptions {
            trust_override: case["override"].as_bool(),
            default_project_trust: match case["policy"].as_str() {
                Some("always") => Some(DefaultProjectTrust::Always),
                Some("never") => Some(DefaultProjectTrust::Never),
                _ => None,
            },
            extensions_result: case.get("handlers").map(|_| &ext),
            on_extension_error: Some(&errors),
            ..ResolveProjectTrustedOptions::new(&cwd, &store, &ctx)
        })
        .await;
        let observed = match result {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":f.text(&error)}),
        };
        if case.get("error").is_some() {
            assert_eq!(observed, json!({"error":case["error"]}), "{}", case["id"]);
        } else {
            assert_eq!(observed, json!({"value":case["value"]}), "{}", case["id"]);
        }
        assert_eq!(
            f.value(json!(trace.lock().unwrap().clone())),
            case["trace"],
            "{}: complete callback trace",
            case["id"]
        );
        assert_eq!(
            f.file(),
            case["file"],
            "{}: exact persistent bytes",
            case["id"]
        );
    }
}

#[tokio::test]
async fn explicit_override_and_empty_project_do_not_read_invalid_store() {
    let f = Fixture::new();
    let cwd = f.p("/root/home/project");
    std::fs::create_dir_all(&cwd).unwrap();
    f.write("/root/agent/trust.json", "invalid");
    let store = f.store();
    let ctx = ProjectTrustContext {
        cwd: cwd.clone(),
        mode: ExtensionMode::Print,
        has_ui: false,
        ui: None,
    };
    assert!(
        resolve_project_trusted(ResolveProjectTrustedOptions::new(&cwd, &store, &ctx))
            .await
            .unwrap()
    );
    f.write("/root/home/project/.pi/settings.json", "{}");
    let mut options = ResolveProjectTrustedOptions::new(&cwd, &store, &ctx);
    options.trust_override = Some(false);
    assert!(!resolve_project_trusted(options).await.unwrap());
    assert!(
        resolve_project_trusted(ResolveProjectTrustedOptions::new(&cwd, &store, &ctx))
            .await
            .unwrap_err()
            .starts_with("Failed to read trust store ")
    );
}

struct WaitingUi {
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl ExtensionUI for WaitingUi {
    fn select<'a>(
        &'a self,
        _: &'a str,
        options: &'a [String],
        _: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        let gate = self.gate.lock().unwrap().take().unwrap();
        Box::pin(async move {
            gate.await.map_err(|e| e.to_string())?;
            Ok(Some(options[0].clone()))
        })
    }
}
#[tokio::test]
async fn async_dialog_cancellation_never_persists_a_decision() {
    let f = Fixture::new();
    let cwd = f.p("/root/home/project");
    f.write("/root/home/project/.pi/settings.json", "{}");
    let store = f.store();
    let (release, gate) = tokio::sync::oneshot::channel();
    let ctx = ProjectTrustContext {
        cwd: cwd.clone(),
        mode: ExtensionMode::Tui,
        has_ui: true,
        ui: Some(Arc::new(WaitingUi {
            gate: Mutex::new(Some(gate)),
        })),
    };
    let mut future = Box::pin(resolve_project_trusted(ResolveProjectTrustedOptions::new(
        &cwd, &store, &ctx,
    )));
    assert!(futures::poll!(future.as_mut()).is_pending());
    assert_eq!(f.file(), Value::Null);
    drop(future);
    assert!(release.send(()).is_err());
    assert_eq!(store.get(&cwd).unwrap(), None);
    assert_eq!(f.file(), Value::Null);
}
