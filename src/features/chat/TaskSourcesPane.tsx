import { useResourceReading } from './use-resource-reading'
import { lazy, Suspense, useContext, useRef, useState } from 'react'
import { ChevronRight, File, Image, Link2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
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
    <section
      className="flex min-h-0 flex-1 flex-col gap-3 p-3"
      aria-label={t('chat:workbench.sources')}
    >
      <div className="flex items-center gap-2">
        <h2 className="flex-1 text-sm font-medium">{t('chat:workbench.sources')}</h2>
        <span className="text-xs text-content-muted">{t('chat:workbench.addSource')}</span>
        <div ref={slots?.setSourceActionHost} className="grid size-9 place-items-center" />
      </div>
      <p className="text-xs text-content-muted">{t('chat:workbench.sourcesScope')}</p>
      <Input
        aria-label={t('chat:workbench.searchSources')}
        placeholder={t('chat:workbench.searchSources')}
        value={view.query}
        onChange={(event) => onViewChange({ query: event.target.value })}
      />
      <select
        aria-label={t('chat:workbench.sourceType')}
        className="h-9 rounded-md border border-border bg-background px-2 text-sm"
        value={view.filter}
        onChange={(event) => onViewChange({ filter: event.target.value })}
      >
        <option value="">{t('chat:workbench.allTypes')}</option>
        <option value="image">{t('chat:workbench.images')}</option>
        <option value="text">{t('chat:workbench.text')}</option>
        <option value="file">{t('chat:workbench.files')}</option>
      </select>
      <div ref={reading.ref} className="min-h-0 flex-1 overflow-y-auto">
        {!filtered.length && (
          <p className="p-3 text-sm text-content-muted">
            {sources.length ? t('chat:workbench.noMatches') : t('chat:workbench.noSources')}
          </p>
        )}
        {filtered.map((source) => {
          const expanded = view.selectedSourceId === source.id
          const Icon = source.kind === 'image' ? Image : source.method === 'path' ? Link2 : File
          return (
            <article key={source.id} className="border-b border-border/60">
              <button
                className="flex min-h-12 w-full min-w-0 items-center gap-2 px-2 text-left"
                aria-expanded={expanded}
                onClick={() => onViewChange({ selectedSourceId: expanded ? '' : source.id })}
              >
                <ChevronRight size={14} className={expanded ? 'rotate-90 shrink-0' : 'shrink-0'} />
                <Icon className="size-4 shrink-0 text-content-muted" />
                <span className="min-w-0 flex-1 truncate text-sm">
                  {source.name || t('chat:chatMessage.fileAttachment')}
                </span>
              </button>
              {expanded && (
                <div className="space-y-2 bg-surface-subtle p-3 text-xs text-content-muted">
                  <p className="break-all">
                    {source.attachment.path ||
                      source.attachment.id ||
                      t('chat:workbench.unavailable')}
                  </p>
                  <p>{source.attachment.mimeType || 'application/octet-stream'}</p>
                  <p>
                    {source.method === 'asset'
                      ? t('chat:workbench.registeredAsset')
                      : source.method === 'path'
                        ? t('chat:workbench.pathReference')
                        : t('chat:workbench.invalidReference')}
                  </p>
                  <Button
                    variant="outline"
                    size="sm"
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
