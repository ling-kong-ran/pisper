// 设置 › 模型 › 子代理：三个模型档位各选一个对话模型，再列出全局子代理定义。
// 定义文件里优先写档位，这里把档位映射到本机配置的模型，项目里的定义因此能跨机器、跨供应商共享。
// 项目定义在 <项目>/.pisper/agents/ 下，只在该项目的会话里生效（同名覆盖全局），不在这里逐个项目列出。
import { useEffect, useId, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { compactModelName } from '@/lib/model-display'
import { AppSelect } from '@/components/AppSelect'
import { FeatureAlert } from '@/components/app/FeaturePage'
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { Notify } from '@/app/route-context'
import { SUBAGENT_MODEL_TIERS, type ConfigData, type SubagentModelTier } from './config-types'
import { subagentApi, type AgentProfileList } from './subagent-api'

type SubagentSettingsProps = {
  config: ConfigData
  notify: Notify
  busy: boolean
  onConfigChanged: (data: ConfigData) => void
}

export function SubagentSettings({ config, notify, busy, onConfigChanged }: SubagentSettingsProps) {
  const { t } = useI18n()
  const tierLabel = (tier: SubagentModelTier) =>
    tier === 'fast'
      ? t('config:subagents.tierFast')
      : tier === 'standard'
        ? t('config:subagents.tierStandard')
        : t('config:subagents.tierStrong')
  const tierHint = (tier: SubagentModelTier) =>
    tier === 'fast'
      ? t('config:subagents.tierFastHint')
      : tier === 'standard'
        ? t('config:subagents.tierStandardHint')
        : t('config:subagents.tierStrongHint')
  const id = useId()
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [listing, setListing] = useState<AgentProfileList | null>(null)
  const [listError, setListError] = useState('')
  // 乐观显示刚选的值；保存失败时回到服务端的值。
  const savedTiers = config.subagentModelTiers || {}
  const savedKey = SUBAGENT_MODEL_TIERS.map((tier) => savedTiers[tier] || '').join('\n')
  const [tiers, setTiers] = useState(savedTiers)
  useEffect(() => {
    const values = savedKey.split('\n')
    setTiers(
      Object.fromEntries(
        SUBAGENT_MODEL_TIERS.map((tier, index) => [tier, values[index]]).filter(
          ([, value]) => value,
        ),
      ),
    )
  }, [savedKey])

  useEffect(() => {
    let active = true
    subagentApi
      .listProfiles()
      .then((result) => active && setListing(result))
      .catch((caught: unknown) => {
        if (!active) return
        setListing({ profiles: [], errors: [] })
        setListError(caught instanceof Error ? caught.message : String(caught))
      })
    return () => {
      active = false
    }
  }, [])

  const chatModels = config.providers
    .filter((provider) => provider.type === 'chat' && provider.configured && provider.enabled)
    .flatMap((provider) =>
      provider.models
        .filter((model) => model.kind === 'chat')
        .map((model) => ({ provider, name: model.name || model.id, id: model.id })),
    )
  // 带路径前缀的模型 ID 只显示能区分的最后几段，下拉框里看得出是哪个模型。
  const chatOptions = chatModels.map(({ provider, name, id }) => ({
    value: `${provider.id}/${id}`,
    label: `${compactModelName(
      name,
      chatModels.map((item) => item.name),
    )} · ${provider.name}`,
  }))

  const saveTier = async (tier: SubagentModelTier, ref: string) => {
    const previous = tiers
    const next = { ...tiers, [tier]: ref }
    if (!ref) delete next[tier]
    setTiers(next)
    setSaving(true)
    setError('')
    try {
      onConfigChanged(await subagentApi.saveModelTiers(next))
      notify(t('config:subagents.tiersSaved'))
    } catch (caught) {
      setTiers(previous)
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  const configuredTiers = SUBAGENT_MODEL_TIERS.filter((tier) => tiers[tier]).length
  const profiles = listing?.profiles || []
  const errors = listing?.errors || []
  return (
    <section
      aria-labelledby={`${id}-heading`}
      aria-busy={saving || undefined}
      data-config-card="models-subagents"
      data-config-target="models-subagents"
      className="min-w-0 border-t border-stroke"
    >
      {/* 大多数人不用分档位：默认收起，标题旁写现在是跟随会话模型还是设了几档。 */}
      <Accordion type="single" collapsible>
        <AccordionItem value="subagents">
          <AccordionTrigger>
            <span className="flex min-w-0 flex-1 items-baseline gap-2">
              <span id={`${id}-heading`}>{t('config:subagents.title')}</span>
              <span className="truncate text-xs font-normal text-content-muted">
                {configuredTiers
                  ? t('config:subagents.configuredSummary', { count: configuredTiers })
                  : t('config:subagents.followSession')}
              </span>
            </span>
          </AccordionTrigger>
          <AccordionContent className="h-auto">
            <p className="text-xs text-content-muted">{t('config:subagents.hint')}</p>
            <div className="mt-3 border-t border-stroke">
              {SUBAGENT_MODEL_TIERS.map((tier) => {
                const value = tiers[tier] || ''
                return (
                  <div
                    key={tier}
                    className="flex min-w-0 flex-wrap items-center gap-x-6 gap-y-2 border-b border-stroke py-3"
                  >
                    <div className="min-w-[min(240px,100%)] flex-1">
                      <div id={`${id}-${tier}`} className="text-sm text-content">
                        {tierLabel(tier)}
                        <code className="ml-2 font-mono text-xs text-content-muted">{tier}</code>
                      </div>
                      <p id={`${id}-${tier}-hint`} className="mt-0.5 text-xs text-content-muted">
                        {tierHint(tier)}
                      </p>
                    </div>
                    <div className="w-[min(280px,100%)] min-w-0 max-[650px]:w-full">
                      <AppSelect
                        aria-labelledby={`${id}-${tier}`}
                        aria-describedby={`${id}-${tier}-hint`}
                        className="w-full"
                        disabled={!chatOptions.length || busy || saving}
                        value={value}
                        onChange={(event) => void saveTier(tier, event.target.value)}
                      >
                        <option value="">{t('config:subagents.followSession')}</option>
                        {/* 已保存但现在不可用的模型也显示出来，便于看见并换掉。 */}
                        {value && !chatOptions.some((option) => option.value === value) && (
                          <option value={value}>{value}</option>
                        )}
                        {chatOptions.map((option) => (
                          <option key={option.value} value={option.value}>
                            {option.label}
                          </option>
                        ))}
                      </AppSelect>
                    </div>
                  </div>
                )
              })}
            </div>
            {error && <FeatureAlert>{error}</FeatureAlert>}

            <h3 className="mt-5 text-[13px] font-medium text-content">
              {t('config:subagents.globalDefinitions')}
            </h3>
            <p className="mt-0.5 text-xs text-content-muted">
              {t('config:subagents.definitionsHint')}
            </p>
            {listing === null ? (
              <p role="status" className="mt-3 flex items-center gap-2 text-xs text-content-muted">
                <OrbitSpinner className="size-3" />
                {t('config:subagents.loading')}
              </p>
            ) : profiles.length === 0 && errors.length === 0 ? (
              <p className="mt-3 text-xs text-content-muted">{t('config:subagents.empty')}</p>
            ) : (
              <ul className="mt-3 flex min-w-0 flex-col border-t border-stroke">
                {profiles.map((profile) => (
                  <li
                    key={profile.name}
                    className="min-w-0 border-b border-stroke py-2 text-[13px]"
                  >
                    <div className="flex min-w-0 items-center gap-2">
                      <code className="shrink-0 font-mono text-content">{profile.name}</code>
                      <span className="min-w-0 truncate text-xs text-content-muted">
                        {profile.modelRef || profile.tier || t('config:subagents.inheritModel')}
                        {profile.thinkingLevel ? ` · ${profile.thinkingLevel}` : ''}
                        {profile.tools
                          ? ` · ${t('config:subagents.toolCount', { count: profile.tools.length })}`
                          : ''}
                      </span>
                    </div>
                    <p className="mt-0.5 text-xs text-content-secondary">{profile.description}</p>
                  </li>
                ))}
                {errors.map((item) => (
                  <li
                    key={`${item.scope}:${item.file}`}
                    className="min-w-0 border-b border-stroke py-2 text-[13px]"
                  >
                    <code className="font-mono text-danger">{item.file || item.scope}</code>
                    <p className="mt-0.5 text-xs text-danger">{item.message}</p>
                  </li>
                ))}
              </ul>
            )}
            {listError && <FeatureAlert>{listError}</FeatureAlert>}
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </section>
  )
}
