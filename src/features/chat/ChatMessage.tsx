import { memo, useEffect, useState } from 'react'
import { Download, File, GitFork, LoaderCircle, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import MarkdownMessage from '@/components/MarkdownMessage'
import type { ChatAttachment, ChatMessage } from '@/types/chat'
import AgentRunActivity, { type AgentRunActivityProps } from './AgentRunActivity'
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

type FocusChatMessageProps = {
  message: ChatMessage
  agentState: string
  showRunActivity: boolean
  runProps: RunProps | null
  onDerive: (boundaryEntryId: string) => Promise<void> | void
}

function focusPropsEqual(prev: FocusChatMessageProps, next: FocusChatMessageProps) {
  return (
    prev.message === next.message &&
    prev.agentState === next.agentState &&
    prev.showRunActivity === next.showRunActivity &&
    prev.runProps === next.runProps &&
    prev.onDerive === next.onDerive
  )
}

export const FocusChatMessage = memo(function FocusChatMessage({
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
          <button
            type="button"
            className="icon-button"
            title={t('chat:chatMessage.deriveFromHere')}
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
