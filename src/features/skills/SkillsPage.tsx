// 技能页（功能 › 扩展 › 技能）：查看 / 导入 / 卸载 Agent 技能，启停与触发条件，已配置的技能包。
// 外形用功能页模板（规格 A6 第 1 批）：工具行放搜索、分类筛选与主操作“导入全局 Skill”；全局 / 项目技能与
// 技能包都是列表行，点行在下方展开详情（原右侧检查器的内容）；数据流与改版前相同。
import {
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { useCallback, useEffect, useState } from 'react'
import { FileCode2, Image, Package, Plus, Save, Sparkles, Trash2, Wrench } from 'lucide-react'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import { useI18n } from '@/app/use-i18n'
import { AppSwitch as Toggle, SegmentedTabs as Segmented } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { apiJson } from '@/lib/api'
import { useShortcutLabel } from '@/lib/shortcuts'
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
  activeSessionId?: string
  // 从功能进入时由应用壳提供：标题上方的“‹ 功能”。
  onBack?: () => void
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

// 展开详情里的只读键值：左侧弱化标签，右侧值可换行（路径、来源很长）。
const KEY_VALUES =
  'grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs [&_dd]:min-w-0 [&_dd]:[overflow-wrap:anywhere] [&_dd]:text-content-soft [&_dt]:text-content-muted [&>div]:contents'

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
  activeSessionId = '',
  onBack,
  requestText,
  requestConfirm,
}: SkillsPageProps) {
  const { t } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
  const [query, setQuery] = useState('')
  const [data, setData] = useState<SkillsData | null>(null)
  // 展开详情的技能（同一时刻只展开一行，原右侧检查器的“选中技能”）。
  const [expandedId, setExpandedId] = useState('')
  const [filter, setFilter] = useState<SkillFilter>('all')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  // 加载技能列表（按活动会话作用域取项目技能）；展开的技能不存在了就收起。
  const load = useCallback(async () => {
    setError('')
    try {
      const result = await apiJson<SkillsData>(skillsApiPath('/api/skills', activeSessionId))
      setData(result)
      setExpandedId((current) =>
        result.skills.some((skill) => skill.id === current) ? current : '',
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

  // 安装技能：询问来源 → 风险确认 → POST 安装并展开新技能；
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
      setExpandedId(result.installed?.[0]?.id || '')
      notify(t('skills:skillsPage.skillInstalledAndLoadedIntoTheAgentRuntime'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [activeSessionId, notify, requestConfirm, requestText, t])

  const skills = data?.skills || []
  const normalizedQuery = query.trim().toLowerCase()
  const filteredSkills = skills.filter(
    (skill) =>
      skillMatchesFilter(skill, filter) &&
      `${skill.name} ${skill.description}`.toLowerCase().includes(normalizedQuery),
  )
  const allGlobalSkills = skills.filter((skill) => skill.sourceInfo?.scope !== 'project')
  const allProjectSkills = skills.filter((skill) => skill.sourceInfo?.scope === 'project')
  const globalSkills = filteredSkills.filter((skill) => skill.sourceInfo?.scope !== 'project')
  const projectSkills = filteredSkills.filter((skill) => skill.sourceInfo?.scope === 'project')
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
    }))
    .filter((item) => `${item.source} ${item.description}`.toLowerCase().includes(normalizedQuery))

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
  const uninstallSkill = async (skill: Skill) => {
    if (!skill.removable) return
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
      await apiJson(skillsApiPath(`/api/skills/${encodeURIComponent(skill.id)}`, activeSessionId), {
        method: 'DELETE',
      })
      await load()
      setExpandedId('')
      notify(t('skills:skillsPage.skillUninstalled'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  // 展开详情：完整说明、触发方式 / 权限 / 范围 / 版本 / 来源、触发条件，底部卸载或保存设置。
  const renderSkillDetail = (skill: Skill) => (
    <div className="skill-detail grid gap-3">
      <p className="text-sm leading-relaxed whitespace-pre-line text-content-soft">
        {skill.description || t('skills:skillsPage.addGlobalOrProjectSkill')}
      </p>
      <dl className={KEY_VALUES}>
        {(
          [
            [
              t('skills:skillsPage.triggerMode'),
              skill.modelInvocationEnabled
                ? t('skills:skillsPage.automaticManual')
                : t('skills:skillsPage.manualOnly'),
              false,
            ],
            [
              t('skills:skillsPage.permissions'),
              skill.allowedTools?.length
                ? skill.allowedTools.join(', ')
                : t('skills:skillsPage.usesChatToolPermissions'),
              Boolean(skill.allowedTools?.length),
            ],
            [
              t('skills:skillsPage.scope'),
              skill.sourceInfo?.scope === 'project'
                ? t('skills:skillsPage.project')
                : t('skills:skillsPage.global'),
              false,
            ],
            [t('skills:skillsPage.version'), skill.version || 'latest', true],
            [t('skills:skillsPage.source'), skill.source || '—', true],
          ] as Array<[string, string, boolean]>
        ).map(([label, value, mono]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd className={mono ? 'font-mono' : undefined}>{value}</dd>
          </div>
        ))}
      </dl>
      <fieldset className="grid min-w-0 gap-0.5">
        <legend className="mb-1 text-xs font-medium text-content-muted">
          {t('skills:skillsPage.triggerConditions')}
        </legend>
        {(
          [
            [
              t('skills:skillsPage.allowAutomaticModelInvocation'),
              Boolean(skill.modelInvocationEnabled),
              false,
              (checked) => void updateSkill(skill, { modelInvocationEnabled: checked }),
            ],
            [t('skills:skillsPage.supportManualSkillCommand'), Boolean(skill.command), true],
            [
              t('skills:skillsPage.projectScopedSkill'),
              skill.sourceInfo?.scope === 'project',
              true,
            ],
            [
              t('skills:skillsPage.declaresRequiredTools'),
              Boolean(skill.allowedTools?.length),
              true,
            ],
          ] as Array<[string, boolean, boolean, ((checked: boolean) => void)?]>
        ).map(([item, checked, disabled, onChange]) => (
          <label
            key={item}
            className="check-row flex min-h-8 items-center gap-2.5 text-sm text-content has-disabled:text-content-muted max-[650px]:min-h-11"
          >
            <Checkbox
              checked={checked}
              disabled={busy || disabled}
              onCheckedChange={(next) => onChange?.(next === true)}
            />
            <span>{item}</span>
          </label>
        ))}
      </fieldset>
      <div className="flex justify-end">
        {skill.removable ? (
          <Button
            variant="ghost"
            className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
            disabled={busy}
            onClick={() => void uninstallSkill(skill)}
          >
            <Trash2 />
            {t('skills:skillsPage.uninstallSkill')}
          </Button>
        ) : (
          <Button
            variant="secondary"
            className="max-[650px]:h-11"
            loading={busy}
            disabled={busy}
            onClick={saveSettings}
          >
            <Save />
            {t('skills:skillsPage.saveSettings')}
          </Button>
        )}
      </div>
    </div>
  )

  const renderSkillScope = (
    title: string,
    scopedSkills: Skill[],
    allScopedSkills: Skill[],
    emptyMessage: string,
    workspace = '',
  ) => (
    <FeatureSection
      className="skill-scope"
      title={title}
      count={scopedSkills.length}
      // 技能所在目录：等宽、可换行，路径很长时不撑破一列。
      description={workspace && <span className="font-mono">{workspace}</span>}
    >
      {scopedSkills.length ? (
        <FeatureList>
          {scopedSkills.map((skill) => {
            const Icon = skillIcon(skill)
            return (
              <FeatureRow
                key={skill.id}
                className="skill-row"
                data-skill-row={skill.id}
                icon={<Icon />}
                name={<span className="font-mono">{skill.name}</span>}
                // 行里只放摘要（最多两行），完整说明在展开的详情里。
                description={skill.description}
                actions={
                  <Toggle
                    value={skill.enabled}
                    disabled={busy}
                    ariaLabel={t('skills:skillsPage.toggleSkillName', { name: skill.name })}
                    onChange={(enabled) => void updateSkill(skill, { enabled })}
                  />
                }
                expanded={expandedId === skill.id}
                onExpandedChange={(open) => setExpandedId(open ? skill.id : '')}
                toggleLabel={t('skills:skillsPage.toggleSkillDetails', { name: skill.name })}
                detail={renderSkillDetail(skill)}
              />
            )
          })}
        </FeatureList>
      ) : (
        <p className="skills-empty-copy border-t border-stroke py-3 text-xs leading-normal text-content-muted">
          {allScopedSkills.length
            ? t('skills:skillsPage.noSkillsMatchTheCurrentFilter')
            : emptyMessage}
        </p>
      )}
    </FeatureSection>
  )

  const title = t('navigation:navigation.skills')
  return (
    <FeaturePage
      title={title}
      description={t('skills:skillsPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchPage', { page: title })}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            // 主操作（原共享页头的“导入全局 Skill”）。
            <Button
              type="button"
              className="h-9"
              data-skills-install
              disabled={busy || !data}
              onClick={() => void installSkill()}
            >
              <Plus />
              {t('navigation:pageHeader.installSkill')}
            </Button>
          }
          filters={
            <Segmented
              options={SKILL_FILTERS.map((item) => skillFilterLabel(item, t))}
              value={skillFilterLabel(filter, t)}
              onChange={(label) =>
                setFilter(
                  SKILL_FILTERS.find((item) => skillFilterLabel(item, t) === label) || 'all',
                )
              }
            />
          }
        />
      }
    >
      {loading && !data ? (
        <PageState
          kind="loading"
          title={t('skills:skillsPage.loadingSkills')}
          description={t('skills:skillsPage.scanningSkillDirectoriesAndConfiguredPackages')}
        />
      ) : !data ? (
        // 读取失败不显示成“没有技能”：写明错误并给重试（规格 1.5 诚实降级）。
        <PageState
          kind="error"
          title={t('skills:skillsPage.loadingSkills')}
          description={error}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              loading={loading}
              disabled={loading}
              onClick={() => {
                setLoading(true)
                void load()
              }}
            >
              {t('common:pageTemplate.retry')}
            </Button>
          }
        />
      ) : (
        <>
          {/* 读取成功后的操作失败（启停、保存、卸载）：写在列表上方，不再静默吞掉。 */}
          {error && (
            <p
              role="alert"
              className="mt-4 rounded-[var(--r-md)] border border-danger-border bg-danger-soft px-3 py-2 text-xs [overflow-wrap:anywhere] text-danger"
            >
              {error}
            </p>
          )}
          {filter !== 'project' &&
            renderSkillScope(
              t('skills:skillsPage.globalSkills'),
              globalSkills,
              allGlobalSkills,
              t('skills:skillsPage.noGlobalSkills'),
              data.locations?.global || '~/.pisper/agent/skills',
            )}
          {filter !== 'global' &&
            renderSkillScope(
              t('skills:skillsPage.projectSkills'),
              projectSkills,
              allProjectSkills,
              t('skills:skillsPage.noProjectSkills'),
              data.locations?.project ||
                (data.cwd ? `${data.cwd.replace(/[\\/]$/, '')}/.pisper/skills` : ''),
            )}
          <FeatureSection
            className="skill-packages"
            title={t('skills:skillsPage.configuredSkillPackages')}
            count={packages.length}
          >
            {packages.length ? (
              <FeatureList>
                {packages.map((item) => (
                  <FeatureRow
                    key={item.source}
                    className="market-row"
                    icon={<Package />}
                    name={
                      <span className="font-mono" title={item.source}>
                        {item.source}
                      </span>
                    }
                    description={item.description}
                    status={item.status}
                  />
                ))}
              </FeatureList>
            ) : (
              <InlineEmptyState
                compact
                icon={Package}
                title={t(
                  'skills:skillsPage.noSkillPackagesAreConfiguredYetAfterYouInstallASkillItsSourceWillAppearHere',
                )}
              />
            )}
          </FeatureSection>
        </>
      )}
    </FeaturePage>
  )
}
