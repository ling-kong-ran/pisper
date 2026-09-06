import AVFoundation
import Foundation
import UIKit
import XCTest
@testable import pisper_mobile_device_plugin

final class SpeechAudioStateTests: XCTestCase {
  func testNativePermissionOnlyExemptsTemporaryInactiveNotification() {
    XCTAssertFalse(SpeechAudioInterruption.shouldForward(
      source: UIApplication.willResignActiveNotification.rawValue, microphonePermissionPending: true))
    XCTAssertTrue(SpeechAudioInterruption.shouldForward(
      source: UIApplication.willResignActiveNotification.rawValue, microphonePermissionPending: false))
    for source in [UIApplication.didEnterBackgroundNotification.rawValue,
      AVAudioSession.interruptionNotification.rawValue, AVAudioSession.routeChangeNotification.rawValue] {
      XCTAssertTrue(SpeechAudioInterruption.shouldForward(source: source, microphonePermissionPending: true))
    }
    XCTAssertTrue(SpeechAudioInterruption.shouldForward(source: nil, microphonePermissionPending: true))
  }

  private let first = "00000000-0000-4000-8000-000000000001"
  private let second = "00000000-0000-4000-8000-000000000002"
  private let third = "00000000-0000-4000-8000-000000000003"
  private let fourth = "00000000-0000-4000-8000-000000000004"

  private func assertCode(_ code: String, file: StaticString = #filePath, line: UInt = #line,
                          _ body: () throws -> Void) {
    XCTAssertThrowsError(try body(), file: file, line: line) {
      XCTAssertEqual(SpeechAudioError.code($0), code, file: file, line: line)
    }
  }

  func testCanonicalUUIDAndMalformedRequestIds() throws {
    XCTAssertEqual(try SpeechAudioRequests.validateId("ABCDEF00-0000-4000-8000-000000000001"),
      "abcdef00-0000-4000-8000-000000000001")
    for invalid in ["", "../audio.wav", first + " ", String(first.dropLast()), "{" + first + "}"] {
      assertCode("speech_invalid_request_id") { _ = try SpeechAudioRequests.validateId(invalid) }
    }
  }

  func testDuplicateSubmissionCannotReplaceRequestOwner() throws {
    let requests = SpeechAudioRequests()
    let original = try requests.begin(first)
    assertCode("speech_request_busy") { _ = try requests.begin(first) }
    requests.finish(original)
    assertCode("speech_request_busy") { _ = try requests.begin(first) }
    let cancelled = try requests.cancel(first)
    XCTAssertTrue(cancelled === original)
    XCTAssertTrue(original.cancelled)
  }

  func testCancelBeforeSubmissionAndRepeatedCancelAreIdempotent() throws {
    let requests = SpeechAudioRequests()
    let cancelled = try requests.cancel(first)
    XCTAssertTrue(try requests.cancel(first) === cancelled)
    assertCode("speech_cancelled") { _ = try requests.begin(first) }
    XCTAssertNoThrow(try requests.begin(second))
  }

  func testBoundedQueueIncludesRunningAndTwoWaitingRequests() throws {
    let requests = SpeechAudioRequests()
    let running = try requests.begin(first)
    _ = try requests.begin(second)
    _ = try requests.begin(third)
    assertCode("speech_engine_busy") { _ = try requests.begin(fourth) }
    _ = try requests.cancel(second)
    // 取消标记不能提前腾出仍被原生任务占用的队列槽位。
    assertCode("speech_engine_busy") { _ = try requests.begin(fourth) }
    requests.finish(running)
    XCTAssertNoThrow(try requests.begin(fourth))
  }

  func testForegroundResumeNeverRevivesCancelledRequests() throws {
    let requests = SpeechAudioRequests()
    let active = try requests.begin(first)
    requests.pause()
    XCTAssertTrue(active.cancelled)
    assertCode("speech_app_backgrounded") { _ = try requests.begin(second) }
    requests.resume()
    assertCode("speech_cancelled") { _ = try requests.begin(first) }
    XCTAssertNoThrow(try requests.begin(second))
  }

  func testRunningOwnershipSurvivesTombstoneExpiry() throws {
    var now: TimeInterval = 0
    let requests = SpeechAudioRequests(now: { now })
    let running = try requests.begin(first)
    now = 601
    XCTAssertTrue(try requests.cancel(first) === running)
    assertCode("speech_cancelled") { try running.check() }
    requests.finish(running)
    now = 1202
    let next = try requests.begin(first)
    XCTAssertFalse(next === running)
    requests.finish(running)
    assertCode("speech_request_busy") { _ = try requests.begin(first) }
  }

