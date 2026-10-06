//! Release Plan v2：依赖图、稳定 ID、临时计划暂停栈和旧 task-list 存储迁移。
use crate::session_workers::persistence::{now, read, write, write_bytes};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub(crate) const MAX_ITEMS: usize = 50;
pub(crate) const MAX_SUSPENDED: usize = 8;
const RESUME: &str = "A previously unfinished plan was restored. Continue its current item unless the user explicitly cancelled or redirected it.";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    title: String,
    status: String,
    note: String,
    assignee: String,
    depends_on: Vec<String>,
    created_at: String,
    updated_at: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    items: Vec<Item>,
    updated_at: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    items: Vec<Item>,
    updated_at: String,
    suspended: Vec<Snapshot>,
}
#[derive(Clone, Default, Serialize)]
struct Document {
    version: u8,
    plans: BTreeMap<String, Plan>,
}
struct Store {
    document: Document,
    migration_pending: bool,
}
pub(crate) struct PlanService {
    path: PathBuf,
    legacy: Option<PathBuf>,
    store: Mutex<Store>,
}
fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}
fn id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
}
fn normalize(value: &Value, previous: Option<&Item>, at: &str) -> Result<Item> {
    let bounded = |key: &str, max: usize| -> Result<String> {
        let value = text(&value[key]).trim().to_string();
        if value.encode_utf16().count() > max {
            bail!("Plan item {key} is limited to {max} characters.")
        }
        Ok(value)
    };
    let title = bounded("title", 300)?;
    if title.is_empty() {
        bail!("Plan item title cannot be empty.")
    }
    let candidate = text(&value["id"]).trim().to_string();
    let candidate = if candidate.is_empty() {
        previous.map(|p| p.id.clone()).unwrap_or_default()
    } else {
        candidate
    };
    let id = if id_valid(&candidate) {
        candidate
    } else {
        crate::product::new_id()
    };
    let status = value["status"]
        .as_str()
        .filter(|s| matches!(*s, "pending" | "in_progress" | "completed" | "blocked"))
        .unwrap_or("pending")
        .into();
    let assignee = if value["assignee"].is_null() {
        previous.map(|p| p.assignee.clone()).unwrap_or_default()
    } else {
        bounded("assignee", 80)?
    };
    let depends_on = if value["dependsOn"].is_null() {
        previous.map(|p| p.depends_on.clone()).unwrap_or_default()
    } else {
        let values = value["dependsOn"]
            .as_array()
            .ok_or_else(|| anyhow!("Plan item dependsOn must be an array of plan item ids."))?;
        if values.len() > 20 {
            bail!("Plan item dependsOn is limited to 20 ids.")
        }
        let mut seen = HashSet::new();
        let mut ids = vec![];
        for value in values {
            let id = text(value).trim().to_string();
            if id.is_empty() {
                continue;
            }
            if !id_valid(&id) {
                bail!("Invalid dependency plan item id: {id}")
            }
            if seen.insert(id.clone()) {
                ids.push(id)
            }
        }
        ids
    };
    Ok(Item {
        id,
        title,
        status,
        note: bounded("note", 1000)?,
        assignee,
        depends_on,
        created_at: previous
            .map(|p| p.created_at.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| at.into()),
        updated_at: at.into(),
    })
}
fn dependency_graph(items: &[Item]) -> Result<()> {
    let by_id: HashMap<_, _> = items.iter().map(|item| (item.id.as_str(), item)).collect();
    for item in items {
        for dependency in &item.depends_on {
            if dependency == &item.id {
                bail!("Plan item cannot depend on itself: {}", item.id)
            }
            if !by_id.contains_key(dependency.as_str()) {
                bail!("Unknown dependency plan item id: {dependency}")
            }
        }
    }
    fn visit<'a>(
        id: &'a str,
        by_id: &HashMap<&'a str, &'a Item>,
        state: &mut HashMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
    ) -> Result<()> {
        if state.get(id) == Some(&2) {
            return Ok(());
        }
        if state.get(id) == Some(&1) {
            stack.push(id);
            bail!("Plan dependency cycle: {}", stack.join(" -> "))
        }
        state.insert(id, 1);
        stack.push(id);
        for dep in &by_id[id].depends_on {
            visit(dep, by_id, state, stack)?;
        }
        stack.pop();
        state.insert(id, 2);
        Ok(())
    }
    let mut state = HashMap::new();
    for item in items {
        visit(&item.id, &by_id, &mut state, &mut vec![])?;
    }
    Ok(())
}
fn counts(items: &[Item]) -> Value {
    let by_id: HashMap<_, _> = items
        .iter()
        .map(|item| (item.id.as_str(), item.status.as_str()))
        .collect();
    let mut pending = 0;
    let mut in_progress = 0;
    let mut completed = 0;
    let mut blocked = 0;
    for item in items {
        if item.status == "completed" {
            completed += 1;
            continue;
        }
        if item.status == "blocked"
            || item
                .depends_on
                .iter()
                .any(|id| by_id.get(id.as_str()) != Some(&"completed"))
        {
            blocked += 1;
            continue;
        }
        if item.status == "in_progress" {
            in_progress += 1
        } else {
            pending += 1
        }
    }
    json!({"pending":pending,"inProgress":in_progress,"completed":completed,"blocked":blocked,"total":items.len()})
}
fn unfinished(items: &[Item]) -> bool {
    items.iter().any(|i| i.status != "completed")
}
fn public(id: &str, plan: Option<&Plan>) -> Value {
    match plan {
        Some(p) => {
            json!({"sessionId":id,"items":p.items,"counts":counts(&p.items),"suspendedCount":p.suspended.len(),"updatedAt":p.updated_at})
        }
        None => {
            json!({"sessionId":id,"items":[],"counts":counts(&[]),"suspendedCount":0,"updatedAt":null})
        }
    }
}
fn persisted_items(value: &Value, at: &str) -> Vec<Item> {
    let mut items = vec![];
    let mut seen = HashSet::new();
    for value in value.as_array().into_iter().flatten().take(MAX_ITEMS) {
        if let Ok(mut item) = normalize(value, None, value["updatedAt"].as_str().unwrap_or(at)) {
            item.created_at = value["createdAt"].as_str().unwrap_or(at).into();
            if !seen.insert(item.id.clone()) {
                item.id = crate::product::new_id();
                seen.insert(item.id.clone());
            }
            items.push(item);
        }
    }
    items
}
fn normalized_document(value: &Value) -> Document {
    let mut document = Document {
        version: 2,
        ..Default::default()
    };
    let values = value["plans"]
        .as_object()
        .or_else(|| value["lists"].as_object());
    for (id, value) in values.into_iter().flatten() {
        if !value["items"].is_array() {
            continue;
        }
        let at = value["updatedAt"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(now);
        let items = persisted_items(&value["items"], &at);
        if items.is_empty() {
            continue;
        }
        let snapshots = value["suspended"].as_array().cloned().unwrap_or_default();
        let suspended = snapshots
            .iter()
            .skip(snapshots.len().saturating_sub(MAX_SUSPENDED))
            .filter_map(|s| {
                let updated_at = s["updatedAt"].as_str().unwrap_or(&at).to_string();
                let items = persisted_items(&s["items"], &updated_at);
                (!items.is_empty() && unfinished(&items)).then_some(Snapshot { items, updated_at })
            })
            .collect();
        document.plans.insert(
            id.clone(),
            Plan {
                items,
                updated_at: at,
                suspended,
            },
        );
    }
    document
}
impl PlanService {
    pub(crate) fn new(path: impl Into<PathBuf>, legacy: Option<PathBuf>) -> Result<Arc<Self>> {
        let path = path.into();
        let stored = read(&path)?;
        let mut pending = false;
        let document = if let Some(stored) = stored {
            normalized_document(&stored)
        } else if let Some(legacy) = &legacy {
            match read(legacy) {
                Ok(Some(value)) if value["plans"].is_object() || value["lists"].is_object() => {
                    pending = true;
                    normalized_document(&value)
                }
                Err(_) => {
                    pending = true;
                    normalized_document(&Value::Null)
                }
                _ => normalized_document(&Value::Null),
            }
        } else {
            normalized_document(&Value::Null)
        };
        let service = Arc::new(Self {
            path,
            legacy,
            store: Mutex::new(Store {
                document,
                migration_pending: pending,
            }),
        });
        if pending {
            let mut store = service.store.lock().expect("plan store");
            if service.persist(&store.document, true).is_ok() {
                store.migration_pending = false;
            }
        }
        Ok(service)
    }
    fn persist(&self, document: &Document, pending: bool) -> Result<()> {
        if pending {
            let source = self
                .legacy
                .as_ref()
                .ok_or_else(|| anyhow!("Legacy plan path missing"))?;
            let target = PathBuf::from(format!("{}.bak", source.to_string_lossy()));
            if target.exists() && !target.is_file() {
                bail!(
                    "Plan migration backup path is not a file: {}",
                    target.display()
                )
            }
            write_bytes(&target, &std::fs::read(source)?)?;
        }
        write(&self.path, &serde_json::to_value(document)?)
    }
    pub(crate) fn get(&self, id: &str) -> Value {
        let store = self.store.lock().expect("plan store");
        public(id, store.document.plans.get(id))
    }
    pub(crate) fn replace(&self, id: &str, input: &Value, mode: &str) -> Result<Value> {
        if id.is_empty() {
            bail!("Plan requires a session.")
        }
        if !matches!(mode, "auto" | "replace") {
            bail!("Unsupported plan update mode: {mode}")
        }
        let input = input
            .as_array()
            .ok_or_else(|| anyhow!("Plan items must be an array."))?;
        if input.len() > MAX_ITEMS {
            bail!("Plan is limited to {MAX_ITEMS} items.")
        }
        let mut store = self.store.lock().expect("plan store");
        let current = store.document.plans.get(id);
        let previous: HashMap<_, _> = current
            .into_iter()
            .flat_map(|p| &p.items)
            .map(|i| (i.id.as_str(), i))
            .collect();
        let at = now();
        let mut items = vec![];
        let mut seen = HashSet::new();
        for value in input {
            let item = normalize(
                value,
                previous
                    .get(value["id"].as_str().unwrap_or_default())
                    .copied(),
                &at,
            )?;
            if !seen.insert(item.id.clone()) {
                bail!("Duplicate plan item id: {}", item.id)
            }
            items.push(item);
        }
        dependency_graph(&items)?;
        let mut next = store.document.clone();
        let mut transition = None;
        if items.is_empty() {
            next.plans.remove(id);
        } else {
            let mut suspended = if mode == "replace" {
                vec![]
            } else {
                current.map(|p| p.suspended.clone()).unwrap_or_default()
            };
            if mode == "auto"
                && current.is_some_and(|p| unfinished(&p.items))
                && !items.iter().any(|i| previous.contains_key(i.id.as_str()))
            {
                if suspended.len() >= MAX_SUSPENDED {
                    bail!("Plan suspension is limited to {MAX_SUSPENDED} nested plans.")
                }
                let current = current.expect("unfinished current");
                suspended.push(Snapshot {
                    items: current.items.clone(),
                    updated_at: current.updated_at.clone(),
                });
            }
            let plan = if !unfinished(&items) && !suspended.is_empty() {
                let resumed = suspended.pop().expect("suspended plan");
                transition = Some(
                    json!({"resumed":true,"completedPlan":{"items":items,"counts":counts(&items),"updatedAt":at},"resumeInstruction":RESUME}),
                );
                Plan {
                    items: resumed.items,
                    updated_at: at,
                    suspended,
                }
            } else {
                Plan {
                    items,
                    updated_at: at,
                    suspended,
                }
            };
            next.plans.insert(id.into(), plan);
        }
        self.persist(&next, store.migration_pending)?;
        store.document = next;
        store.migration_pending = false;
        let mut value = public(id, store.document.plans.get(id));
        if let Some(Value::Object(transition)) = transition {
            value
                .as_object_mut()
                .expect("public plan")
                .extend(transition);
        }
        Ok(value)
    }
    pub(crate) fn remove(&self, id: &str) -> Result<Value> {
        self.replace(id, &json!([]), "replace")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sandbox() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("pisper-native-plan-{}", crate::product::new_id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn persistent_dependencies_and_temporary_plan_resume() {
        let dir = sandbox();
        let path = dir.join("pisper-plans.json");
        let service = PlanService::new(&path, None).unwrap();
        let first=service.replace("s",&json!([{"id":"a","title":"implement","status":"in_progress"},{"id":"b","title":"verify","dependsOn":["a"]}]),"auto").unwrap();
        assert_eq!(
            first["counts"],
            json!({"pending":0,"inProgress":1,"completed":0,"blocked":1,"total":2})
        );
        service
            .replace(
                "s",
                &json!([{"id":"temp","title":"urgent","status":"in_progress"}]),
                "auto",
            )
            .unwrap();
        let restarted = PlanService::new(&path, None).unwrap();
        assert_eq!(restarted.get("s")["suspendedCount"], 1);
        let resumed = restarted
            .replace(
                "s",
                &json!([{"id":"temp","title":"urgent","status":"completed"}]),
                "auto",
            )
            .unwrap();
        assert_eq!(resumed["resumed"], true);
        assert_eq!(
            resumed["items"][0]["createdAt"],
            first["items"][0]["createdAt"]
        );
        assert_eq!(
            PlanService::new(&path, None).unwrap().get("s")["items"],
            first["items"]
        );
        restarted.remove("s").unwrap();
        assert_eq!(restarted.get("s")["counts"]["total"], 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_graph_and_failed_storage_never_publish_candidate() {
        let dir = sandbox();
        let path = dir.join("plans.json");
        let service = PlanService::new(&path, None).unwrap();
        let old = service
            .replace("s", &json!([{"id":"a","title":"original"}]), "auto")
            .unwrap();
        for input in [
            json!([{"id":"a","title":"a","dependsOn":["a"]}]),
            json!([{"id":"a","title":"a","dependsOn":["b"]},{"id":"b","title":"b","dependsOn":["a"]}]),
            json!([{"id":"a","title":"a"},{"id":"a","title":"duplicate"}]),
        ] {
            assert!(service.replace("s", &input, "auto").is_err());
            assert_eq!(service.get("s"), old);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(service
            .replace("s", &json!([{"id":"a","title":"uncommitted"}]), "auto")
            .is_err());
        assert_eq!(service.get("s"), old);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn migration_backs_up_exact_legacy_bytes_before_new_store() {
        let dir = sandbox();
        let legacy = dir.join("pisper-task-lists.json");
        let bytes=br#"{"lists":{"s":{"items":[{"id":"a","title":"legacy","status":"pending"}],"updatedAt":"2026-01-01T00:00:00.000Z"}}}"#;
        std::fs::write(&legacy, bytes).unwrap();
        let path = dir.join("pisper-plans.json");
        let service = PlanService::new(&path, Some(legacy.clone())).unwrap();
        assert_eq!(service.get("s")["items"][0]["title"], "legacy");
        assert_eq!(
            std::fs::read(format!("{}.bak", legacy.display())).unwrap(),
            bytes
        );
        assert_eq!(read(&path).unwrap().unwrap()["version"], 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
