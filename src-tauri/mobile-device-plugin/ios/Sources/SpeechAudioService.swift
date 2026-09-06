import AVFoundation
import Foundation
import ObjectiveC
import UIKit

struct SpeechAudioError: LocalizedError {
  let code: String
  init(_ code: String) { self.code = code }
  var errorDescription: String? { code }
  static func code(_ error: Error) -> String {
    let description = error.localizedDescription
    return description.range(of: "^speech_[a-z0-9_]+$", options: .regularExpression) != nil
      ? description : "speech_native_failed"
  }
}

struct SpeechASRConfiguration: Equatable {
  let encoder: String
  let decoder: String
  let joiner: String
  let tokens: String
  let bpeVocab: String
}

struct SpeechTTSConfiguration: Equatable {
  let model: String
  let tokens: String
  let dictDir: String
  let lexicon: String
  let ruleFsts: [String]
  let numThreads: Int
  let maxTextCodePoints: Int
  let speakerId: Int
  let noiseScale: Float
  let noiseScaleW: Float
  let lengthScale: Float
}

protocol SpeechAudioModelProviding: AnyObject {
  func list() throws -> [String: Any]
  func startDownload(modelId: String) throws -> [String: Any]
  func cancelDownload(modelId: String) throws -> [String: Any]
  func asrConfiguration(modelId: String?, check: () throws -> Void) throws -> SpeechASRConfiguration
  func ttsConfiguration(voiceId: String, check: () throws -> Void) throws -> SpeechTTSConfiguration
}

private final class SpeechAudioModelAdapter: SpeechAudioModelProviding {
  private let store: SpeechModelStore
  private let resources: URL
  private let notices: [String: Any]

  private init(store: SpeechModelStore, resources: URL, notices: [String: Any]) {
    self.store = store; self.resources = resources; self.notices = notices
  }

