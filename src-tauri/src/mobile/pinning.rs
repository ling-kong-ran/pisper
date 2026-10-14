//! TLS 指纹锁定：移动端不依赖系统 CA，而是配对时通过二维码带外获得证书指纹（TOFU），
//! 此后所有连接只接受指纹匹配的那一张证书。
//!
//! 指纹只锁定公开证书，不能证明对端持有私钥；TLS 1.2/1.3 的握手签名仍须由
//! rustls 的密码学实现验证。拒绝重定向和明文 HTTP，避免带认证的请求脱离固定端点。
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use sha2::{Digest, Sha256};

#[derive(Debug)]
struct PinnedCertVerifier {
    expected_prefix: String,
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let digest = Sha256::digest(end_entity.as_ref());
        let actual = digest
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>();
        if actual.starts_with(&self.expected_prefix) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(RustlsError::General(format!(
                "certificate fingerprint mismatch (got SHA256:{actual})"
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// 构造一个只接受指定指纹证书的 HTTPS 客户端。
pub fn pinned_client(fingerprint_prefix: &str) -> Result<reqwest::Client, String> {
    // 桌面依赖图同时启用 ring 和 aws-lc；不能依赖移动端的进程级初始化。
    let tls =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedCertVerifier {
                expected_prefix: fingerprint_prefix.to_uppercase(),
            }))
            .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        // SSE 长连接不能被总超时打断；只约束建连阶段。
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::PrivatePkcs8KeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn client_initializes_without_process_crypto_provider() {
        const CHILD: &str = "PISPER_PINNING_ISOLATED_TEST";
        if std::env::var_os(CHILD).is_some() {
            assert!(rustls::crypto::CryptoProvider::get_default().is_none());
            assert!(pinned_client(&"AB".repeat(32)).is_ok());
            return;
        }
        // 其他传输测试会设置全局 provider；子进程保证这里验证真实桌面冷启动。
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mobile::pinning::tests::client_initializes_without_process_crypto_provider",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[derive(Debug)]
    struct FixedCertificate(Arc<rustls::sign::CertifiedKey>);
    impl rustls::server::ResolvesServerCert for FixedCertificate {
        fn resolve(
            &self,
            _: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    async fn server(
        wrong_key: bool,
        version: &'static rustls::SupportedProtocolVersion,
        redirect: bool,
    ) -> (String, String, tokio::task::JoinHandle<bool>) {
        let certificate = generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let other = generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let fingerprint: String = Sha256::digest(certificate.cert.der())
            .iter()
            .map(|value| format!("{value:02X}"))
            .collect();
        let private_key = if wrong_key {
            &other.key_pair
        } else {
            &certificate.key_pair
        };
        let provider = rustls::crypto::ring::default_provider();
        let signing_key = provider
            .key_provider
            .load_private_key(PrivatePkcs8KeyDer::from(private_key.serialize_der()).into())
            .unwrap();
        // 自定义 resolver 可模拟复制公开证书却不持有对应私钥的对端。
        let resolver = FixedCertificate(Arc::new(rustls::sign::CertifiedKey::new(
            vec![certificate.cert.der().clone()],
            signing_key,
        )));
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[version])
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("https://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let Ok(mut stream) = acceptor.accept(socket).await else {
                return false;
            };
            let mut bytes = [0; 4096];
            if stream.read(&mut bytes).await.unwrap_or(0) == 0 {
                return false;
            }
            let response: &[u8] = if redirect {
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            };
            let _ = stream.write_all(response).await;
            let _ = stream.shutdown().await;
            true
        });
        (address, fingerprint, task)
    }

    #[tokio::test]
    async fn copied_certificate_without_private_key_is_rejected_in_tls12_and_tls13() {
        for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
            let (url, fp, task) = server(true, version, false).await;
            let result = pinned_client(&fp).unwrap().get(url).send().await;
            assert!(
                result.is_err(),
                "Matching public certificate alone must not authenticate {version:?}"
            );
            assert!(!task.await.unwrap());
        }
    }

    #[tokio::test]
    async fn pinned_connections_do_not_follow_redirects_or_allow_plain_http() {
        let (url, fp, task) = server(false, &rustls::version::TLS13, true).await;
        let client = pinned_client(&fp).unwrap();
        assert_eq!(
            client.get(url).send().await.unwrap().status(),
            reqwest::StatusCode::FOUND
        );
        assert!(task.await.unwrap());
        assert!(client
            .get("http://127.0.0.1:1/plain")
            .send()
            .await
            .unwrap_err()
            .is_builder());
    }
}
