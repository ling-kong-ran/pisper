# Node SEA + System WebView Desktop Target

Pisper ships a Tauri 2 desktop application that uses each operating system's WebView plus a supervised Node SEA sidecar. New releases no longer build or publish Electron applications.

## Layout

```text
Pisper / Pisper.exe        Tauri shell and system WebView lifecycle
pisper-sidecar             Node SEA runtime and bootstrap
sidecar-runtime/           Pisper server, Pi packages, Skills, native modules, and vendor binaries
```

The SEA contains Node and a small bootstrap. The application runtime remains external by design because Pi loads Skills and extensions dynamically, MCP starts external processes, and Sandbox/clipboard integrations require native files at stable filesystem paths. This is not an absolute single-file deployment.

The shell starts the sidecar on a random `127.0.0.1` port and waits for a structured readiness message before opening the WebView. A random bootstrap token is exchanged for an `HttpOnly`, `SameSite=Strict` session cookie. Authenticated write requests also reject foreign browser origins. The localhost WebView capability exposes only Pisper's custom desktop commands; it does not grant browser content general shell, filesystem, opener, notification, or updater plugin permissions.

Closing the main window hides it to the system tray. Quitting requests graceful shutdown over stdin and force-terminates the sidecar only after a five-second timeout; the sidecar independently watches its parent PID. The shell also provides single-instance activation, external-link interception, native notifications, window-state restoration, signed updates, and an independent transparent desktop-pet window.

## Build

Prerequisites:

- Node.js 24
- Rust stable and Cargo
- Tauri 2 platform prerequisites
- Windows WebView2, macOS WKWebView, or Linux WebKitGTK 4.1

Commands:

```bash
npm run sidecar:sea
npm run sidecar:sea:smoke
npm run desktop:webview:dev
npm run desktop:webview:smoke -- http://127.0.0.1:9223
npm run desktop:webview:build
```

`sidecar:sea` generates icons and the web application, installs the production Node dependency closure directly into the external runtime, removes development-only content, injects the SEA blob, and prepares the native Tauri sidecar name. It does not invoke Electron or electron-builder. `sidecar:sea:smoke` verifies authentication, API access, first Pi Agent activation, and graceful shutdown.

`desktop:webview:build` selects native bundles for its host platform:

```text
Windows x64          NSIS .exe
macOS x64/ARM64      .app and .dmg
Linux x64            .AppImage and .deb
```

Release assets are normalized so macOS architectures cannot collide:

```text
Pisper_<version>_windows_x86_64-setup.exe
Pisper_<version>_darwin_x86_64.dmg
Pisper_<version>_darwin_aarch64.dmg
Pisper_<version>_linux_x86_64.AppImage
Pisper_<version>_linux_x86_64.deb
```

## Signed Updates

The application compiles the updater verification key from `src-tauri/updater.pubkey`. Keep the corresponding private key and password offline and backed up. Losing either prevents every installed build from accepting future update packages.

The packaging step reads `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` first, then falls back to the ignored `release/tauri-updater.key` and `release/tauri-updater-key.password` files. Signed native jobs produce:

- Windows: NSIS `.exe` and `.exe.sig`
- macOS: `.app.tar.gz` and `.app.tar.gz.sig`
- Linux: `.AppImage` and `.AppImage.sig`

The release job requires `windows-x86_64`, `darwin-x86_64`, `darwin-aarch64`, and `linux-x86_64` before it generates a single `latest.json`. If updater signing secrets are absent, CI publishes installable bundles but deliberately omits `latest.json`; in-app installation remains disabled.

Pisper checks the human-readable GitHub Release first, then enables in-app download only when signed Tauri metadata announces the exact same newer version. The Rust shell owns download progress, signature verification, installation, update logs, and sidecar shutdown. Web content never receives the private key, updater plugin permission, or downloaded installer bytes.

Updater signatures and platform code signatures are separate. Windows Authenticode still requires a platform certificate. macOS distribution requires `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and notarization credentials; without them the DMG can be built but Gatekeeper will not treat it as a verified distribution. Linux AppImage/DEB packages do not require a platform certificate, while their in-app updater artifact still uses the Tauri private key.

## Release Matrix

The tag-triggered workflow builds four native jobs:

| Updater platform | GitHub runner | Public packages |
| --- | --- | --- |
| `windows-x86_64` | `windows-latest` | NSIS |
| `darwin-x86_64` | `macos-15-intel` | DMG |
| `darwin-aarch64` | `macos-15` | DMG |
| `linux-x86_64` | `ubuntu-22.04` | AppImage, DEB |

Existing Electron installations cannot consume Tauri's `latest.json`. Users of those historical builds must install a Tauri package manually; Pisper's Agent data directory remains independent from the application installation directory.
