# Node SEA + System WebView Desktop Target

Pisper keeps the Electron target as the default desktop release and provides an experimental Tauri 2 target that uses the operating system WebView plus a supervised Node SEA sidecar.

## Layout

```text
Pisper.exe                 Tauri shell and system WebView lifecycle
pisper-sidecar.exe         Node SEA runtime and bootstrap
sidecar-runtime/           Pisper server, Pi packages, Skills, native modules, and vendor binaries
```

The SEA contains Node and a small bootstrap. The application runtime remains external by design because Pi loads Skills and extensions dynamically, MCP starts external processes, and Sandbox/clipboard integrations require native files at stable filesystem paths. This is not an absolute single-file deployment.

The shell starts the sidecar on a random `127.0.0.1` port and waits for a structured readiness message before opening the WebView. A random bootstrap token is exchanged for an `HttpOnly`, `SameSite=Strict` session cookie. Authenticated write requests also reject foreign browser origins. Closing the shell requests graceful shutdown over stdin and force-terminates the sidecar only after a five-second timeout; the sidecar independently watches its parent PID.

## Build

Prerequisites:

- Node.js 24 (SEA is still an active-development Node feature)
- Rust stable and Cargo
- Tauri 2 platform prerequisites
- Windows WebView2, macOS WKWebView, or Linux WebKitGTK

Commands:

```bash
npm run sidecar:sea
npm run sidecar:sea:smoke
npm run desktop:webview:dev
npm run desktop:webview:build
```

`sidecar:sea` generates desktop icons, builds the web application, uses the existing Electron package filters to stage the production dependency closure, injects the SEA blob, and prepares target-triple sidecar names for Tauri. `sidecar:sea:smoke` verifies authentication, API access, first Pi Agent activation, and graceful shutdown.

Windows output:

```text
src-tauri/target/release/Pisper.exe
src-tauri/target/release/bundle/nsis/Pisper_<version>_x64-setup.exe
```

## Release Notes

`postject` modifies the Node executable. Code signing must therefore happen after SEA injection and before or during final Tauri bundling. The current local prototype is unsigned.

Electron-specific updater and native bridge behavior remains on the Electron target until equivalent Tauri commands and update signing are implemented. Core Pisper workflows use the existing same-origin web API and run in either shell.
