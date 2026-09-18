// 多 API Key 追加输入：密码只在本次表单状态中存在，列表始终以掩码展示。
import { useRef } from 'react'
import { KeyRound, Plus, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'

import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'

type ApiKeySummary = { id: string; hint: string }

type ApiKeyListProps = {
  keys: string[]
  onChange: (keys: string[]) => void
  draft: string
  onDraftChange: (draft: string) => void
  existing?: ApiKeySummary[]
}

function maskKey(key: string) {
  return key.length <= 8 ? '********' : `${key.slice(0, 3)}...${key.slice(-4)}`
}

export function ApiKeyList({
  keys,
  onChange,
  draft,
  onDraftChange,
  existing = [],
}: ApiKeyListProps) {
  const { t } = useI18n()
  const inputRef = useRef<HTMLInputElement>(null)
  const add = () => {
    const key = draft.trim()
    if (!key) return
    if (!keys.includes(key)) onChange([...keys, key])
    onDraftChange('')
    inputRef.current?.focus()
  }

  return (
    <div className="grid min-w-0 gap-1.5">
      <FieldLabel variant="control">
        {t('config:configPage.apiKeys')}
        <div className="flex min-w-0 items-stretch gap-[6px]">
          <input
            className="min-w-0 flex-1"
            type="password"
            autoComplete="new-password"
            ref={inputRef}
            value={draft}
            onChange={(event) => onDraftChange(event.target.value)}
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
        <div
          className="flex min-w-0 flex-wrap items-center gap-1.5"
          aria-label={t('config:configPage.apiKeys')}
        >
          {existing.map((key) => (
            <div
              key={key.id}
              className="inline-flex min-w-0 max-w-full items-center gap-2 rounded-[var(--r-sm)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)] px-2 py-1"
            >
              <KeyRound size={13} className="flex-none text-[var(--text-muted)]" />
              <span className="min-w-0 truncate font-mono text-xs">{key.hint}</span>
            </div>
          ))}
          {keys.map((key) => (
            <div
              key={key}
              className="inline-flex min-w-0 max-w-full items-center gap-2 rounded-[var(--r-sm)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)] px-2 py-1"
            >
              <KeyRound size={13} className="flex-none text-[var(--text-muted)]" />
              <span className="min-w-0 truncate font-mono text-xs">{maskKey(key)}</span>
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
    </div>
  )
}
