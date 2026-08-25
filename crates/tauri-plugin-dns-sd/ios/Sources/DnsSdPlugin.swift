import Foundation
import Network
import Tauri

private func debugLog(_ message: String) {
    #if DEBUG
    NSLog("%@", message)
    #endif
}

// MARK: - Argument Types
struct BrowseStartArgs: Decodable {
    struct ServiceSpec: Decodable {
        let type: String
        let `protocol`: String
        let domain: String?
    }
    let service: ServiceSpec
    let timeoutMs: UInt64?
    let channel: Channel
}

struct BrowseStopArgs: Decodable {
    let browseId: UInt64
}

struct AdvertiseStartArgs: Decodable {
    struct ServiceSpec: Decodable {
        let name: String
        let type: String
        let `protocol`: String
        let port: Int
        let domain: String?
        let txt: [String: TxtValue]?
    }
    let service: ServiceSpec
}

enum TxtValue: Decodable {
    case bool(Bool)
    case bytes([UInt8])
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let b = try? container.decode(Bool.self) {
            self = .bool(b)
        } else if let bytes = try? container.decode([UInt8].self) {
            self = .bytes(bytes)
        } else {
            self = .null
        }
    }
}

struct AdvertiseStopArgs: Decodable {
    let advertiseId: UInt64
}

/**
 * Resolves a discovered Bonjour instance to its host name, port and IP
 * addresses via `NetService`.
 *
 * `NWBrowser` (used for discovery) only yields an opaque service endpoint and
 * never surfaces host/port/addresses — Apple's Network.framework is designed to
 * resolve lazily inside an `NWConnection`. To reach the same "resolved" fidelity
 * the desktop (`mdns-sd`) and Android (`NsdManager`) backends provide, we run
 * the discovered `name`/`type`/`domain` back through the classic Bonjour
 * resolution API. `NetService` is delegate/run-loop based, so it is scheduled on
 * the main run loop; callbacks are marshalled back to the plugin's queue.
 */
final class ServiceResolver: NSObject, NetServiceDelegate {
    let key: String
    private let netService: NetService
    private let timeout: TimeInterval
    private let onResolved: (String, String?, Int?, [String]) -> Void
    private let onFailed: (String) -> Void
    private var settled = false

    init(
        key: String,
        domain: String,
        type: String,
        name: String,
        timeout: TimeInterval,
        onResolved: @escaping (String, String?, Int?, [String]) -> Void,
        onFailed: @escaping (String) -> Void
    ) {
        self.key = key
        self.timeout = timeout
        self.onResolved = onResolved
        self.onFailed = onFailed
        self.netService = NetService(domain: domain, type: type, name: name)
        super.init()
        self.netService.delegate = self
    }

    func start() {
        DispatchQueue.main.async {
            self.netService.schedule(in: .main, forMode: .common)
            self.netService.resolve(withTimeout: self.timeout)
        }
    }

    func cancel() {
        DispatchQueue.main.async {
            self.netService.stop()
            self.netService.remove(from: .main, forMode: .common)
        }
    }

    func netServiceDidResolveAddress(_ sender: NetService) {
        if settled { return }
        settled = true
        let addresses = ServiceResolver.parseAddresses(sender.addresses)
        let host = sender.hostName
        let port = sender.port >= 0 ? sender.port : nil
        sender.stop()
        sender.remove(from: .main, forMode: .common)
        onResolved(key, host, port, addresses)
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        if settled { return }
        settled = true
        sender.remove(from: .main, forMode: .common)
        onFailed(key)
    }

    /** Turn `NetService.addresses` (raw `sockaddr` blobs) into numeric IP strings. */
    static func parseAddresses(_ data: [Data]?) -> [String] {
        guard let data else { return [] }
        var out = Set<String>()
        for datum in data {
            datum.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
                guard let base = raw.baseAddress, raw.count >= MemoryLayout<sockaddr>.size else {
                    return
                }
                let sa = base.assumingMemoryBound(to: sockaddr.self)
                var hostBuf = [CChar](repeating: 0, count: Int(NI_MAXHOST))
                let status = getnameinfo(
                    sa,
                    socklen_t(datum.count),
                    &hostBuf,
                    socklen_t(hostBuf.count),
                    nil,
                    0,
                    NI_NUMERICHOST
                )
                if status == 0 {
                    let text = String(cString: hostBuf)
                    // Drop any IPv6 zone/scope suffix (e.g. "fe80::1%en0").
                    let cleaned = text.split(separator: "%").first.map(String.init) ?? text
                    out.insert(cleaned)
                }
            }
        }
        return out.sorted()
    }
}

