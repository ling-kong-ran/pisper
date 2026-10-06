// Test-only access to the oracle captures in `tests/fixtures/core_oracle/`. Every
// file was produced by running the actual upstream TypeScript sources with
// node (type stripping); see the generator scripts next to the captures:
// - keybindings_{win32,linux,linux_wsl,darwin}.oracle.json  (oracle_keybindings.mjs)
// - event_bus.oracle.json                                    (oracle_event_bus.mjs)
// - cache_stats.oracle.json                                  (oracle_cache_stats.mjs)
// - messages.oracle.json                                     (oracle_messages.mjs)
// - model_config.oracle.json                                 (oracle_model_config.mjs)
// - models_store.oracle.json                                 (oracle_models_store.mjs)
//
// The `proper-lockfile` and `cross-spawn` packages are oracle-only stubs (the
// upstream npm deps are not installed offline); `typebox` 1.3.11 comes from a
// local install (upstream pins 1.3.27).

/// Upstream keybinding defaults, migration ordering, and manager behavior on
/// native Windows.
pub const KEYBINDINGS_WIN32: &str =
    include_str!("../../../tests/fixtures/core_oracle/keybindings_win32.oracle.json");
/// Same, on plain Linux.
pub const KEYBINDINGS_LINUX: &str =
    include_str!("../../../tests/fixtures/core_oracle/keybindings_linux.oracle.json");
/// Same, on Linux under WSL (`WSL_DISTRO_NAME` set).
pub const KEYBINDINGS_LINUX_WSL: &str =
    include_str!("../../../tests/fixtures/core_oracle/keybindings_linux_wsl.oracle.json");
/// Same, on macOS.
pub const KEYBINDINGS_DARWIN: &str =
    include_str!("../../../tests/fixtures/core_oracle/keybindings_darwin.oracle.json");

/// Upstream event-bus dispatch trace.
pub const EVENT_BUS: &str =
    include_str!("../../../tests/fixtures/core_oracle/event_bus.oracle.json");

/// Upstream cache-stats results over the scenario battery.
pub const CACHE_STATS: &str =
    include_str!("../../../tests/fixtures/core_oracle/cache_stats.oracle.json");

/// Upstream message constructors, `bashExecutionToText`, and `convertToLlm`.
pub const MESSAGES: &str = include_str!("../../../tests/fixtures/core_oracle/messages.oracle.json");

/// Upstream `ModelConfig.load` results over the fixture battery.
pub const MODEL_CONFIG: &str =
    include_str!("../../../tests/fixtures/core_oracle/model_config.oracle.json");

/// Upstream `FileModelsStore` / `InMemoryCodingAgentModelsStore` snapshots,
/// including exact file bytes.
pub const MODELS_STORE: &str =
    include_str!("../../../tests/fixtures/core_oracle/models_store.oracle.json");