  func testAudioTokenIsBoundToOwnerAndConsumedOnce() throws {
    let requests = SpeechAudioRequests()
    let owner = try requests.begin(first)
    let tokens = SpeechAudioTokens(now: { 0 }, remove: { _ in })
    let clip = SpeechAudioClip(id: third, request: owner, file: URL(fileURLWithPath: "/tmp/clip.wav"),
      sampleRate: 24_000, durationMs: 1000, createdAt: 0)
    try tokens.insert(clip)
    assertCode("speech_audio_request_mismatch") { _ = try tokens.consume(audioId: third, requestId: second) }
    XCTAssertEqual(try tokens.consume(audioId: third, requestId: first).id, third)
    assertCode("speech_audio_unknown") { _ = try tokens.consume(audioId: third, requestId: first) }
  }

  func testCancelledAndExpiredTokensDeleteAudio() throws {
    var now: TimeInterval = 0
    var removed: [URL] = []
    let requests = SpeechAudioRequests()
    let tokens = SpeechAudioTokens(now: { now }, remove: { removed.append($0) })
    let owner = try requests.begin(first)
    let firstFile = URL(fileURLWithPath: "/tmp/first.wav")
    try tokens.insert(SpeechAudioClip(id: third, request: owner, file: firstFile,
      sampleRate: 24_000, durationMs: 1000, createdAt: 0))
    _ = try requests.cancel(first)
    tokens.prune()
    XCTAssertEqual(removed, [firstFile])
    assertCode("speech_audio_unknown") { _ = try tokens.consume(audioId: third, requestId: first) }
    let next = try requests.begin(second)
    let secondFile = URL(fileURLWithPath: "/tmp/second.wav")
    try tokens.insert(SpeechAudioClip(id: fourth, request: next, file: secondFile,
      sampleRate: 24_000, durationMs: 1000, createdAt: 0))
    now = 120
    assertCode("speech_audio_unknown") { _ = try tokens.consume(audioId: fourth, requestId: second) }
    XCTAssertEqual(removed, [firstFile, secondFile])
  }

  func testAudioCacheBoundAndLateCancelledInsertion() throws {
    let tokens = SpeechAudioTokens(now: { 0 }, remove: { _ in })
    let owner = SpeechAudioRequest(id: first, now: 0)
    for _ in 0..<4 {
      try tokens.insert(SpeechAudioClip(id: UUID().uuidString.lowercased(), request: owner,
        file: URL(fileURLWithPath: "/tmp/clip.wav"), sampleRate: 24_000, durationMs: 1000, createdAt: 0))
    }
    assertCode("speech_audio_queue_full") { try tokens.checkCapacity() }
    owner.cancel()
    tokens.prune()
    XCTAssertNoThrow(try tokens.checkCapacity())
    assertCode("speech_cancelled") {
      try tokens.insert(SpeechAudioClip(id: third, request: owner, file: URL(fileURLWithPath: "/tmp/late.wav"),
        sampleRate: 24_000, durationMs: 1000, createdAt: 0))
    }
  }

  func testInterruptionPublishesWithoutNativeRequestAndOnlyForItsSender() {
    let center = NotificationCenter()
    let sender = NSObject()
    let other = NSObject()
    var delivered = 0
    var wrongOwner = 0
    let observer = center.addObserver(forName: SpeechAudioInterruption.notification,
      object: sender, queue: nil) { _ in delivered += 1 }
    let otherObserver = center.addObserver(forName: SpeechAudioInterruption.notification,
      object: other, queue: nil) { _ in wrongOwner += 1 }
    defer { center.removeObserver(observer); center.removeObserver(otherObserver) }
    for reason in [AVAudioSession.RouteChangeReason.oldDeviceUnavailable, .newDeviceAvailable,
                   .noSuitableRouteForCategory, .override, .routeConfigurationChange] {
      SpeechAudioInterruption.publish(Notification(name: AVAudioSession.routeChangeNotification,
        userInfo: [AVAudioSessionRouteChangeReasonKey: reason.rawValue]), sender: sender, center: center)
    }
    SpeechAudioInterruption.publish(Notification(name: AVAudioSession.interruptionNotification,
      userInfo: [AVAudioSessionInterruptionTypeKey: AVAudioSession.InterruptionType.began.rawValue]),
      sender: sender, center: center)
    for name in [UIApplication.willResignActiveNotification, UIApplication.didEnterBackgroundNotification,
                 AVAudioSession.mediaServicesWereResetNotification] {
      SpeechAudioInterruption.publish(Notification(name: name), sender: sender, center: center)
    }
    XCTAssertEqual(delivered, 9)
    XCTAssertEqual(wrongOwner, 0)
  }

