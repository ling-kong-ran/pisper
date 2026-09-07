import Darwin
import Foundation
import XCTest
#if canImport(PisperSpeechFoundation)
@testable import PisperSpeechFoundation
#else
@testable import pisper_mobile_device_plugin
#endif

final class SpeechAudioServiceTests: XCTestCase {
    private var temporary: URL!

    override func setUpWithError() throws {
        let physical = try XCTUnwrap(realpath(FileManager.default.temporaryDirectory.path, nil))
        defer { free(physical) }
        temporary = URL(fileURLWithPath: String(cString: physical), isDirectory: true)
            .appendingPathComponent("pisper-speech-roots-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: false)
    }

    override func tearDownWithError() throws {
        if let temporary { try FileManager.default.removeItem(at: temporary) }
    }

    private func assertPathError(_ operation: () throws -> Void,
                                 file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try operation(), file: file, line: line) {
            XCTAssertEqual($0 as? SpeechStorageError, .path, file: file, line: line)
        }
    }

    func testFoundationAliasFailsStrictTraversalButTrustedRootPreservesPhysicalAncestors() throws {
        let physical = URL(fileURLWithPath: "/private/var", isDirectory: true)
        let alias = physical.resolvingSymlinksInPath()
        XCTAssertEqual(alias.path, "/var")
        assertPathError { _ = try SpeechFiles.directory(alias) }
        let resolved = try SpeechTrustedRoots.directory(alias)
        XCTAssertEqual(resolved.path, "/private/var")
        let descriptor = try SpeechFiles.directory(resolved)
        Darwin.close(descriptor)
    }

    func testTrustedRootRejectsMissingDirectoriesAndRegularFiles() throws {
        assertPathError { _ = try SpeechTrustedRoots.directory(temporary.appendingPathComponent("missing")) }
        let file = temporary.appendingPathComponent("file")
        try Data().write(to: file)
        assertPathError { _ = try SpeechTrustedRoots.directory(file) }
        assertPathError { _ = try SpeechTrustedRoots.directory(URL(string: "https://example.com")!) }
    }

    func testResolvingTrustedRootDoesNotPermitLinkedChildren() throws {
        let alias = temporary.appendingPathComponent("system-root")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: temporary)
        let root = try SpeechTrustedRoots.directory(alias)
        XCTAssertEqual(root.path, temporary.path)
        let child = root.appendingPathComponent("linked-child")
        try FileManager.default.createSymbolicLink(at: child, withDestinationURL: temporary)
        assertPathError { _ = try SpeechFiles.directory(child) }
        assertPathError { _ = try SpeechFiles.readSmall(child.appendingPathComponent("file"), limit: 10) }
    }

    func testBundledCatalogFirstListReportsSharedDefaultsWithoutDownloading() throws {
        let alias = temporary.appendingPathComponent("system-support")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: temporary)
        let provider = try SpeechAudioModelAdapter.bundled(supportDirectory: alias)
        let list = try provider.list()
        XCTAssertEqual(list["defaults"] as? [String: String], [
            "asr": "x-asr-480ms-int8", "tts": "vits-melo-tts-zh_en", "voice": "melo-zh-en-female",
        ])
        let models = try XCTUnwrap(list["models"] as? [[String: Any]])
        XCTAssertEqual(models.count, 2)
        XCTAssertTrue(models.allSatisfy { $0["status"] as? String == "not-installed" })
        let storage = temporary.appendingPathComponent("pisper-speech-models")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: storage.path), [".pisper-speech-owner"])
        XCTAssertNoThrow(try provider.list())
    }

    func testBundledFirstListThroughSearchOnlySupportAncestor() throws {
        try XCTSkipIf(geteuid() == 0, "Root bypasses directory permission checks")
        let ancestor = temporary.appendingPathComponent("container-ancestor")
        let support = ancestor.appendingPathComponent("Application Support")
        try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        XCTAssertEqual(chmod(ancestor.path, 0o111), 0)
        defer { chmod(ancestor.path, 0o700) }
        let provider = try SpeechAudioModelAdapter.bundled(supportDirectory: support)
        let list = try provider.list()
        let models = try XCTUnwrap(list["models"] as? [[String: Any]])
        XCTAssertEqual(models.count, 2)
        XCTAssertTrue(models.allSatisfy { $0["status"] as? String == "not-installed" })
        XCTAssertEqual((list["defaults"] as? [String: String])?["tts"], "vits-melo-tts-zh_en")
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath:
            support.appendingPathComponent("pisper-speech-models").path), [".pisper-speech-owner"])
    }

    func testBundledInitializationRejectsLinkedModelStorage() throws {
        let storage = temporary.appendingPathComponent("pisper-speech-models")
        try FileManager.default.createSymbolicLink(at: storage, withDestinationURL: temporary)
        assertPathError { _ = try SpeechAudioModelAdapter.bundled(supportDirectory: temporary) }
    }

    func testBundledInitializationRejectsLinkedResourceDirectory() throws {
        let resources = temporary.appendingPathComponent("SpeechResources")
        try FileManager.default.createSymbolicLink(at: resources, withDestinationURL: temporary)
        assertPathError {
            _ = try SpeechAudioModelAdapter.bundled(bundleRoot: temporary, supportDirectory: temporary)
        }
    }

    func testBundledInitializationRejectsLinkedCatalog() throws {
        let resources = temporary.appendingPathComponent("SpeechResources")
        try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: false)
        let file = temporary.appendingPathComponent("catalog.json")
        try Data("{}".utf8).write(to: file)
        try FileManager.default.createSymbolicLink(at: resources.appendingPathComponent("speech-model-catalog.json"),
                                                  withDestinationURL: file)
        assertPathError {
            _ = try SpeechAudioModelAdapter.bundled(bundleRoot: temporary, supportDirectory: temporary)
        }
    }
}
