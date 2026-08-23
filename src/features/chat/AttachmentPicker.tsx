// 附件入口：手机上传本机文件内容，桌面与 Web 保留 Runtime 路径引用。
import { useRef, useState, type ChangeEvent } from 'react'
import { Paperclip } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useIsMobileApp } from '@/stores/client-store'
import type { ChatAttachment } from '@/types/chat'
import { CHAT_ATTACHMENT_ACCEPT, pathAttachments } from './attachments'
import { PathAttachmentPicker } from './PathAttachmentPicker'

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
  const [pathPickerOpen, setPathPickerOpen] = useState(false)
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
    setPathPickerOpen(true)
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
        className="attach-trigger [.focus-composer_&]:h-[38px] [.focus-composer_&]:border-0 [.focus-composer_&]:rounded-[var(--r-sm)] [.focus-composer_&]:bg-[var(--surface-subtle)] [.focus-composer_&]:text-[12px] [.focus-composer_&]:w-[38px] [.focus-composer_&]:min-w-[38px] [.focus-session.has-conversation_.focus-composer_&]:w-[36px] [.focus-session.has-conversation_.focus-composer_&]:min-w-[36px] [.focus-session.has-conversation_.focus-composer_&]:h-[36px] relative grid place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--star-strong)] [&_i]:absolute [&_i]:top-[-4px] [&_i]:right-[-4px] [&_i]:grid [&_i]:min-w-[14px] [&_i]:h-[14px] [&_i]:place-items-center [&_i]:[border:2px_solid_var(--solid)] [&_i]:rounded-[var(--r-pill)] [&_i]:bg-[var(--blue)] [&_i]:text-[var(--on-accent)] [&_i]:p-[0_3px] [&_i]:text-[13px] [&_i]:[font-style:normal]"
        title={t('chat:focusSession.addAttachment')}
        aria-label={t('chat:focusSession.addAttachment')}
        onClick={() => void chooseAttachments()}
      >
        <Paperclip size={17} />
        {selection.attachments.length > 0 && <i>{selection.attachments.length}</i>}
      </button>
      <PathAttachmentPicker
        open={pathPickerOpen}
        initialPath={cwd}
        onOpenChange={setPathPickerOpen}
        onSelect={(paths) => selection.addAttachments(pathAttachments(paths))}
      />
    </>
  )
}