@objc(DnsSdPlugin)
class DnsSdPlugin: Plugin {
    private var nextBrowseId: UInt64 = 1
    private var nextAdvertiseId: UInt64 = 1

    private struct ServiceSnapshot {
        let key: String
        let signature: String
        let payload: JSObject
    }

    private final class BrowseSession {
        let id: UInt64
        let browser: NWBrowser
        let channel: Channel
        var services: [String: ServiceSnapshot] = [:]
        var timeoutWorkItem: DispatchWorkItem?
        /** Host/port/addresses obtained by resolving a discovered instance. */
        var resolved: [String: ResolvedInfo] = [:]
        /** In-flight resolutions, keyed by instance full name (also used to cancel). */
        var resolvers: [String: ServiceResolver] = [:]

        struct ResolvedInfo {
            let host: String?
            let port: Int?
            let addresses: [String]
        }

        init(id: UInt64, browser: NWBrowser, channel: Channel) {
            self.id = id
            self.browser = browser
            self.channel = channel
        }
    }

    private final class AdvertiseSession {
        let id: UInt64
        let listener: NWListener

        init(id: UInt64, listener: NWListener) {
            self.id = id
            self.listener = listener
        }
    }

    private let sessionQueue = DispatchQueue(label: "com.momics.dnssd.session")
    private var browseSessions: [UInt64: BrowseSession] = [:]
    private var advertiseSessions: [UInt64: AdvertiseSession] = [:]

    // MARK: - Utility
    private func nowMs() -> UInt64 { UInt64(Date().timeIntervalSince1970 * 1000) }
    private func allocBrowseId() -> UInt64 { defer { nextBrowseId += 1 }; return nextBrowseId }
    private func allocAdvertiseId() -> UInt64 { defer { nextAdvertiseId += 1 }; return nextAdvertiseId }

    private func normalizeDomain(_ domain: String) -> String {
        let trimmed = domain.trimmingCharacters(in: CharacterSet(charactersIn: ".")).lowercased()
        return trimmed.isEmpty ? "local" : trimmed
    }

    private func browserDomain(_ domain: String?) -> String? {
        guard let domain else { return nil }
        let normalized = normalizeDomain(domain)
        return normalized == "local" ? nil : normalized
    }

    private func listenerDomain(_ domain: String?) -> String? {
        guard let domain else { return nil }
        let normalized = normalizeDomain(domain)
        return normalized == "local" ? nil : normalized
    }

    private func parseTxtRecordData(_ data: Data?) -> JSObject {
        guard let data, !data.isEmpty else { return JSObject() }
        var result = JSObject()
        var idx = 0
        let bytes = [UInt8](data)
        while idx < bytes.count {
            let len = Int(bytes[idx])
            idx += 1
            if len == 0 || idx + len > bytes.count { break }
            let slice = bytes[idx..<(idx + len)]
            idx += len
            if let eqIdx = slice.firstIndex(of: UInt8(ascii: "=")) {
                let keyBytes = slice[..<eqIdx]
                let valBytes = slice[(eqIdx + 1)...]
                if let key = String(bytes: keyBytes, encoding: .utf8) {
                    if valBytes.isEmpty {
                        // `key=` with an empty value → null (per shared contract).
                        result[key] = NSNull()
                    } else {
                        result[key] = Array(valBytes)
                    }
                }
            } else if let key = String(bytes: slice, encoding: .utf8) {
                // A bare key with no `=` → true.
                result[key] = true
            }
        }
        return result
    }

