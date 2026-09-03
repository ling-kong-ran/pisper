import { LoaderCircle, Mic, MicOff, RotateCcw, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useEffect, useRef, useState } from 'react'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'
import {
  createSpeechRecognizer,
  requestMicrophonePermission,
  startMicrophoneCapture,
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
  if (error instanceof Error && error.message) return error.message
  return fallback
}

export function VoiceInputControl({
  onInsert,
  disabled = false,
}: {
  onInsert: (text: string) => void
  disabled?: boolean
}) {
  const { t } = useI18n()
  const [stage, setStage] = useState<VoiceStage>('idle')
  const [elapsed, setElapsed] = useState(0)
  const [transcript, setTranscript] = useState('')
  const [error, setError] = useState('')
  const recognizerRef = useRef<SpeechRecognizer | null>(null)
  const captureRef = useRef<MicrophoneCapture | null>(null)
  const unsubscribeRef = useRef<(() => void) | null>(null)
  const operationRef = useRef(0)
  const anchorRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (stage !== 'recording') return undefined
    const timer = window.setInterval(() => {
      setElapsed((current) => Number((current + 0.1).toFixed(1)))
    }, 100)
    return () => window.clearInterval(timer)
  }, [stage])

  const releaseResources = async () => {
    unsubscribeRef.current?.()
    unsubscribeRef.current = null
    const capture = captureRef.current
    captureRef.current = null
    await capture?.stop()
    const recognizer = recognizerRef.current
    recognizerRef.current = null
    await recognizer?.dispose()
  }

  useEffect(
    () => () => {
      operationRef.current += 1
      void releaseResources()
    },
    [],
  )

  const reset = () => {
    setStage('idle')
    setElapsed(0)
    setTranscript('')
    setError('')
  }

  const close = () => {
    operationRef.current += 1
    void releaseResources()
    reset()
  }

  const startRecording = async () => {
    if (disabled || stage === 'requesting' || stage === 'recording' || stage === 'transcribing')
      return
    const operation = ++operationRef.current
    setStage('requesting')
    setElapsed(0)
    setTranscript('')
    setError('')

    const recognizer = createSpeechRecognizer()
    recognizerRef.current = recognizer
    unsubscribeRef.current = recognizer.onPartial(setTranscript)
    try {
      await requestMicrophonePermission()
      if (operation !== operationRef.current) return
      await recognizer.start()
      const capture = await startMicrophoneCapture((samples) => recognizer.acceptPcm(samples))
      if (operation !== operationRef.current) {
        await capture.stop()
        return
      }
      captureRef.current = capture
      setStage('recording')
    } catch (caught) {
      if (operation !== operationRef.current) return
      await releaseResources()
      const message = errorMessage(caught, t('chat:voiceInput.failed'))
      setError(message === 'permission' ? t('chat:voiceInput.permissionDenied') : message)
      setStage('error')
    }
  }

  const stopRecording = async () => {
    if (stage !== 'recording') return
    const operation = ++operationRef.current
    setStage('transcribing')
    const capture = captureRef.current
    captureRef.current = null
    await capture?.stop()
    const recognizer = recognizerRef.current
    try {
      const finalTranscript = await recognizer?.finish()
      if (operation !== operationRef.current) return
      if (!finalTranscript) throw new Error(t('chat:voiceInput.empty'))
      onInsert(finalTranscript)
      await releaseResources()
      reset()
    } catch (caught) {
      if (operation !== operationRef.current) return
      await releaseResources()
      setError(errorMessage(caught, t('chat:voiceInput.failed')))
      setStage('error')
    }
  }

  const stageLabel =
    stage === 'requesting'
      ? t('chat:voiceInput.requesting')
      : stage === 'recording'
        ? t('chat:voiceInput.recording')
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
              <div className="flex items-center gap-3 rounded-[var(--r-sm)] bg-[var(--brand-blue-soft)] px-3 py-3 text-[12px] text-[var(--brand-blue-strong)]">
                <LoaderCircle className="size-4 flex-none animate-spin" />
                <span>
                  {stage === 'requesting'
                    ? t('chat:voiceInput.requestingDescription')
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
        title={stage === 'recording' ? t('chat:voiceInput.stop') : t('chat:voiceInput.open')}
        aria-label={stage === 'recording' ? t('chat:voiceInput.stop') : t('chat:voiceInput.open')}
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
