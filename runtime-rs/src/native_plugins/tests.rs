use super::*;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    data: PathBuf,
    app: Arc<Mutex<Value>>,
    port: ConfigPort,
    service: Arc<ToolPluginService>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_clock(Arc::new(|| chrono::Utc::now().timestamp_millis()))
    }
    fn with_clock(clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pisper-native-plugin-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let data = root.join("agent");
        fs::create_dir(&data).unwrap();
        let app = Arc::new(Mutex::new(
            json!({"enabledTools":["read"],"unknown":{"keep":true},"webSearch":{"language":"auto"}}),
        ));
        let read_app = app.clone();
        let update_app = app.clone();
        let config = data.join("pisper.json");
        store::atomic_json(&config, &app.lock().unwrap()).unwrap();
        let port = ConfigPort {
            read: Arc::new(move || Ok(read_app.lock().unwrap().clone())),
            update: Arc::new(move |mutation| {
                let app = update_app.clone();
                let config = config.clone();
                Box::pin(async move {
                    let mut value = app.lock().unwrap();
                    let mut next = value.clone();
                    mutation(&mut next)?;
                    store::atomic_json(&config, &next)?;
                    *value = next;
                    Ok(value.clone())
                })
            }),
            normalize_web_search: Arc::new(|value| {
                crate::native_web_search::normalize_config_checked(&value)
                    .map_err(|error| PluginError::new(error.message))
            }),
        };
        let service = ToolPluginService::open_with_options(
            &data,
            port.clone(),
            catalog(),
            "en-US",
            clock,
            std::env::current_exe().unwrap(),
            Duration::from_secs(120),
        )
        .unwrap();
        Self {
            root,
            data,
            app,
            port,
            service,
        }
    }
    fn source(&self, id: &str, tool: &str, code: &str, entry: &str) -> PathBuf {
        let source = self.root.join(format!("source-{id}"));
        fs::create_dir(&source).unwrap();
        fs::write(source.join(MANIFEST_FILE),serde_json::to_vec(&json!({"schemaVersion":1,"id":id,"name":"Plugin fixture","version":"1.0.0","entry":entry,"permissions":["workspace-read"],"tools":[{"name":tool,"description":"Read an actual synthetic fixture.","parameters":{"type":"object","properties":{}}}]})).unwrap()).unwrap();
        fs::write(source.join(entry), code).unwrap();
        source
    }
    async fn install(&self, id: &str, tool: &str, code: &str) -> Value {
        let source = self.source(id, tool, code, "index.mjs");
        let inspected = self
            .service
            .inspect(source.to_str().unwrap())
            .await
            .unwrap();
        self.service
            .install(inspected["inspectionId"].as_str().unwrap())
            .await
            .unwrap()
    }
    fn request(&self, entry: PathBuf, arguments: Value) -> WorkerRequest {
        WorkerRequest {
            plugin_root: entry.parent().unwrap().to_owned(),
            entry,
            tool_name: "fixture_tool".into(),
            arguments,
            context: ExecutionContext {
                cwd: self.root.clone(),
                session_id: "synthetic-session".into(),
                data_dir: self.data.join("plugin-data/fixture"),
            },
            timeout_ms: 2000,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(actual) = fs::canonicalize(&self.root) {
            let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
            assert!(actual.starts_with(parent));
            assert!(actual
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pisper-native-plugin-test-"));
            let _ = fs::remove_dir_all(actual);
        }
    }
}
fn catalog() -> Catalog {
    Catalog {
        tools: vec![
            json!({"id":"read","name":"Read","label":"Read","category":"filesystem","risk":"low","source":"builtin","description":"Read actual files"}),
            json!({"id":"bash","name":"Shell","category":"terminal","risk":"high","source":"builtin","description":"Run commands"}),
            json!({"id":"plugin_create","name":"Plugin Create","category":"plugins","risk":"high","source":"app","description":"Create a local plugin"}),
        ],
        presets: json!({"full":["read","bash","plugin_create"]}),
    }
}
fn embedded(fixture: &Fixture, path: PathBuf, args: Value) -> Result<Value> {
    let request = fixture.request(path, args);
    executor::execute_embedded(&request, Arc::new(AtomicBool::new(false)))
}

#[test]
fn windows_display_paths_only_convert_absolute_drive_and_complete_unc_namespaces() {
    for (input, expected) in [
        (r"\\?\C:\workspace\插件\data", r"C:\workspace\插件\data"),
        (r"\\?\z:\", r"z:\"),
        (
            r"\\?\UNC\server\share\插件\data",
            r"\\server\share\插件\data",
        ),
        (r"\\?\UNC\server\share", r"\\server\share"),
        (r"C:\ordinary\path", r"C:\ordinary\path"),
        (r"\\server\share\ordinary", r"\\server\share\ordinary"),
        (r"relative\path", r"relative\path"),
        ("/var/lib/plugin-data", "/var/lib/plugin-data"),
        (r"\\?\C:relative", r"\\?\C:relative"),
        (r"\\?\C:/slash", r"\\?\C:/slash"),
        (r"\\?\UNC\server", r"\\?\UNC\server"),
        (r"\\?\UNC\server\", r"\\?\UNC\server\"),
        (r"\\?\UNC\\share", r"\\?\UNC\\share"),
        (r"\\?\Volume{fixture}\data", r"\\?\Volume{fixture}\data"),
        (
            r"\\?\GLOBALROOT\Device\fixture",
            r"\\?\GLOBALROOT\Device\fixture",
        ),
        (r"\\.\pipe\fixture", r"\\.\pipe\fixture"),
    ] {
        assert_eq!(paths::regular_windows_path(input), expected, "{input}");
    }
    #[cfg(not(windows))]
    assert_eq!(
        paths::display_path(std::path::Path::new(r"\\?\C:\literal-posix-name")),
        r"\\?\C:\literal-posix-name"
    );
}

#[test]
fn plugin_context_and_realpath_display_regular_paths_without_changing_internal_paths() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.paths",
        "fixture_paths",
        r#"import fs from 'node:fs/promises';import path from 'node:path';export async function execute({context}){await fs.mkdir(context.dataDir,{recursive:true});await fs.writeFile(path.join(context.dataDir,'display-path.txt'),'actual native write');return {content:[{type:'text',text:'paths'}],details:{...context,realpath:await fs.realpath(context.dataDir)}}}"#,
        "index.mjs",
    );
    let request = fixture.request(source.join("index.mjs"), json!({}));
    let result = executor::execute_embedded(&request, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(result["details"]["sessionId"], "synthetic-session");
    for (key, internal) in [
        ("cwd", &request.context.cwd),
        ("dataDir", &request.context.data_dir),
        ("realpath", &request.context.data_dir),
    ] {
        let returned = result["details"][key].as_str().unwrap();
        assert!(std::path::Path::new(returned).is_absolute());
        assert_eq!(
            fs::canonicalize(returned).unwrap(),
            fs::canonicalize(internal).unwrap()
        );
        #[cfg(windows)]
        assert!(!returned.starts_with(r"\\?\"), "{key}: {returned}");
    }
    assert_eq!(
        fs::read_to_string(request.context.data_dir.join("display-path.txt")).unwrap(),
        "actual native write"
    );
    #[cfg(windows)]
    assert!(request.context.cwd.to_string_lossy().starts_with(r"\\?\"));
}

#[tokio::test]
async fn installed_state_restarts_with_release_paths_and_original_manifest_bytes() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.restart",
        "fixture_restart",
        "export async function execute(){return 'actual'}",
        "index.mjs",
    );
    let bytes = fs::read(source.join(MANIFEST_FILE)).unwrap();
    let inspected = fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(inspected["fileCount"], 2);
    assert!(inspected["plugin"]["installedAt"].is_null());
    assert_eq!(inspected["plugin"]["systemAccess"], true);
    let installed = fixture
        .service
        .install(inspected["inspectionId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(installed["enabled"], true);
    assert_eq!(
        fs::read(
            fixture
                .data
                .join("plugins/fixture.restart/1.0.0/pisper-plugin.json")
        )
        .unwrap(),
        bytes
    );
    let raw: Value =
        serde_json::from_slice(&fs::read(fixture.data.join("pisper-plugins.json")).unwrap())
            .unwrap();
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["plugins"]["fixture.restart"]["version"], "1.0.0");
    assert_eq!(
        raw["plugins"]["fixture.restart"]["digest"],
        inspected["digest"]
    );
    let restarted =
        ToolPluginService::open(&fixture.data, fixture.port.clone(), catalog()).unwrap();
    assert!(restarted.get_state().await.unwrap()["enabledTools"]
        .as_array()
        .unwrap()
        .contains(&json!("fixture_restart")));
    restarted.close().await.unwrap();
}
#[tokio::test]
async fn inspection_ttl_and_source_tampering_are_real_rejections() {
    let clock = Arc::new(AtomicI64::new(1000));
    let clock_read = clock.clone();
    let fixture = Fixture::with_clock(Arc::new(move || clock_read.load(Ordering::SeqCst)));
    let source = fixture.source(
        "fixture.tamper",
        "fixture_tamper",
        "export const execute=()=>true",
        "index.mjs",
    );
    let first = fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap();
    fs::write(source.join("index.mjs"), "export const execute=()=>false").unwrap();
    assert!(fixture
        .service
        .install(first["inspectionId"].as_str().unwrap())
        .await
        .unwrap_err()
        .message
        .contains("发生了变化"));
    let next = fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap();
    clock.store(1000 + INSPECTION_TTL_MS + 1, Ordering::SeqCst);
    assert!(fixture
        .service
        .install(next["inspectionId"].as_str().unwrap())
        .await
        .unwrap_err()
        .message
        .contains("已过期"));
    assert!(!fixture.data.join("plugins/fixture.tamper/1.0.0").exists());
}
#[tokio::test]
async fn concurrent_install_has_one_owner_and_never_overwrites() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.race",
        "fixture_race",
        "export const execute=()=>true",
        "index.mjs",
    );
    let a = fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap();
    let b = fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        fixture.service.install(a["inspectionId"].as_str().unwrap()),
        fixture.service.install(b["inspectionId"].as_str().unwrap())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        fixture.service.get_state().await.unwrap()["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["id"] == "fixture.race")
            .count(),
        1
    );
}
#[tokio::test]
async fn builtin_and_other_installed_tool_conflicts_are_rejected() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.conflict",
        "read",
        "export const execute=()=>true",
        "index.mjs",
    );
    assert!(fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap_err()
        .message
        .contains("内置工具冲突"));
    fixture
        .install(
            "fixture.first",
            "fixture_same",
            "export const execute=()=>true",
        )
        .await;
    let source = fixture.source(
        "fixture.second",
        "fixture_same",
        "export const execute=()=>true",
        "index.mjs",
    );
    assert!(fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap_err()
        .message
        .contains("fixture.first"));
}
#[tokio::test]
async fn save_and_capability_toggle_preserve_unknown_canonical_fields_and_restart() {
    let fixture = Fixture::new();
    fixture
        .install(
            "fixture.toggle",
            "fixture_toggle",
            "export const execute=()=>true",
        )
        .await;
    fixture.app.lock().unwrap()["concurrentUnknown"] = json!({"new":42});
    let state = fixture
        .service
        .set_capability_enabled("fixture.toggle", "fixture_toggle", false)
        .await
        .unwrap();
    assert!(!state["enabledTools"]
        .as_array()
        .unwrap()
        .contains(&json!("fixture_toggle")));
    assert_eq!(fixture.app.lock().unwrap()["unknown"]["keep"], true);
    assert_eq!(fixture.app.lock().unwrap()["concurrentUnknown"]["new"], 42);
    assert_eq!(state["changes"][0]["tool"], "fixture_toggle");
    assert_eq!(state["changes"][0]["enabled"], false);
    fixture
        .service
        .set_plugin_enabled("fixture.toggle", true)
        .await
        .unwrap();
    let restored = ToolPluginService::open(&fixture.data, fixture.port.clone(), catalog()).unwrap();
    assert!(restored.get_state().await.unwrap()["enabledTools"]
        .as_array()
        .unwrap()
        .contains(&json!("fixture_toggle")));
    restored.close().await.unwrap();
}
#[tokio::test]
async fn config_failure_rolls_plugin_state_back() {
    let fixture = Fixture::new();
    fixture
        .install(
            "fixture.rollback",
            "fixture_rollback",
            "export const execute=()=>true",
        )
        .await;
    let mut failed_port = fixture.port.clone();
    failed_port.update =
        Arc::new(|_| Box::pin(async { Err(PluginError::new("synthetic config write failure")) }));
    let failed = ToolPluginService::open(&fixture.data, failed_port, catalog()).unwrap();
    assert!(failed.save_state(json!({"enabledTools":[]})).await.is_err());
    let persisted: Value =
        serde_json::from_slice(&fs::read(fixture.data.join("pisper-plugins.json")).unwrap())
            .unwrap();
    assert_eq!(
        persisted["plugins"]["fixture.rollback"]["enabledTools"],
        json!(["fixture_rollback"])
    );
    failed.close().await.unwrap();
}
#[tokio::test]
async fn create_generates_global_sources_relative_module_and_refuses_overwrite() {
    let fixture = Fixture::new();
    let input = json!({"id":"fixture.created","name":"Created fixture","tools":[{"name":"fixture_created","description":"Read relative bundled module"}],"entryCode":"import {version} from './version.mjs';export async function execute(){return version}","files":[{"path":"version.mjs","content":"export const version='4.5.6'"}]});
    let created = fixture.service.create(input.clone()).await.unwrap();
    assert_eq!(created["version"], "1.0.0");
    assert_eq!(
        fs::canonicalize(created["sourcePath"].as_str().unwrap()).unwrap(),
        fs::canonicalize(fixture.data.join("plugin-sources/fixture.created")).unwrap()
    );
    #[cfg(windows)]
    assert!(!created["sourcePath"].as_str().unwrap().starts_with(r"\\?\"));
    assert!(fixture
        .service
        .create(input)
        .await
        .unwrap_err()
        .message
        .contains("不能覆盖"));
    let result = embedded(
        &fixture,
        fixture.data.join("plugins/fixture.created/1.0.0/index.mjs"),
        json!({}),
    )
    .unwrap();
    assert_eq!(result["content"][0]["text"], "4.5.6");
}
#[tokio::test]
async fn create_rejects_path_escape_and_cleans_failed_generation_without_install() {
    let fixture = Fixture::new();
    let input = json!({"id":"fixture.invalid","name":"Invalid fixture","tools":[{"name":"fixture_invalid","description":"Invalid paths"}],"entryCode":"export const execute=()=>true","files":[{"path":"../escaped.txt","content":"must not escape"}]});
    assert!(fixture.service.create(input).await.is_err());
    assert!(!fixture.data.join("escaped.txt").exists());
    assert!(!fixture.data.join("plugin-sources/fixture.invalid").exists());
}
#[tokio::test]
async fn presets_default_migration_and_mode_gate_follow_release() {
    let fixture = Fixture::new();
    assert_eq!(
        catalog().tools_from_config(&json!({"toolMode":"custom"})),
        vec!["read", "bash", "plugin_create"]
    );
    assert_eq!(
        catalog().tools_from_config(&json!({"toolMode":"workspace","enabledTools":["read"]})),
        vec!["read", "bash", "plugin_create"]
    );
    fixture
        .service
        .ensure_default_tools(&["plugin_create", "unknown"], "fixtureMigrated")
        .await
        .unwrap();
    fixture
        .service
        .ensure_default_tools(&["bash"], "fixtureMigrated")
        .await
        .unwrap();
    assert_eq!(
        fixture.app.lock().unwrap()["enabledTools"],
        json!(["read", "plugin_create"])
    );
    fixture
        .install(
            "fixture.mode",
            "fixture_mode",
            "export const execute=()=>true",
        )
        .await;
    assert_eq!(
        fixture
            .service
            .enabled_tools(&json!({"enabledTools":["read"]}), "workspace-write")
            .unwrap(),
        vec!["read"]
    );
    assert_eq!(
        fixture
            .service
            .enabled_tools(&json!({"enabledTools":["read"]}), "full-access")
            .unwrap(),
        vec!["read", "fixture_mode"]
    );
    assert_eq!(fixture.service.get_tool_risk("fixture_mode"), Some("high"));
}
#[tokio::test]
async fn uninstall_removes_all_installed_versions_but_keeps_source_and_plugin_data() {
    let fixture = Fixture::new();
    fixture
        .install(
            "fixture.remove",
            "fixture_remove",
            "export const execute=()=>true",
        )
        .await;
    fs::create_dir(fixture.data.join("plugins/fixture.remove/0.9.0")).unwrap();
    fs::create_dir_all(fixture.data.join("plugin-data/fixture.remove")).unwrap();
    fs::write(
        fixture.data.join("plugin-data/fixture.remove/keep.txt"),
        "persistent fixture data",
    )
    .unwrap();
    let state = fixture.service.uninstall("fixture.remove").await.unwrap();
    assert!(!fixture.data.join("plugins/fixture.remove").exists());
    assert!(fixture
        .data
        .join("plugin-data/fixture.remove/keep.txt")
        .exists());
    assert!(!state["enabledTools"]
        .as_array()
        .unwrap()
        .contains(&json!("fixture_remove")));
    assert!(fixture
        .service
        .uninstall("builtin.filesystem")
        .await
        .unwrap_err()
        .message
        .contains("本地插件"));
}

#[tokio::test]
async fn stale_tool_definition_cannot_execute_after_capability_is_disabled() {
    let fixture = Fixture::new();
    fixture
        .install(
            "fixture.stale",
            "fixture_stale",
            "export const execute=()=>true",
        )
        .await;
    let definitions = fixture
        .service
        .definitions(&fixture.root, "actual-session", &["fixture_stale".into()])
        .unwrap();
    assert_eq!(definitions.len(), 1);
    fixture
        .service
        .set_capability_enabled("fixture.stale", "fixture_stale", false)
        .await
        .unwrap();
    let error = (definitions[0].execute)("actual-tool-call".into(), json!({}), None, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("已停用"));
    fixture.service.uninstall("fixture.stale").await.unwrap();
}

#[test]
fn plugin_create_result_keeps_release_install_details_and_next_turn_guidance() {
    let details = json!({"id":"fixture.created","name":"Created","version":"1.0.0","sourcePath":"synthetic-source","tools":["first_tool","second_tool"],"installed":{"systemAccess":true}});
    let result = super::tools::created_result(details.clone());
    assert_eq!(result["details"], details);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Created and installed plugin \"Created\" (fixture.created@1.0.0)."));
    assert!(text.contains("Tools: first_tool, second_tool"));
    assert!(text.contains("on the next Agent turn through discover_tools and call_tool."));
}
#[test]
fn manifest_schema_limits_and_errors_match_release() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.schema",
        "fixture_schema",
        "export const execute=()=>true",
        "index.mjs",
    );
    let mut raw: Value =
        serde_json::from_slice(&fs::read(source.join(MANIFEST_FILE)).unwrap()).unwrap();
    raw["tools"][0]["parameters"]["required"] = json!("text");
    assert!(manifest::normalize(&raw)
        .unwrap_err()
        .message
        .contains("required 必须为字符串数组"));
    raw["tools"][0]["parameters"] =
        json!({"type":"object","properties":{"bad":{"type":"not-a-type"}}});
    assert!(manifest::normalize(&raw)
        .unwrap_err()
        .message
        .contains("不是有效的 JSON Schema"));
    fs::write(
        source.join(MANIFEST_FILE),
        " ".repeat(MAX_MANIFEST_BYTES + 1),
    )
    .unwrap();
    assert!(manifest::read(&source)
        .unwrap_err()
        .message
        .contains("超过 256 KB"));
}
#[test]
fn directory_scan_enforces_file_and_total_byte_limits() {
    let fixture = Fixture::new();
    let count = fixture.root.join("too-many");
    fs::create_dir(&count).unwrap();
    for index in 0..=MAX_PLUGIN_FILES {
        fs::write(count.join(format!("{index:04}.txt")), []).unwrap();
    }
    assert!(store::scan(&count, "en-US")
        .unwrap_err()
        .message
        .contains("不能超过 512"));
    let bytes = fixture.root.join("too-big");
    fs::create_dir(&bytes).unwrap();
    let file = fs::File::create(bytes.join("large.bin")).unwrap();
    file.set_len(MAX_PLUGIN_BYTES as u64 + 1).unwrap();
    assert!(store::scan(&bytes, "en-US")
        .unwrap_err()
        .message
        .contains("不能超过 20 MB"));
}
#[test]
fn shipped_package_info_equivalent_performs_real_filesystem_path_and_buffer_work() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("package.json"),r#"{"name":"synthetic-package","version":"2.3.4","scripts":{"test":"fixture","dev":"fixture"}}"#).unwrap();
    let source=fixture.source("fixture.package","fixture_package",r#"import {readFile} from 'node:fs/promises';import {join} from 'node:path';export async function execute({context}){const filePath=join(context.cwd,'package.json');const text=await readFile(filePath,'utf8');if(Buffer.byteLength(text)>1048576)throw new Error('large');const pkg=JSON.parse(text);return {content:[{type:'text',text:pkg.name}],details:{name:pkg.name,version:pkg.version,scripts:Object.keys(pkg.scripts).sort(),filePath,environment:process.env}}}"#,"index.mjs");
    let result = embedded(&fixture, source.join("index.mjs"), json!({})).unwrap();
    assert_eq!(result["details"]["name"], "synthetic-package");
    assert_eq!(result["details"]["version"], "2.3.4");
    assert_eq!(result["details"]["scripts"], json!(["dev", "test"]));
    assert_eq!(result["details"]["environment"], json!({}));
    assert_eq!(
        fs::canonicalize(result["details"]["filePath"].as_str().unwrap()).unwrap(),
        fixture.root.join("package.json")
    );
    #[cfg(windows)]
    assert!(!result["details"]["filePath"]
        .as_str()
        .unwrap()
        .starts_with(r"\\?\"));
}
#[test]
fn native_plugin_writes_persistent_data_and_handles_actual_enoent() {
    let fixture = Fixture::new();
    let source=fixture.source("fixture.io","fixture_io",r#"import fs from 'node:fs/promises';import path from 'node:path';export default async function({context}){await fs.mkdir(context.dataDir,{recursive:true});const file=path.join(context.dataDir,'real.txt');await fs.writeFile(file,Buffer.from('native bytes'));await fs.appendFile(file,' plus');let missing;try{await fs.readFile(path.join(context.cwd,'absent.txt'))}catch(e){missing=e.code}return {content:[{type:'text',text:await fs.readFile(file,'utf8')}],details:{missing,size:(await fs.stat(file)).size}}}"#,"index.mjs");
    let result = embedded(&fixture, source.join("index.mjs"), json!({})).unwrap();
    assert_eq!(result["content"][0]["text"], "native bytes plus");
    assert_eq!(result["details"]["missing"], "ENOENT");
    assert_eq!(
        fs::read_to_string(fixture.data.join("plugin-data/fixture/real.txt")).unwrap(),
        "native bytes plus"
    );
}
#[test]
fn cjs_default_function_and_relative_require_execute_actual_logic() {
    let fixture = Fixture::new();
    let source=fixture.source("fixture.cjs","fixture_cjs","const {version}=require('./version.cjs');module.exports=async({arguments:input})=>input.prefix+version","index.cjs");
    fs::write(source.join("version.cjs"), "exports.version='8.9.0'").unwrap();
    assert_eq!(
        embedded(&fixture, source.join("index.cjs"), json!({"prefix":"v"})).unwrap()["content"][0]
            ["text"],
        "v8.9.0"
    );
}
#[test]
fn js_type_module_entry_and_export_default_object_are_supported() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.js",
        "fixture_js",
        "export default {async execute({arguments:input}){return {value:input.n*3}}}",
        "index.js",
    );
    fs::write(source.join("package.json"), r#"{"type":"module"}"#).unwrap();
    assert_eq!(
        embedded(&fixture, source.join("index.js"), json!({"n":7})).unwrap()["content"][0]["text"],
        "{\n  \"value\": 21\n}"
    );
}
#[test]
fn timers_errors_result_size_serialization_and_unsupported_apis_are_real() {
    let fixture = Fixture::new();
    let source=fixture.source("fixture.timer","fixture_timer","export async function execute(){await new Promise(resolve=>setTimeout(resolve,10));return 'waited'}","index.mjs");
    assert_eq!(
        embedded(&fixture, source.join("index.mjs"), json!({})).unwrap()["content"][0]["text"],
        "waited"
    );
    for (code, message) in [
        (
            "export function execute(){throw new Error('actual plugin exception')}",
            "actual plugin exception",
        ),
        ("export const value=1", "必须导出 execute"),
        (
            "export function execute(){return 'x'.repeat(1048577)}",
            "超过 1 MB",
        ),
        (
            "export function execute(){const result={content:[]};result.self=result;return result}",
            "无法序列化",
        ),
        (
            "import http from 'node:http';export const execute=()=>http",
            "ERR_PISPER_NODE_COMPAT",
        ),
    ] {
        fs::write(source.join("index.mjs"), code).unwrap();
        let request = fixture.request(source.join("index.mjs"), json!({}));
        let started = std::time::Instant::now();
        let error =
            executor::execute_embedded(&request, Arc::new(AtomicBool::new(false))).unwrap_err();
        assert!(
            error.message.contains(message),
            "expected error fragment {message:?}; actual_error={:?}; elapsed_ms={}; configured_timeout_ms={}; entry_code={code:?}",
            error.message,
            started.elapsed().as_millis(),
            request.timeout_ms,
        );
    }
}
#[test]
fn result_limit_checks_serialized_utf8_bytes_of_multibyte_text() {
    let fixture = Fixture::new();
    let envelope = json!({"content":[{"type":"text","text":""}],"details":{}});
    let overhead = serde_json::to_vec(&envelope).unwrap().len();
    let character = "中";
    let accepted_count = (MAX_RESULT_BYTES - overhead) / character.len();
    let accepted_text = character.repeat(accepted_count);
    assert!(accepted_text.encode_utf16().count() < MAX_RESULT_BYTES);
    let source = fixture.source(
        "fixture.utf8-limit",
        "fixture_utf8_limit",
        &format!("export function execute(){{return '中'.repeat({accepted_count})}}"),
        "index.mjs",
    );
    let accepted = embedded(&fixture, source.join("index.mjs"), json!({})).unwrap();
    assert_eq!(accepted["content"][0]["text"], accepted_text);
    let serialized_bytes = serde_json::to_vec(&accepted).unwrap().len();
    assert!(serialized_bytes <= MAX_RESULT_BYTES);
    assert!(MAX_RESULT_BYTES - serialized_bytes < character.len());

    let rejected_count = accepted_count + 1;
    assert!(rejected_count * character.len() + overhead > MAX_RESULT_BYTES);
    assert!(rejected_count * character.encode_utf16().count() < MAX_RESULT_BYTES);
    fs::write(
        source.join("index.mjs"),
        format!("export function execute(){{return '中'.repeat({rejected_count})}}"),
    )
    .unwrap();
    let request = fixture.request(source.join("index.mjs"), json!({}));
    let started = std::time::Instant::now();
    let error = executor::execute_embedded(&request, Arc::new(AtomicBool::new(false))).unwrap_err();
    assert!(
        error.message.contains("超过 1 MB"),
        "actual_error={:?}; elapsed_ms={}; configured_timeout_ms={}; serialized_utf8_bytes={}",
        error.message,
        started.elapsed().as_millis(),
        request.timeout_ms,
        rejected_count * character.len() + overhead,
    );
}

#[test]
fn cpu_infinite_loop_and_cancellation_are_interrupted_with_release_errors() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.loop",
        "fixture_loop",
        "export function execute(){while(true){}}",
        "index.mjs",
    );
    let mut request = fixture.request(source.join("index.mjs"), json!({}));
    request.timeout_ms = 30;
    assert!(
        executor::execute_embedded(&request, Arc::new(AtomicBool::new(false)))
            .unwrap_err()
            .message
            .contains("超过 120 秒")
    );
    let cancel = Arc::new(AtomicBool::new(true));
    assert!(executor::execute_embedded(&request, cancel)
        .unwrap_err()
        .message
        .contains("已取消"));
}
#[tokio::test]
async fn stopped_service_rejects_execution_inspection_and_install() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.closed",
        "fixture_closed",
        "export const execute=()=>true",
        "index.mjs",
    );
    let inspected = fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap();
    fixture.service.close().await.unwrap();
    assert!(fixture
        .service
        .install(inspected["inspectionId"].as_str().unwrap())
        .await
        .unwrap_err()
        .message
        .contains("已停止"));
    assert!(fixture
        .service
        .inspect(source.to_str().unwrap())
        .await
        .unwrap_err()
        .message
        .contains("已停止"));
}
#[cfg(unix)]
#[test]
fn scan_rejects_real_symlinks() {
    let fixture = Fixture::new();
    let source = fixture.source(
        "fixture.link",
        "fixture_link",
        "export const execute=()=>true",
        "index.mjs",
    );
    std::os::unix::fs::symlink(fixture.root.join("outside"), source.join("link")).unwrap();
    assert!(store::scan(&source, "en-US")
        .unwrap_err()
        .message
        .contains("符号链接"));
}

