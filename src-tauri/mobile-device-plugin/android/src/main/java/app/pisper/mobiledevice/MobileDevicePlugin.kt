package app.pisper.mobiledevice

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Looper
import android.provider.ContactsContract
import android.provider.MediaStore
import android.provider.Settings
import android.util.Base64
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import app.tauri.PermissionState
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream
import java.util.concurrent.Executors
import kotlin.math.max
import kotlin.math.roundToInt

private const val CONTACTS = "contacts"
private const val CAMERA = "camera"
private const val LOCATION = "location"
private const val LOCAL_NETWORK = "localNetwork"
private const val EXTERNAL_APPS = "externalApps"
private val PHONE_NUMBER = Regex("^[+*#0-9(). \\-]{1,64}$")
private val PACKAGE_NAME = Regex("^[A-Za-z][A-Za-z0-9_]*(?:\\.[A-Za-z][A-Za-z0-9_]*)+$")
private val FORBIDDEN_APP_SCHEMES = setOf(
    "about", "content", "data", "file", "http", "https", "intent", "javascript"
)

@InvokeArg
class PermissionArgs {
    var capability: String = ""
}

@InvokeArg
class OperationParameters {
    var query: String? = null
    var limit: Int? = null
    var cameraDirection: String? = null
    var url: String? = null
    var latitude: Double? = null
    var longitude: Double? = null
    var label: String? = null
    var phoneNumber: String? = null
    var message: String? = null
    var packageName: String? = null
    var appUrl: String? = null
}

@InvokeArg
class OperationArgs {
    var operation: String = ""
    var parameters: OperationParameters? = null
}

@TauriPlugin(
    permissions = [
        Permission(strings = [Manifest.permission.READ_CONTACTS], alias = CONTACTS),
        Permission(strings = [Manifest.permission.CAMERA], alias = CAMERA),
        Permission(
            strings = [Manifest.permission.ACCESS_COARSE_LOCATION, Manifest.permission.ACCESS_FINE_LOCATION],
            alias = LOCATION
        ),
        Permission(strings = ["android.permission.ACCESS_LOCAL_NETWORK"], alias = LOCAL_NETWORK)
    ]
)
class MobileDevicePlugin(private val activity: Activity) : Plugin(activity) {
    private val worker = Executors.newSingleThreadExecutor()

    private fun state(alias: String): String = getPermissionState(alias).toString().lowercase()

    private fun supportsLocalNetworkPermission(): Boolean {
        if (Build.VERSION.SDK_INT < 36) return false
        return try {
            activity.packageManager.getPermissionInfo(
                "android.permission.ACCESS_LOCAL_NETWORK",
                0,
            )
            true
        } catch (_: PackageManager.NameNotFoundException) {
            // 部分 Android 发行版虽是 API 36，却尚未注册该权限；此时不能把兼容模式误报为拒绝。
            false
        }
    }

    private fun permissionResult(capability: String): JSObject = JSObject().apply {
        put("capability", capability)
        put("state", state(capability))
    }

    private fun localNetworkState(): String =
        if (!supportsLocalNetworkPermission()) "granted" else state(LOCAL_NETWORK)

    private fun localNetworkPermissionResult(): JSObject = JSObject().apply {
        put("capability", LOCAL_NETWORK)
        put("state", localNetworkState())
    }

    @Command
    fun permissionStates(invoke: Invoke) {
        invoke.resolve(JSObject().apply {
            put(CONTACTS, state(CONTACTS))
            put(CAMERA, state(CAMERA))
            put(LOCATION, state(LOCATION))
            put(LOCAL_NETWORK, localNetworkState())
            put(EXTERNAL_APPS, "not-required")
        })
    }

    @Command
    fun requestPermission(invoke: Invoke) {
        when (val capability = invoke.parseArgs(PermissionArgs::class.java).capability) {
            CONTACTS -> requestPermissionForAlias(CONTACTS, invoke, "contactsPermissionCallback")
            CAMERA -> requestPermissionForAlias(CAMERA, invoke, "cameraPermissionCallback")
            LOCATION -> requestPermissionForAlias(LOCATION, invoke, "locationPermissionCallback")
            LOCAL_NETWORK -> {
                if (!supportsLocalNetworkPermission()) {
                    invoke.resolve(localNetworkPermissionResult())
                } else {
                    requestPermissionForAlias(LOCAL_NETWORK, invoke, "localNetworkPermissionCallback")
                }
            }
            else -> invoke.reject("Unsupported mobile capability: $capability")
        }
    }

