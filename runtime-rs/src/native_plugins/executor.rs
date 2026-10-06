use super::{
    node_host, paths::display_path, PluginError, Result, MAX_PLUGIN_BYTES, MAX_RESULT_BYTES,
};
use rquickjs::{
    function::Func,
    loader::{ImportAttributes, Loader, Resolver},
    module::Declared,
    Context, Ctx, Exception, Function, Module, Object, Runtime,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

const MAX_REQUEST_BYTES: usize = MAX_PLUGIN_BYTES + MAX_RESULT_BYTES;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionContext {
    pub cwd: PathBuf,
    pub session_id: String,
    pub data_dir: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRequest {
    pub entry: PathBuf,
    pub plugin_root: PathBuf,
    pub tool_name: String,
    pub arguments: Value,
    pub context: ExecutionContext,
    pub timeout_ms: u64,
}

/// Early executable entry point; stdin is one bounded JSON request. The OS process
/// is also killable by its parent during a blocking native operation.
pub fn worker_main() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(PluginError::new("插件执行请求过大。"));
    }
    let request: WorkerRequest = serde_json::from_slice(&bytes)?;
    let value = match execute_embedded(&request, Arc::new(AtomicBool::new(false))) {
        Ok(result) => json!({"type":"result","result":result}),
        Err(error) => json!({"type":"error","error":{"message":error.message}}),
    };
    let output = serde_json::to_vec(&value)?;
    if output.len() > MAX_RESULT_BYTES + 64 * 1024 {
        return Err(PluginError::new("插件返回结果超过 1 MB 限制。"));
    }
    std::io::stdout().lock().write_all(&output)?;
    Ok(())
}
pub(crate) async fn run_process(
    executable: &Path,
    request: WorkerRequest,
    cancel: CancellationToken,
    signal: Option<CancellationToken>,
) -> Result<Value> {
    if cancel.is_cancelled() || signal.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return Err(PluginError::new("插件执行已取消。"));
    }
    tokio::fs::create_dir_all(&request.context.data_dir).await?;
    let input = serde_json::to_vec(&request)?;
    if input.len() > MAX_REQUEST_BYTES {
        return Err(PluginError::new("插件执行请求过大。"));
    }
    let timeout = Duration::from_millis(request.timeout_ms.min(super::EXECUTION_TIMEOUT_MS));
    let mut command = Command::new(executable);
    command
        .arg("--pisper-plugin-worker")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PluginError::new("插件 Worker 输入不可用。"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PluginError::new("插件 Worker 输出不可用。"))?;
    let input_task = tokio::spawn(async move {
        stdin.write_all(&input).await?;
        stdin.shutdown().await
    });
    let output_task = tokio::spawn(async move {
        let mut output = Vec::new();
        stdout
            .take((MAX_RESULT_BYTES + 64 * 1024 + 1) as u64)
            .read_to_end(&mut output)
            .await?;
        Ok::<_, std::io::Error>(output)
    });
    let aborted = async {
        if let Some(signal) = signal {
            signal.cancelled().await
        } else {
            std::future::pending::<()>().await
        }
    };
    let result = tokio::select! { biased;
        _=cancel.cancelled()=>Err(PluginError::new("插件执行已取消。")),
        _=aborted=>Err(PluginError::new("插件执行已取消。")),
        _=tokio::time::sleep(timeout)=>Err(PluginError::new("插件执行超过 120 秒，已终止。")),
        status=child.wait()=>status.map_err(PluginError::from),
    };
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        input_task.abort();
        output_task.abort();
        let _ = input_task.await;
        let _ = output_task.await;
        return Err(result.unwrap_err());
    }
    let status = result?;
    let written = input_task
        .await
        .map_err(|e| PluginError::new(e.to_string()))?;
    let output = output_task
        .await
        .map_err(|e| PluginError::new(e.to_string()))??;
    if output.len() > MAX_RESULT_BYTES + 64 * 1024 {
        return Err(PluginError::new("插件返回结果超过 1 MB 限制。"));
    }
    if !status.success() {
        return Err(PluginError::new(format!(
            "插件 Worker 异常退出（code {}）。",
            status
                .code()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "signal".into())
        )));
    }
    written?;
    let response: Value = serde_json::from_slice(&output)
        .map_err(|_| PluginError::new("插件 Worker 返回了无效协议。"))?;
    if response["type"] == "error" {
        return Err(PluginError::new(
            response["error"]["message"]
                .as_str()
                .unwrap_or("插件执行失败。"),
        ));
    }
    if response["type"] != "result" {
        return Err(PluginError::new("插件 Worker 返回了无效协议。"));
    }
    let result = response["result"].clone();
    if serde_json::to_vec(&result)?.len() > MAX_RESULT_BYTES {
        return Err(PluginError::new("插件返回结果超过 1 MB 限制。"));
    }
    Ok(result)
}

