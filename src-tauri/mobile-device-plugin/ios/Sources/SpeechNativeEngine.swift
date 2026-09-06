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

enum SpeechNativeEngine {
  static func transcribe(config: SpeechASRConfiguration, samples: [Float], hotwords: String,
                         request: SpeechAudioRequest) throws -> String {
    try request.check()
    guard !samples.isEmpty, samples.count <= SpeechPCM.maxSamples else {
      throw SpeechAudioError("speech_invalid_pcm")
    }
    guard hotwords.isEmpty || !config.bpeVocab.isEmpty else {
      throw SpeechAudioError("speech_hotwords_unsupported")
    }
    let strings = SpeechCStrings()
    defer { strings.release() }
    var native = SherpaOnnxOnlineRecognizerConfig()
    native.feat_config.sample_rate = 16_000
    native.feat_config.feature_dim = 80
    native.model_config.transducer.encoder = try strings.add(config.encoder)
    native.model_config.transducer.decoder = try strings.add(config.decoder)
    native.model_config.transducer.joiner = try strings.add(config.joiner)
    native.model_config.tokens = try strings.add(config.tokens)
    native.model_config.num_threads = 1
    native.model_config.provider = try strings.add("cpu")
    native.model_config.modeling_unit = try strings.add(config.bpeVocab.isEmpty ? "" : "bpe")
    native.model_config.bpe_vocab = try strings.add(config.bpeVocab)
    native.decoding_method = try strings.add(hotwords.isEmpty ? "greedy_search" : "modified_beam_search")
    native.max_active_paths = 2
    native.hotwords_score = 1.5
    guard let recognizer = SherpaOnnxCreateOnlineRecognizer(&native) else {
      throw SpeechAudioError("speech_model_load_failed")
    }
    defer { SherpaOnnxDestroyOnlineRecognizer(recognizer) }
    try request.check()
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
                         request: SpeechAudioRequest) throws -> SpeechWaveResult {
    try request.check()
    guard (1...400).contains(config.maxTextCodePoints), (1...16).contains(config.numThreads),
      config.speakerId == 0,
      [config.noiseScale, config.noiseScaleW, config.lengthScale].allSatisfy({ $0.isFinite && $0 > 0 })
      else { throw SpeechAudioError("speech_catalog_invalid") }
    guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
      text.unicodeScalars.count <= config.maxTextCodePoints, !text.utf8.contains(0) else {
      throw SpeechAudioError("speech_invalid_text")
    }
    let strings = SpeechCStrings()
    defer { strings.release() }
    var native = SherpaOnnxOfflineTtsConfig()
    native.model.vits.model = try strings.add(config.model)
    native.model.vits.tokens = try strings.add(config.tokens)
    native.model.vits.lexicon = try strings.add(config.lexicon)
    // 1.13.7 保留该 ABI 字段；实际 Melo 分词使用 lexicon，路径仍绑定已验证安装树。
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
    defer { SherpaOnnxDestroyOfflineTts(tts) }
    try request.check()
    let sampleRate = Int(SherpaOnnxOfflineTtsSampleRate(tts))
    guard (8_000...48_000).contains(sampleRate) else { throw SpeechAudioError("speech_audio_invalid_sample_rate") }
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
    // 回调只能返回 0 作废生成；同步 Generate 返回后才能逆序释放 audio、context、tts。
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
