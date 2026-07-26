# Vesper desktop pets and Petdex compatibility

## Native Vesper feature

Desktop pets are owned and managed by Vesper. Users do not need to install or run the Petdex CLI, Petdex Desktop, Tauri/Zig binaries, hooks, or the localhost sidecar.

Vesper provides:

- a dedicated **Settings → Desktop pet** page
- native install and selection of Petdex-compatible pets
- Vesper-managed pet directories under Electron `userData/desktop-pets/` and the Web server data directory
- an independent transparent Electron `BrowserWindow` on desktop
- a draggable DOM overlay inside Vesper in Web browsers
- direct mapping from Vesper Agent runtime events to pet animation states
- tray controls and multi-display persistence on desktop, plus browser-local position persistence on Web

Installing a pet downloads its sprite content because the Petdex source/npm package does not contain the community pet assets. This is resource installation performed by Vesper itself, not an external application or companion process.

## Reused open-source contract

- Project: [Petdex](https://petdex.dev)
- Repository: `crafter-station/petdex`
- Source license: MIT
- npm package: `petdex`

The npm package exposes only a CLI binary (`bin.petdex = dist/petdex.js`). It has no JavaScript library entry point or `exports` map, so there is currently no importable Petdex SDK.

Vesper reuses the MIT-licensed desktop format and state contract:

- sprite names: `spritesheet.webp`, `spritesheet.png`, `sprite.webp`, `sprite.png`
- optional metadata: `pet.json`, using `displayName` or `name`
- maximum sprite size: 16 MiB
- frame size: 192×208, eight columns, at least nine state rows
- states: idle, movement, waving, jumping, failure, waiting, running/tool activity, review/thinking

For compatibility, Vesper can also read pets already present in `~/.petdex/pets/` and `~/.codex/pets/`, but those directories are optional. Pets installed from Vesper are stored and selected entirely through Vesper.

## Asset installation security

Vesper retrieves the public manifest from the exact `https://petdex.dev/api/manifest` endpoint and accepts sprite URLs only from `https://assets.petdex.dev`.

Before writing an asset, Vesper validates:

- HTTPS scheme and allowlisted host before and after redirects
- bounded manifest and sprite response sizes
- slug format
- PNG or WebP structure
- 16 MiB maximum size
- 192×208 frame grid compatibility
- local canonical path boundaries before rendering

The sandboxed Electron pet renderer receives a validated data URL through a narrow preload bridge. It has no Node.js access and cannot navigate. The Web renderer receives only validated, installed sprite bytes from Vesper's same-origin `/api/desktop-pet/sprite` endpoint.

## Rendering architecture

Electron creates a separate Vesper-owned `BrowserWindow`:

- 192×288, frameless and transparent
- always on top, skipped from the taskbar, and shown without stealing focus
- independently draggable and clickable
- position persisted across restarts and validated against the current display layout
- kept alive when the main Vesper window is hidden to the system tray
- destroyed only when the feature is disabled or Vesper truly exits

In a regular browser, Vesper renders the same sprite-state contract as a fixed DOM overlay inside the application page. It can be dragged and clicked, and its position is stored in browser local storage. Because it is page content rather than an operating-system window, closing the browser removes the Web pet.

## Agent activity mapping

| Vesper runtime event | Pet state |
| --- | --- |
| `meta`, `text_patch`, `retry`, `queue_update` | `waiting` |
| `thinking_patch`, `thinking_reset`, `compaction_start` | `review` |
| `tool_start`, `tool_update`, `tool_end` | `running` |
| `done` | `waving`, then idle/waiting |
| `error` | `failed`, then idle/waiting |
| pet click | `jumping`, then the current runtime state |

The runtime observer is best-effort and isolated: a desktop-pet error cannot interrupt an Agent response.

## Licensing note

Petdex application source is MIT licensed. Pet sprites are user-submitted content, and Petdex does not claim ownership of third-party underlying intellectual property. Vesper does not bundle community sprites into its source or release package; users explicitly choose which resource to install through Vesper's native settings UI.
