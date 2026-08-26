package com.momics.dnssd

import android.app.Activity
import android.content.Context
import android.net.InetAddresses
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Plugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Channel
import app.tauri.plugin.JSObject
import org.json.JSONArray
import org.json.JSONObject
import java.net.InetAddress
import java.util.concurrent.ConcurrentHashMap

@InvokeArg
class BrowseStartArgs {
    var service: ServiceSpecData? = null
    var timeoutMs: Long? = null
    lateinit var channel: Channel
}

@InvokeArg
class ServiceSpecData {
    var type: String? = null
    var protocol: String? = null
    var domain: String? = null
    // Accepted for API parity; NsdManager discovery does not filter by subtype.
    var subtypes: List<String>? = null
}

@InvokeArg
class BrowseStopArgs {
    var browseId: Long = 0
}

@InvokeArg
class AdvertiseStartArgs {
    var service: AdvertiseServiceSpec? = null
}

@InvokeArg
class AdvertiseServiceSpec {
    var name: String? = null
    var type: String? = null
    var protocol: String? = null
    var port: Long = 0
    var host: String? = null
    var domain: String? = null
    // Accepted for API parity; honoured only on Android 15+ (see advertise_start).
    var subtypes: List<String>? = null
    var txt: JSObject? = null
}

@InvokeArg
class AdvertiseStopArgs {
    var advertiseId: Long = 0
}

// Functional Android mDNS-SD plugin backed by NsdManager.
//
// Discovery, resolution and registration all use the OS NsdManager. Discovery
// events are streamed to JS over a Tauri Channel; the guest-js layer derives the
// unified `kind` (found/resolved/updated/removed) from the `isActive` flag and
// the presence of a resolved host/port.
//
// Resolution parity:
//  - On Android 14+ (API 34) discovery uses NsdManager.registerServiceInfoCallback,
//    which resolves each instance to ALL of its IP addresses and keeps them (and
//    the TXT record) live — matching the desktop `mdns-sd` and iOS `NetService`
//    backends, which also return every address.
//  - Below API 34 the deprecated resolveService is used, which resolves a single
//    address only (an OS limitation on those versions).
//
// Remaining NsdManager limitations (documented in the package README):
//  - Non-`local` domains are ignored (a warning is logged).
//  - A custom advertise `host` is honoured only when it is a numeric IP literal
//    on Android 14+ (via setHostAddresses); a custom host *name* is always chosen
//    by the OS.
//  - Subtypes are only honoured on advertise on Android 15+ (setSubtypes); the
//    installed compile SDK (34) predates that API, so they are accepted for API
//    parity but not registered.
//  - TXT attributes are exposed as a `Map<String, byte[]>`; a key with no value
//    is reported as the bare-key form (`true`) — NsdManager cannot distinguish a
//    bare key from an explicit empty value. On advertise, values are encoded as
//    UTF-8 (NsdManager's public setAttribute only accepts String values).
@TauriPlugin
class DnsSdPlugin(private val activity: Activity): Plugin(activity) {
    private var nextBrowseId: Long = 1L
    private var nextAdvertiseId: Long = 1L

    private data class BrowseSession(
        val id: Long,
        val manager: NsdManager,
        val listener: NsdManager.DiscoveryListener,
        val channel: Channel,
        val services: ConcurrentHashMap<String, NsdServiceInfo> = ConcurrentHashMap(),
        // Live resolution callbacks (API 34+), keyed by service key, for cleanup.
        val infoCallbacks: ConcurrentHashMap<String, NsdManager.ServiceInfoCallback> = ConcurrentHashMap(),
        val timeoutHandler: Handler? = null,
        val timeoutRunnable: Runnable? = null
    )
    private data class AdvertiseSession(
        val id: Long,
        val manager: NsdManager,
        val info: NsdServiceInfo,
        val listener: NsdManager.RegistrationListener
    )

    private val browseMap = ConcurrentHashMap<Long, BrowseSession>()
    private val advertiseMap = ConcurrentHashMap<Long, AdvertiseSession>()

    private fun allocBrowseId(): Long = nextBrowseId.also { nextBrowseId += 1 }
    private fun allocAdvertiseId(): Long = nextAdvertiseId.also { nextAdvertiseId += 1 }
    private fun nsd(): NsdManager? = activity.getSystemService(Context.NSD_SERVICE) as? NsdManager
    private fun nowMs(): Long = System.currentTimeMillis()

