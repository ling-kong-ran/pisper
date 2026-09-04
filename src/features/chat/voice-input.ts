import { waitForMobileRuntimeReady } from '@/lib/http'

export const VOICE_SAMPLE_RATE = 16_000
export const VOICE_MAX_DURATION_SECONDS = 60
const VOICE_MAX_SAMPLES = VOICE_SAMPLE_RATE * VOICE_MAX_DURATION_SECONDS

type PartialListener = (text: string) => void

export type SpeechRecognizer = {
  start: () => Promise<void>
  // 返回 true 时移动端一次性缓存已满，调用方必须立即停止采集以限制内存与推理成本。
  acceptPcm: (samples: Float32Array) => boolean
  onPartial: (listener: PartialListener) => () => void
  finish: () => Promise<string>
  cancel: () => Promise<void>
  dispose: () => Promise<void>
}

export type MicrophoneCapture = {
  stop: () => Promise<void>
}

function mobileInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('移动端原生桥不可用。'))
  return invoke<T>(command, args)
}

export async function requestMicrophonePermission() {
  if (typeof window === 'undefined' || !window.__PISPER_MOBILE_APP__) return
  const result = await mobileInvoke<{ state?: string }>('mobile_request_microphone_permission')
  if (result.state !== 'granted') throw new Error('microphone_permission_denied')
}

async function responseError(response: Response) {
  const body = await response.text().catch(() => '')
  try {
    const payload = JSON.parse(body) as { error?: string }
    return payload.error || `语音转写失败 (${response.status})`
  } catch {
    return body || `语音转写失败 (${response.status})`
  }
}

function mergeChunks(chunks: Float32Array[]) {
  const length = chunks.reduce((total, chunk) => total + chunk.length, 0)
  const merged = new Float32Array(length)
  let offset = 0
  for (const chunk of chunks) {
    merged.set(chunk, offset)
    offset += chunk.length
  }
  return merged
}

// 桌面走 Runtime 流式会话：录音中周期性追加 PCM 并拿回部分文本；
// 移动端原生桥每次调用都会重建识别器，保持停止后一次性转写。
class RuntimeSpeechRecognizer implements SpeechRecognizer {
  private chunks: Float32Array[] = []
  private pending: Float32Array[] = []
  private listeners = new Set<PartialListener>()
  private controller: AbortController | null = null
  private sessionId = ''
  private legacy = false
  private flushTimer = 0
  private sendChain: Promise<void> = Promise.resolve()
  private finished = false
  private retainedSamples = 0

  async start() {
    this.chunks = []
    this.pending = []
    this.finished = false
    this.retainedSamples = 0
    this.controller = new AbortController()
    if (window.__PISPER_MOBILE_APP__) return
    await waitForMobileRuntimeReady()
    const response = await fetch('/api/speech/stream/start', {
      method: 'POST',
      signal: this.controller.signal,
    })
    if (!response.ok) {
      // 旧版 Runtime 没有流式接口，降级为停止后一次性转写。
      if (response.status === 404) {
        this.legacy = true
        return
      }
      throw new Error(await responseError(response))
    }
    const payload = (await response.json()) as { id?: string }
    if (!payload.id) throw new Error('语音识别会话创建失败。')
    this.sessionId = payload.id
    this.flushTimer = window.setInterval(() => void this.flushPending(), 1_000)
  }

  acceptPcm(samples: Float32Array) {
    if (!samples.length || this.finished) return false
    if (window.__PISPER_MOBILE_APP__) {
      const remaining = VOICE_MAX_SAMPLES - this.retainedSamples
      if (remaining <= 0) return true
      const copy = samples.slice(0, remaining)
      this.chunks.push(copy)
      this.retainedSamples += copy.length
      return this.retainedSamples >= VOICE_MAX_SAMPLES
    }

    const copy = samples.slice()
    if (this.sessionId && !this.legacy) this.pending.push(copy)
    else this.chunks.push(copy)
    return false
  }

