package app.pisper.mobiledevice

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.io.FileOutputStream

// 语音识别与合成的原生库（onnxruntime + sherpa JNI，约 25 MB）。GitHub 发布的 APK 不带这两个库，
// 构建时写入 speech-native-android.json，首次下载语音模型时一并下载到 App 私有目录；
// 商店包和本地构建没有这份清单，库照旧随 APK 安装。
// 构建时已去掉 sherpa 类静态初始化里的 System.loadLibrary（scripts/sherpa-aar.mjs），
// 两种情况都由 load() 在第一次创建识别器或合成器之前加载。
internal class SpeechNativeRuntime(private val context: Context, private val root: File) {
    private class Manifest(val id: String, val archive: SpeechArchive, val files: List<SpeechDownloadFile>)

    private val lock = Any()
    @Volatile private var loaded = false

    private val manifest: Manifest? by lazy {
        val text = try {
            context.assets.open(MANIFEST).use { input ->
                val bytes = input.readBytes()
                require(bytes.size <= 64 * 1024) { "speech_native_manifest_invalid" }
                String(bytes, Charsets.UTF_8)
            }
        } catch (_: FileNotFoundException) {
            return@lazy null
        }
        val entry = JSONObject(text)
        require(entry.getInt("version") == 1) { "speech_native_manifest_invalid" }
        val id = entry.getString("id")
        require(Regex("^[A-Za-z0-9][A-Za-z0-9._-]{0,119}$").matches(id)) { "speech_native_manifest_invalid" }
        val archive = entry.getJSONObject("archive")
        require(archive.getString("format") == "tar.bz2") { "speech_native_manifest_invalid" }
        val urls = archive.getJSONArray("urls")
        require(urls.length() in 1..4) { "speech_native_manifest_invalid" }
        val sources = (0 until urls.length()).map { urls.getString(it).also(SpeechModelFiles::trustedUrl) }
        val prefix = archive.getString("stripPrefix")
        require(prefix.endsWith('/')) { "speech_native_manifest_invalid" }
        SpeechModelFiles.relativePath(prefix.removeSuffix("/"))
        val list = entry.getJSONArray("files")
        val files = (0 until list.length()).map { index ->
            val item = list.getJSONObject(index)
            SpeechDownloadFile(
                SpeechModelFiles.relativePath(item.getString("path")),
                item.getLong("bytes"),
                item.getString("sha256"),
                emptyList(),
            )
        }
        // 只接受这两个库，清单不能让 App 加载别的原生代码。
        require(files.map { it.path }.sorted() == LIBRARIES.sorted() &&
            files.all { it.bytes in 1..(256L * 1024 * 1024) && Regex("^[a-f0-9]{64}$").matches(it.sha256) }
        ) { "speech_native_manifest_invalid" }
        val size = archive.getLong("bytes")
        val sha = archive.getString("sha256")
        require(size in 1..(256L * 1024 * 1024) && Regex("^[a-f0-9]{64}$").matches(sha)) {
            "speech_native_manifest_invalid"
        }
        Manifest(id, SpeechArchive(SpeechDownloadFile("archive.tar.bz2", size, sha, sources), prefix), files)
    }

    /** 需要另外下载时为 true；库随 APK 安装时始终为 false。 */
    fun missing(): Boolean {
        val manifest = manifest ?: return false
        if (loaded) return false
        val directory = directory(manifest)
        return !SpeechModelFiles.child(directory, ".complete").isFile || manifest.files.any { file ->
            val path = SpeechModelFiles.child(directory, file.path)
            !path.isFile || path.length() != file.bytes
        }
    }

    /** 还需下载的字节数（压缩包大小），不需要下载时为 0。 */
    fun downloadBytes(): Long = if (missing()) manifest!!.archive.file.bytes else 0

    fun install(job: SpeechDownloadCancellation, progress: (Long) -> Unit) {
        val manifest = manifest ?: return
        if (!missing()) return
        val archive = manifest.archive
        val cached = SpeechModelFiles.child(root, ".archives/native/${manifest.id}.tar.bz2")
        val retained = if (cached.isFile) cached.length().coerceAtMost(archive.file.bytes) else 0L
        require(root.usableSpace >= archive.file.bytes - retained + manifest.files.sumOf { it.bytes } +
            32L * 1024 * 1024) { "speech_insufficient_storage" }
        SpeechModelFiles.download(cached, archive.file, job, progress)
        val staging = SpeechModelFiles.child(root, ".staging/native/${manifest.id}")
        SpeechModelFiles.deleteTree(staging)
        require(staging.mkdirs()) { "speech_storage_error" }
        SpeechModelArchive.extract(cached, staging, archive.stripPrefix, manifest.files, job::check)
        FileOutputStream(SpeechModelFiles.child(staging, ".complete")).use {
            it.write(manifest.id.toByteArray(Charsets.UTF_8)); it.fd.sync()
        }
        val target = directory(manifest)
        require(target.parentFile!!.isDirectory || target.parentFile!!.mkdirs()) { "speech_storage_error" }
        val old = SpeechModelFiles.child(root, ".retired/native/${manifest.id}")
        require(old.parentFile!!.isDirectory || old.parentFile!!.mkdirs()) { "speech_storage_error" }
        SpeechModelFiles.deleteTree(old)
        synchronized(lock) { SpeechModelFiles.publish(staging, target, old, job::check) }
        runCatching { SpeechModelFiles.deleteTree(old) }
        runCatching { cached.delete() }
    }

    fun load(check: () -> Unit = {}) {
        if (loaded) return
        synchronized(lock) {
            if (loaded) return
            val manifest = manifest
            if (manifest == null) {
                // JNI 库在 DT_NEEDED 里声明了 onnxruntime，系统从 APK 的原生库目录一起加载。
                System.loadLibrary(JNI_LIBRARY)
            } else {
                require(!missing()) { "speech_model_not_installed" }
                val directory = directory(manifest)
                // 进程内第一次加载前校验摘要：私有目录里的文件被换掉时不能执行它。
                val files = LIBRARIES.map { name ->
                    val spec = manifest.files.first { it.path == name }
                    SpeechModelFiles.child(directory, name).also { file ->
                        require(SpeechModelFiles.verify(file, spec, check)) { "speech_model_verification_failed" }
                    }
                }
                // 先加载 onnxruntime：JNI 库按 soname 依赖它，已加载的同名库会直接复用。
                files.forEach { System.load(it.absolutePath) }
            }
            loaded = true
        }
    }

    private fun directory(manifest: Manifest): File = SpeechModelFiles.child(root, ".native/${manifest.id}")

    companion object {
        private const val MANIFEST = "speech-native-android.json"
        private const val JNI_LIBRARY = "sherpa-onnx-jni"
        private val LIBRARIES = listOf("libonnxruntime.so", "libsherpa-onnx-jni.so")
    }
}
