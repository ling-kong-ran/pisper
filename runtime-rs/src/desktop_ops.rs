//! release `http/routes/desktop.mjs` 的原生移植：应用更新检查、赞助内容、
//! 桌面宠物皮肤（petdex 清单/安装/状态/精灵图）与桌面 reveal-path。
//! 对齐 release（services/web-desktop-pet-service.mjs、update-check-service.mjs、
//! sponsor-content-service.mjs、shared/desktop-pet-state.mjs）的字段与语义。
//!
//! 诚实接缝：宠物运行状态（state/stateVersion）由 Agent 事件驱动，
//! 本模块先提供 idle 基线；事件映射接入 chat run 后自动更新。

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::HashMap;
use std::path::Path as StdPath;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{product, ApiError, AppState};

type ArcAppState = Arc<AppState>;

const MAX_PET_BYTES: usize = 16 * 1024 * 1024;
const PET_FRAME_WIDTH: u32 = 192;
const PET_FRAME_HEIGHT: u32 = 208;
const PET_SHEET_COLUMNS: u32 = 8;
const PET_MINIMUM_ROWS: u32 = 9;
const PETDEX_MANIFEST_URL: &str = "https://petdex.dev/api/manifest";
const PETDEX_HOST: &str = "petdex.dev";
const PETDEX_ASSETS_HOST: &str = "assets.petdex.dev";
const MANIFEST_MAX_BYTES: usize = 5 * 1024 * 1024;
const MANIFEST_TTL_MS: u64 = 5 * 60 * 1000;
const SLUG_MAX: usize = 80;
const REPOSITORY_API: &str = "https://api.github.com/repos/ling-kong-ran/pisper";
const REPOSITORY_URL: &str = "https://github.com/ling-kong-ran/pisper";
const DEFAULT_BRANCH: &str = "release";
const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_CAMPAIGNS: usize = 50;
const SUPPORTED_LOCALES: [&str; 2] = ["zh-CN", "en-US"];

