// 多 API Key 追加输入：密码只在本次表单状态中存在，列表始终以掩码展示。
import { useState } from 'react'
import { KeyRound, Plus, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'

import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'

type ApiKeySummary = { id: string; hint: string }

type ApiKeyListProps = {
  keys: string[]
  onChange: (keys: string[]) => void
  existing?: ApiKeySummary[]
}

function maskKey(key: string) {
  return key.length <= 8 ? '********' : `${key.slice(0, 3)}...${key.slice(-4)}`
}

export function ApiKeyList({ keys, onChange, existing = [] }: ApiKeyListProps) {
  const { t } = useI18n()
  const [draft, setDraft] = useState('')
  const add = () => {
    const key = draft.trim()
    if (!key || keys.includes(key)) return
    onChange([...keys, key])
    setDraft('')
  }

  return (
    <>
      <FieldLabel variant="control">
        {t('config:configPage.apiKeys')}
        <div className="flex items-stretch gap-[6px]">
          <input
            type="password"
            autoComplete="new-password"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key !== 'Enter') return
              event.preventDefault()
              add()
            }}
            placeholder={t('config:configPage.enterTheProviderAPIKey')}
          />
          <Button
            type="button"
            size="icon"
            className="h-[31px] w-[31px] flex-none"
            aria-label={t('config:configPage.addApiKey')}
            disabled={!draft.trim()}
            onClick={add}
          >
            <Plus size={14} />
          </Button>
        </div>
      </FieldLabel>
      {(existing.length > 0 || keys.length > 0) && (
        <div className="flex flex-col gap-[6px]" aria-label={t('config:configPage.apiKeys')}>
          {existing.map((key) => (
            <div
              key={key.id}
              className="flex min-w-0 items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[6px_10px]"
            >
              <KeyRound size={13} className="flex-none text-[var(--text-muted)]" />
              <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                {key.hint}
              </span>
            </div>
          ))}
          {keys.map((key) => (
            <div
              key={key}
              className="flex min-w-0 items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[6px_10px]"
            >
              <KeyRound size={13} className="flex-none text-[var(--text-muted)]" />
              <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                {maskKey(key)}
              </span>
              <button
                type="button"
                aria-label={t('config:configPage.delete')}
                className="grid w-[20px] h-[20px] flex-none cursor-pointer place-items-center rounded-full border-0 bg-transparent p-0 text-[var(--text-muted)] hover:bg-[var(--surface-highlight)] hover:text-[var(--text)]"
                onClick={() => onChange(keys.filter((item) => item !== key))}
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
      <p className="[margin:0] text-[11px] text-[var(--text-tertiary)]">
        {t('config:configPage.apiKeysHint')}
      </p>
    </>
  )
}
