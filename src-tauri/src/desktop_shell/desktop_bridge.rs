use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{
    fs::{canonicalize, create_dir_all, write as write_file, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Url};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::{NotificationExt, PermissionState};
use tauri_plugin_opener::OpenerExt;

#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
const APP_USER_MODEL_ID: &str = "com.lingkongran.pisper";
const RELEASES_URL: &str = "https://github.com/ling-kong-ran/pisper/releases";
pub const UPDATER_PUBLIC_KEY: &str = match option_env!("PISPER_TAURI_UPDATER_PUBLIC_KEY") {
    Some(value) => value,
    None => "",
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    desktop: bool,
    packaged: bool,
    version: String,
    platform: &'static str,
    arch: &'static str,
    releases_url: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetOpenInput {
    name: String,
    data: String,
}

const OPEN_ASSETS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const OPEN_ASSETS_MAX_BYTES: u64 = 512 * 1024 * 1024;

fn cleanup_open_assets_at(root: &Path, now: SystemTime, ttl: Duration, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || !entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|value| value.is_ascii_digit())
        {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        if age < ttl {
            continue;
        }
        let size = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .filter_map(|file| file.ok())
            .filter_map(|file| file.metadata().ok())
            .filter(|file| file.is_file())
            .map(|file| file.len())
            .sum::<u64>();
        candidates.push((modified, size, path));
    }
    candidates.sort_by_key(|(modified, _, _)| *modified);
    let mut total = candidates.iter().map(|(_, size, _)| *size).sum::<u64>();
    for (_, size, path) in candidates {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_dir_all(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

pub(crate) fn cleanup_open_assets(app: &AppHandle) {
    let Ok(root) = app.path().app_cache_dir() else {
        return;
    };
    cleanup_open_assets_at(
        &root.join("open-assets"),
        SystemTime::now(),
        OPEN_ASSETS_TTL,
        OPEN_ASSETS_MAX_BYTES,
    );
}

#[derive(Deserialize)]
pub struct NotificationInput {
    title: String,
    body: String,
}

#[derive(Serialize)]
pub struct NotificationStatus {
    supported: bool,
    permission: &'static str,
    reason: &'static str,
}

#[derive(Serialize)]
pub struct NotificationResult {
    shown: bool,
    supported: bool,
    permission: &'static str,
    reason: &'static str,
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        value => value,
    }
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        value => value,
    }
}

fn clipped(value: &str, limit: usize) -> String {
    value.trim().chars().take(limit).collect()
}

fn checked_at() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn update_log_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|directory| directory.join("component-updater.log"))
}

fn append_app_log(app: &AppHandle, file_name: &str, line: &str) {
    let Some(path) = app
        .path()
        .app_log_dir()
        .ok()
        .map(|directory| directory.join(file_name))
    else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

fn write_update_log(app: &AppHandle, channel: &str, message: &str) {
    append_app_log(
        app,
        "component-updater.log",
        &format!("{} [{channel}] {message}", checked_at()),
    );
}

/// 记录本地路径 reveal 的每次尝试（含成功）。
/// opener 插件可能吞掉部分系统 Shell 错误后返回成功，没有这份日志就无法
/// 区分用户反馈的「点击无反应」发生在桥接层还是系统 Shell 层。
fn log_local_reveal(app: &AppHandle, message: &str) {
    append_app_log(
        app,
        "local-reveal.log",
        &format!("{} {message}", checked_at()),
    );
}

pub(crate) fn log_component_update(app: &AppHandle, message: &str) {
    write_update_log(app, "component-update", message);
}

#[tauri::command]
pub fn desktop_get_app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        desktop: true,
        packaged: !cfg!(debug_assertions),
        version: app.package_info().version.to_string(),
        platform: platform(),
        arch: arch(),
        releases_url: RELEASES_URL,
    }
}

