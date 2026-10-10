//! 本机 Web 使用系统目录对话框；路径通过参数或环境传递，绝不插入脚本代码。
use crate::ApiError;
use axum::{
    extract::ConnectInfo,
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::Path, time::Duration};
use tokio::{process::Command, sync::Semaphore};
use tokio_util::sync::CancellationToken;

static PICKER: Semaphore = Semaphore::const_new(1);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectoryRequest {
    #[serde(default)]
    initial_directory: String,
}

fn require_local(addr: SocketAddr, headers: &HeaderMap) -> Result<(), ApiError> {
    let origin_local = headers.get("origin").map_or(true, |origin| {
        origin
            .to_str()
            .ok()
            .and_then(|value| reqwest::Url::parse(value).ok())
            .is_some_and(|url| matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
    });
    if !addr.ip().is_loopback()
        || !origin_local
        || headers.contains_key("forwarded")
        || headers.contains_key("x-forwarded-for")
        || headers.contains_key("x-pisper-client")
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "system_picker_local_only",
            "系统目录选择仅供本机客户端使用。",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn directory_command(initial: &str) -> Command {
    let mut command = Command::new("/usr/bin/osascript");
    command.args(["-e", r#"on run argv
try
  if item 1 of argv is "" then
    set selectedFolder to choose folder with prompt "Pisper"
  else
    set selectedFolder to choose folder with prompt "Pisper" default location (POSIX file (item 1 of argv))
  end if
  return POSIX path of selectedFolder
on error number -128
  return ""
end try
end run"#, "--", initial]);
    command
}

#[cfg(target_os = "windows")]
fn directory_command(initial: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-Command",
        r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = 'Pisper'
$dialog.SelectedPath = $env:PISPER_PICKER_INITIAL_DIRECTORY
try {
  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    ConvertTo-Json -InputObject $dialog.SelectedPath -Compress
  } else { 'null' }
} finally { $dialog.Dispose() }
"#,
    ]);
    command.env("PISPER_PICKER_INITIAL_DIRECTORY", initial);
    command
}

#[cfg(target_os = "linux")]
fn directory_command(initial: &str) -> Command {
    let mut command = Command::new("zenity");
    command.args(["--file-selection", "--directory", "--title=Pisper"]);
    if !initial.is_empty() {
        command.arg(format!("--filename={}/", initial.trim_end_matches('/')));
    }
    command
}

fn picker_error() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "system_picker_unavailable",
        "无法打开系统目录选择器；Linux 需要安装 Zenity，并在图形桌面中运行。",
    )
}

fn picker_stopping() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "system_picker_stopping",
        "服务正在退出，已取消目录选择。",
    )
}

fn decode_selection(
    success: bool,
    code: Option<i32>,
    stdout: &[u8],
) -> Result<Option<String>, ApiError> {
    if !success {
        if cfg!(target_os = "linux") && code == Some(1) {
            return Ok(None);
        }
        return Err(picker_error());
    }
    #[cfg(target_os = "windows")]
    {
        serde_json::from_slice(stdout).map_err(|_| picker_error())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let output = std::str::from_utf8(stdout).map_err(|_| picker_error())?;
        // 只移除进程输出的换行，保留文件名中的空格和换行。
        let path = output.strip_suffix('\n').unwrap_or(output);
        Ok((!path.is_empty()).then(|| path.to_string()))
    }
}

pub(crate) async fn pick_directory(
    Extension(shutdown): Extension<CancellationToken>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<DirectoryRequest>,
) -> Result<Json<Value>, ApiError> {
    require_local(addr, &headers)?;
    if body.initial_directory.len() > 4096 || body.initial_directory.contains('\0') {
        return Err(ApiError::bad_request("目录参数无效。"));
    }
    if shutdown.is_cancelled() {
        return Err(picker_stopping());
    }
    let _permit = PICKER.try_acquire().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "system_picker_busy",
            "系统目录选择器已打开。",
        )
    })?;
    let initial = if Path::new(&body.initial_directory).is_absolute()
        && Path::new(&body.initial_directory).is_dir()
    {
        body.initial_directory.as_str()
    } else {
        ""
    };
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let mut command = directory_command(initial);
        command.kill_on_drop(true);
        // 退出 Runtime 时关闭本次对话框，避免未完成的 HTTP 请求阻止优雅退出。
        let output = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return Err(picker_stopping()),
            result = tokio::time::timeout(Duration::from_secs(600), command.output()) => {
                result.map_err(|_| picker_error())?.map_err(|_| picker_error())?
            }
        };
        let path = decode_selection(
            output.status.success(),
            output.status.code(),
            &output.stdout,
        )?;
        if path
            .as_ref()
            .is_some_and(|path| !Path::new(path).is_absolute() || !Path::new(path).is_dir())
        {
            return Err(picker_error());
        }
        Ok(Json(json!({"path": path})))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = initial;
        Err(picker_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_local_requests_can_open_native_dialogs() {
        let local = "127.0.0.1:5183".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert!(require_local(local, &headers).is_ok());
        assert!(require_local("192.168.1.2:42".parse().unwrap(), &headers).is_err());
        headers.insert("origin", "http://127.0.0.1:5183".parse().unwrap());
        assert!(require_local(local, &headers).is_ok());
        headers.insert("origin", "https://example.com".parse().unwrap());
        assert!(require_local(local, &headers).is_err());
        headers.remove("origin");
        headers.insert("x-forwarded-for", "192.168.1.2".parse().unwrap());
        assert!(require_local(local, &headers).is_err());
    }
    #[tokio::test]
    async fn http_contract_rejects_remote_invalid_and_duplicate_dialog_requests() {
        use axum::{body::Body, http::Request, routing::post, Router};
        use tower::ServiceExt;
        let shutdown = CancellationToken::new();
        let app = Router::new()
            .route("/pick", post(pick_directory))
            .layer(Extension(shutdown.clone()))
            .layer(axum::Extension(ConnectInfo(
                "127.0.0.1:1234".parse::<SocketAddr>().unwrap(),
            )));
        for (body, origin, expected) in [
            (json!({}), "https://example.com", StatusCode::FORBIDDEN),
            (
                json!({"initialDirectory":123}),
                "http://localhost:5183",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                json!({"initialDirectory":"x".repeat(4097)}),
                "http://localhost:5183",
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/pick")
                        .header("content-type", "application/json")
                        .header("origin", origin)
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        let _permit = PICKER.acquire().await.unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::post("/pick")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            "system_picker_busy"
        );
        shutdown.cancel();
        let response = app
            .oneshot(
                Request::post("/pick")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            "system_picker_stopping"
        );
    }
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn selection_preserves_path_characters_and_cancel_is_empty() {
        assert_eq!(
            decode_selection(true, Some(0), b"/tmp/a\n b \n").unwrap(),
            Some("/tmp/a\n b ".into())
        );
        assert_eq!(decode_selection(true, Some(0), b"\n").unwrap(), None);
        assert!(decode_selection(false, Some(2), b"").is_err());
        assert!(decode_selection(true, Some(0), &[255]).is_err());
    }
}
