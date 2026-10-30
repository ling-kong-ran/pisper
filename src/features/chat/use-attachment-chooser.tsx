// 附件由系统选择：桌面原生桥接保留路径引用，Web/手机通过浏览器上传内容。
import { useRef, type ChangeEvent } from 'react'
import { useI18n } from '@/app/use-i18n'
import { useIsMobileApp } from '@/stores/client-store'
import type { ChatAttachment } from '@/types/chat'
import { CHAT_ATTACHMENT_ACCEPT, pathAttachments } from './attachments'

export type AttachmentPickerSelection = {
  attachments: ChatAttachment[]
  addFiles: (files: Iterable<File> | null | undefined) => Promise<boolean>
  addAttachments: (attachments: ChatAttachment[]) => void
  setError: (error: unknown) => void
}

// 选择附件的行为与隐藏的文件输入：图标键（AttachmentPicker）与输入框“+”菜单里的“附件”共用。
// 手机与 Web 必须在用户手势内触发文件输入，因此 choose 要在点击 / 菜单选择的同一回调里同步调用。
export function useAttachmentChooser({
  cwd,
  selection,
}: {
  cwd?: string
  selection: AttachmentPickerSelection
}) {
  const { t } = useI18n()
  const mobileApp = useIsMobileApp()
  const deviceFileInputRef = useRef<HTMLInputElement>(null)

  const choose = async () => {
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

  const input = (
    <input
      ref={deviceFileInputRef}
      type="file"
      className="hidden"
      accept={CHAT_ATTACHMENT_ACCEPT}
      multiple
      aria-label={t('chat:focusSession.addAttachment')}
      onChange={addDeviceFiles}
    />
  )
  return { input, choose }
}
