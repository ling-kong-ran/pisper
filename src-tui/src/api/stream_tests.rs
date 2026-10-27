//! 用真实 TCP/SSE 验证独立终端的断流恢复；不调用模型或读取用户配置。

use std::{
    future::Future,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Barrier, Notify},
    task::{JoinHandle, JoinSet},
};

use super::{ApiClient, ChatRequest};
use crate::{
    app::{App, LiveTurn},
    model::{PromptMode, RuntimeEvent, SessionSummary},
};

const TEST_IDLE_TIMEOUT: Duration = Duration::from_millis(500);
const TEST_DEADLINE: Duration = Duration::from_secs(5);
const PROCESS_TEST: &str =
    "api::stream_tests::independent_terminal_processes_recover_without_interrupting_their_sibling";

struct Server {
    url: String,
    task: JoinHandle<()>,
}

impl Server {
    async fn start<F, Fut>(handler: F) -> Result<Self>
    where
        F: Fn(String, TcpStream) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move {
            let handler = Arc::new(handler);
            let mut requests = JoinSet::new();
            loop {
                tokio::select! {
                    incoming = listener.accept() => {
                        let (mut socket, _) = incoming.expect("fixture accept failed");
                        let handler = Arc::clone(&handler);
                        requests.spawn(async move {
                            let request = read_request(&mut socket).await?;
                            handler(request, socket).await
                        });
                    }
                    completed = requests.join_next(), if !requests.is_empty() => {
                        completed.expect("fixture request missing")
                            .expect("fixture request panicked")
                            .expect("fixture request failed");
                    }
                }
            }
        });
        Ok(Self { url, task })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // 取消监听任务会连带取消它拥有的所有连接，卡住的夹具不会遗留后台请求。
        self.task.abort();
    }
}

async fn read_request(socket: &mut TcpStream) -> Result<String> {
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = socket.read(&mut buffer).await?;
        anyhow::ensure!(count > 0, "fixture request ended early");
        request.extend_from_slice(&buffer[..count]);
        anyhow::ensure!(request.len() <= 64 * 1024, "fixture request too large");
        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..end])?;
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                return String::from_utf8(request).context("fixture request is not UTF-8");
            }
        }
    }
}

async fn start_stream(socket: &mut TcpStream, frames: &str) -> Result<()> {
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await?;
    socket.write_all(frames.as_bytes()).await?;
    Ok(())
}

fn request(session: &str) -> Result<ChatRequest> {
    Ok(ChatRequest {
        session_id: session.to_owned(),
        workspace: std::env::current_dir()?,
        message: "isolated stream fixture".to_owned(),
        requested_tool: None,
        attachment_paths: Vec::new(),
        prompt_mode: PromptMode::Plan,
        goal_token_budget: None,
    })
}

fn thinking_app(session_id: &str) -> App {
    let session = SessionSummary {
        id: session_id.to_owned(),
        ..SessionSummary::default()
    };
    let mut app = App::new(
        vec![session.clone()],
        session,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
    );
    app.live = Some(LiveTurn {
        streaming: true,
        ..LiveTurn::default()
    });
    app.status = "thinking".to_owned();
    app
}

#[tokio::test]
async fn silent_connection_resumes_the_original_run_without_resubmitting() -> Result<()> {
    let submissions = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&submissions);
    let server = Server::start(move |request, mut socket| {
        let count = Arc::clone(&count);
        async move {
            if request.starts_with("POST /api/chat ") {
                count.fetch_add(1, Ordering::SeqCst);
                start_stream(&mut socket, "event: run\ndata: {\"runId\":\"original\"}\n\nid: 1\nevent: thinking_patch\ndata: {\"start\":0,\"text\":\"partial\"}\n\n").await?;
                std::future::pending::<()>().await;
            } else {
                anyhow::ensure!(request.starts_with("GET /api/runs/original/events?after=1 "), "wrong resume cursor");
                start_stream(&mut socket, "id: 2\nevent: done\ndata: {\"text\":\"recovered\"}\n\n").await?;
            }
            Ok(())
        }
    }).await?;
    let api = ApiClient::with_stream_idle_timeout(&server.url, "fixture", TEST_IDLE_TIMEOUT)?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    tokio::time::timeout(TEST_DEADLINE, api.stream_chat(request("a")?, sender)).await??;
    assert_eq!(submissions.load(Ordering::SeqCst), 1);
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(event, RuntimeEvent::StreamReconnecting { session_id, attempt: 1, .. } if session_id == "a")));
    assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Stream(frame) if frame.name == "done" && frame.data["text"] == "recovered")));
    Ok(())
}

