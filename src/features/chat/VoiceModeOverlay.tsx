// 语音对话模式全屏页：1:1 复刻 AsLive 的极简布局——纯黑底、左上角品牌字标、
// 中央巨型点云球、底部单个胶囊主按钮、右下角通话计时。没有多余控件：
// 空格/主按钮开始或结束对话，Esc 退出。
import { useEffect, useState } from 'react'
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
      : Math.min(window.innerWidth * 0.52, window.innerHeight * 0.52, 460),
  )
  useEffect(() => {
    if (!open) return undefined
    const update = () => setSize(Math.min(window.innerWidth * 0.52, window.innerHeight * 0.52, 460))
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
  sendPrompt,
  onAbort,
  onClose,
}: VoiceModeOverlayProps) {
  const { t } = useI18n()
  const dark = useIsDarkTheme()
  const voice = useVoiceSession({ open, sessionId, messages, sendPrompt, onAbort })
  const { stage, level, partial, turns, error, elapsed } = voice
  const orbSize = useResponsiveOrbSize(open)

  const active =
    stage === 'listening' ||
    stage === 'transcribing' ||
    stage === 'thinking' ||
    stage === 'speaking' ||
    stage === 'requesting'

  useEffect(() => {
    if (!open) return undefined
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        voice.hangUp()
        onClose()
      }
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
  }, [open, onClose, active])

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
      className={`fixed inset-0 z-[90] flex flex-col overflow-hidden select-none ${dark ? 'bg-[#030304] text-[#cfe8ff]' : 'bg-[#f2f5fa] text-[#1e2c42]'}`}
      role="dialog"
      aria-modal="true"
      aria-label={t('chat:voiceMode.title')}
    >
      {/* 左上品牌字标 + 状态呼吸点 */}
      <header className="pointer-events-none absolute left-7 top-6 flex items-center gap-3">
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

      {/* 中央点云球 */}
      <div className="flex min-h-0 flex-1 items-center justify-center">
        <DigitalOrb stage={stage} level={level} size={orbSize} dark={dark} />
      </div>

      {/* 状态行 + 字幕 */}
      <div className="pointer-events-none absolute inset-x-0 bottom-[120px] flex flex-col items-center gap-3 px-6 text-center">
        <div
          className={`text-[11px] font-semibold uppercase tracking-[0.34em] ${stage === 'error' ? 'text-[#f87171]' : dark ? 'text-[#5f7a94]' : 'text-[#7488a0]'}`}
          role="status"
          aria-live="polite"
        >
          {statusLabel}
        </div>
        <p
          className={`m-0 min-h-[22px] max-w-[640px] text-[15px] leading-[1.6] ${dark ? 'text-[#d7e6f7]' : 'text-[#24344d]'}`}
        >
          {caption}
        </p>
      </div>

      {/* 底部中央：唯一的胶囊主按钮 */}
      <div className="absolute inset-x-0 bottom-9 flex justify-center">
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
      <div className="pointer-events-none absolute bottom-9 right-7 flex items-center gap-2.5">
        <span
          className="grid size-9 place-items-center rounded-full bg-[linear-gradient(135deg,#2563eb,#38bdf8)] text-[10px] font-bold text-white shadow-[0_0_18px_rgba(56,189,248,.4)]"
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

      {/* 左下：退出提示 */}
      <div
        className={`pointer-events-none absolute bottom-11 left-7 text-[10px] uppercase tracking-[0.24em] ${dark ? 'text-[#3d4c5f]' : 'text-[#9aa9bc]'}`}
      >
        {t('chat:voiceMode.escHint')}
      </div>
    </div>,
    document.body,
  )
}
