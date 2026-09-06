package app.pisper.mobiledevice

import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream
import java.io.File
import java.io.FileOutputStream
import java.io.FilterInputStream
import java.io.InputStream
import java.security.MessageDigest

internal data class SpeechArchive(val file: SpeechDownloadFile, val stripPrefix: String)

internal object SpeechModelArchive {
    private class BoundedInput(
        input: InputStream,
        private val maximum: Long,
        private val check: () -> Unit,
    ) : FilterInputStream(input) {
        private var bytes = 0L
        override fun read(): Int {
            check()
            val value = `in`.read()
            if (value >= 0) account(1)
            return value
        }
        override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
            check()
            val count = `in`.read(buffer, offset, minOf(length, 64 * 1024))
            if (count > 0) account(count)
            return count
        }
        private fun account(count: Int) {
            require(count.toLong() <= maximum - bytes) { "speech_archive_expansion_limit" }
            bytes += count
        }
        override fun skip(count: Long): Long {
            val buffer = ByteArray(8192)
            var remaining = count.coerceAtLeast(0)
            while (remaining > 0) {
                val size = read(buffer, 0, minOf(buffer.size.toLong(), remaining).toInt())
                if (size < 0) break
                remaining -= size
            }
            return count.coerceAtLeast(0) - remaining
        }
    }

    fun extract(
        archive: File,
        destination: File,
        stripPrefix: String,
        files: List<SpeechDownloadFile>,
        check: () -> Unit,
        maxEntries: Int = 20_000,
        maxExpandedBytes: Long = minOf(1024L * 1024 * 1024, files.sumOf { it.bytes } + 128L * 1024 * 1024),
    ) {
        require(stripPrefix.endsWith('/')) { "speech_archive_invalid_prefix" }
        val prefix = SpeechModelFiles.relativePath(stripPrefix.removeSuffix("/"))
        val expected = files.associateBy { it.path }
        require(expected.size == files.size && maxEntries > 0) { "speech_catalog_invalid_files" }
        val seen = mutableSetOf<String>()
        val extracted = mutableSetOf<String>()
        var entries = 0
        var declaredBytes = 0L
        val buffer = ByteArray(64 * 1024)
        archive.inputStream().buffered().use { compressed ->
            BZip2CompressorInputStream(compressed, true).use { bzip ->
                BoundedInput(bzip, maxExpandedBytes, check).use { bounded ->
                    TarArchiveInputStream(bounded, "UTF-8").use { tar ->
                        while (true) {
                            check()
                            val entry = tar.nextEntry ?: break
                            require(++entries <= maxEntries) { "speech_archive_entry_limit" }
                            require(entry.isCheckSumOK) { "speech_archive_invalid_header" }
                            require(!entry.isSymbolicLink && !entry.isLink && !entry.isSparse &&
                                (entry.isFile || entry.isDirectory) && entry.linkName.isNullOrEmpty()
                            ) { "speech_archive_unsupported_entry" }
                            val path = SpeechModelFiles.relativePath(
                                if (entry.isDirectory) entry.name.removeSuffix("/") else entry.name,
                            )
                            require(path.split('/').size <= 64 && seen.add(path)) { "speech_archive_invalid_path" }
                            require(path == prefix || path.startsWith("$prefix/")) { "speech_archive_invalid_prefix" }
                            require(entry.size >= 0 && entry.size <= maxExpandedBytes - declaredBytes) {
                                "speech_archive_expansion_limit"
                            }
                            declaredBytes += entry.size
                            if (entry.isDirectory) {
                                require(entry.size == 0L) { "speech_archive_invalid_directory" }
                                continue
                            }
                            require(path != prefix && tar.canReadEntryData(entry)) { "speech_archive_unsupported_entry" }
                            val relative = path.removePrefix("$prefix/")
                            val spec = expected[relative]
                            if (spec != null) require(entry.size == spec.bytes) { "speech_archive_length_mismatch" }
                            val outputFile = spec?.let { SpeechModelFiles.child(destination, it.path) }
                            if (outputFile != null) {
                                require(outputFile.parentFile!!.isDirectory || outputFile.parentFile!!.mkdirs()) {
                                    "speech_storage_error"
                                }
                                require(!outputFile.exists()) { "speech_archive_duplicate_file" }
                            }
                            val digest = if (spec != null) MessageDigest.getInstance("SHA-256") else null
                            var received = 0L
                            val output = outputFile?.let(::FileOutputStream)
                            try {
                                // 即使 README/test_wavs 不安装，也读完并计入预算，不能跳过恶意展开量。
                                while (true) {
                                    check()
                                    val count = tar.read(buffer)
                                    if (count < 0) break
                                    require(count.toLong() <= entry.size - received) { "speech_archive_length_mismatch" }
                                    received += count
                                    digest?.update(buffer, 0, count)
                                    output?.write(buffer, 0, count)
                                }
                                require(received == entry.size) { "speech_archive_incomplete" }
                                output?.fd?.sync()
                            } finally { output?.close() }
                            if (spec != null) {
                                val actual = digest!!.digest().joinToString("") { "%02x".format(it) }
                                require(actual == spec.sha256) { "speech_archive_checksum_mismatch" }
                                extracted.add(relative)
                            }
                        }
                        // 读完压缩流以校验 BZip2 CRC；尾部只能是 TAR 的零填充，且仍受展开预算限制。
                        while (true) {
                            val count = bounded.read(buffer)
                            if (count < 0) break
                            require((0 until count).all { buffer[it] == 0.toByte() }) { "speech_archive_trailing_data" }
                        }
                    }
                }
            }
        }
        check()
        require(extracted == expected.keys) { "speech_archive_missing_files" }
    }
}
