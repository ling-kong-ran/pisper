import { apiJson } from '@/lib/api'
import { abortReason, createAbortScope, throwIfAborted } from '@/lib/abort-signal'
import { invokeLocalSpeech } from './speech-models'

type NativeAudio = { audioId: string; sampleRate: number; durationMs: number }
type PreparedAudio = { requestId: string; native?: NativeAudio; wav?: ArrayBuffer }
const MAX_WAVE_BYTES = 8 * 1024 * 1024

function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  throwIfAborted(signal)
  return new Promise<T>((resolve, reject) => {
    const abort = () => reject(abortReason(signal))
    signal.addEventListener('abort', abort, { once: true })
    promise.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}

// 引擎设计为同一时刻只允许一个语音操作（ASR/TTS 互斥，由服务端测试锁定）。
// 对话轮次切换时 ASR 收尾与 TTS 预取可能短暂重叠，409 busy 属瞬态：
// 在预取侧做有限次退避重试，而不是让整句播报失败。
const BUSY_RETRY_LIMIT = 10
const BUSY_RETRY_DELAY_MS = 500

async function synthesizeWithBusyRetry(
  segment: string,
  voiceId: string,
  requestId: string,
  signal: AbortSignal,
): Promise<Response> {
  for (let attempt = 0; ; attempt += 1) {
    const response = await fetch('/api/speech/synthesize', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: segment, voiceId, requestId }),
      signal,
    })
    if (response.status !== 409 || attempt >= BUSY_RETRY_LIMIT) return response
    const payload = (await response.json().catch(() => ({}))) as { error?: string }
    if (!payload.error || !/busy/i.test(payload.error)) return response
    // 等待可被外部打断（用户插话/挂断），挂起期间不占用请求名额。
    await abortable(
      new Promise<void>((resolve) => setTimeout(resolve, BUSY_RETRY_DELAY_MS)),
      signal,
    )
  }
}

async function waveBytes(response: Response) {
  if (!response.ok) {
    const payload = (await response.json().catch(() => ({}))) as { error?: string }
    throw new Error(payload.error || `Speech synthesis failed (${response.status}).`)
  }
  if (!response.headers.get('content-type')?.startsWith('audio/wav') || !response.body)
    throw new Error('Invalid speech audio response.')
  const reader = response.body.getReader()
  const chunks: Uint8Array[] = []
  let bytes = 0
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      bytes += value.byteLength
      if (bytes > MAX_WAVE_BYTES) throw new Error('Speech audio exceeds the supported limit.')
      chunks.push(value)
    }
  } finally {
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
  if (bytes < 44) throw new Error('Speech audio is empty.')
  const result = new Uint8Array(bytes)
  let offset = 0
  for (const chunk of chunks) {
    result.set(chunk, offset)
    offset += chunk.length
  }
  return result.buffer
}

export type SpeechTextSource = string | AsyncIterable<string>

export async function playLocalSpeech(
  text: SpeechTextSource,
  voiceId: string,
  outerSignal: AbortSignal,
  onSpeaking?: () => void,
) {
  throwIfAborted(outerSignal)
  const lifetime = createAbortScope(outerSignal)
  try {
    await playSpeech(text, voiceId, lifetime, onSpeaking)
  } finally {
    lifetime.abort(new DOMException('Speech playback ended.', 'AbortError'))
    lifetime.dispose()
  }
}

