use super::*;
use futures::{stream, FutureExt};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

fn response(status: u16) -> RawResponse {
    RawResponse {
        status,
        headers: HeaderMap::new(),
        body: Some(stream::empty().boxed()),
        cancel_body: None,
    }
}
#[tokio::test]
async fn upstream_retry_oracle() {
    let fixture: Value = serde_json::from_str(include_str!("management_http_oracle.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let token = CancellationToken::new();
        if case["preAbort"] == true {
            token.cancel();
        }
        let trace = Arc::new(Mutex::new(Vec::<Value>::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let steps = case["steps"].as_array().unwrap().clone();
        let transport: FetchTransport = {
            let token = token.clone();
            let trace = trace.clone();
            let calls = calls.clone();
            Arc::new(move |request| {
                let index = calls.fetch_add(1, Ordering::SeqCst);
                let step = steps[index.min(steps.len() - 1)].clone();
                let token = token.clone();
                let trace = trace.clone();
                async move {
                    trace.lock().unwrap().push(json!(["fetch", index]));
                    assert_eq!(request.url, "https://example.invalid");
                    if step["abortParent"] == true {
                        token.cancel();
                        return Err(FetchError::aborted());
                    }
                    if let Some(name) = step["error"].as_str() {
                        return Err(FetchError::new(name, step["message"].as_str().unwrap()));
                    }
                    let mut result = response(step["status"].as_u64().unwrap() as u16);
                    if step["noBody"] == true {
                        result.body = None;
                    } else {
                        result.cancel_body = Some(Box::new(move || {
                            async move {
                                trace.lock().unwrap().push(json!(["cancel", index]));
                                if step["cancelParent"] == true {
                                    token.cancel();
                                }
                                if step["cancelError"] == true {
                                    Err(FetchError::new("Error", "cannot cancel"))
                                } else {
                                    Ok(())
                                }
                            }
                            .boxed()
                        }));
                    }
                    Ok(result)
                }
                .boxed()
            })
        };
        let number = |value: &Value| {
            value.as_f64().or_else(|| {
                value.as_str().and_then(|s| match s {
                    "NaN" => Some(f64::NAN),
                    "Infinity" => Some(f64::INFINITY),
                    "-Infinity" => Some(f64::NEG_INFINITY),
                    _ => None,
                })
            })
        };
        let options = FetchRetryOptions {
            max_retries: number(&case["options"]["maxRetries"]),
            retry_on_status: case["options"]["retryOnStatus"].as_bool(),
            timeout: case["options"]["timeoutMs"]
                .as_u64()
                .map(Duration::from_millis),
            attempt_timeout: case["options"]["attemptTimeoutMs"]
                .as_u64()
                .map(Duration::from_millis),
        };
        let mut request = ManagementRequest::get("https://example.invalid");
        request.signal = Some(token);
        let outcome = match fetch_with_retry_using(request, options, &transport).await {
            Ok(result) => json!({"status":result.status()}),
            Err(error) => json!({"error":{"name":error.name,"message":error.message}}),
        };
        assert_eq!(outcome, case["outcome"], "{}", case["id"]);
        assert_eq!(
            *trace.lock().unwrap(),
            *case["trace"].as_array().unwrap(),
            "{}",
            case["id"]
        );
    }
}
#[tokio::test(start_paused = true)]
async fn per_attempt_timeout_restarts_and_overall_budget_does_not() {
    let calls = Arc::new(AtomicUsize::new(0));
    let transport: FetchTransport = {
        let calls = calls.clone();
        Arc::new(move |_| {
            let index = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if index == 0 {
                    std::future::pending::<()>().await;
                }
                Ok(response(200))
            }
            .boxed()
        })
    };
    let options = FetchRetryOptions {
        attempt_timeout: Some(Duration::from_secs(4)),
        ..Default::default()
    };
    let start = Instant::now();
    assert_eq!(
        fetch_with_retry_using(ManagementRequest::get("unused"), options, &transport)
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(start.elapsed(), Duration::from_secs(4));
    let calls = Arc::new(AtomicUsize::new(0));
    let transport: FetchTransport = {
        let calls = calls.clone();
        Arc::new(move |_| {
            let index = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if index == 0 {
                    Err(FetchError::new("TypeError", "fetch failed"))
                } else {
                    Ok(response(200))
                }
            }
            .boxed()
        })
    };
    let options = FetchRetryOptions {
        timeout: Some(Duration::from_secs(5)),
        attempt_timeout: Some(Duration::from_secs(4)),
        ..Default::default()
    };
    let start = Instant::now();
    let error = fetch_with_retry_using(ManagementRequest::get("unused"), options, &transport)
        .await
        .err()
        .unwrap();
    assert_eq!(error, FetchError::timed_out());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(start.elapsed(), Duration::from_secs(5));
}
#[tokio::test(start_paused = true)]
async fn response_body_keeps_attempt_overall_and_parent_cancellation() {
    for kind in ["attempt", "overall", "parent"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let transport: FetchTransport = {
            let calls = calls.clone();
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async {
                    let mut result = response(200);
                    result.body = Some(
                        stream::iter([Ok(vec![1, 2, 3])])
                            .chain(stream::pending())
                            .boxed(),
                    );
                    Ok(result)
                }
                .boxed()
            })
        };
        let token = CancellationToken::new();
        let mut request = ManagementRequest::get("unused");
        request.signal = Some(token.clone());
        let options = FetchRetryOptions {
            timeout: (kind == "overall").then_some(Duration::from_secs(5)),
            attempt_timeout: (kind == "attempt").then_some(Duration::from_secs(5)),
            ..Default::default()
        };
        let mut body = fetch_with_retry_using(request, options, &transport)
            .await
            .unwrap();
        assert_eq!(body.next_chunk().await.unwrap(), Some(vec![1, 2, 3]));
        if kind == "parent" {
            token.cancel();
        }
        let error = body.next_chunk().await.unwrap_err();
        assert_eq!(
            error,
            if kind == "parent" {
                FetchError::aborted()
            } else {
                FetchError::timed_out()
            }
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "body failures must not retry the request"
        );
    }
}
#[tokio::test(start_paused = true)]
async fn exhausted_attempt_timeouts_drop_every_inflight_future() {
    struct Dropped(Arc<AtomicUsize>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let transport: FetchTransport = {
        let drops = drops.clone();
        Arc::new(move |_| {
            let guard = Dropped(drops.clone());
            async move {
                let _guard = guard;
                std::future::pending().await
            }
            .boxed()
        })
    };
    let error = fetch_with_retry_using(
        ManagementRequest::get("unused"),
        FetchRetryOptions {
            attempt_timeout: Some(Duration::from_secs(1)),
            ..Default::default()
        },
        &transport,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error, FetchError::timed_out());
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn native_http_manual_redirect_headers_and_body() {
    use wiremock::{
        matchers::{body_bytes, header, method, path},
        Mock, MockServer, ResponseTemplate,
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/manual"))
        .and(header("x-port-test", "yes"))
        .and(body_bytes(b"payload".as_slice()))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "/target")
                .set_body_bytes(b"redirect body"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/target"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/follow"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/target"))
        .mount(&server)
        .await;
    let mut request = ManagementRequest::get(format!("{}/manual", server.uri()));
    request.method = Method::POST;
    request.body = Some(b"payload".to_vec());
    request.manual_redirect = true;
    request
        .headers
        .insert("x-port-test", "yes".parse().unwrap());
    let result = fetch_with_retry(request, Default::default()).await.unwrap();
    assert_eq!(result.status(), 302);
    assert_eq!(result.headers()["location"], "/target");
    assert_eq!(result.bytes().await.unwrap(), b"redirect body");
    let result = fetch_with_retry(
        ManagementRequest::get(format!("{}/follow", server.uri())),
        Default::default(),
    )
    .await
    .unwrap();
    assert!(result.is_success());
    assert_eq!(result.json().await.unwrap(), json!({"ok":true}));
    server.verify().await;
}
#[tokio::test]
async fn null_body_and_transport_body_errors_are_not_retried() {
    let transport: FetchTransport = Arc::new(|_| {
        async {
            let mut result = response(204);
            result.body = None;
            Ok(result)
        }
        .boxed()
    });
    let token = CancellationToken::new();
    let mut request = ManagementRequest::get("unused");
    request.signal = Some(token.clone());
    let result = fetch_with_retry_using(request, Default::default(), &transport)
        .await
        .unwrap();
    token.cancel();
    assert!(!result.has_body());
    assert_eq!(result.bytes().await.unwrap(), Vec::<u8>::new());
    let transport: FetchTransport = Arc::new(|_| {
        async {
            let mut result = response(200);
            result.body =
                Some(stream::iter([Err(FetchError::new("TypeError", "terminated"))]).boxed());
            Ok(result)
        }
        .boxed()
    });
    let result = fetch_with_retry_using(
        ManagementRequest::get("unused"),
        Default::default(),
        &transport,
    )
    .await
    .unwrap();
    assert_eq!(
        result.bytes().await.unwrap_err(),
        FetchError::new("TypeError", "terminated")
    );
}

#[tokio::test]
async fn response_json_uses_fetch_utf8_and_bom_semantics_for_every_chunk_boundary() {
    let fixture: Value = serde_json::from_str(include_str!("management_http_oracle.json")).unwrap();
    for case in fixture["responseJson"].as_array().unwrap() {
        let bytes: Vec<u8> = case["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        for split in 0..=bytes.len() {
            let chunks = vec![bytes[..split].to_vec(), bytes[split..].to_vec()];
            let transport: FetchTransport = Arc::new(move |_| {
                let chunks = chunks.clone();
                async move {
                    Ok(RawResponse {
                        status: 200,
                        headers: HeaderMap::new(),
                        body: Some(stream::iter(chunks.into_iter().map(Ok)).boxed()),
                        cancel_body: None,
                    })
                }
                .boxed()
            });
            let result = fetch_with_retry_using(
                ManagementRequest::get("unused"),
                Default::default(),
                &transport,
            )
            .await
            .unwrap();
            let outcome = match result.json().await {
                Ok(value) => json!({"value": value}),
                Err(error) => json!({"error": {"name": error.name}}),
            };
            assert_eq!(outcome, case["outcome"], "{} at split {split}", case["id"]);
        }
    }
}
