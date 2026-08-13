import { useCallback, useRef, useState, type ClipboardEvent } from 'react'
import { storedLanguage, translateText, type I18nValues } from '@/app/i18n.ts'
import type { ChatAttachment } from '@/types/chat'

const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024
const MAX_TOTAL_ATTACHMENT_BYTES = 20 * 1024 * 1024

export function clipboardImageFiles(clipboardData: DataTransfer | null | undefined): File[] {
  const files = [...(clipboardData?.files || [])].filter((file) => file.type?.startsWith('image/'))
  if (files.length) return files
  return [...(clipboardData?.items || [])]
    .filter((item) => item.kind === 'file' && item.type?.startsWith('image/'))
    .map((item) => item.getAsFile?.())
    .filter((file): file is File => Boolean(file))
}

type Translate = (message: string, values?: I18nValues) => string

function fileToBase64(file: File, t: Translate) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).split(',')[1] || '')
    reader.onerror = () =>
      reject(reader.error || new Error(t('chat:attachments.couldNotReadImage')))
    reader.readAsDataURL(file)
  })
}

async function prepareClipboardImages(
  fileList: Iterable<File>,
  t: Translate,
): Promise<ChatAttachment[]> {
  const files = [...fileList].slice(0, 8)
  const attachments: ChatAttachment[] = []
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES)
      throw new Error(t('chat:attachments.nameExceedsThe10MBLimit', { name: file.name }))
    if (!file.type.startsWith('image/')) continue
    attachments.push({
      id: `${file.name}-${file.lastModified}-${file.size}`,
      kind: 'image',
      name: file.name,
      mimeType: file.type,
      size: file.size,
      data: await fileToBase64(file, t),
    })
  }
  return attachments
}

function pathName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).at(-1) || path
}

export function pathAttachments(paths: Iterable<string>): ChatAttachment[] {
  return [...paths]
    .map((path) => String(path || '').trim())
    .filter(Boolean)
    .slice(0, 8)
    .map((path) => ({
      id: `path:${path}`,
      kind: 'path',
      name: pathName(path),
      path,
    }))
}

export function useAttachmentSelection() {
  const t = useCallback(
    (message: string, values?: I18nValues) => translateText(message, storedLanguage(), values),
    [],
  )
  const [attachments, setAttachments] = useState<ChatAttachment[]>([])
  const [attachmentError, setAttachmentError] = useState('')
  const attachmentsRef = useRef<ChatAttachment[]>([])

  const replaceAttachments = useCallback((items: ChatAttachment[]) => {
    attachmentsRef.current = items
    setAttachments(items)
  }, [])

  const addClipboardImages = useCallback(
    async (fileList: Iterable<File> | null | undefined) => {
      try {
        setAttachmentError('')
        const prepared = await prepareClipboardImages(fileList || [], t)
        const combined = [...attachmentsRef.current, ...prepared].slice(0, 8)
        const binaryBytes = combined
          .filter((item) => item.kind !== 'path')
          .reduce((total, item) => total + (item.size || 0), 0)
        if (binaryBytes > MAX_TOTAL_ATTACHMENT_BYTES)
          throw new Error(t('chat:attachments.totalAttachmentSizeCannotExceed20MB'))
        replaceAttachments(combined)
        return true
      } catch (error) {
        setAttachmentError(error instanceof Error ? error.message : String(error))
        return false
      }
    },
    [replaceAttachments, t],
  )

  const pasteImages = useCallback(
    (event: ClipboardEvent<HTMLElement>) => {
      const images = clipboardImageFiles(event.clipboardData)
      if (!images.length) return
      event.preventDefault()
      void addClipboardImages(images)
    },
    [addClipboardImages],
  )

  const removeAttachment = useCallback(
    (id: string) => {
      replaceAttachments(attachmentsRef.current.filter((item) => item.id !== id))
    },
    [replaceAttachments],
  )

  const clearAttachments = useCallback(() => replaceAttachments([]), [replaceAttachments])

  const addAttachments = useCallback(
    (items: ChatAttachment[]) => {
      setAttachmentError('')
      const next = [...attachmentsRef.current]
      for (const item of items) {
        if (!next.some((existing) => existing.id === item.id)) next.push(item)
      }
      replaceAttachments(next.slice(0, 8))
    },
    [replaceAttachments],
  )

  const setError = useCallback((error: unknown) => {
    setAttachmentError(error instanceof Error ? error.message : String(error || ''))
  }, [])

  return {
    attachments,
    attachmentError,
    pasteImages,
    removeAttachment,
    clearAttachments,
    addAttachments,
    setError,
  }
}