const PET_SPRITE_NAMES: [&str; 4] = [
    "spritesheet.webp",
    "spritesheet.png",
    "sprite.webp",
    "sprite.png",
];

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Pisper/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// release SLUG_PATTERN：/^[a-z0-9][a-z0-9-]{0,79}$/。
fn slug_pattern_ok(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if bytes.is_empty() || bytes.len() > SLUG_MAX {
        return false;
    }
    (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

// ------------------------------------------------------- image dimensions

struct ImageInfo {
    width: u32,
    height: u32,
    mime: &'static str,
}

/// release readImageDimensions：PNG / WEBP (VP8X|VP8L|VP8) 头解析。
fn read_image_dimensions(buffer: &[u8]) -> Option<ImageInfo> {
    if buffer.len() >= 24
        && buffer[..8] == [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
    {
        let width = u32::from_be_bytes([buffer[16], buffer[17], buffer[18], buffer[19]]);
        let height = u32::from_be_bytes([buffer[20], buffer[21], buffer[22], buffer[23]]);
        return Some(ImageInfo { width, height, mime: "image/png" });
    }
    if buffer.len() < 30 {
        return None;
    }
    if &buffer[0..4] != b"RIFF" || &buffer[8..12] != b"WEBP" {
        return None;
    }
    match &buffer[12..16] {
        b"VP8X" => {
            let read_le = |offset: usize| -> u32 {
                (buffer[offset] as u32)
                    | ((buffer[offset + 1] as u32) << 8)
                    | ((buffer[offset + 2] as u32) << 16)
            };
            Some(ImageInfo {
                width: 1 + read_le(24),
                height: 1 + read_le(27),
                mime: "image/webp",
            })
        }
        b"VP8L" if buffer[20] == 0x2f => Some(ImageInfo {
            width: 1 + buffer[21] as u32 + (((buffer[22] & 0x3f) as u32) << 8),
            height: 1
                + (buffer[22] >> 6) as u32
                + ((buffer[23] as u32) << 2)
                + (((buffer[24] & 0x0f) as u32) << 10),
            mime: "image/webp",
        }),
        b"VP8 " if buffer[23] == 0x9d && buffer[24] == 0x01 && buffer[25] == 0x2a => {
            let read_le16 = |offset: usize| -> u32 {
                (buffer[offset] as u32) | ((buffer[offset + 1] as u32) << 8)
            };
            Some(ImageInfo {
                width: read_le16(26) & 0x3fff,
                height: read_le16(28) & 0x3fff,
                mime: "image/webp",
            })
        }
        _ => None,
    }
}

fn is_pet_sheet_dimensions(info: &ImageInfo) -> bool {
    info.width == PET_FRAME_WIDTH * PET_SHEET_COLUMNS
        && info.height >= PET_FRAME_HEIGHT * PET_MINIMUM_ROWS
        && info.height % PET_FRAME_HEIGHT == 0
}

fn normalize_pet_opacity(value: &Value) -> f64 {
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    match number {
        Some(value) if value.is_finite() => {
            (value.clamp(0.2, 1.0) * 100.0).round() / 100.0
        }
        _ => 1.0,
    }
}

// ------------------------------------------------------------ pet storage

fn pet_roots(state: &AppState) -> Vec<(PathBuf, &'static str)> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    vec![
        (StdPath::new(&state.data_dir).join("desktop-pets"), "pisper"),
        (home.join(".petdex").join("pets"), "petdex"),
        (home.join(".codex").join("pets"), "petdex"),
    ]
}

fn pet_preferences(state: &AppState) -> Value {
    let path = StdPath::new(&state.data_dir).join("desktop-pet.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn pet_preferences_patch(state: &AppState, patch: Value) -> Result<Value, ApiError> {
    let mut next = pet_preferences(state).as_object().cloned().unwrap_or_default();
    if let Some(object) = patch.as_object() {
        for (key, value) in object {
            next.insert(key.clone(), value.clone());
        }
    }
    std::fs::create_dir_all(&state.data_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let path = StdPath::new(&state.data_dir).join("desktop-pet.json");
    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&json!(next)).map_err(|e| ApiError::internal(e.to_string()))?))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Value::Object(next))
}

fn safe_json(path: &StdPath) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

struct LoadedPet {
    slug: String,
    name: String,
    path: PathBuf,
    mime: &'static str,
    width: u32,
    height: u32,
    mtime_ms: u64,
}

fn load_pet(root: &StdPath, slug: &str) -> Option<LoadedPet> {
    let directory = root.join(slug);
    for file_name in PET_SPRITE_NAMES {
        let sprite_path = directory.join(file_name);
        let Ok(meta) = std::fs::metadata(&sprite_path) else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() as usize > MAX_PET_BYTES {
            continue;
        }
        let Ok(buffer) = std::fs::read(&sprite_path) else {
            continue;
        };
        let Some(image) = read_image_dimensions(&buffer) else {
            continue;
        };
        if !is_pet_sheet_dimensions(&image) {
            continue;
        }
        let metadata = safe_json(&directory.join("pet.json"));
        let name = metadata["displayName"]
            .as_str()
            .or_else(|| metadata["name"].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(slug)
            .to_string();
        let mime = image.mime;
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        return Some(LoadedPet {
            slug: slug.to_string(),
            name,
            path: sprite_path,
            mime,
            width: image.width,
            height: image.height,
            mtime_ms,
        });
    }
    None
}

fn find_pet(state: &AppState, slug: &str) -> Option<LoadedPet> {
    if !slug_pattern_ok(slug) {
        return None;
    }
    for (root, _) in pet_roots(state) {
        if let Some(pet) = load_pet(&root, slug) {
            return Some(pet);
        }
    }
    None
}

fn installed_pets(state: &AppState) -> Vec<(Value, &'static str)> {
    let mut pets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (root, source) in pet_roots(state) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut slugs: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        slugs.sort();
        for slug in slugs {
            if !seen.insert(slug.clone()) {
                continue;
            }
            let Some(pet) = load_pet(&root, &slug) else {
                continue;
            };
            pets.push((
                json!({"slug": pet.slug, "name": pet.name, "source": source}),
                source,
            ));
        }
    }
    pets
}