#[tauri::command]
pub async fn desktop_pick_directory(
    app: AppHandle,
    initial_directory: Option<String>,
) -> Result<Option<String>, String> {
    let mut dialog = app.dialog().file();
    if let Some(initial_directory) = initial_directory.filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(initial_directory);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog
        .blocking_pick_folder()
        .map(|path| {
            path.simplified()
                .into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .transpose()
}

#[tauri::command]
pub async fn desktop_pick_files(app: AppHandle, initial_directory: Option<String>) -> Vec<String> {
    let mut dialog = app.dialog().file();
    if let Some(initial_directory) = initial_directory.filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(initial_directory);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| path.simplified().into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
pub fn desktop_set_language(app: AppHandle, language: String) -> String {
    let normalized = match language.as_str() {
        "en-US" => "en-US",
        _ => "zh-CN",
    };
    crate::desktop_shell::set_tray_language(&app, normalized);
    normalized.into()
}

#[tauri::command]
pub fn desktop_open_url(app: AppHandle, url: String) -> bool {
    let Ok(parsed) = Url::parse(&url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return false;
    }
    app.opener()
        .open_url(parsed.to_string(), None::<&str>)
        .is_ok()
}

#[tauri::command]
pub fn desktop_open_asset(app: AppHandle, input: AssetOpenInput) -> Result<bool, String> {
    let name = input.name.trim();
    if name.is_empty()
        || name.chars().count() > 180
        || name.chars().any(char::is_control)
        || name.contains(['/', '\\'])
    {
        return Err("资产文件名无效。".into());
    }
    if input.data.len() > 180 * 1024 * 1024 {
        return Err("资产超过 128 MB 原生打开限制。".into());
    }
    let bytes = BASE64_STANDARD
        .decode(input.data)
        .map_err(|_| "资产内容不是有效的 base64 数据。".to_string())?;
    if bytes.is_empty() || bytes.len() > 128 * 1024 * 1024 {
        return Err("资产内容为空或超过 128 MB 原生打开限制。".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("open-assets")
        .join(nonce.to_string());
    create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(name);
    write_file(&path, bytes).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map(|_| true)
        .map_err(|error| error.to_string())
}

fn canonical_local_path(value: &str) -> Result<PathBuf, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 32_768 || value.chars().any(char::is_control) {
        return Err("本地路径无效。".into());
    }
    let requested = PathBuf::from(value);
    if !requested.is_absolute() {
        return Err("本地路径必须是绝对路径。".into());
    }
    let path = canonicalize(requested).map_err(|error| format!("本地路径不可访问：{error}"))?;
    if !path.is_file() && !path.is_dir() {
        return Err("本地路径不是文件或目录。".into());
    }
    Ok(path)
}

// 兜底回退的最大层数：模型常见错误（缺时间戳前缀、多一层子目录）都在 3 层内，
// 更远的祖先打开后没有定位价值，不如直接报错让用户看到「无法显示」。
const ANCESTOR_FALLBACK_MAX_HOPS: usize = 3;

/// 盘符根（Windows 的 `C:\`）或文件系统根（POSIX 的 `/`）：
/// 回退到根目录打开后对定位没有意义，不算有效兜底。
fn is_volume_root(path: &Path) -> bool {
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Prefix(_)), None) => true,
        (Some(Component::Prefix(_)), Some(Component::RootDir)) => components.next().is_none(),
        (None | Some(Component::RootDir), None) => true,
        _ => false,
    }
}

/// 沿父目录向上查找最近的已存在目录（最多 [ANCESTOR_FALLBACK_MAX_HOPS] 层，
/// 且排除盘符/文件系统根目录）；用于链接目标缺失时的兜底 reveal。
fn nearest_existing_ancestor(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 32_768 || value.chars().any(char::is_control) {
        return None;
    }
    let requested = PathBuf::from(value);
    if !requested.is_absolute() {
        return None;
    }
    let mut cursor = requested;
    for _ in 0..ANCESTOR_FALLBACK_MAX_HOPS {
        let parent = cursor.parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        if parent.is_dir() && !is_volume_root(parent) {
            // 按组件重建路径：Windows 上把前端传来的 `C:/...` 正斜杠形态
            // 规范化为反斜杠（新嵌入版 Explorer 的命令行解析器不认正斜杠，
            // 会静默回退到打开「文档」）。
            let normalized: PathBuf = parent.components().collect();
            return Some(normalized);
        }
        cursor = parent.to_path_buf();
    }
    None
}

/// 链接目标的解析结果：路径本身，或兜底的已存在祖先目录。
#[derive(Debug)]
enum RevealTarget {
    /// 请求路径存在，直接 reveal 该路径。
    Path(PathBuf),
    /// 请求路径不存在，退回最近的已存在祖先目录。
    Ancestor(PathBuf),
}

/// 纯解析逻辑（不依赖 Tauri/AppHandle）：目标存在则原样 reveal；
/// 缺失时兜底到最近的已存在祖先目录，便于用单测复现「模型拼写路径缺失」场景。
fn resolve_reveal_target(value: &str) -> Result<RevealTarget, String> {
    match canonical_local_path(value) {
        Ok(path) => Ok(RevealTarget::Path(path)),
        // 模型拼写的路径可能并不存在（例如生成产物缺时间戳前缀）；
        // 兜底显示最近的已存在祖先目录，让用户仍能定位到产物所在目录。
        Err(original_error) => match nearest_existing_ancestor(value) {
            Some(ancestor) => Ok(RevealTarget::Ancestor(ancestor)),
            None => Err(original_error),
        },
    }
}

/// reveal 一个已存在文件；返回的错误字符串会进入 local-reveal.log 与前端 Toast。
///
/// win/macos 不复用 plugin 的 reveal：这两个平台上的「成功」返回值不可信
///（见 shell_reveal 模块文档）；其余平台保留 plugin 的 reveal
///（xdg-desktop-portal 经 D-Bus 真实返回错误）。
fn reveal_local_file(app: &AppHandle, path: &Path) -> Result<(), String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        // 直接系统 shell API 分支不需要 Tauri handle。
        let _ = app;
        shell_reveal::reveal_item(path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        app.opener()
            .reveal_item_in_dir(path)
            .map_err(|error| error.to_string())
    }
}

