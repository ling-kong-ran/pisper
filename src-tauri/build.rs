fn package_version(path: &str, marker: &str) -> String {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    if marker == "json" {
        return serde_json::from_str::<serde_json::Value>(&source)
            .ok()
            .and_then(|value| value.get("version")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "0.0.0".to_string());
    }
    source
        .split("[package]")
        .nth(1)
        .and_then(|section| {
            section.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("version")?
                    .trim_start()
                    .strip_prefix('=')?
                    .trim()
                    .trim_matches('"')
                    .split_whitespace()
                    .next()
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| "0.0.0".to_string())
}

fn link_windows_resources_for_all_targets() {
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let (generated_name, shared_name) = if target_env == "msvc" {
        ("resource.lib", "pisper-resource.lib")
    } else {
        ("libresource.a", "libpisper-resource.a")
    };
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let generated = out_dir.join(generated_name);
    let shared = out_dir.join(shared_name);
    assert!(
        generated.is_file(),
        "tauri-build did not generate the Windows resource at {}",
        generated.display()
    );

    let _ = std::fs::remove_file(&shared);
    std::fs::rename(&generated, &shared).expect("failed to stage the shared Windows resource");
    // tauri-build links `generated` to binaries. Keep that argument valid while the shared
    // resource is linked once to binaries and test harnesses through the general argument.
    std::fs::write(&generated, b"!<arch>\n").expect("failed to create an empty COFF archive");
    println!("cargo:rustc-link-arg={}", shared.display());
}

fn main() {
    println!("cargo:rerun-if-changed=updater.pubkey");
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=../src-tui/Cargo.toml");
    println!("cargo:rerun-if-env-changed=PISPER_TAURI_UPDATER_PUBLIC_KEY");
    let updater_public_key = std::env::var("PISPER_TAURI_UPDATER_PUBLIC_KEY")
        .unwrap_or_else(|_| include_str!("updater.pubkey").trim().to_string());
    println!(
        "cargo:rustc-env=PISPER_TAURI_UPDATER_PUBLIC_KEY={}",
        updater_public_key.trim()
    );
    println!(
        "cargo:rustc-env=PISPER_BUNDLED_RUNTIME_VERSION={}",
        package_version("../package.json", "json")
    );
    println!(
        "cargo:rustc-env=PISPER_BUNDLED_TUI_VERSION={}",
        package_version("../src-tui/Cargo.toml", "toml")
    );
    tauri_build::build();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        link_windows_resources_for_all_targets();
    }
}
