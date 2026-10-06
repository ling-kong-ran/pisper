use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::{NativeRuntimeTools, NativeToolContext};
use crate::agent_core::harness::context::{background_context, Context};
use crate::agent_core::harness::runtime::drive::tools::ToolContextSource;
use crate::agent_core::harness::types::{
    AgentHarnessTool, AgentHarnessToolInvocation, AgentHarnessToolUpdateCallback,
    AgentHarnessToolUpdateOptions, HarnessExecuteFn,
};
use crate::agent_core::types::{
    AgentToolResult, PrepareArgumentsFn, ToolExecutionMode, ToolReplay,
};

#[derive(Default)]
struct Invocation(Mutex<BTreeMap<String, Value>>);

impl AgentHarnessToolInvocation for Invocation {
    fn invocation_id(&self) -> &str {
        "invocation"
    }
    fn operation_id(&self) -> &str {
        "operation"
    }
    fn turn_id(&self) -> &str {
        "turn"
    }
    fn get_memo<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move { self.0.lock().unwrap().get(name).cloned() })
    }
    fn set_memo<'a>(&'a self, name: &'a str, value: Option<Value>) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut values = self.0.lock().unwrap();
            match value {
                Some(value) => {
                    values.insert(name.to_owned(), value);
                }
                None => {
                    values.remove(name);
                }
            }
        })
    }
}

fn tool<T: Send + Sync + 'static>(execute: Arc<HarnessExecuteFn<T>>) -> AgentHarnessTool<T> {
    AgentHarnessTool {
        name: "native".into(),
        label: "Native tool".into(),
        description: "Uses callbacks, not a JSON context".into(),
        parameters: json!({"type": "object", "properties": {"n": {"type": "integer"}}}),
        constrained_sampling: None,
        execute,
        prepare_arguments: None,
        replay: Some(ToolReplay::Safe),
        execution_mode: Some(ToolExecutionMode::Sequential),
    }
}

async fn resolve(source: ToolContextSource<NativeToolContext>) -> NativeToolContext {
    match source {
        ToolContextSource::Value(value) => value,
        ToolContextSource::Provider(provider) => provider(background_context()).await,
        ToolContextSource::FallibleProvider(provider) => {
            provider(background_context()).await.unwrap()
        }
    }
}

#[derive(Clone)]
struct ApplicationContext {
    identity: Arc<AtomicUsize>,
    callback: Arc<dyn Fn(usize) -> usize + Send + Sync>,
}

#[tokio::test]
async fn native_context_preserves_identity_callbacks_updates_and_invocation() {
    let identity = Arc::new(AtomicUsize::new(0));
    let expected_identity = Arc::clone(&identity);
    let callback: Arc<dyn Fn(usize) -> usize + Send + Sync> = Arc::new(|n| n + 7);
    let expected_callback = Arc::clone(&callback);
    let prepare: Arc<PrepareArgumentsFn> = Arc::new(|value| json!({"n": value}));
    let mut native = tool(Arc::new(
        move |id, arguments, update, context: ApplicationContext, invocation, _caller| {
            assert_eq!(id, "call");
            assert_eq!(arguments, json!({"n": 5}));
            assert!(Arc::ptr_eq(&context.identity, &expected_identity));
            assert!(Arc::ptr_eq(&context.callback, &expected_callback));
            Box::pin(async move {
                context.identity.fetch_add(1, Ordering::SeqCst);
                invocation.set_memo("seen", Some(json!(12))).await;
                let result = AgentToolResult {
                    details: Some(json!((context.callback)(5))),
                    ..Default::default()
                };
                update(&result, AgentHarnessToolUpdateOptions { checkpoint: true });
                Ok(result)
            })
        },
    ));
    native.prepare_arguments = Some(Arc::clone(&prepare));
    let expected_declaration = native.declaration();
    let config = NativeRuntimeTools::new(
        vec![native],
        Some(ToolContextSource::Value(ApplicationContext {
            identity: Arc::clone(&identity),
            callback,
        })),
    );
    assert_eq!(config.declarations()[0], expected_declaration);
    let (tools, source) = config.into_parts();
    let native = &tools[0];
    assert!(Arc::ptr_eq(
        native.prepare_arguments.as_ref().unwrap(),
        &prepare
    ));
    assert_eq!(native.replay, Some(ToolReplay::Safe));
    assert_eq!(native.execution_mode, Some(ToolExecutionMode::Sequential));
    let updates = Arc::new(AtomicUsize::new(0));
    let updates_sink = Arc::clone(&updates);
    let update: Arc<AgentHarnessToolUpdateCallback> = Arc::new(move |result, options| {
        assert!(options.checkpoint);
        assert_eq!(result.details, Some(json!(12)));
        updates_sink.fetch_add(1, Ordering::SeqCst);
    });
    let invocation: Arc<dyn AgentHarnessToolInvocation> = Arc::new(Invocation::default());
    let result = (native.execute)(
        "call".into(),
        json!({"n": 5}),
        update,
        resolve(source).await,
        Arc::clone(&invocation),
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(result.details, Some(json!(12)));
    assert_eq!(identity.load(Ordering::SeqCst), 1);
    assert_eq!(updates.load(Ordering::SeqCst), 1);
    assert_eq!(invocation.get_memo("seen").await, Some(json!(12)));
}

#[tokio::test]
async fn native_provider_is_lazy_and_preserves_async_context_values() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let source = ToolContextSource::Provider(Arc::new(move |_context: Context| {
        let calls = Arc::clone(&provider_calls);
        Box::pin(async move {
            tokio::task::yield_now().await;
            calls.fetch_add(1, Ordering::SeqCst);
            Arc::new(AtomicUsize::new(42))
        }) as BoxFuture<'static, Arc<AtomicUsize>>
    }));
    let config = NativeRuntimeTools::new(
        Vec::<AgentHarnessTool<Arc<AtomicUsize>>>::new(),
        Some(source),
    );
    let cloned = config.clone();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "construction and cloning never resolve a context"
    );
    let (_, source) = config.into_parts();
    let value = resolve(source).await;
    assert_eq!(
        value
            .typed::<Arc<AtomicUsize>>()
            .unwrap()
            .load(Ordering::SeqCst),
        42
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (_, source) = cloned.into_parts();
    resolve(source).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a later batch gets a fresh provider result"
    );
}

#[tokio::test]
async fn omitted_context_remains_unit_and_executes_unit_tools() {
    let native = tool(Arc::new(
        |_id, _args, _update, (): (), _invocation, _caller| {
            Box::pin(async { Ok(AgentToolResult::default()) })
        },
    ));
    let (tools, source) = NativeRuntimeTools::new(vec![native], None).into_parts();
    let result = (tools[0].execute)(
        "unit".into(),
        json!({}),
        Arc::new(|_, _| {}),
        resolve(source).await,
        Arc::new(Invocation::default()),
        background_context(),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn omitted_context_never_forges_a_non_unit_application_value() {
    let native = tool(Arc::new(
        |_id, _args, _update, _context: Arc<AtomicUsize>, _invocation, _caller| {
            panic!("the mismatched native context must not reach the executor")
        },
    ));
    let (tools, source) = NativeRuntimeTools::new(vec![native], None).into_parts();
    let error = (tools[0].execute)(
        "missing".into(),
        json!({}),
        Arc::new(|_, _| {}),
        resolve(source).await,
        Arc::new(Invocation::default()),
        background_context(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("Tool context type mismatch"));
}