    private func encodeTxtData(_ txt: [String: TxtValue]?) -> Data? {
        guard let txt else { return nil }
        var result = Data()
        for (key, value) in txt {
            guard !key.isEmpty else { continue }
            guard var keyBytes = key.data(using: .utf8) else { continue }

            switch value {
            case .bool(let flag):
                if !flag { continue }
                break
            case .null:
                // `key=` with an empty value distinguishes null from a bare-key flag.
                keyBytes.append(UInt8(ascii: "="))
            case .bytes(let bytes):
                keyBytes.append(UInt8(ascii: "="))
                keyBytes.append(contentsOf: bytes)
            }

            if keyBytes.count > UInt8.max {
                continue
            }

            result.append(UInt8(keyBytes.count))
            result.append(keyBytes)
        }
        return result.isEmpty ? nil : result
    }

    private func emitBrowseStopped(session: BrowseSession, reason: String) {
        var eventData = JSObject()
        eventData["browseId"] = Int(session.id)
        eventData["reason"] = reason
        do {
            try session.channel.send(eventData)
        } catch {
            debugLog("[dns-sd] failed to emit browse stop event: \(error)")
        }
    }

    private func emitBrowseService(session: BrowseSession, payload: JSObject) {
        var eventData = JSObject()
        eventData["browseId"] = Int(session.id)
        eventData["service"] = payload
        do {
            try session.channel.send(eventData)
        } catch {
            debugLog("[dns-sd] failed to emit browse service event: \(error)")
        }
    }

    private func makeSnapshot(from result: NWBrowser.Result) -> ServiceSnapshot? {
        guard case let .service(name, type, domain, _) = result.endpoint else {
            return nil
        }

        let normalized = normalizeDomain(domain)
        let fullName = "\(name).\(type).\(normalized)."
        let serviceType = "\(type).\(normalized)."
        let protocolName = type.lowercased().contains("_udp") ? "udp" : "tcp"

        var txt = JSObject()
        if case let .bonjour(txtRecord) = result.metadata {
            txt = parseTxtRecordData(txtRecord.data)
        }

        let txtState = txt.keys.sorted().map { key in
            "\(key):\(String(describing: txt[key]))"
        }.joined(separator: ";")

        let signature = "\(fullName)|\(txtState)"

        var payload = JSObject()
        payload["name"] = name
        payload["fullName"] = fullName
        // Unresolved until the OS resolver returns a host/port/addresses. Emit
        // explicit JSON `null` (not an absent key) so the guest-js binding
        // classifies this as `found` rather than a resolved event.
        payload["host"] = NSNull()
        payload["port"] = NSNull()
        payload["serviceType"] = serviceType
        payload["protocol"] = protocolName
        payload["domain"] = normalized
        payload["subtypes"] = []
        payload["addresses"] = []
        payload["txt"] = txt
        payload["isActive"] = true
        payload["lastSeenMs"] = Int64(nowMs())

        return ServiceSnapshot(key: fullName, signature: signature, payload: payload)
    }

    private func handleBrowseResults(browseId: UInt64, results: Set<NWBrowser.Result>) {
        guard let session = browseSessions[browseId] else { return }

        var nextServices: [String: ServiceSnapshot] = [:]
        for result in results {
            guard var snapshot = makeSnapshot(from: result) else { continue }
            // Fold in any host/port/addresses we've already resolved for this
            // instance so a periodic browse refresh never reverts it to `found`.
            if let info = session.resolved[snapshot.key] {
                snapshot = applyResolved(to: snapshot, info)
            }
            nextServices[snapshot.key] = snapshot

            let shouldEmit = session.services[snapshot.key]?.signature != snapshot.signature
            if shouldEmit {
                emitBrowseService(session: session, payload: snapshot.payload)
            }

            // Kick off native resolution once per instance to fill host/port/
            // addresses. `NWBrowser` yields an opaque endpoint, so we resolve it
            // through `NetService` (Bonjour) to reach desktop/Android parity.
            if session.resolved[snapshot.key] == nil,
               session.resolvers[snapshot.key] == nil,
               case let .service(name, type, domain, _) = result.endpoint {
                startResolving(
                    browseId: browseId,
                    key: snapshot.key,
                    name: name,
                    type: type,
                    domain: domain
                )
            }
        }

        for (key, previous) in session.services where nextServices[key] == nil {
            var removedPayload = previous.payload
            removedPayload["isActive"] = false
            removedPayload["lastSeenMs"] = Int64(nowMs())
            emitBrowseService(session: session, payload: removedPayload)
            // Drop resolution state for departed instances.
            session.resolved.removeValue(forKey: key)
            session.resolvers.removeValue(forKey: key)?.cancel()
        }

        session.services = nextServices
    }

