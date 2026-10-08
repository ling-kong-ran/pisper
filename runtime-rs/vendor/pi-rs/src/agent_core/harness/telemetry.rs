//! Port of `packages/agent/src/harness/telemetry.ts` (636 lines) plus the
//! telemetry slice of `packages/agent/src/harness/context.ts` that
//! `context.rs` deferred to this module (`context.ts:27-36`).
//!
//! The file is a schema-definition module: two `as const` schema literals
//! (`AI_TELEMETRY_SCHEMA`, `HARNESS_TELEMETRY_SCHEMA`), the combined
//! vocabulary (`AGENT_TELEMETRY_SCHEMAS`), the hook/event vocabularies, and
//! two thin span starters delegating to the context-attached telemetry
//! backend. The serialized schema literals are a compatibility surface, so
//! every attribute entry preserves its upstream literal field order (the
//! oracle replays `JSON.stringify` of the actual upstream file and compares
//! bytes — see the tests).
//!
//! TypeScript-only machinery (the `TelemetrySchemaSpanName<...>` inference
//! layer and the `ExactTelemetryAttributes` excess-property checks,
//! `telemetry.ts:120-136` and `597-620`) maps to Rust's type system and is
//! not ported; `startAiSpan`/`startHarnessSpan` keep the upstream argument
//! order `(name, attributes, callback, context)`.

use crate::agent_core::harness::context::{with_context_value, Context, ContextKey};
use crate::agent_core::telemetry::schema::{
    AttrField, AttrScalar, ParentsKind, SpanDefinition, StatusDefinition, TelemetrySchemaDefinition,
};
use crate::agent_core::telemetry::{
    SpanAttributes, SpanOptions, TelemetryContext, TelemetrySpanT, TypedSpanStarter,
};
use futures::future::BoxFuture;
use std::sync::{Arc, OnceLock};

/// Upstream `HOOK_NAMES` (`telemetry.ts:149-161`).
pub static HOOK_NAMES: &[&str] = &[
    "before_run",
    "before_drive",
    "before_run_end",
    "transform_context",
    "before_request",
    "before_payload",
    "after_response",
    "before_tool",
    "after_tool",
    "before_compaction",
    "before_navigation",
];

/// Upstream `EVENT_TYPES` (`telemetry.ts:163-192`).
pub static EVENT_TYPES: &[&str] = &[
    "run_start",
    "run_resume",
    "run_suspend",
    "operation_abort",
    "run_end",
    "fault",
    "handler_error",
    "turn_start",
    "turn_end",
    "retry_scheduled",
    "retry_start",
    "retry_end",
    "message_start",
    "message_update",
    "message_end",
    "tool_start",
    "tool_update",
    "tool_end",
    "entry_added",
    "queue_update",
    "value_update",
    "config_update",
    "compaction_start",
    "compaction_end",
    "navigation_start",
    "navigation_end",
    "lane_created",
    "usage",
];

/// Upstream `operationStartAttributes` (`telemetry.ts:194-218`), spread into
/// the three operation spans before their `pi.operation.kind` entry.
const OPERATION_START_ATTRIBUTES: &[(&str, &[AttrField])] = &[
    (
        "pi.session.id",
        &[
            AttrField::Type("string"),
            AttrField::Required(true),
            AttrField::Cardinality("high"),
            AttrField::Description("Session id"),
        ],
    ),
    (
        "pi.lane.name",
        &[
            AttrField::Type("string"),
            AttrField::Required(true),
            AttrField::Cardinality("high"),
            AttrField::Description("Lane name"),
        ],
    ),
    (
        "pi.operation.id",
        &[
            AttrField::Type("string"),
            AttrField::Required(true),
            AttrField::Cardinality("high"),
            AttrField::Description("Durable operation id"),
        ],
    ),
    (
        "pi.operation.recovery",
        &[
            AttrField::Type("boolean"),
            AttrField::Required(true),
            AttrField::Description("Whether this invocation resumes durable work"),
        ],
    ),
];

