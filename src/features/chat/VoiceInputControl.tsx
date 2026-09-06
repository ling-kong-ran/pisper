import { ArrowUpLeft, LoaderCircle, Mic, MicOff, RotateCcw, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useEffect, useRef, useState } from 'react'
import { formatShortcut } from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'
import { useVoiceShortcut } from './use-voice-shortcut'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'
import {
  createSpeechRecognizer,
  requestMicrophonePermission,
  startMicrophoneCapture,
  VOICE_MAX_DURATION_SECONDS,
  type MicrophoneCapture,
  type SpeechRecognizer,
} from './voice-input'

type VoiceStage = 'idle' | 'requesting' | 'recording' | 'transcribing' | 'error'

function formatDuration(seconds: number) {
  const minutes = Math.floor(seconds / 60)
  const remainder = Math.floor(seconds % 60)
  return `${minutes}:${remainder.toString().padStart(2, '0')}`
}

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
  sessionId,
  disabled = false,
  shortcutEnabled = false,
}: {
  onInsert: (text: string) => void
  sessionId?: string
  disabled?: boolean
  shortcutEnabled?: boolean
}) {
  const { t } = useI18n()
  const voiceShortcut = useShortcutStore((state) => state.bindings.voiceInput)
  const [stage, setStage] = useState<VoiceStage>('idle')
  const stageRef = useRef<VoiceStage>('idle')
  const updateStage = (next: VoiceStage) => {
    stageRef.current = next
    setStage(next)
  }
  const [elapsed, setElapsed] = useState(0)
  const [transcript, setTranscript] = useState('')
  const [error, setError] = useState('')
  const [initializing, setInitializing] = useState(false)
  const [windowsDesktop, setWindowsDesktop] = useState(false)
  const recognizerRef = useRef<SpeechRecognizer | null>(null)
  const captureRef = useRef<MicrophoneCapture | null>(null)
  const captureControllerRef = useRef<AbortController | null>(null)
  const unsubscribeRef = useRef<(() => void) | null>(null)
  const operationRef = useRef(0)
  const recordingLimitTimerRef = useRef(0)
  const stopRecordingRef = useRef<() => void>(() => {})
  const stoppingRef = useRef(false)
  const keyboardOperationRef = useRef<number | null>(null)
  const permissionOperationRef = useRef<number | null>(null)
  const closeRef = useRef<() => void>(() => {})
  const anchorRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let active = true
    void window.pisperDesktop
      ?.getAppInfo()
      .then((info) => {
        if (active) setWindowsDesktop(info.platform === 'win32')
      })
      .catch(() => {})
    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    if (stage !== 'recording') return undefined
    const timer = window.setInterval(() => {
      setElapsed((current) => {
        const next = Number((current + 0.1).toFixed(1))
        return window.__PISPER_MOBILE_APP__ ? Math.min(next, VOICE_MAX_DURATION_SECONDS) : next
      })
    }, 100)
    return () => window.clearInterval(timer)
  }, [stage])

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
    const capture = captureRef.current
    captureRef.current = null
    const recognizer = recognizerRef.current
    recognizerRef.current = null
    // 先摘下本轮引用，再等待清理，避免关闭后立即重试时释放掉新一轮资源。
    await Promise.allSettled([capture?.stop(), recognizer?.dispose()])
  }

  useEffect(() => {
    const cancel = () => {
      // Android 授权弹窗也会暂时隐藏页面；仅触控授权阶段等待结果，按住快捷键仍须立即取消。
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
    window.addEventListener('blur', cancel)
    document.addEventListener('visibilitychange', visibility)
    return () => {
      window.removeEventListener('blur', cancel)
      document.removeEventListener('visibilitychange', visibility)
      closeRef.current()
    }
  }, [sessionId, shortcutEnabled, disabled])

  const reset = () => {
    clearRecordingLimitTimer()
    stoppingRef.current = false
    updateStage('idle')
    setInitializing(false)
    setElapsed(0)
    setTranscript('')
    setError('')
  }

  const close = () => {
    keyboardOperationRef.current = null
    permissionOperationRef.current = null
    operationRef.current += 1
    void releaseResources()
    reset()
  }

  closeRef.current = close

  const canStartRecording = () =>
    !disabled && (stageRef.current === 'idle' || stageRef.current === 'error')

  const startRecording = async () => {
    if (!canStartRecording()) return
    const operation = ++operationRef.current
    const captureController = new AbortController()
    captureControllerRef.current = captureController
    stoppingRef.current = false
    updateStage('requesting')
    setElapsed(0)
    setTranscript('')
    setError('')

    const recognizer = createSpeechRecognizer({ chatSessionId: sessionId })
    recognizerRef.current = recognizer
    unsubscribeRef.current = recognizer.onPartial((text) => {
      if (recognizerRef.current === recognizer) setTranscript(text)
    })
    const failStart = async (caught: unknown) => {
      if (operation !== operationRef.current) return
      const failureOperation = ++operationRef.current
      await releaseResources()
      if (failureOperation !== operationRef.current) return
      const message = errorMessage(caught, t('chat:voiceInput.failed'))
      setError(message === 'permission' ? t('chat:voiceInput.permissionDenied') : message)
      setInitializing(false)
      updateStage('error')
    }
    try {
      const androidPermission =
        window.__PISPER_MOBILE_APP__ && window.__PISPER_MOBILE_PLATFORM__ === 'android'
      if (androidPermission) permissionOperationRef.current = operation
      try {
        await requestMicrophonePermission()
      } finally {
        if (permissionOperationRef.current === operation) permissionOperationRef.current = null
      }
      if (operation !== operationRef.current) return
      // 用户可能在授权期间离开应用；权限结果不能让后台页面开始采集，也不能在下次返回时复活。
      if (androidPermission && document.visibilityState === 'hidden') {
        close()
        return
      }
      setInitializing(true)
      // 麦克风授权后立即采集，模型在后台加载；识别器负责缓存开头和等待最终转写。
      void recognizer.start().then(() => {
        if (operation === operationRef.current) setInitializing(false)
      }, failStart)
      const capture = await startMicrophoneCapture((samples) => {
        if (operation === operationRef.current && recognizer.acceptPcm(samples))
          stopRecordingRef.current()
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
    const recognizer = recognizerRef.current
    try {
      await capture?.stop()
      if (operation !== operationRef.current) return
      const finalTranscript = await recognizer?.finish()
      if (operation !== operationRef.current) return
      if (!finalTranscript) throw new Error(t('chat:voiceInput.empty'))
      onInsert(finalTranscript)
      await releaseResources()
      if (operation !== operationRef.current) return
      reset()
    } catch (caught) {
      if (operation !== operationRef.current) return
      await releaseResources()
      if (operation !== operationRef.current) return
      const message = errorMessage(caught, t('chat:voiceInput.failed'))
      setError(message === 'permission' ? t('chat:voiceInput.permissionDenied') : message)
      stoppingRef.current = false
      updateStage('error')
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

  const buttonLabel = stage === 'recording' ? t('chat:voiceInput.stop') : t('chat:voiceInput.open')
  const buttonTitle = voiceShortcut
    ? `${buttonLabel} (${formatShortcut(voiceShortcut)})`
    : buttonLabel

  const stageLabel =
    stage === 'requesting'
      ? t('chat:voiceInput.requesting')
      : stage === 'recording'
        ? initializing
          ? t('chat:voiceInput.recordingPreparing')
          : t('chat:voiceInput.recording')
        : stage === 'transcribing'
          ? t('chat:voiceInput.transcribing')
          : stage === 'error'
            ? t('chat:voiceInput.failed')
            : t('chat:voiceInput.ready')

  return (
    <div ref={anchorRef} className="relative flex flex-none items-center">
      <AnchoredPopupMenu
        open={stage !== 'idle'}
        anchorRef={anchorRef}
        menuRef={menuRef}
        onClose={close}
        placement="top"
        align="end"
        className="voice-input-popup w-[min(370px,calc(100vw-24px))] overflow-hidden rounded-[var(--r-md)] border border-[var(--stroke)] bg-[var(--solid)] text-[var(--text)] shadow-[0_22px_55px_-24px_var(--shadow-strong)]"
      >
        <div data-voice-input="true">
          <div className="flex items-center justify-between border-b border-[var(--stroke-soft)] px-3 py-2.5">
            <div className="flex min-w-0 items-center gap-2">
              <span
                className={`grid size-7 flex-none place-items-center rounded-[var(--r-sm)] ${stage === 'recording' ? 'bg-[var(--danger-soft)] text-[var(--danger-strong)]' : 'bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)]'}`}
              >
                {stage === 'recording' ? <Mic size={14} /> : <MicOff size={14} />}
              </span>
              <div className="min-w-0">
                <div className="truncate text-[12px] font-[650]">{t('chat:voiceInput.title')}</div>
                <div className="text-[10px] text-[var(--text-tertiary)]">{stageLabel}</div>
              </div>
            </div>
            <button
              type="button"
              className="grid size-7 flex-none place-items-center rounded-[var(--r-xs)] border-0 bg-transparent text-[var(--text-muted)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
              title={t('chat:voiceInput.close')}
              aria-label={t('chat:voiceInput.close')}
              onClick={close}
            >
              <X size={15} />
            </button>
          </div>

          <div className="space-y-3 px-3 py-3">
            {(stage === 'requesting' || stage === 'transcribing') && (
              <div
                role="status"
                aria-live="polite"
                className="flex items-center gap-3 rounded-md bg-muted px-3 py-3 text-xs text-content"
              >
                {stage === 'requesting' && windowsDesktop ? (
                  <ArrowUpLeft className="size-5 flex-none text-primary" />
                ) : (
                  <LoaderCircle className="size-4 flex-none animate-spin" />
                )}
                <span className="min-w-0 leading-5 break-words">
                  {stage === 'requesting'
                    ? windowsDesktop
                      ? t('chat:voiceInput.requestingWindowsDescription')
                      : t('chat:voiceInput.requestingDescription')
                    : t('chat:voiceInput.transcribingDescription')}
                </span>
              </div>
            )}

            {stage === 'recording' && (
              <>
                <div className="flex items-center gap-3 rounded-[var(--r-sm)] bg-[var(--danger-soft)] px-3 py-2.5">
                  <span className="relative grid size-8 flex-none place-items-center rounded-full bg-[var(--danger)] text-white">
                    <span className="absolute inset-0 animate-ping rounded-full bg-[var(--danger)] opacity-35" />
                    <Mic size={15} />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="mb-1 flex items-center justify-between gap-2 text-[10px] font-[650] text-[var(--danger-strong)]">
                      <span>{t('chat:voiceInput.listening')}</span>
                      <span className="font-mono tabular-nums">{formatDuration(elapsed)}</span>
                    </div>
                    <div className="flex h-5 items-center gap-0.5" aria-hidden="true">
                      {[4, 11, 17, 8, 14, 20, 10, 16, 6, 13, 18, 9, 15, 5].map((height, index) => (
                        <span
                          key={index}
                          className="w-1 rounded-full bg-[var(--danger)] opacity-75 transition-[height] duration-200"
                          style={{ height: `${height}px` }}
                        />
                      ))}
                    </div>
                  </div>
                </div>
                {transcript && (
                  <p className="m-0 text-[11px] leading-[1.45] text-[var(--text-secondary)]">
                    {transcript}
                  </p>
                )}
                <button
                  type="button"
                  className="inline-flex h-8 w-full items-center justify-center gap-1.5 rounded-[var(--r-sm)] border-0 bg-[var(--danger)] px-3 text-[11px] font-[650] text-white hover:bg-[var(--danger-strong)]"
                  onClick={() => void stopRecording()}
                >
                  <MicOff size={13} />
                  {t('chat:voiceInput.stop')}
                </button>
              </>
            )}

            {stage === 'error' && (
              <div className="space-y-3">
                <p className="m-0 rounded-[var(--r-sm)] bg-[var(--danger-soft)] px-3 py-2.5 text-[11px] leading-[1.5] text-[var(--danger-strong)]">
                  {error}
                </p>
                <button
                  type="button"
                  className="inline-flex h-8 w-full items-center justify-center gap-1.5 rounded-[var(--r-sm)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)] px-3 text-[11px] font-[650] text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
                  onClick={() => void startRecording()}
                >
                  <RotateCcw size={13} />
                  {t('chat:voiceInput.retry')}
                </button>
              </div>
            )}
          </div>

          <div className="border-t border-[var(--stroke-soft)] px-3 py-2 text-[10px] text-[var(--text-tertiary)]">
            {t('chat:voiceInput.localPreview')}
          </div>
        </div>
      </AnchoredPopupMenu>
      <button
        type="button"
        className={`grid !size-11 !min-w-11 place-items-center rounded-[var(--r-sm)] border border-transparent transition-[background-color,color,border-color,box-shadow,transform] duration-200 hover:scale-105 disabled:cursor-not-allowed disabled:opacity-45 ${stage === 'recording' ? 'border-[var(--danger)] bg-[var(--danger-soft)] text-[var(--danger-strong)] shadow-[0_0_0_3px_var(--danger-soft)]' : stage === 'transcribing' || stage === 'requesting' ? 'bg-[var(--brand-blue-soft)] text-[var(--brand-blue-strong)]' : stage === 'error' ? 'bg-[var(--danger-soft)] text-[var(--danger-strong)]' : 'bg-[var(--surface-subtle)] text-[var(--text-muted)] hover:border-[var(--brand-blue)] hover:bg-[var(--brand-blue-soft)] hover:text-[var(--brand-blue-strong)]'}`}
        title={buttonTitle}
        aria-label={buttonLabel}
        aria-expanded={stage !== 'idle'}
        disabled={disabled || stage === 'requesting' || stage === 'transcribing'}
        onClick={stage === 'recording' ? () => void stopRecording() : () => void startRecording()}
      >
        {stage === 'recording' ? (
          <MicOff size={17} />
        ) : stage === 'transcribing' || stage === 'requesting' ? (
          <LoaderCircle className="animate-spin" size={17} />
        ) : (
          <Mic size={17} />
        )}
      </button>
    </div>
  )
}
