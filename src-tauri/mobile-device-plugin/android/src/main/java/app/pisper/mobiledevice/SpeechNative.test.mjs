import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { test } from 'node:test'
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { delimiter, dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const sourceDir = dirname(fileURLToPath(import.meta.url))
const cache = join(homedir(), '.gradle/caches/modules-2/files-2.1')

async function jar(group, artifact) {
  const root = join(cache, group, artifact)
  const versions = (await readdir(root)).sort((a, b) =>
    b.localeCompare(a, undefined, { numeric: true }),
  )
  for (const version of versions) {
    for (const hash of await readdir(join(root, version))) {
      const directory = join(root, version, hash)
      for (const name of await readdir(directory)) {
        if (name.endsWith('.jar') && !name.endsWith('-sources.jar')) return join(directory, name)
      }
    }
  }
  throw new Error(`Missing cached Kotlin dependency ${group}:${artifact}`)
}

async function archiveLibraries(dir) {
  const dependencies = [
    [
      'org.apache.commons',
      'commons-compress',
      '1.28.0',
      'e1522945218456f3649a39bc4afd70ce4bd466221519dba7d378f2141a4642ca',
    ],
    [
      'commons-io',
      'commons-io',
      '2.20.0',
      'df90bba0fe3cb586b7f164e78fe8f8f4da3f2dd5c27fa645f888100ccc25dd72',
    ],
    [
      'org.apache.commons',
      'commons-lang3',
      '3.18.0',
      '4eeeae8d20c078abb64b015ec158add383ac581571cddc45c68f0c9ae0230720',
    ],
    [
      'commons-codec',
      'commons-codec',
      '1.19.0',
      '5c3881e4f556855e9c532927ee0c9dfde94cc66760d5805c031a59887070af5f',
    ],
  ]
  return Promise.all(
    dependencies.map(async ([group, artifact, version, digest]) => {
      const name = `${artifact}-${version}.jar`
      const cachedRoot = join(cache, group, artifact, version)
      for (const hash of await readdir(cachedRoot).catch(() => [])) {
        const path = join(cachedRoot, hash, name)
        const bytes = await readFile(path).catch(() => null)
        if (bytes && createHash('sha256').update(bytes).digest('hex') === digest) return path
      }
      // 只在临时目录取固定摘要的测试依赖，不改项目锁文件或 Gradle 缓存。
      const url = `https://repo.maven.apache.org/maven2/${group.replaceAll('.', '/')}/${artifact}/${version}/${name}`
      const response = await fetch(url, { signal: AbortSignal.timeout(30_000) })
      assert.equal(response.status, 200)
      const bytes = Buffer.from(await response.arrayBuffer())
      assert.equal(createHash('sha256').update(bytes).digest('hex'), digest)
      const path = join(dir, name)
      await writeFile(path, bytes)
      return path
    }),
  )
}

function execute(command, args) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    timeout: 90_000,
    maxBuffer: 4 * 1024 * 1024,
  })
  assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`)
  return result.stdout
}

const harness = String.raw`
package app.pisper.mobiledevice

import java.io.ByteArrayInputStream
import java.io.File
import java.io.FileOutputStream
import org.apache.commons.compress.archivers.tar.TarArchiveEntry
import org.apache.commons.compress.archivers.tar.TarArchiveOutputStream
import org.apache.commons.compress.archivers.tar.TarConstants
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

