//! Web 终端适配：每条同源连接拥有一个 PTY，断开即回收；不持久化 Shell 或输出。
use std::{sync::Arc, time::Duration};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures::SinkExt;
use pisper_terminal::{DesktopTerminalState, TerminalCreateInput, TerminalEvent};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, Semaphore};

use crate::{ApiError, AppState};

static CONNECTIONS: Semaphore = Semaphore::const_new(12);

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/terminal/profiles", get(profiles))
        .route("/api/terminal/connect", get(connect))
}

fn available(state: &AppState) -> Result<(), ApiError> {
    if state.runtime_profile != "desktop" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "terminal_unavailable",
            "此运行时不提供终端。",
        ));
    }
    Ok(())
}

async fn profiles(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    available(&state)?;
    Ok(Json(json!(
        pisper_terminal::profiles().map_err(ApiError::internal)?
    )))
}

async fn connect(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(address): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    available(&state)?;
    // WebSocket 不受浏览器 CORS 保护；无令牌的本机 Web 开发服务也必须拒绝跨站升级。
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let host_name = host.parse::<axum::http::uri::Authority>().ok();
    let loopback = host_name.as_ref().is_some_and(|authority| {
        let name = authority.host().trim_matches(['[', ']']);
        name == "localhost"
            || name
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if state.desktop_token.is_none() && (!loopback || !address.ip().is_loopback()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "terminal_auth_required",
            "终端需要本机连接或已认证的桌面连接。",
        ));
    }
    if host.is_empty()
        || (origin != format!("http://{host}") && origin != format!("https://{host}"))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "终端只接受同源连接。",
        ));
    }
    let permit = CONNECTIONS.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "terminal_limit",
            "终端数量已达到上限。",
        )
    })?;
    Ok(upgrade
        .max_message_size(300 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            serve(socket, state).await;
        })
        .into_response())
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Command {
    Create {
        terminal_id: String,
        profile_id: String,
        session_id: String,
        cols: u16,
        rows: u16,
    },
    Input {
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Close,
}

async fn send(socket: &mut WebSocket, value: serde_json::Value) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(value.to_string().into())),
    )
    .await
    .map_err(|_| "终端连接超时。".to_string())?
    .map_err(|_| "终端连接已断开。".to_string())
}

async fn serve(mut socket: WebSocket, state: Arc<AppState>) {
    let registry = DesktopTerminalState::default();
    let result = run(&mut socket, &state, &registry).await;
    // 先关闭接收方再等待 OS 子进程，避免离开的页面阻塞 ConPTY 关闭握手。
    if let Err(message) = result {
        let _ = send(&mut socket, json!({"type":"error","code":"terminal_failed","message":crate::security::redact_secret_text(&message)})).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
    let _ = tokio::task::spawn_blocking(move || pisper_terminal::shutdown(&registry)).await;
}

async fn run(
    socket: &mut WebSocket,
    state: &AppState,
    registry: &DesktopTerminalState,
) -> Result<(), String> {
    let first = tokio::select! {
        _ = state.shutdown.cancelled() => return Ok(()),
        value = tokio::time::timeout(Duration::from_secs(10), socket.recv()) => value.map_err(|_| "终端启动超时。")?,
    };
    let Some(Ok(Message::Text(text))) = first else {
        return Ok(());
    };
    let command: Command = serde_json::from_str(&text).map_err(|_| "终端请求格式无效。")?;
    let Command::Create {
        terminal_id,
        profile_id,
        session_id,
        cols,
        rows,
    } = command
    else {
        return Err("终端必须先创建。".into());
    };
    // 目录由既有会话所有者解析，客户端不能把任意 Shell 命令或目录作为启动参数。
    if session_id.is_empty() {
        return Err("请选择会话后打开终端。".into());
    }
    let cwd =
        crate::session_api::session_cwd(state, &session_id).map_err(|_| "会话工作目录不可用。")?;
    let (tx, mut rx) = mpsc::channel::<TerminalEvent>(64);
    let create_state = registry.clone();
    let input = TerminalCreateInput {
        terminal_id: terminal_id.clone(),
        profile_id,
        cwd,
        cols,
        rows,
    };
    let created = tokio::task::spawn_blocking(move || {
        pisper_terminal::create_terminal(
            &create_state,
            input,
            Arc::new(move |event| {
                tx.try_send(event)
                    .map_err(|_| "终端接收方已断开或输出过载。".into())
            }),
        )
    })
    .await
    .map_err(|_| "终端启动任务失败。")??;
    send(socket, json!({"type":"created","terminal":created})).await?;
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            event = rx.recv() => {
                let Some(event) = event else { break; };
                let finished = matches!(event, TerminalEvent::Exit {..} | TerminalEvent::Error {..});
                send(socket, json!(event)).await?;
                if finished { break; }
            },
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break; };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => continue,
                    _ => return Err("终端只接受 JSON 消息。".into()),
                };
                let command: Command = serde_json::from_str(&text).map_err(|_| "终端请求格式无效。")?;
                let registry = registry.clone();
                let id = terminal_id.clone();
                match command {
                    Command::Close => break,
                    Command::Create {..} => return Err("终端已经创建。".into()),
                    Command::Input {data} => {
                        tokio::task::spawn_blocking(move || pisper_terminal::write_terminal(&registry, &id, &data))
                            .await.map_err(|_| "终端输入任务失败。")??;
                    },
                    Command::Resize {cols, rows} => {
                        tokio::task::spawn_blocking(move || pisper_terminal::resize_terminal(&registry, &id, cols, rows))
                            .await.map_err(|_| "终端缩放任务失败。")??;
                    },
                }
            },
        }
    }
    Ok(())
}
