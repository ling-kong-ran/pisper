# Pisper desktop pets and Petdex compatibility

## Native Pisper feature

Desktop pets are owned and managed by Pisper. Users do not need to install or run the Petdex CLI, Petdex Desktop, separate native binaries, hooks, or an additional companion process.

Pisper provides:

- a dedicated **Settings → Desktop pet** page
- native install and selection of Petdex-compatible pets
- Pisper-managed pet directories under the Agent data directory
- an independent transparent Tauri WebviewWindow on desktop
- a draggable DOM overlay inside Pisper in Web browsers
- direct mapping from Pisper Agent runtime events to pet animation states
- tray controls, 20%–100% opacity, and multi-display persistence on desktop, plus browser-local position persistence on Web

Installing a pet downloads its sprite content because the Petdex source/npm package does not contain the community pet assets. This is resource installation performed by Pisper itself, not an external application or companion process.

## Reused open-source contract

- Project: [Petdex](https://petdex.dev)
- Repository: `crafter-station/petdex`
- Source license: MIT
- npm package: `petdex`

The npm package exposes only a CLI binary (`bin.petdex = dist/petdex.js`). It has no JavaScript library entry point or `exports` map, so there is currently no importable Petdex SDK.

Pisper reuses the MIT-licensed desktop format and state contract:

- sprite names: `spritesheet.webp`, `spritesheet.png`, `sprite.webp`, `sprite.png`
- optional metadata: `pet.json`, using `displayName` or `name`
- maximum sprite size: 16 MiB
- frame size: 192×208, eight columns, at least nine state rows
- states: idle, movement, waving, jumping, failure, waiting, running/tool activity, review/thinking

For compatibility, Pisper can also read pets already present in `~/.petdex/pets/` and `~/.codex/pets/`, but those directories are optional. Pets installed from Pisper are stored and selected entirely through Pisper.

## Asset installation security

Pisper retrieves the public manifest from the exact `https://petdex.dev/api/manifest` endpoint and accepts sprite URLs only from `https://assets.petdex.dev`.

Before writing an asset, Pisper validates:

- HTTPS scheme and allowlisted host before and after redirects
- bounded manifest and sprite response sizes
- slug format
- PNG or WebP structure
- 16 MiB maximum size
- 192×208 frame grid compatibility
- local canonical path boundaries before rendering

The Tauri pet window loads a dedicated same-origin page through the authenticated localhost sidecar. It receives only validated, installed sprite bytes from Pisper's `/api/desktop-pet/sprite` endpoint, has no Node.js access, and can invoke only the explicitly allowlisted Pisper desktop commands.

## Rendering architecture

Tauri creates a separate Pisper-owned transparent WebviewWindow:

- 192×288, frameless and transparent
- always on top, skipped from the taskbar, and shown without stealing focus
- independently draggable and clickable
- lower-right default placement, with dragged positions persisted across restarts and validated against the current display layout
- adjustable 20%–100% window opacity
- kept alive when the main Pisper window is hidden to the system tray
- destroyed only when the feature is disabled or Pisper truly exits

In a regular browser, Pisper renders the same sprite-state contract as a fixed DOM overlay inside the application page. It can be dragged and clicked, and its position is stored in browser local storage. Because it is page content rather than an operating-system window, closing the browser removes the Web pet.

## Agent activity mapping

| Pisper runtime event | Pet state |
| --- | --- |
| `meta`, `text_patch`, `retry`, `queue_update` | `waiting` |
| `thinking_patch`, `thinking_reset`, `compaction_start` | `review` |
| `tool_start`, `tool_update`, `tool_end` | `running` |
| `done` | `waving`, then idle/waiting |
| `error` | `failed`, then idle/waiting |
| pet click | `jumping`, then the current runtime state |

The runtime observer is best-effort and isolated: a desktop-pet error cannot interrupt an Agent response.

## Licensing note

Petdex application source is MIT licensed. Pet sprites are user-submitted content, and Petdex does not claim ownership of third-party underlying intellectual property. Pisper does not bundle community sprites into its source or release package; users explicitly choose which resource to install through Pisper's native settings UI.
