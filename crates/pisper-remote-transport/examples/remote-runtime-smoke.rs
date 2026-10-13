//! 独立移动端模拟器：从 PNG 识别二维码，通过产品共用的 Iroh/TLS 实现访问真实 Runtime。
use base64::Engine;
use futures_util::StreamExt;
use pisper_remote_transport::{
    iroh_tunnel::{production_relay_mode, TunnelClient, TunnelEndpoint},
    pinning::pinned_client,
    RelayMode, SecretKey,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use std::{error::Error, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing {key}").into())
}

async fn local(
    client: &Client,
    config: &Value,
    method: Method,
    path: &str,
    body: Value,
) -> Result<Value> {
    let base = text(config, "base")?;
    assert!(base.starts_with("http://127.0.0.1:"));
    let response = client
        .request(method, format!("{base}{path}"))
        .header("Origin", base)
        .header(
            "Cookie",
            format!("__pisper_desktop={}", text(config, "cookie")?),
        )
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("local {path}: {}", response.status()).into());
    }
    if response.status() == StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    Ok(response.json().await?)
}

async fn code(client: &Client, config: &Value) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let info = local(
            client,
            config,
            Method::GET,
            "/api/remote/connection-info",
            Value::Null,
        )
        .await?;
        if info["endpoints"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["t"] == "iroh"))
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Iroh endpoint did not become ready".into());
        }
        tokio::task::yield_now().await;
    }
    let response = local(
        client,
        config,
        Method::POST,
        "/api/remote/pairing-code",
        json!({}),
    )
    .await?;
    let png = text(&response, "qrDataUrl")?
        .strip_prefix("data:image/png;base64,")
        .ok_or("invalid QR MIME")?;
    let pixels = image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(png)?)?
        .to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(pixels);
    let grids = prepared.detect_grids();
    assert_eq!(grids.len(), 1, "QR must be machine readable");
    let (_, payload) = grids[0].decode()?;
    let payload: Value = serde_json::from_str(&payload)?;
    assert_eq!(payload["v"], 1);
    assert_eq!(payload["code"], response["code"]);
    Ok(payload)
}

fn endpoint(payload: &Value, public: bool) -> Result<TunnelEndpoint> {
    let endpoints = payload["endpoints"].as_array().ok_or("missing endpoints")?;
    if public {
        assert!(
            endpoints.iter().all(|item| item["t"] == "iroh"),
            "relay-only QR must not expose LAN fallback"
        );
    }
    let value = endpoints
        .iter()
        .find(|item| item["t"] == "iroh")
        .ok_or("missing Iroh endpoint")?;
    let remote: TunnelEndpoint = serde_json::from_value(value.clone())?;
    if public {
        assert!(
            remote.direct_addresses.is_empty(),
            "IP transports must be disabled on PC"
        );
        assert!(remote
            .relay_url
            .as_ref()
            .is_some_and(|url| url.starts_with("https://")));
    }
    Ok(remote)
}

async fn mobile(public: bool) -> Result<TunnelClient> {
    let key = SecretKey::from_bytes(&rand::random::<[u8; 32]>());
    Ok(if public {
        TunnelClient::start_relay_only(key, production_relay_mode()).await?
    } else {
        TunnelClient::start(key, RelayMode::Disabled).await?
    })
}

