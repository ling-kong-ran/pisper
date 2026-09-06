import type { VAD } from '@ozymandiasthegreat/vad'

const SAMPLE_RATE = 16_000
const FRAME_MS = 20
const FRAME_SAMPLES = (SAMPLE_RATE * FRAME_MS) / 1000

type VadHandle = Pick<VAD, 'processFrame' | 'destroy'>

export type VoiceEndpointOptions = {
  onsetMs?: number
  minVoicedMs?: number
  silenceMs?: number
  mode?: 0 | 1 | 2 | 3
}

export type VoiceEndpoint = {
  acceptPcm: (samples: Float32Array) => boolean
  readonly hasSpeech: boolean
  reset: () => void
  dispose: () => void
}

// 工厂可替换以确定性验证帧边界，生产环境始终使用 libfvad 的真实分类器。
export function createVoiceEndpointAdapter(
  createVad: () => VadHandle,
  { onsetMs = 80, minVoicedMs = 160, silenceMs = 900 }: VoiceEndpointOptions = {},
): VoiceEndpoint {
  for (const value of [onsetMs, minVoicedMs, silenceMs]) {
    if (!Number.isFinite(value) || value <= 0) throw new Error('Invalid VAD endpoint duration')
  }
  let vad = createVad()
  // 库内部忽略 byteOffset，必须传独立完整缓冲，不能传大数组的 subarray。
  const frame = new Int16Array(FRAME_SAMPLES)
  let buffered = 0
  let consecutiveVoice = 0
  let voiced = 0
  let silence = 0
  let onset = false
  let ended = false
  let disposed = false
  const hasSpeech = () => onset && voiced * FRAME_MS >= minVoicedMs
  return {
    get hasSpeech() {
      return hasSpeech()
    },
    acceptPcm(samples) {
      if (disposed || ended) return false
      for (let index = 0; index < samples.length; index += 1) {
        const sample = Number.isFinite(samples[index])
          ? Math.max(-1, Math.min(1, samples[index]))
          : 0
        frame[buffered++] = sample * (sample < 0 ? 32768 : 32767)
        if (buffered !== FRAME_SAMPLES) continue
        buffered = 0
        const result = vad.processFrame(frame)
        if (result < 0) throw new Error('VAD frame processing failed')
        if (result === 1) {
          consecutiveVoice += 1
          voiced += 1
          silence = 0
          if (consecutiveVoice * FRAME_MS >= onsetMs) onset = true
        } else {
          consecutiveVoice = 0
          silence += 1
          if (!onset) voiced = 0
          if (silence * FRAME_MS >= silenceMs) {
            if (hasSpeech()) {
              ended = true
              return true
            }
            onset = false
            voiced = 0
          }
        }
      }
      return false
    },
    reset() {
      if (disposed) return
      vad.destroy()
      vad = createVad()
      buffered = consecutiveVoice = voiced = silence = 0
      onset = ended = false
      frame.fill(0)
    },
    dispose() {
      if (disposed) return
      disposed = true
      buffered = 0
      vad.destroy()
    },
  }
}

export async function createVoiceEndpoint(options: VoiceEndpointOptions = {}) {
  const { default: buildVad, VADMode } = await import('@ozymandiasthegreat/vad')
  const Vad = await buildVad()
  return createVoiceEndpointAdapter(
    () => new Vad(options.mode ?? VADMode.AGGRESSIVE, SAMPLE_RATE),
    options,
  )
}