#[tokio::test]
async fn an_expired_run_stops_thinking_and_preserves_partial_output() -> Result<()> {
    let server = Server::start(|request, mut socket| async move {
        if request.starts_with("POST /api/chat ") {
            start_stream(&mut socket, "event: run\ndata: {\"runId\":\"expired\"}\n\nid: 1\nevent: thinking_patch\ndata: {\"start\":0,\"text\":\"partial reasoning\"}\n\n").await?;
            std::future::pending::<()>().await;
        } else {
            anyhow::ensure!(request.starts_with("GET /api/runs/expired/events?after=1 "), "wrong resume cursor");
            socket.write_all(b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        }
        Ok(())
    }).await?;
    let api = ApiClient::with_stream_idle_timeout(&server.url, "fixture", TEST_IDLE_TIMEOUT)?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let error = tokio::time::timeout(TEST_DEADLINE, api.stream_chat(request("a")?, sender))
        .await?
        .expect_err("expired run must fail");
    let mut app = thinking_app("a");
    while let Ok(event) = receiver.try_recv() {
        match event {
            RuntimeEvent::Stream(frame) => app.apply_stream_event(frame),
            RuntimeEvent::StreamReconnecting {
                session_id,
                attempt,
                max_attempts,
            } => {
                app.stream_reconnecting(&session_id, attempt, max_attempts);
                assert!(app.status.starts_with("reconnecting stream"));
            }
            _ => {}
        }
    }
    app.stream_failed(format!("{error:#}"));
    assert!(!app.is_running_state());
    assert!(app.status_error);
    assert!(app.status.contains("no longer resumable"));
    assert_eq!(
        app.live.as_ref().map(|live| live.thinking_target.as_str()),
        Some("partial reasoning")
    );
    Ok(())
}

#[tokio::test]
async fn repeated_silent_resumes_have_a_bounded_retry_budget() -> Result<()> {
    let requests = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&requests);
    let server = Server::start(move |_request, mut socket| {
        let count = Arc::clone(&count);
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            start_stream(&mut socket, "event: run\ndata: {\"runId\":\"silent\"}\n\n").await?;
            std::future::pending::<()>().await;
            Ok(())
        }
    })
    .await?;
    let api = ApiClient::with_stream_idle_timeout(&server.url, "fixture", TEST_IDLE_TIMEOUT)?;
    let (sender, _receiver) = mpsc::unbounded_channel();
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        api.stream_chat(request("a")?, sender),
    )
    .await?
    .expect_err("silent resumptions must exhaust the retry budget");
    assert!(error.to_string().contains("resume attempts exhausted"));
    assert_eq!(requests.load(Ordering::SeqCst), 6);
    Ok(())
}

