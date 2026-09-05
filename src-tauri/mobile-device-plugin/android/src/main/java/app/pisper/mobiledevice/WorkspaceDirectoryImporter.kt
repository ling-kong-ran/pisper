package app.pisper.mobiledevice

import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.util.UUID

internal data class WorkspaceDocument(val id: String, val name: String, val directory: Boolean)

internal interface WorkspaceDocumentSource {
    fun root(): WorkspaceDocument
    fun children(document: WorkspaceDocument, visit: (WorkspaceDocument) -> Unit)
    fun open(document: WorkspaceDocument): InputStream
}

internal class WorkspaceDirectoryImporter(
    private val createFile: (File) -> OutputStream,
    private val maxBytes: Long = 256L * 1024 * 1024,
    private val maxEntries: Int = 10_000,
    private val maxDepth: Int = 64,
) {
    init {
        require(maxBytes in 0..256L * 1024 * 1024)
        require(maxEntries in 1..10_000)
        require(maxDepth in 1..64)
    }

    fun importDirectory(appDataDir: File, destinationRoot: String, source: WorkspaceDocumentSource): File {
        val workspaces = File(appDataDir.canonicalFile, "local-runtime-data/workspace")
        val requested = File(destinationRoot)
        // Tauri 的 app_data_dir 对应 Android dataDir；只接受 Runtime 共享目录，不能写任意私有路径。
        require(requested.isAbsolute &&
            (requested == workspaces || requested == File(appDataDir, "local-runtime-data/workspace")) &&
            requested.canonicalFile == workspaces && workspaces.canonicalFile == workspaces
        ) { "Workspace destination must be the app runtime workspace directory" }
        val root = source.root()
        require(root.directory) { "Selected document is not a directory" }
        validateName(root.name)
        check(workspaces.isDirectory || workspaces.mkdirs()) { "Unable to create workspace directory" }
        // 随机独占目录既是暂存区也是最终父目录；只在完整导入后向调用方公开路径。
        val staging = File(workspaces, "import-${UUID.randomUUID()}")
        check(staging.mkdir()) { "Unable to create workspace staging directory" }
        try {
            val visitedIds = HashSet<String>()
            val visitedPaths = HashSet<String>()
            var entries = 0
            var bytes = 0L
            val buffer = ByteArray(64 * 1024)

            fun copy(document: WorkspaceDocument, parent: File, depth: Int): File {
                require(depth <= maxDepth) { "Workspace exceeds the directory depth limit ($maxDepth)" }
                require(++entries <= maxEntries) { "Workspace exceeds the entry limit ($maxEntries)" }
                validateName(document.name)
                require(document.id.isNotEmpty() && visitedIds.add(document.id)) {
                    "Workspace contains a duplicate document or cycle"
                }
                val target = File(parent, document.name)
                require(target.canonicalFile == target.absoluteFile && target.parentFile == parent) {
                    "Workspace contains an unsafe path"
                }
                require(visitedPaths.add(target.absolutePath) && !target.exists()) {
                    "Workspace contains a duplicate path"
                }
                if (document.directory) {
                    check(target.mkdir()) { "Unable to create imported directory" }
                    source.children(document) { child -> copy(child, target, depth + 1) }
                } else {
                    source.open(document).use { input ->
                        createFile(target).use { output ->
                            while (true) {
                                val count = input.read(buffer)
                                if (count < 0) break
                                check(count > 0) { "Document provider returned an invalid stream" }
                                require(count.toLong() <= maxBytes - bytes) {
                                    "Workspace exceeds the byte limit ($maxBytes)"
                                }
                                output.write(buffer, 0, count)
                                bytes += count
                            }
                        }
                    }
                }
                return target
            }

            return copy(root, staging, 1)
        } catch (error: Throwable) {
            try {
                removeStaging(staging)
            } catch (cleanupError: Throwable) {
                error.addSuppressed(cleanupError)
            }
            throw error
        }
    }

    private fun validateName(name: String) {
        require(name.isNotBlank() && name != "." && name != ".." &&
            name.toByteArray(Charsets.UTF_8).size <= 255 && Charsets.UTF_8.newEncoder().canEncode(name) &&
            name.none { it == '/' || it == '\\' || it.isISOControl() ||
                Character.getType(it) == Character.FORMAT.toInt() }
        ) { "Document provider returned an unsafe name" }
    }

    private fun removeStaging(file: File) {
        // 清理也不能跟随符号链接，避免异常路径影响已有工作区。
        if (file.canonicalFile == file.absoluteFile && file.isDirectory) {
            val children = checkNotNull(file.listFiles()) { "Unable to read staging directory for cleanup" }
            children.forEach(::removeStaging)
        }
        check(file.delete()) { "Unable to remove workspace staging entry" }
    }
}