    /** Merge resolved host/port/addresses into a snapshot, refreshing its signature. */
    private func applyResolved(
        to snapshot: ServiceSnapshot,
        _ info: BrowseSession.ResolvedInfo
    ) -> ServiceSnapshot {
        var payload = snapshot.payload
        payload["host"] = info.host.map { $0 as Any } ?? NSNull()
        payload["port"] = info.port.map { $0 as Any } ?? NSNull()
        payload["addresses"] = info.addresses
        let addrState = info.addresses.joined(separator: ",")
        let signature = "\(snapshot.signature)|\(info.host ?? "")|\(info.port ?? -1)|\(addrState)"
        return ServiceSnapshot(key: snapshot.key, signature: signature, payload: payload)
    }

    /** Start resolving a discovered instance via `NetService` (on the main run loop). */
    private func startResolving(
        browseId: UInt64,
        key: String,
        name: String,
        type: String,
        domain: String
    ) {
        guard let session = browseSessions[browseId] else { return }
        let nsType = type.hasSuffix(".") ? type : type + "."
        let nsDomain = domain.isEmpty
            ? "local."
            : (domain.hasSuffix(".") ? domain : domain + ".")

        let resolver = ServiceResolver(
            key: key,
            domain: nsDomain,
            type: nsType,
            name: name,
            timeout: 5.0,
            onResolved: { [weak self] key, host, port, addresses in
                self?.sessionQueue.async {
                    self?.applyResolution(
                        browseId: browseId,
                        key: key,
                        host: host,
                        port: port,
                        addresses: addresses
                    )
                }
            },
            onFailed: { [weak self] key in
                self?.sessionQueue.async {
                    self?.browseSessions[browseId]?.resolvers.removeValue(forKey: key)
                }
            }
        )
        session.resolvers[key] = resolver
        resolver.start()
    }

    /** Apply a completed resolution: cache it and emit a `resolved` snapshot. */
    private func applyResolution(
        browseId: UInt64,
        key: String,
        host: String?,
        port: Int?,
        addresses: [String]
    ) {
        guard let session = browseSessions[browseId] else { return }
        session.resolvers.removeValue(forKey: key)
        // Ignore results for an instance that departed while resolving.
        guard let base = session.services[key] else { return }

        let info = BrowseSession.ResolvedInfo(host: host, port: port, addresses: addresses)
        session.resolved[key] = info

        let resolvedSnapshot = applyResolved(to: base, info)
        if session.services[key]?.signature != resolvedSnapshot.signature {
            session.services[key] = resolvedSnapshot
            emitBrowseService(session: session, payload: resolvedSnapshot.payload)
        }
    }

    private func stopBrowseSession(browseId: UInt64, reason: String) {
        guard let session = browseSessions.removeValue(forKey: browseId) else { return }
        session.timeoutWorkItem?.cancel()
        for (_, resolver) in session.resolvers { resolver.cancel() }
        session.resolvers.removeAll()
        session.browser.cancel()
        emitBrowseStopped(session: session, reason: reason)
        debugLog("[dns-sd] stopped browse \(browseId): \(reason)")
    }

    // MARK: - Commands
    @objc public func browse_start(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(BrowseStartArgs.self)

        let browseId = allocBrowseId()
        let typeLabel = args.service.type.hasPrefix("_") ? args.service.type : "_\(args.service.type)"
        let protoLabel = args.service.protocol.lowercased() == "udp" ? "_udp" : "_tcp"
        let serviceType = "\(typeLabel).\(protoLabel)"
        let params = args.service.protocol.lowercased() == "udp" ? NWParameters.udp : NWParameters.tcp

        let descriptor = NWBrowser.Descriptor.bonjourWithTXTRecord(
            type: serviceType,
            domain: browserDomain(args.service.domain)
        )
        let browser = NWBrowser(for: descriptor, using: params)
        let session = BrowseSession(id: browseId, browser: browser, channel: args.channel)

        browser.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            self.sessionQueue.async {
                switch state {
                case .failed(let error):
                    self.stopBrowseSession(browseId: browseId, reason: "error:\(error.localizedDescription)")
                case .cancelled:
                    self.stopBrowseSession(browseId: browseId, reason: "search-stopped")
                default:
                    break
                }
            }
        }