fn pet_status(state: &AppState) -> Value {
    let preferences = pet_preferences(state);
    let enabled = preferences["enabled"].as_bool().unwrap_or(false);
    let selected_slug = preferences["selectedSlug"].as_str().unwrap_or("");
    let installed = installed_pets(state);
    let selected = installed
        .iter()
        .find(|(pet, _)| pet["slug"] == selected_slug)
        .or_else(|| installed.first())
        .map(|(pet, _)| pet.clone());
    let selected_slug = selected
        .as_ref()
        .map(|pet| pet["slug"].as_str().unwrap_or("").to_string())
        .unwrap_or_default();
    let selected_name = selected
        .as_ref()
        .map(|pet| pet["name"].as_str().unwrap_or("").to_string())
        .unwrap_or_default();
    let loaded = find_pet(state, &selected_slug);
    let running = enabled && loaded.is_some();
    json!({
        "supported": true,
        "enabled": enabled,
        "running": running,
        "selectedSlug": selected_slug,
        "selectedName": selected_name,
        "installed": installed.iter().map(|(pet, _)| pet.clone()).collect::<Vec<_>>(),
        "opacity": preferences.get("opacity").map(normalize_pet_opacity).unwrap_or(1.0),
        "state": "idle",
        "stateVersion": 0,
        "sheetWidth": loaded.as_ref().map(|pet| pet.width).unwrap_or(0),
        "sheetHeight": loaded.as_ref().map(|pet| pet.height).unwrap_or(0),
        "spriteUrl": loaded.as_ref().map(|pet| {
            format!("/api/desktop-pet/sprite?slug={}&v={}", pet.slug, pet.mtime_ms)
        }).unwrap_or_default(),
    })
}

// ------------------------------------------------------------ petdex catalog

#[derive(Clone)]
struct ManifestEntry {
    slug: String,
    display_name: String,
    spritesheet_url: String,
}

