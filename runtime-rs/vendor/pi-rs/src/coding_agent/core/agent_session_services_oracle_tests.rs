//! Differential consumers of the 47-case, actual-source services oracle.
//!
//! Register this file as a child of agent_session_services.rs. This file does
//! not install a TS host or replace services/queue logic with a second port.
//!
//! Coverage boundaries (intentionally narrower than "47 cases passed"):
//! - All 13 flags cases call the real async services factory. Input/runtime
//!   Map order, values and diagnostic order are checked without sorting. The two
//!   duplicate-declaration cases use the supported extensions_override seam:
//!   the oracle supplies already-loaded extensions, while normal discovery
//!   rejects cross-extension flag conflicts before services sees them.
//! - All 10 providers cases call real loader.reload and production queue
//!   register/unregister/flush helpers. Attempts, snapshots, group clears,
//!   live append, retained iterators and native Arc identity are checked.
//!   Callback failure injection is an explicit synchronous memory collaborator
//!   (as in the TS oracle); it is NOT a test of services' catch/normalization.
//!   Config Values are owned clones: JS config object identity is not claimed.
//!   Array identity is tested by observable iteration, not synthetic # labels.
//! - Additional projections below state their own boundaries. No expected
//!   fixture is used to construct an actual result; only inputs drive actors.
//!
//! Oracle SHA256: c1ad28f32e7011b9e433998d326e70f9c03ad5e001051637b386b3c16d2177dc
//! Executed upstream services SHA256:
//! 831c56842c394969567199a9e5ccdaebb9a3df7bdde825aa677935b825afedd6
//! Provenance/resolve-hook details live alongside the unchanged JSON fixture.

use super::*;
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::models::{faux_provider, FauxProviderOptions};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::InlineExtension;
use crate::coding_agent::core::settings_manager::parse_settings_value;
use crate::coding_agent::extensions::loader::{ExtensionFactory, ExtensionRuntime};
use crate::coding_agent::extensions::types::{
    NativeProvider, PendingNativeProviderRegistration, PendingProviderRegistration,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

fn oracle() -> &'static Value {
    static ORACLE: OnceLock<Value> = OnceLock::new();
    ORACLE.get_or_init(|| {
        let value: Value = serde_json::from_str(include_str!("agent_session_services_oracle.json"))
            .expect("services actual-source oracle must parse");
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["cases"].as_array().unwrap().len(), 47);
        value
    })
}

fn case(id: &str) -> &'static Value {
    oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing oracle case {id}"))
}

fn list(value: &Value) -> &[Value] {
    match value {
        Value::Null => &[],
        Value::Array(items) => items,
        other => panic!("expected optional fixture array, got {other}"),
    }
}

fn text(value: &Value) -> &str {
    value.as_str().expect("fixture string")
}

fn flag(value: &Value) -> FlagValue {
    match value {
        Value::Bool(value) => FlagValue::Bool(*value),
        Value::String(value) => FlagValue::Str(value.clone()),
        other => panic!("unsupported fixture flag {other}"),
    }
}

fn flag_json(value: FlagValue) -> Value {
    match value {
        FlagValue::Bool(value) => json!(value),
        FlagValue::Str(value) => json!(value),
    }
}

fn flag_map(inputs: &Value) -> Option<OrderedMap<FlagValue>> {
    inputs["options"].get("extensionFlagValues").map(|values| {
        let mut map = OrderedMap::new();
        for pair in list(values) {
            map.set(text(&pair[0]), flag(&pair[1]));
        }
        map
    })
}

// Compare Map entries directly: sorting would hide a runtime/reload regression.
fn actual_flags(runtime: &ExtensionRuntime) -> Vec<(String, Value)> {
    runtime
        .flag_values()
        .into_iter()
        .map(|(name, value)| (name, flag_json(value)))
        .collect()
}

fn expected_flags(pairs: &Value) -> Vec<(String, Value)> {
    list(pairs)
        .iter()
        .map(|pair| (text(&pair[0]).to_string(), pair[1].clone()))
        .collect()
}

fn seed_flags(runtime: &ExtensionRuntime, inputs: &Value) {
    for pair in list(&inputs["initialFlagValues"]) {
        runtime.set_flag_value(text(&pair[0]), flag(&pair[1]));
    }
}

fn flag_factories(inputs: &Value) -> Vec<ExtensionFactory> {
    list(&inputs["extensions"])
        .iter()
        .map(|extension| {
            let declarations = list(&extension["flags"]).to_vec();
            Arc::new(
                move |api: &crate::coding_agent::extensions::loader::ExtensionApi| {
                    for declaration in &declarations {
                        let kind = match text(&declaration[1]) {
                            "boolean" => FlagType::Boolean,
                            "string" => FlagType::String,
                            other => panic!("unknown fixture flag type {other}"),
                        };
                        api.register_flag(text(&declaration[0]), None, kind, None)?;
                    }
                    Ok(())
                },
            ) as ExtensionFactory
        })
        .collect()
}

fn loader_options(
    cwd: &str,
    agent_dir: &str,
    settings: &SettingsManager,
    factories: Vec<ExtensionFactory>,
) -> DefaultResourceLoaderOptions {
    DefaultResourceLoaderOptions {
        cwd: cwd.into(),
        agent_dir: agent_dir.into(),
        settings_manager: Some(Arc::new(settings.clone())),
        extension_factories: factories
            .into_iter()
            .enumerate()
            .map(|(index, factory)| InlineExtension::Named {
                factory,
                name: format!("oracle-{index}"),
                hidden: false,
            })
            .collect(),
        no_skills: true,
        no_prompt_templates: true,
        no_themes: true,
        no_context_files: true,
        ..Default::default()
    }
}

fn memory_settings() -> SettingsManager {
    SettingsManager::in_memory(parse_settings_value("{}").unwrap())
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(12), future)
        .await
        .expect("oracle consumer timed out")
}

