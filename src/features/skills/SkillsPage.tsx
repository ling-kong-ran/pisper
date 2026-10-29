import { PageFrame } from '@/components/app/PageFrame'
import { ListDetailLayout } from '@/components/app/ListDetailLayout'
import { InlineEmptyState } from '@/components/app/PageState'
// 技能页：查看/安装/卸载 Agent 技能包，展示技能说明与封面。
import { useCallback, useEffect, useState } from 'react'
import { FileCode2, Image, Package, Save, Sparkles, Trash2, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  AppSwitch as Toggle,
  SegmentedTabs as Segmented,
  StatusBadge as Badge,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
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

// 卡片：1px 描边 + r-md，无投影；卡片头位于次级表面，13/500 标题 + 等宽计数。
const CARD = 'min-w-0 overflow-hidden rounded-md border border-stroke bg-panel'
const CARD_HEAD =
  'flex min-h-11 min-w-0 items-center justify-between gap-3 border-b border-stroke-soft bg-surface-subtle px-3.5 py-1.5'
const ICON_KEY =
  'grid size-7 shrink-0 place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key [&_svg]:size-3.5'
const SELECTED_TICK = 'absolute inset-y-2.5 left-0 w-[3px] rounded-r-[3px] bg-star'
// 只读键值行：左侧弱化标签，右侧值单行省略。
const KEY_VALUE =
  'key-value flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft text-sm [&>dt]:shrink-0 [&>dt]:text-content-muted [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content'

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
  const [detailOpen, setDetailOpen] = useState(false)

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
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <AppEmptyState>
          <OrbitSpinner className="size-5" />
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
    <section className="skill-scope min-w-0 [.skill-scope+&]:mt-3 [.skill-scope+&]:border-t [.skill-scope+&]:border-stroke-soft [.skill-scope+&]:pt-2">
      {/* 分组标题 12/500 弱化文字 + 等宽目录与计数。 */}
      <div className="skill-scope-head flex min-w-0 items-end justify-between gap-2.5 px-1 pt-1.5 pb-1.5">
        <div className="min-w-0">
          <h2 className="text-xs font-medium tracking-[.02em] text-content-muted">{title}</h2>
          {workspace && (
            <small
              className="block truncate font-mono text-xs text-content-muted"
              title={workspace}
            >
              {workspace}
            </small>
          )}
        </div>
        <span className="shrink-0 text-xs text-content-muted tabular-nums">
          {scopedSkills.length}
        </span>
      </div>
      {scopedSkills.length ? (
        <ul className="flex flex-col gap-0.5">
          {scopedSkills.map((skill) => {
            const Icon = skillIcon(skill)
            const active = selected?.id === skill.id
            return (
              <li
                className={cn(
                  'skill-row relative grid w-full min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-md pr-2.5 transition-colors duration-(--d-fast) hover:bg-surface-subtle',
                  active && 'selected bg-surface-muted hover:bg-surface-muted',
                )}
                key={skill.id}
              >
                {active && <i aria-hidden="true" className={SELECTED_TICK} />}
                <button
                  type="button"
                  aria-current={active || undefined}
                  className="skill-row-main grid min-h-[56px] min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-3 py-2 pl-3 text-left focus-visible:-outline-offset-2 [&_small]:text-xs [&_small]:leading-[1.45] [&_small]:text-content-muted"
                  onClick={() => {
                    setDetailOpen(true)
                    setSelectedId(skill.id)
                  }}
                >
                  <span className={ICON_KEY}>
                    <Icon />
                  </span>
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <strong className="truncate font-mono text-[13px] font-medium text-content">
                      {skill.name}
                    </strong>
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
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="skills-empty-copy px-1 py-2 text-xs leading-normal text-content-muted">
          {allScopedSkills.length
            ? t('skills:skillsPage.noSkillsMatchTheCurrentFilter')
            : emptyMessage}
        </p>
      )}
    </section>
  )

  return (
    <PageFrame variant="list-detail">
      <div className="flex h-full min-h-0 flex-col">
        <div className="flex min-h-0 flex-1 flex-col">
          {/* 筛选条位于列表/详情之上，细线分隔；分段控件横向可滚动，不撑破窄屏。 */}
          <div className="shrink-0 border-b border-stroke-soft px-3 py-2.5 @min-[760px]/page:px-4">
            <Segmented
              options={SKILL_FILTERS.map((item) => skillFilterLabel(item, t))}
              value={skillFilterLabel(filter, t)}
              onChange={(label) =>
                setFilter(
                  SKILL_FILTERS.find((item) => skillFilterLabel(item, t) === label) || 'all',
                )
              }
            />
          </div>
          <ListDetailLayout
            selected={detailOpen}
            onBack={() => setDetailOpen(false)}
            backLabel={t('common:pageTemplate.backToList')}
            list={
              <div className="min-w-0">
                <Button
                  variant="secondary"
                  className="mb-3 h-11 w-full @min-[760px]/list-detail:hidden"
                  onClick={() => setDetailOpen(true)}
                >
                  <Package />
                  {t('skills:skillsPage.configuredSkillPackages')}
                </Button>
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
              </div>
            }
            detail={
              <section className={CARD}>
                <header className={CARD_HEAD}>
                  <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-medium text-content">
                    {t('skills:skillsPage.configuredSkillPackages')}
                    <span className="font-normal text-content-muted tabular-nums">
                      {packages.length}
                    </span>
                  </h2>
                </header>
                {packages.length ? (
                  <ul className="divide-y divide-stroke-soft">
                    {packages.map((item) => (
                      <li
                        className="market-row grid min-h-[52px] min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 py-2 pr-3.5 pl-3.5"
                        key={item.source}
                      >
                        <span className={ICON_KEY}>
                          <Package />
                        </span>
                        <span className="flex min-w-0 flex-col gap-0.5">
                          <strong
                            className="truncate font-mono text-xs font-medium text-content"
                            title={item.source}
                          >
                            {item.source}
                          </strong>
                          <small className="truncate text-xs text-content-muted">
                            {item.description}
                          </small>
                        </span>
                        <Badge tone={item.tone as 'green' | 'gray'}>{item.status}</Badge>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <InlineEmptyState
                    compact
                    icon={Package}
                    title={t(
                      'skills:skillsPage.noSkillPackagesAreConfiguredYetAfterYouInstallASkillItsSourceWillAppearHere',
                    )}
                  />
                )}
              </section>
            }
            inspector={
              <div className="flex min-w-0 flex-col gap-4">
                <section className={CARD}>
                  <header className="flex min-w-0 flex-col gap-1 px-3.5 pt-3 pb-2.5">
                    <small className="text-xs font-medium tracking-[.02em] text-content-muted">
                      {t('skills:skillsPage.selectedSkill')}
                    </small>
                    <h2
                      className={cn(
                        'truncate text-[15px] leading-6 font-medium text-content',
                        selected && 'font-mono',
                      )}
                    >
                      {selected?.name || t('skills:skillsPage.noSkillAvailable')}
                    </h2>
                    <p className="text-xs leading-normal text-content-secondary">
                      {selected?.description || t('skills:skillsPage.addGlobalOrProjectSkill')}
                    </p>
                  </header>
                  <dl className="px-3.5">
                    {(
                      [
                        [
                          t('skills:skillsPage.triggerMode'),
                          selected?.modelInvocationEnabled
                            ? t('skills:skillsPage.automaticManual')
                            : t('skills:skillsPage.manualOnly'),
                          false,
                        ],
                        [
                          t('skills:skillsPage.permissions'),
                          selected?.allowedTools?.length
                            ? selected.allowedTools.join(', ')
                            : t('skills:skillsPage.usesChatToolPermissions'),
                          Boolean(selected?.allowedTools?.length),
                        ],
                        [
                          t('skills:skillsPage.scope'),
                          selected?.sourceInfo?.scope === 'project'
                            ? t('skills:skillsPage.project')
                            : t('skills:skillsPage.global'),
                          false,
                        ],
                        [t('skills:skillsPage.version'), selected?.version || 'latest', true],
                        [t('skills:skillsPage.source'), selected?.source || '—', true],
                      ] as Array<[string, string, boolean]>
                    ).map(([label, value, mono]) => (
                      <div className={KEY_VALUE} key={label}>
                        <dt>{label}</dt>
                        <dd className={mono ? 'font-mono text-xs' : undefined} title={value}>
                          {value}
                        </dd>
                      </div>
                    ))}
                  </dl>
                  <footer className="flex justify-end border-t border-stroke-soft bg-surface-subtle px-3.5 py-2.5">
                    {selected?.removable ? (
                      <Button
                        variant="ghost"
                        className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                        disabled={busy}
                        onClick={uninstallSkill}
                      >
                        <Trash2 />
                        {t('skills:skillsPage.uninstallSkill')}
                      </Button>
                    ) : (
                      <Button
                        className="max-[650px]:h-11"
                        loading={busy}
                        disabled={busy}
                        onClick={saveSettings}
                      >
                        <Save />
                        {t('skills:skillsPage.saveSettings')}
                      </Button>
                    )}
                  </footer>
                </section>
                <section className={CARD}>
                  <header className={CARD_HEAD}>
                    <h2 className="text-sm font-medium text-content">
                      {t('skills:skillsPage.triggerConditions')}
                    </h2>
                  </header>
                  <ul className="divide-y divide-stroke-soft">
                    {(
                      [
                        [
                          t('skills:skillsPage.allowAutomaticModelInvocation'),
                          Boolean(selected?.modelInvocationEnabled),
                          false,
                          (checked) =>
                            void updateSkill(selected, { modelInvocationEnabled: checked }),
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
                      <li key={item}>
                        <label className="check-row flex min-h-[40px] items-center gap-2.5 px-3.5 text-sm text-content has-disabled:text-content-muted max-[650px]:min-h-11">
                          <Checkbox
                            checked={checked}
                            disabled={!selected || busy || disabled}
                            onCheckedChange={(next) => onChange?.(next === true)}
                          />
                          <span>{item}</span>
                        </label>
                      </li>
                    ))}
                  </ul>
                </section>
              </div>
            }
          />
        </div>
      </div>
    </PageFrame>
  )
}
