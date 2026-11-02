// 设置 › 模型 › 权限规则：按命令开头或目录批量允许、按命令开头禁止工具调用。
// 这里添加的规则对所有项目生效；审批卡片上“以后都允许”添加的是当前项目的规则，也列在这里可删除。
import { useEffect, useId, useState } from 'react'
import { Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { FeatureAlert } from '@/components/app/FeaturePage'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { Notify } from '@/app/route-context'
import { permissionRulesApi, type PermissionRule } from './permission-rules-api'

function scopeName(cwd: string) {
  return cwd.replaceAll('\\', '/').split('/').filter(Boolean).at(-1) || cwd
}

export function PermissionRulesSettings({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const id = useId()
  const [rules, setRules] = useState<PermissionRule[] | null>(null)
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)
  const [effect, setEffect] = useState<PermissionRule['effect']>('allow')
  const [tool, setTool] = useState<PermissionRule['tool']>('bash')
  const [pattern, setPattern] = useState('')

  useEffect(() => {
    let active = true
    permissionRulesApi
      .list()
      .then((result) => active && setRules(result.rules))
      .catch((caught: unknown) => {
        if (!active) return
        setRules([])
        setError(caught instanceof Error ? caught.message : String(caught))
      })
    return () => {
      active = false
    }
  }, [])

  const run = async (task: () => Promise<{ rules: PermissionRule[] }>, message: string) => {
    setSaving(true)
    setError('')
    try {
      setRules((await task()).rules)
      notify(message)
      return true
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
      return false
    } finally {
      setSaving(false)
    }
  }

  const add = async () => {
    const value = pattern.trim()
    if (!value) return
    if (
      await run(
        () => permissionRulesApi.add({ effect, tool, pattern: value }),
        t('config:permissionRules.added'),
      )
    )
      setPattern('')
  }

  return (
    <section
      aria-labelledby={`${id}-heading`}
      data-config-card="models-permission-rules"
      data-config-target="models-permission-rules"
      className="mt-6 min-w-0"
    >
      <h2 id={`${id}-heading`} className="text-sm font-medium text-content">
        {t('config:permissionRules.title')}
      </h2>
      <p className="mt-0.5 text-xs text-content-muted">{t('config:permissionRules.hint')}</p>
      {rules === null ? (
        <p role="status" className="mt-3 flex items-center gap-2 text-xs text-content-muted">
          <OrbitSpinner className="size-3" />
          {t('config:permissionRules.loading')}
        </p>
      ) : rules.length === 0 ? (
        <p className="mt-3 text-xs text-content-muted">{t('config:permissionRules.empty')}</p>
      ) : (
        <ul className="mt-3 flex min-w-0 flex-col border-t border-stroke">
          {rules.map((rule) => (
            <li
              key={rule.id}
              className="flex min-w-0 items-center gap-2 border-b border-stroke py-2 text-[13px]"
            >
              <span
                className={
                  rule.effect === 'deny'
                    ? 'shrink-0 rounded-sm bg-danger-soft px-1.5 text-xs text-danger'
                    : 'shrink-0 rounded-sm bg-surface-hover px-1.5 text-xs text-content-secondary'
                }
              >
                {rule.effect === 'deny'
                  ? t('config:permissionRules.deny')
                  : t('config:permissionRules.allow')}
              </span>
              <span className="shrink-0 text-xs text-content-muted">
                {rule.tool === 'files'
                  ? t('config:permissionRules.files')
                  : t('config:permissionRules.command')}
              </span>
              <code className="min-w-0 flex-1 truncate font-mono text-content" title={rule.pattern}>
                {rule.pattern}
              </code>
              <span
                className="max-w-[40%] shrink-0 truncate text-xs text-content-muted"
                title={rule.cwd || undefined}
              >
                {rule.cwd ? scopeName(rule.cwd) : t('config:permissionRules.global')}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                className="shrink-0 pointer-coarse:size-11"
                disabled={saving}
                aria-label={t('config:permissionRules.remove', { pattern: rule.pattern })}
                title={t('config:permissionRules.remove', { pattern: rule.pattern })}
                onClick={() =>
                  void run(
                    () => permissionRulesApi.remove(rule.id),
                    t('config:permissionRules.removed'),
                  )
                }
              >
                <Trash2 />
              </Button>
            </li>
          ))}
        </ul>
      )}
      <form
        className="mt-3 flex min-w-0 flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          void add()
        }}
      >
        <AppSelect
          aria-label={t('config:permissionRules.effectLabel')}
          className="w-28"
          disabled={saving}
          value={effect}
          onChange={(event) => {
            const next = event.target.value === 'deny' ? 'deny' : 'allow'
            setEffect(next)
            // 文件写入只有允许规则。
            if (next === 'deny') setTool('bash')
          }}
        >
          <option value="allow">{t('config:permissionRules.allow')}</option>
          <option value="deny">{t('config:permissionRules.deny')}</option>
        </AppSelect>
        <AppSelect
          aria-label={t('config:permissionRules.toolLabel')}
          className="w-32"
          disabled={saving || effect === 'deny'}
          value={tool}
          onChange={(event) => setTool(event.target.value === 'files' ? 'files' : 'bash')}
        >
          <option value="bash">{t('config:permissionRules.command')}</option>
          <option value="files">{t('config:permissionRules.files')}</option>
        </AppSelect>
        <Input
          aria-label={t('config:permissionRules.patternLabel')}
          className="h-[34px] min-w-[12rem] flex-1 font-mono text-[13px] max-[650px]:h-11"
          disabled={saving}
          maxLength={200}
          placeholder={
            tool === 'files'
              ? t('config:permissionRules.filesPlaceholder')
              : t('config:permissionRules.commandPlaceholder')
          }
          value={pattern}
          onChange={(event) => setPattern(event.target.value)}
        />
        <Button
          type="submit"
          variant="secondary"
          disabled={saving || !pattern.trim()}
          className="max-[650px]:min-h-11"
        >
          {saving && <OrbitSpinner className="size-3" />}
          {t('config:permissionRules.add')}
        </Button>
      </form>
      {error && <FeatureAlert>{error}</FeatureAlert>}
    </section>
  )
}
