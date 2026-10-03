// 执行模式选择控件：当前定时任务仅支持完全访问模式。
import { AppSelect } from '@/components/common/AppSelect'
import { useI18n } from '@/app/i18n/use-i18n'
import { FieldLabel } from '@/components/ui/field'
import { SCHEDULE_EXECUTION_MODES } from '@/features/schedules/model/schedule-constants'
import { executionModeHelp } from '@/features/schedules/model/schedule-utils'
import type { ScheduleExecutionMode } from '@/features/schedules/model/schedule-types'

export function ScheduleExecutionModeField({
  value,
  onChange,
}: {
  value: ScheduleExecutionMode
  onChange: (value: ScheduleExecutionMode) => void
}) {
  const { t } = useI18n()
  return (
    <FieldLabel variant="control">
      {t('schedules:schedulesPage.executionMode')}
      <AppSelect
        value={value}
        onChange={(event) => onChange(event.target.value as ScheduleExecutionMode)}
      >
        {SCHEDULE_EXECUTION_MODES.map((mode) => (
          <option value={mode} key={mode}>
            {t('schedules:schedulesPage.fullAccess')}
          </option>
        ))}
      </AppSelect>
      <small>{executionModeHelp(t)}</small>
    </FieldLabel>
  )
}
