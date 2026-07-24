import { useCallback, useRef, useState, type ChangeEvent, type ClipboardEvent } from 'react'
import { storedLanguage, translateText, type I18nValues } from '@/app/i18n.ts'
import type { ChatAttachment } from '@/types/chat'

const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024
const MAX_TOTAL_ATTACHMENT_BYTES = 20 * 1024 * 1024
const MAX_TEXT_CHARS = 200_000
const TEXT_EXTENSIONS = new Set([
  'txt',
  'md',
  'json',
  'js',
  'jsx',
  'ts',
  'tsx',
  'css',
  'html',
  'xml',
  'yaml',
  'yml',
  'csv',
  'log',
  'py',
  'java',
  'go',
  'rs',
  'sh',
  'ps1',
  'toml',
  'sql',
])
const DOCUMENT_EXTENSIONS = new Set([
  'pdf',
  'docx',
  'pptx',
  'xlsx',
  'odt',
  'odp',
  'ods',
  'rtf',
  'epub',
])

export function clipboardImageFiles(clipboardData: DataTransfer | null | undefined): File[] {
  const files = [...(clipboardData?.files || [])].filter((file) => file.type?.startsWith('image/'))
  if (files.length) return files
  return [...(clipboardData?.items || [])]
    .filter((item) => item.kind === 'file' && item.type?.startsWith('image/'))
    .map((item) => item.getAsFile?.())
    .filter((file): file is File => Boolean(file))
}

function fileExtension(name: string) {
  return name.includes('.') ? name.split('.').pop()?.toLowerCase() || '' : ''
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

async function prepareFiles(fileList: Iterable<File>, t: Translate): Promise<ChatAttachment[]> {
  const files = [...fileList].slice(0, 8)
  const attachments: ChatAttachment[] = []
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES)
      throw new Error(t('chat:attachments.nameExceedsThe10MBLimit', { name: file.name }))
    if (file.type.startsWith('image/')) {
      attachments.push({
        id: `${file.name}-${file.lastModified}-${file.size}`,
        kind: 'image',
        name: file.name,
        mimeType: file.type,
        size: file.size,
        data: await fileToBase64(file, t),
      })
      continue
    }
    const extension = fileExtension(file.name)
    if (file.type.startsWith('text/') || TEXT_EXTENSIONS.has(extension)) {
      const text = await file.text()
      attachments.push({
        id: `${file.name}-${file.lastModified}-${file.size}`,
        kind: 'text',
        name: file.name,
        mimeType: file.type || 'text/plain',
        size: file.size,
        text: text.slice(0, MAX_TEXT_CHARS),
        truncated: text.length > MAX_TEXT_CHARS,
      })
      continue
    }
    if (DOCUMENT_EXTENSIONS.has(extension)) {
      attachments.push({
        id: `${file.name}-${file.lastModified}-${file.size}`,
        kind: 'document',
        name: file.name,
        mimeType: file.type || 'application/octet-stream',
        extension,
        size: file.size,
        data: await fileToBase64(file, t),
      })
      continue
    }
    throw new Error(
      t('chat:attachments.nameIsNotSupportedChooseAnImageOrTextCodeFile', { name: file.name }),
    )
  }
  return attachments
}

export function useAttachmentSelection() {
  const t = useCallback(
    (message: string, values?: I18nValues) => translateText(message, storedLanguage(), values),
    [],
  )
  const [attachments, setAttachments] = useState<ChatAttachment[]>([])
  const [attachmentError, setAttachmentError] = useState('')
  const attachmentsRef = useRef<ChatAttachment[]>([])
  const inputRef = useRef<HTMLInputElement>(null)

  const replaceAttachments = useCallback((items: ChatAttachment[]) => {
    attachmentsRef.current = items
    setAttachments(items)
  }, [])

  const addFiles = useCallback(
    async (fileList: Iterable<File> | null | undefined) => {
      try {
        setAttachmentError('')
        const prepared = await prepareFiles(fileList || [], t)
        const combined = [...attachmentsRef.current, ...prepared].slice(0, 8)
        if (
          combined.reduce((total, item) => total + (item.size || 0), 0) > MAX_TOTAL_ATTACHMENT_BYTES
        )
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

  const chooseFiles = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const input = event.currentTarget
      await addFiles(input.files || [])
      input.value = ''
    },
    [addFiles],
  )

  const pasteImages = useCallback(
    (event: ClipboardEvent<HTMLElement>) => {
      const images = clipboardImageFiles(event.clipboardData)
      if (!images.length) return
      event.preventDefault()
      void addFiles(images)
    },
    [addFiles],
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
      const next = [...attachmentsRef.current]
      for (const item of items) {
        if (!next.some((existing) => existing.id === item.id)) next.push(item)
      }
      replaceAttachments(next.slice(0, 8))
    },
    [replaceAttachments],
  )

  return {
    attachments,
    attachmentError,
    inputRef,
    chooseFiles,
    pasteImages,
    removeAttachment,
    clearAttachments,
    addAttachments,
  }
}
