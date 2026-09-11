// 手动追加模型 ID 的共享交互：输入框 + 号（或回车）逐个追加，条目可单独移除。
// 供快速配置向导与 Provider 编辑对话框复用，保证两处交互一致。
import { useState } from 'react'
import { Plus, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'

import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'

type ManualModelIdsProps = {
  ids: string[]
  onChange: (ids: string[]) => void
  placeholder?: string
  // 主模型 ID：追加时与其去重，避免保存出重复条目。
  primaryId?: string
}

export function ManualModelIds({
  ids,
  onChange,
  placeholder,
  primaryId = '',
}: ManualModelIdsProps) {
  const { t } = useI18n()
  const [draft, setDraft] = useState('')
  // 空值、与主模型或已追加项重复时忽略，保持列表干净。
  const add = () => {
    const id = draft.trim()
    if (!id || id === primaryId.trim() || ids.includes(id)) return
    onChange([...ids, id])
    setDraft('')
  }
  return (
    <>
      <FieldLabel variant="control">
        {t('config:configPage.manualModelIds')}
        <div className="flex items-stretch gap-[6px]">
          <input
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                // 阻止表单提交：回车语义是「追加这一条」。
                event.preventDefault()
                add()
              }
            }}
            placeholder={placeholder}
          />
          <Button
            type="button"
            size="icon"
            className="h-[31px] w-[31px] flex-none"
            aria-label={t('config:configPage.manualModelIds')}
            disabled={!draft.trim()}
            onClick={add}
          >
            <Plus size={14} />
          </Button>
        </div>
      </FieldLabel>
      {ids.length > 0 && (
        <div className="flex flex-col gap-[6px]">
          {ids.map((id) => (
            <div
              key={id}
              className="flex min-w-0 items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[6px_10px]"
            >
              <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                {id}
              </span>
              <button
                type="button"
                aria-label={t('config:configPage.delete')}
                className="grid w-[20px] h-[20px] flex-none cursor-pointer place-items-center rounded-full border-0 bg-transparent p-0 text-[var(--text-muted)] hover:bg-[var(--surface-highlight)] hover:text-[var(--text)]"
                onClick={() => onChange(ids.filter((item) => item !== id))}
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
      <p className="[margin:0] text-[11px] text-[var(--text-tertiary)]">
        {t('config:configPage.manualModelIdsHint')}
      </p>
    </>
  )
}
