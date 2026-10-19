import type { ChatAttachment, ChatMessage } from '@/types/chat'

export type TaskSource = {
  id: string
  name: string
  kind: 'image' | 'text' | 'file'
  method: 'asset' | 'path' | 'unresolved'
  available: boolean
  messageId: string
  attachment: ChatAttachment
}

function sourcePath(value: unknown): string {
  if (typeof value !== 'string' || Array.from(value).some((char) => char.charCodeAt(0) < 32))
    return ''
  const path = value.replace(/\\/g, '/')
  if (!path.startsWith('/') && !/^[a-z]:\//i.test(path)) return ''
  // 保留大小写与根路径语义；这里只归一化引用，不授予文件读取权限。
  return path.replace(/\/\.\//g, '/').replace(/\/$/, '') || '/'
}

export function collectTaskSources(messages: readonly ChatMessage[]): TaskSource[] {
  const result: TaskSource[] = []
  const seen = new Set<string>()
  for (const message of messages) {
    if (!Array.isArray(message.attachments)) continue
    for (const [index, candidate] of message.attachments.entries()) {
      if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) continue
      const assetId =
        typeof candidate.id === 'string' &&
        !candidate.id.startsWith('path:') &&
        candidate.id.length <= 512 &&
        !Array.from(candidate.id).some((char) => char.charCodeAt(0) < 32)
          ? candidate.id
          : ''
      const path = sourcePath(candidate.path)
      const method = assetId ? 'asset' : path ? 'path' : 'unresolved'
      const id = assetId
        ? `asset:${assetId}`
        : path
          ? `path:${path}`
          : `unresolved:${message.id}:${index}`
      if (seen.has(id)) continue
      seen.add(id)
      const name =
        typeof candidate.name === 'string' ? candidate.name : path.split('/').pop() || assetId
      const mimeType = typeof candidate.mimeType === 'string' ? candidate.mimeType : ''
      // 只投影校验过的描述字段；下载 URL 根据登记 ID 构造，不透传任意 URL/data。
      const attachment: ChatAttachment = {
        ...(assetId
          ? { id: assetId, downloadUrl: `/api/assets/${encodeURIComponent(assetId)}/download` }
          : {}),
        ...(path ? { id: assetId || `path:${path}`, path } : {}),
        name,
        mimeType,
        ...(typeof candidate.size === 'number' &&
        Number.isFinite(candidate.size) &&
        candidate.size >= 0
          ? { size: candidate.size }
          : {}),
        ...(typeof candidate.text === 'string' ? { text: candidate.text.slice(0, 400_000) } : {}),
      }
      result.push({
        id,
        name,
        method,
        messageId: message.id,
        attachment,
        available: method !== 'unresolved' || typeof candidate.text === 'string',
        kind: mimeType.startsWith('image/')
          ? 'image'
          : mimeType.startsWith('text/')
            ? 'text'
            : 'file',
      })
    }
  }
  return result
}
