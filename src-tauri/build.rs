fn desktop_package() -> serde_json::Value {
    let source = std::fs::read_to_string("desktop-package.json").unwrap_or_default();
    serde_json::from_str(&source).unwrap_or_default()
}

fn manifest_version(manifest: &serde_json::Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(manifest, |value, key| value.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("0.0.0")
        .to_owned()
}

fn stage_windows_test_resource() {
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    // 测试资源名跟随工具链的链接约定：msvc 用 .lib，gnu 用 lib*.a
    // （src/desktop_shell/mod.rs 的 cfg(test) link 块按此查找）。
    let test_name = if target_env == "msvc" {
        "pisper_test_resource.lib"
    } else {
        "libpisper_test_resource.a"
    };
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    // embed-resource 的产物命名随版本变化：msvc 恒为 resource.lib；gnu 在 2.x 时代是
    // libresource.a（windres+ar），3.x 实测为 resource.lib（windres 单步 COFF）。
    // 按序探测两种命名，避免工具链/依赖升级悄悄打断 Windows 构建。
    let generated = ["resource.lib", "libresource.a"]
        .iter()
        .map(|name| out_dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| {
            panic!(
                "tauri-build did not generate the Windows resource in {}",
                out_dir.display()
            )
        });

    // tauri-build links `generated` to the application binary. Unit-test targets opt in to this
    // valid copy through their cfg(test) native link block, so the binary is never linked twice.
    // 绑定名保持 test_resource：release-workflow 源码守卫按此字面断言 copy 语义。
    let test_resource = out_dir.join(test_name);
    std::fs::copy(&generated, &test_resource).expect("failed to stage the Windows test resource");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
}

fn main() {
    println!("cargo:rerun-if-changed=updater.pubkey");
    println!("cargo:rerun-if-changed=desktop-package.json");
    // 移动命令权限变更必须重建 release ACL，避免 Cargo 复用旧 manifest。
    println!("cargo:rerun-if-changed=permissions/mobile.toml");
    println!("cargo:rerun-if-changed=capabilities/mobile-bridge.json");
    println!("cargo:rerun-if-env-changed=PISPER_TAURI_UPDATER_PUBLIC_KEY");
    let updater_public_key = std::env::var("PISPER_TAURI_UPDATER_PUBLIC_KEY")
        .unwrap_or_else(|_| include_str!("updater.pubkey").trim().to_string());
    println!(
        "cargo:rustc-env=PISPER_TAURI_UPDATER_PUBLIC_KEY={}",
        updater_public_key.trim()
    );
    let desktop_package = desktop_package();
    println!(
        "cargo:rustc-env=PISPER_BUNDLED_DESKTOP_VERSION={}",
        manifest_version(&desktop_package, &["version"])
    );
    println!(
        "cargo:rustc-env=PISPER_BUNDLED_RUNTIME_VERSION={}",
        manifest_version(&desktop_package, &["bundled", "runtime"])
    );
    println!(
        "cargo:rustc-env=PISPER_BUNDLED_TUI_VERSION={}",
        manifest_version(&desktop_package, &["bundled", "tui"])
    );
    tauri_build::build();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        // Iroh 的 iOS 网络探测通过这些系统 API 读取 DNS 和网卡信息，必须显式链接对应 framework。
        println!("cargo:rustc-link-lib=framework=SystemConfiguration");
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        stage_windows_test_resource();
    }
}
