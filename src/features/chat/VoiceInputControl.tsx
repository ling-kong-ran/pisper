// 语音输入按钮（去面板化）：点击麦克风直接开录，识别文本实时写入输入框，
// 说话停顿由能量 VAD 自动完成转写；按钮本体承担全部状态表达
// （录音=红色脉冲，转写=转圈，失败=红色+tooltip），不再有悬浮面板。
import { LoaderCircle, Mic, MicOff } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useI18n } from '@/app/use-i18n'
import { formatShortcut } from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'
import { useVoiceShortcut } from './use-voice-shortcut'
import { createLevelSmoother, pcmLevel } from './voice-mode-state'
import { createVoiceEndpoint, type VoiceEndpoint } from './voice-endpoint'
import { useSpeechModels } from './use-speech-models'
import { SpeechModelsDialog } from './SpeechModelsDialog'
import {
  createSpeechRecognizer,
  requestMicrophonePermission,
  startMicrophoneCapture,
  VOICE_MAX_DURATION_SECONDS,
  type MicrophoneCapture,
  type SpeechRecognizer,
} from './voice-input'

// 声波柱数量与渲染节奏：足够密才有「流淌」感，~20fps 足够顺滑。
const WAVE_BAR_COUNT = 32
const WAVE_TICK_MS = 50

type VoiceStage = 'idle' | 'requesting' | 'recording' | 'transcribing' | 'error'

function errorMessage(error: unknown, fallback: string) {
  if (error instanceof DOMException && error.name === 'NotAllowedError') return 'permission'
  const message =
    typeof error === 'string'
      ? error
      : error instanceof Error
        ? error.message
        : typeof error === 'object' && error && 'message' in error
          ? String(error.message)
          : ''
  if (message.includes('microphone_permission_denied')) return 'permission'
  return message || fallback
}