fn passed(name: &str) {
    println!("{}", json!({"check":name}));
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut config: Value =
        serde_json::from_str(&input.next_line().await?.ok_or("missing fixture config")?)?;
    let public = config["public"] == true;
    let control = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(60))
        .build()?;
    local(
        &control,
        &config,
        Method::PUT,
        "/api/remote/enabled",
        json!({"enabled":true,"configureFirewall":false}),
    )
    .await?;
    let qr = code(&control, &config).await?;
    let remote = endpoint(&qr, public)?;
    let fingerprint = text(&qr, "fp")?
        .strip_prefix("SHA256:")
        .ok_or("missing full fingerprint")?;
    assert_eq!(fingerprint.len(), 64);
    let transport = mobile(public).await?;
    let bridge = transport.open_bridge(remote.clone()).await?;
    let base = bridge.url();
    let client = pinned_client(fingerprint)?;
    let bad_client = pinned_client(&"00".repeat(32))?;
    let rejected_certificate = bad_client
        .get(format!("{base}/api/remote/status"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .expect_err("wrong certificate fingerprint accepted");
    assert!(
        format!("{rejected_certificate:?}").contains("certificate fingerprint mismatch"),
        "certificate rejection must not be a network timeout"
    );
    passed("PNG QR decoded; mismatched TLS certificate rejected");
    let pair_body = json!({"code":qr["code"],"deviceName":"Simulated mobile 公网"});
    let paired = client
        .post(format!("{base}/api/remote/pair"))
        .json(&pair_body)
        .send()
        .await?;
    assert_eq!(paired.status(), StatusCode::CREATED);
    let paired: Value = paired.json().await?;
    let token = text(&paired, "token")?;
    let device = text(&paired, "deviceId")?;
    assert_eq!(
        client
            .post(format!("{base}/api/remote/pair"))
            .json(&pair_body)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .get(format!("{base}/api/sessions"))
            .bearer_auth("wrong-fixture-token")
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(format!("{base}/api/remote/devices"))
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(format!("{base}/api/remote/pairing-requests"))
            .json(&json!({"deviceName":"not LAN"}))
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    passed("one-time pairing, device authentication and local management boundary");
    let session = client
        .post(format!("{base}/api/sessions"))
        .bearer_auth(token)
        .json(&json!({"name":"P2P 公网测试"}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let id = text(&session, "id")?;
    let response = client
        .post(format!("{base}/api/chat"))
        .bearer_auth(token)
        .json(&json!({"sessionId":id,"message":"P2P fixture request"}))
        .send()
        .await?
        .error_for_status()?;
    assert!(response.headers()["content-type"]
        .to_str()?
        .starts_with("text/event-stream"));
    let mut stream = response.bytes_stream();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(chunk) = stream.next().await {
            raw.extend(chunk?);
        }
        Ok::<(), reqwest::Error>(())
    })
    .await??;
    let raw = String::from_utf8(raw)?;
    assert!(
        raw.contains("P2P_中文_OK"),
        "model output missing in remote SSE"
    );
    let mut frames = 0;
    for line in raw
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
    {
        let _: Value = serde_json::from_str(line)?;
        frames += 1;
    }
    assert!(frames > 2);
    let local_sessions =
        local(&control, &config, Method::GET, "/api/sessions", Value::Null).await?;
    assert!(
        local_sessions.to_string().contains(id),
        "remote task not visible on PC"
    );
    passed("remote task created on PC; model reply and strict UTF-8/JSON SSE received");
    let pending_qr = code(&control, &config).await?;
    drop(stream);
    drop(client);
    drop(bridge);
    transport.close().await;
    println!("{}", json!({"phase":"restart"}));
    config = serde_json::from_str(
        &input
            .next_line()
            .await?
            .ok_or("missing restarted fixture config")?,
    )?;
    let info = local(
        &control,
        &config,
        Method::GET,
        "/api/remote/connection-info",
        Value::Null,
    )
    .await?;
    let restarted = endpoint(&info, public)?;
    assert_eq!(restarted.node_id, remote.node_id);
    assert_eq!(info["fingerprint"], qr["fp"]);
    let transport = mobile(public).await?;
    let bridge = transport.open_bridge(restarted).await?;
    let base = bridge.url();
    let client = pinned_client(fingerprint)?;
    assert_eq!(
        client
            .post(format!("{base}/api/remote/pair"))
            .json(&json!({"code":pending_qr["code"],"deviceName":"Restart pairing"}))
            .send()
            .await?
            .status(),
        StatusCode::CREATED
    );
    passed("unused QR survives PC restart in the original pairing file");
    assert!(client
        .get(format!("{base}/api/sessions"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?
        .contains(id));
    passed("PC and mobile restart: persisted identity, certificate, device and task reconnect");
    let response = client
        .get(format!("{base}/api/sessions/{id}/live"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?;
    let mut live = response.bytes_stream();
    assert!(tokio::time::timeout(Duration::from_secs(15), live.next())
        .await?
        .ok_or("live stream ended early")?
        .is_ok());
    local(
        &control,
        &config,
        Method::POST,
        &format!("/api/remote/devices/{device}/revoke"),
        json!({}),
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(chunk) = live.next().await {
            chunk?;
        }
        Ok::<(), reqwest::Error>(())
    })
    .await??;
    assert_eq!(
        client
            .get(format!("{base}/api/sessions"))
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    passed("revocation terminates active SSE and rejects subsequent requests");
    local(
        &control,
        &config,
        Method::PUT,
        "/api/remote/enabled",
        json!({"enabled":false,"configureFirewall":false}),
    )
    .await?;
    assert!(client
        .get(format!("{base}/api/sessions"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .is_err());
    drop(live);
    drop(client);
    drop(bridge);
    transport.close().await;
    passed("disabling remote access closes transport");
    println!(
        "{}",
        json!({"status":"passed","mode":if public {"public-relay-only"} else {"offline-direct"}})
    );
    Ok(())
}
