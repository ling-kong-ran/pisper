// 工作目录选择控件：优先使用系统选择器，Web 环境退回对话框。
import { useState } from 'react'
import { AlertTriangle, FolderOpen } from 'lucide-react'
import { WorkspacePicker } from '@/components/common/WorkspacePicker'
import { useI18n } from '@/app/i18n/use-i18n'
import { AppError } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { hasSystemDirectoryPicker, pickSystemDirectory } from '@/lib/platform/pick-system-directory'

export function ScheduleWorkspaceField({
  value,
  onChange,
}: {
  value: string
  onChange: (value: string) => void
}) {
  const { t } = useI18n()
  const [pickerError, setPickerError] = useState('')
  const [webPickerOpen, setWebPickerOpen] = useState(false)

  // 浏览选择工作目录：桌面环境用系统选择器，否则退回 Web 输入对话框。
  const browse = async () => {
    setPickerError('')
    if (!hasSystemDirectoryPicker()) {
      setWebPickerOpen(true)
      return
    }
    try {
      const selected = await pickSystemDirectory(value)
      if (selected) onChange(selected)
    } catch (error) {
      setPickerError(error instanceof Error ? error.message : String(error))
    }
  }

  return (
    <>
      <FieldLabel variant="control">
        {t('schedules:schedulesPage.workingDirectory')}
        <span className="schedule-workspace-input [&_input]:min-w-0 [&_input]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] grid grid-cols-[minmax(0,1fr)_auto] gap-[6px]">
          <input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder={t('schedules:schedulesPage.enterTheProjectSAbsolutePath')}
          />
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={() => void browse()}
          >
            <FolderOpen size={13} />
            {t('schedules:schedulesPage.browseDirectories')}
          </Button>
        </span>
        <small>{t('schedules:schedulesPage.theScheduledAgentWillRunInThisDirectory')}</small>
      </FieldLabel>
      {pickerError && (
        <AppError>
          <AlertTriangle size={13} />
          {pickerError}
        </AppError>
      )}
      <WorkspacePicker
        open={webPickerOpen}
        initialPath={value}
        description={t('common:workspacePicker.selectWorkspaceForSchedule')}
        onOpenChange={setWebPickerOpen}
        onSelect={(selected) => onChange(selected)}
      />
    </>
  )
}
