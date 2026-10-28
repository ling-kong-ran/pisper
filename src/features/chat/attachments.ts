// 附件选择：支持粘贴、拖拽与手机系统文件选择，统一转换成 ChatAttachment；
// 超过大小上限（10MB）的附件被拒绝并提示。
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
} from 'react'
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

export const CHAT_ATTACHMENT_ACCEPT = [
  'image/*',
  'text/*',
  'application/json',
  ...[...IMAGE_EXTENSIONS, ...TEXT_EXTENSIONS, ...DOCUMENT_EXTENSIONS].map(
    (extension) => `.${extension}`,
  ),
].join(',')

export function clipboardFiles(clipboardData: DataTransfer | null | undefined): File[] {
  const files = [...(clipboardData?.files || [])]
  if (files.length) return files
  return [...(clipboardData?.items || [])]
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile?.())
    .filter((file): file is File => Boolean(file))
}

// 浏览器拖放只提供 File，不提供 Runtime 可读的本地路径，因此与粘贴一样走内容附件。
export function isFileDrag(dataTransfer: DataTransfer | null | undefined) {
  return [...(dataTransfer?.types || [])].includes('Files')
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
  ownerKey?: string,
) {
  const t = useCallback(
    (message: string, values?: I18nValues) => translateText(message, storedLanguage(), values),
    [],
  )
  const [attachments, setAttachments] = useState<ChatAttachment[]>(() => [...initialAttachments])
  const [attachmentError, setAttachmentError] = useState('')
  const [preparingAttachments, setPreparingAttachments] = useState<{ id: string; name: string }[]>(
    [],
  )
  const preparations = useRef(new Map<string, { name: string; cancelled: boolean }>())
  const preparationSequence = useRef(0)
  const ownerRef = useRef(ownerKey)
  ownerRef.current = ownerKey
  useEffect(() => {
    const ownedPreparations = preparations.current
    setPreparingAttachments([])
    setAttachmentError('')
    return () => {
      for (const item of ownedPreparations.values()) item.cancelled = true
      ownedPreparations.clear()
    }
  }, [ownerKey])
  const attachmentsRef = useRef<ChatAttachment[]>([...initialAttachments])

  const replaceAttachments = useCallback(
    (items: ChatAttachment[]) => {
      attachmentsRef.current = items
      setAttachments(items)
      onAttachmentsChange?.(items)
    },
    [onAttachmentsChange],
  )

  const addFiles = useCallback(
    async (fileList: Iterable<File> | null | undefined) => {
      const owner = ownerKey
      const files = [...(fileList || [])].slice(0, 8)
      const jobs = files.map((file) => {
        const id = `preparing-${++preparationSequence.current}`
        const job = { name: file.name, cancelled: false }
        preparations.current.set(id, job)
        return { id, job, file }
      })
      const publish = () =>
        setPreparingAttachments(
          [...preparations.current].map(([id, item]) => ({ id, name: item.name })),
        )
      publish()
      setAttachmentError('')
      let added = false
      for (const { id, job, file } of jobs) {
        try {
          const prepared = await prepareClipboardFiles([file], t)
          // 文件读取无法真正取消；提交结果必须同时持有原会话和准备项的身份。
          if (job.cancelled || ownerRef.current !== owner || !preparations.current.has(id)) continue
          const combined = [...attachmentsRef.current, ...prepared].slice(0, 8)
          const binaryBytes = combined
            .filter((item) => item.kind !== 'path')
            .reduce((total, item) => total + (item.size || 0), 0)
          if (binaryBytes > MAX_TOTAL_ATTACHMENT_BYTES)
            throw new Error(t('chat:attachments.totalAttachmentSizeCannotExceed20MB'))
          replaceAttachments(combined)
          added = true
        } catch (error) {
          if (!job.cancelled && ownerRef.current === owner)
            setAttachmentError(error instanceof Error ? error.message : String(error))
        } finally {
          preparations.current.delete(id)
          if (ownerRef.current === owner) publish()
        }
      }
      return added
    },
    [ownerKey, replaceAttachments, t],
  )

  const pasteFiles = useCallback(
    (event: ClipboardEvent<HTMLElement>) => {
      const files = clipboardFiles(event.clipboardData)
      if (!files.length) return
      event.preventDefault()
      void addFiles(files)
    },
    [addFiles],
  )

  const dropFiles = useCallback(
    (event: DragEvent<HTMLElement>) => {
      if (!isFileDrag(event.dataTransfer)) return
      event.preventDefault()
      const files = clipboardFiles(event.dataTransfer)
      if (files.length) void addFiles(files)
    },
    [addFiles],
  )

  const removeAttachment = useCallback(
    (id: string) => {
      const pending = preparations.current.get(id)
      if (pending) {
        pending.cancelled = true
        preparations.current.delete(id)
        setPreparingAttachments(
          [...preparations.current].map(([key, item]) => ({ id: key, name: item.name })),
        )
        return
      }
      replaceAttachments(attachmentsRef.current.filter((item) => item.id !== id))
    },
    [replaceAttachments],
  )

  const clearAttachments = useCallback(() => {
    for (const item of preparations.current.values()) item.cancelled = true
    preparations.current.clear()
    setPreparingAttachments([])
    replaceAttachments([])
  }, [replaceAttachments])

  const addAttachments = useCallback(
    (items: ChatAttachment[]) => {
      if (ownerRef.current !== ownerKey) return
      setAttachmentError('')
      const next = [...attachmentsRef.current]
      for (const item of items) {
        if (!next.some((existing) => existing.id === item.id)) next.push(item)
      }
      replaceAttachments(next.slice(0, 8))
    },
    [ownerKey, replaceAttachments],
  )

  const setError = useCallback((error: unknown) => {
    setAttachmentError(error instanceof Error ? error.message : String(error || ''))
  }, [])

  return {
    attachments,
    attachmentError,
    preparingAttachments,
    addFiles,
    pasteFiles,
    dropFiles,
    removeAttachment,
    clearAttachments,
    addAttachments,
    replaceAttachments,
    setError,
  }
}
