// 会话页泳道条（规格 4.2）：看板折叠后的形态，取代被否决的常驻左侧代理栏。
// [看板 ⇧⌘B] | 需要你分组 | 运行中分组 | ◌ 排队 | ✓ 已完成 | 全部代理 ⌘J
// 数据全部来自看板 store（与看板、舰队计量同一个所有者），不新开轮询或事件流。
// 审批经 useStripApprovals：标准审批一步允许，critical 只有“查看”并打开看板同一个确认层。
// 溢出按 strip-layout 的四步收缩；悬停或聚焦芯片时在下方展开 320px 迷你卡片。
// ⌥↑ / ⌥↓ 按条上顺序切换代理（只换路由），⌘J 打开代理面板；三者都读 shortcut-store 的当前绑定，
// 在“设置 → 快捷键”改绑后立即生效。Esc 只关浮层，永远不用于返回。
// 触屏（pointer: coarse）：芯片、胶囊与内联键保持可见尺寸，命中区用伪元素补到 44px（TOUCH_HIT_AREA）。
// 芯片行是横向滚动容器，会把竖向外扩一起裁掉，所以单行条在触屏上从 36px 加高到 48px（行内 47px 放得下 44px）；
// 伪元素只在竖向外扩（芯片、胶囊都宽于 44），不改变芯片行的 scrollWidth，逐级收缩的实测不受影响。
// 内联“拒绝 / 允许 / 查看”可见宽度补到 44，两键之间不靠伪元素横向外扩，避免互相抢点；可见高度撑满芯片内框，
// 44px 热区不会盖住键上下方仍显示为芯片的部分（双行芯片里否则每侧约 7px 会误触批准 / 拒绝）。
// “全部代理”在芯片行之外，只靠伪元素补宽，不挤芯片行的宽度。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent as ReactFocusEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react'
import { ChevronDown, PanelLeftOpen } from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { SCROLL_FADE_CLASS, useScrollFade } from '@/hooks/useScrollFade'
import { formatCompactDuration } from '@/lib/format'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import {
  formatShortcut,
  matchesShortcut,
  optionTypingInEditable,
  shortcutEventBlocked,
} from '@/lib/shortcuts'
import { TOUCH_HIT_AREA, TOUCH_HIT_AREA_PSEUDO } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import { ApprovalConfirm } from './ApprovalConfirm'
import {
  AgentMiniPreview,
  AgentMiniRow,
  StripDetailText,
  StripItemActions,
  type StripItemHandlers,
} from './AgentMiniCard'
import { stripDetail, stripItemHasInlineActions, useStripWords } from './strip-presentation'
import type { BoardItem } from './board-model'
import {
  boardReturnRect,
  freshReturnRect,
  peekBoardReturn,
  rememberBoardReturn,
  type BoardReturnRect,
} from './board-return'
import { boardStatusGlyph, boardCardTone, elapsedSince } from './card-presentation'
import { AgentPanel } from './AgentPanel'
import { LaneGlyph } from './LaneGlyph'
import { useLaneLabels } from './lane-labels'
import {
  STRIP_CURRENT_CHIP_MAX_WIDTH,
  STRIP_RUNNING_TITLE_WIDTH,
  adjacentStripSession,
  stripAttentionChipWidth,
  stripCurrentChipWidth,
  stripLayout,
  stripOrder,
} from './strip-layout'
import { useStripApprovals } from './useBoardController'
import { useBoardItems } from './useBoardItems'
import { motionReduced } from './useLaneMotion'
import { useNow } from './useNow'

const PREVIEW_WIDTH = 320
const PREVIEW_OPEN_DELAY_MS = 280
const PREVIEW_CLOSE_DELAY_MS = 160

export type SessionLaneStripProps = {
  currentId: string
  // 36px 单行模式（≤1279 宽或 ≤760 高）。
  compact: boolean
  // 应用壳提供的看板入口（看板图标 + “看板”，打开看板；主页改为会话后不再是返回键）。
  leading: ReactNode
  notify: Notify
  // 切换代理：只换路由，看板 store 不变。
  onOpenSession: (sessionId: string) => void
  // 切换的起点：上一次切换还没提交时是它的目标（见 effectiveStripCurrent）；不传时用 currentId。
  resolveCurrentId?: () => string
}

