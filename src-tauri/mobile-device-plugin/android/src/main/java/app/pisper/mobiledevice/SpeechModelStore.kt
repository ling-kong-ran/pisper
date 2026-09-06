package app.pisper.mobiledevice

import android.content.Context
import app.tauri.plugin.JSObject
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

internal data class SpeechModel(
    val id: String,
    val revision: String,
    val kind: String,
    val engine: String,
    val publicFields: JSONObject,
    val config: JSONObject,
    val files: List<SpeechDownloadFile>,
    val archive: SpeechArchive?,
) {
    val filesBytes: Long = files.sumOf { it.bytes }
    val totalBytes: Long = archive?.file?.bytes ?: filesBytes
}

internal class SpeechModelStore private constructor(private val context: Context) {
    companion object {
        @Volatile private var instance: SpeechModelStore? = null
        fun get(context: Context): SpeechModelStore = instance ?: synchronized(this) {
            instance ?: SpeechModelStore(context.applicationContext).also { instance = it }
        }
    }

    private data class State(
        var status: String = "not-installed",
        var downloadedBytes: Long = 0,
        var error: String? = null,
        var job: SpeechDownloadCancellation? = null,
        var verifiedAttributes: List<Pair<Long, Long>>? = null,
        var verificationPending: Boolean = false,
    )

