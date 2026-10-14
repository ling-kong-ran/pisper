import { useEffect, useRef, useState } from 'react'
import { LoaderCircle } from 'lucide-react'
import { parseWorkflowMedia } from '@shared/workflow-inputs.mjs'
import { useI18n } from '@/app/use-i18n'
import { ContentInput } from '@/components/app/ContentInput'
import { workflowMediaApi } from './workflow-media-api'
import type { WorkflowInput, WorkflowInputType } from './types'
import type { WorkflowInputValue } from './workflow-inputs'

export function WorkflowContentField({
  input,
  value,
  onChange,
  label,
  disabled = false,
  onBusyChange,
}: {
  input: Pick<WorkflowInput, 'id' | 'name' | 'type'>
  value: unknown
  onChange: (value: WorkflowInputValue) => void
  label: string
  disabled?: boolean
  onBusyChange?: (id: string, busy: boolean) => void
}) {
  const { t } = useI18n()
  const [preview, setPreview] = useState({ id: '', url: '' })
  const [uploading, setUploading] = useState(false)
  const [error, setError] = useState('')
  const uploadController = useRef<AbortController | null>(null)
  const mounted = useRef(true)
  const busyCallback = useRef(onBusyChange)
  busyCallback.current = onBusyChange
  const latest = useRef({ onChange, id: input.id, type: input.type })
  latest.current = { onChange, id: input.id, type: input.type }
  const type = input.type
  const media = (() => {
    try {
      return parseWorkflowMedia(value)
    } catch {
      return null
    }
  })()
  const mediaId = media?.id
  useEffect(() => {
    mounted.current = true
    setUploading(false)
    setError('')
    return () => {
      mounted.current = false
      uploadController.current?.abort()
      uploadController.current = null
      busyCallback.current?.(input.id, false)
    }
  }, [input.id, type])
  useEffect(() => {
    setPreview({ id: '', url: '' })
    if (!mediaId) return
    const controller = new AbortController()
    let url = ''
    void workflowMediaApi
      .preview(mediaId, controller.signal)
      .then((blob) => {
        if (controller.signal.aborted) return
        url = URL.createObjectURL(blob)
        setPreview({ id: mediaId, url })
      })
      .catch(() => {
        if (!controller.signal.aborted) setError(t('workflows:inputs.previewFailed'))
      })
    return () => {
      controller.abort()
      if (url) URL.revokeObjectURL(url)
    }
  }, [mediaId, t])

  const upload = async (files: File[], inputType: WorkflowInputType) => {
    if (
      !files.length ||
      disabled ||
      uploadController.current ||
      (inputType !== 'image' && inputType !== 'video')
    )
      return
    if (files.length !== 1) {
      setError(t('workflows:inputs.oneFile'))
      return
    }
    const file = files[0]
    const allowed =
      inputType === 'image'
        ? ['image/png', 'image/jpeg', 'image/webp']
        : ['video/mp4', 'video/webm']
    if (!allowed.includes(file.type)) {
      setError(t('workflows:inputs.mediaType'))
      return
    }
    if (file.size > (inputType === 'image' ? 8 : 64) * 1024 * 1024) {
      setError(
        inputType === 'image' ? t('workflows:inputs.imageLimit') : t('workflows:inputs.videoLimit'),
      )
      return
    }
    const controller = new AbortController()
    uploadController.current = controller
    setUploading(true)
    busyCallback.current?.(input.id, true)
    setError('')
    try {
      const reference = await workflowMediaApi.upload(file, controller.signal)
      if (
        mounted.current &&
        !controller.signal.aborted &&
        uploadController.current === controller &&
        latest.current.id === input.id &&
        latest.current.type === inputType
      )
        latest.current.onChange(reference)
    } catch {
      if (mounted.current && !controller.signal.aborted)
        setError(t('workflows:inputs.uploadFailed'))
    } finally {
      if (uploadController.current === controller) {
        uploadController.current = null
        if (mounted.current) {
          setUploading(false)
          busyCallback.current?.(input.id, false)
        }
      }
    }
  }

  return (
    <ContentInput
      label={label}
      value={typeof value === 'string' ? value : ''}
      onValueChange={onChange}
      files={media ? [{ ...media, url: preview.id === media.id ? preview.url : undefined }] : []}
      onFilesSelected={(files) => void upload(files, type)}
      onRemoveFile={() => onChange('')}
      accept={type === 'image' ? 'image' : 'video'}
      allowFiles={type === 'image' || type === 'video'}
      allowText={type !== 'image' && type !== 'video'}
      disabled={disabled || uploading}
      maxLength={16000}
      attachmentLabel={
        type === 'image' ? t('workflows:inputs.chooseImage') : t('workflows:inputs.chooseVideo')
      }
      removeLabel={t('workflows:inputs.removeMedia')}
    >
      {uploading && (
        <p role="status" className="flex items-center gap-1 text-xs text-muted-foreground">
          <LoaderCircle className="size-3 animate-spin" />
          {t('workflows:inputs.uploading')}
        </p>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
    </ContentInput>
  )
}