private fun expectFailure(code: String? = null, block: () -> Unit) {
    var failed = false
    try { block() } catch (error: Exception) {
        if (code != null) check(error.message == code) { "Expected $code, got " + error.message }
        failed = true
    }
    check(failed) { "Expected failure" }
}
private fun sha(value: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(value)
    .joinToString("") { "%02x".format(it) }

private class Response(
    url: URL, val status: Int, val data: ByteArray, val headers: Map<String, String> = emptyMap(),
) : HttpURLConnection(url) {
    override fun connect() {}
    override fun disconnect() {}
    override fun usingProxy() = false
    override fun getResponseCode() = status
    override fun getHeaderField(name: String) = headers[name]
    override fun getInputStream() = ByteArrayInputStream(data)
}

private fun makeArchive(file: File, entries: List<Pair<TarArchiveEntry, ByteArray>>) {
    FileOutputStream(file).use { output ->
        BZip2CompressorOutputStream(output).use { bzip ->
            TarArchiveOutputStream(bzip).use { tar ->
                for ((entry, bytes) in entries) {
                    entry.size = bytes.size.toLong()
                    tar.putArchiveEntry(entry)
                    tar.write(bytes)
                    tar.closeArchiveEntry()
                }
                tar.finish()
            }
        }
    }
}

private fun testEngineCache() {
    var now = 0L
    var created = 0
    var failLoad = false
    val released = mutableListOf<Int>()
    val timers = mutableListOf<() -> Unit>()
    val cache = SpeechEngineCache<String, Int>(
        create = { if (failLoad) error("load failed"); ++created },
        release = { released += it },
        schedule = { delay, operation ->
            check(delay == 30_000L)
            timers += operation
            // 故意保留已取消回调，模拟它已入队但尚未执行的情况。
            val cancel: () -> Unit = {}
            cancel
        },
        now = { now },
    )
    check(cache.use("a") { it } == 1)
    val stale = timers.last()
    now = 10_000
    check(cache.use("a") { it } == 1 && created == 1)
    stale()
    check(released.isEmpty())
    val beforeASR = timers.last()
    now += 60_000
    cache.touch()
    beforeASR()
    check(cache.use("a") { it } == 1 && released.isEmpty())
    check(cache.use("b") { it } == 2 && released == listOf(1))
    timers.last()()
    check(released == listOf(1, 2))
    check(cache.use("b") { it } == 3)
    now += 30_000
    check(cache.use("b") { it } == 4 && released == listOf(1, 2, 3))
    expectFailure { cache.use("b") { error("generate failed") } }
    check(released == listOf(1, 2, 3, 4))
    failLoad = true
    expectFailure { cache.use("b") { it } }
    failLoad = false
    check(cache.use("b") { it } == 5)
    cache.invalidate()
    cache.invalidate()
    check(released == listOf(1, 2, 3, 4, 5))
    timers.forEach { it() }
    check(released.size == 5)

    val queue = java.util.concurrent.Executors.newSingleThreadExecutor()
    var nativeActive = false
    var builds = 0
    var frees = 0
    val queued = SpeechEngineCache<String, Int>(
        create = { ++builds },
        release = { check(!nativeActive); frees++ },
        schedule = { _, _ -> val cancel: () -> Unit = {}; cancel },
    )
    try {
        queue.submit { queued.use("a") { check(it == 1) } }.get()
        // 同类请求共用一条队列，空操作不影响已有缓存。
        queue.submit { check(builds == 1 && frees == 0) }.get()
        queue.submit {
            queued.use("a") {
                nativeActive = true
                queue.execute { queued.invalidate() }
                check(it == 1 && frees == 0)
                nativeActive = false
            }
            check(frees == 0)
        }.get()
        queue.submit { check(builds == 1 && frees == 1); queued.invalidate() }.get()
        queue.submit { check(queued.use("a") { it } == 2); queued.invalidate() }.get()
        check(frees == 2)
    } finally { queue.shutdownNow() }
}

private fun testParallelPreload() {
    for (mode in listOf("success", "first-fails", "second-fails", "both-fail", "cancel", "cancel-adopt", "invalid")) {
        val asrQueue = SpeechEngineQueue()
        val ttsQueue = SpeechEngineQueue()
        val publicationLock = Any()
        val entered = CountDownLatch(2)
        val firstGate = CountDownLatch(1)
        val secondGate = CountDownLatch(1)
        val firstReturned = CountDownLatch(1)
        val constructorCount = AtomicInteger()
        val firstBuilds = AtomicInteger()
        val secondBuilds = AtomicInteger()
        val fail = AtomicBoolean(true)
        val queuedInference = AtomicInteger()
        val sessions = SpeechSessions()
        val request = sessions.begin("00000000-0000-4000-8000-000000000401", listOf("asr", "tts"))
        val asrThread = asrQueue.executor.submit<Thread> { Thread.currentThread() }.get()
        val ttsThread = ttsQueue.executor.submit<Thread> { Thread.currentThread() }.get()
        val released = java.util.Collections.synchronizedList(mutableListOf<String>())
        fun create(name: String, gate: CountDownLatch, builds: AtomicInteger): String {
            check(Thread.currentThread() === if (name == "first") asrThread else ttsThread)
            val serial = builds.incrementAndGet()
            constructorCount.incrementAndGet()
            entered.countDown()
            try {
                check(gate.await(5, TimeUnit.SECONDS))
                if (fail.get() && (mode == "$name-fails" || mode == "both-fail")) {
                    error("speech_" + name + "_failed")
                }
                return "$name-$serial"
            } finally {
                constructorCount.decrementAndGet()
                if (name == "first") firstReturned.countDown()
            }
        }
        fun release(value: String) {
            check(Thread.currentThread() === if (value.startsWith("first")) asrThread else ttsThread)
            check(constructorCount.get() == 0)
            check(value !in released)
            released += value
        }
        val first = SpeechEngineCache<String, String>(
            create = { create("first", firstGate, firstBuilds) }, release = ::release,
            schedule = { _, _ -> {} }, idleState = sessions::idleState,
        )
        val second = SpeechEngineCache<String, String>(
            create = { create("second", secondGate, secondBuilds) }, release = ::release,
            schedule = { _, _ -> {} }, idleState = sessions::idleState,
        )
        fun prepare(key: String = "asr", initial: Boolean = false): CompletableFuture<Throwable?> {
            val result = CompletableFuture<Throwable?>()
            SpeechEnginePreloader.prepare(listOf(
                SpeechEnginePreparationJob(asrQueue, {
                    val candidate = first.prepare(key)
                    object : SpeechEnginePreparation by candidate {
                        override fun adopt() {
                            candidate.adopt()
                            if (initial && mode == "cancel-adopt") sessions.release(request.id)
                        }
                    }
                }),
                SpeechEnginePreparationJob(ttsQueue, { second.prepare("tts") {
                    check(Thread.currentThread() === ttsThread)
                    if (initial && mode == "invalid") error("speech_voice_unavailable")
                } }),
            ), check = { if (initial) request.check() },
                publish = { operation -> synchronized(publicationLock) { operation() } },
                complete = { error ->
                    if (error != null) sessions.finish(request)
                    result.complete(error)
                },
            )
            return result
        }
        fun flush() {
            asrQueue.executor.submit {}.get(5, TimeUnit.SECONDS)
            ttsQueue.executor.submit {}.get(5, TimeUnit.SECONDS)
        }
        try {
            val result = prepare(initial = true)
            // 两个 constructor 必须在放开任一返回闸门前同时进入，不能用并行排队冒充并行加载。
            check(entered.await(5, TimeUnit.SECONDS))
            check(constructorCount.get() == 2)
            asrQueue.executor.execute { queuedInference.incrementAndGet() }
            ttsQueue.executor.execute { queuedInference.incrementAndGet() }
            if (mode == "cancel") synchronized(publicationLock) { sessions.release(request.id) }
            firstGate.countDown()
            check(firstReturned.await(5, TimeUnit.SECONDS))
            try {
                result.get(100, TimeUnit.MILLISECONDS)
                error("prepare returned before observing the late constructor")
            } catch (_: TimeoutException) {}
            check(queuedInference.get() == 0)
            secondGate.countDown()
            val error = result.get(5, TimeUnit.SECONDS)
            val expectedError = when (mode) {
                "success" -> null
                "first-fails", "both-fail" -> "speech_first_failed"
                "second-fails" -> "speech_second_failed"
                "cancel", "cancel-adopt" -> "speech_cancelled"
                else -> "speech_voice_unavailable"
            }
            check(error?.message == expectedError)
            if (mode == "both-fail") check(error!!.suppressed.any { it.message == "speech_second_failed" })
            flush()
            check(queuedInference.get() == 2 && constructorCount.get() == 0)
            val discarded = when (mode) {
                "first-fails" -> listOf("second-1")
                "second-fails" -> listOf("first-1")
                "cancel", "cancel-adopt", "invalid" -> listOf("first-1", "second-1")
                else -> emptyList()
            }
            check(released.sorted() == discarded.sorted())
            fail.set(false)
            check(prepare().get(5, TimeUnit.SECONDS) == null)
            flush()
            val expectedBuilds = if (mode == "success") 1 else 2
            check(firstBuilds.get() == expectedBuilds && secondBuilds.get() == expectedBuilds)
            asrQueue.executor.submit { first.use("asr") { check(constructorCount.get() == 0) } }.get()
            ttsQueue.executor.submit { second.use("tts") { check(constructorCount.get() == 0) } }.get()
            // 只替换 ASR 配置时，已加载 TTS 仍复用；旧句柄仅在 ASR 队列释放。
            check(prepare("asr-next").get(5, TimeUnit.SECONDS) == null)
            flush()
            check(firstBuilds.get() == expectedBuilds + 1 && secondBuilds.get() == expectedBuilds)
            asrQueue.executor.submit { first.invalidate() }.get()
            ttsQueue.executor.submit { second.invalidate() }.get()
            val successfulCreates = firstBuilds.get() + secondBuilds.get() - when (mode) {
                "first-fails", "second-fails" -> 1
                "both-fail" -> 2
                else -> 0
            }
            check(released.size == successfulCreates)
        } finally {
            firstGate.countDown()
            secondGate.countDown()
            asrQueue.executor.shutdown()
            ttsQueue.executor.shutdown()
            check(asrQueue.executor.awaitTermination(5, TimeUnit.SECONDS))
            check(ttsQueue.executor.awaitTermination(5, TimeUnit.SECONDS))
        }
    }
}

private fun testSpeechInferenceQueues() {
    val asr = SpeechEngineQueue()
    val tts = SpeechEngineQueue()
    val entered = CountDownLatch(2)
    val gate = CountDownLatch(1)
    val activeAsr = AtomicInteger()
    val activeTts = AtomicInteger()
    val asrOrder = java.util.Collections.synchronizedList(mutableListOf<Int>())
    val ttsOrder = java.util.Collections.synchronizedList(mutableListOf<Int>())
    val requests = SpeechRequests()
    val asrRequest = requests.begin("00000000-0000-4000-8000-000000000501")
    val ttsRequest = requests.begin("00000000-0000-4000-8000-000000000502")
    fun submit(queue: SpeechEngineQueue, index: Int): CompletableFuture<String> {
        val result = CompletableFuture<String>()
        val active = if (queue === asr) activeAsr else activeTts
        val order = if (queue === asr) asrOrder else ttsOrder
        queue.submit(operation = {
            try {
                check(active.incrementAndGet() == 1)
                order += index
                if (index == 1) {
                    entered.countDown()
                    check(gate.await(5, TimeUnit.SECONDS))
                    if (queue === asr) expectFailure("speech_cancelled") { asrRequest.check() }
                    else ttsRequest.check()
                }
                result.complete("done")
            } catch (error: Throwable) { result.completeExceptionally(error) }
            finally { active.decrementAndGet() }
        }, rejected = { result.complete("busy") })
        return result
    }
    try {
        val asrJobs = (1..3).map { submit(asr, it) }
        check(submit(asr, 4).get(5, TimeUnit.SECONDS) == "busy")
        // ASR 已占满三个槽，空闲 TTS 仍必须进入；两类推理可以同时保持运行状态。
        val ttsJobs = (1..2).map { submit(tts, it) }
        check(entered.await(5, TimeUnit.SECONDS))
        check(activeAsr.get() == 1 && activeTts.get() == 1)
        check(asrOrder == listOf(1) && ttsOrder == listOf(1))
        val control = CompletableFuture<Unit>()
        asr.executor.execute { control.complete(Unit) }
        requests.cancel(asrRequest.id)
        gate.countDown()
        (asrJobs + ttsJobs).forEach { check(it.get(5, TimeUnit.SECONDS) == "done") }
        control.get(5, TimeUnit.SECONDS)
        check(asrOrder == listOf(1, 2, 3) && ttsOrder == listOf(1, 2))
        ttsRequest.check()
    } finally {
        gate.countDown()
        asr.executor.shutdown()
        tts.executor.shutdown()
        check(asr.executor.awaitTermination(5, TimeUnit.SECONDS))
        check(tts.executor.awaitTermination(5, TimeUnit.SECONDS))
    }
}

private fun testConcurrentPrepareOrdering() {
    val asr = SpeechEngineQueue()
    val tts = SpeechEngineQueue()
    val callers = Executors.newFixedThreadPool(2)
    val start = CountDownLatch(1)
    val holdQueues = CountDownLatch(1)
    val publicationLock = Any()
    val orders = listOf(mutableListOf<Int>(), mutableListOf<Int>())
    val queues = listOf(asr, tts)
    val caches = queues.map {
        SpeechEngineCache<Int, Int>(create = { it }, release = {}, schedule = { _, _ -> {} },
            idleState = { SpeechIdleState(true, 0) })
    }
    queues.forEach { it.executor.execute { check(holdQueues.await(5, TimeUnit.SECONDS)) } }
    try {
        val completions = (1..2).map { CompletableFuture<Throwable?>() }
        val submissions = (1..2).map { id ->
            callers.submit {
                check(start.await(5, TimeUnit.SECONDS))
                val indices = if (id == 1) listOf(0, 1) else listOf(1, 0)
                SpeechEnginePreloader.prepare(indices.map { index ->
                    SpeechEnginePreparationJob(queues[index], {
                        orders[index] += id
                        caches[index].prepare(id)
                    })
                }, check = {}, publish = { operation -> synchronized(publicationLock) { operation() } },
                    complete = { completions[id - 1].complete(it) })
            }
        }
        start.countDown()
        submissions.forEach { it.get(5, TimeUnit.SECONDS) }
        holdQueues.countDown()
        completions.forEach { check(it.get(5, TimeUnit.SECONDS) == null) }
        queues.forEachIndexed { index, queue -> queue.executor.submit { caches[index].invalidate() }.get() }
        check(orders[0] == orders[1] && orders[0].sorted() == listOf(1, 2))
    } finally {
        start.countDown()
        holdQueues.countDown()
        callers.shutdown()
        queues.forEach { it.executor.shutdown() }
        check(callers.awaitTermination(5, TimeUnit.SECONDS))
        queues.forEach { check(it.executor.awaitTermination(5, TimeUnit.SECONDS)) }
    }
}

private fun testSpeechSessions() {
    var now = 0L
    val sessions = SpeechSessions { now }
    val first = "00000000-0000-4000-8000-000000000101"
    val second = "00000000-0000-4000-8000-000000000102"
    val third = "00000000-0000-4000-8000-000000000103"
    data class Timer(val at: Long, val operation: () -> Unit, var cancelled: Boolean = false)
    val timers = mutableListOf<Timer>()
    var asrBuilds = 0
    var ttsBuilds = 0
    var asrFrees = 0
    var ttsFrees = 0
    var streams = 0
    var streamFrees = 0
    var failLoad = false
    fun schedule(delay: Long, operation: () -> Unit): () -> Unit {
        val timer = Timer(now + delay, operation)
        timers += timer
        return { timer.cancelled = true }
    }
    val asr = SpeechEngineCache<com.k2fsa.sherpa.onnx.OnlineRecognizerConfig, Int>(
        create = { if (failLoad) error("speech_native_failed"); ++asrBuilds },
        release = { asrFrees++ }, schedule = ::schedule, now = { now }, idleState = sessions::idleState,
    )
    val tts = SpeechEngineCache<String, Int>(
        create = { ++ttsBuilds }, release = { ttsFrees++ },
        schedule = ::schedule, now = { now }, idleState = sessions::idleState,
    )
    fun refresh() { asr.refreshIdle(); tts.refreshIdle() }
    fun pending() = timers.filter { !it.cancelled }
    val greedy = com.k2fsa.sherpa.onnx.OnlineRecognizerConfig(decodingMethod = "greedy_search")
    val beam = greedy.copy(decodingMethod = "modified_beam_search", maxActivePaths = 2, hotwordsScore = 1.5f)
    expectFailure { sessions.begin(first, emptyList()) }
    expectFailure { sessions.begin(first, listOf("asr", "asr")) }
    expectFailure { sessions.begin(first, listOf("unknown")) }
    val conversation = sessions.begin(first, listOf("asr", "tts"))
    expectFailure("speech_request_busy") { sessions.begin(first, listOf("asr", "tts")) }
    asr.use(greedy, markUsed = false) { conversation.check() }
    tts.use("catalog-voice", markUsed = false) { conversation.check() }
    check(asrBuilds == 1 && ttsBuilds == 1 && pending().isEmpty())
    repeat(3) {
        now += 60_000
        asr.use(greedy.copy()) { engine ->
            check(engine == 1)
            streams++
            try { conversation.check() } finally { streamFrees++ }
        }
        tts.use("catalog-voice") { check(it == 1) }
        check(pending().isEmpty())
    }
    check(asrBuilds == 1 && streams == 3 && streamFrees == 3)
    val ordinary = SpeechRequests { now }
    ordinary.finish(ordinary.begin(first))
    check(sessions.idleState().pinned)
    val recording = sessions.begin(second, listOf("asr"))
    sessions.release(first)
    refresh()
    check(pending().isEmpty())
    recording.check()
    expectFailure { conversation.check() }
    now += 1_000
    sessions.release(second)
    val lastRelease = now
    // 清理控制任务可能在 JNI 后才执行，但计时起点仍是最后一次 release。
    now += 2_000
    refresh()
    check(pending().size == 2 && pending().all { it.at == lastRelease + 30_000 })
    val oldTimers = pending().toList()
    val next = sessions.begin(third, listOf("asr"))
    oldTimers.forEach { it.operation() }
    check(asrFrees == 0 && ttsFrees == 0 && pending().isEmpty())
    sessions.release(first)
    sessions.finish(conversation)
    next.check()
    check(sessions.idleState().pinned)
    asr.use(beam, markUsed = false) { next.check() }
    check(asrBuilds == 2 && asrFrees == 1)
    asr.use(beam.copy(), markUsed = false) { next.check() }
    check(asrBuilds == 2)
    asr.use(greedy, markUsed = false) { next.check() }
    check(asrBuilds == 3 && asrFrees == 2)
    sessions.release(third)
    refresh()
    now += 30_000
    pending().toList().forEach { it.operation() }
    check(asrFrees == 3 && ttsFrees == 1)

    val cancelledId = "00000000-0000-4000-8000-000000000104"
    sessions.release(cancelledId)
    expectFailure { sessions.begin(cancelledId, listOf("asr")) }
    val late = sessions.begin("00000000-0000-4000-8000-000000000105", listOf("asr"))
    val newer = sessions.begin("00000000-0000-4000-8000-000000000106", listOf("tts"))
    // 模拟 release 在原生构造返回前发生，迟到构造必须释放句柄且不能复活旧 pin。
    val lateCache = SpeechEngineCache<String, Int>(
        create = { sessions.release(late.id); 1 }, release = { asrFrees++ },
        schedule = ::schedule, now = { now }, idleState = sessions::idleState,
    )
    expectFailure { lateCache.use("late", markUsed = false) { late.check() } }
    sessions.finish(late)
    newer.check()
    check(asrFrees == 4 && sessions.idleState().pinned)
    failLoad = true
    val failed = sessions.begin("00000000-0000-4000-8000-000000000107", listOf("asr"))
    try {
        expectFailure { asr.use(greedy, markUsed = false) { failed.check() } }
    } finally { sessions.finish(failed) }
    newer.check()
    check(sessions.idleState().pinned && asrBuilds == 3)
    failLoad = false
    asr.use(greedy, markUsed = false) { newer.check() }
    tts.use("catalog-voice", markUsed = false) { newer.check() }
    check(pending().isEmpty())
    sessions.pause()
    expectFailure { newer.check() }
    expectFailure { sessions.begin("00000000-0000-4000-8000-000000000108", listOf("asr")) }
    asr.invalidate()
    tts.invalidate()
    check(!sessions.idleState().pinned && asrFrees == 5 && ttsFrees == 2)
    sessions.resume()
    val resumed = sessions.begin("00000000-0000-4000-8000-000000000108", listOf("asr"))
    asr.use(greedy, markUsed = false) { resumed.check() }
    check(asrBuilds == 5)
    sessions.clear()
    expectFailure { resumed.check() }
    asr.invalidate()
    check(!sessions.idleState().pinned && asrFrees == 6)
    val queued = sessions.begin("00000000-0000-4000-8000-000000000109", listOf("asr"))
    sessions.release(queued.id)
    expectFailure {
        queued.check()
        asr.use(greedy, markUsed = false) { queued.check() }
    }
    sessions.finish(queued)
    check(asrBuilds == 5 && !sessions.idleState().pinned)
    val failedOnly = sessions.begin("00000000-0000-4000-8000-000000000110", listOf("asr"))
    failLoad = true
    try {
        expectFailure { asr.use(greedy, markUsed = false) { failedOnly.check() } }
    } finally { sessions.finish(failedOnly) }
    check(asrBuilds == 5 && !sessions.idleState().pinned)
    val bounded = SpeechSessions { now }
    val held = (1..16).map {
        bounded.begin("00000000-0000-4000-8000-" + (200 + it).toString().padStart(12, '0'), listOf("asr"))
    }
    val overflow = "00000000-0000-4000-8000-000000000300"
    expectFailure("speech_engine_busy") { bounded.begin(overflow, listOf("asr")) }
    held.forEach { it.check() }
    bounded.release(held.first().id)
    bounded.begin(overflow, listOf("asr")).check()
    bounded.clear()
    check(!bounded.idleState().pinned)
}

private fun testSpeechSessionChurn() {
    var now = 0L
    val sessions = SpeechSessions { now }
    val held = sessions.begin("00000000-0000-4000-8000-999999999999", listOf("asr"))
    fun id(index: Int) = "00000000-0000-4000-8000-" + index.toString().padStart(12, '0')
    // 模拟长时间正常轮换，累计超过请求表容量；过期 tombstone 应清理，但活跃 pin 不能清理。
    repeat(5_000) { index ->
        now += 1_000
        val request = sessions.begin(id(index), listOf("asr"))
        request.check()
        sessions.release(request.id)
        expectFailure("speech_cancelled") { request.check() }
        held.check()
    }
    expectFailure("speech_request_busy") { sessions.begin(held.id, listOf("asr")) }
    sessions.release(held.id)
    check(!sessions.idleState().pinned)
    now += 10 * 60_000
    expectFailure("speech_cancelled") { sessions.begin(id(4_999), listOf("asr")) }
    now += 1
    val reused = sessions.begin(id(4_999), listOf("asr"))
    reused.check()
    sessions.release(reused.id)
    check(!sessions.idleState().pinned)
}

fun main(args: Array<String>) {
    testEngineCache()
    testParallelPreload()
    testSpeechInferenceQueues()
    testConcurrentPrepareOrdering()
    testSpeechSessions()
    testSpeechSessionChurn()
    val dir = File(args[0]).canonicalFile
    val data = "verified speech resource".toByteArray()
    val spec = SpeechDownloadFile("data !/model.onnx", data.size.toLong(), sha(data), listOf("https://hf-mirror.com/model"))
    check(SpeechModelFiles.relativePath("espeak-ng-data/lang/a ! b") == "espeak-ng-data/lang/a ! b")
    for (path in listOf("", "..", "a/../b", "/root", "a//b", "a/./b", "C:/a", "a\\b", "a\u0000b")) {
        expectFailure { SpeechModelFiles.relativePath(path) }
    }
    for (url in listOf("http://hf-mirror.com/a", "https://127.0.0.1/a", "https://hf-mirror.com.evil/a", "https://u:p@hf-mirror.com/a", "https://hf-mirror.com:8443/a", "https://hf-mirror.com/a#b")) {
        expectFailure { SpeechModelFiles.trustedUrl(url) }
    }
    val file = SpeechModelFiles.child(dir, spec.path)
    val progress = mutableListOf<Long>()
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), { progress += it }) {
        Response(it, 200, data, mapOf("Content-Length" to data.size.toString()))
    }
    check(file.readBytes().contentEquals(data) && progress.last() == data.size.toLong())
    var cacheConnections = 0
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
        cacheConnections++; Response(it, 200, data)
    }
    check(cacheConnections == 0)

    file.writeBytes(data.copyOfRange(0, 7))
    val connections = mutableListOf<Response>()
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
        Response(it, 206, data.copyOfRange(7, data.size), mapOf("Content-Range" to ("bytes 7-" + (data.size - 1) + "/" + data.size)))
            .also { response -> connections += response }
    }
    check(connections.single().getRequestProperty("Range") == "bytes=7-")
    check(file.readBytes().contentEquals(data))

    file.writeBytes(data.copyOfRange(0, 5))
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) { Response(it, 200, data) }
    check(file.length() == data.size.toLong())
    file.writeBytes(byteArrayOf(0))
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            Response(it, 206, data, mapOf("Content-Range" to ("bytes 0-" + (data.size - 1) + "/" + data.size)))
        }
    }
    check(file.length() == 1L)

    file.delete()
    var redirects = 0
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
        redirects++
        if (it.host == "hf-mirror.com") Response(it, 302, byteArrayOf(), mapOf("Location" to "https://cas-bridge.xethub.hf.co/blob?signature=transient"))
        else Response(it, 200, data)
    }
    check(redirects == 2 && file.readBytes().contentEquals(data))
    file.delete()
    var privateConnections = 0
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            if (it.host == "127.0.0.1") privateConnections++
            Response(it, 302, byteArrayOf(), mapOf("Location" to "https://127.0.0.1/private"))
        }
    }
    check(privateConnections == 0 && !file.exists())

    val cancelled = SpeechDownloadCancellation()
    expectFailure {
        SpeechModelFiles.download(file, spec, cancelled, { if (it > 0) cancelled.cancelled.set(true) }) {
            Response(it, 200, data)
        }
    }
    check(cancelled.cancelled.get())
    SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) { Response(it, 200, data) }
    check(SpeechModelFiles.verify(file, spec))
    file.delete()
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            Response(it, 200, ByteArray(data.size) { 0 })
        }
    }
    check(!file.exists())
    expectFailure {
        SpeechModelFiles.download(file, spec, SpeechDownloadCancellation(), {}) {
            Response(it, 200, ByteArray(data.size + 1))
        }
    }
    check(file.length() <= data.size)

    val big = ByteArray(180_000) { (it % 127).toByte() }
    val bigFile = File(dir, "resume.bin")
    val bigSpec = SpeechDownloadFile("resume.bin", big.size.toLong(), sha(big), spec.urls)
    val midCancel = SpeechDownloadCancellation()
    expectFailure {
        SpeechModelFiles.download(bigFile, bigSpec, midCancel, { if (it > 0) midCancel.cancelled.set(true) }) {
            Response(it, 200, big)
        }
    }
    val retained = bigFile.length().toInt()
    check(retained > 0 && retained < big.size)
    SpeechModelFiles.download(bigFile, bigSpec, SpeechDownloadCancellation(), {}) {
        Response(it, 206, big.copyOfRange(retained, big.size),
            mapOf("Content-Range" to ("bytes " + retained + "-" + (big.size - 1) + "/" + big.size)))
    }
    check(bigFile.readBytes().contentEquals(big))

    val staged = File(dir, "staged").also { it.mkdirs() }
    val installed = File(dir, "installed").also { it.mkdirs() }
    val retired = File(dir, "retired")
    File(staged, "version").writeText("new")
    File(installed, "version").writeText("old")
    expectFailure {
        SpeechModelFiles.publish(staged, installed, retired, {}) { from, to ->
            if (from == staged) false else from.renameTo(to)
        }
    }
    check(File(installed, "version").readText() == "old" && staged.isDirectory && !retired.exists())
    expectFailure { SpeechModelFiles.publish(staged, installed, retired, { throw SpeechCancelled() }) }
    check(File(installed, "version").readText() == "old")
    SpeechModelFiles.publish(staged, installed, retired, {})
    check(File(installed, "version").readText() == "new" && File(retired, "version").readText() == "old")
    SpeechModelFiles.deleteTree(retired)
    check(!retired.exists())

    val archive = File(dir, "model.tar.bz2")
    val modelEntry = TarArchiveEntry("pkg/data !/model.onnx") to data
    val rootEntry = TarArchiveEntry("pkg/") to byteArrayOf()
    var extraction = 0
    fun extract(entries: List<Pair<TarArchiveEntry, ByteArray>>, maximum: Long = 1_000_000, entryLimit: Int = 100): File {
        makeArchive(archive, entries)
        val target = File(dir, "extracted-" + extraction++).also { it.mkdirs() }
        SpeechModelArchive.extract(archive, target, "pkg/", listOf(spec), {}, entryLimit, maximum)
        return target
    }
    val extracted = extract(listOf(rootEntry, modelEntry,
        TarArchiveEntry("pkg/README.md") to "readme".toByteArray(),
        TarArchiveEntry("pkg/test_wavs/sample.wav") to ByteArray(512)))
    check(File(extracted, spec.path).readBytes().contentEquals(data))
    check(!File(extracted, "README.md").exists() && !File(extracted, "test_wavs").exists())
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("pkg/../outside") to data)) }
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("/outside", true) to data)) }
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("other/file") to data)) }
    expectFailure { extract(listOf(rootEntry, modelEntry, modelEntry)) }
    expectFailure { extract(listOf(rootEntry)) }
    expectFailure { extract(listOf(rootEntry, TarArchiveEntry("pkg/data !/model.onnx") to ByteArray(data.size))) }
    expectFailure { extract(listOf(rootEntry, modelEntry), entryLimit = 1) }
    expectFailure { extract(listOf(rootEntry, modelEntry, TarArchiveEntry("pkg/skipped.bin") to ByteArray(8192)), maximum = 4096) }
    for (type in listOf(TarConstants.LF_SYMLINK, TarConstants.LF_LINK, TarConstants.LF_FIFO)) {
        val link = TarArchiveEntry("pkg/link", type).also { it.linkName = "../outside" }
        expectFailure { extract(listOf(rootEntry, modelEntry, link to byteArrayOf())) }
    }
    makeArchive(archive, listOf(rootEntry, modelEntry))
    expectFailure {
        SpeechModelArchive.extract(archive, File(dir, "cancel-extract"), "pkg/", listOf(spec), { throw SpeechCancelled() })
    }

    val id = "00000000-0000-4000-8000-000000000001"
    val other = "00000000-0000-4000-8000-000000000002"
    expectFailure { SpeechRequests.validateId("1-1-1-1-1") }
    var clock = 0L
    val requests = SpeechRequests { clock }
    requests.cancel(id)
    expectFailure { requests.begin(id) }
    val running = requests.begin(other)
    expectFailure { requests.begin(other) }
    requests.cancel(id)
    running.check()
    requests.pause()
    expectFailure { running.check() }
    expectFailure { requests.begin("00000000-0000-4000-8000-000000000003") }
    requests.resume()
    expectFailure { requests.begin(id) }
    requests.finish(running)
    clock += 11 * 60_000
    requests.begin(id).check()

    val wave = File(dir, "test.wav")
    SpeechWave.write(wave, floatArrayOf(0f, -1f, 1f, 2f), 24_000) {}
    val bytes = wave.readBytes()
    check(bytes.size == 52 && String(bytes.copyOfRange(0, 4)) == "RIFF")
    val header = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
    check(header.getInt(24) == 24_000 && header.getShort(34).toInt() == 16 && header.getInt(40) == 8)
    check(header.getShort(44).toInt() == 0 && header.getShort(46).toInt() == -32767 && header.getShort(50).toInt() == 32767)
    expectFailure { SpeechWave.write(wave, floatArrayOf(Float.NaN), 24_000) {} }
    expectFailure { SpeechWave.write(wave, FloatArray(8_000 * 45 + 1), 8_000) {} }
    expectFailure { SpeechWave.write(wave, floatArrayOf(0f), 192_000) {} }
    expectFailure { SpeechWave.write(wave, floatArrayOf(0f), 24_000) { throw SpeechCancelled() } }
    println("PASS: path/URL policy, streaming, resume, redirect, integrity, cancellation isolation, lifecycle, WAV limits, engine cache reuse and serial disposal")
}
`

test(
  'Android speech helper behavior and real AAR service signatures',
  { timeout: 120_000 },
  async () => {
    const dir = await mkdtemp(join(tmpdir(), 'pisper-speech-native-'))
    try {
      const toolchain = join(homedir(), '.pisper-android')
      const jdk =
        process.env.JAVA_HOME ||
        join(
          toolchain,
          (await readdir(toolchain)).find((name) => name.startsWith('jdk-')),
        )
      const java = join(jdk, 'bin', process.platform === 'win32' ? 'java.exe' : 'java')
      const jars = await Promise.all([
        jar('org.jetbrains.kotlin', 'kotlin-compiler-embeddable'),
        jar('org.jetbrains.kotlin', 'kotlin-stdlib'),
        jar('org.jetbrains.kotlin', 'kotlin-reflect'),
        jar('org.jetbrains.kotlin', 'kotlin-script-runtime'),
        jar('org.jetbrains.intellij.deps', 'trove4j'),
        jar('org.jetbrains', 'annotations'),
        jar('org.jetbrains.kotlinx', 'kotlinx-coroutines-core-jvm'),
      ])
      const commons = await archiveLibraries(dir)
      const android = join(
        process.env.ANDROID_HOME || join(toolchain, 'sdk'),
        'platforms/android-36/android.jar',
      )
      const aar = resolve(sourceDir, '../../../../../../libs/sherpa-onnx.aar')
      const extracted = join(dir, 'aar')
      await mkdir(extracted)
      const jarTool = join(jdk, 'bin', process.platform === 'win32' ? 'jar.exe' : 'jar')
      const extract = spawnSync(jarTool, ['xf', aar, 'classes.jar'], {
        cwd: extracted,
        encoding: 'utf8',
      })
      assert.equal(extract.status, 0, extract.stderr)
      const stub = join(dir, 'JSObject.kt')
      await writeFile(stub, 'package app.tauri.plugin\nclass JSObject : org.json.JSONObject()\n')
      const fixture = join(dir, 'SpeechNativeHarness.kt')
      await writeFile(fixture, harness)
      const classes = join(dir, 'classes')
      const libraryPath = [
        ...jars.slice(1),
        ...commons,
        android,
        join(extracted, 'classes.jar'),
      ].join(delimiter)
      execute(java, [
        '-cp',
        jars.join(delimiter),
        'org.jetbrains.kotlin.cli.jvm.K2JVMCompiler',
        '-no-stdlib',
        '-no-reflect',
        '-jvm-target',
        '1.8',
        '-classpath',
        libraryPath,
        '-d',
        classes,
        ...[
          'SpeechModelFiles.kt',
          'SpeechModelArchive.kt',
          'SpeechAudioState.kt',
          'SpeechModelStore.kt',
          'SpeechAudioService.kt',
        ].map((name) => join(sourceDir, name)),
        stub,
        fixture,
      ])
      const output = execute(java, [
        '-cp',
        [classes, ...jars.slice(1), ...commons, join(extracted, 'classes.jar')].join(delimiter),
        'app.pisper.mobiledevice.SpeechNativeHarnessKt',
        dir,
      ])
      assert.match(output, /PASS: path\/URL policy/)
      const bytecode = await readFile(join(extracted, 'classes.jar'))
      assert.ok(bytecode.length > 0)
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  },
)