/// 打开目录（文件管理器窗口）；返回的错误字符串同样进入 local-reveal.log 与前端 Toast。
///
/// plugin 的 open_path 内部用 `open::that_detached`（发射后不管：线程结果被丢弃，
/// 所有平台上失败都静默）。这里非 Windows 用阻塞的 `open::that`（真实退出码），
/// Windows 用直接 ShellExecuteExW（真实 HRESULT）。
fn open_local_dir(app: &AppHandle, path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = app;
        shell_reveal::open_dir(path)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        open::that(path).map_err(|error| format!("打开目录 {} 失败：{error}", path.display()))
    }
}

/// 在系统文件管理器中 reveal/打开，统一校验「真实成功信号」。
///
/// tauri-plugin-opener 2.5.x 在多个平台上存在「静默成功」路径：
/// - Windows（Win11 24H2 一代）：`SHOpenFolderAndSelectItems` 返回 S_OK 却
///   完全不打开窗口；`open` crate 的 `that_detached`（plugin open_path 所用）
///   丢弃线程结果，`start` 失败同样不可见；
/// - macOS：`NSWorkspace.activateFileViewerSelectingURLs` 的 BOOL 返回值在绑定层
///   已被丢弃（签名返回 ()），Finder 没显示也会报成功。
///
/// 于是这些系统上「点了没反应」，Pisper 却弹成功 Toast。这里在 win/macos
/// 直接调用系统 shell API，且只在 API 明确报告成功时才算成功（失败透传真实
/// 错误码）；其余平台（Linux/BSD，xdg-desktop-portal）错误经 D-Bus 真实返回，
/// 保留 plugin 路径。
#[cfg(any(windows, target_os = "macos"))]
mod shell_reveal {
    use std::path::Path;

    #[cfg(windows)]
    use windows::Win32::Foundation::CloseHandle;
    #[cfg(windows)]
    use windows::{
        core::{w, HSTRING, PCWSTR, PWSTR},
        Win32::{
            Foundation::ERROR_FILE_NOT_FOUND,
            System::{
                Com::CoInitialize,
                Threading::{
                    CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
                },
            },
            UI::{
                Shell::{
                    Common::ITEMIDLIST, ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems,
                    ShellExecuteExW, SHELLEXECUTEINFOW,
                },
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        },
    };

    #[cfg(windows)]
    struct OwnedItemIdList {
        hstring: HSTRING,
        item: *const ITEMIDLIST,
    }

    #[cfg(windows)]
    impl OwnedItemIdList {
        fn new(path: &Path) -> Result<Self, String> {
            let hstring = HSTRING::from(path);
            let item = unsafe { ILCreateFromPathW(&hstring) };
            if item.is_null() {
                Err(format!("无法转换为 shell 项目 ID 列表：{}", path.display()))
            } else {
                Ok(Self { hstring, item })
            }
        }
    }

    #[cfg(windows)]
    impl Drop for OwnedItemIdList {
        fn drop(&mut self) {
            if !self.item.is_null() {
                unsafe { ILFree(Some(self.item)) };
            }
        }
    }

    /// 把常见 HRESULT 翻译成可读描述，便于在 local-reveal.log 里直接判断失败来源。
    #[cfg(windows)]
    pub(super) fn describe_hresult(code: i32) -> &'static str {
        match code as u32 {
            0x8007_0002 => "文件未找到（FILE_NOT_FOUND）",
            0x8007_0005 => "拒绝访问（可能是提权运行或安全软件拦截）",
            0x8000_4005 => "未指定的失败（E_FAIL）",
            0x8000_4002 => "不支持（E_NOTIMPL）",
            0x8007_007E => "找不到模块（DLL 缺失）",
            0x8007_0057 => "参数无效（INVALID_ARGUMENT）",
            _ => "未知错误码",
        }
    }

