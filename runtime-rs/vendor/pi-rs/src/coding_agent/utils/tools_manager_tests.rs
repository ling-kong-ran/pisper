use super::super::management_http::RawResponse;
use super::*;
use futures::{stream, FutureExt, StreamExt};
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

#[derive(Clone)]
struct OracleHost {
    spec: Value,
    trace: Arc<Mutex<Vec<Value>>>,
    files: Arc<Mutex<HashMap<String, String>>>,
    extracted: Arc<AtomicBool>,
}
impl OracleHost {
    fn record(&self, event: Value) {
        self.trace.lock().unwrap().push(event);
    }
    fn join(&self, parts: &[&str]) -> String {
        if self.spec["platform"] == "win32" {
            win32_join(parts)
        } else {
            posix_join(parts)
        }
    }
    fn root(&self) -> &str {
        if self.spec["platform"] == "win32" {
            "C:\\tools"
        } else {
            "/tools"
        }
    }
    fn binary(&self) -> String {
        format!(
            "{}{}",
            self.spec["tool"].as_str().unwrap(),
            if self.spec["platform"] == "win32" {
                ".exe"
            } else {
                ""
            }
        )
    }
    fn extraction(&self) -> String {
        self.join(&[
            self.root(),
            &format!(
                "extract_tmp_{}_7_1000_i",
                self.spec["tool"].as_str().unwrap()
            ),
        ])
    }
}
struct RecordingWriter {
    name: String,
    files: Arc<Mutex<HashMap<String, String>>>,
}
impl Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.files
            .lock()
            .unwrap()
            .get_mut(&self.name)
            .unwrap()
            .push_str(&String::from_utf8_lossy(bytes));
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl ToolManagerHost for OracleHost {
    fn exists(&self, path: &str) -> bool {
        self.record(json!(["exists", path]));
        if self.spec["local"] == true && path == self.join(&[self.root(), &self.binary()]) {
            return true;
        }
        if self.spec["systemTar"] == true
            && path == self.join(&["C:\\Windows", "System32", "tar.exe"])
        {
            return true;
        }
        if !self.extracted.load(Ordering::SeqCst) {
            return false;
        }
        match self.spec["layout"].as_str() {
            Some("root") => path == self.join(&[&self.extraction(), &self.binary()]),
            Some("nested") => {
                let tool = ToolKind::parse(self.spec["tool"].as_str().unwrap()).unwrap();
                let platform = self.spec["platform"].as_str().unwrap();
                let arch = self.spec["arch"].as_str().unwrap();
                let version = if tool == ToolKind::Fd && platform == "darwin" && arch == "x64" {
                    "10.3.0"
                } else {
                    "12.3.4"
                };
                let asset = tool.asset_name(version, platform, arch).unwrap();
                let stem = asset
                    .strip_suffix(".tar.gz")
                    .or_else(|| asset.strip_suffix(".zip"))
                    .unwrap();
                path == self.join(&[&self.extraction(), stem, &self.binary()])
            }
            _ => false,
        }
    }
    fn spawn(&self, command: &str, args: &[String]) -> SpawnResult {
        self.record(json!(["spawn", command, args]));
        let default = json!({"error":"not found"});
        let spec = self.spec["commands"].get(command).unwrap_or(&default);
        let result = SpawnResult {
            error: spec["error"].as_str().map(str::to_owned),
            status: spec["status"].as_i64().map(|n| n as i32),
            stdout: spec["stdout"].as_str().unwrap_or("").as_bytes().to_vec(),
            stderr: spec["stderr"].as_str().unwrap_or("").as_bytes().to_vec(),
        };
        if args[0] != "--version" && result.error.is_none() && result.status == Some(0) {
            self.extracted.store(true, Ordering::SeqCst);
        }
        result
    }
    fn mkdir(&self, path: &str) -> Result<(), FetchError> {
        self.record(json!(["mkdir", path]));
        Ok(())
    }
    fn create_file(&self, path: &str) -> Result<Box<dyn Write + Send>, FetchError> {
        self.record(json!(["create", path]));
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), String::new());
        Ok(Box::new(RecordingWriter {
            name: path.into(),
            files: self.files.clone(),
        }))
    }
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FetchError> {
        self.record(json!(["readdir", path]));
        if self.spec["layout"] != "recursive" {
            return Ok(vec![]);
        }
        if path == self.extraction() {
            return Ok(["a", "z"]
                .into_iter()
                .map(|s| DirectoryEntry {
                    name: s.into(),
                    is_file: false,
                    is_directory: true,
                })
                .collect());
        }
        if path == self.join(&[&self.extraction(), "z"]) {
            return Ok(vec![DirectoryEntry {
                name: self.binary(),
                is_file: true,
                is_directory: false,
            }]);
        }
        Ok(vec![])
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), FetchError> {
        self.record(json!(["rename", from, to]));
        Ok(())
    }
    fn chmod_executable(&self, path: &str) -> Result<(), FetchError> {
        self.record(json!(["chmod", path, 493]));
        Ok(())
    }
    fn remove_file(&self, path: &str) -> Result<(), FetchError> {
        self.record(json!(["remove",path,{"force":true}]));
        Ok(())
    }
    fn remove_dir(&self, path: &str) -> Result<(), FetchError> {
        self.record(json!(["remove",path,{"recursive":true,"force":true}]));
        Ok(())
    }
}
#[tokio::test]
async fn upstream_tools_manager_oracle() {
    let data: Value = serde_json::from_str(include_str!("tools_manager_oracle.json")).unwrap();
    for case in data["cases"].as_array().unwrap() {
        let trace = Arc::new(Mutex::new(vec![]));
        let files = Arc::new(Mutex::new(HashMap::new()));
        let host = Arc::new(OracleHost {
            spec: case.clone(),
            trace: trace.clone(),
            files: files.clone(),
            extracted: Arc::new(AtomicBool::new(false)),
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let transport: FetchTransport = {
            let spec = case.clone();
            let trace = trace.clone();
            Arc::new(move |request| {
                let spec = spec.clone();
                let trace = trace.clone();
                let index = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                // The TS oracle injects fetchWithRetry (whose retries have their
                // own 77-case oracle); its helper-call trace excludes raw retries.
                if index==0||spec["failure"].is_null(){
                    let (init,options)=if request.url.ends_with("/latest") {
                        assert!(request.manual_redirect);assert_eq!(request.headers["user-agent"],"pi-coding-agent");
                        (json!({"headers":{"User-Agent":"pi-coding-agent"},"redirect":"manual"}),json!({"timeoutMs":10000}))
                    }else{assert!(!request.manual_redirect);(Value::Null,json!({"timeoutMs":120000}))};
                    trace.lock().unwrap().push(json!(["fetch",request.url,init,options]));
                }
                if !spec["failure"].is_null(){return Err(FetchError {name:"TypeError".into(),message:spec["failure"]["message"].as_str().unwrap().into(),causes:spec["failure"]["causes"].as_array().unwrap().iter().map(|v|v.as_str().unwrap().into()).collect()});}
                if request.url.ends_with("/latest"){
                    let mut headers=HeaderMap::new();let default=if spec["tool"]=="fd"{"/a/releases/tag/v12.3.4"}else{"/a/releases/tag/12.3.4"};
                    if spec.get("location")!=Some(&Value::Null){headers.insert("location",spec["location"].as_str().unwrap_or(default).parse().unwrap());}
                    let cancel:super::super::management_http::CancelBody=Box::new(move ||async move {trace.lock().unwrap().push(json!(["cancel"]));if spec["cancelError"]==true{Err(FetchError::new("Error","cancel error"))}else{Ok(())}}.boxed());
                    Ok(RawResponse {status:302,headers,body:Some(stream::empty().boxed()),cancel_body:Some(cancel)})
                }else{
                    Ok(RawResponse {status:spec["downloadStatus"].as_u64().unwrap_or(200) as u16,headers:HeaderMap::new(),body:if spec["noBody"]==true{None}else{Some(stream::iter([Ok(b"archive".to_vec())]).boxed())},cancel_body:None})
                }
            }.boxed()
            })
        };
        // Wrap only the status before entering the transport; the oracle permits
        // status 200 with a Location and error statuses without one.
        let status = case["status"].as_u64().unwrap_or(302) as u16;
        let transport: FetchTransport = Arc::new(move |request| {
            let latest = request.url.ends_with("/latest");
            let inner = transport(request);
            async move {
                let mut response = inner.await?;
                if latest {
                    response.status = status;
                }
                Ok(response)
            }
            .boxed()
        });
        let manager = ToolsManager {
            platform: case["platform"].as_str().unwrap().into(),
            architecture: case["arch"].as_str().unwrap().into(),
            tools_dir: host.root().into(),
            env: serde_json::from_value(case.get("env").cloned().unwrap_or(json!({}))).unwrap(),
            host,
            transport,
            unique_suffix: Arc::new(|| "7_1000_i".into()),
        };
        let tool = ToolKind::parse(case["tool"].as_str().unwrap()).unwrap();
        let statuses = Arc::new(Mutex::new(vec![]));
        let callback: ToolStatusCallback = {
            let statuses = statuses.clone();
            Arc::new(move |status| statuses.lock().unwrap().push(status))
        };
        let result = match case["op"].as_str().unwrap() {
            "asset" => Ok(manager_asset(&manager, tool)),
            "discover" => Ok(manager.get_tool_path(tool)),
            "latest" => manager.get_latest_version("sharkdp/fd").await.map(Some),
            "ensure" => Ok(manager.ensure_tool(tool, Some(&callback)).await),
            _ => panic!("unknown op"),
        };
        let outcome = match result {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":{"name":error.name,"message":error.message}}),
        };
        assert_eq!(outcome, case["outcome"], "outcome {}", case["id"]);
        assert_eq!(
            json!(*statuses.lock().unwrap()),
            case["statuses"],
            "statuses {}",
            case["id"]
        );
        assert_eq!(
            json!(*trace.lock().unwrap()),
            case["trace"],
            "trace {}",
            case["id"]
        );
        assert_eq!(
            json!(*files.lock().unwrap()),
            case["files"],
            "files {}",
            case["id"]
        );
    }
}
fn manager_asset(manager: &ToolsManager, tool: ToolKind) -> Option<String> {
    tool.asset_name("1.2.3", &manager.platform, &manager.architecture)
}

#[test]
fn native_host_reads_without_following_links_and_moves_and_cleans() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().to_str().unwrap();
    let host = NativeToolManagerHost;
    let dir = std::path::Path::new(root).join("extract_tmp");
    let dir = dir.to_str().unwrap();
    host.mkdir(dir).unwrap();
    let from = std::path::Path::new(dir).join("rg");
    let from = from.to_str().unwrap();
    let mut file = host.create_file(from).unwrap();
    file.write_all(b"native bytes").unwrap();
    drop(file);
    let entries = host.read_dir(dir).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_file);
    let to = std::path::Path::new(root).join("rg");
    let to = to.to_str().unwrap();
    host.rename(from, to).unwrap();
    host.chmod_executable(to).unwrap();
    assert!(host.exists(to));
    assert_eq!(std::fs::read(to).unwrap(), b"native bytes");
    host.remove_dir(dir).unwrap();
    host.remove_file(to).unwrap();
    host.remove_file(to).unwrap();
    host.remove_dir(dir).unwrap();
}
#[test]
fn unknown_tool_and_failure_text_follow_upstream() {
    assert_eq!(ToolKind::parse("unknown"), None);
    assert_eq!(get_tool_path("unknown"), None);
    assert_eq!(
        SpawnResult {
            status: None,
            stderr: b" \n".to_vec(),
            ..Default::default()
        }
        .failure(),
        "exit status unknown"
    );
    assert_eq!(
        SpawnResult {
            error: Some("error".into()),
            stderr: b"stderr".to_vec(),
            ..Default::default()
        }
        .failure(),
        "error"
    );
    assert_eq!(
        SpawnResult {
            status: Some(2),
            stdout: "\u{feff} output \u{feff}".as_bytes().to_vec(),
            ..Default::default()
        }
        .failure(),
        "output"
    );
}
