# Native general visual generation handoff

Reference: `pisper-release-parity-reference`, commit
`582160235671903d9f1c7034b457557b1df74b68`. Source contracts, the 23 previously
executed pure Node cases, and the nine registered driver descriptions are in
`oracles/contract.json`. Ten immutable service source copies and their SHA-256
values are in `oracles/dependency-sha.json`; the complete release tool schema is
`oracles/tool-schema.json`. These JavaScript files are test oracles, not production
dependencies. Production execution is entirely Rust.

## Frozen public ports

`VisualGenerationService::new(VisualConfigPort) -> Arc<VisualGenerationService>`.
The read callback returns `Result<VisualConfigSnapshot>` in a boxed async future.
The snapshot contains raw models/auth/app JSON, runtime provider display names,
runtime models, and an ICU locale (`zh-CN` default matching the local Node oracle).
It is private host data; do not serialize credentials or expose the snapshot in
API results. All catalog responses project the public model fields only.

The preference callback accepts `(VisualKind, Option<String>)`, returns
`Result<()>`, and must serialize an atomic canonical `pisper.json` mutation. A
nonempty value is already resolved to a canonical `provider/id`; `None` removes
only that kind's entry. Preserve unrelated app configuration and current other
kind preference. Invalid selection is rejected before this callback is invoked.

Service methods:

```rust
get_model_status(&self, kind: VisualKind) -> Result<Value>
get_all_status(&self) -> Result<Value>
set_preferred_model(&self, kind: VisualKind, requested: &Value) -> Result<Value>
generate(self: &Arc<Self>, request: VisualRequest, options: VisualOptions)
    -> Result<VisualResult>
test_visual(self: &Arc<Self>, data_dir: &Path, options: VisualOptions)
    -> Result<Value>
dispose(&self)
```

`get_all_status` and preference results have exactly `image`, `video`,
`imageModels`, `videoModels`, `imageSelection`, `videoSelection`. Empty selected
models are null; empty selections are empty strings. Raw null models/app/auth
documents propagate the corresponding release property-access error; raw scalar
and array documents are not silently rewritten or forbidden wholesale.

`VisualRequest { cwd: PathBuf, input: Value }` uses the host's actual authorized
cwd. `VisualOptions` has cancellation, optional progress callback, and
`allow_fallback` (default true). Already-paid workflow/game/image-assets callers
must explicitly set false: this prevents SDK retry, Images-to-Responses protocol
fallback, and catalog model fallback. Existing successful files must never be
regenerated because of asset indexing errors. xAI/New API video endpoint
negotiation retains the release's own 404/405 route rules.

`VisualResult` serializes camelCase fields `path`, `kind`, `mimeType`, `size`,
`provider`, `providerName`, `model`, `modelName`, `remoteId`, `operation`,
`fallbackUsed`, `attemptedModels`. Absent remote ID is explicitly null, matching
release index.mjs. Attempted models count catalog attempts, not SDK retries;
Responses protocol fallback alone does not set `fallbackUsed`.

`create_tool(service, VisualContextPort, VisualGeneratedFilePort)` returns the
real Pi `Arc<ToolDefinition>` named `generate_visual`; `manifest()` returns the
release manifest including high risk. The context callback receives the actual
native SessionManager ID and ctx cwd, then returns the public/private owner ID
and authorized cwd. Input `cwd` is ignored. The archive callback receives that
context plus the complete result and runs after successful file creation.
Archive failure is swallowed, exactly as in the release tool. Progress is
forwarded through actual `on_update`; already-aborted SDK signals make no HTTP
request. Tool content, prompt snippet, three guidelines, schema, and English
completion text match the release; generation and polling progress is Chinese.

Root owns native module declaration, AppState lifetime, provider/runtime snapshot
composition, canonical preference writer, actual session resolver/archive,
ordinary extension factory and optional gateway loadout, authorization, thin API
router, frontend capability activation, and shutdown calling `dispose()`.

## Implemented behavior

- All nine registry names dispatch real OpenAI/OpenRouter/Google/xAI/New API
  image or video requests, including family dispatch according to request kind.
  Responses image fallback is implemented separately with exact supported
  statuses and generation-only/Responses-API eligibility.
- Credentials, model headers and configured/runtime model precedence, explicit
  kinds/capabilities, enabled providers, ranking/dedup, locale sorting, explicit
  qualified selection before dedup, and stored preferences follow the catalog.
