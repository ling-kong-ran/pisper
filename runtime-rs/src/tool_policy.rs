//! Rust 运行时的工具准入在 Pi 的执行前 hook 内实施，不能只更新 UI 元数据。
use crate::{
    approval_api::{ApprovalService, AuthorizationRequest, CancelFuture},
    execution_modes, SessionMeta,
};
use pi_rust::{
    agent_core::{
        agent::Agent,
        agent_loop::{BeforeToolCallHook, BeforeToolCallOutcome},
    },
    coding_agent::agent_session::AgentSession,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};

type SharedMetadata = Arc<Mutex<HashMap<String, SessionMeta>>>;

fn policy_hook(
    previous: Option<Arc<BeforeToolCallHook>>,
    session_id: String,
    metadata: SharedMetadata,
    approvals: Arc<ApprovalService>,
    agent: Weak<Agent>,
    cwd: String,
    owned_files: Vec<String>,
) -> Arc<BeforeToolCallHook> {
    Arc::new(move |context| {
        let previous = previous.clone();
        let session_id = session_id.clone();
        let metadata = metadata.clone();
        let approvals = approvals.clone();
        let agent = agent.clone();
        let cwd = cwd.clone();
        let owned_files = owned_files.clone();
        Box::pin(async move {
            let mut outcome = match previous {
                Some(previous) => previous(context.clone()).await,
                None => BeforeToolCallOutcome::default(),
            };
            if outcome
                .result
                .as_ref()
                .is_some_and(|result| result.block == Some(true))
            {
                return outcome;
            }
            // 只在同步读取期间持锁，不让模型 hook / 工具执行等待 metadata 锁。
            let meta = metadata
                .lock()
                .ok()
                .and_then(|metadata| metadata.get(&session_id).cloned())
                .unwrap_or_default();
            let configured_mode = meta
                .execution_mode
                .unwrap_or_else(|| execution_modes::DEFAULT_EXECUTION_MODE.into());
            let mode = execution_modes::normalize(&configured_mode)
                .unwrap_or(execution_modes::DEFAULT_EXECUTION_MODE)
                .to_owned();
            let permission = meta
                .permission_mode
                .as_deref()
                .and_then(|permission| match permission {
                    // Earlier Rust builds persisted the full-access permission as "full".
                    "full" => Some("ignore"),
                    "ask" | "auto" | "ignore" => Some(permission),
                    _ => None,
                })
                .unwrap_or_else(|| execution_modes::permission_mode(&mode))
                .to_string();
            // The Pi hook context omits a signal, but Agent exposes its actual active-run token.
            let cancel = agent
                .upgrade()
                .and_then(|agent| agent.signal())
                .map(|token| Box::pin(async move { token.cancelled().await }) as CancelFuture);
            outcome.result = approvals
                .authorize(
                    AuthorizationRequest {
                        session_id,
                        cwd,
                        tool_name: context.tool_call.name,
                        tool_call_id: context.tool_call.id,
                        args: outcome.args.clone().unwrap_or(context.args),
                        permission_mode: permission,
                        execution_mode: mode,
                        owned_files,
                    },
                    cancel,
                )
                .await;
            outcome
        })
    })
}

pub(crate) fn install(
    session: Arc<AgentSession>,
    metadata: SharedMetadata,
    approvals: Arc<ApprovalService>,
) {
    install_scoped(session, metadata, approvals, vec![]);
}
pub(crate) fn install_scoped(
    session: Arc<AgentSession>,
    metadata: SharedMetadata,
    approvals: Arc<ApprovalService>,
    owned_files: Vec<String>,
) {
    let session_id = session.session_id();
    install_context(session, metadata, approvals, session_id, owned_files);
}

