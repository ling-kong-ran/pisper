import { useEffect, useState } from 'react'
import { Download, File, LoaderCircle, RefreshCw, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { formatFileSize } from '@/lib/format'
import type { ChatAttachment } from '@/types/chat'
import { loadAttachmentPreview, type AttachmentPreview } from './attachment-preview'

export function FileAttachmentPreview({
  attachment,
  onClose,
  onDownload,
  onRestoreFocus,
}: {
  attachment: ChatAttachment
  onClose: () => void
  onDownload: () => void
  onRestoreFocus: () => void
}) {
  const { t } = useI18n()
  const [content, setContent] = useState<AttachmentPreview | null>(null)
  const [error, setError] = useState('')
  const [attempt, setAttempt] = useState(0)
  const [loading, setLoading] = useState(true)
  const name = attachment.name || t('chat:chatMessage.fileAttachment')
  const markdown =
    /\.(md|markdown)$/i.test(name) || (content?.mimeType || attachment.mimeType) === 'text/markdown'

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setContent(null)
    setError('')
    void loadAttachmentPreview(attachment, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) setContent(value)
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted)
          setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [attachment, attempt])

  const text = content?.kind === 'text' ? content.text || '' : undefined
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="flex h-[min(680px,calc(100dvh-2rem))] w-[calc(100%-2rem)] min-w-0 max-w-[900px] flex-col overflow-hidden rounded-lg sm:max-w-[900px]"
        showCloseButton={false}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          onRestoreFocus()
        }}
        aria-describedby={undefined}
      >
        <DialogHeader className="min-w-0 shrink-0 pr-9 text-left">
          <DialogTitle className="truncate text-sm leading-5" title={name}>
            {name}
          </DialogTitle>
          <DialogDescription className="truncate text-xs">
            {attachment.mimeType || 'application/octet-stream'}
            {typeof attachment.size === 'number' && ` · ${formatFileSize(attachment.size)}`}
          </DialogDescription>
        </DialogHeader>
        <DialogClose asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="absolute right-3 top-3"
            aria-label={t('chat:chatMessage.closePreview')}
            title={t('chat:chatMessage.closePreview')}
          >
            <X size={16} />
          </Button>
        </DialogClose>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
          {loading ? (
            <div
              role="status"
              className="flex flex-1 items-center justify-center gap-2 text-muted-foreground"
            >
              <LoaderCircle size={16} className="animate-spin" />
              {t('chat:chatMessage.filePreviewLoading')}
            </div>
          ) : error ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-3">
              <p role="alert" className="max-w-full break-words text-sm text-destructive">
                {error}
              </p>
              <Button
                type="button"
                variant="outline"
                onClick={() => setAttempt((value) => value + 1)}
              >
                <RefreshCw size={14} />
                {t('chat:chatMessage.filePreviewRetry')}
              </Button>
            </div>
          ) : text !== undefined ? (
            markdown ? (
              <Tabs defaultValue="preview" className="min-h-0 min-w-0 flex-1">
                <TabsList className="shrink-0">
                  <TabsTrigger value="preview">
                    {t('chat:chatMessage.fileActionPreview')}
                  </TabsTrigger>
                  <TabsTrigger value="source">
                    {t('chat:chatMessage.filePreviewSource')}
                  </TabsTrigger>
                </TabsList>
                <TabsContent value="preview" className="min-h-0 overflow-auto p-1">
                  <MarkdownMessage>{text}</MarkdownMessage>
                </TabsContent>
                <TabsContent value="source" className="min-h-0 overflow-auto">
                  <pre className="m-0 p-3 font-mono text-xs leading-relaxed">{text}</pre>
                </TabsContent>
              </Tabs>
            ) : (
              <pre className="m-0 min-h-0 flex-1 overflow-auto rounded-md bg-muted/40 p-3 font-mono text-xs leading-relaxed">
                {text}
              </pre>
            )
          ) : (
            <div
              role="status"
              className="flex flex-1 flex-col items-center justify-center gap-3 text-center text-muted-foreground"
            >
              <File size={32} />
              <p className="text-sm">{t('chat:chatMessage.filePreviewUnavailable')}</p>
            </div>
          )}
        </div>
        <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t pt-3">
          {content?.truncated && (
            <p role="status" className="mr-auto text-xs text-muted-foreground">
              {t('chat:chatMessage.filePreviewTruncated')}
            </p>
          )}
          <Button
            type="button"
            variant="outline"
            disabled={!attachment.downloadUrl}
            onClick={onDownload}
          >
            <Download size={14} />
            {t('chat:chatMessage.fileActionDownload')}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