struct Sandbox {
    _dir: tempfile::TempDir,
    cwd: String,
    agent_dir: String,
    settings: SettingsManager,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        Self {
            _dir: dir,
            cwd: cwd.to_str().unwrap().into(),
            agent_dir: agent_dir.to_str().unwrap().into(),
            settings: memory_settings(),
        }
    }

    async fn options(&self, inputs: &Value) -> CreateAgentSessionServicesOptions {
        let runtime = bounded(ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::default())),
            models_path: Some(None),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
            allow_model_network: false,
            refresh_on_create: Some(false),
            ..Default::default()
        }))
        .await
        .unwrap();
        let factories = flag_factories(inputs);
        let mut names = HashSet::new();
        let duplicate_declarations = list(&inputs["extensions"])
            .iter()
            .flat_map(|extension| list(&extension["flags"]))
            .any(|declaration| !names.insert(text(&declaration[0])));
        let mut loader = loader_options(
            &self.cwd,
            &self.agent_dir,
            &self.settings,
            if duplicate_declarations {
                Vec::new()
            } else {
                factories.clone()
            },
        );
        let initial = inputs.clone();
        let cwd = self.cwd.clone();
        loader.extensions_override = Some(Arc::new(move |mut extensions| {
            if duplicate_declarations {
                // Match the oracle's post-discovery boundary, not a weakened
                // conflict filter. Native factory construction remains real.
                let bus = crate::coding_agent::core::event_bus::EventBusController::new();
                for (index, factory) in factories.iter().enumerate() {
                    let extension =
                        crate::coding_agent::extensions::loader::load_extension_from_factory(
                            factory.clone(),
                            &cwd,
                            bus.bus().clone(),
                            &extensions.runtime,
                            Some(&format!("<override:oracle-{index}>")),
                        )
                        .expect("post-discovery fixture factory");
                    extensions.extensions.push(extension);
                }
            }
            seed_flags(&extensions.runtime, &initial);
            extensions
        }));
        CreateAgentSessionServicesOptions {
            cwd: self.cwd.clone(),
            agent_dir: Some(self.agent_dir.clone()),
            settings_manager: Some(self.settings.clone()),
            model_runtime: Some(runtime),
            extension_flag_values: flag_map(inputs),
            resource_loader_options: Some(loader),
            ..Default::default()
        }
    }
}

async fn consume_flags(id: &str) {
    let case = case(id);
    assert_eq!(case["category"], "flags", "{id}");
    let sandbox = Sandbox::new();
    let options = sandbox.options(&case["inputs"]).await;
    // This case has no initial entries and only valid strings, so the
    // production input OrderedMap's order can also be checked independently.
    if id == "flags-input-map-last-value-first-position" {
        let actual: Vec<_> = options
            .extension_flag_values
            .as_ref()
            .unwrap()
            .iter()
            .map(|(name, value)| json!([name, flag_json(value.clone())]))
            .collect();
        assert_eq!(
            json!(actual),
            case["expected"]["flagValues"],
            "{id}: input Map replacement order"
        );
    }
    let result = bounded(create_agent_session_services(options)).await;
    assert!(
        result.is_ok(),
        "{id}: real services rejected: {:?}",
        result.as_ref().err()
    );
    let services = result.unwrap();
    assert_eq!(case["expected"]["outcome"]["kind"], "returned", "{id}");
    let extensions = services.resource_loader.lock().unwrap().get_extensions();
    assert!(
        extensions.errors.is_empty(),
        "{id}: inline factory failed: {:?}",
        extensions.errors
    );
    assert_eq!(
        actual_flags(&extensions.runtime),
        expected_flags(&case["expected"]["flagValues"]),
        "{id}: flags"
    );
    assert_eq!(
        serde_json::to_value(&services.diagnostics).unwrap(),
        case["expected"]["services"]["diagnostics"],
        "{id}: ordered diagnostics"
    );
    assert!(
        extensions
            .runtime
            .pending_provider_registrations()
            .is_empty(),
        "{id}"
    );
    assert!(
        extensions
            .runtime
            .pending_native_provider_registrations()
            .is_empty(),
        "{id}"
    );
}

