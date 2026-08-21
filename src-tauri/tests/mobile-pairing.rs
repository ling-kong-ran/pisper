// 复现移动端配对 panic 的主机侧测试：完整走 mobile::pairing::pair 路径。
use std::sync::Arc;

use pisper_webview_lib::mobile::pairing::{self, QrPayload};
use pisper_webview_lib::mobile::store::ServerEndpoint;

#[tokio::test]
async fn pair_against_tls_upstream() {
    // 自签 TLS 上游模拟桌面端 /api/remote/pair
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der = certified.cert.der().clone();
    let fingerprint = {
        use sha2::Digest;
        sha2::Sha256::digest(cert_der.as_ref())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls_config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert_der],
            rustls::pki_types::PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()).into(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let mut stream = acceptor.accept(stream).await.unwrap();
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if head.len() > 64 * 1024 {
                        return;
                    }
                }
                // 读完请求体（content-length）。
                let text = String::from_utf8_lossy(&head);
                let content_length: usize = text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                let mut body = Vec::new();
                while body.len() < content_length {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    body.extend_from_slice(&buf[..n]);
                }
                let payload = "{\"deviceId\":\"dev_test\",\"token\":\"pst_test\",\"serverName\":\"测试桌面\",\"apiVersion\":1}".as_bytes();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                            payload.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(payload).await.unwrap();
            });
        }
    });

    let payload = QrPayload {
        v: 1,
        name: "测试".into(),
        endpoints: vec![ServerEndpoint {
            kind: "lan".into(),
            url: format!("https://{addr}"),
        }],
        fp: format!("SHA256:{fingerprint}"),
        code: "ABCD-EFGH".into(),
    };
    let profile = pairing::pair(&payload, "测试手机").await.expect("配对应成功");
    assert_eq!(profile.device_id, "dev_test");
    assert_eq!(profile.token, "pst_test");
    assert_eq!(profile.name, "测试桌面");
}
