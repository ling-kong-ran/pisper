package app.pisper.mobiledevice

import java.io.File
import java.io.FileOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.Semaphore
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.roundToInt

internal class SpeechRequest(val id: String, var touchedAt: Long) {
    val cancelled = AtomicBoolean(false)
    var running = false
    fun check() {
        if (cancelled.get()) throw SpeechCancelled()
    }
}

internal class SpeechRequests(private val now: () -> Long = { System.nanoTime() / 1_000_000 }) {
    private val entries = mutableMapOf<String, SpeechRequest>()
    private var foreground = true

    companion object {
        fun validateId(id: String): String {
            require(Regex("^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
                .matches(id) && UUID.fromString(id).toString().equals(id, ignoreCase = true)
            ) { "speech_invalid_request_id" }
            return id.lowercase()
        }
    }

    @Synchronized
    private fun entry(id: String): SpeechRequest {
        val key = validateId(id)
        val time = now()
        entries.entries.removeAll { !it.value.running && time - it.value.touchedAt > 10 * 60_000 }
        return entries[key] ?: run {
            require(entries.size < 4096) { "speech_request_limit" }
            SpeechRequest(key, time).also { entries[key] = it }
        }
    }

    @Synchronized
    fun begin(id: String): SpeechRequest {
        require(foreground) { "speech_app_backgrounded" }
        return entry(id).also {
            it.check()
            require(!it.running) { "speech_request_busy" }
            it.running = true
            it.touchedAt = now()
        }
    }

    @Synchronized
    fun finish(request: SpeechRequest) {
        request.running = false
        request.touchedAt = now()
    }

    @Synchronized
    fun cancel(id: String): SpeechRequest = entry(id).also {
        it.cancelled.set(true)
        it.touchedAt = now()
    }

    @Synchronized
    fun pause() {
        foreground = false
        entries.values.forEach { it.cancelled.set(true) }
    }

    @Synchronized
    fun resume() { foreground = true }
}

internal data class SpeechIdleState(val pinned: Boolean, val releasedAt: Long)

// 会话使用独立请求表，转写请求结束不会解除录音或对话模式的持有。
internal class SpeechSessions(private val now: () -> Long = { System.nanoTime() / 1_000_000 }) {
    private val requests = SpeechRequests(now)
    private val active = mutableMapOf<String, SpeechRequest>()
    private var releasedAt = 0L

    @Synchronized
    fun begin(id: String, kinds: List<String>): SpeechRequest {
        require(kinds.size in 1..2 && kinds.distinct().size == kinds.size &&
            kinds.all { it == "asr" || it == "tts" }) { "speech_invalid_session_kinds" }
        val key = SpeechRequests.validateId(id)
        require(!active.containsKey(key)) { "speech_request_busy" }
        require(active.size < 16) { "speech_engine_busy" }
        return requests.begin(key).also { active[key] = it }
    }

    @Synchronized
    fun release(id: String) {
        val request = requests.cancel(id)
        finish(request)
    }

    @Synchronized
    fun finish(request: SpeechRequest) {
        request.cancelled.set(true)
        if (active[request.id] !== request) return
        active.remove(request.id)
        requests.finish(request)
        if (active.isEmpty()) releasedAt = now()
    }

    @Synchronized
    fun clear() {
        active.values.toList().forEach { finish(it) }
    }

    @Synchronized
    fun pause() { requests.pause(); clear() }

    @Synchronized
    fun resume() { requests.resume() }

    @Synchronized
    fun idleState() = SpeechIdleState(active.isNotEmpty(), releasedAt)
}

internal class SpeechEngineQueue {
    val executor = ScheduledThreadPoolExecutor(1).apply { removeOnCancelPolicy = true }
    private val slots = Semaphore(3)

    fun reserve() = slots.tryAcquire()
    fun releaseSlot() { slots.release() }

