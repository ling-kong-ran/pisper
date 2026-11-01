// 会话树导航的确认与撤销（规格 v2 第 4.5 节老问题 2、3）：
// - “从此处继续”、切到标记所在的分支都会改变当前分支，完成后提示里给“撤销”（有动作的提示显示 10 秒）。
// - 打开不在当前分支上的标记先确认“这个标记在另一条分支上”，不再悄悄切换。
// 撤销 = 把导航响应里的 restoreEntryId 再交给导航接口（Runtime 只加了这个字段，接口本身不变）。
// 本模块只在用户操作时动态加载（⌘K、会话树对话框、聊天页），不进入口包。
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import {
  chatApi,
  type FileRewindPreview,
  type FileRewindResult,
  type SessionTreeNavigationResult,
} from './chat-api'
import { chatErrorMessage } from './chat-errors'
import { announceSessionBranchChanged } from './events'
import { applySessionTreeResponse, invalidateSessionTreeSummary } from './session-tree-summary'

type Translate = (key: string, options?: Record<string, unknown>) => string
type TreeActionDeps = { notify: Notify; t: Translate }

// 撤销目标来自 Runtime 响应：只认非空字符串并限制长度，其余视为“不能撤销”。
export function normalizeRestoreEntryId(value: unknown) {
  if (typeof value !== 'string') return ''
  const id = value.trim()
  return id && id.length <= 200 ? id : ''
}

// 撤销一次树导航。完成后广播分支变化，打开着这个会话的聊天页重读消息；撤销本身不再提供撤销。
export async function undoTreeNavigation(
  sessionId: string,
  restoreEntryId: string,
  { notify, t }: TreeActionDeps,
) {
  try {
    const result = await chatApi.navigateSessionTree(sessionId, restoreEntryId, false)
    applySessionTreeResponse(sessionId, result)
    announceSessionBranchChanged(sessionId)
    notify(t('chat:sessionTree.undone'))
    return true
  } catch (error) {
    notify(chatErrorMessage(error), 'error')
    return false
  }
}

// 导航成功后的提示：Runtime 给了撤销目标才带“撤销”按钮（旧 Runtime 或无法原样恢复时只提示）。
export function notifyTreeNavigation(
  sessionId: string,
  result: Pick<SessionTreeNavigationResult, 'restoreEntryId'> | null | undefined,
  message: string,
  deps: TreeActionDeps,
) {
  const restoreEntryId = normalizeRestoreEntryId(result?.restoreEntryId)
  deps.notify(
    message,
    'success',
    restoreEntryId
      ? {
          action: {
            label: deps.t('chat:sessionTree.undo'),
            onAction: () => void undoTreeNavigation(sessionId, restoreEntryId, deps),
          },
        }
      : undefined,
  )
}

export function otherBranchConfirmOptions(t: Translate): ConfirmDialogOptions {
  return {
    title: t('chat:sessionTree.otherBranchTitle'),
    message: t('chat:sessionTree.otherBranchMessage'),
    confirmLabel: t('chat:sessionTree.switchBranch'),
    tone: 'primary',
  }
}

// 打开另一条分支上的标记：确认 → 切换 → 打开并定位到那个回合 → 提示可撤销。
// 用户取消时返回 false，不改任何东西。打开带定位目标时聊天页会强制重读消息，所以这里不再广播。
export async function switchToTurnOnOtherBranch({
  sessionId,
  entryId,
  confirm,
  open,
  notify,
  t,
}: TreeActionDeps & {
  sessionId: string
  entryId: string
  confirm: (options: ConfirmDialogOptions) => Promise<boolean>
  open: (sessionId: string, entryId: string) => void
}) {
  if (!(await confirm(otherBranchConfirmOptions(t)))) return false
  const result = await chatApi.navigateSessionTreeTarget(sessionId, entryId)
  if (result.cancelled) return false
  void invalidateSessionTreeSummary(sessionId)
  open(sessionId, entryId)
  notifyTreeNavigation(sessionId, result, t('chat:sessionTree.switchedBranch'), { notify, t })
  return true
}

const FILE_REWIND_LIST_LIMIT = 8

// “回到这里（含文件）”的确认：列出会恢复/删除的文件和无法回退的文件，并说明覆盖范围。
export function fileRewindConfirmOptions(
  preview: FileRewindPreview,
  t: Translate,
): ConfirmDialogOptions {
  const changes = preview.files.filter((item) => item.action !== 'unavailable')
  const unavailable = preview.files.filter((item) => item.action === 'unavailable')
  const lines: string[] = []
  if (changes.length) {
    lines.push(
      t('chat:fileRewind.summary', {
        count: changes.length,
        restore: changes.filter((item) => item.action === 'restore').length,
        remove: changes.filter((item) => item.action === 'delete').length,
      }),
    )
    for (const item of changes.slice(0, FILE_REWIND_LIST_LIMIT))
      lines.push(
        item.action === 'delete'
          ? t('chat:fileRewind.deleteItem', { path: item.path })
          : t('chat:fileRewind.restoreItem', { path: item.path }),
      )
    if (changes.length > FILE_REWIND_LIST_LIMIT)
      lines.push(t('chat:fileRewind.more', { count: changes.length - FILE_REWIND_LIST_LIMIT }))
  } else {
    lines.push(t('chat:fileRewind.nothing'))
  }
  if (unavailable.length)
    lines.push(
      t('chat:fileRewind.unavailable', {
        count: unavailable.length,
        paths: unavailable
          .slice(0, FILE_REWIND_LIST_LIMIT)
          .map((item) => item.path)
          .join(', '),
      }),
    )
  if (preview.incomplete) lines.push(t('chat:fileRewind.incomplete'))
  lines.push(t('chat:fileRewind.scope'))
  return {
    title: t('chat:fileRewind.title'),
    message: lines.join('\n'),
    confirmLabel: changes.length
      ? t('chat:fileRewind.confirm')
      : t('chat:fileRewind.confirmChatOnly'),
    tone: changes.length ? 'danger' : 'primary',
  }
}

export function fileRewindNotice(result: FileRewindResult, t: Translate) {
  const changed = result.restored.length + result.deleted.length
  const base = changed
    ? t('chat:fileRewind.done', { count: changed })
    : t('chat:fileRewind.doneChatOnly')
  return result.unavailable.length
    ? `${base} ${t('chat:fileRewind.doneUnavailable', { count: result.unavailable.length })}`
    : base
}