async fn petdex_manifest() -> Result<Vec<ManifestEntry>, ApiError> {
    let response = http_client()
        .get(PETDEX_MANIFEST_URL)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| ApiError::internal(format!("Petdex request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(ApiError::internal(format!(
            "Petdex request failed: HTTP {}",
            response.status().as_u16()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if bytes.len() > MANIFEST_MAX_BYTES {
        return Err(ApiError::internal("宠物资源超过允许的大小。"));
    }
    let data: Value = serde_json::from_slice(&bytes)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let pets = data["pets"].as_array().cloned().unwrap_or_default();
    Ok(pets
        .into_iter()
        .take(5000)
        .filter_map(|pet| {
            let slug = pet["slug"].as_str()?.trim().to_lowercase();
            let display_name = pet["displayName"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&slug)
                .to_string();
            let spritesheet_url = pet["spritesheetUrl"].as_str().unwrap_or("").to_string();
            if !slug_pattern_ok(&slug) {
                return None;
            }
            let ok = spritesheet_url.starts_with("https://")
                && spritesheet_url
                    .trim_start_matches("https://")
                    .split('/')
                    .next()
                    .map(|host| host == PETDEX_ASSETS_HOST)
                    .unwrap_or(false);
            if !ok {
                return None;
            }
            Some(ManifestEntry {
                slug,
                display_name,
                spritesheet_url,
            })
        })
        .collect())
}

/// release GET /api/desktop-pet/catalog?query=
pub(crate) async fn pet_catalog(
    State(state): State<ArcAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let needle = params.get("query").cloned().unwrap_or_default().trim().to_lowercase();
    let manifest = petdex_manifest().await?;
    let pets: Vec<Value> = manifest
        .into_iter()
        .filter(|pet| {
            needle.is_empty()
                || pet.slug.contains(&needle)
                || pet.display_name.to_lowercase().contains(&needle)
        })
        .take(if needle.is_empty() { 12 } else { 40 })
        .map(|pet| json!({"slug": pet.slug, "displayName": pet.display_name}))
        .collect();
    let _ = &state;
    Ok(Json(json!({ "pets": pets })))
}

/// release POST /api/desktop-pet/install {slug}
pub(crate) async fn pet_install(
    State(state): State<ArcAppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let slug = body["slug"].as_str().unwrap_or("").trim().to_lowercase();
    if !slug_pattern_ok(&slug) {
        return Err(ApiError::bad_request("宠物标识格式无效。"));
    }
    let manifest = petdex_manifest().await?;
    let entry = manifest
        .iter()
        .find(|pet| pet.slug == slug)
        .ok_or_else(|| ApiError::bad_request("Petdex 中未找到这只宠物。"))?;
    let response = http_client()
        .get(&entry.spritesheet_url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| ApiError::internal(format!("Petdex request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(ApiError::internal(format!(
            "Petdex request failed: HTTP {}",
            response.status().as_u16()
        )));
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let buffer = response
        .bytes()
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if buffer.is_empty() || buffer.len() > MAX_PET_BYTES {
        return Err(ApiError::internal("宠物资源超过允许的大小。"));
    }
    let image = read_image_dimensions(&buffer)
        .filter(|image| is_pet_sheet_dimensions(image))
        .ok_or_else(|| ApiError::bad_request("宠物图集格式无效。"))?;
    if !content_type.is_empty() && !content_type.starts_with(image.mime) {
        return Err(ApiError::bad_request("宠物图集格式无效。"));
    }
    let directory = StdPath::new(&state.data_dir).join("desktop-pets").join(&slug);
    std::fs::create_dir_all(&directory).map_err(|e| ApiError::internal(e.to_string()))?;
    let extension = if image.mime == "image/png" { "png" } else { "webp" };
    for file_name in PET_SPRITE_NAMES {
        let _ = std::fs::remove_file(directory.join(file_name));
    }
    std::fs::write(directory.join(format!("spritesheet.{extension}")), &buffer)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    std::fs::write(
        directory.join("pet.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({"id": slug, "displayName": entry.display_name}))
                .map_err(|e| ApiError::internal(e.to_string()))?
        ),
    )
    .map_err(|e| ApiError::internal(e.to_string()))?;
    pet_preferences_patch(&state, json!({"selectedSlug": slug}))?;
    Ok(Json(pet_status(&state)))
}

/// release POST /api/desktop-pet/enabled {enabled}
pub(crate) async fn pet_set_enabled(
    State(state): State<ArcAppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let enabled = body["enabled"].as_bool().unwrap_or(false);
    let installed = installed_pets(&state);
    if enabled && installed.is_empty() {
        return Err(ApiError::bad_request("请先安装一只宠物。"));
    }
    let current = pet_preferences(&state);
    let selected = current["selectedSlug"].as_str().unwrap_or("");
    let selected = if selected.is_empty() {
        installed
            .first()
            .and_then(|(pet, _)| pet["slug"].as_str().map(str::to_string))
            .unwrap_or_default()
    } else {
        selected.to_string()
    };
    pet_preferences_patch(&state, json!({"enabled": enabled, "selectedSlug": selected}))?;
    Ok(Json(pet_status(&state)))
}

/// release POST /api/desktop-pet/opacity {opacity}
pub(crate) async fn pet_set_opacity(
    State(state): State<ArcAppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    pet_preferences_patch(&state, json!({"opacity": normalize_pet_opacity(&body["opacity"])}))?;
    Ok(Json(pet_status(&state)))
}

/// release POST /api/desktop-pet/select {slug}
pub(crate) async fn pet_select(
    State(state): State<ArcAppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let slug = body["slug"].as_str().unwrap_or("");
    find_pet(&state, slug).ok_or_else(|| ApiError::bad_request("宠物尚未安装。"))?;
    pet_preferences_patch(&state, json!({"selectedSlug": slug}))?;
    Ok(Json(pet_status(&state)))
}

/// release DELETE /api/desktop-pet/:slug
pub(crate) async fn pet_remove(
    State(state): State<ArcAppState>,
    Path(slug): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let slug = slug.trim().to_lowercase();
    if !slug_pattern_ok(&slug) {
        return Err(ApiError::bad_request("宠物标识格式无效。"));
    }
    let managed_root = StdPath::new(&state.data_dir).join("desktop-pets");
    if load_pet(&managed_root, &slug).is_none() {
        return Err(ApiError::bad_request("只能删除由 Pisper 安装的宠物。"));
    }
    let directory = managed_root.join(&slug);
    std::fs::remove_dir_all(&directory).map_err(|e| ApiError::internal(e.to_string()))?;
    let current = pet_preferences(&state);
    let installed = installed_pets(&state);
    let selected = current["selectedSlug"].as_str().unwrap_or("").to_string();
    let still_installed = installed
        .iter()
        .any(|(pet, _)| pet["slug"] == selected);
    let next_selected = if selected == slug || !still_installed {
        installed
            .first()
            .and_then(|(pet, _)| pet["slug"].as_str().map(str::to_string))
            .unwrap_or_default()
    } else {
        selected
    };
    pet_preferences_patch(
        &state,
        json!({
            "enabled": current["enabled"].as_bool().unwrap_or(false) && !installed.is_empty(),
            "selectedSlug": next_selected,
        }),
    )?;
    Ok(Json(pet_status(&state)))
}

/// release GET /api/desktop-pet/sprite?slug=
pub(crate) async fn pet_sprite(
    State(state): State<ArcAppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let pet = find_pet(&state, params.get("slug").map(String::as_str).unwrap_or(""))
        .ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "pet_not_found", "宠物资源不存在。")
        })?;
    let buffer = std::fs::read(&pet.path).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((
        [
            (header::CONTENT_TYPE, pet.mime.to_string()),
            (header::CACHE_CONTROL, "private, max-age=86400".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        buffer,
    )
        .into_response())
}

/// release GET /api/desktop-pet
pub(crate) async fn pet_status_handler(State(state): State<ArcAppState>) -> Json<Value> {
    Json(pet_status(&state))
}

// ------------------------------------------------------------- app update

fn valid_commit(value: &str) -> String {
    let commit = value.trim().to_lowercase();
    let hex_ok = !commit.is_empty()
        && (7..=40).contains(&commit.len())
        && commit.chars().all(|c| c.is_ascii_hexdigit());
    if hex_ok {
        commit
    } else {
        String::new()
    }
}

async fn current_git_commit(state: &AppState) -> String {
    if let Ok(sha) = std::env::var("PISPER_COMMIT_SHA") {
        let sha = valid_commit(&sha);
        if !sha.is_empty() {
            return sha;
        }
    }
    // release 在 Web 源码根目录跑 git rev-parse；工作区不是仓库时依次
    // 回退到可执行文件所在目录的祖先（源码 checkout 运行场景）。
    let mut candidates = vec![state.cwd.clone()];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_string_lossy().to_string());
        }
    }
    for cwd in candidates {
        let output = tokio::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&cwd)
            .output()
            .await;
        if let Ok(output) = output {
            if output.status.success() {
                let sha = valid_commit(&String::from_utf8_lossy(&output.stdout));
                if !sha.is_empty() {
                    return sha;
                }
            }
        }
    }
    String::new()
}

