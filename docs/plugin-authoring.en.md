# Pisper Plugin Authoring Guide (Specification & Format)

This document is for **plugin developers**. It describes the directory structure, manifest specification, entry-file format, tool JSON Schema, runtime limits, and security boundary of Pisper local plugins.

For end-user installation and operation, see the local plugin guide (currently available in Chinese at [`local-plugins.md`](./local-plugins.md)).

## Concepts: plugins vs. tools

Pisper keeps two layers strictly separate:

- **Plugin**: the unit of install, enable/disable, and uninstall. A plugin is a directory containing a manifest and an entry file, and may expose one or more tools.
- **Tool**: the Agent invocation protocol. The Agent discovers tools via `discover_tools` and calls them via `call_tool`. Tool names must be globally unique.

A plugin declares its tools in the manifest; at runtime each tool is exposed as a distinct Agent tool. Tools are never executed directly by the frontend — the callable set is returned authoritatively by the Runtime.

## Directory structure

A plugin is an ordinary directory with this minimal layout:

```
my-plugin/
├── pisper-plugin.json   # required: the plugin manifest
└── index.mjs            # required: the entry file named by manifest `entry`
```

The directory may also contain other relative files (JS, JSON, text, …) that the entry code imports. The whole directory is copied to the global plugin installation directory; after installation the source directory is no longer referenced.

## Manifest `pisper-plugin.json`

The manifest is a JSON file at the plugin directory root declaring the plugin identity and the tools it provides.

```json
{
  "schemaVersion": 1,
  "id": "example.project-package-info",
  "name": "Project Package Info",
  "version": "1.0.0",
  "description": "Read package metadata from the current chat workspace.",
  "entry": "index.mjs",
  "permissions": ["workspace-read"],
  "tools": [
    {
      "name": "project_package_info",
      "label": "Project package info",
      "description": "Read the current project's package.json and return its name, version, description, package manager, and npm script names.",
      "scope": "The package.json file at the root of the current chat workspace",
      "parameters": {
        "type": "object",
        "properties": {},
        "additionalProperties": false
      }
    }
  ]
}
```

### Top-level fields

| Field | Required | Type | Constraint |
| --- | --- | --- | --- |
| `schemaVersion` | no | number | only `1` is supported; defaults to `1` |
| `id` | yes | string | 1-96 chars; lowercase letters, digits, dot `.`, and hyphen `-` only; must start and end with a letter or digit |
| `name` | yes | string | 1-100 chars |
| `version` | yes | string | semantic version such as `1.0.0`; optional prerelease suffix, e.g. `1.0.0-beta.1` |
| `description` | no | string | up to 1000 chars (truncated beyond) |
| `entry` | yes | string | plugin-relative path; `.js` / `.mjs` / `.cjs` only |
| `permissions` | no | string[] | up to 32 deduplicated entries; **declarative** permission descriptions shown for human review |
| `tools` | yes | object[] | 1-32 tools, see below |

`id` and `version` together determine the install location `<pluginRoot>/<id>/<version>/`, so the same `id` with different `version` values can coexist (overwrite-install of an already-installed `id` is not supported in this version).

