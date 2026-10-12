//! 会话只读索引与有界缓存：目录查询不读取消息正文，文件变化才重新投影。
use crate::ApiError;
use pi_rust::coding_agent::session_manager::{SessionEntry, SessionInfo, SessionManager};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::{File, Metadata},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
impl Stamp {
    fn read(path: &Path) -> Result<Self, ApiError> {
        let meta =
            std::fs::metadata(path).map_err(|_| ApiError::bad_request("会话文件不可读取。"))?;
        Ok(Self::from_metadata(&meta))
    }
    fn from_metadata(meta: &Metadata) -> Self {
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
            },
        }
    }
    fn millis(&self) -> i64 {
        self.modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|time| time.as_millis() as i64)
            .unwrap_or(0)
    }
}

struct Cached<T> {
    stamp: Stamp,
    value: Arc<T>,
    bytes: usize,
    touched: u64,
}
struct Entries<T> {
    values: HashMap<(PathBuf, u64), Cached<T>>,
    clock: u64,
}
/// 同文件并发读取只做一次；不同会话的磁盘读取不持有同一把全局锁。
pub(crate) struct FileCache<T> {
    entries: Mutex<Entries<T>>,
    loading: Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>,
    max_entries: usize,
    max_bytes: usize,
}
impl<T> FileCache<T> {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(Entries {
                values: HashMap::new(),
                clock: 0,
            }),
            loading: Mutex::new(HashMap::new()),
            max_entries,
            max_bytes,
        }
    }
    pub(crate) fn read(
        &self,
        path: &Path,
        discriminator: u64,
        load: impl FnOnce() -> Result<(T, usize), ApiError>,
    ) -> Result<Arc<T>, ApiError> {
        let gate = {
            let mut loading = self.loading.lock().expect("session read gates");
            loading.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = loading.get(path).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                loading.insert(path.to_path_buf(), Arc::downgrade(&gate));
                gate
            }
        };
        let _guard = gate.lock().expect("session read gate");
        let stamp = match Stamp::read(path) {
            Ok(stamp) => stamp,
            Err(error) => {
                self.entries
                    .lock()
                    .expect("session read cache")
                    .values
                    .retain(|(candidate, _), _| candidate != path);
                return Err(error);
            }
        };
        let key = (path.to_path_buf(), discriminator);
        {
            let mut entries = self.entries.lock().expect("session read cache");
            entries
                .values
                .retain(|(candidate, _), cached| candidate != path || cached.stamp == stamp);
            entries.clock += 1;
            let now = entries.clock;
            if let Some(cached) = entries.values.get_mut(&key) {
                if cached.stamp == stamp {
                    cached.touched = now;
                    return Ok(cached.value.clone());
                }
            }
            // 失效的大结果先释放，重建时不同时常驻两份历史。
            entries.values.remove(&key);
        }
        let (value, bytes) = load()?;
        let value = Arc::new(value);
        if bytes <= self.max_bytes && Stamp::read(path).ok().as_ref() == Some(&stamp) {
            let mut entries = self.entries.lock().expect("session read cache");
            entries.clock += 1;
            let now = entries.clock;
            entries.values.insert(
                key,
                Cached {
                    stamp,
                    value: value.clone(),
                    bytes,
                    touched: now,
                },
            );
            while entries.values.len() > self.max_entries
                || entries
                    .values
                    .values()
                    .map(|entry| entry.bytes)
                    .sum::<usize>()
                    > self.max_bytes
            {
                let oldest = entries
                    .values
                    .iter()
                    .min_by_key(|(_, entry)| entry.touched)
                    .map(|(path, _)| path.clone());
                if let Some(path) = oldest {
                    entries.values.remove(&path);
                } else {
                    break;
                }
            }
        }
        Ok(value)
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    parent_session: Option<String>,
}
#[derive(Deserialize)]
struct OutlineMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    timestamp: Value,
}
#[derive(Deserialize)]
struct Outline {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "targetId", default)]
    target_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<OutlineMessage>,
}
fn timestamp(value: &str) -> Option<i64> {
    pi_rust::coding_agent::core::messages::parse_epoch_millis(value)
}
fn read_header(path: &Path) -> Result<Header, ApiError> {
    let file = File::open(path).map_err(|_| ApiError::bad_request("会话文件不可读取。"))?;
    let mut reader = BufReader::new(file);
    if reader
        .fill_buf()
        .map_err(|_| ApiError::bad_request("会话头不可读取。"))?
        .starts_with(&[0xef, 0xbb, 0xbf])
    {
        reader.consume(3);
    }
    // 只解析首个 JSON 对象；忽略未知大字段，不读取或校验后续消息正文。
    let header = Header::deserialize(&mut serde_json::Deserializer::from_reader(reader))
        .map_err(|_| ApiError::bad_request("会话头无效。"))?;
    if header.kind != "session" || header.id.as_deref().unwrap_or("").is_empty() {
        return Err(ApiError::bad_request("会话头无效。"));
    }
    Ok(header)
}
fn paths(agent_dir: &str) -> Vec<PathBuf> {
    let root = Path::new(agent_dir).join("sessions");
    let mut directories = vec![root.clone()];
    if let Ok(entries) = std::fs::read_dir(&root) {
        directories.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        }));
    }
    let mut paths = Vec::new();
    for dir in directories {
        if let Ok(entries) = std::fs::read_dir(dir) {
            paths.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl")),
            );
        }
    }
    paths.sort();
    paths
}

