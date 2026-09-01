import { File, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { formatFileSize } from '@/lib/format'
import type { ChatAttachment } from '@/types/chat'

export function AttachmentTray({
  attachments,
  onRemove,
  compact = false,
}: {
  attachments: ChatAttachment[]
  onRemove: (id: string) => void
  compact?: boolean
}) {
  const { t } = useI18n()
  if (!attachments.length) return null
  return (
    <div
      className={`attachment-tray [&.compact]:max-h-[58px] flex max-h-[116px] flex-wrap gap-[6px] overflow-auto ${compact ? 'compact' : ''}`}
    >
      {attachments.map((attachment) => (
        <div
          className="attachment-chip [&_>_img]:w-[30px] [&_>_img]:h-[30px] [&_>_img]:rounded-[var(--r-xs)] [&_>_img]:object-cover [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_>_button]:grid [&_>_button]:w-[32px] [&_>_button]:h-[32px] [&_>_button]:place-items-center [&_>_button]:border-0 [&_>_button]:rounded-[var(--r-xs)] [&_>_button]:bg-transparent [&_>_button]:text-[var(--text-muted)] [&_>_button:hover]:bg-[var(--danger-soft)] [&_>_button:hover]:text-[var(--danger)] dark:bg-[var(--surface-subtle)] grid min-w-[150px] max-w-[250px] grid-cols-[30px_minmax(0,1fr)_auto] items-center gap-[7px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:5px]"
          key={attachment.id}
        >
          {attachment.kind === 'image' ? (
            <img src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
          ) : (
            <span className="grid w-[30px] h-[30px] place-items-center rounded-[var(--r-xs)] bg-[var(--violet-soft)] text-[var(--violet-strong)]">
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
