// 桌面宠物（功能 › 设备与连接，只在桌面端；功能页模板的分区与列表行）：显示开关与透明度、
// 从 Petdex 搜索安装、选择 / 删除已安装的宠物。
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Blend, Cat, Check, Download, ExternalLink, Search, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { PageState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Switch } from '@/components/ui/switch'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { Slider } from '@/components/ui/slider'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { DesktopPetCatalogItem, DesktopPetStatus } from '@/types/update'

import { Button } from '@/components/ui/button'

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
    <>
      {error && <FeatureAlert>{error}</FeatureAlert>}
      <FeatureSection
        title={t('config:desktopPetSettings.displayTitle')}
        data-config-card="desktop-pet-settings"
      >
        {!status ? (
          <PageState kind="loading" title={t('config:desktopPetSettings.loading')} />
        ) : (
          <FeatureList>
            <FeatureRow
              icon={<Cat />}
              name={t('config:desktopPetSettings.showOnDesktop')}
              description={
                status.selectedName
                  ? t('config:desktopPetSettings.currentPet', { name: status.selectedName })
                  : t('config:desktopPetSettings.installBeforeEnabling')
              }
              status={
                <span className="inline-flex items-center gap-1 text-content-secondary">
                  {status.running && <StatusGlyph status="done" className="size-3.5" />}
                  {status.running
                    ? t('config:desktopPetSettings.running')
                    : t('config:desktopPetSettings.stopped')}
                </span>
              }
              actions={
                <Switch
                  checked={status.enabled}
                  disabled={Boolean(busy) || !status.installed.length}
                  onCheckedChange={(enabled) => void toggle(enabled)}
                  aria-label={t('config:desktopPetSettings.showOnDesktop')}
                />
              }
            />
            <FeatureRow
              icon={<Blend />}
              name={t('config:desktopPetSettings.opacity')}
              nameMeta={`${opacityPercent}%`}
              actions={
                <Slider
                  value={[opacityPercent]}
                  min={20}
                  max={100}
                  step={5}
                  disabled={busy === 'opacity'}
                  onValueChange={([value]) => setOpacityPercent(value)}
                  onValueCommit={([value]) => void changeOpacity(value / 100)}
                  aria-label={t('config:desktopPetSettings.opacity')}
                  className="w-44 max-w-full max-[650px]:w-32"
                />
              }
            />
          </FeatureList>
        )}
      </FeatureSection>

      <FeatureSection
        title={t('config:desktopPetSettings.installTitle')}
        data-config-card="desktop-pet-install"
        description={t('config:desktopPetSettings.installDescription')}
        actions={
          <Button variant="ghost" size="sm" onClick={controller.openCatalog}>
            <ExternalLink />
            {t('config:desktopPetSettings.browsePetdex')}
          </Button>
        }
      >
        {/* 目录搜索：等宽输入 + 搜索键（Enter 同样搜索），手机上两者都是 44 高。 */}
        <div className="mb-3 flex gap-2">
          <Input
            value={slug}
            onChange={(event) => setSlug(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') void searchCatalog()
            }}
            placeholder={t('config:desktopPetSettings.searchPlaceholder')}
            aria-label={t('config:desktopPetSettings.petSlug')}
            className="h-9 min-w-0 flex-1 font-mono pointer-coarse:min-h-11 max-[650px]:min-h-11"
          />
          <Button
            variant="secondary"
            className="h-9 shrink-0 pointer-coarse:h-11 max-[650px]:h-11"
            disabled={busy === 'search'}
            onClick={searchCatalog}
          >
            {busy === 'search' ? <OrbitSpinner className="text-current" /> : <Search />}
            {t('config:desktopPetSettings.search')}
          </Button>
        </div>
        {catalog.length > 0 && (
          <FeatureList>
            {catalog.map((pet) => {
              const installed = status?.installed.some((item) => item.slug === pet.slug)
              return (
                <FeatureRow
                  key={pet.slug}
                  data-pet-catalog={pet.slug}
                  icon={<Cat />}
                  name={pet.displayName}
                  nameMeta={pet.slug}
                  status={installed ? t('config:desktopPetSettings.installed') : undefined}
                  actions={
                    <Button
                      size="sm"
                      variant={installed ? 'ghost' : 'secondary'}
                      disabled={Boolean(busy) || (installed && pet.slug === status?.selectedSlug)}
                      aria-label={`${
                        installed
                          ? t('config:desktopPetSettings.use')
                          : t('config:desktopPetSettings.install')
                      }: ${pet.displayName}`}
                      onClick={() => void (installed ? select(pet.slug) : install(pet.slug))}
                    >
                      {installed ? <Check /> : <Download />}
                      {installed
                        ? t('config:desktopPetSettings.use')
                        : t('config:desktopPetSettings.install')}
                    </Button>
                  }
                />
              )
            })}
          </FeatureList>
        )}
      </FeatureSection>

      {status && status.installed.length > 0 && (
        <FeatureSection
          title={t('config:desktopPetSettings.installedTitle')}
          count={status.installed.length}
          data-config-card="desktop-pet-installed"
          description={t('config:desktopPetSettings.installedDescription')}
        >
          {/* 正在使用的宠物：行左 3px 石墨刻度 + “正在使用”文字，不只靠底色；其余行是“使用”按钮。 */}
          <FeatureList>
            {status.installed.map((pet) => {
              const selected = pet.slug === status.selectedSlug
              return (
                <FeatureRow
                  key={`${pet.source}:${pet.slug}`}
                  data-pet-installed={pet.slug}
                  data-selected={selected || undefined}
                  className={cn(
                    'relative',
                    selected &&
                      'after:absolute after:inset-y-3 after:-left-3 after:w-[3px] after:rounded-full after:bg-star',
                  )}
                  icon={<Cat />}
                  name={pet.name}
                  nameMeta={`${pet.slug} · ${pet.source === 'pisper' ? 'Pisper' : 'Petdex'}`}
                  status={
                    selected ? (
                      <span className="inline-flex items-center gap-1 text-content-secondary">
                        <StatusGlyph status="done" className="size-3.5" />
                        {t('config:desktopPetSettings.inUse')}
                      </span>
                    ) : undefined
                  }
                  actions={
                    <>
                      {!selected && (
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={Boolean(busy)}
                          aria-label={`${t('config:desktopPetSettings.use')}: ${pet.name}`}
                          onClick={() => void select(pet.slug)}
                        >
                          {t('config:desktopPetSettings.use')}
                        </Button>
                      )}
                      {pet.source === 'pisper' && (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          className="hover:bg-danger-soft hover:text-danger max-[650px]:size-11 pointer-coarse:size-11"
                          disabled={Boolean(busy)}
                          onClick={() => void remove(pet)}
                          aria-label={`${t('config:desktopPetSettings.delete')}: ${pet.name}`}
                          title={t('config:desktopPetSettings.delete')}
                        >
                          {busy === `remove:${pet.slug}` ? (
                            <OrbitSpinner className="text-current" />
                          ) : (
                            <Trash2 />
                          )}
                        </Button>
                      )}
                    </>
                  }
                />
              )
            })}
          </FeatureList>
        </FeatureSection>
      )}
    </>
  )
}
