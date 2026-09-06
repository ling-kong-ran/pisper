// 语音对话模式编排 hook：把「采集 → 识别 → 发送 → 播报 → 再聆听」
// 串成一个可打断的循环。所有异步段都用 generation 计数防串台：
// 任意阶段被关闭/打断后，迟到的回调不得再写入新一轮状态。
import { useCallback, useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import type { ChatAttachment, ChatMessage } from '@/types/chat'
import {
  createSpeechRecognizer,
  requestMicrophonePermission,
  startMicrophoneCapture,
  type MicrophoneCapture,
  type SpeechRecognizer,
} from './voice-input'
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
  sendPrompt: SendPrompt
  onAbort: () => Promise<void> | void
  notifyError?: (message: string) => void
}

// TTS 朗读前把 Markdown 结构压平成自然语言，避免把符号逐个读出来。
function speakableText(markdown: string) {
  return markdown
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/^\s*[-*+]\s+/gm, '')
    .replace(/^\s*\d+\.\s+/gm, '')
    .replace(/[*_~>]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 1200)
}

export function useVoiceSession({
  open,
  sessionId,
  messages,
  sendPrompt,
  onAbort,
  notifyError,
}: VoiceSessionOptions) {
  const { t, language } = useI18n()
  const [stage, setStage] = useState<VoiceModeStage>('idle')
  const [level, setLevel] = useState(0)
  const [partial, setPartial] = useState('')
  const [turns, setTurns] = useState<VoiceTurn[]>([])
  const [error, setError] = useState('')
  const [muted, setMuted] = useState(false)
  const [elapsed, setElapsed] = useState(0)

  const stageRef = useRef<VoiceModeStage>('idle')
  const generationRef = useRef(0)
  const recognizerRef = useRef<SpeechRecognizer | null>(null)
  const captureRef = useRef<MicrophoneCapture | null>(null)
  const captureAbortRef = useRef<AbortController | null>(null)
  const unsubscribeRef = useRef<(() => void) | null>(null)
  const smootherRef = useRef(createLevelSmoother())
  const lastLevelPushAtRef = useRef(0)
  const messagesRef = useRef(messages)
  const turnsRef = useRef(turns)
  const autoListenRef = useRef(true)
  messagesRef.current = messages
  turnsRef.current = turns

  const setStageBoth = useCallback((next: VoiceModeStage) => {
    stageRef.current = next
    setStage(next)
  }, [])

  const releaseAudio = useCallback(async () => {
    unsubscribeRef.current?.()
    unsubscribeRef.current = null
    captureAbortRef.current?.abort()
    captureAbortRef.current = null
    const capture = captureRef.current
    captureRef.current = null
    const recognizer = recognizerRef.current
    recognizerRef.current = null
    await Promise.allSettled([capture?.stop(), recognizer?.cancel()])
  }, [])

  const stopSpeaking = useCallback(() => {
    try {
      window.speechSynthesis?.cancel()
    } catch {
      // 某些 WebView 没有语音合成实现，取消失败可忽略。
    }
  }, [])

  // 等待本轮 Agent 回复落盘：onSend 解析后 React 状态可能尚未回灌，
  // 轮询 messagesRef 直到出现基线之后的 assistant 文本（超时则放弃播报）。
  const waitForAgentReply = useCallback(async (baselineIds: Set<string>, generation: number) => {
    const deadline = Date.now() + 8_000
    while (Date.now() < deadline) {
      if (generation !== generationRef.current) return ''
      const reply = [...messagesRef.current]
        .reverse()
        .find((item) => item.role !== 'user' && !baselineIds.has(item.id) && item.text?.trim())
      const text = reply?.text?.trim()
      if (text) return text
      await new Promise((resolve) => window.setTimeout(resolve, 120))
    }
    return ''
  }, [])

  const speak = useCallback(
    (text: string, generation: number) =>
      new Promise<void>((resolve) => {
        const content = speakableText(text)
        if (!content || !window.speechSynthesis) {
          resolve()
          return
        }
        const utterance = new SpeechSynthesisUtterance(content)
        utterance.lang = language === 'zh-CN' ? 'zh-CN' : 'en-US'
        utterance.rate = 1.02
        const done = () => resolve()
        utterance.onend = done
        utterance.onerror = done
        // 防御：部分平台 onend 可能丢失，超时兜底保证循环能继续。
        const timeout = window.setTimeout(done, Math.max(6_000, content.length * 320))
        utterance.onend = () => {
          window.clearTimeout(timeout)
          done()
        }
        utterance.onerror = () => {
          window.clearTimeout(timeout)
          done()
        }
        if (generation === generationRef.current) window.speechSynthesis.speak(utterance)
        else {
          window.clearTimeout(timeout)
          resolve()
        }
      }),
    [language],
  )

  const startListening = useCallback(async () => {
    const generation = ++generationRef.current
    setError('')
    setPartial('')
    setLevel(0)
    smootherRef.current.reset()
    setStageBoth('requesting')

    const fail = (caught: unknown) => {
      if (generation !== generationRef.current) return
      const denied =
        caught instanceof DOMException && caught.name === 'NotAllowedError'
          ? true
          : String(caught instanceof Error ? caught.message : caught).includes(
              'microphone_permission_denied',
            )
      setError(
        denied
          ? t('chat:voiceInput.permissionDenied')
          : caught instanceof Error
            ? caught.message
            : t('chat:voiceMode.failed'),
      )
      setStageBoth('error')
    }

    try {
      await requestMicrophonePermission()
      if (generation !== generationRef.current) return
      const recognizer = createSpeechRecognizer({ chatSessionId: sessionId })
      recognizerRef.current = recognizer
      unsubscribeRef.current = recognizer.onPartial((text) => {
        if (generation === generationRef.current) setPartial(text)
      })
      void recognizer.start().catch(() => {})
      const controller = new AbortController()
      captureAbortRef.current = controller
      const capture = await startMicrophoneCapture((samples) => {
        if (generation !== generationRef.current) return
        // 电平仅驱动视觉，节流到 ~15Hz 足矣，避免 PCM 高频回调带崩渲染。
        const now = performance.now()
        if (now - lastLevelPushAtRef.current > 66) {
          lastLevelPushAtRef.current = now
          setLevel(smootherRef.current.push(pcmLevel(samples)))
        } else {
          smootherRef.current.push(pcmLevel(samples))
        }
        recognizer.acceptPcm(samples)
      }, controller.signal)
      if (generation !== generationRef.current) {
        await capture.stop()
        return
      }
      captureRef.current = capture
      setStageBoth('listening')
    } catch (caught) {
      await releaseAudio()
      fail(caught)
    }
  }, [releaseAudio, sessionId, setStageBoth, t])

  const finishTurn = useCallback(
    async (spoken: string, generation: number) => {
      const baselineIds = new Set(messagesRef.current.map((item) => item.id))
      const replyId = `voice-agent-${Date.now()}`
      setTurns((current) => [
        ...current.slice(-5),
        { id: replyId, role: 'agent', text: '', pending: true },
      ])
      setStageBoth('thinking')
      let reply = ''
      try {
        await sendPrompt(spoken, [], false, false, null)
        if (generation !== generationRef.current) return
        reply = await waitForAgentReply(baselineIds, generation)
      } catch (caught) {
        if (generation !== generationRef.current) return
        const message = caught instanceof Error ? caught.message : t('chat:voiceMode.failed')
        setTurns((current) => current.filter((turn) => turn.id !== replyId))
        setError(message)
        setStageBoth('error')
        notifyError?.(message)
        return
      }
      if (generation !== generationRef.current) return
      setTurns((current) =>
        current.map((turn) =>
          turn.id === replyId
            ? {
                ...turn,
                pending: false,
                text: reply || t('chat:voiceMode.emptyReply'),
              }
            : turn,
        ),
      )
      if (!reply) {
        // 没拿到可读回复时直接继续聆听，不打断对话节奏。
        if (autoListenRef.current) void startListening()
        else setStageBoth('idle')
        return
      }
      setStageBoth('speaking')
      await speak(reply, generation)
      if (generation !== generationRef.current) return
      if (autoListenRef.current && stageRef.current === 'speaking') void startListening()
      else setStageBoth('idle')
    },
    [notifyError, sendPrompt, setStageBoth, speak, startListening, t, waitForAgentReply],
  )

  // 结束当前聆听并进入发送管线；识别为空时回到聆听而不是报错打断节奏。
  const commitListening = useCallback(async () => {
    if (stageRef.current !== 'listening') return
    const generation = ++generationRef.current
    setStageBoth('transcribing')
    setLevel(0)
    const recognizer = recognizerRef.current
    try {
      const capture = captureRef.current
      captureRef.current = null
      captureAbortRef.current?.abort()
      captureAbortRef.current = null
      await capture?.stop()
      if (generation !== generationRef.current) return
      const text = ((await recognizer?.finish()) || '').trim()
      if (generation !== generationRef.current) return
      unsubscribeRef.current?.()
      unsubscribeRef.current = null
      recognizerRef.current = null
      setPartial('')
      if (!text) {
        void startListening()
        return
      }
      setTurns((current) => [
        ...current.slice(-5),
        { id: `voice-user-${Date.now()}`, role: 'user', text },
      ])
      await finishTurn(text, generation)
    } catch (caught) {
      if (generation !== generationRef.current) return
      await releaseAudio()
      setError(caught instanceof Error ? caught.message : t('chat:voiceMode.failed'))
      setStageBoth('error')
    }
  }, [finishTurn, releaseAudio, setStageBoth, startListening, t])

  // 主按钮行为：按当前阶段切换 聆听→提交 / 播报→打断重听 / 思考→中止。
  const toggleMain = useCallback(() => {
    const current = stageRef.current
    if (current === 'idle' || current === 'error') {
      autoListenRef.current = true
      setMuted(false)
      void startListening()
      return
    }
    if (current === 'listening') {
      void commitListening()
      return
    }
    if (current === 'speaking') {
      stopSpeaking()
      void startListening()
      return
    }
    if (current === 'thinking') {
      void onAbort()
      void startListening()
    }
  }, [commitListening, onAbort, startListening, stopSpeaking])

  // 静音 = 暂停自动聆听循环并停掉当前采集，浮层保持打开。
  const toggleMute = useCallback(() => {
    setMuted((current) => {
      const next = !current
      autoListenRef.current = !next
      if (next) {
        stopSpeaking()
        generationRef.current += 1
        void releaseAudio()
        setStageBoth('idle')
        setPartial('')
        setLevel(0)
      } else if (stageRef.current === 'idle' || stageRef.current === 'error') {
        void startListening()
      }
      return next
    })
  }, [releaseAudio, setStageBoth, startListening, stopSpeaking])

  const hangUp = useCallback(() => {
    generationRef.current += 1
    autoListenRef.current = false
    stopSpeaking()
    void releaseAudio()
    setStageBoth('idle')
    setPartial('')
    setLevel(0)
    setError('')
    setElapsed(0)
  }, [releaseAudio, setStageBoth, stopSpeaking])

  // 打开浮层时重置上下文；不自动开始聆听（等用户按「开始对话」）。
  // 关闭/换会话时彻底释放麦克风与语音合成。
  useEffect(() => {
    if (!open) return undefined
    setTurns([])
    setError('')
    setMuted(false)
    setElapsed(0)
    autoListenRef.current = true
    setStageBoth('idle')
    return () => {
      generationRef.current += 1
      stopSpeaking()
      void releaseAudio()
      setStageBoth('idle')
      setPartial('')
      setLevel(0)
    }
    // 仅在开关与会话切换时重建循环，回调引用变化不应重启麦克风。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, sessionId])

  // 通话计时：仅在对话活跃阶段走动，回到待机即停。
  useEffect(() => {
    const active =
      stage === 'listening' ||
      stage === 'transcribing' ||
      stage === 'thinking' ||
      stage === 'speaking'
    if (!active) return undefined
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
  }
}