struct ModuleResolver;
impl Resolver for ModuleResolver {
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        node_host::resolve_module(Path::new(base), name)
            .map_err(|error| Exception::throw_message(ctx, &error.message))
    }
}
struct ModuleLoader;
impl Loader for ModuleLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        let source = if name.starts_with("node:") {
            let exports=match name {
                "node:fs/promises"=>"readFile writeFile appendFile mkdir readdir stat lstat realpath access rename copyFile unlink rmdir rm constants",
                "node:fs"=>"readFile writeFile appendFile mkdir readdir stat lstat realpath access rename copyFile unlink rmdir rm readFileSync writeFileSync appendFileSync mkdirSync readdirSync statSync lstatSync realpathSync accessSync existsSync renameSync copyFileSync unlinkSync rmdirSync rmSync promises constants",
                "node:path"=>"join resolve normalize dirname basename extname isAbsolute sep delimiter",
                "node:buffer"=>"Buffer","node:process"=>"env cwd platform arch pid nextTick",
                "node:timers"=>"setTimeout clearTimeout setInterval clearInterval setImmediate clearImmediate",
                "node:timers/promises"=>"setTimeout","node:os"=>"platform arch tmpdir EOL",
                _=>return Err(Exception::throw_message(ctx,"ERR_PISPER_NODE_COMPAT: Node built-in is not implemented")),
            };
            format!(
                "const api=globalThis.__pisperNodeModules[{}];export default api;{}",
                serde_json::to_string(name).expect("module name"),
                exports
                    .split_whitespace()
                    .map(|key| format!("export const {key}=api.{key};"))
                    .collect::<String>()
            )
        } else {
            let path = Path::new(name);
            let metadata =
                fs::metadata(path).map_err(|e| Exception::throw_message(ctx, &e.to_string()))?;
            if !metadata.is_file() || metadata.len() > MAX_PLUGIN_BYTES as u64 {
                return Err(Exception::throw_message(
                    ctx,
                    "JavaScript module file is invalid or too large",
                ));
            }
            let raw = fs::read_to_string(path)
                .map_err(|e| Exception::throw_message(ctx, &e.to_string()))?;
            if path.extension().is_some_and(|v| v == "json") {
                if attributes
                    .as_ref()
                    .map(|v| v.get_type())
                    .transpose()?
                    .flatten()
                    .as_deref()
                    != Some("json")
                {
                    return Err(Exception::throw_message(
                        ctx,
                        "ERR_IMPORT_ATTRIBUTE_MISSING: JSON module requires type=json",
                    ));
                }
                let _: Value = serde_json::from_str(&raw)
                    .map_err(|e| Exception::throw_message(ctx, &e.to_string()))?;
                format!(
                    "export default JSON.parse({});",
                    serde_json::to_string(&raw).expect("module JSON")
                )
            } else if node_host::is_commonjs(path, &raw) {
                format!("const value=globalThis.__pisperLoadCjs({});export default value;export const execute=value?.execute;",serde_json::to_string(name).expect("CJS file"))
            } else {
                let url = reqwest::Url::from_file_path(path)
                    .map_err(|_| Exception::throw_message(ctx, "Module file URL is invalid"))?;
                let raw = if raw.starts_with("#!") {
                    raw.find('\n').map(|index| &raw[index..]).unwrap_or("")
                } else {
                    &raw
                };
                format!(
                    "import.meta.url={};import.meta.filename={};import.meta.dirname={};\n{raw}",
                    serde_json::to_string(url.as_str()).expect("file URL"),
                    serde_json::to_string(name).expect("filename"),
                    serde_json::to_string(
                        &path.parent().unwrap_or(Path::new(".")).to_string_lossy()
                    )
                    .expect("dirname")
                )
            }
        };
        Module::declare(ctx.clone(), name, source)
    }
}
fn exception(ctx: &Ctx<'_>, error: rquickjs::Error) -> PluginError {
    if error.is_exception() {
        let value = ctx.catch();
        let message = value
            .as_object()
            .and_then(|v| v.get::<_, String>("message").ok())
            .unwrap_or_else(|| format!("{value:?}"));
        PluginError::new(message)
    } else {
        PluginError::new(error.to_string())
    }
}
pub(crate) fn execute_embedded(request: &WorkerRequest, cancel: Arc<AtomicBool>) -> Result<Value> {
    let deadline =
        Instant::now() + Duration::from_millis(request.timeout_ms.min(super::EXECUTION_TIMEOUT_MS));
    let result = execute_embedded_inner(request, cancel.clone(), deadline);
    if result.is_err() {
        if cancel.load(Ordering::SeqCst) {
            return Err(PluginError::new("插件执行已取消。"));
        }
        if Instant::now() >= deadline {
            return Err(PluginError::new("插件执行超过 120 秒，已终止。"));
        }
    }
    result
}
fn execute_embedded_inner(
    request: &WorkerRequest,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) -> Result<Value> {
    let runtime = Runtime::new().map_err(|e| PluginError::new(e.to_string()))?;
    runtime.set_memory_limit(128 * 1024 * 1024);
    runtime.set_max_stack_size(4 * 1024 * 1024);
    let own_cancel = cancel.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        own_cancel.load(Ordering::SeqCst) || Instant::now() >= deadline
    })));
    runtime.set_loader(ModuleResolver, ModuleLoader);
    let context = Context::full(&runtime).map_err(|e| PluginError::new(e.to_string()))?;
    context.with(|ctx|->Result<()> {
        ctx.globals().set("__pisperNative",Func::from(|operation:String,payload:String|node_host::dispatch(&operation,&payload))).map_err(|e|exception(&ctx,e))?;
        ctx.eval::<(),_>(node_host::BOOTSTRAP).map_err(|e|exception(&ctx,e))?;
        ctx.eval::<(),_>("globalThis.__pisperPluginState={done:false,ok:false,payload:''};").map_err(|e|exception(&ctx,e))?;
        // Only the plugin-facing context is displayed like Node. The worker's
        // request and canonical entry/root paths retain their filesystem form.
        let input=json!({"toolName":request.tool_name,"arguments":request.arguments,"context":{
            "cwd":display_path(&request.context.cwd),"sessionId":request.context.session_id,
            "dataDir":display_path(&request.context.data_dir)}});
        let source=format!(r#"import * as plugin from {entry};
            const state=globalThis.__pisperPluginState;
            (async()=>{{
                const execute=plugin.execute||plugin.default?.execute||plugin.default;
                if(typeof execute!=='function')throw new Error('插件入口必须导出 execute 函数。');
                const result=await execute({input});
                let normalized;
                try{{
                    if(typeof result==='string')normalized={{content:[{{type:'text',text:result}}],details:{{}}}};
                    else if(result&&typeof result==='object'&&Array.isArray(result.content))normalized=result;
                    else normalized={{content:[{{type:'text',text:JSON.stringify(result??null,null,2)}}],details:{{}}}};
                    const serialized=JSON.stringify(normalized);
                    state.payload=serialized;
                }}catch(error){{if(error?.message==='插件返回结果超过 1 MB 限制。')throw error;throw new Error('插件返回了无法序列化的结果。');}}
                state.ok=true;state.done=true;
            }})().catch(error=>{{state.payload=String(error?.message||error||'插件执行失败');state.done=true;}});"#,
            entry=serde_json::to_string(&request.entry.to_string_lossy())?,input=serde_json::to_string(&input)?);
        let promise=Module::evaluate(ctx.clone(),"pisper-plugin-invocation.mjs",source).map_err(|e|exception(&ctx,e))?;
        ctx.globals().set("__pisperModulePromise",promise).map_err(|e|exception(&ctx,e))?;
        ctx.eval::<(),_>("__pisperModulePromise.catch(error=>{const state=__pisperPluginState;state.payload=String(error?.message||error);state.done=true;});delete globalThis.__pisperModulePromise;").map_err(|e|exception(&ctx,e))?;
        Ok(())
    })?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(PluginError::new("插件执行已取消。"));
        }
        if Instant::now() >= deadline {
            return Err(PluginError::new("插件执行超过 120 秒，已终止。"));
        }
        while runtime.is_job_pending() {
            runtime.execute_pending_job().map_err(|error| {
                error
                    .0
                    .with(|ctx| exception(&ctx, rquickjs::Error::Exception))
            })?;
        }
        let result = context.with(|ctx| -> Result<Option<(bool, String)>> {
            let pump: Function = ctx
                .globals()
                .get("__pisperPumpTimers")
                .map_err(|e| exception(&ctx, e))?;
            pump.call::<_, ()>(()).map_err(|e| exception(&ctx, e))?;
            let state: Object = ctx
                .globals()
                .get("__pisperPluginState")
                .map_err(|e| exception(&ctx, e))?;
            if state
                .get::<_, bool>("done")
                .map_err(|e| exception(&ctx, e))?
            {
                Ok(Some((
                    state.get("ok").map_err(|e| exception(&ctx, e))?,
                    state.get("payload").map_err(|e| exception(&ctx, e))?,
                )))
            } else {
                Ok(None)
            }
        })?;
        if let Some((ok, payload)) = result {
            if !ok {
                return Err(PluginError::new(payload));
            }
            // Rust String 的长度就是 UTF-8 字节数；不让 JS Buffer 桥创建百万元素字节数组。
            if payload.len() > MAX_RESULT_BYTES {
                return Err(PluginError::new("插件返回结果超过 1 MB 限制。"));
            }
            return serde_json::from_str(&payload).map_err(PluginError::from);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
