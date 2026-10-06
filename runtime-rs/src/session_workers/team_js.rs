//! 受限 QuickJS 编排 worker：无模块加载器、文件、Shell、网络或宿主对象。
//! 同步代码/微任务 drain 五秒预算；等待真实 Agent 的时间不计入同步预算。
use anyhow::{anyhow, bail, Result};
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use rquickjs::{function::Func, Context, Function, Object, Persistent, Runtime};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
const SCRIPT_LIMIT: usize = 256 * 1024;
const RESULT_LIMIT: usize = 1024 * 1024;
type Handler = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value>> + Send + Sync>;
#[derive(Clone, Debug)]
pub(crate) struct Script {
    pub(crate) body: String,
    pub(crate) meta: Value,
    pub(crate) path: String,
    pub(crate) fingerprint: String,
}
fn hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn quoted(chars: &[char], position: &mut usize) -> Result<String> {
    let quote = *chars
        .get(*position)
        .ok_or_else(|| anyhow!("Incomplete workflow meta."))?;
    if !matches!(quote, '\'' | '"') {
        bail!("Workflow meta values must be quoted strings.")
    }
    *position += 1;
    let mut result = String::new();
    while let Some(c) = chars.get(*position).copied() {
        *position += 1;
        if c == quote {
            return Ok(result);
        }
        if c != '\\' {
            result.push(c);
            continue;
        }
        let c = *chars
            .get(*position)
            .ok_or_else(|| anyhow!("Incomplete workflow string escape."))?;
        *position += 1;
        if matches!(c, 'u' | 'x') {
            let count = if c == 'u' { 4 } else { 2 };
            let end = position.saturating_add(count);
            let hex = chars
                .get(*position..end)
                .ok_or_else(|| anyhow!("Invalid workflow string escape."))?
                .iter()
                .collect::<String>();
            let code = u32::from_str_radix(&hex, 16)
                .map_err(|_| anyhow!("Invalid workflow string escape."))?;
            result.push(
                char::from_u32(code).ok_or_else(|| anyhow!("Invalid workflow Unicode escape."))?,
            );
            *position = end;
        } else {
            result.push(match c {
                'b' => '\x08',
                'f' => '\x0c',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'v' => '\x0b',
                '0' => '\0',
                _ => c,
            });
        }
    }
    bail!("Workflow meta contains an incomplete string.")
}
fn parse_meta(source: &str) -> Result<(Value, usize)> {
    let chars = source.chars().collect::<Vec<_>>();
    let mut p = 0;
    let skip = |p: &mut usize| {
        while chars.get(*p).is_some_and(|c| c.is_whitespace()) {
            *p += 1;
        }
    };
    skip(&mut p);
    if chars.get(p) != Some(&'{') {
        bail!("Workflow meta must be a plain object literal.")
    }
    p += 1;
    let mut entries = BTreeMap::new();
    loop {
        skip(&mut p);
        if chars.get(p) == Some(&'}') {
            p += 1;
            break;
        }
        let start = p;
        while chars
            .get(p)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
        {
            p += 1
        }
        let key = chars[start..p].iter().collect::<String>();
        if !matches!(key.as_str(), "name" | "description") || entries.contains_key(&key) {
            bail!("Workflow meta supports one literal name and description only.")
        }
        skip(&mut p);
        if chars.get(p) != Some(&':') {
            bail!("Workflow meta field is missing a colon.")
        }
        p += 1;
        skip(&mut p);
        let value = quoted(&chars, &mut p)?;
        entries.insert(key, value);
        skip(&mut p);
        match chars.get(p) {
            Some(',') => p += 1,
            Some('}') => {}
            _ => bail!("Workflow meta fields must be separated by commas."),
        }
    }
    let name = entries.get("name").map(|v| v.trim()).unwrap_or_default();
    let description = entries
        .get("description")
        .map(|v| v.trim())
        .unwrap_or_default();
    if name.is_empty()
        || description.is_empty()
        || name.encode_utf16().count() > 80
        || description.encode_utf16().count() > 500
    {
        bail!("Workflow meta requires name (80 characters) and description (500 characters).")
    }
    Ok((
        json!({"name":name,"description":description}),
        chars[..p].iter().map(|c| c.len_utf8()).sum(),
    ))
}
pub(crate) fn parse(source: &str, path: String) -> Result<Script> {
    if source.len() > SCRIPT_LIMIT {
        bail!("Team workflow scripts are limited to 262144 bytes.")
    }
    static FORBIDDEN: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let patterns = FORBIDDEN.get_or_init(|| {
        [
            r"\bimport\s*(?:\(|[A-Za-z{*])",
            r"\brequire\s*\(",
            r"\b(?:process|globalThis|global|Buffer|Deno|Bun)\s*[.\[]",
            r"\b(?:eval|Function)\s*\(",
            r"\bDate\s*\.\s*(?:now|parse)\s*\(",
            r"\bMath\s*\.\s*random\s*\(",
            r"\b__pisper[A-Za-z0-9_]*\b",
            r"\bwhile\s*\(\s*(?:true|1)\s*\)",
            r"\bfor\s*\(\s*;\s*;\s*\)",
        ]
        .into_iter()
        .map(|p| regex::Regex::new(p).expect("workflow forbidden pattern"))
        .collect()
    });
    if patterns.iter().any(|p| p.is_match(source)) {
        bail!("Workflow script uses a forbidden capability.")
    }
    let declaration = regex::Regex::new(r"^\s*export\s+const\s+meta\s*=\s*")
        .expect("meta regex")
        .find(source)
        .ok_or_else(|| anyhow!("Workflow script must begin with export const meta."))?;
    let (meta, end) = parse_meta(&source[declaration.end()..])?;
    let body = source[declaration.end() + end..].trim_start().to_string();
    if regex::Regex::new(r"\bexport\s+")
        .expect("export regex")
        .is_match(&body)
    {
        bail!("Workflow scripts may not export values beyond meta.")
    }
    Ok(Script {
        body,
        meta,
        path,
        fingerprint: hash(source.as_bytes()),
    })
}
pub(crate) fn read_script(cwd: &Path, requested: &str) -> Result<Script> {
    let root = std::fs::canonicalize(cwd)?;
    let requested = Path::new(requested.trim());
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let actual = std::fs::canonicalize(candidate)?;
    if !actual.starts_with(&root) {
        bail!("Team workflow scripts must stay inside the current workspace.")
    }
    if std::fs::metadata(&actual)?.len() > SCRIPT_LIMIT as u64 {
        bail!("Team workflow script is too large.")
    }
    let source = std::fs::read_to_string(&actual)?;
    parse(
        &source,
        actual
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/"),
    )
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct Request {
    id: u32,
    payload: String,
}
struct Response {
    id: u32,
    ok: bool,
    payload: String,
}
fn exception(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> anyhow::Error {
    if error.is_exception() {
        let value = ctx.catch();
        let message = value
            .as_object()
            .and_then(|o| o.get::<_, String>("message").ok())
            .unwrap_or_else(|| format!("{value:?}"));
        anyhow!(message)
    } else {
        anyhow!(error.to_string())
    }
}
fn worker(
    body: String,
    args: String,
    cancel: Arc<AtomicBool>,
    requests: tokio::sync::mpsc::UnboundedSender<Request>,
    responses: std::sync::mpsc::Receiver<Response>,
    slice: Duration,
) -> Result<Value> {
    let runtime = Runtime::new().map_err(|e| anyhow!(e.to_string()))?;
    runtime.set_memory_limit(64 * 1024 * 1024);
    runtime.set_max_stack_size(4 * 1024 * 1024);
    let deadline = Arc::new(Mutex::new(Instant::now() + slice));
    let own_deadline = deadline.clone();
    let own_cancel = cancel.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        own_cancel.load(Ordering::SeqCst)
            || Instant::now() >= *own_deadline.lock().expect("workflow deadline")
    })));
    let context = Context::full(&runtime).map_err(|e| anyhow!(e.to_string()))?;
    let state=context.with(|ctx|->Result<Persistent<Object<'static>>>{
        ctx.globals().set("__pisperAgentBridge",Func::from(move|id:u32,payload:String|{let _=requests.send(Request{id,payload});})).map_err(|e|exception(&ctx,e))?;
        let source=format!(r#"(() => {{
            const bridge=globalThis.__pisperAgentBridge; delete globalThis.__pisperAgentBridge;
            const state={{done:false, ok:false, payload:'', settle:null}};
            const pending=new Map(); let sequence=0, currentPhase=''; const logs=[];
            state.settle=(id,ok,payload)=>{{const p=pending.get(id);if(!p)return;pending.delete(id);if(ok)p.resolve(JSON.parse(payload));else p.reject(new Error(payload));}};
            const agent=(prompt,options={{}})=>{{if(++sequence>64)throw new Error('A Team workflow cannot start more than 64 agents.'); const payload=JSON.stringify({{prompt,options:{{...options,__pisperPhase:currentPhase}}}});if(payload.length>1048576)throw new Error('Workflow Agent request is too large.');return new Promise((resolve,reject)=>{{pending.set(sequence,{{resolve,reject}});bridge(sequence,payload);}});}};
            const parallel=async(items)=>{{if(!Array.isArray(items)||items.length>64)throw new Error('Workflow fan-out expects an array of at most 64 items.');return Promise.all(items.map(item=>typeof item==='function'?item():item));}};
            const pipeline=async(items,worker)=>{{if(!Array.isArray(items)||typeof worker!=='function')throw new Error('Workflow pipeline expects an array and a worker function.');return parallel(items.map(item=>()=>worker(item)));}};
            const phase=title=>currentPhase=String(title||'').trim().slice(0,80);
            const log=message=>{{const value=String(message||'').trim();if(value&&logs.length<256)logs.push(value.slice(0,1000));return '';}};
            // 移除所有函数构造器入口，阻止 constructor 别名重新生成代码。
            for(const prototype of [Function.prototype,Object.getPrototypeOf(async()=>{{}}),Object.getPrototypeOf(function*(){{}}),Object.getPrototypeOf(async function*(){{}})])Object.defineProperty(prototype,'constructor',{{value:undefined,writable:false,configurable:false}});
            globalThis.eval=undefined;globalThis.Function=undefined;globalThis.WebAssembly=undefined;
            const api=Object.freeze({{agent,parallel,pipeline,phase,log,logs}});
            (async(__pisperApi,__pisperArgs)=>{{const {{agent,parallel,pipeline,phase,log,logs}}=__pisperApi;const args=__pisperArgs;return await(async()=>{{
                {body}
            }})();}})(api,JSON.parse({args_literal})).then(result=>{{state.payload=JSON.stringify({{result:result===undefined?null:result,logs}});state.ok=true;state.done=true;}}).catch(error=>{{state.payload=String(error?.message||error);state.done=true;}});
            return state;
        }})()"#,args_literal=serde_json::to_string(&args)?);
        let object: Object=ctx.eval(source).map_err(|e|exception(&ctx,e))?;Ok(Persistent::save(&ctx,object))
    })?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("TEAM_WORKFLOW_COMMUNICATION_INTERRUPTED: workflow was cancelled.")
        }
        while runtime.is_job_pending() {
            if Instant::now() >= *deadline.lock().expect("workflow deadline") {
                bail!(
                    "Team workflow script did not yield within {}ms.",
                    slice.as_millis()
                )
            }
            runtime
                .execute_pending_job()
                .map_err(|e| e.0.with(|ctx| exception(&ctx, rquickjs::Error::Exception)))?;
        }
        let completed = context.with(|ctx| -> Result<Option<(bool, String)>> {
            let object = state
                .clone()
                .restore(&ctx)
                .map_err(|e| exception(&ctx, e))?;
            if object
                .get::<_, bool>("done")
                .map_err(|e| exception(&ctx, e))?
            {
                Ok(Some((
                    object.get("ok").map_err(|e| exception(&ctx, e))?,
                    object.get("payload").map_err(|e| exception(&ctx, e))?,
                )))
            } else {
                Ok(None)
            }
        })?;
        if let Some((ok, payload)) = completed {
            if payload.len() > RESULT_LIMIT {
                bail!("Workflow results are limited to 1048576 bytes.")
            }
            if !ok {
                bail!("{payload}")
            }
            return Ok(serde_json::from_str(&payload)?);
        }
        match responses.recv_timeout(Duration::from_millis(50)) {
            Ok(response) => {
                *deadline.lock().expect("workflow deadline") = Instant::now() + slice;
                context.with(|ctx| -> Result<()> {
                    let object = state
                        .clone()
                        .restore(&ctx)
                        .map_err(|e| exception(&ctx, e))?;
                    let settle: Function = object.get("settle").map_err(|e| exception(&ctx, e))?;
                    settle
                        .call::<_, ()>((response.id, response.ok, response.payload))
                        .map_err(|e| exception(&ctx, e))
                })?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("TEAM_WORKFLOW_COMMUNICATION_INTERRUPTED: parent closed the bridge.")
            }
        }
    }
}
pub(crate) async fn execute(script: &Script, args: Value, handler: Handler) -> Result<Value> {
    execute_with_slice(script, args, handler, Duration::from_secs(5)).await
}
async fn execute_with_slice(
    script: &Script,
    args: Value,
    handler: Handler,
    slice: Duration,
) -> Result<Value> {
    let args = serde_json::to_string(&args)?;
    if args.len() > RESULT_LIMIT {
        bail!("Workflow args are limited to 1048576 bytes.")
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    let (request_tx, mut request_rx) = tokio::sync::mpsc::unbounded_channel::<Request>();
    let (response_tx, response_rx) = std::sync::mpsc::channel::<Response>();
    let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
    let body = script.body.clone();
    std::thread::Builder::new()
        .name("pisper-team-workflow".into())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            let result = worker(body, args, cancel, request_tx, response_rx, slice);
            let _ = result_tx.send(result);
        })?;
    let mut pending = FuturesUnordered::<BoxFuture<'static, ()>>::new();
    loop {
        tokio::select! {
            result=&mut result_rx=>return result.map_err(|_|anyhow!("Team workflow worker exited unexpectedly."))?,
            Some(request)=request_rx.recv()=>{let handler=handler.clone();let sender=response_tx.clone();pending.push(Box::pin(async move{
                let result=if request.payload.len()>RESULT_LIMIT{Err(anyhow!("Workflow Agent request is too large."))}else{match serde_json::from_str(&request.payload){Ok(payload)=>handler(payload).await,Err(e)=>Err(e.into())}};
                let (ok,payload)=match result{Ok(value)=>match serde_json::to_string(&value){Ok(text)if text.len()<=RESULT_LIMIT=>(true,text),_=>(false,"Workflow Agent result is too large.".into())},Err(e)=>(false,e.to_string())};let _=sender.send(Response{id:request.id,ok,payload});
            }));},
            _=pending.next(),if !pending.is_empty()=>{},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn worker_fanout_pipeline_and_async_wait_use_real_bridge() {
        let script=parse("export const meta = {name:'fixture',description:'real bridge'}; phase('verify'); log('start'); return await pipeline(args.items, item => agent('item '+item,{label:'unit',schema:{type:'object'}}));","fixture.js".into()).unwrap();
        let handler: Handler = Arc::new(|input| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(90)).await;
                Ok(json!({"prompt":input["prompt"],"phase":input["options"]["__pisperPhase"]}))
            })
        });
        let result = execute_with_slice(
            &script,
            json!({"items":[1,2,3]}),
            handler,
            Duration::from_millis(50),
        )
        .await
        .unwrap();
        assert_eq!(result["result"].as_array().unwrap().len(), 3);
        assert_eq!(result["result"][1]["prompt"], "item 2");
        assert_eq!(result["result"][0]["phase"], "verify");
        assert_eq!(result["logs"], json!(["start"]));
    }
    #[tokio::test]
    async fn worker_limits_sync_execution_and_rejects_dynamic_code() {
        let handler: Handler = Arc::new(|_| Box::pin(async { Ok(Value::Null) }));
        let script=parse("export const meta={name:'loop',description:'bounded'}; let value=0; for(let i=0;i<1e12;i++)value++;return value;","loop.js".into()).unwrap();
        let started = Instant::now();
        assert!(execute_with_slice(
            &script,
            Value::Null,
            handler.clone(),
            Duration::from_millis(25)
        )
        .await
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        let script=parse("export const meta={name:'escape',description:'no code generation'};return (()=>{}).constructor('return 42')();","escape.js".into()).unwrap();
        assert!(execute(&script, Value::Null, handler).await.is_err());
        assert!(parse(
            "export const meta={name:'bad',description:'forbidden'};return process.env;",
            "bad.js".into()
        )
        .is_err());
    }
    #[test]
    fn literal_metadata_and_workspace_boundary_are_enforced() {
        assert!(parse(
            "export const meta={name:'a',name:'b',description:'c'};return 1;",
            "x".into()
        )
        .is_err());
        assert!(parse(
            "export const meta={name:String('a'),description:'b'};return 1;",
            "x".into()
        )
        .is_err());
        assert!(parse(
            "export const meta={name:'a',description:'b'};export default 1;",
            "x".into()
        )
        .is_err());
        let dir = std::env::temp_dir().join(format!("pisper-team-js-{}", crate::product::new_id()));
        std::fs::create_dir_all(dir.join("workspace")).unwrap();
        std::fs::write(
            dir.join("outside.js"),
            "export const meta={name:'a',description:'b'};return 1;",
        )
        .unwrap();
        assert!(read_script(&dir.join("workspace"), "../outside.js").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
