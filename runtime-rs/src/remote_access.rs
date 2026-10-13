//! 远程传输生命周期：TLS 与 Iroh 一起启停；已有证书、身份和设备路径不搬迁。
use axum::{body::Body, extract::ConnectInfo, Router};
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use pisper_remote_transport::{
    iroh_tunnel::{self, TunnelServer},
    RelayMode,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Mutex as AsyncMutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub(crate) struct RemoteConfig {
    secret: PathBuf,
    port: Option<u16>,
    relay_only: bool,
    offline: bool,
}

impl RemoteConfig {
    pub fn from_environment(data_dir: &str) -> anyhow::Result<Self> {
        // 原桌面 Iroh 私钥就在 Tauri 数据目录；状态文件只用于定位，绝不复制到 agent 目录。
        let directory = std::env::var_os("PISPER_IROH_STATUS_FILE")
            .map(PathBuf::from)
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(data_dir));
        let port = std::env::var("PISPER_REMOTE_PORT")
            .ok()
            .map(|value| value.parse())
            .transpose()?;
        Ok(Self {
            secret: directory.join("iroh-secret.key"),
            port,
            relay_only: std::env::var("PISPER_REMOTE_RELAY_ONLY").ok().as_deref() == Some("1"),
            offline: std::env::var("PISPER_REMOTE_OFFLINE").ok().as_deref() == Some("1"),
        })
    }
}

struct Running {
    stop: CancellationToken,
    task: JoinHandle<()>,
    tunnel: Arc<TunnelServer>,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    advertisement: Option<crate::remote_discovery::Advertisement>,
}

pub(crate) struct RemoteAccess {
    config: RemoteConfig,
    directory: PathBuf,
    lifecycle: AsyncMutex<Option<Running>>,
    configuration: AsyncMutex<()>,
    snapshot: Mutex<Value>,
    devices: Mutex<HashMap<String, CancellationToken>>,
    failures: Mutex<HashMap<String, (u64, u8)>>,
}

impl RemoteAccess {
    pub fn new(data_dir: &str, config: RemoteConfig) -> Self {
        Self {
            config,
            directory: data_dir.into(),
            lifecycle: AsyncMutex::new(None),
            configuration: AsyncMutex::new(()),
            snapshot: Mutex::new(
                json!({"apiVersion":1,"listening":false,"error":null,"fingerprint":"","endpoints":[]}),
            ),
            devices: Mutex::default(),
            failures: Mutex::default(),
        }
    }

    pub async fn configure(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.configuration.lock().await
    }

    pub fn info(&self) -> Value {
        self.snapshot.lock().expect("remote snapshot").clone()
    }

    pub fn error(&self, message: &str) {
        self.snapshot.lock().expect("remote snapshot")["error"] = json!(message);
    }

