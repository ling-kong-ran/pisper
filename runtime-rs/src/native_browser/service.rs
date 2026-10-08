use super::{coercion, types::*};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;

#[derive(Default)]
struct BrowserState {
    sessions: HashMap<String, Arc<dyn BrowserDriver>>,
    timers: HashMap<String, (u64, JoinHandle<()>)>,
    generation: u64,
}

pub struct BrowserAutomationService {
    factory: BrowserFactory,
    state: Mutex<BrowserState>,
    idle_timeout: Duration,
}
impl BrowserAutomationService {
    pub fn new(factory: BrowserFactory) -> Arc<Self> {
        Self::with_idle_timeout(factory, Duration::from_secs(600))
    }
    pub fn with_idle_timeout(factory: BrowserFactory, idle_timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            factory,
            state: Mutex::new(BrowserState::default()),
            idle_timeout,
        })
    }
    pub fn with_idle_milliseconds(
        factory: BrowserFactory,
        value: Option<&Value>,
    ) -> BrowserResult<Arc<Self>> {
        let milliseconds = coercion::idle_ms(value)?;
        let delay = if milliseconds == 0.0 {
            0
        } else if !milliseconds.is_finite() || milliseconds > i32::MAX as f64 {
            1
        } else {
            milliseconds.trunc().max(1.0) as u64
        };
        Ok(Self::with_idle_timeout(
            factory,
            Duration::from_millis(delay),
        ))
    }
    fn touch(self: &Arc<Self>, id: &str) {
        let mut state = self.state.lock().expect("browser state");
        if let Some((_, timer)) = state.timers.remove(id) {
            timer.abort()
        }
        if self.idle_timeout.is_zero() {
            return;
        }
        let generation = state.generation;
        state.generation = state.generation.wrapping_add(1);
        let weak = Arc::downgrade(self);
        let key = id.to_owned();
        let timeout = self.idle_timeout;
        let timer = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            let driver = {
                let mut state = service.state.lock().expect("browser state");
                if state
                    .timers
                    .get(&key)
                    .is_some_and(|(current, _)| *current == generation)
                {
                    state.timers.remove(&key);
                    state.sessions.remove(&key)
                } else {
                    None
                }
            };
            if let Some(driver) = driver {
                let _ = driver.close().await;
            }
        });
        state.timers.insert(id.to_owned(), (generation, timer));
    }
    pub async fn execute(
        self: &Arc<Self>,
        session_id: &str,
        input: Value,
        cwd: &Path,
        options: BrowserOptions,
    ) -> BrowserResult<Value> {
        if input.is_null() {
            return Err("Cannot read properties of null (reading 'action')".into());
        }
        let action = coercion::action(input.get("action"))?;
        let viewport = json!({"width":coercion::dimension(input.get("width"),1440,640,2560)?,"height":coercion::dimension(input.get("height"),900,480,1600)?});
        if options.cancellation.is_cancelled() {
            return Err("This operation was aborted".into());
        }
        let id = if session_id.is_empty() {
            "default"
        } else {
            session_id
        };
        if action != "close" {
            self.touch(id)
        }
        if action == "close" {
            self.close_session(id).await?;
            return Ok(json!({"action":"close","closed":true}));
        }
        // release 在分支校验前启动浏览器；失败的 URL/选择器仍有会话和闲置计时器。
        let current = self
            .state
            .lock()
            .expect("browser state")
            .sessions
            .get(id)
            .cloned();
        let driver = if let Some(current) = current {
            current
        } else {
            let driver = (self.factory)(viewport.clone()).await?;
            self.state
                .lock()
                .expect("browser state")
                .sessions
                .insert(id.to_owned(), driver.clone());
            driver
        };
        let mut normalized = input.as_object().cloned().unwrap_or_default();
        normalized.insert("action".into(), json!(action));
        normalized.insert("viewport".into(), viewport);
        match action.as_str() {
            "open" => {
                normalized.insert("url".into(), json!(safe_url(&input["url"])?));
            }
            "click" | "type" => {
                normalized.insert("selector".into(), json!(safe_selector(&input["selector"])?));
                if action == "type" {
                    normalized.insert("text".into(), json!(coercion::text(input.get("text"))?));
                    normalized.insert("submit".into(), json!(coercion::truthy(&input["submit"])));
                }
            }
            "screenshot" => {
                let file = screenshot_path(cwd, &input["outputName"])?;
                tokio::fs::create_dir_all(file.parent().expect("screenshot parent"))
                    .await
                    .map_err(|error| error.to_string())?;
                normalized.insert("outputPath".into(), json!(file));
                normalized.insert(
                    "fullPage".into(),
                    json!(coercion::full_page(input.get("fullPage"))),
                );
            }
            "inspect" | "wait" => {}
            _ => return Err(format!("Unsupported browser action: {action}")),
        }
        driver
            .execute(Value::Object(normalized), options.on_progress)
            .await
    }
    pub async fn close_session(&self, session_id: &str) -> BrowserResult<bool> {
        let id = if session_id.is_empty() {
            "default"
        } else {
            session_id
        };
        let current = {
            let mut state = self.state.lock().expect("browser state");
            if let Some((_, timer)) = state.timers.remove(id) {
                timer.abort()
            }
            state.sessions.remove(id)
        };
        let Some(current) = current else {
            return Ok(false);
        };
        let _ = current.close().await;
        Ok(true)
    }
    pub async fn dispose(&self) {
        let sessions = {
            let mut state = self.state.lock().expect("browser state");
            for (_, (_, timer)) in state.timers.drain() {
                timer.abort()
            }
            std::mem::take(&mut state.sessions)
        };
        futures::future::join_all(sessions.into_values().map(|driver| async move {
            let _ = driver.close().await;
        }))
        .await;
    }
}
impl Drop for BrowserAutomationService {
    fn drop(&mut self) {
        for (_, (_, timer)) in self.state.get_mut().expect("browser state").timers.drain() {
            timer.abort()
        }
    }
}
pub(super) fn safe_url(value: &Value) -> BrowserResult<String> {
    let value = coercion::string_or_empty(value)?;
    let url = reqwest::Url::parse(value.trim_matches(coercion::whitespace))
        .map_err(|_| "Invalid URL".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Browser automation only supports http and https URLs.".into());
    }
    Ok(url.to_string())
}
pub(super) fn safe_selector(value: &Value) -> BrowserResult<String> {
    let value = coercion::string_or_empty(value)?;
    let value = value.trim_matches(coercion::whitespace);
    if value.is_empty() {
        return Err("A selector is required for this browser action.".into());
    }
    if value.encode_utf16().count() > 500 {
        return Err("Browser selector is limited to 500 characters.".into());
    }
    Ok(value.to_owned())
}
pub(super) fn output_name(value: &Value, now_ms: u128) -> BrowserResult<String> {
    let value = coercion::string_or_empty(value)?;
    let value = value.trim_matches(coercion::whitespace);
    #[cfg(windows)]
    let name = value
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    #[cfg(windows)]
    let name = if name.len() >= 2
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name.as_bytes()[1] == b':'
    {
        &name[2..]
    } else {
        name
    };
    #[cfg(not(windows))]
    let name = value
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let mut requested = String::new();
    let mut invalid = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            requested.push(character);
            invalid = false;
        } else if !invalid {
            requested.push('-');
            invalid = true;
        }
    }
    let lower = requested.to_ascii_lowercase();
    for suffix in [".png", ".jpg", ".jpeg", ".webp"] {
        if lower.ends_with(suffix) {
            requested.truncate(requested.len() - suffix.len());
            break;
        }
    }
    let stem = requested.trim_matches('-');
    let stem = if stem.is_empty() {
        format!("browser-{now_ms}")
    } else {
        stem.to_owned()
    };
    Ok(format!(
        "{}.png",
        stem.chars().take(100).collect::<String>()
    ))
}
fn screenshot_path(cwd: &Path, value: &Value) -> BrowserResult<PathBuf> {
    let name = output_name(
        value,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )?;
    let cwd = if cwd.as_os_str().is_empty() {
        std::env::current_dir().map_err(|error| error.to_string())?
    } else if cwd.is_absolute() {
        cwd.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(cwd)
    };
    let mut resolved = PathBuf::new();
    for part in cwd.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            part => resolved.push(part.as_os_str()),
        }
    }
    Ok(resolved.join("generated/browser").join(name))
}
