import { apiJson, consumeEventStream } from '@/lib/api'
import { abortReason, createAbortScope, throwIfAborted } from '@/lib/abort-signal'
import { speechHotwords } from '@shared/speech-terms.mjs'
import { invokeLocalSpeech, type SpeechModelKind } from './speech-models'

export type SpeechSessionOptions = {
  kinds: readonly SpeechModelKind[]
  hotwords?: string
  voiceId?: string
}

export type SpeechSessionLease = {
  requestId: string
  signal: AbortSignal
}

export async function loadSpeechHotwords(chatSessionId: string, signal: AbortSignal) {
  const request = createAbortScope(signal, 30_000)
  try {
    const payload = await apiJson<{ terms: string[] }>(
      `/api/speech/terms?sessionId=${encodeURIComponent(chatSessionId)}`,
      { signal: request.signal },
    )
    throwIfAborted(request.signal)
    if (!Array.isArray(payload.terms) || payload.terms.some((term) => typeof term !== 'string'))
      throw new Error('语音术语响应无效。')
    return { terms: payload.terms, hotwords: speechHotwords(payload.terms) }
  } finally {
    request.dispose()
  }
}

// ready 仅结束初始化等待；连接与原生 pin 归外层 signal 所有，不能随单轮识别结束释放。
export async function prepareSpeechSession(
  options: SpeechSessionOptions,
  signal: AbortSignal,
): Promise<SpeechSessionLease> {
  throwIfAborted(signal)
  const requestId = crypto.randomUUID()
  const args = { requestId, ...options, kinds: [...options.kinds] }
  const native = Boolean(window.__PISPER_MOBILE_APP__)
  let modulesReady = !options.kinds.includes('tts')
  const lifetime = createAbortScope(signal)
  const initialization = createAbortScope(lifetime.signal, 60_000)
  let ready = false
  let modelReady = false
  let resolveReady!: () => void
  let rejectReady!: (error: unknown) => void
  const prepared = new Promise<void>((resolve, reject) => {
    resolveReady = resolve
    rejectReady = reject
  })
  const release = () => {
    // 原生释放也可能迟到失败；只清理本 lease，始终观察拒绝，不能阻塞退出。
    void invokeLocalSpeech('mobile_release_speech_session', { requestId }).catch(() => {})
  }
  const dispose = () => {
    // scope 会在 abort 时自动清理桥接；这里也显式移除 helper 自己的监听并覆盖失败退出。
    initialization.signal.removeEventListener('abort', timeout)
    lifetime.signal.removeEventListener('abort', abort)
    initialization.dispose()
    lifetime.dispose()
  }
  const abort = () => {
    rejectReady(abortReason(lifetime.signal))
    dispose()
    if (native) release()
  }
  const timeout = () => lifetime.abort(abortReason(initialization.signal))
  initialization.signal.addEventListener('abort', timeout, { once: true })
  lifetime.signal.addEventListener('abort', abort, { once: true })
  const finishReady = () => {
    throwIfAborted(lifetime.signal)
    if (!modelReady || !modulesReady || ready) return
    ready = true
    initialization.signal.removeEventListener('abort', timeout)
    initialization.dispose()
    resolveReady()
  }
  const fail = (error: unknown) => {
    if (lifetime.signal.aborted) return
    rejectReady(error)
    lifetime.abort(error)
    // 已 ready 后断流必须撤销正在录音/对话的界面，不能留下假活跃状态。
    if (ready) window.dispatchEvent(new Event('pisper:speech-interrupted'))
  }
  const markReady = (value: unknown) => {
    if (!value || typeof value !== 'object' || !('ready' in value) || value.ready !== true)
      throw new Error('Invalid speech session readiness.')
    modelReady = true
    finishReady()
  }
  // JS 模块与原生模型同时预热，避免首个 SSE 短语再承担动态导入等待。
  if (!modulesReady)
    void Promise.all([import('./speech-text'), import('./speech-stream-text')])
      .then(() => {
        modulesReady = true
        finishReady()
      })
      .catch(fail)
  const run = async () => {
    if (native) {
      try {
        markReady(await invokeLocalSpeech('mobile_prepare_speech_session', args))
      } finally {
        // 取消先于不可中断的原生预热完成时，再释放一次，避免迟到创建的 pin 遗留。
        if (lifetime.signal.aborted) release()
      }
      return
    }
    const response = await fetch('/api/speech/session', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Accept: 'text/event-stream' },
      body: JSON.stringify(args),
      signal: lifetime.signal,
    })
    await consumeEventStream<{ ready?: boolean; error?: string }>(response, (event, data) => {
      throwIfAborted(lifetime.signal)
      if (event === 'ready') markReady(data)
      else if (event === 'error') throw new Error(data.error || 'Speech session failed.')
    })
    throw new Error('Speech session connection was interrupted.')
  }
  void run().catch(fail)
  try {
    await prepared
    throwIfAborted(lifetime.signal)
    return { requestId, signal: lifetime.signal }
  } catch (error) {
    lifetime.abort(error)
    dispose()
    throw error
  }
}
