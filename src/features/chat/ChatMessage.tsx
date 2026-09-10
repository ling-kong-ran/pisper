// 单条聊天消息：Markdown 渲染 + 消息操作（复制/下载/删除/跳转），
// 长代码自动展开，附件与工具调用内嵌展示。
import { memo, useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  AlertTriangle,
  Check,
  ChevronLeft,
  ChevronRight,
  Download,
  File,
  GitFork,
  MessageSquarePlus,
  LoaderCircle,
  RotateCcw,
  Tag,
  Trash2,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import type { ChatAttachment, ChatMessage } from '@/types/chat'
import AgentRunActivity, { type AgentRunActivityProps } from './AgentRunActivity'
import { chatErrorMessage } from './chat-errors'
import { chatApi } from './chat-api'
import { Message as AiMessage } from '@/components/ai-elements/message-shell'

type PreviewImage = { attachment: ChatAttachment; source: string; attachmentIndex: number }
type ImagePreview = { images: PreviewImage[]; index: number }
type RunProps = AgentRunActivityProps

function ImageLightbox({
  images,
  index,
  onClose,
  onNavigate,
}: ImagePreview & { onClose: () => void; onNavigate: (index: number) => void }) {
  const { t } = useI18n()
  const touchStartX = useRef<number | null>(null)
  const image = images[index]
  const hasPrevious = index > 0
  const hasNext = index < images.length - 1
  const navigate = useCallback(
    (nextIndex: number) => {
      if (nextIndex >= 0 && nextIndex < images.length) onNavigate(nextIndex)
    },
    [images.length, onNavigate],
  )

  useEffect(() => {
    const previousOverflow = document.body.style.overflow
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
      else if (event.key === 'ArrowLeft') {
        event.preventDefault()
        navigate(index - 1)
      } else if (event.key === 'ArrowRight') {
        event.preventDefault()
        navigate(index + 1)
      }
    }
    document.body.style.overflow = 'hidden'
    window.addEventListener('keydown', onKeyDown)
    return () => {
      document.body.style.overflow = previousOverflow
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [index, navigate, onClose])

  if (!image) return null
  return (
    <div
      className="image-lightbox [&_img]:h-full [&_img]:min-h-0 [&_img]:w-full [&_img]:object-contain fixed z-[100] inset-0 grid grid-rows-[auto_minmax(0,1fr)] gap-[12px] bg-[var(--lightbox-bg)] [padding:18px_72px_24px_24px] [backdrop-filter:blur(8px)] max-sm:[padding:12px_56px_16px_12px]"
      role="dialog"
      aria-modal="true"
      aria-label={t('chat:chatMessage.fullScreenImagePreview')}
    >
      <div className="image-lightbox-toolbar flex min-w-0 items-center justify-between gap-[16px] text-[var(--on-ink)]">
        <div className="flex min-w-0 items-center gap-[10px]">
          <span
            className="overflow-hidden text-[13px] font-[700] text-ellipsis whitespace-nowrap"
            title={image.attachment.name}
          >
            {image.attachment.name || t('chat:chatMessage.generatedImage')}
          </span>
          {images.length > 1 && (
            <span className="flex-none text-[12px] font-[600] text-[var(--on-ink)]/70">
              {index + 1} / {images.length}
            </span>
          )}
        </div>
        <div className="flex flex-none items-center gap-[8px]">
          <Button
            asChild
            size="lg"
            className="border border-[var(--lightbox-action-border)] bg-[var(--lightbox-action-bg)] text-[var(--lightbox-action-text)] shadow-[0_8px_24px_var(--lightbox-action-shadow)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)]"
          >
            <a
              href={image.attachment.downloadUrl || image.source}
              download={image.attachment.name || 'generated-image'}
              aria-label={t('chat:chatMessage.downloadOriginal')}
              title={t('chat:chatMessage.downloadOriginal')}
            >
              <Download size={14} />
              <span className="max-sm:hidden">{t('chat:chatMessage.downloadOriginal')}</span>
            </a>
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            className="size-11 bg-[var(--lightbox-control-bg)] text-[var(--on-ink)] hover:bg-[var(--lightbox-control-bg)] hover:text-[var(--on-ink)]"
            aria-label={t('chat:chatMessage.closePreview')}
            title={t('chat:chatMessage.closePreview')}
            onClick={onClose}
          >
            <X size={20} />
          </Button>
        </div>
      </div>
      <div
        className="relative flex min-h-0 items-center justify-center [touch-action:pan-y]"
        onTouchStart={(event) => {
          touchStartX.current = event.changedTouches[0]?.clientX ?? null
        }}
        onTouchEnd={(event) => {
          const startX = touchStartX.current
          touchStartX.current = null
          const endX = event.changedTouches[0]?.clientX
          if (startX === null || endX === undefined || Math.abs(endX - startX) < 48) return
          navigate(index + (endX < startX ? 1 : -1))
        }}
        onTouchCancel={() => {
          touchStartX.current = null
        }}
      >
        {images.length > 1 && (
          <>
            <Button
              type="button"
              variant="ghost"
              size="icon-lg"
              className="absolute left-0 z-10 size-11 rounded-full bg-[var(--lightbox-control-bg)] text-[var(--on-ink)] shadow-[0_8px_24px_var(--lightbox-action-shadow)] hover:bg-[var(--lightbox-control-bg)] hover:text-[var(--on-ink)] disabled:opacity-30"
              aria-label={t('chat:chatMessage.previousImage')}
              title={t('chat:chatMessage.previousImage')}
              disabled={!hasPrevious}
              onClick={() => navigate(index - 1)}
            >
              <ChevronLeft size={22} />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-lg"
              className="absolute right-0 z-10 size-11 rounded-full bg-[var(--lightbox-control-bg)] text-[var(--on-ink)] shadow-[0_8px_24px_var(--lightbox-action-shadow)] hover:bg-[var(--lightbox-control-bg)] hover:text-[var(--on-ink)] disabled:opacity-30"
              aria-label={t('chat:chatMessage.nextImage')}
              title={t('chat:chatMessage.nextImage')}
              disabled={!hasNext}
              onClick={() => navigate(index + 1)}
            >
              <ChevronRight size={22} />
            </Button>
          </>
        )}
        <img
          alt={image.attachment.name || t('chat:chatMessage.generatedImage')}
          decoding="async"
          src={image.source}
        />
      </div>
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
  const imagePreviews = attachments.flatMap<PreviewImage>((attachment, attachmentIndex) => {
    const source =
      attachment.url ||
      (attachment.data ? `data:${attachment.mimeType};base64,${attachment.data}` : '')
    return attachment.kind === 'image' && source ? [{ attachment, source, attachmentIndex }] : []
  })
  return (
    <>
      <div
        className={`message-attachments flex flex-wrap gap-[6px] [margin-top:6px] ${compact ? 'compact' : ''}`}
      >
        {attachments.map((attachment, index) => {
          const key = attachment.id || index
          const source =
            attachment.url ||
            (attachment.data ? `data:${attachment.mimeType};base64,${attachment.data}` : '')
          if (attachment.kind === 'image' && source)
            return (
              <button
                type="button"
                className="generated-media [.message-attachments_&]:flex [.message-attachments_&]:w-[min(360px,100%)] [.message-attachments_&]:flex-col [.message-attachments_&]:gap-[5px] [.message-attachments_&]:text-[var(--text-muted)] [.message-attachments_&]:no-underline [.message-attachments_button&]:border-0 [.message-attachments_button&]:bg-transparent [.message-attachments_button&]:p-0 [.message-attachments_button&]:text-left [.message-attachments_button&]:[cursor:zoom-in] [.message-attachments_&_img]:w-full [.message-attachments_&_img]:max-h-[320px] [.message-attachments_&_img]:[border:1px_solid_var(--stroke)] [.message-attachments_&_img]:rounded-[var(--r-sm)] [.message-attachments_&_img]:object-contain [.message-attachments_&_img]:bg-[var(--media-bg)] [.message-attachments_&_video]:w-full [.message-attachments_&_video]:max-h-[320px] [.message-attachments_&_video]:[border:1px_solid_var(--stroke)] [.message-attachments_&_video]:rounded-[var(--r-sm)] [.message-attachments_&_video]:object-contain [.message-attachments_&_video]:bg-[var(--media-bg)] [.message-attachments_&_small]:overflow-hidden [.message-attachments_&_small]:text-[13px] [.message-attachments_&_small]:text-ellipsis [.message-attachments_&_small]:whitespace-nowrap [.message-attachments.compact_&]:w-[min(190px,100%)] [.message-attachments.compact_&_img]:max-h-[130px] [.message-attachments.compact_&_video]:max-h-[130px]"
                onClick={() => {
                  const imageIndex = imagePreviews.findIndex(
                    (item) => item.attachmentIndex === index,
                  )
                  if (imageIndex >= 0) setPreview({ images: imagePreviews, index: imageIndex })
                }}
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
              <div
                className="generated-media [.message-attachments_&]:flex [.message-attachments_&]:w-[min(360px,100%)] [.message-attachments_&]:flex-col [.message-attachments_&]:gap-[5px] [.message-attachments_&]:text-[var(--text-muted)] [.message-attachments_&]:no-underline [.message-attachments_button&]:border-0 [.message-attachments_button&]:bg-transparent [.message-attachments_button&]:p-0 [.message-attachments_button&]:text-left [.message-attachments_button&]:[cursor:zoom-in] [.message-attachments_&_img]:w-full [.message-attachments_&_img]:max-h-[320px] [.message-attachments_&_img]:[border:1px_solid_var(--stroke)] [.message-attachments_&_img]:rounded-[var(--r-sm)] [.message-attachments_&_img]:object-contain [.message-attachments_&_img]:bg-[var(--media-bg)] [.message-attachments_&_video]:w-full [.message-attachments_&_video]:max-h-[320px] [.message-attachments_&_video]:[border:1px_solid_var(--stroke)] [.message-attachments_&_video]:rounded-[var(--r-sm)] [.message-attachments_&_video]:object-contain [.message-attachments_&_video]:bg-[var(--media-bg)] [.message-attachments_&_small]:overflow-hidden [.message-attachments_&_small]:text-[13px] [.message-attachments_&_small]:text-ellipsis [.message-attachments_&_small]:whitespace-nowrap [.message-attachments.compact_&]:w-[min(190px,100%)] [.message-attachments.compact_&_img]:max-h-[130px] [.message-attachments.compact_&_video]:max-h-[130px] video"
                key={key}
              >
                <video controls preload="metadata" src={source} />
                <small>{attachment.name || t('chat:chatMessage.generatedVideo')}</small>
              </div>
            )
          return (
            <a
              className="message-file-attachment [.message-attachments_&]:inline-flex [.message-attachments_&]:items-center [.message-attachments_&]:gap-[5px] [.message-attachments_&]:[border:1px_solid_var(--stroke)] [.message-attachments_&]:rounded-[var(--r-xs)] [.message-attachments_&]:bg-[var(--solid)] [.message-attachments_&]:p-[5px_7px] [.message-attachments_&]:text-[var(--text-tertiary)] [.message-attachments_&]:text-[13px] [.message-attachments_&]:no-underline"
              href={attachment.downloadUrl || undefined}
              key={key}
            >
              <File size={12} />
              {attachment.name || t('chat:chatMessage.fileAttachment')}
            </a>
          )
        })}
      </div>
      {preview &&
        createPortal(
          <ImageLightbox
            images={preview.images}
            index={preview.index}
            onClose={() => setPreview(null)}
            onNavigate={(index) =>
              setPreview((current) => (current ? { ...current, index } : current))
            }
          />,
          document.body,
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

  // 保存条目标签：写入运行时并回显新标签，成功后关闭编辑；防重入。
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

  // 移除条目标签：置空标签并同步回显；防重入。
  const remove = async () => {
    if (saving || loading) return
    setSaving(true)
    setError('')
    try {
      const tree = await chatApi.setSessionTreeLabel(sessionId, entryId, '')
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
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className={savedLabel ? 'bg-surface-hover text-brand' : undefined}
              aria-label={t('chat:chatMessage.labelThisTurn')}
              data-pisper-label-entry={entryId}
            >
              <Tag size={14} />
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="top" sideOffset={6}>
          {t('chat:chatMessage.labelThisTurn')}
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        className="message-label-popover [&_form]:grid [&_form]:gap-[10px] [&_[data-slot='popover-title']]:text-[12px]"
        align="end"
        sideOffset={6}
      >
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
          <div className="message-label-actions flex items-center justify-between gap-[8px]">
            <Button
              type="button"
              size="icon-sm"
              variant="ghost"
              title={t('chat:sessionTree.removeLabel')}
              aria-label={t('chat:sessionTree.removeLabel')}
              disabled={loading || saving}
              onClick={() => void remove()}
            >
              <Trash2 />
            </Button>
            <Button
              className="message-label-save [.message-label-actions_&]:min-w-[104px] [.message-label-actions_&]:text-[var(--primary-foreground)]"
              type="submit"
              disabled={loading || saving}
            >
              {saving ? <LoaderCircle className="animate-spin" /> : <Check />}
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
  /** 会话工作区根目录：Markdown 内相对路径文件链接的解析基址。 */
  cwd?: string
  sessionStreaming?: boolean
  onBranchFromHere: (boundaryEntryId: string) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
  /** 最近一条前置用户消息 ID：存在时最新助手消息上展示重试按钮。 */
  retryUserMessageId?: string
  onRetryLastTurn: () => Promise<void> | void
}

function focusPropsEqual(prev: FocusChatMessageProps, next: FocusChatMessageProps) {
  return (
    prev.sessionId === next.sessionId &&
    prev.message === next.message &&
    prev.agentState === next.agentState &&
    prev.showRunActivity === next.showRunActivity &&
    prev.runProps === next.runProps &&
    prev.cwd === next.cwd &&
    prev.sessionStreaming === next.sessionStreaming &&
    prev.onBranchFromHere === next.onBranchFromHere &&
    prev.onCreateChildSession === next.onCreateChildSession &&
    prev.retryUserMessageId === next.retryUserMessageId &&
    prev.onRetryLastTurn === next.onRetryLastTurn
  )
}

export const FocusChatMessage = memo(function FocusChatMessage({
  sessionId,
  message,
  agentState,
  showRunActivity,
  runProps,
  cwd,
  sessionStreaming,
  onBranchFromHere,
  onCreateChildSession,
  retryUserMessageId,
  onRetryLastTurn,
}: FocusChatMessageProps) {
  const { t } = useI18n()
  const [branching, setBranching] = useState(false)
  const [creatingChild, setCreatingChild] = useState(false)
  const [retrying, setRetrying] = useState(false)
  const streaming = Boolean(message.streaming)
  const fullText = message.text || ''
  const displayText = fullText || (!showRunActivity ? String(message.error || '') : '')
  // 活动区是否有可见内容（思考/工具/团队）；streaming 本身不算——否则首轮事件
  // 空窗期活动区渲染空壳，三点动画永远不会出现。
  const hasVisibleRunActivity = Boolean(
    runProps &&
    (String(runProps.thinkingText || '').trim() ||
      (runProps.activityFeed?.length ?? 0) > 0 ||
      (runProps.tools?.length ?? 0) > 0 ||
      runProps.team),
  )

  return (
    <AiMessage
      from={message.role === 'agent' ? 'assistant' : 'user'}
      className={cn(
        'message mx-auto mb-8 w-full max-w-[1040px] min-w-0 gap-0',
        message.role === 'agent' ? 'items-stretch' : 'items-end',
        message.role,
        message.error && 'has-error',
      )}
      data-pisper-message-id={message.id}
      data-pisper-role={message.role}
      data-pisper-streaming={streaming || undefined}
      data-pisper-error={message.error ? 'true' : undefined}
    >
      {message.role === 'agent' && (
        <span className="mb-3 flex items-center gap-2 text-xs font-medium text-[var(--text-muted)]">
          <span
            className="agent-message-mark grid size-[22px] place-items-center"
            data-state={agentState}
            aria-hidden="true"
          >
            <BrandLogo size={20} />
          </span>
          Pisper
        </span>
      )}
      <div
        className={cn(
          'message-content relative min-w-0',
          message.role === 'agent'
            ? 'w-full'
            : 'w-fit max-w-[78%] @max-[700px]:max-w-[86%] @max-[470px]:max-w-[94%]',
        )}
      >
        {showRunActivity && runProps && <AgentRunActivity {...runProps} />}
        {streaming && !displayText && !hasVisibleRunActivity && (
          // 首轮 SSE 事件到达前的空窗：只有头像会显得卡住，用三点动画表明正在工作。
          <div
            className="agent-thinking-dots [&_i]:w-[4px] [&_i]:h-[4px] [&_i]:rounded-[50%] [&_i]:bg-[var(--text-muted)] [&_i]:[animation:agent-thinking-dot_1.15s_ease-in-out_infinite] [&_i:nth-child(2)]:[animation-delay:.14s] [&_i:nth-child(3)]:[animation-delay:.28s] inline-flex items-center gap-[3px] py-2"
            aria-hidden="true"
          >
            <i />
            <i />
            <i />
          </div>
        )}
        {displayText && (
          <MarkdownMessage
            cwd={cwd}
            streaming={streaming}
            className={cn(
              'min-h-[34px] [overflow-wrap:anywhere] text-[length:var(--app-message-font-size)]',
              message.role === 'agent'
                ? 'w-full py-1 leading-[1.75]'
                : 'rounded-[22px] bg-[var(--user-bubble-bg)] px-4 py-2.5 leading-[1.6] text-[var(--user-bubble-text)]',
            )}
          >
            {displayText}
          </MarkdownMessage>
        )}
        {message.attachments && message.attachments.length > 0 && (
          <MessageAttachments attachments={message.attachments} />
        )}
      </div>
      {message.error && fullText && !streaming && (
        <div className="message-error-notice mt-3 flex items-start gap-1.5 text-[13px] leading-[1.5] text-[var(--danger)]">
          <AlertTriangle size={13} className="mt-[3px] flex-none" />
          <span className="min-w-0">{String(message.error)}</span>
        </div>
      )}
      {message.role === 'agent' &&
        !streaming &&
        (message.turnBoundaryEntryId || retryUserMessageId) && (
          <div className="message-actions mt-4 -ml-1.5 flex items-center gap-1 text-[var(--text-muted)] [&_button]:size-7 [&_button]:min-h-7 [&_button]:rounded-md [&_button]:text-[var(--text-muted)] [&_button:hover]:bg-[var(--surface-hover)] [&_button:hover]:text-[var(--text)]">
            {message.turnBoundaryEntryId && (
              <>
                <MessageTreeLabel sessionId={sessionId} entryId={message.turnBoundaryEntryId} />
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label={t('chat:chatMessage.deriveFromHere')}
                      data-pisper-derive-entry={message.turnBoundaryEntryId}
                      disabled={branching || creatingChild || sessionStreaming}
                      onClick={async () => {
                        const boundaryEntryId = message.turnBoundaryEntryId
                        if (!boundaryEntryId) return
                        setBranching(true)
                        try {
                          await onBranchFromHere(boundaryEntryId)
                        } finally {
                          setBranching(false)
                        }
                      }}
                    >
                      {branching ? (
                        <LoaderCircle className="animate-spin" size={14} />
                      ) : (
                        <GitFork size={14} />
                      )}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top" sideOffset={6}>
                    {t('chat:chatMessage.deriveFromHere')}
                  </TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label={t('chat:chatMessage.createChildChat')}
                      data-pisper-child-entry={message.turnBoundaryEntryId}
                      disabled={branching || creatingChild}
                      onClick={async () => {
                        const boundaryEntryId = message.turnBoundaryEntryId
                        if (!boundaryEntryId) return
                        setCreatingChild(true)
                        try {
                          await onCreateChildSession(boundaryEntryId)
                        } finally {
                          setCreatingChild(false)
                        }
                      }}
                    >
                      {creatingChild ? (
                        <LoaderCircle className="animate-spin" size={14} />
                      ) : (
                        <MessageSquarePlus size={14} />
                      )}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top" sideOffset={6}>
                    {t('chat:chatMessage.createChildChat')}
                  </TooltipContent>
                </Tooltip>
              </>
            )}
            {retryUserMessageId && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={t('chat:chatMessage.retry')}
                    data-pisper-retry-message={retryUserMessageId}
                    disabled={retrying || sessionStreaming}
                    onClick={async () => {
                      setRetrying(true)
                      try {
                        await onRetryLastTurn()
                      } finally {
                        setRetrying(false)
                      }
                    }}
                  >
                    {retrying ? (
                      <LoaderCircle className="animate-spin" size={14} />
                    ) : (
                      <RotateCcw size={14} />
                    )}
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="top" sideOffset={6}>
                  {t('chat:chatMessage.retry')}
                </TooltipContent>
              </Tooltip>
            )}
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
      className={`[&_>_span]:pt-[5px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_>_span]:text-[13px] [&_>_span]:font-[600] [&_>_span]:[text-transform:uppercase] [&.agent_>_span::before]:[content:'✦'] [&.agent_>_span::before]:mr-[4px] [&.agent_>_span::before]:text-[var(--star)] [&_.markdown-body]:rounded-[var(--r-xs)] [&_.markdown-body]:bg-[var(--surface-subtle)] [&_.markdown-body]:[padding:6px_8px] [&_.markdown-body]:text-[13px] [&_.markdown-body]:leading-[1.45] [&_.markdown-body]:[overflow-wrap:anywhere] [&.agent_.markdown-body]:bg-[var(--accent-soft)] [&_.markdown-body_pre]:max-h-[130px] [&_.markdown-body_pre]:[padding:7px] grid grid-cols-[34px_minmax(0,1fr)] gap-[6px] [margin-bottom:6px] [align-items:start] ${message.role}`}
      data-pisper-message-id={message.id}
      data-pisper-role={message.role}
      data-pisper-streaming={message.streaming || undefined}
    >
      <span>{message.role === 'agent' ? 'Pisper' : 'You'}</span>
      <div className="min-w-0">
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