/// Upstream `operationErrorAttributes` (`telemetry.ts:220-231`).
const OPERATION_ERROR_ATTRIBUTES: &[(&str, &[AttrField])] = &[
    (
        "pi.error.code",
        &[
            AttrField::Type("string"),
            AttrField::Cardinality("low"),
            AttrField::Description("Stable operation error code"),
        ],
    ),
    (
        "pi.error.type",
        &[
            AttrField::Type("string"),
            AttrField::Cardinality("low"),
            AttrField::Description("Low-cardinality operation error class"),
        ],
    ),
];

/// Upstream `AI_TELEMETRY_SCHEMA` (`telemetry.ts:42-118`).
pub static AI_TELEMETRY_SCHEMA: TelemetrySchemaDefinition = TelemetrySchemaDefinition {
    version: 1,
    spans: &[(
        "pi.ai.request",
        SpanDefinition {
            description: "One logical request to an AI provider",
            parents: ParentsKind::Any,
            start_attributes: &[
                (
                    "pi.ai.operation",
                    &[
                        AttrField::Type("string"),
                        AttrField::Required(true),
                        AttrField::Values(&[
                            AttrScalar::Str("stream"),
                            AttrScalar::Str("fetch_deferred"),
                            AttrScalar::Str("cancel_deferred"),
                            AttrScalar::Str("generate_images"),
                        ]),
                        AttrField::Description("Logical provider operation"),
                    ],
                ),
                (
                    "pi.ai.provider",
                    &[
                        AttrField::Type("string"),
                        AttrField::Required(true),
                        AttrField::Description("Selected provider id"),
                    ],
                ),
                (
                    "pi.ai.model",
                    &[
                        AttrField::Type("string"),
                        AttrField::Required(true),
                        AttrField::Description("Requested model id"),
                    ],
                ),
                (
                    "pi.ai.api",
                    &[
                        AttrField::Type("string"),
                        AttrField::Required(true),
                        AttrField::Description("Provider API id"),
                    ],
                ),
                (
                    "pi.ai.streaming",
                    &[
                        AttrField::Type("boolean"),
                        AttrField::Required(true),
                        AttrField::Description("Whether this operation returns a stream"),
                    ],
                ),
                (
                    "pi.ai.deferred",
                    &[
                        AttrField::Type("boolean"),
                        AttrField::Required(false),
                        AttrField::Description(
                            "Whether the operation requests or participates in deferred execution",
                        ),
                    ],
                ),
            ],
            end_attributes: &[
                (
                    "pi.ai.response.model",
                    &[
                        AttrField::Type("string"),
                        AttrField::Description("Concrete response model"),
                    ],
                ),
                (
                    "pi.ai.response.id",
                    &[
                        AttrField::Type("string"),
                        AttrField::Cardinality("high"),
                        AttrField::Description("Provider response id"),
                    ],
                ),
                (
                    "pi.ai.response.stop_reason",
                    &[
                        AttrField::Type("string"),
                        AttrField::Values(&[
                            AttrScalar::Str("stop"),
                            AttrScalar::Str("length"),
                            AttrScalar::Str("tool_use"),
                            AttrScalar::Str("error"),
                            AttrScalar::Str("aborted"),
                            AttrScalar::Str("deferred"),
                        ]),
                        AttrField::Description("Normalized terminal response reason"),
                    ],
                ),
                (
                    "pi.ai.http.status_code",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Final HTTP status"),
                    ],
                ),
                (
                    "pi.ai.usage.input_tokens",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported input tokens"),
                    ],
                ),
                (
                    "pi.ai.usage.output_tokens",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported output tokens"),
                    ],
                ),
                (
                    "pi.ai.usage.cache_read_tokens",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported cache-read tokens"),
                    ],
                ),
                (
                    "pi.ai.usage.cache_write_tokens",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported cache-write tokens"),
                    ],
                ),
                (
                    "pi.ai.usage.reasoning_tokens",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported reasoning tokens"),
                    ],
                ),
                (
                    "pi.ai.usage.total_tokens",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported total tokens"),
                    ],
                ),
                (
                    "pi.ai.usage.cost",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Reported total cost"),
                    ],
                ),
                (
                    "pi.ai.stream.chunk_count",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Streamed update chunk count"),
                    ],
                ),
                (
                    "pi.ai.stream.time_to_first_chunk_ms",
                    &[
                        AttrField::Type("number"),
                        AttrField::Description("Elapsed milliseconds to first update chunk"),
                    ],
                ),
                (
                    "pi.ai.error.type",
                    &[
                        AttrField::Type("string"),
                        AttrField::Cardinality("low"),
                        AttrField::Description("Provider or transport error class"),
                    ],
                ),
            ],
            status: StatusDefinition {
                error_when: "The operation throws or returns an error result",
            },
        },
    )],
};

