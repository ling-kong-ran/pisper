import Darwin
import Foundation
import SherpaOnnxC

// 配置和同步推理返回前均保持字符串有效，避免临时 NSString 的 utf8String 悬空。
private final class SpeechCStrings {
  private var pointers: [UnsafeMutablePointer<CChar>] = []
  func add(_ value: String) throws -> UnsafePointer<CChar> {
    guard !value.utf8.contains(0) else { throw SpeechAudioError("speech_catalog_invalid") }
    guard let pointer = value.withCString({ strdup($0) }) else {
      throw SpeechAudioError("speech_native_allocation_failed")
    }
    pointers.append(pointer)
    return UnsafePointer(pointer)
  }
  func release() { pointers.forEach { free($0) }; pointers.removeAll() }
}

enum SpeechPCM {
  static let maxSamples = 16_000 * 60
  static let maxBytes = maxSamples * 4
  static let maxBase64Characters = ((maxBytes + 2) / 3) * 4

  static func validateEncoded(_ encoded: String) throws {
    guard !encoded.isEmpty, encoded.utf8.count <= maxBase64Characters else {
      throw SpeechAudioError("speech_invalid_pcm")
    }
  }

  static func validateHotwords(_ hotwords: String) throws {
    guard hotwords.utf8.count <= 20 * 1024 else { throw SpeechAudioError("speech_invalid_hotwords") }
    if hotwords.isEmpty { return }
    guard !hotwords.unicodeScalars.contains(where: {
      ":#@/".unicodeScalars.contains($0)
        || ((CharacterSet.controlCharacters.contains($0) || CharacterSet.whitespacesAndNewlines.contains($0))
          && $0.value != 10 && $0.value != 32)
    }) else { throw SpeechAudioError("speech_invalid_hotwords") }
    let terms = hotwords.split(separator: "\n", omittingEmptySubsequences: false)
    guard terms.count <= 128, terms.allSatisfy({
      !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && $0.utf16.count <= 128
    }) else {
      throw SpeechAudioError("speech_invalid_hotwords")
    }
  }

  static func decode(_ encoded: String, check: () throws -> Void) throws -> [Float] {
    try validateEncoded(encoded)
    guard let data = Data(base64Encoded: encoded), !data.isEmpty,
      data.count <= maxBytes, data.count % 4 == 0 else { throw SpeechAudioError("speech_invalid_pcm") }
    var samples = [Float]()
    samples.reserveCapacity(data.count / 4)
    try data.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
      for index in stride(from: 0, to: bytes.count, by: 4) {
        if index % 32_000 == 0 { try check() }
        let byte0 = UInt32(bytes[index])
        let byte1 = UInt32(bytes[index + 1]) << 8
        let byte2 = UInt32(bytes[index + 2]) << 16
        let byte3 = UInt32(bytes[index + 3]) << 24
        let bits = byte0 | byte1 | byte2 | byte3
        let sample = Float(bitPattern: bits)
        guard sample.isFinite, (-1...1).contains(sample) else {
          throw SpeechAudioError("speech_invalid_pcm")
        }
        samples.append(sample)
      }
    }
    try check()
    return samples
  }
}

struct SpeechWaveResult {
  let sampleRate: Int
  let durationMs: Int
}

enum SpeechWave {
  static let maxSeconds = 45