#[tokio::test]
async fn heartbeats_keep_a_long_thinking_turn_alive() -> Result<()> {
    let server = Server::start(|request, mut socket| async move {
        anyhow::ensure!(
            request.starts_with("POST /api/chat "),
            "healthy stream was reattached"
        );
        start_stream(&mut socket, "event: run\ndata: {\"runId\":\"healthy\"}\n\n").await?;
        let mut heartbeat = tokio::time::interval(Duration::from_millis(50));
        // 总时长超过空闲窗口；注释心跳本身应保持连接，无需业务文本进展。
        for _ in 0..20 {
            heartbeat.tick().await;
            socket.write_all(b":\n\n").await?;
        }
        socket
            .write_all(b"id: 1\nevent: done\ndata: {}\n\n")
            .await?;
        Ok(())
    })
    .await?;
    let api = ApiClient::with_stream_idle_timeout(&server.url, "fixture", TEST_IDLE_TIMEOUT)?;
    let (sender, _receiver) = mpsc::unbounded_channel();
    tokio::time::timeout(TEST_DEADLINE, api.stream_chat(request("healthy")?, sender)).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_terminal_processes_recover_without_interrupting_their_sibling() -> Result<()> {
    if let Ok(scenario) = std::env::var("PISPER_STREAM_TEST_SCENARIO") {
        let url = std::env::var("PISPER_STREAM_TEST_URL")?;
        let api = ApiClient::with_stream_idle_timeout(&url, "fixture", TEST_IDLE_TIMEOUT)?;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        tokio::time::timeout(TEST_DEADLINE, api.stream_chat(request(&scenario)?, sender)).await??;
        let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::StreamReconnecting { .. })),
            scenario == "stalled"
        );
        assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Stream(frame) if frame.name == "done" && frame.data["text"] == scenario)));
        let mut app = thinking_app(&scenario);
        for event in events {
            match event {
                RuntimeEvent::Stream(frame) => app.apply_stream_event(frame),
                RuntimeEvent::StreamReconnecting {
                    session_id,
                    attempt,
                    max_attempts,
                } => {
                    app.stream_reconnecting(&session_id, attempt, max_attempts);
                    assert!(app.status.starts_with("reconnecting stream"));
                }
                _ => {}
            }
        }
        assert!(!app.is_streaming());
        assert_eq!(app.status, "complete");
        return Ok(());
    }

    let both_started = Arc::new(Barrier::new(2));
    let recovered = Arc::new(Notify::new());
    let submissions = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&submissions);
    let server = Server::start(move |request, mut socket| {
        let both_started = Arc::clone(&both_started);
        let recovered = Arc::clone(&recovered);
        let count = Arc::clone(&count);
        async move {
            if request.starts_with("GET /api/runs/stalled/events?after=1 ") {
                recovered.notify_one();
                start_stream(&mut socket, "id: 2\nevent: done\ndata: {\"text\":\"stalled\"}\n\n").await?;
                return Ok(());
            }
            anyhow::ensure!(request.starts_with("POST /api/chat "), "unexpected request");
            count.fetch_add(1, Ordering::SeqCst);
            let (_, body) = request.split_once("\r\n\r\n").context("missing body")?;
            let payload: serde_json::Value = serde_json::from_str(body)?;
            let session = payload["sessionId"].as_str().context("missing session")?;
            start_stream(&mut socket, &format!("event: run\ndata: {{\"runId\":\"{session}\"}}\n\nid: 1\nevent: thinking_patch\ndata: {{\"start\":0,\"text\":\"partial\"}}\n\n")).await?;
            both_started.wait().await;
            if session == "stalled" {
                std::future::pending::<()>().await;
            } else {
                anyhow::ensure!(session == "healthy", "unexpected session");
                let mut heartbeat = tokio::time::interval(Duration::from_millis(30));
                loop {
                    tokio::select! {
                        _ = recovered.notified() => break,
                        _ = heartbeat.tick() => socket.write_all(b":\n\n").await?,
                    }
                }
                socket.write_all(b"id: 2\nevent: done\ndata: {\"text\":\"healthy\"}\n\n").await?;
            }
            Ok(())
        }
    }).await?;
    let executable = std::env::current_exe()?;
    let workspace = std::env::current_dir()?;
    let mut workers = Vec::new();
    for scenario in ["stalled", "healthy"] {
        let child = Command::new(&executable)
            .args(["--exact", PROCESS_TEST, "--nocapture"])
            .env("PISPER_STREAM_TEST_SCENARIO", scenario)
            .env("PISPER_STREAM_TEST_URL", &server.url)
            .current_dir(&workspace)
            .spawn()?;
        workers.push(tokio::task::spawn_blocking(move || {
            child.wait_with_output()
        }));
    }
    let mut succeeded = true;
    for worker in workers {
        succeeded &= worker.await??.status.success();
    }
    assert!(succeeded, "an independent terminal failed to finish");
    assert_eq!(submissions.load(Ordering::SeqCst), 2);
    Ok(())
}
