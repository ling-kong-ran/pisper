# pi-rs

**A Rust clone of the original [pi](https://github.com/earendil-works/pi) coding agent.**

This project is a faithful re-implementation ("clone") of [pi](https://github.com/earendil-works/pi) — the [Pi Agent Harness](https://github.com/earendil-works/pi) by earendil-works — written from scratch in Rust. It is not affiliated with or endorsed by the upstream project. All design credit for the ported behavior belongs to the original pi authors; the crate is MIT-licensed, same as upstream.

The crate publishes as **`pi-rs`** (this repository is named `pi-rust`); the main binary is **`pirs`**.

## Compatibility with upstream

pi-rs is behaviorally drop-in compatible with upstream pi: session files, `auth.json`, themes/keybindings/skills, JSON output, the RPC protocol, and the extension API all interoperate with upstream. That claim is enforced, not aspirational — the verification harness executes the verbatim upstream TypeScript sources (SHA-pinned) and compares their observable behavior byte-for-byte against the Rust implementation, via committed oracle fixtures. See [Verification](#verification).

Current alignment: **upstream v1.0.2**. Not a line-by-line translation (impossible across languages), but the same architecture and the same external behavior.

## Install

From crates.io:

    cargo install pi-rs

Or build from source (requires a stable Rust toolchain):

    cargo build --release
    # artifacts: target/release/pirs (main CLI), pi-rust, generate-models

Prebuilt binaries for Windows / Linux / macOS (x64 + arm64 where applicable) are attached to each [GitHub release](https://github.com/xiaotubaobiu/pi-rust/releases).

## Usage

### Configure credentials (pick one)

**OAuth login** (anthropic / codex / copilot / openrouter / xai / kimi / radius and more):

    pirs login --provider anthropic

Credentials are stored in `auth.json` (upstream-pi format; default path: `%APPDATA%\pi-rust\auth.json` on Windows, `~/.config/pi-rust/auth.json` on Linux, `~/Library/Application Support/pi-rust/auth.json` on macOS).

**Environment variable**:

    set ANTHROPIC_API_KEY=sk-ant-...        # Windows
    export ANTHROPIC_API_KEY=sk-ant-...     # Linux/macOS

**Explicit flag** (highest priority, never persisted):

    pirs --provider anthropic --api-key sk-ant-...

### Start an interactive session

    pirs --provider anthropic --model claude-sonnet-4-5
    pirs --provider openai-compat --base-url https://open.bigmodel.cn/api/paas/v4 --model glm-4.6
    pirs --provider openai-compat --base-url <url> --model <id> --api-key <key>

Supported `--provider`: anthropic, openai-compat, openai-responses, azure-openai-responses, openai-codex, google, google-vertex, mistral, amazon-bedrock, pi-messages.

In-session commands: `/model`, `/settings`, `/compact`, `/export`, `/share`, `/tree`, `/session`, `/bug` and more. Extensions, skills, hooks, themes and custom keybindings all work.

### Capabilities current with upstream v1.0.2

- **MCP client**: stdio / streamable-HTTP transports, OAuth (dynamic client registration + loopback callback), `mcp.json` config, `/mcp` management, tool/resources/prompts exposure policies (direct/deferred/codemode/hidden).
- **codemode**: an embedded QuickJS (quickjs-ng) sandbox where the model composes tool calls in JavaScript (`searchTools`, namespaces, deferred loading).
- **tool-search**: BM25 tool retrieval with deferred loading.
- **system theme**: live OKHSL palette generation from the terminal's reported background/palette (contrast-rule engine), light/dark terminals, grayscale first frame.
- **pi-durable**: the durable-execution library (JSONL/memory/SQLite storage, forks/transactions/observations, scheduler + harness, provider identities).
- **Per-level sampling params** merged across model defaults / thinking level / request.
- **System One / llama.cpp classifiers**, the ChatGPT/Meta OAuth flows, exponential-backoff retries.

## Verification

- **~5,400+ tests**, all passing on both Windows and Linux (run serially, matching the verified gate protocol).
- **Oracle fixtures**: per-package directories under `tests/fixtures/` pin the exact behavior of upstream TypeScript (executed verbatim under Node with a resolve-hook loader) and the Rust side is asserted to match, including JSON shapes, ordering, and error text.
- The fixtures are excluded from the crates.io package (they are ~69 MB of pinned upstream sources) but ship in this repository; `cargo test` from a repo clone runs the full oracle suite.

## Platform support

- **Windows**: primary development platform, fully verified.
- **Linux**: full gate suite verified (CI + WSL).
- **macOS**: platform branches are ported (Unix sockets, pbcopy clipboard); CI does not cover macOS yet.

## Known limitations

- Clipboard: Windows text-read and win/mac image-read rely on upstream's prebuilt native N-API module and are missing; the full Linux path (termux/wl/x11 subprocesses) and OSC 52 fallback are implemented.
- experimental subsystem runtime bindings (Windows named pipes, node:vm host, esbuild bundling) are left to host assembly per the seam convention.

## License

MIT, same as upstream [pi](https://github.com/earendil-works/pi). The original pi project's copyright and design belong to the earendil-works authors; this repository is an independent Rust clone maintained for the Rust ecosystem.
