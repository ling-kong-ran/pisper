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

`sidecar:sea` builds the web application, installs the production Node dependency closure directly into the external runtime, removes development-only content, injects the SEA blob, and prepares the native Tauri sidecar binaries. It does not invoke Electron or electron-builder. `sidecar:sea:smoke` verifies authentication, API access, first Pi Agent activation, and graceful shutdown.

### Dependency partition

The root `package.json` and `package-lock.json` are the single npm dependency source for both build and runtime packaging. Keep browser source libraries and build/test tooling in `devDependencies`: Vite compiles browser imports into `dist/` before desktop packaging, so those packages are not installed as Node modules in the shipped application. Keep packages loaded by `runtime/`, Node-executed `shared/` modules, Skills, MCP integration, or other sidecar runtime paths in `dependencies`; `sidecar:sea` copies the root manifests and runs `npm ci --omit=dev` in `sidecar-runtime/` to install that production closure. A package used by both browser and sidecar code therefore belongs in `dependencies`.

Do not create a second browser package manifest, npm workspace, or package-manager lockfile to represent this split. Source development, verification, and release SEA builds use Node.js 24 with a patch version that satisfies the locked dependencies. See [development-workflow.md](development-workflow.md) for host-specific requirements and the outstanding manifest compatibility mismatch; the legacy `engines: >=20` declaration does not establish Node 20 support for the current Runtime.

`desktop:webview:build` selects native bundles for its host platform:

```text
Windows x64          NSIS .exe
macOS x64/ARM64      .app and .dmg
Linux x64            .AppImage and .deb
```

### Platform packages and offline installation

Windows builds produce two NSIS installers from the same source and version:

- `Pisper_<version>_windows_x86_64-setup.exe`: the standard package uses
  `webviewInstallMode: embedBootstrapper`. It reuses an installed WebView2 runtime;
  when missing, the small Microsoft bootstrapper downloads and installs it online.
- `Pisper_<version>_windows_x86_64-offline-setup.exe`: the full offline package merges
  `tauri.windows-offline.conf.json` to embed Microsoft's complete Evergreen WebView2
  installer. Use it on disconnected machines without WebView2, or when its presence is unknown.
  Existing WebView2 installations are reused. This payload accounts for most of the extra size.

Both packages include the same Node SEA, production JavaScript closure and TUI. Local
speech models remain on-demand downloads. External MCP programs, extensions and model
services retain their own prerequisites; “offline package” does not mean all features
and models are available without preparation.

