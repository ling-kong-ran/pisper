//! 技能领域（release `services/skills-service.mjs` 的原生移植）：
//! 仪表盘（release 形状）、安装（本地路径）、覆盖设置（enabled/modelInvocation）、
//! 卸载与强制刷新。状态持久化在 data_dir/pisper-skills.json。
//!
//! 诚实接缝：npm/git 包来源的解析依赖 Pi 包管理器（createPackageManager），
//! 当前仅支持本地路径来源；包来源返回与 release 解析失败一致的错误文案。

use axum::extract::{Path, Query, State};
use axum::Json;
use pi_rust::coding_agent::core::skills::{load_skills, LoadSkillsOptions};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path as StdPath;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{product, ApiError, AppState};

const SKILLS_STATE_VERSION: u32 = 2;
const MAX_SKILL_SOURCE_CHARS: usize = 2_000;
const MAX_SKILLS_PER_INSTALL: usize = 100;

type ArcAppState = Arc<AppState>;

#[derive(Default)]
struct SkillState {
    version: u32,
    /// normalizedPath -> {enabled, modelInvocation}
    overrides: HashMap<String, Value>,
    /// normalizedPath -> {source, installedAt}
    installed: HashMap<String, Value>,
}

#[cfg(windows)]
fn value_canonical(value: &str) -> String {
    let path = std::fs::canonicalize(value)
        .map(|p| p.to_string_lossy().replace('/', "\\").to_string())
        .unwrap_or_else(|_| value.replace('/', "\\"));
    path.to_lowercase()
}

#[cfg(not(windows))]
fn value_canonical(value: &str) -> String {
    std::fs::canonicalize(value)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| value.to_string())
}