const TONE_TEXT = {
  attn: 'text-st-attn',
  fail: 'text-st-fail',
  run: 'text-st-run',
  done: 'text-st-done',
  queue: 'text-st-queue',
} as const

// 打开会话的 FLIP：来源卡片的轮廓从看板位置缩进泳道条里的芯片（只动 transform 与 opacity）。
function playOpenFlip(origin: BoardReturnRect, target: Element) {
  const to = target.getBoundingClientRect()
  if (!to.width || !origin.width || typeof document.body.animate !== 'function') return
  const ghost = document.createElement('div')
  ghost.setAttribute('aria-hidden', 'true')
  ghost.className =
    'pointer-events-none fixed z-[70] rounded-md border border-stroke-strong bg-panel shadow-float'
  Object.assign(ghost.style, {
    left: `${origin.left}px`,
    top: `${origin.top}px`,
    width: `${origin.width}px`,
    height: `${origin.height}px`,
    transformOrigin: 'top left',
  })
  document.body.appendChild(ghost)
  const animation = ghost.animate(
    [
      { transform: 'none', opacity: 1 },
      {
        transform: `translate(${to.left - origin.left}px, ${to.top - origin.top}px) scale(${to.width / origin.width}, ${to.height / origin.height})`,
        opacity: 0.15,
      },
    ],
    { duration: 200, easing: 'cubic-bezier(.19,1,.22,1)' },
  )
  const remove = () => ghost.remove()
  animation.onfinish = remove
  animation.oncancel = remove
}

