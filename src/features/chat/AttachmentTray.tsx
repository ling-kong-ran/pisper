import { File, X } from 'lucide-react'
import { OrbitSpinner } from '@/components/ui/spinner'
import { useI18n } from '@/app/use-i18n'
import { formatFileSize } from '@/lib/format'
import type { ChatAttachment } from '@/types/chat'

export function AttachmentTray({
  attachments,
  preparing = [],
  onRemove,
  compact = false,
}: {
  attachments: ChatAttachment[]
  preparing?: { id: string; name: string }[]
  onRemove: (id: string) => void
  compact?: boolean
}) {
  const { t } = useI18n()
  if (!attachments.length && !preparing.length) return null
  return (
    <div
      className={`attachment-tray [&.compact]:max-h-[58px] flex max-h-[116px] flex-wrap gap-[6px] overflow-auto ${compact ? 'compact' : ''}`}
    >
      {preparing.map((item) => (
        <div
          key={item.id}
          className="flex min-h-[42px] max-w-64 items-center gap-2 rounded-sm border border-stroke bg-surface-subtle pr-1 pl-2.5"
          aria-busy="true"
        >
          <OrbitSpinner className="size-3.5" />
          <span className="min-w-0 flex-1 truncate font-mono text-xs text-content">
            {item.name}
          </span>
          <span className="sr-only">{t('chat:focusSession.preparingAttachments')}</span>
          <button
            type="button"
            className="grid size-8 shrink-0 cursor-pointer place-items-center rounded-sm text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
            aria-label={t('chat:focusSession.removeName', { name: item.name })}
            onClick={() => onRemove(item.id)}
          >
            <X size={14} />
          </button>
        </div>
      ))}
      {attachments.map((attachment) => (
        <div
          className="attachment-chip grid min-h-[42px] min-w-[150px] max-w-[250px] grid-cols-[30px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm border border-stroke bg-surface-subtle p-1.5 pr-1 [&_>_button]:grid [&_>_button]:size-7 [&_>_button]:cursor-pointer [&_>_button]:place-items-center [&_>_button]:rounded-xs [&_>_button]:text-content-muted [&_>_button:hover]:bg-danger-soft [&_>_button:hover]:text-danger max-[650px]:[&_>_button]:size-11 [&_>_img]:size-[30px] [&_>_img]:rounded-xs [&_>_img]:object-cover [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_small]:text-xs [&_small]:text-content-muted [&_small]:tabular-nums [&_strong]:truncate [&_strong]:font-mono [&_strong]:text-xs [&_strong]:font-medium [&_strong]:text-content"
          key={attachment.id}
        >
          {attachment.kind === 'image' ? (
            <img src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
          ) : (
            <span className="grid size-[30px] place-items-center rounded-xs border border-key-border bg-key text-content-muted">
              <File size={13} />
            </span>
          )}
          <span>
            <strong>{attachment.name}</strong>
            <small>
              {attachment.kind === 'path'
                ? t('chat:focusSession.localPath')
                : attachment.kind === 'image'
                  ? t('chat:focusSession.image')
                  : attachment.kind === 'document'
                    ? t('chat:focusSession.document')
                    : t('chat:focusSession.text')}
              {attachment.kind !== 'path' ? ` · ${formatFileSize(attachment.size)}` : ''}
              {attachment.truncated ? ` · ${t('chat:focusSession.truncated')}` : ''}
            </small>
          </span>
          <button
            type="button"
            aria-label={t('chat:focusSession.removeName', { name: attachment.name })}
            onClick={() => onRemove(String(attachment.id || ''))}
          >
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  )
}
