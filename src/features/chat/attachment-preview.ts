import { apiJson } from '@/lib/api'
import type { ChatAttachment } from '@/types/chat'

export type AttachmentPreview = {
  kind: string
  text?: string
  mimeType?: string
  truncated?: boolean
}

export async function loadAttachmentPreview(
  attachment: ChatAttachment,
  signal?: AbortSignal,
): Promise<AttachmentPreview> {
  // 只读取已登记资产，避免把附件里的任意 URL 或本地路径当作读取授权。
  if (attachment.id && !attachment.id.startsWith('path:')) {
    return apiJson<AttachmentPreview>(
      `/api/assets/${encodeURIComponent(attachment.id)}/content?preview=1`,
      { signal },
    )
  }
  if (typeof attachment.text === 'string') {
    return {
      kind: 'text',
      text: attachment.text.slice(0, 400_000),
      mimeType: attachment.mimeType,
      truncated: attachment.text.length > 400_000,
    }
  }
  return { kind: 'file' }
}
