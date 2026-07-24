import { lazy, memo, Suspense, useEffect, useState } from 'react'
import { Download, File, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AgentStatusAvatar } from '@/components/AgentStatusAvatar'
import MarkdownMessage from '@/components/MarkdownMessage'
import type { ChatAttachment, ChatMessage } from '@/types/chat'
import type { AgentRunActivityProps } from './AgentRunActivity'
import { splitAssistantStreamText } from './stream-text'
import { Message as AiMessage } from '@/components/ai-elements/message-shell'

const agentRunActivityModule = import('./AgentRunActivity')
const AgentRunActivity = lazy(() => agentRunActivityModule)

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
      <img src={source} alt={attachment.name || t('chat:chatMessage.generatedImage')} />
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
                <img src={source} alt={attachment.name || t('chat:chatMessage.imageAttachment')} />
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
}

function focusPropsEqual(prev: FocusChatMessageProps, next: FocusChatMessageProps) {
  return (
    prev.message === next.message &&
    prev.agentState === next.agentState &&
    prev.showRunActivity === next.showRunActivity &&
    prev.runProps === next.runProps
  )
}

export const FocusChatMessage = memo(function FocusChatMessage({
  message,
  agentState,
  showRunActivity,
  runProps,
}: FocusChatMessageProps) {
  const hasTools = Boolean(runProps?.tools?.length)
  const streaming = Boolean(message.streaming)
  const fullText = message.text || ''
  const split = splitAssistantStreamText(fullText, message.streamPreamble, {
    streaming,
    hasTools,
  })
  const showSplit = streaming && hasTools && split.mode === 'split' && Boolean(split.lead)
  const activityProps = runProps
    ? { ...runProps, text: showSplit ? split.body || split.lead : fullText }
    : runProps

  return (
    <AiMessage
      from={message.role === 'agent' ? 'assistant' : 'user'}
      className={`message ${message.role} ${message.error ? 'has-error' : ''}`}
      data-vesper-message-id={message.id}
      data-vesper-role={message.role}
      data-vesper-streaming={streaming || undefined}
      data-vesper-error={message.error ? 'true' : undefined}
    >
      <span>{message.role === 'agent' ? <AgentStatusAvatar state={agentState} /> : 'You'}</span>
      <div className={`message-content ${showSplit ? 'has-stream-split' : ''}`}>
        {showSplit && (
          <div className="stream-lead">
            <MarkdownMessage streaming={false}>{split.lead}</MarkdownMessage>
          </div>
        )}
        {showRunActivity && activityProps && (
          <Suspense
            fallback={<div className="agent-run-activity agent-run-activity-placeholder" aria-hidden="true" />}
          >
            <AgentRunActivity {...activityProps} />
          </Suspense>
        )}
        {(split.body || (!streaming && fullText) || (!showSplit && (fullText || !streaming))) && (
          <div className={showSplit ? 'stream-body' : undefined}>
            <MarkdownMessage streaming={streaming}>
              {streaming ? (showSplit ? split.body : fullText) : fullText}
            </MarkdownMessage>
          </div>
        )}
        {message.attachments && message.attachments.length > 0 && (
          <MessageAttachments attachments={message.attachments} />
        )}
      </div>
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
      data-vesper-message-id={message.id}
      data-vesper-role={message.role}
      data-vesper-streaming={message.streaming || undefined}
    >
      <span>{message.role === 'agent' ? 'Vesper' : 'You'}</span>
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
