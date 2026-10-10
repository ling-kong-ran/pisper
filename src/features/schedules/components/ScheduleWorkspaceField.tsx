// 工作目录浏览统一调用系统选择器，仍支持直接输入已知路径。
import { useState } from 'react'
import { AlertTriangle, FolderOpen } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { AppError } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { pickSystemDirectory } from '@/lib/platform/pick-system-directory'

export function ScheduleWorkspaceField({
  value,
  onChange,
}: {
  value: string
  onChange: (value: string) => void
}) {
  const { t } = useI18n()
  const [pickerError, setPickerError] = useState('')
  const [picking, setPicking] = useState(false)

  // 系统取消不修改已有目录。
  const browse = async () => {
    setPickerError('')
    setPicking(true)
    try {
      const selected = await pickSystemDirectory(value)
      if (selected) onChange(selected)
    } catch (error) {
      setPickerError(error instanceof Error ? error.message : String(error))
    } finally {
      setPicking(false)
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
            disabled={picking}
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
    </>
  )
}
