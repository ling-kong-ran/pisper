import Foundation
import XCTest
import CryptoKit
import Darwin
#if canImport(PisperSpeechFoundation)
@testable import PisperSpeechFoundation
#else
@testable import pisper_mobile_device_plugin
#endif

final class SpeechModelStoreTests: XCTestCase {
    private var temporary: URL!
    private var stores: [SpeechModelStore] = []

    override func setUpWithError() throws {
        temporary = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
            .appendingPathComponent("pisper-speech-tests-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        for store in stores {
            _ = try? store.cancelDownload(modelId: "asr")
            _ = try? store.cancelDownload(modelId: "vits-melo-tts-zh_en")
        }
        stores.removeAll()
        if let temporary = temporary { try FileManager.default.removeItem(at: temporary) }
    }

    private struct Fixture {
        var catalog: [String: Any]
        var content: [String: Data]
    }

    private func fixture(archive: Bool = false) throws -> Fixture {
        let content: [String: Data] = [
            "asr.onnx": Data(repeating: 0x31, count: 128 * 1024),
            "model.onnx": Data((0..<(65536 + 128)).map { UInt8($0 % 251) }),
            "tokens.txt": Data("tokens".utf8),
            "lexicon.txt": Data("lexicon".utf8),
            "dict/jieba.dict.utf8": Data("dictionary".utf8),
            "dict/pos_dict/prob_start.utf8": Data("nested-dictionary".utf8),
            "date.fst": Data("rule".utf8),
            "archive.tar.bz2": Data("injected-archive-fixture".utf8),
        ]
        func file(_ path: String) -> [String: Any] {
            let data = content[path]!
            return ["path": path, "bytes": data.count, "sha256": speechHash(data),
                    "urls": ["https://huggingface.co/test/" + path]]
        }
        var asr: [String: Any] = [
            "id": "asr", "revision": "r1", "kind": "asr", "engine": "sense-voice", "name": "ASR",
            "languages": ["en"], "license": ["name": "test"],
            "files": [file("asr.onnx"), file("tokens.txt")],
            "config": ["model": "asr.onnx", "tokens": "tokens.txt"],
        ]
        if archive {
            asr["archive"] = ["format": "tar.bz2", "bytes": content["archive.tar.bz2"]!.count,
                              "sha256": speechHash(content["archive.tar.bz2"]!),
                              "urls": ["https://github.com/test/archive.tar.bz2"], "stripPrefix": "fixture/"]
        }
        let config: [String: Any] = [
            "model": "model.onnx", "tokens": "tokens.txt", "lexicon": "lexicon.txt", "dictDir": "dict",
            "ruleFsts": ["date.fst"], "numThreads": 4, "maxTextCodePoints": 16,
        ]
        let voices: [[String: Any]] = [
            ["id": "melo-zh-en-female", "name": "Female", "language": "zh", "sid": 0],
        ]
        let tts: [String: Any] = [
            "id": "vits-melo-tts-zh_en", "revision": "r1", "kind": "tts", "engine": "vits", "name": "TTS",
            "languages": ["zh", "en"], "license": ["name": "test"], "voices": voices,
            "files": [file("model.onnx"), file("tokens.txt"), file("lexicon.txt"),
                      file("dict/jieba.dict.utf8"), file("dict/pos_dict/prob_start.utf8"), file("date.fst")],
            "config": config,
        ]
        return Fixture(catalog: ["version": 1,
                                 "defaults": ["asr": "asr", "tts": "vits-melo-tts-zh_en", "voice": "melo-zh-en-female"],
                                 "models": [asr, tts]], content: content)
    }

    private func store(_ fixture: Fixture, root: URL? = nil,
                       extractor: SpeechArchiveExtractor? = nil,
                       transfer: SpeechModelStore.Transfer? = nil) throws -> SpeechModelStore {
        let root = root ?? temporary.appendingPathComponent("store-" + UUID().uuidString)
        let store = try SpeechModelStore(
            catalogData: JSONSerialization.data(withJSONObject: fixture.catalog), storageDirectory: root,
            archiveExtractor: extractor ?? { _, destination, prefix, files, check in
                XCTAssertEqual(prefix, "fixture/")
                try check()
                for spec in files { try self.write(destination.appendingPathComponent(spec.path), fixture.content[spec.path]!) }
            },
            transfer: transfer ?? { target, spec, job, progress in
                try job.check()
                try self.write(target, fixture.content[spec.path]!)
                progress(spec.bytes)
            }
        )
        stores.append(store)
        return store
    }