/// Upstream `HARNESS_TELEMETRY_SCHEMA` (`telemetry.ts:233-592`); spans in
/// literal order: run, compaction, navigation, checkpoint, turn, step, tool,
/// hook, sleep, event_handler, session.write.
pub static HARNESS_TELEMETRY_SCHEMA: TelemetrySchemaDefinition = TelemetrySchemaDefinition {
    version: 1,
    spans: &[
        (
            "pi.harness.run",
            SpanDefinition {
                description: "One admitted in-process run invocation",
                parents: ParentsKind::RootOrExternal,
                start_attributes: &[
                    ("pi.session.id", OPERATION_START_ATTRIBUTES[0].1),
                    ("pi.lane.name", OPERATION_START_ATTRIBUTES[1].1),
                    ("pi.operation.id", OPERATION_START_ATTRIBUTES[2].1),
                    ("pi.operation.recovery", OPERATION_START_ATTRIBUTES[3].1),
                    (
                        "pi.operation.kind",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(&[AttrScalar::Str("run")]),
                            AttrField::Description("Run operation kind"),
                        ],
                    ),
                ],
                end_attributes: &[
                    (
                        "pi.operation.outcome",
                        &[
                            AttrField::Type("string"),
                            AttrField::Values(&[
                                AttrScalar::Str("completed"),
                                AttrScalar::Str("aborted"),
                                AttrScalar::Str("failed"),
                                AttrScalar::Str("suspended"),
                            ]),
                            AttrField::Description("Run invocation outcome"),
                        ],
                    ),
                    ("pi.error.code", OPERATION_ERROR_ATTRIBUTES[0].1),
                    ("pi.error.type", OPERATION_ERROR_ATTRIBUTES[1].1),
                ],
                status: StatusDefinition {
                    error_when: "The run fails or throws",
                },
            },
        ),
        (
            "pi.harness.compaction",
            SpanDefinition {
                description: "One admitted in-process manual compaction invocation",
                parents: ParentsKind::RootOrExternal,
                start_attributes: &[
                    ("pi.session.id", OPERATION_START_ATTRIBUTES[0].1),
                    ("pi.lane.name", OPERATION_START_ATTRIBUTES[1].1),
                    ("pi.operation.id", OPERATION_START_ATTRIBUTES[2].1),
                    ("pi.operation.recovery", OPERATION_START_ATTRIBUTES[3].1),
                    (
                        "pi.operation.kind",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(&[AttrScalar::Str("compaction")]),
                            AttrField::Description("Compaction operation kind"),
                        ],
                    ),
                ],
                end_attributes: &[
                    (
                        "pi.operation.outcome",
                        &[
                            AttrField::Type("string"),
                            AttrField::Values(&[
                                AttrScalar::Str("completed"),
                                AttrScalar::Str("declined"),
                                AttrScalar::Str("aborted"),
                                AttrScalar::Str("failed"),
                            ]),
                            AttrField::Description("Compaction invocation outcome"),
                        ],
                    ),
                    ("pi.error.code", OPERATION_ERROR_ATTRIBUTES[0].1),
                    ("pi.error.type", OPERATION_ERROR_ATTRIBUTES[1].1),
                ],
                status: StatusDefinition {
                    error_when: "The compaction fails or throws",
                },
            },
        ),
        (
            "pi.harness.navigation",
            SpanDefinition {
                description: "One admitted in-process navigation invocation",
                parents: ParentsKind::RootOrExternal,
                start_attributes: &[
                    ("pi.session.id", OPERATION_START_ATTRIBUTES[0].1),
                    ("pi.lane.name", OPERATION_START_ATTRIBUTES[1].1),
                    ("pi.operation.id", OPERATION_START_ATTRIBUTES[2].1),
                    ("pi.operation.recovery", OPERATION_START_ATTRIBUTES[3].1),
                    (
                        "pi.operation.kind",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(&[AttrScalar::Str("navigation")]),
                            AttrField::Description("Navigation operation kind"),
                        ],
                    ),
                ],
                end_attributes: &[
                    (
                        "pi.operation.outcome",
                        &[
                            AttrField::Type("string"),
                            AttrField::Values(&[
                                AttrScalar::Str("completed"),
                                AttrScalar::Str("declined"),
                                AttrScalar::Str("aborted"),
                                AttrScalar::Str("failed"),
                            ]),
                            AttrField::Description("Navigation invocation outcome"),
                        ],
                    ),
                    ("pi.error.code", OPERATION_ERROR_ATTRIBUTES[0].1),
                    ("pi.error.type", OPERATION_ERROR_ATTRIBUTES[1].1),
                ],
                status: StatusDefinition {
                    error_when: "The navigation fails or throws",
                },
            },
        ),
        (
            "pi.harness.checkpoint",
            SpanDefinition {
                description: "One run checkpoint",
                parents: ParentsKind::Spans(&["pi.harness.run"]),
                start_attributes: &[
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name"),
                        ],
                    ),
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Durable operation id"),
                        ],
                    ),
                    (
                        "pi.checkpoint.kind",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(&[
                                AttrScalar::Str("normal"),
                                AttrScalar::Str("abort_reconcile"),
                            ]),
                            AttrField::Description("Checkpoint purpose"),
                        ],
                    ),
                ],
                end_attributes: &[],
                status: StatusDefinition {
                    error_when: "Checkpoint work throws",
                },
            },
        ),
        (
            "pi.harness.turn",
            SpanDefinition {
                description: "One assistant response and its tool batch",
                parents: ParentsKind::Spans(&["pi.harness.run"]),
                start_attributes: &[
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name"),
                        ],
                    ),
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Durable operation id"),
                        ],
                    ),
                    (
                        "pi.turn.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Invocation-local turn id"),
                        ],
                    ),
                ],
                end_attributes: &[],
                status: StatusDefinition {
                    error_when: "Turn work throws",
                },
            },
        ),
        (
            "pi.harness.step",
            SpanDefinition {
                description: "One durable retry attempt",
                parents: ParentsKind::Spans(&[
                    "pi.harness.turn",
                    "pi.harness.checkpoint",
                    "pi.harness.compaction",
                    "pi.harness.navigation",
                ]),
                start_attributes: &[
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name"),
                        ],
                    ),
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Durable operation id"),
                        ],
                    ),
                    (
                        "pi.step.kind",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(&[
                                AttrScalar::Str("assistant"),
                                AttrScalar::Str("compaction"),
                                AttrScalar::Str("branch_summary"),
                            ]),
                            AttrField::Description("Retryable step kind"),
                        ],
                    ),
                    (
                        "pi.step.attempt",
                        &[
                            AttrField::Type("number"),
                            AttrField::Required(true),
                            AttrField::Description("One-based durable attempt number"),
                        ],
                    ),
                    (
                        "pi.compaction.reason",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Values(&[
                                AttrScalar::Str("manual"),
                                AttrScalar::Str("threshold"),
                                AttrScalar::Str("overflow"),
                            ]),
                            AttrField::Description("Compaction trigger"),
                        ],
                    ),
                ],
                end_attributes: &[(
                    "pi.step.outcome",
                    &[
                        AttrField::Type("string"),
                        AttrField::Values(&[
                            AttrScalar::Str("succeeded"),
                            AttrScalar::Str("retry"),
                            AttrScalar::Str("failed"),
                            AttrScalar::Str("aborted"),
                            AttrScalar::Str("deferred"),
                            AttrScalar::Str("overflow"),
                        ]),
                        AttrField::Description("Attempt outcome"),
                    ],
                )],
                status: StatusDefinition {
                    error_when: "The attempt retries, fails, or throws",
                },
            },
        ),
        (
            "pi.harness.tool",
            SpanDefinition {
                description: "One raw phase-2 tool execution",
                parents: ParentsKind::Spans(&["pi.harness.turn", "pi.harness.run"]),
                start_attributes: &[
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name"),
                        ],
                    ),
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Durable operation id"),
                        ],
                    ),
                    (
                        "pi.turn.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Invocation-local live turn id"),
                        ],
                    ),
                    (
                        "pi.tool.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Description("Tool name"),
                        ],
                    ),
                    (
                        "pi.tool.call_id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Tool call id"),
                        ],
                    ),
                    (
                        "pi.tool.replay",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(&[AttrScalar::Str("never"), AttrScalar::Str("safe")]),
                            AttrField::Description("Declared replay policy"),
                        ],
                    ),
                    (
                        "pi.tool.recovery",
                        &[
                            AttrField::Type("boolean"),
                            AttrField::Required(true),
                            AttrField::Description("Whether this is recovery execution"),
                        ],
                    ),
                ],
                end_attributes: &[(
                    "pi.tool.is_error",
                    &[
                        AttrField::Type("boolean"),
                        AttrField::Description("Whether raw phase-2 execution returned an error"),
                    ],
                )],
                status: StatusDefinition {
                    error_when: "Raw phase-2 execution returns an error",
                },
            },
        ),
        (
            "pi.harness.hook",
            SpanDefinition {
                description: "One registered hook handler invocation",
                parents: ParentsKind::Any,
                start_attributes: &[
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name"),
                        ],
                    ),
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Durable operation id when accepted"),
                        ],
                    ),
                    (
                        "pi.hook.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Values(HOOK_VALUES),
                            AttrField::Description("Hook name"),
                        ],
                    ),
                    (
                        "pi.hook.registration_id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Description("Optional hook registration metadata"),
                        ],
                    ),
                ],
                end_attributes: &[(
                    "pi.hook.outcome",
                    &[
                        AttrField::Type("string"),
                        AttrField::Values(&[
                            AttrScalar::Str("completed"),
                            AttrScalar::Str("skipped"),
                            AttrScalar::Str("blocked"),
                            AttrScalar::Str("failed"),
                        ]),
                        AttrField::Description("Handler outcome"),
                    ],
                )],
                status: StatusDefinition {
                    error_when: "The handler throws",
                },
            },
        ),
        (
            "pi.harness.sleep",
            SpanDefinition {
                description: "One retry delay",
                parents: ParentsKind::Spans(&[
                    "pi.harness.run",
                    "pi.harness.compaction",
                    "pi.harness.navigation",
                    "pi.harness.turn",
                    "pi.harness.checkpoint",
                ]),
                start_attributes: &[
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Durable operation id"),
                        ],
                    ),
                    (
                        "pi.sleep.delay_ms",
                        &[
                            AttrField::Type("number"),
                            AttrField::Required(true),
                            AttrField::Description("Requested delay in milliseconds"),
                        ],
                    ),
                ],
                end_attributes: &[(
                    "pi.sleep.outcome",
                    &[
                        AttrField::Type("string"),
                        AttrField::Values(&[
                            AttrScalar::Str("elapsed"),
                            AttrScalar::Str("aborted"),
                        ]),
                        AttrField::Description("Delay outcome"),
                    ],
                )],
                status: StatusDefinition {
                    error_when: "Sleep work throws",
                },
            },
        ),
        (
            "pi.harness.event_handler",
            SpanDefinition {
                description: "One passive event listener invocation",
                parents: ParentsKind::Any,
                start_attributes: &[
                    (
                        "pi.event.type",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("low"),
                            AttrField::Values(EVENT_VALUES),
                            AttrField::Description("Delivered harness event type"),
                        ],
                    ),
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name for lane-scoped events"),
                        ],
                    ),
                ],
                end_attributes: &[],
                status: StatusDefinition {
                    error_when: "The listener throws",
                },
            },
        ),
        (
            "pi.session.write",
            SpanDefinition {
                description: "One committed session transaction",
                parents: ParentsKind::Any,
                start_attributes: &[
                    (
                        "pi.session.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(true),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Session id"),
                        ],
                    ),
                    (
                        "pi.lane.name",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Cardinality("high"),
                            AttrField::Description("Lane name when supplied by the caller"),
                        ],
                    ),
                    (
                        "pi.operation.id",
                        &[
                            AttrField::Type("string"),
                            AttrField::Required(false),
                            AttrField::Cardinality("high"),
                            AttrField::Description(
                                "Durable operation id when supplied by the caller",
                            ),
                        ],
                    ),
                    (
                        "pi.session.item_count",
                        &[
                            AttrField::Type("number"),
                            AttrField::Required(true),
                            AttrField::Description("Number of writes in the transaction"),
                        ],
                    ),
                    (
                        "pi.session.item_kinds",
                        &[
                            AttrField::Type("string[]"),
                            AttrField::Required(true),
                            AttrField::ElementValues(&[
                                AttrScalar::Str("entry"),
                                AttrScalar::Str("usage"),
                                AttrScalar::Str("value"),
                                AttrScalar::Str("list"),
                            ]),
                            AttrField::Description("Distinct write kinds in the transaction"),
                        ],
                    ),
                ],
                end_attributes: &[
                    (
                        "pi.session.first_seq",
                        &[
                            AttrField::Type("number"),
                            AttrField::Description("First committed sequence in the transaction"),
                        ],
                    ),
                    (
                        "pi.session.last_seq",
                        &[
                            AttrField::Type("number"),
                            AttrField::Description("Last committed sequence in the transaction"),
                        ],
                    ),
                ],
                status: StatusDefinition {
                    error_when: "Storage rejects the transaction",
                },
            },
        ),
    ],
};