struct CatalogInfo {
    info: SessionInfo,
    has_labels: bool,
}

pub(crate) struct SessionReadCache {
    headers: FileCache<Header>,
    catalog: FileCache<CatalogInfo>,
    pub(crate) labels: FileCache<Vec<Value>>,
    pub(crate) summaries: FileCache<StoredSummary>,
    pub(crate) projections: FileCache<StoredRead>,
}
impl Default for SessionReadCache {
    fn default() -> Self {
        Self {
            headers: FileCache::new(2048, 4 * 1024 * 1024),
            catalog: FileCache::new(2048, 4 * 1024 * 1024),
            labels: FileCache::new(512, 4 * 1024 * 1024),
            summaries: FileCache::new(512, 4 * 1024 * 1024),
            // 与旧 Node 的消息投影缓存采用相同条目数和内存预算。
            projections: FileCache::new(8, 24 * 1024 * 1024),
        }
    }
}
impl SessionReadCache {
    fn header(&self, path: &Path) -> Result<Arc<Header>, ApiError> {
        self.headers.read(path, 0, || {
            let header = read_header(path)?;
            let bytes = 256
                + header.id.as_ref().map_or(0, String::len)
                + header.cwd.as_ref().map_or(0, String::len)
                + header.parent_session.as_ref().map_or(0, String::len);
            Ok((header, bytes))
        })
    }
    pub(crate) fn find(&self, agent_dir: &str, id: &str) -> Result<String, ApiError> {
        let candidates: Vec<_> = paths(agent_dir)
            .into_iter()
            .filter(|path| {
                self.header(path)
                    .is_ok_and(|header| header.id.as_deref() == Some(id))
            })
            .collect();
        if candidates.len() == 1 {
            return Ok(candidates[0].to_string_lossy().into_owned());
        }
        // 极少数重复 ID 保持原先按最后消息时间选择的语义。
        let mut infos: Vec<_> = candidates
            .iter()
            .filter_map(|path| self.info(path).ok())
            .collect();
        infos.sort_by(|a, b| {
            b.info
                .modified
                .cmp(&a.info.modified)
                .then_with(|| a.info.path.cmp(&b.info.path))
        });
        infos
            .first()
            .map(|entry| entry.info.path.clone())
            .ok_or_else(|| {
                ApiError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    "session_not_found",
                    "会话不存在。",
                )
            })
    }
    pub(crate) fn cwd(&self, path: &str) -> Result<String, ApiError> {
        let header = self.header(Path::new(path))?;
        pi_rust::coding_agent::utils::paths::resolve_path_auto_base(
            header.cwd.as_deref().unwrap_or("."),
        )
        .map_err(|_| ApiError::bad_request("会话工作目录无效。"))
    }
    fn info(&self, path: &Path) -> Result<Arc<CatalogInfo>, ApiError> {
        self.catalog.read(path, 0, || {
            let header = self.header(path)?;
            let file = File::open(path).map_err(|_| ApiError::bad_request("会话文件不可读取。"))?;
            let mut reader = BufReader::new(file);
            let mut line = Vec::new();
            let mut name = None;
            let mut count = 0;
            let mut labels = HashSet::new();
            let mut activity: Option<i64> = None;
            // serde 跳过未声明字段：统计目录不分配图片、工具输出和消息正文。
            while reader
                .read_until(b'\n', &mut line)
                .map_err(|_| ApiError::bad_request("会话目录不可读取。"))?
                > 0
            {
                if let Ok(row) = serde_json::from_slice::<Outline>(
                    line.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&line),
                ) {
                    if row.kind == "label" && row.id.is_some() && row.timestamp.is_some() {
                        if let Some(target) = row.target_id {
                            if row.label.as_deref().is_some_and(|label| !label.is_empty()) {
                                labels.insert(target);
                            } else {
                                labels.remove(&target);
                            }
                        }
                    }
                    if row.kind == "session_info" {
                        name = row
                            .name
                            .map(|name| name.trim().to_string())
                            .filter(|name| !name.is_empty());
                    }
                    if row.kind == "message" {
                        count += 1;
                        let is_conversation = row.message.as_ref().is_some_and(|message| {
                            matches!(message.role.as_str(), "user" | "assistant")
                        });
                        let time = row
                            .message
                            .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
                            .and_then(|message| message.timestamp.as_i64())
                            .or_else(|| {
                                if is_conversation {
                                    row.timestamp.as_deref().and_then(timestamp)
                                } else {
                                    None
                                }
                            });
                        if let Some(time) = time {
                            activity = Some(activity.map_or(time, |prior| prior.max(time)));
                        }
                    }
                }
                line.clear();
            }
            let created = header.timestamp.as_deref().and_then(timestamp);
            let info = SessionInfo {
                path: path.to_string_lossy().into_owned(),
                id: header.id.clone().unwrap_or_default(),
                cwd: header.cwd.clone().unwrap_or_default(),
                name,
                parent_session_path: header.parent_session.clone(),
                created,
                modified: activity
                    .filter(|time| *time > 0)
                    .or(created)
                    .unwrap_or(Stamp::read(path)?.millis()),
                message_count: count,
                first_message: String::new(),
                all_messages_text: String::new(),
            };
            let bytes =
                512 + info.path.len() + info.cwd.len() + info.name.as_ref().map_or(0, String::len);
            Ok((
                CatalogInfo {
                    info,
                    has_labels: !labels.is_empty(),
                },
                bytes,
            ))
        })
    }
    pub(crate) fn has_labels(&self, path: &str) -> Result<bool, ApiError> {
        Ok(self.info(Path::new(path))?.has_labels)
    }
    pub(crate) fn infos(&self, agent_dir: &str) -> Vec<SessionInfo> {
        let mut infos: Vec<_> = paths(agent_dir)
            .iter()
            .filter_map(|path| self.info(path).ok())
            .map(|entry| entry.info.clone())
            .collect();
        infos.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
        let mut seen = HashSet::new();
        infos.retain(|info| seen.insert(info.id.clone()));
        infos
    }
}

