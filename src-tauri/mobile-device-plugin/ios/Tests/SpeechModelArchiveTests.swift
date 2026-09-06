import CryptoKit
import Darwin
import Foundation
import libarchive
import XCTest

// 测试 target 的模块导入由主线 Package 合同决定；独立 harness 直接编译生产源文件。
#if canImport(PisperSpeechArchiveHarness)
@testable import PisperSpeechArchiveHarness
#else
@testable import pisper_mobile_device_plugin
#endif

final class SpeechModelArchiveTests: XCTestCase {
    private var root: URL!
    private let payload = Data("verified model bytes".utf8)

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
            .appendingPathComponent("pisper-archive-tests-" + UUID().uuidString(), isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
    }

    override func tearDownWithError() throws {
        if let root { try FileManager.default.removeItem(at: root) }
    }

    func testStreamsWhitelistAndDiscardsOfficialSizedExtras() throws {
        let entries = [
            Fixture("model/", type: "5"),
            Fixture("model/model.onnx", body: payload),
            Fixture("model/test_wavs/", type: "5"),
            Fixture("model/README.md", count: 1000),
            Fixture("model/test_wavs/a.wav", count: 3_000_000),
            Fixture("model/test_wavs/b.wav", count: 3_366_294),
        ]
        let archive = try fixture(entries)
        let output = destination()
        try extract(archive, output)
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent("model.onnx")), payload)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: output.path), ["model.onnx"])
        let attributes = try FileManager.default.attributesOfItem(atPath: output.appendingPathComponent("model.onnx").path)
        XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o600)
    }

    func testCreatesOnlyRequiredNestedAncestors() throws {
        let archive = try fixture([Fixture("model/sub/model.onnx", body: payload), Fixture("model/unused/", type: "5")])
        let output = destination()
        try extract(archive, output, files: [spec("sub/model.onnx")])
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent("sub/model.onnx")), payload)
        XCTAssertFalse(FileManager.default.fileExists(atPath: output.appendingPathComponent("unused").path))
    }

    func testGNUAndUSTARHaveTheSameFileContract() throws {
        for gnu in [false, true] {
            let archive = try fixture([Fixture("model/model.onnx", body: payload)], gnu: gnu)
            try extract(archive, destination())
        }
    }

    func testRejectsTraversalAbsoluteDotBackslashAndControls() throws {
        for name in ["/model/model.onnx", "model/../outside", "model/./model.onnx", "model//model.onnx",
                     "model\\model.onnx", "C:/model.onnx", "model/bad\nname", "other/model.onnx"] {
            try rejects([Fixture(name, body: payload), Fixture("model/model.onnx", body: payload)])
        }
    }

    func testRejectsNULInCatalogAndPrefix() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        XCTAssertThrowsError(try extract(archive, destination(), files: [spec("model.onnx\0ignored")]))
        XCTAssertThrowsError(try SpeechModelArchive.extract(archive, destination(), "model\0ignored/", [spec()]) {})
    }

    func testRejectsLinksAndSpecialEntriesEvenForExtras() throws {
        for type in Array("12346") {
            try rejects([Fixture("model/extra", type: type, link: type == "1" || type == "2" ? "model/model.onnx" : ""),
                         Fixture("model/model.onnx", body: payload)])
        }
    }

    func testRejectsOldGNUSparseFile() throws {
        let archive = try fixture([Fixture("model/model.onnx", type: "S", body: Data([1]))], gnu: true)
        XCTAssertThrowsError(try extract(archive, destination()))
    }

    func testAllowsBoundedGNULongNameExtraWithoutWritingIt() throws {
        let name = "model/" + String(repeating: "a", count: 150)
        let archive = try fixture([
            Fixture("././@LongLink", type: "L", body: Data((name + "\0").utf8)),
            Fixture("model/placeholder", body: payload),
            Fixture("model/model.onnx", body: payload),
        ], gnu: true)
        let output = destination()
        try extract(archive, output)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: output.path), ["model.onnx"])
    }

    func testRejectsDuplicatesCaseAliasesAndFileDirectoryConflicts() throws {
        for entries in [
            [Fixture("model/model.onnx", body: payload), Fixture("model/model.onnx", body: payload)],
            [Fixture("model/EXTRA"), Fixture("model/extra")],
            [Fixture("model/a"), Fixture("model/a/b")],
            [Fixture("model/a/b"), Fixture("model/a")],
            [Fixture("model/", type: "5"), Fixture("model/", type: "5")],
        ] {
            try rejects(entries)
        }
    }

    func testRejectsLengthHashMissingAndDirectorySubstitution() throws {
        try rejects([Fixture("model/model.onnx", body: Data("wrong bytes".utf8))])
        try rejects([Fixture("model/model.onnx", body: Data(repeating: 0x78, count: payload.count))])
        try rejects([Fixture("model/extra")])
        try rejects([Fixture("model/model.onnx", type: "5")])
        try rejects([Fixture("model/dir/", type: "5", body: Data([1]))])
    }

    func testRejectsReservedMarkerEvenAsExtra() throws {
        try rejects([Fixture("model/.installation.json"), Fixture("model/model.onnx", body: payload)])
    }

    func testRejectsPAXOverridesAndOversizedMetadata() throws {
        for record in ["path=../outside", "size=19", "linkpath=outside", "path=model/model.onnx"] {
            let metadata = pax(record)
            for type in Array("xg") {
                try rejects([Fixture("PaxHeader", type: type, body: metadata), Fixture("model/model.onnx", body: payload)])
            }
        }
        let metadata = pax("comment=" + String(repeating: "a", count: 64 * 1024))
        try rejects([Fixture("PaxHeader", type: "x", body: metadata), Fixture("model/model.onnx", body: payload)])
    }

    func testRejectsMalformedPAXAndSparseMap() throws {
        try rejects([Fixture("PaxHeader", type: "x", body: Data("-1 path=outside\n".utf8)),
                     Fixture("model/model.onnx", body: payload)])
        let metadata = pax("GNU.sparse.map=0,1") + pax("GNU.sparse.size=19")
        try rejects([Fixture("PaxHeader", type: "x", body: metadata), Fixture("model/model.onnx", body: payload)])
    }

    func testRejectsExpandedByteAndEntryLimits() throws {
        try rejects([Fixture("model/model.onnx", body: payload), Fixture("model/extra", count: 32 * 1024 * 1024)])
        let entries = [Fixture("model/model.onnx", body: payload)] + (0..<4096).map { Fixture("model/extra-\($0)") }
        try rejects(entries)
    }

    func testRejectsTruncatedEntryAndMissingTarEndBlocks() throws {
        try rejects([Fixture("model/model.onnx", body: payload, declared: Int64(payload.count + 1024))], endBlocks: false)
        try rejects([Fixture("model/model.onnx", body: payload)], endBlocks: false)
    }

    func testRejectsInvalidHeaderChecksum() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)], damageHeader: true)
        XCTAssertThrowsError(try extract(archive, destination()))
    }

    func testRejectsTruncatedBzipAndFinalCRCError() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        let compressed = try Data(contentsOf: archive)
        for removed in [1, 5, 12, compressed.count / 2] {
            let damaged = root.appendingPathComponent(UUID().uuidString() + ".bz2")
            try Data(compressed.dropLast(removed)).write(to: damaged)
            XCTAssertThrowsError(try extract(damaged, destination()))
        }
        var corrupted = compressed
        corrupted[corrupted.count - 5] ^= 0x80
        let damaged = root.appendingPathComponent("crc.bz2")
        try corrupted.write(to: damaged)
        XCTAssertThrowsError(try extract(damaged, destination()))
    }

    func testRejectsUncompressedTar() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)], compressed: false)
        XCTAssertThrowsError(try extract(archive, destination()))
    }

    func testDoesNotOverwriteExistingInstallationOrEmptyStaging() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        let output = destination()
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: false)
        XCTAssertThrowsError(try extract(archive, output))
        let old = Data("old installation".utf8)
        try old.write(to: output.appendingPathComponent("model.onnx"))
        XCTAssertThrowsError(try extract(archive, output))
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent("model.onnx")), old)
    }

    func testRejectsSymlinkDestinationAndAncestor() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        let link = root.appendingPathComponent("linked")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: root)
        XCTAssertThrowsError(try extract(archive, link))
        XCTAssertThrowsError(try extract(archive, link.appendingPathComponent("staging")))
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("staging").path))
    }

    func testRejectsArchiveSymlinkHardlinkAndFIFO() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        let link = root.appendingPathComponent("symlink.bz2")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: archive)
        XCTAssertThrowsError(try extract(link, destination()))
        let hardlink = root.appendingPathComponent("hardlink.bz2")
        XCTAssertEqual(Darwin.link(archive.path, hardlink.path), 0)
        XCTAssertThrowsError(try extract(hardlink, destination()))
        let fifo = root.appendingPathComponent("fifo.bz2")
        XCTAssertEqual(mkfifo(fifo.path, 0o600), 0)
        XCTAssertThrowsError(try extract(fifo, destination()))
    }

    func testRejectsNonPrivateStagingParent() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        let shared = root.appendingPathComponent("shared", isDirectory: true)
        try FileManager.default.createDirectory(at: shared, withIntermediateDirectories: false)
        XCTAssertEqual(chmod(shared.path, 0o755), 0)
        XCTAssertThrowsError(try extract(archive, shared.appendingPathComponent("staging")))
    }

    func testCancellationDuringReadRemovesPartialOutputAndClosesDescriptors() throws {
        enum Cancelled: Error { case requested }
        let archive = try fixture([Fixture("model/model.onnx", count: 1024 * 1024)])
        let file = SpeechDownloadFile(path: "model.onnx", bytes: 1024 * 1024, sha256: String(repeating: "0", count: 64), urls: [])
        let before = descriptorCount()
        for _ in 0..<8 {
            let output = destination()
            var checks = 0
            XCTAssertThrowsError(try SpeechModelArchive.extract(archive, output, "model/", [file]) {
                checks += 1
                if checks == 8 { throw Cancelled.requested }
            }) { error in XCTAssertTrue(error is Cancelled) }
            XCTAssertFalse(FileManager.default.fileExists(atPath: output.path))
        }
        XCTAssertEqual(descriptorCount(), before)
    }

    func testRepeatedIntegrityFailureDoesNotLeakDescriptors() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: Data(repeating: 0, count: payload.count))])
        let before = descriptorCount()
        for _ in 0..<8 {
            let output = destination()
            XCTAssertThrowsError(try extract(archive, output))
            XCTAssertFalse(FileManager.default.fileExists(atPath: output.path))
        }
        XCTAssertEqual(descriptorCount(), before)
    }

    func testRawGuardRejectsOversizedMetadataBeforeReceivingBody() throws {
        for type in Array("xgL") {
            for declared in [Int64(0), 65_537] {
                var guardrail = SpeechModelArchive.RawTarGuard(maximum: 64 * 1024 * 1024, maximumEntry: 32 * 1024 * 1024)
                let header = tarHeader(Fixture("Extended", type: type, declared: declared), gnu: false)
                XCTAssertThrowsError(try feed(header, to: &guardrail))
                XCTAssertEqual(guardrail.bufferedMetadataBytes, 0)
            }
        }
    }

    func testRawGuardCountsEveryPhysicalExtensionHeader() throws {
        let metadata = pax("comment=x")
        let header = tarHeader(Fixture("Extended", type: "g", body: metadata), gnu: false)
        let padded = metadata + Data(repeating: 0, count: 512 - metadata.count)
        var guardrail = SpeechModelArchive.RawTarGuard(maximum: 32 * 1024 * 1024, maximumEntry: 32 * 1024 * 1024)
        for _ in 0..<4096 {
            try feed(header, to: &guardrail)
            try feed(padded, to: &guardrail)
        }
        XCTAssertEqual(guardrail.physicalEntries, 4096)
        XCTAssertThrowsError(try feed(header, to: &guardrail)) { error in
            XCTAssertEqual(error as? SpeechModelArchiveError, .entryLimit)
        }
    }

    func testRawGuardRejectsPAXOverridesBeforeMixedGNUCanHideThem() throws {
        for type in Array("xg") {
            for record in ["size=19", "linkpath=outside", "GNU.sparse.map=0,1", "SCHILY.realsize=19", "SUN.holesdata=0 1"] {
                try rejects([
                    Fixture("Extended", type: type, body: pax(record)),
                    Fixture("././@LongLink", type: "L", body: Data("model/model.onnx\0".utf8)),
                    Fixture("model/placeholder", body: payload),
                ])
            }
        }
        try rejects([Fixture("Extended", type: "g", body: pax("path=model/model.onnx")),
                     Fixture("model/model.onnx", body: payload)])
    }

    func testRawGuardValidatesAllPAXRecordBoundaries() throws {
        for metadata in [
            Data("0 a=b\n".utf8), Data("-1 a=b\n".utf8), Data("01 a=b\n".utf8),
            Data("999999999 a=b\n".utf8), Data("100 a=b\n".utf8), Data("7 a=bX".utf8),
            Data("7 abc\n".utf8), pax("comment=x") + Data("0 a=b\n".utf8),
            pax("comment=x") + pax("size=19"), pax("comment=x\0hidden"),
        ] {
            let bytes = rawTar([Fixture("Extended", type: "x", body: metadata), Fixture("model/model.onnx", body: payload)])
            XCTAssertThrowsError(try checkFraming(bytes, chunk: 1))
        }
    }

    func testRawGuardAllowsBoundedBenignPAXAndLocalPathFraming() throws {
        for type in Array("xg") {
            let bytes = rawTar([Fixture("Extended", type: type, body: pax("comment=bounded metadata")),
                                Fixture("model/model.onnx", body: payload)])
            try checkFraming(bytes, chunk: 511)
        }
        let bytes = rawTar([Fixture("Extended", type: "x", body: pax("path=model/model.onnx")),
                            Fixture("model/placeholder", body: payload)])
        try checkFraming(bytes, chunk: 513)
        let maximumMetadata = pax("comment=" + String(repeating: "a", count: 65_521))
        XCTAssertEqual(maximumMetadata.count, 65_536)
        try checkFraming(rawTar([Fixture("Extended", type: "x", body: maximumMetadata),
                                 Fixture("model/model.onnx", body: payload)]), chunk: 65_536)
    }

    func testRejectsRawNULHiddenSuffixInNamesAndGNULongName() throws {
        try rejects([Fixture("model/model.onnx\0hidden", body: payload)])
        try rejects([Fixture("././@LongLink", type: "L", body: Data("model/model.onnx\0hidden\0".utf8)),
                     Fixture("model/placeholder", body: payload)])
        try rejects([Fixture("././@LongLink", type: "L", body: Data("model/model.onnx".utf8)),
                     Fixture("model/placeholder", body: payload)])
        for field in [157, 265, 297, 345] {
            var header = tarHeader(Fixture("model/model.onnx", body: payload), gnu: false)
            header[field] = 0
            header[field + 1] = 65
            repairChecksum(&header)
            var guardrail = SpeechModelArchive.RawTarGuard(maximum: 32 * 1024 * 1024, maximumEntry: 32 * 1024 * 1024)
            XCTAssertThrowsError(try feed(header, to: &guardrail))
        }
    }

    func testRawGuardRejectsInvalidOctalNegativeBinaryAndOverflow() throws {
        let malformed: [[UInt8]] = [
            Array("00000000019\0".utf8), Array("-0000000019\0".utf8),
            Array("000\00000019\0".utf8), [UInt8](repeating: 0xff, count: 12),
            [0x80, 0x01] + [UInt8](repeating: 0, count: 10),
        ]
        for field in malformed {
            XCTAssertEqual(field.count, 12)
            var header = tarHeader(Fixture("model/model.onnx", body: payload), gnu: false)
            header.replaceSubrange(124..<136, with: field)
            repairChecksum(&header)
            var guardrail = SpeechModelArchive.RawTarGuard(maximum: 32 * 1024 * 1024, maximumEntry: 32 * 1024 * 1024)
            XCTAssertThrowsError(try feed(header, to: &guardrail))
        }
    }

    func testRawGuardAllowsPositiveGNUBase256Length() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)], gnu: true, headerMutation: { header in
            header.replaceSubrange(124..<136, with: [0x80] + [UInt8](repeating: 0, count: 10) + [UInt8(self.payload.count)])
        })
        let output = destination()
        try extract(archive, output)
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent("model.onnx")), payload)
    }

    func testRawGuardRejectsEmptySparseAndEveryUnsupportedPhysicalType() throws {
        for type in Array("123467SDKVMN") {
            var guardrail = SpeechModelArchive.RawTarGuard(maximum: 32 * 1024 * 1024, maximumEntry: 32 * 1024 * 1024)
            XCTAssertThrowsError(try feed(tarHeader(Fixture("model/extra", type: type), gnu: true), to: &guardrail))
        }
    }

    func testRawGuardIsIndependentOfChunkBoundaries() throws {
        let bytes = rawTar([
            Fixture("././@LongLink", type: "L", body: Data(("model/" + String(repeating: "a", count: 150) + "\0").utf8)),
            Fixture("model/placeholder", body: payload), Fixture("model/model.onnx", body: payload),
        ])
        for chunk in [1, 7, 511, 512, 513, 65_536] { try checkFraming(bytes, chunk: chunk) }
    }

    func testRawGuardAccountsHeadersMetadataPaddingAndTrailerInTotalBudget() throws {
        let bytes = rawTar([Fixture("Extended", type: "x", body: pax("comment=x")),
                            Fixture("model/model.onnx", body: payload)])
        try checkFraming(bytes, maximum: Int64(bytes.count))
        XCTAssertThrowsError(try checkFraming(bytes, maximum: Int64(bytes.count - 1)))
    }

    func testRawGuardRejectsPartialFramesPaddingAndTrailingArchives() throws {
        let bytes = rawTar([Fixture("model/model.onnx", body: payload)])
        for missing in [1, 512, 1024, 1025] {
            XCTAssertThrowsError(try checkFraming(Data(bytes.dropLast(missing))))
        }
        XCTAssertThrowsError(try checkFraming(bytes + Data([1])))
        XCTAssertThrowsError(try checkFraming(bytes + tarHeader(Fixture("model/extra"), gnu: false)))
        var badPadding = bytes
        badPadding[512 + payload.count] = 1
        XCTAssertThrowsError(try checkFraming(badPadding))
        XCTAssertThrowsError(try checkFraming(rawTar([Fixture("Extended", type: "x", body: pax("comment=x"))])))
    }

    func testNoWhitelistOutputBeforeTheEntireRawArchivePassesPreflight() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload),
                                   Fixture("Extended", type: "x", declared: 65_537)])
        let output = destination()
        var sawOutput = false
        XCTAssertThrowsError(try SpeechModelArchive.extract(archive, output, "model/", [spec()]) {
            sawOutput = sawOutput || FileManager.default.fileExists(atPath: output.appendingPathComponent("model.onnx").path)
        })
        XCTAssertFalse(sawOutput)
        XCTAssertFalse(FileManager.default.fileExists(atPath: output.path))
    }

    func testCancellationDuringSecondPassClosesUnlinkedTarAndOutputDescriptors() throws {
        enum Cancelled: Error { case requested }
        let archive = try fixture([Fixture("model/model.onnx", count: 1024 * 1024)])
        let file = SpeechDownloadFile(path: "model.onnx", bytes: 1024 * 1024, sha256: String(repeating: "0", count: 64), urls: [])
        let before = descriptorCount()
        let names = try FileManager.default.contentsOfDirectory(atPath: root.path).sorted()
        let output = destination()
        var sawOutput = false
        XCTAssertThrowsError(try SpeechModelArchive.extract(archive, output, "model/", [file]) {
            if FileManager.default.fileExists(atPath: output.appendingPathComponent("model.onnx").path) {
                sawOutput = true
                throw Cancelled.requested
            }
        }) { error in XCTAssertTrue(error is Cancelled) }
        XCTAssertTrue(sawOutput)
        XCTAssertEqual(descriptorCount(), before)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: root.path).sorted(), names)
    }

    func testRawTemporaryFileHasNoVisibleNameDuringEitherPass() throws {
        let archive = try fixture([Fixture("model/model.onnx", body: payload)])
        let output = destination()
        try SpeechModelArchive.extract(archive, output, "model/", [spec()]) {
            if FileManager.default.fileExists(atPath: output.path) {
                let names = try FileManager.default.contentsOfDirectory(atPath: output.path)
                XCTAssertFalse(names.contains(where: { $0.hasPrefix(".speech-preflight-") }))
            }
        }
    }

    private func feed(_ data: Data, to guardrail: inout SpeechModelArchive.RawTarGuard) throws {
        try data.withUnsafeBytes { try guardrail.consume($0) }
    }

    private func checkFraming(_ data: Data, chunk: Int = 65_536, maximum: Int64 = 32 * 1024 * 1024) throws {
        var guardrail = SpeechModelArchive.RawTarGuard(maximum: maximum, maximumEntry: 32 * 1024 * 1024)
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let end = min(bytes.count, offset + chunk)
                try guardrail.consume(UnsafeRawBufferPointer(rebasing: bytes[offset..<end]))
                offset = end
            }
        }
        try guardrail.finish()
    }

    private func rawTar(_ entries: [Fixture]) -> Data {
        var bytes = Data()
        for entry in entries {
            bytes += tarHeader(entry, gnu: false)
            bytes += entry.body
            bytes += Data(repeating: 0, count: (512 - entry.body.count % 512) % 512)
        }
        return bytes + Data(repeating: 0, count: 1024)
    }

    private func repairChecksum(_ header: inout Data) {
        header.replaceSubrange(148..<156, with: Array("        ".utf8))
        let checksum = header.reduce(Int64(0)) { $0 + Int64($1) }
        let value = String(checksum, radix: 8)
        header.replaceSubrange(148..<156, with: Array((String(repeating: "0", count: 6 - value.count) + value + "\0 ").utf8))
    }

    private func descriptorCount() -> Int {
        (0..<getdtablesize()).reduce(0) { $0 + (fcntl($1, F_GETFD) >= 0 ? 1 : 0) }
    }

    private func destination() -> URL { root.appendingPathComponent("staging-" + UUID().uuidString()) }

    private func spec(_ path: String = "model.onnx") -> SpeechDownloadFile {
        SpeechDownloadFile(path: path, bytes: Int64(payload.count),
                           sha256: SHA256.hash(data: payload).map { String(format: "%02x", $0) }.joined(), urls: [])
    }

    private func extract(_ archive: URL, _ output: URL, files: [SpeechDownloadFile]? = nil) throws {
        try SpeechModelArchive.extract(archive, output, "model/", files ?? [spec()]) {}
    }

    private func rejects(_ entries: [Fixture], endBlocks: Bool = true) throws {
        let archive = try fixture(entries, endBlocks: endBlocks)
        let output = destination()
        XCTAssertThrowsError(try extract(archive, output))
        XCTAssertFalse(FileManager.default.fileExists(atPath: output.path))
    }

    private struct Fixture {
        let name: String
        let type: Character
        let link: String
        let body: Data
        let count: Int
        let declared: Int64?
        init(_ name: String, type: Character = "0", link: String = "", body: Data = Data(), count: Int? = nil, declared: Int64? = nil) {
            self.name = name
            self.type = type
            self.link = link
            self.body = body
            self.count = count ?? body.count
            self.declared = declared
        }
    }

    // 只在测试中构造畸形 TAR 字节，避免 writer 自动修复测试所需的坏长度、PAX 和校验和。
    // bzip2 编码仍由 libarchive 完成；大体积 extra 使用固定块，不整包缓冲。
    private func fixture(_ entries: [Fixture], endBlocks: Bool = true, gnu: Bool = false,
                         compressed: Bool = true, damageHeader: Bool = false,
                         headerMutation: ((inout Data) -> Void)? = nil) throws -> URL {
        let url = root.appendingPathComponent(UUID().uuidString() + ".tar.bz2")
        let writer = try XCTUnwrap(archive_write_new())
        defer { archive_write_free(writer) }
        try ok(archive_write_set_format_raw(writer))
        if compressed { try ok(archive_write_add_filter_bzip2(writer)) }
        try ok(archive_write_set_bytes_per_block(writer, 0))
        try ok(archive_write_set_bytes_in_last_block(writer, 1))
        try ok(archive_write_open_filename(writer, url.path))
        let entry = try XCTUnwrap(archive_entry_new())
        defer { archive_entry_free(entry) }
        archive_entry_set_pathname(entry, "fixture.tar")
        archive_entry_set_filetype(entry, mode_t(S_IFREG))
        try ok(archive_write_header(writer, entry))
        let zeros = Data(repeating: 0, count: 64 * 1024)
        for item in entries {
            var header = tarHeader(item, gnu: gnu)
            if let headerMutation {
                headerMutation(&header)
                repairChecksum(&header)
            }
            if damageHeader { header[0] ^= 1 }
            try write(writer, header)
            if item.body.isEmpty {
                var remaining = item.count
                while remaining > 0 {
                    let count = min(remaining, zeros.count)
                    try write(writer, zeros.prefix(count))
                    remaining -= count
                }
            } else {
                try write(writer, item.body)
            }
            try write(writer, zeros.prefix((512 - item.count % 512) % 512))
        }
        if endBlocks { try write(writer, zeros.prefix(1024)) }
        try ok(archive_write_close(writer))
        return url
    }

    private func write(_ writer: OpaquePointer, _ data: Data) throws {
        if data.isEmpty { return }
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let count = archive_write_data(writer, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                guard count > 0 else { throw SpeechModelArchiveError.storage }
                offset += count
            }
        }
    }

    private func ok(_ status: Int32) throws {
        guard status == ARCHIVE_OK else { throw SpeechModelArchiveError.integrity }
    }

    private func tarHeader(_ item: Fixture, gnu: Bool) -> Data {
        var bytes = [UInt8](repeating: 0, count: 512)
        func text(_ value: String, _ offset: Int, _ length: Int) {
            for (index, byte) in value.utf8.prefix(length).enumerated() { bytes[offset + index] = byte }
        }
        func octal(_ value: Int64, _ offset: Int, _ length: Int) {
            let value = String(value, radix: 8)
            text(String(repeating: "0", count: max(0, length - value.count - 1)) + value + "\0", offset, length)
        }
        text(item.name, 0, 100)
        octal(0o777, 100, 8)
        octal(0, 108, 8)
        octal(0, 116, 8)
        octal(item.declared ?? Int64(item.count), 124, 12)
        octal(0, 136, 12)
        text("        ", 148, 8)
        text(String(item.type), 156, 1)
        text(item.link, 157, 100)
        text(gnu ? "ustar  \0" : "ustar\u{0}00", 257, 8)
        if item.type == "S" {
            octal(0, 386, 12)
            octal(1, 398, 12)
            octal(19, 483, 12)
        }
        let checksum = bytes.reduce(Int64(0)) { $0 + Int64($1) }
        let value = String(checksum, radix: 8)
        text(String(repeating: "0", count: 6 - value.count) + value + "\0 ", 148, 8)
        return Data(bytes)
    }

    private func pax(_ record: String) -> Data {
        var size = record.utf8.count + 3
        while true {
            let bytes = Data("\(size) \(record)\n".utf8)
            if bytes.count == size { return bytes }
            size = bytes.count
        }
    }
}
