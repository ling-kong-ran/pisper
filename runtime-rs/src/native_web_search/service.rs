use super::{
    config::{
        bounded_integer, js_string_checked, js_whitespace, slice_utf16, truthy,
        validate_js_conversion,
    },
    parse_bing_rss_results,
    rss::{plain_text, result_text},
    SearchResult, WebSearchConfig,
};
use reqwest::{Client, Url};
use serde_json::Value;
use std::{
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const BING_RSS_URL: &str = "https://www.bing.com/search";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError {
    pub message: String,
    pub cancelled: bool,
}
impl SearchError {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
        }
    }
    pub(super) fn cancelled() -> Self {
        Self {
            message: "This operation was aborted".into(),
            cancelled: true,
        }
    }
    fn timeout() -> Self {
        Self::invalid("Bing 搜索请求超时，请检查网络后重试。")
    }
    fn body_timeout() -> Self {
        Self::invalid("The operation was aborted due to timeout")
    }
}
impl std::fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for SearchError {}

pub struct WebSearchService {
    config_path: PathBuf,
    client: Client,
    shutdown: CancellationToken,
    #[cfg(test)]
    endpoint: Url,
    #[cfg(test)]
    timeout: Duration,
}
impl WebSearchService {
    pub fn new(config_path: impl Into<PathBuf>) -> Result<Arc<Self>, SearchError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(20))
            .no_deflate()
            .build()
            .map_err(network_error)?;
        Ok(Arc::new(Self {
            config_path: config_path.into(),
            client,
            shutdown: CancellationToken::new(),
            #[cfg(test)]
            endpoint: Url::parse(BING_RSS_URL).expect("Bing URL"),
            #[cfg(test)]
            timeout: SEARCH_TIMEOUT,
        }))
    }

    pub async fn get_config(&self) -> Result<WebSearchConfig, SearchError> {
        let app: Value = match tokio::fs::read(&self.config_path).await {
            Ok(bytes) => super::parse_config_json(&String::from_utf8_lossy(&bytes))
                .map_err(|error| SearchError::invalid(format!("网页搜索配置无法解析：{error}")))?,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                serde_json::json!({})
            }
            Err(error) => {
                return Err(SearchError::invalid(format!(
                    "网页搜索配置无法读取：{error}"
                )));
            }
        };
        if app.is_null() {
            return Err(SearchError::invalid(
                "Cannot read properties of null (reading 'webSearch')",
            ));
        }
        let config = app
            .get("webSearch")
            .filter(|value| truthy(value))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        WebSearchConfig::try_normalize(&config)
    }

    pub async fn search(
        &self,
        input: &Value,
        config: Option<&Value>,
        cancellation: CancellationToken,
    ) -> Result<SearchResult, SearchError> {
        let settings = match config.filter(|value| truthy(value)) {
            Some(config) => WebSearchConfig::try_normalize(config)?,
            None => self.get_config().await?,
        };
        let query = input
            .get("query")
            .filter(|value| truthy(value))
            .map(js_string_checked)
            .transpose()?
            .unwrap_or_default();
        let query = slice_utf16(query.trim_matches(js_whitespace), 500);
        if query.is_empty() {
            return Err(SearchError::invalid("搜索关键词不能为空。"));
        }
        for field in ["limit", "page"] {
            if let Some(value) = input.get(field) {
                validate_js_conversion(value)?;
            }
        }
        let limit = bounded_integer(input.get("limit"), settings.max_results as usize, 1, 12);
        let page = bounded_integer(input.get("page"), 1, 1, 20);
        let language = input
            .get("language")
            .filter(|value| truthy(value))
            .map(js_string_checked)
            .transpose()?
            .unwrap_or_else(|| settings.language.clone());
        let mut endpoint = self.endpoint();
        {
            let mut params = endpoint.query_pairs_mut();
            params
                .append_pair("q", &query)
                .append_pair("format", "rss")
                .append_pair("count", &limit.to_string())
                .append_pair(
                    "adlt",
                    ["off", "moderate", "strict"][settings.safe_search as usize],
                );
            if page > 1 {
                params.append_pair("first", &((page - 1) * limit + 1).to_string());
            }
            let market = match language.as_str() {
                "zh-CN" => Some(("zh-CN", "zh")),
                "zh-TW" => Some(("zh-TW", "zh-Hant")),
                "en-US" => Some(("en-US", "en")),
                "en-GB" => Some(("en-GB", "en")),
                "ja-JP" => Some(("ja-JP", "ja")),
                "ko-KR" => Some(("ko-KR", "ko")),
                // MARKETS 是 release 的普通 JS 对象；原型上的合法名称也保持其查询参数语义。
                "constructor"
                | "__defineGetter__"
                | "__defineSetter__"
                | "hasOwnProperty"
                | "__lookupGetter__"
                | "__lookupSetter__"
                | "isPrototypeOf"
                | "propertyIsEnumerable"
                | "toString"
                | "valueOf"
                | "__proto__"
                | "toLocaleString" => Some(("undefined", "undefined")),
                _ => None,
            };
            if let Some((market, lang)) = market {
                params
                    .append_pair("mkt", market)
                    .append_pair("setlang", lang);
            }
        }
        let received_headers = AtomicBool::new(false);
        let deadline = Instant::now() + self.timeout();
        let accept_encoding = if endpoint.scheme() == "https" {
            "br, gzip, deflate"
        } else {
            "gzip, deflate"
        };
        let operation = async {
            let response = self
                .client
                .get(endpoint)
                .header("Accept", "application/rss+xml,application/xml,text/xml")
                .header("User-Agent", "Pisper Web Search/1.0")
                .header("Accept-Encoding", accept_encoding)
                .header("Accept-Language", "*")
                .header("Sec-Fetch-Mode", "cors")
                .send()
                .await
                .map_err(network_error)?;
            let status = response.status();
            received_headers.store(true, Ordering::Relaxed);
            let deflate = response
                .headers()
                .get("content-encoding")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("deflate"));
            // release 只对 fetch 阶段封装中文网络错误；response.text 的错误保持原始 fetch 语义。
            let bytes = response
                .bytes()
                .await
                .map_err(|_| SearchError::invalid("terminated"))?;
            let decoded;
            let bytes = if deflate {
                decoded = decode_deflate(&bytes, deadline, &cancellation, &self.shutdown)?;
                decoded.as_slice()
            } else {
                bytes.as_ref()
            };
            // Fetch Response.text always decodes UTF-8, ignores an XML/HTTP
            // charset declaration and strips only the leading UTF-8 BOM.
            let body =
                String::from_utf8_lossy(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes));
            if !status.is_success() {
                let message = plain_text(&body, 300)?;
                return Err(SearchError::invalid(format!(
                    "Bing 搜索失败（HTTP {}）：{}",
                    status.as_u16(),
                    if message.is_empty() {
                        "无响应内容"
                    } else {
                        &message
                    }
                )));
            }
            if !regex::Regex::new(r"(?i-u:<rss)(?:[^A-Za-z0-9_]|$)")
                .expect("RSS marker")
                .is_match(&body)
            {
                return Err(SearchError::invalid(
                    "Bing 没有返回可解析的 RSS 搜索结果，请稍后重试。",
                ));
            }
            let results = parse_bing_rss_results(&body, limit)?;
            Ok(SearchResult {
                text: result_text(&query, &results),
                query,
                provider: "bing".into(),
                results,
            })
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(SearchError::cancelled()),
            _ = self.shutdown.cancelled() => Err(SearchError::cancelled()),
            result = tokio::time::timeout(self.timeout(),operation) => result.unwrap_or_else(|_| Err(if received_headers.load(Ordering::Relaxed) { SearchError::body_timeout() } else { SearchError::timeout() })),
        }
    }

    pub async fn test(
        &self,
        config: &Value,
        cancellation: CancellationToken,
    ) -> Result<SearchResult, SearchError> {
        self.search(
            &serde_json::json!({"query":"Pisper AI agent","limit":3}),
            Some(config),
            cancellation,
        )
        .await
    }
    pub fn dispose(&self) {
        self.shutdown.cancel();
    }

    fn endpoint(&self) -> Url {
        #[cfg(test)]
        {
            self.endpoint.clone()
        }
        #[cfg(not(test))]
        {
            Url::parse(BING_RSS_URL).expect("Bing URL")
        }
    }
    fn timeout(&self) -> Duration {
        #[cfg(test)]
        {
            self.timeout
        }
        #[cfg(not(test))]
        {
            SEARCH_TIMEOUT
        }
    }

    // 仅 Rust 测试可更换服务端点；发布二进制没有环境变量或 HTTP 参数覆盖入口。
    #[cfg(test)]
    pub(super) fn fixture(config_path: PathBuf, endpoint: Url, timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            config_path,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::limited(20))
                .no_deflate()
                .build()
                .unwrap(),
            shutdown: CancellationToken::new(),
            endpoint,
            timeout,
        })
    }
}

