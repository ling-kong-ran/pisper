// 输入草稿管理：为每个会话保存未发送的草稿（文本+附件），
// 会话切换时恢复，附件选择统一走 useAttachmentSelection。
import { useCallback, useEffect, useRef, useState } from 'react'
import type { ChatAttachment } from '@/types/chat'
import { useAttachmentSelection } from './attachments'

export type ComposerDraft = {
  text: string
  attachments: ChatAttachment[]
}

const MAX_RETAINED_DRAFTS = 64
const drafts = new Map<string, ComposerDraft>()

function copyDraft(draft?: ComposerDraft): ComposerDraft {
  return {
    text: draft?.text || '',
    attachments: [...(draft?.attachments || [])],
  }
}

export function readComposerDraft(sessionId: string): ComposerDraft {
  return copyDraft(drafts.get(sessionId))
}

export function updateComposerDraft(sessionId: string, patch: Partial<ComposerDraft>) {
  if (!sessionId) return
  const next = { ...readComposerDraft(sessionId), ...patch }
  next.attachments = [...next.attachments]
  drafts.delete(sessionId)
  if (!next.text && !next.attachments.length) return
  drafts.set(sessionId, next)
  while (drafts.size > MAX_RETAINED_DRAFTS) {
    const oldest = drafts.keys().next().value
    if (typeof oldest !== 'string') break
    drafts.delete(oldest)
  }
}

export function clearComposerDraft(sessionId: string) {
  drafts.delete(sessionId)
}

export function useComposerDraft(sessionId: string) {
  const initialDraft = useRef(readComposerDraft(sessionId))
  const [value, setValueState] = useState(initialDraft.current.text)
  const saveAttachments = useCallback(
    (attachments: ChatAttachment[]) => updateComposerDraft(sessionId, { attachments }),
    [sessionId],
  )
  const selection = useAttachmentSelection(initialDraft.current.attachments, saveAttachments)
  const replaceAttachments = selection.replaceAttachments
  const clearAttachments = selection.clearAttachments

  useEffect(() => {
    const draft = readComposerDraft(sessionId)
    setValueState(draft.text)
    replaceAttachments(draft.attachments)
  }, [replaceAttachments, sessionId])

  const setValue = useCallback(
    (nextValue: string) => {
      setValueState(nextValue)
      updateComposerDraft(sessionId, { text: nextValue })
    },
    [sessionId],
  )
  const clear = useCallback(() => {
    setValueState('')
    clearAttachments()
    clearComposerDraft(sessionId)
  }, [clearAttachments, sessionId])

  return { value, updateValue: setValue, selection, clearDraft: clear }
}
