use crate::models::{
    AdvertiseHandle, AdvertiseOptions, AdvertiseServiceSpec, BrowseHandle, BrowseOptions,
    BrowseServiceSpec, ServiceRecord, TransportProtocol, TxtRecordValue, TxtWireValue,
};
use log::{debug, error, info, warn};
use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo, TxtProperty};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;
use tauri::{AppHandle, Runtime};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

const DEFAULT_BROWSE_TIMEOUT_MS: u64 = 30_000;

// Unified channel message type that can send either browse events or stopped events
// This matches the pattern used by mobile platforms
#[derive(Clone, Serialize)]
#[serde(untagged)]
// The `Service` variant is intentionally larger than `Stopped`; boxing the
// record would complicate every construction site for a message that is created
// once per event and sent straight over the IPC channel.
#[allow(clippy::large_enum_variant)]
pub enum BrowseChannelMessage {
    Service {
        #[serde(rename = "browseId")]
        browse_id: u64,
        service: ServiceRecord,
    },
    Stopped {
        #[serde(rename = "browseId")]
        browse_id: u64,
        reason: String,
    },
}

pub struct MdnsState {
    daemon: Arc<Mutex<Option<ServiceDaemon>>>,
    browse_sessions: Arc<Mutex<HashMap<u64, BrowseSession>>>,
    advertise_sessions: Arc<Mutex<HashMap<u64, AdvertiseSession>>>,
    next_handle_id: AtomicU64,
}

impl Default for MdnsState {
    fn default() -> Self {
        Self {
            daemon: Arc::new(Mutex::new(None)),
            browse_sessions: Arc::new(Mutex::new(HashMap::new())),
            advertise_sessions: Arc::new(Mutex::new(HashMap::new())),
            next_handle_id: AtomicU64::new(1),
        }
    }
}

struct BrowseSession {
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    timeout_task: Option<JoinHandle<()>>,
    known_types: Arc<Mutex<HashSet<String>>>,
    stop_notifier: Arc<AtomicBool>,
    channel: Channel<BrowseChannelMessage>,
}

impl BrowseSession {
    fn new(channel: Channel<BrowseChannelMessage>) -> Self {
        Self {
            cancel: CancellationToken::new(),
            tasks: Vec::new(),
            timeout_task: None,
            known_types: Arc::new(Mutex::new(HashSet::new())),
            stop_notifier: Arc::new(AtomicBool::new(false)),
            channel,
        }
    }
}

struct AdvertiseSession {
    fullname: String,
}

impl MdnsState {
    fn next_id(&self) -> u64 {
        self.next_handle_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn ensure_daemon(&self) -> Result<ServiceDaemon, String> {
        let mut guard = self.daemon.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }
        let daemon =
            ServiceDaemon::new().map_err(|e| format!("failed to create mDNS daemon: {e}"))?;
        *guard = Some(daemon.clone());
        info!("mDNS daemon started");
        Ok(daemon)
    }

    async fn get_daemon(&self) -> Option<ServiceDaemon> {
        self.daemon.lock().await.as_ref().cloned()
    }

    async fn maybe_shutdown_daemon(&self) {
        let mut daemon_guard = self.daemon.lock().await;
        let browse_guard = self.browse_sessions.lock().await;
        let advertise_guard = self.advertise_sessions.lock().await;

        if browse_guard.is_empty() && advertise_guard.is_empty() {
            if let Some(daemon) = daemon_guard.take() {
                info!("mDNS daemon idle; shutting down");
                if let Err(err) = daemon.shutdown() {
                    warn!("failed to shutdown daemon cleanly: {err}");
                }
            }
        }
    }
}