macro_rules! flag_cases {
    ($($name:ident => $id:literal),+ $(,)?) => {
        $(#[tokio::test] async fn $name() { consume_flags($id).await; })+
    };
}

flag_cases! {
    flags_absent => "flags-absent",
    flags_empty_map => "flags-empty-map",
    flags_boolean_false => "flags-boolean-false-becomes-true",
    flags_boolean_string => "flags-boolean-string-becomes-true",
    flags_string_empty_and_text => "flags-string-empty-and-text",
    flags_string_bool_errors => "flags-string-bool-errors-retain-old-values",
    flags_duplicate_later_string => "flags-duplicate-later-string-wins",
    flags_duplicate_later_boolean => "flags-duplicate-later-boolean-wins",
    flags_unknown_singular => "flags-unknown-singular",
    flags_unknown_plural_order => "flags-unknown-plural-input-order",
    flags_known_errors_before_unknown => "flags-known-errors-before-unknown-summary",
    flags_input_map_replacement_order => "flags-input-map-last-value-first-position",
    flags_updates_existing_values => "flags-updates-preserve-map-slot",
}

#[derive(Clone)]
struct QueueEntry {
    queue: String,
    spec: Value,
    native: Option<NativeProvider>,
}

impl QueueEntry {
    fn key(&self) -> &str {
        text(&self.spec["key"])
    }
}

struct QueueProbe {
    runtime: ExtensionRuntime,
    inputs: Value,
    entries: Vec<QueueEntry>,
    attempts: Vec<Value>,
    executed: Vec<String>,
    // These are collaborator outcomes, not services diagnostics.
    collaborator_errors: Vec<(String, Value)>,
}

impl QueueProbe {
    fn new(runtime: ExtensionRuntime, inputs: Value) -> Self {
        let mut entries = Vec::new();
        let mut add = |queue: &str, spec: &Value| {
            let native = (queue == "native").then(|| {
                faux_provider(FauxProviderOptions {
                    provider: Some(text(&spec["id"]).into()),
                    ..Default::default()
                })
                .provider
            });
            entries.push(QueueEntry {
                queue: queue.into(),
                spec: spec.clone(),
                native,
            });
        };
        for queue in ["ordinary", "native"] {
            for spec in list(&inputs["queues"][queue]) {
                add(queue, spec);
            }
        }
        if let Some(actions) = inputs["actions"].as_object() {
            for actions in actions.values() {
                for action in list(actions) {
                    if action["op"] == "append" {
                        add(text(&action["queue"]), &action["registration"]);
                    }
                }
            }
        }
        Self {
            runtime,
            inputs,
            entries,
            attempts: Vec::new(),
            executed: Vec::new(),
            collaborator_errors: Vec::new(),
        }
    }

    fn entry(&self, queue: &str, key: &str) -> &QueueEntry {
        self.entries
            .iter()
            .find(|entry| entry.queue == queue && entry.key() == key)
            .unwrap_or_else(|| panic!("unknown fixture registration {queue}:{key}"))
    }

    fn append(&self, queue: &str, spec: &Value) {
        let entry = self.entry(queue, text(&spec["key"]));
        match queue {
            "ordinary" => self
                .runtime
                .register_provider(
                    text(&entry.spec["name"]),
                    &entry.spec["config"],
                    text(&entry.spec["extensionPath"]),
                )
                .unwrap(),
            "native" => self
                .runtime
                .register_native_provider(
                    entry.native.as_ref().unwrap(),
                    text(&entry.spec["extensionPath"]),
                )
                .unwrap(),
            other => panic!("unsupported fixture queue {other}"),
        }
    }

    fn seed(&self) {
        for queue in ["ordinary", "native"] {
            for spec in list(&self.inputs["queues"][queue]) {
                self.append(queue, spec);
            }
        }
        seed_flags(&self.runtime, &self.inputs);
    }

    fn ordinary_entry(&self, registration: &PendingProviderRegistration) -> &QueueEntry {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.queue == "ordinary" && entry.spec["name"] == registration.name)
            .expect("unexpected ordinary registration");
        assert_eq!(
            registration.config, entry.spec["config"],
            "ordinary config must be forwarded, not reconstructed"
        );
        assert_eq!(
            registration.extension_path,
            text(&entry.spec["extensionPath"])
        );
        entry
    }

    fn native_entry(&self, registration: &PendingNativeProviderRegistration) -> &QueueEntry {
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                entry
                    .native
                    .as_ref()
                    .is_some_and(|original| Arc::ptr_eq(original, &registration.provider))
            })
            .expect("native registration replaced the original Arc identity");
        assert_eq!(registration.provider.id(), text(&entry.spec["id"]));
        assert_eq!(
            registration.extension_path,
            text(&entry.spec["extensionPath"])
        );
        entry
    }

    fn queues(&self) -> Value {
        let ordinary: Vec<_> = self
            .runtime
            .pending_provider_registrations()
            .iter()
            .map(|registration| self.ordinary_entry(registration).key().to_string())
            .collect();
        let native: Vec<_> = self
            .runtime
            .pending_native_provider_registrations()
            .iter()
            .map(|registration| self.native_entry(registration).key().to_string())
            .collect();
        json!({"ordinary":ordinary,"native":native})
    }

    fn record(&mut self, queue: &str, key: &str) {
        self.attempts
            .push(json!({"queue":queue,"key":key,"queues":self.queues(),
            "flags":actual_flags(&self.runtime)}));
    }

    // Input-driven collaborator operations only. There is deliberately no
    // home-grown for-of/clear loop here: flush_pending_* owns all iteration.
    fn actions(&mut self, trigger: &str) -> Result<(), Value> {
        let actions = list(&self.inputs["actions"][trigger]).to_vec();
        for (index, action) in actions.iter().enumerate() {
            let label = format!("{trigger}:{index}");
            if self.executed.contains(&label) {
                continue;
            }
            self.executed.push(label);
            match text(&action["op"]) {
                "append" => self.append(text(&action["queue"]), &action["registration"]),
                "filter" => {
                    for key in list(&action["drop"]) {
                        let entry = self.entry(text(&action["queue"]), text(key));
                        let provider_name = if entry.queue == "native" {
                            text(&entry.spec["id"])
                        } else {
                            text(&entry.spec["name"])
                        };
                        // Production unregister replaces both queue properties;
                        // fixture IDs are disjoint, so the untargeted contents
                        // are unchanged. Array generation labels aren't claimed.
                        self.runtime.unregister_provider(provider_name).unwrap();
                    }
                }
                "throw" => return Err(action["error"].clone()),
                other => panic!("unsupported provider collaborator action {other}"),
            }
        }
        Ok(())
    }

    fn callback(&mut self, queue: &str, key: &str) {
        self.record(queue, key);
        let trigger = format!("{queue}:{key}");
        // registerProvider/registerNativeProvider are synchronous upstream.
        // A typed Err models only that input collaborator failure, not an
        // async rejection and not the production services catch boundary.
        if let Err(error) = self.actions(&trigger) {
            self.collaborator_errors.push((trigger, error));
        }
    }
}

fn queue_projection(queues: &Value) -> Value {
    json!({"ordinary":queues["ordinary"]["entries"],"native":queues["native"]["entries"]})
}