export function VoiceInputControl({
  onInsert,
  onLiveText,
  sessionId,
  disabled = false,
  shortcutEnabled = false,
}: {
  onInsert: (text: string) => void
  // 实时部分转写：频繁回调写入草稿；传 null 表示录音取消/失败，调用方回滚到录音前文本。
  onLiveText?: (text: string | null) => void
  sessionId?: string
  disabled?: boolean
  shortcutEnabled?: boolean
}) {
  const { t } = useI18n()
  // 语音输入只需 ASR 模型；未安装时 ensureReady 会弹出下载引导并等待。
  const speechModels = useSpeechModels(['asr'])
  const voiceShortcut = useShortcutStore((state) => state.bindings.voiceInput)
  const [stage, setStage] = useState<VoiceStage>('idle')
  const [error, setError] = useState('')
  const stageRef = useRef<VoiceStage>('idle')
  const updateStage = (next: VoiceStage) => {
    stageRef.current = next
    setStage(next)
  }
  const recognizerRef = useRef<SpeechRecognizer | null>(null)
  const captureRef = useRef<MicrophoneCapture | null>(null)
  const captureControllerRef = useRef<AbortController | null>(null)
  const unsubscribeRef = useRef<(() => void) | null>(null)
  const operationRef = useRef(0)
  const recordingLimitTimerRef = useRef(0)
  const stopRecordingRef = useRef<() => void>(() => {})
  const stoppingRef = useRef(false)
  const keyboardOperationRef = useRef<number | null>(null)
  // Android 原生授权弹窗会短暂隐藏页面；触控授权阶段标记该操作，豁免 blur/hidden 取消。
  const permissionOperationRef = useRef<number | null>(null)
  const closeRef = useRef<() => void>(() => {})
  const errorTimerRef = useRef(0)
  const smootherRef = useRef(createLevelSmoother())
  const levelRef = useRef(0) // 录音回调里的最新平滑电平，波形条按固定节奏消费
  const [bars, setBars] = useState<number[]>([])
  const barStateRef = useRef<number[]>([]) // 逐柱平滑高度状态
  // 每轮录音独立的端点检测器（libfvad），停顿确认后自动完成转写。
  const endpointRef = useRef<VoiceEndpoint | null>(null)
  // 最新回调走 ref，音频回调里拿到的永远是最新闭包。
  const onInsertRef = useRef(onInsert)
  const onLiveTextRef = useRef(onLiveText)
  onInsertRef.current = onInsert
  onLiveTextRef.current = onLiveText

  const clearRecordingLimitTimer = () => {
    window.clearTimeout(recordingLimitTimerRef.current)
    recordingLimitTimerRef.current = 0
  }

  const releaseResources = async () => {
    clearRecordingLimitTimer()
    const captureController = captureControllerRef.current
    captureControllerRef.current = null
    captureController?.abort()
    unsubscribeRef.current?.()
    unsubscribeRef.current = null
    endpointRef.current?.dispose()
    endpointRef.current = null
    const capture = captureRef.current
    captureRef.current = null
    const recognizer = recognizerRef.current
    recognizerRef.current = null
    // 先摘下本轮引用，再等待清理，避免关闭后立即重试时释放掉新一轮资源。
    await Promise.allSettled([capture?.stop(), recognizer?.dispose()])
  }

  const showError = (message: string) => {
    // 控制台留完整诊断，胶囊里只放用户可读的一句。
    console.warn('[voice-input]', message)
    setError(message)
    updateStage('error')
    // 错误态只作短暂提示，几秒后自动回到待机。
    window.clearTimeout(errorTimerRef.current)
    errorTimerRef.current = window.setTimeout(() => {
      if (stageRef.current === 'error') {
        setError('')
        updateStage('idle')
      }
    }, 4_000)
  }

  const reset = () => {
    clearRecordingLimitTimer()
    stoppingRef.current = false
    updateStage('idle')
    setError('')
  }

  const close = () => {
    keyboardOperationRef.current = null
    permissionOperationRef.current = null
    operationRef.current += 1
    void releaseResources()
    // 取消录音：实时文本已写入草稿，通知调用方回滚。
    onLiveTextRef.current?.(null)
    reset()
  }

  closeRef.current = close

  useEffect(() => {
    const cancel = () => {
      // 移动端授权弹窗也会暂时隐藏页面；仅触控授权阶段等待结果，按住快捷键仍须立即取消。
      if (
        permissionOperationRef.current === operationRef.current &&
        keyboardOperationRef.current !== operationRef.current
      )
        return
      closeRef.current()
    }
    const visibility = () => {
      if (document.visibilityState === 'hidden') cancel()
    }
    const interrupted = () => closeRef.current()
    window.addEventListener('blur', cancel)
    window.addEventListener('pisper:speech-interrupted', interrupted)
    document.addEventListener('visibilitychange', visibility)
    return () => {
      window.removeEventListener('blur', cancel)
      window.removeEventListener('pisper:speech-interrupted', interrupted)
      document.removeEventListener('visibilitychange', visibility)
      window.clearTimeout(errorTimerRef.current)
      cancel()
    }
  }, [sessionId, shortcutEnabled, disabled])

  // 录音中驱动声波胶囊：镜像包络 + 逐柱平滑追踪（攻击快释放慢）+ 空闲涟漪底波。
  useEffect(() => {
    if (stage !== 'recording') {
      setBars([])
      barStateRef.current = []
      return undefined
    }
    const tick = () => {
      const time = performance.now() / 1000
      const level = levelRef.current
      const previous = barStateRef.current
      const next: number[] = []
      for (let index = 0; index < WAVE_BAR_COUNT; index += 1) {
        const centered = (index / (WAVE_BAR_COUNT - 1)) * 2 - 1 // -1…1
        const mirror = Math.cos((centered * Math.PI) / 2) ** 1.3 // 中央高两端低的包络
        const idle =
          0.1 +
          0.06 * Math.sin(time * 2.2 + index * 0.55) +
          0.04 * Math.sin(time * 3.7 + index * 0.21)
        const jitter = 0.75 + 0.5 * Math.abs(Math.sin(time * 9 + index * 1.7))
        const target = Math.min(1, Math.max(0.05, idle + level * mirror * jitter * 1.15))
        const smoothed = previous[index] ?? target
        const k = target > smoothed ? 0.55 : 0.28
        next.push(smoothed + (target - smoothed) * k)
      }
      barStateRef.current = next
      setBars(next)
    }
    tick()
    const timer = window.setInterval(tick, WAVE_TICK_MS)
    return () => window.clearInterval(timer)
  }, [stage])

  const canStartRecording = () =>
    !disabled && (stageRef.current === 'idle' || stageRef.current === 'error')

  const startRecording = async () => {
    if (!canStartRecording()) return
    const operation = ++operationRef.current
    const captureController = new AbortController()
    captureControllerRef.current = captureController
    stoppingRef.current = false
    updateStage('requesting')
    setError('')
    smootherRef.current.reset()

    const recognizer = createSpeechRecognizer({ chatSessionId: sessionId })
    recognizerRef.current = recognizer
    // 部分转写直接流入输入框草稿。
    unsubscribeRef.current = recognizer.onPartial((text) => {
      if (recognizerRef.current === recognizer) onLiveTextRef.current?.(text)
    })
    let endpoint: VoiceEndpoint | null = null
    const failStart = async (caught: unknown) => {
      endpoint?.dispose()
      endpoint = null
      if (operation !== operationRef.current) return
      // 用户关闭模型下载引导等主动取消不报错，静默回到待机。
      if (caught instanceof DOMException && caught.name === 'AbortError') {
        const failureOperation = ++operationRef.current
        await releaseResources()
        if (failureOperation !== operationRef.current) return
        reset()
        return
      }
      const failureOperation = ++operationRef.current
      await releaseResources()
      if (failureOperation !== operationRef.current) return
      const message = errorMessage(caught, t('chat:voiceInput.failed'))
      showError(message === 'permission' ? t('chat:voiceInput.permissionDenied') : message)
    }
    try {
      if (window.__PISPER_MOBILE_APP__) permissionOperationRef.current = operation
      try {
        await requestMicrophonePermission()
      } finally {
        if (permissionOperationRef.current === operation) permissionOperationRef.current = null
      }
      if (operation !== operationRef.current) return
      // 用户可能在授权期间离开应用；权限结果不能让后台页面开始采集，也不能在下次返回时复活。
      if (window.__PISPER_MOBILE_APP__ && document.visibilityState === 'hidden') {
        close()
        return
      }
      // 模型未安装先走下载引导（取消/关闭引导即中止本次录音）；放在授权之后，
      // 避免原生授权弹窗与页内对话框两个等待窗口叠加。
      await speechModels.ensureReady(captureController.signal)
      if (operation !== operationRef.current) return
      // 麦克风授权后立即采集，模型在后台加载；识别器负责缓存开头和等待最终转写。
      void recognizer.start().then(
        () => {},
        () => failStart(new Error(t('chat:voiceInput.failed'))),
      )
      endpoint = await createVoiceEndpoint()
      if (operation !== operationRef.current) {
        endpoint.dispose()
        return
      }
      endpointRef.current = endpoint
      const activeEndpoint = endpoint
      const capture = await startMicrophoneCapture((samples) => {
        if (operation !== operationRef.current) return
        // 电平驱动波形胶囊；VAD 只决定何时停止，不裁剪 PCM（尾静音必须进识别器）。
        levelRef.current = smootherRef.current.push(pcmLevel(samples))
        const ended = activeEndpoint.acceptPcm(samples)
        if (recognizer.acceptPcm(samples) || ended) stopRecordingRef.current()
      }, captureController.signal)
      if (operation !== operationRef.current) {
        await capture.stop()
        return
      }
      captureRef.current = capture
      updateStage('recording')
      if (window.__PISPER_MOBILE_APP__) {
        // 样本计数是主边界，墙钟定时器用于音频回调停滞时仍能按时结束录音。
        recordingLimitTimerRef.current = window.setTimeout(
          () => stopRecordingRef.current(),
          VOICE_MAX_DURATION_SECONDS * 1_000,
        )
      }
    } catch (caught) {
      await failStart(caught)
    }
  }

  const stopRecording = async () => {
    if (stageRef.current !== 'recording' || stoppingRef.current) return
    keyboardOperationRef.current = null
    stoppingRef.current = true
    clearRecordingLimitTimer()
    const operation = ++operationRef.current
    updateStage('transcribing')
    const captureController = captureControllerRef.current
    captureControllerRef.current = null
    captureController?.abort()
    const capture = captureRef.current
    captureRef.current = null
    const endpoint = endpointRef.current
    endpointRef.current = null
    const recognizer = recognizerRef.current
    try {
      await capture?.stop()
      if (operation !== operationRef.current) return
      // 未确认人声时不调用 ASR finish，避免静音转写幻觉写入草稿。
      if (endpoint && !endpoint.hasSpeech) {
        endpoint.dispose()
        onLiveTextRef.current?.(null)
        await releaseResources()
        if (operation !== operationRef.current) return
        reset()
        return
      }
      endpoint?.dispose()
      const finalTranscript = await recognizer?.finish()
      if (operation !== operationRef.current) return
      if (!finalTranscript) throw new Error(t('chat:voiceInput.empty'))
      // 最终转写替换输入框里的实时部分文本。
      onInsertRef.current(finalTranscript)
      await releaseResources()
      if (operation !== operationRef.current) return
      reset()
    } catch (caught) {
      if (operation !== operationRef.current) return
      await releaseResources()
      if (operation !== operationRef.current) return
      onLiveTextRef.current?.(null)
      const message = errorMessage(caught, t('chat:voiceInput.failed'))
      showError(message === 'permission' ? t('chat:voiceInput.permissionDenied') : message)
      stoppingRef.current = false
    }
  }

  stopRecordingRef.current = () => void stopRecording()

  useVoiceShortcut({
    enabled: shortcutEnabled && !disabled,
    binding: voiceShortcut,
    onStart: () => {
      if (!canStartRecording()) return false
      void startRecording()
      keyboardOperationRef.current = operationRef.current
      return true
    },
    onRelease: () => {
      if (keyboardOperationRef.current !== operationRef.current) return
      // 授权或采集尚未就绪时松开即作废本轮，并中止仍在初始化中的麦克风。
      if (!captureRef.current) close()
      else void stopRecording()
    },
    onCancel: () => {
      if (keyboardOperationRef.current === operationRef.current) close()
    },
  })

  const buttonLabel =
    stage === 'recording'
      ? t('chat:voiceInput.stop')
      : stage === 'requesting'
        ? t('chat:voiceInput.close')
        : stage === 'error'
          ? error
          : t('chat:voiceInput.open')
  const buttonTitle =
    voiceShortcut && stage !== 'error'
      ? `${buttonLabel} (${formatShortcut(voiceShortcut)})`
      : buttonLabel

  return (
    <div className="relative flex flex-none items-center">
      {/* 录音中的声波胶囊：悬浮在视口底部正中央（与参考视频一致），仅展示实时波形 */}
      {stage === 'recording' &&
        createPortal(
          <div
            aria-hidden="true"
            className="voice-capsule-pop pointer-events-none fixed bottom-4 left-1/2 z-[80] flex h-10 -translate-x-1/2 items-center gap-[3px] rounded-full border border-[rgba(125,211,252,.35)] bg-[#10141c]/95 px-4"
            style={{
              boxShadow: `0 8px 30px -8px rgba(0,0,0,.55), 0 0 ${14 + levelRef.current * 26}px rgba(80,180,255,${(0.18 + levelRef.current * 0.4).toFixed(3)})`,
            }}
          >
            {bars.map((bar, index) => (
              <span
                key={index}
                className="w-[3px] rounded-full"
                style={{
                  height: `${(3 + bar * 19).toFixed(1)}px`,
                  // 能量越高色相从青转向蓝紫，低能量保持沉静的青色
                  background: `linear-gradient(180deg, hsl(${210 - bar * 45} 92% ${62 + bar * 10}%), hsl(${222 - bar * 30} 85% ${48 + bar * 8}%))`,
                  transition: 'height 70ms linear, background 200ms linear',
                }}
              />
            ))}
          </div>,
          document.body,
        )}
      {/* 错误胶囊：与波形同一位置，直接显示原因，点击可关闭 */}
      {stage === 'error' &&
        createPortal(
          <button
            type="button"
            className="voice-capsule-pop fixed bottom-4 left-1/2 z-[80] flex h-9 max-w-[min(480px,calc(100vw-32px))] -translate-x-1/2 cursor-pointer items-center gap-2 rounded-full border border-[rgba(248,113,113,.4)] bg-[#1c1012]/95 px-4 text-[12px] text-[#fca5a5] shadow-[0_10px_30px_-10px_rgba(0,0,0,.5)]"
            onClick={() => {
              window.clearTimeout(errorTimerRef.current)
              setError('')
              updateStage('idle')
            }}
          >
            <MicOff size={13} className="flex-none" />
            <span className="truncate">{error}</span>
          </button>,
          document.body,
        )}
      <button
        type="button"
        className={`relative grid !size-11 !min-w-11 place-items-center rounded-[var(--r-sm)] border transition-[background-color,color,border-color,box-shadow,transform] duration-200 hover:scale-105 disabled:cursor-not-allowed disabled:opacity-45 ${
          stage === 'recording'
            ? 'border-[var(--danger)] bg-[var(--danger-soft)] text-[var(--danger-strong)] shadow-[0_0_0_3px_var(--danger-soft)]'
            : stage === 'transcribing' || stage === 'requesting'
              ? 'border-transparent bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)]'
              : stage === 'error'
                ? 'border-transparent bg-[var(--danger-soft)] text-[var(--danger-strong)]'
                : 'border-transparent bg-[var(--surface-subtle)] text-[var(--text-muted)] hover:border-[var(--brand-blue)] hover:bg-[var(--brand-blue-soft)] hover:text-[var(--brand-blue-strong)]'
        }`}
        title={buttonTitle}
        aria-label={stage === 'recording' ? t('chat:voiceInput.stop') : t('chat:voiceInput.open')}
        aria-pressed={stage === 'recording'}
        disabled={disabled || stage === 'transcribing'}
        onClick={
          stage === 'recording'
            ? () => void stopRecording()
            : stage === 'requesting'
              ? () => close()
              : () => void startRecording()
        }
      >
        {stage === 'recording' && (
          <span
            aria-hidden="true"
            className="absolute inset-0 animate-ping rounded-[var(--r-sm)] bg-[var(--danger)] opacity-25"
          />
        )}
        {stage === 'recording' ? (
          <MicOff size={17} />
        ) : stage === 'transcribing' || stage === 'requesting' ? (
          <LoaderCircle className="animate-spin" size={17} />
        ) : stage === 'error' ? (
          <MicOff size={17} />
        ) : (
          <Mic size={17} />
        )}
      </button>
      {/* 模型下载引导对话框：ensureReady 发现 ASR 未安装时弹出 */}
      <SpeechModelsDialog manager={speechModels} onClose={() => {}} />
    </div>
  )
}