  onPartial(listener: PartialListener) {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  private emitPartial(text: string) {
    if (!text) return
    for (const listener of this.listeners) listener(text)
  }

  private flushPending() {
    // 串行发送，保证 chunk 顺序与识别器喂入顺序一致。
    this.sendChain = this.sendChain.then(async () => {
      if (!this.sessionId || this.legacy || this.finished || !this.pending.length) return
      const samples = mergeChunks(this.pending.splice(0))
      try {
        const response = await fetch('/api/speech/stream/chunk', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/octet-stream',
            'X-Pisper-Sample-Rate': String(VOICE_SAMPLE_RATE),
            'X-Pisper-Speech-Session': this.sessionId,
          },
          body: samples.buffer as ArrayBuffer,
          signal: this.controller?.signal,
        })
        if (!response.ok) return
        const payload = (await response.json()) as { text?: string }
        if (typeof payload.text === 'string') this.emitPartial(payload.text.trim())
      } catch {
        // 部分结果失败不影响录音，下一轮 flush 会继续带上后续音频。
      }
    })
    return this.sendChain
  }

  private async finishStreaming() {
    window.clearInterval(this.flushTimer)
    this.flushTimer = 0
    // 先把剩余待发音频送入会话，再标记结束，避免尾部语音丢失。
    await this.flushPending()
    this.finished = true
    const response = await fetch('/api/speech/stream/finish', {
      method: 'POST',
      headers: { 'X-Pisper-Speech-Session': this.sessionId },
      signal: this.controller?.signal,
    })
    if (!response.ok) throw new Error(await responseError(response))
    const payload = (await response.json()) as { text?: string }
    const text = typeof payload.text === 'string' ? payload.text.trim() : ''
    if (!text) throw new Error('未识别到语音内容。')
    this.emitPartial(text)
    return text
  }

  private async finishOneShot(samples: Float32Array) {
    if (window.__PISPER_MOBILE_APP__) {
      const bytes = new Uint8Array(samples.buffer)
      let binary = ''
      const blockSize = 0x8000
      for (let offset = 0; offset < bytes.length; offset += blockSize) {
        binary += String.fromCharCode(...bytes.subarray(offset, offset + blockSize))
      }
      const payload = await mobileInvoke<{ text?: string }>('mobile_transcribe_pcm', {
        pcmBase64: btoa(binary),
      })
      const text = typeof payload.text === 'string' ? payload.text.trim() : ''
      if (!text) throw new Error('未识别到语音内容。')
      this.emitPartial(text)
      return text
    }

    const response = await fetch('/api/speech/transcribe', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/octet-stream',
        'X-Pisper-Sample-Rate': String(VOICE_SAMPLE_RATE),
      },
      body: samples.buffer as ArrayBuffer,
      signal: this.controller?.signal,
    })
    if (!response.ok) throw new Error(await responseError(response))
    const payload = (await response.json()) as { text?: string }
    const text = typeof payload.text === 'string' ? payload.text.trim() : ''
    if (!text) throw new Error('未识别到语音内容。')
    this.emitPartial(text)
    return text
  }

  async finish() {
    if (!this.controller) throw new Error('语音识别尚未启动。')
    if (this.sessionId && !this.legacy) return this.finishStreaming()
    this.finished = true
    const samples = mergeChunks(this.chunks)
    if (!samples.length) throw new Error('没有采集到语音。')
    return this.finishOneShot(samples)
  }

  async cancel() {
    this.finished = true
    window.clearInterval(this.flushTimer)
    this.flushTimer = 0
    this.controller?.abort()
    this.controller = null
    this.chunks = []
    this.pending = []
    this.retainedSamples = 0
    if (this.sessionId && !this.legacy) {
      const sessionId = this.sessionId
      this.sessionId = ''
      // 会话清理不阻塞取消操作，失败由服务端 TTL 兜底。
      await fetch('/api/speech/stream/cancel', {
        method: 'POST',
        headers: { 'X-Pisper-Speech-Session': sessionId },
      }).catch(() => {})
    }
  }

  async dispose() {
    await this.cancel()
    this.listeners.clear()
  }
}