/// `HOOK_NAMES` viewed as schema value scalars (`telemetry.ts:472`).
static HOOK_VALUES: &[AttrScalar] = &[
    AttrScalar::Str("before_run"),
    AttrScalar::Str("before_drive"),
    AttrScalar::Str("before_run_end"),
    AttrScalar::Str("transform_context"),
    AttrScalar::Str("before_request"),
    AttrScalar::Str("before_payload"),
    AttrScalar::Str("after_response"),
    AttrScalar::Str("before_tool"),
    AttrScalar::Str("after_tool"),
    AttrScalar::Str("before_compaction"),
    AttrScalar::Str("before_navigation"),
];

/// `EVENT_TYPES` viewed as schema value scalars (`telemetry.ts:532`).
static EVENT_VALUES: &[AttrScalar] = &[
    AttrScalar::Str("run_start"),
    AttrScalar::Str("run_resume"),
    AttrScalar::Str("run_suspend"),
    AttrScalar::Str("operation_abort"),
    AttrScalar::Str("run_end"),
    AttrScalar::Str("fault"),
    AttrScalar::Str("handler_error"),
    AttrScalar::Str("turn_start"),
    AttrScalar::Str("turn_end"),
    AttrScalar::Str("retry_scheduled"),
    AttrScalar::Str("retry_start"),
    AttrScalar::Str("retry_end"),
    AttrScalar::Str("message_start"),
    AttrScalar::Str("message_update"),
    AttrScalar::Str("message_end"),
    AttrScalar::Str("tool_start"),
    AttrScalar::Str("tool_update"),
    AttrScalar::Str("tool_end"),
    AttrScalar::Str("entry_added"),
    AttrScalar::Str("queue_update"),
    AttrScalar::Str("value_update"),
    AttrScalar::Str("config_update"),
    AttrScalar::Str("compaction_start"),
    AttrScalar::Str("compaction_end"),
    AttrScalar::Str("navigation_start"),
    AttrScalar::Str("navigation_end"),
    AttrScalar::Str("lane_created"),
    AttrScalar::Str("usage"),
];