async function playSpeech(
  text: SpeechTextSource,
  voiceId: string,
  lifetime: ReturnType<typeof createAbortScope>,
  onSpeaking?: () => void,
) {
  const { signal } = lifetime
  const { speechSegments } = await abortable(import('./speech-text'), signal)
  throwIfAborted(signal)
  const segments =
    typeof text === 'string'
      ? (async function* () {
          yield* speechSegments(text)
        })()
      : (await abortable(import('./speech-stream-text'), signal)).streamingSpeechSegments(
          text,
          signal,
        )
  const iterator = segments[Symbol.asyncIterator]()
  const native = Boolean(window.__PISPER_MOBILE_APP__)
  const requests = new Set<string>()
  let context: AudioContext | undefined
  let source: AudioBufferSourceNode | undefined
  let rejectPlayback: ((error: unknown) => void) | undefined
  const cancel = (requestId: string) => {
    const task = native
      ? invokeLocalSpeech('mobile_cancel_speech', { requestId })
      : apiJson('/api/speech/cancel', { method: 'POST', body: { requestId }, timeout: 3000 })
    void task.catch(() => {})
  }
  const abort = () => {
    for (const requestId of requests) cancel(requestId)
    try {
      source?.stop()
    } catch {
      /* 尚未启动或已结束的节点无需再次停止。 */
    }
    rejectPlayback?.(signal.reason || new DOMException('Speech playback cancelled.', 'AbortError'))
  }
  signal.addEventListener('abort', abort, { once: true })

  const prepare = (segment: string): Promise<PreparedAudio> => {
    throwIfAborted(signal)
    const requestId = crypto.randomUUID()
    requests.add(requestId)
    const task = (async () => {
      if (native) {
        const result = await invokeLocalSpeech<NativeAudio>('mobile_synthesize_speech', {
          text: segment,
          voiceId,
          requestId,
        })
        if (signal.aborted) {
          cancel(requestId)
          throwIfAborted(signal)
        }
        if (
          !result ||
          typeof result.audioId !== 'string' ||
          !result.audioId.trim() ||
          result.audioId.length > 128 ||
          !Number.isInteger(result.sampleRate) ||
          result.sampleRate < 8000 ||
          result.sampleRate > 48000 ||
          !Number.isFinite(result.durationMs) ||
          result.durationMs <= 0 ||
          result.durationMs > 45_000
        )
          throw new Error('Invalid native speech audio.')
        return { requestId, native: result }
      }
      const request = createAbortScope(signal, 60_000)
      try {
        const response = await synthesizeWithBusyRetry(segment, voiceId, requestId, request.signal)
        return { requestId, wav: await waveBytes(response) }
      } finally {
        request.dispose()
      }
    })()
    // 预取下一句可能先于当前播放失败，先安装拒绝处理器，错误在轮到该句时仍会抛出。
    task.catch(() => {})
    return task
  }

  try {
    if (!native) {
      context = new AudioContext({ sampleRate: 24_000 })
      let timer: ReturnType<typeof setTimeout> | undefined
      try {
        await abortable(
          Promise.race([
            context.resume(),
            new Promise<never>((_, reject) => {
              timer = setTimeout(() => reject(new Error('Audio playback is unavailable.')), 5000)
            }),
          ]),
          signal,
        )
      } finally {
        clearTimeout(timer)
      }
    }
    const prepareNext = async () => {
      const item = await abortable(iterator.next(), signal)
      return item.done ? undefined : prepare(item.value)
    }
    let next = prepareNext()
    let played = false
    for (;;) {
      const audio = await abortable(next, signal)
      throwIfAborted(signal)
      if (!audio) break
      next = prepareNext()
      next.catch(() => {})
      played = true
      onSpeaking?.()
      if (audio.native) {
        const result = await abortable(
          invokeLocalSpeech<{ completed: boolean }>('mobile_play_speech', {
            audioId: audio.native.audioId,
            requestId: audio.requestId,
          }),
          signal,
        )
        if (result?.completed !== true) throw new Error('Speech playback was interrupted.')
      } else {
        const buffer = await abortable(context!.decodeAudioData(audio.wav!), signal)
        throwIfAborted(signal)
        if (
          buffer.numberOfChannels !== 1 ||
          !Number.isFinite(buffer.duration) ||
          buffer.duration <= 0 ||
          buffer.duration > 45
        )
          throw new Error('Invalid speech audio duration.')
        const node = context!.createBufferSource()
        source = node
        node.buffer = buffer
        node.connect(context!.destination)
        try {
          await new Promise<void>((resolve, reject) => {
            rejectPlayback = reject
            node.onended = () => resolve()
            node.start()
          })
        } finally {
          rejectPlayback = undefined
          node.onended = null
          node.disconnect()
          source = undefined
        }
      }
      requests.delete(audio.requestId)
      throwIfAborted(signal)
    }
    if (!played) throw new Error('The reply has no speakable text.')
  } finally {
    signal.removeEventListener('abort', abort)
    // 错误退出同样作废预取，迟到的原生结果仍需按自身请求再次释放。
    lifetime.abort(new DOMException('Speech playback ended.', 'AbortError'))
    void iterator.return?.(undefined).catch(() => {})
    for (const requestId of requests) cancel(requestId)
    try {
      source?.stop()
    } catch {
      /* 关闭只影响当前播放，不触及其他 AudioContext。 */
    }
    await context?.close().catch(() => {})
  }
}
