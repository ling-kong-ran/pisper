import CryptoKit
import Darwin
import Foundation
import LibArchive

enum SpeechModelArchiveError: String, Error, LocalizedError {
    case catalog = "speech_archive_invalid_catalog"
    case path = "speech_archive_invalid_path"
    case staging = "speech_archive_requires_new_private_staging"
    case format = "speech_archive_unsupported_format"
    case metadata = "speech_archive_metadata_limit"
    case entryLimit = "speech_archive_entry_limit"
    case expansion = "speech_archive_expansion_limit"
    case integrity = "speech_archive_integrity_error"
    case length = "speech_archive_length_mismatch"
    case missing = "speech_archive_missing_files"
    case storage = "speech_archive_storage_error"

    var errorDescription: String? { rawValue }
}

enum SpeechModelArchive {
    private static let blockSize = 64 * 1024
    private static let extraBudget: Int64 = 32 * 1024 * 1024
    private static let metadataLimit: Int64 = 64 * 1024
    private static let entryLimit = 4096

    // destination 必须尚不存在；发布、压缩文件的 catalog 摘要校验由 Store 负责。
    static func extract(
        _ archiveURL: URL,
        _ destinationURL: URL,
        _ stripPrefix: String,
        _ files: [SpeechDownloadFile],
        _ checkCancellation: () throws -> Void
    ) throws {
        try checkCancellation()
        guard stripPrefix.hasSuffix("/") else { throw SpeechModelArchiveError.catalog }
        let prefix = String(stripPrefix.dropLast())
        _ = try components(prefix)
        let (expected, budget) = try manifest(files)
        let input = try openInput(archiveURL)
        let destination = try absoluteComponents(destinationURL)
        guard let leaf = destination.last else { throw SpeechModelArchiveError.staging }
        let parent = try openDirectory(Array(destination.dropLast()))
        let parentStat = try directoryStat(parent.raw)
        guard parentStat.st_uid == geteuid(), parentStat.st_mode & 0o077 == 0 else {
            throw SpeechModelArchiveError.staging
        }
        guard mkdirat(parent.raw, leaf, 0o700) == 0 else {
            throw SpeechModelArchiveError.staging
        }
        var succeeded = false
        var createdFiles = [String]()
        var createdDirectories = [String]()
        var root: Descriptor?
        defer {
            if !succeeded, let root {
                // 只删除本次创建的名字，不递归跟随可能被其他代码插入的链接。
                cleanup(root.raw, files: createdFiles, directories: createdDirectories)
            }
            if !succeeded, let root, sameDirectory(parent.raw, leaf, root.raw) {
                _ = unlinkat(parent.raw, leaf, AT_REMOVEDIR)
            }
        }
        root = try openChildDirectory(parent.raw, leaf)
        guard let root else { throw SpeechModelArchiveError.storage }
        let rootStat = try directoryStat(root.raw)
        guard rootStat.st_uid == geteuid(), rootStat.st_mode & 0o077 == 0 else {
            throw SpeechModelArchiveError.staging
        }
        let maximumEntry = max(extraBudget, files.map(\.bytes).max() ?? 0)
        let validatedTar = try preflight(input.raw, root.raw, budget, maximumEntry, checkCancellation)
        try input.closeChecked()
        guard let reader = archive_read_new() else { throw SpeechModelArchiveError.storage }
        var readerClosed = false
        var readerFreed = false
        defer {
            if !readerClosed { _ = archive_read_close(reader) }
            if !readerFreed { _ = archive_read_free(reader) }
        }
        // 此 reader 只能看到完成原始预检的、无名称且只读的 TAR 文件。
        try archiveOK(archive_read_support_filter_none(reader))
        try archiveOK(archive_read_support_format_tar(reader))
        try archiveOK(archive_read_open_fd(reader, validatedTar.raw, blockSize))
        guard archive_filter_count(reader) == 1,
              archive_filter_code(reader, 0) == ARCHIVE_FILTER_NONE else {
            throw SpeechModelArchiveError.format
        }
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: blockSize, alignment: 16)
        defer { buffer.deallocate() }
        var seen = [String: Bool]()
        var found = Set<String>()
        var declared: Int64 = 0
        var received: Int64 = 0
        var entries = 0
        while true {
            try checkCancellation()
            var entry: OpaquePointer?
            let status = archive_read_next_header(reader, &entry)
            try checkExpansion(reader, budget)
            if status == ARCHIVE_EOF { break }
            try archiveOK(status)
            guard let entry else { throw SpeechModelArchiveError.integrity }
            entries += 1
            guard entries <= entryLimit else { throw SpeechModelArchiveError.entryLimit }
            let format = archive_format(reader)
            guard format == ARCHIVE_FORMAT_TAR || format == ARCHIVE_FORMAT_TAR_USTAR ||
                    format == ARCHIVE_FORMAT_TAR_GNUTAR else {
                throw SpeechModelArchiveError.format
            }
            // Swift 无法导入 libarchive 的组合宏，展开全部 NFS4 位以保留 ACL 拒绝语义。
            guard archive_entry_symlink(entry) == nil, archive_entry_hardlink(entry) == nil,
                  archive_entry_sparse_count(entry) == 0,
                  archive_entry_xattr_count(entry) == 0,
                  archive_entry_acl_count(entry, ARCHIVE_ENTRY_ACL_TYPE_ACCESS | ARCHIVE_ENTRY_ACL_TYPE_DEFAULT |
                    ARCHIVE_ENTRY_ACL_TYPE_ALLOW | ARCHIVE_ENTRY_ACL_TYPE_DENY |
                    ARCHIVE_ENTRY_ACL_TYPE_AUDIT | ARCHIVE_ENTRY_ACL_TYPE_ALARM) == 0 else {
                throw SpeechModelArchiveError.path
            }
            let type = archive_entry_filetype(entry)
            let isDirectory = type == mode_t(S_IFDIR)
            guard isDirectory || type == mode_t(S_IFREG) else { throw SpeechModelArchiveError.path }
            guard archive_entry_size_is_set(entry) != 0 else { throw SpeechModelArchiveError.length }
            let size = archive_entry_size(entry)
            guard size >= 0, size <= budget - declared else { throw SpeechModelArchiveError.expansion }
            declared += size
            guard !isDirectory || size == 0 else { throw SpeechModelArchiveError.length }
            var name = try entryName(archive_entry_pathname_utf8(entry))
            if isDirectory, name.hasSuffix("/") { name.removeLast() }
            _ = try components(name)
            guard (isDirectory && name == prefix) || name.hasPrefix(stripPrefix) else {
                throw SpeechModelArchiveError.path
            }
            let relative = name == prefix ? "" : String(name.dropFirst(stripPrefix.count))
            let key = collisionKey(relative)
            guard seen[key] == nil else { throw SpeechModelArchiveError.path }
            for (other, otherIsDirectory) in seen {
                if (!otherIsDirectory && key.hasPrefix(other + "/")) ||
                    (!isDirectory && other.hasPrefix(key + "/")) {
                    throw SpeechModelArchiveError.path
                }
            }
            seen[key] = isDirectory
            guard key != ".installation.json", !key.hasPrefix(".installation.json/") else {
                throw SpeechModelArchiveError.path
            }
            let spec = expected[relative]
            if let spec, isDirectory || spec.bytes != size { throw SpeechModelArchiveError.length }
            if !isDirectory, relative.isEmpty { throw SpeechModelArchiveError.path }
            // 归档中的目录和 extra 文件不驱动落盘，只为白名单文件创建必需祖先。
            let output: Descriptor?
            if let spec {
                output = try createOutput(root.raw, spec.path, &createdDirectories, &createdFiles)
            } else {
                output = nil
            }
            var hash = SHA256()
            var entryBytes: Int64 = 0
            while true {
                try checkCancellation()
                let count = archive_read_data(reader, buffer, blockSize)
                guard count >= 0 else { throw SpeechModelArchiveError.integrity }
                try checkExpansion(reader, budget)
                if count == 0 { break }
                guard Int64(count) <= size - entryBytes, Int64(count) <= budget - received else {
                    throw SpeechModelArchiveError.length
                }
                entryBytes += Int64(count)
                received += Int64(count)
                if let output {
                    hash.update(bufferPointer: UnsafeRawBufferPointer(start: buffer, count: count))
                    try writeAll(output.raw, buffer, count, checkCancellation)
                }
            }
            guard entryBytes == size else { throw SpeechModelArchiveError.length }
            if let spec, let output {
                let digest = hash.finalize().map { String(format: "%02x", $0) }.joined()
                guard digest == spec.sha256.lowercased() else { throw SpeechModelArchiveError.integrity }
                let info = try regularStat(output.raw)
                guard info.st_size == spec.bytes else { throw SpeechModelArchiveError.length }
                try checkCancellation()
                guard fsync(output.raw) == 0 else { throw SpeechModelArchiveError.storage }
                try output.closeChecked()
                found.insert(spec.path)
            }
        }
        try checkCancellation()
        guard found == Set(expected.keys) else { throw SpeechModelArchiveError.missing }
        let closeStatus = archive_read_close(reader)
        readerClosed = true
        try archiveOK(closeStatus)
        let freeStatus = archive_read_free(reader)
        readerFreed = true
        try archiveOK(freeStatus)
        try validatedTar.closeChecked()
        guard fsync(root.raw) == 0 else { throw SpeechModelArchiveError.storage }
        try checkCancellation()
        guard sameDirectory(parent.raw, leaf, root.raw) else { throw SpeechModelArchiveError.staging }
        succeeded = true
    }

    private static func preflight(
        _ input: Int32, _ directory: Int32, _ maximum: Int64, _ maximumEntry: Int64,
        _ check: () throws -> Void
    ) throws -> Descriptor {
        let name = ".speech-preflight-" + UUID().uuidString + ".tar"
        let output = try Descriptor(openat(directory, name, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600))
        var linked = true
        defer { if linked { _ = unlinkat(directory, name, 0) } }
        let before = try regularStat(output.raw)
        let validated = try Descriptor(openat(directory, name, O_RDONLY | O_NOFOLLOW | O_CLOEXEC))
        let after = try regularStat(validated.raw)
        guard before.st_dev == after.st_dev, before.st_ino == after.st_ino else {
            throw SpeechModelArchiveError.staging
        }
        // 两个 FD 固定同一 inode；立即移除名字，不留下取消、崩溃或重试可复用的半成品。
        guard unlinkat(directory, name, 0) == 0 else { throw SpeechModelArchiveError.storage }
        linked = false
        guard let reader = archive_read_new() else { throw SpeechModelArchiveError.storage }
        var readerClosed = false
        var readerFreed = false
        defer {
            if !readerClosed { _ = archive_read_close(reader) }
            if !readerFreed { _ = archive_read_free(reader) }
        }
        // RAW 不解析 TAR/PAX，只提供成熟 bzip2 解压；任何 TAR reader 创建前完成全部预检。
        // 强制格式和 filter 的 API 自行注册实现，预先 support 会重复注册并返回 WARN。
        try archiveOK(archive_read_set_format(reader, ARCHIVE_FORMAT_RAW))
        try archiveOK(archive_read_append_filter(reader, ARCHIVE_FILTER_BZIP2))
        try archiveOK(archive_read_open_fd(reader, input, blockSize))
        guard archive_filter_count(reader) == 2,
              archive_filter_code(reader, 0) == ARCHIVE_FILTER_BZIP2,
              archive_filter_code(reader, 1) == ARCHIVE_FILTER_NONE else {
            throw SpeechModelArchiveError.format
        }
        var entry: OpaquePointer?
        try check()
        try archiveOK(archive_read_next_header(reader, &entry))
        guard entry != nil, archive_format(reader) == ARCHIVE_FORMAT_RAW else {
            throw SpeechModelArchiveError.format
        }
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: blockSize, alignment: 16)
        defer { buffer.deallocate() }
        var framing = RawTarGuard(maximum: maximum, maximumEntry: maximumEntry)
        while true {
            try check()
            let count = archive_read_data(reader, buffer, blockSize)
            guard count >= 0 else { throw SpeechModelArchiveError.integrity }
            if count == 0 { break }
            // 在落盘及 TAR 解析前拒绝越界块；metadata 的输入大小不决定内存分配大小。
            try framing.consume(UnsafeRawBufferPointer(start: buffer, count: count))
            try writeAll(output.raw, buffer, count, check)
        }
        try framing.finish()
        try check()
        guard archive_read_next_header(reader, &entry) == ARCHIVE_EOF else {
            throw SpeechModelArchiveError.integrity
        }
        let compressed = try regularStat(input)
        guard archive_filter_bytes(reader, -1) == compressed.st_size else {
            throw SpeechModelArchiveError.integrity
        }
        let closeStatus = archive_read_close(reader)
        readerClosed = true
        try archiveOK(closeStatus)
        let freeStatus = archive_read_free(reader)
        readerFreed = true
        try archiveOK(freeStatus)
        guard fsync(output.raw) == 0 else { throw SpeechModelArchiveError.storage }
        try output.closeChecked()
        try check()
        // 只读 FD 未参与写入，偏移仍为零；禁止通过名称重新打开已预检文件。
        return validated
    }

    // 这里只检查原始 framing/预算，不合并路径、不解释条目、不进行实际 TAR 解包。
    // 内部可见性用于 XCTest 直接证明恶意声明在接收 metadata 或调用 TAR parser 前被拒绝。
    struct RawTarGuard {
        private let maximum: Int64
        private let maximumEntry: Int64
        private var header = [UInt8](repeating: 0, count: 512)
        private var metadata = [UInt8](repeating: 0, count: 64 * 1024)
        private var headerBytes = 0
        private var bodyRemaining: Int64 = 0
        private var paddingRemaining = 0
        private var metadataKind: UInt8 = 0
        private var metadataSize = 0
        private(set) var bufferedMetadataBytes = 0
        private var position: Int64 = 0
        private var trailingBytes: Int64 = 0
        private var pendingExtension = false
        private(set) var expandedBytes: Int64 = 0
        private(set) var physicalEntries = 0

        init(maximum: Int64, maximumEntry: Int64) {
            self.maximum = maximum
            self.maximumEntry = maximumEntry
        }

        mutating func consume(_ bytes: UnsafeRawBufferPointer) throws {
            guard maximum >= 0, maximumEntry >= 0, Int64(bytes.count) <= maximum - expandedBytes else {
                throw SpeechModelArchiveError.expansion
            }
            expandedBytes += Int64(bytes.count)
            var offset = 0
            while offset < bytes.count {
                if trailingBytes > 0 {
                    guard bytes[offset...].allSatisfy({ $0 == 0 }) else { throw SpeechModelArchiveError.integrity }
                    trailingBytes += Int64(bytes.count - offset)
                    position += Int64(bytes.count - offset)
                    return
                }
                if bodyRemaining > 0 {
                    let count = Int(min(bodyRemaining, Int64(bytes.count - offset)))
                    if metadataKind != 0 {
                        for index in 0..<count { metadata[bufferedMetadataBytes + index] = bytes[offset + index] }
                        bufferedMetadataBytes += count
                    }
                    offset += count
                    position += Int64(count)
                    bodyRemaining -= Int64(count)
                    if bodyRemaining == 0, metadataKind != 0 { try validateMetadata() }
                    continue
                }
                if paddingRemaining > 0 {
                    let count = min(paddingRemaining, bytes.count - offset)
                    guard bytes[offset..<(offset + count)].allSatisfy({ $0 == 0 }) else {
                        throw SpeechModelArchiveError.integrity
                    }
                    offset += count
                    position += Int64(count)
                    paddingRemaining -= count
                    continue
                }
                let count = min(512 - headerBytes, bytes.count - offset)
                for index in 0..<count { header[headerBytes + index] = bytes[offset + index] }
                headerBytes += count
                offset += count
                position += Int64(count)
                if headerBytes == 512 {
                    headerBytes = 0
                    try validateHeader()
                }
            }
        }

        func finish() throws {
            guard headerBytes == 0, bodyRemaining == 0, paddingRemaining == 0,
                  !pendingExtension, trailingBytes >= 1024, trailingBytes % 512 == 0 else {
                throw SpeechModelArchiveError.integrity
            }
        }

        private mutating func validateHeader() throws {
            if header.allSatisfy({ $0 == 0 }) {
                guard !pendingExtension else { throw SpeechModelArchiveError.integrity }
                trailingBytes = 512
                return
            }
            physicalEntries += 1
            guard physicalEntries <= entryLimit else { throw SpeechModelArchiveError.entryLimit }
            let checksum = try number(148..<156)
            let actual = header.enumerated().reduce(Int64(0)) { total, item in
                total + Int64((148..<156).contains(item.offset) ? 32 : item.element)
            }
            guard checksum == actual else { throw SpeechModelArchiveError.integrity }
            let type = header[156]
            let extended = type == 120 || type == 103 || type == 76
            guard extended || type == 0 || type == 48 || type == 53 else {
                throw SpeechModelArchiveError.path
            }
            _ = try number(100..<108)
            _ = try number(108..<116)
            _ = try number(116..<124)
            _ = try number(136..<148)
            let size = try number(124..<136)
            guard size <= maximumEntry else { throw SpeechModelArchiveError.expansion }
            // 必须先检查声明，再接收任何 metadata；固定 64 KiB 缓冲不会随声明扩容。
            if extended, size < 1 || size > metadataLimit { throw SpeechModelArchiveError.metadata }
            guard type != 53 || size == 0 else { throw SpeechModelArchiveError.length }
            let padding = (512 - size % 512) % 512
            guard size <= maximum - position, padding <= maximum - position - size else {
                throw SpeechModelArchiveError.expansion
            }
            _ = try stringField(header, 0..<100)
            guard try stringField(header, 157..<257) == 0 else { throw SpeechModelArchiveError.path }
            let signature = Array(header[257..<265])
            if signature == Array("ustar\u{0}00".utf8) {
                _ = try stringField(header, 345..<500)
                guard header[500..<512].allSatisfy({ $0 == 0 }) else { throw SpeechModelArchiveError.format }
            } else if signature == Array("ustar  \0".utf8) {
                _ = try number(345..<357, allowBlank: true)
                _ = try number(357..<369, allowBlank: true)
                guard try number(369..<381, allowBlank: true) == 0,
                      header[381..<483].allSatisfy({ $0 == 0 }),
                      try number(483..<495, allowBlank: true) == 0,
                      header[495..<512].allSatisfy({ $0 == 0 }) else {
                    throw SpeechModelArchiveError.path
                }
            } else {
                guard header[257..<512].allSatisfy({ $0 == 0 }) else { throw SpeechModelArchiveError.format }
            }
            if !header[257..<265].allSatisfy({ $0 == 0 }) {
                _ = try stringField(header, 265..<297)
                _ = try stringField(header, 297..<329)
                guard try number(329..<337, allowBlank: true) == 0,
                      try number(337..<345, allowBlank: true) == 0 else {
                    throw SpeechModelArchiveError.path
                }
            }
            bodyRemaining = size
            paddingRemaining = Int(padding)
            metadataKind = extended ? type : 0
            metadataSize = extended ? Int(size) : 0
            bufferedMetadataBytes = 0
            pendingExtension = extended
        }

        private func number(_ range: Range<Int>, allowBlank: Bool = false) throws -> Int64 {
            var result: Int64 = 0
            if header[range.lowerBound] & 0x80 != 0 {
                // 接受 GNU 正 base-256，拒绝负值、非规范高字节和 Int64 溢出。
                guard header[range.lowerBound] == 0x80 else { throw SpeechModelArchiveError.integrity }
                for index in (range.lowerBound + 1)..<range.upperBound {
                    guard result <= (Int64.max - Int64(header[index])) / 256 else {
                        throw SpeechModelArchiveError.integrity
                    }
                    result = result * 256 + Int64(header[index])
                }
                return result
            }
            var digits = 0
            var terminated = false
            for index in range {
                let byte = header[index]
                if byte == 0 || byte == 32 {
                    if byte == 0 || digits > 0 { terminated = true }
                } else {
                    guard !terminated, byte >= 48, byte <= 55,
                          result <= (Int64.max - Int64(byte - 48)) / 8 else {
                        throw SpeechModelArchiveError.integrity
                    }
                    digits += 1
                    result = result * 8 + Int64(byte - 48)
                }
            }
            guard digits > 0 || allowBlank else { throw SpeechModelArchiveError.integrity }
            return result
        }

        private func stringField(_ buffer: [UInt8], _ range: Range<Int>) throws -> Int {
            let end = range.first(where: { buffer[$0] == 0 }) ?? range.upperBound
            guard buffer[end..<range.upperBound].allSatisfy({ $0 == 0 }),
                  String(bytes: buffer[range.lowerBound..<end], encoding: .utf8) != nil else {
                throw SpeechModelArchiveError.path
            }
            return end - range.lowerBound
        }

        private func validateMetadata() throws {
            if metadataKind == 76 {
                let length = try stringField(metadata, 0..<metadataSize)
                guard length > 0, length < metadataSize else { throw SpeechModelArchiveError.metadata }
                return
            }
            var offset = 0
            while offset < metadataSize {
                let start = offset
                guard metadata[offset] >= 49, metadata[offset] <= 57 else {
                    throw SpeechModelArchiveError.metadata
                }
                var length = 0
                while offset < metadataSize, metadata[offset] >= 48, metadata[offset] <= 57 {
                    guard offset - start < 8 else { throw SpeechModelArchiveError.metadata }
                    length = length * 10 + Int(metadata[offset] - 48)
                    offset += 1
                }
                guard offset < metadataSize, metadata[offset] == 32, length <= metadataSize - start else {
                    throw SpeechModelArchiveError.metadata
                }
                let end = start + length
                let keyStart = offset + 1
                guard end > keyStart + 2, metadata[end - 1] == 10,
                      let equals = (keyStart..<(end - 1)).first(where: { metadata[$0] == 61 }), equals > keyStart,
                      !metadata[keyStart..<(end - 1)].contains(0),
                      metadata[keyStart..<equals].allSatisfy({ $0 >= 33 && $0 <= 126 }),
                      let key = String(bytes: metadata[keyStart..<equals], encoding: .utf8),
                      String(bytes: metadata[(equals + 1)..<(end - 1)], encoding: .utf8) != nil else {
                    throw SpeechModelArchiveError.metadata
                }
                guard key != "size", key != "linkpath", !(metadataKind == 103 && key == "path"),
                      !key.hasPrefix("GNU.sparse"), key != "SCHILY.realsize", key != "SCHILY.filetype",
                      key != "SUN.holesdata", key != "SCHILY.holesdata", key != "GNU.dumpdir" else {
                    throw SpeechModelArchiveError.path
                }
                offset = end
            }
        }
    }

    private static func manifest(_ files: [SpeechDownloadFile]) throws -> ([String: SpeechDownloadFile], Int64) {
        guard !files.isEmpty, files.count <= 1024 else { throw SpeechModelArchiveError.catalog }
        var expected = [String: SpeechDownloadFile]()
        var keys = Set<String>()
        var total = extraBudget
        for file in files {
            _ = try components(file.path)
            let key = collisionKey(file.path)
            guard key != ".installation.json", !key.hasPrefix(".installation.json/"), file.bytes >= 0,
                  file.sha256.utf8.count == 64,
                  file.sha256.utf8.allSatisfy({ (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) }),
                  !keys.contains(where: { $0 == key || $0.hasPrefix(key + "/") || key.hasPrefix($0 + "/") }) else {
                throw SpeechModelArchiveError.catalog
            }
            let sum = total.addingReportingOverflow(file.bytes)
            guard !sum.overflow else { throw SpeechModelArchiveError.catalog }
            total = sum.partialValue
            keys.insert(key)
            expected[file.path] = file
        }
        return (expected, total)
    }

    private static func collisionKey(_ path: String) -> String {
        path.precomposedStringWithCanonicalMapping.lowercased()
    }

    private static func components(_ path: String) throws -> [String] {
        let parts = path.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        guard !path.isEmpty, path.utf8.count <= 1024, parts.count <= 32,
              !path.contains("\\"), !path.contains(":"),
              !path.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) || $0.properties.generalCategory == .format }),
              parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && $0.utf8.count <= 255 }) else {
            throw SpeechModelArchiveError.path
        }
        return parts
    }

    private static func entryName(_ value: UnsafePointer<CChar>?) throws -> String {
        guard let value else { throw SpeechModelArchiveError.path }
        let count = strnlen(value, Int(metadataLimit) + 1)
        guard count > 0, count <= Int(metadataLimit) else { throw SpeechModelArchiveError.metadata }
        let bytes = UnsafeRawBufferPointer(start: value, count: count)
        guard let name = String(bytes: bytes, encoding: .utf8) else { throw SpeechModelArchiveError.path }
        return name
    }

    private static func absoluteComponents(_ url: URL) throws -> [String] {
        guard url.isFileURL, url.host == nil || url.host == "" || url.host == "localhost",
              url.path.hasPrefix("/") else { throw SpeechModelArchiveError.path }
        var path = url.path
        if path.hasSuffix("/") { path.removeLast() }
        return try components(String(path.dropFirst()))
    }

    private static func archiveOK(_ status: Int32) throws {
        guard status == ARCHIVE_OK else { throw SpeechModelArchiveError.integrity }
    }

    private static func checkExpansion(_ reader: OpaquePointer, _ budget: Int64) throws {
        let consumed = archive_filter_bytes(reader, 0)
        guard consumed >= 0, consumed <= budget else { throw SpeechModelArchiveError.expansion }
    }

    private final class Descriptor {
        private(set) var raw: Int32
        init(_ raw: Int32) throws {
            guard raw >= 0 else { throw SpeechModelArchiveError.storage }
            self.raw = raw
        }
        deinit { if raw >= 0 { _ = Darwin.close(raw) } }
        func closeChecked() throws {
            let descriptor = raw
            raw = -1
            guard Darwin.close(descriptor) == 0 else { throw SpeechModelArchiveError.storage }
        }
    }

    private static func directoryStat(_ fd: Int32) throws -> stat {
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR) else {
            throw SpeechModelArchiveError.path
        }
        // 目录的正常 st_nlink 可大于 1；普通文件则必须只有一个硬链接。
        return info
    }

    private static func sameDirectory(_ parent: Int32, _ name: String, _ fd: Int32) -> Bool {
        var named = stat()
        var opened = stat()
        return fstatat(parent, name, &named, AT_SYMLINK_NOFOLLOW) == 0 && fstat(fd, &opened) == 0 &&
            named.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR) && named.st_dev == opened.st_dev &&
            named.st_ino == opened.st_ino && named.st_uid == geteuid() && named.st_mode & 0o077 == 0
    }

    private static func regularStat(_ fd: Int32) throws -> stat {
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG), info.st_nlink == 1 else {
            throw SpeechModelArchiveError.path
        }
        return info
    }

    private static func openChildDirectory(_ parent: Int32, _ name: String) throws -> Descriptor {
        let child = try Descriptor(openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC))
        _ = try directoryStat(child.raw)
        return child
    }

    private static func openDirectory(_ parts: [String]) throws -> Descriptor {
        // 与 Store 共用只搜索祖先、只读最终目录及逐级拒绝链接的合同，不解析不可信子路径。
        let url = URL(fileURLWithPath: "/" + parts.joined(separator: "/"), isDirectory: true)
        let descriptor: Int32
        do { descriptor = try SpeechFiles.directory(url) }
        catch { throw SpeechModelArchiveError.storage }
        let directory = try Descriptor(descriptor)
        _ = try directoryStat(directory.raw)
        return directory
    }

    private static func openInput(_ url: URL) throws -> Descriptor {
        let parts = try absoluteComponents(url)
        guard let leaf = parts.last else { throw SpeechModelArchiveError.path }
        let parent = try openDirectory(Array(parts.dropLast()))
        let input = try Descriptor(openat(parent.raw, leaf, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC))
        _ = try regularStat(input.raw)
        return input
    }

    private static func createOutput(_ root: Int32, _ path: String, _ created: inout [String], _ createdFiles: inout [String]) throws -> Descriptor {
        let parts = try components(path)
        var directory = try Descriptor(dup(root))
        var relative = [String]()
        for part in parts.dropLast() {
            relative.append(part)
            if mkdirat(directory.raw, part, 0o700) == 0 {
                created.append(relative.joined(separator: "/"))
            } else if errno != EEXIST {
                throw SpeechModelArchiveError.storage
            }
            directory = try openChildDirectory(directory.raw, part)
            let info = try directoryStat(directory.raw)
            guard info.st_uid == geteuid(), info.st_mode & 0o077 == 0 else {
                throw SpeechModelArchiveError.path
            }
        }
        guard let leaf = parts.last else { throw SpeechModelArchiveError.path }
        let output = try Descriptor(openat(directory.raw, leaf, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600))
        createdFiles.append(path)
        _ = try regularStat(output.raw)
        return output
    }

    private static func writeAll(_ fd: Int32, _ buffer: UnsafeRawPointer, _ count: Int, _ check: () throws -> Void) throws {
        var offset = 0
        while offset < count {
            try check()
            let written = Darwin.write(fd, buffer.advanced(by: offset), count - offset)
            if written < 0, errno == EINTR { continue }
            guard written > 0 else { throw SpeechModelArchiveError.storage }
            offset += written
        }
    }

    private static func cleanup(_ root: Int32, files: [String], directories: [String]) {
        for (path, flags) in files.reversed().map({ ($0, Int32(0)) }) +
            directories.reversed().map({ ($0, AT_REMOVEDIR) }) {
            do {
                let parts = try components(path)
                var parent = try Descriptor(dup(root))
                for part in parts.dropLast() { parent = try openChildDirectory(parent.raw, part) }
                if let leaf = parts.last { _ = unlinkat(parent.raw, leaf, flags) }
            } catch {
                // 可疑祖先不跟随、不递归删除；Store 不得发布失败的 staging。
            }
        }
    }
}
