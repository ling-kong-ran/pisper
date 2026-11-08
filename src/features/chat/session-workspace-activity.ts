// /live 的 workspace 字段（同一文件夹里的协作）：按不可信输入校验，字段不对的条目丢掉；
// 两样都没有时返回 null，会话页什么都不显示。
import type { SessionMessageSender, SessionWorkspaceActivity } from '@/types/chat'

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function text(value: unknown, limit: number) {
  return typeof value === 'string' ? value.slice(0, limit) : ''
}

export function sessionSender(value: unknown): SessionMessageSender | null {
  const item = record(value)
  const id = text(item?.id, 200)
  return id ? { id, title: text(item?.title, 80) } : null
}

export function workspaceActivity(value: unknown): SessionWorkspaceActivity | null {
  const data = record(value)
  if (!data) return null
  const neighbours = (Array.isArray(data.neighbours) ? data.neighbours : [])
    .map(record)
    .filter((item): item is Record<string, unknown> => Boolean(item && text(item.id, 200)))
    .slice(0, 5)
    .map((item) => ({
      id: text(item.id, 200),
      title: text(item.title, 80),
      running: item.running === true,
      fileCount: Number.isSafeInteger(item.fileCount) ? Number(item.fileCount) : 0,
      files: (Array.isArray(item.files) ? item.files : [])
        .map((file) => text(file, 500))
        .filter(Boolean)
        .slice(0, 5),
      lastChangedAt: text(item.lastChangedAt, 40),
    }))
  const messages = (Array.isArray(data.messages) ? data.messages : [])
    .map(record)
    .flatMap((item) => {
      const from = sessionSender(item?.from)
      const id = text(item?.id, 200)
      return item && from && id
        ? [{ id, from, text: text(item.text, 4000), at: text(item.at, 40) }]
        : []
    })
    .slice(0, 20)
  return neighbours.length || messages.length ? { neighbours, messages } : null
}