    private func write(_ url: URL, _ data: Data) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url)
    }

    private func snapshot(_ store: SpeechModelStore, id: String) throws -> [String: Any] {
        let list = try XCTUnwrap(try store.list()["models"] as? [[String: Any]])
        return try XCTUnwrap(list.first { $0["id"] as? String == id })
    }

    private func waitFor(_ store: SpeechModelStore, id: String, status: String = "installed") throws {
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            let current = try snapshot(store, id: id)
            if current["status"] as? String == status { return }
            if current["status"] as? String == "error" {
                XCTFail("Unexpected status: \(current["error"] ?? "error")")
                throw SpeechStorageError.storage
            }
            Thread.sleep(forTimeInterval: 0.01)
        }
        XCTFail("Model task did not finish")
        throw SpeechStorageError.busy
    }

    private func changingTTS(_ body: (inout [String: Any]) -> Void) throws -> Fixture {
        var fixture = try fixture()
        var models = fixture.catalog["models"] as! [[String: Any]]
        body(&models[1])
        fixture.catalog["models"] = models
        return fixture
    }

    private func changingConfig(_ body: (inout [String: Any]) -> Void) throws -> Fixture {
        try changingTTS { model in
            var config = model["config"] as! [String: Any]
            body(&config)
            model["config"] = config
        }
    }

    func testStartupOnlyReportsStateWithoutDownloading() throws {
        var transfers = 0
        let store = try store(fixture(), transfer: { _, _, _, _ in transfers += 1 })
        XCTAssertEqual(try snapshot(store, id: "asr")["status"] as? String, "not-installed")
        XCTAssertEqual(transfers, 0)
        XCTAssertEqual(store.defaultASRModelId, "asr")
        XCTAssertEqual(store.defaultTTSModelId, "vits-melo-tts-zh_en")
        XCTAssertEqual(store.defaultVoiceId, "melo-zh-en-female")
        let voices = try XCTUnwrap(try snapshot(store, id: "vits-melo-tts-zh_en")["voices"] as? [[String: Any]])
        XCTAssertEqual(voices.first?["id"] as? String, "melo-zh-en-female")
        XCTAssertNil(voices.first?["sid"])
        XCTAssertEqual(try store.voice(id: "melo-zh-en-female").sourceSpeaker, 0)
        XCTAssertThrowsError(try store.modelDirectory(modelId: "asr"))
    }

    func testCatalogRejectsUnsafePathsCaseAliasesAndBooleanSizes() throws {
        for path in ["../escape", "a/../b", "/absolute", "a\\b", "CON.txt", "a\u{200b}b", "a."] {
            XCTAssertThrowsError(try SpeechFiles.relative(path), path)
        }
        for replacement: Any in [true, -1, 1.5] {
            var fixture = try fixture()
            var models = fixture.catalog["models"] as! [[String: Any]]
            var files = models[0]["files"] as! [[String: Any]]
            files[0]["bytes"] = replacement
            models[0]["files"] = files
            fixture.catalog["models"] = models
            XCTAssertThrowsError(try store(fixture))
        }
        var fixture = try fixture()
        var models = fixture.catalog["models"] as! [[String: Any]]
        var files = models[0]["files"] as! [[String: Any]]
        var duplicate = files[0]
        duplicate["path"] = "ASR.ONNX"
        files.append(duplicate)
        models[0]["files"] = files
        fixture.catalog["models"] = models
        XCTAssertThrowsError(try store(fixture))
    }

    func testTrustedHTTPSRejectsCredentialsPortsAndLocalTargets() throws {
        for url in ["http://github.com/file", "https://localhost/file", "https://127.0.0.1/file",
                    "https://github.com.evil.example/file", "https://user:pass@github.com/file",
                    "https://github.com:444/file", "https://github.com/file#fragment"] {
            XCTAssertThrowsError(try SpeechHTTPTransfer.trustedURL(url), url)
        }
        XCTAssertNoThrow(try SpeechHTTPTransfer.trustedURL("https://release-assets.githubusercontent.com/file?signature=x"))
    }

    func testStrictRangeAndLengthContract() throws {
        let url = URL(string: "https://huggingface.co/file")!
        func response(_ status: Int, _ headers: [String: String]) -> HTTPURLResponse {
            HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: headers)!
        }
        XCTAssertEqual(try SpeechHTTPTransfer.responseOffset(response(206, ["Content-Range": "bytes 4-9/10", "Content-Length": "6"]), offset: 4, bytes: 10), 4)
        XCTAssertEqual(try SpeechHTTPTransfer.responseOffset(response(200, ["Content-Length": "10"]), offset: 4, bytes: 10), 0)
        for range in ["bytes 3-9/10", "bytes 4-8/10", "bytes 4-9/*", "bytes 4-9/11"] {
            XCTAssertThrowsError(try SpeechHTTPTransfer.responseOffset(response(206, ["Content-Range": range]), offset: 4, bytes: 10))
        }
        XCTAssertThrowsError(try SpeechHTTPTransfer.responseOffset(response(206, ["Content-Range": "bytes 0-9/10"]), offset: 0, bytes: 10))
        XCTAssertThrowsError(try SpeechHTTPTransfer.responseOffset(response(200, ["Content-Range": "bytes 0-9/10"]), offset: 0, bytes: 10))
        for length in ["09", "-1", "10x", "11", "9999999999999999999999999"] {
            XCTAssertThrowsError(try SpeechHTTPTransfer.responseOffset(response(200, ["Content-Length": length]), offset: 0, bytes: 10))
        }
        XCTAssertThrowsError(try SpeechHTTPTransfer.responseOffset(response(200, ["Content-Encoding": "gzip"]), offset: 0, bytes: 10))
        XCTAssertThrowsError(try SpeechHTTPTransfer.responseOffset(response(416, [:]), offset: 10, bytes: 10))
    }

    func testPerFileDownloadPublishesAndVerifiesBytes() throws {
        let fixture = try fixture()
        let store = try store(fixture)
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr")
        let directory = try store.modelDirectory(modelId: "asr")
        XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("asr.onnx")), fixture.content["asr.onnx"])
        let model = try store.model(id: "asr")
        XCTAssertEqual(try snapshot(store, id: "asr")["downloadedBytes"] as? Int64, model.totalBytes)
    }

    func testArchiveExtractionUsesVerifiedManifestAndIndependentTreeCheck() throws {
        let fixture = try fixture(archive: true)
        let store = try store(fixture, extractor: { archive, directory, prefix, files, check in
            XCTAssertEqual(try Data(contentsOf: archive), fixture.content["archive.tar.bz2"])
            XCTAssertEqual(prefix, "fixture/")
            XCTAssertEqual(files.count, 2)
            XCTAssertFalse(FileManager.default.fileExists(atPath: directory.path))
            let parent = try SpeechFiles.directory(directory.deletingLastPathComponent())
            defer { Darwin.close(parent) }
            XCTAssertEqual(try SpeechFiles.attributes(parent).st_mode & 0o777, 0o700)
            try check()
            for file in files { try self.write(directory.appendingPathComponent(file.path), fixture.content[file.path]!) }
        })
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr")
        XCTAssertNoThrow(try store.modelDirectory(modelId: "asr"))
    }

    func testArchiveExtraFileFailsWithoutDeletingForeignData() throws {
        let fixture = try fixture(archive: true)
        let root = temporary.appendingPathComponent("store")
        let store = try store(fixture, root: root, extractor: { _, directory, _, files, _ in
            for file in files { try self.write(directory.appendingPathComponent(file.path), fixture.content[file.path]!) }
            try self.write(directory.appendingPathComponent("foreign.txt"), Data("keep".utf8))
        })
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr", status: "error")
        let model = try store.model(id: "asr")
        let foreign = root.appendingPathComponent(".asr.\(model.fingerprint).partial/foreign.txt")
        XCTAssertEqual(try Data(contentsOf: foreign), Data("keep".utf8))
        XCTAssertThrowsError(try store.modelDirectory(modelId: "asr"))
    }

    func testSameModelDeduplicatesAcrossStoreInstancesAndModelsSerialize() throws {
        let fixture = try fixture()
        let root = temporary.appendingPathComponent("shared-store")
        let entered = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        let counter = NSLock()
        var active = 0
        var maximum = 0
        var asrTransfers = 0
        let transfer: SpeechModelStore.Transfer = { target, spec, job, progress in
            counter.lock()
            active += 1
            maximum = max(maximum, active)
            if spec.path == "asr.onnx" { asrTransfers += 1 }
            counter.unlock()
            defer { counter.lock(); active -= 1; counter.unlock() }
            if spec.path == "asr.onnx" {
                entered.signal()
                XCTAssertEqual(release.wait(timeout: .now() + 3), .success)
            }
            try job.check()
            try self.write(target, fixture.content[spec.path]!)
            progress(spec.bytes)
        }
        let first = try store(fixture, root: root, transfer: transfer)
        let second = try store(fixture, root: root, transfer: transfer)
        _ = try first.startDownload(modelId: "asr")
        XCTAssertEqual(entered.wait(timeout: .now() + 2), .success)
        _ = try second.startDownload(modelId: "asr")
        _ = try second.startDownload(modelId: "vits-melo-tts-zh_en")
        release.signal()
        try waitFor(first, id: "asr")
        try waitFor(second, id: "vits-melo-tts-zh_en")
        counter.lock(); defer { counter.unlock() }
        XCTAssertEqual(asrTransfers, 1)
        XCTAssertEqual(maximum, 1)
    }

    func testCancellationWaitsForClosedHandleAndRetainsPartialForResume() throws {
        let fixture = try fixture()
        let entered = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        let cancelled = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var firstAttempt = true
        var closed = false
        var resumedAt: Int64 = -1
        let store = try store(fixture, transfer: { target, spec, job, progress in
            if spec.path != "asr.onnx" { try self.write(target, fixture.content[spec.path]!); progress(spec.bytes); return }
            let handle = try SpeechFiles.open(target, writable: true)
            defer { try? handle.close(); lock.lock(); closed = true; lock.unlock() }
            lock.lock()
            let first = firstAttempt
            firstAttempt = false
            lock.unlock()
            if first {
                try handle.write(contentsOf: fixture.content[spec.path]!.prefix(65536))
                progress(65536)
                entered.signal()
                XCTAssertEqual(release.wait(timeout: .now() + 3), .success)
                try job.check()
            } else {
                let offset = try handle.seekToEnd()
                lock.lock(); resumedAt = Int64(offset); lock.unlock()
                try handle.write(contentsOf: fixture.content[spec.path]!.dropFirst(Int(offset)))
                progress(spec.bytes)
            }
        })
        _ = try store.startDownload(modelId: "asr")
        XCTAssertEqual(entered.wait(timeout: .now() + 2), .success)
        XCTAssertEqual(try snapshot(store, id: "asr")["downloadedBytes"] as? Int64, 65536)
        DispatchQueue.global().async { _ = try? store.cancelDownload(modelId: "asr"); cancelled.signal() }
        XCTAssertEqual(cancelled.wait(timeout: .now() + 0.05), .timedOut)
        release.signal()
        XCTAssertEqual(cancelled.wait(timeout: .now() + 2), .success)
        lock.lock(); let didClose = closed; lock.unlock()
        XCTAssertTrue(didClose)
        XCTAssertEqual(try snapshot(store, id: "asr")["status"] as? String, "cancelled")
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr")
        lock.lock(); let offset = resumedAt; lock.unlock()
        XCTAssertEqual(offset, 65536)
    }

    func testMarkerCannotAuthorizeCorruptedSourceOrUnexpectedTree() throws {
        let fixture = try fixture()
        let store = try store(fixture)
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr")
        let directory = try store.modelDirectory(modelId: "asr")
        try write(directory.appendingPathComponent("asr.onnx"), Data(repeating: 0x32, count: 128 * 1024))
        XCTAssertThrowsError(try store.modelDirectory(modelId: "asr")) { error in
            XCTAssertEqual(error as? SpeechStorageError, .missing)
            XCTAssertFalse(error.localizedDescription.contains(directory.path))
        }
        try write(directory.appendingPathComponent("asr.onnx"), fixture.content["asr.onnx"]!)
        try write(directory.appendingPathComponent("extra"), Data([1]))
        XCTAssertThrowsError(try store.modelDirectory(modelId: "asr"))
    }

    func testModelDirectoryFirstHashPassCanBeCancelledAndRetried() throws {
        let fixture = try fixture()
        let root = temporary.appendingPathComponent("store")
        let store = try store(fixture, root: root)
        let model = try store.model(id: "asr")
        let directory = root.appendingPathComponent("asr")
        for file in model.files { try write(directory.appendingPathComponent(file.path), fixture.content[file.path]!) }
        let marker: [String: Any] = [
            "version": 1, "id": model.id, "fingerprint": model.fingerprint,
            "files": model.files.map { ["path": $0.path, "bytes": $0.bytes, "sha256": $0.sha256] as [String: Any] },
        ]
        try write(directory.appendingPathComponent(SpeechFiles.marker), JSONSerialization.data(withJSONObject: marker))
        var checks = 0
        XCTAssertThrowsError(try store.modelDirectory(modelId: "asr", checkCancellation: {
            checks += 1
            if checks == 3 { throw NSError(domain: "private-cancellation-detail", code: 1) }
        })) { error in XCTAssertEqual(error as? SpeechStorageError, .cancelled) }
        XCTAssertEqual(checks, 3)
        XCTAssertNoThrow(try store.modelDirectory(modelId: "asr"))
        XCTAssertEqual(try snapshot(store, id: "asr")["status"] as? String, "installed")
    }

    func testMissingMarkerFailsEvenAfterSuccessfulProof() throws {
        let store = try store(fixture())
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr")
        let directory = try store.modelDirectory(modelId: "asr")
        try FileManager.default.removeItem(at: directory.appendingPathComponent(SpeechFiles.marker))
        XCTAssertThrowsError(try store.modelDirectory(modelId: "asr"))
    }

    func testLinksAreRejectedIncludingLinksInsideRootAndHardLinks() throws {
        let source = temporary.appendingPathComponent("source")
        try write(source, Data("abc".utf8))
        let link = temporary.appendingPathComponent("link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: source)
        XCTAssertThrowsError(try SpeechFiles.open(link))
        let hard = temporary.appendingPathComponent("hard")
        try FileManager.default.linkItem(at: source, to: hard)
        XCTAssertThrowsError(try SpeechFiles.open(source))
        XCTAssertThrowsError(try SpeechFiles.open(hard))
        let actual = temporary.appendingPathComponent("actual")
        try FileManager.default.createDirectory(at: actual, withIntermediateDirectories: true)
        let directoryLink = temporary.appendingPathComponent("directory-link")
        try FileManager.default.createSymbolicLink(at: directoryLink, withDestinationURL: actual)
        XCTAssertThrowsError(try SpeechFiles.directory(directoryLink))
    }

    func testUnownedStorageIsNotAdoptedOrDeleted() throws {
        let root = temporary.appendingPathComponent("unowned")
        let foreign = root.appendingPathComponent("notes.txt")
        try write(foreign, Data("user".utf8))
        XCTAssertThrowsError(try store(fixture(), root: root))
        XCTAssertEqual(try Data(contentsOf: foreign), Data("user".utf8))
    }

    func testExistingUnmarkedModelDirectoryIsNotReplaced() throws {
        let fixture = try fixture()
        let root = temporary.appendingPathComponent("store")
        let store = try store(fixture, root: root)
        let foreign = root.appendingPathComponent("asr/asr.onnx")
        try write(foreign, Data("user-data".utf8))
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr", status: "error")
        XCTAssertEqual(try Data(contentsOf: foreign), Data("user-data".utf8))
    }

    func testAncestorMtimeDoesNotInvalidateContentProof() throws {
        let fixture = try fixture()
        let store = try store(fixture)
        _ = try store.startDownload(modelId: "asr")
        try waitFor(store, id: "asr")
        let directory = try store.modelDirectory(modelId: "asr")
        let files = try store.model(id: "asr").files
        let before = try SpeechFiles.tree(directory, files: files, complete: true)
        try write(directory.deletingLastPathComponent().appendingPathComponent("unrelated"), Data([1]))
        let after = try SpeechFiles.tree(directory, files: files, complete: true)
        XCTAssertEqual(before, after)
        XCTAssertNoThrow(try store.modelDirectory(modelId: "asr"))
    }

    func testPublicationFailureRollsBackOldInstallation() throws {
        let target = temporary.appendingPathComponent("target")
        let stage = temporary.appendingPathComponent("stage")
        let backup = temporary.appendingPathComponent("backup")
        try write(target.appendingPathComponent("weight"), Data("old".utf8))
        try write(stage.appendingPathComponent("weight"), Data("new".utf8))
        let files = [SpeechDownloadFile(path: "weight", bytes: 3, sha256: speechHash(Data("new".utf8)), urls: [])]
        var moves = 0
        XCTAssertThrowsError(try SpeechFiles.publish(stage, target: target, backup: backup, files: files) { from, to in
            moves += 1
            if moves == 2 { throw SpeechStorageError.storage }
            try SpeechFiles.rename(from, to)
        })
        XCTAssertEqual(moves, 3)
        XCTAssertEqual(try Data(contentsOf: target.appendingPathComponent("weight")), Data("old".utf8))
        XCTAssertFalse(try SpeechFiles.exists(backup))
    }

    func testFailedRollbackPreservesRecoverableBackup() throws {
        let target = temporary.appendingPathComponent("target")
        let stage = temporary.appendingPathComponent("stage")
        let backup = temporary.appendingPathComponent("backup")
        try write(target.appendingPathComponent("weight"), Data("old".utf8))
        try write(stage.appendingPathComponent("weight"), Data("new".utf8))
        let files = [SpeechDownloadFile(path: "weight", bytes: 3, sha256: speechHash(Data("new".utf8)), urls: [])]
        var moves = 0
        XCTAssertThrowsError(try SpeechFiles.publish(stage, target: target, backup: backup, files: files) { from, to in
            moves += 1
            if moves > 1 { throw SpeechStorageError.storage }
            try SpeechFiles.rename(from, to)
        }) { error in XCTAssertEqual(error as? SpeechStorageError, .recovery) }
        XCTAssertEqual(try Data(contentsOf: backup.appendingPathComponent("weight")), Data("old".utf8))
    }

    func testInterruptedPublicationRecoversVerifiedBackup() throws {
        let fixture = try fixture()
        let root = temporary.appendingPathComponent("store")
        let store = try store(fixture, root: root)
        let model = try store.model(id: "asr")
        let backup = root.appendingPathComponent(".asr.\(model.fingerprint).previous")
        for file in model.files { try write(backup.appendingPathComponent(file.path), fixture.content[file.path]!) }
        let marker: [String: Any] = [
            "version": 1, "id": model.id, "fingerprint": model.fingerprint,
            "files": model.files.map { ["path": $0.path, "bytes": $0.bytes, "sha256": $0.sha256] as [String: Any] },
        ]
        try write(backup.appendingPathComponent(SpeechFiles.marker), JSONSerialization.data(withJSONObject: marker))
        XCTAssertEqual(try store.modelDirectory(modelId: "asr"), root.appendingPathComponent("asr"))
        XCTAssertFalse(try SpeechFiles.exists(backup))
    }

    func testVitsPublicVoicesRoundTripAsJSONWithoutInternalFields() throws {
        let store = try store(fixture())
        let data = try JSONSerialization.data(withJSONObject: store.list())
        let decoded = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        let models = try XCTUnwrap(decoded["models"] as? [[String: Any]])
        let model = try XCTUnwrap(models.first { $0["id"] as? String == "vits-melo-tts-zh_en" })
        let voices = try XCTUnwrap(model["voices"] as? [[String: Any]])
        XCTAssertEqual(voices.count, 1)
        let voice = try XCTUnwrap(voices.first)
        XCTAssertEqual(Set(voice.keys), Set(["id", "name", "language"]))
        XCTAssertEqual(voice["id"] as? String, "melo-zh-en-female")
        XCTAssertEqual(voice["name"] as? String, "Female")
        XCTAssertEqual(voice["language"] as? String, "zh")
        XCTAssertNil(voice["sid"])
        XCTAssertNil(model["config"])
        XCTAssertNil(model["files"])
        let internalVoice = try store.voice(id: "melo-zh-en-female")
        XCTAssertEqual(internalVoice.model.id, "vits-melo-tts-zh_en")
        XCTAssertEqual(internalVoice.model.engine, "vits")
        XCTAssertEqual(internalVoice.sourceSpeaker, 0)
        XCTAssertThrowsError(try store.voice(id: "missing")) { error in
            XCTAssertEqual(error as? SpeechStorageError, .voice)
        }
    }

    func testVitsAcceptsRequiredFilesAndOptionalRulesAndScales() throws {
        let store = try store(fixture())
        let model = try store.model(kind: "tts")
        XCTAssertEqual(model.id, "vits-melo-tts-zh_en")
        XCTAssertEqual(model.config["numThreads"] as? Int, 4)
        XCTAssertEqual(model.config["maxTextCodePoints"] as? Int, 16)
        for key in ["model", "tokens", "lexicon"] {
            let path = try XCTUnwrap(model.config[key] as? String)
            XCTAssertTrue(model.files.contains { $0.path == path })
        }
        XCTAssertEqual(model.config["dictDir"] as? String, "dict")
        XCTAssertNoThrow(try self.store(changingConfig { $0.removeValue(forKey: "ruleFsts") }))
        XCTAssertNoThrow(try self.store(changingConfig { $0["ruleFsts"] = [String]() }))
        XCTAssertNoThrow(try self.store(changingConfig {
            $0["noiseScale"] = 0.667; $0["noiseScaleW"] = 0.8; $0["lengthScale"] = 1.0
        }))
    }

    func testVitsRejectsMissingUnsafeAndUnlistedConfigPaths() throws {
        for key in ["model", "tokens", "lexicon", "dictDir", "numThreads", "maxTextCodePoints"] {
            XCTAssertThrowsError(try store(changingConfig { $0.removeValue(forKey: key) }), key)
        }
        for key in ["model", "tokens", "lexicon", "dictDir"] {
            for value: Any in ["../outside", "/absolute", "missing", "MODEL.ONNX", ["model.onnx"], true] {
                XCTAssertThrowsError(try store(changingConfig { $0[key] = value }), key)
            }
        }
        for directory in ["model.onnx", "dict/pos", ".derived", "dict/jieba.dict.utf8"] {
            XCTAssertThrowsError(try store(changingConfig { $0["dictDir"] = directory }), directory)
        }
        XCTAssertThrowsError(try store(changingConfig { $0["unknownOption"] = true }))
        XCTAssertThrowsError(try store(changingTTS { $0["engine"] = "unsupported" }))
    }

    func testVitsRejectsInvalidRulesAndCommaDelimitedWhitelistFiles() throws {
        for value: Any in ["date.fst", ["missing.fst"], ["../date.fst"], ["DATE.FST"], [true]] {
            XCTAssertThrowsError(try store(changingConfig { $0["ruleFsts"] = value }))
        }
        // 即使文件列入白名单，也不能让逗号被原生接口解释为多个资源。
        for key in ["lexicon", "ruleFsts"] {
            let fixture = try changingTTS { model in
                var files = model["files"] as! [[String: Any]]
                var file = files[0]
                file["path"] = "joined,resource.txt"
                files.append(file)
                model["files"] = files
                var config = model["config"] as! [String: Any]
                if key == "lexicon" { config[key] = "joined,resource.txt" }
                else { config[key] = ["joined,resource.txt"] }
                model["config"] = config
            }
            XCTAssertThrowsError(try store(fixture), key)
        }
    }

    func testVitsRejectsBooleanFractionalAndOutOfRangeLimits() throws {
        for (key, maximum) in [("numThreads", 16), ("maxTextCodePoints", 400)] {
            for value: Any in [true, false, 0, -1, 1.5, maximum + 1, "4"] {
                XCTAssertThrowsError(try store(changingConfig { $0[key] = value }), key)
            }
            XCTAssertNoThrow(try store(changingConfig { $0[key] = 1 }))
            XCTAssertNoThrow(try store(changingConfig { $0[key] = maximum }))
        }
    }

    func testVitsRejectsMultipleVoicesAndNonzeroOrInvalidSpeaker() throws {
        XCTAssertThrowsError(try store(changingTTS { $0["voices"] = [[String: Any]]() }))
        XCTAssertThrowsError(try store(changingTTS { model in
            var voices = model["voices"] as! [[String: Any]]
            voices.append(["id": "another", "sid": 0])
            model["voices"] = voices
        }))
        for value: Any in [true, false, -1, 1, 0.5, "0", NSNull()] {
            XCTAssertThrowsError(try store(changingTTS { model in
                var voices = model["voices"] as! [[String: Any]]
                voices[0]["sid"] = value
                model["voices"] = voices
            }))
        }
    }

    func testVitsScalesRejectInvalidNumbersAndKeepExplicitDefaults() throws {
        for (key, fallback): (String, Float) in [("noiseScale", 0.667), ("noiseScaleW", 0.8), ("lengthScale", 1.0)] {
            XCTAssertEqual(try speechVitsScale(nil, defaultValue: fallback), fallback)
            XCTAssertEqual(try speechVitsScale(NSNumber(value: fallback), defaultValue: fallback), fallback)
            for value: Any in [true, false, 0, -1, "0.8", NSNull(), 1e100, 1e-100] {
                XCTAssertThrowsError(try speechVitsScale(value, defaultValue: fallback), key)
                XCTAssertThrowsError(try store(changingConfig { $0[key] = value }), key)
            }
            // JSON 不允许非有限数，直接验证同一生产校验函数，避免只测到序列化失败。
            for value in [Double.nan, Double.infinity, -Double.infinity] {
                XCTAssertThrowsError(try speechVitsScale(value, defaultValue: fallback), key)
            }
        }
    }

    func testVitsDownloadAndCachedProofCannotHideCorruptFilesOrForeignTree() throws {
        let fixture = try fixture()
        let store = try store(fixture)
        let id = "vits-melo-tts-zh_en"
        _ = try store.startDownload(modelId: id)
        try waitFor(store, id: id)
        let directory = try store.modelDirectory(modelId: id)
        let model = try store.model(id: id)
        XCTAssertEqual(try snapshot(store, id: id)["downloadedBytes"] as? Int64, model.totalBytes)
        for file in model.files {
            let target = directory.appendingPathComponent(file.path)
            XCTAssertEqual(try Data(contentsOf: target), fixture.content[file.path])
            XCTAssertNoThrow(try store.modelDirectory(modelId: id))
            let corrupt = Data(repeating: 0, count: Int(file.bytes))
            try write(target, corrupt)
            XCTAssertThrowsError(try store.modelDirectory(modelId: id)) { error in
                XCTAssertEqual(error as? SpeechStorageError, .missing)
                XCTAssertFalse(error.localizedDescription.contains(directory.path))
            }
            XCTAssertEqual(try Data(contentsOf: target), corrupt)
            try write(target, fixture.content[file.path]!)
            XCTAssertNoThrow(try store.modelDirectory(modelId: id))
        }
        let foreign = directory.appendingPathComponent("dict/foreign.txt")
        try write(foreign, Data("keep".utf8))
        XCTAssertThrowsError(try store.modelDirectory(modelId: id))
        XCTAssertEqual(try Data(contentsOf: foreign), Data("keep".utf8))
    }

    func testVitsCachedDirectoryCancellationPreservesInstallationAndForeignData() throws {
        let fixture = try fixture()
        let root = temporary.appendingPathComponent("store")
        let store = try store(fixture, root: root)
        let id = "vits-melo-tts-zh_en"
        _ = try store.startDownload(modelId: id)
        try waitFor(store, id: id)
        let directory = try store.modelDirectory(modelId: id)
        let foreign = root.appendingPathComponent(".partial-foreign/keep")
        try write(foreign, Data("keep".utf8))
        var checks = 0
        XCTAssertThrowsError(try store.modelDirectory(modelId: id, checkCancellation: {
            checks += 1
            if checks == 2 { throw NSError(domain: "private-cancellation-detail", code: 1) }
        })) { error in XCTAssertEqual(error as? SpeechStorageError, .cancelled) }
        XCTAssertEqual(checks, 2)
        XCTAssertEqual(try Data(contentsOf: foreign), Data("keep".utf8))
        for file in try store.model(id: id).files {
            XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent(file.path)), fixture.content[file.path])
        }
        XCTAssertNoThrow(try store.modelDirectory(modelId: id))
    }

    func testVitsDictionarySymlinkCannotReuseSuccessfulProof() throws {
        let fixture = try fixture()
        let store = try store(fixture)
        let id = "vits-melo-tts-zh_en"
        _ = try store.startDownload(modelId: id)
        try waitFor(store, id: id)
        let directory = try store.modelDirectory(modelId: id)
        let dictionary = directory.appendingPathComponent("dict")
        let outside = temporary.appendingPathComponent("dictionary")
        try FileManager.default.moveItem(at: dictionary, to: outside)
        try FileManager.default.createSymbolicLink(at: dictionary, withDestinationURL: outside)
        XCTAssertThrowsError(try SpeechFiles.directory(dictionary))
        XCTAssertThrowsError(try store.modelDirectory(modelId: id))
        XCTAssertEqual(try Data(contentsOf: outside.appendingPathComponent("jieba.dict.utf8")), fixture.content["dict/jieba.dict.utf8"])
    }
}