  static func bundled() throws -> SpeechAudioModelProviding {
    guard let bundleRoot = Bundle.module.resourceURL else { throw SpeechAudioError("speech_resources_missing") }
    let resources = bundleRoot.appendingPathComponent("SpeechResources", isDirectory: true).resolvingSymlinksInPath()
    let support = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
      appropriateFor: nil, create: true).resolvingSymlinksInPath()
    let store = try SpeechModelStore(catalogURL: resources.appendingPathComponent("speech-model-catalog.json"),
      storageDirectory: support.appendingPathComponent("pisper-speech-models", isDirectory: true),
      archiveExtractor: SpeechModelArchive.extract)
    let data = try SpeechFiles.readSmall(resources.appendingPathComponent("speech-resource-notices.json"),
      limit: 4 * 1024 * 1024)
    guard let notices = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      try speechInteger(notices["schemaVersion"]) == 1 else { throw SpeechAudioError("speech_catalog_invalid") }
    return SpeechAudioModelAdapter(store: store, resources: resources, notices: notices)
  }

  func list() throws -> [String: Any] { try store.list() }
  func startDownload(modelId: String) throws -> [String: Any] { try store.startDownload(modelId: modelId) }
  func cancelDownload(modelId: String) throws -> [String: Any] { try store.cancelDownload(modelId: modelId) }

  private func path(_ model: SpeechModel, _ directory: URL, _ key: String) throws -> String {
    guard let path = model.config[key] as? String,
      model.files.contains(where: { $0.path == path }) else { throw SpeechAudioError("speech_catalog_invalid") }
    return try SpeechFiles.child(directory, path).path
  }

  private func pathList(_ model: SpeechModel, _ directory: URL, _ key: String) throws -> [String] {
    guard let raw = model.config[key] else { return [] }
    guard let paths = raw as? [String] else { throw SpeechAudioError("speech_catalog_invalid") }
    return try paths.map { path in
      guard !path.contains(","), model.files.contains(where: { $0.path == path }) else {
        throw SpeechAudioError("speech_catalog_invalid")
      }
      return try SpeechFiles.child(directory, path).path
    }
  }

  private func bpeVocab(_ model: SpeechModel, check: () throws -> Void) throws -> String {
    guard let path = model.config["bpeVocabResource"] as? String else { return "" }
    guard let models = notices["models"] as? [[String: Any]],
      let notice = models.first(where: { $0["id"] as? String == model.id && $0["revision"] as? String == model.revision }),
      let modifications = notice["modifications"] as? [[String: Any]],
      let modification = modifications.first(where: { ($0["derived"] as? [String: Any])?["path"] as? String == path }),
      let source = modification["source"] as? [String: Any],
      let sourcePath = source["path"] as? String, let sourceSHA = source["sha256"] as? String,
      let derived = modification["derived"] as? [String: Any], let sha = derived["sha256"] as? String else {
      throw SpeechAudioError("speech_catalog_invalid")
    }
    let sourceBytes = try speechInteger(source["bytes"])
    guard model.files.contains(where: { $0.path == sourcePath && $0.sha256 == sourceSHA && $0.bytes == sourceBytes }) else {
      throw SpeechAudioError("speech_catalog_invalid")
    }
    let bytes = try speechInteger(derived["bytes"], maximum: 1024 * 1024)
    guard bytes > 0 else { throw SpeechAudioError("speech_catalog_invalid") }
    let url = try SpeechFiles.child(resources, path)
    try SpeechFiles.scan(url, spec: SpeechDownloadFile(path: path, bytes: bytes, sha256: sha, urls: []), check: check)
    return url.path
  }

  func asrConfiguration(modelId: String?, check: () throws -> Void) throws -> SpeechASRConfiguration {
    try check()
    let model = try store.model(id: modelId, kind: "asr")
    guard model.engine == "online-transducer" else { throw SpeechAudioError("speech_model_engine_unsupported") }
    let directory = try store.modelDirectory(modelId: model.id, checkCancellation: check)
    try check()
    return try SpeechASRConfiguration(encoder: path(model, directory, "encoder"),
      decoder: path(model, directory, "decoder"), joiner: path(model, directory, "joiner"),
      tokens: path(model, directory, "tokens"), bpeVocab: bpeVocab(model, check: check))
  }

  func ttsConfiguration(voiceId: String, check: () throws -> Void) throws -> SpeechTTSConfiguration {
    try check()
    let voice = try store.voice(id: voiceId.isEmpty ? store.defaultVoiceId : voiceId)
    let model = voice.model
    guard model.engine == "vits", voice.sourceSpeaker == 0,
      let dictDir = model.config["dictDir"] as? String else {
      throw SpeechAudioError("speech_model_engine_unsupported")
    }
    let threads = Int(try speechInteger(model.config["numThreads"], maximum: 16))
    let maxText = Int(try speechInteger(model.config["maxTextCodePoints"], maximum: 400))
    guard threads > 0, maxText > 0 else { throw SpeechAudioError("speech_catalog_invalid") }
    let directory = try store.modelDirectory(modelId: model.id, checkCancellation: check)
    try check()
    guard model.files.contains(where: { $0.path.hasPrefix(dictDir + "/") }) else {
      throw SpeechAudioError("speech_catalog_invalid")
    }
    let dictionary = try SpeechFiles.child(directory, dictDir)
    let descriptor = try SpeechFiles.directory(dictionary)
    try FileHandle(fileDescriptor: descriptor, closeOnDealloc: true).close()
    try check()
    return try SpeechTTSConfiguration(model: path(model, directory, "model"),
      tokens: path(model, directory, "tokens"), dictDir: dictionary.path,
      lexicon: path(model, directory, "lexicon"), ruleFsts: pathList(model, directory, "ruleFsts"),
      numThreads: threads, maxTextCodePoints: maxText, speakerId: 0,
      noiseScale: speechVitsScale(model.config["noiseScale"], defaultValue: 0.667),
      noiseScaleW: speechVitsScale(model.config["noiseScaleW"], defaultValue: 0.8),
      lengthScale: speechVitsScale(model.config["lengthScale"], defaultValue: 1.0))
  }
}

enum SpeechAudioKind: Equatable {
  case asr
  case tts
}

final class SpeechAudioRequest {
  let id: String
  private let lock = NSLock()
  private var stopped = false
  fileprivate var running = false
  fileprivate var kind: SpeechAudioKind = .tts
  fileprivate var started = false
  fileprivate var touchedAt: TimeInterval

  init(id: String, now: TimeInterval) { self.id = id; touchedAt = now }
  var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return stopped }
  func cancel() { lock.lock(); stopped = true; lock.unlock() }
  func check() throws { if cancelled { throw SpeechAudioError("speech_cancelled") } }
}

final class SpeechAudioRequests {
  static let retention: TimeInterval = 600
  private let lock = NSLock()
  private let now: () -> TimeInterval
  private var entries: [String: SpeechAudioRequest] = [:]
  private var foreground = true
  private let capacity: Int

  init(capacity: Int = 3, now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
    self.capacity = capacity
    self.now = now
  }