#[tokio::test]
async fn malformed_search_conversion_leaves_both_plugin_and_canonical_stores_unchanged() {
    let fixture = Fixture::new();
    let config = fixture.data.join("pisper.json");
    let state = fixture.data.join("pisper-plugins.json");
    let before_config = fs::read(&config).unwrap();
    let before_state = fs::read(&state).ok();
    let before_app = fixture.app.lock().unwrap().clone();
    for web_search in [
        Value::Null,
        json!({"language":{"toString":null}}),
        json!({"safeSearch":{"toString":false}}),
        json!({"maxResults":{"toString":4}}),
    ] {
        let error = fixture
            .service
            .save_state(json!({
                "enabledTools":[], "webSearch":web_search,
            }))
            .await
            .unwrap_err();
        assert!(error.message.contains("Cannot"), "{}", error.message);
        assert_eq!(fs::read(&config).unwrap(), before_config);
        assert_eq!(fs::read(&state).ok(), before_state);
        assert_eq!(*fixture.app.lock().unwrap(), before_app);
    }
}

#[tokio::test]
async fn explicit_false_search_resets_defaults_while_omitted_search_preserves_configuration() {
    let fixture = Fixture::new();
    let configured = json!({"provider":"bing","language":"ko-KR","safeSearch":2,"maxResults":11});
    fixture
        .service
        .save_state(json!({"enabledTools":["read"],"webSearch":configured}))
        .await
        .unwrap();
    let retained = fixture
        .service
        .save_state(json!({"enabledTools":["read"]}))
        .await
        .unwrap();
    assert_eq!(retained["webSearch"], configured);
    let reset = fixture
        .service
        .save_state(json!({"enabledTools":["read"],"webSearch":false}))
        .await
        .unwrap();
    assert_eq!(
        reset["webSearch"],
        json!({"provider":"bing","language":"auto","safeSearch":1,"maxResults":8})
    );
    fixture.app.lock().unwrap()["webSearch"] = Value::Null;
    assert_eq!(
        fixture.service.get_state().await.unwrap()["webSearch"]["language"],
        "auto"
    );
    fixture.app.lock().unwrap()["webSearch"] = json!({"language":{"toString":null}});
    assert_eq!(
        fixture.service.get_state().await.unwrap_err().message,
        "Cannot convert object to primitive value"
    );
}