/// release GET /api/app-update：与远端 release 分支比较 commit。
pub(crate) async fn app_update(State(state): State<ArcAppState>) -> Result<Json<Value>, ApiError> {
    let current_commit = current_git_commit(&state).await;
    if current_commit.is_empty() {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "update_check_failed",
            "无法识别当前 Web 源码的 Git commit。请使用 Git 仓库运行，或设置 PISPER_COMMIT_SHA。",
        ));
    }
    let url = format!("{REPOSITORY_API}/compare/{current_commit}...{DEFAULT_BRANCH}");
    let response = http_client()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                "update_check_failed",
                error.to_string(),
            )
        })?;
    if !response.status().is_success() {
        // release app-update 路由把检查失败包裹为 502 {error}。
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "update_check_failed",
            format!("GitHub commit 比较失败：HTTP {}", response.status().as_u16()),
        ));
    }
    let comparison: Value = response.json().await.map_err(|error| {
        ApiError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "update_check_failed",
            error.to_string(),
        )
    })?;
    let commits = comparison["commits"].as_array().cloned().unwrap_or_default();
    let ahead_by = comparison["ahead_by"].as_u64().unwrap_or(0).max(0);
    let available = ahead_by > 0;
    let latest = commits.last().cloned().unwrap_or_else(|| comparison["base_commit"].clone());
    let notes = {
        let lines: Vec<String> = commits
            .iter()
            .rev()
            .take(20)
            .map(|item| {
                let title = item["commit"]["message"]
                    .as_str()
                    .unwrap_or("Untitled commit")
                    .lines()
                    .next()
                    .unwrap_or("Untitled commit")
                    .trim();
                let title = if title.is_empty() { "Untitled commit" } else { title };
                let sha = item["sha"].as_str().unwrap_or("");
                format!("- {title} ({})", &sha[..sha.len().min(7)])
            })
            .collect();
        if lines.is_empty() {
            String::new()
        } else {
            format!("## {DEFAULT_BRANCH} 分支更新\n\n{}", lines.join("\n"))
        }
    };
    let available_commit = latest["sha"].as_str().map(valid_commit).unwrap_or_default();
    let release_date = latest["commit"]["committer"]["date"]
        .as_str()
        .or_else(|| latest["commit"]["author"]["date"].as_str())
        .map(str::to_string);
    Ok(Json(json!({
        "state": if available { "available" } else { "current" },
        "currentVersion": env!("CARGO_PKG_VERSION"),
        "currentCommit": current_commit,
        "availableCommit": available_commit,
        "behindBy": ahead_by,
        "releaseDate": release_date,
        "branch": DEFAULT_BRANCH,
        "notes": notes,
        "releaseUrl": format!("{REPOSITORY_URL}/commits/{DEFAULT_BRANCH}"),
        "canDownload": false,
        "checkedAt": crate::session_ops::iso_timestamp(product::now_ms()),
        "message": if available {
            format!("Web 源码落后 {DEFAULT_BRANCH} {ahead_by} 个提交，请查看更新内容后自行更新。")
        } else {
            format!("当前 Web 源码已同步 {DEFAULT_BRANCH}。")
        },
    })))
}

