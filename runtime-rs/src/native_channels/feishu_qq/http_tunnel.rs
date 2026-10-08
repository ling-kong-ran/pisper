//! Axios's retained HTTPS agent forwarding HTTP through an HTTPS proxy.
//! This narrow client path needs TLS inside CONNECT, then an absolute-form GET.
use crate::native_channels::{ChannelError, Result};
use base64::Engine;
use std::{collections::HashMap, io::Read, sync::Arc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}
type Stream = Box<dyn Transport>;
const LIMIT: usize = 50 * 1024 * 1024;
pub(super) struct Response {
    pub(super) status: u16,
    pub(super) location: Option<String>,
    pub(super) body: Vec<u8>,
}
fn failed() -> ChannelError {
    ChannelError::new("fetch source URL failed")
}
fn host(url: &reqwest::Url) -> Result<&str> {
    url.host_str()
        .map(|host| host.trim_matches(['[', ']']))
        .ok_or_else(failed)
}
fn authority(url: &reqwest::Url) -> Result<String> {
    let host = host(url)?;
    Ok(format!(
        "{}:{}",
        if host.contains(':') {
            format!("[{host}]")
        } else {
            host.into()
        },
        url.port_or_known_default().ok_or_else(failed)?
    ))
}
fn authorization(url: &reqwest::Url) -> String {
    if url.username().is_empty() {
        return String::new();
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
        "{}:{}",
        url.username(),
        url.password().unwrap_or("")
    ));
    format!("Proxy-Authorization: Basic {encoded}\r\n")
}
pub(super) fn certificates(pem: &[u8]) -> Result<Vec<Vec<u8>>> {
    let text = std::str::from_utf8(pem).map_err(|_| failed())?;
    let mut certificates = Vec::new();
    let mut validated = rustls::RootCertStore::empty();
    for part in text.split("-----BEGIN CERTIFICATE-----").skip(1) {
        let encoded = part
            .split("-----END CERTIFICATE-----")
            .next()
            .ok_or_else(failed)?
            .split_whitespace()
            .collect::<String>();
        let der = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| failed())?;
        validated
            .add(rustls::pki_types::CertificateDer::from(der.clone()))
            .map_err(|_| failed())?;
        certificates.push(der);
    }
    if certificates.is_empty() {
        return Err(failed());
    }
    Ok(certificates)
}
fn tls_config(extra_root: Option<&[u8]>) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = extra_root {
        for der in certificates(pem)? {
            roots
                .add(rustls::pki_types::CertificateDer::from(der))
                .map_err(|_| failed())?;
        }
    }
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| failed())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
async fn tls(
    stream: Stream,
    url: &reqwest::Url,
    config: Arc<rustls::ClientConfig>,
) -> Result<Stream> {
    let server =
        rustls::pki_types::ServerName::try_from(host(url)?.to_owned()).map_err(|_| failed())?;
    tokio_rustls::TlsConnector::from(config)
        .connect(server, stream)
        .await
        .map(|stream| Box::new(stream) as Stream)
        .map_err(|_| failed())
}
async fn headers(stream: &mut Stream) -> Result<(u16, HashMap<String, String>, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = stream.read(&mut buffer).await.map_err(|_| failed())?;
        if count == 0 {
            return Err(failed());
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            if end > 16 * 1024 {
                return Err(failed());
            }
            let text = std::str::from_utf8(&bytes[..end]).map_err(|_| failed())?;
            let mut lines = text.split("\r\n");
            let status = lines
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|code| code.parse().ok())
                .ok_or_else(failed)?;
            let mut fields = HashMap::new();
            for line in lines {
                let (key, value) = line.split_once(':').ok_or_else(failed)?;
                fields.insert(key.to_ascii_lowercase(), value.trim().into());
            }
            return Ok((status, fields, bytes[end + 4..].to_vec()));
        }
        if bytes.len() > 16 * 1024 {
            return Err(failed());
        }
    }
}
fn chunked(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut at = 0;
    let mut result = Vec::new();
    loop {
        let end = bytes[at..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|index| at + index)
            .ok_or_else(failed)?;
        let text = std::str::from_utf8(&bytes[at..end])
            .map_err(|_| failed())?
            .split(';')
            .next()
            .ok_or_else(failed)?
            .trim();
        let count = usize::from_str_radix(text, 16).map_err(|_| failed())?;
        if count == 0 {
            return Ok(result);
        }
        at = end + 2;
        let end = at
            .checked_add(count)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(failed)?;
        if bytes.get(end..end + 2) != Some(b"\r\n".as_slice())
            || result.len().saturating_add(count) > LIMIT
        {
            return Err(failed());
        }
        result.extend_from_slice(&bytes[at..end]);
        at = end + 2;
    }
}
pub(super) fn decode(body: Vec<u8>, encoding: Option<&str>) -> Result<Vec<u8>> {
    let read = |decoder: &mut dyn Read| {
        let mut decoded = Vec::new();
        decoder
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut decoded)
            .map_err(|_| failed())?;
        if decoded.len() > LIMIT {
            Err(failed())
        } else {
            Ok(decoded)
        }
    };
    let encoding = encoding.map(str::to_ascii_lowercase);
    match encoding.as_deref() {
        Some("gzip" | "x-gzip" | "compress" | "x-compress") => {
            read(&mut flate2::read::MultiGzDecoder::new(body.as_slice()))
                .or_else(|_| read(&mut flate2::read::ZlibDecoder::new(body.as_slice())))
        }
        Some("deflate") => read(&mut flate2::read::ZlibDecoder::new(body.as_slice()))
            .or_else(|_| read(&mut flate2::read::DeflateDecoder::new(body.as_slice()))),
        Some("br") => read(&mut brotli::Decompressor::new(body.as_slice(), 4096)),
        Some("zstd") => {
            read(&mut zstd::stream::read::Decoder::new(body.as_slice()).map_err(|_| failed())?)
        }
        _ => Ok(body),
    }
}
pub(super) async fn get(
    target: &reqwest::Url,
    proxy: &reqwest::Url,
    retained_proxy: &reqwest::Url,
    extra_root: Option<&[u8]>,
) -> Result<Response> {
    let config = tls_config(extra_root)?;
    let tcp = tokio::net::TcpStream::connect((
        host(retained_proxy)?,
        retained_proxy.port_or_known_default().ok_or_else(failed)?,
    ))
    .await
    .map_err(|_| failed())?;
    let mut stream: Stream = Box::new(tcp);
    if retained_proxy.scheme() == "https" {
        stream = tls(stream, retained_proxy, config.clone()).await?;
    } else if retained_proxy.scheme() != "http" {
        return Err(failed());
    }
    let proxy_authority = authority(proxy)?;
    stream.write_all(format!("CONNECT {proxy_authority} HTTP/1.1\r\nHost: {proxy_authority}\r\n{}Connection: close\r\n\r\n", authorization(retained_proxy)).as_bytes()).await.map_err(|_| failed())?;
    let (status, _, leftover) = headers(&mut stream).await?;
    if status != 200 || !leftover.is_empty() {
        return Err(failed());
    }
    stream = tls(stream, proxy, config).await?;
    let mut request_url = target.clone();
    request_url.set_fragment(None);
    request_url.set_username("").map_err(|_| failed())?;
    request_url.set_password(None).map_err(|_| failed())?;
    let target_host = target.host_str().ok_or_else(failed)?;
    let target_authority = if let Some(port) = target.port() {
        format!("{target_host}:{port}")
    } else {
        target_host.into()
    };
    stream.write_all(format!("GET {request_url} HTTP/1.1\r\nHost: {target_authority}\r\n{}User-Agent: larksuiteoapi/node-sdk/1.72.0\r\nAccept: application/json, text/plain, */*\r\nAccept-Encoding: gzip, deflate, br\r\nConnection: close\r\n\r\n", authorization(proxy)).as_bytes()).await.map_err(|_| failed())?;
    let (status, fields, mut body) = headers(&mut stream).await?;
    let location = fields.get("location").cloned();
    if (300..400).contains(&status) && location.is_some() {
        return Ok(Response {
            status,
            location,
            body: Vec::new(),
        });
    }
    let mut buffer = [0u8; 16384];
    loop {
        let count = stream.read(&mut buffer).await.map_err(|_| failed())?;
        if count == 0 {
            break;
        }
        if body.len().saturating_add(count) > LIMIT {
            return Err(failed());
        }
        body.extend_from_slice(&buffer[..count]);
    }
    if fields
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        body = chunked(&body)?;
    } else if let Some(length) = fields.get("content-length") {
        let length: usize = length.parse().map_err(|_| failed())?;
        if body.len() < length {
            return Err(failed());
        }
        body.truncate(length);
    }
    let body = if status == 204 {
        Vec::new()
    } else {
        decode(body, fields.get("content-encoding").map(String::as_str))?
    };
    Ok(Response {
        status,
        location,
        body,
    })
}
