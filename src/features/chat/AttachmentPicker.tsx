// 输入区的附件图标键（任务工作区的来源操作位使用）；选择逻辑在 use-attachment-chooser。
import { Paperclip } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useAttachmentChooser, type AttachmentPickerSelection } from './use-attachment-chooser'

export function AttachmentPicker({
  cwd,
  selection,
}: {
  cwd?: string
  selection: AttachmentPickerSelection
}) {
  const { t } = useI18n()
  const { input, choose } = useAttachmentChooser({ cwd, selection })

  return (
    <>
      {input}
      <button
        type="button"
        className="relative grid size-[30px] min-w-[30px] flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent text-content-muted transition-colors hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-50 max-[650px]:size-11 max-[650px]:min-w-11 pointer-coarse:size-11 pointer-coarse:min-w-11"
        title={t('chat:focusSession.addAttachment')}
        aria-label={t('chat:focusSession.addAttachment')}
        onClick={() => void choose()}
      >
        <Paperclip size={16} />
        {selection.attachments.length > 0 && (
          <span className="absolute -top-1 -right-1 grid h-4 min-w-4 place-items-center rounded-full border-2 border-panel bg-star px-0.5 text-[10px] leading-none font-medium text-on-accent tabular-nums">
            {selection.attachments.length}
          </span>
        )}
      </button>
    </>
  )
}