fn decode_deflate(
    bytes: &[u8],
    deadline: Instant,
    cancellation: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<Vec<u8>, SearchError> {
    // Node fetch's InflateStream selects zlib versus raw DEFLATE from the
    // compression-method nibble of the first byte; reqwest supports zlib only.
    let mut reader: Box<dyn Read + '_> = if bytes.first().is_some_and(|byte| byte & 0x0f == 8) {
        Box::new(flate2::read::ZlibDecoder::new(bytes))
    } else {
        Box::new(flate2::read::DeflateDecoder::new(bytes))
    };
    let mut output = Vec::new();
    let mut chunk = [0; 16 * 1024];
    loop {
        if cancellation.is_cancelled() || shutdown.is_cancelled() {
            return Err(SearchError::cancelled());
        }
        if Instant::now() >= deadline {
            return Err(SearchError::body_timeout());
        }
        let count = reader
            .read(&mut chunk)
            .map_err(|_| SearchError::invalid("terminated"))?;
        if count == 0 {
            return Ok(output);
        }
        output.extend_from_slice(&chunk[..count]);
    }
}

fn network_error(error: reqwest::Error) -> SearchError {
    if error.is_timeout() {
        return SearchError::timeout();
    }
    // Node fetch 对 DNS、TLS、连接及重定向失败统一给出此公开文本，不能外发请求 URL。
    SearchError::invalid("无法连接 Bing：fetch failed")
}