export function SessionLaneStrip({
  currentId,
  compact,
  leading,
  notify,
  onOpenSession,
  resolveCurrentId,
}: SessionLaneStripProps) {
  const { t } = useI18n()
  const labels = useLaneLabels()
  const word = useStripWords()
  const { items, capabilities, isLoading } = useBoardItems('today')
  const ordered = useMemo(() => stripOrder(items), [items])
  const lanes = useMemo(() => {
    const result: Record<BoardLane, BoardItem[]> = {
      attention: [],
      running: [],
      queued: [],
      done: [],
    }
    for (const item of ordered) result[item.lane].push(item)
    return result
  }, [ordered])
  const attentionAll = lanes.attention
  const approvals = useStripApprovals({ attentionAll, notify })
  const now = useNow(lanes.attention.length + lanes.running.length > 0)
  const rootRef = useRef<HTMLElement>(null)
  const rowRef = useRef<HTMLDivElement>(null)
  const [rowWidth, setRowWidth] = useState(0)
  const [panelOpen, setPanelOpen] = useState(false)
  const [panelMounted, setPanelMounted] = useState(false)
  const panelReturnRef = useRef<HTMLElement | null>(null)
  const currentRef = useRef(currentId)
  currentRef.current = currentId
  const resolveRef = useRef(resolveCurrentId)
  resolveRef.current = resolveCurrentId
  // 键盘与点击切换的起点：界面可能还停在上一个会话（路由 transition 未提交）。
  const switchBase = useCallback(() => resolveRef.current?.() ?? currentRef.current, [])

  const queuedVisible = capabilities.sessionQueue && lanes.queued.length > 0
  const currentRunningEntry = lanes.running.find((item) => item.id === currentId)
  const currentRunning = Boolean(currentRunningEntry)
  // 宽度按固定芯片宽估算；中英混排标题或字体回退时实际仍可能放不下（1024 平板上“已完成”被裁）。
  // 渲染后实测溢出就把最低收缩步抬高一级，直到放得下或到第 4 步；可用宽度、芯片数或标题一变就重新从 0 算。
  const fitKey = [
    rowWidth,
    lanes.attention.length,
    lanes.running.length,
    queuedVisible,
    lanes.done.length > 0,
    currentId,
    currentRunningEntry?.title ?? '',
    compact,
  ].join('|')
  // tight：第 4 步之后仍放不下时的最后一步——运行中 / 排队 / 已完成胶囊只留图标 + 计数（名称在无障碍名里）。
  const [fit, setFit] = useState({ key: '', level: 0, tight: false })
  const tight = fit.key === fitKey && fit.tight
  const layout = stripLayout({
    minLevel: fit.key === fitKey ? fit.level : 0,
    width: rowWidth || Number.POSITIVE_INFINITY,
    attention: lanes.attention.length,
    attentionActions: lanes.attention.map(stripItemHasInlineActions),
    currentWidth: currentRunningEntry
      ? stripCurrentChipWidth(currentRunningEntry.title)
      : undefined,
    running: lanes.running.length,
    currentRunning,
    queued: queuedVisible,
    done: lanes.done.length > 0,
    compact,
  })

  useLayoutEffect(() => {
    const element = rowRef.current
    if (!element) return
    const measure = () => setRowWidth(element.clientWidth)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  // 布局提交后、绘制前测一次：仍溢出就收下一步（同步重渲染，不会闪出被裁的一帧）。
  useLayoutEffect(() => {
    const element = rowRef.current
    if (!element || !rowWidth || tight) return
    if (element.scrollWidth <= element.clientWidth + 1) return
    if (layout.level < 4) setFit({ key: fitKey, level: layout.level + 1, tight: false })
    else setFit({ key: fitKey, level: 4, tight: true })
  }, [fitKey, layout.level, rowWidth, tight])
  // 收到最紧仍放不下（极窄窗口）只能横向滑动：被裁的一端渐隐，提示还有芯片。
  const rowFade = useScrollFade(rowRef, `${fitKey}|${layout.level}|${tight}`)

  const open = useCallback(
    (sessionId: string) => {
      setPanelOpen(false)
      if (!sessionId || sessionId === switchBase()) return
      onOpenSession(sessionId)
    },
    [onOpenSession, switchBase],
  )

  const handlers = useMemo<StripItemHandlers>(
    () => ({
      onOpen: open,
      onApprove: approvals.approve,
      onDeny: approvals.deny,
      onReview: approvals.review,
      onRetry: approvals.retry,
      isPending: approvals.isPending,
    }),
    [
      approvals.approve,
      approvals.deny,
      approvals.isPending,
      approvals.retry,
      approvals.review,
      open,
    ],
  )

  // —— 打开 / 返回的 FLIP ——
  const flipPlayedRef = useRef(false)
  useLayoutEffect(() => {
    if (flipPlayedRef.current || isLoading) return
    flipPlayedRef.current = true
    const state = peekBoardReturn()
    const origin = freshReturnRect(state?.origin)
    if (!state || !origin || state.focusId !== currentId) return
    rememberBoardReturn({ ...state, origin: undefined })
    const chip = rowRef.current?.querySelector(`[data-strip-chip="${CSS.escape(currentId)}"]`)
    if (chip && !motionReduced()) playOpenFlip(origin, chip)
  }, [currentId, isLoading])
  // 离开会话页时记下当前芯片的位置：看板据此把卡片从泳道条展开回去，焦点回到这张卡片。
  useLayoutEffect(
    () => () => {
      const id = currentRef.current
      if (!id) return
      const chip = rowRef.current?.querySelector(`[data-strip-chip="${CSS.escape(id)}"]`)
      const rect = boardReturnRect(chip)
      const previous = peekBoardReturn()
      rememberBoardReturn({
        focusId: id,
        scroll: previous?.scroll ?? {},
        ...(rect ? { from: { ...rect, at: Date.now() } } : {}),
      })
    },
    [],
  )

  // —— 快捷键：⌘J 代理面板，⌥↑ / ⌥↓ 切换代理（默认键，可在设置里改绑或禁用）——
  const panelBinding = useShortcutStore((state) => state.bindings.agentPanel)
  const previousBinding = useShortcutStore((state) => state.bindings.previousAgent)
  const nextBinding = useShortcutStore((state) => state.bindings.nextAgent)
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return
      // 改绑成 ⌥ + 字母时，macOS 输入框里的 ⌥ 组合是在打特殊字符（⌥J → ∆），交还输入框；
      // 默认的 ⌥↑ / ⌥↓ 不产生字符，照常生效。
      if (optionTypingInEditable(event) && event.key.length === 1) return
      if (matchesShortcut(event, panelBinding)) {
        // 面板本身是对话框：打开时同一个键负责关闭。
        if (!panelOpen && shortcutEventBlocked(event)) return
        event.preventDefault()
        if (!panelOpen)
          panelReturnRef.current =
            document.activeElement instanceof HTMLElement ? document.activeElement : null
        setPanelMounted(true)
        setPanelOpen((value) => !value)
        return
      }
      const step = matchesShortcut(event, nextBinding)
        ? 1
        : matchesShortcut(event, previousBinding)
          ? -1
          : 0
      if (!step || shortcutEventBlocked(event)) return
      const next = adjacentStripSession(ordered, switchBase(), step)
      if (!next) return
      event.preventDefault()
      onOpenSession(next)
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [nextBinding, onOpenSession, ordered, panelBinding, panelOpen, previousBinding, switchBase])

  // —— 悬停 / 聚焦预览 ——
  const [preview, setPreview] = useState<{ id: string; left: number } | null>(null)
  const previewTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const clearPreviewTimer = () => {
    if (previewTimer.current) clearTimeout(previewTimer.current)
    previewTimer.current = null
  }
  useEffect(() => clearPreviewTimer, [])
  const showPreview = (id: string, chip: HTMLElement, immediate: boolean) => {
    clearPreviewTimer()
    const place = () => {
      const root = rootRef.current
      if (!root) return
      const box = root.getBoundingClientRect()
      const rect = chip.getBoundingClientRect()
      const left = Math.max(8, Math.min(rect.left - box.left, box.width - PREVIEW_WIDTH - 8))
      setPreview({ id, left })
    }
    if (immediate) place()
    else previewTimer.current = setTimeout(place, PREVIEW_OPEN_DELAY_MS)
  }
  const hidePreview = () => {
    clearPreviewTimer()
    previewTimer.current = setTimeout(() => setPreview(null), PREVIEW_CLOSE_DELAY_MS)
  }
  const previewItem = preview ? ordered.find((item) => item.id === preview.id) : undefined
  useEffect(() => {
    if (!preview) return
    // 捕获阶段先收起预览并消费这次 Esc：不再连带关闭检查器覆盖层，也不会被当成“停止运行”。
    const close = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      event.preventDefault()
      setPreview(null)
    }
    window.addEventListener('keydown', close, true)
    return () => window.removeEventListener('keydown', close, true)
  }, [preview])

  const chipEvents = (id: string) => ({
    onPointerEnter: (event: ReactPointerEvent<HTMLElement>) => {
      if (event.pointerType === 'mouse') showPreview(id, event.currentTarget, false)
    },
    onPointerLeave: hidePreview,
    onFocus: (event: ReactFocusEvent<HTMLElement>) => {
      if (event.currentTarget.contains(event.target) && event.target.matches(':focus-visible'))
        showPreview(id, event.currentTarget, true)
    },
    onBlur: (event: ReactFocusEvent<HTMLElement>) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) hidePreview()
    },
  })

  const attentionShown = lanes.attention.slice(0, layout.attentionShown)
  const attentionRest = lanes.attention.slice(layout.attentionShown)
  const panelShortcut = formatShortcut(panelBinding)
  // 快捷键被禁用时只读“全部代理”，不留空括号。
  const panelLabel = panelShortcut
    ? t('board:strip.allAgentsShortcut', { shortcut: panelShortcut })
    : t('board:strip.allAgents')

  const attentionChip = (item: BoardItem) => {
    const tone = boardCardTone(item)
    const twoLine = layout.attentionLines === 2
    const detail = stripDetail(item, t)
    const current = item.id === currentId
    return (
      <div
        key={item.id}
        data-strip-chip={item.id}
        data-lane="attention"
        data-status={item.status}
        data-severity={item.approvals[0]?.severity}
        data-current={current || undefined}
        {...chipEvents(item.id)}
        style={{
          width: stripItemHasInlineActions(item)
            ? layout.actionChipWidth
            : stripAttentionChipWidth(layout.attentionLines),
        }}
        className={cn(
          'relative flex flex-none items-center gap-1.5 rounded-md border pr-1 pl-2 transition-colors duration-(--d-fast)',
          twoLine ? 'h-10' : 'h-7',
          tone === 'fail'
            ? 'border-st-fail-border bg-st-fail-soft'
            : 'border-st-attn-border bg-st-attn-soft',
          current && 'ring-2 ring-stroke-strong',
        )}
      >
        <button
          type="button"
          className={cn(
            'absolute inset-0 rounded-[inherit] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
            TOUCH_HIT_AREA_PSEUDO,
          )}
          aria-label={t('board:card.label', { title: item.title, status: word(item) })}
          aria-current={current ? 'page' : undefined}
          aria-describedby={preview?.id === item.id ? 'lane-strip-preview' : undefined}
          onClick={() => open(item.id)}
        />
        <StatusGlyph
          status={boardStatusGlyph(item)}
          className={cn('pointer-events-none size-3.5 flex-none', TONE_TEXT[tone])}
        />
        <span className="pointer-events-none grid min-w-0 flex-1">
          <span className="flex min-w-0 items-center gap-1 text-xs leading-4">
            <span className="min-w-0 truncate font-medium text-content">{item.title}</span>
            <span className={cn('flex-none', TONE_TEXT[tone])}>{word(item)}</span>
          </span>
          {twoLine && detail.text && (
            <StripDetailText detail={detail} className="leading-4 text-content-secondary" />
          )}
        </span>
        {/* 触屏上撑满芯片内框高度，让内联键能整列铺满（见 StripItemActions）。 */}
        <span className="relative z-1 flex flex-none items-center gap-0.5 pointer-coarse:self-stretch">
          <StripItemActions item={item} handlers={handlers} />
        </span>
      </div>
    )
  }

  const runningChip = (item: BoardItem, titleOnly: boolean) => {
    const current = item.id === currentId
    const elapsed = elapsedSince(item.startedAt, now)
    return (
      <div
        key={item.id}
        data-strip-chip={item.id}
        data-lane="running"
        data-status={item.status}
        data-current={current || undefined}
        {...chipEvents(item.id)}
        // 当前会话的芯片带“当前”标记：只留标题时也按内容在 140–220 之间伸缩，不把短标题截断。
        style={
          titleOnly
            ? current
              ? { minWidth: STRIP_RUNNING_TITLE_WIDTH, maxWidth: STRIP_CURRENT_CHIP_MAX_WIDTH }
              : { width: STRIP_RUNNING_TITLE_WIDTH }
            : undefined
        }
        className={cn(
          'relative flex flex-none items-center gap-1.5 rounded-md px-2 text-xs transition-colors duration-(--d-fast)',
          !titleOnly && 'max-w-[200px]',
          // 当前会话“凸起”：实底、强描边与一级阴影，高度 32（其余 28），在 52px 条里一眼可辨。
          current
            ? 'h-8 border border-stroke-strong bg-solid text-content shadow-[var(--sh-1)]'
            : 'h-7 text-content-secondary hover:bg-surface-hover hover:text-content',
        )}
      >
        <button
          type="button"
          className={cn(
            'absolute inset-0 rounded-[inherit] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
            TOUCH_HIT_AREA_PSEUDO,
          )}
          aria-label={t('board:card.label', { title: item.title, status: word(item) })}
          aria-current={current ? 'page' : undefined}
          onClick={() => open(item.id)}
        />
        <StatusGlyph status="run" className="pointer-events-none size-3.5 flex-none text-st-run" />
        <span className="pointer-events-none min-w-0 truncate font-medium">{item.title}</span>
        {!titleOnly && elapsed !== null && (
          <span className="pointer-events-none flex-none font-mono text-[11px] text-content-muted tabular-nums">
            {formatCompactDuration(elapsed)}
          </span>
        )}
        {current && (
          <span className="pointer-events-none flex-none rounded-xs bg-surface-muted px-1 text-xs text-content-secondary">
            {t('board:strip.current')}
          </span>
        )}
      </div>
    )
  }

  const capsule = (
    key: string,
    content: ReactNode,
    label: string,
    list: BoardItem[],
    title: string,
    tone: string,
  ) => (
    <Popover key={key}>
      <PopoverTrigger asChild>
        <button
          type="button"
          data-strip-capsule={key}
          aria-label={label}
          className={cn(
            'flex h-7 flex-none items-center gap-1 rounded-full border border-stroke bg-panel px-2.5 text-xs font-medium transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus data-[state=open]:bg-surface-hover',
            TOUCH_HIT_AREA,
            tone,
          )}
        >
          {content}
          <ChevronDown className="size-3 text-content-muted" aria-hidden="true" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[360px] gap-1 p-1.5" aria-label={title}>
        <p className="px-2 pt-1 pb-0.5 text-xs font-medium text-content-muted">{title}</p>
        <div className="grid max-h-[min(60vh,420px)] gap-0.5 overflow-y-auto">
          {list.map((item) => (
            <AgentMiniRow
              key={item.id}
              item={item}
              now={now}
              current={item.id === currentId}
              handlers={handlers}
              onOpen={open}
              word={word(item)}
            />
          ))}
        </div>
      </PopoverContent>
    </Popover>
  )

  return (
    <nav
      ref={rootRef}
      aria-label={t('board:strip.label')}
      data-lane-strip
      data-strip-compact={compact || undefined}
      data-strip-level={layout.level}
      data-strip-tight={tight || undefined}
      className={cn(
        'relative z-20 flex flex-none items-center gap-2 border-b border-stroke-soft bg-chrome px-3',
        compact ? 'h-9 pointer-coarse:h-12' : 'h-[52px]',
      )}
    >
      {/* 单行模式下返回键只留“← 看板”，快捷键提示收进 aria-label，给芯片让出宽度。 */}
      <div
        className={cn('flex flex-none items-center [&>button]:h-7', compact && '[&_kbd]:hidden')}
      >
        {leading}
      </div>
      <span aria-hidden="true" className="h-5 w-px flex-none bg-stroke" />
      <div
        ref={rowRef}
        data-strip-row
        data-fade={rowFade}
        // 四步收缩之后仍放不下时（极窄窗口）横向滚动而不是裁掉芯片；焦点移到芯片时自动滚入视野。
        className={cn(
          'flex h-full min-w-0 flex-1 items-center gap-1.5 overflow-x-auto overflow-y-hidden [scrollbar-width:none] [&::-webkit-scrollbar]:hidden',
          SCROLL_FADE_CLASS,
        )}
      >
        {lanes.attention.length > 0 && (
          <div
            role="group"
            aria-label={`${labels.name.attention} ${lanes.attention.length}`}
            className="flex flex-none items-center gap-1.5"
          >
            <span
              className="relative flex h-7 flex-none items-center gap-1 px-1 text-xs font-semibold text-st-attn tabular-nums"
              aria-hidden="true"
            >
              <LaneGlyph lane="attention" className="size-3.5" />
              {lanes.attention.length}
              <span className="absolute inset-x-1 bottom-0 h-1 rounded-full bg-[image:var(--hatch-attn)]" />
            </span>
            {attentionShown.map(attentionChip)}
            {attentionRest.length > 0 &&
              capsule(
                'attention',
                <span className="text-st-attn">
                  {t('board:strip.moreAttention', { count: attentionRest.length })}
                </span>,
                t('board:strip.moreAttention', { count: attentionRest.length }),
                attentionRest,
                labels.name.attention,
                'border-st-attn-border',
              )}
          </div>
        )}
        {lanes.running.length > 0 && (
          <div
            role="group"
            aria-label={`${labels.name.running} ${lanes.running.length}`}
            className="flex flex-none items-center gap-1.5"
          >
            {layout.runningMode === 'capsule' ? (
              <>
                {capsule(
                  'running',
                  <>
                    <StatusGlyph status="run" spin={false} className="size-3.5 text-st-run" />
                    {tight
                      ? lanes.running.length
                      : t('board:strip.runningCapsule', { count: lanes.running.length })}
                  </>,
                  t('board:strip.runningCapsule', { count: lanes.running.length }),
                  lanes.running,
                  labels.name.running,
                  'text-content',
                )}
                {currentRunningEntry && runningChip(currentRunningEntry, true)}
              </>
            ) : (
              <>
                <span
                  aria-hidden="true"
                  className="flex h-7 flex-none items-center gap-1 px-1 text-xs font-semibold text-st-run tabular-nums"
                >
                  <LaneGlyph lane="running" spin={false} className="size-3.5" />
                  {lanes.running.length}
                </span>
                {lanes.running.map((item) => runningChip(item, layout.runningMode === 'title'))}
              </>
            )}
          </div>
        )}
        {queuedVisible &&
          capsule(
            'queued',
            <>
              <StatusGlyph status="queued" className="size-3.5 text-st-queue" />
              {tight
                ? lanes.queued.length
                : t('board:strip.queued', { count: lanes.queued.length })}
            </>,
            t('board:strip.queued', { count: lanes.queued.length }),
            lanes.queued,
            labels.name.queued,
            'text-content-secondary',
          )}
        {lanes.done.length > 0 &&
          capsule(
            'done',
            <>
              <StatusGlyph status="done" className="size-3.5 text-st-done" />
              {tight ? lanes.done.length : t('board:strip.done', { count: lanes.done.length })}
            </>,
            t('board:strip.done', { count: lanes.done.length }),
            lanes.done,
            t('board:strip.doneTitle'),
            'text-content-secondary',
          )}
        {!isLoading && ordered.length === 0 && (
          <span className="truncate px-1 text-xs text-content-muted">{t('board:strip.empty')}</span>
        )}
      </div>
      <Button
        type="button"
        variant="ghost"
        data-strip-agents
        // 触屏上只剩图标时 34px 宽：可见宽度不变（不挤芯片行的宽度），命中区用伪元素左右各补 5px，
        // 落在与芯片行之间 8px 的间隙和条的右内边距里。
        className={cn('h-7 flex-none gap-1.5 px-2 text-xs', TOUCH_HIT_AREA)}
        aria-label={panelLabel}
        title={panelLabel}
        aria-keyshortcuts={ariaKeyShortcuts(panelBinding)}
        aria-expanded={panelOpen}
        aria-haspopup="dialog"
        onClick={(event) => {
          panelReturnRef.current = event.currentTarget
          setPanelMounted(true)
          setPanelOpen(true)
        }}
      >
        <PanelLeftOpen aria-hidden="true" />
        <span className={cn(compact && 'max-[1180px]:hidden')}>{t('board:strip.allAgents')}</span>
        {/* 窄窗口只留图标：孤零零的键帽像一条走丢的快捷键提示，快捷键改在提示与 aria-label 里。 */}
        {panelShortcut && (
          <Kbd
            aria-hidden="true"
            className={cn('pointer-coarse:hidden', compact && 'max-[1180px]:hidden')}
          >
            {panelShortcut}
          </Kbd>
        )}
      </Button>
      {previewItem && preview && (
        <div
          id="lane-strip-preview"
          role="group"
          aria-label={t('board:strip.previewLabel', { title: previewItem.title })}
          data-strip-preview={previewItem.id}
          className="absolute top-full z-30 mt-1 rounded-md border border-stroke bg-popover p-3 text-popover-foreground shadow-float motion-safe:animate-[fade-in_120ms_var(--ease-enter)]"
          style={{ left: preview.left, width: PREVIEW_WIDTH }}
          onPointerEnter={clearPreviewTimer}
          onPointerLeave={hidePreview}
          onFocus={clearPreviewTimer}
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node | null)) hidePreview()
          }}
        >
          <AgentMiniPreview
            item={previewItem}
            now={now}
            handlers={handlers}
            word={word(previewItem)}
          />
        </div>
      )}
      {panelMounted && (
        <AgentPanel
          open={panelOpen}
          onOpenChange={setPanelOpen}
          lanes={lanes}
          queueAvailable={capabilities.sessionQueue}
          loading={isLoading}
          currentId={currentId}
          now={now}
          handlers={handlers}
          returnFocus={() => panelReturnRef.current}
        />
      )}
      <ApprovalConfirm
        target={approvals.confirm?.target ?? null}
        phone={false}
        pending={approvals.confirm?.pending ?? false}
        error={approvals.confirm?.error ?? ''}
        onConfirm={() => void approvals.confirmStripApproval()}
        onOpenChange={(value) => {
          if (!value) approvals.closeConfirm()
        }}
      />
    </nav>
  )
}
