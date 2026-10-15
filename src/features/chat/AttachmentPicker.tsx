// 附件由系统选择：桌面原生桥接保留路径引用，Web/手机通过浏览器上传内容。
import { useRef, type ChangeEvent } from 'react'
import { Paperclip } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useIsMobileApp } from '@/stores/client-store'
import type { ChatAttachment } from '@/types/chat'
import { CHAT_ATTACHMENT_ACCEPT, pathAttachments } from './attachments'

type AttachmentPickerSelection = {
  attachments: ChatAttachment[]
  addFiles: (files: Iterable<File> | null | undefined) => Promise<boolean>
  addAttachments: (attachments: ChatAttachment[]) => void
  setError: (error: unknown) => void
}

export function AttachmentPicker({
  cwd,
  selection,
}: {
  cwd?: string
  selection: AttachmentPickerSelection
}) {
  const { t } = useI18n()
  const mobileApp = useIsMobileApp()
  const deviceFileInputRef = useRef<HTMLInputElement>(null)

  const chooseAttachments = async () => {
    if (mobileApp) {
      // 手机路径无法由远端 Runtime 读取，必须在用户手势内打开系统 picker。
      deviceFileInputRef.current?.click()
      return
    }
    if (window.pisperDesktop?.pickFiles) {
      try {
        const paths = await window.pisperDesktop.pickFiles(cwd)
        selection.addAttachments(pathAttachments(paths || []))
      } catch (caught) {
        selection.setError(caught)
      }
      return
    }
    deviceFileInputRef.current?.click()
  }

  const addDeviceFiles = (event: ChangeEvent<HTMLInputElement>) => {
    const files = [...(event.currentTarget.files || [])]
    // 允许用户删除附件后再次选择同一个文件。
    event.currentTarget.value = ''
    if (files.length) void selection.addFiles(files)
  }

  return (
    <>
      <input
        ref={deviceFileInputRef}
        type="file"
        className="hidden"
        accept={CHAT_ATTACHMENT_ACCEPT}
        multiple
        aria-label={t('chat:focusSession.addAttachment')}
        onChange={addDeviceFiles}
      />
      <button
        type="button"
        className="relative grid size-9 shrink-0 cursor-pointer place-items-center rounded-md border-0 bg-transparent text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        title={t('chat:focusSession.addAttachment')}
        aria-label={t('chat:focusSession.addAttachment')}
        onClick={() => void chooseAttachments()}
      >
        <Paperclip size={17} />
        {selection.attachments.length > 0 && (
          <span className="absolute -top-1 -right-1 grid min-w-3.5 place-items-center rounded-full border-2 border-background bg-primary px-0.5 text-xs text-primary-foreground">
            {selection.attachments.length}
          </span>
        )}
      </button>
    </>
  )
}
