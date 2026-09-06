import { useCallback, useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { throwIfAborted } from '@/lib/abort-signal'
import type { ChatAttachment, ChatMessage } from '@/types/chat'
import {
  createSpeechRecognizer,
  requestMicrophonePermission,
  startMicrophoneCapture,
  VOICE_MAX_DURATION_SECONDS,
  type MicrophoneCapture,
  type SpeechRecognizer,
} from './voice-input'
import { createVoiceEndpoint, type VoiceEndpoint } from './voice-endpoint'
import { loadSpeechHotwords, prepareSpeechSession } from './speech-session'
import type { SpeechTextSource } from './speech-output'
import { createVoiceTextStream, subscribeVoiceResponse } from './voice-response-stream'
import {
  createLevelSmoother,
  pcmLevel,
  type VoiceModeStage,
  type VoiceTurn,
} from './voice-mode-state'

type SendPrompt = (
  value: string,
  attachments: ChatAttachment[],
  goalMode: boolean,
  teamMode: boolean,
  goalTokenBudget: number | null,
) => Promise<void> | void

type VoiceSessionOptions = {
  open: boolean
  sessionId: string
  messages: ChatMessage[]
  streaming?: boolean
  sendPrompt: SendPrompt
  onAbort: () => Promise<void> | void
  notifyError?: (message: string) => void
  ensureReady?: (signal: AbortSignal) => Promise<void>
  speakText: (text: SpeechTextSource, signal: AbortSignal, onSpeaking?: () => void) => Promise<void>
}

type Round = {
  generation: number
  sessionId: string
  controller: AbortController
  microphone: AbortController
  recognizer?: SpeechRecognizer
  capture?: MicrophoneCapture
  capturePending?: Promise<MicrophoneCapture>
  captureStopping?: Promise<void>
  recognizerDisposing?: Promise<void>
  endpoint?: VoiceEndpoint
  unsubscribe?: () => void
  limitTimer?: number
  permissionPending: boolean
  committed: boolean
  commitRequested: boolean
  ownsRun: boolean
  baseline?: Set<string>
  prompt?: string
  abortRun?: VoiceSessionOptions['onAbort']
  cleanup?: Promise<void>
  playback?: Promise<void>
  responseSubscription?: () => void
  checkOwnership?: () => void
}

// 每轮独立持有资源，旧初始化失败或迟到设备授权只能清理自己的上下文。
export function useVoiceSession(options: VoiceSessionOptions) {
  const { t } = useI18n()
  const latest = useRef({ ...options, t })
  latest.current = { ...options, t }
  const [stage, setStage] = useState<VoiceModeStage>('idle')
  const [level, setLevel] = useState(0)
  const [partial, setPartial] = useState('')
  const [turns, setTurns] = useState<VoiceTurn[]>([])
  const [error, setError] = useState('')
  const [muted, setMuted] = useState(false)
  const [elapsed, setElapsed] = useState(0)
  const stageRef = useRef<VoiceModeStage>('idle')
  const generation = useRef(0)
  const roundRef = useRef<Round | null>(null)
  const releaseBarrier = useRef(Promise.resolve())
  const ready = useRef(false)
  const speechSession = useRef<{
    controller: AbortController
    preparing: Promise<void>
  } | null>(null)
  const active = useRef(false)
  const mutedRef = useRef(false)
  const autoListen = useRef(false)
  const actions = useRef({ start: async () => {}, commit: async () => {}, stop: () => {} })

  const transition = useCallback((next: VoiceModeStage) => {
    stageRef.current = next
    setStage(next)
  }, [])
  const isCurrent = (round: Round) =>
    active.current &&
    latest.current.open &&
    round.sessionId === latest.current.sessionId &&
    roundRef.current === round &&
    generation.current === round.generation &&
    !round.controller.signal.aborted
  const externalStreaming = () =>
    Boolean(
      latest.current.streaming || latest.current.messages.some((message) => message.streaming),
    )

  function releaseSpeechSession() {
    const session = speechSession.current
    speechSession.current = null
    session?.controller.abort()
  }

  function ensureSpeechSession() {
    if (speechSession.current) return speechSession.current.preparing
    const controller = new AbortController()
    const chatSessionId = latest.current.sessionId
    const preparing = (async () => {
      if (!ready.current) {
        await latest.current.ensureReady?.(controller.signal)
        throwIfAborted(controller.signal)
        ready.current = true
      }
      const { hotwords } = await loadSpeechHotwords(chatSessionId, controller.signal)
      await prepareSpeechSession({ kinds: ['asr', 'tts'], hotwords }, controller.signal)
      throwIfAborted(controller.signal)
    })().catch((caught: unknown) => {
      if (speechSession.current?.controller === controller) speechSession.current = null
      controller.abort()
      throw caught
    })
    // 会话预热不归某一轮所有；静音、思考及轮次取消不能启动模型空闲淘汰。
    speechSession.current = { controller, preparing }
    return preparing
  }

  function stopCapture(round: Round) {
    window.clearTimeout(round.limitTimer)
    round.limitTimer = undefined
    round.microphone.abort()
    round.endpoint?.dispose()
    round.endpoint = undefined
    const capture = round.capture
    round.capture = undefined
    if (capture) round.captureStopping = capture.stop()
    return round.captureStopping ?? Promise.resolve()
  }

  function disposeRound(round: Round) {
    if (round.cleanup) return round.cleanup
    round.controller.abort()
    round.unsubscribe?.()
    round.unsubscribe = undefined
    round.responseSubscription?.()
    round.responseSubscription = undefined
    round.checkOwnership = undefined
    const captureStopped = stopCapture(round)
    const recognizer = round.recognizer
    round.recognizer = undefined
    // 先同步停麦和播放器，再等待清理；新一轮必须等旧音频上下文关闭。
    round.cleanup = Promise.allSettled([
      captureStopped,
      round.capturePending,
      round.recognizerDisposing,
      round.playback,
      recognizer?.dispose(),
    ]).then(() => {})
    return round.cleanup
  }

  function cancelRound() {
    generation.current += 1
    const round = roundRef.current
    roundRef.current = null
    if (!round) return
    let aborting: Promise<void> = Promise.resolve()
    const newUsers = latest.current.messages.filter(
      (message) => message.role === 'user' && !round.baseline?.has(message.id),
    )
    const anotherRun =
      latest.current.sessionId === round.sessionId &&
      (newUsers.length > 1 || (newUsers.length === 1 && newUsers[0].text?.trim() !== round.prompt))
    if (round.ownsRun && !anotherRun) {
      round.ownsRun = false
      try {
        aborting = Promise.resolve(round.abortRun?.()).catch(() => {})
      } catch {
        // 终止失败不应阻止本地释放麦克风和播放器。
      }
    }
    releaseBarrier.current = Promise.allSettled([
      releaseBarrier.current,
      disposeRound(round),
      aborting,
    ]).then(() => {})
  }

  function stop() {
    autoListen.current = false
    cancelRound()
    transition('idle')
    setPartial('')
    setLevel(0)
    setTurns((current) => current.filter((turn) => !turn.pending))
  }

  function fail(round: Round, caught: unknown) {
    if (!isCurrent(round)) return
    const message =
      caught instanceof Error ? caught.message : latest.current.t('chat:voiceMode.failed')
    const denied =
      (caught instanceof DOMException && caught.name === 'NotAllowedError') ||
      message.includes('microphone_permission_denied')
    const text = denied ? latest.current.t('chat:voiceInput.permissionDenied') : message
    stop()
    setError(text)
    transition('error')
    latest.current.notifyError?.(text)
  }

  function waitForRender(round: Round) {
    return new Promise<void>((resolve) => {
      const signal = round.controller.signal
      const done = () => {
        window.clearTimeout(timer)
        signal.removeEventListener('abort', done)
        resolve()
      }
      const timer = window.setTimeout(done, 50)
      signal.addEventListener('abort', done, { once: true })
      if (signal.aborted) done()
    })
  }

  async function streamReply(round: Round, baseline: Set<string>, replyId: string) {
    const signal = round.controller.signal
    const stream = createVoiceTextStream(signal)
    let messageId: string | undefined
    let runId: string | undefined
    let startedAt: string | undefined
    let recovering = false
    let snapshotStreaming = false
    let receivedSSE = false
    let receivedTerminal = false
    let sendingFinished = false
    let deadline = Infinity
    const hasForeignUser = (messages: ChatMessage[]) => {
      const users = messages.filter(
        (message) => message.role === 'user' && !baseline.has(message.id),
      )
      return users.length > 1 || (users.length === 1 && users[0].text?.trim() !== round.prompt)
    }
    round.checkOwnership = () => {
      if (!isCurrent(round)) return
      if (hasForeignUser(latest.current.messages)) {
        round.ownsRun = false
        fail(round, new Error(latest.current.t('chat:voiceMode.failed')))
      }
    }
    const update = (text: string) => {
      stream.update(text)
      setTurns((current) =>
        current.map((turn) =>
          turn.id === replyId ? { ...turn, text, pending: !text.trim() } : turn,
        ),
      )
    }
    round.responseSubscription = subscribeVoiceResponse(round.sessionId, (event) => {
      if (!isCurrent(round) || (event.source !== 'snapshot' && baseline.has(event.messageId)))
        return
      try {
        const snapshot = event.source === 'snapshot'
        const sameSnapshotRun = Boolean(startedAt && event.startedAt === startedAt)
        if (
          (event.users && hasForeignUser(event.users)) ||
          (event.prompt !== undefined && event.prompt.trim() !== round.prompt) ||
          (runId && event.runId && runId !== event.runId) ||
          (startedAt && event.startedAt && startedAt !== event.startedAt) ||
          (snapshot &&
            !sameSnapshotRun &&
            !(receivedTerminal && !event.startedAt && event.status === 'completed')) ||
          (!snapshot && messageId && messageId !== event.messageId)
        ) {
          round.ownsRun = false
          throw new Error(latest.current.t('chat:voiceMode.failed'))
        }
        // 已收齐的文本不再回灌，但新 run 的身份检查必须保留到尾音结束。
        if (receivedTerminal && snapshot) return
        runId ||= event.runId
        startedAt ||= event.startedAt
        if (event.messageId) messageId = event.messageId
        if (event.status === 'recovering') {
          recovering = true
          return
        }
        if (snapshot) {
          recovering = true
          snapshotStreaming = event.status === 'streaming' || event.status === 'started'
        }
        if (event.status === 'started') return
        receivedSSE = true
        if (event.status === 'failed') {
          round.ownsRun = false
          throw new Error(event.error || latest.current.t('chat:voiceMode.failed'))
        }
        update(event.text)
        if (event.status === 'completed') {
          receivedTerminal = true
          round.ownsRun = false
          stream.finish()
        }
      } catch (caught) {
        stream.finish(caught)
        fail(round, caught)
      }
    })
    const sending = Promise.resolve()
      .then(() => {
        throwIfAborted(signal)
        return latest.current.sendPrompt(round.prompt!, [], false, false, null)
      })
      .catch((caught) => {
        round.ownsRun = false
        stream.finish(caught)
        throw caught
      })
      .finally(() => {
        // send 可能仅结束 SSE 传输；已确认的 run 直到终态仍归本轮所有。
        if (!receivedSSE && !runId && !startedAt) round.ownsRun = false
        sendingFinished = true
        deadline = Date.now() + 15_000
      })
    const synchronize = async () => {
      while (isCurrent(round)) {
        await waitForRender(round)
        if (!isCurrent(round)) return
        const added = latest.current.messages.filter((message) => !baseline.has(message.id))
        const users = added.filter((message) => message.role === 'user')
        if (users.length > 1 || (users.length === 1 && users[0].text?.trim() !== round.prompt))
          throw new Error(latest.current.t('chat:voiceMode.failed'))
        const replies = added.filter(
          (message) => message.role === 'assistant' || message.role === 'agent',
        )
        const failed = replies.find((message) => message.error)
        if (failed) throw new Error(String(failed.error))
        // 完成事件已校验本轮归属；持久化回灌会替换临时消息 ID，不能再次覆盖或取消已收齐的语音流。
        if (receivedTerminal && sendingFinished && !externalStreaming()) return
        const reply = replies.at(-1)
        // 重连快照和无 SSE 的回复仍可回灌；不让落后的打字机文本覆盖直接收到的增量。
        if (
          !recovering &&
          reply?.text &&
          (!receivedSSE || (sendingFinished && !externalStreaming()))
        ) {
          if (messageId && messageId !== reply.id)
            throw new Error(latest.current.t('chat:voiceMode.failed'))
          messageId = reply.id
          update(reply.text)
        }
        if (!recovering && sendingFinished && !externalStreaming() && reply?.text?.trim()) {
          round.ownsRun = false
          stream.finish()
          return
        }
        // 活跃运行没有“等待渲染”期限；仅在传输结算且没有运行现场时等待有限时间。
        if (sendingFinished && (externalStreaming() || snapshotStreaming))
          deadline = Date.now() + 15_000
        if (Date.now() >= deadline) throw new Error(latest.current.t('chat:voiceMode.emptyReply'))
      }
    }
    try {
      round.playback = Promise.resolve().then(() =>
        latest.current.speakText(stream, signal, () => {
          if (isCurrent(round)) transition('speaking')
        }),
      )
      await Promise.all([sending, synchronize(), round.playback])
    } catch (caught) {
      stream.finish(caught)
      throw caught
    } finally {
      round.responseSubscription?.()
      round.responseSubscription = undefined
      round.checkOwnership = undefined
      stream.finish()
    }
  }

  async function commit() {
    const round = roundRef.current
    if (!round || !isCurrent(round) || round.committed || stageRef.current !== 'listening') return
    round.committed = true
    const heardSpeech = round.endpoint?.hasSpeech
    transition('transcribing')
    setLevel(0)
    try {
      await stopCapture(round)
      if (!isCurrent(round)) return
      // 未确认人声时不调用 ASR finish，避免静音转写幻觉触发自动提示词。
      if (!heardSpeech) {
        stop()
        return
      }
      const text = (await round.recognizer?.finish())?.trim()
      if (!isCurrent(round)) return
      round.unsubscribe?.()
      round.unsubscribe = undefined
      const recognizer = round.recognizer
      round.recognizer = undefined
      round.recognizerDisposing = recognizer?.dispose()
      await round.recognizerDisposing
      if (!isCurrent(round)) return
      setPartial('')
      if (!text) throw new Error(latest.current.t('chat:voiceMode.emptyReply'))
      if (externalStreaming()) {
        stop()
        return
      }
      const baseline = new Set(latest.current.messages.map((message) => message.id))
      round.baseline = baseline
      round.prompt = text
      const replyId = `voice-agent-${round.generation}`
      setTurns((current) => [
        ...current.slice(-4),
        { id: `voice-user-${round.generation}`, role: 'user', text },
        { id: replyId, role: 'agent', text: '', pending: true },
      ])
      transition('thinking')
      round.abortRun = latest.current.onAbort
      round.ownsRun = true
      await streamReply(round, baseline, replyId)
      if (!isCurrent(round)) return
      if (autoListen.current && !mutedRef.current) {
        transition('idle')
        await actions.current.start()
      } else stop()
    } catch (caught) {
      fail(round, caught)
    }
  }

  async function start() {
    if (
      !active.current ||
      !latest.current.open ||
      mutedRef.current ||
      document.hidden ||
      !['idle', 'error', 'speaking'].includes(stageRef.current)
    )
      return
    cancelRound()
    const round: Round = {
      generation: ++generation.current,
      sessionId: latest.current.sessionId,
      controller: new AbortController(),
      microphone: new AbortController(),
      permissionPending: false,
      committed: false,
      commitRequested: false,
      ownsRun: false,
    }
    roundRef.current = round
    autoListen.current = true
    setError('')
    setPartial('')
    setLevel(0)
    transition('requesting')
    const smoother = createLevelSmoother()
    let lastLevelPush = 0
    try {
      await releaseBarrier.current
      if (!isCurrent(round)) return
      await ensureSpeechSession()
      if (!isCurrent(round)) return
      if (externalStreaming()) {
        stop()
        return
      }
      round.permissionPending = true
      try {
        await requestMicrophonePermission()
      } finally {
        round.permissionPending = false
      }
      if (!isCurrent(round)) return
      if (document.hidden || externalStreaming()) {
        stop()
        return
      }
      const endpoint = await createVoiceEndpoint()
      if (!isCurrent(round)) {
        endpoint.dispose()
        return
      }
      round.endpoint = endpoint
      const recognizer = createSpeechRecognizer({ chatSessionId: round.sessionId })
      round.recognizer = recognizer
      round.unsubscribe = recognizer.onPartial((text) => {
        if (isCurrent(round)) setPartial(text)
      })
      // 等初始化明确成功才开麦，失败直接退出，不留下热麦或吞掉拒绝。
      await recognizer.start()
      if (!isCurrent(round)) return
      round.capturePending = startMicrophoneCapture((samples) => {
        if (!isCurrent(round) || round.committed || round.microphone.signal.aborted) return
        try {
          const visualLevel = smoother.push(pcmLevel(samples))
          const now = performance.now()
          if (now - lastLevelPush >= 66) {
            lastLevelPush = now
            setLevel(visualLevel)
          }
          // 尾静音必须先进入识别器，VAD 只负责决定何时停止，不裁剪 PCM。
          const full = recognizer.acceptPcm(samples)
          const ended = endpoint.acceptPcm(samples)
          if (full || ended) {
            round.commitRequested = true
            if (stageRef.current === 'listening') void actions.current.commit()
            else round.microphone.abort()
          }
        } catch (caught) {
          fail(round, caught)
        }
      }, round.microphone.signal).then(async (capture) => {
        if (!isCurrent(round) || round.microphone.signal.aborted) await capture.stop()
        else round.capture = capture
        return capture
      })
      await round.capturePending
      if (!isCurrent(round)) return
      transition('listening')
      if (round.commitRequested) {
        await actions.current.commit()
        return
      }
      round.limitTimer = window.setTimeout(
        () => void actions.current.commit(),
        VOICE_MAX_DURATION_SECONDS * 1000,
      )
    } catch (caught) {
      if (isCurrent(round) && round.commitRequested && round.microphone.signal.aborted) {
        transition('listening')
        await actions.current.commit()
      } else fail(round, caught)
    }
  }
  actions.current = { start, commit, stop }

  const commitListening = useCallback(() => actions.current.commit(), [])
  const interrupt = useCallback(() => {
    actions.current.stop()
    const stoppedGeneration = generation.current
    // abort 会异步回灌 streaming=false，结算前不能开启新一轮或抢占外部运行。
    void releaseBarrier.current.then(() => {
      if (generation.current === stoppedGeneration && active.current) void actions.current.start()
    })
  }, [])
  const toggleMain = useCallback(() => {
    if (stageRef.current === 'listening') void actions.current.commit()
    else if (stageRef.current === 'thinking' || stageRef.current === 'speaking') interrupt()
    else if (stageRef.current === 'idle' || stageRef.current === 'error') {
      mutedRef.current = false
      setMuted(false)
      void actions.current.start()
    }
  }, [interrupt])
  const toggleMute = useCallback(() => {
    const next = !mutedRef.current
    mutedRef.current = next
    setMuted(next)
    if (next) actions.current.stop()
    else void actions.current.start()
  }, [])
  const hangUp = useCallback(() => {
    releaseSpeechSession()
    actions.current.stop()
    setError('')
    setElapsed(0)
  }, [])

  useEffect(() => {
    if (!options.open) return
    active.current = true
    ready.current = false
    mutedRef.current = false
    setMuted(false)
    setTurns([])
    setElapsed(0)
    transition('idle')
    void actions.current.start()
    const background = () => {
      const round = roundRef.current
      // 移动端原生授权弹窗可能暂时隐藏 WebView，只在等待该权限结果时豁免。
      if (round?.permissionPending && window.__PISPER_MOBILE_APP__) return
      releaseSpeechSession()
      actions.current.stop()
    }
    const visibility = () => {
      if (document.hidden) background()
    }
    const interrupted = () => {
      releaseSpeechSession()
      actions.current.stop()
    }
    window.addEventListener('blur', background)
    window.addEventListener('pisper:speech-interrupted', interrupted)
    document.addEventListener('visibilitychange', visibility)
    return () => {
      active.current = false
      releaseSpeechSession()
      actions.current.stop()
      window.removeEventListener('blur', background)
      window.removeEventListener('pisper:speech-interrupted', interrupted)
      document.removeEventListener('visibilitychange', visibility)
    }
  }, [options.open, options.sessionId, transition])

  useEffect(() => {
    // 文本流终结后仍可能有尾音；消息变化必须继续撤销旧轮，不能等新回复首字才停播。
    roundRef.current?.checkOwnership?.()
  }, [options.messages, options.streaming])

  useEffect(() => {
    if (!['listening', 'transcribing', 'thinking', 'speaking'].includes(stage)) return
    const timer = window.setInterval(() => setElapsed((value) => value + 1), 1000)
    return () => window.clearInterval(timer)
  }, [stage])

  return {
    stage,
    level,
    partial,
    turns,
    error,
    muted,
    elapsed,
    toggleMain,
    toggleMute,
    hangUp,
    commitListening,
    interrupt,
  }
}
