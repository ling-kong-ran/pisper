mod ios_build;

const COMMANDS: &[&str] = &[
    "permission_states",
    "request_permission",
    "open_app_settings",
    "execute",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .try_build()
        .expect("failed to build the Pisper mobile device plugin");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        // 保留插件权限生成，但避免上游 swift-rs 用宿主 macOS 依赖图构建 iOS。
        ios_build::build().expect("failed to build the Pisper iOS device plugin");
    }
}
