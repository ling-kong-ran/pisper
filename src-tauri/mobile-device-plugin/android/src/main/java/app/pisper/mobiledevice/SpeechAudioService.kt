package app.pisper.mobiledevice

import android.app.Activity
import android.app.Application
import android.content.ComponentCallbacks2
import android.content.res.Configuration
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import app.tauri.plugin.JSObject
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineSenseVoiceModelConfig
import com.k2fsa.sherpa.onnx.OfflineTts
import com.k2fsa.sherpa.onnx.OfflineTtsConfig
import com.k2fsa.sherpa.onnx.OfflineTtsVitsModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsModelConfig
import com.k2fsa.sherpa.onnx.OnlineModelConfig
import com.k2fsa.sherpa.onnx.OnlineRecognizer
import com.k2fsa.sherpa.onnx.OnlineRecognizerConfig
import com.k2fsa.sherpa.onnx.OnlineTransducerModelConfig
import java.io.File
import java.util.UUID
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

internal class SpeechAudioService(
    private val activity: Activity,
    private val models: SpeechModelStore,
) : Application.ActivityLifecycleCallbacks, ComponentCallbacks2 {
    private data class Audio(
        val id: String,
        val request: SpeechRequest,
        val file: File,
        val sampleRate: Int,
        val durationMs: Long,
        val createdAt: Long,
    )

    private class Playback(
        val audio: Audio,
        val player: MediaPlayer,
        val complete: (JSObject) -> Unit,
        val reject: (String) -> Unit,
    ) {
        var finished = false
    }

    companion object {
        // Activity 重建也复用同类队列；ASR 与 TTS 可并行，同类加载和推理始终互斥。
        private val asrQueue = SpeechEngineQueue()
        private val ttsQueue = SpeechEngineQueue()
    }

    private val lock = Any()
    private val requests = SpeechRequests()
    private val sessions = SpeechSessions()
    private data class AsrConfiguration(
        val online: OnlineRecognizerConfig? = null,
        val offline: OfflineRecognizerConfig? = null,
    )
    private class AsrEngine(configuration: AsrConfiguration) {
        val online = configuration.online?.let { OnlineRecognizer(assetManager = null, config = it) }
        val offline = configuration.offline?.let { OfflineRecognizer(assetManager = null, config = it) }
        fun release() { online?.release(); offline?.release() }
    }
    private val idleTimers = mutableSetOf<ScheduledFuture<*>>()
    private fun scheduleIdle(
        queue: SpeechEngineQueue, delay: Long, operation: () -> Unit,
    ): () -> Unit = synchronized(lock) {
        if (sessions.idleState().pinned) return@synchronized { Unit }
        lateinit var future: ScheduledFuture<*>
        future = queue.executor.schedule({
            synchronized(lock) {
                idleTimers.remove(future)
                operation()
            }
        }, delay, TimeUnit.MILLISECONDS)
        idleTimers.add(future)
        val cancel: () -> Unit = {
            synchronized(lock) { idleTimers.remove(future); future.cancel(false); Unit }
        }
        cancel
    }

    private fun cancelIdleTimers() {
        idleTimers.forEach { it.cancel(false) }
        idleTimers.clear()
    }
    private val asrCache = SpeechEngineCache<AsrConfiguration, AsrEngine>(
        create = { AsrEngine(it) },
        release = { it.release() },
        schedule = { delay, operation -> scheduleIdle(asrQueue, delay, operation) },
        idleState = sessions::idleState,
    )
    private val ttsCache = SpeechEngineCache<OfflineTtsConfig, OfflineTts>(
        create = { OfflineTts(assetManager = null, config = it) },
        release = { it.release() },
        schedule = { delay, operation -> scheduleIdle(ttsQueue, delay, operation) },
        idleState = sessions::idleState,
    )
    private val main = Handler(Looper.getMainLooper())
    private val audio = mutableMapOf<String, Audio>()
    private var playback: Playback? = null
    private var destroyed = false
    private var cacheInitialized = false
    private val cache: File by lazy {
        SpeechModelFiles.child(activity.cacheDir.canonicalFile, "pisper-speech").also {
            require(it.isDirectory || it.mkdirs()) { "speech_storage_error" }
        }
    }
    private val cleanup = object : Runnable {
        override fun run() {
            synchronized(lock) { pruneAudio() }
            if (!destroyed) main.postDelayed(this, 60_000)
        }
    }

    init {
        activity.application.registerActivityLifecycleCallbacks(this)
        activity.application.registerComponentCallbacks(this)
        main.postDelayed(cleanup, 60_000)
    }

    private fun errorCode(error: Throwable): String = error.message
        ?.takeIf { it.matches(Regex("speech_[a-z0-9_]+")) } ?: "speech_native_failed"

    private fun pruneAudio() {
        val stale = audio.values.filter {
            it.request.cancelled.get() || System.nanoTime() / 1_000_000 - it.createdAt > 120_000
        }
        stale.forEach { audio.remove(it.id); it.file.delete() }
    }

    private fun initializeCache() {
        if (cacheInitialized) return
        // 仅处理本服务生成的文件名；重启后的旧音频不能重新获得可播放 token。
        cache.listFiles()?.filter {
            it.name.matches(Regex("^[0-9a-f-]{36}\\.(wav|part)$"))
        }?.forEach { file ->
            require(file.canonicalFile == file.absoluteFile && file.isFile) { "speech_invalid_path" }
            require(file.delete()) { "speech_storage_error" }
        }
        cacheInitialized = true
    }

    private fun submit(
        queue: SpeechEngineQueue,
        requestId: String,
        complete: (JSObject) -> Unit,
        reject: (String) -> Unit,
        operation: (SpeechRequest) -> JSObject,
    ) {
        val request = try {
            synchronized(lock) {
                require(!destroyed) { "speech_app_destroyed" }
                requests.begin(requestId)
            }
        } catch (error: Throwable) {
            reject(errorCode(error))
            return
        }
        queue.submit(operation = {
            try {
                request.check()
                val result = operation(request)
                synchronized(lock) {
                    request.check()
                    complete(result)
                }
            } catch (error: Throwable) {
                reject(errorCode(error))
            } finally {
                if (queue === asrQueue) asrCache.touch() else ttsCache.touch()
                requests.finish(request)
            }
        }, rejected = {
            requests.finish(request)
            reject("speech_engine_busy")
        })
    }

    fun transcribe(
        modelId: String?,
        requestId: String?,
        samples: () -> FloatArray,
        hotwords: String,
        complete: (JSObject) -> Unit,
        reject: (String) -> Unit,
    ) {
        submit(asrQueue, requestId ?: UUID.randomUUID().toString(), complete, reject) { request ->
            val model = models.model(modelId, "asr")
            val directory = models.installedDirectory(model, request::check)
            val pcm = samples()
            request.check()
            val configuration = asrConfiguration(model, directory, hotwords)
            val text = asrCache.use(configuration) { engine ->
                request.check()
                if (engine.online != null) recognizeOnline(engine.online, pcm, hotwords, request)
                else recognizeOffline(requireNotNull(engine.offline), pcm, request)
            }
            request.check()
            JSObject().apply { put("text", text.trim()) }
        }
    }

    private fun asrConfiguration(model: SpeechModel, directory: File, hotwords: String): AsrConfiguration =
        when (model.engine) {
            "online-transducer" -> AsrConfiguration(online = onlineConfiguration(model, directory, hotwords))
            "sense-voice" -> AsrConfiguration(offline = offlineConfiguration(model, directory))
            else -> throw IllegalArgumentException("speech_model_engine_unsupported")
        }

    private fun onlineConfiguration(
        model: SpeechModel, directory: File, hotwords: String,
    ): OnlineRecognizerConfig {
        val vocab = models.bpeVocab(model)
        require(hotwords.isEmpty() || vocab.isNotEmpty()) { "speech_hotwords_unsupported" }
        return OnlineRecognizerConfig(
            featConfig = FeatureConfig(sampleRate = 16_000, featureDim = 80),
            modelConfig = OnlineModelConfig(
                transducer = OnlineTransducerModelConfig(
                    encoder = models.path(model, directory, "encoder"),
                    decoder = models.path(model, directory, "decoder"),
                    joiner = models.path(model, directory, "joiner"),
                ),
                tokens = models.path(model, directory, "tokens"),
                modelingUnit = if (vocab.isEmpty()) "" else "bpe",
                bpeVocab = vocab,
                numThreads = 1,
                provider = "cpu",
            ),
            decodingMethod = if (hotwords.isEmpty()) "greedy_search" else "modified_beam_search",
            maxActivePaths = 2,
            hotwordsScore = 1.5f,
        )
    }

    private fun recognizeOnline(
        recognizer: OnlineRecognizer,
        samples: FloatArray,
        hotwords: String,
        request: SpeechRequest,
    ): String {
        request.check()
        val stream = recognizer.createStream(hotwords.replace('\n', '/'))
        try {
            // 分块提供取消检查点，不把整个 60 秒样本一次交给原生特征提取。
            var offset = 0
            while (offset < samples.size) {
                request.check()
                val end = minOf(offset + 8_000, samples.size)
                stream.acceptWaveform(samples.copyOfRange(offset, end), 16_000)
                while (recognizer.isReady(stream)) {
                    request.check()
                    recognizer.decode(stream)
                }
                offset = end
            }
            // X-ASR 的尾部上下文需要补齐，避免立即松开录音时漏掉末尾词。
            stream.acceptWaveform(FloatArray(16_000), 16_000)
            stream.inputFinished()
            while (recognizer.isReady(stream)) {
                request.check()
                recognizer.decode(stream)
            }
            return recognizer.getResult(stream).text
        } finally { stream.release() }
    }

    private fun offlineConfiguration(model: SpeechModel, directory: File): OfflineRecognizerConfig =
        OfflineRecognizerConfig(
            featConfig = FeatureConfig(sampleRate = 16_000, featureDim = 80),
            modelConfig = OfflineModelConfig(
                senseVoice = OfflineSenseVoiceModelConfig(
                    model = models.path(model, directory, "model"),
                    language = model.config.optString("language", "auto"),
                    useInverseTextNormalization = true,
                ),
                tokens = models.path(model, directory, "tokens"),
                numThreads = 1,
                provider = "cpu",
            ),
        )

    private fun recognizeOffline(
        recognizer: OfflineRecognizer,
        samples: FloatArray,
        request: SpeechRequest,
    ): String {
        request.check()
        val stream = recognizer.createStream()
        try {
            stream.acceptWaveform(samples, 16_000)
            request.check()
            recognizer.decode(stream)
            request.check()
            return recognizer.getResult(stream).text
        } finally { stream.release() }
    }

    private fun ttsConfiguration(
        voiceId: String, request: SpeechRequest, text: String? = null,
    ): Pair<OfflineTtsConfig, Int> {
        val (model, sid) = models.voice(voiceId)
        require(model.engine == "vits" && sid == 0) { "speech_voice_unavailable" }
        val maxCodePoints = model.config.optInt("maxTextCodePoints", 16)
        val numThreads = model.config.optInt("numThreads", 4)
        require(maxCodePoints in 1..400 && numThreads in 1..16) { "speech_catalog_invalid" }
        if (text != null) require(text.codePointCount(0, text.length) <= maxCodePoints) { "speech_invalid_text" }
        val directory = models.installedDirectory(model, request::check)
        val dictDir = models.directoryPath(model, directory, "dictDir")
        request.check()
        fun pathList(key: String): String {
            val list = model.config.optJSONArray(key) ?: return ""
            return (0 until list.length()).joinToString(",") {
                val relative = list.getString(it)
                require(model.files.any { file -> file.path == relative } && !relative.contains(',')) {
                    "speech_catalog_missing_resource"
                }
                SpeechModelFiles.child(directory, relative).absolutePath
            }
        }
        return OfflineTtsConfig(
            model = OfflineTtsModelConfig(
                vits = OfflineTtsVitsModelConfig(
                    model = models.path(model, directory, "model"),
                    tokens = models.path(model, directory, "tokens"),
                    lexicon = models.path(model, directory, "lexicon"),
                    // 1.13.7 保留该 ABI 字段；实际 Melo 分词使用 lexicon，路径仍绑定已验证安装树。
                    dictDir = dictDir,
                    noiseScale = model.config.optDouble("noiseScale", 0.667).toFloat(),
                    noiseScaleW = model.config.optDouble("noiseScaleW", 0.8).toFloat(),
                    lengthScale = model.config.optDouble("lengthScale", 1.0).toFloat(),
                ),
                numThreads = numThreads,
                provider = "cpu",
            ),
            ruleFsts = pathList("ruleFsts"),
            maxNumSentences = 1,
            silenceScale = 0.2f,
        ) to sid
    }

    fun synthesize(
        text: String,
        voiceId: String,
        requestId: String,
        complete: (JSObject) -> Unit,
        reject: (String) -> Unit,
    ) {
        if (text.isBlank() || text.length > 400 || text.any { it == '\u0000' }) {
            reject("speech_invalid_text")
            return
        }
        submit(ttsQueue, requestId, complete, reject) { request ->
            val (configuration, sid) = ttsConfiguration(voiceId, request, text)
            initializeCache()
            synchronized(lock) {
                pruneAudio()
                require(audio.size < 4) { "speech_audio_queue_full" }
            }
            val audioId = UUID.randomUUID().toString()
            val temporary = SpeechModelFiles.child(cache, "$audioId.part")
            val file = SpeechModelFiles.child(cache, "$audioId.wav")
            try {
                request.check()
                val generated = ttsCache.use(configuration) { tts ->
                    require(sid < tts.numSpeakers()) { "speech_voice_unavailable" }
                    // VITS 仍是整句推理；复用引擎不改变音频粒度，取消后只丢弃结果。
                    val result = tts.generate(text, sid, 1.0f)
                    request.check()
                    SpeechWave.write(temporary, result.samples, result.sampleRate, request::check)
                    result
                }
                val duration = generated.samples.size.toLong() * 1000 / generated.sampleRate
                synchronized(lock) {
                    request.check()
                    require(temporary.renameTo(file)) { "speech_storage_error" }
                    audio[audioId] = Audio(
                        audioId, request, file, generated.sampleRate, duration, System.nanoTime() / 1_000_000,
                    )
                }
                JSObject().apply {
                    put("audioId", audioId)
                    put("sampleRate", generated.sampleRate)
                    put("durationMs", duration)
                }
            } catch (error: Throwable) {
                synchronized(lock) { audio.remove(audioId); file.delete() }
                throw error
            } finally {
                temporary.delete()
            }
        }
    }

    fun play(audioId: String, requestId: String, complete: (JSObject) -> Unit, reject: (String) -> Unit) {
        val id = try { SpeechRequests.validateId(requestId) } catch (error: Throwable) {
            reject(errorCode(error)); return
        }
        main.post {
            val clip = try {
                synchronized(lock) {
                    require(!destroyed) { "speech_app_destroyed" }
                    pruneAudio()
                    require(playback == null) { "speech_playback_busy" }
                    val clip = audio[audioId] ?: throw IllegalArgumentException("speech_audio_unknown")
                    require(clip.request.id == id) { "speech_audio_request_mismatch" }
                    clip.request.check()
                    audio.remove(audioId)
                    clip
                }
            } catch (error: Throwable) {
                reject(errorCode(error)); return@post
            }
            val player = try { MediaPlayer() } catch (_: Exception) {
                clip.file.delete()
                reject("speech_playback_failed")
                return@post
            }
            val current = Playback(clip, player, complete, reject)
            playback = current
            try {
                clip.request.check()
                player.setAudioAttributes(AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                player.setOnCompletionListener { finishPlayback(current, !clip.request.cancelled.get()) }
                player.setOnErrorListener { _, _, _ ->
                    finishPlayback(current, false, "speech_playback_failed")
                    true
                }
                player.setOnPreparedListener {
                    if (current.finished || clip.request.cancelled.get()) finishPlayback(current, false)
                    else try { player.start() } catch (_: Exception) {
                        finishPlayback(current, false, "speech_playback_failed")
                    }
                }
                player.setDataSource(clip.file.absolutePath)
                player.prepareAsync()
            } catch (error: Throwable) {
                finishPlayback(current, false, if (clip.request.cancelled.get()) null else errorCode(error))
            }
        }
    }

    private fun finishPlayback(current: Playback, completed: Boolean, error: String? = null) {
        if (current.finished) return
        current.finished = true
        runCatching { current.player.stop() }
        runCatching { current.player.release() }
        current.audio.file.delete()
        if (playback === current) playback = null
        if (error == null) current.complete(JSObject().apply { put("completed", completed) })
        else current.reject(error)
    }

    fun cancel(requestId: String, complete: (JSObject) -> Unit, reject: (String) -> Unit) {
        val request = try {
            synchronized(lock) {
                requests.cancel(requestId).also { pruneAudio() }
            }
        } catch (error: Throwable) {
            reject(errorCode(error)); return
        }
        // 控制命令只等主线程停播，绝不进入可能被 JNI 占用的推理队列。
        main.post {
            playback?.takeIf { it.audio.request.id == request.id }?.let { finishPlayback(it, false) }
            complete(JSObject().apply { put("cancelled", true) })
        }
    }

    private fun refreshIdle() {
        asrQueue.executor.execute { asrCache.refreshIdle() }
        ttsQueue.executor.execute { ttsCache.refreshIdle() }
    }

    fun prepareSession(
        requestId: String, kinds: List<String>, hotwords: String, voiceId: String,
        complete: (JSObject) -> Unit, reject: (String) -> Unit,
    ) {
        val request = try {
            synchronized(lock) {
                require(!destroyed) { "speech_app_destroyed" }
                sessions.begin(requestId, kinds).also { cancelIdleTimers() }
            }
        } catch (error: Throwable) {
            reject(errorCode(error)); return
        }
        // 先登记 pin，再向独立队列提交加载；失败和释放都不能让迟到任务重新登记。
        refreshIdle()
        val jobs = mutableListOf<SpeechEnginePreparationJob>()
        if ("asr" in kinds) jobs += SpeechEnginePreparationJob(asrQueue, prepare = {
            val model = models.model(null, "asr")
            val directory = models.installedDirectory(model, request::check)
            val configuration = asrConfiguration(model, directory, hotwords)
            request.check()
            asrCache.prepare(configuration)
        }, finish = { asrCache.refreshIdle() })
        if ("tts" in kinds) jobs += SpeechEnginePreparationJob(ttsQueue, prepare = {
            val selectedVoice = voiceId.ifEmpty {
                models.listModels().getJSONObject("defaults").getString("voice")
            }
            val (configuration, sid) = ttsConfiguration(selectedVoice, request)
            request.check()
            ttsCache.prepare(configuration) { tts ->
                require(sid < tts.numSpeakers()) { "speech_voice_unavailable" }
            }
        }, finish = { ttsCache.refreshIdle() })
        SpeechEnginePreloader.prepare(jobs, request::check,
            publish = { operation -> synchronized(lock) { operation() } },
            complete = { error ->
                if (error == null) complete(JSObject().apply { put("ready", true) })
                else {
                    sessions.finish(request)
                    refreshIdle()
                    reject(errorCode(error))
                }
            },
        )
    }

    fun releaseSession(requestId: String, complete: (JSObject) -> Unit, reject: (String) -> Unit) {
        try {
            synchronized(lock) { sessions.release(requestId) }
            refreshIdle()
            complete(JSObject().apply { put("released", true) })
        } catch (error: Throwable) { reject(errorCode(error)) }
    }

    private fun releaseEngine() {
        synchronized(lock) {
            sessions.clear()
            cancelIdleTimers()
            // 控制清理不占请求槽，各原生句柄只在自己的队列释放。
            asrQueue.executor.execute { asrCache.invalidate() }
            ttsQueue.executor.execute { ttsCache.invalidate() }
        }
    }

    private fun pause() {
        synchronized(lock) { requests.pause(); sessions.pause(); pruneAudio() }
        playback?.let { finishPlayback(it, false) }
        releaseEngine()
    }

    override fun onConfigurationChanged(configuration: Configuration) {}
    override fun onLowMemory() { releaseEngine() }
    override fun onTrimMemory(level: Int) {
        if (level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_MODERATE) releaseEngine()
    }

    override fun onActivityPaused(owner: Activity) { if (owner === activity) pause() }
    override fun onActivityStopped(owner: Activity) { if (owner === activity) pause() }
    override fun onActivityResumed(owner: Activity) {
        if (owner === activity) synchronized(lock) { requests.resume(); sessions.resume() }
    }
    override fun onActivityDestroyed(owner: Activity) {
        if (owner !== activity) return
        pause()
        synchronized(lock) { destroyed = true }
        // 共享同类执行器不销毁；旧任务已标记取消，清理分别排在各自 JNI 返回之后。
        main.removeCallbacks(cleanup)
        activity.application.unregisterActivityLifecycleCallbacks(this)
        activity.application.unregisterComponentCallbacks(this)
    }
    override fun onActivityCreated(owner: Activity, state: Bundle?) {}
    override fun onActivityStarted(owner: Activity) {}
    override fun onActivitySaveInstanceState(owner: Activity, state: Bundle) {}
}