// --------------------------------------------------------------- sponsors

fn sponsor_cache_path(state: &AppState) -> PathBuf {
    StdPath::new(&state.data_dir).join("sponsors-cache.json")
}

fn locale_text(value: &Value, locale: &str) -> String {
    value[locale]
        .as_str()
        .or_else(|| value["zh-CN"].as_str())
        .or_else(|| value["en-US"].as_str())
        .unwrap_or("")
        .to_string()
}

fn active_campaign(campaign: &Value, now_ms: u64) -> bool {
    if campaign["enabled"] == false {
        return false;
    }
    let starts_at = campaign["startsAt"].as_str().and_then(crate::session_ops::parse_timestamp_ms);
    if let Some(starts_at) = starts_at {
        if starts_at > now_ms {
            return false;
        }
    }
    let ends_at = campaign["endsAt"].as_str().and_then(crate::session_ops::parse_timestamp_ms);
    if let Some(ends_at) = ends_at {
        if ends_at <= now_ms {
            return false;
        }
    }
    true
}

/// release GET /api/sponsors/:placement?locale=&refresh=
pub(crate) async fn sponsor_placement(
    State(state): State<ArcAppState>,
    Path(placement): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let placement = placement.trim().to_lowercase();
    if !slug_pattern_ok(&placement) {
        return Err(ApiError::bad_request("赞助位名称无效。"));
    }
    let locale = params
        .get("locale")
        .map(String::as_str)
        .filter(|value| SUPPORTED_LOCALES.contains(value))
        .unwrap_or("zh-CN")
        .to_string();
    // 读取缓存（sponsors-cache.json；远端拉取失败时用缓存兜底）。
    let cache: Value = std::fs::read_to_string(sponsor_cache_path(&state))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({"version": 1, "document": {"schemaVersion": 1, "campaigns": []}}));
    let document = cache["document"].clone();
    let checked_at = cache["checkedAt"].as_str().unwrap_or("").to_string();
    let source = cache["source"].as_str().unwrap_or("cache").to_string();
    let now = product::now_ms();
    let mut campaigns: Vec<Value> = document["campaigns"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|campaign| campaign["placement"] == placement && active_campaign(&campaign, now))
        .collect();
    campaigns.sort_by(|left, right| {
        let left_priority = left["priority"].as_i64().unwrap_or(0);
        let right_priority = right["priority"].as_i64().unwrap_or(0);
        right_priority
            .cmp(&left_priority)
            .then_with(|| left["id"].as_str().unwrap_or("").cmp(right["id"].as_str().unwrap_or("")))
    });
    let campaigns: Vec<Value> = campaigns
        .into_iter()
        .map(|campaign| {
            json!({
                "id": campaign["id"],
                "name": locale_text(&campaign["name"], &locale),
                "description": locale_text(&campaign["description"], &locale),
                "href": campaign["href"],
            })
        })
        .collect();
    Ok(Json(json!({
        "placement": placement,
        "campaigns": campaigns,
        "checkedAt": if checked_at.is_empty() { Value::Null } else { Value::String(checked_at) },
        "source": source,
    })))
}

