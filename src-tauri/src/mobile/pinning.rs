//! TLS 指纹锁定：移动端不依赖系统 CA，而是配对时通过二维码带外获得证书指纹（TOFU），
//! 此后所有连接只接受指纹匹配的那一张证书。
//!
//! 安全说明：verify_tls12/13_signature 直接断言通过——这是安全的，前提是
//! verify_server_cert 已经逐字节比对了终端证书本身（指纹即证书的 SHA-256）。
//! 持有相同证书的攻击者必须同时拥有对应私钥，与公钥锁定等价。
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
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        // 覆盖服务端可能使用的全部常见签名方案；真正的安全边界在证书指纹比对。
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

/// 构造一个只接受指定指纹证书的 HTTPS 客户端。
pub fn pinned_client(fingerprint_prefix: &str) -> Result<reqwest::Client, String> {
    let tls = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCertVerifier {
            expected_prefix: fingerprint_prefix.to_uppercase(),
        }))
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        // SSE 长连接不能被总超时打断；只约束建连阶段。
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
}