pub async fn browse_start_impl<R: Runtime>(
    _app: AppHandle<R>,
    state: Arc<MdnsState>,
    options: BrowseOptions,
    channel: Channel<BrowseChannelMessage>,
) -> Result<BrowseHandle, String> {
    let daemon = state.ensure_daemon().await?;
    let browse_id = state.next_id();
    let mut session = BrowseSession::new(channel);
    let cancel_token = session.cancel.clone();
    let known_types = session.known_types.clone();
    let stop_notifier = session.stop_notifier.clone();
    let session_channel = session.channel.clone();
    let timeout_ms = options.timeout_ms.unwrap_or(DEFAULT_BROWSE_TIMEOUT_MS);

    if timeout_ms > 0 {
        let timeout_handle =
            spawn_browse_timeout(state.clone(), browse_id, Duration::from_millis(timeout_ms));
        session.timeout_task = Some(timeout_handle);
    }

    state
        .browse_sessions
        .lock()
        .await
        .insert(browse_id, session);

    // Browse for the specific service type
    let watch_types = service_types_from_spec(&options.service);
    {
        let mut guard = known_types.lock().await;
        for ty in &watch_types {
            guard.insert(ty.clone());
        }
    }

    for ty in watch_types {
        let receiver = daemon
            .browse(&ty)
            .map_err(|e| format!("failed to browse {ty}: {e}"))?;
        let handle = spawn_service_receiver(
            browse_id,
            ty.clone(),
            receiver,
            cancel_token.clone(),
            stop_notifier.clone(),
            session_channel.clone(),
        );
        register_task(&state.browse_sessions, browse_id, handle).await;
    }

    info!("started browse session {browse_id}");
    Ok(BrowseHandle { browse_id })
}

pub async fn browse_stop_impl<R: Runtime>(
    _app: AppHandle<R>,
    state: Arc<MdnsState>,
    browse_id: u64,
) -> Result<(), String> {
    stop_browse_session(state, browse_id, "stopped").await;
    Ok(())
}

pub async fn advertise_start_impl<R: Runtime>(
    _app: AppHandle<R>,
    state: Arc<MdnsState>,
    options: AdvertiseOptions,
) -> Result<AdvertiseHandle, String> {
    let daemon = state.ensure_daemon().await?;
    let advertise_id = state.next_id();
    let (service_info, fullname) = build_service_info(&options.service)?;
    daemon
        .register(service_info)
        .map_err(|e| format!("failed to register service: {e}"))?;
    let name = options.service.name.clone();
    state
        .advertise_sessions
        .lock()
        .await
        .insert(advertise_id, AdvertiseSession { fullname });
    info!("registered advertisement {advertise_id}");
    Ok(AdvertiseHandle { advertise_id, name })
}

pub async fn advertise_stop_impl<R: Runtime>(
    _app: AppHandle<R>,
    state: Arc<MdnsState>,
    advertise_id: u64,
) -> Result<(), String> {
    let Some(session) = state.advertise_sessions.lock().await.remove(&advertise_id) else {
        return Ok(());
    };
    let daemon = state.ensure_daemon().await?;
    match daemon.unregister(&session.fullname) {
        Ok(receiver) => {
            tokio::spawn(async move {
                if let Err(err) = receiver.recv_async().await {
                    warn!("failed to confirm unregister: {err}");
                }
            });
        }
        Err(err) => warn!("failed to unregister service {}: {err}", session.fullname),
    }
    info!("advertisement {advertise_id} unregistered");
    state.maybe_shutdown_daemon().await;
    Ok(())
}

async fn register_task(
    sessions: &Arc<Mutex<HashMap<u64, BrowseSession>>>,
    browse_id: u64,
    handle: JoinHandle<()>,
) {
    let mut guard = sessions.lock().await;
    if let Some(session) = guard.get_mut(&browse_id) {
        session.tasks.push(handle);
    } else {
        handle.abort();
    }
}

fn spawn_browse_timeout(
    state: Arc<MdnsState>,
    browse_id: u64,
    timeout: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        sleep(timeout).await;
        debug!("browse {browse_id} timed out after {:?}", timeout);
        stop_browse_session(state, browse_id, "timeout").await;
    })
}

