use super::{
    catalog::{Catalog, Model, ModelFile, MARKER},
    error::{download, Result},
    storage,
};
use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::{sync::Notify, task::JoinHandle};
use tokio_util::sync::CancellationToken;
static QUEUE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
#[derive(Clone)]
struct State {
    status: String,
    bytes: u64,
    error: Option<String>,
}
struct Task {
    token: CancellationToken,
    done: AtomicBool,
    notify: Notify,
    handle: Mutex<Option<JoinHandle<()>>>,
}
pub struct SpeechDownloads {
    pub catalog: Catalog,
    root: PathBuf,
    client: reqwest::Client,
    states: Mutex<HashMap<String, State>>,
    tasks: Mutex<HashMap<String, Arc<Task>>>,
    proofs: Mutex<HashMap<String, Vec<(String, String)>>>,
    closed: AtomicBool,
}
impl SpeechDownloads {
    pub fn new(agent_dir: &Path, catalog: Catalog) -> Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| download("http"))?;
        Ok(Arc::new(Self {
            catalog,
            root: agent_dir.join("speech-models"),
            client,
            states: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            proofs: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }))
    }
    fn set(&self, id: &str, status: &str, bytes: u64, error: Option<&str>) {
        if let Ok(mut states) = self.states.lock() {
            states.insert(
                id.to_owned(),
                State {
                    status: status.into(),
                    bytes,
                    error: error.map(str::to_owned),
                },
            );
        }
    }
    fn snapshot(&self, model: &Model) -> Value {
        let state = self
            .states
            .lock()
            .ok()
            .and_then(|states| states.get(&model.id).cloned())
            .unwrap_or(State {
                status: "not-installed".into(),
                bytes: 0,
                error: None,
            });
        model.public(&state.status, state.bytes, state.error.as_deref())
    }
    pub async fn list(self: &Arc<Self>) -> Result<Value> {
        let mut models = Vec::new();
        for model in &self.catalog.models {
            models.push(self.status(&model.id).await?);
        }
        Ok(json!({"defaults":self.catalog.defaults,"models":models}))
    }
    pub async fn status(self: &Arc<Self>, id: &str) -> Result<Value> {
        let model = self.catalog.model(id)?.clone();
        let inflight = self
            .tasks
            .lock()
            .map_err(|_| download("storage"))?
            .get(id)
            .is_some_and(|task| !task.done.load(Ordering::Acquire));
        if !inflight {
            let store = self.clone();
            let candidate = model.clone();
            let installed = tokio::task::spawn_blocking(move || {
                store.verify(&candidate, &CancellationToken::new())
            })
            .await
            .map_err(|_| download("storage"))?
            .unwrap_or(false);
            let inflight = self
                .tasks
                .lock()
                .map_err(|_| download("storage"))?
                .get(id)
                .is_some_and(|task| !task.done.load(Ordering::Acquire));
            if !inflight {
                if installed {
                    self.set(id, "installed", model.total_bytes(), None);
                } else {
                    let mut states = self.states.lock().map_err(|_| download("storage"))?;
                    if states.get(id).is_some_and(|s| s.status == "installed") {
                        states.remove(id);
                    }
                }
            }
        }
        Ok(self.snapshot(&model))
    }
    pub async fn model_directory(self: &Arc<Self>, id: &str) -> Result<PathBuf> {
        let model = self.catalog.model(id)?.clone();
        let store = self.clone();
        let valid =
            tokio::task::spawn_blocking(move || store.verify(&model, &CancellationToken::new()))
                .await
                .map_err(|_| download("missing"))?
                .unwrap_or(false);
        if valid {
            Ok(self.root.join(id))
        } else {
            Err(download("missing"))
        }
    }
    pub fn start(self: &Arc<Self>, id: &str) -> Result<Value> {
        let model = self.catalog.model(id)?.clone();
        let mut tasks = self.tasks.lock().map_err(|_| download("storage"))?;
        if self.closed.load(Ordering::Acquire) {
            return Err(download("disposed"));
        }
        if tasks
            .get(id)
            .is_some_and(|task| !task.done.load(Ordering::Acquire))
        {
            return Ok(self.snapshot(&model));
        }
        self.proofs
            .lock()
            .map_err(|_| download("storage"))?
            .remove(id);
        self.set(id, "downloading", 0, None);
        let task = Arc::new(Task {
            token: CancellationToken::new(),
            done: AtomicBool::new(false),
            notify: Notify::new(),
            handle: Mutex::new(None),
        });
        let worker = task.clone();
        let store = self.clone();
        let target = model.clone();
        let handle = tokio::spawn(async move {
            let queue = QUEUE.get_or_init(|| tokio::sync::Mutex::new(()));
            let result = tokio::select! {_=worker.token.cancelled()=>Err(download("cancelled")),guard=queue.lock()=>{let _guard=guard;store.run(&target,&worker.token).await}};
            if let Err(error) = result {
                let bytes = store
                    .states
                    .lock()
                    .ok()
                    .and_then(|s| s.get(&target.id).map(|s| s.bytes))
                    .unwrap_or(0);
                store.set(
                    &target.id,
                    if error.code == "cancelled" {
                        "cancelled"
                    } else {
                        "error"
                    },
                    bytes,
                    if error.code == "cancelled" {
                        None
                    } else {
                        Some(error.message)
                    },
                );
            }
            worker.done.store(true, Ordering::Release);
            worker.notify.notify_waiters();
        });
        *task.handle.lock().map_err(|_| download("storage"))? = Some(handle);
        tasks.insert(id.to_owned(), task);
        Ok(self.snapshot(&model))
    }
    async fn wait(task: &Arc<Task>) {
        loop {
            let notified = task.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if task.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
        let handle = task.handle.lock().ok().and_then(|mut h| h.take());
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
    pub async fn download(self: &Arc<Self>, id: &str) -> Result<Value> {
        self.start(id)?;
        let task = self
            .tasks
            .lock()
            .map_err(|_| download("storage"))?
            .get(id)
            .cloned()
            .ok_or_else(|| download("storage"))?;
        Self::wait(&task).await;
        let state = self.status(id).await?;
        if state["status"] != "installed" {
            return Err(download(if state["status"] == "cancelled" {
                "cancelled"
            } else {
                "storage"
            }));
        }
        Ok(state)
    }
    pub async fn cancel(self: &Arc<Self>, id: &str) -> Result<Value> {
        self.catalog.model(id)?;
        let task = self
            .tasks
            .lock()
            .map_err(|_| download("storage"))?
            .get(id)
            .cloned();
        if let Some(task) = task {
            task.token.cancel();
            Self::wait(&task).await;
        }
        self.status(id).await
    }
    pub async fn shutdown(self: &Arc<Self>) {
        let tasks = {
            let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
            self.closed.store(true, Ordering::Release);
            tasks.values().cloned().collect::<Vec<_>>()
        };
        for task in &tasks {
            task.token.cancel();
        }
        for task in tasks {
            Self::wait(&task).await;
        }
        if let Ok(mut proofs) = self.proofs.lock() {
            proofs.clear();
        }
    }
    fn verify(&self, model: &Model, token: &CancellationToken) -> Result<bool> {
        storage::cancelled(token)?;
        let directory = self.root.join(&model.id);
        if !storage::directory(&directory, false)? {
            return Ok(false);
        }
        let before = tree(&directory, model, true)?;
        if self
            .proofs
            .lock()
            .map_err(|_| download("storage"))?
            .get(&model.id)
            == Some(&before)
        {
            return Ok(true);
        }
        let marker = match storage::bounded_read(&directory.join(MARKER), 1024 * 1024) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).ok(),
            Err(_) => None,
        };
        if marker.as_ref() != Some(&model.marker()) {
            return Ok(false);
        }
        for file in &model.files {
            if storage::digest_file(&directory.join(&file.path), file.bytes, token)? != file.sha256
            {
                return Ok(false);
            }
        }
        let after = tree(&directory, model, true)?;
        storage::cancelled(token)?;
        if before != after {
            return Ok(false);
        }
        if !self.closed.load(Ordering::Acquire) {
            self.proofs
                .lock()
                .map_err(|_| download("storage"))?
                .insert(model.id.clone(), after);
        }
        Ok(true)
    }
    async fn run(self: &Arc<Self>, model: &Model, token: &CancellationToken) -> Result<()> {
        storage::cancelled(token)?;
        storage::directory(&self.root, true)?;
        let store = self.clone();
        let candidate = model.clone();
        let check = token.clone();
        if tokio::task::spawn_blocking(move || store.verify(&candidate, &check))
            .await
            .map_err(|_| download("storage"))?
            .unwrap_or(false)
        {
            self.set(&model.id, "installed", model.total_bytes(), None);
            return Ok(());
        }
        let staging = self
            .root
            .join(format!(".{}.{}.partial", model.id, model.fingerprint));
        storage::directory(&staging, true)?;
        tree(&staging, model, false)?;
        let mut completed = 0;
        let archive_path = model.archive.as_ref().map(|_| {
            self.root
                .join(format!(".{}.{}.tar.bz2", model.id, model.fingerprint))
        });
        if let (Some(archive), Some(archive_path)) = (&model.archive, &archive_path) {
            self.download_file(
                model,
                &ModelFile {
                    path: String::new(),
                    bytes: archive.bytes,
                    sha256: archive.sha256.clone(),
                    urls: archive.urls.clone(),
                },
                archive_path,
                0,
                token,
            )
            .await?;
            completed = archive.bytes;
            self.set(&model.id, "verifying", completed, None);
            tree(&staging, model, false)?;
            checked_remove(&staging, &self.root)?;
            storage::directory(&staging, true)?;
            let candidate = model.clone();
            let archive_path = archive_path.clone();
            let output = staging.clone();
            let cancel = token.clone();
            // Await the blocking owner: cancellation cannot leave an extraction writer detached.
            tokio::task::spawn_blocking(move || {
                extract(&candidate, &archive_path, &output, &cancel)
            })
            .await
            .map_err(|_| download("integrity"))??;
        } else {
            for file in &model.files {
                storage::cancelled(token)?;
                let path = staging.join(&file.path);
                storage::directory(path.parent().ok_or_else(|| download("path"))?, true)?;
                self.download_file(model, file, &path, completed, token)
                    .await?;
                completed += file.bytes;
            }
        }
        self.set(&model.id, "verifying", completed, None);
        let candidate = model.clone();
        let output = staging.clone();
        let cancel = token.clone();
        tokio::task::spawn_blocking(move || {
            for file in &candidate.files {
                if storage::digest_file(&output.join(&file.path), file.bytes, &cancel)?
                    != file.sha256
                {
                    return Err(download("integrity"));
                }
            }
            tree(&output, &candidate, true)?;
            storage::cancelled(&cancel)?;
            storage::write_json(&output.join(MARKER), &candidate.marker())
        })
        .await
        .map_err(|_| download("storage"))??;
        storage::cancelled(token)?;
        self.proofs
            .lock()
            .map_err(|_| download("storage"))?
            .remove(&model.id);
        let destination = self.root.join(&model.id);
        let backup = self
            .root
            .join(format!(".{}.{}.previous", model.id, uuid::Uuid::new_v4()));
        let existing = storage::directory(&destination, false)?;
        if existing {
            tree(&destination, model, true)?;
            fs::rename(&destination, &backup).map_err(|_| download("storage"))?;
        }
        if fs::rename(&staging, &destination).is_err() {
            if existing {
                let _ = fs::rename(&backup, &destination);
            }
            return Err(download("storage"));
        }
        if existing {
            let _ = checked_remove(&backup, &self.root);
        }
        if let Some(path) = archive_path {
            if storage::checked_file(&path, false, false).is_ok() {
                let _ = fs::remove_file(path);
            }
        }
        self.set(&model.id, "installed", model.total_bytes(), None);
        Ok(())
    }
    async fn download_file(
        &self,
        model: &Model,
        file: &ModelFile,
        path: &Path,
        completed: u64,
        token: &CancellationToken,
    ) -> Result<()> {
        let mut last = download("http");
        for url in &file.urls {
            match self
                .from_url(model, file, path, completed, token, url)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    storage::cancelled(token)?;
                    if ["path", "storage"].contains(&error.code) {
                        return Err(error);
                    }
                    last = error;
                }
            }
        }
        Err(last)
    }
    async fn from_url(
        &self,
        model: &Model,
        file: &ModelFile,
        path: &Path,
        completed: u64,
        token: &CancellationToken,
        initial: &str,
    ) -> Result<()> {
        storage::cancelled(token)?;
        let mut output = storage::checked_file(path, true, true)?;
        let mut offset = output.metadata().map_err(|_| download("storage"))?.len();
        let mut hash = Sha256::new();
        if offset > file.bytes {
            output.set_len(0).map_err(|_| download("storage"))?;
            offset = 0;
        } else {
            let mut buffer = [0; 128 * 1024];
            loop {
                storage::cancelled(token)?;
                let n = output.read(&mut buffer).map_err(|_| download("storage"))?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
        }
        if offset == file.bytes {
            if hex(hash.clone().finalize().as_slice()) == file.sha256 {
                self.set(&model.id, "downloading", completed + offset, None);
                return Ok(());
            }
            output.set_len(0).map_err(|_| download("storage"))?;
            offset = 0;
            hash = Sha256::new();
        }
        self.set(&model.id, "downloading", completed + offset, None);
        let started = Instant::now();
        let mut url = initial.to_owned();
        let mut redirects = 0;
        let response = loop {
            storage::cancelled(token)?;
            let mut request = self.client.get(&url).header("Accept-Encoding", "identity");
            if offset > 0 {
                request = request.header("Range", format!("bytes={offset}-"));
            }
            let response = tokio::select! {_=token.cancelled()=>return Err(download("cancelled")),result=tokio::time::timeout(Duration::from_secs(30),request.send())=>result.map_err(|_|download("timeout"))?.map_err(|_|download("http"))?};
            if !response.status().is_redirection() {
                break response;
            }
            if redirects >= 2 {
                return Err(download("redirect"));
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| download("redirect"))?;
            let next = reqwest::Url::parse(&url)
                .map_err(|_| download("redirect"))?
                .join(location)
                .map_err(|_| download("redirect"))?;
            let allowed = [
                "hf-mirror.com",
                "cas-bridge.xethub.hf.co",
                "release-assets.githubusercontent.com",
            ];
            if !super::catalog::public_url(next.as_str())
                || (!file.urls.iter().any(|u| u == next.as_str())
                    && !allowed.contains(&next.host_str().unwrap_or("")))
            {
                return Err(download("redirect"));
            }
            url = next.to_string();
            redirects += 1;
        };
        let status = response.status().as_u16();
        if status != 200 && status != 206 {
            return Err(download("http"));
        }
        let headers = response.headers();
        if headers
            .get("content-encoding")
            .is_some_and(|v| v != "identity")
        {
            return Err(download("size"));
        }
        if status == 206 {
            if offset == 0
                || headers.get("content-range").and_then(|v| v.to_str().ok())
                    != Some(format!("bytes {offset}-{}/{}", file.bytes - 1, file.bytes).as_str())
            {
                return Err(download("range"));
            }
        } else {
            if headers.contains_key("content-range") {
                return Err(download("range"));
            }
            offset = 0;
            hash = Sha256::new();
            output.set_len(0).map_err(|_| download("storage"))?;
        }
        if let Some(length) = headers.get("content-length") {
            if !length.to_str().is_ok_and(|value| {
                regex::Regex::new(r"^(0|[1-9][0-9]*)$")
                    .unwrap()
                    .is_match(value)
            }) || length.to_str().ok().and_then(|v| v.parse::<u64>().ok())
                != Some(file.bytes - offset)
            {
                return Err(download("size"));
            }
        }
        output
            .seek(SeekFrom::Start(offset))
            .map_err(|_| download("storage"))?;
        let mut chunks = response.bytes_stream();
        loop {
            if started.elapsed() > Duration::from_secs(1800) {
                return Err(download("timeout"));
            }
            let chunk = tokio::select! {_=token.cancelled()=>return Err(download("cancelled")),result=tokio::time::timeout(Duration::from_secs(30),chunks.next())=>result.map_err(|_|download("timeout"))?};
            let Some(chunk) = chunk else {
                break;
            };
            let bytes = chunk.map_err(|_| download("http"))?;
            if bytes.len() as u64 > file.bytes - offset {
                return Err(download("size"));
            }
            output.write_all(&bytes).map_err(|_| download("storage"))?;
            hash.update(&bytes);
            offset += bytes.len() as u64;
            self.set(&model.id, "downloading", completed + offset, None);
        }
        if offset != file.bytes {
            return Err(download("size"));
        }
        if hex(hash.finalize().as_slice()) != file.sha256 {
            output.set_len(0).map_err(|_| download("storage"))?;
            self.set(&model.id, "downloading", completed, None);
            return Err(download("integrity"));
        }
        output.sync_all().map_err(|_| download("storage"))?;
        Ok(())
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
pub(crate) fn tree(root: &Path, model: &Model, marker: bool) -> Result<Vec<(String, String)>> {
    storage::directory(root, false)?;
    let mut proof = Vec::new();
    let mut pending = vec![(root.to_owned(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|_| download("storage"))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| download("storage"))?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| download("path"))?;
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let info = fs::symlink_metadata(entry.path()).map_err(|_| download("storage"))?;
            if storage::linked(&info) {
                return Err(download("path"));
            }
            if info.is_dir()
                && model
                    .files
                    .iter()
                    .any(|f| f.path.starts_with(&format!("{relative}/")))
            {
                pending.push((entry.path(), relative));
            } else if info.is_file()
                && (model.files.iter().any(|f| f.path == relative)
                    || (marker && relative == MARKER))
            {
                proof.push((relative, storage::file_stamp(&entry.path())?));
            } else {
                return Err(download("path"));
            }
        }
    }
    proof.sort();
    Ok(proof)
}
fn checked_remove(path: &Path, root: &Path) -> Result<()> {
    if path.parent() != Some(root)
        || !path
            .file_name()
            .is_some_and(|v| v.to_string_lossy().starts_with('.'))
    {
        return Err(download("path"));
    }
    storage::directory(root, false)?;
    let mut pending = vec![path.to_owned()];
    while let Some(directory) = pending.pop() {
        storage::directory(&directory, false)?;
        for entry in fs::read_dir(&directory).map_err(|_| download("storage"))? {
            let entry = entry.map_err(|_| download("storage"))?;
            let info = fs::symlink_metadata(entry.path()).map_err(|_| download("storage"))?;
            if storage::linked(&info) {
                return Err(download("path"));
            }
            if info.is_dir() {
                pending.push(entry.path());
            } else {
                storage::checked_file(&entry.path(), false, false)?;
            }
        }
    }
    fs::remove_dir_all(path).map_err(|_| download("storage"))
}
fn extract(model: &Model, input: &Path, output: &Path, token: &CancellationToken) -> Result<()> {
    let archive = model.archive.as_ref().ok_or_else(|| download("catalog"))?;
    let file = storage::checked_file(input, false, false)?;
    let mut source = bzip2::read::BzDecoder::new(file);
    let limit = model.files.iter().map(|f| f.bytes).sum::<u64>() + 32 * 1024 * 1024;
    let entry_limit = model
        .files
        .iter()
        .map(|f| f.bytes)
        .max()
        .unwrap_or(0)
        .max(32 * 1024 * 1024);
    let started = Instant::now();
    let mut total = 0u64;
    let mut count = 0;
    let mut seen = HashMap::<String, bool>::new();
    let mut found = HashSet::new();
    let mut extended_path = None::<String>;
    loop {
        storage::cancelled(token)?;
        if started.elapsed() > Duration::from_secs(1800) {
            return Err(download("timeout"));
        }
        let mut block = [0u8; 512];
        match source.read_exact(&mut block) {
            Ok(()) => {}
            Err(_) => return Err(download("integrity")),
        }
        total += 512;
        if total > limit {
            return Err(download("size"));
        }
        if block.iter().all(|v| *v == 0) {
            break;
        }
        count += 1;
        if count > 4096 {
            return Err(download("size"));
        }
        let header = tar::Header::from_byte_slice(&block);
        let sum = block
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    *v as u32
                }
            })
            .sum::<u32>();
        if header.cksum().map_err(|_| download("integrity"))? != sum {
            return Err(download("integrity"));
        }
        let size = header.entry_size().map_err(|_| download("size"))?;
        let kind = header.entry_type();
        if size > entry_limit || total.checked_add(size).is_none_or(|v| v > limit) {
            return Err(download("size"));
        }
        if kind.is_pax_local_extensions()
            || kind.is_pax_global_extensions()
            || kind.is_gnu_longname()
        {
            if size == 0 || size > 64 * 1024 {
                return Err(download("size"));
            }
            let mut bytes = vec![0; size as usize];
            source
                .read_exact(&mut bytes)
                .map_err(|_| download("integrity"))?;
            if kind.is_gnu_longname() {
                extended_path = Some(
                    String::from_utf8(bytes)
                        .map_err(|_| download("path"))?
                        .trim_end_matches('\0')
                        .to_owned(),
                );
            } else {
                let mut position = 0;
                while position < bytes.len() {
                    let space = bytes[position..]
                        .iter()
                        .position(|v| *v == b' ')
                        .map(|v| v + position)
                        .ok_or_else(|| download("integrity"))?;
                    let digits = &bytes[position..space];
                    if digits.is_empty()
                        || digits.len() > 8
                        || digits[0] == b'0'
                        || !digits.iter().all(u8::is_ascii_digit)
                    {
                        return Err(download("integrity"));
                    }
                    let length = std::str::from_utf8(digits)
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                        .filter(|v| *v > 0)
                        .ok_or_else(|| download("integrity"))?;
                    let end = position
                        .checked_add(length)
                        .filter(|v| *v <= bytes.len() && *v > space + 2)
                        .ok_or_else(|| download("integrity"))?;
                    if bytes[end - 1] != b'\n' {
                        return Err(download("integrity"));
                    }
                    let record = std::str::from_utf8(&bytes[space + 1..end - 1])
                        .map_err(|_| download("integrity"))?;
                    let (key, value) = record
                        .split_once('=')
                        .ok_or_else(|| download("integrity"))?;
                    if key.is_empty() {
                        return Err(download("integrity"));
                    }
                    if key == "size"
                        || key == "linkpath"
                        || (kind.is_pax_global_extensions() && key == "path")
                    {
                        return Err(download("path"));
                    }
                    if key == "path" {
                        extended_path = Some(value.to_owned());
                    }
                    position = end;
                }
            }
        } else {
            if !(kind.is_file() || kind.is_dir())
                || header.link_name().map_err(|_| download("path"))?.is_some()
            {
                return Err(download("path"));
            }
            if kind.is_dir() && size != 0 {
                return Err(download("size"));
            }
            let name = extended_path.take().unwrap_or(
                header
                    .path()
                    .map_err(|_| download("path"))?
                    .to_str()
                    .ok_or_else(|| download("path"))?
                    .to_owned(),
            );
            let name = if kind.is_dir() {
                name.strip_suffix('/').unwrap_or(&name)
            } else {
                &name
            };
            if name.encode_utf16().count() > 1024
                || name.split('/').count() > 32
                || !name.split('/').all(super::catalog::safe_segment)
            {
                return Err(download("path"));
            }
            let relative = if kind.is_dir() && name == archive.strip_prefix.trim_end_matches('/') {
                ""
            } else {
                name.strip_prefix(&archive.strip_prefix)
                    .ok_or_else(|| download("path"))?
            };
            if !relative.is_empty() {
                let key = relative.to_lowercase();
                if !super::catalog::safe_relative(relative)
                    || seen.keys().any(|other| {
                        other == &key
                            || (!kind.is_dir() && other.starts_with(&format!("{key}/")))
                            || (!seen[other] && key.starts_with(&format!("{other}/")))
                    })
                {
                    return Err(download("path"));
                }
                seen.insert(key, kind.is_dir());
            }
            let target = model.files.iter().find(|f| f.path == relative);
            if target.is_some_and(|f| !kind.is_file() || f.bytes != size) {
                return Err(download("size"));
            }
            let mut output_file = if let Some(target) = target {
                let path = output.join(&target.path);
                storage::directory(path.parent().ok_or_else(|| download("path"))?, true)?;
                Some(storage::checked_file(&path, true, true)?)
            } else {
                None
            };
            let mut remaining = size;
            let mut hash = Sha256::new();
            let mut buffer = [0; 128 * 1024];
            while remaining > 0 {
                storage::cancelled(token)?;
                let length = remaining.min(buffer.len() as u64) as usize;
                source
                    .read_exact(&mut buffer[..length])
                    .map_err(|_| download("integrity"))?;
                if let Some(file) = &mut output_file {
                    file.write_all(&buffer[..length])
                        .map_err(|_| download("storage"))?;
                    hash.update(&buffer[..length]);
                }
                remaining -= length as u64;
            }
            if let Some(target) = target {
                if hex(hash.finalize().as_slice()) != target.sha256 {
                    return Err(download("integrity"));
                }
                output_file
                    .as_ref()
                    .unwrap()
                    .sync_all()
                    .map_err(|_| download("storage"))?;
                found.insert(target.path.clone());
            }
        }
        let padding = (512 - size % 512) % 512;
        if padding > 0 {
            let mut bytes = [0; 512];
            source
                .read_exact(&mut bytes[..padding as usize])
                .map_err(|_| download("integrity"))?;
        }
        total += size + padding;
    }
    // Drain the remaining decompressed stream under the same budget; reject a concatenated archive payload.
    let mut buffer = [0; 4096];
    loop {
        storage::cancelled(token)?;
        let n = source
            .read(&mut buffer)
            .map_err(|_| download("integrity"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit || buffer[..n].iter().any(|v| *v != 0) {
            return Err(download("size"));
        }
    }
    if found.len() != model.files.len() {
        return Err(download("integrity"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::{write::BzEncoder, Compression};
    use tar::{Builder, EntryType, Header};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("pisper-speech-archive-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn model(bytes: &[u8]) -> Model {
        Catalog::parse(&json!({"defaults":{"asr":"fixture"},"models":[{
            "id":"fixture","kind":"asr","engine":"online-transducer","name":"Fixture","languages":["en"],"license":{},"config":{},
            "files":[{"path":"model.onnx","bytes":bytes.len(),"sha256":super::super::catalog::hash(bytes)}],
            "archive":{"format":"tar.bz2","bytes":1,"sha256":"0".repeat(64),"stripPrefix":"fixture/","urls":["https://models.example/fixture"]}
        }]}).to_string()).unwrap().models.remove(0)
    }
    fn archive(path: &Path, entries: &[(&str, &[u8], EntryType)]) {
        let encoder = BzEncoder::new(fs::File::create(path).unwrap(), Compression::fast());
        let mut builder = Builder::new(encoder);
        for (name, bytes, kind) in entries {
            let mut header = Header::new_gnu();
            header.set_mode(0o600);
            header.set_size(bytes.len() as u64);
            header.set_entry_type(*kind);
            if kind.is_symlink() {
                header.set_link_name("outside").unwrap();
            }
            header.set_cksum();
            builder.append_data(&mut header, name, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }
    #[test]
    fn actual_bzip_tar_extracts_only_manifest_and_rejects_aliases_links_and_cancel() {
        let fixture = Fixture::new();
        let input = fixture.0.join("fixture.tar.bz2");
        let output = fixture.0.join("output");
        fs::create_dir_all(&output).unwrap();
        let bytes = b"real bounded tar model fixture";
        let model = model(bytes);
        archive(
            &input,
            &[
                ("fixture/", b"", EntryType::Directory),
                ("fixture/model.onnx", bytes, EntryType::Regular),
                (
                    "fixture/README.txt",
                    b"ignored safe upstream metadata",
                    EntryType::Regular,
                ),
            ],
        );
        extract(&model, &input, &output, &CancellationToken::new()).unwrap();
        assert_eq!(fs::read(output.join("model.onnx")).unwrap(), bytes);
        assert!(!output.join("README.txt").exists());
        fs::remove_file(output.join("model.onnx")).unwrap();
        archive(
            &input,
            &[
                ("fixture/model.onnx", bytes, EntryType::Regular),
                ("fixture/MODEL.onnx", bytes, EntryType::Regular),
            ],
        );
        assert!(extract(&model, &input, &output, &CancellationToken::new()).is_err());
        fs::remove_file(output.join("model.onnx")).unwrap();
        archive(&input, &[("fixture/model.onnx", b"", EntryType::Symlink)]);
        assert!(extract(&model, &input, &output, &CancellationToken::new()).is_err());
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(
            extract(&model, &input, &output, &token).unwrap_err().code,
            "cancelled"
        );
    }
    #[test]
    fn storage_rejects_actual_hard_links_before_hashing_or_opening() {
        let fixture = Fixture::new();
        let path = fixture.0.join("model.onnx");
        let alias = fixture.0.join("alias.onnx");
        fs::write(&path, b"fixture").unwrap();
        fs::hard_link(&path, &alias).unwrap();
        assert!(storage::digest_file(&path, 7, &CancellationToken::new()).is_err());
        fs::remove_file(alias).unwrap();
        assert!(storage::digest_file(&path, 7, &CancellationToken::new()).is_ok());
    }
}