    #[cfg(windows)]
    fn shell_error(label: &str, code: i32) -> String {
        format!(
            "{label} 失败：HRESULT 0x{:08X}（{}）",
            code as u32,
            describe_hresult(code)
        )
    }

    /// 构造 `explorer.exe /select` 参数；含双引号的路径无法安全解析，返回 None。
    #[cfg(windows)]
    pub(super) fn build_select_params(path: &Path) -> Option<String> {
        let display = path.to_string_lossy();
        if display.contains('"') {
            return None;
        }
        // explorer 命令行参数统一用反斜杠（新嵌入版 Explorer 对正斜杠路径
        // 会静默回退到打开「文档」）；Windows 路径中 `/` 只会作为分隔符出现。
        let display = display.replace('/', "\\");
        Some(format!("/select,\"{display}\""))
    }

    /// 在资源管理器中选中 `path`；失败时返回带真实 HRESULT 的错误。
    #[cfg(windows)]
    pub fn reveal_item(path: &Path) -> Result<(), String> {
        // shell API 对 \\?\ verbatim 路径不友好，先规范化（与 plugin 的 dunce 处理一致）。
        let path = dunce::simplified(path);

        // shell 命名空间 API 需要当前线程已初始化 COM；重复初始化是安全的。
        let _ = unsafe { CoInitialize(None) };

        // 首选 `explorer.exe /select,"path"`：在 Win11 24H2 一代嵌入版 Explorer
        // 上，SHOpenFolderAndSelectItems 是静默 no-op（返回 S_OK 但不打开窗口），
        // 而 /select 走 explorer.exe 启动链路、可靠可见（已在受影响系统实测验证）。
        // 参数直接传给 explorer.exe（不经 shell 解析，无注入面）。
        if let Some(params) = build_select_params(path) {
            let file_string = HSTRING::from("explorer.exe");
            let params_string = HSTRING::from(params.as_str());
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as _,
                nShow: SW_SHOWNORMAL.0,
                lpFile: PCWSTR(file_string.as_ptr()),
                lpParameters: PCWSTR(params_string.as_ptr()),
                ..Default::default()
            };
            if unsafe { ShellExecuteExW(&mut info) }.is_ok() {
                return Ok(());
            }
        }
        // 兜底：官方 SHOpenFolderAndSelectItems（/select 被策略限制的环境）。
        reveal_via_shopenfolder(path)
    }

    #[cfg(windows)]
    fn reveal_via_shopenfolder(path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| format!("路径没有可用的父目录：{}", path.display()))?;

        let parent_list = OwnedItemIdList::new(parent)?;
        let item_list = OwnedItemIdList::new(path)?;
        let pidls = vec![item_list.item];
        if let Err(error) = unsafe { SHOpenFolderAndSelectItems(parent_list.item, Some(&pidls), 0) }
        {
            // 与 Electron/plugin 一致：个别系统上文件明明存在却报 FILE_NOT_FOUND，
            // 此时退回 ShellExecute 打开父目录（不选中文件）至少保证用户到达目标目录。
            if error.code().0 == ERROR_FILE_NOT_FOUND.0 as i32 {
                let is_dir = parent.is_dir();
                let mut info = SHELLEXECUTEINFOW {
                    cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as _,
                    nShow: SW_SHOWNORMAL.0,
                    lpFile: PCWSTR(parent_list.hstring.as_ptr()),
                    lpClass: if is_dir { w!("folder") } else { PCWSTR::null() },
                    lpVerb: if is_dir {
                        w!("explore")
                    } else {
                        PCWSTR::null()
                    },
                    ..Default::default()
                };
                return unsafe { ShellExecuteExW(&mut info) }
                    .map_err(|error| shell_error("ShellExecuteExW", error.code().0));
            }
            // 这正是 plugin 静默吞掉的那类错误——必须把真实错误码暴露出来。
            return Err(shell_error("SHOpenFolderAndSelectItems", error.code().0));
        }
        Ok(())
    }

    /// 在文件管理器中打开目录；失败时返回带真实错误码的错误。
    #[cfg(windows)]
    pub fn open_dir(path: &Path) -> Result<(), String> {
        let path = dunce::simplified(path);
        // 首选：CreateProcessW 直接启动 explorer.exe "<path>"。
        // 在本机（Win11 24H2 嵌入版 Explorer）实测：ShellExecuteExW 的参数化
        // 启动（带引号目录 /root /e 各种形式）都会只拉起进程而不开窗，只有
        // /select 例外；而 CreateProcessW 的普通目录参数可靠开窗。程序名固定、
        // 参数为已校验的绝对路径、不经过任何 shell 解析，无注入面。
        // Windows 路径不可能含双引号（非法字符），遇到则退回 explore 动词。
        let quoted = path.to_string_lossy().into_owned();
        if !quoted.contains('"') && launch_explorer(&quoted).is_ok() {
            return Ok(());
        }
        open_dir_via_explore_verb(path)
    }

    /// CreateProcessW 启动 explorer.exe "<folder>"（打开目录的唯一可靠机制，见 open_dir 注释）。
    #[cfg(windows)]
    fn launch_explorer(folder: &str) -> Result<(), String> {
        // explorer 命令行参数统一用反斜杠（新嵌入版 Explorer 对正斜杠路径
        // 会静默回退到打开「文档」）；Windows 路径中 `/` 只会作为分隔符出现。
        let folder = folder.replace('/', "\\");
        let mut command_line: Vec<u16> = format!("\"explorer.exe\" \"{folder}\"")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let startup = STARTUPINFOW::default();
        let mut info = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                None,
                Some(PWSTR(command_line.as_mut_ptr())),
                None,
                None,
                false,
                PROCESS_CREATION_FLAGS(0),
                None,
                None,
                &startup,
                &mut info,
            )
            .map_err(|error| format!("启动 explorer.exe 失败：{error}"))?;
            // 进程已独立运行，立即关闭句柄避免泄漏。
            let _ = CloseHandle(info.hProcess);
            let _ = CloseHandle(info.hThread);
        }
        Ok(())
    }

    /// explore 动词（兜底路径；在部分构建上是静默 no-op，仅作最后手段）。
    #[cfg(windows)]
    fn open_dir_via_explore_verb(path: &Path) -> Result<(), String> {
        let file_string = HSTRING::from(path);
        let class_string = HSTRING::from("folder");
        let verb_string = HSTRING::from("explore");
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as _,
            nShow: SW_SHOWNORMAL.0,
            lpFile: PCWSTR(file_string.as_ptr()),
            lpClass: PCWSTR(class_string.as_ptr()),
            lpVerb: PCWSTR(verb_string.as_ptr()),
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }
            .map_err(|error| shell_error("ShellExecuteExW", error.code().0))
    }

    /// 在 Finder 中选中 `path`；必须校验 NSWorkspace 的真实返回值。
    #[cfg(target_os = "macos")]
    pub fn reveal_item(path: &Path) -> Result<(), String> {
        use objc2::msg_send;
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSArray, NSString, NSURL};

        let name = NSString::from_str(&path.to_string_lossy());
        let url = NSURL::fileURLWithPath(&name);
        let urls = NSArray::from_retained_slice(std::slice::from_ref(&url));
        let workspace = NSWorkspace::new();
        // 绑定层 activateFileViewerSelectingURLs 的签名是 ()（BOOL 被丢弃），
        // 必须用显式返回类型调原始 selector，否则 Finder 静默失败会被当成成功。
        let activated: bool =
            unsafe { msg_send![&*workspace, activateFileViewerSelectingURLs: &*urls] };
        if activated {
            Ok(())
        } else {
            Err(format!(
                "Finder 未能显示文件（activateFileViewerSelectingURLs 返回 false）：{}",
                path.display()
            ))
        }
    }
}

