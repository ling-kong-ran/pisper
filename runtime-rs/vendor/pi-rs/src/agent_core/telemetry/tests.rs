//! Tests for the telemetry runtime core: no-op pass-through semantics and
//! the in-memory backend replayed against the actual upstream
//! `packages/telemetry/src/memory.ts` executed under Node
//! (`docs/migration/reference/telemetry/oracle.mjs` → `telemetry_oracle.json`).

use super::memory::InMemoryTelemetryContext;
use super::{
    start_child_span, AttributeValue, SpanError, SpanOptions, SpanStatus, TelemetryContext,
    TelemetrySpanT,
};
use std::sync::{Arc, Mutex};

fn oracle_lines() -> Vec<String> {
    let oracle = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/migration/reference/telemetry/telemetry_oracle.json"
    ))
    .expect("telemetry oracle file");
    oracle
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

fn str_value(value: &str) -> AttributeValue {
    AttributeValue::Str(value.to_string())
}

fn num_value(value: i64) -> AttributeValue {
    AttributeValue::Num(serde_json::Number::from(value))
}

/// Upstream no-op context (`noop.ts`): the callback runs, its value is
/// returned, and its `Err` rejects — span recording is inert.
#[tokio::test]
async fn noop_context_runs_the_callback_and_propagates_errors() {
    let telemetry = TelemetryContext::noop();
    let value = telemetry
        .start_span(SpanOptions::new("span", vec![]), |_span| {
            Box::pin(async { Ok(7_i32) })
        })
        .await
        .unwrap();
    assert_eq!(value, 7);

    let error = telemetry
        .start_span(SpanOptions::new("span", vec![]), |_span| {
            Box::pin(async { Err::<(), _>(anyhow::anyhow!("boom")) })
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "boom");
}

/// Replay of the upstream in-memory scenario: attribute merge semantics,
/// explicit/automatic statuses, settled-parent no-op delegation, and
/// post-settle mutation suppression — byte-compared against the actual
/// upstream backend's `getSpans()` serialization.
#[tokio::test]
async fn memory_backend_replays_the_upstream_oracle() {
    let lines = oracle_lines();
    assert_eq!(lines.len(), 3, "oracle layout: schemas, spans, delegation");

    // Scenario A (oracle line 1): nested spans, merge, statuses.
    let memory = InMemoryTelemetryContext::new();
    let memory_context: TelemetryContext = memory.clone().into();
    memory_context
        .start_span(
            SpanOptions::new(
                "root",
                vec![
                    ("a".to_string(), str_value("1")),
                    ("n".to_string(), num_value(2)),
                    ("b".to_string(), AttributeValue::Bool(true)),
                    (
                        "arr".to_string(),
                        AttributeValue::StrArray(vec!["x".to_string(), "y".to_string()]),
                    ),
                ],
            ),
            |root| {
                Box::pin(async move {
                    root.add_event("evt", vec![("k".to_string(), str_value("v"))]);
                    // `n` replaces in place; `extra` appends (`memory.ts:63-69`).
                    root.set_attributes(vec![
                        ("n".to_string(), num_value(9)),
                        ("extra".to_string(), str_value("e")),
                    ]);
                    start_child_span::<&str, _>(
                        &root,
                        SpanOptions::new("child", vec![]),
                        |child| {
                            Box::pin(async move {
                                child.set_status(SpanStatus::Error {
                                    error: Some(SpanError {
                                        name: "AbortError".to_string(),
                                        message: "stop".to_string(),
                                    }),
                                });
                                Ok("child-ok")
                            })
                        },
                    )
                    .await?;
                    start_child_span::<&str, _>(
                        &root,
                        SpanOptions::new("fail-child", vec![]),
                        |_child| Box::pin(async { Err(anyhow::anyhow!("boom")) }),
                    )
                    .await
                    .inspect_err(|error| assert_eq!(error.to_string(), "boom"))
                    .ok();
                    start_child_span::<&str, _>(
                        &root,
                        SpanOptions::new("explicit-error", vec![]),
                        |span| {
                            Box::pin(async move {
                                span.set_status(SpanStatus::Error { error: None });
                                Ok("handled")
                            })
                        },
                    )
                    .await?;
                    // An explicit status suppresses the automatic one
                    // (`memory.ts:96`); the failure still propagates.
                    let suppressed = start_child_span::<&str, _>(
                        &root,
                        SpanOptions::new("explicit-error-fail", vec![]),
                        |span| {
                            Box::pin(async move {
                                span.set_status(SpanStatus::Error {
                                    error: Some(SpanError {
                                        name: "Handled".to_string(),
                                        message: "known".to_string(),
                                    }),
                                });
                                Err(anyhow::anyhow!("boom2"))
                            })
                        },
                    )
                    .await;
                    assert_eq!(suppressed.unwrap_err().to_string(), "boom2");
                    Ok("root-ok")
                })
            },
        )
        .await
        .unwrap();
    let recorded = serde_json::to_string(&memory.get_spans()).unwrap();
    assert_eq!(recorded, lines[1], "memory scenario spans");

    // Scenario B (oracle line 2): a child of a settled span delegates to the
    // no-op context (`memory.ts:126`) and post-settle mutations are ignored.
    let scope = InMemoryTelemetryContext::new();
    let scope_context: TelemetryContext = scope.clone().into();
    let slot: Arc<Mutex<Option<Arc<dyn TelemetrySpanT>>>> = Arc::new(Mutex::new(None));
    let slot_for_callback = Arc::clone(&slot);
    scope_context
        .start_span(SpanOptions::new("r", vec![]), move |root| {
            let slot = Arc::clone(&slot_for_callback);
            Box::pin(async move {
                *slot.lock().unwrap() = Some(root);
                Ok(())
            })
        })
        .await
        .unwrap();
    let settled_root = slot.lock().unwrap().clone().unwrap();
    let late = start_child_span::<&str, _>(
        &settled_root,
        SpanOptions::new("late-child", vec![]),
        |_span| Box::pin(async { Ok("ignored") }),
    )
    .await
    .unwrap();
    assert_eq!(late, "ignored");
    settled_root.add_event("late-event", vec![("x".to_string(), num_value(1))]);
    settled_root.set_attributes(vec![("late".to_string(), str_value("no"))]);
    settled_root.set_status(SpanStatus::Error { error: None });
    let delegation = serde_json::to_string(&scope.get_spans()).unwrap();
    assert_eq!(delegation, lines[2], "delegation scenario spans");
}
