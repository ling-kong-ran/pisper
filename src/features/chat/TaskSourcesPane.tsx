import { useResourceReading } from './use-resource-reading'
import { lazy, Suspense, useContext, useRef, useState } from 'react'
import { ChevronRight, File, Image, Link2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { InlineEmptyState } from '@/components/app/PageState'
import type { ContextPageView } from './session-context-pages'
import type { TaskSource } from './task-sources'
import { TaskWorkspaceSlots } from './task-workspace-context'
const Preview = lazy(() =>
  import('./FileAttachmentPreview').then((module) => ({ default: module.FileAttachmentPreview })),
)

export function TaskSourcesPane({
  sources,
  view,
  onViewChange,
}: {
  sources: TaskSource[]
  view: ContextPageView
  onViewChange: (patch: Partial<ContextPageView>) => void
}) {
  const { t } = useI18n()
  const reading = useResourceReading<HTMLDivElement>(view.reading?.list, (position) =>
    onViewChange({ reading: { ...view.reading, list: position } }),
  )
  const slots = useContext(TaskWorkspaceSlots)
  const [previewId, setPreviewId] = useState('')
  const opener = useRef<HTMLButtonElement | null>(null)
  const preview = sources.find((source) => source.id === previewId)
  const filtered = sources.filter(
    (source) =>
      (!view.filter || source.kind === view.filter) &&
      source.name.toLocaleLowerCase().includes(view.query.toLocaleLowerCase()),
  )
  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label={t('chat:workbench.sources')}>
      {/* 细线分区页头：12/500 弱化标签 + 等宽计数；附件入口由会话输入框经插槽挂入。 */}
      <header className="flex min-h-10 min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft pr-1.5 pl-3">
        <h2 className="min-w-0 flex-1 truncate text-xs font-medium tracking-[.02em] text-content-muted">
          {t('chat:workbench.sources')}
          <span className="ml-1.5 tabular-nums">{sources.length}</span>
        </h2>
        <span className="shrink-0 text-xs text-content-muted">{t('chat:workbench.addSource')}</span>
        <div ref={slots?.setSourceActionHost} className="grid min-h-7 min-w-7 place-items-center" />
      </header>
      <div className="flex shrink-0 flex-col gap-2 border-b border-stroke-soft px-3 py-2.5">
        <p className="text-xs leading-relaxed text-content-muted">
          {t('chat:workbench.sourcesScope')}
        </p>
        <div className="flex min-w-0 gap-2">
          <Input
            className="min-w-0 flex-1"
            aria-label={t('chat:workbench.searchSources')}
            placeholder={t('chat:workbench.searchSources')}
            value={view.query}
            onChange={(event) => onViewChange({ query: event.target.value })}
          />
          <select
            aria-label={t('chat:workbench.sourceType')}
            className="h-8.5 max-w-[45%] shrink-0 rounded-sm border border-control bg-panel px-2 text-sm text-content outline-none focus-visible:border-focus focus-visible:ring-3 focus-visible:ring-focus-ring pointer-coarse:h-11"
            value={view.filter}
            onChange={(event) => onViewChange({ filter: event.target.value })}
          >
            <option value="">{t('chat:workbench.allTypes')}</option>
            <option value="image">{t('chat:workbench.images')}</option>
            <option value="text">{t('chat:workbench.text')}</option>
            <option value="file">{t('chat:workbench.files')}</option>
          </select>
        </div>
      </div>
      <div ref={reading.ref} className="min-h-0 flex-1 overflow-y-auto">
        {!filtered.length && (
          <InlineEmptyState
            compact
            title={sources.length ? t('chat:workbench.noMatches') : t('chat:workbench.noSources')}
          />
        )}
        {filtered.map((source) => {
          const expanded = view.selectedSourceId === source.id
          const Icon = source.kind === 'image' ? Image : source.method === 'path' ? Link2 : File
          return (
            <article key={source.id} className="border-b border-stroke-soft">
              <button
                className={`flex min-h-9 w-full min-w-0 items-center gap-2 px-2 text-left hover:bg-surface-subtle focus-visible:-outline-offset-2 max-[650px]:min-h-11 pointer-coarse:min-h-11 ${expanded ? 'bg-surface-subtle' : ''}`}
                aria-expanded={expanded}
                onClick={() => onViewChange({ selectedSourceId: expanded ? '' : source.id })}
              >
                <ChevronRight
                  size={14}
                  className={`shrink-0 text-content-muted transition-transform duration-(--d-fast) ${expanded ? 'rotate-90' : ''}`}
                />
                <Icon className="size-3.5 shrink-0 text-content-muted" />
                <span className="min-w-0 flex-1 truncate font-mono text-xs">
                  {source.name || t('chat:chatMessage.fileAttachment')}
                </span>
              </button>
              {expanded && (
                <div className="space-y-1.5 border-t border-stroke-soft bg-code py-2.5 pr-3 pl-8 text-xs text-content-muted">
                  <p className="break-all font-mono text-content-secondary">
                    {source.attachment.path ||
                      source.attachment.id ||
                      t('chat:workbench.unavailable')}
                  </p>
                  <p className="font-mono">
                    {source.attachment.mimeType || 'application/octet-stream'}
                  </p>
                  <p>
                    {source.method === 'asset'
                      ? t('chat:workbench.registeredAsset')
                      : source.method === 'path'
                        ? t('chat:workbench.pathReference')
                        : t('chat:workbench.invalidReference')}
                  </p>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="mt-1"
                    disabled={!source.available}
                    onClick={(event) => {
                      opener.current = event.currentTarget
                      setPreviewId(source.id)
                    }}
                  >
                    {t('chat:chatMessage.fileActionPreview')}
                  </Button>
                </div>
              )}
            </article>
          )
        })}
      </div>
      {preview && (
        <Suspense fallback={null}>
          <Preview
            attachment={preview.attachment}
            onClose={() => setPreviewId('')}
            onRestoreFocus={() => opener.current?.focus()}
            onDownload={() => {
              if (typeof preview.attachment.downloadUrl === 'string')
                window.open(preview.attachment.downloadUrl, '_blank', 'noopener,noreferrer')
            }}
          />
        </Suspense>
      )}
    </section>
  )
}