  static func validateId(_ value: String) throws -> String {
    guard value.utf8.count == 36, let uuid = UUID(uuidString: value),
      uuid.uuidString.caseInsensitiveCompare(value) == .orderedSame
    else { throw SpeechAudioError("speech_invalid_request_id") }
    return uuid.uuidString.lowercased()
  }

  private func entry(_ value: String) throws -> SpeechAudioRequest {
    let id = try Self.validateId(value)
    let time = now()
    entries = entries.filter { $0.value.running || time - $0.value.touchedAt <= Self.retention }
    if let found = entries[id] { return found }
    guard entries.count < 4096 else { throw SpeechAudioError("speech_request_limit") }
    let request = SpeechAudioRequest(id: id, now: time)
    entries[id] = request
    return request
  }

  func begin(_ id: String, kind: SpeechAudioKind = .tts) throws -> SpeechAudioRequest {
    lock.lock(); defer { lock.unlock() }
    guard foreground else { throw SpeechAudioError("speech_app_backgrounded") }
    let request = try entry(id)
    try request.check()
    guard !request.started else { throw SpeechAudioError("speech_request_busy") }
    guard entries.values.filter({ $0.running && $0.kind == kind }).count < capacity else {
      throw SpeechAudioError("speech_engine_busy")
    }
    request.kind = kind
    request.started = true
    request.running = true
    request.touchedAt = now()
    return request
  }

  func finish(_ request: SpeechAudioRequest) {
    lock.lock(); defer { lock.unlock() }
    guard entries[request.id] === request else { return }
    request.running = false
    request.touchedAt = now()
  }

  @discardableResult func cancel(_ id: String) throws -> SpeechAudioRequest {
    lock.lock(); defer { lock.unlock() }
    let request = try entry(id)
    request.cancel()
    request.touchedAt = now()
    return request
  }

  func pause() {
    lock.lock(); defer { lock.unlock() }
    foreground = false
    entries.values.forEach { $0.cancel() }
  }

  func resume() { lock.lock(); foreground = true; lock.unlock() }
  func checkForeground() throws {
    lock.lock(); defer { lock.unlock() }
    guard foreground else { throw SpeechAudioError("speech_app_backgrounded") }
  }
}

// 控制线程先登记所有权，推理线程只检查凭证；释放后的 ID 不得被迟到预热复活。
final class SpeechAudioSessions {
  private let lock = NSLock()
  private var entries: [String: SpeechAudioRequest] = [:]
  private var active: Set<String> = []
  private var idleSince: TimeInterval?
  private let now: () -> TimeInterval

  init(now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
    self.now = now
  }

  var state: (pinned: Bool, idleSince: TimeInterval?) {
    lock.lock(); defer { lock.unlock() }
    return (!active.isEmpty, idleSince)
  }

  private func pruneLocked(at time: TimeInterval) {
    // 已取消但仍在预热/发布的对象不能回收；迟到工作始终检查原凭证。
    entries = entries.filter { id, request in
      active.contains(id) || request.running || !request.cancelled
        || time - request.touchedAt <= SpeechAudioRequests.retention
    }
  }

  func begin(_ value: String, kinds: [String], hotwords: String) throws -> SpeechAudioRequest {
    let id = try SpeechAudioRequests.validateId(value)
    guard (1...2).contains(kinds.count), Set(kinds).count == kinds.count,
      kinds.allSatisfy({ ["asr", "tts"].contains($0) }) else {
      throw SpeechAudioError("speech_invalid_session_kinds")
    }
    try SpeechPCM.validateHotwords(hotwords)
    lock.lock(); defer { lock.unlock() }
    let time = now()
    pruneLocked(at: time)
    if let existing = entries[id] {
      try existing.check()
      throw SpeechAudioError("speech_request_busy")
    }
    guard active.count < 16 else { throw SpeechAudioError("speech_engine_busy") }
    guard entries.count < 4096 else { throw SpeechAudioError("speech_request_limit") }
    let request = SpeechAudioRequest(id: id, now: time)
    request.running = true
    entries[id] = request
    active.insert(id)
    idleSince = nil
    return request
  }

  func release(_ value: String) throws {
    let id = try SpeechAudioRequests.validateId(value)
    lock.lock(); defer { lock.unlock() }
    let time = now()
    pruneLocked(at: time)
    if entries[id] == nil {
      guard entries.count < 4096 else { throw SpeechAudioError("speech_request_limit") }
      entries[id] = SpeechAudioRequest(id: id, now: time)
    }
    entries[id]?.cancel()
    entries[id]?.touchedAt = time
    if active.remove(id) != nil && active.isEmpty { idleSince = time }
  }