fn consume_queues(id: &str) {
    let case = case(id);
    assert_eq!(case["category"], "providers", "{id}");
    let sandbox = Sandbox::new();
    let slot: Arc<Mutex<Option<QueueProbe>>> = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    let inputs = case["inputs"].clone();
    let mut options = loader_options(
        &sandbox.cwd,
        &sandbox.agent_dir,
        &sandbox.settings,
        flag_factories(&inputs),
    );
    options.extensions_override = Some(Arc::new(move |extensions| {
        let mut probe = QueueProbe::new(extensions.runtime.clone(), inputs.clone());
        probe.seed();
        probe
            .actions("reload")
            .expect("provider fixtures have no reload rejection");
        let mut slot = capture.lock().unwrap();
        assert!(
            slot.is_none(),
            "queue seeding must happen once per actual reload"
        );
        *slot = Some(probe);
        extensions
    }));
    let mut loader = DefaultResourceLoader::new(options);
    loader.reload_without_trust().unwrap();
    let extensions = loader.get_extensions();
    assert!(extensions.errors.is_empty(), "{id}: loader failures");
    let runtime = extensions.runtime;
    // These are the exact production helpers used by services. Test code does
    // not iterate or clear the pending queue itself.
    runtime.flush_pending_providers(|registration| {
        let mut guard = slot.lock().unwrap();
        let probe = guard.as_mut().unwrap();
        let key = probe.ordinary_entry(&registration).key().to_string();
        probe.callback("ordinary", &key);
    });
    runtime.flush_pending_native_providers(|registration| {
        let mut guard = slot.lock().unwrap();
        let probe = guard.as_mut().unwrap();
        let key = probe.native_entry(&registration).key().to_string();
        probe.callback("native", &key);
    });
    let probe = slot.lock().unwrap().take().unwrap();
    let expected_attempts: Vec<_> = list(&case["expected"]["trace"])
        .iter()
        .filter_map(|event| {
            let queue = match event["event"].as_str()? {
                "runtime.registerProvider" => "ordinary",
                "runtime.registerNativeProvider" => "native",
                _ => return None,
            };
            Some(
                json!({"queue":queue,"key":event["key"],"queues":queue_projection(&event["queues"]),
            "flags":expected_flags(&event["flagValues"])}),
            )
        })
        .collect();
    assert_eq!(
        probe.attempts, expected_attempts,
        "{id}: ordered callbacks and callback-time queue contents"
    );
    assert_eq!(
        probe.queues(),
        queue_projection(&case["expected"]["queues"]),
        "{id}: final pending contents"
    );
    assert_eq!(
        json!(probe.executed),
        case["expected"]["executedActions"],
        "{id}: reentrant actions actually reached"
    );
    let expected_failures: Vec<_> = list(&case["expected"]["trace"])
        .iter()
        .filter(|event| event["event"] == "collaborator.action" && event["action"] == "throw")
        .map(|event| {
            let trigger = text(&event["trigger"]);
            let index = event["index"].as_u64().unwrap() as usize;
            (
                trigger.to_string(),
                case["inputs"]["actions"][trigger][index]["error"].clone(),
            )
        })
        .collect();
    assert_eq!(
        probe.collaborator_errors, expected_failures,
        "{id}: injected callback outcomes (not services diagnostics)"
    );
    // Flags are intentionally not applied here: no synchronous stand-in for
    // services' awaited runtime.refresh is used. Full flag cases run above.
}