fn skill_id(file_path: &str) -> String {
    let normalized = value_canonical(file_path);
    let digest = Sha256::digest(normalized.as_bytes());
    digest
        .iter()
        .take(10)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn load_state(state: &AppState) -> SkillState {
    let mut result = SkillState {
        version: SKILLS_STATE_VERSION,
        ..Default::default()
    };
    let Ok(bytes) = std::fs::read(
        StdPath::new(&state.data_dir).join("pisper-skills.json"),
    ) else {
        return result;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return result;
    };
    result.version = value["version"].as_u64().unwrap_or(SKILLS_STATE_VERSION as u64) as u32;
    if let Some(map) = value["overrides"].as_object() {
        for (path, item) in map {
            if item.is_object() {
                result.overrides.insert(path.clone(), item.clone());
            }
        }
    }
    if let Some(map) = value["installed"].as_object() {
        for (path, item) in map {
            if item.is_object() {
                result.installed.insert(path.clone(), item.clone());
            }
        }
    }
    result
}

fn save_state(state: &AppState, skill_state: &SkillState) -> Result<(), ApiError> {
    let value = json!({
        "version": SKILLS_STATE_VERSION,
        "overrides": skill_state.overrides,
        "installed": skill_state.installed,
    });
    std::fs::create_dir_all(&state.data_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let directory = StdPath::new(&state.data_dir);
    let temporary = directory.join("pisper-skills.json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&value).map_err(|e| ApiError::internal(e.to_string()))?)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    std::fs::rename(&temporary, directory.join("pisper-skills.json"))
        .map_err(|e| ApiError::internal(e.to_string()))
}

fn path_inside(root: &str, target: &str) -> bool {
    let root = PathBuf::from(root);
    let target = PathBuf::from(target);
    target.starts_with(&root)
}

fn project_skills_dir(cwd: &str) -> String {
    StdPath::new(cwd).join(".pisper").join("skills").to_string_lossy().to_string()
}

/// release parseFrontmatterDetails 的最小等价：SKILL.md 头部的
/// version / license / allowedTools 三项。
fn frontmatter_details(file_path: &str) -> (String, String, Vec<String>) {
    let mut version = "latest".to_string();
    let mut license = String::new();
    let mut allowed_tools: Vec<String> = Vec::new();
    let Ok(content) = std::fs::read_to_string(file_path) else {
        return (version, license, allowed_tools);
    };
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return (version, license, allowed_tools);
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let value = raw_value.trim().trim_matches('"').to_string();
        match key.trim() {
            "version" if !value.is_empty() => version = value,
            "license" => license = value,
            "allowed-tools" | "allowedTools" => {
                let trimmed = raw_value.trim();
                if trimmed.starts_with('[') {
                    allowed_tools = trimmed
                        .trim_matches(|c| c == '[' || c == ']')
                        .split(',')
                        .map(|item| item.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
                        .filter(|item| !item.is_empty())
                        .collect();
                } else if !trimmed.is_empty() {
                    allowed_tools = trimmed
                        .split(',')
                        .map(|item| item.trim().to_string())
                        .filter(|item| !item.is_empty())
                        .collect();
                }
            }
            _ => {}
        }
    }
    (version, license, allowed_tools)
}

fn source_label(source_info: &Value) -> String {
    if source_info["origin"] == "package" {
        return source_info["source"]
            .as_str()
            .unwrap_or("package")
            .to_string();
    }
    match source_info["scope"].as_str() {
        Some("project") => "project".to_string(),
        Some("user") => "user".to_string(),
        _ => source_info["source"]
            .as_str()
            .unwrap_or("custom")
            .to_string(),
    }
}

fn expand_path(value: &str, cwd: &str) -> PathBuf {
    let input = value.trim();
    if input == "~" {
        return dirs_home();
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        return dirs_home().join(rest);
    }
    let path = StdPath::new(input);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        StdPath::new(cwd).join(path)
    }
}

fn dirs_home() -> PathBuf {
    // 与 release homedir() 对齐；不经第三方 crate。
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn discover(state: &AppState, cwd: &str) -> pi_rust::coding_agent::core::skills::LoadSkillsResult {
    let services = state.runtime.services();
    let mut skill_paths = vec![StdPath::new(&state.agent_dir)
        .join("skills")
        .to_string_lossy()
        .to_string()];
    let project_dir = project_skills_dir(cwd);
    if StdPath::new(&project_dir).exists() {
        skill_paths.push(project_dir);
    }
    load_skills(LoadSkillsOptions {
        cwd: cwd.to_string(),
        agent_dir: services.agent_dir.clone(),
        skill_paths,
        include_defaults: true,
    })
}

fn public_skill(
    skill_state: &SkillState,
    skills_dir: &str,
    skill: &pi_rust::coding_agent::core::skills::Skill,
) -> Value {
    let key = value_canonical(&skill.file_path);
    let override_value = skill_state.overrides.get(&key).cloned().unwrap_or_default();
    let managed = skill_state.installed.get(&key).cloned();
    let (version, license, allowed_tools) = frontmatter_details(&skill.file_path);
    let source_info = serde_json::to_value(&skill.source_info).unwrap_or_default();
    let model_invocation_enabled = override_value
        .get("modelInvocation")
        .and_then(Value::as_bool)
        .unwrap_or(!skill.disable_model_invocation);
    json!({
        "id": skill_id(&skill.file_path),
        "name": skill.name,
        "description": skill.description,
        "filePath": skill.file_path,
        "baseDir": skill.base_dir,
        "enabled": override_value.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "modelInvocationEnabled": model_invocation_enabled,
        "command": format!("/skill:{}", skill.name),
        "version": version,
        "license": license,
        "allowedTools": allowed_tools,
        "source": managed
            .as_ref()
            .and_then(|m| m["source"].as_str().map(str::to_string))
            .unwrap_or_else(|| source_label(&source_info)),
        "sourceInfo": {
            "path": source_info["path"],
            "source": source_info["source"],
            "scope": source_info["scope"],
            "origin": source_info["origin"],
            "baseDir": source_info["baseDir"],
        },
        "removable": managed.is_some() && path_inside(skills_dir, &skill.file_path),
    })
}

fn build_dashboard(state: &AppState, cwd: &str) -> Value {
    let skills_dir = StdPath::new(&state.agent_dir)
        .join("skills")
        .to_string_lossy()
        .to_string();
    let skill_state = load_state(state);
    let discovered = discover(state, cwd);
    let skills: Vec<Value> = discovered
        .skills
        .iter()
        .map(|skill| public_skill(&skill_state, &skills_dir, skill))
        .collect();
    let project_dir = project_skills_dir(cwd);
    let is_project = |skill: &Value| {
        skill["sourceInfo"]["scope"] == "project"
            || StdPath::new(skill["filePath"].as_str().unwrap_or("")).starts_with(&project_dir)
    };
    let enabled = skills
        .iter()
        .filter(|skill| skill["enabled"] == true)
        .count();
    json!({
        "cwd": cwd,
        "locations": {
            "global": skills_dir,
            "project": project_dir,
        },
        "skills": skills,
        "diagnostics": discovered.diagnostics.iter().map(|item| {
            json!({"type": item.r#type.as_str(), "message": item.message, "path": item.path.clone().unwrap_or_default()})
        }).collect::<Vec<_>>(),
        "packages": [],
        "counts": {
            "installed": skills.len(),
            "global": skills.iter().filter(|skill| !is_project(skill)).count(),
            "project": skills.iter().filter(|skill| is_project(skill)).count(),
            "enabled": enabled,
            "modelInvocable": skills.iter().filter(|skill| skill["enabled"] == true && skill["modelInvocationEnabled"] == true).count(),
        },
    })
}

fn find_skill<'a>(
    discovered: &'a pi_rust::coding_agent::core::skills::LoadSkillsResult,
    id: &str,
) -> Option<&'a pi_rust::coding_agent::core::skills::Skill> {
    discovered
        .skills
        .iter()
        .find(|skill| skill_id(&skill.file_path) == id)
}

// ------------------------------------------------------------------- HTTP

/// release GET /api/skills?sessionId=：技能仪表盘（release 形状）。
pub(crate) async fn dashboard(
    State(state): State<ArcAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let session_id = params.get("sessionId").cloned().unwrap_or_default();
    let cwd = if session_id.is_empty() {
        state.cwd.clone()
    } else {
        crate::session_ops::session_cwd_for(&state, &session_id).await?
    };
    Ok(Json(build_dashboard(&state, &cwd)))
}

/// release POST /api/skills/reload：强制刷新后返回最新仪表盘。
pub(crate) async fn reload(
    State(state): State<ArcAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    dashboard(State(state), Query(params)).await
}

/// release POST /api/skills/install {source}：本地路径来源安装到全局技能目录。
pub(crate) async fn install(
    State(state): State<ArcAppState>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), ApiError> {
    let session_id = params.get("sessionId").cloned().unwrap_or_default();
    let cwd = if session_id.is_empty() {
        state.cwd.clone()
    } else {
        crate::session_ops::session_cwd_for(&state, &session_id).await?
    };
    let source = body["source"].as_str().unwrap_or_default().trim().to_string();
    if source.is_empty() {
        return Err(ApiError::bad_request("请输入技能目录、SKILL.md、npm 包或 git 来源。"));
    }
    if source.chars().count() > MAX_SKILL_SOURCE_CHARS {
        return Err(ApiError::bad_request("技能来源过长。"));
    }
    let local = expand_path(&source, &cwd);
    if !local.exists() {
        return Err(ApiError::bad_request(
            "该来源没有发现符合 Agent Skills 标准的技能。",
        ));
    }
    // release resolveInstallSkills 接受三种来源形状：技能根目录、单技能目录
    // （内含 SKILL.md）、SKILL.md 文件。pi-rs 的 load_skills 对目录按根扫描、
    // 对 .md 文件单载 —— 含 SKILL.md 的目录必须转成其 SKILL.md 文件路径。
    let source_path = if local.is_file() {
        local.clone()
    } else if local.join("SKILL.md").is_file() {
        local.join("SKILL.md")
    } else {
        local.clone()
    };
    let services = state.runtime.services();
    let loaded = load_skills(LoadSkillsOptions {
        cwd: cwd.clone(),
        agent_dir: services.agent_dir.clone(),
        skill_paths: vec![source_path.to_string_lossy().to_string()],
        include_defaults: false,
    });
    if loaded.skills.is_empty() {
        let message = loaded
            .diagnostics
            .first()
            .map(|item| item.message.clone())
            .unwrap_or_else(|| "该来源没有发现符合 Agent Skills 标准的技能。".to_string());
        return Err(ApiError::bad_request(message));
    }
    if loaded.skills.len() > MAX_SKILLS_PER_INSTALL {
        return Err(ApiError::bad_request(format!(
            "一次最多安装 {MAX_SKILLS_PER_INSTALL} 个技能。"
        )));
    }
    let skills_dir = StdPath::new(&state.agent_dir).join("skills");
    let mut skill_state = load_state(&state);
    let existing = discover(&state, &cwd);
    let duplicate = loaded
        .skills
        .iter()
        .find(|skill| existing.skills.iter().any(|item| item.name == skill.name));
    if let Some(duplicate) = duplicate {
        return Err(ApiError::bad_request(format!(
            "技能 {} 已存在，可直接启用或调用。",
            duplicate.name
        )));
    }
    let mut installed_paths: Vec<String> = Vec::new();
    for skill in &loaded.skills {
        let name = slug(&skill.name);
        let is_directory_install = StdPath::new(&skill.file_path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.eq_ignore_ascii_case("skill.md"))
            .unwrap_or(false);
        let destination = if is_directory_install {
            skills_dir.join(&name)
        } else {
            let extension = StdPath::new(&skill.file_path)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| format!(".{ext}"))
                .unwrap_or_else(|| ".md".to_string());
            skills_dir.join(format!("{name}{extension}"))
        };
        if destination.exists() {
            return Err(ApiError::bad_request(format!(
                "技能 {} 已安装。",
                skill.name
            )));
        }
        copy_skill_source(
            if is_directory_install {
                StdPath::new(&skill.base_dir)
            } else {
                StdPath::new(&skill.file_path)
            },
            &destination,
            is_directory_install,
        )?;
        installed_paths.push(
            if is_directory_install {
                destination.join("SKILL.md")
            } else {
                destination
            }
            .to_string_lossy()
            .to_string(),
        );
        skill_state.installed.insert(
            value_canonical(
                &installed_paths
                    .last()
                    .cloned()
                    .unwrap_or_default(),
            ),
            json!({
                "source": source,
                "installedAt": iso_now(),
            }),
        );
    }
    save_state(&state, &skill_state)?;
    let mut dashboard_value = build_dashboard(&state, &cwd);
    let installed: Vec<Value> = dashboard_value["skills"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|skill| {
            installed_paths
                .iter()
                .any(|path| value_canonical(path) == value_canonical(skill["filePath"].as_str().unwrap_or("")))
        })
        .collect();
    dashboard_value["installed"] = Value::Array(installed);
    dashboard_value["source"] = json!(source);
    // release: json(201, await runtime.installSkill(...))
    Ok((axum::http::StatusCode::CREATED, Json(dashboard_value)))
}

fn copy_skill_source(source: &StdPath, destination: &StdPath, directory: bool) -> Result<(), ApiError> {
    if directory {
        copy_dir_recursive(source, destination)?;
    } else if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
        std::fs::copy(source, destination).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(())
}

fn copy_dir_recursive(source: &StdPath, destination: &StdPath) -> Result<(), ApiError> {
    std::fs::create_dir_all(destination).map_err(|e| ApiError::internal(e.to_string()))?;
    for entry in std::fs::read_dir(source).map_err(|e| ApiError::internal(e.to_string()))? {
        let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
        let target = destination.join(entry.file_name());
        if entry.file_type().map_err(|e| ApiError::internal(e.to_string()))?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| ApiError::internal(e.to_string()))?;
        }
    }
    Ok(())
}