  static func write(file: URL, samples: UnsafeBufferPointer<Float>, sampleRate: Int,
                    check: () throws -> Void) throws -> SpeechWaveResult {
    guard (8_000...48_000).contains(sampleRate), !samples.isEmpty,
      samples.count <= sampleRate * maxSeconds else {
      throw SpeechAudioError("speech_audio_duration_exceeded")
    }
    let durationMs = samples.count * 1000 / sampleRate
    guard durationMs > 0 else { throw SpeechAudioError("speech_audio_duration_exceeded") }
    try check()
    let output = try SpeechFiles.open(file, writable: true, exclusive: true)
    defer { try? output.close() }
    var header = Data()
    func u16(_ value: UInt16) { header.append(UInt8(truncatingIfNeeded: value)); header.append(UInt8(value >> 8)) }
    func u32(_ value: UInt32) {
      for shift in stride(from: 0, to: 32, by: 8) { header.append(UInt8(truncatingIfNeeded: value >> shift)) }
    }
    header.append(contentsOf: "RIFF".utf8); u32(UInt32(36 + samples.count * 2))
    header.append(contentsOf: "WAVEfmt ".utf8); u32(16); u16(1); u16(1)
    u32(UInt32(sampleRate)); u32(UInt32(sampleRate * 2)); u16(2); u16(16)
    header.append(contentsOf: "data".utf8); u32(UInt32(samples.count * 2))
    try output.write(contentsOf: header)
    var bytes = Data(capacity: 8192)
    for start in stride(from: 0, to: samples.count, by: 4096) {
      try check()
      bytes.removeAll(keepingCapacity: true)
      for index in start..<min(start + 4096, samples.count) {
        let sample = samples[index]
        guard sample.isFinite else { throw SpeechAudioError("speech_audio_invalid_sample") }
        // 与 Android roundToInt 一致：有限超幅样本饱和，恰好半步时向正方向取整。
        let bounded = min(max(sample, -1), 1)
        let value = Int16(floorf(bounded * 32767 + 0.5))
        let bits = UInt16(bitPattern: value)
        bytes.append(UInt8(truncatingIfNeeded: bits)); bytes.append(UInt8(bits >> 8))
      }
      try output.write(contentsOf: bytes)
    }
    try check()
    try output.synchronize()
    return SpeechWaveResult(sampleRate: sampleRate, durationMs: durationMs)
  }
}

private final class SpeechGenerationContext {
  let request: SpeechAudioRequest
  let maxSamples: Int
  var received = 0
  var failure: SpeechAudioError?
  init(request: SpeechAudioRequest, sampleRate: Int) {
    self.request = request; self.maxSamples = sampleRate * SpeechWave.maxSeconds
  }
  func accept(samples: UnsafePointer<Float>?, count: Int32, progress: Float) -> Int32 {
    if request.cancelled || failure != nil { return 0 }
    guard count >= 0, progress.isFinite, (0...1).contains(progress),
      Int(count) <= maxSamples - received, count == 0 || samples != nil else {
      failure = SpeechAudioError("speech_audio_duration_exceeded")
      return 0
    }
    if let samples {
      for index in 0..<Int(count) {
        if index % 4096 == 0 && request.cancelled { return 0 }
        guard samples[index].isFinite else {
          failure = SpeechAudioError("speech_audio_invalid_sample")
          return 0
        }
      }
    }
    received += Int(count)
    return 1
  }
}

private let speechGenerationCallback: @convention(c)
  (UnsafePointer<Float>?, Int32, Float, UnsafeMutableRawPointer?) -> Int32 = { samples, count, progress, raw in
    guard let raw else { return 0 }
    let context = Unmanaged<SpeechGenerationContext>.fromOpaque(raw).takeUnretainedValue()
    return context.accept(samples: samples, count: count, progress: progress)
  }

// 操作与回收只在所属 kind 队列执行；锁仅保护跨队列汇合读取的一次性结果。
// 不把 native/cache 声明为 Sendable，跨线程通道仅限此受控结果容器。
final class SpeechEngineTask: @unchecked Sendable {
  let queue: DispatchQueue
  private let lock = NSLock()
  private let operation: () throws -> (() -> Void)
  private var result: Result<() -> Void, Error>?

  init(queue: DispatchQueue, operation: @escaping () throws -> (() -> Void)) {
    self.queue = queue; self.operation = operation
  }

  func perform() {
    dispatchPrecondition(condition: .onQueue(queue))
    let completed = Result { try autoreleasepool { try operation() } }
    lock.lock()
    precondition(result == nil)
    result = completed
    lock.unlock()
  }

  var failure: Error? {
    lock.lock(); defer { lock.unlock() }
    guard let result else { return SpeechAudioError("speech_model_load_failed") }
    if case .failure(let error) = result { return error }
    return nil
  }

  func discard() {
    dispatchPrecondition(condition: .onQueue(queue))
    lock.lock()
    let completed = result
    result = nil
    lock.unlock()
    if case .success(let release)? = completed { release() }
  }
}