    // Collect every resolved IP address for a service, deduplicated and sorted so
    // the emitted list is stable and matches the desktop/iOS backends. Uses the
    // full address list on Android 14+, falling back to the single legacy host.
    @Suppress("DEPRECATION")
    private fun collectAddresses(info: NsdServiceInfo): List<String> {
        val out = LinkedHashSet<String>()
        if (Build.VERSION.SDK_INT >= 34) {
            for (addr in info.hostAddresses) {
                addr.hostAddress?.substringBefore('%')?.let { out.add(it) }
            }
        }
        if (out.isEmpty()) {
            info.host?.hostAddress?.substringBefore('%')?.let { out.add(it) }
        }
        return out.sorted()
    }

    @Suppress("DEPRECATION")
    private fun emitService(session: BrowseSession, info: NsdServiceInfo, isActive: Boolean) {
        val addresses = collectAddresses(info)
        val serviceType = info.serviceType.trimEnd('.').removeSuffix(".local")
        val qualifiedServiceType = "$serviceType.local."

        val serviceData = JSObject()
        serviceData.put("name", info.serviceName)
        serviceData.put("fullName", info.serviceName + "." + qualifiedServiceType)
        serviceData.put("host", info.host?.hostAddress?.substringBefore('%') ?: addresses.firstOrNull())
        serviceData.put("port", info.port)
        serviceData.put("serviceType", qualifiedServiceType)
        serviceData.put("protocol", if (serviceType.contains("_udp")) "udp" else "tcp")
        serviceData.put("domain", "local")

        val subtypesArray = JSONArray()
        serviceData.put("subtypes", subtypesArray)

        val addressesArray = JSONArray()
        for (address in addresses) addressesArray.put(address)
        serviceData.put("addresses", addressesArray)

        val txt = JSObject()
        for ((key, value) in info.attributes) {
            if (value == null || value.isEmpty()) {
                // NsdManager cannot distinguish a bare key from an empty value;
                // report the bare-key form (`true`), per the shared contract.
                txt.put(key, true)
            } else {
                val bytes = JSONArray()
                for (byte in value) {
                    bytes.put(byte.toInt() and 0xFF)
                }
                txt.put(key, bytes)
            }
        }
        serviceData.put("txt", txt)
        serviceData.put("isActive", isActive)
        serviceData.put("lastSeenMs", nowMs())

        val payload = JSObject()
        payload.put("browseId", session.id)
        payload.put("service", serviceData)

        try {
            session.channel.send(payload)
        } catch (e: Exception) {
            Log.e("dns-sd", "Error sending service event: ${e.message}")
        }
    }

    private fun emitBrowseStopped(session: BrowseSession, reason: String) {
        val payload = JSObject()
        payload.put("browseId", session.id)
        payload.put("reason", reason)
        try {
            session.channel.send(payload)
        } catch (e: Exception) {
            Log.e("dns-sd", "Error sending browse stopped event: ${e.message}")
        }
    }

    // Idempotent removal shared by the discovery listener and the resolution
    // callback (both can report a loss). Emits `removed` only on the first call.
    private fun handleLost(browseId: Long, key: String, fallback: NsdServiceInfo?) {
        val session = browseMap[browseId] ?: return
        val previous = session.services.remove(key)
        val callback = session.infoCallbacks.remove(key)
        callback?.let { cb ->
            try { session.manager.unregisterServiceInfoCallback(cb) } catch (_: Exception) {}
        }
        // Nothing left to remove -> the other path already handled it; don't
        // emit a duplicate `removed` event.
        if (previous == null && callback == null) return
        val info = previous ?: fallback ?: return
        emitService(session, info, false)
    }

