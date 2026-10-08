//! release `shared/desktop-pet-state.mjs` + `services/web-desktop-pet-service.mjs`
//! 的运行状态部分：Agent 运行事件 → 宠物状态映射（petStateForAgentEvent /
//! observeRuntimeEvent / publishState），含 stateVersion 递增与
//! resetAfter 定时回落（waving 1400ms / failed 2200ms）。
//!
//! 回落用惰性到期实现（deadline + 代际，在下一次状态读取时应用）：
//! 与 release 的 setTimeout 语义在可观察行为上等价——客户端只能通过
//! 状态轮询看到回落，且新 publishState 会使旧定时器失效（clearTimeout）。

use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use crate::AppState;

const WAVING_RESET_MS: u64 = 1_400;
const FAILED_RESET_MS: u64 = 2_200;

#[derive(Default)]
pub(crate) struct PetState {
    inner: Mutex<PetInner>,
    /// 回落定时器代际：新 publishState 会使旧定时器失效（release clearTimeout）。
    timer_generation: AtomicU64,
}

struct PetInner {
    state: String,
    version: u64,
    sessions: HashSet<String>,
    reset: Option<(Instant, u64)>,
}

impl Default for PetInner {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            version: 0,
            sessions: HashSet::new(),
            reset: None,
        }
    }
}

impl PetState {
    /// release publishState：状态 + 版本递增；resetAfter>0 时安排回落。
    fn publish(&self, next: &str, reset_after_ms: Option<u64>) {
        let mut inner = self.inner.lock().expect("pet state lock");
        inner.state = next.to_string();
        inner.version += 1;
        inner.reset = reset_after_ms.map(|ms| {
            (
                Instant::now() + std::time::Duration::from_millis(ms),
                self.timer_generation.fetch_add(1, Ordering::SeqCst) + 1,
            )
        });
    }

    /// release observeRuntimeEvent：事件 → 状态映射 + 活跃会话集合维护。
    pub(crate) fn observe(&self, event: &str, session_id: &str) {
        match event {
            "error" | "done" => {
                let next = if event == "error" { "failed" } else { "waving" };
                let reset = if event == "error" {
                    FAILED_RESET_MS
                } else {
                    WAVING_RESET_MS
                };
                {
                    let mut inner = self.inner.lock().expect("pet state lock");
                    inner.sessions.remove(session_id);
                }
                self.publish(next, Some(reset));
            }
            other => {
                if let Some(state) = pet_state_for_agent_event(other) {
                    {
                        let mut inner = self.inner.lock().expect("pet state lock");
                        inner.sessions.insert(session_id.to_string());
                    }
                    self.publish(state, None);
                }
            }
        }
    }

    /// release status() 的 state/stateVersion 字段；读取时应用已到期的回落。
    pub(crate) fn fields(&self) -> (String, u64) {
        let mut inner = self.inner.lock().expect("pet state lock");
        if let Some((deadline, generation)) = inner.reset {
            if Instant::now() >= deadline
                && self.timer_generation.load(Ordering::SeqCst) == generation
            {
                inner.state = if inner.sessions.is_empty() {
                    "idle".to_string()
                } else {
                    "waiting".to_string()
                };
                inner.version += 1;
                inner.reset = None;
            }
        }
        (inner.state.clone(), inner.version)
    }
}

/// release petStateForAgentEvent：把运行事件映射为宠物状态，无关事件返回 None。
pub(crate) fn pet_state_for_agent_event(event: &str) -> Option<&'static str> {
    match event {
        "tool_start" | "tool_update" | "tool_end" => Some("running"),
        "thinking_patch" | "thinking_reset" | "compaction_start" => Some("review"),
        "meta" | "text_patch" | "text_delta" | "retry" | "queue_update" => Some("waiting"),
        _ => None,
    }
}

/// chat 事件分发的统一入口（chat_stream 的 listener 与终态 record 处调用）。
pub(crate) fn observe_runtime_event(state: &AppState, event: &str, data: &Value) {
    let session_id = data["sessionId"].as_str().unwrap_or("");
    state.pet.observe(event, session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_release_agent_events_to_pet_states() {
        assert_eq!(pet_state_for_agent_event("tool_start"), Some("running"));
        assert_eq!(pet_state_for_agent_event("tool_end"), Some("running"));
        assert_eq!(pet_state_for_agent_event("thinking_patch"), Some("review"));
        assert_eq!(pet_state_for_agent_event("compaction_start"), Some("review"));
        assert_eq!(pet_state_for_agent_event("text_delta"), Some("waiting"));
        assert_eq!(pet_state_for_agent_event("meta"), Some("waiting"));
        assert_eq!(pet_state_for_agent_event("agent_end"), None);
    }

    #[test]
    fn terminal_events_remove_the_session_and_bump_version() {
        let pet = PetState::default();
        pet.observe("text_delta", "s1");
        pet.observe("tool_start", "s1");
        assert_eq!(pet.fields(), ("running".to_string(), 2));
        pet.observe("done", "s1");
        assert_eq!(pet.fields(), ("waving".to_string(), 3));
        pet.observe("error", "s1");
        assert_eq!(pet.fields(), ("failed".to_string(), 4));
    }

    #[test]
    fn version_increments_on_every_publish() {
        let pet = PetState::default();
        pet.observe("meta", "s1");
        pet.observe("text_delta", "s1");
        pet.observe("text_delta", "s1");
        assert_eq!(pet.fields().1, 3);
    }

    #[test]
    fn reset_falls_back_to_idle_when_no_sessions_remain() {
        let pet = PetState::default();
        pet.observe("text_delta", "s1");
        pet.observe("done", "s1");
        assert_eq!(pet.fields(), ("waving".to_string(), 2));
        // 惰性回落：到期后读取即回落（waving 1400ms）。
        std::thread::sleep(std::time::Duration::from_millis(1_450));
        assert_eq!(pet.fields(), ("idle".to_string(), 3));
    }

    #[test]
    fn reset_falls_back_to_waiting_when_sessions_remain() {
        let pet = PetState::default();
        pet.observe("text_delta", "s1");
        pet.observe("text_delta", "s2");
        pet.observe("done", "s1");
        assert_eq!(pet.fields(), ("waving".to_string(), 3));
        std::thread::sleep(std::time::Duration::from_millis(1_450));
        assert_eq!(pet.fields(), ("waiting".to_string(), 4));
    }

    #[test]
    fn newer_publish_cancels_the_previous_reset() {
        let pet = PetState::default();
        pet.observe("text_delta", "s1");
        pet.observe("done", "s1"); // waving + 1400ms 回落
        pet.observe("error", "s1"); // failed + 2200ms 回落（取代旧定时器）
        assert_eq!(pet.fields(), ("failed".to_string(), 3));
        std::thread::sleep(std::time::Duration::from_millis(1_450));
        // 旧 waving 回落（1400ms）已被取消；2200ms 未到，状态保持 failed。
        assert_eq!(pet.fields(), ("failed".to_string(), 3));
    }
}
