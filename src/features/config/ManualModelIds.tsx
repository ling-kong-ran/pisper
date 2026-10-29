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
        <div className="flex items-stretch gap-1.5">
          <input
            aria-label={t('config:configPage.manualModelIds')}
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
            variant="secondary"
            className="h-auto w-8.5 flex-none pointer-coarse:w-11"
            aria-label={t('config:configPage.manualModelIds')}
            disabled={!draft.trim()}
            onClick={add}
          >
            <Plus size={14} />
          </Button>
        </div>
      </FieldLabel>
      {ids.length > 0 && (
        <div className="flex flex-col overflow-hidden rounded-md border border-stroke">
          {ids.map((id) => (
            <div
              key={id}
              className="flex min-h-9 min-w-0 items-center gap-2 border-b border-stroke-soft py-1 pr-1 pl-2.5 last:border-b-0"
            >
              <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-content">
                {id}
              </span>
              <button
                type="button"
                aria-label={t('config:configPage.delete')}
                className="grid size-7 flex-none cursor-pointer place-items-center rounded-sm border-0 bg-transparent p-0 text-content-muted hover:bg-danger-soft hover:text-danger pointer-coarse:size-11"
                onClick={() => onChange(ids.filter((item) => item !== id))}
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
      <p className="text-xs text-content-muted">{t('config:configPage.manualModelIdsHint')}</p>
    </>
  )
}
