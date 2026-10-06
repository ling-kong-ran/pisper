//! Bounded immediate retries for idempotent management HTTP requests, from
//! `utils/management-http.ts`. Never use this for model/agent requests.
//! Returned bodies retain caller/overall/attempt cancellation deadlines.
use futures::{future::BoxFuture, stream::BoxStream, StreamExt};
use reqwest::{header::HeaderMap, Method};
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError {
    pub name: String,
    pub message: String,
    pub causes: Vec<String>,
}
impl FetchError {
    pub fn new(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message: message.into(),
            causes: vec![],
        }
    }
    pub fn aborted() -> Self {
        Self::new("AbortError", "This operation was aborted")
    }
    pub fn timed_out() -> Self {
        Self::new("TimeoutError", "The operation was aborted due to timeout")
    }
    fn transport(error: reqwest::Error) -> Self {
        let mut causes = vec![];
        let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        while let Some(error) = cause {
            let message = error.to_string();
            if !causes.contains(&message) {
                causes.push(message);
            }
            if causes.len() >= 4 {
                break;
            }
            cause = error.source();
        }
        Self {
            name: "TypeError".into(),
            message: "fetch failed".into(),
            causes,
        }
    }
}
impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for FetchError {}
#[derive(Debug, Clone)]
pub struct ManagementRequest {
    pub url: String,
    pub method: Method,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
    pub manual_redirect: bool,
    pub signal: Option<CancellationToken>,
}
impl ManagementRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: Method::GET,
            headers: HeaderMap::new(),
            body: None,
            manual_redirect: false,
            signal: None,
        }
    }
}
#[derive(Debug, Clone, Default)]
pub struct FetchRetryOptions {
    pub max_retries: Option<f64>,
    pub retry_on_status: Option<bool>,
    /// Typed native duration: JS's invalid timeout-number errors do not apply.
    pub timeout: Option<Duration>,
    pub attempt_timeout: Option<Duration>,
}
impl FetchRetryOptions {
    fn retries(&self) -> f64 {
        self.max_retries
            .filter(|v| v.is_finite())
            .map(|v| v.floor().max(0.0))
            .unwrap_or(2.0)
    }
}
pub type ResponseBody = BoxStream<'static, Result<Vec<u8>, FetchError>>;
pub type CancelBody = Box<dyn FnOnce() -> BoxFuture<'static, Result<(), FetchError>> + Send>;
pub struct RawResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Option<ResponseBody>,
    /// Optional transport cleanup hook. Errors discarding retry bodies are ignored.
    pub cancel_body: Option<CancelBody>,
}
impl RawResponse {
    pub async fn discard(mut self) -> Result<(), FetchError> {
        self.body.take();
        if let Some(cancel) = self.cancel_body.take() {
            cancel().await?;
        }
        Ok(())
    }
}
pub type FetchTransport = Arc<
    dyn Fn(ManagementRequest) -> BoxFuture<'static, Result<RawResponse, FetchError>> + Send + Sync,
>;

#[derive(Clone)]
struct RequestScope {
    parent: Option<CancellationToken>,
    overall: Option<Instant>,
    attempt: Option<Instant>,
}
impl RequestScope {
    fn parent_aborted(&self) -> bool {
        self.parent
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }
    fn overall_expired(&self) -> bool {
        self.overall.is_some_and(|at| Instant::now() >= at)
    }
    fn attempt_expired(&self) -> bool {
        self.attempt.is_some_and(|at| Instant::now() >= at)
    }
    fn check(&self) -> Result<(), FetchError> {
        if self.parent_aborted() {
            Err(FetchError::aborted())
        } else if self.overall_expired() || self.attempt_expired() {
            Err(FetchError::timed_out())
        } else {
            Ok(())
        }
    }
    async fn run<T>(&self, future: impl std::future::Future<Output = T>) -> Result<T, FetchError> {
        self.check()?;
        tokio::select! { biased;
            _=async { match &self.parent { Some(signal)=>signal.cancelled().await, None=>std::future::pending().await } }=>Err(FetchError::aborted()),
            _=wait_until(self.overall)=>Err(FetchError::timed_out()),
            _=wait_until(self.attempt)=>Err(FetchError::timed_out()),
            result=future=>Ok(result),
        }
    }
}
async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
fn deadline(duration: Option<Duration>) -> Option<Instant> {
    duration
        .filter(|d| !d.is_zero())
        .map(|d| Instant::now() + d)
}

