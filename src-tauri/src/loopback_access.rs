//! 桌面远程窗口的回环认证。回环地址并非权限边界：网页和同机进程都能向它发请求。
//! 引导令牌只使用一次；后续请求必须有端口独立的 HttpOnly Cookie、准确 Host 和同源来源。
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{header, Method, Request, Response, StatusCode};
use rand::RngCore;

pub(crate) const BOOTSTRAP_PATH: &str = "/_pisper/remote/bootstrap";

pub(crate) struct LoopbackAccess {
    authority: String,
    origin: String,
    cookie_name: String,
    token: String,
    bootstrapped: AtomicBool,
}

impl LoopbackAccess {
    pub(crate) fn new(port: u16) -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let authority = format!("127.0.0.1:{port}");
        Self {
            origin: format!("http://{authority}"),
            authority,
            // Cookie 不按端口隔离，必须为每个窗口使用不同名字。
            cookie_name: format!("__pisper_remote_{port}"),
            token: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
            bootstrapped: AtomicBool::new(false),
        }
    }

    pub(crate) fn bootstrap_url(&self) -> String {
        format!("{}{BOOTSTRAP_PATH}?token={}", self.origin, self.token)
    }

    /// Some 为已处理（拒绝或引导跳转），None 才允许代理接触任何上游。
    pub(crate) fn intercept<B>(&self, request: &Request<B>) -> Option<Response<Full<Bytes>>> {
        let headers = request.headers();
        if headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            != Some(&self.authority)
            || request
                .uri()
                .authority()
                .is_some_and(|value| value.as_str() != self.authority)
            || request
                .uri()
                .scheme_str()
                .is_some_and(|value| value != "http")
            || headers
                .get_all(header::ORIGIN)
                .iter()
                .any(|value| value != self.origin.as_str())
            || headers
                .get_all("sec-fetch-site")
                .iter()
                .any(|value| value != "same-origin" && value != "none")
        {
            return Some(denied(StatusCode::FORBIDDEN));
        }
        if request.uri().path() == BOOTSTRAP_PATH {
            let expected = format!("token={}", self.token);
            if request.method() != Method::GET
                || request.uri().query() != Some(expected.as_str())
                || self.bootstrapped.swap(true, Ordering::AcqRel)
            {
                return Some(denied(StatusCode::UNAUTHORIZED));
            }
            return Some(
                Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::LOCATION, "/")
                    .header(header::CACHE_CONTROL, "no-store")
                    .header("Referrer-Policy", "no-referrer")
                    .header(
                        header::SET_COOKIE,
                        format!(
                            "{}={}; HttpOnly; SameSite=Strict; Path=/",
                            self.cookie_name, self.token
                        ),
                    )
                    .body(Full::new(Bytes::new()))
                    .expect("static bootstrap response"),
            );
        }
        let expected = format!("{}={}", self.cookie_name, self.token);
        let authenticated = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .any(|value| value.trim() == expected);
        if !authenticated || !self.bootstrapped.load(Ordering::Acquire) {
            return Some(denied(StatusCode::UNAUTHORIZED));
        }
        // 私有引导地址和非 API 的写请求不能借代理取得本机 sidecar 权限。
        let path = request.uri().path();
        // reqwest 会规范化 dot segments 与反斜杠；转发前拒绝歧义路径，
        // 防止原本被判为前端的路径在本机请求中变成 /api/* 或私有控制入口。
        let canonical = tauri::Url::parse(&format!("{}{path}", self.origin)).is_ok_and(|url| {
            url.path() == path && url.origin().ascii_serialization() == self.origin
        });
        if !canonical
            || path.starts_with("/_pisper/")
            || (!path.starts_with("/api/")
                && request.method() != Method::GET
                && request.method() != Method::HEAD)
        {
            return Some(denied(StatusCode::FORBIDDEN));
        }
        None
    }
}

fn denied(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from_static(
            b"{\"error\":\"Remote workspace access denied.\"}",
        )))
        .expect("static denial response")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(access: &LoopbackAccess, path: &str) -> Request<()> {
        Request::builder()
            .uri(path)
            .header(header::HOST, &access.authority)
            .body(())
            .unwrap()
    }

    fn bootstrap(access: &LoopbackAccess) -> String {
        let response = access
            .intercept(&request(access, &access.bootstrap_url()))
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        cookie.split(';').next().unwrap().to_string()
    }

    #[test]
    fn one_time_bootstrap_and_authenticated_requests() {
        let access = LoopbackAccess::new(5132);
        assert_eq!(
            access
                .intercept(&request(&access, "/api/sessions"))
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let cookie = bootstrap(&access);
        let mut authenticated = request(&access, "/api/sessions");
        authenticated
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        assert!(access.intercept(&authenticated).is_none());
        assert_eq!(
            access
                .intercept(&request(&access, &access.bootstrap_url()))
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn rejected_bootstrap_does_not_consume_token() {
        let access = LoopbackAccess::new(5133);
        let invalid = request(&access, &format!("{BOOTSTRAP_PATH}?token=wrong"));
        assert_eq!(
            access.intercept(&invalid).unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        bootstrap(&access);
    }

    #[test]
    fn rejects_dns_rebinding_cross_site_and_foreign_origin_even_with_cookie() {
        let access = LoopbackAccess::new(5134);
        let cookie = bootstrap(&access);
        for (name, value) in [
            ("host", "evil.test:5134"),
            ("origin", "https://evil.test"),
            ("origin", "null"),
            ("origin", "http://127.0.0.1:5135"),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-site", "cross-site"),
        ] {
            let mut req = request(&access, "/api/chat");
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            req.headers_mut().insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
            assert_eq!(
                access.intercept(&req).unwrap().status(),
                StatusCode::FORBIDDEN,
                "{name}: {value}"
            );
        }
    }

    #[test]
    fn cookies_are_window_scoped_and_local_control_routes_are_blocked() {
        let first = LoopbackAccess::new(5135);
        let second = LoopbackAccess::new(5136);
        let cookie = bootstrap(&first);
        bootstrap(&second);
        let mut req = request(&second, "/api/chat");
        req.headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        assert_eq!(
            second.intercept(&req).unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        for path in [
            "/_pisper/desktop/bootstrap?token=wrong",
            "/_pisper/shutdown",
            "/assets/../api/config",
            "/assets/%2e%2e/api/config",
            "/%2e/_pisper/shutdown",
            "/api/../_pisper/shutdown",
            r"/assets\..\api/config",
        ] {
            let mut req = request(&first, path);
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            assert_eq!(
                first.intercept(&req).unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        let mut req = request(&first, "/assets/module.js");
        req.headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        *req.method_mut() = Method::POST;
        assert_eq!(
            first.intercept(&req).unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
}