/// 子 Agent 的审批归属父会话，权限读取父元数据；实际取消 token 仍来自子 Agent。
pub(crate) fn install_context(
    session: Arc<AgentSession>,
    metadata: SharedMetadata,
    approvals: Arc<ApprovalService>,
    session_id: String,
    owned_files: Vec<String>,
) {
    // ensure_hosted 常常重复调用；同一 Agent 只包装一次，避免重复链式 hook。
    type InstalledPolicies = Vec<(Weak<Agent>, Weak<BeforeToolCallHook>)>;
    static INSTALLED: OnceLock<Mutex<InstalledPolicies>> = OnceLock::new();
    let mut installed = INSTALLED
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("tool policy registry");
    installed.retain(|(agent, _)| agent.strong_count() > 0);
    let agent = Arc::downgrade(&session.agent);
    let mut runtime = session.agent.runtime();
    // Pi reload 会为同一 Agent 重装自己的 hook。仅当前回调仍是本模块包装时跳过，
    // 否则重新包装新的 Pi 回调，既不丢失权限策略也不增加重复链。
    if installed.iter().any(|(existing, policy)| {
        existing.ptr_eq(&agent)
            && policy
                .upgrade()
                .zip(runtime.before_tool_call.as_ref())
                .is_some_and(|(policy, current)| Arc::ptr_eq(&policy, current))
    }) {
        return;
    }
    let previous = runtime.before_tool_call.clone();
    let cwd = session
        .session_manager
        .lock()
        .expect("session manager")
        .get_cwd()
        .to_string();
    let policy = policy_hook(
        previous,
        session_id.clone(),
        metadata,
        approvals.clone(),
        agent.clone(),
        cwd,
        owned_files,
    );
    runtime.before_tool_call = Some(policy.clone());
    let previous_after = runtime.after_tool_call.clone();
    runtime.after_tool_call = Some(Arc::new(move |context| {
        let previous = previous_after.clone();
        let approvals = approvals.clone();
        let session_id = session_id.clone();
        Box::pin(async move {
            if let Ok(result) = serde_json::to_value(&context.result) {
                approvals.observe_computer_use_result(
                    &session_id,
                    &context.tool_call.name,
                    &result,
                );
            }
            if let Some(previous) = previous {
                previous(context).await
            } else {
                None
            }
        })
    }));
    installed.retain(|(existing, _)| !existing.ptr_eq(&agent));
    installed.push((agent, Arc::downgrade(&policy)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_rust::{
        agent_core::{
            agent_loop::{agent_loop, AgentLoopConfig, BeforeToolCallContext},
            types::{AgentMessage, AgentTool, AgentToolResult, BeforeToolCallResult},
            AgentContext,
        },
        ai::{
            models::{
                create_models, faux_assistant_message, faux_provider, faux_tool_call,
                CreateModelsOptions, FauxMessageOptions, FauxProviderOptions, FauxToolCallOptions,
            },
            types::{
                message::{StringOrBlocks, UserMessage},
                primitives::StopReason,
            },
        },
    };
    use serde_json::json;

    fn metadata(mode: &str) -> SharedMetadata {
        Arc::new(Mutex::new(HashMap::from([(
            "test-session".to_string(),
            SessionMeta {
                execution_mode: Some(mode.to_string()),
                ..Default::default()
            },
        )])))
    }

    fn test_service() -> Arc<ApprovalService> {
        ApprovalService::with_timeout(
            std::env::temp_dir().join(format!("pisper-hook-{}.json", crate::product::new_id())),
            std::time::Duration::from_millis(30),
        )
        .unwrap()
    }
    fn hook(
        previous: Option<Arc<BeforeToolCallHook>>,
        id: String,
        metadata: SharedMetadata,
    ) -> Arc<BeforeToolCallHook> {
        policy_hook(
            previous,
            id,
            metadata,
            test_service(),
            Weak::new(),
            std::env::temp_dir().to_string_lossy().into(),
            vec![],
        )
    }

    fn context(name: &str) -> BeforeToolCallContext {
        let block = faux_tool_call(
            name,
            json!({}),
            FauxToolCallOptions {
                id: Some("test-call".to_string()),
            },
        );
        let pi_rust::ai::types::message::AssistantBlock::ToolCall(tool_call) = block.clone() else {
            panic!("tool call")
        };
        BeforeToolCallContext {
            assistant_message: faux_assistant_message(block, FauxMessageOptions::default()),
            tool_call,
            args: json!({}),
            context: AgentContext::default(),
        }
    }

    #[tokio::test]
    async fn metadata_changes_are_applied_without_reinstalling_hooks() {
        let meta = metadata("approval-required");
        let hook = hook(None, "test-session".to_string(), meta.clone());
        assert!(hook(context("read")).await.result.is_none());
        assert_eq!(
            hook(context("write")).await.result.unwrap().block,
            Some(true)
        );
        assert_eq!(
            hook(context("mcp__untrusted__read"))
                .await
                .result
                .unwrap()
                .block,
            Some(true)
        );
        meta.lock()
            .unwrap()
            .get_mut("test-session")
            .unwrap()
            .execution_mode = Some("full-access".to_string());
        assert!(hook(context("write")).await.result.is_none());
        meta.lock()
            .unwrap()
            .get_mut("test-session")
            .unwrap()
            .execution_mode = Some("workspace-write".to_string());
        assert!(hook(context("read")).await.result.is_none());
        assert!(hook(context("write")).await.result.is_none());
    }

    #[tokio::test]
    async fn prior_hook_decisions_and_argument_mutations_are_preserved() {
        let previous: Arc<BeforeToolCallHook> = Arc::new(|_| {
            Box::pin(async {
                BeforeToolCallOutcome {
                    args: Some(json!({"preserved":true})),
                    result: None,
                }
            })
        });
        let hook = hook(
            Some(previous),
            "test-session".to_string(),
            metadata("approval-required"),
        );
        let outcome = hook(context("read")).await;
        assert_eq!(outcome.args.unwrap()["preserved"], true);
        let previous: Arc<BeforeToolCallHook> = Arc::new(|_| {
            Box::pin(async {
                BeforeToolCallOutcome {
                    args: None,
                    result: Some(BeforeToolCallResult {
                        block: Some(true),
                        reason: Some("previous rejection".to_string()),
                        ..Default::default()
                    }),
                }
            })
        });
        let hook = self::hook(
            Some(previous),
            "test-session".to_string(),
            metadata("full-access"),
        );
        assert_eq!(
            hook(context("write"))
                .await
                .result
                .unwrap()
                .reason
                .as_deref(),
            Some("previous rejection")
        );
    }

    #[tokio::test]
    async fn unknown_persisted_modes_and_permissions_keep_approval_required() {
        let meta = metadata("unknown-old-mode");
        meta.lock()
            .unwrap()
            .get_mut("test-session")
            .unwrap()
            .permission_mode = Some("invalid-old-permission".into());
        let outcome = hook(None, "test-session".into(), meta)(context("write")).await;
        assert_eq!(outcome.result.unwrap().block, Some(true));
        assert_eq!(
            actual_tool_executions("unknown-old-mode", "write", false).await,
            0
        );
    }

    async fn actual_tool_executions(mode: &str, name: &str, approve: bool) -> usize {
        let faux = faux_provider(FauxProviderOptions::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(faux.provider.clone());
        let model = faux.get_model(None).unwrap();
        faux.set_responses(vec![
            faux_assistant_message(
                faux_tool_call(
                    name,
                    json!({}),
                    FauxToolCallOptions {
                        id: Some("test-tool".to_string()),
                    },
                ),
                FauxMessageOptions {
                    stop_reason: Some(StopReason::ToolUse),
                    ..Default::default()
                },
            )
            .into(),
            faux_assistant_message("done", FauxMessageOptions::default()).into(),
        ]);
        let executed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = executed.clone();
        let tool = AgentTool {
            name: name.to_string(),
            label: name.to_string(),
            description: "isolated policy test".to_string(),
            parameters: json!({"type":"object","properties":{}}),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
            execute: Arc::new(move |_, _, _, _| {
                let counter = counter.clone();
                Box::pin(async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(AgentToolResult::default())
                })
            }),
        };
        let mut config = AgentLoopConfig::new(
            model,
            Arc::new(|messages| {
                Box::pin(async move {
                    messages
                        .iter()
                        .filter_map(|message| message.to_message())
                        .collect()
                })
            }),
        );
        let service = test_service();
        let weak = Arc::downgrade(&service);
        let observed = executed.clone();
        let subscription = service.subscribe(
            Some("test-session"),
            Arc::new(move |_, event, data| {
                if event == "permission_request" {
                    assert_eq!(
                        observed.load(std::sync::atomic::Ordering::SeqCst),
                        0,
                        "Approval must precede actual tool execution"
                    );
                    if let Some(service) = weak.upgrade() {
                        service.resolve("test-session", data["id"].as_str().unwrap(), approve);
                    }
                }
            }),
        );
        config.before_tool_call = Some(policy_hook(
            None,
            "test-session".to_string(),
            metadata(mode),
            service.clone(),
            Weak::new(),
            std::env::temp_dir().to_string_lossy().into(),
            vec![],
        ));
        let (_events, task) = agent_loop(
            vec![AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text("test tools".to_string()),
                timestamp: 1,
            })],
            AgentContext {
                messages: vec![],
                tools: vec![Arc::new(tool)],
            },
            config,
            Arc::new(models),
            None,
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(subscription);
        executed.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn pi_engine_really_blocks_write_and_allows_read_or_explicit_full_access() {
        assert_eq!(
            actual_tool_executions("approval-required", "write", false).await,
            0
        );
        assert_eq!(
            actual_tool_executions("approval-required", "bash", false).await,
            0
        );
        assert_eq!(
            actual_tool_executions("approval-required", "bash", true).await,
            1
        );
        assert_eq!(
            actual_tool_executions("approval-required", "read", false).await,
            1
        );
        assert_eq!(
            actual_tool_executions("full-access", "write", false).await,
            1
        );
        assert_eq!(
            actual_tool_executions("workspace-write", "write", false).await,
            1
        );
    }
}
