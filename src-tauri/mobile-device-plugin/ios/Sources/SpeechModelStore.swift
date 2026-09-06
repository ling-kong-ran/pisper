import Foundation
import CoreFoundation
import CryptoKit
import Darwin

public struct SpeechDownloadFile {
    public let path: String
    public let bytes: Int64
    public let sha256: String
    public let urls: [URL]

    public init(path: String, bytes: Int64, sha256: String, urls: [URL]) {
        self.path = path
        self.bytes = bytes
        self.sha256 = sha256
        self.urls = urls
    }
}

public struct SpeechArchive {
    public let file: SpeechDownloadFile
    public let stripPrefix: String
}

public struct SpeechModel {
    public let id: String
    public let revision: String
    public let kind: String
    public let engine: String
    public let config: [String: Any]
    public let publicFields: [String: Any]
    public let files: [SpeechDownloadFile]
    public let archive: SpeechArchive?
    public let fingerprint: String
    public var filesBytes: Int64 { files.reduce(0) { $0 + $1.bytes } }
    public var totalBytes: Int64 { archive?.file.bytes ?? filesBytes }
}

// 绑定必须用成熟流式库：展开总量 <= filesBytes + 32 MiB、物理条目 <= 4096、
// 扩展 metadata <= 64 KiB；拒绝链接、特殊/稀疏条目、重复/冲突路径及前缀逃逸。
// 未安装的条目也必须读完计入预算，校验 TAR/压缩 CRC，取消或失败后关闭全部句柄。
// destination 必须尚不存在，父目录由当前 UID 独占（0700）；归档 URL 本身不可 realpath。
public typealias SpeechArchiveExtractor = (
    _ archiveURL: URL,
    _ destinationURL: URL,
    _ stripPrefix: String,
    _ files: [SpeechDownloadFile],
    _ checkCancellation: () throws -> Void
) throws -> Void

public enum SpeechStorageError: String, Error, LocalizedError {
    case catalog = "speech_catalog_invalid"
    case unknown = "speech_model_unknown"
    case voice = "speech_voice_unknown"
    case path = "speech_invalid_path"
    case integrity = "speech_model_checksum_mismatch"
    case size = "speech_download_length_mismatch"
    case range = "speech_download_invalid_range"
    case http = "speech_download_http_failed"
    case redirect = "speech_untrusted_download_url"
    case cancelled = "speech_cancelled"
    case storage = "speech_storage_error"
    case recovery = "speech_storage_recovery_required"
    case missing = "speech_model_not_installed"
    case busy = "speech_download_busy"
    case space = "speech_insufficient_storage"
    public var errorDescription: String? { rawValue }
}

func speechRequire(_ condition: Bool, _ error: SpeechStorageError = .catalog) throws {
    if !condition { throw error }
}

func speechSafeError(_ error: Error) -> SpeechStorageError {
    error as? SpeechStorageError ?? .storage
}

// NSNumber 的布尔桥接和浮点截断不能变成合法文件大小或补丁位置。
func speechInteger(_ value: Any?, maximum: Int64 = 9_007_199_254_740_991) throws -> Int64 {
    guard let number = value as? NSNumber,
          CFGetTypeID(number) != CFBooleanGetTypeID() else { throw SpeechStorageError.catalog }
    let numberValue = number.doubleValue
    try speechRequire(numberValue.isFinite && numberValue >= 0 && numberValue <= Double(maximum)
        && numberValue.rounded(.towardZero) == numberValue)
    return number.int64Value
}

func speechVitsScale(_ value: Any?, defaultValue: Float) throws -> Float {
    guard let value else { return defaultValue }
    guard let number = value as? NSNumber,
          CFGetTypeID(number) != CFBooleanGetTypeID() else { throw SpeechStorageError.catalog }
    let scale = number.floatValue
    try speechRequire(number.doubleValue.isFinite && scale.isFinite && scale > 0)
    return scale
}

