# Node SEA + System WebView Desktop Target

Pisper ships a Tauri 2 desktop application that uses each operating system's WebView plus a supervised Node SEA sidecar. New releases no longer build or publish Electron applications.

## Layout

```text
Pisper / Pisper.exe        Tauri shell and system WebView lifecycle
pisper-sidecar             Node SEA runtime and bootstrap
sidecar-runtime/           Pisper runtime, Pi packages, Skills, native modules, and vendor binaries
```

The SEA contains Node and a small bootstrap. The application runtime remains external by design because Pi loads Skills and extensions dynamically, MCP starts external processes, and clipboard and local-process integrations require native files at stable filesystem paths. This is not an absolute single-file deployment.

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

### Dependency partition

The root `package.json` and `package-lock.json` are the single npm dependency source for both build and runtime packaging. Keep browser source libraries and build/test tooling in `devDependencies`: Vite compiles browser imports into `dist/` before desktop packaging, so those packages are not installed as Node modules in the shipped application. Keep packages loaded by `runtime/`, Node-executed `shared/` modules, Skills, MCP integration, or other sidecar runtime paths in `dependencies`; `sidecar:sea` copies the root manifests and runs `npm ci --omit=dev` in `sidecar-runtime/` to install that production closure. A package used by both browser and sidecar code therefore belongs in `dependencies`.

Do not create a second browser package manifest, npm workspace, or package-manager lockfile to represent this split. The repository supports Node.js 20 and newer for development and web/runtime execution, while release SEA builds continue to target Node.js 24.

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

The release job requires `windows-x86_64`, `darwin-x86_64`, `darwin-aarch64`, and `linux-x86_64` before it generates a single `latest.json`. If updater signing secrets are absent, CI publishes installable bundles but deliberately omits `latest.json`.

`latest.json` and the signed full installers remain a compatibility upgrade path for already released clients that use the former Tauri updater. The update UI invokes that path only when an old installed Shell exposes its updater bridge. Current Shells do not register the plugin or its download commands; their in-app update path downloads only signed Desktop, TUI, and Runtime components. This keeps old installations upgradeable without retaining a second full-installer implementation in new clients.

Updater signatures and platform code signatures are separate. Windows Authenticode still requires a platform certificate. macOS distribution requires `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, and notarization credentials; without them the DMG can be built but Gatekeeper will not treat it as a verified distribution. Linux AppImage/DEB packages do not require a platform certificate, while compatibility updater artifacts still use the Tauri private key.

## Component Releases

Desktop, TUI, and runtime/web versions advance independently:

| Scope | Version source | Tag | Platform package |
| --- | --- | --- | --- |
| `desktop` | `src-tauri/desktop-package.json` | `vX.Y.Z` | Signed Tauri installer and updater |
| `tui` | `src-tui/Cargo.toml` | `tui-vX.Y.Z` | Self-contained distribution plus a thin TUI update component |
| `runtime` | root `package.json` | `runtime-vX.Y.Z` | Full SEA Runtime plus a Node-only npm Runtime closure |

Run `npm run release -- <patch|minor|major|X.Y.Z>` to compare each component with its own latest tag and publish every component with substantive owned changes. Component scopes are always detected from changed product paths; the command rejects manual `desktop`, `tui`, or `runtime` scopes so a release cannot omit another changed component. Multi-component dispatches share one immutable source SHA and complete sequentially, appending only validated component version commits. The workflow runs only the detected scopes' checks and packaging. TUI keeps its SEA sidecar/runtime in the standalone archive so a direct download remains usable, while its thin component archive contains only the TUI executable. Each Runtime job publishes both the self-contained SEA distribution and a separately signed Node-only closure used by npm. Runtime releases do not compile either Rust client. Desktop releases still integrate the current runtime and TUI sources into the application bundle.

Only desktop Releases are marked as GitHub `latest`; this preserves `/releases/latest` and `latest.json` for legacy client upgrades. TUI and runtime Releases are published with `--latest=false` and use their prefixed tags.

TUI and Runtime component archives are signed with the same minisign key used by the Tauri updater. The shared Rust updater requires an exact tag and platform asset name, downloads both the archive and `.sig`, enforces size limits, verifies the embedded public key, rejects links and traversal during extraction, then installs under the per-user `components/<component>/versions/<version>` directory. An atomic `current.json` pointer activates the version. Desktop startup and standalone TUI launchers prefer these installed SEA components and fall back to bundled binaries when no valid pointer is present. Runtime updates require an app/process restart; managed TUI launchers are refreshed immediately when possible.

The desktop update settings use one check action for the desktop package, TUI client, and Runtime, then expose install controls only for components with an available update. Standalone TUI distributions remain self-contained and do not expose component update commands.

The npm launcher requires Node.js 20 or newer and receives Web, TUI, and the signed Node Runtime closure entirely through the configured npm registry. Its platform package contains no SEA executable. Installation verifies the minisign signatures locally and keeps the Node closure under `components/npm-runtime/` so it cannot replace Desktop or standalone TUI Runtime components. The launcher sets `PISPER_RUNTIME_NODE` to npm's current Node executable and fails with the direct Runtime diagnostic instead of falling back to SEA. npm installations update as one unit through `pisper update [--check]`.

All scopes use four native jobs:

| Platform | GitHub runner | Desktop package | TUI/runtime package |
| --- | --- | --- | --- |
| `windows-x86_64` | `windows-latest` | NSIS | `.tar.gz` |
| `darwin-x86_64` | `macos-15-intel` | DMG | `.tar.gz` |
| `darwin-aarch64` | `macos-15` | DMG | `.tar.gz` |
| `linux-x86_64` | `ubuntu-22.04` | AppImage, DEB | `.tar.gz` |

Existing Electron installations cannot consume Tauri's `latest.json`. Users of those historical builds must install a Tauri package manually; Pisper's Agent data directory remains independent from the application installation directory.
