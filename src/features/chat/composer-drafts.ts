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

export function commandDraft(invocation: string, value: string) {
  const slash = value.match(/^\/[^\s]*(?:\s+([\s\S]*))?$/)
  const argumentsText = (slash ? slash[1] || '' : value).trim()
  return `${invocation}${argumentsText ? ` ${argumentsText}` : ' '}`
}

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

// 撤回不能覆盖正在编辑的草稿，也不能截断此前已经成功入队的附件。
export function mergeComposerDraft(current: ComposerDraft, restored: ComposerDraft): ComposerDraft {
  const attachments = [...current.attachments]
  const ids = new Set(attachments.map((item) => item.id))
  for (const attachment of restored.attachments) {
    let id = attachment.id
    if (!id || ids.has(id)) {
      let suffix = 1
      do {
        id = `${attachment.id || 'attachment'}-restored-${suffix++}`
      } while (ids.has(id))
    }
    ids.add(id)
    attachments.push({ ...attachment, id })
  }
  return {
    text:
      current.text && restored.text
        ? `${current.text}\n${restored.text}`
        : current.text || restored.text,
    attachments,
  }
}

export function useComposerDraft(sessionId: string) {
  const activeSessionIdRef = useRef(sessionId)
  activeSessionIdRef.current = sessionId
  const mountedRef = useRef(true)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])
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
    const current = readComposerDraft(sessionId)
    // 排队请求等待期间可能已输入新内容或撤回另一条消息，只能清空原提交快照。
    if (
      current.text !== value ||
      current.attachments.length !== selection.attachments.length ||
      current.attachments.some((attachment, index) => attachment !== selection.attachments[index])
    )
      return
    clearComposerDraft(sessionId)
    if (!mountedRef.current || activeSessionIdRef.current !== sessionId) return
    setValueState('')
    clearAttachments()
  }, [clearAttachments, selection.attachments, sessionId, value])

  const restoreDraft = useCallback(
    (restored: ComposerDraft) => {
      const next = mergeComposerDraft(readComposerDraft(sessionId), restored)
      updateComposerDraft(sessionId, next)
      // 切换或关闭面板期间仍保存原会话草稿，但不修改当前另一会话的输入区。
      if (!mountedRef.current || activeSessionIdRef.current !== sessionId) return false
      setValueState(next.text)
      replaceAttachments(next.attachments)
      return true
    },
    [replaceAttachments, sessionId],
  )

  return { value, updateValue: setValue, selection, clearDraft: clear, restoreDraft }
}