    pub async fn start(&self, router: Router, main_port: Option<u16>) -> Result<(), String> {
        let mut lifecycle = self.lifecycle.lock().await;
        if lifecycle.is_some() {
            return Ok(());
        }
        let certificate = crate::remote_tls::load_or_create(&self.directory)?;
        let secret = iroh_tunnel::load_or_create_secret(&self.config.secret)?;
        // 安装版沿用 5174；开发 Web 已占用该端口时使用 5175，二维码记录真实端口。
        let port = self
            .config
            .port
            .unwrap_or(if main_port == Some(5174) { 5175 } else { 5174 });
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
            .await
            .map_err(|_| "无法监听远程 TLS 端口。")?;
        let port = listener
            .local_addr()
            .map_err(|_| "无法读取远程 TLS 端口。")?
            .port();
        let relay_mode = if self.config.offline {
            RelayMode::Disabled
        } else {
            iroh_tunnel::production_relay_mode()
        };
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let tunnel = Arc::new(if self.config.relay_only {
            iroh_tunnel::start_server_relay_only(target, secret, relay_mode).await?
        } else {
            iroh_tunnel::start_server(target, secret, relay_mode).await?
        });
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let tls = tokio_rustls::TlsAcceptor::from(certificate.config);
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopping.cancelled() => break,
                    _ = connections.join_next(), if !connections.is_empty() => {},
                    incoming = listener.accept(), if connections.len() < 128 => {
                        let Ok((stream, address)) = incoming else { break; };
                        let tls = tls.clone();
                        let router = router.clone().layer(axum::Extension(ConnectInfo(address)));
                        connections.spawn(async move {
                            let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(10), tls.accept(stream)).await else {return;};
                            let service = TowerToHyperService::new(router);
                            let _ = hyper::server::conn::http1::Builder::new()
                                .timer(hyper_util::rt::TokioTimer::new())
                                .header_read_timeout(Duration::from_secs(15))
                                .max_buf_size(64 * 1024)
                                .serve_connection(TokioIo::new(stream), hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                                    let service = service.clone();
                                    async move { hyper::service::Service::call(&service, request.map(Body::new)).await }
                                })).await;
                        });
                    },
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        let mut endpoints = Vec::new();
        if !self.config.relay_only {
            // 查询系统路由所选本地地址，不发送数据或披露该地址给第三方。
            if let Ok(socket) = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
                if socket.connect("192.0.2.1:9").is_ok() {
                    if let Ok(address) = socket.local_addr() {
                        if !address.ip().is_unspecified() && !address.ip().is_loopback() {
                            endpoints.push(
                                json!({"t":"lan","url":format!("https://{}:{port}",address.ip())}),
                            );
                        }
                    }
                }
            }
        }
        *self.snapshot.lock().expect("remote snapshot") = json!({"apiVersion":1,"listening":true,"error":null,
            "fingerprint":certificate.fingerprint,"endpoints":endpoints,"port":port});
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let advertisement = if self.config.relay_only || self.config.offline {
            None
        } else {
            match crate::remote_discovery::Advertisement::start(port, &certificate.fingerprint) {
                Ok(advertisement) => Some(advertisement),
                Err(_) => {
                    tracing::warn!(
                        code = "remote_discovery_unavailable",
                        "局域网发现未启动，二维码连接仍可使用。"
                    );
                    None
                }
            }
        };
        *lifecycle = Some(Running {
            stop,
            task,
            tunnel,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            advertisement,
        });
        Ok(())
    }

    pub async fn refresh_endpoints(&self) -> Value {
        let tunnel = self
            .lifecycle
            .lock()
            .await
            .as_ref()
            .map(|running| running.tunnel.clone());
        if let Some(tunnel) = tunnel {
            let published = tunnel.endpoint(Duration::from_secs(3)).await;
            let lifecycle = self.lifecycle.lock().await;
            if lifecycle
                .as_ref()
                .is_some_and(|running| Arc::ptr_eq(&running.tunnel, &tunnel))
            {
                let mut info = self.snapshot.lock().expect("remote snapshot");
                let endpoints = info["endpoints"].as_array_mut().expect("remote endpoints");
                endpoints.retain(|endpoint| endpoint["t"] != "iroh");
                if published.relay_url.is_some()
                    || (!self.config.relay_only && !published.direct_addresses.is_empty())
                {
                    let mut endpoint = json!(published);
                    endpoint["t"] = json!("iroh");
                    endpoints.push(endpoint);
                }
            }
        }
        self.info()
    }

    pub async fn stop(&self) {
        let mut lifecycle = self.lifecycle.lock().await;
        if let Some(running) = lifecycle.take() {
            running.stop.cancel();
            running.tunnel.shutdown().await;
            let _ = running.task.await;
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            if let Some(advertisement) = running.advertisement {
                advertisement.stop().await;
            }
        }
        for (_, token) in self.devices.lock().expect("remote devices").drain() {
            token.cancel();
        }
        let mut info = self.snapshot.lock().expect("remote snapshot");
        info["listening"] = json!(false);
        info["endpoints"] = json!([]);
    }

    pub fn device_stream(&self, id: &str) -> CancellationToken {
        self.devices
            .lock()
            .expect("remote devices")
            .entry(id.into())
            .or_default()
            .clone()
    }

    pub fn revoke(&self, id: &str) {
        // 鉴权与响应注册可能交错；即使尚无流，也留下已取消的记录以封闭竞态。
        self.devices
            .lock()
            .expect("remote devices")
            .entry(id.into())
            .or_default()
            .cancel();
    }

    pub fn paired(&self, source: &str) {
        self.failures
            .lock()
            .expect("remote failures")
            .remove(source);
    }

    pub fn pairing_attempt(&self, source: &str) -> bool {
        let now = crate::product::now_ms();
        let mut failures = self.failures.lock().expect("remote failures");
        failures.retain(|_, (start, _)| now.saturating_sub(*start) < 60_000);
        if failures.len() >= 1024 && !failures.contains_key(source) {
            return false;
        }
        let entry = failures.entry(source.into()).or_insert((now, 0));
        if entry.1 >= 5 {
            return false;
        }
        entry.1 += 1;
        true
    }
}

pub(crate) fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "安全随机数不可用。")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_attempts_are_bounded_and_success_resets_the_source() {
        let remote = RemoteAccess::new(
            "unused-fixture",
            RemoteConfig {
                secret: PathBuf::from("unused-fixture"),
                port: Some(0),
                relay_only: false,
                offline: true,
            },
        );
        for _ in 0..5 {
            assert!(remote.pairing_attempt("fixture-source"));
        }
        assert!(!remote.pairing_attempt("fixture-source"));
        assert!(remote.pairing_attempt("other-source"));
        remote.paired("fixture-source");
        assert!(remote.pairing_attempt("fixture-source"));
        remote
            .failures
            .lock()
            .unwrap()
            .insert("expired-source".into(), (0, 5));
        assert!(remote.pairing_attempt("expired-source"));
    }

    #[test]
    fn revocation_cancels_existing_and_late_device_streams() {
        let remote = RemoteAccess::new(
            "unused-fixture",
            RemoteConfig {
                secret: PathBuf::from("unused-fixture"),
                port: Some(0),
                relay_only: false,
                offline: true,
            },
        );
        let existing = remote.device_stream("paired");
        remote.revoke("paired");
        assert!(existing.is_cancelled());
        assert!(remote.device_stream("paired").is_cancelled());
        remote.revoke("not-yet-streaming");
        assert!(remote.device_stream("not-yet-streaming").is_cancelled());
        assert!(!remote.device_stream("other-device").is_cancelled());
    }
}
