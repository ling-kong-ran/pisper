// 附件入口：桌面桥接选择路径；Web 与手机通过系统选择器上传文件内容。
import { useRef, useState, type ChangeEvent } from 'react'
import { Paperclip } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { ComposerActionButton } from '@/features/chat/components/composer/ComposerActionButton'
import { useIsMobileApp } from '@/stores/client-store'
import type { ChatAttachment } from '@/types/chat'
import { CHAT_ATTACHMENT_ACCEPT, pathAttachments } from '@/features/chat/model/attachments'

type AttachmentPickerSelection = {
  attachments: ChatAttachment[]
  addFiles: (files: Iterable<File> | null | undefined) => Promise<boolean>
  addAttachments: (attachments: ChatAttachment[]) => void
  setError: (error: unknown) => void
}

export function AttachmentPicker({
  cwd,
  selection,
  menu = false,
}: {
  cwd?: string
  menu?: boolean
  selection: AttachmentPickerSelection
}) {
  const { t } = useI18n()
  const mobileApp = useIsMobileApp()
  const [picking, setPicking] = useState(false)
  const pickingRef = useRef(false)
  const deviceFileInputRef = useRef<HTMLInputElement>(null)

  const chooseAttachments = async () => {
    if (mobileApp || !window.pisperDesktop?.pickFiles) {
      // 手机路径无法由远端 Runtime 读取，必须在用户手势内打开系统 picker。
      deviceFileInputRef.current?.click()
      return
    }
    if (pickingRef.current) return
    pickingRef.current = true
    setPicking(true)
    if (window.pisperDesktop?.pickFiles) {
      try {
        const paths = await window.pisperDesktop.pickFiles(cwd)
        selection.addAttachments(pathAttachments(paths || []))
      } catch (caught) {
        selection.setError(caught)
      } finally {
        pickingRef.current = false
        setPicking(false)
      }
      return
    }
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
      <ComposerActionButton
        menu={menu}
        label={t('chat:focusSession.addAttachment')}
        className="attach-trigger"
        disabled={picking}
        onClick={() => void chooseAttachments()}
      >
        <Paperclip size={17} />
        {selection.attachments.length > 0 && (
          <span className="absolute -right-1 -top-1 grid min-w-4 place-items-center rounded-full bg-primary px-1 text-[10px] text-primary-foreground">
            {selection.attachments.length}
          </span>
        )}
      </ComposerActionButton>
    </>
  )
}