/// Upstream `AGENT_TELEMETRY_SCHEMAS` (`telemetry.ts:595`): the combined
/// typed span vocabulary for agent-owned AI-request and harness telemetry.
pub static AGENT_TELEMETRY_SCHEMAS: &[&TelemetrySchemaDefinition] =
    &[&AI_TELEMETRY_SCHEMA, &HARNESS_TELEMETRY_SCHEMA];

/// Upstream `TELEMETRY_CONTEXT_KEY` (`context.ts:27`): the context slot the
/// harness subtree reads the telemetry parent from.
pub fn telemetry_context_key() -> &'static ContextKey<TelemetryContext> {
    static KEY: OnceLock<ContextKey<TelemetryContext>> = OnceLock::new();
    KEY.get_or_init(|| ContextKey::new("pi.telemetryContext"))
}

/// Upstream `getTelemetryContext` (`context.ts:30`): the telemetry parent
/// attached to a context, or the shared no-op parent.
pub fn get_telemetry_context(context: &Context) -> TelemetryContext {
    context
        .get(telemetry_context_key())
        .map(|telemetry| (*telemetry).clone())
        .unwrap_or_else(TelemetryContext::noop)
}

/// Upstream `withTelemetryContext` (`context.ts:35-36`): derive a context
/// whose telemetry children use the supplied parent or active span.
pub fn with_telemetry_context(telemetry: TelemetryContext, context: Context) -> Context {
    with_context_value(telemetry_context_key(), telemetry, context)
}