- Source/mask files use extension and stat checks, 24 MiB per file, filtering and
  maximum eight sources. Absolute paths outside cwd, relative paths and followed
  symlinks are allowed. No raster decoding, aggregate budget, output size budget,
  8 MiB workflow limit, or 4096-dimension restriction is added.
- Source multipart filenames, masks, quality/output format, Google 4K imageSize,
  video duration/resolution/aspect normalization and request body shapes follow
  the individual driver contracts.
- SDK custom headers use nullable deletion and array append semantics; fetch
  headers retain JS object spread and case-insensitive duplicate combination.
  SDK X-Stainless headers are filtered. Actual Node SDK oracle proves JSON null
  Content-Type becomes application/json while a nonnull configured multipart
  Content-Type remains configured on this runtime.
- SDK HTTP retry has maxRetries one, or zero for allow_fallback false; retry
  status/header/delay and 180-second image/600-second video header timeout match
  the reference SDK. Fetch drivers have no added overall timeout. Video polling
  waits five seconds, propagates status/progress/failures and downloads actual
  bytes. SDK video content failures preserve API error status for catalog
  fallback; ordinary fetch download errors remain untyped as in release.
- xAI-origin New API detection uses an unauthenticated root GET, 3500 ms timeout,
  per-origin cached true/false result, and original signature patterns.
- Outputs recursively create `generated/visuals`, use UTC seconds and Unicode
  safe names, and preserve release same-second overwrite behavior. They are
  returned only after write success. No UUID naming or output validation budget
  is imposed. Progress/caller cancellation, dropped task and service disposal
  settle owned jobs; closing the service prevents new provider requests.
- Configuration test uses the fixed release image prompt and `config-test` name
  under `data_dir/visual-test`; preview is included only up to 6 MiB.

## Actual verification and reproducibility

Windows scoped Rust tests: **40 passed, 0 failed**, default parallel execution.
`scoped-test.ps1` compiles `harness.rs` with the frozen existing dependency rlibs
from `target-web-search-parity/debug/deps`; it does not run Cargo or rebuild the
application. All compiler outputs and logs are in
`runtime-rs/target-native-visual-scope`, outside src. Expected harness-only unused
public-export warnings are recorded in the compiler log.

```powershell
& 'runtime-rs/src/native_visual/scoped-test.ps1'
& 'C:/Users/13063/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node.exe' 'runtime-rs/src/native_visual/oracles/http-header-oracle.mjs'
```

`http-header-oracle.mjs` executes the read-only release drivers and catalog with
OpenAI SDK 6.49.0, synthetic credentials, loopback HTTP and temporary isolated
configuration files. **13 passed, 0 failed**; result JSON is saved in
`oracles/http-header-results.json`. No paid API or personal profile was used.

Rust evidence includes actual HTTP requests and persisted bytes for all nine
driver registrations; all four video creation/poll/download families; Images
and Responses request boundaries; automatic/explicit/paid model fallback;
cross-provider 401 versus same-provider termination; safety termination; SDK
retry boundaries; New API detection/legacy route/duplicate-duration behavior;
source/mask multipart and Google 4K fields; typed SDK video download failure;
header distinctions and secret-array redaction; fixed API test prompt and
preview; source/output/canonical preference boundaries; real Pi extension ctx,
update/result callbacks, archive failure, and already-aborted signals; caller
cancel/drop and service shutdown. Unix symlink-specific test is cfg(unix) and was
not executed on Windows.

The media bytes are controlled HTTP fixture payloads. No external model
generation or video playback claim is made. No Cargo-wide product test, packaged
binary, frontend/DOM or complete AgentSession/model/gateway roundtrip is covered
by this isolated harness. Root must complete those host integration checks with
the production factory and isolated config/session data before enabling the
capability. Production dependency requirements are already present in Cargo:
reqwest multipart+gzip+brotli, flate2, ICU collator/locale, base64, chrono,
futures, getrandom, regex, serde/serde_json, tokio/tokio-util and Pi SDK; axum and
uuid are used by tests. No new shared library is requested by this module.

The host uses typed cancellation and rejects new work after disposal. Error
messages redact credentials/custom header secrets after fallback classification;
this is stricter than release raw exception exposure. Rust API cancellation
errors use the unified aborted message rather than exposing separate SDK versus
DOM exception class identities. JavaScript UTF-16 filename slicing becomes a
valid Rust/OS string at a split surrogate; filesystem replacement matches Node,
but an invalid lone surrogate's JSON spelling is not preserved in the result.
