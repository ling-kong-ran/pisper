import { waitForMobileRuntimeReady } from '@/lib/http'

export const VOICE_SAMPLE_RATE = 16_000

type PartialListener = (text: string) => void

export type SpeechRecognizer = {
  start: () => Promise<void>
  acceptPcm: (samples: Float32Array) => void
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
  if (result.state !== 'granted') throw new Error('麦克风权限未授予。')
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

class RuntimeSpeechRecognizer implements SpeechRecognizer {
  private chunks: Float32Array[] = []
  private listeners = new Set<PartialListener>()
  private controller: AbortController | null = null

  async start() {
    this.chunks = []
    this.controller = new AbortController()
  }

  acceptPcm(samples: Float32Array) {
    if (samples.length) this.chunks.push(samples.slice())
  }

  onPartial(listener: PartialListener) {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  async finish() {
    if (!this.controller) throw new Error('语音识别尚未启动。')
    const length = this.chunks.reduce((total, chunk) => total + chunk.length, 0)
    if (!length) throw new Error('没有采集到语音。')
    const samples = new Float32Array(length)
    let offset = 0
    for (const chunk of this.chunks) {
      samples.set(chunk, offset)
      offset += chunk.length
    }

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
      for (const listener of this.listeners) listener(text)
      return text
    }

    await waitForMobileRuntimeReady()
    const response = await fetch('/api/speech/transcribe', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/octet-stream',
        'X-Pisper-Sample-Rate': String(VOICE_SAMPLE_RATE),
      },
      body: samples,
      signal: this.controller.signal,
    })
    if (!response.ok) throw new Error(await responseError(response))
    const payload = (await response.json()) as { text?: string }
    const text = typeof payload.text === 'string' ? payload.text.trim() : ''
    if (!text) throw new Error('未识别到语音内容。')
    for (const listener of this.listeners) listener(text)
    return text
  }

  async cancel() {
    this.controller?.abort()
    this.controller = null
    this.chunks = []
  }

  async dispose() {
    await this.cancel()
    this.listeners.clear()
  }
}

export function createSpeechRecognizer(): SpeechRecognizer {
  return new RuntimeSpeechRecognizer()
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
    await context.audioWorklet.addModule(new URL('./voice-input-worklet.ts', import.meta.url))
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
