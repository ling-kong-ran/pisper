// 桌面宠物设置：启用/透明度/宠物选择与商店浏览。
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Blend, Cat, Download, ExternalLink, Search, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsCardHeader,
  SettingsSwitch as Toggle,
} from './settings-primitives'
import { SettingsRow } from '@/components/app/SettingsRow'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { Slider } from '@/components/ui/slider'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { DesktopPetCatalogItem, DesktopPetStatus } from '@/types/update'

import { Button } from '@/components/ui/button'

// 宠物标记：小键帽里的猫形图标，与提供方字母标记同一套键帽语言。
function PetMark() {
  return (
    <span
      aria-hidden="true"
      className="grid size-7.5 place-items-center rounded-md border border-key-border bg-key text-content-muted shadow-key"
    >
      <Cat size={15} />
    </span>
  )
}

type PetController = {
  getStatus: () => Promise<DesktopPetStatus>
  setEnabled: (enabled: boolean) => Promise<DesktopPetStatus>
  setOpacity: (opacity: number) => Promise<DesktopPetStatus>
  search: (query: string) => Promise<DesktopPetCatalogItem[]>
  install: (slug: string) => Promise<DesktopPetStatus>
  select: (slug: string) => Promise<DesktopPetStatus>
  remove: (slug: string) => Promise<DesktopPetStatus>
  openCatalog: () => void
}

function announcePetChange(status: DesktopPetStatus) {
  window.dispatchEvent(new CustomEvent('pisper:desktop-pet-changed', { detail: status }))
}