enum SpeechEnginePreparation {
  static func run(_ tasks: [SpeechEngineTask], completionQueue: DispatchQueue = .main,
                  check: @escaping () throws -> Void,
                  completion: @escaping (Result<Void, Error>) -> Void) {
    let group = DispatchGroup()
    for task in tasks {
      group.enter()
      task.queue.async {
        defer { group.leave() }
        task.perform()
      }
    }
    // 两项都结束后才发布或回收，不能因一项失败而丢弃另一项迟到的结果。
    group.notify(queue: completionQueue) {
      let errors = tasks.compactMap { $0.failure }
      var failure = errors.first(where: { SpeechAudioError.code($0) != "speech_cancelled" }) ?? errors.first
      do { try check() }
      catch { if failure == nil { failure = error } }
      guard let failure else { completion(.success(())); return }
      let cleanup = DispatchGroup()
      for task in tasks {
        cleanup.enter()
        task.queue.async {
          defer { cleanup.leave() }
          task.discard()
        }
      }
      cleanup.notify(queue: completionQueue) { completion(.failure(failure)) }
    }
  }
}

// 所有访问和定时回调均由所属 kind 的串行队列排序，生命周期控制线程只负责排队。
final class SpeechEngineCache<Key: Equatable, Engine> {
  private let create: (Key) throws -> Engine
  private let release: (Engine) -> Void
  private let schedule: (TimeInterval, @escaping () -> Void) -> (() -> Void)
  private let now: () -> TimeInterval
  private let sessionState: () -> (pinned: Bool, idleSince: TimeInterval?)
  private var key: Key?
  private var engine: Engine?
  private var cancelIdle: (() -> Void)?
  private var generation: UInt64 = 0
  private var ownership: UInt64 = 0
  private var usedAt: TimeInterval = 0

  init(create: @escaping (Key) throws -> Engine, release: @escaping (Engine) -> Void,
       sessionState: @escaping () -> (pinned: Bool, idleSince: TimeInterval?) = { (false, nil) },
       schedule: @escaping (TimeInterval, @escaping () -> Void) -> (() -> Void),
       now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
    self.create = create; self.release = release; self.schedule = schedule; self.now = now
    self.sessionState = sessionState
  }

  func invalidate() {
    ownership &+= 1
    generation &+= 1
    cancelIdle?()
    cancelIdle = nil
    let previous = engine
    engine = nil
    key = nil
    if let previous { release(previous) }
  }

  func touch() {
    guard engine != nil else { return }
    cancelIdle?()
    cancelIdle = nil
    let state = sessionState()
    usedAt = max(usedAt, state.idleSince ?? now())
    generation &+= 1
    guard !state.pinned else { return }
    let currentGeneration = generation
    cancelIdle = schedule(max(0, 30 - (now() - usedAt))) { [weak self] in
      guard let self, self.generation == currentGeneration, !self.sessionState().pinned else { return }
      // release 已发生但刷新消息还在队列中时，旧 timer 也必须遵守新的退出时间。
      if let idleSince = self.sessionState().idleSince, self.now() - idleSince < 30 {
        self.touch()
        return
      }
      self.invalidate()
    }
  }

  func prepare(_ configuration: Key, check: () throws -> Void) throws -> (() -> Void) {
    try check()
    let state = sessionState()
    let reused = engine != nil && key == configuration
      && (state.pinned || now() - max(state.idleSince ?? usedAt, usedAt) < 30)
    if reused {
      try check()
      ownership &+= 1
      touch()
      return {}
    }
    try use(configuration, markUsed: false) { _ in try check() }
    let revision = ownership
    return { [weak self] in
      // 只回收本次新建且未被后续任务接管的引擎，迟到失败不能销毁新会话正在复用的缓存。
      guard let self, self.ownership == revision else { return }
      self.invalidate()
    }
  }

  func use<Result>(_ configuration: Key, markUsed: Bool = true,
                   operation: (Engine) throws -> Result) throws -> Result {
    let state = sessionState()
    if engine != nil && (key != configuration || (!state.pinned && now() - max(state.idleSince ?? usedAt, usedAt) >= 30)) {
      invalidate()
    }
    cancelIdle?()
    cancelIdle = nil
    generation &+= 1
    ownership &+= 1
    do {
      let current: Engine
      if let engine { current = engine }
      else {
        current = try create(configuration)
        engine = current
        key = configuration
      }
      let result = try operation(current)
      // 无会话的旧调用仍按真实推理计时；持有 pin 的迟到工作不得推迟退出时间。
      if markUsed && !state.pinned { usedAt = now() }
      touch()
      return result
    } catch {
      invalidate()
      throw error
    }
  }
}

final class SpeechNativeTTS {
  let pointer: OpaquePointer
  let sampleRate: Int
  private let strings: SpeechCStrings