#[tauri::command]
pub fn desktop_reveal_path(app: AppHandle, path: String) -> Result<bool, String> {
    let requested = path;
    let target = match resolve_reveal_target(&requested) {
        Ok(target) => target,
        Err(error) => {
            log_local_reveal(&app, &format!("resolve-failed {requested}: {error}"));
            return Err(error);
        }
    };
    match target {
        RevealTarget::Path(path) if path.is_dir() => match open_local_dir(&app, &path) {
            Ok(()) => {
                log_local_reveal(&app, &format!("open-dir-ok {}", path.display()));
                Ok(true)
            }
            Err(error) => {
                log_local_reveal(
                    &app,
                    &format!("open-dir-error {} -> {error}", path.display()),
                );
                Err(error)
            }
        },
        RevealTarget::Path(path) => {
            // reveal 失败时退回打开父目录，仍能保证用户到达目标文件所在目录；
            // win/macos 的 reveal_local_file / open_local_dir 透传真实错误，
            // 日志可定位系统 shell 问题。
            let reveal_error = match reveal_local_file(&app, &path) {
                Ok(()) => {
                    log_local_reveal(&app, &format!("reveal-ok {}", path.display()));
                    return Ok(true);
                }
                Err(error) => error,
            };
            let parent = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            open_local_dir(&app, &parent)
                .map(|_| {
                    log_local_reveal(
                        &app,
                        &format!("reveal-fallback-ok {} -> {reveal_error}", path.display()),
                    );
                    true
                })
                .map_err(|error| {
                    log_local_reveal(
                        &app,
                        &format!(
                            "reveal-fallback-error {} -> reveal: {reveal_error}; open-dir: {error}",
                            path.display()
                        ),
                    );
                    error
                })
        }
        RevealTarget::Ancestor(ancestor) => open_local_dir(&app, &ancestor)
            .map(|_| {
                log_local_reveal(
                    &app,
                    &format!("ancestor-open-ok {requested} -> {}", ancestor.display()),
                );
                true
            })
            .map_err(|error| {
                log_local_reveal(&app, &format!("ancestor-open-error {requested} -> {error}"));
                format!("本地路径不可访问，且无法打开上级目录：{error}")
            }),
    }
}

