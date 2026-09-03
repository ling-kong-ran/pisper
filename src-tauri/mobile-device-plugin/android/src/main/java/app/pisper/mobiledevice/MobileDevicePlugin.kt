package app.pisper.mobiledevice

import android.Manifest
import android.app.Activity
import android.app.ActivityManager
import android.content.ActivityNotFoundException
import android.content.res.Configuration
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentUris
import android.content.ContentValues
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.Bundle
import android.os.Debug
import android.os.Environment
import android.os.Looper
import android.os.StatFs
import android.os.PowerManager
import android.os.VibrationEffect
import android.os.Vibrator
import android.provider.ContactsContract
import android.provider.MediaStore
import android.provider.Settings
import android.util.Base64
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OnlineModelConfig
import com.k2fsa.sherpa.onnx.OnlineRecognizer
import com.k2fsa.sherpa.onnx.OnlineRecognizerConfig
import com.k2fsa.sherpa.onnx.OnlineZipformer2CtcModelConfig
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
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
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import java.util.UUID
import java.util.concurrent.Executors
import kotlin.math.max
import kotlin.math.roundToInt

private const val CONTACTS = "contacts"
private const val CAMERA = "camera"
private const val LOCATION = "location"
private const val PHOTOS = "photos"
private const val NOTIFICATIONS = "notifications"
private const val LOCAL_NETWORK = "localNetwork"
private const val MICROPHONE = "microphone"
private const val EXTERNAL_APPS = "externalApps"
private val PHOTO_ID = Regex("^[0-9]+$")
private val ALBUM_NAME = Regex("^[^/\\\\]{1,120}$")
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
    var text: String? = null
    var body: String? = null
    var title: String? = null
    var notifyId: String? = null
    var packageName: String? = null
    var appUrl: String? = null
    var intensity: String? = null
    var durationMs: Int? = null
    var enabled: Boolean? = null
    var albumId: String? = null
    var albumName: String? = null
    var assetIds: List<String>? = null
    var confirmed: Boolean? = null
    var mediaType: String? = null
    var fromDate: String? = null
    var toDate: String? = null
    var fileName: String? = null
    var mimeType: String? = null
    var data: String? = null
}

@InvokeArg
class OperationArgs {
    var operation: String = ""
    var parameters: OperationParameters? = null
}

@InvokeArg
class TranscribeArgs {
    var pcmBase64: String = ""
}

class PisperAssetFileProvider : FileProvider()

@TauriPlugin(
    permissions = [
        Permission(strings = [Manifest.permission.READ_CONTACTS], alias = CONTACTS),
        Permission(strings = [Manifest.permission.CAMERA], alias = CAMERA),
        Permission(
            strings = [Manifest.permission.ACCESS_COARSE_LOCATION, Manifest.permission.ACCESS_FINE_LOCATION],
            alias = LOCATION
        ),
        Permission(
            strings = [
                "android.permission.READ_MEDIA_IMAGES",
                "android.permission.READ_MEDIA_VISUAL_USER_SELECTED",
                Manifest.permission.READ_EXTERNAL_STORAGE,
            ],
            alias = PHOTOS
        ),
        Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = NOTIFICATIONS),
        Permission(strings = ["android.permission.ACCESS_LOCAL_NETWORK"], alias = LOCAL_NETWORK),
        Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = MICROPHONE)
    ]
)
class MobileDevicePlugin(private val activity: Activity) : Plugin(activity) {
    private val worker = Executors.newSingleThreadExecutor()

    private fun state(alias: String): String = getPermissionState(alias).toString().lowercase()