        browser.browseResultsChangedHandler = { [weak self] latestResults, _ in
            guard let self else { return }
            self.sessionQueue.async {
                self.handleBrowseResults(browseId: browseId, results: latestResults)
            }
        }

        sessionQueue.async {
            self.browseSessions[browseId] = session
            let timeoutMs = args.timeoutMs ?? 30_000
            if timeoutMs > 0 {
                let workItem = DispatchWorkItem { [weak self] in
                    self?.sessionQueue.async {
                        self?.stopBrowseSession(browseId: browseId, reason: "timeout")
                    }
                }
                session.timeoutWorkItem = workItem
                self.sessionQueue.asyncAfter(deadline: .now() + .milliseconds(Int(timeoutMs)), execute: workItem)
            }
            browser.start(queue: self.sessionQueue)
            debugLog("[dns-sd] started browse \(browseId) for \(serviceType)")
        }

        invoke.resolve(["browseId": browseId])
    }

    @objc public func browse_stop(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(BrowseStopArgs.self)
        sessionQueue.async {
            self.stopBrowseSession(browseId: args.browseId, reason: "stopped")
        }
        invoke.resolve()
    }

    @objc public func advertise_start(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(AdvertiseStartArgs.self)
        let name = args.service.name
        let typeRaw = args.service.type
        let protoRaw = args.service.protocol
        let port = args.service.port

        if port < 0 || port > 65535 {
            invoke.reject("Invalid port")
            return
        }

        let typeLabel = typeRaw.hasPrefix("_") ? typeRaw : "_\(typeRaw)"
        let protoLabel = protoRaw.lowercased() == "udp" ? "_udp" : "_tcp"
        let serviceType = "\(typeLabel).\(protoLabel)"
        let advertiseId = allocAdvertiseId()

        guard let nwPort = NWEndpoint.Port(rawValue: UInt16(port)) else {
            invoke.reject("Invalid port")
            return
        }

        let parameters = protoRaw.lowercased() == "udp" ? NWParameters.udp : NWParameters.tcp
        let listener: NWListener
        do {
            listener = try NWListener(using: parameters, on: nwPort)
        } catch {
            invoke.reject("Failed to create listener: \(error.localizedDescription)")
            return
        }

        listener.service = NWListener.Service(
            name: name,
            type: serviceType,
            domain: listenerDomain(args.service.domain),
            txtRecord: encodeTxtData(args.service.txt)
        )

        listener.newConnectionHandler = { connection in
            // This plugin only advertises services; it does not accept app-level traffic.
            connection.cancel()
        }

        listener.stateUpdateHandler = { state in
            switch state {
            case .failed(let error):
                debugLog("[dns-sd] advertisement \(advertiseId) failed: \(error.localizedDescription)")
            case .ready:
                debugLog("[dns-sd] advertisement \(advertiseId) is ready")
            default:
                break
            }
        }

        sessionQueue.async {
            self.advertiseSessions[advertiseId] = AdvertiseSession(id: advertiseId, listener: listener)
            listener.start(queue: self.sessionQueue)
        }

        invoke.resolve(["advertiseId": advertiseId, "name": name])
    }

    @objc public func advertise_stop(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(AdvertiseStopArgs.self)
        sessionQueue.async {
            guard let session = self.advertiseSessions.removeValue(forKey: args.advertiseId) else {
                return
            }
            session.listener.cancel()
            debugLog("[dns-sd] stopped advertisement \(args.advertiseId)")
        }
        invoke.resolve()
    }
}

// Export initializer symbol expected by Tauri runtime for this plugin.
@_cdecl("init_plugin_dns_sd")
func initPlugin() -> Plugin {
    return DnsSdPlugin()
}
