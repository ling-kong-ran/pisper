import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { AlertTriangle, Download, ExternalLink, File, Paperclip } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatFileSize } from '@/lib/format'
import { canOpenAssetInApplication } from '@/lib/open-asset'
import { assetMediaUrl, type Asset } from './assets-api'
import { assetPreviewQueryOptions } from './assets-queries'

export function AssetPreviewDialog({
  asset,
  generation,
  trigger,
  fallback,
  onClose,
  onUse,
  onOpen,
  busy,
  actionError,
}: {
  asset: Asset
  generation: number
  trigger: HTMLElement | null
  fallback: () => HTMLElement | null
  onClose: () => void
  onUse: () => void
  onOpen: () => void
  busy?: string
  actionError?: string
}) {
  const { t } = useI18n()
  const video = asset.mimeType?.startsWith('video/')
  const media = asset.kind === 'image' || video
  const content = useQuery({
    ...assetPreviewQueryOptions(asset.id, generation),
    enabled: !media && asset.kind !== 'link',
  })
  const [mediaFailed, setMediaFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const markdown = /\.md$/i.test(asset.name) || asset.mimeType === 'text/markdown'
  const retryMedia = () => {
    setMediaFailed(false)
    setAttempt((value) => value + 1)
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent
        size="wide"
        scrollMode="body"
        className="w-[min(96vw,1200px)] max-w-[min(96vw,1200px)]"
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          const visible = (element: HTMLElement | null) =>
            element?.isConnected &&
            element.getClientRects().length &&
            getComputedStyle(element).visibility !== 'hidden'
          const target = visible(trigger) ? trigger : fallback()
          if (visible(target)) target?.focus({ preventScroll: true })
        }}
      >
        <DialogHeader className="pr-8">
          <DialogTitle className="break-words leading-6">{asset.name}</DialogTitle>
          <DialogDescription className="font-mono break-words tabular-nums">
            {asset.kind === 'link'
              ? asset.url
              : `${asset.mimeType || ''} · ${formatFileSize(asset.size)}`}
          </DialogDescription>
        </DialogHeader>
        <DialogBody className="min-h-[min(240px,30dvh)] rounded-md border border-stroke bg-surface-subtle p-3">
          {media ? (
            mediaFailed ? (
              <div role="alert" className="flex flex-col items-center gap-3 p-4 text-sm">
                <AlertTriangle className="size-5 text-danger" />
                <p>{t('assets:assetsPage.mediaFailed')}</p>
                <Button variant="secondary" onClick={retryMedia}>
                  {t('assets:assetsPage.retry')}
                </Button>
              </div>
            ) : video ? (
              <video
                key={attempt}
                controls
                playsInline
                preload="metadata"
                className="mx-auto max-h-[65dvh] w-full object-contain"
                src={assetMediaUrl(asset.id)}
                onError={() => setMediaFailed(true)}
              />
            ) : (
              <img
                key={attempt}
                className="mx-auto max-h-[65dvh] max-w-full object-contain"
                src={assetMediaUrl(asset.id)}
                alt={asset.name}
                onError={() => setMediaFailed(true)}
              />
            )
          ) : asset.kind === 'link' ? (
            <a
              className="flex items-center gap-2 font-mono text-sm break-all text-content underline underline-offset-4"
              href={asset.url}
              target="_blank"
              rel="noreferrer"
            >
              <ExternalLink className="shrink-0" />
              {asset.url}
            </a>
          ) : content.isPending ? (
            <div
              role="status"
              className="flex items-center justify-center gap-2 p-8 text-sm text-content-muted"
            >
              <OrbitSpinner />
              {t('assets:assetsPage.loadingPreview')}
            </div>
          ) : content.isError ? (
            <div role="alert" className="flex flex-wrap items-center gap-2 p-4 text-sm text-danger">
              <AlertTriangle className="size-4 shrink-0" />
              <span className="min-w-0 break-words">{content.error.message}</span>
              <Button
                variant="secondary"
                onClick={() => {
                  void content.refetch()
                }}
              >
                {t('assets:assetsPage.retry')}
              </Button>
            </div>
          ) : content.data?.kind === 'text' ? (
            markdown ? (
              <div className="mx-auto max-w-3xl text-left">
                <MarkdownMessage>{content.data.text || ''}</MarkdownMessage>
              </div>
            ) : (
              <div
                className="mx-auto max-w-3xl whitespace-pre-wrap font-sans text-base leading-7 [overflow-wrap:anywhere]"
                data-asset-plain
              >
                {content.data.text}
              </div>
            )
          ) : (
            <div className="flex flex-col items-center gap-2 p-6 text-center text-sm">
              <File className="size-8 text-content-faint" strokeWidth={1.5} />
              <strong className="font-medium break-words">{asset.name}</strong>
              <p className="max-w-md text-xs text-content-muted">
                {t(
                  'assets:assetsPage.thisFileTypeCanBeDownloadedSupportedDocumentsCanAlsoBeAddedToAChatForAnalysis',
                )}
              </p>
            </div>
          )}
          {content.data?.truncated && (
            <p className="mt-3 text-xs text-content-muted">
              {t('assets:assetsPage.previewTruncated')}
            </p>
          )}
        </DialogBody>
        {actionError && (
          <p role="alert" className="break-words text-sm text-danger">
            {actionError}
          </p>
        )}
        <DialogFooter>
          {asset.kind !== 'link' && (
            <Button
              variant="secondary"
              size="dialogAction"
              disabled={Boolean(busy)}
              loading={busy === 'open'}
              onClick={onOpen}
            >
              {canOpenAssetInApplication() ? <ExternalLink /> : <Download />}
              {canOpenAssetInApplication()
                ? t('assets:assetsPage.openInApplication')
                : t('assets:assetsPage.download')}
            </Button>
          )}
          <Button
            size="dialogAction"
            disabled={Boolean(busy)}
            loading={busy === 'use'}
            onClick={onUse}
          >
            <Paperclip />
            {t('assets:assetsPage.useInChat')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