    // Resolve + track a discovered instance on Android 14+, delivering all of its
    // addresses and any later TXT/address changes as `updated` events.
    private fun startInfoCallback(browseId: Long, key: String, serviceInfo: NsdServiceInfo) {
        val session = browseMap[browseId] ?: return
        if (session.infoCallbacks.containsKey(key)) return
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                Log.w("dns-sd", "ServiceInfoCallback registration failed ($errorCode) for $key")
                browseMap[browseId]?.infoCallbacks?.remove(key)
            }
            override fun onServiceUpdated(info: NsdServiceInfo) {
                val current = browseMap[browseId] ?: return
                current.services[key] = info
                emitService(current, info, true)
            }
            override fun onServiceLost() {
                handleLost(browseId, key, null)
            }
            override fun onServiceInfoCallbackUnregistered() {
                browseMap[browseId]?.infoCallbacks?.remove(key)
            }
        }
        session.infoCallbacks[key] = callback
        try {
            session.manager.registerServiceInfoCallback(serviceInfo, activity.mainExecutor, callback)
        } catch (e: Exception) {
            session.infoCallbacks.remove(key)
            Log.e("dns-sd", "registerServiceInfoCallback failed for $key: ${e.message}")
        }
    }

    private fun stopBrowseSession(session: BrowseSession, reason: String) {
        browseMap.remove(session.id)
        session.timeoutHandler?.let { handler ->
            session.timeoutRunnable?.let { handler.removeCallbacks(it) }
        }
        for (cb in session.infoCallbacks.values) {
            try { session.manager.unregisterServiceInfoCallback(cb) } catch (_: Exception) {}
        }
        session.infoCallbacks.clear()
        try {
            session.manager.stopServiceDiscovery(session.listener)
        } catch (_: Exception) {}
        emitBrowseStopped(session, reason)
    }

    @Command
    @Suppress("DEPRECATION")
    fun browse_start(invoke: Invoke) {
        val manager = nsd() ?: return invoke.reject("NsdManager unavailable")
        val args = invoke.parseArgs(BrowseStartArgs::class.java)
        val channel = args.channel
        val browseId = allocBrowseId()
        val typeRaw = args.service?.type
        val protoRaw = args.service?.protocol
        val serviceType = if (typeRaw != null && protoRaw != null) {
            val typeLabel = if (typeRaw.startsWith("_")) typeRaw else "_${typeRaw}"
            val protoLabel = if (protoRaw.lowercase() == "udp") "_udp" else "_tcp"
            "$typeLabel.$protoLabel"
        } else {
            // Default to http tcp if not specified
            "_http._tcp"
        }
        val timeoutMs = args.timeoutMs ?: 30_000L
        val domain = args.service?.domain
        if (!domain.isNullOrBlank() && domain.lowercase() != "local") {
            Log.w("dns-sd", "Android NSD ignores non-local domain '$domain'")
        }
        val listener = object : NsdManager.DiscoveryListener {
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                browseMap[browseId]?.let { stopBrowseSession(it, "error:$errorCode") }
            }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                browseMap[browseId]?.let { stopBrowseSession(it, "error:$errorCode") }
            }
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {
                browseMap[browseId]?.let { stopBrowseSession(it, "search-stopped") }
            }
            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                val key = serviceInfo.serviceName + serviceInfo.serviceType
                val session = browseMap[browseId] ?: return
                session.services[key] = serviceInfo
                if (Build.VERSION.SDK_INT >= 34) {
                    // Resolve + track all addresses and live changes.
                    startInfoCallback(browseId, key, serviceInfo)
                } else {
                    manager.resolveService(serviceInfo, object : NsdManager.ResolveListener {
                        override fun onServiceResolved(resolved: NsdServiceInfo) {
                            browseMap[browseId]?.let {
                                it.services[key] = resolved
                                emitService(it, resolved, true)
                            }
                        }
                        override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                            browseMap[browseId]?.let { emitService(it, serviceInfo, false) }
                        }
                    })
                }
            }
            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                val key = serviceInfo.serviceName + serviceInfo.serviceType
                handleLost(browseId, key, serviceInfo)
            }
        }
        val handler = if (timeoutMs > 0) Handler(Looper.getMainLooper()) else null
        val timeoutRunnable = if (timeoutMs > 0) Runnable {
            browseMap[browseId]?.let { stopBrowseSession(it, "timeout") }
        } else null
        val session = BrowseSession(browseId, manager, listener, channel, timeoutHandler = handler, timeoutRunnable = timeoutRunnable)
        browseMap[browseId] = session
        try {
            manager.discoverServices(serviceType, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            browseMap.remove(browseId)
            return invoke.reject("Discovery failed: ${e.message}")
        }
        if (timeoutMs > 0) {
            handler?.postDelayed(timeoutRunnable!!, timeoutMs)
        }
        val ret = JSObject()
        ret.put("browseId", browseId)
        invoke.resolve(ret)
    }

    @Command
    fun browse_stop(invoke: Invoke) {
        val args = invoke.parseArgs(BrowseStopArgs::class.java)
        val session = browseMap.remove(args.browseId)
        if (session != null) {
            session.timeoutHandler?.let { handler ->
                session.timeoutRunnable?.let { handler.removeCallbacks(it) }
            }
            for (cb in session.infoCallbacks.values) {
                try { session.manager.unregisterServiceInfoCallback(cb) } catch (_: Exception) {}
            }
            session.infoCallbacks.clear()
            try { session.manager.stopServiceDiscovery(session.listener) } catch (_: Exception) {}
            emitBrowseStopped(session, "stopped")
        }
        invoke.resolve()
    }

    @Command
    fun advertise_start(invoke: Invoke) {
        val manager = nsd() ?: return invoke.reject("NsdManager unavailable")
        val args = invoke.parseArgs(AdvertiseStartArgs::class.java)
        val name = args.service?.name ?: return invoke.reject("Missing service.name")
        val typeRaw = args.service?.type ?: return invoke.reject("Missing service.type")
        val protoRaw = args.service?.protocol ?: "tcp"
        val port = args.service?.port?.toInt() ?: -1
        if (port <= 0) return invoke.reject("Invalid port")
        args.service?.domain?.let {
            if (it.lowercase() != "local") {
                Log.w("dns-sd", "Android NSD ignores non-local domain '$it' for advertise_start")
            }
        }
        val typeLabel = if (typeRaw.startsWith("_")) typeRaw else "_${typeRaw}"
        val protoLabel = if (protoRaw.lowercase() == "udp") "_udp" else "_tcp"
        val serviceType = "$typeLabel.$protoLabel"
        val info = NsdServiceInfo().apply {
            serviceName = name
            this.serviceType = serviceType
            setPort(port)
        }
        // Honour a custom host only when it is a numeric IP literal on Android 14+,
        // where setHostAddresses lets us advertise explicit A/AAAA records. A custom
        // host *name* is always chosen by the OS and cannot be overridden.
        args.service?.host?.let { host ->
            val applied = if (host.isNotBlank() && Build.VERSION.SDK_INT >= 34 && InetAddresses.isNumericAddress(host)) {
                info.setHostAddresses(listOf(InetAddresses.parseNumericAddress(host)))
                true
            } else false
            if (!applied) {
                Log.w("dns-sd", "Android NSD ignores custom host '$host' (only a numeric IP on Android 14+ is honoured)")
            }
        }
        args.service?.txt?.let { txt ->
            val keys = txt.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                val value = txt.opt(key)
                // NsdManager's public setAttribute only accepts String values, so
                // byte payloads are encoded as UTF-8. A null value is a bare key.
                when (value) {
                    null, JSONObject.NULL -> info.setAttribute(key, null as String?)
                    is JSONArray -> {
                        val bytes = ByteArray(value.length()) { idx ->
                            (value.optInt(idx, 0) and 0xFF).toByte()
                        }
                        info.setAttribute(key, String(bytes, Charsets.UTF_8))
                    }
                    is Boolean -> if (value) info.setAttribute(key, null as String?)
                    else -> info.setAttribute(key, value.toString())
                }
            }
        }
        val advertiseId = allocAdvertiseId()
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(serviceInfo: NsdServiceInfo) {}
            override fun onRegistrationFailed(serviceInfo: NsdServiceInfo, errorCode: Int) { /* Could emit error */ }
            override fun onServiceUnregistered(serviceInfo: NsdServiceInfo) {}
            override fun onUnregistrationFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {}
        }
        advertiseMap[advertiseId] = AdvertiseSession(advertiseId, manager, info, listener)
        try { manager.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener) } catch (e: Exception) {
            advertiseMap.remove(advertiseId)
            return invoke.reject("Registration failed: ${e.message}")
        }
        val ret = JSObject()
        ret.put("advertiseId", advertiseId)
        ret.put("name", name)
        invoke.resolve(ret)
    }

    @Command
    fun advertise_stop(invoke: Invoke) {
        val args = invoke.parseArgs(AdvertiseStopArgs::class.java)
        val session = advertiseMap.remove(args.advertiseId)
        if (session != null) {
            try { session.manager.unregisterService(session.listener) } catch (_: Exception) {}
        }
        invoke.resolve()
    }
}