async fn stop_browse_session(state: Arc<MdnsState>, browse_id: u64, reason: &str) {
    let Some(mut session) = state.browse_sessions.lock().await.remove(&browse_id) else {
        return;
    };

    let known_types_to_stop = session.known_types.lock().await.clone();
    session.cancel.cancel();
    for handle in session.tasks.drain(..) {
        handle.abort();
    }
    if let Some(handle) = session.timeout_task.take() {
        handle.abort();
    }

    if let Some(daemon) = state.get_daemon().await {
        for service_type in known_types_to_stop {
            if let Err(err) = daemon.stop_browse(&service_type) {
                warn!("failed to stop browse for {}: {}", service_type, err);
            }
        }
    }

    notify_browse_stop(browse_id, reason, &session.stop_notifier, &session.channel);
    info!("browse session {browse_id} stopped ({reason})");
    state.maybe_shutdown_daemon().await;
}

fn spawn_service_receiver(
    browse_id: u64,
    service_type: String,
    receiver: Receiver<ServiceEvent>,
    cancel: CancellationToken,
    stop_notifier: Arc<AtomicBool>,
    channel: Channel<BrowseChannelMessage>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! { _ = cancel.cancelled() => { debug!("browse {browse_id} cancelled for {service_type}"); break; } event = receiver.recv_async() => { match event { Ok(ServiceEvent::ServiceResolved(info)) => { let record = service_record_from_resolved(&info, true); emit_browse_event(browse_id, record, &channel); } Ok(ServiceEvent::ServiceRemoved(ty, fullname)) => { let record = removal_record(&ty, &fullname); emit_browse_event(browse_id, record, &channel); } Ok(ServiceEvent::SearchStopped(_)) => { notify_browse_stop(browse_id, "search-stopped", &stop_notifier, &channel); break; } Ok(other) => { debug!("ignored event for {service_type}: {:?}", other); } Err(err) => { warn!("receiver closed for {service_type}: {err}"); notify_browse_stop(browse_id, "receiver-closed", &stop_notifier, &channel); break; } } } }
        }
    })
}

fn emit_browse_event(
    browse_id: u64,
    service: ServiceRecord,
    channel: &Channel<BrowseChannelMessage>,
) {
    let message = BrowseChannelMessage::Service { browse_id, service };
    if let Err(err) = channel.send(message) {
        error!("failed to send browse event via channel: {err}");
    }
}

fn notify_browse_stop(
    browse_id: u64,
    reason: &str,
    flag: &Arc<AtomicBool>,
    channel: &Channel<BrowseChannelMessage>,
) {
    if flag.swap(true, Ordering::SeqCst) {
        return;
    }
    let message = BrowseChannelMessage::Stopped {
        browse_id,
        reason: reason.to_string(),
    };
    if let Err(err) = channel.send(message) {
        error!("failed to send browse stop event via channel: {err}");
    }
}