The packager stages the standard and offline outputs in separate directories before
Tauri can overwrite its NSIS output. Both receive updater signatures for release integrity,
but `latest.json` selects only the standard package: installed applications already have
WebView2. CI requires both packages and their signatures before publishing, extracts
both payloads, checks the expected bootstrapper/full-installer distinction, verifies
Microsoft Authenticode signatures, and records sizes, versions and SHA-256 digests.
WebView2 distribution is managed by Tauri's bundler under Microsoft's terms; no binary
copies are committed. See [Tauri's installer options](https://v2.tauri.app/distribute/windows-installer/#webview2-installation-options).

macOS DMGs use the system WKWebView; select Apple Silicon or Intel. Linux provides
AppImage and DEB, with distribution-specific system prerequisites (including FUSE where
needed for AppImage and WebKitGTK dependencies for DEB); neither is a universal offline
OS dependency bundle. Android uses the system WebView and ships a signed APK; iOS uses
WKWebView and the public IPA requires re-signing. These platforms do not gain a second
browser-engine package merely to mirror the Windows variants.

The homepage resolves each explicit platform/architecture/variant link against actual
Release assets, shares a single desktop manifest request, and falls back to the Release
page if metadata or an asset is unavailable. Existing versions before the split retain
their original files. In particular, versions 0.5.72/0.5.73 used the ordinary filename
for a package containing the full WebView2 installer; filenames are not renamed retroactively.

Windows acceptance must cover the standard package with WebView2 already installed and
network disconnected, the standard package without WebView2 with internet access, and
the offline package without WebView2 with network disconnected. Also verify upgrading
from either variant uses the standard package and preserves user data. CI payload checks
are not evidence of clean-machine installation acceptance.

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

Desktop, TUI, runtime/web, npm, and mobile App versions advance independently:

| Scope | Version source | Tag | Platform package |
| --- | --- | --- | --- |
| `desktop` | `src-tauri/desktop-package.json` | `vX.Y.Z` | Signed Tauri installer and updater |
| `tui` | `src-tui/Cargo.toml` | `tui-vX.Y.Z` | Self-contained distribution plus a thin TUI update component |
| `runtime` | root `package.json` | `runtime-vX.Y.Z` | Full SEA Runtime plus a Node-only npm Runtime closure |
| `app` | `src-tauri/mobile-package.json` | `app-vX.Y.Z` | Signed Android APK plus unsigned iOS IPA |

Run `npm run release -- <patch|minor|major|X.Y.Z>` to compare every channel with its own latest tag and publish all channels with substantive owned changes. Scopes are always detected from changed product paths; the command rejects manual `desktop`, `tui`, `runtime`, or `app` scopes so a release cannot omit another changed channel. All dispatches share one immutable source SHA and complete sequentially in the global release queue. Component workflows append only validated component/npm version commits, then the independent App workflow runs last and publishes its own `app-v*` tag and update manifest. The command waits for every workflow before returning and runs only the union of detected scopes' local checks once. TUI keeps its SEA sidecar/runtime in the standalone archive so a direct download remains usable, while its thin component archive contains only the TUI executable. Each Runtime job publishes both the self-contained SEA distribution and a separately signed Node-only closure used by npm. Runtime releases do not compile either Rust client. Desktop releases still integrate the current runtime and TUI sources into the application bundle; App releases retain mandatory Android, macOS/iOS, Simulator Runtime, signing, and asset gates.

Only desktop Releases are marked as GitHub `latest`; this preserves `/releases/latest` and `latest.json` for legacy client upgrades. TUI and runtime Releases are published with `--latest=false` and use their prefixed tags.

TUI and Runtime component archives are signed with the same minisign key used by the Tauri updater. The shared Rust updater requires an exact tag and platform asset name, downloads both the archive and `.sig`, enforces size limits, verifies the embedded public key, rejects links and traversal during extraction, then installs under the per-user `components/<component>/versions/<version>` directory. An atomic `current.json` pointer activates the version. Desktop startup and standalone TUI launchers prefer these installed SEA components and fall back to bundled binaries when no valid pointer is present. Runtime updates require an app/process restart; managed TUI launchers are refreshed immediately when possible.

The desktop update settings use one check action for the desktop package, TUI client, and Runtime, then expose install controls only for components with an available update. Standalone TUI distributions remain self-contained and do not expose component update commands.

The npm launcher receives Web, TUI, and the signed Node Runtime closure entirely through the configured npm registry. Its platform package contains no SEA executable. Installation verifies the minisign signatures locally and keeps the Node closure under `components/npm-runtime/` so it cannot replace Desktop or standalone TUI Runtime components. The launcher sets `PISPER_RUNTIME_NODE` to npm's current Node executable, so that host must satisfy the downloaded Runtime's dependency requirements as well as the launcher's requirements. The current Pi dependency requires Node >=22.19; use the Node 24 baseline documented in [development-workflow.md](development-workflow.md). The launcher's existing Node >=20 metadata still needs alignment and does not prove that the full Runtime works on Node 20. The launcher fails with the direct Runtime diagnostic instead of falling back to SEA. npm installations update as one unit through `pisper update [--check]`.

All scopes use four native jobs:

| Platform | GitHub runner | Desktop package | TUI/runtime package |
| --- | --- | --- | --- |
| `windows-x86_64` | `windows-latest` | NSIS | `.tar.gz` |
| `darwin-x86_64` | `macos-15-intel` | DMG | `.tar.gz` |
| `darwin-aarch64` | `macos-15` | DMG | `.tar.gz` |
| `linux-x86_64` | `ubuntu-22.04` | AppImage, DEB | `.tar.gz` |

Existing Electron installations cannot consume Tauri's `latest.json`. Users of those historical builds must install a Tauri package manually; Pisper's Agent data directory remains independent from the application installation directory.

## Rust desktop macOS local verification (2026-10-09)

The published v0.6.0 source only packages Windows. The local macOS adaptation in
`scripts/package-tauri-rust.mjs` additionally accepts an Apple Silicon host and
produces App/DMG bundles. This does not enable macOS in the release workflow or
claim Intel macOS support. The updater signature remains separate from local ad-hoc
code signing; an ad-hoc DMG is a verification artifact, not a notarized distribution.
Without `APPLE_SIGNING_IDENTITY`, local packaging explicitly seals the entire App
with the ad-hoc identity (`-`); a linker-only Mach-O signature is insufficient.
An explicit signing identity is preserved. `codesign --verify --deep --strict`
must pass before staging the installer.

The platform adapter stages the locked `sherpa-onnx-darwin-arm64@1.13.7` C API and
the same-version ONNX Runtime 1.28.1 dylib from Microsoft’s official macOS arm64
archive. Both versions were read through their native version APIs; sizes and SHA-256 digests are pinned in `stage-rust-speech.mjs`. ONNX Runtime
1.28.1 LICENSE and ThirdPartyNotices were checked against the official v1.28.1 tag
and are byte-identical to the already stored 1.27.1 notices. The resource manifest
records the actual platform, library versions, digests, origins and licenses.
The npm ONNX dylib required macOS 26.5; the official same-version archive declares
macOS 14.0. Both the archive and extracted library are digest-pinned. The macOS
bundle now declares 14.0 as its minimum version. Local execution is verified on
macOS 26.6.2; macOS 14/15 and Intel hardware remain untested. This does not certify
the older macOS baseline of previous releases.
Windows retains its pinned DLL closure. All platforms use the same speech model
catalog, model precision, model digests and BPE resource; no model policy is changed.

`startup_diagnostics.rs` checks platform-native filenames instead of requiring
Windows DLLs on macOS. Desktop commands, permissions and lifecycle remain owned by
the existing Tauri bridge. Local acceptance must launch the generated App with
`PISPER_DESKTOP_DATA_DIR`, `PISPER_AGENT_DIR` and `PISPER_WORKSPACE_DIR` pointing at
isolated fixture directories, then verify actual WKWebView rendering, native IPC
and streamed chat against a loopback-only model fixture. WKWebView uses a
non-persistent store only for explicit isolated tests, since its data-directory
option does not isolate cookies. The packaging entry also
runs resource and real PTY tests. On macOS those tests reuse the release profile.
Cargo tests use the canonical temporary directory so the system `/var` alias does
not trigger the production symlink checks; those checks remain enabled.
The macOS bundling subprocess alone uses the Tauri-supported CI layout mode to
avoid Finder AppleEvent timeouts; test processes do not inherit this override.
`npm run test:release` covers both Windows signature behavior and macOS packaging
command boundaries; command-boundary fixtures do not replace a real App launch.

Android/iOS remain blocked by a different migration: their build entries call
`build-mobile-runtime.mjs`, which still copies the removed Node `runtime/` tree.
Android release packaging was attempted and failed with ENOENT before producing
an APK. Reintroducing an old Node payload is not equivalent to verifying the Rust
backend. The mobile embedded-runtime lifecycle, native bridge and resource closure
must be migrated before claiming mobile support or issuing mobile release assets.

No version metadata or user data migration is part of this local packaging change.
Rollback comprises the packaging/platform-resource adapters and their tests; it
must not modify a user's agent data or replace their installed desktop application.

## Provider upgrade and mobile migration gates

The Rust provider adapter reads `pisper-provider-models.json` and
`pisper-provider-keyrings.json` without deleting or rewriting them on startup.
The discovered catalog applies only to the matching configured endpoint; manually
configured models survive catalog refreshes. Keys remain private, with only hashes
and masked hints in ordinary configuration responses. Explicit configuration
export/import preserves the keyring and catalog, and serialized mutations update
resident sessions. Opus 5.5 on Anthropic Messages uses adaptive thinking and effort;
turning reasoning off omits the unsupported disabled parameter.

`npm run test:startup` and Rust desktop packaging run
`scripts/smoke-rust-provider-upgrade.mjs` against the newly built binary. The
fixture covers an old catalog, multiple keys, masking, import/export, restart,
replacement and explicit key removal, using isolated data and synthetic credentials.

Android/iOS still require a Rust library with explicit start/stop ownership,
platform paths, permissions and app lifecycle integration. Their current bridge
starts Node and cannot run this Rust desktop executable on iOS. Until that migration
is implemented and tested on both platforms, all three mobile build entry points
fail before downloads, project generation or archive replacement with
`MOBILE_RUNTIME_NOT_MIGRATED`. This is a build diagnostic, **not a working mobile
package**. Existing release channel exclusion remains in place.

MCP upgrade uses the domain adapter in `runtime-rs/src/mcp_api/legacy.rs` before
Pi loads its extensions. If `agent_dir/mcp.json` does not exist, it reads the
Node v1 `data_dir/pisper-mcp.json` and publishes the complete new configuration
atomically. Original service IDs, display names, enabled states, timeouts,
credentials and per-tool restrictions are preserved. Existing `mcp.json` files,
including deliberately emptied configurations, take precedence; the old file is
never modified, so an older client can still read it. Cached tools and call
history are not reported as live connections. Invalid inputs and legacy SSE
transports fail explicitly without publishing a partial configuration.

The existing provider JSON writer now lives in `runtime-rs/src/config_storage.rs`.
Providers retain their write lock and document ownership; MCP owns its migration.
The shared module only handles private temporary files and atomic publication,
including creation without overwriting a concurrently created target. Tests cover
legacy conversion, restart, disabled tools, damaged input and concurrent writers.

Project skills remain owned by `<workspace>/.pisper/skills`, while global skills
live under `<agent_dir>/skills`. A separate development workspace does not inherit
the source repository's project skills. Local restoration may copy the skills to
the development workspace; it must not silently move or delete the original files.
