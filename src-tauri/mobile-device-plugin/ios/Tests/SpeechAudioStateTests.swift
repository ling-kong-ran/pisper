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

  func testEngineCacheReusesConfigurationAndRejectsStaleIdleCleanup() throws {
    var now: TimeInterval = 0
    var created = 0
    var released: [Int] = []
    var timers: [() -> Void] = []
    let cache = SpeechEngineCache<String, Int>(
      create: { _ in created += 1; return created },
      release: { released.append($0) },
      schedule: { delay, operation in
        XCTAssertEqual(delay, 30)
        timers.append(operation)
        // 保留旧回调以覆盖定时器已入队、取消来不及移除的情况。
        return {}
      }, now: { now })
    XCTAssertEqual(try cache.use("a") { $0 }, 1)
    let stale = try XCTUnwrap(timers.last)
    now = 10
    XCTAssertEqual(try cache.use("a") { $0 }, 1)
    stale()
    XCTAssertTrue(released.isEmpty)
    let beforeASR = try XCTUnwrap(timers.last)
    now += 60
    cache.touch()
    beforeASR()
    XCTAssertEqual(try cache.use("a") { $0 }, 1)
    XCTAssertTrue(released.isEmpty)
    XCTAssertEqual(try cache.use("b") { $0 }, 2)
    XCTAssertEqual(released, [1])
    try XCTUnwrap(timers.last)()
    XCTAssertEqual(released, [1, 2])
    XCTAssertEqual(try cache.use("b") { $0 }, 3)
    now += 30
    XCTAssertEqual(try cache.use("b") { $0 }, 4)
    XCTAssertEqual(released, [1, 2, 3])
    cache.invalidate()
    cache.invalidate()
    timers.forEach { $0() }
    XCTAssertEqual(created, 4)
    XCTAssertEqual(released, [1, 2, 3, 4])
  }

  func testEngineCacheRecoversFromLoadAndGenerationErrors() throws {
    var failLoad = true
    var created = 0
    var released: [Int] = []
    let cache = SpeechEngineCache<String, Int>(
      create: { _ in
        if failLoad { throw SpeechAudioError("speech_model_load_failed") }
        created += 1
        return created
      }, release: { released.append($0) }, schedule: { _, _ in {} })
    assertCode("speech_model_load_failed") { _ = try cache.use("a") { $0 } }
    failLoad = false
    XCTAssertEqual(try cache.use("a") { $0 }, 1)
    assertCode("speech_synthesis_failed") {
      try cache.use("a") { _ in throw SpeechAudioError("speech_synthesis_failed") }
    }
    XCTAssertEqual(released, [1])
    XCTAssertEqual(try cache.use("a") { $0 }, 2)
    cache.invalidate()
    XCTAssertEqual(released, [1, 2])
  }

  func testLifecycleCleanupWaitsForGenerationAndASRPreservesTTS() throws {
    let queue = DispatchQueue(label: "test.speech.engine")
    var active = false
    var created = 0
    var released = 0
    let cache = SpeechEngineCache<String, Int>(
      create: { _ in created += 1; return created },
      release: { _ in XCTAssertFalse(active); released += 1 }, schedule: { _, _ in {} })
    try queue.sync { XCTAssertEqual(try cache.use("a") { $0 }, 1) }
    queue.sync { cache.touch() }
    try queue.sync {
      try cache.use("a") { value in
        active = true
        queue.async { cache.invalidate() }
        XCTAssertEqual(value, 1)
        XCTAssertEqual(released, 0)
        active = false
      }
      XCTAssertEqual(released, 0)
    }
    queue.sync { XCTAssertEqual(released, 1); cache.invalidate() }
    try queue.sync { XCTAssertEqual(try cache.use("a") { $0 }, 2); cache.invalidate() }
    XCTAssertEqual(created, 2)
    XCTAssertEqual(released, 2)
  }

  private final class EngineEvents: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String] = []
    func append(_ value: String) { lock.lock(); values.append(value); lock.unlock() }
    var snapshot: [String] { lock.lock(); defer { lock.unlock() }; return values }
  }

  func testKindQueuesLoadAndInferConcurrentlyWhileEachKindRemainsFIFO() throws {
    let asr = DispatchQueue(label: "test.speech.asr")
    let tts = DispatchQueue(label: "test.speech.tts")
    let joined = DispatchQueue(label: "test.speech.join")
    let constructorsEntered = DispatchSemaphore(value: 0)
    let allowConstructors = DispatchSemaphore(value: 0)
    let inferenceEntered = DispatchSemaphore(value: 0)
    let allowInference = DispatchSemaphore(value: 0)
    let events = EngineEvents()
    let ready = expectation(description: "both native engines ready")
    func cache(_ name: String, _ queue: DispatchQueue) -> SpeechEngineCache<String, Int> {
      SpeechEngineCache(create: { _ in
        dispatchPrecondition(condition: .onQueue(queue))
        events.append(name + ":create")
        constructorsEntered.signal()
        guard allowConstructors.wait(timeout: .now() + 5) == .success else {
          throw SpeechAudioError("speech_model_load_failed")
        }
        return 1
      }, release: { _ in
        dispatchPrecondition(condition: .onQueue(queue))
        events.append(name + ":release")
      }, sessionState: { (true, nil) }, schedule: { _, _ in XCTFail("活跃时不能启动 timer"); return {} })
    }
    let asrCache = cache("asr", asr)
    let ttsCache = cache("tts", tts)
    SpeechEnginePreparation.run([
      SpeechEngineTask(queue: asr) { try asrCache.prepare("model", check: {}) },
      SpeechEngineTask(queue: tts) { try ttsCache.prepare("model", check: {}) },
    ], completionQueue: joined, check: {}) { result in
      XCTAssertNoThrow(try result.get())
      ready.fulfill()
    }
    // 两个 constructor 都必须进入才能放行，不用睡眠推测执行是否重叠。
    XCTAssertEqual(constructorsEntered.wait(timeout: .now() + 5), .success)
    XCTAssertEqual(constructorsEntered.wait(timeout: .now() + 5), .success)
    let finished = DispatchGroup()
    for (name, queue, engine) in [("asr", asr, asrCache), ("tts", tts, ttsCache)] {
      for index in 1...2 {
        finished.enter()
        queue.async {
          defer { finished.leave() }
          XCTAssertNoThrow(try engine.use("model") { _ in
            events.append("\(name):infer\(index):start")
            if index == 1 {
              inferenceEntered.signal()
              XCTAssertEqual(allowInference.wait(timeout: .now() + 5), .success)
            }
            events.append("\(name):infer\(index):end")
          })
        }
      }
    }
    XCTAssertEqual(events.snapshot.count, 2)
    allowConstructors.signal(); allowConstructors.signal()
    XCTAssertEqual(inferenceEntered.wait(timeout: .now() + 5), .success)
    XCTAssertEqual(inferenceEntered.wait(timeout: .now() + 5), .success)
    XCTAssertFalse(events.snapshot.contains(where: { $0.contains("infer2") }))
    allowInference.signal(); allowInference.signal()
    XCTAssertEqual(finished.wait(timeout: .now() + 5), .success)
    wait(for: [ready], timeout: 5)
    for name in ["asr", "tts"] {
      XCTAssertEqual(events.snapshot.filter { $0.hasPrefix(name) }, [
        name + ":create", name + ":infer1:start", name + ":infer1:end",
        name + ":infer2:start", name + ":infer2:end",
      ])
    }
    asr.sync { asrCache.invalidate() }
    tts.sync { ttsCache.invalidate() }
  }

  func testParallelPrepareWaitsForLateSuccessAndReleasesItOnItsOwnQueueAfterPeerFailure() throws {
    let asr = DispatchQueue(label: "test.speech.asr.failure")
    let tts = DispatchQueue(label: "test.speech.tts.late")
    let joined = DispatchQueue(label: "test.speech.join.failure")
    let entered = DispatchSemaphore(value: 0)
    let allowFailure = DispatchSemaphore(value: 0)
    let allowLate = DispatchSemaphore(value: 0)
    let failed = DispatchSemaphore(value: 0)
    let events = EngineEvents()
    let complete = expectation(description: "failure observes late handle cleanup")
    let asrCache = SpeechEngineCache<String, Int>(create: { _ in
      entered.signal()
      guard allowFailure.wait(timeout: .now() + 5) == .success else {
        throw SpeechAudioError("speech_cancelled")
      }
      throw SpeechAudioError("speech_model_load_failed")
    }, release: { _ in XCTFail("失败的构造不能交出句柄") }, schedule: { _, _ in {} })
    let ttsCache = SpeechEngineCache<String, Int>(create: { _ in
      entered.signal()
      guard allowLate.wait(timeout: .now() + 5) == .success else {
        throw SpeechAudioError("speech_cancelled")
      }
      events.append("tts:created")
      return 1
    }, release: { _ in
      dispatchPrecondition(condition: .onQueue(tts))
      events.append("tts:released")
    }, sessionState: { (true, nil) }, schedule: { _, _ in {} })
    SpeechEnginePreparation.run([
      SpeechEngineTask(queue: asr) {
        defer { failed.signal() }
        return try asrCache.prepare("model", check: {})
      },
      SpeechEngineTask(queue: tts) { try ttsCache.prepare("model", check: {}) },
    ], completionQueue: joined, check: {}) { result in
      if case .failure(let error) = result { XCTAssertEqual(SpeechAudioError.code(error), "speech_model_load_failed") }
      else { XCTFail("不能发布 ready") }
      events.append("complete")
      complete.fulfill()
    }
    XCTAssertEqual(entered.wait(timeout: .now() + 5), .success)
    XCTAssertEqual(entered.wait(timeout: .now() + 5), .success)
    allowFailure.signal()
    XCTAssertEqual(failed.wait(timeout: .now() + 5), .success)
    XCTAssertTrue(events.snapshot.isEmpty)
    allowLate.signal()
    wait(for: [complete], timeout: 5)
    XCTAssertEqual(events.snapshot, ["tts:created", "tts:released", "complete"])
    asr.sync { asrCache.invalidate() }
    tts.sync { ttsCache.invalidate() }
    XCTAssertEqual(events.snapshot.filter { $0 == "tts:released" }.count, 1)
  }

  func testParallelPrepareCancellationAndBackgroundCleanLateHandlesWithoutRevivingOldUUID() throws {
    for background in [false, true] {
      let asr = DispatchQueue(label: "test.speech.asr.cancel")
      let tts = DispatchQueue(label: "test.speech.tts.cancel")
      let joined = DispatchQueue(label: "test.speech.join.cancel")
      let entered = DispatchSemaphore(value: 0)
      let allowFinish = DispatchSemaphore(value: 0)
      let sessions = SpeechAudioSessions()
      let request = try sessions.begin(first, kinds: ["asr", "tts"], hotwords: "")
      let events = EngineEvents()
      let complete = expectation(description: "cancelled native handles observed")
      func cache(_ name: String, _ queue: DispatchQueue) -> SpeechEngineCache<String, Int> {
        SpeechEngineCache(create: { _ in
          events.append(name + ":create")
          entered.signal()
          guard allowFinish.wait(timeout: .now() + 5) == .success else {
            throw SpeechAudioError("speech_model_load_failed")
          }
          return 1
        }, release: { _ in
          dispatchPrecondition(condition: .onQueue(queue))
          events.append(name + ":release")
        }, sessionState: { sessions.state }, schedule: { _, _ in {} })
      }
      let asrCache = cache("asr", asr)
      let ttsCache = cache("tts", tts)
      SpeechEnginePreparation.run([
        SpeechEngineTask(queue: asr) { try asrCache.prepare("model", check: request.check) },
        SpeechEngineTask(queue: tts) { try ttsCache.prepare("model", check: request.check) },
      ], completionQueue: joined, check: request.check) { result in
        if case .failure(let error) = result { XCTAssertEqual(SpeechAudioError.code(error), "speech_cancelled") }
        else { XCTFail("取消后不能发布 ready") }
        sessions.finish(request)
        complete.fulfill()
      }
      XCTAssertEqual(entered.wait(timeout: .now() + 5), .success)
      XCTAssertEqual(entered.wait(timeout: .now() + 5), .success)
      if background {
        sessions.clear()
        asr.async { asrCache.invalidate() }
        tts.async { ttsCache.invalidate() }
      } else { try sessions.release(first) }
      let next = try sessions.begin(second, kinds: ["asr"], hotwords: "")
      XCTAssertTrue(request.cancelled)
      XCTAssertFalse(events.snapshot.contains(where: { $0.hasSuffix(":release") }))
      allowFinish.signal(); allowFinish.signal()
      wait(for: [complete], timeout: 5)
      asr.sync { asrCache.invalidate() }
      tts.sync { ttsCache.invalidate() }
      XCTAssertEqual(events.snapshot.filter { $0.hasPrefix("asr") }, ["asr:create", "asr:release"])
      XCTAssertEqual(events.snapshot.filter { $0.hasPrefix("tts") }, ["tts:create", "tts:release"])
      sessions.fail(request)
      XCTAssertFalse(next.cancelled)
      XCTAssertTrue(sessions.state.pinned)
    }
  }

  func testPrepareRollbackPreservesReusedAndNewlyClaimedCachesButNotUnclaimedLoads() throws {
    let queue = DispatchQueue(label: "test.speech.prepare.ownership")
    var created = 0
    var released: [Int] = []
    let cache = SpeechEngineCache<String, Int>(create: { _ in created += 1; return created },
      release: { released.append($0) }, sessionState: { (true, nil) }, schedule: { _, _ in {} })
    try queue.sync {
      let oldRollback = try cache.prepare("a", check: {})
      let reusedRollback = try cache.prepare("a", check: {})
      reusedRollback(); oldRollback()
      XCTAssertEqual(created, 1)
      XCTAssertTrue(released.isEmpty)
      let replacementRollback = try cache.prepare("b", check: {})
      XCTAssertEqual(released, [1])
      XCTAssertEqual(try cache.use("b") { $0 }, 2)
      replacementRollback()
      XCTAssertEqual(released, [1])
      let unclaimedRollback = try cache.prepare("c", check: {})
      cache.touch()
      unclaimedRollback(); unclaimedRollback()
      XCTAssertEqual(released, [1, 2, 3])
      cache.invalidate()
      XCTAssertEqual(released, [1, 2, 3])
    }
  }

  func testPrepareDoesNotMarkUnpinnedIdleAsNewInferenceActivity() throws {
    var now: TimeInterval = 20
    var delays: [TimeInterval] = []
    let cache = SpeechEngineCache<String, Int>(create: { _ in 1 }, release: { _ in },
      sessionState: { (false, 10) },
      schedule: { delay, _ in delays.append(delay); return {} }, now: { now })
    _ = try cache.prepare("model", check: {})
    XCTAssertEqual(delays.last, 20)
    now = 25
    _ = try cache.prepare("model", check: {})
    XCTAssertEqual(delays.last, 15)
    now = 26
    _ = try cache.use("model") { $0 }
    XCTAssertEqual(delays.last, 30)
    cache.invalidate()
  }

  func testSessionPinsBothCachesUntilLastReleaseAndReusesNativeLoads() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    var created = 0
    var released = 0
    var timers: [(TimeInterval, () -> Void)] = []
    func cache() -> SpeechEngineCache<String, Int> {
      SpeechEngineCache<String, Int>(create: { _ in created += 1; return created },
        release: { _ in released += 1 }, sessionState: { sessions.state },
        schedule: { delay, operation in timers.append((delay, operation)); return {} }, now: { now })
    }
    let asr = cache()
    let tts = cache()
    let conversation = try sessions.begin(first, kinds: ["asr", "tts"], hotwords: "Pisper")
    XCTAssertEqual(try asr.use("beam") { $0 }, 1)
    XCTAssertEqual(try tts.use("voice") { $0 }, 2)
    XCTAssertTrue(timers.isEmpty)
    now = 120
    XCTAssertEqual(try asr.use("beam") { $0 }, 1)
    XCTAssertEqual(try tts.use("voice") { $0 }, 2)
    _ = try sessions.begin(second, kinds: ["asr"], hotwords: "")
    try sessions.release(first)
    asr.touch(); tts.touch()
    XCTAssertTrue(conversation.cancelled)
    XCTAssertTrue(timers.isEmpty)
    XCTAssertTrue(sessions.state.pinned)
    now = 180
    try sessions.release(second)
    asr.touch(); tts.touch()
    XCTAssertEqual(timers.map { $0.0 }, [30, 30])
    now = 190
    try sessions.release(second)
    XCTAssertEqual(sessions.state.idleSince, 180)
    now = 210
    timers.forEach { $0.1() }
    XCTAssertEqual(created, 2)
    XCTAssertEqual(released, 2)
  }

  func testSessionRejectsDuplicateAndReleasedIdsWithoutAffectingOtherOwners() throws {
    let sessions = SpeechAudioSessions()
    let old = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    assertCode("speech_request_busy") { _ = try sessions.begin(first, kinds: ["asr"], hotwords: "") }
    assertCode("speech_request_busy") { _ = try sessions.begin(first, kinds: ["tts"], hotwords: "") }
    let current = try sessions.begin(second, kinds: ["asr", "tts"], hotwords: "")
    try sessions.release(first)
    sessions.fail(old)
    XCTAssertFalse(current.cancelled)
    XCTAssertTrue(sessions.state.pinned)
    assertCode("speech_cancelled") { _ = try sessions.begin(first, kinds: ["asr"], hotwords: "") }
    try sessions.release(third)
    assertCode("speech_cancelled") { _ = try sessions.begin(third, kinds: ["tts"], hotwords: "") }
    for kinds in [[], ["asr", "asr"], ["asr", "tts", "asr"], ["other"]] {
      assertCode("speech_invalid_session_kinds") {
        _ = try sessions.begin(fourth, kinds: kinds, hotwords: "")
      }
    }
    assertCode("speech_invalid_hotwords") {
      _ = try sessions.begin(fourth, kinds: ["asr"], hotwords: "bad/word")
    }
    try sessions.release(second)
    XCTAssertFalse(sessions.state.pinned)
  }

  func testSessionPinCapacityIsSixteenAndRejectedIdCanRetryAfterRelease() throws {
    let sessions = SpeechAudioSessions()
    var owners: [SpeechAudioRequest] = []
    for _ in 0..<16 {
      owners.append(try sessions.begin(UUID().uuidString, kinds: ["asr"], hotwords: ""))
    }
    assertCode("speech_request_busy") {
      _ = try sessions.begin(owners[0].id, kinds: ["tts"], hotwords: "")
    }
    assertCode("speech_engine_busy") {
      _ = try sessions.begin(first, kinds: ["asr", "tts"], hotwords: "")
    }
    try sessions.release(owners[0].id)
    let next = try sessions.begin(first, kinds: ["asr", "tts"], hotwords: "")
    try sessions.release(owners[0].id)
    sessions.fail(owners[0])
    XCTAssertFalse(next.cancelled)
    assertCode("speech_engine_busy") {
      _ = try sessions.begin(second, kinds: ["tts"], hotwords: "")
    }
    sessions.clear()
    XCTAssertFalse(sessions.state.pinned)
  }

  func testSessionTombstonesExpireAcrossMoreThan4096CompletedRounds() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    for index in 0..<4200 {
      let request = try sessions.begin(UUID().uuidString, kinds: ["asr"], hotwords: "")
      sessions.finish(request)
      if index % 2 == 0 { try sessions.release(request.id) }
      else { sessions.clear() }
      XCTAssertTrue(request.cancelled)
      now += 1
    }
    XCTAssertNoThrow(try sessions.begin(first, kinds: ["asr"], hotwords: ""))
  }

  func testSessionExpiryPreservesActiveAndRunningTokensAndOldCleanupCannotReleaseReplacement() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    let active = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    sessions.finish(active)
    let running = try sessions.begin(second, kinds: ["asr"], hotwords: "")
    try sessions.release(second)
    now = SpeechAudioRequests.retention + 1
    assertCode("speech_request_busy") { _ = try sessions.begin(first, kinds: ["tts"], hotwords: "") }
    assertCode("speech_cancelled") { _ = try sessions.begin(second, kinds: ["asr"], hotwords: "") }
    XCTAssertFalse(active.cancelled)
    sessions.clear()
    now += SpeechAudioRequests.retention + 1
    assertCode("speech_cancelled") { _ = try sessions.begin(second, kinds: ["asr"], hotwords: "") }
    sessions.finish(running)
    now += SpeechAudioRequests.retention
    assertCode("speech_cancelled") { _ = try sessions.begin(second, kinds: ["asr"], hotwords: "") }
    now += 1
    let replacement = try sessions.begin(second, kinds: ["asr"], hotwords: "")
    XCTAssertFalse(replacement === running)
    assertCode("speech_cancelled") { try running.check() }
    sessions.fail(running)
    sessions.finish(running)
    XCTAssertFalse(replacement.cancelled)
    XCTAssertTrue(sessions.state.pinned)
    assertCode("speech_request_busy") { _ = try sessions.begin(second, kinds: ["tts"], hotwords: "") }
    // 旧会话的 ID 释放也不能影响使用另一 ID 的新会话。
    try sessions.release(first)
    XCTAssertFalse(replacement.cancelled)
    XCTAssertTrue(sessions.state.pinned)
  }

  func testCancelBeforeBeginTombstoneExpiresAndReleasePrunesOldEntries() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    try sessions.release(first)
    now = SpeechAudioRequests.retention
    assertCode("speech_cancelled") { _ = try sessions.begin(first, kinds: ["asr"], hotwords: "") }
    now += 1
    let next = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    sessions.finish(next)
    try sessions.release(first)
    for _ in 0..<4200 {
      now += 1
      try sessions.release(UUID().uuidString)
    }
    XCTAssertNoThrow(try sessions.begin(first, kinds: ["asr"], hotwords: ""))
  }

  func testUnpinnedUseRefreshesIdleDespiteOldSessionReleaseTimestamp() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    var created = 0
    var released = 0
    var timers: [(TimeInterval, () -> Void)] = []
    let cache = SpeechEngineCache<String, Int>(create: { _ in created += 1; return created },
      release: { _ in released += 1 }, sessionState: { sessions.state },
      schedule: { delay, operation in timers.append((delay, operation)); return {} }, now: { now })
    let request = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    _ = try cache.use("asr") { $0 }
    sessions.finish(request)
    XCTAssertTrue(timers.isEmpty)
    now = 10
    try sessions.release(first)
    cache.touch()
    let sessionTimer = try XCTUnwrap(timers.last).1
    now = 20
    XCTAssertEqual(try cache.use("asr") { value in now = 22; return value }, 1)
    XCTAssertEqual(try XCTUnwrap(timers.last).0, 30)
    now = 30
    cache.touch()
    XCTAssertEqual(try XCTUnwrap(timers.last).0, 22)
    now = 40
    sessionTimer()
    XCTAssertEqual(released, 0)
    now = 52
    try XCTUnwrap(timers.last).1()
    XCTAssertEqual(released, 1)
    now = 100
    XCTAssertEqual(try cache.use("asr") { $0 }, 2)
    XCTAssertEqual(try XCTUnwrap(timers.last).0, 30)
    let staleTimer = try XCTUnwrap(timers.last).1
    _ = try sessions.begin(second, kinds: ["asr"], hotwords: "")
    cache.touch()
    let timerCount = timers.count
    now = 300
    staleTimer()
    XCTAssertEqual(try cache.use("asr") { $0 }, 2)
    XCTAssertEqual(timers.count, timerCount)
    XCTAssertEqual(released, 1)
    cache.invalidate()
  }

  func testPrepareFailureReleasesOnlyItsPinAndAllowsNewSession() throws {
    let sessions = SpeechAudioSessions()
    let other = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    let failed = try sessions.begin(second, kinds: ["tts"], hotwords: "")
    var fail = true
    var created = 0
    let cache = SpeechEngineCache<String, Int>(create: { _ in
      if fail { throw SpeechAudioError("speech_model_load_failed") }
      created += 1; return created
    }, release: { _ in }, sessionState: { sessions.state }, schedule: { _, _ in {} })
    assertCode("speech_model_load_failed") {
      do { _ = try cache.use("tts") { $0 } }
      catch { sessions.fail(failed); throw error }
    }
    XCTAssertTrue(failed.cancelled)
    XCTAssertFalse(other.cancelled)
    XCTAssertTrue(sessions.state.pinned)
    try sessions.release(first)
    XCTAssertFalse(sessions.state.pinned)
    let next = try sessions.begin(third, kinds: ["tts"], hotwords: "")
    fail = false
    XCTAssertEqual(try cache.use("tts") { _ in try next.check(); return created }, 1)
    XCTAssertTrue(sessions.state.pinned)
    cache.invalidate()
  }

  func testReleaseDuringNativeLoadCannotPublishOrDelayIdleDeadline() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    let old = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    var created = 0
    var released: [Int] = []
    let cache = SpeechEngineCache<String, Int>(create: { _ in
      created += 1
      if created == 1 {
        now = 10
        try sessions.release(first)
        _ = try sessions.begin(second, kinds: ["asr"], hotwords: "")
      }
      return created
    }, release: { released.append($0) }, sessionState: { sessions.state }, schedule: { _, _ in {} }, now: { now })
    assertCode("speech_cancelled") {
      do { try cache.use("old") { _ in try old.check() } }
      catch { sessions.fail(old); throw error }
    }
    XCTAssertEqual(released, [1])
    XCTAssertTrue(sessions.state.pinned)
    XCTAssertEqual(try cache.use("new") { $0 }, 2)
    sessions.fail(old)
    XCTAssertTrue(sessions.state.pinned)
    XCTAssertEqual(try cache.use("new") { $0 }, 2)
    try sessions.release(second)
    XCTAssertEqual(sessions.state.idleSince, 10)
    now = 20
    sessions.fail(old)
    XCTAssertEqual(sessions.state.idleSince, 10)
    cache.invalidate()
  }

  func testCancelledQueuedPrepareDoesNotCreateNativeAndPartialFailureStartsIdle() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    var created = 0
    var timers: [(TimeInterval, () -> Void)] = []
    let cache = SpeechEngineCache<String, Int>(create: { _ in created += 1; return created },
      release: { _ in }, sessionState: { sessions.state },
      schedule: { delay, operation in timers.append((delay, operation)); return {} }, now: { now })
    let cancelled = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    try sessions.release(first)
    assertCode("speech_cancelled") {
      try cancelled.check()
      _ = try cache.use("asr") { $0 }
    }
    XCTAssertEqual(created, 0)
    let partial = try sessions.begin(second, kinds: ["asr", "tts"], hotwords: "")
    _ = try cache.use("asr") { $0 }
    XCTAssertTrue(timers.isEmpty)
    now = 10
    sessions.fail(partial)
    // 第二个模型加载失败或取消后，已加载的第一个模型从退出时刻计时。
    now = 15
    cache.touch()
    XCTAssertEqual(try XCTUnwrap(timers.last).0, 25)
    XCTAssertFalse(sessions.state.pinned)
    cache.invalidate()
  }

  func testQueuedOldTimerHonorsPinsAndLastReleaseTime() throws {
    var now: TimeInterval = 0
    let sessions = SpeechAudioSessions(now: { now })
    var released = 0
    var timers: [(TimeInterval, () -> Void)] = []
    let cache = SpeechEngineCache<String, Int>(create: { _ in 1 }, release: { _ in released += 1 },
      sessionState: { sessions.state },
      schedule: { delay, operation in timers.append((delay, operation)); return {} }, now: { now })
    _ = try cache.use("a") { $0 }
    let oldTimer = try XCTUnwrap(timers.last).1
    _ = try sessions.begin(first, kinds: ["asr"], hotwords: "")
    now = 60
    oldTimer()
    XCTAssertEqual(released, 0)
    try sessions.release(first)
    now = 70
    oldTimer()
    XCTAssertEqual(released, 0)
    XCTAssertEqual(try XCTUnwrap(timers.last).0, 20)
    now = 90
    try XCTUnwrap(timers.last).1()
    XCTAssertEqual(released, 1)
  }

  func testASRConfigurationKeySeparatesModelAndGreedyBeamMode() throws {
    let model = SpeechASRConfiguration(encoder: "encoder", decoder: "decoder", joiner: "joiner",
      tokens: "tokens", bpeVocab: "bpe")
    let other = SpeechASRConfiguration(encoder: "other", decoder: "decoder", joiner: "joiner",
      tokens: "tokens", bpeVocab: "bpe")
    let greedy = SpeechASREngineConfiguration(model: model, usesHotwords: false)
    let beam = SpeechASREngineConfiguration(model: model, usesHotwords: true)
    XCTAssertNotEqual(greedy, beam)
    XCTAssertNotEqual(beam, SpeechASREngineConfiguration(model: other, usesHotwords: true))
    var created = 0
    var released = 0
    let cache = SpeechEngineCache<SpeechASREngineConfiguration, Int>(
      create: { _ in created += 1; return created }, release: { _ in released += 1 },
      sessionState: { (true, nil) }, schedule: { _, _ in XCTFail("活跃会话不应计时"); return {} })
    XCTAssertEqual(try cache.use(greedy) { $0 }, 1)
    XCTAssertEqual(try cache.use(greedy) { $0 }, 1)
    XCTAssertEqual(try cache.use(beam) { $0 }, 2)
    XCTAssertEqual(try cache.use(beam) { $0 }, 2)
    XCTAssertEqual(created, 2)
    XCTAssertEqual(released, 1)
    cache.invalidate()
  }

  func testBackgroundMemoryAndHostCleanupClearPinsBeforeSerialNativeRelease() throws {
    for _ in 0..<3 {
      let queue = DispatchQueue(label: "test.speech.session.lifecycle")
      let sessions = SpeechAudioSessions()
      let request = try sessions.begin(first, kinds: ["asr", "tts"], hotwords: "")
      var inUse = false
      var released = 0
      let cache = SpeechEngineCache<String, Int>(create: { _ in 1 },
        release: { _ in XCTAssertFalse(inUse); released += 1 }, sessionState: { sessions.state },
        schedule: { _, _ in {} })
      try queue.sync {
        try cache.use("native") { _ in
          inUse = true
          sessions.clear()
          queue.async { cache.invalidate() }
          XCTAssertTrue(request.cancelled)
          XCTAssertFalse(sessions.state.pinned)
          XCTAssertEqual(released, 0)
          inUse = false
        }
      }
      queue.sync { XCTAssertEqual(released, 1) }
      assertCode("speech_cancelled") { _ = try sessions.begin(first, kinds: ["asr"], hotwords: "") }
      XCTAssertNoThrow(try sessions.begin(second, kinds: ["asr"], hotwords: ""))
      sessions.fail(request)
      XCTAssertTrue(sessions.state.pinned)
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

  func testRequestAdmissionIsBoundedPerKindWithoutCrossKindStarvation() throws {
    let requests = SpeechAudioRequests()
    let asr = try requests.begin(first, kind: .asr)
    _ = try requests.begin(second, kind: .asr)
    _ = try requests.begin(third, kind: .asr)
    assertCode("speech_engine_busy") { _ = try requests.begin(fourth, kind: .asr) }
    // ASR 已满不占 TTS 的三个槽位；拒绝过的 UUID 未取得 running 所有权。
    let tts = try requests.begin(fourth, kind: .tts)
    _ = try requests.begin(UUID().uuidString, kind: .tts)
    _ = try requests.begin(UUID().uuidString, kind: .tts)
    let pending = UUID().uuidString
    assertCode("speech_engine_busy") { _ = try requests.begin(pending, kind: .tts) }
    assertCode("speech_request_busy") { _ = try requests.begin(first, kind: .tts) }
    XCTAssertTrue(try requests.cancel(first) === asr)
    XCTAssertFalse(tts.cancelled)
    // 控制取消无需槽位，但队列中的旧工作真正结束前不能提前让出本类容量。
    assertCode("speech_engine_busy") { _ = try requests.begin(pending, kind: .asr) }
    requests.finish(asr)
    let nextASR = try requests.begin(pending, kind: .asr)
    let nextTTSId = UUID().uuidString
    assertCode("speech_engine_busy") { _ = try requests.begin(nextTTSId, kind: .tts) }
    requests.finish(tts)
    XCTAssertNoThrow(try requests.begin(nextTTSId, kind: .tts))
    XCTAssertFalse(nextASR.cancelled)
    requests.finish(asr)
    assertCode("speech_engine_busy") { _ = try requests.begin(UUID().uuidString, kind: .asr) }
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
