fn main() {
    println!("cargo:rerun-if-changed=updater.pubkey");
    println!("cargo:rerun-if-env-changed=PISPER_TAURI_UPDATER_PUBLIC_KEY");
    let updater_public_key = std::env::var("PISPER_TAURI_UPDATER_PUBLIC_KEY")
        .unwrap_or_else(|_| include_str!("updater.pubkey").trim().to_string());
    println!(
        "cargo:rustc-env=PISPER_TAURI_UPDATER_PUBLIC_KEY={}",
        updater_public_key.trim()
    );
    tauri_build::build()
}