// ------------------------------------------------------------ reveal path

#[cfg(target_os = "windows")]
fn reveal_command(target: &StdPath, is_directory: bool) -> Result<(), ApiError> {
    use std::os::windows::process::CommandExt;
    // 部分 Windows Explorer 版本会静默忽略正斜杠路径。
    let windows_path = target.to_string_lossy().replace('/', "\\");
    let result = if is_directory {
        tokio::process::Command::new("explorer.exe")
            .arg(&windows_path)
            .spawn()
    } else {
        // /select, 需要整体作为一个参数传给 Explorer，路径含空格时必须带引号。
        tokio::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{windows_path}\""))
            .spawn()
    };
    result
        .map(|_| ())
        .map_err(|error| ApiError::internal(format!("启动文件管理器失败：{error}")))
}

#[cfg(not(target_os = "windows"))]
fn reveal_command(target: &StdPath, is_directory: bool) -> Result<(), ApiError> {
    let result = if cfg!(target_os = "macos") {
        if is_directory {
            tokio::process::Command::new("open").arg(target).spawn()
        } else {
            tokio::process::Command::new("open").arg("-R").arg(target).spawn()
        }
    } else {
        tokio::process::Command::new("xdg-open").arg(target).spawn()
    };
    result
        .map(|_| ())
        .map_err(|error| ApiError::internal(format!("启动文件管理器失败：{error}")))
}

/// release POST /api/desktop/reveal-path {path}：在宿主文件管理器中显示。
pub(crate) async fn reveal_path(
    State(state): State<ArcAppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let input = body["path"].as_str().unwrap_or("").to_string();
    if input.trim().is_empty() {
        return Err(ApiError::bad_request("path 不能为空。"));
    }
    let target = PathBuf::from(&input);
    let meta = std::fs::metadata(&target)
        .map_err(|_| ApiError::bad_request(format!("本地路径不存在：{input}")))?;
    let is_directory = meta.is_dir();
    let windows_path = target.to_string_lossy().replace('/', "\\");
    let revealed = reveal_command(&target, is_directory);
    revealed?;
    Ok(Json(json!({
        "revealed": true,
        "path": target.to_string_lossy(),
        "fallback": false,
    })))
}