macro_rules! queue_cases {
    ($($name:ident => $id:literal),+ $(,)?) => {
        $(#[test] fn $name() { consume_queues($id); })+
    };
}

queue_cases! {
    queues_ordinary_before_native => "providers-ordinary-before-native",
    queues_callback_errors_keep_later_attempts => "providers-errors-nonfatal-before-flags",
    queues_non_error_callback_values => "providers-non-error-null-undefined-number",
    queues_live_append => "providers-live-append-same-arrays",
    queues_append_before_callback_error => "providers-append-then-throw-still-drains",
    queues_filter_retains_ordinary_iterator => "providers-filter-keeps-active-ordinary-iterator",
    queues_filter_retains_native_iterator => "providers-filter-keeps-active-native-iterator",
    queues_filter_upcoming_native => "providers-filter-upcoming-native-group",
    queues_native_leaves_late_ordinary_pending => "providers-native-appends-already-cleared-ordinary",
    queues_real_reload_populates => "providers-reload-populates-queues",
}

// Real async refresh projection of refresh-awaited-before-late-flag-discovery.
// The concrete Rust loader owns an immutable declaration snapshot after reload;
// it cannot accept the oracle collaborator's addExtension during refresh.
// Therefore declarations from those input actions are installed at reload, and
// ONLY offline options, actual suspension, flag-before/after ordering and final
// values/diagnostics are certified here. Late declaration discovery is a gap.
struct RefreshGate {
    registration_refreshed: tokio::sync::Notify,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    runtime: Mutex<Option<ExtensionRuntime>>,
    phases: Mutex<Vec<Value>>,
    actions: Vec<Value>,
}

struct GatedRefreshProvider {
    gate: Arc<RefreshGate>,
    auth: crate::ai::auth::types::ProviderAuth,
}

impl crate::ai::models::Provider for GatedRefreshProvider {
    fn id(&self) -> &str {
        "services-oracle-offline-gate"
    }
    fn name(&self) -> &str {
        self.id()
    }
    fn auth(&self) -> &crate::ai::auth::types::ProviderAuth {
        &self.auth
    }
    fn get_models(&self) -> Result<Vec<Model>, crate::ai::auth::resolve::ModelsError> {
        Ok(Vec::new())
    }
    fn is_dynamic(&self) -> bool {
        true
    }
    fn refresh_models(
        &self,
        context: crate::ai::models::RefreshModelsContext,
    ) -> Option<
        futures::future::BoxFuture<'static, Result<(), crate::ai::models::RefreshModelsError>>,
    > {
        let gate = self.gate.clone();
        Some(Box::pin(async move {
            // Even on a regression enabling network this provider never makes
            // a request; record the supplied option and fail the assertion.
            let runtime = gate.runtime.lock().unwrap().clone();
            let Some(runtime) = runtime else {
                // Native registration starts a separate background refresh.
                // Settle its provider phase before installing the services
                // gate so it cannot supersede the refresh under observation.
                gate.registration_refreshed.notify_one();
                return Ok(());
            };
            gate.phases.lock().unwrap().push(json!({
                "event":"start","allowNetwork":context.allow_network,"flags":actual_flags(&runtime)
            }));
            gate.entered.notify_one();
            gate.release.notified().await;
            for action in &gate.actions {
                match text(&action["op"]) {
                    "setFlag" => {
                        runtime.set_flag_value(text(&action["name"]), flag(&action["value"]))
                    }
                    "addExtension" => {} // explicitly predeclared at reload, see boundary above
                    other => panic!("unsupported refresh projection action {other}"),
                }
            }
            gate.phases
                .lock()
                .unwrap()
                .push(json!({"event":"end","flags":actual_flags(&runtime)}));
            Ok(())
        }))
    }
}

#[tokio::test]
async fn refresh_real_await_offline_then_apply_flags_projection() {
    let id = "refresh-awaited-before-late-flag-discovery";
    let case = case(id);
    let sandbox = Sandbox::new();
    let mut inputs = case["inputs"].clone();
    let actions = list(&inputs["actions"]["refresh"]).to_vec();
    let declarations: Vec<_> = actions
        .iter()
        .filter(|action| action["op"] == "addExtension")
        .map(|action| action["extension"].clone())
        .collect();
    inputs["extensions"] = json!(declarations);
    let mut options = sandbox.options(&inputs).await;
    let gate = Arc::new(RefreshGate {
        registration_refreshed: tokio::sync::Notify::new(),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        runtime: Mutex::new(None),
        phases: Mutex::new(Vec::new()),
        actions,
    });
    options
        .model_runtime
        .as_ref()
        .unwrap()
        .register_native_provider_sync(Arc::new(GatedRefreshProvider {
            gate: gate.clone(),
            auth: Default::default(),
        }))
        .unwrap();
    bounded(gate.registration_refreshed.notified()).await;
    assert!(gate.phases.lock().unwrap().is_empty());
    let capture = gate.clone();
    let initial = case["inputs"].clone();
    options
        .resource_loader_options
        .as_mut()
        .unwrap()
        .extensions_override = Some(Arc::new(move |extensions| {
        seed_flags(&extensions.runtime, &initial);
        *capture.runtime.lock().unwrap() = Some(extensions.runtime.clone());
        extensions
    }));
    let mut pending = Box::pin(create_agent_session_services(options));
    bounded(async {
        tokio::select! {
            result = &mut pending => panic!("{id}: factory settled before refresh gate: {:?}", result.err()),
            _ = gate.entered.notified() => {}
        }
    }).await;
    match futures::poll!(pending.as_mut()) {
        std::task::Poll::Pending => {}
        std::task::Poll::Ready(result) => panic!(
            "{id}: real await settled before release: error={:?}, phases={:?}",
            result.err(),
            gate.phases.lock().unwrap()
        ),
    }
    let runtime = gate.runtime.lock().unwrap().clone().unwrap();
    assert_eq!(
        actual_flags(&runtime),
        expected_flags(&case["inputs"]["initialFlagValues"]),
        "{id}: flags applied before refresh settled"
    );
    let start = list(&case["expected"]["trace"])
        .iter()
        .find(|event| event["event"] == "runtime.refresh.start")
        .unwrap();
    assert_eq!(
        gate.phases.lock().unwrap().as_slice(),
        &[json!({
            "event":"start","allowNetwork":start["options"]["allowNetwork"],"flags":expected_flags(&start["flagValues"])
        })],
        "{id}: real refresh options and pre-flag state"
    );
    gate.release.notify_one();
    let services = bounded(pending).await.unwrap();
    let end = list(&case["expected"]["trace"])
        .iter()
        .find(|event| event["event"] == "runtime.refresh.end")
        .unwrap();
    let phases = gate.phases.lock().unwrap();
    assert_eq!(phases.len(), 2, "{id}: exactly one offline refresh phase");
    assert_eq!(
        phases[1],
        json!({"event":"end","flags":expected_flags(&end["flagValues"])})
    );
    assert_eq!(
        actual_flags(&runtime),
        expected_flags(&case["expected"]["flagValues"]),
        "{id}: flags after actual await"
    );
    assert_eq!(
        serde_json::to_value(services.diagnostics).unwrap(),
        case["expected"]["services"]["diagnostics"]
    );
}

// Exact Win32 path-value projections through the existing production path
// helper. These five IDs do NOT certify the services default-directory branch,
// runtime/settings constructors, or process-global cwd/environment mutation.
#[test]
fn construction_explicit_paths_use_real_path_helper_against_ts_values() {
    use crate::coding_agent::utils::paths::{resolve_path_with, PathInputOptions};
    for id in [
        "construct-relative-agent-dir",
        "construct-absolute-agent-dir",
        "construct-tilde-paths",
        "construct-windows-shell-paths",
        "construct-file-url-paths",
    ] {
        let case = case(id);
        let options = PathInputOptions {
            home_dir: Some(text(&oracle()["environment"]["homeDir"]).into()),
            ..Default::default()
        };
        for field in ["cwd", "agentDir"] {
            let actual = resolve_path_with(
                text(&case["inputs"]["options"][field]),
                text(&oracle()["environment"]["processCwd"]),
                &options,
                true,
            )
            .unwrap();
            assert_eq!(
                json!(actual),
                case["expected"]["services"][field],
                "{id}: {field}"
            );
        }
    }
}

// Constructors use temporary directories and *both* supplied memory handles for
// safety. The three reuse cases certify only their explicitly supplied handles
// and ignored creation signal, not creation of the other unsupplied handle or
// empty-agentDir/default-home behavior.
#[tokio::test]
async fn construction_reuse_handles_and_ignore_creation_signal_projections() {
    for id in [
        "construct-runtime-reused",
        "construct-settings-reused",
        "construct-both-reused-signal-not-consumed",
    ] {
        let case = case(id);
        let sandbox = Sandbox::new();
        let mut options = sandbox.options(&case["inputs"]).await;
        let runtime = options.model_runtime.as_ref().unwrap().clone();
        if case["inputs"]["options"]
            .get("modelRuntimeSignal")
            .is_some()
        {
            let token = CancellationToken::new();
            token.cancel();
            options.model_runtime_signal = Some(token);
        }
        let services = bounded(create_agent_session_services(options))
            .await
            .unwrap();
        if case["inputs"]["options"].get("modelRuntime").is_some() {
            let provider = faux_provider(FauxProviderOptions {
                provider: Some("oracle-shared-runtime".into()),
                ..Default::default()
            })
            .provider;
            runtime
                .register_native_provider_sync(provider.clone())
                .unwrap();
            let returned = bounded(services.model_runtime.get_provider(provider.id()))
                .await
                .unwrap();
            assert_eq!(
                json!(Arc::ptr_eq(&returned, &provider)),
                case["expected"]["identity"]["modelRuntime"],
                "{id}: shared runtime after mutation"
            );
        }
        if case["inputs"]["options"].get("settingsManager").is_some() {
            sandbox.settings.set_block_images(true);
            assert_eq!(
                json!(services.settings_manager.get_block_images()),
                case["expected"]["identity"]["settingsManager"],
                "{id}: shared settings after mutation"
            );
        }
        assert_eq!(
            serde_json::to_value(services.diagnostics).unwrap(),
            case["expected"]["services"]["diagnostics"],
            "{id}"
        );
    }
}

#[tokio::test]
async fn construction_loader_spread_overrides_reserved_fields_projection() {
    let id = "construct-spread-overrides-reserved-fields";
    let case = case(id);
    let sandbox = Sandbox::new();
    let mut options = sandbox.options(&case["inputs"]).await;
    let requested = &case["inputs"]["options"]["resourceLoaderOptions"];
    let shadow = memory_settings();
    shadow.set_project_trusted(true);
    let loader = options.resource_loader_options.as_mut().unwrap();
    loader.cwd = text(&requested["cwd"]).into();
    loader.agent_dir = text(&requested["agentDir"]).into();
    loader.settings_manager = Some(Arc::new(shadow.clone()));
    loader.no_extensions = requested["noExtensions"].as_bool().unwrap();
    loader.no_skills = requested["noSkills"].as_bool().unwrap();
    loader.system_prompt = Some(text(&requested["systemPrompt"]).into());
    // Explicit local paths only: no package fetching. The missing-file
    // diagnostic supplies an observable real loader cwd without private access.
    loader.additional_extension_paths = list(&requested["additionalExtensionPaths"])
        .iter()
        .map(|path| text(path).to_string())
        .collect();
    options.resource_loader_reload_options = Some(ResourceLoaderReloadOptions {
        resolve_project_trust: Some(Arc::new(|_| Box::pin(async { Ok(false) }))),
    });
    let services = bounded(create_agent_session_services(options))
        .await
        .unwrap();
    let loader = services.resource_loader.lock().unwrap();
    assert_eq!(
        loader.get_system_prompt().as_deref(),
        requested["systemPrompt"].as_str(),
        "{id}: nonreserved option survives"
    );
    assert!(
        !sandbox.settings.is_project_trusted(),
        "{id}: effective settings receives reload"
    );
    assert!(
        shadow.is_project_trusted(),
        "{id}: shadow loader settings must not receive reload"
    );
    let expected_cwd = resolve_path_auto_base(&sandbox.cwd).unwrap();
    let extensions = loader.get_extensions();
    assert!(
        extensions.errors.iter().any(|error| {
            error.path
                == path_join(
                    &expected_cwd,
                    text(&requested["additionalExtensionPaths"][0]),
                )
        }),
        "{id}: relative extension path must use effective services cwd, errors: {:?}",
        extensions.errors
    );
    assert_eq!(services.cwd, expected_cwd);
    assert_eq!(
        services.agent_dir,
        resolve_path_auto_base(&sandbox.agent_dir).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&services.diagnostics).unwrap(),
        case["expected"]["services"]["diagnostics"]
    );
    // The TS spy fixture has no actual file-loader diagnostics. The loader
    // error above is a cwd witness, not a claimed oracle diagnostic match.
}