`permissions` is review metadata, not an access-control mechanism; plugin code still runs as the current OS user — see [Security boundary](#security-boundary).

### Tool object `tools[]`

| Field | Required | Type | Constraint |
| --- | --- | --- | --- |
| `name` | yes | string | 1-64 chars; starts with a lowercase letter; lowercase letters, digits, and underscores only |
| `label` | no | string | up to 100 chars; defaults to `name` |
| `description` | yes | string | up to 1000 chars; state what the tool does and when to call it |
| `scope` | no | string | up to 500 chars; what files/services it can affect; defaults to `description` |
| `parameters` | no | object | JSON Schema describing the arguments; `type` must be `object` |

Constraints:

- A tool `name` must not collide with built-in tools (such as `plugin_create`, `skill_create`, `web_search`, …) or with tools of any other installed plugin.
- Tool names within one plugin must be unique.
- `parameters` must be valid JSON Schema; `type` must be `object`, `properties` (if present) must be an object, and `required` (if present) must be an array of strings. The Runtime uses it to validate Agent arguments.

## Entry file

The entry file exports an `execute` function. The Runtime starts an isolated Worker that loads the entry for each tool call:

```js
export async function execute({ toolName, arguments: input, context }) {
  return {
    content: [{ type: 'text', text: `called ${toolName}` }],
    details: {},
  }
}
```

### Export resolution

The Runtime resolves the entry in this order:

1. `module.execute`
2. `module.default?.execute`
3. `module.default`

So a default-exported function, a default-exported `{ execute }` object, or a named `execute` export all work.

### Arguments object

`execute` receives a single object:

- `toolName`: the tool name of this call (dispatch on this when a plugin provides multiple tools).
- `arguments`: the JSON-Schema-validated argument object.
- `context`: execution context:
  - `cwd`: the current session working directory (**always the session directory, never the plugin directory**).
  - `sessionId`: the current session ID.
  - `dataDir`: the plugin's persistent data directory, where the plugin may read and write its own state.

### Return value

The return value can be:

1. **A string**: the Runtime wraps it as a single text `content`.
2. **A Pi tool result object**:

```js
{
  content: [
    { type: 'text', text: '...' },
  ],
  details: { /* optional structured side data */ },
}
```

The serialized result must not exceed 1 MB.

## Complete minimal example

```js
// index.mjs
export async function execute({ toolName, context }) {
  if (toolName !== 'example_echo') {
    throw new Error(`Unsupported tool: ${toolName}`)
  }
  return {
    content: [{ type: 'text', text: `workspace: ${context.cwd}` }],
    details: { sessionId: context.sessionId },
  }
}
```

The matching manifest is the full example at the top of this document. A read-only example plugin is also available at [`examples/local-plugins/project-package-info`](../examples/local-plugins/project-package-info).

## Limits

| Item | Limit |
| --- | --- |
| Manifest size | 256 KB max |
| Directory file count | 512 files max |
| Directory total size | 20 MB max |
| Symbolic links | not allowed in the directory, manifest, or entry |
| Tools | 1-32 per plugin |
| Execution timeout | 120 s; the Worker is terminated on timeout |
| Result size | 1 MB max after serialization |
| Worker memory | 128 MB old-generation cap |
| Extra files (`plugin_create`) | 64 max |

Path safety: `entry` and extra file paths must be plugin-relative; the Runtime rejects absolute paths and `..` traversal, and verifies the resolved entry stays inside the directory.

## Installation & validation

Before installation the Runtime runs a **static preflight that executes no code** (`inspect`):

1. Parse and validate the manifest and each tool's JSON Schema.
2. Verify `entry` points to a regular file inside the directory.
3. Check tool-name conflicts with built-in and installed plugins.
4. Scan the directory, compute the content SHA-256 digest, and check file count, size, and symlinks.

Preflight results are valid for 10 minutes. At install time the Runtime recomputes the digest and rejects the install if the directory changed after preflight; it also re-compares the digest after copying before committing. So "pass preflight → edit files → install directly" is blocked.

Install location: `<dataDir>/plugins/<id>/<version>/` (where `<dataDir>` is `~/.pisper/agent`, overridable via `PISPER_AGENT_DIR`).

## Creating plugins with the Agent

In full-access execution mode, the Agent can use the built-in `plugin_create` tool to generate a plugin from structured parameters. It runs the exact same manifest, Schema, path, size, and tool-name conflict checks as the Plugins page. Generated sources are written to the global `<dataDir>/plugin-sources/<plugin-id>` and are not tied to a project.

When using `plugin_create`, developers pass `id`, `name`, `tools`, `entryCode`, and optional `files` — equivalent to writing the manifest and entry by hand and then going through the same preflight-install chain. On failure, only files whose content still matches this generation are cleaned up; existing sources and installed plugins are never overwritten.

## Security boundary

- Plugin code runs in an isolated Worker, but it is **not an OS sandbox**. It can access files, network, and Node.js modules available to the current OS user.
- Third-party plugins are always marked **high risk** and are only exposed to the Agent in the session's full-access execution mode.
- A plugin cannot be uninstalled while it is executing; cancelling the Agent call or a timeout terminates the Worker.
- Only install trusted, reviewed code, and never hardcode credentials in a plugin.

## Not supported in this version

This is the local-plugin MVP, so the following are not supported: plugin marketplace, npm/Git download, auto-update, native modules, lifecycle scripts, Provider injection, TUI UI, and overriding built-in tools.
