// 卡片展示用的纯推导：状态 → 字形与色调、元信息（项目、模型、消息数）、子代理行、时长。
// 只读看板卡片与会话摘要，不读写任何 store；没有数据的字段一律返回空，由界面隐藏。
import {
  COMMAND_GUARD_REASON_PREFIX,
  type BoardLane,
  type BoardStatus,
} from '@shared/session-board-status.mjs'
import type { StatusGlyphKind, StatusTone } from '@/components/app/status-glyph-kinds'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'

export const BOARD_STATUS_GLYPHS: Readonly<Record<BoardStatus, StatusGlyphKind>> = Object.freeze({
  approval: 'approval',
  question: 'question',
  failed: 'failed',
  running: 'run',
  queued: 'queued',
  done: 'done',
  stopped: 'stopped',
})

export const BOARD_LANE_GLYPHS: Readonly<Record<BoardLane, StatusGlyphKind>> = Object.freeze({
  attention: 'approval',
  running: 'run',
  queued: 'queued',
  done: 'done',
})

export const BOARD_LANE_TONES: Readonly<Record<BoardLane, StatusTone>> = Object.freeze({
  attention: 'attn',
  running: 'run',
  queued: 'queue',
  done: 'done',
})

// 卡片色调：已处理的失败仍是红色八边形，只是落在已完成泳道、换成细边。
export function boardCardTone(item: Pick<BoardItem, 'status'>): StatusTone {
  switch (item.status) {
    case 'approval':
    case 'question':
      return 'attn'
    case 'failed':
      return 'fail'
    case 'running':
      return 'run'
    case 'done':
      return 'done'
    default:
      return 'queue'
  }
}

// 左侧状态边宽度（px）：需要处理的 4、运行中 3、其余 2。
export function boardEdgeWidth(item: Pick<BoardItem, 'lane' | 'status'>) {
  if (item.lane === 'attention') return 4
  if (item.status === 'running') return 3
  return 2
}

// 路径最后一段作为项目标签；Windows 盘符根目录保留盘符。
export function projectLabel(cwd: string) {
  const path = cwd.replace(/[\\/]+$/, '')
  return path.split(/[\\/]/).pop() || path
}

// 简写路径：家目录换成 ~，只保留最后两段，长路径在带头里也放得下。
export function shortPath(cwd: string) {
  const normalized = cwd.replace(/\\/g, '/').replace(/\/+$/, '')
  const home = normalized
    .replace(/^\/(?:home|Users)\/[^/]+/, '~')
    .replace(/^[A-Za-z]:\/Users\/[^/]+/, '~')
  const parts = home.split('/').filter(Boolean)
  if (parts.length <= 3) return home || normalized
  return `${parts[0] === '~' ? '~' : ''}/…/${parts.slice(-2).join('/')}`
}

// 模型短名：provider/id 只取 id；没有模型时为空。
export function modelShortLabel(model: unknown) {
  if (typeof model !== 'string' || !model.trim()) return ''
  return model.split('/').at(-1)?.trim() || ''
}

// 消息条数（用户与助手消息）：Runtime 列表只提供这个计数，不提供轮次，因此如实写“N 条消息”。
export function messageCount(item: Pick<BoardItem, 'session'>) {
  const value = item.session.messageCount
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? Math.floor(value) : 0
}

export type BoardSubAgent = { name: string; extra: number }

// 子代理行：只有 Runtime 列表的 agents（排队 / 启动 / 运行中的子代理）有数据时显示。
export function boardSubAgent(item: Pick<BoardItem, 'session'>): BoardSubAgent | null {
  const agents = Array.isArray(item.session.agents) ? item.session.agents : []
  const named = agents
    .map((agent) => {
      if (!agent || typeof agent !== 'object') return ''
      const record = agent as Record<string, unknown>
      const name = record.taskName ?? record.canonicalName ?? record.id
      return typeof name === 'string' ? name.trim() : ''
    })
    .filter(Boolean)
  if (!named.length) return null
  return { name: named[0], extra: named.length - 1 }
}

