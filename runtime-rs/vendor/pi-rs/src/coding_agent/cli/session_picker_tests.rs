//! Tests for the ported `coding-agent/src/cli/session-picker.ts` contract
//! (the TUI screen is the disclosed seam; the loader/callback surface is
//! exercised here).

use std::sync::{Arc, Mutex};

use crate::coding_agent::cli::session_picker::{
    select_session, SessionInfo, SessionSelectorCallbacks, SessionSelectorOptions,
    SessionSelectorUi,
};

#[derive(Default)]
struct StubSelectorUi {
    cancelled: Mutex<bool>,
}

impl SessionSelectorUi for StubSelectorUi {
    fn show(
        &self,
        callbacks: SessionSelectorCallbacks,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> {
        let cancelled = *self.cancelled.lock().unwrap();
        Box::pin(async move {
            if cancelled {
                (callbacks.on_cancel)();
                return None;
            }
            (callbacks.on_select)("session-1.jsonl".to_string());
            Some("session-1.jsonl".to_string())
        })
    }
}

fn loaders() -> (
    crate::coding_agent::cli::session_picker::SessionsLoader,
    crate::coding_agent::cli::session_picker::SessionsLoader,
) {
    fn loader(
        sessions: Vec<SessionInfo>,
    ) -> crate::coding_agent::cli::session_picker::SessionsLoader {
        Arc::new(move |_progress| {
            let sessions = sessions.clone();
            Box::pin(async move { sessions.clone() })
        })
    }
    (
        loader(vec![SessionInfo {
            path: "session-1.jsonl".to_string(),
        }]),
        loader(vec![SessionInfo {
            path: "session-1.jsonl".to_string(),
        }]),
    )
}

/// Upstream `selectSession` resolves the selected session path.
#[test]
fn select_session_returns_the_selected_path() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (current, all) = loaders();
    let selected = Arc::new(Mutex::new(None));
    {
        let selected = Arc::clone(&selected);
        let selected_for_assert = Arc::clone(&selected);
        let callbacks = SessionSelectorCallbacks {
            on_select: Box::new(move |path| *selected.lock().unwrap() = Some(path)),
            on_cancel: Box::new(|| panic!("cancel must not fire on select")),
            on_exit: Box::new(|| panic!("exit must not fire on select")),
            request_render: Box::new(|| {}),
        };
        let result = rt.block_on(select_session(
            Arc::new(StubSelectorUi::default()),
            current,
            all,
            Arc::new(settings_manager()),
            callbacks,
            SessionSelectorOptions {
                show_rename_hint: false,
            },
        ));
        assert_eq!(result.as_deref(), Some("session-1.jsonl"));
        assert_eq!(
            selected_for_assert.lock().unwrap().as_deref(),
            Some("session-1.jsonl")
        );
    }
}

/// Upstream `selectSession` resolves null when the selector is cancelled.
#[test]
fn select_session_returns_none_on_cancel() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (current, all) = loaders();
    let cancelled = Arc::new(StubSelectorUi::default());
    *cancelled.cancelled.lock().unwrap() = true;
    let callback_fired = Arc::new(Mutex::new(false));
    {
        let callback_fired_in = Arc::clone(&callback_fired);
        let callback_fired = Arc::clone(&callback_fired);
        let callbacks = SessionSelectorCallbacks {
            on_select: Box::new(|_| panic!("select must not fire on cancel")),
            on_cancel: Box::new(move || *callback_fired_in.lock().unwrap() = true),
            on_exit: Box::new(|| {}),
            request_render: Box::new(|| {}),
        };
        let result = rt.block_on(select_session(
            cancelled,
            current,
            all,
            Arc::new(settings_manager()),
            callbacks,
            SessionSelectorOptions {
                show_rename_hint: false,
            },
        ));
        assert_eq!(result, None);
        assert!(*callback_fired.lock().unwrap());
    }
}

fn settings_manager() -> crate::coding_agent::core::settings_manager::SettingsManager {
    use crate::coding_agent::core::settings_manager::SettingsValue;
    crate::coding_agent::core::settings_manager::SettingsManager::in_memory(SettingsValue::obj(
        vec![],
    ))
}
