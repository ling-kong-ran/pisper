// 语音页通过 Portal 脱离 App 外壳，需要自行保护安全区和始终可用的退出入口。
import { useEffect, useRef, useState } from 'react'
import { AudioLines, Mic, MicOff, Square, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { SpeechModelsDialog } from './SpeechModelsDialog'
import { useSpeechModels } from './use-speech-models'
import { playLocalSpeech } from './speech-output'
import { createPortal } from 'react-dom'
import { useI18n } from '@/app/use-i18n'
import { DigitalOrb } from './DigitalOrb'
import { useVoiceSession } from './use-voice-session'
import { truncateVoiceText, type VoiceModeStage } from './voice-mode-state'
import type { ChatAttachment, ChatMessage } from '@/types/chat'

type VoiceModeOverlayProps = {
  open: boolean
  sessionId: string
  sessionName: string
  messages: ChatMessage[]
  streaming?: boolean
  sendPrompt: (
    value: string,
    attachments: ChatAttachment[],
    goalMode: boolean,
    teamMode: boolean,
    goalTokenBudget: number | null,
  ) => Promise<void> | void
  onAbort: () => Promise<void> | void
  onClose: () => void
}

type Translate = (key: string) => string

// 状态文案需要 t() 字面量 key（i18n 静态检查），集中在映射函数里。
function stageLabels(t: Translate): Record<VoiceModeStage, string> {
  return {
    idle: t('chat:voiceMode.idle'),
    requesting: t('chat:voiceMode.requesting'),
    listening: t('chat:voiceMode.listening'),
    transcribing: t('chat:voiceMode.transcribing'),
    thinking: t('chat:voiceMode.thinking'),
    speaking: t('chat:voiceMode.speaking'),
    error: t('chat:voiceMode.failed'),
  }
}

function formatElapsed(totalSeconds: number) {
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60
  return `${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`
}

// 读取 documentElement 上已解析的主题（App.tsx 统一写入 data-theme），
// 用 MutationObserver 跟踪切换，覆盖 system/scheduled 所有模式。
function useIsDarkTheme() {
  const [dark, setDark] = useState(() => document.documentElement.dataset.theme !== 'light')
  useEffect(() => {
    const root = document.documentElement
    const update = () => setDark(root.dataset.theme !== 'light')
    update()
    const observer = new MutationObserver(update)
    observer.observe(root, { attributes: true, attributeFilter: ['data-theme'] })
    return () => observer.disconnect()
  }, [])
  return dark
}

function useResponsiveOrbSize(open: boolean) {
  const [size, setSize] = useState(() =>
    typeof window === 'undefined'
      ? 380
      : Math.max(48, Math.min(window.innerWidth * 0.52, window.innerHeight - 304, 460)),
  )
  useEffect(() => {
    if (!open) return undefined
    const update = () =>
      setSize(Math.max(48, Math.min(window.innerWidth * 0.52, window.innerHeight - 304, 460)))
    update()
    window.addEventListener('resize', update)
    return () => window.removeEventListener('resize', update)
  }, [open])
  return Math.floor(size)
}

export function VoiceModeOverlay({
  open,
  sessionId,
  messages,
  streaming,
  sendPrompt,
  onAbort,
  onClose,
}: VoiceModeOverlayProps) {
  const { t } = useI18n()
  const dark = useIsDarkTheme()
  const models = useSpeechModels(['asr', 'tts'])
  const voice = useVoiceSession({
    open,
    sessionId,
    messages,
    streaming,
    sendPrompt,
    onAbort,
    ensureReady: models.ensureReady,
    speakText: (text, signal, onSpeaking) =>
      playLocalSpeech(text, models.selectedVoice, signal, onSpeaking),
  })
  const { stage, level, partial, turns, error, elapsed } = voice
  const orbSize = useResponsiveOrbSize(open)
  const dialogRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const previous = document.activeElement
    dialogRef.current?.focus({ preventScroll: true })
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected)
        previous.focus({ preventScroll: true })
    }
  }, [open])

  const closeOverlay = () => {
    try {
      voice.hangUp()
    } finally {
      // 退出不等待原生音频和远程请求清理，清理异常也不能困住用户。
      models.close()
      onClose()
    }
  }

  const active =
    stage === 'listening' ||
    stage === 'transcribing' ||
    stage === 'thinking' ||
    stage === 'speaking' ||
    stage === 'requesting'

  useEffect(() => {
    if (!open) return undefined
    const onKey = (event: KeyboardEvent) => {
      if (models.open || event.defaultPrevented || event.isComposing) return
      if (event.key === 'Escape') {
        event.preventDefault()
        closeOverlay()
        return
      }
      if (event.key === 'Tab') {
        const buttons = [
          ...(dialogRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ||
            []),
        ]
        const first = buttons[0]
        const last = buttons[buttons.length - 1]
        if (
          event.shiftKey &&
          (document.activeElement === first || document.activeElement === dialogRef.current)
        ) {
          event.preventDefault()
          last?.focus()
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault()
          first?.focus()
        }
      }
      if (
        event.target instanceof Element &&
        event.target.closest('input, textarea, select, button, [contenteditable="true"]')
      )
        return
      if (event.key === ' ' && !event.repeat) {
        event.preventDefault()
        if (active) voice.hangUp()
        else voice.toggleMain()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // 键盘行为跟随最新回调，但只在开关时绑定一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, onClose, active, models.open])

  if (!open) return null

  const stageText = stageLabels(t)
  // 字幕：聆听时显示实时部分转写，否则显示最近一轮对话。
  const lastTurn = turns[turns.length - 1]
  const caption =
    stage === 'listening' && partial
      ? partial
      : lastTurn
        ? `${lastTurn.role === 'user' ? '' : ''}${truncateVoiceText(lastTurn.text, 120)}`
        : ''
  const statusLabel = stage === 'error' ? error || t('chat:voiceMode.failed') : stageText[stage]

  return createPortal(
    <div
      className={`fixed inset-0 z-[90] grid grid-cols-[minmax(0,1fr)] grid-rows-[minmax(0,1fr)_100px_108px] overflow-hidden select-none pt-[env(safe-area-inset-top)] pr-[env(safe-area-inset-right)] pb-[env(safe-area-inset-bottom)] pl-[env(safe-area-inset-left)] ${dark ? 'bg-[#030304] text-[#cfe8ff]' : 'bg-[#f2f5fa] text-[#1e2c42]'}`}
      ref={dialogRef}
      tabIndex={-1}
      role="dialog"
      aria-modal="true"
      aria-label={t('chat:voiceMode.title')}
    >
      {/* 左上品牌字标 + 状态呼吸点 */}
      <header className="pointer-events-none absolute left-[calc(env(safe-area-inset-left)_+_28px)] top-[calc(env(safe-area-inset-top)_+_24px)] flex items-center gap-3">
        <span
          className={`inline-block size-2 rounded-full ${active ? 'animate-pulse bg-[#59e6ff]' : dark ? 'bg-[#3b4a5c]' : 'bg-[#b6c3d4]'}`}
          aria-hidden="true"
        />
        <span
          className={`text-[13px] font-bold tracking-[0.42em] ${dark ? 'text-[#9be7ff] [text-shadow:0_0_14px_rgba(89,230,255,.45)]' : 'text-[#0d6ac9]'}`}
        >
          PISPER
        </span>
      </header>

      <div className="absolute top-[calc(env(safe-area-inset-top)_+_16px)] right-[calc(env(safe-area-inset-right)_+_16px)] z-10 flex items-center gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t('chat:speechModels.title')}
              onClick={() => {
                voice.hangUp()
                models.show()
              }}
            >
              <AudioLines />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{t('chat:speechModels.title')}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="size-11 flex-none"
              aria-label={t('chat:speechModels.close')}
              onClick={closeOverlay}
            >
              <X />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{t('chat:speechModels.close')}</TooltipContent>
        </Tooltip>
      </div>
      <SpeechModelsDialog manager={models} onClose={() => voice.hangUp()} />

      {/* 中央点云球 */}
      <div className="flex min-h-0 items-center justify-center pt-16 pb-6">
        <DigitalOrb stage={stage} level={level} size={orbSize} dark={dark} />
      </div>

      {/* 状态行 + 字幕 */}
      <div className="flex min-h-0 min-w-0 flex-col items-center justify-center gap-2 px-6 text-center">
        <div
          className={`max-w-full text-[11px] font-semibold uppercase tracking-normal ${stage === 'error' ? 'max-h-full overflow-y-auto break-words text-[#f87171]' : `line-clamp-2 ${dark ? 'text-[#5f7a94]' : 'text-[#7488a0]'}`}`}
          role="status"
          aria-live="polite"
        >
          {statusLabel}
        </div>
        <p
          className={`m-0 line-clamp-2 min-h-[22px] max-w-[640px] text-[15px] leading-[1.6] ${dark ? 'text-[#d7e6f7]' : 'text-[#24344d]'}`}
        >
          {caption}
        </p>
      </div>

      {/* 底部中央：唯一的胶囊主按钮 */}
      <div className="relative flex justify-center pt-3 sm:items-center sm:pt-0">
        <button
          type="button"
          className={`h-11 cursor-pointer rounded-full border bg-transparent px-9 text-[11px] font-semibold uppercase tracking-[0.3em] transition-[box-shadow,background-color,border-color] duration-300 ${
            dark
              ? 'border-[rgba(140,220,255,.4)] text-[#9be7ff] hover:border-[rgba(140,220,255,.8)] hover:bg-[rgba(89,230,255,.08)] hover:shadow-[0_0_28px_rgba(89,230,255,.25)]'
              : 'border-[rgba(13,106,201,.45)] text-[#0d6ac9] hover:border-[rgba(13,106,201,.85)] hover:bg-[rgba(13,106,201,.07)] hover:shadow-[0_0_28px_rgba(13,106,201,.18)]'
          }`}
          onClick={() => (active ? voice.hangUp() : voice.toggleMain())}
        >
          {active ? t('chat:voiceMode.endConversation') : t('chat:voiceMode.startConversation')}
        </button>
      </div>

      {/* 右下：通话计时 */}
      <div className="pointer-events-none absolute bottom-[calc(env(safe-area-inset-bottom)_+_12px)] right-[calc(env(safe-area-inset-right)_+_20px)] flex items-center gap-2.5 sm:bottom-[calc(env(safe-area-inset-bottom)_+_36px)] sm:right-[calc(env(safe-area-inset-right)_+_28px)]">
        <span
          className="hidden size-9 place-items-center rounded-full sm:grid bg-[linear-gradient(135deg,#2563eb,#38bdf8)] text-[10px] font-bold text-white shadow-[0_0_18px_rgba(56,189,248,.4)]"
          aria-hidden="true"
        >
          {active ? '●' : '○'}
        </span>
        <span
          className={`font-mono text-[13px] tabular-nums tracking-[0.12em] ${dark ? 'text-[#7d93ad]' : 'text-[#61748c]'}`}
        >
          {formatElapsed(elapsed)}
        </span>
      </div>

      <div className="absolute bottom-[env(safe-area-inset-bottom)] left-[calc(env(safe-area-inset-left)_+_20px)] sm:bottom-[calc(env(safe-area-inset-bottom)_+_36px)] sm:left-[calc(env(safe-area-inset-left)_+_28px)]">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-11 flex-none"
              disabled={!active && !voice.muted}
              aria-label={
                stage === 'speaking' || stage === 'thinking'
                  ? t('chat:voiceMode.interrupt')
                  : voice.muted
                    ? t('chat:voiceMode.unmute')
                    : t('chat:voiceMode.mute')
              }
              onClick={() =>
                stage === 'speaking' || stage === 'thinking'
                  ? voice.interrupt()
                  : voice.toggleMute()
              }
            >
              {stage === 'speaking' || stage === 'thinking' ? (
                <Square />
              ) : voice.muted ? (
                <MicOff />
              ) : (
                <Mic />
              )}
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            {stage === 'speaking' || stage === 'thinking'
              ? t('chat:voiceMode.interrupt')
              : voice.muted
                ? t('chat:voiceMode.unmute')
                : t('chat:voiceMode.mute')}
          </TooltipContent>
        </Tooltip>
      </div>
    </div>,
    document.body,
  )
}