#[tokio::test]
async fn construction_reload_options_reach_original_callback_projection() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let id = "construct-signal-and-reload-options-identity";
    let case = case(id);
    let sandbox = Sandbox::new();
    let mut options = sandbox.options(&case["inputs"]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let capture = calls.clone();
    options.resource_loader_reload_options = Some(ResourceLoaderReloadOptions {
        resolve_project_trust: Some(Arc::new(move |_| {
            capture.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(false) })
        })),
    });
    let append = &case["inputs"]["options"]["resourceLoaderOptions"]["appendSystemPrompt"];
    options
        .resource_loader_options
        .as_mut()
        .unwrap()
        .append_system_prompt = Some(
        list(append)
            .iter()
            .map(|value| text(value).to_string())
            .collect(),
    );
    let services = bounded(create_agent_session_services(options))
        .await
        .unwrap();
    let reload = list(&case["expected"]["trace"])
        .iter()
        .filter(|event| event["event"] == "loader.reload.start")
        .count();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        reload,
        "{id}: original callback invocation count"
    );
    let construct = list(&case["expected"]["trace"])
        .iter()
        .find(|event| event["event"] == "loader.construct")
        .unwrap();
    assert_eq!(
        json!(services
            .resource_loader
            .lock()
            .unwrap()
            .get_append_system_prompt()),
        construct["options"]["appendSystemPrompt"]
    );
    // Callback invocation is exercised here; the async trust integration tests
    // cover suspension/rejection. No claim about JS Promise identity or signal when
    // constructing a new runtime (a supplied memory runtime is used here).
}

