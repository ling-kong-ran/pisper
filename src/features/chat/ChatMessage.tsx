import { memo, useEffect, useState } from 'react'
import { Check, Download, File, GitFork, LoaderCircle, Tag, Trash2, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { ChatAttachment, ChatMessage } from '@/types/chat'
import AgentRunActivity, { type AgentRunActivityProps } from './AgentRunActivity'
import { chatErrorMessage } from './chat-errors'
import { chatApi } from './chat-api'
import { Message as AiMessage } from '@/components/ai-elements/message-shell'

type ImagePreview = { attachment: ChatAttachment; source: string }
type RunProps = AgentRunActivityProps

function ImageLightbox({ attachment, source, onClose }: ImagePreview & { onClose: () => void }) {
  const { t } = useI18n()
  useEffect(() => {
    const previousOverflow = document.body.style.overflow
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    document.body.style.overflow = 'hidden'
    window.addEventListener('keydown', onKeyDown)
    return () => {
      document.body.style.overflow = previousOverflow
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose])
  return (
    <div
      className="image-lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={t('chat:chatMessage.fullScreenImagePreview')}
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="image-lightbox-toolbar">
        <span title={attachment.name}>
          {attachment.name || t('chat:chatMessage.generatedImage')}
        </span>
        <div>
          <a
            className="button secondary"
            href={attachment.downloadUrl || source}
            download={attachment.name || 'generated-image'}
          >
            <Download size={14} />
            {t('chat:chatMessage.downloadOriginal')}
          </a>
          <button
            type="button"
            className="icon-button"
            aria-label={t('chat:chatMessage.closePreview')}
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </div>
      </div>
      <img
        alt={attachment.name || t('chat:chatMessage.generatedImage')}
        decoding="async"
        src={source}
      />
    </div>
  )
}

export function MessageAttachments({
  attachments,
  compact = false,
}: {
  attachments: ChatAttachment[]
  compact?: boolean
}) {
  const { t } = useI18n()
  const [preview, setPreview] = useState<ImagePreview | null>(null)
  return (
    <>
      <div className={`message-attachments ${compact ? 'compact' : ''}`}>
        {attachments.map((attachment, index) => {
          const key = attachment.id || index
          const source =
            attachment.url ||
            (attachment.data ? `data:${attachment.mimeType};base64,${attachment.data}` : '')
          if (attachment.kind === 'image' && source)
            return (
              <button
                type="button"
                className="generated-media"
                onClick={() => setPreview({ attachment, source })}
                title={t('chat:chatMessage.openFullScreenPreview')}
                key={key}
              >
                <img
                  alt={attachment.name || t('chat:chatMessage.imageAttachment')}
                  decoding="async"
                  loading="lazy"
                  src={source}
                />
                <small>{attachment.name || t('chat:chatMessage.generatedImage')}</small>
              </button>
            )
          if (attachment.kind === 'video' && source)
            return (
              <div className="generated-media video" key={key}>
                <video controls preload="metadata" src={source} />
                <small>{attachment.name || t('chat:chatMessage.generatedVideo')}</small>
              </div>
            )
          return (
            <a
              className="message-file-attachment"
              href={attachment.downloadUrl || undefined}
              key={key}
            >
              <File size={12} />
              {attachment.name || t('chat:chatMessage.fileAttachment')}
            </a>
          )
        })}
      </div>
      {preview && (
        <ImageLightbox
          attachment={preview.attachment}
          source={preview.source}
          onClose={() => setPreview(null)}
        />
      )}
    </>
  )
}

function MessageTreeLabel({ sessionId, entryId }: { sessionId: string; entryId: string }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [label, setLabel] = useState('')
  const [savedLabel, setSavedLabel] = useState('')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (!open) return
    let active = true
    setLoading(true)
    setError('')
    void chatApi
      .getSessionTree(sessionId)
      .then((tree) => {
        if (!active) return
        const currentLabel = tree.nodes.find((node) => node.id === entryId)?.label || ''
        setLabel(currentLabel)
        setSavedLabel(currentLabel)
      })
      .catch((reason) => active && setError(chatErrorMessage(reason)))
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [entryId, open, sessionId])

  const save = async () => {
    if (saving || loading) return
    setSaving(true)
    setError('')
    try {
      const tree = await chatApi.setSessionTreeLabel(sessionId, entryId, label)
      const nextLabel = tree.nodes.find((node) => node.id === entryId)?.label || ''
      setLabel(nextLabel)
      setSavedLabel(nextLabel)
      setOpen(false)
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <button
              type="button"
              className={`icon-button${savedLabel ? ' active' : ''}`}
              aria-label={t('chat:chatMessage.labelThisTurn')}
              data-pisper-label-entry={entryId}
            >
              <Tag size={14} />
            </button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="top" sideOffset={6}>
          {t('chat:chatMessage.labelThisTurn')}
        </TooltipContent>
      </Tooltip>
      <PopoverContent className="message-label-popover" align="end" sideOffset={6}>
        <form
          onSubmit={(event) => {
            event.preventDefault()
            void save()
          }}
        >
          <PopoverTitle>{t('chat:sessionTree.nodeLabel')}</PopoverTitle>
          <Input
            autoFocus
            value={label}
            maxLength={80}
            disabled={loading || saving}
            placeholder={t('chat:sessionTree.labelPlaceholder')}
            onChange={(event) => setLabel(event.target.value)}
          />
          {error && <small className="danger-text">{error}</small>}
          <div className="message-label-actions">
            <Button
              type="button"
              size="icon-sm"
              variant="ghost"
              title={t('chat:sessionTree.removeLabel')}
              aria-label={t('chat:sessionTree.removeLabel')}
              disabled={!label || loading || saving}
              onClick={() => setLabel('')}
            >
              <Trash2 />
            </Button>
            <Button className="message-label-save" type="submit" disabled={loading || saving}>
              {saving ? <LoaderCircle className="spin" /> : <Check />}
              {saving ? t('chat:sessionTree.savingLabel') : t('chat:sessionTree.saveLabel')}
            </Button>
          </div>
        </form>
      </PopoverContent>
    </Popover>
  )
}

type FocusChatMessageProps = {
  sessionId: string
  message: ChatMessage
  agentState: string
  showRunActivity: boolean
  runProps: RunProps | null
  onDerive: (boundaryEntryId: string) => Promise<void> | void
}

function focusPropsEqual(prev: FocusChatMessageProps, next: FocusChatMessageProps) {
  return (
    prev.sessionId === next.sessionId &&
    prev.message === next.message &&
    prev.agentState === next.agentState &&
    prev.showRunActivity === next.showRunActivity &&
    prev.runProps === next.runProps &&
    prev.onDerive === next.onDerive
  )
}

export const FocusChatMessage = memo(function FocusChatMessage({
  sessionId,
  message,
  agentState,
  showRunActivity,
  runProps,
  onDerive,
}: FocusChatMessageProps) {
  const { t } = useI18n()
  const [deriving, setDeriving] = useState(false)
  const streaming = Boolean(message.streaming)
  const fullText = message.text || ''
  const displayText = fullText || (!showRunActivity ? String(message.error || '') : '')

  return (
    <AiMessage
      from={message.role === 'agent' ? 'assistant' : 'user'}
      className={`message ${message.role} ${message.error ? 'has-error' : ''}`}
      data-pisper-message-id={message.id}
      data-pisper-role={message.role}
      data-pisper-streaming={streaming || undefined}
      data-pisper-error={message.error ? 'true' : undefined}
    >
      <span>
        {message.role === 'agent' ? (
          <span className="agent-message-mark" data-state={agentState} aria-label="Pisper">
            <BrandLogo size={20} />
          </span>
        ) : (
          'You'
        )}
      </span>
      <div className="message-content">
        {showRunActivity && runProps && <AgentRunActivity {...runProps} />}
        {displayText && <MarkdownMessage streaming={streaming}>{displayText}</MarkdownMessage>}
        {message.attachments && message.attachments.length > 0 && (
          <MessageAttachments attachments={message.attachments} />
        )}
      </div>
      {message.role === 'agent' && message.turnBoundaryEntryId && !streaming && (
        <div
          className="chat-history-actions message-actions"
          style={{ gridColumn: 2, gridRow: 2, paddingRight: 0 }}
        >
          <MessageTreeLabel sessionId={sessionId} entryId={message.turnBoundaryEntryId} />
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                className="icon-button"
                aria-label={t('chat:chatMessage.deriveFromHere')}
                data-pisper-derive-entry={message.turnBoundaryEntryId}
                disabled={deriving}
                onClick={async () => {
                  const boundaryEntryId = message.turnBoundaryEntryId
                  if (!boundaryEntryId) return
                  setDeriving(true)
                  try {
                    await onDerive(boundaryEntryId)
                  } finally {
                    setDeriving(false)
                  }
                }}
              >
                {deriving ? <LoaderCircle className="spin" size={14} /> : <GitFork size={14} />}
              </button>
            </TooltipTrigger>
            <TooltipContent side="top" sideOffset={6}>
              {t('chat:chatMessage.deriveFromHere')}
            </TooltipContent>
          </Tooltip>
        </div>
      )}
    </AiMessage>
  )
}, focusPropsEqual)

type MiniChatMessageProps = { message: ChatMessage }

function miniPropsEqual(prev: MiniChatMessageProps, next: MiniChatMessageProps) {
  return prev.message === next.message
}

export const MiniChatMessage = memo(function MiniChatMessage({ message }: MiniChatMessageProps) {
  return (
    <AiMessage
      from={message.role === 'agent' ? 'assistant' : 'user'}
      className={`mini-message ${message.role}`}
      data-pisper-message-id={message.id}
      data-pisper-role={message.role}
      data-pisper-streaming={message.streaming || undefined}
    >
      <span>{message.role === 'agent' ? 'Pisper' : 'You'}</span>
      <div className="mini-message-content">
        {(message.text || !message.streaming) && (
          <MarkdownMessage streaming={message.streaming}>{message.text}</MarkdownMessage>
        )}
        {message.attachments && message.attachments.length > 0 && (
          <MessageAttachments attachments={message.attachments} compact />
        )}
      </div>
    </AiMessage>
  )
}, miniPropsEqual)

/** Stable empty run props for memoized messages that are not the active agent turn. */
export const EMPTY_RUN_PROPS = null