fn slug(value: &str) -> String {
    let mut result = String::new();
    for character in value.to_lowercase().chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            result.push(character);
        } else if character == '-' {
            result.push('-');
        } else {
            result.push('-');
        }
    }
    while result.contains("--") {
        result = result.replace("--", "-");
    }
    let result = result.trim_matches('-').to_string();
    if result.is_empty() { "skill".to_string() } else { result }
}

fn iso_now() -> String {
    // release new Date().toISOString()；与 session_ops 的毫秒精度一致。
    crate::session_ops::iso_timestamp(product::now_ms())
}

/// release PATCH /api/skills/:skillName（按 id 查找）：
/// 保存 enabled / modelInvocationEnabled 覆盖，返回 publicSkill；不存在 → 404。
pub(crate) async fn update(
    State(state): State<ArcAppState>,
    Path(skill_name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    if body.get("enabled").map(|v| !v.is_boolean()).unwrap_or(false) {
        return Err(ApiError::bad_request("技能启用状态无效。"));
    }
    if body
        .get("modelInvocationEnabled")
        .map(|v| !v.is_boolean())
        .unwrap_or(false)
    {
        return Err(ApiError::bad_request("技能自动调用状态无效。"));
    }
    let session_id = params.get("sessionId").cloned().unwrap_or_default();
    let cwd = if session_id.is_empty() {
        state.cwd.clone()
    } else {
        crate::session_ops::session_cwd_for(&state, &session_id).await?
    };
    let discovered = discover(&state, &cwd);
    let Some(skill) = find_skill(&discovered, &skill_name) else {
        return Err(ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "skill_not_found",
            "技能不存在。",
        ));
    };
    let key = value_canonical(&skill.file_path);
    let mut skill_state = load_state(&state);
    let mut current = skill_state
        .overrides
        .get(&key)
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(enabled) = body["enabled"].as_bool() {
        current["enabled"] = json!(enabled);
    }
    if let Some(model_invocation) = body["modelInvocationEnabled"].as_bool() {
        current["modelInvocation"] = json!(model_invocation);
    }
    if current.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
        skill_state.overrides.insert(key.clone(), current);
    } else {
        skill_state.overrides.remove(&key);
    }
    save_state(&state, &skill_state)?;
    let skills_dir = StdPath::new(&state.agent_dir)
        .join("skills")
        .to_string_lossy()
        .to_string();
    Ok(Json(public_skill(&skill_state, &skills_dir, skill)))
}

