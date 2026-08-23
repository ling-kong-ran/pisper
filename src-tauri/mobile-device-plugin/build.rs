const COMMANDS: &[&str] = &[
    "permission_states",
    "request_permission",
    "open_app_settings",
    "execute",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .try_build()
        .expect("failed to build the Pisper mobile device plugin");
}