/// Upstream `startAiSpan` (`telemetry.ts:138-147`): run the callback inside a
/// schema-named span started from the context's telemetry parent; the
/// callback receives the span and a context whose telemetry children parent
/// to it.
pub async fn start_ai_span<T, F>(
    name: &'static str,
    attributes: SpanAttributes,
    callback: F,
    context: Context,
) -> anyhow::Result<T>
where
    F: FnOnce(Arc<dyn TelemetrySpanT>, Context) -> BoxFuture<'static, anyhow::Result<T>>
        + Send
        + 'static,
    T: Send + 'static,
{
    let telemetry = get_telemetry_context(&context);
    telemetry
        .start_span(SpanOptions::new(name, attributes), move |span| {
            let child_context =
                with_telemetry_context(TelemetryContext::from_span(Arc::clone(&span)), context);
            callback(span, child_context)
        })
        .await
}

/// Upstream `startHarnessSpan` (`telemetry.ts:622-635`): the harness-schema
/// twin of [`start_ai_span`].
pub async fn start_harness_span<T, F>(
    name: &'static str,
    attributes: SpanAttributes,
    callback: F,
    context: Context,
) -> anyhow::Result<T>
where
    F: FnOnce(Arc<dyn TelemetrySpanT>, Context) -> BoxFuture<'static, anyhow::Result<T>>
        + Send
        + 'static,
    T: Send + 'static,
{
    let telemetry = get_telemetry_context(&context);
    telemetry
        .start_span(SpanOptions::new(name, attributes), move |span| {
            let child_context =
                with_telemetry_context(TelemetryContext::from_span(Arc::clone(&span)), context);
            callback(span, child_context)
        })
        .await
}

/// Upstream `createTypedSpanStarter` (`telemetry.ts` via pi-telemetry
/// `index.ts:349-354`): bind an explicit parent context to the combined span
/// vocabulary of the agent schemas.
pub fn create_typed_span_starter(telemetry_context: TelemetryContext) -> TypedSpanStarter {
    TypedSpanStarter::new(telemetry_context, AGENT_TELEMETRY_SCHEMAS)
}

#[cfg(test)]
mod tests;