func speechHash(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

func speechMatches(_ value: String, _ pattern: String) -> Bool {
    value.range(of: pattern, options: .regularExpression) != nil
}

// 所有读写都逐级 openat(O_NOFOLLOW)，不把符号链接或硬链接当作自有模型文件。
enum SpeechFiles {
    static let block = 64 * 1024
    static let marker = ".installation.json"

    static func relative(_ value: String) throws -> String {
        try speechRequire(!value.isEmpty && value.utf16.count <= 512, .path)
        let parts = value.split(separator: "/", omittingEmptySubsequences: false)
        try speechRequire(parts.count <= 16, .path)
        for raw in parts {
            let part = String(raw)
            try speechRequire(!part.isEmpty && part.utf16.count <= 255 && part != "." && part != ".."
                && !part.hasSuffix(".") && !part.hasSuffix(" ")
                && !speechMatches(part, "[\\\\<>:\"|?*\\p{Cc}\\p{Cf}]")
                && !speechMatches(part, "(?i)^(con|prn|aux|nul|conin\\$|conout\\$|clock\\$|com[0-9\u{00b9}\u{00b2}\u{00b3}]|lpt[0-9\u{00b9}\u{00b2}\u{00b3}])(?:\\.|$)"), .path)
        }
        return value
    }

    static func root(_ url: URL) throws -> URL {
        try speechRequire(url.isFileURL && url.host.map { $0.isEmpty || $0 == "localhost" } != false
            && url.query == nil && url.fragment == nil && url.path.hasPrefix("/")
            && !url.path.hasPrefix("//"), .path)
        for part in url.path.split(separator: "/") { _ = try relative(String(part)) }
        return url.standardizedFileURL
    }

    static func child(_ root: URL, _ path: String) throws -> URL {
        root.appendingPathComponent(try relative(path))
    }

    static func directory(_ url: URL, create: Bool = false) throws -> Int32 {
        let root = try self.root(url)
        var fd = Darwin.open("/", O_RDONLY | O_DIRECTORY | O_CLOEXEC)
        try speechRequire(fd >= 0, .storage)
        do {
            for part in root.path.split(separator: "/") {
                let name = String(part)
                if create && mkdirat(fd, name, 0o700) != 0 && errno != EEXIST {
                    throw SpeechStorageError.storage
                }
                let next = openat(fd, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                try speechRequire(next >= 0, .path)
                Darwin.close(fd)
                fd = next
            }
            return fd
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    static func attributes(_ fd: Int32) throws -> stat {
        var value = stat()
        try speechRequire(fstat(fd, &value) == 0, .storage)
        return value
    }

    static func identity(_ value: stat) -> String {
        let base = "\(value.st_dev):\(value.st_ino):\(value.st_mode):\(value.st_uid):\(value.st_gid):\(value.st_nlink)"
        return base + ":\(value.st_size):\(value.st_mtimespec.tv_sec):\(value.st_mtimespec.tv_nsec):\(value.st_ctimespec.tv_sec):\(value.st_ctimespec.tv_nsec)"
    }

    static func statAt(_ parent: Int32, _ name: String) throws -> stat? {
        var value = stat()
        if fstatat(parent, name, &value, AT_SYMLINK_NOFOLLOW) == 0 { return value }
        if errno == ENOENT { return nil }
        throw SpeechStorageError.path
    }

    static func exists(_ url: URL) throws -> Bool {
        let parent = try directory(url.deletingLastPathComponent())
        defer { Darwin.close(parent) }
        return try statAt(parent, url.lastPathComponent) != nil
    }

    static func open(_ url: URL, writable: Bool = false, exclusive: Bool = false) throws -> FileHandle {
        let parent = try directory(url.deletingLastPathComponent())
        defer { Darwin.close(parent) }
        let before = try statAt(parent, url.lastPathComponent)
        if let before = before {
            try speechRequire((before.st_mode & S_IFMT) == S_IFREG && before.st_nlink == 1, .path)
        }
        var flags = writable ? O_RDWR | O_CREAT : O_RDONLY
        if exclusive { flags |= O_EXCL }
        let fd = openat(parent, url.lastPathComponent, flags | O_NOFOLLOW | O_CLOEXEC, 0o600)
        try speechRequire(fd >= 0, .path)
        do {
            let actual = try attributes(fd)
            try speechRequire((actual.st_mode & S_IFMT) == S_IFREG && actual.st_nlink == 1, .path)
            if let before = before { try speechRequire(identity(before) == identity(actual), .path) }
            return FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    static func readSmall(_ url: URL, limit: Int) throws -> Data {
        let handle = try open(url)
        defer { try? handle.close() }
        let attrs = try attributes(handle.fileDescriptor)
        try speechRequire(attrs.st_size <= limit, .size)
        let data = try handle.read(upToCount: limit + 1) ?? Data()
        try speechRequire(data.count <= limit && data.count == attrs.st_size, .size)
        return data
    }

    static func writeSmall(_ url: URL, data: Data, exclusive: Bool = true) throws {
        let handle = try open(url, writable: true, exclusive: exclusive)
        defer { try? handle.close() }
        try handle.truncate(atOffset: 0)
        try handle.write(contentsOf: data)
        try handle.synchronize()
    }

    @discardableResult
    static func scan(_ url: URL, spec: SpeechDownloadFile, check: () throws -> Void = {},
                     consume: (Data, Int64) throws -> Void = { _, _ in }) throws -> String {
        let handle = try open(url)
        defer { try? handle.close() }
        let before = try attributes(handle.fileDescriptor)
        try speechRequire(before.st_size == spec.bytes, .size)
        var hash = SHA256()
        var position: Int64 = 0
        while position < spec.bytes {
            try check()
            let data = try handle.read(upToCount: Int(min(Int64(block), spec.bytes - position))) ?? Data()
            try speechRequire(!data.isEmpty, .size)
            hash.update(data: data)
            try consume(data, position)
            position += Int64(data.count)
        }
        try check()
        let digest = hash.finalize().map { String(format: "%02x", $0) }.joined()
        try speechRequire(digest == spec.sha256, .integrity)
        let current = try open(url)
        defer { try? current.close() }
        try speechRequire(identity(before) == identity(try attributes(handle.fileDescriptor))
            && identity(before) == identity(try attributes(current.fileDescriptor)), .path)
        return digest
    }

    static func tree(_ url: URL, files: [SpeechDownloadFile], complete: Bool,
                     marker: Bool = true) throws -> String {
        try speechRequire(Set(files.map { $0.path }).count == files.count, .catalog)
        for file in files { _ = try relative(file.path) }
        let allowed = Dictionary(uniqueKeysWithValues: files.map { ($0.path, $0.bytes) })
        var found = Set<String>()
        var proof: [String] = []
        func walk(_ directoryURL: URL, _ prefix: String) throws {
            let fd = try directory(directoryURL)
            defer { Darwin.close(fd) }
            // 只记录模型子树属性，外层 storage/容器目录的 mtime 不属于缓存证明。
            let before = identity(try attributes(fd))
            proof.append(prefix + ":" + before)
            let duplicate = dup(fd)
            try speechRequire(duplicate >= 0, .storage)
            guard let stream = fdopendir(duplicate) else { Darwin.close(duplicate); throw SpeechStorageError.storage }
            defer { closedir(stream) }
            var names: [String] = []
            errno = 0
            while let item = readdir(stream) {
                let capacity = Int(item.pointee.d_namlen) + 1
                let name = withUnsafePointer(to: &item.pointee.d_name) {
                    $0.withMemoryRebound(to: CChar.self, capacity: capacity) {
                        String(validatingUTF8: $0)
                    }
                }
                guard let name = name else { throw SpeechStorageError.path }
                if name != "." && name != ".." { names.append(name) }
                try speechRequire(names.count <= files.count * 17 + 1, .path)
                errno = 0
            }
            try speechRequire(errno == 0, .storage)
            for name in names.sorted() {
                _ = try relative(name)
                let path = prefix.isEmpty ? name : prefix + "/" + name
                guard let attrs = try statAt(fd, name) else { throw SpeechStorageError.path }
                let child = directoryURL.appendingPathComponent(name)
                if (attrs.st_mode & S_IFMT) == S_IFDIR {
                    try speechRequire(allowed.keys.contains { $0.hasPrefix(path + "/") }, .path)
                    try walk(child, path)
                } else {
                    try speechRequire((attrs.st_mode & S_IFMT) == S_IFREG && attrs.st_nlink == 1
                        && (allowed[path] != nil || (marker && path == self.marker)), .path)
                    let handle = try open(child)
                    defer { try? handle.close() }
                    try speechRequire(identity(attrs) == identity(try attributes(handle.fileDescriptor)), .path)
                    if let size = allowed[path] {
                        if complete { try speechRequire(attrs.st_size == size, .size) }
                        found.insert(path)
                    } else { try speechRequire(attrs.st_size <= 1024 * 1024, .size) }
                    proof.append(path + ":" + identity(attrs))
                }
            }
            let current = try directory(directoryURL)
            defer { Darwin.close(current) }
            try speechRequire(before == identity(try attributes(fd))
                && before == identity(try attributes(current)), .path)
        }
        try walk(url, "")
        if complete { try speechRequire(found == Set(allowed.keys), .integrity) }
        return proof.joined(separator: "\n")
    }

    static func rename(_ source: URL, _ destination: URL) throws {
        let from = try directory(source.deletingLastPathComponent())
        defer { Darwin.close(from) }
        let to = try directory(destination.deletingLastPathComponent())
        defer { Darwin.close(to) }
        try speechRequire(try statAt(to, destination.lastPathComponent) == nil, .path)
        try speechRequire(renameatx_np(from, source.lastPathComponent, to, destination.lastPathComponent,
                                      UInt32(RENAME_EXCL)) == 0, .storage)
        // rename 已提交后不能再抛同步错误，否则调用者会错误地回滚一个成功的发布。
        _ = fsync(from)
        _ = fsync(to)
    }

    static func installationFiles(_ directory: URL, modelId: String) throws -> [SpeechDownloadFile] {
        let data = try readSmall(directory.appendingPathComponent(marker), limit: 1024 * 1024)
        guard let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              value["id"] as? String == modelId,
              let fingerprint = value["fingerprint"] as? String, fingerprint.utf8.count == 64,
              speechMatches(fingerprint, "^[a-f0-9]{64}$"),
              let entries = value["files"] as? [[String: Any]], (1...1024).contains(entries.count),
              try speechInteger(value["version"]) == 1 else { throw SpeechStorageError.path }
        var paths = Set<String>()
        let files = try entries.map { entry -> SpeechDownloadFile in
            guard let path = entry["path"] as? String, let sha = entry["sha256"] as? String,
                  sha.utf8.count == 64, speechMatches(sha, "^[a-f0-9]{64}$") else { throw SpeechStorageError.path }
            _ = try relative(path)
            let key = path.lowercased()
            try speechRequire(key != marker && !key.hasPrefix(marker + "/")
                && !paths.contains { $0 == key || $0.hasPrefix(key + "/") || key.hasPrefix($0 + "/") }, .path)
            paths.insert(key)
            let bytes = try speechInteger(entry["bytes"], maximum: 2 * 1024 * 1024 * 1024)
            return SpeechDownloadFile(path: path, bytes: bytes, sha256: sha, urls: [])
        }
        try speechRequire(files.reduce(Int64(0)) { $0 + $1.bytes } <= 4 * 1024 * 1024 * 1024, .path)
        return files
    }

    static func publish(_ staging: URL, target: URL, backup: URL, files: [SpeechDownloadFile],
                        move: (URL, URL) throws -> Void = SpeechFiles.rename) throws {
        try speechRequire(try !exists(backup), .recovery)
        let hadOld = try exists(target)
        if hadOld {
            _ = try tree(target, files: files, complete: false)
            try move(target, backup)
        }
        do { try move(staging, target) }
        catch {
            if hadOld {
                do { try move(backup, target) }
                catch { throw SpeechStorageError.recovery }
            }
            throw SpeechStorageError.storage
        }
        if hadOld { try? removeOwnedTree(backup, files: files) }
    }

    static func unlink(_ url: URL, isDirectory: Bool = false) throws {
        let parent = try directory(url.deletingLastPathComponent())
        defer { Darwin.close(parent) }
        guard let attrs = try statAt(parent, url.lastPathComponent) else { return }
        try speechRequire(isDirectory ? (attrs.st_mode & S_IFMT) == S_IFDIR
            : (attrs.st_mode & S_IFMT) == S_IFREG && attrs.st_nlink == 1, .path)
        try speechRequire(unlinkat(parent, url.lastPathComponent, isDirectory ? AT_REMOVEDIR : 0) == 0, .storage)
    }

    // 只删除已通过完整白名单树检查的文件，额外文件或链接会阻止清理而不是被递归抹掉。
    static func removeOwnedTree(_ url: URL, files: [SpeechDownloadFile], marker: Bool = true) throws {
        _ = try tree(url, files: files, complete: false, marker: marker)
        var directories = Set<String>()
        for file in files {
            let parts = file.path.split(separator: "/")
            for count in 1..<parts.count { directories.insert(parts.prefix(count).joined(separator: "/")) }
            let target = url.appendingPathComponent(file.path)
            if let parent = try? directory(target.deletingLastPathComponent()) {
                Darwin.close(parent)
                try unlink(target)
            }
        }
        if marker { try unlink(url.appendingPathComponent(self.marker)) }
        for path in directories.sorted(by: { $0.count > $1.count }) {
            let target = url.appendingPathComponent(path)
            if let fd = try? directory(target) { Darwin.close(fd); try unlink(target, isDirectory: true) }
        }
        try unlink(url, isDirectory: true)
    }
}

final class SpeechCancellation {
    private let condition = NSCondition()
    private var cancelled = false
    private var started = false
    private var finished = false
    private var interrupt: (() -> Void)?

    func begin() -> Bool {
        condition.lock(); defer { condition.unlock() }
        if finished { return false }
        started = true
        return true
    }
    var isCancelled: Bool {
        condition.lock(); defer { condition.unlock() }
        return cancelled
    }
    func check() throws {
        if isCancelled { throw SpeechStorageError.cancelled }
    }
    func bind(_ action: (() -> Void)?) {
        condition.lock()
        interrupt = action
        let stop = cancelled ? action : nil
        condition.unlock()
        stop?()
    }
    func cancel() {
        condition.lock()
        cancelled = true
        let action = interrupt
        if !started { finished = true; condition.broadcast() }
        condition.unlock()
        action?()
    }
    func finish() {
        condition.lock()
        finished = true
        interrupt = nil
        condition.broadcast()
        condition.unlock()
    }
    func wait() {
        condition.lock(); defer { condition.unlock() }
        while !finished { condition.wait() }
    }
}

final class SpeechHTTPTransfer: NSObject, URLSessionDataDelegate {
    static let trustedHosts: Set<String> = [
        "hf-mirror.com", "huggingface.co", "cas-bridge.xethub.hf.co", "ghfast.top", "github.com",
        "release-assets.githubusercontent.com", "cdn-lfs.huggingface.co",
        "cdn-lfs-us-1.huggingface.co", "cdn-lfs-eu-1.huggingface.co",
    ]

    static func trustedURL(_ value: String) throws -> URL {
        guard let parts = URLComponents(string: value), let url = parts.url else { throw SpeechStorageError.redirect }
        try speechRequire(parts.scheme == "https" && parts.host.map { trustedHosts.contains($0) } == true
            && parts.user == nil && parts.password == nil && parts.fragment == nil
            && (parts.port == nil || parts.port == 443), .redirect)
        return url
    }

    static func checkAddress(_ url: URL) throws {
        guard let host = url.host else { throw SpeechStorageError.redirect }
        var hints = addrinfo()
        hints.ai_family = AF_UNSPEC
        hints.ai_socktype = SOCK_STREAM
        var result: UnsafeMutablePointer<addrinfo>?
        try speechRequire(getaddrinfo(host, "443", &hints, &result) == 0 && result != nil, .http)
        defer { if let result = result { freeaddrinfo(result) } }
        var cursor = result
        while let item = cursor {
            let info = item.pointee
            guard let address = info.ai_addr else { throw SpeechStorageError.redirect }
            var bytes: [UInt8]
            if info.ai_family == AF_INET {
                bytes = address.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
                    withUnsafeBytes(of: $0.pointee.sin_addr) { Array($0) }
                }
            } else if info.ai_family == AF_INET6 {
                bytes = address.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) {
                    withUnsafeBytes(of: $0.pointee.sin6_addr) { Array($0) }
                }
                if bytes.prefix(10).allSatisfy({ $0 == 0 }) && bytes[10] == 255 && bytes[11] == 255 {
                    bytes = Array(bytes.suffix(4))
                } else {
                    try speechRequire(bytes[0] & 0xe0 == 0x20, .redirect)
                    cursor = info.ai_next
                    continue
                }
            } else { throw SpeechStorageError.redirect }
            try speechRequire(bytes[0] != 0 && bytes[0] != 10 && bytes[0] != 127 && bytes[0] < 224
                && !(bytes[0] == 169 && bytes[1] == 254)
                && !(bytes[0] == 172 && (16...31).contains(bytes[1]))
                && !(bytes[0] == 192 && bytes[1] == 168)
                && !(bytes[0] == 100 && (64...127).contains(bytes[1])), .redirect)
            cursor = info.ai_next
        }
    }

    static func responseOffset(_ response: HTTPURLResponse, offset: Int64, bytes: Int64) throws -> Int64 {
        try speechRequire(response.statusCode == 200 || response.statusCode == 206, .http)
        let encoding = response.value(forHTTPHeaderField: "Content-Encoding")
        try speechRequire(encoding == nil || encoding == "identity", .size)
        let range = response.value(forHTTPHeaderField: "Content-Range")
        let start: Int64
        if response.statusCode == 206 {
            try speechRequire(offset > 0 && range == "bytes \(offset)-\(bytes - 1)/\(bytes)", .range)
            start = offset
        } else {
            try speechRequire(range == nil, .range)
            start = 0
        }
        if let length = response.value(forHTTPHeaderField: "Content-Length") {
            try speechRequire(speechMatches(length, "^(0|[1-9][0-9]*)$") && Int64(length) == bytes - start, .size)
        }
        return start
    }

    private let file: FileHandle
    private let spec: SpeechDownloadFile
    private let cancellation: SpeechCancellation
    private let progress: (Int64) -> Void
    private let completed = DispatchSemaphore(value: 0)
    private var received: Int64
    private var accepted = false
    private var redirects = 0
    private var failure: Error?

    init(file: FileHandle, spec: SpeechDownloadFile, offset: Int64,
         cancellation: SpeechCancellation, progress: @escaping (Int64) -> Void) {
        self.file = file
        self.spec = spec
        self.received = offset
        self.cancellation = cancellation
        self.progress = progress
    }

    func run(_ source: String) throws {
        let url = try Self.trustedURL(source)
        try cancellation.check()
        try Self.checkAddress(url)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 30 * 60
        configuration.httpShouldSetCookies = false
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        configuration.urlCache = nil
        configuration.connectionProxyDictionary = [:]
        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = 1
        let session = URLSession(configuration: configuration, delegate: self, delegateQueue: queue)
        defer { session.invalidateAndCancel(); cancellation.bind(nil) }
        var request = URLRequest(url: url)
        request.setValue("identity", forHTTPHeaderField: "Accept-Encoding")
        request.setValue("Pisper-iOS-Speech", forHTTPHeaderField: "User-Agent")
        if received > 0 { request.setValue("bytes=\(received)-", forHTTPHeaderField: "Range") }
        let task = session.dataTask(with: request)
        cancellation.bind { task.cancel() }
        task.resume()
        completed.wait()
        try cancellation.check()
        if let failure = failure { throw speechSafeError(failure) }
        try speechRequire(accepted && received == spec.bytes, .size)
        try file.synchronize()
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        do {
            try cancellation.check()
            try speechRequire([301, 302, 303, 307, 308].contains(response.statusCode) && redirects < 2, .redirect)
            guard let absolute = request.url?.absoluteString else { throw SpeechStorageError.redirect }
            let url = try Self.trustedURL(absolute)
            try Self.checkAddress(url)
            redirects += 1
            var next = URLRequest(url: url)
            next.setValue("identity", forHTTPHeaderField: "Accept-Encoding")
            if received > 0 { next.setValue("bytes=\(received)-", forHTTPHeaderField: "Range") }
            completionHandler(next)
        } catch {
            failure = speechSafeError(error)
            completionHandler(nil)
            task.cancel()
        }
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        do {
            try cancellation.check()
            guard let response = response as? HTTPURLResponse else { throw SpeechStorageError.http }
            received = try Self.responseOffset(response, offset: received, bytes: spec.bytes)
            if received == 0 { try file.truncate(atOffset: 0) }
            try file.seek(toOffset: UInt64(received))
            accepted = true
            progress(received)
            completionHandler(.allow)
        } catch {
            failure = speechSafeError(error)
            completionHandler(.cancel)
        }
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard failure == nil else { return }
        do {
            try cancellation.check()
            try speechRequire(accepted && Int64(data.count) <= spec.bytes - received, .size)
            var offset = 0
            while offset < data.count {
                try cancellation.check()
                let end = min(offset + SpeechFiles.block, data.count)
                try file.write(contentsOf: data.subdata(in: offset..<end))
                received += Int64(end - offset)
                offset = end
                progress(received)
            }
        } catch {
            failure = speechSafeError(error)
            dataTask.cancel()
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if failure == nil && error != nil { failure = SpeechStorageError.http }
        completed.signal()
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        completionHandler(challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust
            ? .performDefaultHandling : .cancelAuthenticationChallenge, nil)
    }
}

public final class SpeechModelStore {
    private final class State {
        var status = "not-installed"
        var downloadedBytes: Int64 = 0
        var error: String?
        var proof: String?
        var job: SpeechCancellation?
    }

    // 跨实例统一串行队列和任务表，避免多个插件入口重复写同一模型。
    private static let lock = NSRecursiveLock()
    private static let filesystem = NSRecursiveLock()
    private static let worker = DispatchQueue(label: "app.pisper.speech-model-download", qos: .utility)
    private static var states: [String: State] = [:]
    private let root: URL
    private let models: [SpeechModel]
    public let defaults: [String: String]
    public var defaultASRModelId: String { defaults["asr"]! }
    public var defaultTTSModelId: String { defaults["tts"]! }
    public var defaultVoiceId: String { defaults["voice"]! }
    private let extractor: SpeechArchiveExtractor
    typealias Transfer = (URL, SpeechDownloadFile, SpeechCancellation, @escaping (Int64) -> Void) throws -> Void
    private let transfer: Transfer

    public convenience init(catalogURL: URL, storageDirectory: URL,
                            archiveExtractor: @escaping SpeechArchiveExtractor) throws {
        do {
            let data = try SpeechFiles.readSmall(catalogURL, limit: 4 * 1024 * 1024)
            try self.init(catalogData: data, storageDirectory: storageDirectory,
                          archiveExtractor: archiveExtractor, transfer: Self.download)
        } catch { throw speechSafeError(error) }
    }

    init(catalogData: Data, storageDirectory: URL, archiveExtractor: @escaping SpeechArchiveExtractor,
         transfer: @escaping Transfer) throws {
        do {
            try speechRequire(catalogData.count <= 4 * 1024 * 1024)
            guard let catalog = try JSONSerialization.jsonObject(with: catalogData) as? [String: Any],
                  let entries = catalog["models"] as? [[String: Any]],
                  let defaults = catalog["defaults"] as? [String: String] else { throw SpeechStorageError.catalog }
            try speechRequire(try speechInteger(catalog["version"]) == 1 && (1...32).contains(entries.count))
            let parsed = try entries.map(Self.parse)
            try speechRequire(Set(parsed.map { $0.id.lowercased() }).count == parsed.count)
            var voices = Set<String>()
            for model in parsed where model.kind == "tts" {
                for voice in model.publicFields["voices"] as? [[String: Any]] ?? [] {
                    guard let id = voice["id"] as? String else { throw SpeechStorageError.catalog }
                    try speechRequire(voices.insert(id).inserted)
                }
            }
            try speechRequire(parsed.contains { $0.id == defaults["asr"] && $0.kind == "asr" }
                && parsed.contains { $0.id == defaults["tts"] && $0.kind == "tts" }
                && defaults["voice"].map { voices.contains($0) } == true)
            self.models = parsed
            self.defaults = defaults
            self.root = try SpeechFiles.root(storageDirectory)
            self.extractor = archiveExtractor
            self.transfer = transfer
            Self.filesystem.lock(); defer { Self.filesystem.unlock() }
            try ensureOwnership()
        } catch { throw speechSafeError(error) }
    }

    private static func identifier(_ value: Any?) throws -> String {
        guard let text = value as? String else { throw SpeechStorageError.catalog }
        try speechRequire(speechMatches(text, "^[A-Za-z0-9][A-Za-z0-9._-]{0,119}$"))
        _ = try SpeechFiles.relative(text)
        return text
    }

    private static func file(_ value: [String: Any], archive: Bool) throws -> SpeechDownloadFile {
        guard let path = value["path"] as? String, let sha = value["sha256"] as? String else { throw SpeechStorageError.catalog }
        _ = try SpeechFiles.relative(path)
        try speechRequire(path.lowercased().split(separator: "/").first.map(String.init) != SpeechFiles.marker
            && sha.utf8.count == 64 && speechMatches(sha, "^[a-f0-9]{64}$"))
        let bytes = try speechInteger(value["bytes"], maximum: 2 * 1024 * 1024 * 1024)
        let urls: [String]
        if let raw = value["urls"] { guard let list = raw as? [String] else { throw SpeechStorageError.catalog }; urls = list }
        else { urls = [] }
        try speechRequire(urls.count <= 2 && (archive || !urls.isEmpty))
        let sources = try urls.map(SpeechHTTPTransfer.trustedURL)
        return SpeechDownloadFile(path: path, bytes: bytes, sha256: sha, urls: sources)
    }

    private static func parse(_ value: [String: Any]) throws -> SpeechModel {
        let id = try identifier(value["id"])
        let revision = try identifier(value["revision"])
        guard let kind = value["kind"] as? String, let engine = value["engine"] as? String,
              let config = value["config"] as? [String: Any], let entries = value["files"] as? [[String: Any]],
              let name = value["name"] as? String, !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw SpeechStorageError.catalog
        }
        try speechRequire((kind == "asr" && ["online-transducer", "sense-voice"].contains(engine))
            || (kind == "tts" && engine == "vits"))
        var archive: SpeechArchive?
        if let raw = value["archive"] {
            guard var entry = raw as? [String: Any], entry["format"] as? String == "tar.bz2",
                  let prefix = entry["stripPrefix"] as? String, prefix.hasSuffix("/") else { throw SpeechStorageError.catalog }
            _ = try SpeechFiles.relative(String(prefix.dropLast()))
            entry["path"] = "archive.tar.bz2"
            let spec = try file(entry, archive: false)
            try speechRequire(spec.bytes > 0)
            archive = SpeechArchive(file: spec, stripPrefix: prefix)
        }
        try speechRequire((1...1024).contains(entries.count))
        let files = try entries.map { try file($0, archive: archive != nil) }
        var paths = Set<String>()
        for item in files {
            let key = item.path.lowercased()
            try speechRequire(!paths.contains { $0 == key || $0.hasPrefix(key + "/") || key.hasPrefix($0 + "/") })
            paths.insert(key)
        }
        try speechRequire(files.reduce(Int64(0)) { $0 + $1.bytes } <= 4 * 1024 * 1024 * 1024)
        let exactPaths = Set(files.map { $0.path })
        let keys = engine == "online-transducer" ? ["encoder", "decoder", "joiner", "tokens"]
            : engine == "vits" ? ["model", "tokens", "lexicon"] : ["model", "tokens"]
        for key in keys { try speechRequire((config[key] as? String).map { exactPaths.contains($0) } == true) }
        if engine == "vits" {
            let allowed: Set<String> = ["model", "tokens", "lexicon", "dictDir", "ruleFsts", "numThreads",
                "maxTextCodePoints", "noiseScale", "noiseScaleW", "lengthScale"]
            try speechRequire(config.keys.allSatisfy { allowed.contains($0) })
            guard let dictDir = config["dictDir"] as? String,
                  let lexicon = config["lexicon"] as? String else { throw SpeechStorageError.catalog }
            _ = try SpeechFiles.relative(dictDir)
            try speechRequire(exactPaths.contains { $0.hasPrefix(dictDir + "/") } && !lexicon.contains(","))
            if let raw = config["ruleFsts"] {
                guard let list = raw as? [String] else { throw SpeechStorageError.catalog }
                try speechRequire(list.allSatisfy { exactPaths.contains($0) && !$0.contains(",") })
            }
            try speechRequire(try speechInteger(config["numThreads"], maximum: 16) > 0)
            try speechRequire(try speechInteger(config["maxTextCodePoints"], maximum: 400) > 0)
            _ = try speechVitsScale(config["noiseScale"], defaultValue: 0.667)
            _ = try speechVitsScale(config["noiseScaleW"], defaultValue: 0.8)
            _ = try speechVitsScale(config["lengthScale"], defaultValue: 1.0)
        }
        if let raw = config["bpeVocabResource"] {
            guard let path = raw as? String else { throw SpeechStorageError.catalog }
            _ = try SpeechFiles.relative(path)
            try speechRequire(path.hasPrefix("speech-resources/"))
        }
        var fields: [String: Any] = [:]
        for key in ["id", "kind", "engine", "name", "languages", "license", "voices"] { fields[key] = value[key] }
        if kind == "tts" {
            guard let voices = fields["voices"] as? [[String: Any]], voices.count == 1 else { throw SpeechStorageError.catalog }
            for voice in voices {
                _ = try identifier(voice["id"])
                _ = try speechInteger(voice["sid"], maximum: 0)
            }
        }
        let fingerprint = speechHash(try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]))
        let model = SpeechModel(id: id, revision: revision, kind: kind, engine: engine, config: config,
                                publicFields: fields, files: files, archive: archive, fingerprint: fingerprint)
        return model
    }

    public func model(id: String? = nil, kind: String) throws -> SpeechModel {
        guard let value = models.first(where: { $0.id == (id ?? defaults[kind]) && $0.kind == kind }) else { throw SpeechStorageError.unknown }
        return value
    }

    public func model(id: String) throws -> SpeechModel {
        guard let value = models.first(where: { $0.id == id }) else { throw SpeechStorageError.unknown }
        return value
    }

    public func voice(id: String) throws -> (model: SpeechModel, sourceSpeaker: Int) {
        for model in models where model.kind == "tts" {
            for voice in model.publicFields["voices"] as? [[String: Any]] ?? [] where voice["id"] as? String == id {
                return (model, Int(try speechInteger(voice["sid"])))
            }
        }
        throw SpeechStorageError.voice
    }

    private func key(_ model: SpeechModel) -> String { root.path + ":" + model.id + ":" + model.fingerprint }
    private func state(_ model: SpeechModel) -> State {
        if let state = Self.states[key(model)] { return state }
        let state = State()
        Self.states[key(model)] = state
        return state
    }
    private func destination(_ model: SpeechModel) -> URL { root.appendingPathComponent(model.id) }
    private func staging(_ model: SpeechModel) -> URL { root.appendingPathComponent(".\(model.id).\(model.fingerprint).partial") }
    private func retired(_ model: SpeechModel) -> URL { root.appendingPathComponent(".\(model.id).\(model.fingerprint).previous") }

    private func ensureOwnership() throws {
        let fd = try SpeechFiles.directory(root, create: true)
        defer { Darwin.close(fd) }
        let attributes = try SpeechFiles.attributes(fd)
        try speechRequire(attributes.st_uid == getuid() && attributes.st_mode & 0o777 == 0o700, .path)
        let owner = root.appendingPathComponent(".pisper-speech-owner")
        let signature = Data("pisper-speech-models-v1\n".utf8)
        if try SpeechFiles.exists(owner) {
            try speechRequire(try SpeechFiles.readSmall(owner, limit: 128) == signature, .path)
        } else {
            let names = try FileManager.default.contentsOfDirectory(atPath: root.path)
            try speechRequire(names.isEmpty, .path)
            try SpeechFiles.writeSmall(owner, data: signature)
        }
    }

    private func snapshot(_ model: SpeechModel, _ state: State) -> [String: Any] {
        var fields = model.publicFields
        // 内部仍保留源 speaker 映射，但跨桥只暴露三端共用的音色元数据。
        if let voices = fields["voices"] as? [[String: Any]] {
            fields["voices"] = voices.map { voice -> [String: Any] in
                var output: [String: Any] = [:]
                for key in ["id", "name", "language"] { output[key] = voice[key] }
                return output
            }
        }
        fields["status"] = state.status
        fields["downloadedBytes"] = state.downloadedBytes
        fields["totalBytes"] = model.totalBytes
        fields["filesBytes"] = model.filesBytes
        fields["error"] = state.error
        return fields
    }

    public func list() throws -> [String: Any] {
        Self.lock.lock(); defer { Self.lock.unlock() }
        return ["defaults": defaults, "models": models.map { model -> [String: Any] in
            let state = self.state(model)
            if state.job == nil {
                let target = destination(model)
                let exists = (try? SpeechFiles.exists(target)) == true || (try? SpeechFiles.exists(retired(model))) == true
                if exists && (state.status == "not-installed" || state.status == "installed") {
                    let proof = try? SpeechFiles.tree(target, files: model.files, complete: true)
                    if state.proof == nil || proof != state.proof { schedule(model, state, download: false) }
                } else if state.status == "installed" {
                    state.status = "not-installed"; state.proof = nil; state.downloadedBytes = 0
                }
            }
            return snapshot(model, state)
        }]
    }

    public func startDownload(modelId: String) throws -> [String: Any] {
        let model = try self.model(id: modelId)
        Self.lock.lock(); defer { Self.lock.unlock() }
        let state = self.state(model)
        if state.job == nil { schedule(model, state, download: true) }
        return snapshot(model, state)
    }

    public func cancelDownload(modelId: String) throws -> [String: Any] {
        let model = try self.model(id: modelId)
        Self.lock.lock()
        let state = self.state(model)
        let job = state.job
        job?.cancel()
        Self.lock.unlock()
        // 同步返回前等待网络 delegate 和提取器结束，续传绝不与未关闭的写句柄重叠。
        job?.wait()
        Self.lock.lock(); defer { Self.lock.unlock() }
        if state.job === job && job != nil {
            state.job = nil; state.status = "cancelled"; state.error = nil
        }
        return snapshot(model, state)
    }

    private func schedule(_ model: SpeechModel, _ state: State, download: Bool) {
        if Self.states.values.filter({ $0.job != nil }).count >= 64 {
            state.status = "error"; state.error = SpeechStorageError.busy.rawValue
            return
        }
        let job = SpeechCancellation()
        state.job = job
        state.status = download ? "downloading" : "verifying"
        state.error = nil
        Self.worker.async {
            guard job.begin() else { return }
            defer { job.finish() }
            do {
                if download { try self.install(model, job: job) }
                else {
                    _ = try self.verifiedDirectory(model, check: job.check)
                    self.update(model, job: job) { state in
                        state.status = "installed"; state.downloadedBytes = model.totalBytes
                    }
                }
                self.update(model, job: job) { $0.job = nil }
            } catch {
                self.update(model, job: job) { state in
                    let safe = job.isCancelled ? SpeechStorageError.cancelled : speechSafeError(error)
                    state.status = safe == .cancelled ? "cancelled" : "error"
                    state.error = safe == .cancelled ? nil : safe.rawValue
                    state.proof = nil
                    state.job = nil
                }
            }
        }
    }

    private func update(_ model: SpeechModel, job: SpeechCancellation, _ body: (State) -> Void) {
        Self.lock.lock(); defer { Self.lock.unlock() }
        let state = self.state(model)
        if state.job === job { body(state) }
    }

    public func modelDirectory(modelId: String, checkCancellation: () throws -> Void = {}) throws -> URL {
        let model = try self.model(id: modelId)
        func check() throws {
            do { try checkCancellation() }
            catch { throw SpeechStorageError.cancelled }
        }
        do { return try verifiedDirectory(model, check: check) }
        catch {
            if (error as? SpeechStorageError) == .cancelled { throw SpeechStorageError.cancelled }
            throw SpeechStorageError.missing
        }
    }

    private func verifiedDirectory(_ model: SpeechModel, check: () throws -> Void = {}) throws -> URL {
        Self.filesystem.lock(); defer { Self.filesystem.unlock() }
        try check()
        try ensureOwnership()
        let target = destination(model)
        let backup = retired(model)
        if try !SpeechFiles.exists(target) && SpeechFiles.exists(backup) {
            // 先恢复自有旧版本；是否满足当前 catalog 仍由后续完整校验决定。
            let previous = try SpeechFiles.installationFiles(backup, modelId: model.id)
            _ = try SpeechFiles.tree(backup, files: previous, complete: false)
            try check()
            try SpeechFiles.rename(backup, target)
        }
        Self.lock.lock()
        let proof = state(model).proof
        Self.lock.unlock()
        let verified = try verify(model, directory: target, proof: proof, check: check)
        Self.lock.lock()
        let current = state(model)
        current.proof = verified
        if current.job == nil {
            current.status = "installed"
            current.downloadedBytes = model.totalBytes
            current.error = nil
        }
        Self.lock.unlock()
        return target
    }

    private func verify(_ model: SpeechModel, directory: URL, proof: String?,
                        check: () throws -> Void) throws -> String {
        let before = try SpeechFiles.tree(directory, files: model.files, complete: true)
        try check()
        if before == proof { return before }
        let data = try SpeechFiles.readSmall(directory.appendingPathComponent(SpeechFiles.marker), limit: 1024 * 1024)
        guard let marker = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              marker["id"] as? String == model.id, marker["fingerprint"] as? String == model.fingerprint,
              try speechInteger(marker["version"]) == 1 else { throw SpeechStorageError.integrity }
        let recorded = try SpeechFiles.installationFiles(directory, modelId: model.id)
        try speechRequire(recorded.count == model.files.count && zip(recorded, model.files).allSatisfy { pair in
            pair.0.path == pair.1.path && pair.0.bytes == pair.1.bytes && pair.0.sha256 == pair.1.sha256
        }, .integrity)
        for file in model.files { try SpeechFiles.scan(directory.appendingPathComponent(file.path), spec: file, check: check) }
        let after = try SpeechFiles.tree(directory, files: model.files, complete: true)
        try speechRequire(before == after, .path)
        try check()
        return after
    }

    private func checkSpace(_ model: SpeechModel, stage: URL) throws {
        func retained(_ url: URL, maximum: Int64) throws -> Int64 {
            guard (try? SpeechFiles.exists(url)) == true else { return 0 }
            let handle = try SpeechFiles.open(url)
            defer { try? handle.close() }
            return min(maximum, Int64(try SpeechFiles.attributes(handle.fileDescriptor).st_size))
        }
        let allowance: Int64 = 32 * 1024 * 1024
        let required: Int64
        if let archive = model.archive {
            let cached = root.appendingPathComponent(".\(model.id).\(model.fingerprint).tar.bz2")
            let downloaded = try retained(cached, maximum: archive.file.bytes)
            // 归档绑定的 framing 预检会保留一份展开 tar；它与压缩包、安装树可能同时存在。
            required = archive.file.bytes - downloaded + (model.filesBytes + allowance)
                + model.filesBytes + allowance
        } else {
            var remaining: Int64 = 0
            for file in model.files {
                remaining += try file.bytes - retained(stage.appendingPathComponent(file.path), maximum: file.bytes)
            }
            required = remaining + allowance
        }
        let attributes = try FileManager.default.attributesOfFileSystem(forPath: root.path)
        guard let available = attributes[.systemFreeSize] as? NSNumber else { throw SpeechStorageError.storage }
        try speechRequire(available.int64Value >= required, .space)
    }

    private func install(_ model: SpeechModel, job: SpeechCancellation) throws {
        try job.check()
        do {
            _ = try verifiedDirectory(model, check: job.check)
            update(model, job: job) { $0.status = "installed"; $0.downloadedBytes = model.totalBytes }
            return
        } catch {
            try job.check()
            if let safe = error as? SpeechStorageError, safe == .recovery { throw safe }
        }
        try ensureOwnership()
        let stage = staging(model)
        let fd = try SpeechFiles.directory(stage, create: true)
        Darwin.close(fd)
        _ = try SpeechFiles.tree(stage, files: model.files, complete: false)
        try checkSpace(model, stage: stage)
        if let archive = model.archive {
            let cached = root.appendingPathComponent(".\(model.id).\(model.fingerprint).tar.bz2")
            try transfer(cached, archive.file, job) { bytes in self.update(model, job: job) { $0.downloadedBytes = bytes } }
            try SpeechFiles.scan(cached, spec: archive.file, check: job.check)
            update(model, job: job) { $0.status = "verifying" }
            try SpeechFiles.removeOwnedTree(stage, files: model.files)
            try extractor(cached, stage, archive.stripPrefix, model.files, job.check)
            try SpeechFiles.scan(cached, spec: archive.file, check: job.check)
        } else {
            var completed: Int64 = 0
            for file in model.files {
                try job.check()
                let target = stage.appendingPathComponent(file.path)
                let parent = try SpeechFiles.directory(target.deletingLastPathComponent(), create: true)
                Darwin.close(parent)
                let base = completed
                try transfer(target, file, job) { bytes in self.update(model, job: job) { $0.downloadedBytes = base + bytes } }
                completed += file.bytes
            }
        }
        update(model, job: job) { $0.status = "verifying" }
        _ = try SpeechFiles.tree(stage, files: model.files, complete: true)
        for file in model.files { try SpeechFiles.scan(stage.appendingPathComponent(file.path), spec: file, check: job.check) }
        let marker: [String: Any] = [
            "version": 1, "id": model.id, "fingerprint": model.fingerprint,
            "files": model.files.map { ["path": $0.path, "bytes": $0.bytes, "sha256": $0.sha256] as [String: Any] },
        ]
        try SpeechFiles.writeSmall(stage.appendingPathComponent(SpeechFiles.marker),
                                   data: JSONSerialization.data(withJSONObject: marker, options: [.sortedKeys]), exclusive: false)
        let stageProof = try verify(model, directory: stage, proof: nil, check: job.check)
        Self.filesystem.lock(); defer { Self.filesystem.unlock() }
        Self.lock.lock(); defer { Self.lock.unlock() }
        try job.check()
        try ensureOwnership()
        try speechRequire(stageProof == SpeechFiles.tree(stage, files: model.files, complete: true), .path)
        let target = destination(model)
        let backup = retired(model)
        let previous = try SpeechFiles.exists(target)
            ? SpeechFiles.installationFiles(target, modelId: model.id) : model.files
        try SpeechFiles.publish(stage, target: target, backup: backup, files: previous)
        let state = self.state(model)
        // 目录 rename 改变自身 ctime；重新采集已验证子树身份，无需再次读取权重。
        state.proof = try? SpeechFiles.tree(target, files: model.files, complete: true)
        state.status = "installed"
        state.downloadedBytes = model.totalBytes
        state.error = nil
        if model.archive != nil {
            try? SpeechFiles.unlink(root.appendingPathComponent(".\(model.id).\(model.fingerprint).tar.bz2"))
        }
    }

    private static func download(_ url: URL, spec: SpeechDownloadFile, job: SpeechCancellation,
                                 progress: @escaping (Int64) -> Void) throws {
        var last: Error = SpeechStorageError.http
        for source in spec.urls {
            for _ in 0..<2 {
                try job.check()
                do {
                    let handle = try SpeechFiles.open(url, writable: true)
                    defer { try? handle.close() }
                    var size = Int64(try SpeechFiles.attributes(handle.fileDescriptor).st_size)
                    if size > spec.bytes { try handle.truncate(atOffset: 0); size = 0 }
                    if size == spec.bytes {
                        do {
                            try SpeechFiles.scan(url, spec: spec, check: job.check)
                            progress(size)
                            return
                        } catch {
                            try job.check()
                            if speechSafeError(error) != .integrity { throw error }
                            try handle.truncate(atOffset: 0)
                            size = 0
                        }
                    }
                    progress(size)
                    try SpeechHTTPTransfer(file: handle, spec: spec, offset: size,
                                           cancellation: job, progress: progress).run(source.absoluteString)
                    do { try SpeechFiles.scan(url, spec: spec, check: job.check) }
                    catch {
                        if speechSafeError(error) == .integrity { try handle.truncate(atOffset: 0); progress(0) }
                        throw error
                    }
                    return
                } catch {
                    try job.check()
                    last = speechSafeError(error)
                    if let safe = last as? SpeechStorageError, safe == .path || safe == .storage { throw safe }
                }
            }
        }
        throw last
    }
}