  func finish(_ request: SpeechAudioRequest) {
    lock.lock(); defer { lock.unlock() }
    guard entries[request.id] === request else { return }
    request.running = false
    request.touchedAt = now()
  }

  func fail(_ request: SpeechAudioRequest) {
    lock.lock(); defer { lock.unlock() }
    guard entries[request.id] === request else { return }
    request.cancel()
    request.touchedAt = now()
    if active.remove(request.id) != nil && active.isEmpty { idleSince = request.touchedAt }
  }

  func clear() {
    lock.lock(); defer { lock.unlock() }
    let time = now()
    pruneLocked(at: time)
    entries.values.forEach {
      if !$0.cancelled { $0.touchedAt = time }
      $0.cancel()
    }
    active.removeAll()
    idleSince = time
  }
}

struct SpeechAudioClip {
  let id: String
  let request: SpeechAudioRequest
  let file: URL
  let sampleRate: Int
  let durationMs: Int
  let createdAt: TimeInterval
}

final class SpeechAudioTokens {
  private let lock = NSLock()
  private let now: () -> TimeInterval
  private let remove: (URL) -> Void
  private var clips: [String: SpeechAudioClip] = [:]

  init(now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime },
       remove: @escaping (URL) -> Void = { try? FileManager.default.removeItem(at: $0) }) {
    self.now = now
    self.remove = remove
  }

  private func pruneLocked() {
    let expired = clips.values.filter { $0.request.cancelled || now() - $0.createdAt >= 120 }
    for clip in expired { clips.removeValue(forKey: clip.id); remove(clip.file) }
  }

  func prune() { lock.lock(); defer { lock.unlock() }; pruneLocked() }

  func checkCapacity() throws {
    lock.lock(); defer { lock.unlock() }
    pruneLocked()
    guard clips.count < 4 else { throw SpeechAudioError("speech_audio_queue_full") }
  }

  func insert(_ clip: SpeechAudioClip) throws {
    lock.lock(); defer { lock.unlock() }
    pruneLocked()
    try clip.request.check()
    guard clips.count < 4 else { throw SpeechAudioError("speech_audio_queue_full") }
    guard clips[clip.id] == nil else { throw SpeechAudioError("speech_audio_unknown") }
    clips[clip.id] = clip
  }

  func consume(audioId: String, requestId: String) throws -> SpeechAudioClip {
    let id = try SpeechAudioRequests.validateId(requestId)
    let audioId = try SpeechAudioRequests.validateId(audioId)
    lock.lock(); defer { lock.unlock() }
    pruneLocked()
    guard let clip = clips[audioId] else { throw SpeechAudioError("speech_audio_unknown") }
    guard clip.request.id == id else { throw SpeechAudioError("speech_audio_request_mismatch") }
    try clip.request.check()
    clips.removeValue(forKey: audioId)
    return clip
  }
}

enum SpeechAudioInterruption {
  static let notification = Notification.Name("pisper.speech.interrupted")

  static func invalidates(_ note: Notification) -> Bool {
    if note.name == AVAudioSession.routeChangeNotification {
      // WebView 初次启麦和服务切换输出类别都不是物理设备中断，不能终止正常轮次。
      return (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt)
        != AVAudioSession.RouteChangeReason.categoryChange.rawValue
    }
    if note.name == AVAudioSession.interruptionNotification {
      return (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt)
        != AVAudioSession.InterruptionType.ended.rawValue
    }
    return [UIApplication.willResignActiveNotification, UIApplication.didEnterBackgroundNotification,
      AVAudioSession.mediaServicesWereResetNotification].contains(note.name)
  }

  static func publish(_ note: Notification, sender: AnyObject, center: NotificationCenter = .default) {
    guard invalidates(note) else { return }
    // 即使 listening/thinking 尚无原生 requestId，也必须让共享前端停止旧轮次。
    center.post(name: notification, object: sender, userInfo: ["source": note.name.rawValue])
  }

  static func shouldForward(source: String?, microphonePermissionPending: Bool) -> Bool {
    // 系统麦克风授权会短暂 inactive；只豁免该弹窗，真实后台或音频中断仍须终止轮次。
    return !microphonePermissionPending || source != UIApplication.willResignActiveNotification.rawValue
  }
}

enum SpeechAudioWebOrigin {
  static func trusted(_ url: URL?) -> Bool {
    guard let url, url.user == nil, url.password == nil else { return false }
    return (url.scheme == "http" && url.host == "127.0.0.1")
      || (url.scheme == "tauri" && url.host == "localhost" && url.port == nil)
  }
}

