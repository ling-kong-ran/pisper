// 聚焦会话输入区的小型展示组件：输入框上方的托盘（运行状态 + 停止、排队消息）、
// 资源调用芯片与发送键。
import { useEffect, useRef, useState } from 'react'
import { ArrowUp, Braces, CornerDownLeft, Pencil, RefreshCw, Square, Wrench, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { useShortcutLabel } from '@/lib/shortcuts'
import { Kbd } from '@/components/ui/kbd'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { runDurationMs } from '@/features/chat/run-activity'
import type { EntityRecord, ResourceInvocation } from '@/types/chat'

// 手机（≤650）输入框工具行的触控范围：看得见的圆 / 胶囊是 36，透明的 ::after 向四周各扩 4px，
// 手指能点到的仍是 44（规格 v2 的触屏 44 不变），只是画面上不再是一排 44 的大灰圆（参考 Claude 手机端）。
// 宽屏的粗指针（平板）照旧是 44 的实体按钮。
export const COMPOSER_PHONE_TARGET_CLASSES =
  'relative max-[650px]:after:absolute max-[650px]:after:-inset-1'

// 输入区图标键：桌面 32px，手机 36（触控 44，见上），平板等粗指针 44；“+”菜单与托盘共用同一组工具类。
// 输入框工具行的文字芯片（盾牌芯片、新任务草稿的项目芯片）：桌面是 28 高的透明芯片；
// 手机是 36 高的无边框浅灰胶囊，深色用 surface-active 才和输入框底色分得开。
export const COMPOSER_CHIP_CLASSES = `inline-flex h-7 min-w-0 shrink cursor-pointer items-center gap-1.5 rounded-sm border border-transparent px-2 text-[13px] text-content-secondary transition-colors hover:bg-surface-hover hover:text-content aria-expanded:bg-surface-hover aria-expanded:text-content max-[650px]:h-9 max-[650px]:rounded-full max-[650px]:bg-surface-muted max-[650px]:dark:bg-surface-active max-[650px]:px-3 max-[650px]:text-content min-[651px]:pointer-coarse:h-11 ${COMPOSER_PHONE_TARGET_CLASSES}`

export const COMPOSER_ICON_KEY_CLASSES = `grid size-8 min-w-8 flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent text-content-muted transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50 max-[650px]:size-9 max-[650px]:min-w-9 min-[651px]:pointer-coarse:size-11 min-[651px]:pointer-coarse:min-w-11 ${COMPOSER_PHONE_TARGET_CLASSES}`

// 手机工具行的图标统一 16px（桌面保持原尺寸）。
export const COMPOSER_PHONE_GLYPH_CLASSES = 'max-[650px]:size-4'

// 托盘计时与侧栏运行中行同一种写法（3:12、1:02:05），同一个运行在两处读起来一致。
export function formatTrayClock(milliseconds: number) {
  const total = Math.max(0, Math.floor(milliseconds / 1000))
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = String(total % 60).padStart(2, '0')
  return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${seconds}` : `${minutes}:${seconds}`
}

// 每秒刷新只留在这段计时文字内，避免整个输入区随计时器重渲染；缺少开始时间时不以 0 冒充。
function TrayClock({ startedAt }: { startedAt?: string | null }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [])
  const elapsed = runDurationMs(startedAt, null, now)
  if (!elapsed) return null
  return (
    <span
      aria-hidden="true"
      className="shrink-0 font-mono text-[12.5px] text-content-muted tabular-nums"
      data-tray-clock
    >
      {formatTrayClock(elapsed)}
    </span>
  )
}

const TRAY_ROW_CLASS =
  'flex min-h-[34px] min-w-0 items-center gap-2 pr-1.5 pl-3 text-[13px] max-[650px]:min-h-11 max-[650px]:pr-0 pointer-coarse:min-h-11'
// 排队行的编辑 / 取消：桌面悬停或聚焦才出现，手机与触屏常驻 44×44（规格 4.3 / 7.2）。
const TRAY_ACTION_CLASS =
  'grid size-[26px] flex-none cursor-pointer place-items-center rounded-sm text-content-muted opacity-0 transition-opacity duration-(--d-fast) group-hover/queued:opacity-100 group-focus-within/queued:opacity-100 hover:bg-surface-hover hover:text-content focus-visible:opacity-100 disabled:cursor-default disabled:opacity-40 max-[650px]:size-11 max-[650px]:opacity-100 pointer-coarse:size-11 pointer-coarse:opacity-100 [@media(hover:none)]:opacity-100'

// 输入框正上方的托盘（规格 4.3）：只在运行中、压缩上下文或有排队时出现。
// 运行行只写“运行中 3:12 · 停止 Esc”，不重复正在跑的命令（命令在转录的工具组里）；
// 停止键是唯一的停止入口，发送键不兼作停止。排队消息一行一条，编辑 / 取消。
export function ComposerTray({
  streaming,
  compaction,
  startedAt,
  stopping = false,
  onStop,
  queuedInputs = [],
  withdrawingInputIds = [],
  onEdit,
  onCancel,
  onSendNow,
  runRow = true,
  queueList = true,
}: {
  // 桌面排队列表：每条可“立即发送”（打断当前这一步，这条马上送达）。
  onSendNow?: (inputId: string) => void
  // 手机不画排队列表（排队消息在对话里是标着“排队中”的气泡）。
  queueList?: boolean
  // 手机照参考稿不画运行行：运行状态看页头与代理头像旁的转圈，停止在发送键上（压缩上下文照常显示）。
  runRow?: boolean
  streaming?: boolean
  compaction?: EntityRecord | null
  startedAt?: string | null
  stopping?: boolean
  onStop?: () => void
  queuedInputs?: EntityRecord[]
  withdrawingInputIds?: string[]
  onEdit?: (inputId: string) => Promise<void> | void
  onCancel?: (inputId: string) => Promise<void> | void
}) {
  const { t } = useI18n()
  const compacting = Boolean(compaction?.active)
  const running = (Boolean(streaming) && runRow) || compacting
  const listed = queueList ? queuedInputs : []
  if (!running && !listed.length) return null
  const queueLabel = t('chat:focusSession.sentToTheRunningAgent')
  return (
    <div
      className="composer-tray mx-3.5 flex min-w-0 flex-col overflow-hidden rounded-t-(--r-lg) border border-b-0 border-stroke bg-well text-content-secondary max-[650px]:mx-2 [&>*+*]:border-t [&>*+*]:border-stroke"
      data-composer-tray
    >
      {running && (
        <div className={TRAY_ROW_CLASS} data-run-bar={compacting ? 'compacting' : 'running'}>
          <span role="status" className="flex min-w-0 flex-1 items-center gap-2">
            {compacting ? (
              <OrbitSpinner className="size-3.5 flex-none text-content-secondary" />
            ) : (
              <StatusGlyph status="run" className="size-4" />
            )}
            <span className="shrink-0 text-content">
              {compacting
                ? t('chat:focusSession.compactingContext')
                : t('chat:focusSession.running')}
            </span>
            {streaming && <TrayClock startedAt={startedAt} />}
          </span>
          {streaming && onStop && (
            <button
              type="button"
              className="inline-flex h-[26px] shrink-0 cursor-pointer items-center gap-1.5 rounded-sm px-2 text-[13px] text-content transition-colors hover:bg-surface-hover disabled:cursor-default disabled:opacity-60 max-[650px]:h-11 max-[650px]:min-w-[72px] max-[650px]:justify-center pointer-coarse:h-11 pointer-coarse:min-w-[72px]"
              aria-label={stopping ? t('chat:focusSession.stopping') : t('chat:focusSession.stop')}
              title={t('chat:focusSession.stopRunShortcut')}
              aria-busy={stopping || undefined}
              disabled={stopping}
              onClick={onStop}
              data-run-stop
            >
              <Square size={10} fill="currentColor" aria-hidden="true" />
              {stopping ? t('chat:focusSession.stopping') : t('chat:focusSession.stop')}
              <Kbd className="max-[650px]:hidden pointer-coarse:hidden">Esc</Kbd>
            </button>
          )}
        </div>
      )}
      {listed.length > 0 && (
        <ul
          className="m-0 flex max-h-[136px] min-w-0 list-none flex-col overflow-x-hidden overflow-y-auto overscroll-contain p-0 [&>*+*]:border-t [&>*+*]:border-stroke-soft"
          aria-label={queueLabel}
          data-pisper-queue-size={listed.length}
          tabIndex={listed.length > 3 ? 0 : undefined}
        >
          {listed.map((item, index) => {
            const inputId = typeof item.id === 'string' ? item.id : ''
            const pending = withdrawingInputIds.includes(inputId)
            const unavailable = !inputId || pending
            const text = String(item.text || '')
            return (
              <li
                key={inputId || `${item.behavior}-${index}`}
                className={cn(TRAY_ROW_CLASS, 'group/queued')}
                aria-busy={pending || undefined}
              >
                {/* 运行中 ↵ 排队（followUp，本轮结束后再交给代理）与 ⌥↵ 插话（steer，下一步就读到）分开标注。 */}
                <span
                  className="inline-flex h-[18px] flex-none items-center rounded-xs border border-stroke bg-solid px-1.5 text-[11.5px] text-content-muted"
                  data-queued-behavior={item.behavior === 'steer' ? 'steer' : 'followUp'}
                >
                  {item.behavior === 'steer'
                    ? t('board:composer.queuedSteer')
                    : t('board:composer.queuedFollowUp')}
                </span>
                <span
                  className="min-w-0 flex-1 truncate text-content [overflow-wrap:anywhere]"
                  title={text}
                >
                  {text}
                </span>
                {onSendNow && (
                  <button
                    type="button"
                    className={TRAY_ACTION_CLASS}
                    aria-label={t('board:composer.sendQueuedNow')}
                    title={t('board:composer.sendQueuedNow')}
                    disabled={unavailable}
                    data-queued-send-now
                    onClick={() => onSendNow(inputId)}
                  >
                    <CornerDownLeft size={14} aria-hidden="true" />
                  </button>
                )}
                {onEdit && (
                  <button
                    type="button"
                    className={TRAY_ACTION_CLASS}
                    aria-label={t('board:composer.editQueued')}
                    title={t('board:composer.editQueued')}
                    disabled={unavailable}
                    data-queued-edit
                    onClick={() => void onEdit(inputId)}
                  >
                    <Pencil size={14} aria-hidden="true" />
                  </button>
                )}
                {onCancel && (
                  <button
                    type="button"
                    className={TRAY_ACTION_CLASS}
                    aria-label={t('board:composer.cancelQueued')}
                    title={t('board:composer.cancelQueued')}
                    disabled={unavailable}
                    data-queued-cancel
                    onClick={() => void onCancel(inputId)}
                  >
                    <X size={14} aria-hidden="true" />
                  </button>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}

export function ComposerResourceChip({
  invocation,
  onRemove,
}: {
  invocation: ResourceInvocation
  onRemove: () => void
}) {
  const { t } = useI18n()
  return (
    <div
      className={`composer-resource-chip [&.workflow]:border-success-border [&.workflow]:bg-success-soft [&.workflow]:text-success-strong [&_button]:grid [&_button]:size-6 [&_button]:place-items-center [&_button]:rounded-xs [&_button]:text-content-muted [&_button]:hover:bg-surface-hover [&_button]:hover:text-content inline-flex h-7 self-start items-center gap-1.5 rounded-sm border border-star-border bg-star-soft pr-0.5 pl-2 text-xs font-medium text-star-strong max-[650px]:h-11 max-[650px]:[&_button]:size-11 ${invocation.kind}`}
    >
      {invocation.kind === 'tool' ? <Wrench size={13} /> : <Braces size={13} />}
      <span>
        {invocation.kind === 'skill'
          ? 'Skill'
          : invocation.kind === 'tool'
            ? t('chat:resourcePicker.tool')
            : t('chat:resourcePicker.workflow')}{' '}
        · {invocation.resourceName}
      </span>
      <button type="button" aria-label={t('chat:resourcePicker.remove')} onClick={onRemove}>
        <X size={13} />
      </button>
    </div>
  )
}

export function ComposerSendButton({
  streaming,
  queueing,
  submitting = false,
  stopping = false,
  preparing = false,
  hasDraft = false,
  canQueue = false,
  blockedReason,
  disabled,
  onLongPress,
  longPressHint,
  onStop,
}: {
  // 手机（参考稿）：运行中输入框为空时发送键就是停止键；输入了文字又变回发送（排队，长按插话）。
  onStop?: () => void
  streaming?: boolean
  queueing: boolean
  submitting?: boolean
  stopping?: boolean
  preparing?: boolean
  hasDraft?: boolean
  canQueue?: boolean
  blockedReason?: string
  disabled: boolean
  // 触屏长按（≥500ms）的替代动作：手机会话页运行中长按发送键是“插话”（触屏没有 ⌥↵）。
  onLongPress?: () => void
  longPressHint?: string
}) {
  const { t } = useI18n()
  const sendShortcut = useShortcutLabel('sendMessage')
  const pressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  // 长按已经触发时，松手后的 click 不再提交第二次。
  const longPressed = useRef(false)
  const clearPress = () => {
    if (pressTimer.current) clearTimeout(pressTimer.current)
    pressTimer.current = null
  }
  useEffect(() => {
    const timer = pressTimer
    return () => {
      if (timer.current) clearTimeout(timer.current)
    }
  }, [])
  // 桌面发送键只负责发送；运行中有草稿时把补充说明排入队列，停止由运行条上的停止键负责。
  const stopMode = Boolean(onStop && streaming && !hasDraft)
  const waitingForDraft = Boolean(streaming && !(hasDraft && canQueue)) && !stopMode
  const busy = queueing || submitting || stopping || preparing
  const unavailable = disabled || busy || waitingForDraft || Boolean(blockedReason)
  const label = preparing
    ? t('chat:focusSession.preparingAttachments')
    : stopping
      ? t('chat:focusSession.stopping')
      : stopMode
        ? t('chat:focusSession.stop')
        : t('chat:focusSession.sendMessage')
  return (
    <button
      type={stopMode ? 'button' : 'submit'}
      data-send-mode={stopMode ? 'stop' : 'send'}
      // 石墨圆键（规格 4.3）：不兼作停止；草稿为空时淡出但保留形状（手机上换成浅灰圆底、深色箭头）。
      className={`send-button grid size-8 min-w-8 flex-none cursor-pointer place-items-center rounded-full border border-transparent bg-star text-on-accent transition-[background-color,opacity] select-none [-webkit-touch-callout:none] hover:enabled:bg-star-hover active:enabled:translate-y-px disabled:cursor-not-allowed disabled:opacity-30 aria-disabled:opacity-30 max-[650px]:disabled:bg-surface-muted max-[650px]:disabled:text-content max-[650px]:disabled:opacity-100 max-[650px]:aria-disabled:bg-surface-muted max-[650px]:aria-disabled:text-content max-[650px]:aria-disabled:opacity-100 max-[650px]:dark:disabled:bg-surface-active max-[650px]:dark:aria-disabled:bg-surface-active data-[send-mode=stop]:bg-surface-muted data-[send-mode=stop]:text-content data-[send-mode=stop]:hover:enabled:bg-surface-hover dark:data-[send-mode=stop]:bg-surface-active max-[650px]:size-9 max-[650px]:min-w-9 min-[651px]:pointer-coarse:size-11 min-[651px]:pointer-coarse:min-w-11 ${COMPOSER_PHONE_TARGET_CLASSES}`}
      title={blockedReason || (!busy && sendShortcut ? `${label} (${sendShortcut})` : label)}
      aria-label={blockedReason ? `${label}: ${blockedReason}` : label}
      aria-busy={busy || undefined}
      aria-disabled={unavailable || undefined}
      aria-description={onLongPress && !stopMode ? longPressHint : undefined}
      data-long-press={onLongPress && !stopMode ? 'steer' : undefined}
      onPointerDown={(event) => {
        longPressed.current = false
        if (!onLongPress || stopMode || unavailable || event.pointerType === 'mouse') return
        clearPress()
        pressTimer.current = setTimeout(() => {
          pressTimer.current = null
          longPressed.current = true
          onLongPress()
        }, 500)
      }}
      onPointerUp={clearPress}
      onPointerCancel={clearPress}
      onPointerLeave={clearPress}
      // 长按时不弹系统菜单（Android 长按约 500ms 会派发 contextmenu）。
      onContextMenu={(event) => {
        if (onLongPress) event.preventDefault()
      }}
      onClick={(event) => {
        if (longPressed.current) {
          longPressed.current = false
          event.preventDefault()
          return
        }
        if (unavailable) {
          event.preventDefault()
          return
        }
        if (stopMode) onStop?.()
      }}
      disabled={!blockedReason && unavailable}
    >
      {busy ? (
        <RefreshCw className={`animate-spin ${COMPOSER_PHONE_GLYPH_CLASSES}`} size={17} />
      ) : stopMode ? (
        <Square size={13} fill="currentColor" aria-hidden="true" />
      ) : (
        <ArrowUp size={17} strokeWidth={2.1} className={COMPOSER_PHONE_GLYPH_CLASSES} />
      )}
    </button>
  )
}