/// release DELETE /api/skills/:skillName（按 id 查找）：仅可卸载由 Pisper 安装的技能。
pub(crate) async fn remove(
    State(state): State<ArcAppState>,
    Path(skill_name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let session_id = params.get("sessionId").cloned().unwrap_or_default();
    let cwd = if session_id.is_empty() {
        state.cwd.clone()
    } else {
        crate::session_ops::session_cwd_for(&state, &session_id).await?
    };
    let discovered = discover(&state, &cwd);
    let Some(skill) = find_skill(&discovered, &skill_name) else {
        return Err(ApiError::new(
            axum::http::StatusCode::NOT_FOUND,
            "skill_not_found",
            "技能不存在。",
        ));
    };
    let key = value_canonical(&skill.file_path);
    let skills_dir = StdPath::new(&state.agent_dir).join("skills");
    let mut skill_state = load_state(&state);
    let managed = skill_state.installed.contains_key(&key);
    if !managed || !path_inside(&skills_dir.to_string_lossy(), &skill.file_path) {
        return Err(ApiError::bad_request(
            "只能卸载由 Pisper 安装的技能；其他来源可以禁用。",
        ));
    }
    let file_name_is_skill_md = StdPath::new(&skill.file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("skill.md"))
        .unwrap_or(false);
    let target = if file_name_is_skill_md {
        StdPath::new(&skill.file_path)
            .parent()
            .map(StdPath::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(&skill.file_path))
    } else {
        PathBuf::from(&skill.file_path)
    };
    let result = if target.is_dir() {
        std::fs::remove_dir_all(&target)
    } else {
        std::fs::remove_file(&target)
    };
    result.map_err(|e| ApiError::internal(e.to_string()))?;
    skill_state.overrides.remove(&key);
    skill_state.installed.remove(&key);
    save_state(&state, &skill_state)?;
    Ok(Json(json!({"deleted": true})))
}
