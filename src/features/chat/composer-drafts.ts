// 输入草稿管理：为每个会话保存未发送的草稿（文本+附件），
// 会话切换时恢复，附件选择统一走 useAttachmentSelection。
import { useCallback, useEffect, useRef, useState } from 'react'
import type { ChatAttachment } from '@/types/chat'
import { useAttachmentSelection } from './attachments'

export type ComposerDraft = {
  text: string
  attachments: ChatAttachment[]
}

const drafts = new Map<string, ComposerDraft>()
const textRevisions = new Map<string, number>()

export type ComposerSubmission = {
  accept: () => void
}

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
  const previous = readComposerDraft(sessionId)
  if (
    previous.text === next.text &&
    previous.attachments.length === next.attachments.length &&
    previous.attachments.every((item, index) => item === next.attachments[index])
  )
    return
  if (previous.text !== next.text)
    textRevisions.set(sessionId, (textRevisions.get(sessionId) || 0) + 1)
  drafts.delete(sessionId)
  if (!next.text && !next.attachments.length) return
  drafts.set(sessionId, next)
  // 未发送内容只由提交、显式清空或任务删除清理，不能按最近访问数量回收。
}

export function clearComposerDraft(sessionId: string) {
  textRevisions.set(sessionId, (textRevisions.get(sessionId) || 0) + 1)
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
  const selection = useAttachmentSelection(
    initialDraft.current.attachments,
    saveAttachments,
    sessionId,
  )
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

  const captureSubmission = useCallback((): ComposerSubmission => {
    const snapshot = readComposerDraft(sessionId)
    const textRevision = textRevisions.get(sessionId) || 0
    let accepted = false
    return {
      accept() {
        if (accepted) return
        accepted = true
        const current = readComposerDraft(sessionId)
        // 文本用版本而非值比较：删掉又重输相同内容也属于新草稿；附件按提交身份移除。
        const next = {
          text: (textRevisions.get(sessionId) || 0) === textRevision ? '' : current.text,
          attachments: current.attachments.filter((item) => !snapshot.attachments.includes(item)),
        }
        updateComposerDraft(sessionId, next)
        if (!mountedRef.current || activeSessionIdRef.current !== sessionId) return
        setValueState(next.text)
        // 新准备项不属于本次提交，不能用 clearAttachments 取消它们。
        replaceAttachments(next.attachments)
      },
    }
  }, [replaceAttachments, sessionId])

  return {
    value,
    updateValue: setValue,
    selection,
    clearDraft: clear,
    restoreDraft,
    captureSubmission,
  }
}