  func testCategoryChangeAndForegroundNeverPublishInterruption() {
    let center = NotificationCenter()
    let sender = NSObject()
    var delivered = 0
    let observer = center.addObserver(forName: SpeechAudioInterruption.notification,
      object: sender, queue: nil) { _ in delivered += 1 }
    defer { center.removeObserver(observer) }
    // 自身播放配置和 WebView 第一次开麦使用同一类别通知，不依赖容易失效的时间窗口。
    for _ in 0..<2 {
      SpeechAudioInterruption.publish(Notification(name: AVAudioSession.routeChangeNotification,
        userInfo: [AVAudioSessionRouteChangeReasonKey: AVAudioSession.RouteChangeReason.categoryChange.rawValue]),
        sender: sender, center: center)
    }
    SpeechAudioInterruption.publish(Notification(name: AVAudioSession.interruptionNotification,
      userInfo: [AVAudioSessionInterruptionTypeKey: AVAudioSession.InterruptionType.ended.rawValue]),
      sender: sender, center: center)
    SpeechAudioInterruption.publish(Notification(name: UIApplication.didBecomeActiveNotification),
      sender: sender, center: center)
    XCTAssertEqual(delivered, 0)
  }

  func testInterruptionBridgeRejectsExternalAndCredentialOrigins() {
    for url in ["http://127.0.0.1:41873/chat", "tauri://localhost/mobile-startup.html"] {
      XCTAssertTrue(SpeechAudioWebOrigin.trusted(URL(string: url)))
    }
    for url in ["https://example.com", "http://127.0.0.1.example.com", "http://localhost:41873",
                "http://user:password@127.0.0.1", "tauri://other", "file:///index.html"] {
      XCTAssertFalse(SpeechAudioWebOrigin.trusted(URL(string: url)))
    }
    XCTAssertFalse(SpeechAudioWebOrigin.trusted(nil))
  }

  private final class FakeSession: SpeechAudioSessionControlling {
    var category: AVAudioSession.Category = .ambient
    var activations = 0
    var deactivations = 0
    var failActivation = false
    var failDeactivation = false
    func configurePlayback() throws { category = .playback }
    func activate() throws {
      if failActivation { throw SpeechAudioError("speech_playback_failed") }
      activations += 1
    }
    func deactivate() throws {
      if failDeactivation { throw SpeechAudioError("speech_playback_failed") }
      deactivations += 1
    }
  }

  func testSessionLeaseReturnsOwnedActivationExactlyOnce() throws {
    let session = FakeSession()
    let lease = SpeechAudioSessionLease(session: session)
    try lease.prepare()
    XCTAssertTrue(lease.owned)
    XCTAssertEqual(session.category, .playback)
    try lease.release()
    try lease.release()
    XCTAssertFalse(lease.owned)
    XCTAssertEqual(session.activations, 1)
    XCTAssertEqual(session.deactivations, 1)
  }

  func testSessionLeaseDoesNotDeactivateFailedActivationOrBorrowedInput() throws {
    let failed = FakeSession()
    failed.failActivation = true
    let failedLease = SpeechAudioSessionLease(session: failed)
    XCTAssertThrowsError(try failedLease.prepare())
    try failedLease.release()
    XCTAssertFalse(failedLease.owned)
    XCTAssertEqual(failed.deactivations, 0)
    for category in [AVAudioSession.Category.playAndRecord, .record, .multiRoute] {
      let session = FakeSession()
      session.category = category
      let lease = SpeechAudioSessionLease(session: session)
      try lease.prepare()
      try lease.release()
      XCTAssertEqual(session.activations, 0)
      XCTAssertEqual(session.deactivations, 0)
      XCTAssertEqual(session.category, category)
    }
  }

  func testSessionLeaseNeverReclaimsWebViewCategoryTakeover() throws {
    let session = FakeSession()
    let lease = SpeechAudioSessionLease(session: session)
    try lease.prepare()
    session.category = .playAndRecord
    lease.observeCategory()
    session.category = .playback
    try lease.release()
    XCTAssertFalse(lease.owned)
    XCTAssertEqual(session.deactivations, 0)
  }

  func testSessionLeaseChecksWebViewCaptureBeforeActivationAndRelease() throws {
    let session = FakeSession()
    var capturing = true
    let lease = SpeechAudioSessionLease(session: session, microphoneInUse: { capturing })
    try lease.prepare()
    XCTAssertEqual(session.activations, 0)
    capturing = false
    try lease.prepare()
    capturing = true
    try lease.release()
    XCTAssertFalse(lease.owned)
    XCTAssertEqual(session.activations, 1)
    XCTAssertEqual(session.deactivations, 0)
  }

