const COMMANDS: &[&str] = &[
    "browse_start",
    "browse_stop",
    "advertise_start",
    "advertise_stop",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