    private val lock = Any()
    private val io = ThreadPoolExecutor(1, 1, 0, TimeUnit.MILLISECONDS, ArrayBlockingQueue<Runnable>(64))
    private val root by lazy {
        SpeechModelFiles.child(context.dataDir.canonicalFile, "pisper-speech-models").also {
            require(it.isDirectory || it.mkdirs()) { "speech_storage_error" }
        }
    }
    private val catalog by lazy {
        val bytes = context.assets.open("speech-model-catalog.json").use { input ->
            val output = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                require(output.size() + count <= 4 * 1024 * 1024) { "speech_catalog_too_large" }
                output.write(buffer, 0, count)
            }
            output.toByteArray()
        }
        JSONObject(String(bytes, Charsets.UTF_8)).also {
            require(it.getInt("version") == 1) { "speech_catalog_version_unsupported" }
        }
    }
    private val models: List<SpeechModel> by lazy {
        val entries = catalog.getJSONArray("models")
        require(entries.length() in 1..32) { "speech_catalog_invalid" }
        val parsed = (0 until entries.length()).map { index -> parseModel(entries.getJSONObject(index)) }
        require(parsed.map { it.id }.distinct().size == parsed.size) { "speech_catalog_duplicate_id" }
        val voices = parsed.flatMap { model ->
            val list = model.publicFields.optJSONArray("voices") ?: JSONArray()
            (0 until list.length()).map { list.getJSONObject(it).getString("id") }
        }
        require(voices.distinct().size == voices.size) { "speech_catalog_duplicate_voice" }
        parsed
    }
    private val states = mutableMapOf<String, State>()

    private fun identifier(value: String): String {
        require(Regex("^[A-Za-z0-9][A-Za-z0-9._-]{0,119}$").matches(value) && value != "..") {
            "speech_invalid_model_id"
        }
        return value
    }

    private fun parseModel(entry: JSONObject): SpeechModel {
        val id = identifier(entry.getString("id"))
        val revision = identifier(entry.getString("revision"))
        val kind = entry.getString("kind")
        val engine = entry.getString("engine")
        require((kind == "asr" && engine in setOf("online-transducer", "sense-voice")) ||
            (kind == "tts" && engine == "vits")
        ) { "speech_catalog_engine_unsupported" }
        val archive = entry.optJSONObject("archive")?.let { item ->
            require(item.getString("format") == "tar.bz2") { "speech_catalog_archive_unsupported" }
            val size = item.getLong("bytes")
            val sha = item.getString("sha256")
            require(size in 1..(2L * 1024 * 1024 * 1024) && Regex("^[a-f0-9]{64}$").matches(sha)) {
                "speech_catalog_invalid_archive"
            }
            val urls = item.getJSONArray("urls")
            require(urls.length() in 1..4) { "speech_catalog_invalid_urls" }
            val sources = (0 until urls.length()).map { urls.getString(it).also(SpeechModelFiles::trustedUrl) }
            val prefix = item.getString("stripPrefix")
            require(prefix.endsWith('/')) { "speech_archive_invalid_prefix" }
            SpeechModelFiles.relativePath(prefix.removeSuffix("/"))
            SpeechArchive(SpeechDownloadFile("archive.tar.bz2", size, sha, sources), prefix)
        }
        val entries = entry.getJSONArray("files")
        require(entries.length() in 1..20_000) { "speech_catalog_invalid_files" }
        val files = (0 until entries.length()).map { index ->
            val item = entries.getJSONObject(index)
            val path = SpeechModelFiles.relativePath(item.getString("path"))
            require(path.substringBefore('/') != ".complete") { "speech_catalog_reserved_path" }
            val urls = item.optJSONArray("urls") ?: JSONArray()
            require(urls.length() in (if (archive == null) 1 else 0)..4) { "speech_catalog_invalid_urls" }
            val sources = (0 until urls.length()).map { urls.getString(it).also(SpeechModelFiles::trustedUrl) }
            val size = item.getLong("bytes")
            val sha = item.getString("sha256")
            require(size in 0..(2L * 1024 * 1024 * 1024) && Regex("^[a-f0-9]{64}$").matches(sha)) {
                "speech_catalog_invalid_file"
            }
            SpeechDownloadFile(path, size, sha, sources)
        }
        val paths = files.map { it.path }.toSet()
        require(paths.size == files.size && files.sumOf { it.bytes } <= 4L * 1024 * 1024 * 1024) {
            "speech_catalog_invalid_files"
        }
        require(paths.none { path -> path.split('/').dropLast(1).indices.any { depth ->
            path.split('/').take(depth + 1).joinToString("/") in paths
        } }) { "speech_catalog_path_conflict" }
        val config = entry.getJSONObject("config")
        val keys = when (engine) {
            "online-transducer" -> listOf("encoder", "decoder", "joiner", "tokens")
            "sense-voice" -> listOf("model", "tokens")
            else -> listOf("model", "tokens", "lexicon")
        }
        keys.forEach { key -> require(config.getString(key) in paths) { "speech_catalog_missing_resource" } }
        if (engine == "vits") {
            val allowed = setOf("model", "tokens", "lexicon", "dictDir", "ruleFsts", "numThreads",
                "maxTextCodePoints", "noiseScale", "noiseScaleW", "lengthScale")
            require(config.keys().asSequence().all { it in allowed }) { "speech_catalog_invalid" }
            for (key in listOf("model", "tokens", "lexicon", "dictDir")) {
                require(config.get(key) is String) { "speech_catalog_invalid" }
            }
            val dict = SpeechModelFiles.relativePath(config.getString("dictDir"))
            require(paths.any { it.startsWith("$dict/") } && !config.getString("lexicon").contains(',')) {
                "speech_catalog_missing_resource"
            }
            val list = if (config.has("ruleFsts")) config.getJSONArray("ruleFsts") else JSONArray()
            for (index in 0 until list.length()) {
                require(list.get(index) is String && list.getString(index) in paths && !list.getString(index).contains(',')) {
                    "speech_catalog_missing_resource"
                }
            }
            for ((key, maximum) in listOf("numThreads" to 16, "maxTextCodePoints" to 400)) {
                val value = config.get(key)
                require(value is Number && value.toDouble().isFinite() && value.toDouble() in 1.0..maximum.toDouble()
                    && value.toDouble() == value.toInt().toDouble()) { "speech_catalog_invalid" }
            }
            for (key in listOf("noiseScale", "noiseScaleW", "lengthScale")) {
                if (!config.has(key)) continue
                val value = config.get(key)
                require(value is Number && value.toDouble().isFinite() && value.toFloat().isFinite()
                    && value.toFloat() > 0) { "speech_catalog_invalid" }
            }
        }
        if (config.has("bpeVocabResource")) {
            require(config.getString("bpeVocabResource").startsWith("speech-resources/")) {
                "speech_catalog_invalid_resource"
            }
            SpeechModelFiles.relativePath(config.getString("bpeVocabResource"))
        }
        val fields = JSONObject()
        for (key in listOf("id", "kind", "engine", "name", "languages", "license", "voices")) {
            if (entry.has(key)) fields.put(key, entry.get(key))
        }
        if (kind == "tts") {
            val voices = fields.getJSONArray("voices")
            require(voices.length() == 1) { "speech_catalog_invalid_voices" }
            for (index in 0 until voices.length()) {
                val voice = voices.getJSONObject(index)
                identifier(voice.getString("id"))
                val sid = voice.get("sid")
                require(sid is Number && sid.toDouble() == 0.0) { "speech_catalog_invalid_voice" }
            }
        }
        return SpeechModel(id, revision, kind, engine, fields, config, files, archive)
    }

    fun model(id: String?, kind: String): SpeechModel {
        val selected = id ?: catalog.getJSONObject("defaults").getString(kind)
        return models.firstOrNull { it.id == selected && it.kind == kind }
            ?: throw IllegalArgumentException("speech_model_unknown")
    }

    private fun find(id: String): SpeechModel = models.firstOrNull { it.id == id }
        ?: throw IllegalArgumentException("speech_model_unknown")

    fun voice(id: String): Pair<SpeechModel, Int> {
        for (model in models.filter { it.kind == "tts" }) {
            val voices = model.publicFields.getJSONArray("voices")
            for (index in 0 until voices.length()) {
                val voice = voices.getJSONObject(index)
                if (voice.getString("id") == id) return model to voice.getInt("sid")
            }
        }
        throw IllegalArgumentException("speech_voice_unknown")
    }

    private fun directory(model: SpeechModel): File = SpeechModelFiles.child(root, "${model.id}/${model.revision}")

    private fun attributes(model: SpeechModel, directory: File): List<Pair<Long, Long>> = model.files.map { file ->
        val path = SpeechModelFiles.child(directory, file.path)
        require(path.isFile && path.length() == file.bytes) { "speech_model_incomplete" }
        path.length() to path.lastModified()
    }

    private fun verify(model: SpeechModel, directory: File, check: () -> Unit = {}): List<Pair<Long, Long>> {
        val before = attributes(model, directory)
        model.files.forEach { file ->
            check()
            require(SpeechModelFiles.verify(SpeechModelFiles.child(directory, file.path), file, check)) {
                "speech_model_checksum_mismatch"
            }
        }
        val after = attributes(model, directory)
        require(before == after) { "speech_model_changed" }
        return after
    }

    private fun scheduleVerification(model: SpeechModel, state: State) {
        if (state.verificationPending || state.job != null) return
        state.status = "verifying"
        state.verificationPending = true
        try {
            io.execute {
                try {
                    val directory = directory(model)
                    require(SpeechModelFiles.child(directory, ".complete").isFile) { "speech_model_not_installed" }
                    val stamp = verify(model, directory)
                    synchronized(lock) {
                        if (state.job == null) {
                            state.verifiedAttributes = stamp
                            state.downloadedBytes = model.totalBytes
                            state.status = "installed"
                            state.error = null
                        }
                    }
                } catch (_: Exception) {
                    synchronized(lock) {
                        if (state.job == null) {
                            state.verifiedAttributes = null
                            state.status = "error"
                            state.error = "speech_model_verification_failed"
                        }
                    }
                } finally {
                    synchronized(lock) { state.verificationPending = false }
                }
            }
        } catch (_: RejectedExecutionException) {
            state.verificationPending = false
            state.status = "error"
            state.error = "speech_download_busy"
        }
    }

    private fun state(model: SpeechModel): State = states.getOrPut(model.id) {
        State().also { state ->
            val directory = directory(model)
            val retired = SpeechModelFiles.child(root, ".retired/${model.id}/${model.revision}")
            // 发布中进程被系统终止时，旧安装可能已挪走；先恢复旧目录再做摘要检查。
            if (!directory.exists() && SpeechModelFiles.child(retired, ".complete").isFile) {
                require(directory.parentFile!!.isDirectory || directory.parentFile!!.mkdirs()) { "speech_storage_error" }
                require(retired.renameTo(directory)) { "speech_storage_recovery_required" }
            }
            if (SpeechModelFiles.child(directory, ".complete").isFile) scheduleVerification(model, state)
        }
    }

    private fun snapshot(model: SpeechModel, state: State): JSObject = JSObject().apply {
        model.publicFields.keys().forEach { key -> put(key, model.publicFields.get(key)) }
        model.publicFields.optJSONArray("voices")?.let { voices ->
            put("voices", JSONArray((0 until voices.length()).map { index ->
                val voice = voices.getJSONObject(index)
                JSONObject().apply {
                    for (key in listOf("id", "name", "language")) if (voice.has(key)) put(key, voice.get(key))
                }
            }))
        }
        put("status", state.status)
        put("downloadedBytes", state.downloadedBytes)
        put("totalBytes", model.totalBytes)
        put("filesBytes", model.filesBytes)
        state.error?.let { put("error", it) }
    }

    fun listModels(): JSObject {
        val available = models
        return synchronized(lock) {
            JSObject().apply {
                put("models", JSONArray(available.map { model ->
                    val state = state(model)
                    if (state.status == "installed" &&
                        (!SpeechModelFiles.child(directory(model), ".complete").isFile ||
                            runCatching { attributes(model, directory(model)) }.getOrNull() != state.verifiedAttributes)
                    ) scheduleVerification(model, state)
                    snapshot(model, state)
                }))
                put("defaults", JSONObject(catalog.getJSONObject("defaults").toString()))
            }
        }
    }

    fun download(id: String): JSObject {
        val model = find(id)
        return synchronized(lock) {
            val state = state(model)
            if (state.status == "installed" || state.job?.cancelled?.get() == false) {
                return@synchronized snapshot(model, state)
            }
            val job = SpeechDownloadCancellation()
            state.job = job
            state.status = "downloading"
            state.error = null
            state.downloadedBytes = 0
            try {
                io.execute { install(model, state, job) }
            } catch (_: RejectedExecutionException) {
                state.job = null
                state.status = "error"
                state.error = "speech_download_busy"
            }
            snapshot(model, state)
        }
    }

    fun cancelDownload(id: String): JSObject {
        val model = find(id)
        return synchronized(lock) {
            val state = state(model)
            state.job?.let { job ->
                job.cancelled.set(true)
                // 不等待网络读超时；重试在同一个 IO 执行器上接续同一临时目录。
                state.status = "cancelled"
                state.error = null
            }
            snapshot(model, state)
        }
    }

    private fun install(model: SpeechModel, state: State, job: SpeechDownloadCancellation) {
        try {
            job.check()
            val staging = SpeechModelFiles.child(root, ".staging/${model.id}/${model.revision}")
            require(staging.isDirectory || staging.mkdirs()) { "speech_storage_error" }
            val archive = model.archive
            if (archive != null) {
                val cached = SpeechModelFiles.child(root, ".archives/${model.id}/${model.revision}.tar.bz2")
                val retained = if (cached.isFile) cached.length().coerceAtMost(archive.file.bytes) else 0L
                require(root.usableSpace >= archive.file.bytes - retained + model.filesBytes + 32L * 1024 * 1024) {
                    "speech_insufficient_storage"
                }
                SpeechModelFiles.download(cached, archive.file, job, { bytes ->
                    synchronized(lock) {
                        if (state.job === job && !job.cancelled.get()) state.downloadedBytes = bytes
                    }
                })
                synchronized(lock) {
                    job.check()
                    if (state.job === job) state.status = "verifying"
                }
                // 归档本体可续传；提取目录每次重建，不能把上次中断留下的文件当作已安装权重。
                SpeechModelFiles.deleteTree(staging)
                require(staging.mkdirs()) { "speech_storage_error" }
                SpeechModelArchive.extract(cached, staging, archive.stripPrefix, model.files, job::check)
            } else {
                val remaining = model.files.sumOf { file ->
                    val size = SpeechModelFiles.child(staging, file.path).takeIf { it.isFile }?.length() ?: 0
                    (file.bytes - size.coerceAtMost(file.bytes)).coerceAtLeast(0)
                }
                require(root.usableSpace >= remaining + 32L * 1024 * 1024) { "speech_insufficient_storage" }
                var completed = 0L
                for (file in model.files) {
                    job.check()
                    SpeechModelFiles.download(SpeechModelFiles.child(staging, file.path), file, job, { bytes ->
                        synchronized(lock) {
                            if (state.job === job && !job.cancelled.get()) state.downloadedBytes = completed + bytes
                        }
                    })
                    completed += file.bytes
                }
            }
            synchronized(lock) {
                job.check()
                if (state.job === job) state.status = "verifying"
            }
            val stamps = verify(model, staging, job::check)
            val marker = SpeechModelFiles.child(staging, ".complete")
            FileOutputStream(marker).use { it.write(model.revision.toByteArray(Charsets.UTF_8)); it.fd.sync() }
            val target = directory(model)
            require(target.parentFile!!.isDirectory || target.parentFile!!.mkdirs()) { "speech_storage_error" }
            val old = SpeechModelFiles.child(root, ".retired/${model.id}/${model.revision}")
            require(old.parentFile!!.isDirectory || old.parentFile!!.mkdirs()) { "speech_storage_error" }
            SpeechModelFiles.deleteTree(old)
            synchronized(lock) {
                job.check()
                require(state.job === job) { "speech_cancelled" }
                SpeechModelFiles.publish(staging, target, old, job::check)
                state.verifiedAttributes = stamps
                state.downloadedBytes = model.totalBytes
                state.status = "installed"
                state.error = null
                state.job = null
            }
            runCatching { SpeechModelFiles.deleteTree(old) }
            if (model.archive != null) runCatching {
                SpeechModelFiles.child(root, ".archives/${model.id}/${model.revision}.tar.bz2").delete()
            }
        } catch (error: Exception) {
            synchronized(lock) {
                if (state.job === job) {
                    state.status = if (job.cancelled.get()) "cancelled" else "error"
                    state.error = if (job.cancelled.get()) null else
                        error.message?.takeIf { it.matches(Regex("speech_[a-z0-9_]+")) } ?: "speech_download_failed"
                    state.job = null
                }
            }
        }
    }

    fun installedDirectory(model: SpeechModel, check: () -> Unit): File {
        check()
        val directory = directory(model)
        require(SpeechModelFiles.child(directory, ".complete").isFile) { "speech_model_not_installed" }
        val attrs = attributes(model, directory)
        val cached = synchronized(lock) { state(model).verifiedAttributes }
        if (cached != attrs) {
            val verified = verify(model, directory, check)
            synchronized(lock) {
                val state = state(model)
                if (state.job == null) {
                    state.verifiedAttributes = verified
                    state.status = "installed"
                    state.downloadedBytes = model.totalBytes
                    state.error = null
                }
            }
        }
        check()
        return directory
    }

    fun path(model: SpeechModel, directory: File, key: String): String {
        val relative = model.config.getString(key)
        require(model.files.any { it.path == relative }) { "speech_catalog_missing_resource" }
        return SpeechModelFiles.child(directory, relative).absolutePath
    }

    fun directoryPath(model: SpeechModel, directory: File, key: String): String {
        val relative = SpeechModelFiles.relativePath(model.config.getString(key))
        require(model.files.any { it.path.startsWith("$relative/") }) { "speech_catalog_missing_resource" }
        val target = SpeechModelFiles.child(directory, relative)
        require(target.isDirectory) { "speech_model_incomplete" }
        return target.absolutePath
    }

    fun bpeVocab(model: SpeechModel): String {
        val resource = model.config.optString("bpeVocabResource", "")
        if (resource.isEmpty()) return ""
        val bytes = context.assets.open(resource).use { input ->
            val output = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                require(output.size() + count <= 256 * 1024) { "speech_resource_too_large" }
                output.write(buffer, 0, count)
            }
            output.toByteArray()
        }
        val target = SpeechModelFiles.child(root, ".resources/${model.id}-${model.revision}.vocab")
        require(target.parentFile!!.isDirectory || target.parentFile!!.mkdirs()) { "speech_storage_error" }
        val expected = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        if (!target.isFile || SpeechModelFiles.digest(target) != expected) {
            val temp = SpeechModelFiles.child(root, ".resources/${model.id}-${model.revision}.tmp")
            FileOutputStream(temp).use { it.write(bytes); it.fd.sync() }
            require(temp.renameTo(target)) { "speech_storage_error" }
        }
        return target.absolutePath
    }
}
