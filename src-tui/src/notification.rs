//! 桌面通知的构造与展示。
//!
//! 通知内容统一由 Runtime 分发（`/api/settings/notifications/*`），
//! 只有用户开启系统通知时才在本地弹系统通知；本模块负责把 App 状态
//! 整理成可发送的通知结构，并限制摘要长度避免弹窗过大。

use notify_rust::Notification;

use crate::app::{App, Approval};

// 通知摘要的最大字符数：终端会话标题可能很长，过长的摘要既难读也会被系统截断。
const MAX_NOTIFICATION_SUMMARY_CHARS: usize = 320;

/// 「对话完成」通知的载荷：标题 + 摘要 + 模型名。
#[derive(Debug, Eq, PartialEq)]
pub struct ChatCompletion {
    pub title: String,
    pub summary: String,
    pub model: String,
}

/// 「等待审批」通知的载荷：标题 + 工具名 + 审批原因 + 模型名。
#[derive(Debug, Eq, PartialEq)]
pub struct ChatWaiting {
    pub title: String,
    pub tool: String,
    pub reason: String,
    pub model: String,
}

/// 通知标题：未命名会话使用固定文案，避免每个新会话都弹「New conversation」。
fn chat_title(app: &App) -> &str {
    match app.session.name.trim() {
        "" | "New conversation" => "Pisper conversation",
        name => name,
    }
}

/// 组装「对话完成」通知：优先取正在流式输出的文本作为摘要，
/// 没有输出时用通用文案兜底；多行文本折叠为单行并截断到上限。
pub fn chat_completion(app: &App) -> ChatCompletion {
    let response = app
        .live
        .as_ref()
        .map(|live| live.text_target.trim())
        .filter(|text| !text.is_empty())
        .unwrap_or("The Agent has finished responding.");
    let summary = response.split_whitespace().collect::<Vec<_>>().join(" ");

    ChatCompletion {
        title: chat_title(app).to_owned(),
        summary: summary
            .chars()
            .take(MAX_NOTIFICATION_SUMMARY_CHARS)
            .collect(),
        model: app.model.clone(),
    }
}

/// 组装「等待审批」通知：携带待审批的工具名与原因，
/// 便于用户在终端之外也能了解当前被拦截的操作。
pub fn chat_waiting(app: &App, approval: &Approval) -> ChatWaiting {
    ChatWaiting {
        title: chat_title(app).to_owned(),
        tool: approval.tool_name.clone(),
        reason: approval.reason.trim().to_owned(),
        model: app.model.clone(),
    }
}

/// 直接弹出一条系统通知。
/// 返回错误被忽略：通知失败不应中断主流程（终端里已有同等信息）。
pub fn show_system(title: &str, body: &str) {
    let _ = Notification::new()
        .appname("Pisper")
        .summary(title)
        .body(body)
        .show();
}

#[cfg(test)]
mod tests {
    use super::{chat_completion, chat_waiting};
    use crate::{
        app::{App, Approval, LiveTurn},
        model::SessionSummary,
    };

    fn test_app() -> App {
        let session = SessionSummary {
            id: "session-1".to_owned(),
            name: "Resize audit".to_owned(),
            model: "provider/model".to_owned(),
            cwd: "/workspace".to_owned(),
            execution_mode: "full-access".to_owned(),
            ..SessionSummary::default()
        };
        App::new(
            vec![session.clone()],
            session,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        )
    }

    #[test]
    fn completion_notifications_use_the_session_title_and_final_response() {
        let mut app = test_app();
        app.live = Some(LiveTurn {
            text_target: "The resize fix is complete.\nAll tests passed.".to_owned(),
            ..LiveTurn::default()
        });

        assert_eq!(
            chat_completion(&app),
            super::ChatCompletion {
                title: "Resize audit".to_owned(),
                summary: "The resize fix is complete. All tests passed.".to_owned(),
                model: "provider/model".to_owned(),
            }
        );
    }

    #[test]
    fn waiting_notifications_describe_the_pending_approval() {
        let app = test_app();
        let approval = Approval {
            id: "approval-1".to_owned(),
            tool_name: "bash".to_owned(),
            args: serde_json::json!({ "command": "npm test" }),
            risk: "high".to_owned(),
            reason: "Runs a command outside the workspace.".to_owned(),
        };

        assert_eq!(
            chat_waiting(&app, &approval),
            super::ChatWaiting {
                title: "Resize audit".to_owned(),
                tool: "bash".to_owned(),
                reason: "Runs a command outside the workspace.".to_owned(),
                model: "provider/model".to_owned(),
            }
        );
    }
}