fn service_record_from_resolved(info: &ResolvedService, is_active: bool) -> ServiceRecord {
    let mut addresses: Vec<String> = info
        .get_addresses()
        .iter()
        .map(|addr| addr.to_ip_addr().to_string())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    addresses.sort();

    let txt = info
        .get_properties()
        .iter()
        .map(|prop| (prop.key().to_string(), txt_wire_from_property(prop)))
        .collect::<HashMap<_, _>>();
    ServiceRecord {
        name: info
            .get_fullname()
            .split('.')
            .next()
            .unwrap_or_default()
            .to_string(),
        full_name: info.get_fullname().to_string(),
        host: Some(info.get_hostname().to_string()),
        port: Some(info.get_port()),
        service_type: info.ty_domain.clone(),
        protocol: protocol_from_type(&info.ty_domain).to_string(),
        domain: extract_domain(&info.ty_domain),
        subtypes: info
            .get_subtype()
            .as_ref()
            .map(|s| vec![s.clone()])
            .unwrap_or_default(),
        addresses,
        txt,
        is_active,
        last_seen_ms: now_ms(),
    }
}
fn removal_record(service_type: &str, fullname: &str) -> ServiceRecord {
    ServiceRecord {
        name: fullname.split('.').next().unwrap_or_default().to_string(),
        full_name: fullname.to_string(),
        host: None,
        port: None,
        service_type: service_type.to_string(),
        protocol: protocol_from_type(service_type).to_string(),
        domain: extract_domain(service_type),
        subtypes: Vec::new(),
        addresses: Vec::new(),
        txt: HashMap::new(),
        is_active: false,
        last_seen_ms: now_ms(),
    }
}
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}
/// Map an mdns-sd TXT property to the 3-state wire value expected by the shared
/// contract: a bare key (no value) → `true`, an empty value (`key=`) → `null`,
/// and a non-empty value → its bytes.
fn txt_wire_from_property(prop: &TxtProperty) -> TxtWireValue {
    match prop.val() {
        None => TxtWireValue::Flag(true),
        Some([]) => TxtWireValue::Null,
        Some(bytes) => TxtWireValue::Bytes(bytes.to_vec()),
    }
}
fn service_types_from_spec(spec: &BrowseServiceSpec) -> Vec<String> {
    let base = format_service_type(&spec.type_name, spec.protocol, spec.domain.as_deref());
    if spec.subtypes.is_empty() {
        return vec![base];
    }
    // Browse each requested subtype: `_<sub>._sub._<type>._<proto>.<domain>`.
    spec.subtypes
        .iter()
        .map(|sub| format!("{}._sub.{base}", ensure_prefixed(sub)))
        .collect()
}
fn format_service_type(ty: &str, protocol: TransportProtocol, domain: Option<&str>) -> String {
    let type_part = ensure_prefixed(ty);
    let domain_part = normalize_domain(domain.unwrap_or("local"));
    format!("{type_part}.{}.{}", protocol.dns_label(), domain_part)
}
fn ensure_prefixed(value: &str) -> String {
    if value.starts_with('_') {
        value.to_string()
    } else {
        format!("_{value}")
    }
}
fn normalize_domain(domain: &str) -> String {
    let trimmed = domain.trim().trim_end_matches('.').to_lowercase();
    if trimmed.is_empty() {
        "local.".into()
    } else {
        format!("{trimmed}.")
    }
}
fn extract_domain(service_type: &str) -> String {
    let lowered = service_type.to_lowercase();
    for marker in ["._tcp.", "._udp."] {
        if let Some(idx) = lowered.find(marker) {
            return lowered[idx + marker.len()..].to_string();
        }
    }
    lowered
}
fn protocol_from_type(service_type: &str) -> &'static str {
    if service_type.contains("._udp.") {
        "udp"
    } else {
        "tcp"
    }
}
fn build_service_info(spec: &AdvertiseServiceSpec) -> Result<(ServiceInfo, String), String> {
    let ty_domain = format_service_type(&spec.type_name, spec.protocol, spec.domain.as_deref());
    let host_name = spec
        .host
        .as_ref()
        .map(|host| ensure_hostname(host))
        .unwrap_or_else(|| format!("{}.{}", sanitize_instance_name(&spec.name), "local."));
    let txt_properties = txt_properties_from_map(&spec.txt)?;
    let info = if let Some(host_ip) = spec.host.as_ref() {
        ServiceInfo::new(
            &ty_domain,
            &spec.name,
            &host_name,
            host_ip.as_str(),
            spec.port,
            txt_properties,
        )
    } else {
        ServiceInfo::new(
            &ty_domain,
            &spec.name,
            &host_name,
            (),
            spec.port,
            txt_properties,
        )
        .map(|info| info.enable_addr_auto())
    };
    let info = info.map_err(|e| format!("invalid service info: {e}"))?;
    let fullname = format!("{}.{ty_domain}", spec.name);
    Ok((info, fullname.to_lowercase()))
}
fn ensure_hostname(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.ends_with('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}
fn sanitize_instance_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "service".into()
    } else {
        trimmed.to_string()
    }
}
fn txt_properties_from_map(
    map: &HashMap<String, TxtRecordValue>,
) -> Result<Vec<TxtProperty>, String> {
    let mut props = Vec::new();
    for (key, value) in map {
        if key.trim().is_empty() {
            return Err("TXT property keys cannot be empty".into());
        }
        let prop = match value {
            TxtRecordValue::BooleanFlag(true) => TxtProperty::from(key.as_str()),
            TxtRecordValue::BooleanFlag(false) => {
                return Err(format!(
                    "TXT record '{key}' has boolean value false; only true is allowed for flags"
                ));
            }
            TxtRecordValue::BinaryData(bytes) => TxtProperty::from((key.as_str(), bytes.clone())),
            // An explicit empty value (`key=`) is distinct from a bare key: encode
            // it as a zero-length value so it round-trips to `null` on the wire.
            TxtRecordValue::Null => TxtProperty::from((key.as_str(), Vec::<u8>::new())),
        };
        props.push(prop);
    }
    Ok(props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AdvertiseOptions, AdvertiseServiceSpec, BrowseOptions, BrowseServiceSpec};
    use std::sync::Mutex as StdMutex;
    use tauri::ipc::{Channel, InvokeResponseBody};
    use tauri::test::{mock_app, MockRuntime};
    use tauri::AppHandle;

    // ── Always-on unit tests (no network) ───────────────────────────────────

    #[test]
    fn txt_three_state_round_trips_through_service_info() {
        // A bare key (`true`), an explicit empty value (`null`) and a binary
        // value must survive encoding into an mdns-sd `ServiceInfo` and decoding
        // back through `txt_wire_from_property`, matching the shared contract.
        let mut txt = HashMap::new();
        txt.insert("flag".to_string(), TxtRecordValue::BooleanFlag(true));
        txt.insert("empty".to_string(), TxtRecordValue::Null);
        txt.insert(
            "data".to_string(),
            TxtRecordValue::BinaryData(vec![1, 2, 3]),
        );

        let spec = AdvertiseServiceSpec {
            name: "TXT Test".into(),
            type_name: "http".into(),
            protocol: TransportProtocol::Tcp,
            port: 8080,
            host: None,
            domain: None,
            subtypes: Vec::new(),
            txt,
        };

        let (info, _fullname) = build_service_info(&spec).expect("service info");
        let decoded: HashMap<String, TxtWireValue> = info
            .get_properties()
            .iter()
            .map(|p| (p.key().to_string(), txt_wire_from_property(p)))
            .collect();

        assert_eq!(decoded.get("flag"), Some(&TxtWireValue::Flag(true)));
        assert_eq!(decoded.get("empty"), Some(&TxtWireValue::Null));
        assert_eq!(
            decoded.get("data"),
            Some(&TxtWireValue::Bytes(vec![1, 2, 3]))
        );
    }

    #[test]
    fn service_type_and_domain_formatting() {
        assert_eq!(
            format_service_type("http", TransportProtocol::Tcp, None),
            "_http._tcp.local."
        );
        assert_eq!(
            format_service_type("_ipp", TransportProtocol::Udp, Some("example")),
            "_ipp._udp.example."
        );
        assert_eq!(extract_domain("_http._tcp.local."), "local.");
        assert_eq!(protocol_from_type("_x._udp.local."), "udp");
        assert_eq!(protocol_from_type("_x._tcp.local."), "tcp");
    }

    #[test]
    fn subtype_browse_types_are_sub_qualified() {
        let spec = BrowseServiceSpec {
            type_name: "http".into(),
            protocol: TransportProtocol::Tcp,
            domain: None,
            subtypes: vec!["printer".into()],
        };
        let types = service_types_from_spec(&spec);
        assert_eq!(types, vec!["_printer._sub._http._tcp.local.".to_string()]);
    }

    // ── Network-gated end-to-end tests (real mDNS on the local segment) ──────

    fn network_tests_enabled() -> bool {
        std::env::var("DNS_SD_NETWORK_TESTS")
            .map(|v| v == "1")
            .unwrap_or(false)
    }

    type Collected = Arc<StdMutex<Vec<serde_json::Value>>>;

    fn collector_channel() -> (Channel<BrowseChannelMessage>, Collected) {
        let store: Collected = Arc::new(StdMutex::new(Vec::new()));
        let sink = store.clone();
        let channel = Channel::new(move |body: InvokeResponseBody| {
            if let InvokeResponseBody::Json(json) = body {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                    sink.lock().unwrap().push(value);
                }
            }
            Ok(())
        });
        (channel, store)
    }

    async fn wait_for(
        store: &Collected,
        timeout: Duration,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if store.lock().unwrap().iter().any(&pred) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    fn app_handle() -> (tauri::App<MockRuntime>, AppHandle<MockRuntime>) {
        let app = mock_app();
        let handle = app.handle().clone();
        (app, handle)
    }

    fn is_active_service(value: &serde_json::Value, name: &str, active: bool) -> bool {
        value
            .get("service")
            .map(|svc| {
                svc.get("name").and_then(|n| n.as_str()) == Some(name)
                    && svc.get("isActive").and_then(|a| a.as_bool()) == Some(active)
            })
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn browse_discovers_advertised_service_with_txt_and_goodbye() {
        if !network_tests_enabled() {
            eprintln!("skipping: set DNS_SD_NETWORK_TESTS=1 to run network tests");
            return;
        }
        let (_app, handle) = app_handle();
        let state = Arc::new(MdnsState::default());

        let mut txt = HashMap::new();
        txt.insert("path".into(), TxtRecordValue::BinaryData(b"/api".to_vec()));
        txt.insert("flag".into(), TxtRecordValue::BooleanFlag(true));
        txt.insert("empty".into(), TxtRecordValue::Null);

        let adv = advertise_start_impl(
            handle.clone(),
            state.clone(),
            AdvertiseOptions {
                service: AdvertiseServiceSpec {
                    name: "Rust E2E".into(),
                    type_name: "http".into(),
                    protocol: TransportProtocol::Tcp,
                    port: 8081,
                    host: None,
                    domain: None,
                    subtypes: Vec::new(),
                    txt,
                },
            },
        )
        .await
        .expect("advertise");

        let (channel, store) = collector_channel();
        let browse = browse_start_impl(
            handle.clone(),
            state.clone(),
            BrowseOptions {
                service: BrowseServiceSpec {
                    type_name: "http".into(),
                    protocol: TransportProtocol::Tcp,
                    domain: None,
                    subtypes: Vec::new(),
                },
                timeout_ms: Some(0),
            },
            channel,
        )
        .await
        .expect("browse");

        assert!(
            wait_for(&store, Duration::from_secs(20), |v| {
                is_active_service(v, "Rust E2E", true)
            })
            .await,
            "expected to discover the advertised service"
        );

        // TXT parity: path=bytes, flag=true, empty=null.
        {
            let guard = store.lock().unwrap();
            let resolved = guard
                .iter()
                .filter_map(|v| v.get("service"))
                .find(|s| s.get("name").and_then(|n| n.as_str()) == Some("Rust E2E"))
                .expect("resolved record");
            let txt = resolved.get("txt").expect("txt");
            assert!(
                txt.get("path").map(|p| p.is_array()).unwrap_or(false),
                "path bytes"
            );
            assert_eq!(txt.get("flag"), Some(&serde_json::Value::Bool(true)));
            assert_eq!(txt.get("empty"), Some(&serde_json::Value::Null));
        }

        // Goodbye: stopping the advertisement should surface a removal.
        advertise_stop_impl(handle.clone(), state.clone(), adv.advertise_id)
            .await
            .expect("advertise stop");

        assert!(
            wait_for(&store, Duration::from_secs(10), |v| {
                is_active_service(v, "Rust E2E", false)
            })
            .await,
            "expected a removal after goodbye"
        );

        browse_stop_impl(handle, state, browse.browse_id)
            .await
            .expect("browse stop");
    }

    #[tokio::test]
    async fn browse_timeout_emits_stopped() {
        if !network_tests_enabled() {
            eprintln!("skipping: set DNS_SD_NETWORK_TESTS=1 to run network tests");
            return;
        }
        let (_app, handle) = app_handle();
        let state = Arc::new(MdnsState::default());
        let (channel, store) = collector_channel();
        browse_start_impl(
            handle,
            state,
            BrowseOptions {
                service: BrowseServiceSpec {
                    type_name: "http".into(),
                    protocol: TransportProtocol::Tcp,
                    domain: None,
                    subtypes: Vec::new(),
                },
                timeout_ms: Some(200),
            },
            channel,
        )
        .await
        .expect("browse");

        assert!(
            wait_for(&store, Duration::from_secs(5), |v| {
                v.get("reason").and_then(|r| r.as_str()) == Some("timeout")
            })
            .await,
            "expected a timeout stop event"
        );
    }
}
