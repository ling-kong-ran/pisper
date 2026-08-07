#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

#[cfg(all(test, target_os = "windows"))]
#[link(name = "pisper_test_resource", kind = "static")]
extern "C" {}

fn main() {
    pisper_webview_lib::run();
}