export function createSpeechRecognizer(): SpeechRecognizer {
  return new RuntimeSpeechRecognizer()
}

const VOICE_WORKLET_JS = `
class PisperVoiceInputProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super()
    this.targetSampleRate = Number(options?.processorOptions?.targetSampleRate) || 16000
    this.inputSampleRate = sampleRate
    this.ratio = this.inputSampleRate / this.targetSampleRate
    this.inputBuffer = []
    this.readPosition = 0
    this.outputBuffer = []
  }

  process(inputs, outputs) {
    const channel = inputs[0]?.[0]
    if (channel?.length) {
      for (const sample of channel) this.inputBuffer.push(sample)
      while (this.readPosition + 1 < this.inputBuffer.length) {
        const index = Math.floor(this.readPosition)
        const fraction = this.readPosition - index
        const current = this.inputBuffer[index]
        const next = this.inputBuffer[index + 1]
        this.outputBuffer.push(current + (next - current) * fraction)
        this.readPosition += this.ratio
      }
      const consumed = Math.floor(this.readPosition)
      if (consumed > 0) {
        this.inputBuffer = this.inputBuffer.slice(consumed)
        this.readPosition -= consumed
      }
      while (this.outputBuffer.length >= 320) {
        const chunk = new Float32Array(this.outputBuffer.splice(0, 320))
        this.port.postMessage(chunk.buffer, [chunk.buffer])
      }
    }
    const output = outputs[0]?.[0]
    if (output) output.fill(0)
    return true
  }
}
registerProcessor('pisper-voice-input', PisperVoiceInputProcessor)
`

async function loadVoiceWorklet(context: AudioContext) {
  const blob = new Blob([VOICE_WORKLET_JS], { type: 'application/javascript' })
  const url = URL.createObjectURL(blob)
  try {
    await context.audioWorklet.addModule(url)
  } finally {
    URL.revokeObjectURL(url)
  }
}

export async function startMicrophoneCapture(onPcm: (samples: Float32Array) => void) {
  if (!navigator.mediaDevices?.getUserMedia) throw new Error('当前环境不支持麦克风采集。')
  if (!window.AudioContext && !window.webkitAudioContext)
    throw new Error('当前环境不支持音频处理。')

  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
    video: false,
  })
  const AudioContextConstructor = window.AudioContext || window.webkitAudioContext
  const context = new AudioContextConstructor({ latencyHint: 'interactive' })
  let processor: AudioWorkletNode | null = null
  let source: MediaStreamAudioSourceNode | null = null
  let gain: GainNode | null = null

  try {
    await loadVoiceWorklet(context)
    source = context.createMediaStreamSource(stream)
    processor = new AudioWorkletNode(context, 'pisper-voice-input', {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      processorOptions: { targetSampleRate: VOICE_SAMPLE_RATE },
    })
    gain = context.createGain()
    gain.gain.value = 0
    processor.port.onmessage = (event: MessageEvent<ArrayBuffer>) => {
      onPcm(new Float32Array(event.data))
    }
    source.connect(processor)
    processor.connect(gain)
    gain.connect(context.destination)
    await context.resume()
  } catch (error) {
    for (const track of stream.getTracks()) track.stop()
    await context.close().catch(() => {})
    throw error
  }

  let stopped = false
  return {
    async stop() {
      if (stopped) return
      stopped = true
      processor?.port.close()
      source?.disconnect()
      processor?.disconnect()
      gain?.disconnect()
      for (const track of stream.getTracks()) track.stop()
      await context.close().catch(() => {})
    },
  } satisfies MicrophoneCapture
}

declare global {
  interface Window {
    webkitAudioContext?: typeof AudioContext
  }
}
