package app.pisper.mobiledevice

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.InputStream
import java.io.OutputStream

internal class SafWorkspaceDocumentSource(
    private val resolver: ContentResolver,
    private val tree: Uri,
) : WorkspaceDocumentSource {
    private val rootId: String
    private val columns = arrayOf(
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE,
        DocumentsContract.Document.COLUMN_FLAGS,
    )

    init {
        require(tree.scheme == "content" && !tree.authority.isNullOrEmpty() &&
            DocumentsContract.isTreeUri(tree)
        ) { "System picker did not return a document tree" }
        rootId = DocumentsContract.getTreeDocumentId(tree)
        require(rootId.isNotEmpty()) { "Document tree ID is empty" }
    }

    override fun root(): WorkspaceDocument {
        val uri = DocumentsContract.buildDocumentUriUsingTree(tree, rootId)
        return checkNotNull(resolver.query(uri, columns, null, null, null)) {
            "Unable to read selected directory"
        }.use { cursor ->
            checkComplete(cursor)
            check(cursor.moveToFirst()) { "Selected directory is unavailable" }
            val document = readDocument(cursor)
            require(document.id == rootId && !cursor.moveToNext()) { "Invalid selected directory metadata" }
            document
        }
    }

    override fun children(document: WorkspaceDocument, visit: (WorkspaceDocument) -> Unit) {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, document.id)
        checkNotNull(resolver.query(uri, columns, null, null, null)) {
            "Unable to enumerate selected directory"
        }.use { cursor ->
            checkComplete(cursor)
            // 逐项访问，避免恶意提供方在数量限制生效前返回无界列表。
            while (cursor.moveToNext()) visit(readDocument(cursor))
            checkComplete(cursor)
        }
    }

    override fun open(document: WorkspaceDocument): InputStream =
        checkNotNull(resolver.openInputStream(DocumentsContract.buildDocumentUriUsingTree(tree, document.id))) {
            "Unable to read selected document"
        }

    private fun checkComplete(cursor: Cursor) {
        val extras = cursor.extras
        check(!extras.getBoolean(DocumentsContract.EXTRA_LOADING, false) &&
            !extras.containsKey(DocumentsContract.EXTRA_ERROR)
        ) { "Document provider has not finished loading the directory; retry the import" }
    }

    private fun readDocument(cursor: Cursor): WorkspaceDocument {
        val id = cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID))
        val name = cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME))
        val mime = cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE))
        val flags = cursor.getLong(cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_FLAGS))
        require(!id.isNullOrEmpty() && !name.isNullOrEmpty() && !mime.isNullOrEmpty()) {
            "Document provider returned incomplete metadata"
        }
        require(flags and DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT.toLong() == 0L) {
            "Virtual documents cannot be imported as workspace files"
        }
        return WorkspaceDocument(id, name, mime == DocumentsContract.Document.MIME_TYPE_DIR)
    }
}

internal fun createWorkspaceFile(file: File): OutputStream {
    // O_EXCL 原子拒绝已有文件，O_NOFOLLOW 防止并发替换为符号链接后越界写入。
    val descriptor = Os.open(file.absolutePath,
        OsConstants.O_WRONLY or OsConstants.O_CREAT or OsConstants.O_EXCL or OsConstants.O_NOFOLLOW,
        384,
    )
    val parcel = try {
        ParcelFileDescriptor.dup(descriptor)
    } finally {
        Os.close(descriptor)
    }
    return ParcelFileDescriptor.AutoCloseOutputStream(parcel)
}