    fun submit(operation: () -> Unit, rejected: () -> Unit) {
        if (!reserve()) { rejected(); return }
        try {
            executor.execute {
                try { operation() } finally { releaseSlot() }
            }
        } catch (_: RejectedExecutionException) {
            releaseSlot()
            rejected()
        }
    }
}

internal interface SpeechEnginePreparation {
    fun validate()
    fun adopt()
    fun discard()
}

internal data class SpeechEnginePreparationJob(
    val queue: SpeechEngineQueue,
    val prepare: () -> SpeechEnginePreparation,
    val finish: () -> Unit = {},
)

internal object SpeechEnginePreloader {
    private val submissionLock = Any()

    fun prepare(
        jobs: List<SpeechEnginePreparationJob>,
        check: () -> Unit,
        publish: (() -> Unit) -> Unit,
        complete: (Throwable?) -> Unit,
    ) {
        require(jobs.isNotEmpty() && jobs.map { it.queue }.distinct().size == jobs.size)
        val state = Any()
        val loaded = CountDownLatch(jobs.size)
        val adopted = CountDownLatch(jobs.size)
        var remaining = jobs.size
        var failure: Throwable? = null
        var decided = false
        var committed = false
        fun observe(operation: () -> Unit) {
            try { operation() } catch (error: Throwable) {
                synchronized(state) {
                    if (failure == null) failure = error
                    else if (failure !== error) failure!!.addSuppressed(error)
                }
            }
        }
        fun await(latch: CountDownLatch) {
            var interrupted = false
            while (true) {
                try { latch.await(); break } catch (error: InterruptedException) {
                    interrupted = true
                    observe { throw error }
                }
            }
            if (interrupted) Thread.currentThread().interrupt()
        }
        fun finished() {
            val error = synchronized(state) {
                remaining--
                if (remaining == 0 && !committed) failure else null
            }
            if (error != null) complete(error)
        }
        // 多模型预热必须原子地按相同顺序入队，否则两组预热可能分别堵住对方的同类队列。
        synchronized(submissionLock) {
            val reserved = mutableListOf<SpeechEngineQueue>()
            for (job in jobs) {
                if (!job.queue.reserve()) {
                    reserved.forEach { it.releaseSlot() }
                    complete(IllegalStateException("speech_engine_busy"))
                    return
                }
                reserved += job.queue
            }
            for (job in jobs) {
                try {
                    job.queue.executor.execute {
                        var preparation: SpeechEnginePreparation? = null
                        observe {
                            check()
                            preparation = job.prepare()
                            preparation!!.validate()
                        }
                        loaded.countDown()
                        await(loaded)
                        observe {
                            publish {
                                check()
                                if (synchronized(state) { failure == null }) preparation!!.adopt()
                            }
                        }
                        adopted.countDown()
                        await(adopted)
                        // 双方均完成接管后才决定 ready；取消或任一失败会在各自队列回滚新句柄。
                        try {
                            observe {
                                publish {
                                    synchronized(state) {
                                        if (!decided) {
                                            observe { check() }
                                            decided = true
                                            committed = failure == null
                                            if (committed) complete(null)
                                        }
                                    }
                                }
                            }
                        } finally {
                            if (!synchronized(state) { committed }) observe { preparation?.discard() }
                            observe { job.finish() }
                            job.queue.releaseSlot()
                            finished()
                        }
                    }
                } catch (error: RejectedExecutionException) {
                    observe { throw error }
                    loaded.countDown()
                    adopted.countDown()
                    job.queue.releaseSlot()
                    finished()
                }
            }
        }
    }
}

// 缓存读写、加载、校验、接管和释放只在所属语音队列执行，两类队列可独立并行。
internal class SpeechEngineCache<K, E>(
    private val create: (K) -> E,
    private val release: (E) -> Unit,
    private val schedule: (Long, () -> Unit) -> (() -> Unit),
    private val now: () -> Long = { System.nanoTime() / 1_000_000 },
    private val idleState: () -> SpeechIdleState = { SpeechIdleState(false, 0L) },
) {
    private var key: K? = null
    private var engine: E? = null
    private var cancelIdle: (() -> Unit)? = null
    private var generation = 0L
    private var usedAt = 0L

    fun invalidate() {
        generation++
        cancelIdle?.invoke()
        cancelIdle = null
        val previous = engine
        engine = null
        key = null
        if (previous != null) release(previous)
    }

    fun touch() {
        usedAt = now()
        refreshIdle()
    }

    fun refreshIdle() {
        cancelIdle?.invoke()
        cancelIdle = null
        val currentGeneration = ++generation
        val state = idleState()
        if (engine == null || state.pinned) return
        val delay = (30_000 - (now() - maxOf(usedAt, state.releasedAt))).coerceAtLeast(0)
        cancelIdle = schedule(delay) {
            // pin 在控制线程先登记，已排队的旧回调也必须重新确认最新会话状态。
            if (generation == currentGeneration) {
                val latest = idleState()
                if (!latest.pinned && latest.releasedAt == state.releasedAt) invalidate()
                else refreshIdle()
            }
        }
    }

    private fun select(configuration: K) {
        val state = idleState()
        if (engine != null && (key != configuration || (!state.pinned &&
                now() - maxOf(usedAt, state.releasedAt) >= 30_000))) invalidate()
        cancelIdle?.invoke()
        cancelIdle = null
        generation++
    }

    fun prepare(configuration: K, validate: (E) -> Unit = {}): SpeechEnginePreparation {
        select(configuration)
        val cached = engine
        val selectedGeneration = generation
        val candidate = cached ?: create(configuration)
        return object : SpeechEnginePreparation {
            private var adopted = false
            private var discarded = false

            override fun validate() {
                require(generation == selectedGeneration) { "speech_cancelled" }
                validate(candidate)
            }

            override fun adopt() {
                if (cached == null) {
                    engine = candidate
                    key = configuration
                    generation++
                    adopted = true
                }
            }

            override fun discard() {
                if (cached != null || discarded) return
                discarded = true
                if (adopted) invalidate() else release(candidate)
            }
        }
    }

    fun <R> use(configuration: K, markUsed: Boolean = true, operation: (E) -> R): R {
        select(configuration)
        try {
            val current = engine ?: create(configuration).also { engine = it; key = configuration }
            val result = operation(current)
            if (markUsed) touch() else refreshIdle()
            return result
        } catch (error: Throwable) {
            invalidate()
            throw error
        }
    }
}

internal object SpeechWave {
    const val MAX_SECONDS = 45

