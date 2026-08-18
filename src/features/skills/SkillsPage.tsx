// 技能页：查看/安装/卸载 Agent 技能包，展示技能说明与封面。
import { useCallback, useEffect, useState } from 'react'
import { FileCode2, Image, Package, RefreshCw, Save, Sparkles, Trash2, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
  SegmentedTabs as Segmented,
  StatusBadge as Badge,
  AppCardHeader,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { EntityRecord } from '@/types/chat'

type Skill = EntityRecord & {
  id: string
  name: string
  description?: string
  allowedTools?: string[]
  enabled?: boolean
  modelInvocationEnabled?: boolean
  removable?: boolean
  command?: string
}

type SkillPackage = EntityRecord & { source: string; scope?: string; installed?: boolean }
type SkillsData = EntityRecord & {
  cwd?: string
  locations?: { global?: string; project?: string }
  skills: Skill[]
  packages?: SkillPackage[]
}
type SkillsPageProps = {
  notify: Notify
  query?: string
  activeSessionId?: string
  registerPrimaryAction: (action: () => void) => () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type SkillFilter = 'all' | 'global' | 'project' | 'design' | 'code' | 'docs' | 'privileged'

const SKILL_FILTERS: SkillFilter[] = [
  'all',
  'global',
  'project',
  'design',
  'code',
  'docs',
  'privileged',
]

function skillsApiPath(path: string, activeSessionId = '') {
  if (!activeSessionId) return path
  const separator = path.includes('?') ? '&' : '?'
  return `${path}${separator}sessionId=${encodeURIComponent(activeSessionId)}`
}

function skillFilterLabel(filter: SkillFilter, t: ReturnType<typeof useI18n>['t']) {
  if (filter === 'global') return t('skills:skillsPage.global')
  if (filter === 'project') return t('skills:skillsPage.project')
  if (filter === 'design') return t('skills:skillsPage.design')
  if (filter === 'code') return t('skills:skillsPage.code')
  if (filter === 'docs') return t('skills:skillsPage.documents')
  if (filter === 'privileged') return t('skills:skillsPage.highPrivilege')
  return t('skills:skillsPage.all')
}

function skillIcon(skill: Skill) {
  const text = `${skill?.name || ''} ${skill?.description || ''}`.toLowerCase()
  if (/image|visual|design|figma|svg|图片|视觉|设计/.test(text)) return Image
  if (/doc|pdf|文档|说明/.test(text)) return FileCode2
  if (/install|package|market|安装|包/.test(text)) return Package
  if (/code|test|plugin|skill|代码|测试|插件|技能/.test(text)) return Wrench
  return Sparkles
}

function skillMatchesFilter(skill: Skill, filter: SkillFilter) {
  if (filter === 'all') return true
  if (filter === 'global') return skill.sourceInfo?.scope !== 'project'
  if (filter === 'project') return skill.sourceInfo?.scope === 'project'
  const text =
    `${skill.name} ${skill.description} ${(skill.allowedTools || []).join(' ')}`.toLowerCase()
  if (filter === 'design') return /image|visual|design|figma|svg|图片|视觉|设计/.test(text)
  if (filter === 'code') return /code|test|plugin|代码|测试|插件/.test(text)
  if (filter === 'docs') return /doc|pdf|文档|说明/.test(text)
  if (filter === 'privileged')
    return (skill.allowedTools || []).some((tool: string) =>
      ['bash', 'write', 'edit'].includes(tool),
    )
  return true
}

export function SkillsPage({
  notify,
  query = '',
  activeSessionId = '',
  registerPrimaryAction,
  requestText,
  requestConfirm,
}: SkillsPageProps) {
  const { t } = useI18n()
  const [data, setData] = useState<SkillsData | null>(null)
  const [selectedId, setSelectedId] = useState('')
  const [filter, setFilter] = useState<SkillFilter>('all')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [, setError] = useState('')

  // 加载技能列表（按活动会话作用域取项目技能），失效选中回退首项。
  const load = useCallback(async () => {
    setError('')
    try {
      const result = await apiJson<SkillsData>(skillsApiPath('/api/skills', activeSessionId))
      setData(result)
      setSelectedId((current) =>
        result.skills.some((skill) => skill.id === current) ? current : result.skills[0]?.id || '',
      )
      return result
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
      return null
    } finally {
      setLoading(false)
    }
  }, [activeSessionId])

  useEffect(() => {
    void load()
  }, [load])

  // 安装技能：询问来源 → 风险确认 → POST 安装并选中新技能；
  // 来源支持本地路径/npm 包/GitHub 仓库。
  const installSkill = useCallback(async () => {
    const source = await requestText?.({
      title: t('skills:skillsPage.importGlobalSkill'),
      message: t('skills:skillsPage.globalSkillSourceHelp'),
      inputLabel: t('skills:skillsPage.skillSource'),
      placeholder: 'E:\\path\\to\\skill, npm:@scope/package, or https://github.com/...',
      maxLength: 2_000,
      confirmLabel: t('skills:skillsPage.continue'),
    })
    if (!source?.trim()) return
    const approved = await requestConfirm?.({
      title: t('skills:skillsPage.importGlobalSkill'),
      message: t(
        'skills:skillsPage.skillsProvideInstructionsToTheAgentAndMayIncludeExecutableScriptsConfirmThatYouTrustTheSource',
      ),
      confirmLabel: t('skills:skillsPage.import'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<SkillsData & { installed?: Skill[] }>(
        skillsApiPath('/api/skills/install', activeSessionId),
        {
          method: 'POST',
          body: JSON.stringify({ source }),
        },
      )
      setData(result)
      setSelectedId(result.installed?.[0]?.id || result.skills[0]?.id || '')
      notify(t('skills:skillsPage.skillInstalledAndLoadedIntoTheAgentRuntime'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [activeSessionId, notify, requestConfirm, requestText, t])

  usePagePrimaryAction(registerPrimaryAction, installSkill)

  if (loading && !data) {
    return (
      <div className="skills-page flex min-h-[100%] flex-col gap-[12px]">
        <AppEmptyState>
          <RefreshCw className="animate-spin" size={23} />
          <h2>{t('skills:skillsPage.loadingSkills')}</h2>
          <p>{t('skills:skillsPage.scanningSkillDirectoriesAndConfiguredPackages')}</p>
        </AppEmptyState>
      </div>
    )
  }

  const skills = data?.skills || []
  const filteredSkills = skills.filter(
    (skill) =>
      skillMatchesFilter(skill, filter) &&
      `${skill.name} ${skill.description}`.toLowerCase().includes(query.toLowerCase()),
  )
  const allGlobalSkills = skills.filter((skill) => skill.sourceInfo?.scope !== 'project')
  const allProjectSkills = skills.filter((skill) => skill.sourceInfo?.scope === 'project')
  const globalSkills = filteredSkills.filter((skill) => skill.sourceInfo?.scope !== 'project')
  const projectSkills = filteredSkills.filter((skill) => skill.sourceInfo?.scope === 'project')
  const selected =
    skills.find((skill) => skill.id === selectedId) || filteredSkills[0] || skills[0] || null
  const packages = (data?.packages || [])
    .map((item) => ({
      source: item.source,
      description:
        item.scope === 'project'
          ? t('skills:skillsPage.projectScopedConfiguration')
          : t('skills:skillsPage.userScopedConfiguration'),
      status: item.installed
        ? t('skills:skillsPage.locallyReady')
        : t('skills:skillsPage.notMaterialized'),
      tone: item.installed ? 'green' : 'gray',
    }))
    .filter((item) =>
      `${item.source} ${item.description}`.toLowerCase().includes(query.toLowerCase()),
    )

  // 更新技能（启停/模型调用等）：PATCH 并就地更新列表与统计。
  const updateSkill = async (skill: Skill | null, patch: Partial<Skill>) => {
    if (!skill) return
    setBusy(true)
    setError('')
    try {
      const updated = await apiJson<Skill>(
        skillsApiPath(`/api/skills/${encodeURIComponent(skill.id)}`, activeSessionId),
        {
          method: 'PATCH',
          body: JSON.stringify(patch),
        },
      )
      setData((current) => {
        if (!current) return current
        const nextSkills = current.skills.map((item) => (item.id === updated.id ? updated : item))
        return {
          ...current,
          skills: nextSkills,
          counts: {
            ...current.counts,
            enabled: nextSkills.filter((item) => item.enabled).length,
            modelInvocable: nextSkills.filter((item) => item.enabled && item.modelInvocationEnabled)
              .length,
          },
        }
      })
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  // 保存设置并重载技能（POST reload），应用全局技能开关。
  const saveSettings = async () => {
    setBusy(true)
    setError('')
    try {
      setData(
        await apiJson<SkillsData>(skillsApiPath('/api/skills/reload', activeSessionId), {
          method: 'POST',
          body: '{}',
        }),
      )
      notify(t('skills:skillsPage.skillSettingsSavedAndReloaded'), 'success')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  // 卸载技能（确认后），仅可移除的（非内置）技能可卸载。
  const uninstallSkill = async () => {
    if (!selected?.removable) return
    const approved = await requestConfirm?.({
      title: t('skills:skillsPage.uninstallSkill'),
      message: t(
        'skills:skillsPage.thisRemovesTheSkillDirectoryInstalledByPisperItDoesNotUninstallTheOriginalNpmOrGitPackage',
      ),
      confirmLabel: t('skills:skillsPage.uninstall'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      await apiJson(
        skillsApiPath(`/api/skills/${encodeURIComponent(selected.id)}`, activeSessionId),
        { method: 'DELETE' },
      )
      const result = await load()
      setSelectedId(result?.skills[0]?.id || '')
      notify(t('skills:skillsPage.skillUninstalled'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  const renderSkillScope = (
    title: string,
    scopedSkills: Skill[],
    allScopedSkills: Skill[],
    emptyMessage: string,
    workspace = '',
  ) => (
    <section className="skill-scope [.skill-scope_+_&]:[border-top:1px_solid_var(--stroke)] [.skill-scope_+_&]:pt-[14px]">
      <div className="skill-scope-head [&_>_div]:min-w-0 [&_small]:block [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_>_span]:min-w-[24px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[12px] [&_>_span]:font-[600] [&_>_span]:text-right flex min-w-0 items-start justify-between gap-[10px]">
        <div>
          <SectionTitle title={title} />
          {workspace && <small title={workspace}>{workspace}</small>}
        </div>
        <span>{scopedSkills.length}</span>
      </div>
      {scopedSkills.length ? (
        scopedSkills.map((skill) => {
          const Icon = skillIcon(skill)
          return (
            <div
              className={`skill-row grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] border-0 [border-top:1px_solid_var(--stroke-soft)] bg-transparent p-[10px_8px] text-left hover:rounded-[var(--r-sm)] hover:bg-[var(--accent-soft)] [&.selected]:rounded-[var(--r-sm)] [&.selected]:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] grid-cols-[minmax(0,1fr)_auto] [padding-block:4px] ${selected?.id === skill.id ? 'selected' : ''}`}
              key={skill.id}
            >
              <button
                className="skill-row-main [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-[9px] border-0 bg-transparent [padding:6px_4px] text-left"
                onClick={() => setSelectedId(skill.id)}
              >
                <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
                  <Icon size={15} />
                </span>
                <span>
                  <strong>{skill.name}</strong>
                  <small className="line-clamp-2" title={skill.description}>
                    {skill.description}
                  </small>
                </span>
              </button>
              <Toggle
                value={skill.enabled}
                disabled={busy}
                ariaLabel={t('skills:skillsPage.toggleSkillName', { name: skill.name })}
                onChange={(enabled) => void updateSkill(skill, { enabled })}
              />
            </div>
          )
        })
      ) : (
        <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55] skills-empty-copy !m-[8px_2px_4px]">
          {allScopedSkills.length
            ? t('skills:skillsPage.noSkillsMatchTheCurrentFilter')
            : emptyMessage}
        </p>
      )}
    </section>
  )

  return (
    <div className="skills-page flex min-h-[100%] flex-col gap-[12px]">
      <Segmented
        options={SKILL_FILTERS.map((item) => skillFilterLabel(item, t))}
        value={skillFilterLabel(filter, t)}
        onChange={(label) =>
          setFilter(SKILL_FILTERS.find((item) => skillFilterLabel(item, t) === label) || 'all')
        }
      />
      <div className="skills-layout max-[1150px]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:grid-cols-[1fr] grid min-h-0 flex-1 grid-cols-[minmax(240px,.85fr)_minmax(290px,1.05fr)_minmax(280px,1fr)] gap-[12px]">
        <Panel className="flex flex-col gap-[16px]">
          {filter !== 'project' &&
            renderSkillScope(
              t('skills:skillsPage.globalSkills'),
              globalSkills,
              allGlobalSkills,
              t('skills:skillsPage.noGlobalSkills'),
              data?.locations?.global || '~/.pisper/agent/skills',
            )}
          {filter !== 'global' &&
            renderSkillScope(
              t('skills:skillsPage.projectSkills'),
              projectSkills,
              allProjectSkills,
              t('skills:skillsPage.noProjectSkills'),
              data?.locations?.project ||
                (data?.cwd ? `${data.cwd.replace(/[\\/]$/, '')}/.pisper/skills` : ''),
            )}
        </Panel>
        <Panel>
          <AppCardHeader>
            <SectionTitle title={t('skills:skillsPage.configuredSkillPackages')} />
            <span className="text-[var(--text-muted)] text-[12px] font-[600]">
              {t('skills:skillsPage.countSkillPackages', { count: packages.length })}
            </span>
          </AppCardHeader>
          {packages.length ? (
            packages.map((item) => (
              <div
                className="market-row [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap grid min-h-[48px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] [border-top:1px_solid_var(--stroke-soft)] [padding:6px_2px]"
                key={item.source}
              >
                <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
                  <Package size={15} />
                </span>
                <span>
                  <strong title={item.source}>{item.source}</strong>
                  <small>{item.description}</small>
                </span>
                <Badge tone={item.tone as 'green' | 'gray'}>{item.status}</Badge>
              </div>
            ))
          ) : (
            <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55] skills-empty-copy !m-[8px_2px_4px]">
              {t(
                'skills:skillsPage.noSkillPackagesAreConfiguredYetAfterYouInstallASkillItsSourceWillAppearHere',
              )}
            </p>
          )}
        </Panel>
        <div className="detail-stack flex min-w-0 flex-col gap-[12px] [.mcp-layout_>_&]:min-h-0 max-[1150px]:[.memory-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.memory-layout_>_&]:grid max-[1150px]:[.memory-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.mcp-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.mcp-layout_>_&]:grid max-[1150px]:[.mcp-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.skills-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.skills-layout_>_&]:grid max-[1150px]:[.skills-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:[.memory-layout_>_&]:[grid-column:auto] max-[650px]:[.memory-layout_>_&]:grid-cols-[1fr] max-[650px]:[.mcp-layout_>_&]:[grid-column:auto] max-[650px]:[.mcp-layout_>_&]:grid-cols-[1fr] max-[650px]:[.skills-layout_>_&]:[grid-column:auto] max-[650px]:[.skills-layout_>_&]:grid-cols-[1fr]">
          <Panel>
            <SectionTitle title={t('skills:skillsPage.selectedSkill')} />
            <h2>{selected?.name || t('skills:skillsPage.noSkillAvailable')}</h2>
            <p className="muted-copy m-[8px_0_14px] text-[var(--text-muted)] text-[12px] leading-[1.55]">
              {selected?.description || t('skills:skillsPage.addGlobalOrProjectSkill')}
            </p>
            {[
              [
                t('skills:skillsPage.triggerMode'),
                selected?.modelInvocationEnabled
                  ? t('skills:skillsPage.automaticManual')
                  : t('skills:skillsPage.manualOnly'),
              ],
              [
                t('skills:skillsPage.permissions'),
                selected?.allowedTools?.length
                  ? selected.allowedTools.join(', ')
                  : t('skills:skillsPage.usesChatToolPermissions'),
              ],
              [
                t('skills:skillsPage.scope'),
                selected?.sourceInfo?.scope === 'project'
                  ? t('skills:skillsPage.project')
                  : t('skills:skillsPage.global'),
              ],
              [t('skills:skillsPage.version'), selected?.version || 'latest'],
              [t('skills:skillsPage.source'), selected?.source || '—'],
            ].map((row) => (
              <div
                className="key-value [&:first-of-type]:mt-[7px] [&_span]:text-[var(--text-muted)] [&_button]:flex [&_button]:items-center [&_button]:gap-[4px] [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-soft)] [&_button]:text-[12px] [&_strong]:min-w-0 [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap flex min-h-[31px] items-center justify-between gap-[10px] [border-top:1px_solid_var(--stroke-soft)] text-[13px]"
                key={row[0]}
              >
                <span>{row[0]}</span>
                <strong>{row[1]}</strong>
              </div>
            ))}
            <Button
              variant={selected?.removable ? 'destructive' : 'default'}
              size="lg"
              className="w-full"
              disabled={busy}
              onClick={selected?.removable ? uninstallSkill : saveSettings}
            >
              {selected?.removable ? <Trash2 size={14} /> : <Save size={14} />}
              {selected?.removable
                ? t('skills:skillsPage.uninstallSkill')
                : t('skills:skillsPage.saveSettings')}
            </Button>
          </Panel>
          <Panel>
            <SectionTitle title={t('skills:skillsPage.triggerConditions')} />
            {(
              [
                [
                  t('skills:skillsPage.allowAutomaticModelInvocation'),
                  Boolean(selected?.modelInvocationEnabled),
                  false,
                  (checked) => void updateSkill(selected, { modelInvocationEnabled: checked }),
                ],
                [
                  t('skills:skillsPage.supportManualSkillCommand'),
                  Boolean(selected?.command),
                  true,
                ],
                [
                  t('skills:skillsPage.projectScopedSkill'),
                  selected?.sourceInfo?.scope === 'project',
                  true,
                ],
                [
                  t('skills:skillsPage.declaresRequiredTools'),
                  Boolean(selected?.allowedTools?.length),
                  true,
                ],
              ] as Array<[string, boolean, boolean, ((checked: boolean) => void)?]>
            ).map(([item, checked, disabled, onChange]) => (
              <label
                className="check-row [&_input]:[accent-color:var(--star-strong)] flex items-center gap-[7px] [border-top:1px_solid_var(--stroke-soft)] [padding:8px_2px] text-[12px]"
                key={item}
              >
                <input
                  type="checkbox"
                  checked={checked}
                  disabled={!selected || busy || disabled}
                  onChange={(event) => onChange?.(event.target.checked)}
                />
                <span>{item}</span>
              </label>
            ))}
          </Panel>
        </div>
      </div>
    </div>
  )
}
