import { useCallback, useEffect, useState } from 'react'
import { FileCode2, Image, Package, RefreshCw, Save, Sparkles, Trash2, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Badge, Panel, SectionTitle, Segmented, Toggle } from '@/components/ui'
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
type SkillsData = EntityRecord & { skills: Skill[]; packages?: SkillPackage[] }
type SkillsPageProps = {
  notify: Notify
  query?: string
  registerPrimaryAction: (action: () => void) => () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type SkillFilter = 'all' | 'installed' | 'design' | 'code' | 'docs' | 'privileged'

const SKILL_FILTERS: SkillFilter[] = ['all', 'installed', 'design', 'code', 'docs', 'privileged']

function skillFilterLabel(filter: SkillFilter, t: ReturnType<typeof useI18n>['t']) {
  if (filter === 'installed') return t('skills:skillsPage.installed')
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
  if (filter === 'all' || filter === 'installed') return true
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

  const load = useCallback(async () => {
    setError('')
    try {
      const result = await apiJson<SkillsData>('/api/skills')
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
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const installSkill = useCallback(async () => {
    const source = await requestText?.({
      title: t('skills:skillsPage.installSkill'),
      message: t(
        'skills:skillsPage.enterALocalSkillDirectorySKILLMdFileNpmPackageOrGitSourceVesperImportsOnlyItsSkillResources',
      ),
      inputLabel: t('skills:skillsPage.skillSource'),
      placeholder: 'npm:@scope/vesper-skills or ./path/to/skill',
      maxLength: 2_000,
      confirmLabel: t('skills:skillsPage.continue'),
    })
    if (!source?.trim()) return
    const approved = await requestConfirm?.({
      title: t('skills:skillsPage.installSkill'),
      message: t(
        'skills:skillsPage.skillsProvideInstructionsToTheAgentAndMayIncludeExecutableScriptsConfirmThatYouTrustTheSource',
      ),
      confirmLabel: t('skills:skillsPage.install'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<SkillsData & { installed?: Skill[] }>('/api/skills/install', {
        method: 'POST',
        body: JSON.stringify({ source }),
      })
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
  }, [notify, requestConfirm, requestText, t])

  usePagePrimaryAction(registerPrimaryAction, installSkill)

  if (loading && !data) {
    return (
      <div className="skills-page">
        <Panel className="empty-state">
          <RefreshCw className="spin" size={23} />
          <h2>{t('skills:skillsPage.loadingSkills')}</h2>
          <p>{t('skills:skillsPage.scanningSkillDirectoriesAndConfiguredPackages')}</p>
        </Panel>
      </div>
    )
  }

  const skills = data?.skills || []
  const filteredSkills = skills.filter(
    (skill) =>
      skillMatchesFilter(skill, filter) &&
      `${skill.name} ${skill.description}`.toLowerCase().includes(query.toLowerCase()),
  )
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

  const updateSkill = async (skill: Skill | null, patch: Partial<Skill>) => {
    if (!skill) return
    setBusy(true)
    setError('')
    try {
      const updated = await apiJson<Skill>(`/api/skills/${encodeURIComponent(skill.id)}`, {
        method: 'PATCH',
        body: JSON.stringify(patch),
      })
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

  const saveSettings = async () => {
    setBusy(true)
    setError('')
    try {
      setData(await apiJson<SkillsData>('/api/skills/reload', { method: 'POST', body: '{}' }))
      notify(t('skills:skillsPage.skillSettingsSavedAndReloaded'), 'success')
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  const uninstallSkill = async () => {
    if (!selected?.removable) return
    const approved = await requestConfirm?.({
      title: t('skills:skillsPage.uninstallSkill'),
      message: t(
        'skills:skillsPage.thisRemovesTheSkillDirectoryInstalledByVesperItDoesNotUninstallTheOriginalNpmOrGitPackage',
      ),
      confirmLabel: t('skills:skillsPage.uninstall'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      await apiJson(`/api/skills/${encodeURIComponent(selected.id)}`, { method: 'DELETE' })
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

  return (
    <div className="skills-page">
      <Segmented
        options={SKILL_FILTERS.map((item) => skillFilterLabel(item, t))}
        value={skillFilterLabel(filter, t)}
        onChange={(label) =>
          setFilter(SKILL_FILTERS.find((item) => skillFilterLabel(item, t) === label) || 'all')
        }
      />
      <div className="skills-layout">
        <Panel>
          <SectionTitle title={t('skills:skillsPage.installedSkills')} />
          {filteredSkills.length ? (
            filteredSkills.map((skill) => {
              const Icon = skillIcon(skill)
              return (
                <button
                  className={`skill-row ${selected?.id === skill.id ? 'selected' : ''}`}
                  onClick={() => setSelectedId(skill.id)}
                  key={skill.id}
                >
                  <span className="list-icon">
                    <Icon size={15} />
                  </span>
                  <span>
                    <strong>{skill.name}</strong>
                    <small>{skill.description}</small>
                  </span>
                  <Toggle
                    value={skill.enabled}
                    disabled={busy}
                    ariaLabel={t('skills:skillsPage.toggleSkillName', { name: skill.name })}
                    onChange={(enabled) => void updateSkill(skill, { enabled })}
                  />
                </button>
              )
            })
          ) : (
            <p className="muted-copy skills-empty-copy">
              {skills.length
                ? t('skills:skillsPage.noSkillsMatchTheCurrentFilter')
                : t(
                    'skills:skillsPage.noSkillsInstalledYetUseInstallSkillInTheUpperRightToAddALocalDirectoryNpmPackageOrGitSource',
                  )}
            </p>
          )}
        </Panel>
        <Panel>
          <div className="card-head">
            <SectionTitle title={t('skills:skillsPage.configuredSkillPackages')} />
            <span className="skills-package-count">
              {t('skills:skillsPage.countSkillPackages', { count: packages.length })}
            </span>
          </div>
          {packages.length ? (
            packages.map((item) => (
              <div className="market-row" key={item.source}>
                <span className="list-icon">
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
            <p className="muted-copy skills-empty-copy">
              {t(
                'skills:skillsPage.noSkillPackagesAreConfiguredYetAfterYouInstallASkillItsSourceWillAppearHere',
              )}
            </p>
          )}
        </Panel>
        <div className="detail-stack">
          <Panel>
            <SectionTitle title={t('skills:skillsPage.selectedSkill')} />
            <h2>{selected?.name || t('skills:skillsPage.noSkillInstalled')}</h2>
            <p className="muted-copy">
              {selected?.description ||
                t(
                  'skills:skillsPage.useTheButtonInTheUpperRightToInstallAnAgentSkillsCompatibleSkill',
                )}
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
              [t('skills:skillsPage.version'), selected?.version || 'latest'],
              [t('skills:skillsPage.source'), selected?.source || '—'],
            ].map((row) => (
              <div className="key-value" key={row[0]}>
                <span>{row[0]}</span>
                <strong>{row[1]}</strong>
              </div>
            ))}
            <button
              className={`button ${selected?.removable ? 'danger' : 'primary'} wide`}
              disabled={busy}
              onClick={selected?.removable ? uninstallSkill : saveSettings}
            >
              {selected?.removable ? <Trash2 size={14} /> : <Save size={14} />}
              {selected?.removable
                ? t('skills:skillsPage.uninstallSkill')
                : t('skills:skillsPage.saveSettings')}
            </button>
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
              <label className="check-row" key={item}>
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