type DesktopPetSettingsProps = {
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

export function DesktopPetSettings({ notify, requestConfirm }: DesktopPetSettingsProps) {
  const { t } = useI18n()
  const bridge = window.pisperDesktop
  const controller = useMemo<PetController>(() => {
    if (bridge?.getPetStatus) {
      return {
        getStatus: () => bridge.getPetStatus!(),
        setEnabled: (enabled) => bridge.setPetEnabled!(enabled),
        setOpacity: (opacity) => bridge.setPetOpacity!(opacity),
        search: (query) => bridge.searchPets!(query),
        install: (nextSlug) => bridge.installPet!(nextSlug),
        select: (nextSlug) => bridge.selectPet!(nextSlug),
        remove: (nextSlug) => bridge.removePet!(nextSlug),
        openCatalog: () => void bridge.openPetdex?.(),
      }
    }
    return {
      getStatus: () => apiJson<DesktopPetStatus>('/api/desktop-pet'),
      setEnabled: (enabled) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/enabled', {
          method: 'POST',
          body: { enabled },
        }),
      setOpacity: (opacity) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/opacity', {
          method: 'POST',
          body: { opacity },
        }),
      search: (query) =>
        apiJson<DesktopPetCatalogItem[]>(
          `/api/desktop-pet/catalog?query=${encodeURIComponent(query)}`,
        ),
      install: (nextSlug) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/install', {
          method: 'POST',
          body: { slug: nextSlug },
        }),
      select: (nextSlug) =>
        apiJson<DesktopPetStatus>('/api/desktop-pet/select', {
          method: 'POST',
          body: { slug: nextSlug },
        }),
      remove: (nextSlug) =>
        apiJson<DesktopPetStatus>(`/api/desktop-pet/${encodeURIComponent(nextSlug)}`, {
          method: 'DELETE',
        }),
      openCatalog: () => window.open('https://petdex.dev', '_blank', 'noopener,noreferrer'),
    }
  }, [bridge])
  const [status, setStatus] = useState<DesktopPetStatus | null>(null)
  const [slug, setSlug] = useState('')
  const [catalog, setCatalog] = useState<DesktopPetCatalogItem[]>([])
  const [opacityPercent, setOpacityPercent] = useState(100)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')

  const applyStatus = useCallback((next: DesktopPetStatus) => {
    setStatus(next)
    setOpacityPercent(Math.round((next.opacity ?? 1) * 100))
  }, [])

  const load = useCallback(async () => {
    try {
      setError('')
      applyStatus(await controller.getStatus())
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }, [applyStatus, controller])

  const searchCatalog = useCallback(async () => {
    setBusy('search')
    try {
      setError('')
      setCatalog(await controller.search(slug))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }, [controller, slug])

  useEffect(() => {
    void load()
    void controller
      .search('')
      .then(setCatalog)
      .catch(() => {})
  }, [controller, load])

  const toggle = async (enabled: boolean) => {
    const previous = status
    setStatus((current) =>
      current
        ? { ...current, enabled, running: Boolean(enabled && current.installed.length) }
        : current,
    )
    setBusy('toggle')
    try {
      setError('')
      const next = await controller.setEnabled(enabled)
      applyStatus(next)
      announcePetChange(next)
      notify(
        enabled
          ? t('config:desktopPetSettings.petEnabled')
          : t('config:desktopPetSettings.petDisabled'),
      )
    } catch (caught) {
      setStatus(previous)
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const changeOpacity = async (opacity: number) => {
    setBusy('opacity')
    try {
      setError('')
      const next = await controller.setOpacity(opacity)
      applyStatus(next)
      announcePetChange(next)
    } catch (caught) {
      setOpacityPercent(Math.round((status?.opacity ?? 1) * 100))
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const install = async (requestedSlug = slug) => {
    if (!requestedSlug.trim()) return
    setBusy('install')
    try {
      setError('')
      const next = await controller.install(requestedSlug)
      applyStatus(next)
      announcePetChange(next)
      notify(
        t('config:desktopPetSettings.petInstalled', {
          name: next.selectedName || requestedSlug,
        }),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const select = async (nextSlug: string) => {
    if (nextSlug === status?.selectedSlug) return
    setBusy(`select:${nextSlug}`)
    try {
      setError('')
      const next = await controller.select(nextSlug)
      applyStatus(next)
      announcePetChange(next)
      notify(t('config:desktopPetSettings.petSelected', { name: next.selectedName || nextSlug }))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const remove = async (pet: DesktopPetStatus['installed'][number]) => {
    const confirmed = await requestConfirm({
      title: t('config:desktopPetSettings.deleteTitle'),
      message: t('config:desktopPetSettings.deleteConfirm', { name: pet.name }),
      confirmLabel: t('config:desktopPetSettings.delete'),
      tone: 'danger',
    })
    if (!confirmed) return
    setBusy(`remove:${pet.slug}`)
    try {
      setError('')
      const next = await controller.remove(pet.slug)
      applyStatus(next)
      announcePetChange(next)
      notify(t('config:desktopPetSettings.petRemoved', { name: pet.name }))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Panel className="p-0 py-0" data-config-card="desktop-pet-settings">
        <SettingsCardHeader
          className="border-b border-stroke-soft px-4 py-3.5"
          active={Boolean(status?.enabled)}
          icon={<Cat size={18} />}
          title={t('config:desktopPetSettings.title')}
          description={t('config:desktopPetSettings.description')}
        />
        {!status ? (
          <p role="status" className="flex items-center gap-2 px-4 py-3 text-sm text-content-muted">
            <OrbitSpinner />
            {t('config:desktopPetSettings.loading')}
          </p>
        ) : (
          <div className="divide-y divide-stroke-soft px-4">
            <SettingsRow
              title={
                <span className="flex flex-wrap items-center gap-2">
                  {t('config:desktopPetSettings.showOnDesktop')}
                  <Badge tone={status.running ? 'green' : 'gray'}>
                    {status.running
                      ? t('config:desktopPetSettings.running')
                      : t('config:desktopPetSettings.stopped')}
                  </Badge>
                </span>
              }
              description={
                status.selectedName
                  ? t('config:desktopPetSettings.currentPet', { name: status.selectedName })
                  : t('config:desktopPetSettings.installBeforeEnabling')
              }
              control={
                <Toggle
                  value={status.enabled}
                  disabled={Boolean(busy) || !status.installed.length}
                  onChange={toggle}
                  ariaLabel={t('config:desktopPetSettings.showOnDesktop')}
                />
              }
            />
            <SettingsRow
              title={
                <span className="flex items-center gap-2">
                  <Blend size={14} aria-hidden="true" className="text-content-muted" />
                  {t('config:desktopPetSettings.opacity')}
                  <span className="text-xs font-normal text-content-muted tabular-nums">
                    {opacityPercent}%
                  </span>
                </span>
              }
              control={
                <Slider
                  value={[opacityPercent]}
                  min={20}
                  max={100}
                  step={5}
                  disabled={busy === 'opacity'}
                  onValueChange={([value]) => setOpacityPercent(value)}
                  onValueCommit={([value]) => void changeOpacity(value / 100)}
                  aria-label={t('config:desktopPetSettings.opacity')}
                  className="w-48 max-w-full"
                />
              }
            />
          </div>
        )}
        {error && (
          <p
            role="alert"
            className="mx-4 mb-3 rounded-sm bg-danger-soft px-2.5 py-2 text-sm text-danger"
          >
            {error}
          </p>
        )}
      </Panel>

      <Panel className="p-4 py-4" data-config-card="desktop-pet-install">
        <SettingsCardHeader
          icon={<Download size={18} />}
          title={t('config:desktopPetSettings.installTitle')}
          description={t('config:desktopPetSettings.installDescription')}
        />
        {/* 搜索框：等宽输入 + 主操作键，整体是一个 34px 字段（手机 44px）。 */}
        <div className="mt-4 flex gap-2">
          <Input
            value={slug}
            onChange={(event) => setSlug(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void searchCatalog()
            }}
            placeholder={t('config:desktopPetSettings.searchPlaceholder')}
            aria-label={t('config:desktopPetSettings.petSlug')}
            className="min-w-0 flex-1 font-mono pointer-coarse:min-h-11 max-[650px]:min-h-11"
          />
          <Button
            className="h-8.5 shrink-0 pointer-coarse:h-11 max-[650px]:h-11"
            disabled={busy === 'search'}
            onClick={searchCatalog}
          >
            {busy === 'search' ? <OrbitSpinner className="text-current" /> : <Search size={14} />}
            {t('config:desktopPetSettings.search')}
          </Button>
        </div>
        {catalog.length > 0 && (
          <div className="mt-4 grid grid-cols-2 gap-2 max-[650px]:grid-cols-1">
            {catalog.map((pet) => {
              const installed = status?.installed.some((item) => item.slug === pet.slug)
              return (
                <button
                  type="button"
                  className="grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-md border border-stroke bg-panel px-3 py-2.5 text-left transition-colors hover:bg-surface-subtle disabled:opacity-50"
                  disabled={Boolean(busy)}
                  onClick={() => (installed ? select(pet.slug) : install(pet.slug))}
                  key={pet.slug}
                >
                  <PetMark />
                  <span className="flex min-w-0 flex-col">
                    <span className="truncate text-sm font-medium text-content">
                      {pet.displayName}
                    </span>
                    <span className="truncate font-mono text-xs text-content-muted">
                      {pet.slug}
                    </span>
                  </span>
                  <Badge tone={installed ? 'green' : 'blue'}>
                    {installed
                      ? t('config:desktopPetSettings.installed')
                      : t('config:desktopPetSettings.install')}
                  </Badge>
                </button>
              )
            })}
          </div>
        )}
        <div className="mt-4 flex gap-2 max-[650px]:flex-wrap">
          <Button variant="secondary" className="max-[650px]:h-11" onClick={controller.openCatalog}>
            <ExternalLink size={14} />
            {t('config:desktopPetSettings.browsePetdex')}
          </Button>
        </div>
      </Panel>

      {status && status.installed.length > 0 && (
        <Panel className="p-4 py-4" data-config-card="desktop-pet-installed">
          <SettingsCardHeader
            icon={<Cat size={18} />}
            title={t('config:desktopPetSettings.installedTitle')}
            description={t('config:desktopPetSettings.installedDescription')}
          />
          {/* 已安装宠物：选中项用 3px 强调刻线 + 次级底色标记，而不是只靠浅色底。 */}
          <div className="mt-4 grid grid-cols-2 gap-2 max-[650px]:grid-cols-1" role="radiogroup">
            {status.installed.map((pet) => {
              const selected = pet.slug === status.selectedSlug
              return (
                <div
                  className={cn(
                    'relative grid grid-cols-[minmax(0,1fr)_auto] items-center gap-1 overflow-hidden rounded-md border border-stroke pr-1.5 transition-colors',
                    selected
                      ? 'bg-surface-muted before:absolute before:inset-y-2.5 before:left-0 before:w-[3px] before:rounded-r-[3px] before:bg-star'
                      : 'bg-panel hover:bg-surface-subtle',
                  )}
                  key={`${pet.source}:${pet.slug}`}
                >
                  <button
                    type="button"
                    className={cn(
                      'grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-sm px-3 py-2.5 text-left disabled:pointer-events-none disabled:opacity-50',
                      pet.source !== 'pisper' && 'col-span-2',
                    )}
                    role="radio"
                    aria-checked={selected}
                    disabled={Boolean(busy)}
                    onClick={() => select(pet.slug)}
                  >
                    <PetMark />
                    <span className="flex min-w-0 flex-col">
                      <span
                        className={cn(
                          'truncate text-sm text-content',
                          selected ? 'font-medium' : 'font-normal',
                        )}
                      >
                        {pet.name}
                      </span>
                      <span className="truncate font-mono text-xs text-content-muted">
                        {pet.slug}
                      </span>
                    </span>
                    <Badge tone={pet.source === 'pisper' ? 'blue' : 'gray'}>
                      {pet.source === 'pisper' ? 'Pisper' : 'Petdex'}
                    </Badge>
                  </button>
                  {pet.source === 'pisper' && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      className="hover:bg-danger-soft hover:text-danger pointer-coarse:size-11"
                      disabled={Boolean(busy)}
                      onClick={() => void remove(pet)}
                      aria-label={t('config:desktopPetSettings.delete')}
                      title={t('config:desktopPetSettings.delete')}
                    >
                      {busy === `remove:${pet.slug}` ? (
                        <OrbitSpinner className="text-current" />
                      ) : (
                        <Trash2 size={14} />
                      )}
                    </Button>
                  )}
                </div>
              )
            })}
          </div>
        </Panel>
      )}
    </div>
  )
}
