// 执行目标字段：Prompt 或已发布工作流选择及其输入参数。
import { AppSelect } from '@/components/common/AppSelect'
import { useI18n } from '@/app/i18n/use-i18n'
import { FieldLabel } from '@/components/ui/field'
import { workflowInputDefaults } from '@/features/schedules/model/schedule-utils'
import type {
  ScheduleTargetType,
  ScheduleWorkflow,
} from '@/features/schedules/model/schedule-types'

export function ScheduleTargetFields({
  targetType,
  prompt,
  workflowId,
  workflowInputs,
  workflows,
  onChange,
}: {
  targetType: ScheduleTargetType
  prompt: string
  workflowId: string
  workflowInputs: Record<string, unknown>
  workflows: ScheduleWorkflow[]
  onChange: (patch: {
    targetType?: ScheduleTargetType
    prompt?: string
    workflowId?: string
    workflowInputs?: Record<string, unknown>
  }) => void
}) {
  const { t } = useI18n()
  const workflow = workflows.find((item) => item.id === workflowId)
  return (
    <div className="grid gap-[10px]">
      <FieldLabel variant="control">
        {t('schedules:schedulesPage.executionTarget')}
        <AppSelect
          value={targetType}
          onChange={(event) => {
            const nextTarget = event.target.value as ScheduleTargetType
            const nextWorkflow = workflows[0]
            onChange(
              nextTarget === 'workflow'
                ? {
                    targetType: nextTarget,
                    workflowId: workflowId || nextWorkflow?.id || '',
                    workflowInputs: workflowId
                      ? workflowInputs
                      : workflowInputDefaults(nextWorkflow),
                  }
                : { targetType: nextTarget },
            )
          }}
        >
          <option value="prompt">Prompt</option>
          <option value="workflow">{t('schedules:schedulesPage.workflow')}</option>
        </AppSelect>
      </FieldLabel>
      {targetType === 'prompt' ? (
        <FieldLabel variant="control">
          Prompt
          <textarea
            value={prompt}
            onChange={(event) => onChange({ prompt: event.target.value })}
            placeholder={t('schedules:schedulesPage.describeTheWorkTheAgentShouldCompleteEachTime')}
          />
        </FieldLabel>
      ) : (
        <>
          <FieldLabel variant="control">
            {t('schedules:schedulesPage.workflow')}
            <AppSelect
              value={workflowId}
              onChange={(event) => {
                const nextWorkflow = workflows.find((item) => item.id === event.target.value)
                onChange({
                  workflowId: event.target.value,
                  workflowInputs: workflowInputDefaults(nextWorkflow),
                })
              }}
            >
              <option value="">{t('schedules:schedulesPage.selectPublishedWorkflow')}</option>
              {workflows.map((item) => (
                <option value={item.id} key={item.id}>
                  {item.name} · v{item.revision}
                </option>
              ))}
            </AppSelect>
            <small>
              {workflow?.description ||
                (workflows.length
                  ? t('schedules:schedulesPage.workflowUsesItsOwnRuntimeSettings')
                  : t('schedules:schedulesPage.noPublishedWorkflows'))}
            </small>
          </FieldLabel>
          {workflow?.inputs.length ? (
            <div className="grid gap-[8px]">
              <div className="schedule-workflow-inputs-heading [&_strong]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-right max-[650px]:items-start max-[650px]:flex-col max-[650px]:gap-[2px] max-[650px]:[&_small]:text-left flex [align-items:baseline] justify-between gap-[12px]">
                <strong>{t('schedules:schedulesPage.workflowInputs')}</strong>
                <small>{t('schedules:schedulesPage.workflowInputsHelp')}</small>
              </div>
              <div className="schedule-workflow-inputs [&_input[type='checkbox']]:w-[16px] [&_input[type='checkbox']]:h-[16px] [&_input[type='checkbox']]:[justify-self:end] max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,minmax(0,1fr))] gap-[8px]">
                {workflow.inputs.map((input) => (
                  <FieldLabel
                    variant="control"
                    className={
                      input.type === 'boolean'
                        ? 'grid grid-cols-[minmax(0,1fr)_auto] items-center'
                        : undefined
                    }
                    key={input.id}
                  >
                    {input.label}
                    {input.required ? ' *' : ''}
                    {input.type === 'boolean' ? (
                      <input
                        type="checkbox"
                        checked={Boolean(workflowInputs[input.name])}
                        onChange={(event) =>
                          onChange({
                            workflowInputs: {
                              ...workflowInputs,
                              [input.name]: event.target.checked,
                            },
                          })
                        }
                      />
                    ) : (
                      <input
                        type={input.type === 'number' ? 'number' : 'text'}
                        required={input.required}
                        value={String(workflowInputs[input.name] ?? '')}
                        onChange={(event) =>
                          onChange({
                            workflowInputs: {
                              ...workflowInputs,
                              [input.name]: event.target.value,
                            },
                          })
                        }
                      />
                    )}
                    {input.description && <small>{input.description}</small>}
                  </FieldLabel>
                ))}
              </div>
            </div>
          ) : null}
        </>
      )}
    </div>
  )
}
