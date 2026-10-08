//! Tests for the harness telemetry schema module: byte-comparison of the two
//! schema literals against `JSON.stringify` of the actual upstream
//! `packages/agent/src/harness/telemetry.ts` (run under Node by
//! `docs/migration/reference/telemetry/oracle.mjs`), and the context
//! threading of `getTelemetryContext`/`withTelemetryContext`/`startHarnessSpan`.

use super::{
    create_typed_span_starter, get_telemetry_context, start_ai_span, start_harness_span,
    with_telemetry_context, AGENT_TELEMETRY_SCHEMAS, AI_TELEMETRY_SCHEMA, HARNESS_TELEMETRY_SCHEMA,
};
use crate::agent_core::harness::context::Context;
use crate::agent_core::telemetry::{AttributeValue, SpanOptions};
use serde::Serialize;

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

#[derive(Serialize)]
struct SchemaOracle {
    ai: &'static crate::agent_core::telemetry::schema::TelemetrySchemaDefinition,
    harness: &'static crate::agent_core::telemetry::schema::TelemetrySchemaDefinition,
}

/// The serialized schema constants byte-match `JSON.stringify` of the actual
/// upstream literals (field order included — attribute entries interleave
/// `cardinality` differently per entry; the internal `HOOK_NAMES`/
/// `EVENT_TYPES` vocabularies are observable through the schema `values`
/// arrays they feed).
#[test]
fn schema_literals_match_the_actual_upstream_serialization() {
    let lines = oracle_lines();
    let actual = serde_json::to_string(&SchemaOracle {
        ai: &AI_TELEMETRY_SCHEMA,
        harness: &HARNESS_TELEMETRY_SCHEMA,
    })
    .unwrap();
    assert_eq!(actual, lines[0], "schema vocabulary");
}

/// `AGENT_TELEMETRY_SCHEMAS` combines the two schemas in upstream order.
#[test]
fn agent_schemas_combine_ai_and_harness_in_order() {
    assert_eq!(AGENT_TELEMETRY_SCHEMAS.len(), 2);
    assert!(std::ptr::eq(
        AGENT_TELEMETRY_SCHEMAS[0],
        &AI_TELEMETRY_SCHEMA
    ));
    assert!(std::ptr::eq(
        AGENT_TELEMETRY_SCHEMAS[1],
        &HARNESS_TELEMETRY_SCHEMA
    ));
}

/// Without an attached backend the shared no-op parent serves
/// (`context.ts:30`): the callback runs and its result/error propagate.
#[tokio::test]
async fn start_harness_span_defaults_to_the_noop_backend() {
    let context = Context::background();
    assert!(get_telemetry_context(&context)
        .start_span::<i32, _>(SpanOptions::new("probe", vec![]), |_| Box::pin(async {
            Ok(1_i32)
        }))
        .await
        .is_ok());

    let value = start_harness_span(
        "pi.harness.turn",
        vec![
            (
                "pi.lane.name".to_string(),
                AttributeValue::Str("main".to_string()),
            ),
            (
                "pi.operation.id".to_string(),
                AttributeValue::Str("op-1".to_string()),
            ),
            (
                "pi.turn.id".to_string(),
                AttributeValue::Str("turn-1".to_string()),
            ),
        ],
        |_span, _child_context| Box::pin(async { Ok("ok") }),
        context,
    )
    .await
    .unwrap();
    assert_eq!(value, "ok");

    let error = start_ai_span(
        "pi.ai.request",
        vec![],
        |_span, _child_context| Box::pin(async { Err::<(), _>(anyhow::anyhow!("provider down")) }),
        Context::background(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "provider down");
}

/// With an in-memory backend attached, `startHarnessSpan` records through the
/// context slot and the derived child context parents nested spans — the
/// upstream `withTelemetryContext(span, context)` wiring.
#[tokio::test]
async fn start_harness_span_records_through_the_context_backend() {
    let memory = crate::agent_core::telemetry::memory::InMemoryTelemetryContext::new();
    let context = with_telemetry_context(memory.clone().into(), Context::background());

    start_harness_span(
        "pi.harness.turn",
        vec![
            (
                "pi.lane.name".to_string(),
                AttributeValue::Str("main".to_string()),
            ),
            (
                "pi.operation.id".to_string(),
                AttributeValue::Str("op-1".to_string()),
            ),
            (
                "pi.turn.id".to_string(),
                AttributeValue::Str("turn-1".to_string()),
            ),
        ],
        |_span, child_context| {
            Box::pin(async move {
                start_harness_span(
                    "pi.harness.checkpoint",
                    vec![
                        (
                            "pi.lane.name".to_string(),
                            AttributeValue::Str("main".to_string()),
                        ),
                        (
                            "pi.operation.id".to_string(),
                            AttributeValue::Str("op-1".to_string()),
                        ),
                        (
                            "pi.checkpoint.kind".to_string(),
                            AttributeValue::Str("normal".to_string()),
                        ),
                    ],
                    |_checkpoint, _grandchild_context| Box::pin(async { Ok("checkpointed") }),
                    child_context,
                )
                .await
            })
        },
        context,
    )
    .await
    .unwrap();

    let spans = memory.get_spans();
    assert_eq!(spans.len(), 2, "turn + checkpoint recorded");
    assert_eq!(spans[0].name, "pi.harness.turn");
    assert_eq!(spans[0].parent_id, None);
    assert_eq!(spans[1].name, "pi.harness.checkpoint");
    assert_eq!(spans[1].parent_id, Some(spans[0].id));
    assert_eq!(
        spans[0]
            .attributes
            .iter()
            .find(|(name, _)| name == "pi.turn.id")
            .map(|(_, value)| value.clone()),
        Some(AttributeValue::Str("turn-1".to_string()))
    );
}

/// Upstream `createTypedSpanStarter` (`index.ts:349-354`): binds a parent
/// context; children started through the passed starter parent to the span.
#[tokio::test]
async fn typed_span_starter_parents_children_to_the_active_span() {
    let memory = crate::agent_core::telemetry::memory::InMemoryTelemetryContext::new();
    let starter = create_typed_span_starter(memory.clone().into());

    starter
        .start(
            SpanOptions::new("pi.harness.run", vec![]),
            |_span, child_starter| {
                Box::pin(async move {
                    child_starter
                        .start::<&str, _>(
                            SpanOptions::new("pi.harness.step", vec![]),
                            |_step, _grandchild| Box::pin(async { Ok("stepped") }),
                        )
                        .await
                })
            },
        )
        .await
        .unwrap();

    let spans = memory.get_spans();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].name, "pi.harness.run");
    assert_eq!(spans[1].name, "pi.harness.step");
    assert_eq!(spans[1].parent_id, Some(spans[0].id));
}