  func testSessionLeaseRetainsOwnershipWhenDeactivationFails() throws {
    let session = FakeSession()
    let lease = SpeechAudioSessionLease(session: session)
    try lease.prepare()
    session.failDeactivation = true
    XCTAssertThrowsError(try lease.release())
    XCTAssertTrue(lease.owned)
    session.failDeactivation = false
    try lease.release()
    XCTAssertFalse(lease.owned)
    XCTAssertEqual(session.deactivations, 1)
  }

  private func encoded(_ samples: [Float]) -> String {
    var data = Data()
    for sample in samples {
      let bits = sample.bitPattern
      for shift in stride(from: 0, to: 32, by: 8) { data.append(UInt8(truncatingIfNeeded: bits >> shift)) }
    }
    return data.base64EncodedString()
  }

  func testPCMRejectsOversizedMalformedNonfiniteAndOutOfRangeInput() throws {
    XCTAssertEqual(try SpeechPCM.decode(encoded([-1, 0, 0.5, 1]), check: {}), [-1, 0, 0.5, 1])
    for input in ["", "not base64", Data([0, 0, 0]).base64EncodedString(),
                  encoded([.nan]), encoded([.infinity]), encoded([1.01]), encoded([-1.01])] {
      assertCode("speech_invalid_pcm") { _ = try SpeechPCM.decode(input, check: {}) }
    }
    assertCode("speech_invalid_pcm") {
      try SpeechPCM.validateEncoded(String(repeating: "A", count: SpeechPCM.maxBase64Characters + 1))
    }
    assertCode("speech_cancelled") {
      _ = try SpeechPCM.decode(encoded([0]), check: { throw SpeechAudioError("speech_cancelled") })
    }
  }

  func testHotwordSyntaxMatchesAndroidLimits() throws {
    XCTAssertNoThrow(try SpeechPCM.validateHotwords("Pisper\nhello world"))
    for input in [" ", "a\n", "a/b", "a:b", "a#b", "a@b", "a\tb", "a\rb",
                  String(repeating: "a", count: 129), Array(repeating: "a", count: 129).joined(separator: "\n")] {
      assertCode("speech_invalid_hotwords") { try SpeechPCM.validateHotwords(input) }
    }
  }

  func testWaveIsPCM16AndRejectsNonfiniteSamplesWithoutSilencing() throws {
    let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let wave = root.appendingPathComponent("valid.wav")
    let samples = [Float](repeating: 0.5, count: 24)
    let result = try samples.withUnsafeBufferPointer {
      try SpeechWave.write(file: wave, samples: $0, sampleRate: 24_000, check: {})
    }
    XCTAssertEqual(result.durationMs, 1)
    let data = try Data(contentsOf: wave)
    XCTAssertEqual(data.count, 44 + 48)
    XCTAssertEqual(String(data: data.prefix(4), encoding: .ascii), "RIFF")
    XCTAssertEqual(Array(data[20..<24]), [1, 0, 1, 0])
    XCTAssertEqual(Array(data[34..<36]), [16, 0])
    XCTAssertEqual(Array(data[44..<46]), [0, 64])
    let saturatedFile = root.appendingPathComponent("saturated.wav")
    var saturated = [Float](repeating: 0, count: 24)
    saturated[0] = 1.1
    saturated[1] = -1.1
    _ = try saturated.withUnsafeBufferPointer {
      try SpeechWave.write(file: saturatedFile, samples: $0, sampleRate: 24_000, check: {})
    }
    XCTAssertEqual(Array(try Data(contentsOf: saturatedFile)[44..<48]), [255, 127, 1, 128])
    for invalid in [Float.nan, .infinity] {
      let invalidFile = root.appendingPathComponent(UUID().uuidString + ".part")
      let invalidSamples = [Float](repeating: invalid, count: 24)
      assertCode("speech_audio_invalid_sample") {
        _ = try invalidSamples.withUnsafeBufferPointer {
          try SpeechWave.write(file: invalidFile, samples: $0, sampleRate: 24_000, check: {})
        }
      }
    }
    assertCode("speech_audio_duration_exceeded") {
      _ = try samples.withUnsafeBufferPointer {
        try SpeechWave.write(file: root.appendingPathComponent("bad-rate.wav"), samples: $0, sampleRate: 96_000, check: {})
      }
    }
  }
}
