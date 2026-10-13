//! 沿用 Node 的 remote-tls.json；已有证书和私钥保持原字节，不迁移或替换损坏文件。
use std::{io::Write, path::Path, sync::Arc};

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct StoredCertificate {
    version: u32,
    cert: String,
    key: String,
}

pub(crate) struct RemoteCertificate {
    pub fingerprint: String,
    pub config: Arc<rustls::ServerConfig>,
}

fn parse(stored: &StoredCertificate) -> Result<RemoteCertificate, String> {
    if stored.version != 1 {
        return Err("远程证书版本无效，已有文件已保留。".into());
    }
    let certificates = CertificateDer::pem_slice_iter(stored.cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "远程证书无法解析，已有文件已保留。")?;
    let first = certificates
        .first()
        .ok_or("远程证书为空，已有文件已保留。")?;
    let fingerprint = format!(
        "SHA256:{}",
        Sha256::digest(first.as_ref())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
    );
    let key = PrivateKeyDer::from_pem_slice(stored.key.as_bytes())
        .map_err(|_| "远程私钥无法解析，已有文件已保留。")?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "远程 TLS 配置失败。")?
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .map_err(|_| "远程证书与私钥不匹配，已有文件已保留。")?;
    Ok(RemoteCertificate {
        fingerprint,
        config: Arc::new(config),
    })
}

pub(crate) fn load_or_create(directory: &Path) -> Result<RemoteCertificate, String> {
    let path = directory.join("remote-tls.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            return parse(
                &serde_json::from_slice(&bytes)
                    .map_err(|_| "远程证书文件格式无效，已有文件已保留。")?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法读取远程证书。".into()),
    }
    let generated = rcgen::generate_simple_self_signed(vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "::1".into(),
    ])
    .map_err(|_| "无法创建远程证书。")?;
    let stored = StoredCertificate {
        version: 1,
        cert: generated.cert.pem(),
        key: generated.key_pair.serialize_pem(),
    };
    let certificate = parse(&stored)?;
    std::fs::create_dir_all(directory).map_err(|_| "无法创建远程证书目录。")?;
    let temporary = directory.join(format!("remote-tls.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(&stored)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
        return Err("无法保存远程证书。".into());
    }
    Ok(certificate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_reopens_at_original_path_without_rewriting_and_preserves_corruption() {
        let directory =
            std::env::temp_dir().join(format!("pisper-remote-cert-{}", uuid::Uuid::new_v4()));
        let first = load_or_create(&directory).unwrap();
        let path = directory.join("remote-tls.json");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(first.fingerprint.len(), 71);
        assert_eq!(
            load_or_create(&directory).unwrap().fingerprint,
            first.fingerprint
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(&path, b"broken legacy file").unwrap();
        assert!(load_or_create(&directory).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken legacy file");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
