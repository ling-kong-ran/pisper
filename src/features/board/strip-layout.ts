// 会话页泳道条的纯布局推导：芯片宽度预算、溢出时的四步收缩顺序（规格 4.2）与 ⌥↑/⌥↓ 的切换顺序。
// 只读芯片数量与可用宽度，不读写任何 store，便于单测钉住收缩顺序。
import type { BoardLane } from '@shared/session-board-status.mjs'

// 芯片宽度（px）：界面用同样的固定宽度渲染，估算与实际一致，收缩判断不依赖逐个测量。
export const STRIP_ATTENTION_CHIP_WIDTH = 220
// 带内联 [拒绝][允许] 的标准审批芯片在有余量时放宽到规格上限 280px（两个 24px 键挤进来后
// 标题仍能显示约 10 个汉字）。放宽只用收缩之后剩下的宽度，不会因此多收一步。
export const STRIP_ATTENTION_ACTION_CHIP_WIDTH = 280
// 单行时只剩标题 + 状态词（+ 24px 操作键），收窄到 180px；带两个内联键的有余量时放宽到 220px。
export const STRIP_ATTENTION_CHIP_ONE_LINE_WIDTH = 180
export const STRIP_ATTENTION_ACTION_ONE_LINE_WIDTH = 220
export const STRIP_RUNNING_CHIP_WIDTH = 200
export const STRIP_RUNNING_TITLE_WIDTH = 140
// 当前会话的运行中芯片带“当前”标记，不按“只留标题”压到 140：按内容在 140–220 之间伸缩，
// 预算按标题估算（stripCurrentChipWidth）。
export const STRIP_CURRENT_CHIP_MAX_WIDTH = 220
// 当前芯片除标题外的固定部分：左右内边距与描边 18、图标 14、两个间距 12、“当前”标记 32。
const STRIP_CURRENT_CHIP_CHROME = 76
export const STRIP_GROUP_HEADER_WIDTH = 36
export const STRIP_CAPSULE_WIDTH = 104
export const STRIP_GAP = 6
// 需要你超过 2 个时，从第 3 个开始收成“+N 需要你”胶囊。
export const STRIP_ATTENTION_KEEP = 2

export type StripRunningMode = 'full' | 'title' | 'capsule'

export type StripLayoutInput = {
  // 芯片区可用宽度（不含返回键与“全部代理”键）。
  width: number
  attention: number
  // 每个需要你芯片（按显示顺序）是否带两个内联操作键（标准审批）；缺省按不带处理。
  attentionActions?: readonly boolean[]
  // 当前会话芯片的估算宽度（stripCurrentChipWidth）；缺省按“只留标题”的 140。
  currentWidth?: number
  running: number
  // 当前会话在运行中泳道：收成胶囊时它的芯片保留在外面。
  currentRunning: boolean
  // 排队 / 已完成胶囊是否出现（数量为 0 或能力缺失时不占位）。
  queued: boolean
  done: boolean
  // 36px 单行模式：需要你芯片本来就只有一行。
  compact: boolean
}

export type StripLayout = {
  // 0 = 不收缩；1–4 依次对应规格 4.2 的四步。
  level: 0 | 1 | 2 | 3 | 4
  runningMode: StripRunningMode
  attentionLines: 1 | 2
  // 直接显示的需要你芯片数；其余进“+N 需要你”胶囊。
  attentionShown: number
  // 带内联键的需要你芯片的渲染宽度：收缩形态定下之后，用剩余宽度把它们放宽（不超过上限）。
  actionChipWidth: number
}

// 当前会话芯片的估算宽度：12px 字号下汉字与全角符号按 12、其余按 7 估，夹在 140–220 之间。
export function stripCurrentChipWidth(title: string) {
  let text = 0
  for (const char of title) text += /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef]/u.test(char) ? 12 : 7
  return Math.min(
    STRIP_CURRENT_CHIP_MAX_WIDTH,
    Math.max(STRIP_RUNNING_TITLE_WIDTH, Math.ceil(STRIP_CURRENT_CHIP_CHROME + text)),
  )
}

function groupWidth(chips: number[]) {
  if (!chips.length) return 0
  return STRIP_GROUP_HEADER_WIDTH + chips.reduce((sum, width) => sum + width + STRIP_GAP, 0)
}

// 需要你芯片的基础宽度（收缩判断按它）；带内联键的芯片再用 StripLayout.actionChipWidth 放宽。
export function stripAttentionChipWidth(lines: 1 | 2) {
  return lines === 2 ? STRIP_ATTENTION_CHIP_WIDTH : STRIP_ATTENTION_CHIP_ONE_LINE_WIDTH
}