protocol SpeechAudioSessionControlling: AnyObject {
  var category: AVAudioSession.Category { get }
  func configurePlayback() throws
  func activate() throws
  func deactivate() throws
}

private final class SpeechSystemAudioSession: SpeechAudioSessionControlling {
  private let session = AVAudioSession.sharedInstance()
  var category: AVAudioSession.Category { session.category }
  func configurePlayback() throws { try session.setCategory(.playback, mode: .default) }
  func activate() throws { try session.setActive(true) }
  func deactivate() throws { try session.setActive(false, options: .notifyOthersOnDeactivation) }
}

final class SpeechAudioSessionLease {
  private let session: SpeechAudioSessionControlling
  private let microphoneInUse: () -> Bool
  private(set) var owned = false

  init(session: SpeechAudioSessionControlling, microphoneInUse: @escaping () -> Bool = { false }) {
    self.session = session; self.microphoneInUse = microphoneInUse
  }

  func prepare() throws {
    if microphoneInUse() || [.playAndRecord, .record, .multiRoute].contains(session.category) {
      owned = false
      return
    }
    if session.category != .playback { try session.configurePlayback() }
    try session.activate()
    owned = true
  }

  func observeCategory() {
    if session.category != .playback || microphoneInUse() { owned = false }
  }

  func release() throws {
    observeCategory()
    guard owned else { return }
    // 停播后才交还自己激活的输出 session；WebView 已接管输入时绝不 deactivate。
    try session.deactivate()
    owned = false
  }
}

protocol SpeechAudioMicrophoneOwner: AnyObject {
  var speechMicrophoneInUse: Bool { get }
}

private var speechHostReleaseKey: UInt8 = 0

// 宿主通过关联对象持有清理凭证，服务不强持有插件，也不依赖定时轮询发现析构。
private final class SpeechAudioHostRelease {
  private let release: () -> Void
  init(_ release: @escaping () -> Void) { self.release = release }
  deinit { release() }
}

private final class SpeechAudioWeakMicrophoneOwner {
  weak var value: SpeechAudioMicrophoneOwner?
  init(_ value: SpeechAudioMicrophoneOwner) { self.value = value }
}

final class SpeechAudioService: NSObject, AVAudioPlayerDelegate {
  typealias Completion = (Result<[String: Any], Error>) -> Void
  // 全 App 共用两个 kind 队列：同类 FIFO，ASR/TTS 的加载和实际推理可并行。
  static let shared = SpeechAudioService()
  private let asrQueue = DispatchQueue(label: "app.pisper.speech.asr", qos: .userInitiated)
  private let ttsQueue = DispatchQueue(label: "app.pisper.speech.tts", qos: .userInitiated)
  private lazy var ttsCache = SpeechEngineCache<SpeechTTSConfiguration, SpeechNativeTTS>(
    create: { try SpeechNativeTTS(config: $0) }, release: { $0.release() },
    sessionState: { [sessions] in sessions.state },
    schedule: { [ttsQueue] delay, operation in
      let item = DispatchWorkItem(block: operation)
      ttsQueue.asyncAfter(deadline: .now() + delay, execute: item)
      return { item.cancel() }
    })
  private lazy var asrCache = SpeechEngineCache<SpeechASREngineConfiguration, SpeechNativeASR>(
    create: { try SpeechNativeASR(config: $0) }, release: { $0.release() },
    sessionState: { [sessions] in sessions.state },
    schedule: { [asrQueue] delay, operation in
      let item = DispatchWorkItem(block: operation)
      asrQueue.asyncAfter(deadline: .now() + delay, execute: item)
      return { item.cancel() }
    })
  private let sessions = SpeechAudioSessions()
  private let modelControl = DispatchQueue(label: "app.pisper.speech.models", qos: .utility)
  private let requests = SpeechAudioRequests()
  private let tokens = SpeechAudioTokens()
  private var provider: SpeechAudioModelProviding?
  private var observers: [NSObjectProtocol] = []
  private var timer: Timer?
  private var interrupted = false
  private var microphoneOwners: [SpeechAudioWeakMicrophoneOwner] = []
  private lazy var sessionLease = SpeechAudioSessionLease(session: SpeechSystemAudioSession(),
    microphoneInUse: { [weak self] in
      self?.microphoneOwners.contains(where: { $0.value?.speechMicrophoneInUse == true }) ?? false
    })
  private var cacheDirectory: URL?
  private var playback: Playback?