#[tauri::command]
pub fn desktop_open_releases(app: AppHandle) -> bool {
    desktop_open_url(app, RELEASES_URL.into())
}

#[tauri::command]
pub fn desktop_open_update_log(app: AppHandle) -> bool {
    let Some(path) = update_log_path(&app) else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    app.opener().reveal_item_in_dir(path).is_ok()
}

#[cfg(windows)]
fn registry_dword(key: &str, name: &str) -> Option<u32> {
    let output = Command::new("reg.exe")
        .args(["query", key, "/v", name])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some(name))?;
    let value = line.split_whitespace().last()?;
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

fn notification_status(app: &AppHandle) -> NotificationStatus {
    let plugin_granted = matches!(
        app.notification().permission_state(),
        Ok(PermissionState::Granted) | Ok(PermissionState::Prompt)
    );
    if !plugin_granted {
        return NotificationStatus {
            supported: true,
            permission: "denied",
            reason: "app-disabled",
        };
    }

    #[cfg(windows)]
    {
        let global_key =
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
        let push_key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications";
        let app_key = format!("{global_key}\\{APP_USER_MODEL_ID}");
        if registry_dword(push_key, "ToastEnabled") == Some(0)
            || registry_dword(global_key, "NOC_GLOBAL_SETTING_TOASTS_ENABLED") == Some(0)
        {
            return NotificationStatus {
                supported: true,
                permission: "denied",
                reason: "system-disabled",
            };
        }
        if registry_dword(&app_key, "Enabled") == Some(0) {
            return NotificationStatus {
                supported: true,
                permission: "denied",
                reason: "app-disabled",
            };
        }
    }

    NotificationStatus {
        supported: true,
        permission: "granted",
        reason: "",
    }
}

#[tauri::command]
pub fn desktop_get_notification_status(app: AppHandle) -> NotificationStatus {
    notification_status(&app)
}