  init(config: SpeechTTSConfiguration) throws {
    let strings = SpeechCStrings()
    self.strings = strings
    var native = SherpaOnnxOfflineTtsConfig()
    do {
      native.model.vits.model = try strings.add(config.model)
      native.model.vits.tokens = try strings.add(config.tokens)
      native.model.vits.lexicon = try strings.add(config.lexicon)
      // 保留 ABI 字段与已验证安装树；Melo 实际分词仍使用 lexicon。
      native.model.vits.dict_dir = try strings.add(config.dictDir)
      native.model.vits.noise_scale = config.noiseScale
      native.model.vits.noise_scale_w = config.noiseScaleW
      native.model.vits.length_scale = config.lengthScale
      native.model.num_threads = Int32(config.numThreads)
      native.model.provider = try strings.add("cpu")
      native.rule_fsts = try strings.add(config.ruleFsts.joined(separator: ","))
      native.max_num_sentences = 1
      native.silence_scale = 0.2
      guard let tts = SherpaOnnxCreateOfflineTts(&native) else {
        throw SpeechAudioError("speech_model_load_failed")
      }
      let rate = Int(SherpaOnnxOfflineTtsSampleRate(tts))
      guard (8_000...48_000).contains(rate) else {
        SherpaOnnxDestroyOfflineTts(tts)
        throw SpeechAudioError("speech_audio_invalid_sample_rate")
      }
      pointer = tts
      sampleRate = rate
    } catch {
      strings.release()
      throw error
    }
  }

  func release() {
    SherpaOnnxDestroyOfflineTts(pointer)
    strings.release()
  }
}

// 热词内容属于每次 stream，只有解码模式和模型路径改变时重建 recognizer。
struct SpeechASREngineConfiguration: Equatable {
  let model: SpeechASRConfiguration
  let usesHotwords: Bool
}

final class SpeechNativeASR {
  let pointer: OpaquePointer
  private let strings: SpeechCStrings

  init(config: SpeechASREngineConfiguration) throws {
    guard !config.usesHotwords || !config.model.bpeVocab.isEmpty else {
      throw SpeechAudioError("speech_hotwords_unsupported")
    }
    let strings = SpeechCStrings()
    self.strings = strings
    do {
      let model = config.model
      var native = SherpaOnnxOnlineRecognizerConfig()
      native.feat_config.sample_rate = 16_000
      native.feat_config.feature_dim = 80
      native.model_config.transducer.encoder = try strings.add(model.encoder)
      native.model_config.transducer.decoder = try strings.add(model.decoder)
      native.model_config.transducer.joiner = try strings.add(model.joiner)
      native.model_config.tokens = try strings.add(model.tokens)
      native.model_config.num_threads = 1
      native.model_config.provider = try strings.add("cpu")
      native.model_config.modeling_unit = try strings.add(model.bpeVocab.isEmpty ? "" : "bpe")
      native.model_config.bpe_vocab = try strings.add(model.bpeVocab)
      native.decoding_method = try strings.add(config.usesHotwords ? "modified_beam_search" : "greedy_search")
      native.max_active_paths = 2
      native.hotwords_score = 1.5
      guard let recognizer = SherpaOnnxCreateOnlineRecognizer(&native) else {
        throw SpeechAudioError("speech_model_load_failed")
      }
      pointer = recognizer
    } catch {
      strings.release()
      throw error
    }
  }

  func release() {
    // 先销毁持有配置的 native，再释放所有 C 字符串。
    SherpaOnnxDestroyOnlineRecognizer(pointer)
    strings.release()
  }
}

enum SpeechNativeEngine {
  static func transcribe(config: SpeechASRConfiguration, samples: [Float], hotwords: String,
                         request: SpeechAudioRequest,
                         engines: SpeechEngineCache<SpeechASREngineConfiguration, SpeechNativeASR>) throws -> String {
    try request.check()
    guard !samples.isEmpty, samples.count <= SpeechPCM.maxSamples else {
      throw SpeechAudioError("speech_invalid_pcm")
    }
    let key = SpeechASREngineConfiguration(model: config, usesHotwords: !hotwords.isEmpty)
    return try engines.use(key) { engine in
      try transcribe(recognizer: engine.pointer, samples: samples, hotwords: hotwords, request: request)
    }
  }

