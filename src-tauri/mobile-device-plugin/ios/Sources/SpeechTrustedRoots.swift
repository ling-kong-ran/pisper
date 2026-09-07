import Darwin
import Foundation

// 仅解析系统返回的 bundle/Application Support 根；资源、模型和归档子路径仍须拒绝链接。
enum SpeechTrustedRoots {
    static func directory(_ systemURL: URL) throws -> URL {
        let checked = try SpeechFiles.root(systemURL)
        guard let resolved = realpath(checked.path, nil) else { throw SpeechStorageError.path }
        defer { free(resolved) }
        // Foundation 会将 /private/var 美化成 /var，不能再次 resolvingSymlinksInPath。
        let physical = URL(fileURLWithPath: String(cString: resolved), isDirectory: true)
        let descriptor = try SpeechFiles.directory(physical)
        defer { Darwin.close(descriptor) }
        return physical
    }
}