    @PermissionCallback
    fun contactsPermissionCallback(invoke: Invoke) = invoke.resolve(permissionResult(CONTACTS))

    @PermissionCallback
    fun cameraPermissionCallback(invoke: Invoke) = invoke.resolve(permissionResult(CAMERA))

    @PermissionCallback
    fun locationPermissionCallback(invoke: Invoke) = invoke.resolve(permissionResult(LOCATION))

    @PermissionCallback
    fun localNetworkPermissionCallback(invoke: Invoke) =
        invoke.resolve(localNetworkPermissionResult())

    @Command
    fun openAppSettings(invoke: Invoke) {
        val intent = Intent(
            Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
            Uri.fromParts("package", activity.packageName, null)
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        startActivityForResult(invoke, intent, "openSettingsResult")
    }

    @ActivityCallback
    @Suppress("UNUSED_PARAMETER")
    private fun openSettingsResult(invoke: Invoke, result: ActivityResult) = invoke.resolve()

    @Command
    fun execute(invoke: Invoke) {
        val args = invoke.parseArgs(OperationArgs::class.java)
        when (args.operation) {
            "contacts.search" -> searchContacts(invoke, args.parameters)
            "camera.capture" -> capturePhoto(invoke, args.parameters)
            "location.current" -> currentLocation(invoke)
            "apps.open_url" -> openUrl(invoke, args.parameters)
            "apps.open_map" -> openMap(invoke, args.parameters)
            "apps.open_system_settings" -> openSystemSettings(invoke)
            "apps.open_dialer" -> openDialer(invoke, args.parameters)
            "apps.compose_sms" -> composeSms(invoke, args.parameters)
            "apps.open_app" -> openApp(invoke, args.parameters)
            else -> invoke.reject("Unsupported mobile operation: ${args.operation}")
        }
    }

    private fun launchIntent(invoke: Invoke, operation: String, target: String, intent: Intent) {
        activity.runOnUiThread {
            try {
                activity.startActivity(intent)
                invoke.resolve(JSObject().apply {
                    put("opened", true)
                    put("operation", operation)
                    put("target", target)
                })
            } catch (error: ActivityNotFoundException) {
                invoke.reject(error.message ?: "No compatible application is installed")
            } catch (error: SecurityException) {
                invoke.reject(error.message ?: "Android blocked the external application")
            }
        }
    }

    private fun openUrl(invoke: Invoke, parameters: OperationParameters?) {
        val value = parameters?.url?.trim().orEmpty()
        val uri = Uri.parse(value)
        if (value.length > 2_048 || uri.scheme?.lowercase() !in setOf("http", "https") || uri.host.isNullOrBlank()) {
            invoke.reject("Only valid HTTP or HTTPS URLs can be opened")
            return
        }
        launchIntent(invoke, "apps.open_url", uri.toString(), Intent(Intent.ACTION_VIEW, uri))
    }

    private fun openMap(invoke: Invoke, parameters: OperationParameters?) {
        val latitude = parameters?.latitude
        val longitude = parameters?.longitude
        if (latitude == null || !latitude.isFinite() || latitude !in -90.0..90.0 ||
            longitude == null || !longitude.isFinite() || longitude !in -180.0..180.0
        ) {
            invoke.reject("Map coordinates are invalid")
            return
        }
        val label = parameters.label?.trim().orEmpty()
        if (label.length > 120) {
            invoke.reject("Map label is too long")
            return
        }
        val query = if (label.isEmpty()) "$latitude,$longitude" else "$latitude,$longitude ($label)"
        val uri = Uri.parse("geo:$latitude,$longitude?q=${Uri.encode(query)}")
        launchIntent(invoke, "apps.open_map", "$latitude,$longitude", Intent(Intent.ACTION_VIEW, uri))
    }

    private fun openSystemSettings(invoke: Invoke) {
        launchIntent(
            invoke,
            "apps.open_system_settings",
            "system",
            Intent(Settings.ACTION_SETTINGS)
        )
    }

    private fun openDialer(invoke: Invoke, parameters: OperationParameters?) {
        val number = parameters?.phoneNumber?.trim().orEmpty()
        if (!PHONE_NUMBER.matches(number)) {
            invoke.reject("Phone number is invalid")
            return
        }
        val uri = Uri.fromParts("tel", number, null)
        launchIntent(invoke, "apps.open_dialer", number, Intent(Intent.ACTION_DIAL, uri))
    }

    private fun composeSms(invoke: Invoke, parameters: OperationParameters?) {
        val number = parameters?.phoneNumber?.trim().orEmpty()
        if (!PHONE_NUMBER.matches(number)) {
            invoke.reject("Phone number is invalid")
            return
        }
        val message = parameters?.message.orEmpty()
        if (message.length > 2_000) {
            invoke.reject("SMS draft is too long")
            return
        }
        val intent = Intent(Intent.ACTION_SENDTO, Uri.fromParts("smsto", number, null))
        message.takeIf { it.isNotEmpty() }?.let { intent.putExtra("sms_body", it) }
        launchIntent(invoke, "apps.compose_sms", number, intent)
    }

    private fun openApp(invoke: Invoke, parameters: OperationParameters?) {
        val packageName = parameters?.packageName?.trim().orEmpty()
        if (packageName.length > 200 || !PACKAGE_NAME.matches(packageName)) {
            invoke.reject("Android application package name is invalid")
            return
        }
        val appUrl = parameters?.appUrl?.trim().orEmpty()
        val intent = if (appUrl.isNotEmpty()) {
            val uri = Uri.parse(appUrl)
            val scheme = uri.scheme?.lowercase()
            if (appUrl.length > 2_048 || scheme.isNullOrBlank() || scheme in FORBIDDEN_APP_SCHEMES) {
                invoke.reject("Application URL scheme is invalid")
                return
            }
            Intent(Intent.ACTION_VIEW, uri).setPackage(packageName)
        } else {
            Intent(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .setPackage(packageName)
        }
        launchIntent(invoke, "apps.open_app", packageName, intent)
    }

    private fun requirePermission(alias: String, invoke: Invoke): Boolean {
        if (getPermissionState(alias) == PermissionState.GRANTED) return true
        invoke.reject("Pisper does not have $alias permission")
        return false
    }

    private fun searchContacts(invoke: Invoke, parameters: OperationParameters?) {
        if (!requirePermission(CONTACTS, invoke)) return
        val query = parameters?.query?.trim().orEmpty()
        val limit = (parameters?.limit ?: 50).coerceIn(1, 200)
        worker.execute {
            try {
                val projection = arrayOf(
                    ContactsContract.CommonDataKinds.Phone.CONTACT_ID,
                    ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME,
                    ContactsContract.CommonDataKinds.Phone.NUMBER
                )
                val selection = if (query.isEmpty()) null else
                    "${ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME} LIKE ? OR ${ContactsContract.CommonDataKinds.Phone.NUMBER} LIKE ?"
                val selectionArgs = if (query.isEmpty()) null else arrayOf("%$query%", "%$query%")
                val contacts = linkedMapOf<Long, Pair<String, MutableSet<String>>>()
                activity.contentResolver.query(
                    ContactsContract.CommonDataKinds.Phone.CONTENT_URI,
                    projection,
                    selection,
                    selectionArgs,
                    "${ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME} COLLATE NOCASE ASC"
                )?.use { cursor ->
                    val idColumn = cursor.getColumnIndexOrThrow(ContactsContract.CommonDataKinds.Phone.CONTACT_ID)
                    val nameColumn = cursor.getColumnIndexOrThrow(ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME)
                    val phoneColumn = cursor.getColumnIndexOrThrow(ContactsContract.CommonDataKinds.Phone.NUMBER)
                    while (cursor.moveToNext() && contacts.size < limit) {
                        val id = cursor.getLong(idColumn)
                        val name = cursor.getString(nameColumn).orEmpty()
                        val phone = cursor.getString(phoneColumn).orEmpty()
                        contacts.getOrPut(id) { name to linkedSetOf() }.second.add(phone)
                    }
                }
                val items = JSArray()
                contacts.forEach { (id, value) ->
                    items.put(JSObject().apply {
                        put("id", id.toString())
                        put("name", value.first)
                        put("phones", JSArray(value.second.toList()))
                    })
                }
                invoke.resolve(JSObject().apply {
                    put("contacts", items)
                    put("count", contacts.size)
                    put("limited", contacts.size >= limit)
                })
            } catch (error: Exception) {
                invoke.reject(error.message ?: "Unable to read contacts")
            }
        }
    }

    private fun capturePhoto(invoke: Invoke, parameters: OperationParameters?) {
        if (!requirePermission(CAMERA, invoke)) return
        val intent = Intent(MediaStore.ACTION_IMAGE_CAPTURE)
        if (parameters?.cameraDirection == "front") {
            intent.putExtra("android.intent.extras.CAMERA_FACING", 1)
            intent.putExtra("android.intent.extra.USE_FRONT_CAMERA", true)
        }
        if (intent.resolveActivity(activity.packageManager) == null) {
            invoke.reject("No camera capture activity is available")
            return
        }
        startActivityForResult(invoke, intent, "cameraResult")
    }

    @ActivityCallback
    private fun cameraResult(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            invoke.reject("Camera capture was cancelled")
            return
        }
        @Suppress("DEPRECATION")
        val bitmap = result.data?.extras?.get("data") as? Bitmap
        if (bitmap == null) {
            invoke.reject("Camera did not return an image")
            return
        }
        val largest = max(bitmap.width, bitmap.height)
        val scale = if (largest > 1600) 1600.0 / largest else 1.0
        val width = (bitmap.width * scale).roundToInt()
        val height = (bitmap.height * scale).roundToInt()
        val output = if (scale < 1.0) Bitmap.createScaledBitmap(bitmap, width, height, true) else bitmap
        val bytes = ByteArrayOutputStream().use { stream ->
            output.compress(Bitmap.CompressFormat.JPEG, 85, stream)
            stream.toByteArray()
        }
        if (output !== bitmap) output.recycle()
        invoke.resolve(JSObject().apply {
            put("data", Base64.encodeToString(bytes, Base64.NO_WRAP))
            put("mimeType", "image/jpeg")
            put("width", width)
            put("height", height)
        })
    }

    private fun currentLocation(invoke: Invoke) {
        if (!requirePermission(LOCATION, invoke)) return
        val manager = activity.getSystemService(Activity.LOCATION_SERVICE) as LocationManager
        val provider = when {
            manager.isProviderEnabled(LocationManager.GPS_PROVIDER) -> LocationManager.GPS_PROVIDER
            manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER) -> LocationManager.NETWORK_PROVIDER
            else -> null
        }
        if (provider == null) {
            invoke.reject("Location services are disabled")
            return
        }
        try {
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                manager.getCurrentLocation(provider, null, ContextCompat.getMainExecutor(activity)) { location ->
                    resolveLocation(invoke, location)
                }
            } else {
                @Suppress("DEPRECATION")
                manager.requestSingleUpdate(provider, object : LocationListener {
                    override fun onLocationChanged(location: Location) = resolveLocation(invoke, location)
                    override fun onProviderDisabled(provider: String) = invoke.reject("Location provider was disabled")
                    override fun onProviderEnabled(provider: String) = Unit
                    @Deprecated("Deprecated in Android")
                    override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) = Unit
                }, Looper.getMainLooper())
            }
        } catch (error: SecurityException) {
            invoke.reject(error.message ?: "Unable to read location")
        }
    }

    private fun resolveLocation(invoke: Invoke, location: Location?) {
        if (location == null) {
            invoke.reject("No current location is available")
            return
        }
        invoke.resolve(JSObject().apply {
            put("latitude", location.latitude)
            put("longitude", location.longitude)
            put("accuracyMeters", location.accuracy.toDouble())
            put("altitudeMeters", if (location.hasAltitude()) location.altitude else null)
            put("timestamp", location.time)
        })
    }
}