  private static func transcribe(recognizer: OpaquePointer, samples: [Float], hotwords: String,
                                 request: SpeechAudioRequest) throws -> String {
    try request.check()
    let strings = SpeechCStrings()
    defer { strings.release() }
    let stream = hotwords.isEmpty ? SherpaOnnxCreateOnlineStream(recognizer)
      : SherpaOnnxCreateOnlineStreamWithHotwords(recognizer,
          try strings.add(hotwords.replacingOccurrences(of: "\n", with: "/")))
    guard let stream else { throw SpeechAudioError("speech_stream_create_failed") }
    defer { SherpaOnnxDestroyOnlineStream(stream) }
    func drain() throws {
      while SherpaOnnxIsOnlineStreamReady(recognizer, stream) != 0 {
        try request.check()
        SherpaOnnxDecodeOnlineStream(recognizer, stream)
      }
    }
    try samples.withUnsafeBufferPointer { buffer in
      guard let base = buffer.baseAddress else { throw SpeechAudioError("speech_invalid_pcm") }
      for start in stride(from: 0, to: buffer.count, by: 8000) {
        try request.check()
        SherpaOnnxOnlineStreamAcceptWaveform(stream, 16_000, base.advanced(by: start),
          Int32(min(8000, buffer.count - start)))
        try drain()
      }
    }
    try request.check()
    // 与 Android 使用同样尾部上下文，避免录音结束时漏掉末尾词。
    let tail = [Float](repeating: 0, count: 16_000)
    tail.withUnsafeBufferPointer { buffer in
      SherpaOnnxOnlineStreamAcceptWaveform(stream, 16_000, buffer.baseAddress, 16_000)
    }
    SherpaOnnxOnlineStreamInputFinished(stream)
    try drain()
    try request.check()
    guard let result = SherpaOnnxGetOnlineStreamResult(recognizer, stream) else {
      throw SpeechAudioError("speech_recognition_failed")
    }
    defer { SherpaOnnxDestroyOnlineRecognizerResult(result) }
    guard let text = result.pointee.text, let copied = String(validatingUTF8: text) else {
      throw SpeechAudioError("speech_recognition_failed")
    }
    return copied
  }

  static func synthesize(config: SpeechTTSConfiguration, text: String, file: URL,
                         request: SpeechAudioRequest,
                         engines: SpeechEngineCache<SpeechTTSConfiguration, SpeechNativeTTS>) throws -> SpeechWaveResult {
    try request.check()
    guard (1...400).contains(config.maxTextCodePoints), (1...16).contains(config.numThreads),
      config.speakerId == 0,
      [config.noiseScale, config.noiseScaleW, config.lengthScale].allSatisfy({ $0.isFinite && $0 > 0 })
      else { throw SpeechAudioError("speech_catalog_invalid") }
    guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      text.unicodeScalars.count <= config.maxTextCodePoints, !text.utf8.contains(0) else {
      throw SpeechAudioError("speech_invalid_text")
    }
    return try engines.use(config) { engine in
      let strings = SpeechCStrings()
      defer { strings.release() }
      let tts = engine.pointer
      let sampleRate = engine.sampleRate
      try request.check()
      guard config.speakerId < Int(SherpaOnnxOfflineTtsNumSpeakers(tts)) else {
        throw SpeechAudioError("speech_voice_unavailable")
      }
      var generation = SherpaOnnxGenerationConfig()
      generation.sid = Int32(config.speakerId)
      generation.speed = 1
      generation.silence_scale = 0.2
      let context = SpeechGenerationContext(request: request, sampleRate: sampleRate)
      let retained = Unmanaged.passRetained(context)
      defer { retained.release() }
      // VITS 回调仍是完整句子粒度；引擎缓存不改变播放合同，取消后不发布旧结果。
      let audio = SherpaOnnxOfflineTtsGenerateWithConfig(tts, try strings.add(text), &generation,
        speechGenerationCallback, retained.toOpaque())
      guard let audio else {
        try request.check()
        if let failure = context.failure { throw failure }
        throw SpeechAudioError("speech_synthesis_failed")
      }
      defer { SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio) }
      try request.check()
      if let failure = context.failure { throw failure }
      guard Int(audio.pointee.sample_rate) == sampleRate, audio.pointee.n > 0,
        Int(audio.pointee.n) <= sampleRate * SpeechWave.maxSeconds,
        let samples = audio.pointee.samples else {
        throw SpeechAudioError("speech_audio_duration_exceeded")
      }
      return try SpeechWave.write(file: file,
        samples: UnsafeBufferPointer(start: samples, count: Int(audio.pointee.n)),
        sampleRate: sampleRate, check: request.check)
    }
  }
}
