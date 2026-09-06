package app.pisper.mobiledevice

import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.URI
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean

internal class SpeechCancelled : IllegalStateException("speech_cancelled")

internal class SpeechDownloadCancellation {
    val cancelled = AtomicBoolean(false)
    fun check() {
        if (cancelled.get()) throw SpeechCancelled()
    }
}

internal data class SpeechDownloadFile(
    val path: String,
    val bytes: Long,
    val sha256: String,
    val urls: List<String>,
)

internal object SpeechModelFiles {
    private val trustedHosts = setOf(
        "hf-mirror.com", "huggingface.co", "cas-bridge.xethub.hf.co",
        "ghfast.top", "github.com", "release-assets.githubusercontent.com",
        "cdn-lfs.huggingface.co", "cdn-lfs-us-1.huggingface.co", "cdn-lfs-eu-1.huggingface.co",
    )

    fun relativePath(value: String): String {
        require(value.isNotEmpty() && value.length <= 1024 && !value.startsWith('/') &&
            !value.contains('\\') && !value.contains(':') && value.none { it.isISOControl() } &&
            value.split('/').all { it.isNotEmpty() && it != "." && it != ".." }
        ) { "speech_invalid_path" }
        return value
    }

    fun child(root: File, path: String): File {
        relativePath(path)
        val canonicalRoot = root.canonicalFile
        val child = File(canonicalRoot, path)
        // canonical 必须与逐段构造结果一致，连指向根内的符号链接也不能当作模型文件。
        require(child.canonicalFile == child.absoluteFile &&
            child.canonicalPath.startsWith(canonicalRoot.path + File.separator)
        ) { "speech_invalid_path" }
        return child
    }

    fun trustedUrl(value: String): URL {
        val uri = URI(value)
        require(uri.scheme == "https" && uri.host in trustedHosts &&
            uri.rawUserInfo == null && uri.fragment == null && uri.port in setOf(-1, 443)
        ) { "speech_untrusted_download_url" }
        return uri.toURL()
    }

    private fun checkAddress(url: URL) {
        val addresses = InetAddress.getAllByName(url.host)
        require(addresses.isNotEmpty() && addresses.none { address ->
            address.isAnyLocalAddress || address.isLoopbackAddress || address.isLinkLocalAddress ||
                address.isSiteLocalAddress || address.isMulticastAddress ||
                (address.address.size == 16 && (address.address[0].toInt() and 0xfe) == 0xfc) ||
                (address.address.size == 4 && (address.address[0].toInt() and 255) == 100 &&
                    (address.address[1].toInt() and 255) in 64..127)
        }) { "speech_private_download_address" }
    }

    fun digest(file: File, check: () -> Unit = {}): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                check()
                val size = input.read(buffer)
                if (size < 0) break
                digest.update(buffer, 0, size)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    fun verify(file: File, spec: SpeechDownloadFile, check: () -> Unit = {}): Boolean =
        file.isFile && file.length() == spec.bytes && digest(file, check) == spec.sha256

    fun download(
        file: File,
        spec: SpeechDownloadFile,
        cancellation: SpeechDownloadCancellation,
        progress: (Long) -> Unit,
        connect: (URL) -> HttpURLConnection = { url ->
            checkAddress(url)
            url.openConnection() as HttpURLConnection
        },
    ) {
        require(spec.bytes >= 0 && spec.bytes <= 2L * 1024 * 1024 * 1024 &&
            Regex("^[a-f0-9]{64}$").matches(spec.sha256) && spec.urls.isNotEmpty()
        ) { "speech_invalid_file_manifest" }
        require(file.parentFile!!.isDirectory || file.parentFile!!.mkdirs()) { "speech_storage_error" }
        if (file.exists() && (file.length() > spec.bytes || !file.isFile)) {
            require(file.delete()) { "speech_storage_error" }
        }
        if (file.isFile && file.length() == spec.bytes) {
            if (verify(file, spec, cancellation::check)) {
                progress(spec.bytes)
                return
            }
            require(file.delete()) { "speech_storage_error" }
        }
        var succeeded = false
        for (source in spec.urls) {
            for (attempt in 0..1) {
                cancellation.check()
                try {
                    transfer(file, spec, source, cancellation, progress, connect)
                    cancellation.check()
                    require(verify(file, spec, cancellation::check)) { "speech_download_checksum_mismatch" }
                    succeeded = true
                    break
                } catch (error: Exception) {
                    cancellation.check()
                    // 摘要错误的完整文件不能再次以 Range 尾部续传；签名跳转地址从不写入磁盘。
                    if (file.isFile && file.length() >= spec.bytes) file.delete()
                    if (attempt == 1 && source == spec.urls.last()) throw error
                }
            }
            if (succeeded) return
        }
        throw IllegalStateException("speech_download_failed")
    }