    fun write(file: File, samples: FloatArray, sampleRate: Int, check: () -> Unit) {
        require(sampleRate in 8_000..48_000 && samples.isNotEmpty() &&
            samples.size <= sampleRate * MAX_SECONDS
        ) { "speech_audio_duration_exceeded" }
        val dataBytes = samples.size * 2
        val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("RIFF".toByteArray(Charsets.US_ASCII)); putInt(36 + dataBytes)
            put("WAVEfmt ".toByteArray(Charsets.US_ASCII)); putInt(16)
            putShort(1); putShort(1); putInt(sampleRate); putInt(sampleRate * 2)
            putShort(2); putShort(16)
            put("data".toByteArray(Charsets.US_ASCII)); putInt(dataBytes)
        }.array()
        FileOutputStream(file).use { output ->
            output.write(header)
            val bytes = ByteArray(8192)
            var index = 0
            while (index < samples.size) {
                check()
                val count = minOf(bytes.size / 2, samples.size - index)
                for (offset in 0 until count) {
                    val sample = samples[index + offset]
                    require(sample.isFinite()) { "speech_audio_invalid_sample" }
                    val pcm = (sample.coerceIn(-1f, 1f) * 32767).roundToInt()
                    bytes[offset * 2] = pcm.toByte()
                    bytes[offset * 2 + 1] = (pcm shr 8).toByte()
                }
                output.write(bytes, 0, count * 2)
                index += count
            }
            output.fd.sync()
        }
    }
}