fn handle_value<'a>(inputs: &'a Value, value: &'a Value) -> &'a Value {
    match value.get("$handle") {
        Some(name) => &inputs["handles"][text(name)]["value"],
        None => value,
    }
}

fn optional_forwarded<'a>(inputs: &'a Value, options: &'a Value, key: &str) -> Option<&'a Value> {
    options
        .get(key)
        .filter(|value| value.get("$undefined") != Some(&Value::Bool(true)))
        .map(|value| handle_value(inputs, value))
}

fn string_options(inputs: &Value, options: &Value, key: &str) -> Option<Vec<String>> {
    optional_forwarded(inputs, options, key).map(|values| {
        list(values)
            .iter()
            .map(|value| text(value).to_string())
            .collect()
    })
}

fn forward_model(spec: &Value) -> Model {
    // The oracle model handle contains only id/provider. Supply an explicitly
    // in-memory reasoning-capable Faux model to the *real* SDK; no stream runs.
    // JS Model object identity cannot be represented by Rust's owned Model.
    faux_provider(FauxProviderOptions {
        api: Some("services-oracle-memory-api".into()),
        provider: Some(text(&spec["provider"]).into()),
        models: vec![crate::ai::models::FauxModelDefinition {
            id: text(&spec["id"]).into(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    })
    .get_model(Some(text(&spec["id"])))
    .unwrap()
}

// Four forwarding cases call the real async from-services -> SDK -> AgentSession
// path. Assertions observe model/thinking/scoped order, tools, original session
// manager/custom tool/callback Arcs, shared runtime/settings and session_start.
// The opaque TS SDK-spy return object/undefined-key enumeration and raw path
// identity are not observable through this typed SDK. It resolves cwd before
// use. Loader identity is a shared-extension-runtime witness, not Arc::ptr_eq
// on AgentSession's private loader. noTools/excludeTools are exercised but their
// independent effect is masked by explicit tools in these particular fixtures.
async fn consume_forwarding(id: &str) {
    use crate::coding_agent::agent_session::ExtensionBindings;
    use crate::coding_agent::session_manager::NewSessionOptions;
    let case = case(id);
    let inputs = &case["inputs"];
    let provided = &inputs["options"];
    let expected = &case["expected"]["forwarded"];
    let sandbox = Sandbox::new();
    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let factory: ExtensionFactory = Arc::new(move |api| {
        let capture = capture.clone();
        api.on(
            "session_start",
            Arc::new(move |event, _| {
                let capture = capture.clone();
                Box::pin(async move {
                    tokio::task::yield_now().await;
                    capture.lock().unwrap().push(event.clone());
                    Ok(None)
                })
            }),
        )?;
        Ok(())
    });
    let mut creation = sandbox.options(&json!({})).await;
    creation.resource_loader_options = Some(loader_options(
        &sandbox.cwd,
        &sandbox.agent_dir,
        &sandbox.settings,
        vec![factory],
    ));
    let mut services = bounded(create_agent_session_services(creation))
        .await
        .unwrap();
    let runtime = services.model_runtime.clone();
    let loader = services.resource_loader.clone();
    services.cwd = text(&inputs["services"]["cwd"]).into();
    services.agent_dir = text(&inputs["services"]["agentDir"]).into();
    services.diagnostics =
        serde_json::from_value(inputs["services"]["diagnostics"].clone()).unwrap();
    let manager = Arc::new(Mutex::new(
        SessionManager::in_memory(
            &sandbox.cwd,
            Some(&NewSessionOptions {
                id: Some(format!("oracle-{id}")),
                ..Default::default()
            }),
            None,
        )
        .unwrap(),
    ));
    let mut options = CreateAgentSessionFromServicesOptions::new(services, manager.clone());
    options.model = optional_forwarded(inputs, provided, "model").map(forward_model);
    options.thinking_level = optional_forwarded(inputs, provided, "thinkingLevel")
        .map(|value| serde_json::from_value(value.clone()).unwrap());
    options.scoped_models = optional_forwarded(inputs, provided, "scopedModels")
        .map(|values| {
            list(values)
                .iter()
                .map(|entry| ScopedModel {
                    model: forward_model(handle_value(inputs, &entry["model"])),
                    thinking_level: entry
                        .get("thinkingLevel")
                        .map(|value| serde_json::from_value(value.clone()).unwrap()),
                })
                .collect()
        })
        .unwrap_or_default();
    options.tools = string_options(inputs, provided, "tools");
    options.exclude_tools = string_options(inputs, provided, "excludeTools");
    options.no_tools =
        optional_forwarded(inputs, provided, "noTools").map(|value| match text(value) {
            "builtin" => NoTools::Builtin,
            "all" => NoTools::All,
            other => panic!("unsupported noTools oracle value {other}"),
        });
    options.custom_tools = optional_forwarded(inputs, provided, "customTools")
        .map(|values| {
            list(values)
                .iter()
                .map(|spec| {
                    let mut definition = ToolDefinition::new(
                        text(&spec["name"]),
                        "Memory tool",
                        "No I/O",
                        json!({"type":"object"}),
                    );
                    if spec.get("execute").is_some() {
                        definition.execute = Some(Arc::new(|_, _, _, _, _| {
                            Ok(json!({"content":[],"details":{}}))
                        }));
                    }
                    Arc::new(definition)
                })
                .collect()
        })
        .unwrap_or_default();
    options.session_start_event =
        optional_forwarded(inputs, provided, "sessionStartEvent").cloned();
    let original_tools = options.custom_tools.clone();
    let result = bounded(create_agent_session_from_services(options))
        .await
        .unwrap();
    assert_eq!(case["expected"]["outcome"]["kind"], "returned", "{id}");
    let session = result.session;
    let identity = &list(&case["expected"]["trace"])[0]["identity"];
    assert_eq!(
        json!(Arc::ptr_eq(&session.session_manager, &manager)),
        identity["sessionManager"],
        "{id}: original session manager"
    );
    sandbox.settings.set_block_images(true);
    assert_eq!(
        json!(session.settings_manager.get_block_images()),
        identity["settingsManager"],
        "{id}: shared settings mutation"
    );
    let provider = faux_provider(FauxProviderOptions {
        provider: Some("oracle-forwarded-runtime".into()),
        ..Default::default()
    })
    .provider;
    runtime
        .register_native_provider_sync(provider.clone())
        .unwrap();
    let returned = bounded(session.model_runtime().get_provider(provider.id()))
        .await
        .unwrap();
    assert_eq!(
        json!(Arc::ptr_eq(&returned, &provider)),
        identity["modelRuntime"],
        "{id}: original runtime registry"
    );
    let loader_runtime = loader.lock().unwrap().get_extensions().runtime;
    loader_runtime.set_flag_value("post-sdk-identity-witness", FlagValue::Str(id.into()));
    assert_eq!(
        result
            .extensions_result
            .runtime
            .flag_value("post-sdk-identity-witness"),
        Some(FlagValue::Str(id.into())),
        "{id}: supplied loader runtime is shared"
    );
    assert_eq!(
        session.extension_runner().create_context().cwd().unwrap(),
        resolve_path_auto_base(text(&expected["cwd"])).unwrap(),
        "{id}: cwd reaches real SDK"
    );
    if let Some(model) = optional_forwarded(inputs, expected, "model") {
        let actual = session.model().expect("explicit model must reach SDK");
        assert_eq!(
            json!({"id":actual.id,"provider":actual.provider}),
            *model,
            "{id}: model value"
        );
    }
    if let Some(thinking) = optional_forwarded(inputs, expected, "thinkingLevel") {
        assert_eq!(
            serde_json::to_value(session.thinking_level()).unwrap(),
            *thinking,
            "{id}: thinking value"
        );
    }
    let actual_scoped: Vec<_> = session.scoped_models().into_iter().map(|entry| json!({
        "model":{"id":entry.model.id,"provider":entry.model.provider},"thinkingLevel":entry.thinking_level
    })).collect();
    let expected_scoped: Vec<_> = optional_forwarded(inputs, expected, "scopedModels")
        .map(|values| {
            list(values).iter().map(|entry| json!({
            "model":handle_value(inputs, &entry["model"]),"thinkingLevel":entry["thinkingLevel"]
        })).collect()
        })
        .unwrap_or_default();
    assert_eq!(
        actual_scoped, expected_scoped,
        "{id}: ordered scoped models"
    );
    if let Some(mut tools) = string_options(inputs, expected, "tools") {
        if let Some(excluded) = string_options(inputs, expected, "excludeTools") {
            tools.retain(|name| !excluded.contains(name));
        }
        assert_eq!(
            session.get_active_tool_names(),
            tools,
            "{id}: explicit tool list and order"
        );
    }
    let expected_custom = optional_forwarded(inputs, expected, "customTools")
        .map(list)
        .unwrap_or(&[]);
    assert_eq!(original_tools.len(), expected_custom.len(), "{id}");
    for (original, expected_tool) in original_tools.iter().zip(expected_custom) {
        let actual = session
            .get_tool_definition(text(&expected_tool["name"]))
            .expect("custom tool forwarded");
        assert!(
            Arc::ptr_eq(original, &actual),
            "{id}: original custom ToolDefinition Arc"
        );
        if let Some(execute) = &original.execute {
            assert!(
                Arc::ptr_eq(execute, actual.execute.as_ref().unwrap()),
                "{id}: original execute closure Arc"
            );
        }
    }
    if let Some(start) = optional_forwarded(inputs, expected, "sessionStartEvent") {
        bounded(session.bind_extensions(ExtensionBindings::default()))
            .await
            .unwrap();
        assert_eq!(
            events.lock().unwrap().as_slice(),
            std::slice::from_ref(start),
            "{id}: actual awaited session_start handler"
        );
    }
    session.dispose();
}

macro_rules! forwarding_cases {
    ($($name:ident => $id:literal),+ $(,)?) => {
        $(#[tokio::test] async fn $name() { consume_forwarding($id).await; })+
    };
}

forwarding_cases! {
    forward_required_real_sdk_projection => "forward-required-only-explicit-undefined-keys",
    forward_all_handles_real_sdk_projection => "forward-all-options-preserve-handle-identity",
    forward_no_tools_all_real_sdk_projection => "forward-all-no-tools",
    forward_empty_arrays_real_sdk_projection => "forward-empty-arrays",
}

// Deliberately NOT consumed (nine fixture IDs):
// construct-default-agent-dir
// construct-empty-cwd-empty-agent-dir
// construct-default-agent-dir-not-resolved-again
// construct-nullish-runtime-settings-fallback
// refresh-rejects-no-flag-application
// reload-rejects-no-registration-or-refresh
// runtime-create-rejects-before-settings
// settings-create-rejects-before-loader
// forward-sdk-rejection-propagates
//
// The first four require a safe services-level default-environment/constructor
// seam. The five rejection fixtures throw from the TS memory collaborators;
// concrete Rust constructors/SDK expose no corresponding replaceable factory.
// Returning fabricated Err values from a test wrapper would not certify these
// production await/propagation boundaries, so no such pseudo-coverage is used.
