//! LAN 发现只广播公开证书指纹与 TLS 端口，绝不广播配对码或设备令牌。
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};

pub(crate) struct Advertisement(ServiceDaemon);

impl Advertisement {
    pub fn start(port: u16, fingerprint: &str) -> Result<Self, String> {
        let identity = fingerprint.strip_prefix("SHA256:").unwrap_or(fingerprint);
        let suffix = identity.get(..12).ok_or("无效的服务器指纹。")?;
        let properties = [("v", "1"), ("name", "Pisper"), ("fp", fingerprint)];
        let service = ServiceInfo::new(
            "_pisper._tcp.local.",
            &format!("Pisper-{suffix}"),
            &format!("pisper-{}.local.", suffix.to_ascii_lowercase()),
            (),
            port,
            &properties[..],
        )
        .map_err(|_| "无法创建局域网发现服务。")?
        .enable_addr_auto();
        let daemon = ServiceDaemon::new().map_err(|_| "无法启动局域网发现。")?;
        // TLS 监听器当前为 IPv4，不能广播无法连接的 IPv6 LAN 地址；Iroh 独立处理 IPv6。
        if daemon.disable_interface(IfKind::IPv6).is_err() || daemon.register(service).is_err() {
            let _ = daemon.shutdown();
            return Err("无法注册局域网发现服务。".into());
        }
        Ok(Self(daemon))
    }

    pub async fn stop(self) {
        if let Ok(done) = self.0.shutdown() {
            // 等待 mDNS 线程撤销广播；不遗留持有网络接口的后台线程。
            let _ = done.recv_async().await;
        }
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}
