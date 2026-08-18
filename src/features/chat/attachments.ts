// 附件选择：支持粘贴/拖拽/文件选择，统一转换成 ChatAttachment，
// 超过大小上限（10MB）的附件被拒绝并提示。
import { useCallback, useRef, useState, type ClipboardEvent } from 'react'
import { storedLanguage, translateText, type I18nValues } from '@/app/i18n.ts'
import type { ChatAttachment } from '@/types/chat'

const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024
const MAX_TOTAL_ATTACHMENT_BYTES = 20 * 1024 * 1024
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
const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'])
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

export function clipboardFiles(clipboardData: DataTransfer | null | undefined): File[] {
  const files = [...(clipboardData?.files || [])]
  if (files.length) return files
  return [...(clipboardData?.items || [])]
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile?.())
    .filter((file): file is File => Boolean(file))
}

type Translate = (message: string, values?: I18nValues) => string

function fileToBase64(file: File, t: Translate) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).split(',')[1] || '')
    reader.onerror = () => reject(reader.error || new Error(t('chat:attachments.couldNotReadFile')))
    reader.readAsDataURL(file)
  })
}

function fileExtension(name: string) {
  return name.toLowerCase().split('.').at(-1) || ''
}

async function prepareClipboardFiles(
  fileList: Iterable<File>,
  t: Translate,
): Promise<ChatAttachment[]> {
  const files = [...fileList].slice(0, 8)
  const attachments: ChatAttachment[] = []
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES)
      throw new Error(t('chat:attachments.nameExceedsThe10MBLimit', { name: file.name }))
    const extension = fileExtension(file.name)
    const common = {
      id: `${file.name}-${file.lastModified}-${file.size}`,
      name: file.name,
      mimeType: file.type || 'application/octet-stream',
      size: file.size,
    }
    if (file.type.startsWith('image/') || IMAGE_EXTENSIONS.has(extension)) {
      attachments.push({ ...common, kind: 'image', data: await fileToBase64(file, t) })
    } else if (
      file.type.startsWith('text/') ||
      file.type === 'application/json' ||
      TEXT_EXTENSIONS.has(extension)
    ) {
      attachments.push({ ...common, kind: 'text', text: await file.text() })
    } else if (DOCUMENT_EXTENSIONS.has(extension)) {
      attachments.push({
        ...common,
        kind: 'document',
        extension,
        data: await fileToBase64(file, t),
      })
    } else {
      throw new Error(
        t('chat:attachments.nameIsNotSupportedChooseAnImageOrTextCodeFile', { name: file.name }),
      )
    }
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

export function useAttachmentSelection(
  initialAttachments: ChatAttachment[] = [],
  onAttachmentsChange?: (attachments: ChatAttachment[]) => void,
) {
  const t = useCallback(
    (message: string, values?: I18nValues) => translateText(message, storedLanguage(), values),
    [],
  )
  const [attachments, setAttachments] = useState<ChatAttachment[]>(() => [...initialAttachments])
  const [attachmentError, setAttachmentError] = useState('')
  const attachmentsRef = useRef<ChatAttachment[]>([...initialAttachments])

  const replaceAttachments = useCallback(
    (items: ChatAttachment[]) => {
      attachmentsRef.current = items
      setAttachments(items)
      onAttachmentsChange?.(items)
    },
    [onAttachmentsChange],
  )

  const addClipboardFiles = useCallback(
    async (fileList: Iterable<File> | null | undefined) => {
      try {
        setAttachmentError('')
        const prepared = await prepareClipboardFiles(fileList || [], t)
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

  const pasteFiles = useCallback(
    (event: ClipboardEvent<HTMLElement>) => {
      const files = clipboardFiles(event.clipboardData)
      if (!files.length) return
      event.preventDefault()
      void addClipboardFiles(files)
    },
    [addClipboardFiles],
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
    pasteFiles,
    removeAttachment,
    clearAttachments,
    addAttachments,
    replaceAttachments,
    setError,
  }
}