    private fun transfer(
        file: File,
        spec: SpeechDownloadFile,
        source: String,
        cancellation: SpeechDownloadCancellation,
        progress: (Long) -> Unit,
        connect: (URL) -> HttpURLConnection,
    ) {
        var url = trustedUrl(source)
        val offset = if (file.isFile) file.length() else 0L
        for (hop in 0..5) {
            cancellation.check()
            val connection = connect(url)
            try {
                connection.instanceFollowRedirects = false
                connection.connectTimeout = 15_000
                connection.readTimeout = 5_000
                connection.setRequestProperty("Accept-Encoding", "identity")
                connection.setRequestProperty("User-Agent", "Pisper-Android-Speech")
                if (offset > 0) connection.setRequestProperty("Range", "bytes=$offset-")
                val status = connection.responseCode
                if (status in setOf(301, 302, 303, 307, 308)) {
                    require(hop < 5) { "speech_download_redirect_limit" }
                    url = trustedUrl(URL(url, connection.getHeaderField("Location")
                        ?: throw IllegalStateException("speech_download_redirect_missing")).toString())
                    continue
                }
                require(status == 200 || status == 206) { "speech_download_http_$status" }
                require(connection.contentEncoding.isNullOrEmpty() || connection.contentEncoding == "identity") {
                    "speech_download_unexpected_encoding"
                }
                var received = if (status == 206) offset else 0L
                if (status == 206) {
                    val range = Regex("^bytes ([0-9]+)-([0-9]+)/([0-9]+)$")
                        .matchEntire(connection.getHeaderField("Content-Range").orEmpty())
                        ?: throw IllegalStateException("speech_download_invalid_range")
                    require(range.groupValues[1].toLong() == offset &&
                        range.groupValues[2].toLong() == spec.bytes - 1 &&
                        range.groupValues[3].toLong() == spec.bytes
                    ) { "speech_download_invalid_range" }
                }
                val length = connection.getHeaderField("Content-Length")?.toLongOrNull()
                require(length == null || length == spec.bytes - received) { "speech_download_length_mismatch" }
                progress(received)
                connection.inputStream.use { input ->
                    FileOutputStream(file, status == 206 && offset > 0).use { output ->
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            cancellation.check()
                            val count = input.read(buffer)
                            if (count < 0) break
                            require(count.toLong() <= spec.bytes - received) { "speech_download_too_large" }
                            output.write(buffer, 0, count)
                            received += count
                            progress(received)
                        }
                        output.fd.sync()
                    }
                }
                require(received == spec.bytes) { "speech_download_incomplete" }
                return
            } finally {
                connection.disconnect()
            }
        }
        throw IllegalStateException("speech_download_redirect_limit")
    }

    fun publish(
        staging: File,
        target: File,
        retired: File,
        check: () -> Unit,
        rename: (File, File) -> Boolean = { from, to -> from.renameTo(to) },
    ) {
        check()
        val hadOld = target.exists()
        if (hadOld) require(rename(target, retired)) { "speech_storage_error" }
        if (!rename(staging, target)) {
            if (hadOld) require(rename(retired, target)) { "speech_storage_recovery_required" }
            throw IllegalStateException("speech_storage_error")
        }
    }

    fun deleteTree(root: File) {
        if (!root.exists()) return
        require(root.canonicalFile == root.absoluteFile) { "speech_invalid_path" }
        root.listFiles()?.forEach { child ->
            require(child.canonicalFile == child.absoluteFile) { "speech_invalid_path" }
            if (child.isDirectory) deleteTree(child) else require(child.delete()) { "speech_storage_error" }
        }
        require(root.delete()) { "speech_storage_error" }
    }
}
