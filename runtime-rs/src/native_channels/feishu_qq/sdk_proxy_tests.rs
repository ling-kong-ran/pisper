use super::*;
use std::io::{BufRead, BufReader, Write};
fn oracle() -> Value {
    serde_json::from_str(include_str!("sdk-proxy-oracle.json")).unwrap()
}
fn substitutions(endpoints: &Value) -> HashMap<String, String> {
    let mut values: HashMap<_, _> = endpoints
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
        .collect();
    for (key, value) in endpoints.as_object().unwrap() {
        values.insert(
            format!("{key}_PORT"),
            reqwest::Url::parse(value.as_str().unwrap())
                .unwrap()
                .port()
                .unwrap()
                .to_string(),
        );
    }
    for (key, source, host) in [
        ("HTTP_ORIGIN_DNS", "HTTP_ORIGIN", "localhost"),
        ("HTTPS_ORIGIN_DNS", "HTTPS_ORIGIN", "localhost"),
        ("HTTP_REDIRECT_DNS", "HTTP_ORIGIN", "redirect.invalid"),
        ("HTTPS_REDIRECT_DNS", "HTTPS_ORIGIN", "redirect.invalid"),
        ("HTTP_PROXY_DNS", "HTTP_PROXY", "localhost"),
    ] {
        values.insert(key.into(), values[source].replace("127.0.0.1", host));
    }
    values.insert(
        "HTTP_PROXY_AUTHORITY".into(),
        values["HTTP_PROXY"].trim_start_matches("http://").into(),
    );
    values.insert(
        "HTTP_PROXY_AUTH".into(),
        values["HTTP_PROXY"].replace("http://", "http://synthetic:proxy-only@"),
    );
    values.insert(
        "OWNED_CA_CERT".into(),
        if cfg!(windows) {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/native_channels/feishu_qq/proxy-test-ca.pem"
            )
            .replace('/', "\\")
        } else {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/native_channels/feishu_qq/proxy-test-ca.pem"
            )
            .into()
        },
    );
    values.insert(
        "HTTP_PROXY_AUTH_ENCODED".into(),
        values["HTTP_PROXY"].replace("http://", "http://synthetic%40user:proxy%3Aonly@"),
    );
    values.insert(
        "HTTP_PROXY_PASSWORD_ONLY".into(),
        values["HTTP_PROXY"].replace("http://", "http://:synthetic-only@"),
    );
    values
}
fn expand(value: &str, values: &HashMap<String, String>) -> String {
    let mut expanded = value.to_owned();
    for (key, value) in values {
        expanded = expanded.replace(&format!("@{key}@"), value);
    }
    expanded
}
fn expanded_json(value: &Value, values: &HashMap<String, String>) -> Value {
    match value {
        Value::String(value) => json!(expand(value, values)),
        Value::Array(value) => json!(value
            .iter()
            .map(|value| expanded_json(value, values))
            .collect::<Vec<_>>()),
        Value::Object(value) => Value::Object(
            value
                .iter()
                .map(|(key, value)| (key.clone(), expanded_json(value, values)))
                .collect(),
        ),
        value => value.clone(),
    }
}
fn environment(case: &Value, values: &HashMap<String, String>) -> media_proxy::Environment {
    case["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), expand(value.as_str().unwrap(), values)))
        .collect()
}
#[test]
fn actual_sdk_axios_effective_proxy_and_no_proxy_selection_matches() {
    let oracle = oracle();
    assert_eq!(oracle["sdkVersion"], "1.72.0");
    assert_eq!(oracle["axiosVersion"], "1.19.0");
    assert_eq!(oracle["proxyFromEnvVersion"], "2.1.0");
    let endpoints = json!({"HTTP_ORIGIN":"http://127.0.0.1:19001","HTTPS_ORIGIN":"https://127.0.0.1:19002","HTTP_PROXY":"http://127.0.0.1:19003","HTTP_PROXY_2":"http://127.0.0.1:19004","HTTPS_PROXY":"https://127.0.0.1:19005"});
    let values = substitutions(&endpoints);
    for case in oracle["cases"].as_array().unwrap() {
        let env = environment(case, &values);
        for candidate in case["candidates"].as_array().unwrap() {
            let url =
                reqwest::Url::parse(&expand(candidate["url"].as_str().unwrap(), &values)).unwrap();
            let actual = media_proxy::select(&url, &env)
                .unwrap()
                .map(|value| value.to_string().trim_end_matches('/').to_owned())
                .unwrap_or_default();
            assert_eq!(
                actual,
                expand(candidate["proxy"].as_str().unwrap(), &values),
                "{} {url}",
                case["name"]
            );
        }
    }
}
struct OwnedFixture {
    child: std::process::Child,
    endpoints: Value,
}
impl OwnedFixture {
    fn new() -> Self {
        // env_clear 后 unix 的 execvp 退回默认 PATH 找不到 node;显式继承父进程
        // PATH 供子进程使用,程序查找也按父 PATH 解析。
        let node = std::env::var_os("PISPER_TEST_NODE24").unwrap_or_else(|| "node".into());
        let parent_path = std::env::var_os("PATH");
        let mut child = std::process::Command::new(&node)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/native_channels/feishu_qq/sdk-proxy-oracle.mjs"
            ))
            .arg("--serve")
            .env_clear()
            .env("PATH", parent_path.unwrap_or_default())
            .env("SYSTEMROOT", "C:/Windows")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut output)
            .unwrap();
        Self {
            child,
            endpoints: serde_json::from_str::<Value>(&output).unwrap()["endpoints"].clone(),
        }
    }
}
impl Drop for OwnedFixture {
    fn drop(&mut self) {
        if let Some(mut input) = self.child.stdin.take() {
            let _ = input.write_all(b"stop\n");
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[tokio::test]
async fn actual_sdk_media_proxy_routes_match_owned_http_https_and_connect_transports() {
    let fixture = OwnedFixture::new();
    let values = substitutions(&fixture.endpoints);
    let session = Session {
        config: Value::Null,
        base: values["HTTP_ORIGIN"].clone(),
        client: common::client(),
        token: AsyncMutex::new(Token {
            value: String::new(),
            until: Instant::now(),
        }),
        live: Live::new(),
        bot: Mutex::new(String::new()),
        fragment_epoch: Mutex::new(tokio::time::Instant::now()),
    };
    let root = include_bytes!("proxy-test-ca.pem");
    let records = reqwest::Client::builder().no_proxy().build().unwrap();
    let records_url = values["HTTP_ORIGIN"].clone() + "/__records";
    for case in oracle()["cases"].as_array().unwrap() {
        let source = expand(
            &case["source"]
                .as_str()
                .unwrap()
                .replace("%40", "@")
                .replace("%2F", "/")
                .replace("%3F", "?")
                .replace("%3D", "="),
            &values,
        );
        let env = environment(case, &values);
        let result = session
            .fetch_source_url_with_environment(
                reqwest::Url::parse(&source).unwrap(),
                Some(std::net::Ipv4Addr::LOCALHOST.into()),
                &env,
                if case["name"] == "extra_ca_environment" {
                    None
                } else {
                    Some(root)
                },
            )
            .await;
        if case["error"].is_null() {
            assert_eq!(
                String::from_utf8(
                    result.unwrap_or_else(|error| panic!("{}: {error}", case["name"]))
                )
                .unwrap(),
                case["body"],
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                result.unwrap_err().message,
                case["error"]["message"],
                "{}",
                case["name"]
            );
        }
        let actual: Value = records
            .get(&records_url)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            actual,
            expanded_json(&case["records"], &values),
            "{}",
            case["name"]
        );
    }
    let env = HashMap::from([
        ("HTTPS_PROXY".into(), values["HTTP_PROXY"].clone()),
        ("HTTP_PROXY".into(), values["HTTPS_PROXY"].clone()),
    ]);
    let slow = reqwest::Url::parse(&format!(
        "{}/redirect?to={}/slow",
        values["HTTPS_ORIGIN"], values["HTTP_ORIGIN"]
    ))
    .unwrap();
    let mut pending = Box::pin(session.fetch_source_url_with_environment(
        slow,
        Some(std::net::Ipv4Addr::LOCALHOST.into()),
        &env,
        Some(root),
    ));
    let reached = async {
        loop {
            let observed: Value = records
                .get(&records_url)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if observed
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["path"] == "/slow")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! { result = &mut pending => panic!("nested transport settled before cancellation: {result:?}"), result = tokio::time::timeout(Duration::from_secs(5), reached) => result.unwrap() }
    session.live.token.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap_err()
            .message,
        common::cancelled().message
    );
    session.live.stop().await;
}