  private final class Playback {
    let clip: SpeechAudioClip
    let player: AVAudioPlayer
    let completion: Completion
    init(clip: SpeechAudioClip, player: AVAudioPlayer, completion: @escaping Completion) {
      self.clip = clip; self.player = player; self.completion = completion
    }
  }

  private override init() {
    super.init()
    DispatchQueue.main.async { self.observeLifecycle() }
  }

  func registerMicrophoneOwner(_ owner: SpeechAudioMicrophoneOwner) {
    precondition(Thread.isMainThread)
    microphoneOwners.removeAll { $0.value == nil || $0.value === owner }
    microphoneOwners.append(SpeechAudioWeakMicrophoneOwner(owner))
    if objc_getAssociatedObject(owner, &speechHostReleaseKey) == nil {
      let release = SpeechAudioHostRelease { [weak self] in self?.releaseEngine() }
      objc_setAssociatedObject(owner, &speechHostReleaseKey, release, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
    }
  }

  // 测试或宿主可在首次访问前提供适配器，之后不能替换正在使用的模型存储。
  func installModelProvider(_ models: SpeechAudioModelProviding) throws {
    precondition(Thread.isMainThread)
    guard provider == nil else { throw SpeechAudioError("speech_model_store_already_configured") }
    provider = models
  }

  private func models() throws -> SpeechAudioModelProviding {
    precondition(Thread.isMainThread)
    if let provider { return provider }
    let bundled = try SpeechAudioModelAdapter.bundled()
    provider = bundled
    return bundled
  }

  private func observeLifecycle() {
    requests.pause()
    if UIApplication.shared.applicationState == .active { requests.resume() }
    let center = NotificationCenter.default
    for name in [UIApplication.willResignActiveNotification, UIApplication.didEnterBackgroundNotification,
                 AVAudioSession.interruptionNotification, AVAudioSession.routeChangeNotification,
                 AVAudioSession.mediaServicesWereResetNotification] {
      observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] note in
        guard let self else { return }
        self.sessionLease.observeCategory()
        if name == AVAudioSession.routeChangeNotification,
          !SpeechAudioInterruption.invalidates(note) { return }
        if name == AVAudioSession.interruptionNotification {
          let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
          self.interrupted = raw != AVAudioSession.InterruptionType.ended.rawValue
          if !self.interrupted {
            if UIApplication.shared.applicationState == .active { self.requests.resume() }
            return
          }
        }
        self.pause()
        SpeechAudioInterruption.publish(note, sender: self)
        // 路由切换只作废当前工作；下一次用户命令可重试，不恢复旧播放或麦克风。
        if name != UIApplication.willResignActiveNotification,
          name != UIApplication.didEnterBackgroundNotification,
          name != AVAudioSession.interruptionNotification,
          !self.interrupted, UIApplication.shared.applicationState == .active { self.requests.resume() }
      })
    }
    observers.append(center.addObserver(forName: UIApplication.didBecomeActiveNotification,
      object: nil, queue: .main) { [weak self] _ in
        guard let self, !self.interrupted else { return }
        self.requests.resume()
      })
    for name in [UIApplication.didReceiveMemoryWarningNotification, UIApplication.willTerminateNotification] {
      observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
        self?.releaseEngine()
      })
    }
    timer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in self?.tokens.prune() }
  }

  private func releaseEngine() {
    // 宿主析构可能来自任意线程，统一控制消息顺序后才排 native 释放。
    let release = {
      self.sessions.clear()
      self.ttsQueue.async { self.ttsCache.invalidate() }
      self.asrQueue.async { self.asrCache.invalidate() }
    }
    if Thread.isMainThread { release() }
    else { DispatchQueue.main.async(execute: release) }
  }

  private func touchEngines() {
    ttsQueue.async { self.ttsCache.touch() }
    asrQueue.async { self.asrCache.touch() }
  }

  func prepareSession(requestId: String, kinds: [String], hotwords: String, voiceId: String,
                      completion: @escaping Completion) {
    DispatchQueue.main.async {
      var owned: SpeechAudioRequest?
      do {
        try self.requests.checkForeground()
        let request = try self.sessions.begin(requestId, kinds: kinds, hotwords: hotwords)
        owned = request
        let models = try self.models()
        self.touchEngines()
        var tasks: [SpeechEngineTask] = []
        if kinds.contains("asr") {
          tasks.append(SpeechEngineTask(queue: self.asrQueue) {
            do {
              try request.check()
              let model = try models.asrConfiguration(modelId: nil, check: request.check)
              let config = SpeechASREngineConfiguration(model: model, usesHotwords: !hotwords.isEmpty)
              return try self.asrCache.prepare(config, check: request.check)
            } catch {
              self.sessions.fail(request)
              self.touchEngines()
              throw error
            }
          })
        }
        if kinds.contains("tts") {
          tasks.append(SpeechEngineTask(queue: self.ttsQueue) {
            do {
              try request.check()
              let config = try models.ttsConfiguration(voiceId: voiceId, check: request.check)
              return try self.ttsCache.prepare(config, check: request.check)
            } catch {
              self.sessions.fail(request)
              self.touchEngines()
              throw error
            }
          })
        }
        SpeechEnginePreparation.run(tasks, check: request.check) { result in
          defer { self.sessions.finish(request) }
          switch result {
          case .success:
            completion(.success(["ready": true]))
          case .failure(let error):
            self.sessions.fail(request)
            self.touchEngines()
            completion(.failure(error))
          }
        }
      } catch {
        if let owned {
          self.sessions.fail(owned)
          self.sessions.finish(owned)
        }
        self.touchEngines()
        completion(.failure(error))
      }
    }
  }

  func releaseSession(requestId: String, completion: @escaping Completion) {
    DispatchQueue.main.async {
      do {
        try self.sessions.release(requestId)
        self.touchEngines()
        completion(.success(["released": true]))
      } catch { completion(.failure(error)) }
    }
  }

  private func pause() {
    requests.pause()
    tokens.prune()
    finishPlayback(completed: false)
    releaseEngine()
  }

  func modelOperation(_ operation: String, modelId: String? = nil, completion: @escaping Completion) {
    DispatchQueue.main.async {
      do {
        let models = try self.models()
        self.modelControl.async {
          let result = Result<[String: Any], Error> {
            switch operation {
            case "list": return try models.list()
            case "download":
              guard let modelId else { throw SpeechAudioError("speech_model_unknown") }
              return try models.startDownload(modelId: modelId)
            case "cancel":
              guard let modelId else { throw SpeechAudioError("speech_model_unknown") }
              return try models.cancelDownload(modelId: modelId)
            default: throw SpeechAudioError("speech_model_unknown")
            }
          }
          DispatchQueue.main.async { completion(result) }
        }
      } catch { completion(.failure(error)) }
    }
  }

  private func submit(requestId: String, asr: Bool = false, completion: @escaping Completion,
                      operation: @escaping (SpeechAudioRequest, SpeechAudioModelProviding) throws -> [String: Any]) {
    DispatchQueue.main.async {
      do {
        let models = try self.models()
        let request = try self.requests.begin(requestId, kind: asr ? .asr : .tts)
        let queue = asr ? self.asrQueue : self.ttsQueue
        queue.async {
          let result = Result<[String: Any], Error> {
            try request.check()
            return try autoreleasepool { try operation(request, models) }
          }
          // 推理活动只刷新所属缓存；跨 kind 的 pin 变化由控制消息分别排队。
          if asr { self.asrCache.touch() }
          else { self.ttsCache.touch() }
          DispatchQueue.main.async {
            defer { self.requests.finish(request) }
            do {
              // 取消与最终发布同在主线程排序，迟到结果不能越过取消重新获得 token。
              try request.check()
              completion(result)
            } catch {
              self.tokens.prune()
              completion(.failure(error))
            }
          }
        }
      } catch { completion(.failure(error)) }
    }
  }

  func transcribe(pcmBase64: String, hotwords: String, modelId: String?, requestId: String?,
                  completion: @escaping Completion) {
    do {
      try SpeechPCM.validateEncoded(pcmBase64)
      try SpeechPCM.validateHotwords(hotwords)
    } catch { DispatchQueue.main.async { completion(.failure(error)) }; return }
    submit(requestId: requestId ?? UUID().uuidString, asr: true, completion: completion) { request, models in
      let config = try models.asrConfiguration(modelId: modelId, check: request.check)
      let samples = try SpeechPCM.decode(pcmBase64, check: request.check)
      let text = try SpeechNativeEngine.transcribe(config: config, samples: samples,
        hotwords: hotwords, request: request, engines: self.asrCache)
      return ["text": text.trimmingCharacters(in: .whitespacesAndNewlines)]
    }
  }

  private func cache() throws -> URL {
    if let cacheDirectory { return cacheDirectory }
    let directory = FileManager.default.temporaryDirectory.resolvingSymlinksInPath()
      .appendingPathComponent("pisper-speech", isDirectory: true)
    let descriptor = try SpeechFiles.directory(directory, create: true)
    try FileHandle(fileDescriptor: descriptor, closeOnDealloc: true).close()
    // 重启后只删除本服务的 UUID 文件，不从磁盘重建可跨请求复用的播放凭证。
    for file in try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) {
      if ["part", "wav"].contains(file.pathExtension),
        (try? SpeechAudioRequests.validateId(file.deletingPathExtension().lastPathComponent)) != nil {
        try FileManager.default.removeItem(at: file)
      }
    }
    cacheDirectory = directory
    return directory
  }

  func synthesize(text: String, voiceId: String, requestId: String, completion: @escaping Completion) {
    guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      text.unicodeScalars.count <= 400, !text.utf8.contains(0) else {
      DispatchQueue.main.async { completion(.failure(SpeechAudioError("speech_invalid_text"))) }
      return
    }
    submit(requestId: requestId, completion: completion) { request, models in
      let config = try models.ttsConfiguration(voiceId: voiceId, check: request.check)
      try self.tokens.checkCapacity()
      let directory = try self.cache()
      let id = UUID().uuidString.lowercased()
      let temporary = directory.appendingPathComponent("\(id).part")
      let file = directory.appendingPathComponent("\(id).wav")
      var published = false
      defer {
        try? FileManager.default.removeItem(at: temporary)
        if !published { try? FileManager.default.removeItem(at: file) }
      }
      let generated = try SpeechNativeEngine.synthesize(config: config, text: text,
        file: temporary, request: request, engines: self.ttsCache)
      try request.check()
      try FileManager.default.moveItem(at: temporary, to: file)
      let clip = SpeechAudioClip(id: id, request: request, file: file,
        sampleRate: generated.sampleRate, durationMs: generated.durationMs,
        createdAt: ProcessInfo.processInfo.systemUptime)
      try self.tokens.insert(clip)
      published = true
      return ["audioId": id, "sampleRate": clip.sampleRate, "durationMs": clip.durationMs]
    }
  }

  func play(audioId: String, requestId: String, completion: @escaping Completion) {
    DispatchQueue.main.async {
      do {
        try self.requests.checkForeground()
        guard self.playback == nil else { throw SpeechAudioError("speech_playback_busy") }
        let clip = try self.tokens.consume(audioId: audioId, requestId: requestId)
        do {
          try clip.request.check()
          try self.sessionLease.prepare()
          let player = try AVAudioPlayer(contentsOf: clip.file)
          self.playback = Playback(clip: clip, player: player, completion: completion)
          player.delegate = self
          guard player.prepareToPlay(), player.play() else {
            self.finishPlayback(completed: false, error: SpeechAudioError("speech_playback_failed"))
            return
          }
        } catch {
          // player 构造失败时尚无 Playback，仍须归还已取得的 session lease。
          try? self.sessionLease.release()
          try? FileManager.default.removeItem(at: clip.file)
          throw error
        }
      } catch { completion(.failure(error)) }
    }
  }

  private func finishPlayback(completed: Bool, error: Error? = nil) {
    precondition(Thread.isMainThread)
    guard let current = playback else { return }
    playback = nil
    current.player.delegate = nil
    current.player.stop()
    try? FileManager.default.removeItem(at: current.clip.file)
    var completionError = error
    do { try sessionLease.release() }
    catch { if completionError == nil { completionError = error } }
    if let error = completionError { current.completion(.failure(error)) }
    else { current.completion(.success(["completed": completed && !current.clip.request.cancelled])) }
  }

  func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
    DispatchQueue.main.async {
      guard self.playback?.player === player else { return }
      self.finishPlayback(completed: flag,
        error: flag ? nil : SpeechAudioError("speech_playback_failed"))
    }
  }

  func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
    DispatchQueue.main.async {
      guard self.playback?.player === player else { return }
      self.finishPlayback(completed: false, error: SpeechAudioError("speech_playback_failed"))
    }
  }

  func cancel(requestId: String, completion: @escaping Completion) {
    // 取消只排主线程控制消息，绝不等待 ORT 队列或从另一线程销毁原生指针。
    DispatchQueue.main.async {
      do {
        let request = try self.requests.cancel(requestId)
        self.tokens.prune()
        if self.playback?.clip.request.id == request.id { self.finishPlayback(completed: false) }
        completion(.success(["cancelled": true]))
      } catch { completion(.failure(error)) }
    }
  }
}