    private fun hasPhotoPermission(): Boolean {
        val granted = PackageManager.PERMISSION_GRANTED
        return when {
            Build.VERSION.SDK_INT >= 34 ->
                ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_MEDIA_IMAGES) == granted ||
                    ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED) == granted
            Build.VERSION.SDK_INT >= 33 ->
                ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_MEDIA_IMAGES) == granted
            else ->
                ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_EXTERNAL_STORAGE) == granted
        }
    }

    private fun photoPermissionState(): String {
        if (hasPhotoPermission()) {
            val full = if (Build.VERSION.SDK_INT >= 33) {
                ContextCompat.checkSelfPermission(activity, Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            } else true
            return if (full) "granted" else "limited"
        }
        return "denied"
    }

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
        put("state", if (capability == PHOTOS) photoPermissionState() else state(capability))
    }

    private fun photoPermissionResult(): JSObject = permissionResult(PHOTOS)

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
            put(PHOTOS, photoPermissionState())
            put(NOTIFICATIONS, if (Build.VERSION.SDK_INT < 33) "granted" else state(NOTIFICATIONS))
            put(LOCAL_NETWORK, localNetworkState())
            put(MICROPHONE, state(MICROPHONE))
            put(EXTERNAL_APPS, "not-required")
        })
    }

    @Command
    fun requestPermission(invoke: Invoke) {
        when (val capability = invoke.parseArgs(PermissionArgs::class.java).capability) {
            CONTACTS -> requestPermissionForAlias(CONTACTS, invoke, "contactsPermissionCallback")
            CAMERA -> requestPermissionForAlias(CAMERA, invoke, "cameraPermissionCallback")
            LOCATION -> requestPermissionForAlias(LOCATION, invoke, "locationPermissionCallback")
            PHOTOS -> requestPermissionForAlias(PHOTOS, invoke, "photosPermissionCallback")
            NOTIFICATIONS -> {
                if (Build.VERSION.SDK_INT < 33) {
                    invoke.resolve(JSObject().apply {
                        put("capability", NOTIFICATIONS)
                        put("state", "granted")
                    })
                } else {
                    requestPermissionForAlias(NOTIFICATIONS, invoke, "notificationsPermissionCallback")
                }
            }
            LOCAL_NETWORK -> {
                if (!supportsLocalNetworkPermission()) {
                    invoke.resolve(localNetworkPermissionResult())
                } else {
                    requestPermissionForAlias(LOCAL_NETWORK, invoke, "localNetworkPermissionCallback")
                }
            }
            MICROPHONE -> requestPermissionForAlias(MICROPHONE, invoke, "microphonePermissionCallback")
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
    fun photosPermissionCallback(invoke: Invoke) = invoke.resolve(photoPermissionResult())

    @PermissionCallback
    fun notificationsPermissionCallback(invoke: Invoke) = invoke.resolve(permissionResult(NOTIFICATIONS))

    @PermissionCallback
    fun localNetworkPermissionCallback(invoke: Invoke) =
        invoke.resolve(localNetworkPermissionResult())

    @PermissionCallback
    fun microphonePermissionCallback(invoke: Invoke) = invoke.resolve(permissionResult(MICROPHONE))

    @Command
    fun transcribePcm(invoke: Invoke) {
        val pcmBase64 = invoke.parseArgs(TranscribeArgs::class.java).pcmBase64
        worker.execute {
            try {
                val encoded = Base64.decode(pcmBase64, Base64.DEFAULT)
                require(encoded.isNotEmpty() && encoded.size % 4 == 0) { "Invalid PCM data" }
                val samples = FloatArray(encoded.size / 4)
                val buffer = ByteBuffer.wrap(encoded).order(ByteOrder.LITTLE_ENDIAN)
                for (index in samples.indices) samples[index] = buffer.float
                val recognizer = OnlineRecognizer(
                    activity.assets,
                    OnlineRecognizerConfig(
                        featConfig = FeatureConfig(sampleRate = 16000, featureDim = 80),
                        modelConfig = OnlineModelConfig(
                            zipformer2Ctc = OnlineZipformer2CtcModelConfig("speech-model/model.int8.onnx"),
                            tokens = "speech-model/tokens.txt",
                            numThreads = 1,
                            provider = "cpu",
                        ),
                        decodingMethod = "greedy_search",
                    ),
                )
                val stream = recognizer.createStream()
                stream.acceptWaveform(samples, 16000)
                while (recognizer.isReady(stream)) recognizer.decode(stream)
                stream.inputFinished()
                while (recognizer.isReady(stream)) recognizer.decode(stream)
                val text = recognizer.getResult(stream).text.trim()
                stream.release()
                recognizer.release()
                invoke.resolve(JSObject().apply { put("text", text) })
            } catch (error: Throwable) {
                invoke.reject(error.message ?: "Speech recognition failed")
            }
        }
    }

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
            "device.info" -> deviceInfo(invoke)
            "device.capabilities" -> capabilities(invoke)
            "device.battery" -> batteryStatus(invoke)
            "device.storage" -> storageStatus(invoke)
            "device.memory" -> memoryStatus(invoke)
            "device.network" -> networkStatus(invoke)
            "device.display" -> displayStatus(invoke)
            "device.locale" -> localeStatus(invoke)
            "device.status" -> deviceStatus(invoke)
            "device.clipboard.get" -> getClipboard(invoke)
            "device.clipboard.set" -> setClipboard(invoke, args.parameters)
            "device.vibrate" -> vibrate(invoke, args.parameters)
            "device.notify" -> sendNotification(invoke, args.parameters)
            "device.flashlight" -> setFlashlight(invoke, args.parameters)
            "photos.list" -> listPhotos(invoke, args.parameters)
            "photos.create_album" -> createPhotoAlbum(invoke, args.parameters)
            "photos.add_to_album" -> addPhotosToAlbum(invoke, args.parameters)
            "photos.delete" -> deletePhotos(invoke, args.parameters)
            "apps.share_text" -> shareText(invoke, args.parameters)
            "apps.open_url" -> openUrl(invoke, args.parameters)
            "apps.open_map" -> openMap(invoke, args.parameters)
            "apps.open_system_settings" -> openSystemSettings(invoke)
            "apps.open_dialer" -> openDialer(invoke, args.parameters)
            "apps.compose_sms" -> composeSms(invoke, args.parameters)
            "apps.open_app" -> openApp(invoke, args.parameters)
            "files.open" -> openFile(invoke, args.parameters)
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

    private fun requirePhotoPermission(invoke: Invoke): Boolean {
        if (photoPermissionState() in setOf("granted", "limited")) return true
        invoke.reject("Pisper does not have photos permission")
        return false
    }

    private fun capability(
        action: String,
        operation: String,
        available: Boolean = true,
        permission: String = "none",
        permissionState: String = "not-required",
        requiredParameters: List<String> = emptyList(),
        optionalParameters: List<String> = emptyList(),
        limitations: List<String> = emptyList(),
    ): JSObject = JSObject().apply {
        put("action", action)
        put("operation", operation)
        put("available", available)
        put("permission", permission)
        put("permissionState", permissionState)
        put("requiredParameters", JSArray(requiredParameters))
        put("optionalParameters", JSArray(optionalParameters))
        put("limitations", JSArray(limitations))
    }

    private fun canResolve(intent: Intent): Boolean =
        activity.packageManager.resolveActivity(intent, PackageManager.MATCH_DEFAULT_ONLY) != null

    private fun hasFlashlight(): Boolean {
        val camera = activity.getSystemService(Activity.CAMERA_SERVICE) as CameraManager
        return try {
            camera.cameraIdList.any { id ->
                camera.getCameraCharacteristics(id).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
            }
        } catch (_: Exception) {
            false
        }
    }

    private fun capabilities(invoke: Invoke) {
        val version = activity.packageManager.getPackageInfo(activity.packageName, 0).versionName
        val cameraAvailable = activity.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)
        val location = activity.getSystemService(Activity.LOCATION_SERVICE) as LocationManager
        val locationAvailable = runCatching {
            location.isProviderEnabled(LocationManager.GPS_PROVIDER) ||
                location.isProviderEnabled(LocationManager.NETWORK_PROVIDER)
        }.getOrDefault(false)
        val urlAvailable = canResolve(Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com")))
        val mapAvailable = canResolve(Intent(Intent.ACTION_VIEW, Uri.parse("geo:0,0?q=0,0")))
        val dialerAvailable = canResolve(Intent(Intent.ACTION_DIAL, Uri.fromParts("tel", "0", null)))
        val smsAvailable = canResolve(Intent(Intent.ACTION_SENDTO, Uri.fromParts("smsto", "0", null)))
        val items = JSArray().apply {
            put(capability("search_contacts", "contacts.search", permission = CONTACTS, permissionState = state(CONTACTS), optionalParameters = listOf("query", "limit")))
            put(capability("capture_photo", "camera.capture", available = cameraAvailable, permission = CAMERA, permissionState = state(CAMERA), optionalParameters = listOf("cameraDirection"), limitations = listOf("需要在前台显示系统相机界面。")))
            put(capability("record_audio", "audio.record", permission = MICROPHONE, permissionState = state(MICROPHONE), limitations = listOf("仅在用户主动点击语音输入后采集。")))
            put(capability("get_location", "location.current", available = locationAvailable, permission = LOCATION, permissionState = state(LOCATION), limitations = listOf("定位服务关闭时不可用。")))
            put(capability("get_device_info", "device.info"))
            put(capability("get_battery_status", "device.battery"))
            put(capability("get_storage_status", "device.storage", limitations = listOf("返回内部数据分区的总容量、可用容量和已用容量。")))
            put(capability("get_memory_status", "device.memory"))
            put(capability("get_network_status", "device.network"))
            put(capability("get_display_status", "device.display"))
            put(capability("get_locale_status", "device.locale"))
            put(capability("get_device_status", "device.status", limitations = listOf("一次返回只读设备状态。")))
            put(capability("get_clipboard", "device.clipboard.get", limitations = listOf("只能读取当前系统剪贴板内容。")))
            put(capability("set_clipboard", "device.clipboard.set", requiredParameters = listOf("text")))
            put(capability("vibrate", "device.vibrate", available = (activity.getSystemService(Activity.VIBRATOR_SERVICE) as Vibrator).hasVibrator(), optionalParameters = listOf("intensity", "durationMs")))
            put(capability("set_flashlight", "device.flashlight", available = hasFlashlight(), permission = CAMERA, permissionState = state(CAMERA), requiredParameters = listOf("enabled")))
            put(capability("send_notification", "device.notify", permission = NOTIFICATIONS, permissionState = if (Build.VERSION.SDK_INT < 33) "granted" else state(NOTIFICATIONS), requiredParameters = listOf("body"), optionalParameters = listOf("title", "notifyId")))
            put(capability("list_photos", "photos.list", permission = PHOTOS, permissionState = photoPermissionState(), optionalParameters = listOf("limit", "mediaType", "fromDate", "toDate"), limitations = listOf("当前 Android 实现只列出图片。")))
            put(capability("create_photo_album", "photos.create_album", permission = PHOTOS, permissionState = photoPermissionState(), requiredParameters = listOf("albumName")))
            put(capability("add_photos_to_album", "photos.add_to_album", permission = PHOTOS, permissionState = photoPermissionState(), requiredParameters = listOf("albumId", "assetIds", "confirmed"), limitations = listOf("只能操作已知照片 ID，且必须明确确认。")))
            put(capability("delete_photos", "photos.delete", permission = PHOTOS, permissionState = photoPermissionState(), requiredParameters = listOf("assetIds", "confirmed"), limitations = listOf("必须明确确认删除。")))
            put(capability("share_text", "apps.share_text", requiredParameters = listOf("text"), limitations = listOf("打开系统分享面板不代表内容已经分享。")))
            put(capability("open_url", "apps.open_url", available = urlAvailable, requiredParameters = listOf("url"), limitations = listOf("只接受 HTTP 或 HTTPS URL。")))
            put(capability("open_map", "apps.open_map", available = mapAvailable, requiredParameters = listOf("latitude", "longitude"), optionalParameters = listOf("label")))
            put(capability("open_system_settings", "apps.open_system_settings"))
            put(capability("open_dialer", "apps.open_dialer", available = dialerAvailable, requiredParameters = listOf("phoneNumber"), limitations = listOf("只打开预填号码的拨号界面，不会自动拨号。")))
            put(capability("compose_sms", "apps.compose_sms", available = smsAvailable, requiredParameters = listOf("phoneNumber"), optionalParameters = listOf("message"), limitations = listOf("只打开预填短信界面，不会自动发送。")))
            put(capability("open_app", "apps.open_app", requiredParameters = listOf("packageName 或 appUrl"), limitations = listOf("必须提供已安装 Android 应用的包名，或目标应用已公开的 URL Scheme。")))
        }
        invoke.resolve(JSObject().apply {
            put("platform", "android")
            put("systemVersion", Build.VERSION.RELEASE)
            put("sdkInt", Build.VERSION.SDK_INT)
            put("appVersion", version ?: "")
            put("capabilities", items)
            put("platformLimitations", JSArray(listOf(
                "不支持读取短信内容或自动发送短信。",
                "不支持注入或控制第三方 App 的界面。",
                "照片和联系人结果受 Android 版本及系统权限限制。",
            )))
        })
    }

    private fun deviceInfoObject(): JSObject {
        val memory = activity.getSystemService(Activity.ACTIVITY_SERVICE) as ActivityManager
        val info = ActivityManager.MemoryInfo().also(memory::getMemoryInfo)
        val version = activity.packageManager.getPackageInfo(activity.packageName, 0).versionName
        return JSObject().apply {
            put("platform", "android")
            put("manufacturer", Build.MANUFACTURER)
            put("model", Build.MODEL)
            put("device", Build.DEVICE)
            put("systemVersion", Build.VERSION.RELEASE)
            put("sdkInt", Build.VERSION.SDK_INT)
            put("appVersion", version ?: "")
            put("memory", JSObject().apply {
                put("totalBytes", info.totalMem)
                put("availableBytes", info.availMem)
                put("lowMemory", info.lowMemory)
            })
        }
    }

    private fun deviceInfo(invoke: Invoke) {
        invoke.resolve(deviceInfoObject())
    }

    private fun storageStatusObject(): JSObject {
        val stats = StatFs(Environment.getDataDirectory().path)
        val totalBytes = stats.totalBytes
        val availableBytes = stats.availableBytes.coerceAtMost(totalBytes)
        return JSObject().apply {
            put("totalBytes", totalBytes)
            put("availableBytes", availableBytes)
            put("usedBytes", (totalBytes - availableBytes).coerceAtLeast(0L))
            put("scope", "internal_data_volume")
        }
    }

    private fun storageStatus(invoke: Invoke) {
        invoke.resolve(storageStatusObject())
    }

    private fun memoryStatusObject(): JSObject {
        val manager = activity.getSystemService(Activity.ACTIVITY_SERVICE) as ActivityManager
        val system = ActivityManager.MemoryInfo().also(manager::getMemoryInfo)
        val process = Debug.MemoryInfo().also { Debug.getMemoryInfo(it) }
        return JSObject().apply {
            put("totalBytes", system.totalMem)
            put("availableBytes", system.availMem)
            put("processAvailableBytes", null)
            put("appUsedBytes", process.totalPss.toLong() * 1024L)
            put("lowMemory", system.lowMemory)
            put("scope", "physical_memory")
            put("limitations", JSArray(listOf(
                "availableBytes 是系统报告的当前可用内存快照。",
            )))
        }
    }

    private fun memoryStatus(invoke: Invoke) {
        invoke.resolve(memoryStatusObject())
    }

    private fun networkStatusObject(): JSObject {
        val manager = activity.getSystemService(Activity.CONNECTIVITY_SERVICE) as ConnectivityManager
        val capabilities = manager.activeNetwork?.let(manager::getNetworkCapabilities)
        val interfaces = mutableListOf<String>()
        fun addTransport(transport: Int, name: String) {
            if (capabilities?.hasTransport(transport) == true) interfaces += name
        }
        addTransport(NetworkCapabilities.TRANSPORT_VPN, "vpn")
        addTransport(NetworkCapabilities.TRANSPORT_WIFI, "wifi")
        addTransport(NetworkCapabilities.TRANSPORT_CELLULAR, "cellular")
        addTransport(NetworkCapabilities.TRANSPORT_ETHERNET, "ethernet")
        addTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH, "bluetooth")
        if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_LOWPAN) == true) interfaces += "lowpan"
        val connected = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        return JSObject().apply {
            put("connected", connected)
            put("transport", if (connected) interfaces.firstOrNull() ?: "other" else "none")
            put("interfaces", JSArray(interfaces))
            put("metered", capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) != true)
            put("constrained", false)
            put("validated", capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true)
        }
    }

    private fun networkStatus(invoke: Invoke) {
        invoke.resolve(networkStatusObject())
    }

    private fun displayStatusObject(): JSObject {
        val metrics = activity.resources.displayMetrics
        val orientation = when (activity.resources.configuration.orientation) {
            Configuration.ORIENTATION_PORTRAIT -> "portrait"
            Configuration.ORIENTATION_LANDSCAPE -> "landscape"
            else -> "unknown"
        }
        return JSObject().apply {
            put("widthPixels", metrics.widthPixels)
            put("heightPixels", metrics.heightPixels)
            put("scale", metrics.density.toDouble())
            put("orientation", orientation)
            put("brightness", null)
        }
    }

    private fun displayStatus(invoke: Invoke) {
        invoke.resolve(displayStatusObject())
    }

    private fun localeStatusObject(): JSObject {
        val locale = Locale.getDefault()
        return JSObject().apply {
            put("languageTag", locale.toLanguageTag())
            put("languageCode", locale.language)
            put("regionCode", locale.country)
            put("calendar", Calendar.getInstance(locale).calendarType)
            put("timeZone", TimeZone.getDefault().id)
        }
    }

    private fun localeStatus(invoke: Invoke) {
        invoke.resolve(localeStatusObject())
    }

    private fun batteryStatusObject(): JSObject {
        val battery = activity.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, 100) ?: 100
        val status = when (battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1)) {
            BatteryManager.BATTERY_STATUS_CHARGING -> "charging"
            BatteryManager.BATTERY_STATUS_FULL -> "full"
            BatteryManager.BATTERY_STATUS_NOT_CHARGING -> "not-charging"
            BatteryManager.BATTERY_STATUS_DISCHARGING -> "discharging"
            else -> "unknown"
        }
        val power = activity.getSystemService(Activity.POWER_SERVICE) as PowerManager
        return JSObject().apply {
            put("level", if (level >= 0 && scale > 0) level.toDouble() / scale else -1.0)
            put("status", status)
            put("temperatureC", (battery?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Int.MIN_VALUE)
                ?.takeUnless { it == Int.MIN_VALUE }?.toDouble()?.div(10.0)))
            put("voltageMillivolts", battery?.getIntExtra(BatteryManager.EXTRA_VOLTAGE, -1) ?: -1)
            put("lowPowerMode", power.isPowerSaveMode)
        }
    }

    private fun batteryStatus(invoke: Invoke) {
        invoke.resolve(batteryStatusObject())
    }

    private fun deviceStatus(invoke: Invoke) {
        invoke.resolve(JSObject().apply {
            put("device", deviceInfoObject())
            put("memory", memoryStatusObject())
            put("storage", storageStatusObject())
            put("battery", batteryStatusObject())
            put("network", networkStatusObject())
            put("display", displayStatusObject())
            put("locale", localeStatusObject())
            put("platformLimitations", JSArray(listOf(
                "不支持读取短信内容或自动发送短信。",
                "不支持注入或控制第三方 App 的界面。",
                "照片和联系人结果受 Android 版本及系统权限限制。",
            )))
        })
    }

    private fun getClipboard(invoke: Invoke) {
        val clipboard = activity.getSystemService(Activity.CLIPBOARD_SERVICE) as ClipboardManager
        val text = clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(activity)?.toString()
        if (text != null && text.length > 100_000) {
            invoke.reject("剪贴板文本超过 100000 个字符。")
            return
        }
        invoke.resolve(JSObject().apply {
            put("text", text ?: "")
            put("hasText", text != null)
            put("length", text?.length ?: 0)
        })
    }

    private fun setClipboard(invoke: Invoke, parameters: OperationParameters?) {
        val text = parameters?.text
        if (text.isNullOrEmpty() || text.length > 100_000) {
            invoke.reject("剪贴板文本不能为空且不能超过 100000 个字符。")
            return
        }
        val clipboard = activity.getSystemService(Activity.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.setPrimaryClip(ClipData.newPlainText("Pisper", text))
        invoke.resolve(JSObject().apply {
            put("set", true)
            put("length", text.length)
        })
    }

    private fun sendNotification(invoke: Invoke, parameters: OperationParameters?) {
        val body = parameters?.body?.trim().orEmpty()
        val title = parameters?.title?.trim().orEmpty()
        val notifyId = parameters?.notifyId?.trim().orEmpty()
        if (body.isEmpty() || body.length > 4_000 || title.length > 120 ||
            title.any { it.isISOControl() } || notifyId.length > 120 || notifyId.any { it.isISOControl() }
        ) {
            invoke.reject("Notification title, body, or ID is invalid")
            return
        }
        if (Build.VERSION.SDK_INT >= 33 && state(NOTIFICATIONS) != "granted") {
            invoke.reject("Pisper does not have notifications permission")
            return
        }
        val channelId = "pisper-agent"
        val manager = activity.getSystemService(Activity.NOTIFICATION_SERVICE) as android.app.NotificationManager
        if (Build.VERSION.SDK_INT >= 26) {
            manager.createNotificationChannel(
                android.app.NotificationChannel(
                    channelId,
                    "Pisper",
                    android.app.NotificationManager.IMPORTANCE_DEFAULT,
                ),
            )
        }
        val notification = androidx.core.app.NotificationCompat.Builder(activity, channelId)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(title.takeIf { it.isNotEmpty() } ?: "Pisper")
            .setContentText(body)
            .setStyle(androidx.core.app.NotificationCompat.BigTextStyle().bigText(body))
            .setAutoCancel(true)
            .build()
        val id = notifyId.takeIf { it.isNotEmpty() }?.hashCode()
            ?: (System.currentTimeMillis() and 0x7fffffff).toInt()
        manager.notify(id, notification)
        invoke.resolve(JSObject().apply {
            put("posted", true)
            put("notifyId", notifyId.takeIf { it.isNotEmpty() } ?: id.toString())
        })
    }

    private fun vibrate(invoke: Invoke, parameters: OperationParameters?) {
        val vibrator = activity.getSystemService(Activity.VIBRATOR_SERVICE) as Vibrator
        if (!vibrator.hasVibrator()) {
            invoke.reject("This device does not provide haptic feedback")
            return
        }
        val duration = (parameters?.durationMs ?: 120).coerceIn(10, 2_000).toLong()
        val amplitude = when (parameters?.intensity) {
            "light" -> 80
            "heavy" -> 220
            else -> 150
        }
        if (Build.VERSION.SDK_INT >= 26) {
            vibrator.vibrate(VibrationEffect.createOneShot(duration, amplitude))
        } else {
            @Suppress("DEPRECATION")
            vibrator.vibrate(duration)
        }
        invoke.resolve(JSObject().apply {
            put("vibrated", true)
            put("durationMs", duration)
        })
    }

    private fun setFlashlight(invoke: Invoke, parameters: OperationParameters?) {
        val enabled = parameters?.enabled ?: true
        val camera = activity.getSystemService(Activity.CAMERA_SERVICE) as CameraManager
        try {
            val cameraId = camera.cameraIdList.firstOrNull { id ->
                camera.getCameraCharacteristics(id).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
            } ?: run {
                invoke.reject("This device does not provide a flashlight")
                return
            }
            camera.setTorchMode(cameraId, enabled)
            invoke.resolve(JSObject().apply {
                put("enabled", enabled)
            })
        } catch (error: SecurityException) {
            invoke.reject(error.message ?: "Android blocked flashlight access")
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Unable to change flashlight state")
        }
    }

    private fun validateAlbum(value: String?): String? {
        val album = value?.trim().orEmpty()
        if (!ALBUM_NAME.matches(album) || album.any { it.isISOControl() }) return null
        return album
    }

    private fun photoIds(parameters: OperationParameters?): List<Long>? {
        val values = parameters?.assetIds.orEmpty()
        if (values.isEmpty() || values.size > 100) return null
        return values.map { value ->
            if (!PHOTO_ID.matches(value)) return null
            value.toLongOrNull() ?: return null
        }
    }

    private fun validAlbumPath(value: String?): String? {
        val albumId = value?.trim().orEmpty()
        if (albumId.length > 256 || !albumId.startsWith("Pictures/") ||
            albumId.contains('\\') || albumId.any { it.isISOControl() }
        ) return null
        val path = if (albumId.endsWith('/')) albumId else "$albumId/"
        val segments = path.removeSuffix("/").split('/')
        if (segments.size < 2 || segments.first() != "Pictures" ||
            segments.drop(1).any { it.isEmpty() || it == "." || it == ".." }
        ) return null
        return path
    }

    private fun photoUri(id: Long): Uri = ContentUris.withAppendedId(
        MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
        id,
    )

    private fun dateSeconds(value: String?): Long? = value?.let {
        runCatching {
            SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSSXXX", Locale.US).apply {
                isLenient = false
            }.parse(it)?.time?.div(1_000)
        }.getOrNull()
    }

    private fun isoString(millis: Long): String =
        SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSSXXX", Locale.US).format(Date(millis))

    private fun listPhotos(invoke: Invoke, parameters: OperationParameters?) {
        if (!requirePhotoPermission(invoke)) return
        if (parameters?.mediaType != null && parameters.mediaType != "image") {
            invoke.reject("Android photo listing currently supports images only")
            return
        }
        val query = parameters?.query?.trim().orEmpty()
        val albumId = parameters?.albumId?.trim().orEmpty()
        if (query.length > 120 || albumId.length > 256 || albumId.contains("..") ||
            albumId.contains('\\') || albumId.any { it.isISOControl() }
        ) {
            invoke.reject("Photo album ID is invalid")
            return
        }
        val fromSeconds = dateSeconds(parameters?.fromDate)
        val toSeconds = dateSeconds(parameters?.toDate)
        if ((parameters?.fromDate != null && fromSeconds == null) ||
            (parameters?.toDate != null && toSeconds == null) ||
            (fromSeconds != null && toSeconds != null && fromSeconds > toSeconds)
        ) {
            invoke.reject("Photo date filters are invalid")
            return
        }
        val limit = (parameters?.limit ?: 50).coerceIn(1, 200)
        worker.execute {
            try {
                val columns = mutableListOf(
                    MediaStore.Images.Media._ID,
                    MediaStore.Images.Media.DISPLAY_NAME,
                    MediaStore.Images.Media.MIME_TYPE,
                    MediaStore.Images.Media.SIZE,
                    MediaStore.Images.Media.DATE_TAKEN,
                    MediaStore.Images.Media.DATE_ADDED,
                    MediaStore.Images.Media.DATE_MODIFIED,
                    MediaStore.Images.Media.WIDTH,
                    MediaStore.Images.Media.HEIGHT,
                    MediaStore.Images.Media.BUCKET_DISPLAY_NAME,
                )
                if (Build.VERSION.SDK_INT >= 29) columns += MediaStore.Images.Media.RELATIVE_PATH
                if (Build.VERSION.SDK_INT >= 30) columns += MediaStore.Images.Media.IS_FAVORITE
                val clauses = mutableListOf<String>()
                val args = mutableListOf<String>()
                if (query.isNotEmpty()) {
                    clauses += "${MediaStore.Images.Media.DISPLAY_NAME} LIKE ?"
                    args += "%$query%"
                }
                if (albumId.isNotEmpty()) {
                    if (Build.VERSION.SDK_INT >= 29) {
                        clauses += "${MediaStore.Images.Media.RELATIVE_PATH} = ?"
                        args += if (albumId.endsWith('/')) albumId else "$albumId/"
                    } else {
                        clauses += "${MediaStore.Images.Media.BUCKET_DISPLAY_NAME} = ?"
                        args += albumId
                    }
                }
                fromSeconds?.let {
                    clauses += "${MediaStore.Images.Media.DATE_ADDED} >= ?"
                    args += it.toString()
                }
                toSeconds?.let {
                    clauses += "${MediaStore.Images.Media.DATE_ADDED} <= ?"
                    args += it.toString()
                }
                val photos = JSArray()
                activity.contentResolver.query(
                    MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                    columns.toTypedArray(),
                    clauses.takeIf { it.isNotEmpty() }?.joinToString(" AND "),
                    args.takeIf { it.isNotEmpty() }?.toTypedArray(),
                    "${MediaStore.Images.Media.DATE_TAKEN} DESC, ${MediaStore.Images.Media._ID} DESC",
                )?.use { cursor ->
                    val idColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media._ID)
                    val nameColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DISPLAY_NAME)
                    val mimeColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.MIME_TYPE)
                    val sizeColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.SIZE)
                    val takenColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_TAKEN)
                    val addedColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_ADDED)
                    val modifiedColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_MODIFIED)
                    val widthColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.WIDTH)
                    val heightColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.HEIGHT)
                    val bucketColumn = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.BUCKET_DISPLAY_NAME)
                    val relativeColumn = cursor.getColumnIndex(MediaStore.Images.Media.RELATIVE_PATH)
                    val favoriteColumn = cursor.getColumnIndex(MediaStore.Images.Media.IS_FAVORITE)
                    while (cursor.moveToNext() && photos.length() < limit) {
                        val id = cursor.getLong(idColumn)
                        val created = cursor.getLong(takenColumn).takeIf { it > 0 }
                            ?: cursor.getLong(addedColumn) * 1_000
                        photos.put(JSObject().apply {
                            put("id", id.toString())
                            put("uri", photoUri(id).toString())
                            put("filename", cursor.getString(nameColumn).orEmpty())
                            put("mimeType", cursor.getString(mimeColumn).orEmpty())
                            put("mediaType", "image")
                            put("sizeBytes", cursor.getLong(sizeColumn))
                            put("durationSeconds", 0.0)
                            put("createdAt", isoString(created))
                            put("modifiedAt", isoString(cursor.getLong(modifiedColumn) * 1_000))
                            put("width", cursor.getInt(widthColumn))
                            put("height", cursor.getInt(heightColumn))
                            put("albumName", cursor.getString(bucketColumn).orEmpty())
                            put("albumId", if (relativeColumn >= 0) cursor.getString(relativeColumn).orEmpty() else cursor.getString(bucketColumn).orEmpty())
                            put("favorite", favoriteColumn >= 0 && cursor.getInt(favoriteColumn) != 0)
                            put("hidden", false)
                            put("latitude", null)
                            put("longitude", null)
                        })
                    }
                }
                invoke.resolve(JSObject().apply {
                    put("photos", photos)
                    put("count", photos.length())
                    put("limited", photoPermissionState() == "limited")
                })
            } catch (error: Exception) {
                invoke.reject(error.message ?: "Unable to read photos")
            }
        }
    }

    private fun createPhotoAlbum(invoke: Invoke, parameters: OperationParameters?) {
        if (!requirePhotoPermission(invoke)) return
        val album = validateAlbum(parameters?.albumName)
        if (album == null) {
            invoke.reject("Photo album name is invalid")
            return
        }
        if (Build.VERSION.SDK_INT < 29) {
            invoke.reject("Android photo albums require Android 10 or newer")
            return
        }
        val path = "${Environment.DIRECTORY_PICTURES}/$album/"
        invoke.resolve(JSObject().apply {
            put("albumId", path)
            put("albumName", album)
            put("created", false)
            put("readyForPhotos", true)
            put("note", "Android creates the folder when the first photo is moved into it")
        })
    }

    private fun addPhotosToAlbum(invoke: Invoke, parameters: OperationParameters?) {
        if (!requirePhotoPermission(invoke)) return
        if (parameters?.confirmed != true) {
            invoke.reject("Adding photos to an album requires explicit confirmation")
            return
        }
        if (Build.VERSION.SDK_INT < 29) {
            invoke.reject("Moving photos into albums requires Android 10 or newer")
            return
        }
        val ids = photoIds(parameters)
        val path = validAlbumPath(parameters?.albumId)
        if (ids == null || path == null) {
            invoke.reject("Photo IDs or album ID are invalid")
            return
        }
        worker.execute {
            try {
                val values = ContentValues().apply { put(MediaStore.Images.Media.RELATIVE_PATH, path) }
                var moved = 0
                ids.forEach { if (activity.contentResolver.update(photoUri(it), values, null, null) > 0) moved++ }
                invoke.resolve(JSObject().apply {
                    put("albumId", path)
                    put("requested", ids.size)
                    put("moved", moved)
                    put("requiresSystemConfirmation", false)
                })
            } catch (error: SecurityException) {
                invoke.reject(error.message ?: "Android requires user confirmation to move these photos")
            } catch (error: Exception) {
                invoke.reject(error.message ?: "Unable to move photos")
            }
        }
    }

    private fun deletePhotos(invoke: Invoke, parameters: OperationParameters?) {
        if (!requirePhotoPermission(invoke)) return
        if (parameters?.confirmed != true) {
            invoke.reject("Deleting photos requires explicit confirmation")
            return
        }
        val ids = photoIds(parameters)
        if (ids == null) {
            invoke.reject("Photo IDs are invalid")
            return
        }
        worker.execute {
            try {
                var deleted = 0
                ids.forEach { if (activity.contentResolver.delete(photoUri(it), null, null) > 0) deleted++ }
                invoke.resolve(JSObject().apply {
                    put("requested", ids.size)
                    put("deleted", deleted)
                })
            } catch (error: SecurityException) {
                invoke.reject(error.message ?: "Android requires user confirmation to delete these photos")
            } catch (error: Exception) {
                invoke.reject(error.message ?: "Unable to delete photos")
            }
        }
    }

    private fun openFile(invoke: Invoke, parameters: OperationParameters?) {
        val fileName = parameters?.fileName?.trim().orEmpty()
        val mimeType = parameters?.mimeType?.trim().orEmpty()
        val encoded = parameters?.data.orEmpty()
        if (fileName.isEmpty() || fileName.length > 180 || fileName.any { it.isISOControl() } ||
            fileName.contains('/') || fileName.contains('\\') || mimeType.isEmpty() ||
            mimeType.length > 120 || !mimeType.contains('/') || encoded.isEmpty() ||
            encoded.length > 180 * 1024 * 1024
        ) {
            invoke.reject("Asset file parameters are invalid")
            return
        }
        worker.execute {
            try {
                val bytes = Base64.decode(encoded, Base64.DEFAULT)
                if (bytes.isEmpty() || bytes.size > 128 * 1024 * 1024) {
                    invoke.reject("Asset file is empty or exceeds 128 MB")
                    return@execute
                }
                val directory = File(activity.cacheDir, "pisper-open-assets/${UUID.randomUUID()}")
                if (!directory.mkdirs() && !directory.isDirectory) {
                    invoke.reject("Unable to create the asset cache directory")
                    return@execute
                }
                val file = File(directory, fileName)
                file.writeBytes(bytes)
                val uri = FileProvider.getUriForFile(
                    activity,
                    "${activity.packageName}.pisperassets",
                    file
                )
                val intent = Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(uri, mimeType)
                    clipData = ClipData.newUri(activity.contentResolver, fileName, uri)
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                launchIntent(
                    invoke,
                    "files.open",
                    fileName,
                    Intent.createChooser(intent, "Open with")
                )
            } catch (error: IllegalArgumentException) {
                invoke.reject(error.message ?: "Asset data is not valid base64")
            } catch (error: Exception) {
                invoke.reject(error.message ?: "Unable to open the asset")
            }
        }
    }

    private fun shareText(invoke: Invoke, parameters: OperationParameters?) {
        val text = parameters?.text?.trim().orEmpty()
        val title = parameters?.title?.trim().orEmpty()
        if (text.isEmpty() || text.length > 100_000 || title.length > 120 ||
            title.any { it.isISOControl() }
        ) {
            invoke.reject("Share text or title is invalid")
            return
        }
        val shareTitle = title.takeIf { it.isNotEmpty() } ?: "Share with Pisper"
        val intent = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, text)
        }
        launchIntent(invoke, "apps.share_text", shareTitle, Intent.createChooser(intent, shareTitle))
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
