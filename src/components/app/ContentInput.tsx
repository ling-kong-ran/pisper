import { useId, useRef } from 'react'
import type { ReactNode } from 'react'
import { ImagePlus, Paperclip, Video, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'

export type ContentInputFile = {
  id: string
  name: string
  mimeType: string
  url?: string
  size?: number
}

// 只负责输入和预览；上传、校验、持久化及 URL 的生命周期由调用领域持有。
export function ContentInput({
  value = '',
  onValueChange,
  files = [],
  onFilesSelected,
  onRemoveFile,
  accept = 'media',
  allowFiles = true,
  allowText = true,
  multiple = false,
  disabled = false,
  label,
  placeholder,
  attachmentLabel,
  removeLabel,
  children,
  className,
  maxLength,
}: {
  value?: string
  onValueChange?: (text: string) => void
  files?: ContentInputFile[]
  onFilesSelected?: (files: File[]) => void
  onRemoveFile?: (id: string) => void
  accept?: 'image' | 'video' | 'media'
  allowFiles?: boolean
  allowText?: boolean
  multiple?: boolean
  disabled?: boolean
  label: string
  placeholder?: string
  attachmentLabel: string
  removeLabel: string
  children?: ReactNode
  className?: string
  maxLength?: number
}) {
  const id = useId()
  const picker = useRef<HTMLInputElement>(null)
  const acceptTypes =
    accept === 'image'
      ? 'image/png,image/jpeg,image/webp'
      : accept === 'video'
        ? 'video/mp4,video/webm'
        : 'image/png,image/jpeg,image/webp,video/mp4,video/webm'
  return (
    <div className={cn('flex min-w-0 flex-col gap-2', className)}>
      <Label htmlFor={allowText ? `${id}-text` : `${id}-picker`}>{label}</Label>
      <div
        className="min-w-0 rounded-xl border bg-background focus-within:ring-2 focus-within:ring-ring/25"
        onDragOver={(event) => {
          if (allowFiles) event.preventDefault()
        }}
        onDrop={(event) => {
          if (!allowFiles) return
          event.preventDefault()
          if (disabled) return
          if (event.dataTransfer.files.length)
            onFilesSelected?.(Array.from(event.dataTransfer.files))
        }}
      >
        {allowText && (
          <Textarea
            id={`${id}-text`}
            value={value}
            onChange={(event) => onValueChange?.(event.target.value)}
            placeholder={placeholder}
            disabled={disabled}
            maxLength={maxLength}
            className="min-h-28 resize-y border-0 bg-transparent shadow-none focus-visible:ring-0"
          />
        )}
        {files.length > 0 && (
          <ul className={cn('grid min-w-0 gap-2 p-3', files.length > 1 && 'sm:grid-cols-2')}>
            {files.map((file) => (
              <li key={file.id} className="relative min-w-0 rounded-lg border bg-muted/20 p-2">
                {file.url && file.mimeType.startsWith('image/') ? (
                  <img
                    src={file.url}
                    alt={file.name}
                    className="h-32 w-full rounded object-contain"
                  />
                ) : file.url && file.mimeType.startsWith('video/') ? (
                  <video
                    src={file.url}
                    controls
                    preload="metadata"
                    className="h-32 w-full rounded object-contain"
                    aria-label={file.name}
                  />
                ) : (
                  <div className="flex h-12 items-center justify-center text-muted-foreground">
                    {file.mimeType.startsWith('video/') ? <Video /> : <ImagePlus />}
                  </div>
                )}
                <p className="mt-2 truncate pr-8 text-xs" title={file.name}>
                  {file.name}
                </p>
                {onRemoveFile && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    disabled={disabled}
                    className="absolute right-1 bottom-1"
                    aria-label={`${removeLabel} ${file.name}`}
                    onClick={() => onRemoveFile(file.id)}
                  >
                    <X className="size-3.5" />
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
        {allowFiles && (
          <div className="p-2">
            <input
              id={`${id}-picker`}
              ref={picker}
              className="sr-only"
              type="file"
              accept={acceptTypes}
              multiple={multiple}
              disabled={disabled}
              aria-label={attachmentLabel}
              onChange={(event) => {
                const selected = Array.from(event.currentTarget.files ?? [])
                event.currentTarget.value = ''
                if (selected.length) onFilesSelected?.(selected)
              }}
            />
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={disabled}
              onClick={() => picker.current?.click()}
            >
              <Paperclip className="size-4" />
              {attachmentLabel}
            </Button>
          </div>
        )}
      </div>
      {children}
    </div>
  )
}