export function stripRequiredWidth(
  input: StripLayoutInput,
  layout: Omit<StripLayout, 'level' | 'actionChipWidth'>,
) {
  const attentionChips: number[] = Array.from({ length: layout.attentionShown }, () =>
    stripAttentionChipWidth(layout.attentionLines),
  )
  if (input.attention > layout.attentionShown) attentionChips.push(STRIP_CAPSULE_WIDTH)
  // 当前会话的芯片不按“只留标题”压窄（见 STRIP_CURRENT_CHIP_MAX_WIDTH），胶囊形态下它也留在外面。
  const current = input.currentRunning
    ? [Math.max(STRIP_RUNNING_TITLE_WIDTH, input.currentWidth ?? STRIP_RUNNING_TITLE_WIDTH)]
    : []
  const others = Math.max(0, input.running - current.length)
  const runningChips =
    layout.runningMode === 'capsule'
      ? [STRIP_CAPSULE_WIDTH, ...current]
      : layout.runningMode === 'title'
        ? [...Array.from({ length: others }, () => STRIP_RUNNING_TITLE_WIDTH), ...current]
        : Array.from({ length: input.running }, () => STRIP_RUNNING_CHIP_WIDTH)
  const capsules = (input.queued ? 1 : 0) + (input.done ? 1 : 0)
  // 运行中收成胶囊后不再单独显示分组标题（胶囊本身带图标与计数）。
  const runningWidth = !input.running
    ? 0
    : layout.runningMode === 'capsule'
      ? runningChips.reduce((sum, width) => sum + width + STRIP_GAP, 0)
      : groupWidth(runningChips)
  return groupWidth(attentionChips) + runningWidth + capsules * (STRIP_CAPSULE_WIDTH + STRIP_GAP)
}

// 依次尝试四步收缩，返回第一个放得下的形态；都放不下时停在第 4 步（芯片区自己横向裁切，
// “需要你”芯片仍然带标题，不会退化成图标 + 数字）。
export function stripLayout(input: StripLayoutInput): StripLayout {
  const attentionLines: 1 | 2 = input.compact ? 1 : 2
  const steps: Omit<StripLayout, 'actionChipWidth'>[] = [
    { level: 0, runningMode: 'full', attentionLines, attentionShown: input.attention },
    { level: 1, runningMode: 'title', attentionLines, attentionShown: input.attention },
    { level: 2, runningMode: 'capsule', attentionLines, attentionShown: input.attention },
    { level: 3, runningMode: 'capsule', attentionLines: 1, attentionShown: input.attention },
    {
      level: 4,
      runningMode: 'capsule',
      attentionLines: 1,
      attentionShown:
        input.attention > STRIP_ATTENTION_KEEP ? STRIP_ATTENTION_KEEP : input.attention,
    },
  ]
  // 没有运行中芯片时，第 1、2 步不改变任何东西，直接看下一步。
  const chosen =
    steps.find((step) => stripRequiredWidth(input, step) <= input.width) ?? steps[steps.length - 1]
  return { ...chosen, actionChipWidth: actionChipWidth(input, chosen) }
}

// 收缩形态定下后的余量平均分给带内联键的芯片，最多放宽到上限；没有余量时保持基础宽度。
function actionChipWidth(input: StripLayoutInput, layout: Omit<StripLayout, 'actionChipWidth'>) {
  const base = stripAttentionChipWidth(layout.attentionLines)
  const max =
    layout.attentionLines === 2
      ? STRIP_ATTENTION_ACTION_CHIP_WIDTH
      : STRIP_ATTENTION_ACTION_ONE_LINE_WIDTH
  const count = (input.attentionActions ?? [])
    .slice(0, layout.attentionShown)
    .filter(Boolean).length
  if (!count || !Number.isFinite(input.width)) return count ? max : base
  const slack = input.width - stripRequiredWidth(input, layout)
  return Math.max(base, Math.min(max, base + Math.floor(slack / count)))
}

// 一次已发出、界面还没换过去的代理切换：路由在 transition 里提交，URL 已是 to，界面仍是 from。
export type PendingStripSwitch = { from: string; to: string }

// 切换代理时的“当前会话”：上一次切换还没提交（界面仍停在 from）就以它的目标为起点，
// 否则连按 ⌥↓⌥↑ 会从旧会话再算邻居（落到别的代理），回到原会话也会被当成“已在这里”丢掉。
export function effectiveStripCurrent(currentId: string, pending: PendingStripSwitch | null) {
  return pending && pending.from === currentId ? pending.to : currentId
}

// ⌥↑ / ⌥↓ 的切换顺序：与泳道条从左到右一致（需要你 → 运行中 → 排队 → 已完成）。
// 当前会话不在条上时，↓ 去第一个、↑ 去最后一个；只有当前会话一个时不动。
export function adjacentStripSession(
  order: readonly { id: string; lane: BoardLane }[],
  currentId: string,
  direction: 1 | -1,
): string {
  if (!order.length) return ''
  const index = order.findIndex((entry) => entry.id === currentId)
  if (index < 0) return (direction === 1 ? order[0] : order[order.length - 1]).id
  const next = order[(index + direction + order.length) % order.length]
  return next.id === currentId ? '' : next.id
}

const STRIP_LANE_ORDER: readonly BoardLane[] = ['attention', 'running', 'queued', 'done']

export function stripOrder<T extends { id: string; lane: BoardLane }>(items: readonly T[]): T[] {
  return STRIP_LANE_ORDER.flatMap((lane) => items.filter((item) => item.lane === lane))
}