// 需要你泳道的等待时长、运行中的已运行时长、已完成的完成时间：都从同一个起点推导。
export function elapsedSince(since: number | null, now: number) {
  return since === null ? null : Math.max(0, now - since)
}

// 守卫原因形如“命令守卫拦截：改写远端分支”；危险等级标签只取冒号后的摘要。
export function guardReasonSummary(reason: string) {
  const text = reason.startsWith(COMMAND_GUARD_REASON_PREFIX)
    ? reason.slice(COMMAND_GUARD_REASON_PREFIX.length).replace(/^[\s:：·-]+/u, '')
    : reason
  return text.split(/[\n。；;]/u)[0]?.trim() || ''
}

// 审批的完整内容：bash 是命令本身；其余工具（插件安装、浏览器交互、MCP 等）没有命令和路径时，
// 退回为格式化后的完整参数。批准前必须能看到全部内容，界面只按行数折叠，从不截断文本。
// 参数按“键: 值”逐行列出（字符串原样，其余取紧凑 JSON），比缩进 JSON 更省行，3 行内能看到更多内容。
export function approvalArgsText(approval: Pick<BoardApproval, 'command' | 'args'> | null) {
  if (!approval) return ''
  if (approval.command) return approval.command
  return Object.entries(approval.args)
    .map(([key, value]) => {
      let text: string
      try {
        text = typeof value === 'string' ? value : (JSON.stringify(value) ?? String(value))
      } catch {
        text = String(value)
      }
      return `${key}: ${text}`
    })
    .join('\n')
}

// 守卫原因进危险等级标签；其他原因（插件安装、浏览器交互等）单独成行展示，帮助判断。
export function isGuardReason(reason: string) {
  return reason.trimStart().startsWith(COMMAND_GUARD_REASON_PREFIX)
}

// 同名项目（worktree、fork 很常见）只靠显示名分不清：重名时返回简写路径作为辅助说明。
export function projectDisambiguation(
  projects: readonly { key: string; cwd: string; label: string }[],
) {
  const counts = new Map<string, number>()
  for (const project of projects) counts.set(project.label, (counts.get(project.label) ?? 0) + 1)
  const hints = new Map<string, string>()
  for (const project of projects)
    if ((counts.get(project.label) ?? 0) > 1 && project.cwd)
      hints.set(project.key, shortPath(project.cwd))
  return hints
}

// 尾巴里哪些行算“失败行”（标红）：测试框架的失败计数、FAIL / ERR! 前缀、✗ 标记、error / panic 等关键词，
// 以及非零退出码。“0 failed / 0 errors”这类成功摘要不标红。只影响颜色，文本原样显示。
export function tailLineFailed(line: string) {
  const text = line.trim()
  if (!text) return false
  if (/\b[1-9]\d* (?:failed|failing|errors?)\b/i.test(text)) return true
  if (/\b0 (?:failed|failing|errors?)\b/i.test(text)) return false
  return /^FAIL\b|\bERR!|^[✗✘×]|\b(?:error|failed|failure|fatal|panic|exception|traceback)\b|\bexit (?:code )?[1-9]/i.test(
    text,
  )
}

// 上下文用量（card.contextPct）显示为整数百分比；没有数据时返回 null，元信息行退回“N 条消息”。
export function contextPercent(item: Pick<BoardItem, 'contextPct'>) {
  const value = item.contextPct
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.min(100, Math.max(0, Math.round(value)))
    : null
}

// 改动统计只在确知（known / partial）且确有文件改动时显示；unavailable 不再去读 /change-summary。
export function visibleChangeStats(item: Pick<BoardItem, 'changeStats'>) {
  const stats = item.changeStats
  if (!stats || (stats.status !== 'known' && stats.status !== 'partial') || !stats.files)
    return null
  return { files: stats.files, added: stats.added ?? 0, removed: stats.removed ?? 0 }
}
