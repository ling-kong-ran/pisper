use super::{
    config::{strings, unique_strings},
    executor, manifest,
    paths::display_path,
    store, Catalog, ConfigPort, Manifest, PluginError, Result, INSPECTION_TTL_MS, MANIFEST_FILE,
    MAX_PLUGIN_BYTES,
};
use pi_rust::agent_core::types::AgentTool;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
struct Installed {
    root: PathBuf,
    manifest: Manifest,
    record: Value,
}
#[derive(Clone)]
struct Inspection {
    root: PathBuf,
    manifest: Manifest,
    scan: store::Scan,
    expires_at: i64,
}
struct Inner {
    state: Value,
    installed: Vec<Installed>,
    inspections: HashMap<String, Inspection>,
    changing: HashSet<String>,
    active: HashMap<String, HashMap<Uuid, CancellationToken>>,
    closed: bool,
}
pub struct ToolPluginService {
    data_dir: PathBuf,
    root: PathBuf,
    state_path: PathBuf,
    config: ConfigPort,
    catalog: Catalog,
    locale: String,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    inner: Mutex<Inner>,
    mutation: Arc<tokio::sync::Mutex<()>>,
    creation: tokio::sync::Mutex<()>,
    drained: tokio::sync::Notify,
    pub(crate) worker_executable: PathBuf,
    pub(crate) execution_timeout: Duration,
}
impl ToolPluginService {
    pub fn open(data_dir: &Path, config: ConfigPort, catalog: Catalog) -> Result<Arc<Self>> {
        Self::open_with_options(
            data_dir,
            config,
            catalog,
            "en-US",
            Arc::new(|| chrono::Utc::now().timestamp_millis()),
            std::env::current_exe()?,
            Duration::from_millis(super::EXECUTION_TIMEOUT_MS),
        )
    }
    pub fn open_with_options(
        data_dir: &Path,
        config: ConfigPort,
        catalog: Catalog,
        locale: &str,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
        worker_executable: PathBuf,
        execution_timeout: Duration,
    ) -> Result<Arc<Self>> {
        catalog.validate()?;
        let data_dir = fs::canonicalize(data_dir)?;
        let root = data_dir.join("plugins");
        fs::create_dir_all(&root)?;
        store::directory(&root, "插件安装根目录必须是普通目录。")?;
        let state_path = data_dir.join("pisper-plugins.json");
        let mut state = json!({"version":1,"plugins":{}});
        match fs::read(&state_path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) if value["version"] == 1 && value["plugins"].is_object() => state = value,
                _ => tracing::warn!("failed to load plugin state: invalid schema"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "failed to load plugin state"),
        }
        let mut installed = Vec::new();
        for (id, record) in state["plugins"].as_object().into_iter().flatten() {
            let result = (|| -> Result<Installed> {
                if !manifest::valid_id(id) || !record.is_object() {
                    return Err(PluginError::new("插件安装记录无效。"));
                }
                let version = record["version"]
                    .as_str()
                    .ok_or_else(|| PluginError::new("插件安装记录无效。"))?;
                if version.contains(['/', '\\', ':']) || version == "." || version == ".." {
                    return Err(PluginError::new("插件安装记录无效。"));
                }
                let path = root.join(id).join(version);
                store::directory(&root.join(id), "插件安装目录必须是普通目录。")?;
                store::directory(&path, "插件安装目录必须是普通目录。")?;
                let manifest = manifest::read(&path)?;
                if manifest.id != *id || manifest.version != version {
                    return Err(PluginError::new("插件清单与安装记录不一致。"));
                }
                Ok(Installed {
                    root: path,
                    manifest,
                    record: record.clone(),
                })
            })();
            match result {
                Ok(value) => installed.push(value),
                Err(error) => tracing::warn!(%id, %error, "failed to load local plugin"),
            }
        }
        Ok(Arc::new(Self {
            data_dir,
            root,
            state_path,
            config,
            catalog,
            locale: locale.into(),
            clock,
            inner: Mutex::new(Inner {
                state,
                installed,
                inspections: HashMap::new(),
                changing: HashSet::new(),
                active: HashMap::new(),
                closed: false,
            }),
            mutation: Arc::new(tokio::sync::Mutex::new(())),
            creation: tokio::sync::Mutex::new(()),
            drained: tokio::sync::Notify::new(),
            worker_executable,
            execution_timeout,
        }))
    }
    fn inner(&self) -> Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| PluginError::new("插件状态锁无效。"))
    }
    fn ensure_open(inner: &Inner) -> Result<()> {
        if inner.closed {
            Err(PluginError::new("插件服务已停止。"))
        } else {
            Ok(())
        }
    }
    fn conflicts(&self, inner: &Inner, manifest: &Manifest) -> Result<()> {
        let builtin = self.catalog.ids();
        for tool in &manifest.tools {
            if builtin.contains(&tool.name) {
                return Err(PluginError::new(format!(
                    "工具名称 {} 与内置工具冲突。",
                    tool.name
                )));
            }
            for other in &inner.installed {
                if other.manifest.tools.iter().any(|v| v.name == tool.name) {
                    return Err(PluginError::new(format!(
                        "工具名称 {} 已由插件 {} 提供。",
                        tool.name, other.manifest.id
                    )));
                }
            }
        }
        Ok(())
    }
    pub async fn inspect(self: &Arc<Self>, source: &str) -> Result<Value> {
        let requested = source.trim();
        if requested.is_empty() {
            return Err(PluginError::new("请选择插件目录。"));
        }
        let path = if Path::new(requested).is_absolute() {
            PathBuf::from(requested)
        } else {
            std::env::current_dir()?.join(requested)
        };
        let locale = self.locale.clone();
        let (root, manifest, scan) = blocking(move || {
            store::directory(&path, "插件来源必须是不含符号链接的目录。")?;
            let root = fs::canonicalize(&path)?;
            let manifest = manifest::read(&root)?;
            let entry = fs::canonicalize(root.join(&manifest.entry))?;
            if !entry.starts_with(&root) || !fs::metadata(entry)?.is_file() {
                return Err(PluginError::new("插件 entry 必须指向目录内的普通文件。"));
            }
            let scan = store::scan(&root, &locale)?;
            Ok((root, manifest, scan))
        })
        .await?;
        let mut inner = self.inner()?;
        Self::ensure_open(&inner)?;
        if inner.state["plugins"].get(&manifest.id).is_some() {
            return Err(PluginError::new(format!(
                "插件 {} 已安装；当前版本不支持覆盖安装。",
                manifest.id
            )));
        }
        self.conflicts(&inner, &manifest)?;
        let inspection_id = Uuid::new_v4().to_string();
        let record =
            json!({"enabledTools":manifest.tools.iter().map(|v| &v.name).collect::<Vec<_>>()});
        let value = json!({"inspectionId":inspection_id,"plugin":public_plugin(&record,&manifest),"fileCount":scan.file_count,"byteCount":scan.byte_count,"digest":scan.digest,
            "warnings":["插件代码将在独立 Worker 中运行，但仍拥有当前系统用户可访问的本机文件和网络权限。","第三方插件能力仅在“完全访问”执行模式下提供给 Agent。"]});
        inner.inspections.insert(
            inspection_id,
            Inspection {
                root,
                manifest,
                scan,
                expires_at: (self.clock)() + INSPECTION_TTL_MS,
            },
        );
        Ok(value)
    }
    pub async fn install(self: &Arc<Self>, inspection_id: &str) -> Result<Value> {
        let _mutation = self.mutation.clone().lock_owned().await;
        let inspection = {
            let mut inner = self.inner()?;
            Self::ensure_open(&inner)?;
            let inspection = inner
                .inspections
                .get(inspection_id)
                .filter(|v| v.expires_at >= (self.clock)())
                .cloned()
                .ok_or_else(|| PluginError::new("插件检查结果已过期，请重新检查目录。"))?;
            if inner.state["plugins"]
                .get(&inspection.manifest.id)
                .is_some()
            {
                return Err(PluginError::new(format!(
                    "插件 {} 已安装。",
                    inspection.manifest.id
                )));
            }
            self.conflicts(&inner, &inspection.manifest)?;
            if !inner.changing.insert(inspection.manifest.id.clone()) {
                return Err(PluginError::new("插件正在变更，请稍后重试。"));
            }
            inspection
        };
        let changing = Changing {
            service: self.clone(),
            id: inspection.manifest.id.clone(),
        };
        let service = self.clone();
        let token = inspection_id.to_owned();
        blocking(move || {
            let (_mutation, _changing) = (_mutation, changing);
            let current = store::scan(&inspection.root, &service.locale)?;
            if current.digest != inspection.scan.digest { return Err(PluginError::new("插件目录在检查后发生了变化，请重新检查。")); }
            store::directory(&service.root, "插件安装根目录必须是普通目录。")?;
            let directory = service.root.join(&inspection.manifest.id);
            fs::create_dir_all(&directory)?;
            store::directory(&directory, "插件安装目录必须是普通目录。")?;
            let destination = directory.join(&inspection.manifest.version);
            if fs::symlink_metadata(&destination).is_ok() { return Err(PluginError::new(format!("插件 {}@{} 已存在。", inspection.manifest.id, inspection.manifest.version))); }
            let stage = directory.join(format!("{}.install-{}", inspection.manifest.version, Uuid::new_v4()));
            let result = (|| -> Result<Value> {
                store::copy_snapshot(&stage, &current)?;
                if store::scan(&stage, &service.locale)?.digest != inspection.scan.digest { return Err(PluginError::new("插件目录在复制期间发生了变化，请重新检查。")); }
                fs::rename(&stage, &destination)?;
                let record = json!({"version":inspection.manifest.version,"installedAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"digest":inspection.scan.digest,"enabledTools":inspection.manifest.tools.iter().map(|v| &v.name).collect::<Vec<_>>()});
                let mut inner = service.inner()?;
                if let Err(error) = Self::ensure_open(&inner) { drop(inner); fs::remove_dir_all(&destination)?; return Err(error); }
                let mut next = inner.state.clone(); next["plugins"][&inspection.manifest.id] = record.clone();
                if let Err(error) = store::atomic_json(&service.state_path, &next) { drop(inner); let _ = fs::remove_dir_all(&destination); return Err(error); }
                inner.state = next;
                inner.installed.push(Installed { root: destination, manifest: inspection.manifest.clone(), record: record.clone() });
                inner.inspections.remove(&token);
                Ok(public_plugin(&record, &inspection.manifest))
            })();
            if stage.exists() { let _ = fs::remove_dir_all(stage); }
            result
        }).await
    }
    pub async fn create(self: &Arc<Self>, input: Value) -> Result<Value> {
        let service = self.clone();
        // 调用方取消不释放正在写盘的创建事务；close 必须等真实提交或清理结束。
        tokio::spawn(async move { service.create_inner(input).await })
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
    }
    async fn create_inner(self: &Arc<Self>, input: Value) -> Result<Value> {
        let _creation = self.creation.lock().await;
        let entry_code = manifest::js_string(&input["entryCode"]);
        if entry_code.trim().is_empty() {
            return Err(PluginError::new("插件入口代码不能为空。"));
        }
        let version = if manifest::js_string(&input["version"]).is_empty() {
            json!("1.0.0")
        } else {
            input["version"].clone()
        };
        let manifest = manifest::normalize(
            &json!({"schemaVersion":1,"id":input["id"],"name":input["name"],"version":version,"description":input["description"],"entry":"index.mjs","permissions":input["permissions"],"tools":input["tools"]}),
        )?;
        let extras = input["files"].as_array().cloned().unwrap_or_default();
        if extras.len() > 64 {
            return Err(PluginError::new("插件附加文件不能超过 64 个。"));
        }
        let mut seen: HashSet<String> = [MANIFEST_FILE.into(), manifest.entry.clone()]
            .into_iter()
            .collect();
        let mut files = vec![
            (
                MANIFEST_FILE.to_owned(),
                format!("{}\n", serde_json::to_string_pretty(&manifest)?),
            ),
            (manifest.entry.clone(), entry_code),
        ];
        for (index, extra) in extras.iter().enumerate() {
            if !extra.is_object() {
                return Err(PluginError::new(format!("files[{index}] 必须是对象。")));
            }
            let path = manifest::relative_path(&extra["path"], &format!("files[{index}].path"))?
                .trim_start_matches("./")
                .to_owned();
            if path == "." || path.ends_with('/') || !seen.insert(path.clone()) {
                return Err(PluginError::new(format!(
                    "插件附加文件路径重复或无效：{path}"
                )));
            }
            files.push((
                path,
                if extra["content"].is_null() {
                    String::new()
                } else {
                    manifest::value_string(&extra["content"])
                },
            ));
        }
        if files
            .iter()
            .map(|(_, content)| content.len())
            .sum::<usize>()
            > MAX_PLUGIN_BYTES
        {
            return Err(PluginError::new("插件生成内容不能超过 20 MB。"));
        }
        // Generation and installation share the same install validator; source cleanup only
        // removes bytes generated by this call, never user edits made during an error.
        let root = self.data_dir.join("plugin-sources");
        let id = manifest.id.clone();
        let source = root.join(&id);
        {
            let _mutation = self.mutation.lock().await;
            {
                let inner = self.inner()?;
                Self::ensure_open(&inner)?;
                if inner.state["plugins"].get(&id).is_some() {
                    return Err(PluginError::new(format!("插件 {id} 已安装，不能覆盖。")));
                }
            }
            let source = source.clone();
            let root = root.clone();
            let files = files.clone();
            blocking(move || {
                fs::create_dir_all(&root)?;
                store::directory(&root, "全局插件源码目录必须是普通目录，不能使用符号链接。")?;
                fs::create_dir(&source).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        PluginError::new(format!(
                            "插件源码目录 {} 已存在，不能覆盖。",
                            display_path(&source)
                        ))
                    } else {
                        error.into()
                    }
                })?;
                write_generated(&source, &files)
            })
            .await?;
        }
        let result = async {
            let inspection = self.inspect(&source.to_string_lossy()).await?;
            let installed = self.install(inspection["inspectionId"].as_str().ok_or_else(|| PluginError::new("插件检查结果无效。"))?).await?;
            Ok(json!({"id":manifest.id,"name":manifest.name,"version":manifest.version,"sourcePath":display_path(&source),"tools":manifest.tools.iter().map(|v| &v.name).collect::<Vec<_>>(),"installed":installed}))
        }.await;
        if result.is_err() {
            let cleanup_source = source.clone();
            let _ = blocking(move || {
                cleanup_generated(&cleanup_source, &files);
                Ok(())
            })
            .await;
        }
        result
    }
    pub async fn get_state(&self) -> Result<Value> {
        let app = (self.config.read)()?;
        let inner = self.inner()?;
        Self::ensure_open(&inner)?;
        self.state_projection(&inner, &app)
    }
    fn state_projection(&self, inner: &Inner, app: &Value) -> Result<Value> {
        let builtins = self.catalog.tools_from_config(app);
        let mut plugins = self.catalog.builtin_plugins(&builtins);
        plugins.extend(
            inner
                .installed
                .iter()
                .map(|v| public_plugin(&v.record, &v.manifest)),
        );
        let mut enabled = builtins.clone();
        enabled.extend(
            inner
                .installed
                .iter()
                .flat_map(|v| strings(&v.record["enabledTools"])),
        );
        let tools: Vec<Value> = plugins
            .iter()
            .flat_map(|plugin| {
                plugin["capabilities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|capability| {
                        let mut value = capability.clone();
                        value["id"] = if capability["id"].is_null() {
                            capability["name"].clone()
                        } else {
                            capability["id"].clone()
                        };
                        value["name"] =
                            if capability["label"].as_str().is_some_and(|v| !v.is_empty()) {
                                capability["label"].clone()
                            } else {
                                capability["name"].clone()
                            };
                        value["pluginId"] = plugin["id"].clone();
                        value["pluginName"] = plugin["name"].clone();
                        value["source"] = if plugin["builtIn"] == true {
                            capability["source"].clone()
                        } else {
                            plugin["source"].clone()
                        };
                        value
                    })
            })
            .collect();
        let raw_search = app
            .get("webSearch")
            .filter(|value| json_truthy(value))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let web_search = (self.config.normalize_web_search)(raw_search)?;
        Ok(
            json!({"plugins":plugins,"tools":tools,"presets":self.catalog.presets,"enabledTools":enabled,"preset":self.catalog.preset(&builtins),
            "changes":app["pluginChanges"].as_array().map(|v| v.iter().take(20).cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "updatedAt":app.get("pluginsUpdatedAt").cloned().unwrap_or(Value::Null),
            "webSearch":web_search,
            "piExtensions":if app["piExtensions"].is_object() { app["piExtensions"].clone() } else { json!({}) },
            "computerUseEnabled":app["computerUseEnabled"] != false}),
        )
    }
    pub async fn save_state(self: &Arc<Self>, input: Value) -> Result<Value> {
        let service = self.clone();
        tokio::spawn(async move { service.save_state_inner(input).await })
            .await
            .map_err(|error| PluginError::new(error.to_string()))?
    }
    async fn save_state_inner(&self, input: Value) -> Result<Value> {
        let _mutation = self.mutation.clone().lock_owned().await;
        let (next, previous, current, allowed, builtin_ids) = {
            let inner = self.inner()?;
            Self::ensure_open(&inner)?;
            let app = (self.config.read)()?;
            let current = self.state_projection(&inner, &app)?;
            let allowed: HashSet<String> = current["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["id"].as_str().map(str::to_owned))
                .collect();
            let requested: Vec<String> = unique_strings(&input["enabledTools"])
                .into_iter()
                .filter(|v| allowed.contains(v))
                .collect();
            let mut next = inner.state.clone();
            for installed in &inner.installed {
                next["plugins"][&installed.manifest.id]["enabledTools"] = json!(installed
                    .manifest
                    .tools
                    .iter()
                    .filter(|v| requested.contains(&v.name))
                    .map(|v| &v.name)
                    .collect::<Vec<_>>());
            }
            (
                next,
                inner.state.clone(),
                current,
                requested,
                self.catalog.ids(),
            )
        };
        // The reference validates even explicit null/false before touching its
        // plugin state file. A conversion error must not mutate either store.
        let web_search = (self.config.normalize_web_search)(
            input
                .get("webSearch")
                .cloned()
                .unwrap_or_else(|| current["webSearch"].clone()),
        )?;
        let state_path = self.state_path.clone();
        let written = next.clone();
        blocking(move || store::atomic_json(&state_path, &written)).await?;
        let catalog = self.catalog.clone();
        let update = Arc::new(move |app: &mut Value| -> Result<()> {
            if !app.is_object() {
                return Err(PluginError::new("应用配置必须是 JSON 对象。"));
            }
            let builtin: Vec<String> = allowed
                .iter()
                .filter(|v| builtin_ids.contains(v))
                .cloned()
                .collect();
            let existing = strings(&current["enabledTools"]);
            let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let mut changes: Vec<Value> = allowed.iter().filter(|v| !existing.contains(v)).chain(existing.iter().filter(|v| !allowed.contains(v))).map(|name| {
                let label = current["tools"].as_array().into_iter().flatten().find(|v| v["id"] == *name).and_then(|v| v["label"].as_str()).unwrap_or(name);
                json!({"tool":name,"name":label,"enabled":allowed.contains(name),"timestamp":timestamp})
            }).collect();
            changes.extend(
                app["pluginChanges"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            changes.truncate(50);
            app["toolMode"] = json!(catalog.preset(&builtin));
            app["enabledTools"] = json!(builtin);
            app["pluginChanges"] = json!(changes);
            app["pluginsUpdatedAt"] = json!(timestamp);
            app["webSearch"] = web_search.clone();
            if input["piExtensions"].is_object() {
                app["piExtensions"] = input["piExtensions"].clone();
            } else if !app["piExtensions"].is_object() {
                app["piExtensions"] = json!({});
            }
            app["computerUseEnabled"] = json!(input["computerUseEnabled"]
                .as_bool()
                .unwrap_or(app["computerUseEnabled"] != false));
            Ok(())
        });
        let app = match (self.config.update)(update).await {
            Ok(app) => app,
            Err(error) => {
                let path = self.state_path.clone();
                let _ = blocking(move || store::atomic_json(&path, &previous)).await;
                return Err(error);
            }
        };
        let mut inner = self.inner()?;
        inner.state = next;
        let state = inner.state.clone();
        for installed in &mut inner.installed {
            installed.record = state["plugins"][&installed.manifest.id].clone();
        }
        self.state_projection(&inner, &app)
    }
    pub async fn set_plugin_enabled(
        self: &Arc<Self>,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<Value> {
        let current = self.get_state().await?;
        let plugin = current["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["id"] == plugin_id)
            .ok_or_else(|| PluginError::new(format!("插件 {plugin_id} 不存在。")))?;
        let names: Vec<String> = plugin["capabilities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .collect();
        let mut tools = strings(&current["enabledTools"]);
        if enabled {
            for name in names {
                if !tools.contains(&name) {
                    tools.push(name);
                }
            }
        } else {
            tools.retain(|v| !names.contains(v));
        }
        self.save_state(json!({"enabledTools":tools})).await
    }
    pub async fn set_capability_enabled(
        self: &Arc<Self>,
        plugin_id: &str,
        name: &str,
        enabled: bool,
    ) -> Result<Value> {
        let current = self.get_state().await?;
        let plugin = current["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["id"] == plugin_id);
        if !plugin.is_some_and(|v| {
            v["capabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|v| v["name"] == name)
        }) {
            return Err(PluginError::new(format!(
                "插件 {plugin_id} 不提供能力 {name}。"
            )));
        }
        let mut tools = strings(&current["enabledTools"]);
        if enabled {
            if !tools.iter().any(|v| v == name) {
                tools.push(name.into());
            }
        } else {
            tools.retain(|v| v != name);
        }
        self.save_state(json!({"enabledTools":tools})).await
    }
    pub async fn uninstall(self: &Arc<Self>, plugin_id: &str) -> Result<Value> {
        let mutation = self.mutation.clone().lock_owned().await;
        {
            let mut inner = self.inner()?;
            Self::ensure_open(&inner)?;
            if !inner.installed.iter().any(|v| v.manifest.id == plugin_id) {
                return Err(PluginError::new(format!("本地插件 {plugin_id} 不存在。")));
            }
            if inner.changing.contains(plugin_id) {
                return Err(PluginError::new("插件正在变更，请稍后重试。"));
            }
            if inner.active.get(plugin_id).is_some_and(|v| !v.is_empty()) {
                return Err(PluginError::new("插件正在执行，暂时无法卸载。"));
            }
            inner.changing.insert(plugin_id.into());
        }
        let changing = Changing {
            service: self.clone(),
            id: plugin_id.into(),
        };
        let service = self.clone();
        let id = plugin_id.to_owned();
        blocking(move || {
            let (_mutation, _changing) = (mutation, changing);
            let directory = service.root.join(&id);
            store::directory(&service.root, "插件安装根目录必须是普通目录。")?;
            store::directory(&directory, "插件安装目录必须是普通目录。")?;
            let quarantine = service
                .root
                .join(format!(".uninstall-{id}-{}", Uuid::new_v4()));
            fs::rename(&directory, &quarantine)?;
            let result = (|| -> Result<()> {
                let mut inner = service.inner()?;
                let mut next = inner.state.clone();
                next["plugins"]
                    .as_object_mut()
                    .ok_or_else(|| PluginError::new("插件状态无效。"))?
                    .remove(&id);
                store::atomic_json(&service.state_path, &next)?;
                inner.state = next;
                inner.installed.retain(|v| v.manifest.id != id);
                Ok(())
            })();
            if let Err(error) = result {
                let _ = fs::rename(&quarantine, &directory);
                return Err(error);
            }
            if let Err(error) = fs::remove_dir_all(quarantine) {
                tracing::warn!(%error,"failed to remove plugin uninstall quarantine");
            }
            Ok(())
        })
        .await?;
        self.get_state().await
    }
    pub fn is_third_party_tool(&self, name: &str) -> bool {
        self.inner().is_ok_and(|inner| {
            inner
                .installed
                .iter()
                .any(|v| v.manifest.tools.iter().any(|v| v.name == name))
        })
    }
    pub async fn ensure_default_tools(&self, ids: &[&str], migration_key: &str) -> Result<()> {
        let catalog = self.catalog.clone();
        let ids: Vec<String> = ids.iter().map(|v| (*v).to_owned()).collect();
        let migration_key = migration_key.to_owned();
        (self.config.update)(Arc::new(move |app| {
            if app[&migration_key].as_bool() == Some(true) {
                return Ok(());
            }
            let mut enabled = catalog.tools_from_config(app);
            for id in &ids {
                if catalog.ids().contains(id) && !enabled.contains(id) {
                    enabled.push(id.clone());
                }
            }
            app["toolMode"] = json!(catalog.preset(&enabled));
            app["enabledTools"] = json!(enabled);
            app[&migration_key] = json!(true);
            Ok(())
        }))
        .await?;
        Ok(())
    }
    pub fn get_tool_risk(&self, name: &str) -> Option<&'static str> {
        self.is_third_party_tool(name).then_some("high")
    }
    pub(crate) fn app_config(&self) -> Result<Value> {
        (self.config.read)()
    }
    pub(crate) fn registered_local_tools(&self) -> Result<Vec<(String, super::ToolManifest)>> {
        let inner = self.inner()?;
        Self::ensure_open(&inner)?;
        Ok(inner
            .installed
            .iter()
            .flat_map(|installed| {
                installed
                    .manifest
                    .tools
                    .iter()
                    .cloned()
                    .map(|tool| (installed.manifest.id.clone(), tool))
            })
            .collect())
    }
    pub fn enabled_tools(&self, app: &Value, execution_mode: &str) -> Result<Vec<String>> {
        let mut tools = self.catalog.tools_from_config(app);
        if execution_mode == "full-access" {
            let inner = self.inner()?;
            tools.extend(
                inner
                    .installed
                    .iter()
                    .flat_map(|v| strings(&v.record["enabledTools"])),
            );
        }
        Ok(tools)
    }
    pub fn definitions(
        self: &Arc<Self>,
        cwd: &Path,
        session_id: &str,
        enabled: &[String],
    ) -> Result<Vec<AgentTool>> {
        let inner = self.inner()?;
        Self::ensure_open(&inner)?;
        let mut definitions = Vec::new();
        for installed in &inner.installed {
            for tool in &installed.manifest.tools {
                if !enabled.contains(&tool.name)
                    || !strings(&installed.record["enabledTools"]).contains(&tool.name)
                {
                    continue;
                }
                let service = self.clone();
                let id = installed.manifest.id.clone();
                let name = tool.name.clone();
                let cwd = cwd.to_owned();
                let session_id = session_id.to_owned();
                definitions.push(AgentTool {
                    name: tool.name.clone(),
                    label: tool.label.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                    constrained_sampling: None,
                    prepare_arguments: None,
                    replay: None,
                    execution_mode: None,
                    execute: Arc::new(move |_, arguments, signal, _| {
                        let service = service.clone();
                        let id = id.clone();
                        let name = name.clone();
                        let cwd = cwd.clone();
                        let session_id = session_id.clone();
                        Box::pin(async move {
                            let result = service
                                .execute(&id, &name, arguments, cwd, session_id, signal)
                                .await?;
                            serde_json::from_value(result)
                                .map_err(|error| anyhow::anyhow!("插件结果不符合工具协议：{error}"))
                        })
                    }),
                });
            }
        }
        Ok(definitions)
    }
    pub async fn execute(
        self: &Arc<Self>,
        plugin_id: &str,
        tool_name: &str,
        arguments: Value,
        cwd: PathBuf,
        session_id: String,
        signal: Option<CancellationToken>,
    ) -> Result<Value> {
        let (installed, ticket, cancel) = {
            let mut inner = self.inner()?;
            Self::ensure_open(&inner)?;
            if inner.changing.contains(plugin_id) {
                return Err(PluginError::new("插件正在变更，请稍后重试。"));
            }
            let installed = inner
                .installed
                .iter()
                .find(|v| v.manifest.id == plugin_id)
                .cloned()
                .ok_or_else(|| PluginError::new(format!("本地插件 {plugin_id} 不存在。")))?;
            if !installed.manifest.tools.iter().any(|v| v.name == tool_name) {
                return Err(PluginError::new(format!(
                    "插件 {plugin_id} 不提供能力 {tool_name}。"
                )));
            }
            if !strings(&installed.record["enabledTools"])
                .iter()
                .any(|name| name == tool_name)
            {
                return Err(PluginError::new(format!("插件能力 {tool_name} 已停用。")));
            }
            let ticket = Uuid::new_v4();
            let cancel = CancellationToken::new();
            inner
                .active
                .entry(plugin_id.into())
                .or_default()
                .insert(ticket, cancel.clone());
            (installed, ticket, cancel)
        };
        let lease = ExecutionLease {
            service: self.clone(),
            id: plugin_id.into(),
            ticket,
        };
        let data_dir = self.data_dir.join("plugin-data").join(plugin_id);
        let request = executor::WorkerRequest {
            entry: installed.root.join(installed.manifest.entry),
            plugin_root: installed.root,
            tool_name: tool_name.into(),
            arguments,
            context: executor::ExecutionContext {
                cwd,
                session_id,
                data_dir,
            },
            timeout_ms: self.execution_timeout.as_millis().min(u64::MAX as u128) as u64,
        };
        // A detached task owns the lease and child through actual wait/reaping, even
        // if its caller drops the future while a tool has already started.
        let executable = self.worker_executable.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let _lease = lease;
            executor::run_process(&executable, request, task_cancel, signal).await
        });
        let _drop_cancel = CancelOnDrop(cancel);
        task.await
            .map_err(|error| PluginError::new(error.to_string()))?
    }
    pub async fn close(&self) -> Result<()> {
        {
            let mut inner = self.inner()?;
            inner.closed = true;
            inner.inspections.clear();
            for tokens in inner.active.values() {
                for token in tokens.values() {
                    token.cancel();
                }
            }
        }
        loop {
            let notified = self.drained.notified();
            if self.inner()?.active.is_empty() {
                break;
            }
            notified.await;
        }
        // 与 create 的持锁顺序相同，防止创建正等安装锁时停机反向等待创建锁。
        let _creation = self.creation.lock().await;
        let _mutation = self.mutation.lock().await;
        Ok(())
    }
}
struct Changing {
    service: Arc<ToolPluginService>,
    id: String,
}
impl Drop for Changing {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.service.inner() {
            inner.changing.remove(&self.id);
        }
    }
}
struct ExecutionLease {
    service: Arc<ToolPluginService>,
    id: String,
    ticket: Uuid,
}
impl Drop for ExecutionLease {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.service.inner() {
            if let Some(tokens) = inner.active.get_mut(&self.id) {
                tokens.remove(&self.ticket);
                if tokens.is_empty() {
                    inner.active.remove(&self.id);
                }
            }
        }
        self.service.drained.notify_waiters();
    }
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn public_plugin(record: &Value, manifest: &Manifest) -> Value {
    let enabled = strings(&record["enabledTools"]);
    let capabilities: Vec<Value> = manifest
        .tools
        .iter()
        .map(|tool| {
            let mut value = serde_json::to_value(tool).expect("tool manifest");
            value["category"] = json!("integration");
            value["risk"] = json!("high");
            value["effectiveRisk"] = json!("high");
            value["enabled"] = json!(enabled.contains(&tool.name));
            value
        })
        .collect();
    let mut value = json!({"id":manifest.id,"name":manifest.name,"description":manifest.description,"version":manifest.version,"source":"local","builtIn":false,"enabled":capabilities.iter().any(|v| v["enabled"]==true),"permissions":manifest.permissions,"systemAccess":true,"capabilities":capabilities});
    for field in ["installedAt", "digest"] {
        if let Some(field_value) = record.get(field) {
            value[field] = field_value.clone();
        }
    }
    value
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| PluginError::new(error.to_string()))?
}
fn write_generated(source: &Path, files: &[(String, String)]) -> Result<()> {
    use std::io::Write;
    let mut created = Vec::new();
    let result = (|| -> Result<()> {
        for (path, content) in files {
            let target = source.join(path);
            fs::create_dir_all(
                target
                    .parent()
                    .ok_or_else(|| PluginError::new("插件源码路径无效。"))?,
            )?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            created.push((path.clone(), content.clone()));
            file.write_all(content.as_bytes())?;
        }
        Ok(())
    })();
    if result.is_err() {
        cleanup_generated(source, &created);
    }
    result
}
fn cleanup_generated(source: &Path, files: &[(String, String)]) {
    let mut parents = HashSet::new();
    for (path, content) in files.iter().rev() {
        let target = source.join(path);
        if fs::read(&target).is_ok_and(|bytes| bytes == content.as_bytes()) {
            let _ = fs::remove_file(&target);
        }
        let mut parent = target.parent();
        while let Some(dir) = parent {
            if dir == source {
                break;
            }
            if !dir.starts_with(source) {
                break;
            }
            parents.insert(dir.to_owned());
            parent = dir.parent();
        }
    }
    let mut parents: Vec<_> = parents.into_iter().collect();
    parents.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in parents {
        let _ = fs::remove_dir(dir);
    }
    let _ = fs::remove_dir(source);
}
