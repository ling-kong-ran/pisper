// 记忆页的文案映射：把后端的类型、来源、动作和原因代码变成人话。键都写成字面量，i18n 检查才看得到。
import {
  AlertTriangle,
  Archive,
  BookOpen,
  CheckCheck,
  Clock,
  Eraser,
  FileCode2,
  GitMerge,
  History,
  Lightbulb,
  ListTodo,
  Milestone,
  Pencil,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  Replace,
  RotateCcw,
  SlidersHorizontal,
  Trash2,
  Undo2,
  type LucideIcon,
} from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import type { MemoryNode, MemorySpace, MemoryType } from './memory-types'

export type Translate = (message: string, values?: I18nValues) => string

export const MEMORY_TYPE_ICONS: Record<MemoryType, LucideIcon> = {
  concept: Lightbulb,
  file: FileCode2,
  risk: AlertTriangle,
  preference: SlidersHorizontal,
  decision: Milestone,
  fact: BookOpen,
  task: ListTodo,
}

export function memoryTypeLabel(type: MemoryType | undefined, t: Translate) {
  if (type === 'file') return t('memory:memoryPage.fileType')
  if (type === 'risk') return t('memory:memoryPage.riskType')
  if (type === 'preference') return t('memory:memoryPage.preferenceType')
  if (type === 'decision') return t('memory:memoryPage.decisionType')
  if (type === 'fact') return t('memory:memoryPage.factType')
  if (type === 'task') return t('memory:memoryPage.taskType')
  return t('memory:memoryPage.conceptType')
}

export function memorySourceLabel(sourceType: string | undefined, t: Translate) {
  if (sourceType === 'manual') return t('memory:memoryPage.sourceManual')
  if (sourceType === 'user_confirmed') return t('memory:memoryPage.sourceUserRequest')
  if (sourceType === 'agent') return t('memory:memoryPage.sourceAgent')
  if (sourceType === 'tool_verified') return t('memory:memoryPage.sourceTool')
  if (sourceType === 'external') return t('memory:memoryPage.sourceExternal')
  return t('memory:memoryPage.sourceConversation')
}

export function memoryEvidenceRoleLabel(role: string | undefined, t: Translate) {
  if (role === 'assistant') return t('memory:memoryPage.roleAssistant')
  if (role === 'tool') return t('memory:memoryPage.roleTool')
  if (role === 'external') return t('memory:memoryPage.roleExternal')
  if (role === 'manual') return t('memory:memoryPage.roleManual')
  return t('memory:memoryPage.roleUser')
}

// 状态徽标的文案。中性底：红色只留给删除，琥珀只表示“需要你”（规格 6.1），这里都不是。
export function memoryBadges(node: MemoryNode, t: Translate) {
  return [
    node.pinned && t('memory:memoryPage.pinnedBadge'),
    node.status === 'stale' && t('memory:memoryPage.staleBadge'),
    node.tainted && t('memory:memoryPage.taintedBadge'),
    node.status === 'archived' && t('memory:memoryPage.archivedBadge'),
    node.status === 'superseded' && t('memory:memoryPage.supersededBadge'),
  ].filter((label): label is string => Boolean(label))
}

export function memorySpaceLabel(space: MemorySpace | null | undefined, t: Translate) {
  return space?.kind === 'global' ? t('memory:memoryPage.globalMemorySpace') : space?.name || ''
}

export function formatMemoryTime(value: string | undefined, locale = 'zh-CN') {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(date)
}

export function formatMemoryDate(value: string | undefined, locale = 'zh-CN') {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(date)
}

// 动作代码和图标、文案一一对应。不认识的动作（更新的 Runtime 写的）显示通用文案，不报错。
export function memoryEventPresentation(action: string, t: Translate) {
  const byAction: Record<string, { icon: LucideIcon; label: () => string }> = {
    add: { icon: Plus, label: () => t('memory:memoryPage.eventAdd') },
    supersede: { icon: Replace, label: () => t('memory:memoryPage.eventSupersede') },
    merge: { icon: GitMerge, label: () => t('memory:memoryPage.eventMerge') },
    archive: { icon: Archive, label: () => t('memory:memoryPage.eventArchive') },
    expire: { icon: Clock, label: () => t('memory:memoryPage.eventExpire') },
    stale: { icon: AlertTriangle, label: () => t('memory:memoryPage.eventStale') },
    confirm: { icon: CheckCheck, label: () => t('memory:memoryPage.eventConfirm') },
    restore: { icon: RotateCcw, label: () => t('memory:memoryPage.eventRestore') },
    edit: { icon: Pencil, label: () => t('memory:memoryPage.eventEdit') },
    pin: { icon: Pin, label: () => t('memory:memoryPage.eventPin') },
    unpin: { icon: PinOff, label: () => t('memory:memoryPage.eventUnpin') },
    delete: { icon: Trash2, label: () => t('memory:memoryPage.eventDelete') },
    purge: { icon: Eraser, label: () => t('memory:memoryPage.eventPurge') },
    undo: { icon: Undo2, label: () => t('memory:memoryPage.eventUndo') },
    reinforce: { icon: RefreshCw, label: () => t('memory:memoryPage.eventReinforce') },
    migrate: { icon: History, label: () => t('memory:memoryPage.eventMigrate') },
  }
  const entry = byAction[action]
  return entry
    ? { icon: entry.icon, label: entry.label() }
    : { icon: History, label: t('memory:memoryPage.eventGeneric') }
}

export function memoryActorLabel(actor: string, t: Translate) {
  if (actor === 'user' || actor === 'manual' || actor === 'user_confirmed')
    return t('memory:memoryPage.actorYou')
  if (['consolidator', 'reconcile', 'lifecycle'].includes(actor))
    return t('memory:memoryPage.actorConsolidator')
  if (actor === 'conversation') return t('memory:memoryPage.actorConversation')
  if (actor === 'agent' || actor === 'tool_verified') return t('memory:memoryPage.actorAgent')
  if (actor === 'external') return t('memory:memoryPage.sourceExternal')
  return t('memory:memoryPage.actorSystem')
}

// 原因可能是代码，也可能是模型写的一句话（对账的判定理由）：认识的翻译，其余原样显示。
export function memoryReasonLabel(reason: string, t: Translate) {
  if (reason === 'same_identity') return t('memory:memoryPage.reasonSameIdentity')
  if (reason === 'lower_authority_conflict') return t('memory:memoryPage.reasonLowerAuthority')
  if (reason === 'expired' || reason === 'retention_expired')
    return t('memory:memoryPage.reasonExpired')
  if (reason === 'unused' || reason === 'low_utility')
    return t('memory:memoryPage.reasonLowUtility')
  if (reason === 'user_deleted') return t('memory:memoryPage.reasonUserDeleted')
  if (reason === 'user_requested') return t('memory:memoryPage.reasonUserRequested')
  const promoted = /^promoted (\d+) pending candidates$/.exec(reason)
  if (promoted) return t('memory:memoryPage.reasonPromoted', { count: Number(promoted[1]) })
  return reason
}
