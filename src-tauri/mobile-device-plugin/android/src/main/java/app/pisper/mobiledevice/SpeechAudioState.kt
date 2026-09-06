package app.pisper.mobiledevice

import java.io.File
import java.io.FileOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.UUID
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