#[tauri::command]
pub fn desktop_open_notification_settings(app: AppHandle) -> bool {
    #[cfg(windows)]
    {
        app.opener()
            .open_url("ms-settings:notifications", None::<&str>)
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
}

#[tauri::command]
pub fn desktop_show_notification(app: AppHandle, input: NotificationInput) -> NotificationResult {
    let status = notification_status(&app);
    if status.permission != "granted" {
        return NotificationResult {
            shown: false,
            supported: status.supported,
            permission: status.permission,
            reason: status.reason,
        };
    }
    let title = clipped(&input.title, 120);
    let body = clipped(&input.body, 2_000);
    if title.is_empty() {
        return NotificationResult {
            shown: false,
            supported: true,
            permission: "granted",
            reason: "invalid-title",
        };
    }
    let shown = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .is_ok();
    NotificationResult {
        shown,
        supported: true,
        permission: "granted",
        reason: if shown { "" } else { "show-failed" },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_local_path, cleanup_open_assets_at, is_volume_root, nearest_existing_ancestor,
        resolve_reveal_target, RevealTarget,
    };
    use std::time::{Duration, SystemTime};

    #[test]
    fn local_path_bridge_accepts_only_existing_absolute_paths() {
        let current = std::env::current_dir().expect("读取当前目录");
        let canonical =
            canonical_local_path(current.to_string_lossy().as_ref()).expect("当前目录应可规范化");
        assert!(canonical.is_dir());
        assert!(canonical_local_path("relative/path.txt").is_err());
        assert!(canonical_local_path("bad\0path").is_err());
        assert!(canonical_local_path(
            current
                .join("pisper-missing-path")
                .to_string_lossy()
                .as_ref()
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn shell_reveal_describes_known_hresult_codes() {
        use super::shell_reveal::describe_hresult;
        // HRESULT 以 i32 传入（高位为 1，转 u32 后匹配）。
        let file_not_found = 0x8007_0002u32 as i32;
        let access_denied = 0x8007_0005u32 as i32;
        let e_fail = 0x8000_4005u32 as i32;
        let unknown = 0x8000_1234u32 as i32;
        assert_eq!(
            describe_hresult(file_not_found),
            "文件未找到（FILE_NOT_FOUND）"
        );
        assert!(describe_hresult(access_denied).contains("拒绝访问"));
        assert_eq!(describe_hresult(e_fail), "未指定的失败（E_FAIL）");
        assert_eq!(describe_hresult(unknown), "未知错误码");
    }

    #[cfg(windows)]
    #[test]
    fn shell_reveal_builds_select_params_and_rejects_quotes() {
        use super::shell_reveal::build_select_params;
        let path = std::path::Path::new(r"C:\Users\lkr\generated\visuals\a b.gif");
        assert_eq!(
            build_select_params(path).as_deref(),
            Some("/select,\"C:\\Users\\lkr\\generated\\visuals\\a b.gif\"")
        );
        // 双引号会破坏 /select 参数解析：拒绝而不是拼出危险参数。
        assert_eq!(
            build_select_params(std::path::Path::new("C:\\we\"ird.txt")),
            None
        );
        // 正斜杠路径规范化为反斜杠（嵌入版 Explorer 对正斜杠会静默回退到「文档」）。
        assert_eq!(
            build_select_params(std::path::Path::new("C:/Users/lkr/generated/visuals/a.gif"))
                .as_deref(),
            Some("/select,\"C:\\Users\\lkr\\generated\\visuals\\a.gif\"")
        );
    }

    #[test]
    fn nearest_existing_ancestor_walks_up_to_existing_directory() {
        let root = std::env::temp_dir().join(format!(
            "pisper-reveal-ancestor-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("generated").join("visuals")).expect("创建目录");
        let missing = root.join("generated").join("visuals").join("missing.gif");
        let ancestor = nearest_existing_ancestor(missing.to_string_lossy().as_ref())
            .expect("应找到缺失文件的父目录");
        assert_eq!(ancestor, root.join("generated").join("visuals"));
        let deep_missing = root.join("nowhere").join("deep").join("x.txt");
        let ancestor = nearest_existing_ancestor(deep_missing.to_string_lossy().as_ref())
            .expect("应回退到已存在的根目录");
        assert_eq!(ancestor, root);
        // 正斜杠形态（前端链接的常见形态）应同样兜底，且结果规范化为反斜杠——
        // 新嵌入版 Explorer 的命令行解析器不认正斜杠路径。
        let missing_fwd = missing.to_string_lossy().replace('\\', "/");
        let ancestor =
            nearest_existing_ancestor(missing_fwd.as_str()).expect("应找到缺失文件的父目录");
        assert_eq!(ancestor, root.join("generated").join("visuals"));
        #[cfg(windows)]
        assert!(!ancestor.to_string_lossy().contains('/'));
        // 超过 3 层的缺失路径不做兜底：回退到太远的祖先没有定位价值，应拒绝。
        let too_deep = root.join("a").join("b").join("c").join("d").join("e.txt");
        assert!(nearest_existing_ancestor(too_deep.to_string_lossy().as_ref()).is_none());
        // 卷根目录（如 C:\ 或 /）不是有效兜底：贴着卷根的缺失路径应拒绝，
        // 而不是把用户带到盘符根目录。
        let temp_root = std::env::temp_dir();
        let volume = temp_root
            .ancestors()
            .find(|p| is_volume_root(p))
            .expect("应能找到卷根目录");
        let under_volume = volume.join("definitely-missing-pisper-e2e").join("x.txt");
        assert!(nearest_existing_ancestor(under_volume.to_string_lossy().as_ref()).is_none());
        assert!(nearest_existing_ancestor("relative/path.txt").is_none());
        assert!(nearest_existing_ancestor("").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_reveal_target_reproduces_missing_generated_visual_link() {
        // 复现用户真实场景：目录里只有带时间戳前缀的生成产物，
        // 而模型在消息里链接了不带前缀的「干净名」——该文件不存在。
        let root =
            std::env::temp_dir().join(format!("pisper-reveal-target-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let visuals = root.join("generated").join("visuals");
        std::fs::create_dir_all(&visuals).expect("创建目录");
        let actual = visuals.join("2026-09-08T01-02-03-chibi_character_replacement_corrected.gif");
        std::fs::write(&actual, b"gif").expect("写入带时间戳前缀的产物");
        let linked = visuals.join("chibi_character_replacement_corrected.gif");
        assert!(!linked.exists());

        // 模型链接的缺失路径应兜底到产物所在目录，而不是整体失败。
        match resolve_reveal_target(linked.to_string_lossy().as_ref()).expect("应能解析") {
            RevealTarget::Ancestor(ancestor) => assert_eq!(ancestor, visuals),
            other => panic!("期望回退到祖先目录，实际得到 {other:?}"),
        }

        // 真实存在的产物路径应原样 reveal，不做改写。
        match resolve_reveal_target(actual.to_string_lossy().as_ref()).expect("应能解析") {
            RevealTarget::Path(path) => assert!(path == actual.canonicalize().expect("规范化")),
            other => panic!("期望 reveal 路径本身，实际得到 {other:?}"),
        }

        // 相对路径、控制字符依旧拒绝。
        assert!(resolve_reveal_target("relative/file.gif").is_err());
        assert!(resolve_reveal_target("bad\0path.gif").is_err());

        // 超过 3 层的虚构路径：没有值得打开的祖先目录，必须报错（前端弹错误 Toast）。
        let too_deep = visuals
            .join("a")
            .join("b")
            .join("c")
            .join("d")
            .join("e.gif");
        assert!(resolve_reveal_target(too_deep.to_string_lossy().as_ref()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn open_assets_cleanup_removes_expired_numeric_directories_only() {
        let root =
            std::env::temp_dir().join(format!("pisper-open-assets-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("123")).expect("创建测试目录");
        std::fs::write(root.join("123").join("asset.bin"), b"asset").expect("写入测试资产");
        std::fs::create_dir_all(root.join("keep-me")).expect("创建非资产目录");
        cleanup_open_assets_at(
            &root,
            SystemTime::now() + Duration::from_secs(2),
            Duration::from_secs(1),
            0,
        );
        assert!(!root.join("123").exists());
        assert!(root.join("keep-me").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_assets_cleanup_protects_recent_directories() {
        let root = std::env::temp_dir().join(format!(
            "pisper-open-assets-recent-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("456")).expect("创建测试目录");
        std::fs::write(root.join("456").join("asset.bin"), b"asset").expect("写入测试资产");
        cleanup_open_assets_at(&root, SystemTime::now(), Duration::from_secs(3600), 0);
        assert!(root.join("456").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
