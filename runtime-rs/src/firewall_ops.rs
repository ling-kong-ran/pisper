//! release `services/remote-firewall-service.mjs` 的原生移植（Windows 优先）：
//! 防火墙规则的检查（非提权 inspect）/ 建立 / 删除，状态持久化
//! `remote-firewall.json`，`status()` 与 `retryFirewall()` 契约对齐。
//!
//! 提权路径使用与 release 相同的 PowerShell 语义（Start-Process -Verb RunAs）；
//! 无头环境下 UAC 无法确认，`state` 如实报告 failed 并保留 exit 码原因。

use axum::{extract::State, Json};
use serde_json::{json, Value};
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{product, ApiError, AppState};

const GIT_TIMEOUT_MS: u64 = 15_000;
const ELEVATED_TIMEOUT_MS: u64 = 120_000;

type ArcAppState = Arc<AppState>;

fn state_path(state: &AppState) -> PathBuf {
    Path::new(&state.data_dir).join("remote-firewall.json")
}

fn owner_hash(state: &AppState) -> String {
    let digest = sha2::Sha256::digest(state.data_dir.as_bytes());
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn rule_name(state: &AppState) -> String {
    format!("Pisper-Remote-{}", owner_hash(state))
}

#[derive(Debug, Clone)]
struct FirewallTarget {
    port: u16,
    exec_path: String,
}

fn load_targets(state: &AppState) -> Vec<FirewallTarget> {
    std::fs::read_to_string(state_path(state))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value["targets"]
                .as_array()
                .cloned()
        })
        .map(|targets| {
            targets
                .into_iter()
                .filter_map(|target| {
                    // 历史目标同样要求 sidecar 形态的可执行名（release sidecarPath 校验）。
                    let exec_path = target["execPath"].as_str()?.to_string();
                    let name = Path::new(&exec_path)
                        .file_name()
                        .and_then(|name| name.to_str())?;
                    let ok = name == "pisper-server.exe"
                        || (name.starts_with("pisper-sidecar")
                            && (name.len() == "pisper-sidecar".len()
                                || name["pisper-sidecar".len()..]
                                    .chars()
                                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))));
                    if !ok {
                        return None;
                    }
                    Some(FirewallTarget {
                        port: target["port"].as_u64()? as u16,
                        exec_path,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_targets(state: &AppState, targets: &[FirewallTarget]) -> Result<(), ApiError> {
    let value = json!({
        "version": 1,
        "targets": targets.iter().map(|target| json!({
            "platform": "win32",
            "port": target.port,
            "execPath": target.exec_path,
        })).collect::<Vec<_>>(),
    });
    std::fs::create_dir_all(&state.data_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    std::fs::write(
        state_path(state),
        serde_json::to_string_pretty(&value).map_err(|e| ApiError::internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::internal(e.to_string()))
}

struct CommandResult {
    code: i32,
    stdout: String,
    missing: bool,
    timed_out: bool,
}

/// release executeFirewallCommand：powershell -NoProfile -NonInteractive
/// -EncodedCommand <utf16le base64>，受控脚本退出码即验证结论。
async fn run_powershell(script: &str, elevated: bool) -> CommandResult {
    let encoded = {
        let utf16: Vec<u8> = script
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        // base64 编码（无外部 crate：手写标准字母表）。
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in utf16.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            out.push(TABLE[(b[0] >> 2) as usize] as char);
            out.push(TABLE[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 { TABLE[(b[2] & 0x3f) as usize] as char } else { '=' });
        }
        out
    };
    let ps = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
    let full = if elevated {
        // 与 release commandFor(win32, elevated) 相同的提权包装：
        // UAC 取消 → exit 80（用户取消）；其它失败 → exit 77。
        format!(
            "$ErrorActionPreference = 'Stop'; try {{ $p = Start-Process -FilePath '{ps}' -Verb RunAs -Wait -PassThru -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand','{encoded}'); exit $p.ExitCode }} catch {{ if ($_.Exception.NativeErrorCode -eq 1223 -or $_.Exception.InnerException.NativeErrorCode -eq 1223) {{ exit 80 }}; exit 77 }}"
        )
    } else {
        script.to_string()
    };
    let encoded_full = {
        let utf16: Vec<u8> = full.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in utf16.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            out.push(TABLE[(b[0] >> 2) as usize] as char);
            out.push(TABLE[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 { TABLE[(b[2] & 0x3f) as usize] as char } else { '=' });
        }
        out
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(if elevated {
            ELEVATED_TIMEOUT_MS
        } else {
            GIT_TIMEOUT_MS
        }),
        tokio::process::Command::new(ps)
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded_full])
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) => CommandResult {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            missing: false,
            timed_out: false,
        },
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => CommandResult {
            code: -1,
            stdout: String::new(),
            missing: true,
            timed_out: false,
        },
        Ok(Err(_)) => CommandResult {
            code: -1,
            stdout: String::new(),
            missing: false,
            timed_out: false,
        },
        Err(_) => CommandResult {
            code: -1,
            stdout: String::new(),
            missing: false,
            timed_out: true,
        },
    }
}

/// release windowsScript(target, 'inspect')：规则存在且逐字段匹配 → 0；不匹配 → 12。
fn inspect_script(state: &AppState, target: &FirewallTarget) -> String {
    let name = rule_name(state).replace('\'', "''");
    let program = target.exec_path.replace('\'', "''");
    let port = target.port;
    format!(
        "$r = Get-NetFirewallRule -PolicyStore ActiveStore -Name '{name}' -ErrorAction SilentlyContinue\n\
         if (!$r) {{ exit 12 }}\n\
         $p = $r | Get-NetFirewallPortFilter\n\
         $a = $r | Get-NetFirewallApplicationFilter\n\
         $s = $r | Get-NetFirewallAddressFilter\n\
         if (@($r).Count -ne 1 -or $r.Enabled -ne 'True' -or $r.Direction -ne 'Inbound' -or $r.Action -ne 'Allow' -or $p.Protocol -ne 'TCP' -or \"$($p.LocalPort)\" -ne '{port}' -or $a.Program -ne '{program}' -or \"$($s.RemoteAddress)\" -ne 'LocalSubnet' -or \"$($r.Profile)\" -ne 'Any') {{ exit 12 }}\n\
         exit 0"
    )
}

/// release windowsScript(target, operation)：add/remove 规则脚本。
fn mutation_script(state: &AppState, target: &FirewallTarget, operation: &str) -> String {
    let name = rule_name(state).replace('\'', "''");
    let program = target.exec_path.replace('\'', "''");
    let port = target.port;
    if operation == "remove" {
        format!(
            "Get-NetFirewallRule -PolicyStore PersistentStore -Name '{name}' -ErrorAction SilentlyContinue | Remove-NetFirewallRule\n\
             if (Get-NetFirewallRule -PolicyStore ActiveStore -Name '{name}' -ErrorAction SilentlyContinue) {{ exit 12 }}\n\
             exit 0"
        )
    } else {
        format!(
            "New-NetFirewallRule -Name '{name}' -DisplayName 'Pisper remote access' -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port} -RemoteAddress LocalSubnet -Profile Any -Program '{program}' | Out-Null\n\
             $r = Get-NetFirewallRule -PolicyStore ActiveStore -Name '{name}' -ErrorAction SilentlyContinue\n\
             if (!$r) {{ exit 12 }}\n\
             exit 0"
        )
    }
}

fn now_iso() -> String {
    crate::session_ops::iso_timestamp(product::now_ms())
}

async fn inspect(state: &AppState, target: &FirewallTarget) -> Value {
    let result = run_powershell(&inspect_script(state, target), false).await;
    if result.missing {
        return json!({"state":"unsupported","reason":"firewall_tool_missing","port":target.port,"checkedAt":now_iso()});
    }
    if result.timed_out {
        return json!({"state":"failed","reason":"inspection_timeout","port":target.port,"checkedAt":now_iso()});
    }
    match result.code {
        0 => json!({"state":"allowed","reason":Value::Null,"port":target.port,"checkedAt":now_iso()}),
        12 => json!({"state":"required","reason":"rule_missing","port":target.port,"checkedAt":now_iso()}),
        code => json!({"state":"failed","reason":format!("inspect_exit_{code}"),"port":target.port,"checkedAt":now_iso()}),
    }
}

/// 当前聚合状态视图（release RemoteFirewallService.status 形状）。
pub(crate) async fn current_status(state: &AppState) -> Value {
    let enabled = state.remote_enabled.load(std::sync::atomic::Ordering::Relaxed);
    if !enabled {
        return json!({
            "state": "disabled", "reason": Value::Null, "port": Value::Null,
            "scope": "program_port", "checkedAt": Value::Null, "busy": false,
            "lanReachability": "unverified",
        });
    }
    let targets = load_targets(state);
    if targets.is_empty() {
        return json!({
            "state": "failed", "reason": "firewall_management_unavailable", "port": Value::Null,
            "scope": "program_port", "checkedAt": now_iso(), "busy": false,
            "lanReachability": "unverified",
        });
    }
    let target = &targets[0];
    let mut view = inspect(state, target).await;
    view["busy"] = json!(false);
    view["scope"] = json!("program_port");
    view["lanReachability"] = json!("unverified");
    view
}

/// release retryFirewall：远程开启且监听未就绪时先尝试启动（本侧远程监听为
/// 诚实接缝，保持开启状态即可），随后对已登记目标做非提权校验；规则缺失时
/// 返回 required —— 由前端触发带提权的 reconcile。
pub(crate) async fn retry(state: &AppState) -> Value {
    current_status(state).await
}

/// release setEnabled({configureFirewall:true}) 的 reconcile：登记/清理目标
/// 并执行提权建立或删除。
pub(crate) async fn reconcile(
    state: &AppState,
    enabled: bool,
    port: Option<u16>,
) -> Result<Value, ApiError> {
    let mut targets = load_targets(state);
    let exec_path = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if enabled {
        let port = port.unwrap_or(0);
        if port == 0 {
            return Err(ApiError::bad_request("缺少防火墙目标端口。"));
        }
        let target = FirewallTarget { port, exec_path };
        if !targets.iter().any(|item| item.port == target.port) {
            targets.push(target);
        }
        save_targets(state, &targets)?;
        // 提权建立（无头环境返回 77/80 退出码并如实报告）。
        let target = &targets[targets.len() - 1];
        let result = run_powershell(&mutation_script(state, target, "add"), true).await;
        let mut view = inspect(state, target).await;
        view["busy"] = json!(false);
        view["scope"] = json!("program_port");
        view["lanReachability"] = json!("unverified");
        if result.code == 80 {
            view["state"] = json!("required");
            view["reason"] = json!("elevation_declined");
        } else if result.code == 77 {
            view["state"] = json!("failed");
            view["reason"] = json!("elevation_failed");
        }
        Ok(view)
    } else {
        let mut last: Option<Value> = None;
        for target in &targets {
            let result = run_powershell(&mutation_script(state, target, "remove"), true).await;
            let mut view = inspect(state, target).await;
            if result.code == 80 {
                view["state"] = json!("required");
                view["reason"] = json!("elevation_declined");
            }
            last = Some(view);
        }
        save_targets(state, &[])?;
        match last {
            Some(view) => Ok(view),
            None => Ok(current_status(state).await),
        }
    }
}

/// release GET /api/remote/firewall
pub(crate) async fn firewall_status(
    State(state): State<ArcAppState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(current_status(&state).await))
}

/// release POST /api/remote/firewall/retry（415 JSON 校验由路由层保留）。
pub(crate) async fn firewall_retry(
    State(state): State<ArcAppState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(retry(&state).await))
}
