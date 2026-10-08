//! Native, type-preserving tools at the non-generic [`super::Lane`] boundary.
//!
//! `AgentHarness<TContext>` owns typed tools and a typed context source. Lane
//! is deliberately non-generic (durable state, watch and navigation do not
//! depend on an application's context). This adapter erases only the Rust
//! *type* at that boundary: it never serializes contexts, callbacks or handles.
//! The original execute/prepare/update/invocation functions remain live.
//!
//! Call [`NativeRuntimeTools::new`] when projecting a configuration snapshot,
//! not when accepting a run. Resolve the returned context source once per tool
//! batch, at the same boundary as upstream `runTools` (after ready results and
//! cancellation handling). Constructing or cloning this adapter must not call
//! a context provider.

use std::any::{type_name, Any};
use std::sync::Arc;

use crate::agent_core::harness::runtime::drive::tools::ToolContextSource;
use crate::agent_core::harness::types::AgentHarnessTool;
use crate::ai::types::tool::Tool;

/// An owned native context. Cloning retains the original object, including
/// any `Arc` identities and closures stored in it; no JSON round-trip occurs.
#[derive(Clone)]
pub struct NativeToolContext {
    value: Arc<dyn Any + Send + Sync>,
    type_name: &'static str,
}

impl NativeToolContext {
    fn new<T: Send + Sync + 'static>(value: T) -> Self {
        Self {
            value: Arc::new(value),
            type_name: type_name::<T>(),
        }
    }

    fn typed<T: Clone + Send + Sync + 'static>(&self) -> anyhow::Result<T> {
        self.value.downcast_ref::<T>().cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "Tool context type mismatch: expected {}, got {}. Supply a matching native tool context; an omitted context is ().",
                type_name::<T>(),
                self.type_name,
            )
        })
    }
}

/// One native tool configuration snapshot. The declarations and executors are
/// derived from the same typed tools so native functions are never replaced
/// by the LLM-facing `Tool` declaration.
#[derive(Clone)]
pub struct NativeRuntimeTools {
    tools: Vec<AgentHarnessTool<NativeToolContext>>,
    context: ToolContextSource<NativeToolContext>,
}

impl Default for NativeRuntimeTools {
    fn default() -> Self {
        Self::new(Vec::<AgentHarnessTool<()>>::new(), None)
    }
}

impl NativeRuntimeTools {
    /// Erase a typed snapshot without executing its context provider. `None`
    /// means the harness's unit/undefined context; it does not manufacture a
    /// default value of an arbitrary application type. Tools that require a
    /// non-unit context must be configured with that value or a provider.
    pub fn new<T: Clone + Send + Sync + 'static>(
        tools: Vec<AgentHarnessTool<T>>,
        context: Option<ToolContextSource<T>>,
    ) -> Self {
        let context = match context {
            None => ToolContextSource::Value(NativeToolContext::new(())),
            Some(ToolContextSource::Value(value)) => {
                ToolContextSource::Value(NativeToolContext::new(value))
            }
            Some(ToolContextSource::Provider(provider)) => {
                ToolContextSource::Provider(Arc::new(move |context| {
                    let provider = Arc::clone(&provider);
                    Box::pin(async move { NativeToolContext::new(provider(context).await) })
                }))
            }
            Some(ToolContextSource::FallibleProvider(provider)) => {
                ToolContextSource::FallibleProvider(Arc::new(move |context| {
                    let provider = Arc::clone(&provider);
                    Box::pin(async move { provider(context).await.map(NativeToolContext::new) })
                }))
            }
        };
        let tools = tools
            .into_iter()
            .map(|tool| {
                let execute = tool.execute;
                AgentHarnessTool {
                    name: tool.name,
                    label: tool.label,
                    description: tool.description,
                    parameters: tool.parameters,
                    constrained_sampling: tool.constrained_sampling,
                    execute: Arc::new(
                        move |id,
                              arguments,
                              update,
                              context: NativeToolContext,
                              invocation,
                              caller| {
                            match context.typed::<T>() {
                                Ok(context) => {
                                    execute(id, arguments, update, context, invocation, caller)
                                }
                                Err(error) => Box::pin(async move { Err(error) }),
                            }
                        },
                    ),
                    prepare_arguments: tool.prepare_arguments,
                    replay: tool.replay,
                    execution_mode: tool.execution_mode,
                }
            })
            .collect();
        Self { tools, context }
    }

    /// LLM declarations for generation, from the same snapshot as executors.
    pub fn declarations(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(AgentHarnessTool::declaration)
            .collect()
    }

    /// Runtime-only parts. The context must be resolved lazily by the tool
    /// phase, never cached across tool batches or across installed passes.
    pub fn into_parts(
        self,
    ) -> (
        Vec<AgentHarnessTool<NativeToolContext>>,
        ToolContextSource<NativeToolContext>,
    ) {
        (self.tools, self.context)
    }
}

#[cfg(test)]
#[path = "native_tools_tests.rs"]
mod tests;