/// 借用分支条目而不是克隆整段工具输出；不改变 Pi 的分支选择规则。
pub(crate) fn branch(manager: &SessionManager) -> Vec<&SessionEntry> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut current = manager.get_leaf_entry();
    while let Some(entry) = current {
        if !entry.id().is_some_and(|id| seen.insert(id)) {
            break;
        }
        rows.push(entry);
        current = entry.parent_id().and_then(|id| manager.get_entry(id));
    }
    rows.reverse();
    rows
}

#[derive(Clone)]
pub(crate) struct StoredSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) cwd: String,
    pub(crate) model: String,
    pub(crate) thinking: String,
    pub(crate) created: Option<String>,
    pub(crate) message_count: usize,
    pub(crate) first_message: String,
}
impl StoredSummary {
    pub(crate) fn from_manager(manager: &SessionManager) -> Self {
        let mut value = Self {
            id: manager.get_session_id().to_owned(),
            name: manager.get_session_name().unwrap_or_default(),
            cwd: manager.get_cwd().to_owned(),
            model: String::new(),
            thinking: "off".into(),
            created: manager.get_header().and_then(|header| header.timestamp),
            message_count: 0,
            first_message: String::new(),
        };
        let mut first_user_seen = false;
        for entry in branch(manager) {
            match entry {
                SessionEntry::ModelChange(entry) => {
                    value.model = format!("{}/{}", entry.provider, entry.model_id)
                }
                SessionEntry::ThinkingLevelChange(entry) => {
                    value.thinking = entry.thinking_level.clone()
                }
                SessionEntry::Message(entry) => {
                    value.message_count += 1;
                    if !first_user_seen && entry.message.role() == "user" {
                        first_user_seen = true;
                        if let Ok(message) = serde_json::to_value(&entry.message) {
                            value.first_message = match &message["content"] {
                                Value::String(text) => text.clone(),
                                Value::Array(parts) => parts
                                    .iter()
                                    .filter(|part| part["type"] == "text")
                                    .filter_map(|part| part["text"].as_str())
                                    .collect::<Vec<_>>()
                                    .join(""),
                                _ => String::new(),
                            };
                        }
                    }
                }
                _ => {}
            }
        }
        value
    }
    pub(crate) fn bytes(&self) -> usize {
        256 + self.id.len()
            + self.name.len()
            + self.cwd.len()
            + self.model.len()
            + self.first_message.len()
    }
}
pub(crate) struct StoredRead {
    pub(crate) summary: StoredSummary,
    pub(crate) transcript: Vec<Value>,
    pub(crate) session_usage: Value,
    pub(crate) context_usage: Value,
    pub(crate) entry_count: usize,
}
pub(crate) fn value_bytes(value: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match value {
            Value::String(value) => value.capacity(),
            Value::Array(values) => values.iter().map(value_bytes).sum(),
            Value::Object(values) => values
                .iter()
                .map(|(key, value)| key.capacity() + value_bytes(value) + 32)
                .sum(),
            _ => 0,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pisper-session-read-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(path.join("sessions")).unwrap();
            Self(std::fs::canonicalize(path).unwrap())
        }
        fn session(&self, filename: &str, id: &str) -> PathBuf {
            let path = self.0.join("sessions").join(filename);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{}\n", serde_json::json!({"type":"session","version":3,"id":id,"timestamp":"2026-10-11T00:00:00.000Z","cwd":self.0}))).unwrap();
            path
        }
        fn append(path: &Path, value: Value) {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{value}").unwrap();
            file.sync_all().unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn locating_one_session_never_requires_parsing_unrelated_message_bodies() {
        let fixture = Fixture::new();
        let wanted = fixture.session("one.jsonl", "one");
        let unrelated = fixture.session("project/two.jsonl", "two");
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(unrelated)
            .unwrap()
            .write_all(&[255; 1024])
            .unwrap();
        let cache = SessionReadCache::default();
        assert_eq!(
            cache.find(fixture.0.to_str().unwrap(), "one").unwrap(),
            wanted.to_string_lossy()
        );
        std::fs::remove_file(wanted).unwrap();
        assert!(cache.find(fixture.0.to_str().unwrap(), "one").is_err());
    }

    #[test]
    fn catalog_refreshes_append_rename_cwd_rewrite_creation_and_deletion() {
        let fixture = Fixture::new();
        let path = fixture.session("one.jsonl", "one");
        let cache = SessionReadCache::default();
        let root = fixture.0.to_str().unwrap();
        assert_eq!(cache.infos(root)[0].message_count, 0);
        Fixture::append(
            &path,
            serde_json::json!({"type":"message","id":"m1","parentId":null,"timestamp":"2026-10-11T00:00:01.000Z","message":{"role":"user","timestamp":1791676801000_i64,"content":"hello"}}),
        );
        Fixture::append(
            &path,
            serde_json::json!({"type":"session_info","id":"name","parentId":"m1","timestamp":"2026-10-11T00:00:02.000Z","name":"renamed"}),
        );
        let row = cache.infos(root).remove(0);
        assert_eq!(row.name.as_deref(), Some("renamed"));
        assert_eq!(row.message_count, 1);
        let expected =
            SessionManager::list_all(Some(path.parent().unwrap().to_str().unwrap()), None)
                .remove(0);
        assert_eq!(
            (row.id, row.name, row.modified, row.created, row.cwd),
            (
                expected.id,
                expected.name,
                expected.modified,
                expected.created,
                expected.cwd
            )
        );
        let changed_workspace = fixture.0.join("changed-workspace");
        std::fs::create_dir(&changed_workspace).unwrap();
        let changed_workspace = std::fs::canonicalize(changed_workspace).unwrap();
        let changed_cwd = changed_workspace.to_str().unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let (header, records) = content.split_once('\n').unwrap();
        let mut header: Value = serde_json::from_str(header).unwrap();
        // Windows 的路径在 JSON 中含转义反斜杠；只改字段，保留后续记录原字节。
        header["cwd"] = serde_json::json!(changed_cwd);
        std::fs::write(&path, format!("{header}\n{records}")).unwrap();
        assert_eq!(cache.cwd(path.to_str().unwrap()).unwrap(), changed_cwd);
        let reopened = SessionManager::open(path.to_str().unwrap(), None, None).unwrap();
        assert_eq!(reopened.get_cwd(), changed_cwd);
        let row = cache.infos(root).remove(0);
        assert_eq!(row.cwd, changed_cwd);
        assert_eq!(row.name.as_deref(), Some("renamed"));
        assert_eq!(row.message_count, 1);
        let second = fixture.session("nested/second.jsonl", "two");
        assert_eq!(cache.infos(root).len(), 2);
        std::fs::remove_file(second).unwrap();
        assert_eq!(cache.infos(root).len(), 1);
    }

    #[test]
    fn cache_coalesces_concurrent_reads_without_changing_returned_values() {
        let fixture = Fixture::new();
        let path = fixture.session("one.jsonl", "one");
        let cache = Arc::new(FileCache::<String>::new(2, 128));
        let loads = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(9));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (cache, loads, barrier, path) =
                    (cache.clone(), loads.clone(), barrier.clone(), path.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    cache
                        .read(&path, 0, || {
                            loads.fetch_add(1, Ordering::SeqCst);
                            Ok(("value".into(), 32))
                        })
                        .unwrap()
                })
            })
            .collect();
        barrier.wait();
        for thread in threads {
            assert_eq!(&*thread.join().unwrap(), "value");
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn replacement_same_size_and_mtime_invalidates_cached_reads() {
        let fixture = Fixture::new();
        let path = fixture.0.join("history");
        std::fs::write(&path, "old").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let cache = FileCache::new(2, 1024);
        let read = || {
            cache
                .read(&path, 0, || {
                    Ok((std::fs::read_to_string(&path).unwrap(), 3))
                })
                .unwrap()
        };
        assert_eq!(&*read(), "old");
        let replacement = fixture.0.join("replacement");
        std::fs::write(&replacement, "new").unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(&*read(), "new");
        assert_eq!(
            &*cache
                .read(&path, 1, || Ok(("changed-model-catalog".to_string(), 32)))
                .unwrap(),
            "changed-model-catalog"
        );
    }

    #[test]
    fn different_pages_share_a_file_without_evicting_each_other() {
        let fixture = Fixture::new();
        let path = fixture.session("one.jsonl", "one");
        let cache = FileCache::new(8, 128);
        for page in [40, 80] {
            cache.read(&path, page, || Ok((page, 16))).unwrap();
        }
        for page in [40, 80] {
            assert_eq!(
                *cache
                    .read(&path, page, || panic!("unchanged pages should be cached"))
                    .unwrap(),
                page
            );
        }
        Fixture::append(&path, serde_json::json!({"type":"custom"}));
        for page in [40, 80] {
            assert_eq!(
                *cache.read(&path, page, || Ok((page + 1, 16))).unwrap(),
                page + 1
            );
        }
    }

    #[test]
    fn files_changed_during_read_are_not_published_as_current_cache_entries() {
        let fixture = Fixture::new();
        let path = fixture.0.join("history");
        std::fs::write(&path, "old").unwrap();
        let cache = FileCache::new(2, 128);
        let old = cache
            .read(&path, 0, || {
                std::fs::write(&path, "new-longer").unwrap();
                Ok(("old", 3))
            })
            .unwrap();
        assert_eq!(*old, "old");
        let latest = cache.read(&path, 0, || Ok(("new-longer", 10))).unwrap();
        assert_eq!(*latest, "new-longer");
    }

    #[test]
    fn cache_budget_and_root_isolation_hold_after_eviction() {
        let fixture = Fixture::new();
        let other = Fixture::new();
        let one = fixture.session("one.jsonl", "same");
        let two = other.session("one.jsonl", "same");
        let index = SessionReadCache::default();
        assert_eq!(
            index.find(fixture.0.to_str().unwrap(), "same").unwrap(),
            one.to_string_lossy()
        );
        assert_eq!(
            index.find(other.0.to_str().unwrap(), "same").unwrap(),
            two.to_string_lossy()
        );
        let cache = FileCache::new(1, 10);
        cache.read(&one, 0, || Ok(("one", 8))).unwrap();
        cache.read(&two, 0, || Ok(("two", 8))).unwrap();
        let result = cache.read(&one, 0, || Ok(("reloaded", 8))).unwrap();
        assert_eq!(*result, "reloaded");
        cache.read(&one, 1, || Ok(("oversized", 11))).unwrap();
        assert_eq!(
            *cache.read(&one, 1, || Ok(("not-cached", 10))).unwrap(),
            "not-cached"
        );
    }

    #[test]
    fn metadata_follows_pi_branch_without_cloning_tool_payloads() {
        let fixture = Fixture::new();
        let path = fixture.session("branch.jsonl", "branch");
        Fixture::append(
            &path,
            serde_json::json!({"type":"message","id":"user","parentId":null,"timestamp":"2026-10-11T00:00:01.000Z","message":{"role":"user","timestamp":1791676801000_i64,"content":"first prompt"}}),
        );
        Fixture::append(
            &path,
            serde_json::json!({"type":"model_change","id":"discarded","parentId":"user","timestamp":"2026-10-11T00:00:02.000Z","provider":"wrong","modelId":"wrong"}),
        );
        Fixture::append(
            &path,
            serde_json::json!({"type":"model_change","id":"selected","parentId":"user","timestamp":"2026-10-11T00:00:03.000Z","provider":"fixture","modelId":"correct"}),
        );
        Fixture::append(
            &path,
            serde_json::json!({"type":"thinking_level_change","id":"thinking","parentId":"selected","timestamp":"2026-10-11T00:00:04.000Z","thinkingLevel":"high"}),
        );
        let manager = SessionManager::open(path.to_str().unwrap(), None, None).unwrap();
        let info = StoredSummary::from_manager(&manager);
        assert_eq!(info.model, "fixture/correct");
        assert_eq!(info.thinking, "high");
        assert_eq!(info.first_message, "first prompt");
        assert_eq!(info.message_count, 1);
        let actual: Vec<_> = branch(&manager)
            .iter()
            .map(|entry| entry.id().unwrap().to_string())
            .collect();
        let expected: Vec<_> = manager
            .get_branch(None)
            .iter()
            .map(|entry| entry.id().unwrap().to_string())
            .collect();
        assert_eq!(actual, expected);
    }
}