pub struct ManagementResponse {
    raw: RawResponse,
    scope: RequestScope,
}
impl ManagementResponse {
    pub fn status(&self) -> u16 {
        self.raw.status
    }
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.raw.status)
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.raw.headers
    }
    pub fn has_body(&self) -> bool {
        self.raw.body.is_some()
    }
    pub async fn cancel_body(self) -> Result<(), FetchError> {
        self.raw.discard().await
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, FetchError> {
        match self.raw.body.as_mut() {
            Some(body) => self.scope.run(body.next()).await?.transpose(),
            None => Ok(None),
        }
    }
    pub async fn bytes(mut self) -> Result<Vec<u8>, FetchError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.next_chunk().await? {
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn json(self) -> Result<serde_json::Value, FetchError> {
        // Fetch's consume-body JSON path uses WHATWG UTF-8 decoding: replace
        // malformed bytes and remove one leading BOM before JSON.parse.
        // SyntaxError messages remain serde diagnostics, not V8-specific text;
        // serde_json::Value also cannot represent lone UTF-16 surrogates.
        let bytes = self.bytes().await?;
        let text = String::from_utf8_lossy(&bytes);
        // Upstream strips leading BOMs until none remain (the byte-level
        // oracle's "double-bom" case parses to {}): loop-strip, not a single
        // prefix strip.
        let text = text.trim_start_matches('\u{feff}');
        serde_json::from_str(text).map_err(|e| FetchError::new("SyntaxError", e.to_string()))
    }
}

pub fn native_transport() -> FetchTransport {
    static FOLLOW: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(20))
            .build()
            .expect("management HTTP client")
    });
    static MANUAL: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("management HTTP client")
    });
    Arc::new(|request| {
        Box::pin(async move {
            let client = if request.manual_redirect {
                &*MANUAL
            } else {
                &*FOLLOW
            };
            let head_only = request.method == Method::HEAD;
            let mut builder = client
                .request(request.method, &request.url)
                .headers(request.headers);
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let response = builder.send().await.map_err(FetchError::transport)?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let body = if head_only || matches!(status, 101 | 204 | 205 | 304) {
                None
            } else {
                Some(
                    response
                        .bytes_stream()
                        .map(|chunk| {
                            chunk
                                .map(|bytes| bytes.to_vec())
                                .map_err(FetchError::transport)
                        })
                        .boxed(),
                )
            };
            Ok(RawResponse {
                status,
                headers,
                body,
                cancel_body: None,
            })
        })
    })
}
pub async fn fetch_with_retry(
    request: ManagementRequest,
    options: FetchRetryOptions,
) -> Result<ManagementResponse, FetchError> {
    fetch_with_retry_using(request, options, &native_transport()).await
}
pub async fn fetch_with_retry_using(
    request: ManagementRequest,
    options: FetchRetryOptions,
    transport: &FetchTransport,
) -> Result<ManagementResponse, FetchError> {
    let base = RequestScope {
        parent: request.signal.clone(),
        overall: deadline(options.timeout),
        attempt: None,
    };
    let retries = options.retries();
    let mut attempt = 0.0;
    loop {
        base.check()?;
        let scope = RequestScope {
            attempt: deadline(options.attempt_timeout),
            ..base.clone()
        };
        let result = scope
            .run((transport)(request.clone()))
            .await
            .and_then(|value| value);
        match result {
            Ok(response) => {
                let retry = options.retry_on_status.unwrap_or(true)
                    && matches!(response.status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
                    && attempt < retries;
                if !retry {
                    return Ok(ManagementResponse {
                        raw: response,
                        scope,
                    });
                }
                let _ = response.discard().await;
            }
            Err(error) => {
                let attempt_timed_out =
                    scope.attempt_expired() && !base.parent_aborted() && !base.overall_expired();
                if base.parent_aborted()
                    || base.overall_expired()
                    || (error.name == "AbortError" && !attempt_timed_out && base.overall.is_none())
                    || attempt >= retries
                {
                    return Err(error);
                }
            }
        }
        attempt += 1.0;
    }
}
#[cfg(test)]
#[path = "management_http_tests.rs"]
mod tests;
